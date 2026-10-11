//! Tauri commands for the accessibility checker. The UI is the user; the assistant calls the
//! functions in `super` with its turn.

use super::{Fix, Progress, Report, check_doc, fix_doc};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use tauri::ipc::Channel;

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;
type Jobs<'a> = tauri::State<'a, std::sync::Arc<crate::tools::Jobs>>;

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

/// Applies one automatic fix (or Autotag) as one undo step.
#[tauri::command]
pub async fn a11y_fix(eng: Eng<'_>, doc: DocId, fix: Fix) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || fix_doc(&eng, doc, &fix, EditSource::User)).await
}
