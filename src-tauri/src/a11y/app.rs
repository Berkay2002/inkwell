//! Tauri commands for the accessibility checker. The UI is the user; the assistant calls the
//! functions in `super` with its turn.

use super::autotag::Preview;
use super::order::ReadingOrder;
use super::report::Overrides;
use super::tags::{TagNode, TagPatch};
use super::{
    Fix, Problem, Progress, Protection, Report, artifact_doc, check_doc, edit_tag_doc, fix_doc_with, merge_doc, merge_many_doc, move_tag_doc, preview_doc, protection_doc, reading_order_doc, reorder_doc,
    report_bytes, set_role_doc, split_doc, tag_problems_doc, tags_doc,
};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::paths::Paths;
use serde::Deserialize;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{Manager, Window};

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, Arc<Engine>>;
type Jobs<'a> = tauri::State<'a, Arc<crate::tools::Jobs>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// Runs the full check off the UI thread; `on_progress` gets one event per page scanned.
/// `job`: an id the UI picks; `tool_cancel(job)` stops the check between pages.
#[tauri::command]
pub async fn a11y_check(eng: Eng<'_>, jobs: Jobs<'_>, job: u64, doc: DocId, on_progress: Channel<Progress>) -> Result<Report> {
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        check_doc(&eng, doc, &mut progress, &guard.cancel)
    })
    .await
}

/// Applies one automatic fix (or Autotag, or a group of fixes) as one undo step. Autotag reports per page through
/// `on_progress`, and `tool_cancel(job)` ends it with "cancelled", leaving the document as it was.
#[tauri::command]
pub async fn a11y_fix(eng: Eng<'_>, jobs: Jobs<'_>, job: u64, doc: DocId, fix: Fix, on_progress: Channel<Progress>) -> Result<EditState> {
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        fix_doc_with(&eng, doc, &fix, EditSource::User, &mut progress, &guard.cancel)
    })
    .await
}

/// What Autotag would do to the document, counted from the file (cheap): shown before it runs.
#[tauri::command]
pub async fn a11y_preview(eng: Eng<'_>, doc: DocId) -> Result<Preview> {
    let eng = eng.inner().clone();
    blocking(move || preview_doc(&eng, doc)).await
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ReportFormat {
    Html,
    Pdf,
}

/// Checks the document again and writes the report to the file of save token `to` (from `pick_save`); returns the
/// path. `date` is display text for the report's header. `overrides` (rule id to `passed` or `skipped`) are the rules
/// the person set aside by hand; they change how the report reads and never the document.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn a11y_export_report(
    window: Window,
    eng: Eng<'_>,
    reg: tauri::State<'_, Paths>,
    jobs: Jobs<'_>,
    job: u64,
    doc: DocId,
    format: ReportFormat,
    date: String,
    to: String,
    overrides: Option<Overrides>,
    on_progress: Channel<Progress>,
) -> Result<String> {
    let dest = reg.take_save(window.label(), &to)?;
    // The report replaces what is at `dest`: never a document that is open (a later save would corrupt it).
    eng.check_not_open(&dest, None)?;
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        let bytes = report_bytes(&eng, doc, matches!(format, ReportFormat::Pdf), &date, &overrides.unwrap_or_default(), &mut progress, &guard.cancel)?;
        crate::history_ipc::write_replace(std::path::Path::new(&dest), &bytes).map_err(|e| EngineError::Io(e.to_string()))?;
        window.state::<Paths>().learn(&dest);
        Ok(dest)
    })
    .await
}

/// Whether the document is certified or signed (cheap, no check needed): the rail disables its edit
/// controls on this.
#[tauri::command]
pub async fn a11y_protection(eng: Eng<'_>, doc: DocId) -> Result<Protection> {
    let eng = eng.inner().clone();
    blocking(move || protection_doc(&eng, doc)).await
}

// ---- reading order and tags ----------------------------------------------------------------------

/// The regions of a page in reading order: role, text preview, box, MCIDs and the elements that hold them,
/// plus marked content no element points at. Reads the document as it is now, unsaved edits included.
#[tauri::command]
pub async fn a11y_reading_order(eng: Eng<'_>, doc: DocId, page: u32) -> Result<ReadingOrder> {
    let eng = eng.inner().clone();
    blocking(move || reading_order_doc(&eng, doc, page)).await
}

/// The tag tree, nested, with each node's role, alternate text, actual text, language, table scope and headers.
/// With `page`, only the branches that hold content on that page.
#[tauri::command]
pub async fn a11y_tags(eng: Eng<'_>, doc: DocId, page: Option<u32>) -> Result<Vec<TagNode>> {
    let eng = eng.inner().clone();
    blocking(move || tags_doc(&eng, doc, page)).await
}

/// Where the ParentTree and the tags disagree, as `[{page, xref, message}]` (empty: they agree). The fix
/// `{"fix": "parentTree"}` of `a11y_fix` rebuilds it.
#[tauri::command]
pub async fn a11y_tag_problems(eng: Eng<'_>, doc: DocId) -> Result<Vec<Problem>> {
    let eng = eng.inner().clone();
    blocking(move || tag_problems_doc(&eng, doc)).await
}

/// Puts the regions of a page in a new reading order (`order`: every region's object number, in the order to
/// read them). One undo step.
#[tauri::command]
pub async fn a11y_reorder(eng: Eng<'_>, doc: DocId, page: u32, order: Vec<i32>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || reorder_doc(&eng, doc, page, &order, EditSource::User)).await
}

/// Changes an element's role (P, H1-H6, Figure, Table, L, LI, ...); `Artifact` marks its content as artifact.
#[tauri::command]
pub async fn a11y_set_role(eng: Eng<'_>, doc: DocId, xref: i32, role: String) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || set_role_doc(&eng, doc, xref, &role, EditSource::User)).await
}

/// Takes an element's content out of the reading order and marks it as artifact.
#[tauri::command]
pub async fn a11y_artifact(eng: Eng<'_>, doc: DocId, xref: i32) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || artifact_doc(&eng, doc, xref, EditSource::User)).await
}

/// Merges region `from` into region `into`.
#[tauri::command]
pub async fn a11y_merge_regions(eng: Eng<'_>, doc: DocId, into: i32, from: i32) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || merge_doc(&eng, doc, into, from, EditSource::User)).await
}

/// Merges the regions `from` into region `into`, in the order given, as one undo step.
#[tauri::command]
pub async fn a11y_merge_many(eng: Eng<'_>, doc: DocId, into: i32, from: Vec<i32>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || merge_many_doc(&eng, doc, into, &from, EditSource::User)).await
}

/// Splits a region before the content item with MCID `at_mcid`; the rest becomes a new region after it
/// (with `role`, else the same role).
#[tauri::command]
pub async fn a11y_split_region(eng: Eng<'_>, doc: DocId, xref: i32, at_mcid: i32, role: Option<String>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || split_doc(&eng, doc, xref, at_mcid, role.as_deref(), EditSource::User)).await
}

/// Edits one tag: `{role?, alt?, actualText?, lang?, scope?, headers?}`; a field left out stays, an empty text
/// removes it. `headers` lists the object numbers of the header cells (TH) a cell is read with.
#[tauri::command]
pub async fn a11y_edit_tag(eng: Eng<'_>, doc: DocId, xref: i32, patch: TagPatch) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || edit_tag_doc(&eng, doc, xref, &patch, EditSource::User)).await
}

/// Moves a tag under `parent` (0: the top of the tree), before its kid `anchor`, or after it with
/// `after`; the last kid when there is no anchor. A move that changes nothing is refused.
#[tauri::command]
pub async fn a11y_move_tag(eng: Eng<'_>, doc: DocId, xref: i32, parent: i32, anchor: Option<i32>, after: bool) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || move_tag_doc(&eng, doc, xref, parent, anchor, after, EditSource::User)).await
}
