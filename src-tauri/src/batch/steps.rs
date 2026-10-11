//! The steps: each one is a thin adapter over an engine that already exists (compress, OCR, marks,
//! redact, protect, forms, convert). A step reads the current working file and either writes the next
//! one or leaves it as it is; the runner chains them. Nothing here writes outside the file's staging
//! folder, and a step only ever opens its input read-only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mupdf::pdf::{PdfDocument, PdfObject, PdfWriteOptions};
use mupdf::Document;

use super::action::*;
use crate::compress;
use crate::convert::{self, ExportKind, Staging, Target};
use crate::engine::EngineError;
use crate::engine::edit::{open_source, rewrite_opts, write_doc};
use crate::marks::{self, AddRequest, Date, Mark};
use crate::ocr::{self, backend::Recognizer};
use crate::protect::{self, SecurityChange};
use crate::redact::{self, ApplyOptions, MarkStyle, Query, RedactError, SanitizeReport};
use crate::tools;

// ---------------------------------------------------------------- environment

/// What a step may need to know about the machine and the moment. Everything here is read-only.
#[derive(Clone)]
pub struct Env {
    /// The configured `tesseract.exe` (the OCR settings), if any.
    pub tesseract: Option<PathBuf>,
    /// The date header/footer tokens expand to (UTC).
    pub date: Date,
    /// True for a file a tab has open: a batch leaves it alone, its unsaved edits are not on disk.
    pub is_open: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
    /// A recogniser to use instead of the real engines (tests).
    pub recognizer: Option<Arc<dyn Recognizer>>,
    /// A file that remembers the folders runs worked in, so a sweep at startup can find what a crash left.
    pub journal: Option<PathBuf>,
}

impl Env {
    pub fn new(tesseract: Option<PathBuf>, is_open: Arc<dyn Fn(&Path) -> bool + Send + Sync>) -> Self {
        Self { tesseract, date: today_utc(), is_open, recognizer: None, journal: None }
    }

    /// Nothing is open; today's date; no Tesseract.
    pub fn bare() -> Self {
        Self::new(None, Arc::new(|_| false))
    }

    pub fn backend(&self, choice: tools::BackendChoice) -> Result<Arc<dyn Recognizer>, String> {
        if let Some(r) = &self.recognizer {
            return Ok(r.clone());
        }
        match tools::make_backend(choice, self.tesseract.as_deref()) {
            Ok(b) => Ok(Arc::from(b)),
            Err(e) if e.0 == "tesseract-not-found" => {
                Err("Tesseract was not found. Choose tesseract.exe in the OCR settings or pick Windows OCR.".into())
            }
            Err(e) => Err(e.0),
        }
    }
}

/// Today in UTC (civil date from days since 1970, Howard Hinnant's algorithm).
pub fn today_utc() -> Date {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    civil_from_days(secs.div_euclid(86_400))
}

pub(super) fn civil_from_days(days: i64) -> Date {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    Date { year, month, day }
}

// ---------------------------------------------------------------- the trait

/// Why a step cannot be used.
pub enum Problem {
    /// The settings are not valid.
    Invalid(String),
    /// Valid, but not on this machine (no OCR engine).
    Unavailable(String),
}

/// Why a step did not finish.
#[derive(Debug)]
pub enum StepError {
    Cancelled,
    /// Not done on purpose (a password is needed): the file is skipped, not failed.
    Refused(String),
    Failed(String),
}

impl StepError {
    fn failed(e: impl std::fmt::Display) -> Self {
        StepError::Failed(e.to_string())
    }
}

impl From<EngineError> for StepError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::NeedsPassword => StepError::Refused("needs a password".into()),
            EngineError::BadPassword => StepError::Refused("is protected with a password".into()),
            e => StepError::Failed(e.to_string()),
        }
    }
}

impl From<mupdf::Error> for StepError {
    fn from(e: mupdf::Error) -> Self {
        StepError::failed(e)
    }
}

impl From<std::io::Error> for StepError {
    fn from(e: std::io::Error) -> Self {
        StepError::failed(e)
    }
}

impl From<compress::Error> for StepError {
    fn from(e: compress::Error) -> Self {
        match e {
            compress::Error::Cancelled => StepError::Cancelled,
            compress::Error::NeedsPassword => StepError::Refused("needs a password".into()),
            compress::Error::BadPassword => StepError::Refused("is protected with a password".into()),
            e => StepError::failed(e),
        }
    }
}

impl From<ocr::OcrError> for StepError {
    fn from(e: ocr::OcrError) -> Self {
        match e {
            ocr::OcrError::Cancelled => StepError::Cancelled,
            ocr::OcrError::Pdf(m) if m.contains("password") => StepError::Refused("needs a password".into()),
            e => StepError::failed(e),
        }
    }
}

impl From<RedactError> for StepError {
    fn from(e: RedactError) -> Self {
        match e {
            RedactError::Leaked(l) => {
                StepError::Failed(format!("the redaction was refused: the redacted text would still be recoverable in {} place(s)", l.len()))
            }
            e => StepError::failed(e),
        }
    }
}

impl From<convert::Error> for StepError {
    fn from(e: convert::Error) -> Self {
        match e {
            convert::Error::Cancelled => StepError::Cancelled,
            convert::Error::NeedsPassword => StepError::Refused("needs a password".into()),
            convert::Error::BadPassword => StepError::Refused("is protected with a password".into()),
            e => StepError::failed(e),
        }
    }
}

pub enum Outcome {
    /// `output` holds the next working file.
    Wrote,
    /// The step found nothing to do; the working file stays.
    Unchanged,
}

/// A file a step made besides the working PDF, still in staging.
pub(crate) enum Product {
    File { staged: PathBuf, ext: &'static str },
    Folder { staged: PathBuf, label: &'static str },
}

/// What a step sees of the run.
pub(crate) struct Cx<'a> {
    pub env: &'a Env,
    pub cancel: &'a AtomicBool,
    pub stage: &'a Staging,
    /// The source's name without its extension.
    pub stem: &'a str,
    pub index: usize,
    /// Bates numbers: the next free number per Bates step, from the files finished so far.
    pub bates_next: &'a HashMap<usize, u64>,
    /// The numbers this file used (kept when the file succeeds).
    pub bates_used: Vec<(usize, u64)>,
    pub products: Vec<Product>,
    pub notes: Vec<String>,
    /// Things the person should look at in the result even though the file was processed.
    pub warnings: Vec<String>,
}

pub(super) trait StepImpl {
    /// What the step is called in the run's progress and report.
    fn label(&self) -> String;
    /// True when the step rewrites the file (a signature would no longer validate).
    fn breaks_signature(&self) -> bool {
        true
    }
    fn validate(&self, _env: &Env) -> Result<(), Problem> {
        Ok(())
    }
    /// Reads `input`; writes the next file at `output` (which does not exist) and says so.
    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError>;
}

// ---------------------------------------------------------------- shared helpers

fn open_in(path: &Path) -> Result<PdfDocument, StepError> {
    Ok(open_source(&path.to_string_lossy(), None)?)
}

/// Opens the written file again: it has the pages it should and the first and last load.
fn verify_written(path: &Path, pages: u32, password: Option<&str>) -> Result<(), StepError> {
    let bad = || StepError::Failed("the written file did not read back correctly".into());
    let mut d = Document::open(path.to_string_lossy().as_ref()).map_err(|_| bad())?;
    if d.needs_password()? && !password.is_some_and(|pw| d.authenticate(pw).unwrap_or(false)) {
        return Err(bad());
    }
    if d.page_count()?.max(0) as u32 != pages {
        return Err(bad());
    }
    for p in [0, pages.saturating_sub(1)] {
        d.load_page(p as i32).map_err(|_| bad())?;
    }
    Ok(())
}

/// Writes `doc` at `out` and reads it back.
fn finish(doc: &PdfDocument, out: &Path, opts: PdfWriteOptions, password: Option<&str>) -> Result<Outcome, StepError> {
    let pages = doc.page_count()?.max(0) as u32;
    let bytes = write_doc(doc, opts, 1 << 20)?;
    std::fs::write(out, &bytes)?;
    verify_written(out, pages, password)?;
    Ok(Outcome::Wrote)
}

fn all_pages(doc: &PdfDocument) -> Result<Vec<u32>, StepError> {
    Ok((0..doc.page_count()?.max(0) as u32).collect())
}

/// A one-page document to try a mark's settings on.
fn probe() -> Result<PdfDocument, String> {
    let bytes = crate::engine::fixture::build(&crate::engine::fixture::Fixture { pages: 1, ..Default::default() });
    PdfDocument::from_bytes(&bytes).map_err(|e| e.to_string())
}

/// The marks engine checks its own settings while adding; asking it on a scratch page reports a bad
/// setting at validation time with the engine's own words.
fn probe_mark(mark: &Mark, date: Date) -> Result<(), Problem> {
    let mut doc = probe().map_err(Problem::Invalid)?;
    marks::add(&mut doc, &AddRequest { mark: mark.clone(), pages: vec![0], replace: false, date })
        .map(drop)
        .map_err(|e| Problem::Invalid(e.to_string()))
}

fn stamp(cx: &Cx, input: &Path, output: &Path, mark: Mark) -> Result<Outcome, StepError> {
    let mut doc = open_in(input)?;
    let pages = all_pages(&doc)?;
    // A second run over the same file updates the mark instead of stamping twice.
    marks::add(&mut doc, &AddRequest { mark, pages, replace: true, date: cx.env.date })?;
    finish(&doc, output, rewrite_opts(), None)
}

fn no_pictures(picture: &Option<marks::Picture>) -> Result<(), Problem> {
    match picture {
        Some(_) => Err(Problem::Invalid(
            "A picture watermark names a file by a token that does not outlast the session. Use text.".into(),
        )),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------- steps

pub(super) struct InvalidStep;

impl StepImpl for InvalidStep {
    fn label(&self) -> String {
        "Unknown step".into()
    }
    fn run(&self, _cx: &mut Cx, _input: &Path, _output: &Path) -> Result<Outcome, StepError> {
        Err(StepError::Failed("the action has a step that cannot run".into()))
    }
}

/// `en`, `en-US`, `sr-Latn-RS`, `eng`: letters, then groups of letters and digits after `-` or `_`.
pub(super) fn language_tag_ok(tag: &str) -> bool {
    let mut parts = tag.split(['-', '_']);
    let first = parts.next().unwrap_or("");
    (2..=8).contains(&first.len())
        && first.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn same_language(a: &str, b: &str) -> bool {
    let primary = |t: &str| t.split(['-', '_']).next().unwrap_or(t).to_ascii_lowercase();
    primary(a) == primary(b)
}

impl StepImpl for CompressStep {
    fn label(&self) -> String {
        let p = match self.preset {
            compress::Preset::Smallest => "smallest",
            compress::Preset::Balanced => "balanced",
            compress::Preset::HighQuality => "high quality",
            compress::Preset::Custom => "custom",
        };
        format!("Compress ({p})")
    }

    fn validate(&self, _env: &Env) -> Result<(), Problem> {
        match self.preset {
            compress::Preset::Custom => Err(Problem::Invalid("Choose Smallest, Balanced or High quality.".into())),
            _ => Ok(()),
        }
    }

    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        let mut opts = compress::Options::preset(self.preset);
        opts.output = Some(output.to_path_buf());
        let r = compress::compress(input, &opts, cx.cancel, &mut |_| {})?;
        if r.kept_original {
            // Nothing got smaller: the output is the input's bytes again, so drop it.
            let _ = std::fs::remove_file(output);
            cx.notes.push("already as small as it gets".into());
            return Ok(Outcome::Unchanged);
        }
        cx.notes.push(format!("{:.0}% smaller", r.saved_percent));
        Ok(Outcome::Wrote)
    }
}

impl StepImpl for OcrStep {
    fn label(&self) -> String {
        "Recognize text (OCR)".into()
    }

    fn validate(&self, env: &Env) -> Result<(), Problem> {
        let backend = env.backend(self.backend).map_err(Problem::Unavailable)?;
        if self.languages.len() > 8 {
            return Err(Problem::Invalid("Choose at most 8 languages.".into()));
        }
        // A tag becomes part of a command line and a file name inside Tesseract's data folder.
        if let Some(bad) = self.languages.iter().find(|l| !language_tag_ok(l)) {
            return Err(Problem::Invalid(format!("\"{bad}\" is not a language tag (for example en-US).")));
        }
        if !self.languages.is_empty() {
            let have = backend.languages().map_err(|e| Problem::Unavailable(e.to_string()))?;
            if let Some(missing) = self.languages.iter().find(|l| !have.iter().any(|h| same_language(l, h))) {
                return Err(Problem::Unavailable(format!("The OCR language \"{missing}\" is not installed.")));
            }
        }
        Ok(())
    }

    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        let backend = cx.env.backend(self.backend).map_err(StepError::Failed)?;
        let opts = ocr::Options { languages: self.languages.clone(), force: self.force, ..ocr::Options::default() };
        let r = ocr::ocr_file_with(input, output, false, backend.as_ref(), &opts, &|_| {}, cx.cancel)?;
        if r.pages_recognized == 0 {
            let _ = std::fs::remove_file(output);
            cx.notes.push("no page needed text".into());
            return Ok(Outcome::Unchanged);
        }
        cx.notes.push(format!("{} page(s) recognized", r.pages_recognized));
        if r.pages_failed > 0 {
            cx.notes.push(format!("{} page(s) could not be read", r.pages_failed));
        }
        Ok(Outcome::Wrote)
    }
}

impl StepImpl for marks::HeaderFooter {
    fn label(&self) -> String {
        "Add header and footer".into()
    }
    fn validate(&self, env: &Env) -> Result<(), Problem> {
        probe_mark(&Mark::HeaderFooter(self.clone()), env.date)
    }
    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        stamp(cx, input, output, Mark::HeaderFooter(self.clone()))
    }
}

impl StepImpl for marks::Watermark {
    fn label(&self) -> String {
        "Add watermark".into()
    }
    fn validate(&self, env: &Env) -> Result<(), Problem> {
        no_pictures(&self.picture)?;
        probe_mark(&Mark::Watermark(self.clone()), env.date)
    }
    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        stamp(cx, input, output, Mark::Watermark(self.clone()))
    }
}

impl StepImpl for BatesStep {
    fn label(&self) -> String {
        "Add Bates numbers".into()
    }
    fn validate(&self, env: &Env) -> Result<(), Problem> {
        probe_mark(&Mark::Bates(self.bates.clone()), env.date)
    }
    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        let start = match (self.continue_across_files, cx.bates_next.get(&cx.index)) {
            (true, Some(n)) => *n,
            _ => self.bates.start,
        };
        let mut doc = open_in(input)?;
        let pages = all_pages(&doc)?;
        let count = pages.len() as u64;
        let bates = marks::Bates { start, ..self.bates.clone() };
        marks::add(&mut doc, &AddRequest { mark: Mark::Bates(bates), pages, replace: true, date: cx.env.date })?;
        cx.bates_used.push((cx.index, start + count));
        cx.notes.push(format!(
            "numbered {} to {}",
            marks::bates_text(&self.bates, start),
            marks::bates_text(&self.bates, start + count.saturating_sub(1))
        ));
        finish(&doc, output, rewrite_opts(), None)
    }
}

impl StepImpl for ProtectStep {
    fn label(&self) -> String {
        "Protect with a password".into()
    }

    fn validate(&self, _env: &Env) -> Result<(), Problem> {
        let spec = self.spec();
        spec.validate().map_err(|e| {
            if e == protect::SpecError::NothingToSet {
                Problem::Invalid("Type the password for this step: a stored action does not keep passwords.".into())
            } else {
                Problem::Invalid(e.message().into())
            }
        })
    }

    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        let spec = Arc::new(self.spec());
        spec.validate().map_err(|e| StepError::Failed(e.message().into()))?;
        let doc = open_in(input)?;
        let opts = protect::write_options(rewrite_opts(), &SecurityChange::Set(spec.clone()));
        let r = finish(&doc, output, opts, spec.open());
        cx.notes.push("protected with a password".into());
        r
    }
}

fn removed(r: &SanitizeReport) -> u32 {
    r.metadata
        + r.hidden_layer_blocks
        + r.hidden_text_pages
        + r.scripts_actions
        + r.embedded_files
        + r.annotations
        + r.form_fields
        + r.bookmarks
        + r.quoting
}

/// Pages (zero-based) with no text at all: scans and pictures, where a search finds nothing.
fn pages_without_text(doc: &PdfDocument) -> Result<Vec<u32>, StepError> {
    let mut out = Vec::new();
    for p in 0..doc.page_count()?.max(0) {
        let text = doc.load_page(p)?.to_text_page(mupdf::TextPageFlags::empty())?.to_text()?;
        if text.trim().is_empty() {
            out.push(p as u32);
        }
    }
    Ok(out)
}

impl StepImpl for redact::Sanitize {
    fn label(&self) -> String {
        "Sanitize hidden information".into()
    }

    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        let mut doc = open_in(input)?;
        let r = redact::sanitize(&mut doc, self, &[])?;
        if r == SanitizeReport::default() {
            cx.notes.push("nothing to remove".into());
            return Ok(Outcome::Unchanged);
        }
        cx.notes.push(format!("{} item(s) removed", removed(&r)));
        finish(&doc, output, redact::write_options(), None)
    }
}

impl StepImpl for RedactStep {
    fn label(&self) -> String {
        "Redact by search".into()
    }

    fn validate(&self, _env: &Env) -> Result<(), Problem> {
        if self.queries.is_empty() {
            return Err(Problem::Invalid("Add something to search for.".into()));
        }
        for q in &self.queries {
            match q {
                RedactQuery::Pattern { .. } => {}
                RedactQuery::Text { text, .. } if text.trim().is_empty() => {
                    return Err(Problem::Invalid("A search text is empty.".into()));
                }
                RedactQuery::Text { .. } => {}
                RedactQuery::Regex { regex, case_sensitive } => {
                    if regex.is_empty() {
                        return Err(Problem::Invalid("A search expression is empty.".into()));
                    }
                    regex::RegexBuilder::new(regex)
                        .case_insensitive(!case_sensitive)
                        .build()
                        .map_err(|e| Problem::Invalid(format!("\"{regex}\" is not a valid expression: {e}")))?;
                }
            }
        }
        if self.overlay_text.as_deref().is_some_and(|t| t.chars().count() > 100 || t.chars().any(char::is_control)) {
            return Err(Problem::Invalid("The text on the boxes is too long or has control characters.".into()));
        }
        Ok(())
    }

    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        let mut doc = open_in(input)?;
        let blind = pages_without_text(&doc)?;
        if !blind.is_empty() {
            let shown: Vec<String> = blind.iter().take(5).map(|p| (p + 1).to_string()).collect();
            cx.warnings.push(format!(
                "{} page(s) have no text to search ({}{}): scanned pages need \"Make searchable\" before they can be redacted by search.",
                blind.len(),
                shown.join(", "),
                if blind.len() > 5 { ", ..." } else { "" }
            ));
        }
        let mut hits = Vec::new();
        for q in &self.queries {
            let query = match q {
                RedactQuery::Pattern { pattern } => Query::Pattern(*pattern),
                RedactQuery::Text { text, case_sensitive, whole_word } => {
                    Query::Text { needle: text.clone(), case_sensitive: *case_sensitive, whole_word: *whole_word }
                }
                RedactQuery::Regex { regex, case_sensitive } => Query::Regex { pattern: regex.clone(), case_sensitive: *case_sensitive },
            };
            hits.extend(redact::find(&doc, &query)?);
        }
        if hits.is_empty() {
            // Nothing on the pages, but a value can sit in the properties, a form field or a comment:
            // the sanitize switches still apply.
            let r = redact::sanitize(&mut doc, &self.sanitize, &[])?;
            if r == SanitizeReport::default() {
                cx.notes.push("nothing found to redact".into());
                return Ok(Outcome::Unchanged);
            }
            cx.notes.push(format!("nothing found on the pages; {} hidden item(s) removed", removed(&r)));
            return finish(&doc, output, redact::write_options(), None);
        }
        let style = MarkStyle { overlay_text: self.overlay_text.clone().filter(|t| !t.is_empty()), ..MarkStyle::default() };
        redact::mark_hits(&mut doc, &hits, &style)?;
        // Apply removes the content, sanitizes, and reads a full rewrite back for the redacted text;
        // those verified bytes are what is written, no other write of the document is.
        let opts = ApplyOptions { sanitize: self.sanitize, ..ApplyOptions::default() };
        let report = redact::apply(&mut doc, &opts)?;
        std::fs::write(output, &report.bytes)?;
        verify_written(output, doc.page_count()?.max(0) as u32, None)?;
        cx.notes.push(format!("{} item(s) redacted on {} page(s)", report.marks, report.pages.len()));
        Ok(Outcome::Wrote)
    }
}

impl StepImpl for FlattenFormsStep {
    fn label(&self) -> String {
        "Flatten form fields".into()
    }

    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        let mut doc = open_in(input)?;
        let fields = crate::forms::list(&doc)?.fields.len();
        if fields == 0 {
            cx.notes.push("no form fields".into());
            return Ok(Outcome::Unchanged);
        }
        // Widgets are drawn into the page content and removed; annotations (comments) stay.
        doc.bake(false, true)?;
        cx.notes.push(format!("{fields} field(s) flattened"));
        finish(&doc, output, redact::write_options(), None)
    }
}

impl StepImpl for ExportImagesStep {
    fn label(&self) -> String {
        "Export pages as images".into()
    }

    fn breaks_signature(&self) -> bool {
        false
    }

    fn validate(&self, _env: &Env) -> Result<(), Problem> {
        if !(36..=1200).contains(&self.dpi) || !(1..=100).contains(&self.quality) {
            return Err(Problem::Invalid("Resolution must be 36 to 1200 dpi and JPEG quality 1 to 100.".into()));
        }
        Ok(())
    }

    fn run(&self, cx: &mut Cx, input: &Path, _output: &Path) -> Result<Outcome, StepError> {
        let dir = cx.stage.path(&format!("export-{}", cx.index));
        std::fs::create_dir(&dir)?;
        let doc = Document::open(input.to_string_lossy().as_ref())?;
        let kind = ExportKind::Images { format: self.format, dpi: self.dpi, gray: self.gray, quality: self.quality };
        let out = convert::export_document(&doc, cx.stem, &kind, None, Target::Folder(dir.clone()), cx.cancel, &mut |_| {})?;
        cx.notes.push(format!("{} image(s)", out.files.len()));
        cx.products.push(Product::Folder { staged: dir, label: "images" });
        Ok(Outcome::Unchanged)
    }
}

impl StepImpl for ExportTextStep {
    fn label(&self) -> String {
        "Export text".into()
    }

    fn breaks_signature(&self) -> bool {
        false
    }

    fn run(&self, cx: &mut Cx, input: &Path, _output: &Path) -> Result<Outcome, StepError> {
        let dir = cx.stage.path(&format!("export-{}", cx.index));
        std::fs::create_dir(&dir)?;
        let file = dir.join("text.txt");
        let doc = Document::open(input.to_string_lossy().as_ref())?;
        let kind = ExportKind::Text { layout: self.layout };
        convert::export_document(&doc, cx.stem, &kind, None, Target::File(file.clone()), cx.cancel, &mut |_| {})?;
        cx.products.push(Product::File { staged: file, ext: "txt" });
        Ok(Outcome::Unchanged)
    }
}

/// A PDF text string: ASCII as is, anything else as UTF-16BE with a byte order mark.
fn text_obj(doc: &PdfDocument, s: &str) -> Result<PdfObject, StepError> {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    if s.is_ascii() {
        return Ok(PdfObject::new_string(&s)?);
    }
    let hex: String = s.encode_utf16().map(|u| format!("{u:04X}")).collect();
    Ok(doc.new_object_from_str(&format!("<FEFF{hex}>"))?)
}

impl StepImpl for PropertiesStep {
    fn label(&self) -> String {
        "Set document properties".into()
    }

    fn validate(&self, _env: &Env) -> Result<(), Problem> {
        let all = self.entries();
        if all.iter().all(|(_, v)| v.is_none()) {
            return Err(Problem::Invalid("Fill in at least one property.".into()));
        }
        if all.iter().any(|(_, v)| v.is_some_and(|v| v.chars().count() > 2000)) {
            return Err(Problem::Invalid("A property is too long (2000 characters at most).".into()));
        }
        Ok(())
    }

    fn run(&self, cx: &mut Cx, input: &Path, output: &Path) -> Result<Outcome, StepError> {
        let mut doc = open_in(input)?;
        let mut info = match doc.trailer()?.get_dict("Info")?.filter(|i| i.is_dict().unwrap_or(false)) {
            Some(i) => i,
            None => {
                let i = doc.add_object(&doc.new_dict()?)?;
                doc.trailer()?.dict_put("Info", i.clone())?;
                i
            }
        };
        for (key, value) in self.entries() {
            match value {
                None => {}
                Some("") => {
                    if info.get_dict(key)?.is_some() {
                        info.dict_delete(key)?;
                    }
                }
                Some(v) => info.dict_put(key, text_obj(&doc, v)?)?,
            }
        }
        cx.notes.push("properties set".into());
        finish(&doc, output, rewrite_opts(), None)
    }
}

impl PropertiesStep {
    fn entries(&self) -> [(&'static str, Option<&str>); 6] {
        [
            ("Title", self.title.as_deref()),
            ("Author", self.author.as_deref()),
            ("Subject", self.subject.as_deref()),
            ("Keywords", self.keywords.as_deref()),
            ("Creator", self.creator.as_deref()),
            ("Producer", self.producer.as_deref()),
        ]
    }
}
