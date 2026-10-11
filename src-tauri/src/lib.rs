pub mod a11y;
pub mod ai;
pub mod annot;
pub mod forms;
pub mod compress;
pub mod convert;
pub mod diff;
pub mod engine;
pub mod history;
pub mod history_ipc;
pub mod imageedit;
pub mod links;
pub mod marks;
pub mod measure;
pub mod ocr;
pub mod organize;
pub mod outline;
pub mod paths;
pub mod preflight;
pub mod prepress;
pub mod print;
pub mod protect;
pub mod recents;
pub mod redact;
pub mod search;
pub mod textedit;
pub mod tools;

use engine::cache::TileKey;
use engine::edit::{EditOp, EditState, SplitResult};
use engine::{DocId, Engine, EngineError, EngineStats, OpenInfo, OutlineItem, SearchEvent, TextLine, TileOutcome};
use history_ipc::History;
use paths::{FileRef, Paths};
use search::SearchOpts;
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
    Search,
}

/// What the save dialog filters for (the UI never names a pattern).
#[derive(serde::Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum SaveKind {
    #[default]
    Pdf,
    Text,
    Html,
    Xhtml,
    Word,
    /// No filter: print-to-file output whose format the printer driver decides.
    Any,
}

/// What an open dialog for Create filters for.
#[derive(serde::Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum SourceKind {
    Images,
    Text,
}

/// Open dialog in Rust; the picked files come back as open tokens.
#[tauri::command]
async fn pick_open(window: Window, multiple: bool, title: Option<PickTitle>) -> Result<Vec<FileRef>, EngineError> {
    blocking(move || {
        let mut d = window.dialog().file().set_parent(&window).add_filter("PDF documents", PDF);
        match title {
            Some(PickTitle::Locate) => d = d.set_title("Locate the moved file"),
            Some(PickTitle::Compare) => d = d.set_title("Choose two PDFs to compare"),
            Some(PickTitle::Search) => d = d.set_title("Choose PDFs to search"),
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
    kind: Option<SaveKind>,
) -> Result<Option<FileRef>, EngineError> {
    let near = match (doc, near) {
        (Some(doc), _) => Some(eng.doc_entry(doc)?.path()),
        (None, Some(t)) => Some(window.state::<Paths>().open(window.label(), &t)?),
        _ => None,
    };
    blocking(move || {
        let name = std::path::Path::new(&name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // Automation (debug builds only, like INKWELL_PRINT_DIR): every save dialog answers `<dir>/<file name>`.
        #[cfg(debug_assertions)]
        if let Some(dir) = std::env::var_os("INKWELL_SAVE_DIR").filter(|d| !d.is_empty()).filter(|_| !name.is_empty()) {
            let p = std::path::Path::new(&dir).join(&name);
            return Ok(Some(window.state::<Paths>().issue_save(window.label(), p.to_string_lossy().into_owned())));
        }
        let d = window.dialog().file().set_parent(&window).set_file_name(name);
        let mut d = match kind.unwrap_or_default() {
            SaveKind::Pdf => d.add_filter("PDF documents", PDF),
            SaveKind::Text => d.add_filter("Text", &["txt"]),
            SaveKind::Html => d.add_filter("HTML", &["html", "htm"]),
            SaveKind::Xhtml => d.add_filter("XHTML", &["xhtml"]),
            SaveKind::Word => d.add_filter("Word document", &["docx"]),
            SaveKind::Any => d,
        };
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

/// Open dialog for the files Create builds a PDF from (images or one text file); open tokens come back.
#[tauri::command]
async fn pick_source(window: Window, kind: SourceKind, multiple: bool) -> Result<Vec<FileRef>, EngineError> {
    blocking(move || {
        let d = window.dialog().file().set_parent(&window);
        let d = match kind {
            SourceKind::Images => d.set_title("Choose images").add_filter("Images", &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "gif"]),
            SourceKind::Text => d.set_title("Choose a text file").add_filter("Text", &["txt", "text", "md", "log", "csv"]),
        };
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

/// Folder dialog in Rust, starting next to open document `doc`; the result is a one-use save token
/// (it names the folder a multi-file export writes into).
#[tauri::command]
async fn pick_folder(window: Window, eng: Eng<'_>, doc: Option<DocId>) -> Result<Option<FileRef>, EngineError> {
    let near = match doc {
        Some(doc) => Some(eng.doc_entry(doc)?.path()),
        None => None,
    };
    blocking(move || {
        let mut d = window.dialog().file().set_parent(&window).set_title("Choose a folder for the files");
        if let Some(dir) = near.as_deref().and_then(|p| std::path::Path::new(p).parent()) {
            d = d.set_directory(dir);
        }
        let picked = d.blocking_pick_folder().and_then(|f| f.into_path().ok());
        Ok(picked.map(|p| window.state::<Paths>().issue_save(window.label(), p.to_string_lossy().into_owned())))
    })
    .await
}

#[tauri::command]
async fn open_document(window: Window, app: tauri::AppHandle, eng: Eng<'_>, reg: Reg<'_>, hist: State<'_, Arc<History>>, file: String, scratch: Option<bool>) -> Result<OpenOutcome, EngineError> {
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
    // A scratch file (a created PDF nobody has saved yet) gets its history when it is saved as.
    if let (Ok(OpenOutcome::Opened(info)), false) = (&r, scratch.unwrap_or(false)) {
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
fn close_document(eng: Eng<'_>, hist: State<'_, Arc<History>>, pre: State<'_, Arc<prepress::app::Prepress>>, doc: DocId) {
    hist.forget(doc);
    pre.drop_doc(doc);
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
fn search(eng: Eng<'_>, doc: DocId, query: String, opts: Option<SearchOpts>, on_event: Channel<SearchEvent>) -> Result<u64, EngineError> {
    eng.inner().search_with(
        doc,
        query,
        opts.unwrap_or_default(),
        Arc::new(move |e| {
            let _ = on_event.send(e);
        }),
    )
}

/// Where a search across files looks: a folder (open token from `pick_search_folder`) or picked files.
#[derive(serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum SearchScope {
    #[serde(rename_all = "camelCase")]
    Folder { token: String, recursive: bool },
    Files { tokens: Vec<String> },
}

/// Folder dialog for search across files; the folder comes back as an open token (Rust lists its PDFs).
#[tauri::command]
async fn pick_search_folder(window: Window) -> Result<Option<FileRef>, EngineError> {
    blocking(move || {
        let d = window.dialog().file().set_parent(&window).set_title("Choose a folder to search");
        let picked = d.blocking_pick_folder().and_then(|f| f.into_path().ok());
        Ok(picked.map(|p| window.state::<Paths>().issue_open(window.label(), p.to_string_lossy().into_owned())))
    })
    .await
}

/// Searches several PDFs for `query` in the background; events (`started` first, then one per file with hits,
/// progress, done) go to `on_event`. Returns the search id (also in `started`, which comes before the folder is
/// listed) for `cancel_search_files`. A result's open token comes from `search_file_token` when it is opened.
#[tauri::command]
async fn search_files(
    window: Window,
    searches: State<'_, Arc<search::files::FolderSearches>>,
    scope: SearchScope,
    query: String,
    opts: Option<SearchOpts>,
    on_event: Channel<search::files::FilesEvent>,
) -> Result<u64, EngineError> {
    use search::files::{list_pdfs, list_picked, FilesEvent, Listing};
    let searches = searches.inner().clone();
    // The id exists before the (possibly long) walk, so Cancel reaches it.
    let (id, cancel) = searches.begin(window.label());
    let _ = on_event.send(FilesEvent::Started { search: id });
    blocking(move || {
        let listed: Result<Listing, EngineError> = (|| {
            let reg = window.state::<Paths>();
            match scope {
                SearchScope::Folder { token, recursive } => {
                    let dir = std::path::PathBuf::from(reg.open(window.label(), &token)?);
                    if !dir.is_dir() {
                        return Err(EngineError::Io(format!("{} is not a folder", dir.display())));
                    }
                    Ok(list_pdfs(&dir, recursive, &cancel))
                }
                SearchScope::Files { tokens } => {
                    let mut files = Vec::new();
                    for t in &tokens {
                        files.push(std::path::PathBuf::from(reg.open(window.label(), t)?));
                    }
                    Ok(list_picked(files))
                }
            }
        })();
        let listing = match listed {
            Ok(l) => l,
            Err(e) => {
                // Say it ended, so nothing waits for it.
                searches.cancel(window.label(), id);
                searches.run(id, cancel, Vec::new(), false, 0, "", SearchOpts::default(), Arc::new(move |e| {
                    let _ = on_event.send(e);
                }));
                return Err(e);
            }
        };
        searches.run(
            id,
            cancel,
            listing.files,
            listing.capped,
            listing.cloud_only,
            &query,
            opts.unwrap_or_default(),
            Arc::new(move |e| {
                let _ = on_event.send(e);
            }),
        );
        Ok(id)
    })
    .await
}

/// An open token for file `index` of search `search` (a result the user opens), issued now.
#[tauri::command]
fn search_file_token(window: Window, searches: State<'_, Arc<search::files::FolderSearches>>, search: u64, index: u32) -> Result<FileRef, EngineError> {
    let path = searches
        .path_of(window.label(), search, index)
        .ok_or_else(|| EngineError::Io("that search result is no longer available".into()))?;
    Ok(window.state::<Paths>().issue_open(window.label(), path.to_string_lossy().into_owned()))
}

#[tauri::command]
fn cancel_search_files(window: Window, searches: State<'_, Arc<search::files::FolderSearches>>, search: u64) {
    searches.cancel(window.label(), search);
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

/// The current state, changing nothing: what the UI asks for after a command that failed half way.
#[tauri::command]
async fn edit_state(eng: Eng<'_>, doc: DocId) -> Result<EditState, EngineError> {
    let eng = eng.inner().clone();
    blocking(move || eng.edit_state(doc)).await
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
        after_save(&window, &app, &hist, doc, to.as_deref(), &state, saved);
        Ok(state)
    })
    .await
}

/// What follows a save: the folder is remembered, and the version history records what was written.
fn after_save(window: &Window, app: &tauri::AppHandle, hist: &Arc<History>, doc: DocId, to: Option<&str>, state: &EditState, saved: Option<engine::edit::Saved>) {
    if let Some(to) = to {
        window.state::<Paths>().learn(to);
    }
    match saved {
        // Save as: the document is another file now, with a history of its own
        Some(s) if s.moved => hist.spawn_open(app.clone(), doc, state.path.clone()),
        // history errors never fail a save
        Some(s) => match hist.saved(doc, &s) {
            Ok(_) => drop(app.emit("history-changed", doc)),
            Err(e) => eprintln!("inkwell history: {e}"),
        },
        None => {}
    }
}

/// Protects the document and saves it (over its file, or to `to`, a save token) as one operation:
/// when the save fails the protection is taken back, so nothing is left pending on the original.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn protect_and_save(
    window: Window,
    app: tauri::AppHandle,
    eng: Eng<'_>,
    reg: Reg<'_>,
    hist: State<'_, Arc<History>>,
    doc: DocId,
    spec: protect::ProtectSpec,
    permissions_password: Option<String>,
    to: Option<String>,
) -> Result<EditState, EngineError> {
    let to = to.map(|t| reg.take_save(window.label(), &t)).transpose()?;
    let (eng, hist) = (eng.inner().clone(), hist.inner().clone());
    blocking(move || {
        let (state, saved) = eng.protect_and_save(doc, spec, permissions_password.as_deref(), to.as_deref())?;
        after_save(&window, &app, &hist, doc, to.as_deref(), &state, saved);
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

// ---- export and create (see convert) -------------------------------------------------------------

/// `req.save_to`: a save token from `pick_save` (one file) or `pick_folder` (several files).
#[tauri::command]
async fn export_start(
    window: Window,
    eng: Eng<'_>,
    reg: Reg<'_>,
    jobs: JobState<'_>,
    job: u64,
    req: convert::ExportRequest,
    on_progress: Channel<convert::Progress>,
) -> Result<convert::ExportOutput, tools::ToolError> {
    let to = std::path::PathBuf::from(reg.take_save(window.label(), &req.save_to)?);
    let target = if req.kind.is_folder() { convert::Target::Folder(to) } else { convert::Target::File(to) };
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    tool_blocking(move || {
        if let convert::Target::Folder(d) = &target {
            if !d.is_dir() {
                return Err(tools::ToolError(format!("{} is not a folder", d.display())));
            }
        }
        let r = convert::export(&eng, &req, target, &guard.cancel, &mut |p| {
            let _ = on_progress.send(p);
        })?;
        window.state::<Paths>().learn(&r.folder);
        for f in &r.files {
            window.state::<Paths>().learn(f);
        }
        Ok(r)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Created {
    file: FileRef,
    pages: u32,
    warnings: Vec<String>,
}

/// Builds a PDF into a scratch folder and returns an open token for it (the UI opens it as an unsaved tab).
#[tauri::command]
async fn create_start(
    window: Window,
    reg: Reg<'_>,
    jobs: JobState<'_>,
    job: u64,
    req: convert::CreateRequest,
    on_progress: Channel<convert::Progress>,
) -> Result<Created, tools::ToolError> {
    let source = req.source.resolve(&|t| reg.open(window.label(), t))?;
    let guard = jobs.inner().begin(job);
    tool_blocking(move || {
        let r = convert::create(&source, &req, &guard.cancel, &mut |p| {
            let _ = on_progress.send(p);
        })?;
        let file = window.state::<Paths>().issue_open(window.label(), r.path);
        Ok(Created { file, pages: r.pages, warnings: r.warnings })
    })
    .await
}

/// Deletes a created file that was never saved (its tab was closed, or it was saved elsewhere).
#[tauri::command]
fn discard_scratch(window: Window, reg: Reg<'_>, file: String) -> Result<bool, EngineError> {
    let path = reg.open(window.label(), &file)?;
    Ok(convert::discard_scratch(std::path::Path::new(&path)))
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
    let prepress = Arc::new(prepress::app::Prepress::default());
    let proto_prepress = prepress.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(engine)
        .manage(ai::Assistant::new())
        .manage(Paths::default())
        .manage(Arc::new(tools::Jobs::default()))
        .manage(Arc::new(measure::SnapCache::default()))
        .manage(Arc::new(search::files::FolderSearches::default()))
        .manage(hist)
        .manage(prepress)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                on_drop(window, paths);
            }
        })
        .register_asynchronous_uri_scheme_protocol("inkwell", move |_ctx, request, responder| {
            // `/p/...`: a page tile in a print production view (plates, coverage)
            if let Some((key, view, stream, epoch, rank)) = prepress::app::parse_tile_request(request.uri()) {
                proto_prepress.request_tile(
                    &proto_engine,
                    key,
                    view,
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
                return;
            }
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
            pick_search_folder,
            search_files,
            cancel_search_files,
            search_file_token,
            engine_stats,
            set_cache_budget,
            initial_files,
            tile_raw,
            edit_apply,
            edit_undo,
            edit_state,
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
            history_ipc::history_compare_run,
            history_ipc::history_compare_report,
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
            protect::app::security_info,
            protect::app::protect_apply,
            protect_and_save,
            protect::app::remove_security,
            marks::app::marks_info,
            marks::app::marks_add,
            marks::app::marks_remove,
            marks::app::marks_bates_files,
            tool_cancel,
            ocr_languages,
            ocr_choose_tesseract,
            ocr_clear_tesseract,
            ocr_start,
            pick_source,
            pick_folder,
            export_start,
            create_start,
            discard_scratch,
            a11y::app::a11y_check,
            a11y::app::a11y_fix,
            a11y::app::a11y_preview,
            a11y::app::a11y_export_report,
            prepress::app::prepress_separations,
            prepress::app::prepress_view,
            prepress::app::prepress_ink,
            prepress::app::prepress_inspect,
            prepress::app::prepress_convert,
            prepress::app::prepress_boxes,
            prepress::app::prepress_marks,
            a11y::app::a11y_reading_order,
            a11y::app::a11y_tags,
            a11y::app::a11y_tag_problems,
            a11y::app::a11y_reorder,
            a11y::app::a11y_set_role,
            a11y::app::a11y_artifact,
            a11y::app::a11y_merge_regions,
            a11y::app::a11y_split_region,
            a11y::app::a11y_edit_tag,
            a11y::app::a11y_move_tag,
            preflight::app::preflight_check,
            preflight::app::preflight_claim,
            preflight::app::preflight_fix,
            preflight::app::preflight_convert,
            preflight::app::preflight_export_report,
            annot::app::annot_default_author,
            annot::app::annot_list,
            annot::app::annot_add,
            annot::app::annot_edit,
            annot::app::annot_delete,
            annot::app::annot_reply,
            annot::app::annot_state,
            measure::app::measure_scale_get,
            measure::app::measure_scale_set,
            measure::app::measure_add,
            measure::app::measure_value,
            measure::app::measure_list,
            measure::app::measure_snap,
            forms::app::forms_list,
            forms::app::forms_set_value,
            forms::app::forms_reset,
            forms::app::forms_add,
            forms::app::forms_add_many,
            forms::app::forms_move,
            forms::app::forms_edit,
            forms::app::forms_delete,
            forms::app::forms_tab_order,
            forms::app::forms_arrange,
            forms::app::forms_detect,
            organize::app::combine_inspect,
            organize::app::combine_run,
            organize::app::pick_attachments,
            organize::app::attachments_list,
            organize::app::attachments_add,
            organize::app::attachment_save,
            organize::app::attachment_open,
            organize::app::attachments_remove,
            organize::app::attachment_describe,
            organize::app::labels_list,
            organize::app::labels_set,
            organize::app::labels_remove,
            organize::app::transitions_list,
            organize::app::transitions_set,
            textedit::app::text_blocks,
            textedit::app::text_edit,
            textedit::app::text_add,
            outline::app::bookmarks_list,
            outline::app::bookmarks_apply,
            outline::app::bookmarks_headings,
            links::app::links_list,
            links::app::links_apply,
            links::app::links_urls,
            links::app::links_audit,
            links::app::link_check_url,
            links::app::link_pick_file,
            links::app::link_open_url,
            links::app::link_file_path,
            links::app::link_file_ref,
            textedit::app::text_fonts,
            imageedit::app::image_blocks,
            imageedit::app::image_edit,
            redact::app::redact_find,
            redact::app::redact_mark,
            redact::app::redact_unmark,
            redact::app::redact_list_marks,
            redact::app::redact_options,
            redact::app::redact_apply,
            print::app::print_printers,
            print::app::print_capabilities,
            print::app::print_plan,
            print::app::print_preview,
            print::app::print_start,
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

