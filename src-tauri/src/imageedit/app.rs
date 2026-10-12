//! Tauri commands for editing images. The UI is the user; the assistant calls the functions in
//! `super` with its turn.

use super::{ClipInfo, ImageBlocks, ImageChange, ImageClipboard, ImageEdited, ImageItem};
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

/// One change to each of several images of `page` listed at `rev`, as one undo step (move, turn,
/// flip or delete).
#[tauri::command]
pub async fn image_edit_many(eng: Eng<'_>, doc: DocId, rev: u32, page: u32, items: Vec<ImageItem>) -> Result<ImageEdited> {
    let eng = eng.inner().clone();
    blocking(move || super::edit_many(&eng, doc, rev, page, &items, EditSource::User)).await
}

/// Copies images `indices` of `page` (listed at `rev`) to the app's picture clipboard.
#[tauri::command]
pub async fn image_copy(eng: Eng<'_>, clip: tauri::State<'_, ImageClipboard>, doc: DocId, rev: u32, page: u32, indices: Vec<u32>) -> Result<ClipInfo> {
    let eng = eng.inner().clone();
    let copied = blocking(move || super::copy(&eng, doc, rev, page, &indices)).await?;
    let info = copied.info();
    *clip.0.lock() = Some(std::sync::Arc::new(copied));
    Ok(info)
}

/// What the picture clipboard holds, if anything.
#[tauri::command]
pub fn image_clip_info(clip: tauri::State<'_, ImageClipboard>) -> Option<ClipInfo> {
    clip.0.lock().as_ref().map(|c| c.info())
}

/// Pastes the picture clipboard on `page`, its top left corner at page-space `at`.
#[tauri::command]
pub async fn image_paste(eng: Eng<'_>, clip: tauri::State<'_, ImageClipboard>, doc: DocId, rev: u32, page: u32, at: [f64; 2]) -> Result<ImageEdited> {
    let held = clip.0.lock().clone().ok_or_else(|| EngineError::Invalid("no pictures are copied".into()))?;
    let eng = eng.inner().clone();
    blocking(move || super::paste(&eng, doc, rev, page, at, &held, EditSource::User)).await
}

/// Moves images `indices` of `page` to `to_page` of the same document, the group's top left
/// corner at page-space `at` there.
#[tauri::command]
pub async fn image_move_to_page(
    eng: Eng<'_>,
    doc: DocId,
    rev: u32,
    page: u32,
    indices: Vec<u32>,
    to_page: u32,
    at: [f64; 2],
) -> Result<ImageEdited> {
    let eng = eng.inner().clone();
    blocking(move || super::move_to_page(&eng, doc, rev, page, &indices, to_page, at, EditSource::User)).await
}
