//! Tauri commands for bookmarks. The UI is the user; the assistant calls the functions in `super`.

use super::{BookmarkEdited, BookmarkOp, Bookmarks, HeadingPreview};
use crate::engine::edit::EditSource;
use crate::engine::{DocId, Engine, EngineError};

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// The bookmark tree, with the `rev` it belongs to.
#[tauri::command]
pub async fn bookmarks_list(eng: Eng<'_>, doc: DocId) -> Result<Bookmarks> {
    let eng = eng.inner().clone();
    blocking(move || super::list(&eng, doc)).await
}

/// Applies one change as listed at `rev` (`Stale` if the document moved on).
#[tauri::command]
pub async fn bookmarks_apply(eng: Eng<'_>, doc: DocId, rev: u32, op: BookmarkOp) -> Result<BookmarkEdited> {
    let eng = eng.inner().clone();
    blocking(move || super::apply(&eng, doc, rev, &op, EditSource::User)).await
}

/// How many bookmarks "from headings" would make (and the first of them), changing nothing.
#[tauri::command]
pub async fn bookmarks_headings(eng: Eng<'_>, doc: DocId) -> Result<HeadingPreview> {
    let eng = eng.inner().clone();
    blocking(move || super::preview_headings(&eng, doc)).await
}
