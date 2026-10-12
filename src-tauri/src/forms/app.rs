//! Tauri commands for forms. The UI is the user; the assistant calls the functions in `super` with its turn.

use super::{
    Axis, Copied, Detected, Dim, Edge, FieldPatch, FieldValue, FormDto, NewField, ScriptsState, Stepped, add_many, add_one, arrange_one, calc_order_one, delete_many, detect_one, edit_one, list_all,
    move_one, reset_report, scripts_enable, scripts_state, set_value_report, tab_order_one,
    align_one, center_one, copies_one, distribute_one, duplicate_one, match_size_one, nudge_one, rename_one,
};
use super::af::{self, Format};
use super::doc_js::{self, Applied, Check, DocJs, Op, Source};
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

// ---- layout tools: one undo step each. `items` are `(page, xref)` pairs; the last one is the anchor. ----

#[tauri::command]
pub async fn forms_align(eng: Eng<'_>, doc: DocId, items: Vec<(u32, i32)>, edge: Edge) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || align_one(&eng, doc, &items, edge, EditSource::User)).await
}

#[tauri::command]
pub async fn forms_center(eng: Eng<'_>, doc: DocId, items: Vec<(u32, i32)>, axis: Axis) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || center_one(&eng, doc, &items, axis, EditSource::User)).await
}

#[tauri::command]
pub async fn forms_distribute(eng: Eng<'_>, doc: DocId, items: Vec<(u32, i32)>, axis: Axis) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || distribute_one(&eng, doc, &items, axis, EditSource::User)).await
}

#[tauri::command]
pub async fn forms_match_size(eng: Eng<'_>, doc: DocId, items: Vec<(u32, i32)>, dim: Dim) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || match_size_one(&eng, doc, &items, dim, EditSource::User)).await
}

/// Moves the selection by `dx`, `dy` points on the page (the arrow keys; one step per burst).
#[tauri::command]
pub async fn forms_nudge(eng: Eng<'_>, doc: DocId, items: Vec<(u32, i32)>, dx: f32, dy: f32) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || nudge_one(&eng, doc, &items, dx, dy, EditSource::User)).await
}

/// The same field on other pages (0-based).
#[tauri::command]
pub async fn forms_duplicate(eng: Eng<'_>, doc: DocId, item: (u32, i32), pages: Vec<u32>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || duplicate_one(&eng, doc, item, &pages, EditSource::User)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopiesMade {
    pub state: EditState,
    #[serde(flatten)]
    pub copied: Copied,
}

/// A grid of copies: `rows` x `cols`, `gap_x` and `gap_y` points apart.
#[tauri::command]
pub async fn forms_copies(eng: Eng<'_>, doc: DocId, item: (u32, i32), rows: u32, cols: u32, gap_x: f32, gap_y: f32) -> Result<CopiesMade> {
    let eng = eng.inner().clone();
    blocking(move || {
        let (state, copied) = copies_one(&eng, doc, item, rows, cols, (gap_x, gap_y), EditSource::User)?;
        Ok(CopiesMade { state, copied })
    })
    .await
}

/// The field's full (dotted) name. `merge` joins it to the field that has that name when both are of one kind.
#[tauri::command]
pub async fn forms_rename(eng: Eng<'_>, doc: DocId, item: (u32, i32), name: String, merge: bool) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || rename_one(&eng, doc, item, &name, merge, EditSource::User)).await
}

// ---- document JavaScript (the sheet; see doc_js.rs). Not an assistant tool. ----

/// Every script the file carries: named, document and page actions in full, field/link/bookmark scripts counted.
#[tauri::command]
pub async fn doc_js_list(eng: Eng<'_>, doc: DocId) -> Result<DocJs> {
    let eng = eng.inner().clone();
    blocking(move || doc_js::list_all(&eng, doc)).await
}

/// The text of one script (`id` from the list).
#[tauri::command]
pub async fn doc_js_source(eng: Eng<'_>, doc: DocId, id: String) -> Result<Source> {
    let eng = eng.inner().clone();
    blocking(move || doc_js::source_of(&eng, doc, &id)).await
}

/// Whether `source` parses. Compiles only; nothing is run.
#[tauri::command]
pub async fn doc_js_check(source: String) -> Result<Check> {
    blocking(move || Ok(doc_js::check(&source))).await
}

/// One edit of the file's scripts: one undo step.
#[tauri::command]
pub async fn doc_js_apply(eng: Eng<'_>, doc: DocId, op: Op) -> Result<Applied> {
    let eng = eng.inner().clone();
    blocking(move || doc_js::apply(&eng, doc, &op, EditSource::User)).await
}
