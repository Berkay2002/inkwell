//! Compress and OCR as the UI uses them: the logic behind the Tauri commands in `lib.rs`.
//!
//! Plain functions over `&Engine`, a progress callback and a cancel flag, so the tests run them
//! without a Tauri runtime. Compress is file in, new file out (the caller saves pending edits first).
//! OCR in place is the edit core's business: `ocr::recognize` runs in parallel on the document's
//! current snapshot, then `ocr::apply` runs inside `Engine::apply_custom` on the serial lane, so it
//! is one undo step and the normal Save writes it.

pub mod paths;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::compress::{self, Audit, Options, Preset};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, DocSource, Engine, EngineError};
use crate::ocr::backend::{Recognizer, TesseractCli};
use crate::ocr::detect::{self, Detection};
use crate::ocr::lang::{self, Catalog, Plan};
use crate::ocr::{self, Input, OcrError};

// ---------------------------------------------------------------- errors and jobs

/// What the UI's promise rejects with: a code it matches on (`cancelled`, `output-exists`,
/// `stale`, `needs-password`, `invalid-options`, `language-unavailable`, `tesseract-not-found`)
/// or a message to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError(pub String);

impl Serialize for ToolError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<compress::Error> for ToolError {
    fn from(e: compress::Error) -> Self {
        // Same codes the compress engine serialises.
        ToolError(serde_json::to_value(&e).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| e.to_string()))
    }
}

impl From<OcrError> for ToolError {
    fn from(e: OcrError) -> Self {
        ToolError(match e {
            OcrError::Cancelled => "cancelled".into(),
            OcrError::OutputExists(_) => "output-exists".into(),
            OcrError::LanguageUnavailable(_) => "language-unavailable".into(),
            other => other.to_string(),
        })
    }
}

impl From<EngineError> for ToolError {
    fn from(e: EngineError) -> Self {
        ToolError(match e {
            EngineError::Stale => "stale".into(),
            EngineError::NeedsPassword => "needs-password".into(),
            EngineError::BadPassword => "bad-password".into(),
            other => other.to_string(),
        })
    }
}

/// Cancel flags of the running jobs, by the id the UI picked. A cancel that arrives before its job has registered is
/// remembered (the job then starts cancelled), but only for the last few ids; a cancel for a job that has finished
/// is dropped, so neither can pile up or reach a later job.
#[derive(Default)]
pub struct Jobs {
    flags: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    rings: Mutex<Rings>,
}

/// Ids a cancel arrived for before their job began, and ids of jobs that ended: the last few of each.
#[derive(Default)]
struct Rings {
    early: std::collections::VecDeque<u64>,
    ended: std::collections::VecDeque<u64>,
}

const RING: usize = 64;

impl Rings {
    fn push(ring: &mut std::collections::VecDeque<u64>, id: u64) {
        ring.retain(|x| *x != id);
        if ring.len() == RING {
            ring.pop_front();
        }
        ring.push_back(id);
    }
}

pub struct JobGuard {
    jobs: Arc<Jobs>,
    id: u64,
    pub cancel: Arc<AtomicBool>,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.jobs.flags.lock().remove(&self.id);
        let mut r = self.jobs.rings.lock();
        Rings::push(&mut r.ended, self.id);
    }
}

impl Jobs {
    pub fn begin(self: &Arc<Self>, id: u64) -> JobGuard {
        self.begin_inner(id, false).expect("a shared id always begins")
    }

    /// [`begin`](Self::begin) for an id nothing is running under: `None` when a job already has it
    /// (two jobs under one id would share a cancel flag).
    pub fn try_begin(self: &Arc<Self>, id: u64) -> Option<JobGuard> {
        self.begin_inner(id, true)
    }

    fn begin_inner(self: &Arc<Self>, id: u64, exclusive: bool) -> Option<JobGuard> {
        let cancel = {
            let mut flags = self.flags.lock();
            if exclusive && flags.contains_key(&id) {
                return None;
            }
            flags.entry(id).or_default().clone()
        };
        let mut r = self.rings.lock();
        r.ended.retain(|x| *x != id);
        if let Some(i) = r.early.iter().position(|x| *x == id) {
            r.early.remove(i);
            cancel.store(true, Ordering::Relaxed);
        }
        drop(r);
        Some(JobGuard { jobs: self.clone(), id, cancel })
    }

    /// Stops a running job. A cancel for an id that has already finished is ignored; one for an id nobody began yet
    /// is held for it (a few at most).
    pub fn cancel(&self, id: u64) {
        if let Some(f) = self.flags.lock().get(&id) {
            f.store(true, Ordering::Relaxed);
            return;
        }
        let mut r = self.rings.lock();
        if !r.ended.contains(&id) {
            Rings::push(&mut r.early, id);
        }
    }
}

// ---------------------------------------------------------------- compress

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReply {
    pub audit: Audit,
    pub size: u64,
    /// Where the compressed file would go, and whether that name is already taken.
    pub default_output: String,
    pub output_exists: bool,
    /// The document carries signatures; the compressed copy invalidates them.
    pub signed: bool,
}

/// Sample-based predicted sizes (see `compress::estimate`). Separate from the audit because it recompresses
/// real images: the UI runs it after the audit and cancels it with the job id when the rail closes.
pub fn estimate_for_ui(eng: &Engine, doc: DocId, cancel: &AtomicBool) -> Result<Vec<compress::estimate::Estimate>, ToolError> {
    let path = PathBuf::from(eng.doc_entry(doc)?.path());
    let password = eng.file_password(doc)?;
    let audit = compress::audit_file(&path, password.as_ref().map(|p| p.as_str()))?;
    Ok(compress::estimate::estimate(&path, password.as_ref().map(|p| p.as_str()), &audit, cancel)?)
}

/// The same for a set of Advanced settings: one estimate, on the sample, with the settings applied.
pub fn estimate_custom_for_ui(
    eng: &Engine,
    doc: DocId,
    opts: &Options,
    cancel: &AtomicBool,
) -> Result<compress::estimate::Estimate, ToolError> {
    let path = PathBuf::from(eng.doc_entry(doc)?.path());
    let password = eng.file_password(doc)?;
    let pw = password.as_ref().map(|p| p.as_str());
    let audit = compress::audit_file(&path, pw)?;
    let opts = Options { preset: Preset::Custom, password: None, output: None, ..opts.clone() };
    Ok(compress::estimate::estimate_custom(&path, pw, &audit, &opts, cancel)?)
}

pub fn audit_for_ui(eng: &Engine, doc: DocId) -> Result<AuditReply, ToolError> {
    let path = PathBuf::from(eng.doc_entry(doc)?.path());
    let path = path.as_path();
    let (audit, signed) = compress::audit_file_signed(path, eng.file_password(doc)?.as_ref().map(|p| p.as_str()))?;
    let out = compress::default_output(path);
    Ok(AuditReply {
        signed,
        size: std::fs::metadata(path).map_err(|e| ToolError(e.to_string()))?.len(),
        default_output: out.to_string_lossy().into_owned(),
        output_exists: out.exists(),
        audit,
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompressRequest {
    /// The open document; its file on disk is the input (the UI saves pending edits first).
    pub doc: DocId,
    #[serde(default)]
    pub preset: Preset,
    /// Options from a Customize panel; wins over `preset` (which is then `custom`). Its `output`
    /// and `password` are ignored: see [`run_compress`].
    pub custom: Option<Options>,
    /// A save token (`pick_save`) for the output file instead of `<name> (compressed).pdf`.
    pub save_to: Option<String>,
}

/// `output`: where to write, named by a save token (it may replace the file the user
/// confirmed, never the input); `None` = `<name> (compressed).pdf` next to the input, never overwritten.
pub fn run_compress(
    eng: &Engine,
    req: &CompressRequest,
    output: Option<PathBuf>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(compress::Progress),
) -> Result<compress::Report, ToolError> {
    let input = PathBuf::from(eng.doc_entry(req.doc)?.path());
    let mut opts = match &req.custom {
        Some(c) => Options { preset: Preset::Custom, ..c.clone() },
        None => Options::preset(req.preset),
    };
    // A name only gets here from a save token, where the user confirmed replacing a file. The
    // input is still never replaced (the engine checks).
    opts.replace_output = output.is_some();
    opts.output = output;
    // The file on disk is what compress reads: its password, which Save may have just changed.
    opts.password = eng.file_password(req.doc)?.map(|p| p.to_string());
    // The fraction only ever grows, whatever the stages report.
    let mut last = 0.0f32;
    let mut mono = |mut p: compress::Progress| {
        p.fraction = p.fraction.max(last);
        last = p.fraction;
        progress(p);
    };
    Ok(compress::compress(&input, &opts, cancel, &mut mono)?)
}

// ---------------------------------------------------------------- OCR

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendChoice {
    /// The engine is picked from the languages (see `ocr::lang::pick_engine`).
    #[default]
    Auto,
    Windows,
    Tesseract,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrLanguages {
    /// BCP-47 tags of the installed Windows OCR packs.
    pub windows: Vec<String>,
    /// Present when `tesseract.exe` was found (with its tessdata).
    pub tesseract: Option<TesseractInfo>,
    /// Both engines' languages merged: what the language list shows.
    pub catalog: Catalog,
    /// The user's Windows languages, display language first.
    pub profile: Vec<String>,
    /// The selection to start on when nothing better is known: display language plus English.
    pub defaults: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TesseractInfo {
    pub path: String,
    pub languages: Vec<String>,
}

/// A configured Tesseract location is run as a program, so it has to be named like one.
pub(crate) fn configured_tesseract(path: Option<&Path>) -> Option<&Path> {
    let p = path.filter(|p| !p.as_os_str().is_empty())?;
    let name = p.file_stem()?.to_string_lossy().to_ascii_lowercase();
    (name == "tesseract").then_some(p)
}

pub fn ocr_languages(tesseract_path: Option<&Path>) -> OcrLanguages {
    #[cfg(windows)]
    let windows = ocr::backend::WinRtOcr.languages().unwrap_or_default();
    #[cfg(not(windows))]
    let windows = Vec::new();
    let tess = TesseractCli::discover(configured_tesseract(tesseract_path));
    let tess_langs = tess.as_ref().map(|t| t.languages().unwrap_or_default());
    let tessdata = tess.as_ref().and_then(|t| t.tessdata.as_ref()).map(|d| d.to_string_lossy().into_owned());
    let catalog = Catalog::build(&windows, tess_langs.as_deref(), tessdata);
    let profile = ocr::backend::profile_languages();
    let defaults = lang::default_languages(&profile, &catalog);
    let tesseract = tess.map(|t| TesseractInfo {
        languages: tess_langs.unwrap_or_default(),
        path: t.exe.to_string_lossy().into_owned(),
    });
    OcrLanguages { windows, tesseract, catalog, profile, defaults }
}

/// What the sheet shows for the chosen languages: the engine that will read them and what is missing.
pub fn ocr_plan(languages: &[String], choice: BackendChoice, tesseract_path: Option<&Path>) -> Plan {
    let l = ocr_languages(tesseract_path);
    let requested: Vec<String> = if languages.is_empty() { l.defaults.clone() } else { languages.to_vec() };
    let prefer = match choice {
        BackendChoice::Auto => None,
        BackendChoice::Windows => Some(lang::Engine::Windows),
        BackendChoice::Tesseract => Some(lang::Engine::Tesseract),
    };
    lang::pick_engine(&requested, &l.catalog, prefer)
}

/// Settles the engine and the languages of a request from what is installed, and returns the
/// recogniser. `req.languages` and `req.backend` are replaced by the plan's.
pub fn resolve_backend(req: &mut OcrRequest, tesseract_path: Option<&Path>) -> Result<Box<dyn Recognizer>, ToolError> {
    let plan = ocr_plan(&req.languages, req.backend, tesseract_path);
    let engine = match plan.engine {
        Some(lang::Engine::Windows) => BackendChoice::Windows,
        Some(lang::Engine::Tesseract) => BackendChoice::Tesseract,
        None => {
            // Nothing installed reads the language; the sheet already showed how to add it.
            return Err(ToolError(if req.backend == BackendChoice::Tesseract && ocr_languages(tesseract_path).tesseract.is_none() {
                "tesseract-not-found".into()
            } else {
                "language-unavailable".into()
            }));
        }
    };
    req.languages = plan.languages;
    req.backend = engine;
    make_backend(engine, tesseract_path)
}

/// The language the document is probably in (see `ocr::detect`): existing text, a sample read, then
/// the user's Windows languages.
pub fn ocr_detect(eng: &Engine, doc: DocId, tesseract_path: Option<&Path>, cancel: &AtomicBool) -> Result<Detection, ToolError> {
    let l = ocr_languages(tesseract_path);
    #[cfg(windows)]
    let win = Some(ocr::backend::WinRtOcr);
    #[cfg(not(windows))]
    let win: Option<ocr::backend::TesseractCli> = None;
    let tess = TesseractCli::discover(configured_tesseract(tesseract_path));
    let (_, source) = eng.doc_source(doc)?;
    let run = |input: &Input| {
        detect::detect(input, &l.catalog, &l.profile, win.as_ref().map(|w| w as &dyn Recognizer), tess.as_ref().map(|t| t as &dyn Recognizer), cancel)
    };
    Ok(match &source {
        DocSource::Path(p) => run(&Input::Path(Path::new(p)))?,
        DocSource::Bytes(b, n) => run(&Input::Bytes(&b[..*n]))?,
    })
}

/// The recogniser for `choice`. Tesseract is only ever the configured path, `INKWELL_TESSERACT` or
/// the Program Files install (see `TesseractCli::discover`), never `PATH`.
pub fn make_backend(choice: BackendChoice, tesseract_path: Option<&Path>) -> Result<Box<dyn Recognizer>, ToolError> {
    match choice {
        BackendChoice::Tesseract => TesseractCli::discover(configured_tesseract(tesseract_path))
            .map(|t| Box::new(t) as Box<dyn Recognizer>)
            .ok_or_else(|| ToolError("tesseract-not-found".into())),
        #[cfg(windows)]
        BackendChoice::Windows | BackendChoice::Auto => Ok(Box::new(ocr::backend::WinRtOcr)),
        #[cfg(not(windows))]
        BackendChoice::Windows | BackendChoice::Auto => Err(ToolError("Windows OCR is only available on Windows".into())),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OcrRequest {
    pub doc: DocId,
    /// One-based range like `1-3, 5`; none = every page.
    pub pages: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    /// Read pages that already have text and replace an earlier layer of ours.
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub backend: BackendChoice,
    /// What the pages look like afterwards (searchable image, cleaned up, editable text).
    #[serde(default)]
    pub style: ocr::Style,
    /// Render resolution: 150, 200 (default) or 300.
    pub dpi: Option<u32>,
    /// The face editable text is set in.
    #[serde(default)]
    pub font: ocr::FontPref,
    /// A save token (`pick_save`): write a new file instead of changing the open document.
    pub save_to: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrOutcome {
    pub report: ocr::Report,
    /// The document after an in-place run (new `rev`, undo label); none for `saveAs`.
    pub state: Option<EditState>,
    /// The file written by `saveAs`.
    pub output: Option<String>,
}

fn options_for(eng: &Engine, req: &OcrRequest) -> Result<ocr::Options, ToolError> {
    let count = eng.doc_entry(req.doc)?.page_count();
    let mut o = ocr::Options { languages: req.languages.clone(), force: req.force, style: req.style, font: req.font, ..Default::default() };
    if let Some(d) = req.dpi {
        // The sheet offers three; anything else is snapped to the nearest rather than trusted.
        o.dpi = [150u32, 200, 300].into_iter().min_by_key(|c| c.abs_diff(d)).unwrap_or(200);
    }
    if req.style == ocr::Style::Editable {
        // Real text is only ever made from pages that have none: never over a page's own text.
        o.force = false;
    }
    if let Some(spec) = req.pages.as_deref().filter(|s| !s.trim().is_empty()) {
        o.pages = Some(ocr::parse_page_range(spec, count)?);
    }
    Ok(o)
}

/// Runs OCR on an open document. `progress` sees `done` only ever grow (workers finish out of order).
/// `save_as`: the file behind `req.save_to`, resolved by the command.
pub fn run_ocr(
    eng: &Engine,
    req: &OcrRequest,
    save_as: Option<&Path>,
    backend: &dyn Recognizer,
    progress: &(dyn Fn(ocr::Progress) + Sync),
    cancel: &AtomicBool,
) -> Result<OcrOutcome, ToolError> {
    let opts = options_for(eng, req)?;
    if opts.style != ocr::Style::Searchable && eng.read_custom(req.doc, |d| Ok(ocr::is_signed(d)))? {
        // Changing the pictures of a signed document would break its signature; nothing is read or changed.
        return Err(ToolError(ocr::SIGNED.into()));
    }
    let last = AtomicU32::new(0);
    let mono = |p: ocr::Progress| {
        if last.fetch_max(p.done, Ordering::SeqCst) < p.done {
            progress(p);
        }
    };
    let (rev, source) = eng.doc_source(req.doc)?;

    if let Some(out) = save_as {
        // The dialog may confirm replacing a file, but never the document's own (the source may be
        // a snapshot in memory, so the engine's own input check would not see it).
        if ocr::is_same_file(Path::new(&eng.doc_entry(req.doc)?.path()), out) {
            return Err(OcrError::Io("output must be a different file than the input".into()).into());
        }
        // A new file is made from what the document shows now: pending edits included.
        return Ok(match &source {
            DocSource::Path(p) => ocr_to_file(Path::new(p), out, backend, &opts, &mono, cancel)?,
            DocSource::Bytes(b, n) => {
                let tmp = std::env::temp_dir().join(format!("inkwell-ocr-src-{}-{}.pdf", std::process::id(), req.doc));
                std::fs::write(&tmp, &b[..*n]).map_err(|e| ToolError(e.to_string()))?;
                let r = ocr_to_file(&tmp, out, backend, &opts, &mono, cancel);
                let _ = std::fs::remove_file(&tmp);
                r?
            }
        });
    }

    let rec = match &source {
        DocSource::Path(p) => ocr::recognize(&Input::Path(Path::new(p)), backend, &opts, &mono, cancel)?,
        DocSource::Bytes(b, n) => ocr::recognize(&Input::Bytes(&b[..*n]), backend, &opts, &mono, cancel)?,
    };
    if rec.cancelled {
        return Err(OcrError::Cancelled.into());
    }
    if let Some(m) = no_page_readable(&rec) {
        return Err(OcrError::Backend(m).into());
    }

    let entry = eng.doc_entry(req.doc)?;
    let to_write = rec.pages.iter().filter(|p| p.status == ocr::PageStatus::Recognized).count();
    let mut applied = ocr::ApplyReport::default();
    let mut warnings = Vec::new();
    let state = if to_write == 0 {
        // Nothing to add (pages already had text, or nothing was found): no undo step either.
        eng.edit_state(req.doc)?
    } else {
        let pages = format!("{to_write} page{}", if to_write == 1 { "" } else { "s" });
        let label = match opts.style {
            ocr::Style::Searchable => format!("Recognize text ({pages})"),
            ocr::Style::Cleaned => format!("Recognize text, cleaned up ({pages})"),
            ocr::Style::Editable => format!("Recognize text as editable text ({pages})"),
        };
        let work = |pdf: &mut mupdf::pdf::PdfDocument| {
            // Page indices belong to the generation the pages were read from: an edit that
            // landed meanwhile (delete, move) would put the text on the wrong pages.
            if entry.generation() != rev {
                return Err(EngineError::Stale);
            }
            if ocr::is_signed(pdf) {
                warnings.push("The document is signed; saving it makes the signature invalid.".to_string());
            }
            applied = ocr::apply_styled(pdf, &rec.pages, &opts).map_err(|e| EngineError::Invalid(e.to_string()))?;
            Ok(rec.pages.iter().filter(|p| p.status == ocr::PageStatus::Recognized).map(|p| p.page).collect())
        };
        if opts.style == ocr::Style::Searchable {
            eng.apply_custom(req.doc, label, EditSource::User, false, work)?
        } else {
            // Pictures were replaced: a save writes the file without the old ones.
            eng.apply_removing(req.doc, label, EditSource::User, work)?
        }
    };
    Ok(OcrOutcome { report: ocr::build_report(&rec, &applied, warnings), state: Some(state), output: None })
}

/// Before and after pictures of one page, as `data:` URLs, for the sheet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrPreview {
    pub before: String,
    pub after: String,
    pub applied: ocr::ApplyReport,
}

/// Renders what `req.style` would do to page `page` (zero-based) of the open document, without
/// changing it. The longer side of each picture is at most `max_px`.
pub fn ocr_preview(
    eng: &Engine,
    req: &OcrRequest,
    page: u32,
    max_px: u32,
    backend: &dyn Recognizer,
    cancel: &AtomicBool,
) -> Result<OcrPreview, ToolError> {
    use base64::Engine as _;
    let opts = options_for(eng, req)?;
    let (_, source) = eng.doc_source(req.doc)?;
    let p = match &source {
        DocSource::Path(path) => ocr::preview(&Input::Path(Path::new(path)), page, backend, &opts, max_px, cancel)?,
        DocSource::Bytes(b, n) => ocr::preview(&Input::Bytes(&b[..*n]), page, backend, &opts, max_px, cancel)?,
    };
    let url = |png: &[u8]| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
    Ok(OcrPreview { before: url(&p.before), after: url(&p.after), applied: p.applied })
}

fn no_page_readable(rec: &ocr::Recognition) -> Option<String> {
    let mut failed = rec.pages.iter().filter_map(|p| match &p.status {
        ocr::PageStatus::Failed(m) => Some(m.clone()),
        _ => None,
    });
    let first = failed.next()?;
    rec.pages
        .iter()
        .all(|p| !matches!(p.status, ocr::PageStatus::Recognized))
        .then(|| format!("no page could be read: {first}"))
}

fn ocr_to_file(
    input: &Path,
    out: &Path,
    backend: &dyn Recognizer,
    opts: &ocr::Options,
    progress: &(dyn Fn(ocr::Progress) + Sync),
    cancel: &AtomicBool,
) -> Result<OcrOutcome, OcrError> {
    // `out` only comes from the save dialog, where replacing a file was confirmed.
    let report = ocr::ocr_file_with(input, out, true, backend, opts, progress, cancel)?;
    Ok(OcrOutcome { report, state: None, output: Some(out.to_string_lossy().into_owned()) })
}
