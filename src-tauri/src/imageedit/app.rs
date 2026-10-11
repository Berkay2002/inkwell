//! Tauri commands for editing images. The UI is the user; the assistant calls the functions in
//! `super` with its turn.

use super::{ImageBlocks, ImageChange, ImageEdited};
use crate::engine::edit::EditSource;
use crate::engine::{DocId, Engine, EngineError};
use crate::paths::Paths;
use tauri::Window;

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// The images `page` (0-based) draws, with the `rev` they belong to.
#[tauri::command]
pub async fn image_blocks(eng: Eng<'_>, doc: DocId, page: u32) -> Result<ImageBlocks> {
    let eng = eng.inner().clone();
    blocking(move || super::blocks(&eng, doc, page)).await
}

/// Changes image `index` of `page` as listed at `rev`, or adds one (`index` omitted, `op: "add"`).
/// An image file (`replace`, `add`) is an open token from `pick_source`, never a path.
#[tauri::command]
pub async fn image_edit(
    window: Window,
    eng: Eng<'_>,
    reg: tauri::State<'_, Paths>,
    doc: DocId,
    rev: u32,
    page: u32,
    index: Option<u32>,
    mut change: ImageChange,
) -> Result<ImageEdited> {
    if let Some(file) = change.file_mut() {
        *file = reg.open(window.label(), file)?;
    }
    let eng = eng.inner().clone();
    blocking(move || super::edit(&eng, doc, rev, page, index, &change, EditSource::User)).await
}
