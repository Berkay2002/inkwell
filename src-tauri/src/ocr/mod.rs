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
mod order;

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
    #[error("output already exists: {0}")]
    OutputExists(String),
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
    /// Degrees (0, 90, 180, 270) the page raster was turned clockwise to read it, because the scan
    /// was fed sideways or upside down. Words are in the turned frame, `to_page` undoes it. The page
    /// itself is never rotated.
    pub rotation: u16,
    /// Displayed page size in points (the text frame rotates about its centre).
    pub page_size: [f32; 2],
    pub ms: u32,
}

impl PageResult {
    /// Maps a text-frame point to the displayed page.
    pub(crate) fn to_page(&self, x: f32, y: f32) -> (f32, f32) {
        let [w, h] = self.page_size;
        // Frame the engine saw: the page turned by `rotation`.
        let (fw, fh) = if self.rotation % 180 == 90 { (h, w) } else { (w, h) };
        let (mut x, mut y) = (x, y);
        if let Some(deg) = self.skew_deg {
            let (s, c) = deg.to_radians().sin_cos();
            let (cx, cy) = (fw / 2.0, fh / 2.0);
            let (dx, dy) = (x - cx, y - cy);
            (x, y) = (cx + dx * c - dy * s, cy + dx * s + dy * c);
        }
        match self.rotation % 360 {
            90 => (y, h - x),
            180 => (w - x, h - y),
            270 => (w - y, x),
            _ => (x, y),
        }
    }

    /// Direction of the text baseline on the displayed page, degrees clockwise (y down).
    pub(crate) fn baseline_deg(&self) -> f32 {
        self.skew_deg.unwrap_or(0.0) - self.rotation as f32
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
    /// Look for pages scanned sideways or upside down when the first read finds little text.
    pub detect_orientation: bool,
    /// Pages recognised at once. The default is half the logical cores, at most 8: Windows OCR
    /// scaled 92 -> 50 -> 26 -> 15 ms/page at 1/2/4/8 workers and only to 12 at 16 (docs/ocr-engine.md).
    pub workers: usize,
}

impl Default for Options {
    fn default() -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        Self { pages: None, languages: Vec::new(), dpi: 200, force: false, detect_orientation: true, workers: (cores / 2).clamp(1, 8) }
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
        .filter(|l: &Vec<Word>| !l.is_empty())
        .collect();
    let lines = order::reading_order(lines);
    let skew = raw.angle_deg.filter(|a| a.abs() >= MIN_SKEW_DEG && a.abs() <= MAX_SKEW_DEG);
    (lines, skew)
}

struct Read {
    status: PageStatus,
    lines: Vec<Vec<Word>>,
    skew_deg: Option<f32>,
    rotation: u16,
    page_size: [f32; 2],
}

/// Very common words of the main Latin-script languages. Text read upside down or sideways comes
/// back as word-shaped junk (`n`/`u`, `p`/`d` swap), but junk is not "the", "och" or "der".
const STOPWORDS: &[&str] = &[
    "the", "and", "of", "to", "in", "is", "that", "for", "with", "on", "are", "as", "was", "be", "by", "it", "this",
    "from", "at", "or", "not", "have", "has", "will", "an", "which", "their", "all", "och", "att", "det", "som",
    "en", "är", "på", "för", "med", "av", "den", "till", "har", "inte", "om", "ett", "de", "vi", "kan", "jag",
    "der", "die", "und", "das", "nicht", "ist", "ein", "zu", "mit", "von", "le", "la", "les", "des", "et", "est",
    "une", "pour", "dans", "que", "qui", "pas", "el", "los", "las", "una", "por", "con", "para", "del", "es",
];

struct Plausibility {
    /// Alphabetic words of three or more letters with a vowel (or a non-Latin letter).
    words: usize,
    chars: usize,
    /// Of those, words whose box is taller than wide: Tesseract reads sideways text where it lies.
    tall: usize,
    stops: usize,
}

impl Plausibility {
    fn of(raw: &RawPage) -> Self {
        let mut p = Plausibility { words: 0, chars: 0, tall: 0, stops: 0 };
        for w in raw.lines.iter().flat_map(|l| &l.words) {
            let t: String = w.text.chars().filter(|c| !matches!(c, '.' | ',' | ':' | ';' | '!' | '?')).collect();
            if !t.is_empty() && STOPWORDS.contains(&t.to_lowercase().as_str()) {
                p.stops += 1;
            }
            let n = t.chars().count();
            let ok = n >= 3
                && t.chars().all(|c| c.is_alphabetic())
                && t.chars().any(|c| "aeiouyAEIOUYåäöÅÄÖéèêáíóúüÜ".contains(c) || !c.is_ascii() && !c.is_uppercase() && !c.is_lowercase());
            if ok {
                p.words += 1;
                p.chars += n;
                if w.rect[3] - w.rect[1] > w.rect[2] - w.rect[0] {
                    p.tall += 1;
                }
            }
        }
        p
    }

    /// Larger = more like real text.
    fn score(&self) -> usize {
        self.chars + 10 * self.stops
    }

    /// Upright running text: enough words, mostly wider than tall, with the usual share of
    /// common words (about 40% in English; junk has a few percent).
    fn healthy(&self) -> bool {
        self.words >= 8 && self.chars >= 60 && self.tall * 2 < self.words && self.stops * 8 >= self.words
    }
}

/// Clockwise degrees to turn `bmp` so its text is upright. `upright` is the read at 0 degrees.
/// A page in a language outside the stopword list pays for three half-size reads and stays at 0
/// unless a turn is a clear win.
fn find_rotation(backend: &dyn Recognizer, bmp: &Bitmap, languages: &[String], upright: &RawPage, cancel: &AtomicBool) -> u16 {
    let first = Plausibility::of(upright);
    if first.healthy() {
        return 0;
    }
    if let Some(r) = backend.osd_rotation(bmp, cancel) {
        return r;
    }
    // No detector: read the page turned each way at half size and keep the clear winner.
    let small = if bmp.w.max(bmp.h) >= 1600 { bmp.halved() } else { bmp.rotated(0) };
    let mut best = (0u16, first.score());
    for r in [90u16, 180, 270] {
        let Ok(raw) = backend.recognize(&small.rotated(r), languages, cancel) else { continue };
        let sc = Plausibility::of(&raw).score();
        if sc > best.1 {
            best = (r, sc);
        }
    }
    // Turning needs a clear win: a handful of short words is not evidence.
    if best.0 != 0 && best.1 >= first.score() * 3 / 2 + 24 { best.0 } else { 0 }
}

fn read_page(doc: &PdfDocument, backend: &dyn Recognizer, opts: &Options, index: u32, cancel: &AtomicBool) -> Result<Read, OcrError> {
    let b = doc.load_page(index as i32)?.bounds()?;
    let page_size = [b.x1 - b.x0, b.y1 - b.y0];
    if !opts.force && page_has_text(doc, index)? {
        return Ok(Read { status: PageStatus::SkippedHasText, lines: Vec::new(), skew_deg: None, rotation: 0, page_size });
    }
    let (bmp, scale) = render(doc, index, opts.dpi, backend.max_dimension())?;
    let mut raw = backend.recognize(&bmp, &opts.languages, cancel)?;
    let mut rotation = 0;
    if opts.detect_orientation {
        rotation = find_rotation(backend, &bmp, &opts.languages, &raw, cancel);
        if rotation != 0 {
            raw = backend.recognize(&bmp.rotated(rotation), &opts.languages, cancel)?;
        }
    }
    let (lines, skew_deg) = to_words(raw, scale);
    let status = if lines.is_empty() { PageStatus::NoText } else { PageStatus::Recognized };
    Ok(Read { status, lines, skew_deg, rotation, page_size })
}

fn same_language(a: &str, b: &str) -> bool {
    let primary = |t: &str| t.split(['-', '_']).next().unwrap_or(t).to_ascii_lowercase();
    primary(a) == primary(b)
}

/// One check before any page is read: a run in a language the backend lacks would otherwise
/// "succeed" with every page failed.
fn check_languages(backend: &dyn Recognizer, requested: &[String]) -> Result<(), OcrError> {
    let have = backend.languages()?;
    if have.is_empty() {
        return Err(OcrError::LanguageUnavailable("no OCR language is installed".into()));
    }
    match requested.first() {
        Some(first) if !requested.iter().any(|r| have.iter().any(|h| same_language(r, h))) => {
            Err(OcrError::LanguageUnavailable(first.clone()))
        }
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Scratch files
// ---------------------------------------------------------------------------------------------

/// A scratch file next to `near`, unique per call (process id, counter, random) and created
/// exclusively, so it can never be an existing file such as the input. Removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn beside(near: &Path) -> Result<Self, OcrError> {
        use std::hash::{BuildHasher, Hasher};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let name = near.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        loop {
            let nonce = std::collections::hash_map::RandomState::new().build_hasher().finish();
            let path = near.with_file_name(format!(
                ".{name}.inkwell-ocr-{}-{}-{nonce:016x}.tmp",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(OcrError::Io(e.to_string())),
            }
        }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Gives `tmp`'s content the name `out` without ever replacing an existing file (a hard link is
/// atomic; file systems without links get an exclusive create and a copy). The caller drops `tmp`.
fn publish(tmp: &Path, out: &Path) -> Result<(), OcrError> {
    use std::io::ErrorKind::AlreadyExists;
    let exists = || OcrError::OutputExists(out.display().to_string());
    match std::fs::hard_link(tmp, out) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == AlreadyExists => Err(exists()),
        Err(_) => {
            let mut dst = match std::fs::OpenOptions::new().write(true).create_new(true).open(out) {
                Ok(f) => f,
                Err(e) if e.kind() == AlreadyExists => return Err(exists()),
                Err(e) => return Err(OcrError::Io(e.to_string())),
            };
            let copied = std::fs::File::open(tmp).and_then(|mut src| std::io::copy(&mut src, &mut dst)).and_then(|_| dst.sync_all());
            if let Err(e) = copied {
                drop(dst);
                let _ = std::fs::remove_file(out);
                return Err(OcrError::Io(e.to_string()));
            }
            Ok(())
        }
    }
}

/// True when `output` names the same file as `input` (case, symlinks, `..` resolved), whether or
/// not `output` exists yet.
fn is_same_file(input: &Path, output: &Path) -> bool {
    let Ok(input) = input.canonicalize() else { return false };
    let resolved = match output.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
            match (parent.canonicalize(), output.file_name()) {
                (Ok(p), Some(n)) => p.join(n),
                _ => return false,
            }
        }
    };
    if cfg!(windows) {
        input.to_string_lossy().eq_ignore_ascii_case(&resolved.to_string_lossy())
    } else {
        input == resolved
    }
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
    check_languages(backend, &opts.languages)?;
    // Workers each open their own document; bytes would be copied once per worker, so a byte
    // input is written to one temp file they all read.
    let spill;
    let spilled;
    let input: &Input = match input {
        Input::Bytes(b) => {
            spill = TempFile::beside(&std::env::temp_dir().join("inkwell-ocr-input.pdf"))?;
            std::fs::write(&spill.0, b).map_err(|e| OcrError::Io(e.to_string()))?;
            spilled = Input::Path(&spill.0);
            &spilled
        }
        other => other,
    };
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
                                read_page(d, backend, opts, page, cancel)
                            }));
                            match run {
                                Ok(r) => r,
                                Err(_) => Err(OcrError::Backend("recognizer crashed".into())),
                            }
                        }
                        Err(e) => Err(OcrError::Pdf(e.to_string())),
                    };
                    // A cancelled page is not a failed page: leave it out.
                    if matches!(r, Err(OcrError::Cancelled)) {
                        return;
                    }
                    let r = r.unwrap_or_else(|e| Read {
                        status: PageStatus::Failed(e.to_string()),
                        lines: Vec::new(),
                        skew_deg: None,
                        rotation: 0,
                        page_size: [0.0, 0.0],
                    });
                    results.lock().unwrap().push(PageResult {
                        page,
                        status: r.status,
                        lines: r.lines,
                        skew_deg: r.skew_deg,
                        rotation: r.rotation,
                        page_size: r.page_size,
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
    let codes = layer::Codes::build(todo.iter().flat_map(|r| r.lines.iter().flatten()).map(|w| w.text.as_str()));
    let font = layer::install_font(doc, &codes)?;
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
        let font_res = layer::font_name_for(&page);
        let stream = layer::layer_stream(r, &user_from_page, &codes, &font_res);
        layer::add_layer(doc, &mut page, &font, &font_res, &stream)?;
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
    /// Things the user should know about the output (a signed file, for one).
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PageSummary {
    pub page: u32,
    pub status: PageStatus,
    pub words: u32,
    pub skew_deg: Option<f32>,
    pub rotation: u16,
    pub ms: u32,
}

/// Scan in, searchable copy out: recognise, add the layer to a copy of `input`, publish it as
/// `output`. `output` must not exist and must not be the input (`Err(OutputExists)` / `Err(Io)`):
/// neither the input nor anything else is ever replaced. On cancel, or when no page could be read,
/// nothing is written and an error is returned.
pub fn ocr_file(
    input: &Path,
    output: &Path,
    backend: &dyn Recognizer,
    opts: &Options,
    progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
) -> Result<Report, OcrError> {
    if is_same_file(input, output) {
        return Err(OcrError::Io("output must be a different file than the input".into()));
    }
    if output.exists() {
        return Err(OcrError::OutputExists(output.display().to_string()));
    }
    let rec = recognize(&Input::Path(input), backend, opts, progress, cancel)?;
    if rec.cancelled {
        return Err(OcrError::Cancelled);
    }
    let failed: Vec<&str> = rec.pages.iter().filter_map(|p| match &p.status { PageStatus::Failed(m) => Some(m.as_str()), _ => None }).collect();
    if !failed.is_empty() && rec.pages.iter().all(|p| !matches!(p.status, PageStatus::Recognized)) {
        return Err(OcrError::Backend(format!("no page could be read: {}", failed[0])));
    }

    // Work on a scratch copy beside the output: an incremental save appends to the file it was
    // opened from, and a failure must never leave a half-written output or touch the input.
    let tmp = TempFile::beside(output)?;
    let mut applied = ApplyReport::default();
    let mut warnings: Vec<String> = Vec::new();
    let mut full: Option<TempFile> = None;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), OcrError> {
        let io = |e: std::io::Error| OcrError::Io(e.to_string());
        let mut dst = std::fs::OpenOptions::new().write(true).truncate(true).open(&tmp.0).map_err(io)?;
        std::io::copy(&mut std::fs::File::open(input).map_err(io)?, &mut dst).map_err(io)?;
        drop(dst);
        // The document must be closed before its file is linked or renamed (Windows).
        let mut doc = Input::Path(&tmp.0).open()?;
        applied = apply(&mut doc, &rec.pages, opts.force)?;
        if applied.pages_written > 0 {
            let signed = doc
                .catalog()
                .ok()
                .and_then(|c| c.get_dict("AcroForm").ok().flatten())
                .and_then(|f| f.get_dict("SigFlags").ok().flatten())
                .and_then(|f| f.as_int().ok())
                .is_some_and(|f| f & 1 != 0);
            if doc.can_be_saved_incrementally() {
                save(&doc, &tmp.0)?;
                if signed {
                    warnings.push("The document is signed; the signature covers the original revision only and viewers may report the copy as modified.".into());
                }
            } else {
                let f = TempFile::beside(output)?;
                save(&doc, &f.0)?;
                full = Some(f);
                if signed {
                    warnings.push("The document is signed; the signatures are invalid in the OCR copy.".into());
                }
            }
        }
        drop(doc);
        publish(full.as_ref().map_or(&tmp.0, |f| &f.0), output)
    }));
    drop(full);
    drop(tmp);
    match r {
        Ok(r) => r?,
        Err(_) => return Err(OcrError::Backend("OCR crashed while writing the copy".into())),
    }

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
                rotation: p.rotation,
                ms: p.ms,
            })
            .collect(),
        warnings,
    })
}
