//! Tauri commands for combine, attachments, page labels and transitions. The UI is the user; the
//! assistant calls the functions in `super` with its turn. Files are named by token only.

use super::attach::{Attachment, NewFile};
use super::combine::{self, CombineOptions, Plan, SourceSpec};
use super::labels::{LabelSpec, LabelStyle};
use super::trans::{PageTrans, TransChange};
use super::{LabelsDto, attach_add, attach_data, attach_describe, attach_list, attach_remove, labels_apply, labels_clear, labels_get, trans_get, trans_set};
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

/// The largest file attached in one go.
const MAX_ATTACH: u64 = 512 << 20;

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
}

fn specs(window: &Window, reg: &Paths, files: &[CombineFile]) -> Result<Vec<SourceSpec>> {
    files
        .iter()
        .map(|f| Ok(SourceSpec { path: reg.open(window.label(), &f.file)?, range: f.range.clone(), title: f.title.clone() }))
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

/// Open dialog for any files to attach; open tokens come back.
#[tauri::command]
pub async fn pick_attachments(window: Window) -> Result<Vec<FileRef>> {
    blocking(move || {
        let d = window.dialog().file().set_parent(&window).set_title("Attach files");
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
            new.push(NewFile { name: file_name_of(p), data, description: description.clone().unwrap_or_default() });
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
        let (_, bytes) = attach_data(&eng, doc, &id)?;
        write_plain(std::path::Path::new(&to), &bytes)?;
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

/// Bytes to `path` through a temporary file next to it; an existing file is replaced as a whole.
fn write_plain(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
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

