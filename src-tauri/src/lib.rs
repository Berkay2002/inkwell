pub mod ai;
pub mod compress;
pub mod engine;
pub mod ocr;
pub mod recents;
pub mod redact;
pub mod tools;

use engine::cache::TileKey;
use engine::edit::{EditOp, EditState, SplitResult};
use engine::{DocId, Engine, EngineError, EngineStats, OpenInfo, OutlineItem, SearchEvent, TextLine, TileOutcome};
use serde::Serialize;
use std::sync::Arc;
use tauri::ipc::{Channel, Response};
use tauri::{Manager, State};

type Eng<'a> = State<'a, Arc<Engine>>;

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

#[tauri::command]
async fn open_document(eng: Eng<'_>, path: String) -> Result<OpenOutcome, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || {
        let path = recents::canonical(&path);
        let (doc, r) = eng.open_pending(&path);
        match r {
            Ok(info) => Ok(OpenOutcome::Opened(info)),
            Err(EngineError::NeedsPassword) => Ok(OpenOutcome::NeedsPassword { doc, path }),
            Err(e) => Err(e),
        }
    })
    .await
}

#[tauri::command]
async fn authenticate(eng: Eng<'_>, doc: DocId, password: String) -> Result<OpenInfo, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.authenticate(doc, &password)).await
}

#[tauri::command]
fn close_document(eng: Eng<'_>, doc: DocId) {
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

#[tauri::command]
async fn edit_apply(eng: Eng<'_>, doc: DocId, op: EditOp) -> Result<EditState, EngineError> {
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

/// `path`: Save as; omitted: save over the document's own file.
#[tauri::command]
async fn save_document(eng: Eng<'_>, doc: DocId, path: Option<String>) -> Result<EditState, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.save(doc, path.as_deref())).await
}

#[tauri::command]
async fn extract_pages(
    eng: Eng<'_>,
    doc: DocId,
    pages: Vec<u32>,
    out: String,
    delete_after: bool,
) -> Result<EditState, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.extract(doc, &pages, &out, delete_after)).await
}

#[tauri::command]
async fn split_document(eng: Eng<'_>, doc: DocId, every: u32) -> Result<SplitResult, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.split(doc, every)).await
}

#[tauri::command]
async fn merge_files(eng: Eng<'_>, paths: Vec<String>, out: String) -> Result<u32, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.merge(&paths, &out)).await
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

/// `job`: an id the UI picks, for `tool_cancel`. The output file is named by a dialog shown here.
#[tauri::command]
async fn compress_start(
    app: tauri::AppHandle,
    eng: Eng<'_>,
    jobs: JobState<'_>,
    job: u64,
    req: tools::CompressRequest,
    on_progress: Channel<compress::Progress>,
) -> Result<compress::Report, tools::ToolError> {
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    tool_blocking(move || {
        let output = if req.choose_output {
            let input = std::path::PathBuf::from(eng.doc_entry(req.doc)?.path());
            let out = tools::paths::choose_output(&app, &compress::default_output(&input));
            Some(out.ok_or_else(|| tools::ToolError("cancelled".into()))?)
        } else {
            None
        };
        tools::run_compress(&eng, &req, output, &guard.cancel, &mut |p| {
            let _ = on_progress.send(p);
        })
    })
    .await
}

#[tauri::command]
fn tool_cancel(jobs: JobState<'_>, job: u64) {
    jobs.cancel(job);
}

type Paths<'a> = State<'a, Arc<tools::paths::ToolPaths>>;

#[tauri::command]
async fn ocr_languages(paths: Paths<'_>) -> Result<tools::OcrLanguages, tools::ToolError> {
    let configured = paths.tesseract();
    tool_blocking(move || Ok(tools::ocr_languages(configured.as_deref()))).await
}

/// Shows a file picker for tesseract.exe and remembers the choice. Returns it for display.
#[tauri::command]
async fn ocr_choose_tesseract(app: tauri::AppHandle, paths: Paths<'_>) -> Result<Option<String>, tools::ToolError> {
    let paths = paths.inner().clone();
    tool_blocking(move || {
        let Some(p) = tools::paths::choose_tesseract(&app) else { return Ok(None) };
        paths.set_tesseract(Some(p.clone())).map_err(tools::ToolError)?;
        Ok(Some(p.to_string_lossy().into_owned()))
    })
    .await
}

#[tauri::command]
fn ocr_clear_tesseract(paths: Paths<'_>) -> Result<(), tools::ToolError> {
    paths.set_tesseract(None).map_err(tools::ToolError)
}

#[tauri::command]
async fn ocr_start(
    app: tauri::AppHandle,
    eng: Eng<'_>,
    jobs: JobState<'_>,
    paths: Paths<'_>,
    job: u64,
    req: tools::OcrRequest,
    on_progress: Channel<ocr::Progress>,
) -> Result<tools::OcrOutcome, tools::ToolError> {
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    let tesseract = paths.tesseract();
    tool_blocking(move || {
        let save_as = if req.save_as {
            let input = std::path::PathBuf::from(eng.doc_entry(req.doc)?.path());
            let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
            let out = tools::paths::choose_output(&app, &input.with_file_name(format!("{stem} (searchable).pdf")));
            Some(out.ok_or_else(|| tools::ToolError("cancelled".into()))?)
        } else {
            None
        };
        let backend = tools::make_backend(req.backend, tesseract.as_deref())?;
        tools::run_ocr(
            &eng,
            &req,
            save_as.as_deref(),
            backend.as_ref(),
            &|p| {
                let _ = on_progress.send(p);
            },
            &guard.cancel,
        )
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
fn initial_files() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-') && std::path::Path::new(a).is_file())
        .collect()
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

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(engine)
        .manage(ai::Assistant::new())
        .manage(Arc::new(tools::Jobs::default()))
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
            save_document,
            extract_pages,
            split_document,
            merge_files,
            compress_audit,
            compress_start,
            tool_cancel,
            ocr_languages,
            ocr_choose_tesseract,
            ocr_clear_tesseract,
            ocr_start,
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
        .setup(|app| {
            #[cfg(windows)]
            disable_browser_accelerators(app);
            app.manage(Arc::new(tools::paths::ToolPaths::load(app.handle())));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Inkwell");
}

