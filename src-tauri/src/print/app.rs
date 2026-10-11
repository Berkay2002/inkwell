//! Tauri commands for printing (the logic is in the parent module; the commands only resolve tokens
//! and move work off the async runtime). Windows only, like the rest of the app.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::ipc::{Channel, Response};
use tauri::{Manager, State, Window};

use super::{pdf, PlanInfo, Progress, Report, Settings, win};
use crate::engine::{DocId, Engine};
use crate::paths::{FileRef, Paths};
use crate::tools::{Jobs, ToolError};

type Eng<'a> = State<'a, Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R, ToolError> + Send + 'static) -> Result<R, ToolError> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or_else(|_| Err(ToolError("printing crashed".into())))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Printers {
    pub printers: Vec<win::Printer>,
    /// Set by `INKWELL_PRINT_DIR`: print-to-file printers write there without asking (automation).
    pub file_dir_set: bool,
}

fn print_dir() -> Option<PathBuf> {
    std::env::var_os("INKWELL_PRINT_DIR").filter(|d| !d.is_empty()).map(PathBuf::from)
}

/// The installed printers (the default first) and whether file printers need a destination from the user.
#[tauri::command]
pub async fn print_printers() -> Result<Printers, ToolError> {
    blocking(|| Ok(Printers { printers: win::printers(), file_dir_set: print_dir().is_some() })).await
}

/// What a printer's driver offers: paper sizes, trays, two-sided, colour. None when it does not answer.
#[tauri::command]
pub async fn print_capabilities(printer: String) -> Result<Option<win::Capabilities>, ToolError> {
    blocking(move || Ok(win::capabilities(&printer))).await
}

#[tauri::command]
pub async fn print_plan(eng: Eng<'_>, doc: DocId, settings: Settings) -> Result<PlanInfo, ToolError> {
    let eng = eng.inner().clone();
    blocking(move || Ok(super::plan_info(&eng, doc, &settings)?)).await
}

/// PNG bytes of side `sheet` (0-based) of the plan, `max_px` on its longer edge.
#[tauri::command]
pub async fn print_preview(eng: Eng<'_>, doc: DocId, settings: Settings, sheet: u32, max_px: u32) -> Result<Response, ToolError> {
    let eng = eng.inner().clone();
    let png = blocking(move || Ok(super::preview_png(&eng, doc, &settings, sheet, max_px)?)).await?;
    Ok(Response::new(png))
}

/// Prints. `job` is an id the UI picks, for `tool_cancel`. `save_to`: a save token from `pick_save`, for
/// printers that print to a file.
#[tauri::command]
pub async fn print_start(
    window: Window,
    eng: Eng<'_>,
    reg: State<'_, Paths>,
    jobs: State<'_, Arc<Jobs>>,
    job: u64,
    doc: DocId,
    settings: Settings,
    save_to: Option<String>,
    on_progress: Channel<Progress>,
) -> Result<Report, ToolError> {
    let mut output = save_to.as_deref().map(|t| reg.take_save(window.label(), t)).transpose()?.map(PathBuf::from);
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    let title = eng
        .doc_entry(doc)
        .ok()
        .and_then(|e| std::path::Path::new(&e.path()).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Inkwell".into());
    blocking(move || {
        if output.is_none() {
            // Automation: a print-to-file printer writes into INKWELL_PRINT_DIR instead of asking.
            if let Some(dir) = print_dir() {
                let to_file = win::printers()
                    .iter()
                    .find(|p| settings.printer.as_deref().map_or(p.is_default, |n| n == p.name))
                    .is_some_and(|p| p.to_file);
                if to_file {
                    let stem = std::path::Path::new(&title).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    output = Some(dir.join(format!("{stem}-print.pdf")));
                }
            }
        }
        let r = super::run(&eng, doc, &settings, &title, output.clone(), &guard.cancel, &mut |p| {
            let _ = on_progress.send(p);
        })?;
        if let Some(o) = &output {
            window.state::<Paths>().learn(&o.to_string_lossy());
        }
        Ok(r)
    })
    .await
}

/// What Save as PDF made: the new file (an open token, for a new tab) and its size.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPdf {
    pub file: FileRef,
    pub pages: u32,
    pub sheets: u32,
}

/// Writes the plan as a new PDF to `save_to` (a save token from `pick_save`). Needs no printer.
/// `job` is an id the UI picks, for `tool_cancel`; `on_progress` counts sheets written.
#[tauri::command]
pub async fn print_save_pdf(
    window: Window,
    eng: Eng<'_>,
    reg: State<'_, Paths>,
    jobs: State<'_, Arc<Jobs>>,
    job: u64,
    doc: DocId,
    settings: Settings,
    save_to: String,
    on_progress: Channel<Progress>,
) -> Result<SavedPdf, ToolError> {
    let out = reg.take_save(window.label(), &save_to)?;
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let out = PathBuf::from(out);
        let r = pdf::save(&eng, doc, &settings, &out, &guard.cancel, &mut |p| {
            let _ = on_progress.send(p);
        })?;
        let file = window.state::<Paths>().issue_open(window.label(), out.to_string_lossy().into_owned());
        Ok(SavedPdf { file, pages: r.pages, sheets: r.sheets })
    })
    .await
}
