//! Tauri commands for combine, attachments, page labels and transitions. The UI is the user; the
//! assistant calls the functions in `super` with its turn. Files are named by token only.

use crate::batch::action::Secret;
use super::attach::{Attachment, MAX_ATTACH, NewFile, clean_name};
use super::combine::{self, CombineOptions, Plan, SourceSpec};
use super::labels::{LabelSpec, LabelStyle};
use super::portfolio::{self, PortfolioInfo, Source};
use super::threads::{Info, Thread};
use super::trans::{PageTrans, TransChange};
use super::{LabelsDto, attach_add, attach_data, attach_describe, attach_list, attach_remove, labels_apply, labels_clear, labels_get, thread_add_bead, thread_create, thread_delete, thread_delete_bead, thread_move_bead, thread_reorder, thread_set_info, threads_get, trans_get, trans_set};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::paths::{FileRef, Paths};
use serde::{Deserialize, Serialize};
use tauri::{Manager, Window};
use tauri_plugin_dialog::DialogExt;

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;
type Reg<'a> = tauri::State<'a, Paths>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}


// ---- combine ------------------------------------------------------------------------------------

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CombineFile {
    /// An open token.
    pub file: String,
    #[serde(default)]
    pub range: String,
    #[serde(default)]
    pub title: Option<String>,
    /// The open password the person typed for a locked file; in memory only, never written anywhere.
    #[serde(default)]
    pub password: Option<Secret>,
}

fn specs(window: &Window, reg: &Paths, files: &[CombineFile]) -> Result<Vec<SourceSpec>> {
    files
        .iter()
        .map(|f| Ok(SourceSpec { path: reg.open(window.label(), &f.file)?, range: f.range.clone(), title: f.title.clone(), password: f.password.clone() }))
        .collect()
}

/// What combining these files would give: the sheet's prediction, and the reason for every file left out.
#[tauri::command]
pub async fn combine_inspect(window: Window, reg: Reg<'_>, files: Vec<CombineFile>, options: CombineOptions) -> Result<Plan> {
    let specs = specs(&window, &reg, &files)?;
    blocking(move || combine::inspect(&specs, options)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Combined {
    pub file: FileRef,
    pub plan: Plan,
}

/// Opens a locked source's engine handle (the one the sheet drew its row with) with the password the
/// person typed, so its pages can be drawn. Unlike `authenticate` this does not start a history for the
/// file: it is only a source. A wrong password is `bad-password` and changes nothing; a document that is
/// not waiting for a password (an open tab) is refused.
#[tauri::command]
pub async fn combine_unlock(eng: Eng<'_>, doc: DocId, password: Secret) -> Result<crate::engine::OpenInfo> {
    let eng = eng.inner().clone();
    blocking(move || eng.unlock_pending(doc, password.as_str())).await
}

/// Combines into the file a save token names, and hands back an open token for it.
#[tauri::command]
pub async fn combine_run(window: Window, eng: Eng<'_>, reg: Reg<'_>, files: Vec<CombineFile>, options: CombineOptions, out: String) -> Result<Combined> {
    let specs = specs(&window, &reg, &files)?;
    let out = reg.take_save(window.label(), &out)?;
    let eng = eng.inner().clone();
    blocking(move || {
        let r = combine::run(&eng, &specs, options, &out)?;
        let file = window.state::<Paths>().issue_open(window.label(), r.path);
        Ok(Combined { file, plan: r.plan })
    })
    .await
}

// ---- attachments --------------------------------------------------------------------------------

/// Open dialog for any files, with a title Rust owns; open tokens come back.
async fn pick_any(window: Window, title: &'static str) -> Result<Vec<FileRef>> {
    blocking(move || {
        let d = window.dialog().file().set_parent(&window).set_title(title);
        let reg = window.state::<Paths>();
        Ok(d.blocking_pick_files()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.into_path().ok())
            .map(|p| reg.issue_open(window.label(), p.to_string_lossy().into_owned()))
            .collect())
    })
    .await
}

/// Open dialog for any files to attach; open tokens come back.
#[tauri::command]
pub async fn pick_attachments(window: Window) -> Result<Vec<FileRef>> {
    pick_any(window, "Attach files").await
}

#[tauri::command]
pub async fn attachments_list(eng: Eng<'_>, doc: DocId) -> Result<Vec<Attachment>> {
    let eng = eng.inner().clone();
    blocking(move || attach_list(&eng, doc)).await
}

/// Attaches the files the tokens name (one undo step).
#[tauri::command]
pub async fn attachments_add(window: Window, eng: Eng<'_>, reg: Reg<'_>, doc: DocId, files: Vec<String>, description: Option<String>) -> Result<EditState> {
    let paths: Vec<String> = files.iter().map(|t| reg.open(window.label(), t)).collect::<Result<_>>()?;
    let eng = eng.inner().clone();
    blocking(move || {
        let mut new = Vec::new();
        for p in &paths {
            let meta = std::fs::metadata(p).map_err(|e| EngineError::Io(format!("{p}: {e}")))?;
            if !meta.is_file() {
                return Err(EngineError::Invalid(format!("{p} is not a file")));
            }
            if meta.len() > MAX_ATTACH {
                return Err(EngineError::Invalid(format!("{} is over 512 MB, too large to attach", file_name_of(p))));
            }
            let data = std::fs::read(p).map_err(|e| EngineError::Io(format!("{p}: {e}")))?;
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
            new.push(NewFile { name: file_name_of(p), data, description: description.clone().unwrap_or_default(), modified });
        }
        attach_add(&eng, doc, new, EditSource::User)
    })
    .await
}

/// Writes the attachment to the file a save token names. It is only written, never run.
#[tauri::command]
pub async fn attachment_save(window: Window, eng: Eng<'_>, reg: Reg<'_>, doc: DocId, id: String, to: String) -> Result<String> {
    let to = reg.take_save(window.label(), &to)?;
    let eng = eng.inner().clone();
    blocking(move || {
        // Never over a file a tab has open: it would fail on Windows or swap the file under the engine.
        eng.check_not_open(&to, None)?;
        let (_, bytes) = attach_data(&eng, doc, &id)?;
        write_plain(std::path::Path::new(&to), &bytes)?;
        mark_from_internet(&to);
        window.state::<Paths>().learn(&to);
        Ok(to)
    })
    .await
}

/// A PDF attachment as an unsaved document of its own: the bytes are checked to be a PDF, written
/// to the scratch folder and handed back as an open token. Any other file is refused.
#[tauri::command]
pub async fn attachment_open(window: Window, eng: Eng<'_>, doc: DocId, id: String) -> Result<FileRef> {
    let eng = eng.inner().clone();
    blocking(move || {
        let (name, bytes) = attach_data(&eng, doc, &id)?;
        if !bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-") {
            return Err(EngineError::Invalid(format!("{name} is not a PDF. Save it to a folder to use it.")));
        }
        let name = clean_name(&name);
        let name = if name.to_lowercase().ends_with(".pdf") { name } else { format!("{name}.pdf") };
        let path = crate::convert::write_scratch_bytes(&name, &bytes).map_err(|e| EngineError::Io(e.to_string()))?;
        // Not a file the user chose: an open token, but not a "known" path for the recent list.
        Ok(window.state::<Paths>().issue_open(window.label(), path.to_string_lossy().into_owned()))
    })
    .await
}

#[tauri::command]
pub async fn attachments_remove(eng: Eng<'_>, doc: DocId, ids: Vec<String>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || attach_remove(&eng, doc, &ids, EditSource::User)).await
}

#[tauri::command]
pub async fn attachment_describe(eng: Eng<'_>, doc: DocId, id: String, text: String) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || attach_describe(&eng, doc, &id, text.trim(), EditSource::User)).await
}

fn file_name_of(p: &str) -> String {
    std::path::Path::new(p).file_name().map_or_else(|| p.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Marks a file taken out of a document as coming from the Internet zone (Zone.Identifier, ZoneId=3),
/// so Windows warns before it is run. Best effort: a file system without streams just goes without.
#[cfg(windows)]
fn mark_from_internet(path: &str) {
    let _ = std::fs::write(format!("{path}:Zone.Identifier"), b"[ZoneTransfer]\r\nZoneId=3\r\n");
}

#[cfg(not(windows))]
fn mark_from_internet(_path: &str) {}

/// Bytes to `path` through a temporary file next to it; an existing file is replaced as a whole.
pub(crate) fn write_plain(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "attachment".into());
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let tmp = path.with_file_name(format!(".{name}.{nonce:x}.inkwell-tmp"));
    let io = |e: std::io::Error| EngineError::Io(e.to_string());
    let r = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(io)
}

// ---- page labels --------------------------------------------------------------------------------

#[tauri::command]
pub async fn labels_list(eng: Eng<'_>, doc: DocId) -> Result<LabelsDto> {
    let eng = eng.inner().clone();
    blocking(move || labels_get(&eng, doc)).await
}

/// Labels pages `from..=to` (0-based).
#[tauri::command]
pub async fn labels_set(eng: Eng<'_>, doc: DocId, from: u32, to: u32, style: LabelStyle, prefix: String, start: u32, extend: bool) -> Result<EditState> {
    let eng = eng.inner().clone();
    let spec = LabelSpec { style, prefix, start, extend };
    blocking(move || labels_apply(&eng, doc, from, to, &spec, EditSource::User)).await
}

#[tauri::command]
pub async fn labels_remove(eng: Eng<'_>, doc: DocId) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || labels_clear(&eng, doc, EditSource::User)).await
}

// ---- transitions --------------------------------------------------------------------------------

#[tauri::command]
pub async fn transitions_list(eng: Eng<'_>, doc: DocId) -> Result<Vec<PageTrans>> {
    let eng = eng.inner().clone();
    blocking(move || trans_get(&eng, doc)).await
}

#[tauri::command]
pub async fn transitions_set(eng: Eng<'_>, doc: DocId, pages: Vec<u32>, change: TransChange) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || trans_set(&eng, doc, &pages, &change, EditSource::User)).await
}


// ---- article threads ----------------------------------------------------------------------------

#[tauri::command]
pub async fn threads_list(eng: Eng<'_>, doc: DocId) -> Result<Vec<Thread>> {
    let eng = eng.inner().clone();
    blocking(move || threads_get(&eng, doc)).await
}

#[tauri::command]
pub async fn thread_start(eng: Eng<'_>, doc: DocId, title: String, page: u32, rect: [f32; 4]) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || thread_create(&eng, doc, &title, page, rect, EditSource::User)).await
}

/// `after`: the bead to put the new one behind; none puts it at the end.
#[tauri::command]
pub async fn thread_bead_add(eng: Eng<'_>, doc: DocId, thread: i32, after: Option<i32>, page: u32, rect: [f32; 4]) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || thread_add_bead(&eng, doc, thread, after, page, rect, EditSource::User)).await
}

#[tauri::command]
pub async fn thread_bead_move(eng: Eng<'_>, doc: DocId, thread: i32, bead: i32, page: u32, rect: [f32; 4]) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || thread_move_bead(&eng, doc, thread, bead, page, rect, EditSource::User)).await
}

#[tauri::command]
pub async fn thread_bead_delete(eng: Eng<'_>, doc: DocId, thread: i32, bead: i32) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || thread_delete_bead(&eng, doc, thread, bead, EditSource::User)).await
}

#[tauri::command]
pub async fn thread_remove(eng: Eng<'_>, doc: DocId, thread: i32) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || thread_delete(&eng, doc, thread, EditSource::User)).await
}

#[tauri::command]
pub async fn thread_describe(eng: Eng<'_>, doc: DocId, thread: i32, info: Info) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || thread_set_info(&eng, doc, thread, &info, EditSource::User)).await
}

#[tauri::command]
pub async fn thread_move(eng: Eng<'_>, doc: DocId, thread: i32, to: u32) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || thread_reorder(&eng, doc, thread, to, EditSource::User)).await
}

// ---- portfolios ---------------------------------------------------------------------------------

/// The portfolio view of a document, none for a plain PDF. A collection the engine refuses (folders that
/// loop) is an error: the document stays a plain one.
#[tauri::command]
pub async fn portfolio_info(eng: Eng<'_>, doc: DocId) -> Result<Option<PortfolioInfo>> {
    let eng = eng.inner().clone();
    blocking(move || super::portfolio_info(&eng, doc)).await
}

/// Open dialog for the files of a new portfolio (any type); open tokens come back.
#[tauri::command]
pub async fn pick_portfolio_files(window: Window) -> Result<Vec<FileRef>> {
    pick_any(window, "Choose files for the portfolio").await
}

/// A new portfolio from the files the tokens name, written to the file a save token names; an open token
/// for it comes back. The files are only read.
#[tauri::command]
pub async fn portfolio_create(window: Window, eng: Eng<'_>, reg: Reg<'_>, files: Vec<String>, out: String) -> Result<FileRef> {
    let sources: Vec<Source> = files
        .iter()
        .map(|t| reg.open(window.label(), t).map(|path| Source { name: file_name_of(&path), path }))
        .collect::<Result<_>>()?;
    let out = reg.take_save(window.label(), &out)?;
    let eng = eng.inner().clone();
    blocking(move || {
        let made = portfolio::create(&eng, &sources, &out)?;
        Ok(window.state::<Paths>().issue_open(window.label(), made.path))
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractReport {
    pub folder: String,
    pub written: Vec<String>,
    pub skipped: Vec<String>,
}

/// Writes every file of the portfolio into the folder a save token names (from `pick_folder`), never
/// replacing a file there: a name that is taken gets " (2)". Files are only written, never run.
#[tauri::command]
pub async fn portfolio_extract_all(window: Window, eng: Eng<'_>, reg: Reg<'_>, doc: DocId, to: String) -> Result<ExtractReport> {
    let folder = reg.take_save(window.label(), &to)?;
    let eng = eng.inner().clone();
    blocking(move || {
        // Each file is marked as soon as it is written, so a run that stops half way leaves none unmarked.
        let r = super::portfolio_extract_all(&eng, doc, std::path::Path::new(&folder), &mut |p| mark_from_internet(&p.to_string_lossy()))?;
        window.state::<Paths>().learn(&folder);
        Ok(ExtractReport { folder, written: r.written, skipped: r.skipped })
    })
    .await
}
