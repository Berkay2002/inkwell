//! Tauri commands for rich media. The UI is the user; the assistant has no media tool (a click is
//! what starts anything playing).

use super::{Cache, MediaItem, list_all};
use crate::engine::{DocId, Engine, EngineError};
use std::sync::Arc;

type Result<T> = std::result::Result<T, EngineError>;

/// The document's video, sound, 3D and Flash annotations, with what each can play.
#[tauri::command]
pub async fn media_list(eng: tauri::State<'_, Arc<Engine>>, doc: DocId) -> Result<Vec<MediaItem>> {
    let eng = eng.inner().clone();
    tauri::async_runtime::spawn_blocking(move || list_all(&eng, doc)).await.unwrap_or(Err(EngineError::Panic))
}

/// The player closed: the decoded asset is let go.
#[tauri::command]
pub fn media_release(cache: tauri::State<'_, Arc<Cache>>) {
    cache.clear();
}
