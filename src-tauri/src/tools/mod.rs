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

use crate::compress::{self, Audit, Category, Options, Preset};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, DocSource, Engine, EngineError};
use crate::ocr::backend::{Recognizer, TesseractCli};
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

/// Cancel flags of the running jobs, by the id the UI picked. A cancel that arrives before its job
/// has registered is remembered, so the job starts cancelled.
#[derive(Default)]
pub struct Jobs {
    flags: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

pub struct JobGuard {
    jobs: Arc<Jobs>,
    id: u64,
    pub cancel: Arc<AtomicBool>,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.jobs.flags.lock().remove(&self.id);
    }
}

impl Jobs {
    pub fn begin(self: &Arc<Self>, id: u64) -> JobGuard {
        let cancel = self.flags.lock().entry(id).or_default().clone();
        JobGuard { jobs: self.clone(), id, cancel }
    }

    pub fn cancel(&self, id: u64) {
        self.flags.lock().entry(id).or_default().store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- compress

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prediction {
    pub preset: Preset,
    /// Estimated size of the compressed file in bytes.
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReply {
    pub audit: Audit,
    pub size: u64,
    /// Where the compressed file would go, and whether that name is already taken.
    pub default_output: String,
    pub output_exists: bool,
    pub predictions: Vec<Prediction>,
}

/// Share of the image bytes each preset keeps, measured on the study scans (docs/compress-engine.md:
/// 1.2% / 9% / 23% of a 300 ppi photo scan). Images that are already small shrink less, so this is a
/// guide ("about 2 MB"), never below what a re-run would find: the result screen shows the real size.
// ponytail: fixed ratios; derive from per-image ppi once the audit reports it.
const IMAGE_KEEP: [(Preset, f64); 3] = [(Preset::Smallest, 0.12), (Preset::Balanced, 0.30), (Preset::HighQuality, 0.55)];

pub fn predict(audit: &Audit) -> Vec<Prediction> {
    let images = audit.bytes(Category::Images) as f64;
    let fonts = audit.bytes(Category::Fonts) as f64;
    IMAGE_KEEP
        .iter()
        .map(|&(preset, keep)| {
            let font_keep = if preset == Preset::Smallest { 0.5 } else { 1.0 };
            let saved = images * (1.0 - keep) + fonts * (1.0 - font_keep);
            Prediction { preset, bytes: (audit.total as f64 - saved).max(1.0) as u64 }
        })
        .collect()
}

pub fn audit_for_ui(eng: &Engine, doc: DocId) -> Result<AuditReply, ToolError> {
    let path = PathBuf::from(eng.doc_entry(doc)?.path());
    let path = path.as_path();
    let audit = compress::audit_file(path, None)?;
    let out = compress::default_output(path);
    Ok(AuditReply {
        size: std::fs::metadata(path).map_err(|e| ToolError(e.to_string()))?.len(),
        default_output: out.to_string_lossy().into_owned(),
        output_exists: out.exists(),
        predictions: predict(&audit),
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
    /// Name the file in a dialog instead of `<name> (compressed).pdf` (used when that is taken).
    #[serde(default)]
    pub choose_output: bool,
}

/// `output`: where to write, chosen in a dialog on the Rust side (it may replace the file the user
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
    // A name only gets here from the save dialog, where the user confirmed replacing a file. The
    // input is still never replaced (the engine checks).
    opts.replace_output = output.is_some();
    opts.output = output;
    opts.password = None;
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
    #[default]
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
    let tesseract = TesseractCli::discover(configured_tesseract(tesseract_path)).map(|t| TesseractInfo {
        languages: t.languages().unwrap_or_default(),
        path: t.exe.to_string_lossy().into_owned(),
    });
    OcrLanguages { windows, tesseract }
}

/// The recogniser for `choice`. Tesseract is only ever the configured path, `INKWELL_TESSERACT` or
/// the Program Files install (see `TesseractCli::discover`), never `PATH`.
pub fn make_backend(choice: BackendChoice, tesseract_path: Option<&Path>) -> Result<Box<dyn Recognizer>, ToolError> {
    match choice {
        BackendChoice::Tesseract => TesseractCli::discover(configured_tesseract(tesseract_path))
            .map(|t| Box::new(t) as Box<dyn Recognizer>)
            .ok_or_else(|| ToolError("tesseract-not-found".into())),
        #[cfg(windows)]
        BackendChoice::Windows => Ok(Box::new(ocr::backend::WinRtOcr)),
        #[cfg(not(windows))]
        BackendChoice::Windows => Err(ToolError("Windows OCR is only available on Windows".into())),
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
    /// Write a new file (named in a dialog) instead of changing the open document.
    #[serde(default)]
    pub save_as: bool,
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
    let mut o = ocr::Options { languages: req.languages.clone(), force: req.force, ..Default::default() };
    if let Some(spec) = req.pages.as_deref().filter(|s| !s.trim().is_empty()) {
        o.pages = Some(ocr::parse_page_range(spec, count)?);
    }
    Ok(o)
}

/// Runs OCR on an open document. `progress` sees `done` only ever grow (workers finish out of order).
/// `save_as`: the file for `req.save_as`, chosen on the Rust side.
pub fn run_ocr(
    eng: &Engine,
    req: &OcrRequest,
    save_as: Option<&Path>,
    backend: &dyn Recognizer,
    progress: &(dyn Fn(ocr::Progress) + Sync),
    cancel: &AtomicBool,
) -> Result<OcrOutcome, ToolError> {
    let opts = options_for(eng, req)?;
    let last = AtomicU32::new(0);
    let mono = |p: ocr::Progress| {
        if last.fetch_max(p.done, Ordering::SeqCst) < p.done {
            progress(p);
        }
    };
    let (rev, source) = eng.doc_source(req.doc)?;

    if let Some(out) = save_as {
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
        eng.apply_custom(
            req.doc,
            format!("Recognize text ({to_write} page{})", if to_write == 1 { "" } else { "s" }),
            EditSource::User,
            false,
            |pdf| {
                // Page indices belong to the generation the pages were read from: an edit that
                // landed meanwhile (delete, move) would put the text on the wrong pages.
                if entry.generation() != rev {
                    return Err(EngineError::Stale);
                }
                if ocr::is_signed(pdf) {
                    warnings.push("The document is signed; saving it makes the signature invalid.".to_string());
                }
                applied = ocr::apply(pdf, &rec.pages, req.force).map_err(|e| EngineError::Invalid(e.to_string()))?;
                Ok(rec
                    .pages
                    .iter()
                    .filter(|p| p.status == ocr::PageStatus::Recognized)
                    .map(|p| p.page)
                    .collect())
            },
        )?
    };
    Ok(OcrOutcome { report: ocr::build_report(&rec, &applied, warnings), state: Some(state), output: None })
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
    let report = ocr::ocr_file(input, out, backend, opts, progress, cancel)?;
    Ok(OcrOutcome { report, state: None, output: Some(out.to_string_lossy().into_owned()) })
}
