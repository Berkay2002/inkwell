//! Tauri commands for comments. The UI is the user; the assistant calls the functions in `super` with its turn.

use super::stamps::{self, Gate, Library, Source, Stamp};
use super::{AnnotList, NewAnnot, Patch, Style, add_one, delete_many, edit_one, list_status, reply_one, state_one};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::exchange::{self, Format};
use crate::paths::Paths;
use serde::Serialize;
use std::path::PathBuf;
use tauri::{Manager, Window};

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// The edit state after a change, and the xref of what it created (0 when nothing was created).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changed {
    pub state: EditState,
    pub xref: i32,
}

/// The login name comments are written under until the user sets another one.
#[tauri::command]
pub fn annot_default_author() -> String {
    super::os_user(|k| std::env::var(k).ok())
}

#[tauri::command]
pub async fn annot_list(eng: Eng<'_>, doc: DocId) -> Result<AnnotList> {
    let eng = eng.inner().clone();
    blocking(move || list_status(&eng, doc)).await
}

#[tauri::command]
pub async fn annot_add(eng: Eng<'_>, doc: DocId, page: u32, what: NewAnnot, style: Option<Style>, author: String) -> Result<Changed> {
    let eng = eng.inner().clone();
    blocking(move || {
        let (state, xref) = add_one(&eng, doc, page, &what, &style.unwrap_or_default(), &author, EditSource::User)?;
        Ok(Changed { state, xref })
    })
    .await
}

#[tauri::command]
pub async fn annot_edit(eng: Eng<'_>, doc: DocId, page: u32, xref: i32, patch: Patch, author: String) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || edit_one(&eng, doc, page, xref, &patch, &author, EditSource::User)).await
}

/// `items`: `(page, xref)` pairs.
#[tauri::command]
pub async fn annot_delete(eng: Eng<'_>, doc: DocId, items: Vec<(u32, i32)>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || delete_many(&eng, doc, &items, EditSource::User)).await
}

#[tauri::command]
pub async fn annot_reply(eng: Eng<'_>, doc: DocId, page: u32, parent: i32, text: String, author: String) -> Result<Changed> {
    let eng = eng.inner().clone();
    blocking(move || {
        let (state, xref) = reply_one(&eng, doc, page, parent, &text, &author, EditSource::User)?;
        Ok(Changed { state, xref })
    })
    .await
}

/// `state`: "Completed" (resolved), "None" (reopen), "Accepted", "Rejected" or "Cancelled".
#[tauri::command]
pub async fn annot_state(eng: Eng<'_>, doc: DocId, page: u32, parent: i32, state: String, author: String) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || state_one(&eng, doc, page, parent, &state, &author, EditSource::User)).await
}

// ---- custom stamps (docs/comments.md "Custom stamps") ---------------------------------------------

/// The library's folder: `<data dir>/stamps` (`INKWELL_DATA_DIR` moves it, like the recent list).
fn stamps_dir(app: &tauri::AppHandle) -> Result<PathBuf> {
    crate::recents::data_dir(app).map(|(d, _)| d.join("stamps")).map_err(EngineError::Io)
}

#[tauri::command]
pub async fn stamp_list(app: tauri::AppHandle) -> Result<Library> {
    let dir = stamps_dir(&app)?;
    blocking(move || Ok(stamps::list(&dir))).await
}

/// Makes a stamp from a picture or PDF the user picked (`file` is an open token from `pick_source`); `page` is
/// the 0-based page of a PDF.
#[tauri::command]
pub async fn stamp_create(window: Window, app: tauri::AppHandle, reg: tauri::State<'_, Paths>, file: String, name: String, category: String, page: Option<u32>) -> Result<Stamp> {
    let path = reg.open(window.label(), &file)?;
    let dir = stamps_dir(&app)?;
    blocking(move || stamps::create(&dir, &Source { path: &path, page }, &name, &category)).await
}

#[tauri::command]
pub async fn stamp_update(app: tauri::AppHandle, id: String, name: Option<String>, category: Option<String>) -> Result<Stamp> {
    let dir = stamps_dir(&app)?;
    blocking(move || stamps::update(&dir, &id, name.as_deref(), category.as_deref())).await
}

#[tauri::command]
pub async fn stamp_delete(app: tauri::AppHandle, id: String) -> Result<()> {
    let dir = stamps_dir(&app)?;
    blocking(move || stamps::delete(&dir, &id)).await
}

/// A stamp's thumbnail as a `data:` URL (the webview cannot read the data folder).
#[tauri::command]
pub async fn stamp_thumb(app: tauri::AppHandle, id: String) -> Result<String> {
    use base64::Engine as _;
    let dir = stamps_dir(&app)?;
    blocking(move || stamps::thumbnail(&dir, &id).map(|png| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))).await
}

/// Places library stamp `id` centred on `at` of `page`: one undo step.
#[tauri::command]
pub async fn stamp_place(app: tauri::AppHandle, eng: Eng<'_>, doc: DocId, page: u32, id: String, at: [f32; 2], author: String) -> Result<Changed> {
    let dir = stamps_dir(&app)?;
    let eng = eng.inner().clone();
    blocking(move || {
        let (stamp, png) = stamps::picture(&dir, &id)?;
        let (state, xref) = stamps::place_one(&eng, doc, page, &stamp, &png, at, &author, EditSource::User)?;
        Ok(Changed { state, xref })
    })
    .await
}

/// Whether a stamp can be placed in `doc` (and why not), and whether it is signed.
#[tauri::command]
pub async fn stamp_gate(eng: Eng<'_>, doc: DocId) -> Result<Gate> {
    let eng = eng.inner().clone();
    blocking(move || stamps::gate(&eng, doc)).await
}

/// Writes every comment to `to` (a save token) as XFDF, or FDF when the name ends in `.fdf`.
#[tauri::command]
pub async fn annot_export(window: Window, eng: Eng<'_>, reg: tauri::State<'_, Paths>, doc: DocId, to: String) -> Result<super::xfdf::Exported> {
    let dest = reg.take_save(window.label(), &to)?;
    eng.check_not_open(&dest, None)?;
    let eng = eng.inner().clone();
    blocking(move || {
        let name = std::path::Path::new(&eng.doc_entry(doc)?.path()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (bytes, done) = super::xfdf::export(&eng, doc, Format::of_path(&dest, false), &name)?;
        std::fs::write(&dest, bytes).map_err(|e| EngineError::Io(e.to_string()))?;
        window.state::<Paths>().learn(&dest);
        Ok(done)
    })
    .await
}

/// Reads comments from `file` (an open token; XFDF or FDF, told by what is inside) as one undo step.
#[tauri::command]
pub async fn annot_import(window: Window, eng: Eng<'_>, reg: tauri::State<'_, Paths>, doc: DocId, file: String) -> Result<super::xfdf::Imported> {
    let path = reg.open(window.label(), &file)?;
    let eng = eng.inner().clone();
    blocking(move || {
        let parsed = super::xfdf::parse(&exchange::read_capped(&path)?)?;
        super::xfdf::import_one(&eng, doc, &parsed, EditSource::User)
    })
    .await
}

/// What the import menus may offer for `doc` (see [`exchange::gates`]).
#[tauri::command]
pub async fn data_gates(eng: Eng<'_>, doc: DocId) -> Result<exchange::Gates> {
    let eng = eng.inner().clone();
    blocking(move || exchange::gates(&eng, doc)).await
}
