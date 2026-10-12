//! Tauri commands for editing existing text. The UI is the user; the assistant calls the
//! functions in `super` with its turn.

use super::replace::{ReplaceRequest, Replaced};
use super::{BlockChange, FontInfo, NewTextBox, TextBlocks, TextEdited};
use crate::engine::edit::EditSource;
use crate::engine::edit::EditState;
use crate::engine::{DocId, Engine, EngineError};

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// The text blocks of `page` (0-based), with the `rev` they belong to.
#[tauri::command]
pub async fn text_blocks(eng: Eng<'_>, doc: DocId, page: u32) -> Result<TextBlocks> {
    let eng = eng.inner().clone();
    blocking(move || super::blocks(&eng, doc, page)).await
}

/// Changes block `index` of `page` as listed at `rev` (`Stale` if the document moved on).
#[tauri::command]
pub async fn text_edit(eng: Eng<'_>, doc: DocId, rev: u32, page: u32, index: u32, change: BlockChange) -> Result<TextEdited> {
    let eng = eng.inner().clone();
    blocking(move || super::edit(&eng, doc, rev, page, index, &change, EditSource::User)).await
}

/// Adds a text box to `page` (0-based) as listed at `rev`.
#[tauri::command]
pub async fn text_add(eng: Eng<'_>, doc: DocId, rev: u32, page: u32, new: NewTextBox) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || super::add(&eng, doc, rev, page, &new, EditSource::User)).await
}

/// Replaces the find text with `req.with` in every text block that holds it (`req.at` none), or in the one
/// match at `req.at`: one undo step. See `replace.rs`.
#[tauri::command]
pub async fn text_replace(eng: Eng<'_>, doc: DocId, rev: u32, req: ReplaceRequest) -> Result<Replaced> {
    let eng = eng.inner().clone();
    blocking(move || super::replace::replace(&eng, doc, rev, &req, EditSource::User)).await
}

/// The fonts to offer for text: the document's, the standard 14 and the bundled faces.
#[tauri::command]
pub async fn text_fonts(eng: Eng<'_>, doc: DocId) -> Result<Vec<FontInfo>> {
    let eng = eng.inner().clone();
    blocking(move || super::fonts(&eng, doc)).await
}
