pub mod ai;
pub mod annot;
pub mod compress;
pub mod diff;
pub mod engine;
pub mod history;
pub mod history_ipc;
pub mod ocr;
pub mod paths;
pub mod recents;
pub mod redact;
pub mod tools;

use engine::cache::TileKey;
use engine::edit::{EditOp, EditState, SplitResult};
use engine::{DocId, Engine, EngineError, EngineStats, OpenInfo, OutlineItem, SearchEvent, TextLine, TileOutcome};
use history_ipc::History;
use paths::{FileRef, Paths};
use serde::Serialize;
use std::sync::Arc;
use tauri::ipc::{Channel, Response};
use tauri::{Emitter, Manager, State, Window};
use tauri_plugin_dialog::DialogExt;

type Eng<'a> = State<'a, Arc<Engine>>;
type Reg<'a> = State<'a, Paths>;

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum OpenOutcome {
    Opened(OpenInfo),
    NeedsPassword { doc: DocId, path: String },
}

/// Runs blocking engine work off the async runtime's worker threads. A panic in `f` becomes an
/// error the UI's promise rejects with, not a dead command.
async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R, EngineError> + Send + 'static) -> Result<R, EngineError> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

// ---- files: the UI names them by token only (see paths.rs) -------------------------------------

const PDF: &[&str] = &["pdf"];

/// Dialog titles the UI may ask for (the text is Rust's, never the webview's).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum PickTitle {
    Locate,
    Compare,
}

/// Open dialog in Rust; the picked files come back as open tokens.
#[tauri::command]
async fn pick_open(window: Window, multiple: bool, title: Option<PickTitle>) -> Result<Vec<FileRef>, EngineError> {
    blocking(move || {
        let mut d = window.dialog().file().set_parent(&window).add_filter("PDF documents", PDF);
        match title {
            Some(PickTitle::Locate) => d = d.set_title("Locate the moved file"),
            Some(PickTitle::Compare) => d = d.set_title("Choose two PDFs to compare"),
            None => {}
        }
        let picked = if multiple { d.blocking_pick_files() } else { d.blocking_pick_file().map(|f| vec![f]) };
        let reg = window.state::<Paths>();
        Ok(picked
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.into_path().ok())
            .map(|p| reg.issue_open(window.label(), p.to_string_lossy().into_owned()))
            .collect())
    })
    .await
}

/// Save dialog in Rust, starting in the folder of open document `doc` or of open token `near`,
/// with `name` (a file name, nothing more) filled in. The result is a one-use save token.
#[tauri::command]
async fn pick_save(
    window: Window,
    eng: Eng<'_>,
    doc: Option<DocId>,
    near: Option<String>,
    name: String,
) -> Result<Option<FileRef>, EngineError> {
    let near = match (doc, near) {
        (Some(doc), _) => Some(eng.doc_entry(doc)?.path()),
        (None, Some(t)) => Some(window.state::<Paths>().open(window.label(), &t)?),
        _ => None,
    };
    blocking(move || {
        let name = std::path::Path::new(&name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut d = window.dialog().file().set_parent(&window).add_filter("PDF documents", PDF).set_file_name(name);
        if let Some(dir) = near.as_deref().and_then(|p| std::path::Path::new(p).parent()) {
            d = d.set_directory(dir);
        }
        let picked = d.blocking_save_file().and_then(|f| f.into_path().ok());
        Ok(picked.map(|p| window.state::<Paths>().issue_save(window.label(), p.to_string_lossy().into_owned())))
    })
    .await
}

/// An open token for a file the user chose before (recent list, Locate): Rust checks it knows the path.
#[tauri::command]
async fn known_file(window: Window, path: String) -> Result<FileRef, EngineError> {
    blocking(move || window.state::<Paths>().known_file(window.label(), path)).await
}

#[tauri::command]
async fn open_document(window: Window, app: tauri::AppHandle, eng: Eng<'_>, reg: Reg<'_>, hist: State<'_, Arc<History>>, file: String) -> Result<OpenOutcome, EngineError> {
    let path = reg.open(window.label(), &file)?;
    let eng = eng.inner().clone();
    let r = blocking(move || {
        let path = recents::canonical(&path);
        // the recent list stores this spelling
        window.state::<Paths>().learn(&path);
        let (doc, r) = eng.open_pending(&path);
        match r {
            Ok(info) => Ok(OpenOutcome::Opened(info)),
            Err(EngineError::NeedsPassword) => Ok(OpenOutcome::NeedsPassword { doc, path }),
            Err(e) => Err(e),
        }
    })
    .await;
    // History is resolved in the background after the open; its errors never reach this command.
    if let Ok(OpenOutcome::Opened(info)) = &r {
        hist.inner().spawn_open(app, info.doc, info.path.clone());
    }
    r
}

#[tauri::command]
async fn authenticate(app: tauri::AppHandle, eng: Eng<'_>, hist: State<'_, Arc<History>>, doc: DocId, password: String) -> Result<OpenInfo, EngineError> {
    let eng = eng.inner().clone();
    let r = blocking(move || eng.authenticate(doc, &password)).await;
    if let Ok(info) = &r {
        hist.inner().spawn_open(app, info.doc, info.path.clone());
    }
    r
}

#[tauri::command]
fn close_document(eng: Eng<'_>, hist: State<'_, Arc<History>>, doc: DocId) {
    hist.forget(doc);
    eng.close(doc);
}

#[derive(Serialize, Clone)]
struct SizesChunk {
    start: u32,
    sizes: Vec<[f32; 2]>,
}

/// Streams every page's size (in chunks) so the UI can lay out a long document accurately.
#[tauri::command]
fn page_sizes(eng: Eng<'_>, doc: DocId, on_chunk: Channel<SizesChunk>) -> Result<(), EngineError> {
    eng.inner().scan_sizes(
        doc,
        Arc::new(move |start, sizes| {
            let _ = on_chunk.send(SizesChunk { start, sizes });
        }),
    )
}

#[tauri::command]
async fn outline(eng: Eng<'_>, doc: DocId) -> Result<Vec<OutlineItem>, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.call(engine::pool::class::INTERACTIVE, move |e| e.outline(doc))).await
}

#[tauri::command]
async fn page_text(eng: Eng<'_>, doc: DocId, page: u32) -> Result<Vec<TextLine>, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.call(engine::pool::class::INTERACTIVE, move |e| e.page_text(doc, page))).await
}

#[tauri::command]
fn search(eng: Eng<'_>, doc: DocId, query: String, on_event: Channel<SearchEvent>) -> Result<u64, EngineError> {
    eng.inner().search(
        doc,
        query,
        Arc::new(move |e| {
            let _ = on_event.send(e);
        }),
    )
}

#[tauri::command]
fn cancel_search(eng: Eng<'_>, doc: DocId) {
    eng.cancel_search(doc);
}

// ---- editing (see engine::edit) ----------------------------------------------------------------

/// Swaps the file token of an insert for its path.
pub(crate) fn resolve_op(reg: &Paths, window: &str, op: &mut EditOp) -> Result<(), EngineError> {
    if let EditOp::InsertFile { path, .. } = op {
        *path = reg.open(window, path)?;
    }
    Ok(())
}

#[tauri::command]
async fn edit_apply(window: Window, eng: Eng<'_>, reg: Reg<'_>, doc: DocId, mut op: EditOp) -> Result<EditState, EngineError> {
    resolve_op(&reg, window.label(), &mut op)?;
    let eng = eng.inner().clone();
    blocking(move || eng.apply(doc, op)).await
}

#[tauri::command]
async fn edit_undo(eng: Eng<'_>, doc: DocId) -> Result<EditState, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.undo(doc)).await
}

#[tauri::command]
async fn edit_redo(eng: Eng<'_>, doc: DocId) -> Result<EditState, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.redo(doc)).await
}

/// `to`: a save token (Save as); omitted: save over the document's own file.
#[tauri::command]
async fn save_document(
    window: Window,
    app: tauri::AppHandle,
    eng: Eng<'_>,
    reg: Reg<'_>,
    hist: State<'_, Arc<History>>,
    doc: DocId,
    to: Option<String>,
) -> Result<EditState, EngineError> {
    let to = to.map(|t| reg.take_save(window.label(), &t)).transpose()?;
    let (eng, hist) = (eng.inner().clone(), hist.inner().clone());
    blocking(move || {
        let (state, saved) = eng.save_recorded(doc, to.as_deref())?;
        if let Some(to) = &to {
            window.state::<Paths>().learn(to);
        }
        match saved {
            // Save as: the document is another file now, with a history of its own
            Some(s) if s.moved => hist.spawn_open(app, doc, state.path.clone()),
            // history errors never fail a save
            Some(s) => match hist.saved(doc, &s) {
                Ok(_) => drop(app.emit("history-changed", doc)),
                Err(e) => eprintln!("inkwell history: {e}"),
            },
            None => {}
        }
        Ok(state)
    })
    .await
}

/// `out`: a save token.
#[tauri::command]
async fn extract_pages(
    window: Window,
    eng: Eng<'_>,
    reg: Reg<'_>,
    doc: DocId,
    pages: Vec<u32>,
    out: String,
    delete_after: bool,
) -> Result<EditState, EngineError> {
    let out = reg.take_save(window.label(), &out)?;
    let eng = eng.inner().clone();
    blocking(move || {
        let s = eng.extract(doc, &pages, &out, delete_after)?;
        window.state::<Paths>().learn(&out);
        Ok(s)
    })
    .await
}

#[tauri::command]
async fn split_document(eng: Eng<'_>, doc: DocId, every: u32) -> Result<SplitResult, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.split(doc, every)).await
}

/// `files`: open tokens, in order; `out`: a save token. Returns an open token for the result.
#[tauri::command]
async fn merge_files(window: Window, eng: Eng<'_>, reg: Reg<'_>, files: Vec<String>, out: String) -> Result<FileRef, EngineError> {
    let paths = files.iter().map(|t| reg.open(window.label(), t)).collect::<Result<Vec<_>, _>>()?;
    let out = reg.take_save(window.label(), &out)?;
    let eng = eng.inner().clone();
    blocking(move || {
        eng.merge(&paths, &out)?;
        Ok(window.state::<Paths>().issue_open(window.label(), out))
    })
    .await
}

// ---- compress and OCR (see tools) ----------------------------------------------------------------

type JobState<'a> = State<'a, Arc<tools::Jobs>>;

async fn tool_blocking<R: Send + 'static>(
    f: impl FnOnce() -> Result<R, tools::ToolError> + Send + 'static,
) -> Result<R, tools::ToolError> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or_else(|_| Err(tools::ToolError("the tool crashed".into())))
}

#[tauri::command]
async fn compress_audit(eng: Eng<'_>, doc: DocId) -> Result<tools::AuditReply, tools::ToolError> {
    let eng = eng.inner().clone();
    tool_blocking(move || tools::audit_for_ui(&eng, doc)).await
}

/// `job`: an id the UI picks, for `tool_cancel`.
#[tauri::command]
async fn compress_estimate(
    eng: Eng<'_>,
    jobs: JobState<'_>,
    job: u64,
    doc: DocId,
) -> Result<Vec<compress::estimate::Estimate>, tools::ToolError> {
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    tool_blocking(move || tools::estimate_for_ui(&eng, doc, &guard.cancel)).await
}

/// `job`: an id the UI picks, for `tool_cancel`. `req.save_to`: a save token from `pick_save`.
#[tauri::command]
async fn compress_start(
    window: Window,
    eng: Eng<'_>,
    reg: Reg<'_>,
    jobs: JobState<'_>,
    job: u64,
    req: tools::CompressRequest,
    on_progress: Channel<compress::Progress>,
) -> Result<compress::Report, tools::ToolError> {
    let output = req.save_to.as_deref().map(|t| reg.take_save(window.label(), t)).transpose()?.map(std::path::PathBuf::from);
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    tool_blocking(move || {
        let r = tools::run_compress(&eng, &req, output, &guard.cancel, &mut |p| {
            let _ = on_progress.send(p);
        })?;
        window.state::<Paths>().learn(&r.output.to_string_lossy());
        Ok(r)
    })
    .await
}

#[tauri::command]
fn tool_cancel(jobs: JobState<'_>, job: u64) {
    jobs.cancel(job);
}

type Tess<'a> = State<'a, Arc<tools::paths::ToolPaths>>;

#[tauri::command]
async fn ocr_languages(paths: Tess<'_>) -> Result<tools::OcrLanguages, tools::ToolError> {
    let configured = paths.tesseract();
    tool_blocking(move || Ok(tools::ocr_languages(configured.as_deref()))).await
}

/// Shows a file picker for tesseract.exe and remembers the choice. Returns it for display.
#[tauri::command]
async fn ocr_choose_tesseract(app: tauri::AppHandle, paths: Tess<'_>) -> Result<Option<String>, tools::ToolError> {
    let paths = paths.inner().clone();
    tool_blocking(move || {
        let Some(p) = tools::paths::choose_tesseract(&app) else { return Ok(None) };
        paths.set_tesseract(Some(p.clone())).map_err(tools::ToolError)?;
        Ok(Some(p.to_string_lossy().into_owned()))
    })
    .await
}

#[tauri::command]
fn ocr_clear_tesseract(paths: Tess<'_>) -> Result<(), tools::ToolError> {
    paths.set_tesseract(None).map_err(tools::ToolError)
}

/// `req.save_to`: a save token from `pick_save`; none = change the open document.
#[tauri::command]
async fn ocr_start(
    window: Window,
    eng: Eng<'_>,
    reg: Reg<'_>,
    jobs: JobState<'_>,
    paths: Tess<'_>,
    job: u64,
    req: tools::OcrRequest,
    on_progress: Channel<ocr::Progress>,
) -> Result<tools::OcrOutcome, tools::ToolError> {
    let save_as = req.save_to.as_deref().map(|t| reg.take_save(window.label(), t)).transpose()?.map(std::path::PathBuf::from);
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    let tesseract = paths.tesseract();
    tool_blocking(move || {
        let backend = tools::make_backend(req.backend, tesseract.as_deref())?;
        let r = tools::run_ocr(
            &eng,
            &req,
            save_as.as_deref(),
            backend.as_ref(),
            &|p| {
                let _ = on_progress.send(p);
            },
            &guard.cancel,
        )?;
        if let Some(out) = &r.output {
            window.state::<Paths>().learn(out);
        }
        Ok(r)
    })
    .await
}

#[tauri::command]
fn engine_stats(eng: Eng<'_>) -> EngineStats {
    eng.stats()
}

#[tauri::command]
fn set_cache_budget(eng: Eng<'_>, megabytes: usize) {
    eng.set_cache_budget(megabytes << 20);
}

/// Files passed on the command line (and, later, by the Windows file association).
#[tauri::command]
fn initial_files(window: Window, reg: Reg<'_>) -> Vec<FileRef> {
    std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-') && std::path::Path::new(a).is_file())
        .map(|a| reg.issue_open(window.label(), a))
        .collect()
}

/// Files dropped on a window from Explorer: open tokens for the PDFs, sent to that window.
fn on_drop(window: &Window, paths: &[std::path::PathBuf]) {
    let reg = window.state::<Paths>();
    let refs: Vec<FileRef> = paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")))
        .map(|p| reg.issue_open(window.label(), p.to_string_lossy().into_owned()))
        .collect();
    if !refs.is_empty() {
        let _ = window.emit_to(window.label(), "files-dropped", refs);
    }
}

/// Same tile as the `inkwell://` protocol, delivered through the IPC channel. Kept so the
/// transport can be benchmarked head to head; the UI uses the protocol.
#[tauri::command]
async fn tile_raw(
    eng: Eng<'_>,
    doc: DocId,
    page: u32,
    rot: u8,
    scale: u32,
    tx: u16,
    ty: u16,
) -> Result<Response, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || {
        let entry = eng.doc_entry(doc)?;
        let rev = entry.generation();
        let t = eng.render_tile(&entry, &TileKey { doc, rev, page, rot, scale, tx, ty })?;
        Ok(Response::new(t.rgba.as_ref().clone()))
    })
    .await
}

/// `/t/{doc}/{page}/{rot}/{scale}/{tx}/{ty}?e={epoch}&r={rank}&s={stream}&g={generation}` -> raw RGBA with
/// `x-tile-w` / `x-tile-h` headers. 410 means "superseded, ignore".
fn parse_tile_request(uri: &tauri::http::Uri) -> Option<(TileKey, usize, u64, u32)> {
    let mut parts = uri.path().trim_start_matches('/').split('/');
    if parts.next()? != "t" {
        return None;
    }
    let mut n = || parts.next()?.parse::<u32>().ok();
    let mut key = TileKey {
        doc: n()?,
        rev: 0,
        page: n()?,
        rot: n()? as u8,
        scale: n()?,
        tx: n()? as u16,
        ty: n()? as u16,
    };
    let (mut epoch, mut rank, mut stream) = (0u64, 0u32, 0usize);
    for kv in uri.query().unwrap_or("").split('&') {
        match kv.split_once('=') {
            Some(("e", v)) => epoch = v.parse().unwrap_or(0),
            Some(("r", v)) => rank = v.parse().unwrap_or(0),
            Some(("s", v)) => stream = v.parse().unwrap_or(0),
            Some(("g", v)) => key.rev = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    Some((key, stream, epoch, rank))
}

fn reply(status: u16, body: Vec<u8>, dims: Option<(u32, u32)>) -> tauri::http::Response<std::borrow::Cow<'static, [u8]>> {
    let mut b = tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", "application/octet-stream")
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Expose-Headers", "x-tile-w, x-tile-h")
        .header("Cache-Control", "no-store");
    if let Some((w, h)) = dims {
        b = b.header("x-tile-w", w.to_string()).header("x-tile-h", h.to_string());
    }
    b.body(std::borrow::Cow::Owned(body)).unwrap()
}

pub fn cache_budget_bytes() -> usize {
    std::env::var("INKWELL_CACHE_MB").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(64) << 20
}

/// WebView2's own shortcuts (Ctrl+R reload, Ctrl+F find, Ctrl+=/-/0 browser zoom, F5, Ctrl+P ...)
/// would fight the app's; the UI handles every shortcut itself.
#[cfg(windows)]
fn disable_browser_accelerators(app: &tauri::App) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows_core::Interface;
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.with_webview(|wv| unsafe {
            if let Ok(core) = wv.controller().CoreWebView2() {
                if let Ok(settings) = core.Settings() {
                    if let Ok(s3) = settings.cast::<ICoreWebView2Settings3>() {
                        let _ = s3.SetAreBrowserAcceleratorKeysEnabled(false);
                    }
                }
            }
        });
    }
}

pub fn run() {
    let engine = Engine::new(Engine::default_workers(), cache_budget_bytes());
    let proto_engine = engine.clone();
    let hist = Arc::new(History::new());
    let h = hist.clone();
    // a saved redaction then replaces the versions from before it (history_ipc::History::saved)
    engine::edit::set_redaction_hook(move |doc| h.on_redactions_applied(doc));
    let autosave = (hist.clone(), engine.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(engine)
        .manage(ai::Assistant::new())
        .manage(Paths::default())
        .manage(Arc::new(tools::Jobs::default()))
        .manage(hist)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                on_drop(window, paths);
            }
        })
        .register_asynchronous_uri_scheme_protocol("inkwell", move |_ctx, request, responder| {
            let Some((key, stream, epoch, rank)) = parse_tile_request(request.uri()) else {
                return responder.respond(reply(404, Vec::new(), None));
            };
            proto_engine.request_tile(
                key,
                stream,
                epoch,
                rank,
                Box::new(move |outcome| {
                    responder.respond(match outcome {
                        TileOutcome::Ready(t) => reply(200, t.rgba.as_ref().clone(), Some((t.w, t.h))),
                        TileOutcome::Stale => reply(410, Vec::new(), None),
                        TileOutcome::Failed(e) => reply(500, e.into_bytes(), None),
                    })
                }),
            );
        })
        .invoke_handler(tauri::generate_handler![
            pick_open,
            pick_save,
            known_file,
            open_document,
            authenticate,
            close_document,
            page_sizes,
            outline,
            page_text,
            search,
            cancel_search,
            engine_stats,
            set_cache_budget,
            initial_files,
            tile_raw,
            edit_apply,
            edit_undo,
            edit_redo,
            history_ipc::history_state,
            history_ipc::history_versions,
            history_ipc::history_check,
            history_ipc::history_restore,
            history_ipc::history_recover,
            history_ipc::history_save_copy,
            history_ipc::history_name,
            history_ipc::history_confirm_same,
            history_ipc::history_delete,
            history_ipc::history_never_keep,
            history_ipc::history_compare,
            history_ipc::history_render,
            history_ipc::history_visual_diff,
            history_ipc::history_checkpoint,
            save_document,
            extract_pages,
            split_document,
            merge_files,
            compress_audit,
            compress_estimate,
            compress_start,
            tool_cancel,
            ocr_languages,
            ocr_choose_tesseract,
            ocr_clear_tesseract,
            ocr_start,
            annot::app::annot_list,
            annot::app::annot_add,
            annot::app::annot_edit,
            annot::app::annot_delete,
            annot::app::annot_reply,
            annot::app::annot_state,
            redact::app::redact_find,
            redact::app::redact_mark,
            redact::app::redact_unmark,
            redact::app::redact_list_marks,
            redact::app::redact_options,
            redact::app::redact_apply,
            recents::stat_files,
            recents::recents_load,
            recents::recents_save,
            recents::reveal_in_folder,
            ai::ai_status,
            ai::ai_prepare,
            ai::ai_ask,
            ai::ai_cancel,
            ai::ai_tool_decision,
            ai::ai_set_permissions,
            ai::ai_close,
        ])
        .setup(move |app| {
            #[cfg(windows)]
            disable_browser_accelerators(app);
            autosave.0.spawn_autosave(app.handle().clone(), autosave.1.clone());
            if let Some(json) = recents::data_dir(app.handle()).ok().and_then(|(d, _)| recents::read_json(&d)) {
                app.state::<Paths>().learn_recents(&json);
            }
            app.manage(Arc::new(tools::paths::ToolPaths::load(app.handle())));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Inkwell");
}

