//! Tauri commands for the accessibility checker. The UI is the user; the assistant calls the
//! functions in `super` with its turn.

use super::autotag::Preview;
use super::{Fix, Progress, Report, check_doc, fix_doc_with, preview_doc, report_bytes};
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
/// path. `date` is display text for the report's header.
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
    on_progress: Channel<Progress>,
) -> Result<String> {
    let dest = reg.take_save(window.label(), &to)?;
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        let bytes = report_bytes(&eng, doc, matches!(format, ReportFormat::Pdf), &date, &mut progress, &guard.cancel)?;
        crate::history_ipc::write_replace(std::path::Path::new(&dest), &bytes).map_err(|e| EngineError::Io(e.to_string()))?;
        window.state::<Paths>().learn(&dest);
        Ok(dest)
    })
    .await
}
