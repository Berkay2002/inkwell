//! Tauri commands for comments. The UI is the user; the assistant calls the functions in `super` with its turn.

use super::{AnnotDto, NewAnnot, Patch, Style, add_one, delete_many, edit_one, list_all, reply_one, state_one};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use serde::Serialize;

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
pub async fn annot_list(eng: Eng<'_>, doc: DocId) -> Result<Vec<AnnotDto>> {
    let eng = eng.inner().clone();
    blocking(move || list_all(&eng, doc)).await
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
