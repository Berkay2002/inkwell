//! OCR: recognise the text on scanned pages and add it as an invisible text layer.
//!
//! Two stages, so the edit core can run the second one on its serial document lane
//! (docs/engine-rules.md rule 1):
//!
//! 1. [`recognize`] reads the pages (each worker opens its own read-only copy of the file),
//!    renders them, runs a [`backend::Recognizer`] and returns [`PageResult`]s. Parallel,
//!    cancellable, reports progress, never mutates anything.
//! 2. [`apply`] adds the results to a `PdfDocument` as one extra content stream per page and a
//!    single shared font. Page images, MediaBox and CropBox are left alone.
//!
//! [`ocr_file`] is both stages plus a save, for callers that just want "scan in, searchable out".

pub mod backend;
mod glyphless;
mod layer;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use mupdf::{Colorspace, Matrix, TextPageFlags};
use serde::Serialize;

use backend::{Bitmap, RawPage, Recognizer};

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    #[error("{0}")]
    Pdf(String),
    #[error("{0}")]
    Backend(String),
    #[error("OCR language not installed: {0}")]
    LanguageUnavailable(String),
    #[error("page {0} is out of range")]
    BadPage(u32),
    #[error("{0}")]
    Io(String),
    #[error("cancelled")]
    Cancelled,
}

impl From<mupdf::Error> for OcrError {
    fn from(e: mupdf::Error) -> Self {
        OcrError::Pdf(e.to_string())
    }
}

/// A recognised word; `bbox` is `[x0, y0, x1, y1]` in points in the page's *text frame*: MuPDF
/// page space (y down, origin at the top-left of the visible page, `/Rotate` and CropBox applied),
/// turned by [`PageResult::skew_deg`] about the page centre when the engine deskews the page first
/// (Windows OCR does; its boxes are for the straightened page). Use
/// [`PageResult::words_on_page`] for boxes on the page as displayed.
#[derive(Clone, Debug, Serialize)]
pub struct Word {
    pub text: String,
    pub bbox: [f32; 4],
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
pub enum PageStatus {
    /// Words were found; they are in `lines`.
    Recognized,
    /// The page already has real text and `force` was not set.
    SkippedHasText,
    /// The page was read but the engine found no text (blank page, picture).
    NoText,
    Failed(String),
}

#[derive(Clone, Debug, Serialize)]
pub struct PageResult {
    /// Zero-based page index.
    pub page: u32,
    pub status: PageStatus,
    /// Recognised lines in reading order, each a list of words.
    pub lines: Vec<Vec<Word>>,
    /// Skew of the text the engine reports (degrees, clockwise positive); `None` if the page is
    /// straight or the engine does not say. Detection only: the page image is never changed, the
    /// text layer is rotated to follow the scan instead.
    pub skew_deg: Option<f32>,
    /// Displayed page size in points (the text frame rotates about its centre).
    pub page_size: [f32; 2],
    pub ms: u32,
}

impl PageResult {
    /// Maps a text-frame point to the displayed page.
    pub(crate) fn to_page(&self, x: f32, y: f32) -> (f32, f32) {
        let Some(deg) = self.skew_deg else { return (x, y) };
        let (s, c) = deg.to_radians().sin_cos();
        let (cx, cy) = (self.page_size[0] / 2.0, self.page_size[1] / 2.0);
        let (dx, dy) = (x - cx, y - cy);
        (cx + dx * c - dy * s, cy + dx * s + dy * c)
    }

    /// Recognised words as axis-aligned boxes on the page as displayed, in reading order.
    pub fn words_on_page(&self) -> Vec<Vec<Word>> {
        self.lines
            .iter()
            .map(|l| {
                l.iter()
                    .map(|w| {
                        let [x0, y0, x1, y1] = w.bbox;
                        let p = [self.to_page(x0, y0), self.to_page(x1, y0), self.to_page(x1, y1), self.to_page(x0, y1)];
                        let xs = p.map(|q| q.0);
                        let ys = p.map(|q| q.1);
                        let min = |v: [f32; 4]| v.into_iter().fold(f32::MAX, f32::min);
                        let max = |v: [f32; 4]| v.into_iter().fold(f32::MIN, f32::max);
                        Word { text: w.text.clone(), bbox: [min(xs), min(ys), max(xs), max(ys)] }
                    })
                    .collect()
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// Zero-based page indices to read; `None` = every page. See [`parse_page_range`].
    pub pages: Option<Vec<u32>>,
    /// BCP-47 tags in priority order (`"en-US"`, `"sv-SE"`); empty = the backend's default.
    pub languages: Vec<String>,
    /// Render resolution handed to the engine (the scan is not modified). Clamped to what the
    /// backend accepts. 200 matched 300 on every benchmark case (and Tesseract's layout analysis
    /// was better at 200 on the table) at 1.5-2x the speed.
    pub dpi: u32,
    /// Read pages even if they already have text, and replace an earlier layer of ours.
    pub force: bool,
    /// Pages recognised at once.
    pub workers: usize,
}

impl Default for Options {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self { pages: None, languages: Vec::new(), dpi: 200, force: false, workers: (cores / 2).clamp(1, 6) }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    /// The page that just finished (zero-based).
    pub page: u32,
}

pub enum Input<'a> {
    Path(&'a Path),
    Bytes(&'a [u8]),
}

impl Input<'_> {
    fn open(&self) -> Result<PdfDocument, OcrError> {
        let doc = match self {
            Input::Path(p) => PdfDocument::open(p.to_string_lossy().as_ref())?,
            Input::Bytes(b) => PdfDocument::from_bytes(b)?,
        };
        if doc.needs_password()? {
            return Err(OcrError::Pdf("document is password protected".into()));
        }
        Ok(doc)
    }
}

pub struct Recognition {
    /// One entry per requested page that was reached, ordered by page index.
    pub pages: Vec<PageResult>,
    pub cancelled: bool,
    pub backend: &'static str,
    pub total_ms: u64,
}

/// Fewer non-space characters than this count as "no text" (a Bates number or a stamp on a scan).
const MIN_TEXT_CHARS: usize = 12;

/// `"1-3,5,8-"` (one-based, `-` open ends, `,` or `;` separators) to zero-based indices.
pub fn parse_page_range(spec: &str, page_count: u32) -> Result<Vec<u32>, OcrError> {
    let bad = || OcrError::Io(format!("bad page range: {spec}"));
    let mut out = Vec::new();
    for part in spec.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => {
                let a = if a.trim().is_empty() { 1 } else { a.trim().parse::<u32>().map_err(|_| bad())? };
                let b = if b.trim().is_empty() { page_count } else { b.trim().parse::<u32>().map_err(|_| bad())? };
                (a, b)
            }
            None => {
                let n = part.parse::<u32>().map_err(|_| bad())?;
                (n, n)
            }
        };
        if a == 0 || b < a || b > page_count {
            return Err(bad());
        }
        out.extend(a - 1..b);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

pub fn page_count(input: &Input) -> Result<u32, OcrError> {
    Ok(input.open()?.page_count()? as u32)
}

/// Renders page `index` as an 8-bit grey bitmap at (at most) `dpi`; returns it with the scale
/// from page points to bitmap pixels.
fn render(doc: &PdfDocument, index: u32, dpi: u32, max_dim: u32) -> Result<(Bitmap, f32), OcrError> {
    let page = doc.load_page(index as i32)?;
    let b = page.bounds()?;
    let (pw, ph) = (b.x1 - b.x0, b.y1 - b.y0);
    let mut scale = dpi.max(36) as f32 / 72.0;
    let longest = pw.max(ph) * scale;
    if longest > max_dim as f32 {
        scale *= max_dim as f32 / longest;
    }
    let pix = page.to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_gray(), false, false)?;
    let (w, h, stride) = (pix.width() as usize, pix.height() as usize, pix.stride() as usize);
    let n = pix.n() as usize;
    let s = pix.samples();
    let mut pixels = Vec::with_capacity(w * h);
    for row in 0..h {
        let r = &s[row * stride..row * stride + w * n];
        if n == 1 {
            pixels.extend_from_slice(r);
        } else {
            pixels.extend(r.chunks_exact(n).map(|p| p[0]));
        }
    }
    Ok((Bitmap { w: w as u32, h: h as u32, pixels }, scale))
}

fn page_has_text(doc: &PdfDocument, index: u32) -> Result<bool, OcrError> {
    let page = doc.load_page(index as i32)?;
    let text = page.to_text_page(TextPageFlags::empty())?.to_text()?;
    Ok(text.chars().filter(|c| !c.is_whitespace()).count() >= MIN_TEXT_CHARS)
}

/// Engines return typographic dashes for the hyphens of monospace and sans fonts (Windows OCR
/// reads `INV-2026` in Courier as `INV\u{2014}2026`). Inside a token that also has a number the
/// dash is almost always a hyphen-minus, and the ASCII form is what users search for.
fn normalize(text: String) -> String {
    const DASHES: [char; 7] = ['\u{2010}', '\u{2011}', '\u{2012}', '\u{2013}', '\u{2014}', '\u{2015}', '\u{2212}'];
    if text.chars().any(|c| c.is_ascii_digit()) && text.chars().any(|c| DASHES.contains(&c)) {
        text.chars().map(|c| if DASHES.contains(&c) { '-' } else { c }).collect()
    } else {
        text
    }
}

/// Below this the page counts as straight.
const MIN_SKEW_DEG: f32 = 0.15;
/// Above this the engine is reporting an orientation, not a skew; the text frame stays unrotated.
const MAX_SKEW_DEG: f32 = 20.0;

/// Joins `$1` + `,234.56` (Windows OCR splits a number at its separator, and so did Acrobat in the
/// study) when the two boxes nearly touch: a real space is wider than the gap in a number.
fn join_number_fragments(words: Vec<Word>) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::with_capacity(words.len());
    for w in words {
        if let Some(p) = out.last_mut() {
            // Only the separator-leading fragment: "3. 4 items" must stay two words.
            let mut it = w.text.chars();
            let leads_with_sep = it.next().is_some_and(|c| matches!(c, ',' | '.')) && it.next().is_some_and(|c| c.is_ascii_digit());
            let height = (p.bbox[3] - p.bbox[1]).max(1.0);
            if leads_with_sep && p.text.ends_with(|c: char| c.is_ascii_digit()) && w.bbox[0] - p.bbox[2] < 0.6 * height {
                p.text.push_str(&w.text);
                p.bbox = [p.bbox[0].min(w.bbox[0]), p.bbox[1].min(w.bbox[1]), p.bbox[2].max(w.bbox[2]), p.bbox[3].max(w.bbox[3])];
                continue;
            }
        }
        out.push(w);
    }
    out
}

fn to_words(raw: RawPage, scale: f32) -> (Vec<Vec<Word>>, Option<f32>) {
    let lines = raw
        .lines
        .into_iter()
        .map(|l| {
            let words = l.words.into_iter().map(|w| Word { text: normalize(w.text), bbox: w.rect.map(|v| v / scale) }).collect();
            join_number_fragments(words)
        })
        .filter(|l| !l.is_empty())
        .collect();
    let skew = raw.angle_deg.filter(|a| a.abs() >= MIN_SKEW_DEG && a.abs() <= MAX_SKEW_DEG);
    (lines, skew)
}

fn read_page(
    doc: &PdfDocument,
    backend: &dyn Recognizer,
    opts: &Options,
    index: u32,
) -> Result<(PageStatus, Vec<Vec<Word>>, Option<f32>, [f32; 2]), OcrError> {
    let b = doc.load_page(index as i32)?.bounds()?;
    let size = [b.x1 - b.x0, b.y1 - b.y0];
    if !opts.force && page_has_text(doc, index)? {
        return Ok((PageStatus::SkippedHasText, Vec::new(), None, size));
    }
    let (bmp, scale) = render(doc, index, opts.dpi, backend.max_dimension())?;
    let raw = backend.recognize(&bmp, &opts.languages)?;
    let (lines, skew) = to_words(raw, scale);
    let status = if lines.is_empty() { PageStatus::NoText } else { PageStatus::Recognized };
    Ok((status, lines, skew, size))
}

/// Stage 1. Never writes. `progress` is called from worker threads after each page; set `cancel`
/// to stop: workers finish the page they are on and no further page starts.
pub fn recognize(
    input: &Input,
    backend: &dyn Recognizer,
    opts: &Options,
    progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
) -> Result<Recognition, OcrError> {
    let started = Instant::now();
    let count = page_count(input)?;
    let pages: Vec<u32> = match &opts.pages {
        Some(p) => {
            if let Some(&bad) = p.iter().find(|&&i| i >= count) {
                return Err(OcrError::BadPage(bad));
            }
            p.clone()
        }
        None => (0..count).collect(),
    };
    let total = pages.len() as u32;
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<PageResult>> = Mutex::new(Vec::with_capacity(pages.len()));
    let workers = opts.workers.clamp(1, pages.len().max(1));

    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                // A worker whose document cannot be opened reports every page it would have taken.
                let doc = input.open();
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&page) = pages.get(i) else { return };
                    let t = Instant::now();
                    let r = match &doc {
                        Ok(d) => {
                            // A backend panic must not take the whole run (and the UI) with it.
                            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                read_page(d, backend, opts, page)
                            }));
                            match run {
                                Ok(r) => r,
                                Err(_) => Err(OcrError::Backend("recognizer crashed".into())),
                            }
                        }
                        Err(e) => Err(OcrError::Pdf(e.to_string())),
                    };
                    let (status, lines, skew_deg, page_size) = match r {
                        Ok(v) => v,
                        Err(e) => (PageStatus::Failed(e.to_string()), Vec::new(), None, [0.0, 0.0]),
                    };
                    results.lock().unwrap().push(PageResult {
                        page,
                        status,
                        lines,
                        skew_deg,
                        page_size,
                        ms: t.elapsed().as_millis() as u32,
                    });
                    let n = done.fetch_add(1, Ordering::Relaxed) as u32 + 1;
                    progress(Progress { done: n, total, page });
                }
            });
        }
    });

    let mut pages_out = results.into_inner().unwrap();
    pages_out.sort_by_key(|r| r.page);
    Ok(Recognition {
        cancelled: cancel.load(Ordering::Relaxed) && (pages_out.len() as u32) < total,
        pages: pages_out,
        backend: backend.name(),
        total_ms: started.elapsed().as_millis() as u64,
    })
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ApplyReport {
    pub pages_written: u32,
    pub words: u32,
    /// Earlier Inkwell layers removed because `replace` was set.
    pub layers_replaced: u32,
}

/// Stage 2. Adds the recognised text of every `Recognized` result to `doc`. With `replace`, a
/// layer an earlier OCR run of ours left on the page is removed first (the user's own content is
/// never touched). The caller saves; see [`save`].
pub fn apply(doc: &mut PdfDocument, results: &[PageResult], replace: bool) -> Result<ApplyReport, OcrError> {
    let todo: Vec<&PageResult> = results.iter().filter(|r| r.status == PageStatus::Recognized).collect();
    let mut report = ApplyReport::default();
    if todo.is_empty() {
        return Ok(report);
    }
    let max_code = todo
        .iter()
        .flat_map(|r| r.lines.iter().flatten())
        .flat_map(|w| w.text.encode_utf16())
        .fold(0u32, |a, u| a.max(u as u32));
    let font = layer::install_font(doc, max_code)?;
    for r in todo {
        let mut page = doc.load_pdf_page(r.page as i32)?;
        if replace {
            report.layers_replaced += layer::strip_layers(doc, &mut page)? as u32;
        }
        // Words are in displayed-page space; the layer lives in default user space.
        let page_from_user = page.ctm()?;
        let user_from_page = page_from_user
            .invert()
            .ok_or_else(|| OcrError::Pdf(format!("page {} has a singular transform", r.page + 1)))?;
        let (stream, _) = layer::layer_stream(r, &user_from_page);
        layer::add_layer(doc, &mut page, &font, &stream)?;
        report.pages_written += 1;
        report.words += r.lines.iter().map(|l| l.len() as u32).sum::<u32>();
    }
    Ok(report)
}

/// Writes `doc` to `path`. Incremental when the file allows it (the original bytes, images
/// included, stay exactly as they were and a 100-page scan is not rewritten), otherwise a full
/// write that does not touch stream encodings.
pub fn save(doc: &PdfDocument, path: &Path) -> Result<(), OcrError> {
    let mut o = PdfWriteOptions::default();
    if doc.can_be_saved_incrementally() {
        o.set_incremental(true);
    } else {
        o.set_compress_images(false).set_compress_fonts(false).set_decompress(false);
    }
    doc.save_with_options(path.to_string_lossy().as_ref(), o)?;
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub backend: &'static str,
    pub pages_requested: u32,
    pub pages_recognized: u32,
    pub pages_skipped_text: u32,
    pub pages_no_text: u32,
    pub pages_failed: u32,
    pub words: u32,
    pub total_ms: u64,
    pub pages: Vec<PageSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PageSummary {
    pub page: u32,
    pub status: PageStatus,
    pub words: u32,
    pub skew_deg: Option<f32>,
    pub ms: u32,
}

/// Scan in, searchable copy out: recognise, add the layer to a copy of `input`, save to `output`.
/// `input` and `output` must differ. On cancel nothing is written and `Err(Cancelled)` is returned.
pub fn ocr_file(
    input: &Path,
    output: &Path,
    backend: &dyn Recognizer,
    opts: &Options,
    progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
) -> Result<Report, OcrError> {
    if input == output {
        return Err(OcrError::Io("output must be a different file than the input".into()));
    }
    let rec = recognize(&Input::Path(input), backend, opts, progress, cancel)?;
    if rec.cancelled {
        return Err(OcrError::Cancelled);
    }
    // Work on a temp copy beside the output: an incremental save appends to the file it was
    // opened from, and a failure must never leave a half-written output.
    let tmp: PathBuf = output.with_extension("ocr-tmp");
    let mut applied = ApplyReport::default();
    let r = (|| {
        std::fs::copy(input, &tmp).map_err(|e| OcrError::Io(e.to_string()))?;
        // Windows refuses to rename a file that is still open: `doc` lives in its own scope.
        let full = {
            let mut doc = Input::Path(&tmp).open()?;
            applied = apply(&mut doc, &rec.pages, opts.force)?;
            if applied.pages_written == 0 {
                None
            } else if doc.can_be_saved_incrementally() {
                save(&doc, &tmp)?;
                None
            } else {
                let full = output.with_extension("ocr-full");
                save(&doc, &full)?;
                Some(full)
            }
        };
        if let Some(full) = full {
            std::fs::rename(&full, &tmp).map_err(|e| OcrError::Io(e.to_string()))?;
        }
        std::fs::rename(&tmp, output).map_err(|e| OcrError::Io(e.to_string()))
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r?;

    let count = |f: fn(&PageStatus) -> bool| rec.pages.iter().filter(|p| f(&p.status)).count() as u32;
    Ok(Report {
        backend: rec.backend,
        pages_requested: rec.pages.len() as u32,
        pages_recognized: count(|s| *s == PageStatus::Recognized),
        pages_skipped_text: count(|s| *s == PageStatus::SkippedHasText),
        pages_no_text: count(|s| *s == PageStatus::NoText),
        pages_failed: count(|s| matches!(s, PageStatus::Failed(_))),
        words: applied.words,
        total_ms: rec.total_ms,
        pages: rec
            .pages
            .iter()
            .map(|p| PageSummary {
                page: p.page,
                status: p.status.clone(),
                words: p.lines.iter().map(|l| l.len() as u32).sum(),
                skew_deg: p.skew_deg,
                ms: p.ms,
            })
            .collect(),
    })
}
