//! Tauri commands for preflight. The UI is the user; the assistant calls the functions in `super`.

use super::{Claim, Convert, Converted, Profile, Progress, Report, check_doc, claim_doc, convert_in_place, convert_to_file, report_bytes};
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

/// Runs the profile's rules off the UI thread; `on_progress` gets one event per page.
/// `tool_cancel(job)` stops the run between pages.
#[tauri::command]
pub async fn preflight_check(eng: Eng<'_>, jobs: Jobs<'_>, job: u64, doc: DocId, profile: Profile, on_progress: Channel<Progress>) -> Result<Report> {
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        check_doc(&eng, doc, profile, &mut progress, &guard.cancel)
    })
    .await
}

/// What the document's XMP claims (cheap): the badge in Document properties.
#[tauri::command]
pub async fn preflight_claim(eng: Eng<'_>, doc: DocId) -> Result<Option<Claim>> {
    let eng = eng.inner().clone();
    blocking(move || claim_doc(&eng, doc)).await
}

/// Converts the open document as one undo step (the checker's fix groups the person ticked).
#[tauri::command]
pub async fn preflight_fix(eng: Eng<'_>, jobs: Jobs<'_>, job: u64, doc: DocId, profile: Profile, convert: Convert, on_progress: Channel<Progress>) -> Result<EditState> {
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        convert_in_place(&eng, doc, profile, &convert, EditSource::User, &mut progress, &guard.cancel)
    })
    .await
}

/// Converts into the new file of save token `to` (from `pick_save`); the open document stays as it is.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn preflight_convert(
    window: Window,
    eng: Eng<'_>,
    reg: tauri::State<'_, Paths>,
    jobs: Jobs<'_>,
    job: u64,
    doc: DocId,
    profile: Profile,
    convert: Convert,
    to: String,
    on_progress: Channel<Progress>,
) -> Result<Converted> {
    let dest = reg.take_save(window.label(), &to)?;
    // The conversion replaces what is at `dest`: never a document that is open (a later save would corrupt it).
    eng.check_not_open(&dest, None)?;
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        let done = convert_to_file(&eng, doc, profile, &convert, &dest, &mut progress, &guard.cancel)?;
        window.state::<Paths>().learn(&dest);
        Ok(done)
    })
    .await
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum ReportFormat {
    Html,
    Pdf,
}

/// Checks the document against `profile` again and writes the report to the file of save token `to`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn preflight_export_report(
    window: Window,
    eng: Eng<'_>,
    reg: tauri::State<'_, Paths>,
    jobs: Jobs<'_>,
    job: u64,
    doc: DocId,
    profile: Profile,
    format: ReportFormat,
    date: String,
    to: String,
    on_progress: Channel<Progress>,
) -> Result<String> {
    let dest = reg.take_save(window.label(), &to)?;
    eng.check_not_open(&dest, None)?;
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        let bytes = report_bytes(&eng, doc, profile, matches!(format, ReportFormat::Pdf), &date, &mut progress, &guard.cancel)?;
        crate::history_ipc::write_replace(std::path::Path::new(&dest), &bytes).map_err(|e| EngineError::Io(e.to_string()))?;
        window.state::<Paths>().learn(&dest);
        Ok(dest)
    })
    .await
}
