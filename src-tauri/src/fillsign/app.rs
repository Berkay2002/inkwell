//! Tauri commands for Fill & Sign. Moving, resizing and deleting an item are `annot_edit` and
//! `annot_delete`. No command here takes a path: an image from disk comes in as an open token.

use super::store::{self, Kind, Saved, Source, Store};
use super::{ItemDto, NewItem, Spec, add_one, flatten_one, list_all};
use crate::annot::app::{Changed, blocking};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use base64::Engine as _;
use serde::Serialize;

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;

/// A saved signature as the webview shows it.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SavedDto {
    pub id: String,
    pub kind: Kind,
    pub source: Source,
    pub font: Option<String>,
    /// Unix seconds.
    pub created: u64,
    pub width: u32,
    pub height: u32,
    /// `data:image/png;base64,...`
    pub data_url: String,
}

impl From<&Saved> for SavedDto {
    fn from(s: &Saved) -> Self {
        let [width, height] = store::png_size(&s.png);
        let data_url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&s.png));
        SavedDto { id: s.id.clone(), kind: s.kind, source: s.source, font: s.font.clone(), created: s.created, width, height, data_url }
    }
}

fn store_of(app: &tauri::AppHandle) -> Result<Store> {
    let (dir, _) = crate::recents::data_dir(app).map_err(EngineError::Io)?;
    Ok(Store::new(dir.join("signatures")))
}

#[tauri::command]
pub async fn fillsign_list(eng: Eng<'_>, doc: DocId) -> Result<Vec<ItemDto>> {
    let eng = eng.inner().clone();
    blocking(move || list_all(&eng, doc)).await
}

/// Places one item. A signature or initials names a saved one (`savedId`) or carries a PNG (`pngBase64`).
#[tauri::command]
pub async fn fillsign_add(app: tauri::AppHandle, eng: Eng<'_>, doc: DocId, item: NewItem) -> Result<Changed> {
    let eng = eng.inner().clone();
    let store = store_of(&app)?;
    blocking(move || {
        let png = match &item.spec {
            Spec::Image { saved_id: Some(id), .. } => Some(store.get(id)?.png),
            Spec::Image { png_base64: Some(b), .. } => Some(store::decode_png(b)?),
            _ => None,
        };
        let (state, xref) = add_one(&eng, doc, &item, png.as_deref(), EditSource::User)?;
        Ok(Changed { state, xref })
    })
    .await
}

/// Bakes the Fill & Sign items of `pages` (all pages when absent) into the page content: one undo step.
#[tauri::command]
pub async fn fillsign_flatten(eng: Eng<'_>, doc: DocId, pages: Option<Vec<u32>>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || flatten_one(&eng, doc, pages.as_deref(), EditSource::User)).await
}

#[tauri::command]
pub async fn fillsign_saved_list(app: tauri::AppHandle) -> Result<Vec<SavedDto>> {
    let store = store_of(&app)?;
    blocking(move || Ok(store.list()?.iter().map(SavedDto::from).collect())).await
}

#[tauri::command]
pub async fn fillsign_saved_add(app: tauri::AppHandle, kind: Kind, source: Source, font: Option<String>, png_base64: String) -> Result<SavedDto> {
    let store = store_of(&app)?;
    blocking(move || Ok((&store.add(kind, source, font, store::decode_png(&png_base64)?)?).into())).await
}

/// Saves a PNG or JPEG the user picked (`pick_open` token) as a signature or initials.
#[tauri::command]
pub async fn fillsign_saved_add_file(
    window: tauri::Window,
    app: tauri::AppHandle,
    reg: tauri::State<'_, crate::paths::Paths>,
    kind: Kind,
    token: String,
) -> Result<SavedDto> {
    let path = reg.open(window.label(), &token)?;
    let store = store_of(&app)?;
    blocking(move || {
        let f = std::fs::File::open(&path).map_err(|e| EngineError::Io(e.to_string()))?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(f, store::MAX_SOURCE + 1), &mut bytes).map_err(|e| EngineError::Io(e.to_string()))?;
        if bytes.len() as u64 > store::MAX_SOURCE {
            return Err(EngineError::Invalid("choose an image smaller than 4 MB".into()));
        }
        let png = store::normalize(&bytes, false, true)?;
        Ok((&store.add(kind, Source::Image, None, png)?).into())
    })
    .await
}

#[tauri::command]
pub async fn fillsign_saved_remove(app: tauri::AppHandle, id: String) -> Result<()> {
    let store = store_of(&app)?;
    blocking(move || store.remove(&id)).await
}
