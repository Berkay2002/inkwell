//! Tauri commands for forms. The UI is the user; the assistant calls the functions in `super` with its turn.

use super::{
    Detected, FieldPatch, FieldValue, FormDto, NewField, ScriptsState, Stepped, add_many, add_one, arrange_one, calc_order_one, delete_many, detect_one, edit_one, list_all,
    move_one, reset_report, scripts_enable, scripts_state, set_value_report, tab_order_one,
};
use super::af::{self, Format};
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

#[tauri::command]
pub async fn forms_list(eng: Eng<'_>, doc: DocId) -> Result<FormDto> {
    let eng = eng.inner().clone();
    blocking(move || list_all(&eng, doc)).await
}

/// Fill a field. A value that fails the field's format or range (or a script that rejects it) is an
/// error carrying Acrobat's message. Returns the edit state with the `alerts` and `requests` the field
/// scripts raised (a request is only ever acted on after the person agrees).
#[tauri::command]
pub async fn forms_set_value(eng: Eng<'_>, doc: DocId, page: u32, xref: i32, value: FieldValue) -> Result<Stepped> {
    let eng = eng.inner().clone();
    blocking(move || set_value_report(&eng, doc, page, xref, &value, EditSource::User)).await
}

/// Reset the listed fields (widget xrefs), or the whole form when `fields` is absent ("Clear form").
#[tauri::command]
pub async fn forms_reset(eng: Eng<'_>, doc: DocId, fields: Option<Vec<i32>>) -> Result<Stepped> {
    let eng = eng.inner().clone();
    blocking(move || reset_report(&eng, doc, fields, EditSource::User)).await
}

/// Whether the file has scripts, whether they run, and the script log (`{level, field?, message}`).
#[tauri::command]
pub async fn forms_scripts(eng: Eng<'_>, doc: DocId) -> Result<ScriptsState> {
    let eng = eng.inner().clone();
    blocking(move || scripts_state(&eng, doc)).await
}

/// Turns the scripts of this open file on or off (kept in memory, on by default).
#[tauri::command]
pub async fn forms_scripts_enable(eng: Eng<'_>, doc: DocId, on: bool) -> Result<ScriptsState> {
    let eng = eng.inner().clone();
    blocking(move || scripts_enable(&eng, doc, on)).await
}

#[tauri::command]
pub async fn forms_add(eng: Eng<'_>, doc: DocId, page: u32, what: NewField) -> Result<Changed> {
    let eng = eng.inner().clone();
    blocking(move || {
        let (state, xref) = add_one(&eng, doc, page, &what, EditSource::User)?;
        Ok(Changed { state, xref })
    })
    .await
}

/// Add several fields in one undo step (what "Detect fields" found).
#[tauri::command]
pub async fn forms_add_many(eng: Eng<'_>, doc: DocId, page: u32, items: Vec<NewField>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || add_many(&eng, doc, page, &items, EditSource::User)).await
}

#[tauri::command]
pub async fn forms_move(eng: Eng<'_>, doc: DocId, page: u32, xref: i32, rect: [f32; 4]) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || move_one(&eng, doc, page, xref, rect, EditSource::User)).await
}

#[tauri::command]
pub async fn forms_edit(eng: Eng<'_>, doc: DocId, page: u32, xref: i32, patch: FieldPatch) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || edit_one(&eng, doc, page, xref, &patch, EditSource::User)).await
}

/// `items`: `(page, xref)` pairs.
#[tauri::command]
pub async fn forms_delete(eng: Eng<'_>, doc: DocId, items: Vec<(u32, i32)>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || delete_many(&eng, doc, &items, EditSource::User)).await
}

/// Runs the calculated fields in this order (`nodes` are the `node` of each field; unlisted calculated fields follow).
#[tauri::command]
pub async fn forms_calc_order(eng: Eng<'_>, doc: DocId, nodes: Vec<i32>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || calc_order_one(&eng, doc, &nodes, EditSource::User)).await
}

/// What a format makes of a sample entry, for the live preview beside the Format pickers. An empty `sample` takes a default
/// one. Null for no format and for a custom script.
#[tauri::command]
pub fn forms_format_preview(format: Format, sample: String) -> Option<FormatPreview> {
    af::format_preview(&format, &sample).map(|(sample, shown)| FormatPreview { sample, shown })
}

#[derive(Serialize)]
pub struct FormatPreview {
    pub sample: String,
    pub shown: String,
}

/// The page's widgets in this order (widget xrefs); others follow.
#[tauri::command]
pub async fn forms_tab_order(eng: Eng<'_>, doc: DocId, page: u32, order: Vec<i32>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || tab_order_one(&eng, doc, page, &order, EditSource::User)).await
}

/// `by`: "row" or "column".
#[tauri::command]
pub async fn forms_arrange(eng: Eng<'_>, doc: DocId, page: u32, by: String) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || arrange_one(&eng, doc, page, by != "column", EditSource::User)).await
}

/// Where the page expects answers (nothing is changed).
#[tauri::command]
pub async fn forms_detect(eng: Eng<'_>, doc: DocId, page: u32) -> Result<Vec<Detected>> {
    let eng = eng.inner().clone();
    blocking(move || detect_one(&eng, doc, page)).await
}
