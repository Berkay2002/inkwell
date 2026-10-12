//! The batch commands. The webview names files by token only: the files to process are open tokens,
//! a folder result is a `pick_folder` token, an exported action goes to a `pick_save` token.
//!
//! `batch_run` takes the Action itself (the editor's current state, passwords of a Protect step
//! included) so that an unsaved action can be tried; the passwords live in that call only.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;

use parking_lot::Mutex;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, Window};

use super::action::{Action, Issue, StepEntry};
use super::run::{self, BatchError, Event, Preview, Report};
use super::steps::{Answer, Env};
use super::store::{self, Store, StoreError};
use crate::engine::Engine;
use crate::paths::{FileRef, Paths};
use crate::tools::paths::ToolPaths;
use crate::tools::{Jobs, ToolError};

type Eng<'a> = State<'a, Arc<Engine>>;
type Reg<'a> = State<'a, Paths>;
type JobState<'a> = State<'a, Arc<Jobs>>;
type Tess<'a> = State<'a, Arc<ToolPaths>>;
type PromptState<'a> = State<'a, Arc<Prompts>>;

/// The prompts a run is waiting on: `(job, request)` to where the answer goes. An entry is removed when it is
/// answered or when its run ends, so an answer that comes late (or twice, or for another run) finds nothing
/// and is dropped.
#[derive(Default)]
pub struct Prompts {
    waiting: Mutex<HashMap<(u64, u64), mpsc::Sender<Answer>>>,
}

impl Prompts {
    fn register(&self, job: u64, request: u64) -> mpsc::Receiver<Answer> {
        let (tx, rx) = mpsc::channel();
        self.waiting.lock().insert((job, request), tx);
        rx
    }

    /// Hands `answer` to the prompt that is waiting under `(job, request)`; false when none is.
    pub fn answer(&self, job: u64, request: u64, answer: Answer) -> bool {
        let tx = self.waiting.lock().remove(&(job, request));
        tx.is_some_and(|tx| tx.send(answer).is_ok())
    }

    fn end_job(&self, job: u64) {
        self.waiting.lock().retain(|(j, _), _| *j != job);
    }
}

/// What the person answers to a prompt of a run.
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PromptAnswer {
    /// Run the step with these options on this file.
    Continue { step: StepEntry },
    /// Leave the step out for this file.
    Skip,
    /// Stop the run.
    Stop,
}

impl From<StoreError> for ToolError {
    fn from(e: StoreError) -> Self {
        ToolError(e.to_string())
    }
}

impl From<BatchError> for ToolError {
    fn from(e: BatchError) -> Self {
        ToolError(e.to_string())
    }
}

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R, ToolError> + Send + 'static) -> Result<R, ToolError> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or_else(|_| Err(ToolError("the batch tool crashed".into())))
}

fn store_of(app: &AppHandle) -> Result<Store, ToolError> {
    let (dir, _) = crate::recents::data_dir(app).map_err(ToolError)?;
    Ok(Store::new(dir.join("batch-actions")))
}

/// What steps need to know: Tesseract, the date, and which files a tab has open.
fn env_of(app: &AppHandle, eng: &Arc<Engine>, tesseract: Option<PathBuf>) -> Env {
    let eng = eng.clone();
    let mut env = Env::new(tesseract, Arc::new(move |p| eng.check_not_open(&p.to_string_lossy(), None).is_err()));
    env.journal = journal_of(app);
    let app = app.clone();
    env.saved = Some(Arc::new(move |p| crate::search::catalog::app::forget_saved(&app, &p.to_string_lossy())));
    env
}

fn journal_of(app: &AppHandle) -> Option<PathBuf> {
    crate::recents::data_dir(app).ok().map(|(d, _)| d.join("batch-actions").join("run-dirs.txt"))
}

/// At startup: removes the staging folders and empty claimed names that a run which died left behind.
pub fn sweep_at_startup(app: &AppHandle) {
    if let Some(journal) = journal_of(app) {
        std::thread::spawn(move || {
            let _ = run::sweep(&journal);
        });
    }
}

/// The built-in actions, then the saved ones.
#[tauri::command]
pub async fn batch_list(app: AppHandle) -> Result<Vec<Action>, ToolError> {
    let store = store_of(&app)?;
    blocking(move || Ok(store.list())).await
}

/// Stores the action (a new id when it has none) and returns it as stored.
#[tauri::command]
pub async fn batch_save(app: AppHandle, action: Action) -> Result<Action, ToolError> {
    let store = store_of(&app)?;
    blocking(move || Ok(store.save(action)?)).await
}

#[tauri::command]
pub async fn batch_delete(app: AppHandle, id: String) -> Result<(), ToolError> {
    let store = store_of(&app)?;
    blocking(move || Ok(store.delete(&id)?)).await
}

/// Reads an action file (`file`: an open token) into the store under a new id.
#[tauri::command]
pub async fn batch_import(window: Window, app: AppHandle, reg: Reg<'_>, file: String) -> Result<Action, ToolError> {
    let path = reg.open(window.label(), &file).map_err(|e| ToolError(e.to_string()))?;
    let store = store_of(&app)?;
    blocking(move || {
        let len = std::fs::metadata(&path).map_err(|e| ToolError(e.to_string()))?.len();
        if len > store::MAX_FILE {
            return Err(ToolError("That file is too big to be an action.".into()));
        }
        let bytes = std::fs::read(&path).map_err(|e| ToolError(e.to_string()))?;
        Ok(store.import(&bytes)?)
    })
    .await
}

/// Writes the action to the file behind `save_to` (a `pick_save` token). Passwords are never written.
#[tauri::command]
pub async fn batch_export(window: Window, reg: Reg<'_>, action: Action, save_to: String) -> Result<(), ToolError> {
    let to = PathBuf::from(reg.take_save(window.label(), &save_to).map_err(|e| ToolError(e.to_string()))?);
    blocking(move || {
        let bytes = store::to_file(&action)?;
        store::write_file_atomic(&to, &bytes).map_err(|e| ToolError(e.to_string()))?;
        window.state::<Paths>().learn(&to.to_string_lossy());
        Ok(())
    })
    .await
}

/// Everything that keeps the action from running here; empty when it can.
#[tauri::command]
pub async fn batch_validate(app: AppHandle, eng: Eng<'_>, tess: Tess<'_>, action: Action) -> Result<Vec<Issue>, ToolError> {
    let env = env_of(&app, eng.inner(), tess.tesseract());
    blocking(move || Ok(action.validate(&env))).await
}

/// The most PDFs a folder gives a batch (a run is one file at a time; a longer list is a job for a script).
const FOLDER_MAX: usize = 500;

/// What a folder holds for a batch: `folder` is an open token of a directory (`pick_search_folder`), the
/// PDFs in it come back as open tokens, in order, at most [`FOLDER_MAX`]. `capped` says the list was cut.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderFiles {
    pub files: Vec<FileRef>,
    pub capped: bool,
}

#[tauri::command]
pub async fn batch_folder_files(window: Window, reg: Reg<'_>, folder: String, recursive: bool) -> Result<FolderFiles, ToolError> {
    let dir = PathBuf::from(reg.open(window.label(), &folder).map_err(|e| ToolError(e.to_string()))?);
    blocking(move || {
        if !dir.is_dir() {
            return Err(ToolError(format!("{} is not a folder.", dir.display())));
        }
        let mut l = crate::search::files::list_pdfs(&dir, recursive, &std::sync::atomic::AtomicBool::new(false));
        let capped = l.capped || l.files.len() > FOLDER_MAX;
        l.files.truncate(FOLDER_MAX);
        let reg = window.state::<Paths>();
        let files = l.files.into_iter().map(|p| reg.issue_open(window.label(), p.to_string_lossy().into_owned())).collect();
        Ok(FolderFiles { files, capped })
    })
    .await
}

/// A dry run over the files (open tokens): per file, whether it would run or be skipped and why, and the
/// names it would write. Writes nothing, spends no token and starts no job. A folder result is chosen
/// when the run starts, so the preview does not take one.
#[tauri::command]
pub async fn batch_preview(
    window: Window,
    app: AppHandle,
    eng: Eng<'_>,
    reg: Reg<'_>,
    tess: Tess<'_>,
    action: Action,
    files: Vec<String>,
) -> Result<Preview, ToolError> {
    let label = window.label().to_string();
    let paths: Vec<PathBuf> =
        files.iter().map(|t| reg.open(&label, t).map(PathBuf::from)).collect::<Result<_, _>>().map_err(|e| ToolError(e.to_string()))?;
    let env = env_of(&app, eng.inner(), tess.tesseract());
    blocking(move || Ok(run::preview(&action, &paths, &env)?)).await
}

/// Runs `action` over the files (open tokens). `folder`: a `pick_folder` token, needed when the output
/// rule is a folder. `job`: an id the UI picks, for `batch_cancel`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn batch_run(
    window: Window,
    app: AppHandle,
    eng: Eng<'_>,
    reg: Reg<'_>,
    jobs: JobState<'_>,
    tess: Tess<'_>,
    prompts: PromptState<'_>,
    job: u64,
    action: Action,
    files: Vec<String>,
    folder: Option<String>,
    on_event: Channel<Event>,
) -> Result<Report, ToolError> {
    let label = window.label().to_string();
    let paths: Vec<PathBuf> = files
        .iter()
        .map(|t| reg.open(&label, t).map(PathBuf::from))
        .collect::<Result<_, _>>()
        .map_err(|e| ToolError(e.to_string()))?;
    let folder = folder.map(|t| reg.take_save(&label, &t).map(PathBuf::from)).transpose().map_err(|e| ToolError(e.to_string()))?;
    let mut env = env_of(&app, eng.inner(), tess.tesseract());
    let ask = prompts.inner().clone();
    env.ask = Some(Arc::new(move |request| ask.register(job, request)));
    let prompts = prompts.inner().clone();
    // A job id names one running job: a second run under it would share, and so cancel, the first.
    let Some(guard) = jobs.inner().try_begin(job) else {
        return Err(ToolError("A batch with that job id is already running.".into()));
    };
    blocking(move || {
        let reg = window.state::<Paths>();
        let tokenize = |r: &mut run::FileReport| {
            for o in &mut r.outputs {
                reg.learn(&o.path);
                o.token = Some(reg.issue_open(&label, o.path.clone()).token);
            }
        };
        let ran = run::run(&action, &paths, folder.as_deref(), &env, &guard.cancel, &mut |mut ev| {
            if let Event::FileDone { report, .. } = &mut ev {
                tokenize(report);
            }
            let _ = on_event.send(ev);
        });
        prompts.end_job(job);
        let mut report = ran?;
        for f in &mut report.files {
            tokenize(f);
        }
        Ok(report)
    })
    .await
}

/// The person's answer to a `prompt` event of the run `job`. False when nothing was waiting under `request`
/// (it was answered, the run ended or was cancelled, or the id is not one of this run's): the answer is dropped.
#[tauri::command]
pub fn batch_prompt_answer(prompts: PromptState<'_>, job: u64, request: u64, answer: PromptAnswer) -> bool {
    let a = match answer {
        PromptAnswer::Continue { step } => Answer::Continue(step),
        PromptAnswer::Skip => Answer::Skip,
        PromptAnswer::Stop => Answer::Stop,
    };
    prompts.answer(job, request, a)
}

#[tauri::command]
pub fn batch_cancel(jobs: JobState<'_>, job: u64) {
    jobs.cancel(job);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::action::Step;

    /// The exact JSON the UI sends (`BatchAnswer` in api.ts): a wrong tag or casing would leave a prompt
    /// unanswerable, and the run waiting until it timed out.
    #[test]
    fn the_answers_the_ui_sends_are_read_and_a_late_or_second_one_is_dropped() {
        let go: PromptAnswer = serde_json::from_str(
            r#"{"kind":"continue","step":{"kind":"watermark","text":"FINAL","pages":"1-2","prompt":true}}"#,
        )
        .unwrap();
        let PromptAnswer::Continue { step } = go else { panic!("not a continue") };
        assert!(matches!(&step.step, Step::Watermark(w) if w.text == "FINAL"));
        assert_eq!(step.pages, "1-2");
        assert!(step.prompt);
        assert!(matches!(serde_json::from_str::<PromptAnswer>(r#"{"kind":"skip"}"#).unwrap(), PromptAnswer::Skip));
        assert!(matches!(serde_json::from_str::<PromptAnswer>(r#"{"kind":"stop"}"#).unwrap(), PromptAnswer::Stop));
        assert!(serde_json::from_str::<PromptAnswer>(r#"{"kind":"carryOn"}"#).is_err());

        let prompts = Prompts::default();
        let rx = prompts.register(3, 11);
        assert!(!prompts.answer(3, 99, Answer::Skip), "an unknown request");
        assert!(!prompts.answer(4, 11, Answer::Skip), "another run's request");
        assert!(prompts.answer(3, 11, Answer::Stop));
        assert!(matches!(rx.recv().unwrap(), Answer::Stop));
        assert!(!prompts.answer(3, 11, Answer::Skip), "a second answer");
        // a run that ends takes its open questions with it
        let _rx = prompts.register(5, 20);
        prompts.end_job(5);
        assert!(!prompts.answer(5, 20, Answer::Skip));
    }
}
