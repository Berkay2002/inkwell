//! Tauri commands for measuring. The UI is the user; the assistant calls the functions in `super` with its turn.

use super::{Kind, MeasureDto, PageScale, Quantity, Scale, SnapCache, SnapHit, SnapKinds, add_one, csv_of, list_all, rescale, scale_get, scale_set, snap_at_kinds, value_of};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::paths::Paths;
use serde::Serialize;
use tauri::{Manager, Window};

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// The edit state after a measurement, the xref of the annotation and its value.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Added {
    pub state: EditState,
    pub xref: i32,
    pub quantity: Quantity,
}

/// The scale of a page (its widest viewport), 1 in = 1 in when the file has none.
#[tauri::command]
pub async fn measure_scale_get(eng: Eng<'_>, doc: DocId, page: u32) -> Result<PageScale> {
    let eng = eng.inner().clone();
    blocking(move || scale_get(&eng, doc, page)).await
}

/// Makes `scale` the scale of the whole page; one undo step.
#[tauri::command]
pub async fn measure_scale_set(eng: Eng<'_>, doc: DocId, page: u32, scale: Scale) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || scale_set(&eng, doc, page, &scale, EditSource::User)).await
}

/// Adds a measurement of `kind` through `points` (page space) as one undo step. `scale`: this measurement's own;
/// absent, the page's scale at the first point.
#[tauri::command]
pub async fn measure_add(eng: Eng<'_>, doc: DocId, page: u32, kind: Kind, points: Vec<[f32; 2]>, scale: Option<Scale>, author: String) -> Result<Added> {
    let eng = eng.inner().clone();
    blocking(move || {
        let (state, xref, quantity) = add_one(&eng, doc, page, kind, &points, scale.as_ref(), &author, EditSource::User)?;
        Ok(Added { state, xref, quantity })
    })
    .await
}

/// The value `points` would measure (the live readout while clicking), without adding anything.
#[tauri::command]
pub async fn measure_value(eng: Eng<'_>, doc: DocId, page: u32, kind: Kind, points: Vec<[f32; 2]>, scale: Option<Scale>) -> Result<Quantity> {
    let eng = eng.inner().clone();
    blocking(move || value_of(&eng, doc, page, kind, &points, scale.as_ref())).await
}

/// Every measurement annotation of the document, valued from its own `/Measure`.
#[tauri::command]
pub async fn measure_list(eng: Eng<'_>, doc: DocId) -> Result<Vec<MeasureDto>> {
    let eng = eng.inner().clone();
    blocking(move || list_all(&eng, doc)).await
}

/// Gives the measurement `xref` another scale; one undo step. The annotation is replaced, so `xref` changes.
#[tauri::command]
pub async fn measure_rescale(eng: Eng<'_>, doc: DocId, page: u32, xref: i32, scale: Scale) -> Result<Added> {
    let eng = eng.inner().clone();
    blocking(move || {
        let (state, xref, quantity) = rescale(&eng, doc, page, xref, &scale, EditSource::User)?;
        Ok(Added { state, xref, quantity })
    })
    .await
}

/// Writes every measurement of the document as CSV to the file a save token names. Resolves to the path.
#[tauri::command]
pub async fn measure_export_csv(window: Window, eng: Eng<'_>, reg: tauri::State<'_, Paths>, doc: DocId, to: String) -> Result<String> {
    let to = reg.take_save(window.label(), &to)?;
    let eng = eng.inner().clone();
    blocking(move || {
        // Never over a file a tab has open.
        eng.check_not_open(&to, None)?;
        let csv = csv_of(&list_all(&eng, doc)?);
        crate::organize::app::write_plain(std::path::Path::new(&to), csv.as_bytes())?;
        window.state::<Paths>().learn(&to);
        Ok(to)
    })
    .await
}

/// The nearest end point, midpoint, crossing or line of the page's vector paths within `radius` points of `(x, y)`
/// (page space), among the `kinds` that are on (all of them when absent).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn measure_snap(
    eng: Eng<'_>,
    cache: tauri::State<'_, std::sync::Arc<SnapCache>>,
    doc: DocId,
    page: u32,
    x: f32,
    y: f32,
    radius: f32,
    kinds: Option<SnapKinds>,
) -> Result<Option<SnapHit>> {
    let eng = eng.inner().clone();
    let cache = cache.inner().clone();
    blocking(move || snap_at_kinds(&eng, &cache, doc, page, x, y, radius, &kinds.unwrap_or_default())).await
}
