//! The batch commands. The webview names files by token only: the files to process are open tokens,
//! a folder result is a `pick_folder` token, an exported action goes to a `pick_save` token.
//!
//! `batch_run` takes the Action itself (the editor's current state, passwords of a Protect step
//! included) so that an unsaved action can be tried; the passwords live in that call only.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, Window};

use super::action::{Action, Issue};
use super::run::{self, BatchError, Event, Report};
use super::steps::Env;
use super::store::{self, Store, StoreError};
use crate::engine::Engine;
use crate::paths::Paths;
use crate::tools::paths::ToolPaths;
use crate::tools::{Jobs, ToolError};

type Eng<'a> = State<'a, Arc<Engine>>;
type Reg<'a> = State<'a, Paths>;
type JobState<'a> = State<'a, Arc<Jobs>>;
type Tess<'a> = State<'a, Arc<ToolPaths>>;

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
    let env = env_of(&app, eng.inner(), tess.tesseract());
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
        let mut report = run::run(&action, &paths, folder.as_deref(), &env, &guard.cancel, &mut |mut ev| {
            if let Event::FileDone { report, .. } = &mut ev {
                tokenize(report);
            }
            let _ = on_event.send(ev);
        })?;
        for f in &mut report.files {
            tokenize(f);
        }
        Ok(report)
    })
    .await
}

#[tauri::command]
pub fn batch_cancel(jobs: JobState<'_>, job: u64) {
    jobs.cancel(job);
}
