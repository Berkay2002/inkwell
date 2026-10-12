//! Compress / optimize: input path + options -> a new PDF + a report.
//!
//! Standalone on purpose. It reads the file on disk with its own MuPDF documents, never goes through the
//! engine's replicas, and never writes to the input: the caller saves pending edits first and opens the
//! output afterwards. Everything runs on the calling thread (MuPDF contexts are per thread), so use
//! [`spawn`] from the UI.
//!
//! Pipeline: open -> audit -> discard objects -> unembed fonts -> flatten transparency ->
//! downsample/recompress images -> subset fonts -> write (garbage collect, dedupe, object streams,
//! Flate) -> keep the original when nothing got smaller -> reopen, audit and render-compare the result.
//!
//! `Options` is both the three presets and the "Advanced" settings of the sheet (Acrobat's Optimized
//! PDF: images per class, fonts, transparency, discard objects, clean up); named presets are saved
//! options (`presets`).
//!
//! Gaps against Acrobat, by design of the engine underneath: MuPDF cannot linearize ("Fast Web View")
//! and has no JBIG2 encoder (monochrome images use CCITT G4 or Flate). Transparency is flattened by
//! drawing the page (text and vectors on it become part of the picture).

mod audit;
pub mod estimate;
mod flatten;
mod fonts;
pub mod presets;
mod strip;
#[cfg(test)]
mod tests;
mod visual;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use mupdf::pdf::document::{ImageClassOptions, ImageRewriteOptions, ImageRewriteStats, Recompress};
use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use mupdf::Document;
use serde::{Deserialize, Serialize};

pub use audit::{Audit, AuditItem, Category, CategoryUsage};
pub use flatten::{FlattenReport, Transparency};
pub use fonts::{FontUsage, Unembedded};
pub use strip::{Strip, StripReport};
pub use visual::{PageDiff, VisualCheck};

// ---------------------------------------------------------------- options

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preset {
    Smallest,
    #[default]
    Balanced,
    HighQuality,
    Custom,
}

/// How an image class is encoded after the optional downsampling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Codec {
    /// Keep the encoding. Images that get downsampled are re-encoded in their own family.
    Retain,
    Jpeg,
    /// JPEG 2000 (PDF 1.5+). Acrobat's one-click "Reduce File Size" uses it.
    Jpx,
    /// Deflate. Never applied to a lossy source (a JPEG stays a JPEG).
    Flate,
    /// CCITT G4, monochrome images only (MuPDF has no JBIG2 encoder).
    Fax,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageClass {
    /// Downsample images shown at more than this many ppi (0 = never downsample).
    pub above_ppi: u32,
    /// ... down to this many ppi.
    pub to_ppi: u32,
    pub codec: Codec,
    /// 1..=100 for Jpeg / Jpx.
    pub quality: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Images {
    pub color: ImageClass,
    pub gray: ImageClass,
    pub mono: ImageClass,
    /// Keep the original when re-encoding does not make an image smaller (Acrobat's guard).
    pub only_if_smaller: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    pub preset: Preset,
    pub images: Images,
    /// Subset embedded fonts to the glyphs used. Off unless asked for.
    pub subset_fonts: bool,
    /// Fonts (by `/BaseFont`, as listed in the audit) whose embedded file is taken out. Only simple,
    /// non-symbolic fonts qualify; the others are reported as skipped.
    pub unembed_fonts: Vec<String>,
    /// Unembed every font that may be (what a saved preset keeps instead of font names).
    pub unembed_all: bool,
    /// Draw pages that use transparency as one picture.
    pub transparency: Transparency,
    /// Deflate streams that are stored raw (content, uncompressed images, fonts).
    pub compress_streams: bool,
    /// Pack objects and the xref into compressed streams (PDF 1.5).
    pub object_streams: bool,
    /// 0 none, 1 drop unused, 2 + renumber, 3 + merge identical objects, 4 + merge identical streams.
    pub garbage: u8,
    pub strip: Strip,
    /// Pages rendered before/after for the visual check (spread evenly; 0 = skip the check).
    pub visual_pages: u32,
    pub password: Option<String>,
    /// Where to write; default `<name> (compressed).pdf` next to the input. Never replaces an existing
    /// file unless `replace_output` says so. Not part of the wire format: a webview cannot name a file,
    /// the command fills it from a dialog on the Rust side.
    #[serde(skip)]
    pub output: Option<PathBuf>,
    /// `output` may replace the file already there (the user confirmed that in the save dialog).
    /// The input itself is never replaced.
    #[serde(skip)]
    pub replace_output: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self::preset(Preset::Balanced)
    }
}

impl Options {
    pub fn preset(p: Preset) -> Self {
        let class = |above_ppi, to_ppi, codec, quality| ImageClass { above_ppi, to_ppi, codec, quality };
        let (color, mono, subset_fonts, strip) = match p {
            Preset::Smallest => (
                class(108, 72, Codec::Jpeg, 18),
                class(300, 200, Codec::Fax, 0),
                true,
                Strip { thumbnails: true, ..Strip::default() },
            ),
            Preset::Balanced | Preset::Custom => {
                (class(225, 150, Codec::Jpx, 75), class(450, 300, Codec::Fax, 0), false, Strip::default())
            }
            Preset::HighQuality => {
                (class(300, 225, Codec::Jpx, 85), class(600, 450, Codec::Fax, 0), false, Strip::default())
            }
        };
        Self {
            preset: p,
            images: Images { color, gray: color, mono, only_if_smaller: true },
            subset_fonts,
            unembed_fonts: Vec::new(),
            unembed_all: false,
            transparency: Transparency::default(),
            compress_streams: true,
            object_streams: true,
            garbage: 4,
            strip,
            visual_pages: 8,
            password: None,
            output: None,
            replace_output: false,
        }
    }
}

// ---------------------------------------------------------------- errors, progress, report

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cancelled")]
    Cancelled,
    #[error("password required")]
    NeedsPassword,
    #[error("wrong password")]
    BadPassword,
    #[error("{0} already exists")]
    OutputExists(PathBuf),
    #[error("the output would overwrite the input")]
    SameFile,
    #[error("invalid options: {0}")]
    InvalidOptions(String),
    #[error("verification failed: {0}")]
    Verify(String),
    /// The document is certified: a compressed copy of it would not be.
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Mupdf(#[from] mupdf::Error),
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // The UI matches on these codes, like the engine's.
        s.serialize_str(&match self {
            Error::Cancelled => "cancelled".to_string(),
            Error::NeedsPassword => "needs-password".to_string(),
            Error::BadPassword => "bad-password".to_string(),
            Error::OutputExists(_) => "output-exists".to_string(),
            Error::InvalidOptions(_) => "invalid-options".to_string(),
            other => other.to_string(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Open,
    Scan,
    Flatten,
    Images,
    Fonts,
    Write,
    Verify,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: Stage,
    /// 0..=1 over the whole job.
    pub fraction: f32,
    pub page: u32,
    pub pages: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    pub stage: &'static str,
    pub ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageStats {
    pub seen: u32,
    pub resampled: u32,
    pub replaced: u32,
    pub kept: u32,
    pub failed: u32,
}

impl From<ImageRewriteStats> for ImageStats {
    fn from(s: ImageRewriteStats) -> Self {
        Self { seen: s.seen, resampled: s.resampled, replaced: s.replaced, kept: s.kept, failed: s.failed }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub input: PathBuf,
    /// The file that now exists (a copy of the input when `kept_original`).
    pub output: PathBuf,
    pub size_before: u64,
    pub size_after: u64,
    pub saved_bytes: u64,
    pub saved_percent: f64,
    /// Nothing got smaller, so `output` holds the original bytes.
    pub kept_original: bool,
    pub pages: u32,
    pub before: Audit,
    pub after: Audit,
    pub images: ImageStats,
    pub stripped: StripReport,
    pub unembedded: Unembedded,
    pub flattened: FlattenReport,
    pub visual: Option<VisualCheck>,
    pub timings: Vec<Timing>,
    pub elapsed_ms: u64,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------- helpers

struct Run<'a> {
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(Progress),
    t0: Instant,
    last: Instant,
    timings: Vec<Timing>,
}

impl Run<'_> {
    fn check(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Relaxed) { Err(Error::Cancelled) } else { Ok(()) }
    }

    fn emit(&mut self, stage: Stage, base: f32, span: f32, page: usize, pages: usize) {
        let within = if pages == 0 { 0.0 } else { page as f32 / pages as f32 };
        (self.progress)(Progress { stage, fraction: base + span * within, page: page as u32, pages: pages as u32 });
    }

    fn lap(&mut self, stage: &'static str) {
        let now = Instant::now();
        self.timings.push(Timing { stage, ms: (now - self.last).as_millis() as u64 });
        self.last = now;
    }
}

/// A scratch file next to the output, unique per job (process id, counter, random), created exclusively
/// so two jobs on the same input never share one. Removed when dropped.
struct TempFile(PathBuf);

impl TempFile {
    fn new(out_path: &Path) -> Result<Self, Error> {
        use std::hash::{BuildHasher, Hasher};
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = out_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        loop {
            let nonce = std::collections::hash_map::RandomState::new().build_hasher().finish();
            let path = out_path.with_file_name(format!(
                ".{name}.inkwell-{}-{}-{nonce:016x}.tmp",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Gives `tmp`'s content the name `out`. Without `replace` it never replaces a file that is already
/// there: a hard link does it atomically; file systems without links (FAT32, exFAT, some network
/// shares) get an exclusive create plus a copy. With `replace` (a dialog-confirmed name) `tmp` is
/// renamed over `out`. The caller removes `tmp` afterwards.
fn publish(tmp: &Path, out: &Path, replace: bool) -> Result<(), Error> {
    use std::io::ErrorKind::AlreadyExists;
    if replace {
        return Ok(fs::rename(tmp, out)?);
    }
    match fs::hard_link(tmp, out) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == AlreadyExists => return Err(Error::OutputExists(out.to_path_buf())),
        Err(_) => copy_exclusive(tmp, out),
    }
}

/// `out` is created exclusively and filled from `tmp`; a failed copy leaves nothing behind.
fn copy_exclusive(tmp: &Path, out: &Path) -> Result<(), Error> {
    use std::io::ErrorKind::AlreadyExists;
    let mut dst = match fs::OpenOptions::new().write(true).create_new(true).open(out) {
        Ok(f) => f,
        Err(e) if e.kind() == AlreadyExists => return Err(Error::OutputExists(out.to_path_buf())),
        Err(e) => return Err(e.into()),
    };
    let copied = fs::File::open(tmp).and_then(|mut src| std::io::copy(&mut src, &mut dst)).and_then(|_| dst.sync_all());
    if let Err(e) = copied {
        drop(dst);
        let _ = fs::remove_file(out);
        return Err(e.into());
    }
    Ok(())
}

fn open_pdf(path: &Path, password: Option<&str>) -> Result<PdfDocument, Error> {
    let mut doc = Document::open(path.to_string_lossy().as_ref())?;
    if doc.needs_password()? {
        match password {
            None => return Err(Error::NeedsPassword),
            Some(p) if !doc.authenticate(p)? => return Err(Error::BadPassword),
            Some(_) => {}
        }
    }
    Ok(PdfDocument::try_from(doc)?)
}

/// `<dir>/<stem> (compressed).pdf` unless the caller gave a path.
pub fn default_output(input: &Path) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "document".into());
    input.with_file_name(format!("{stem} (compressed).pdf"))
}

fn resolve_output(input: &Path, opts: &Options) -> Result<PathBuf, Error> {
    let out = opts.output.clone().unwrap_or_else(|| default_output(input));
    if out.exists() {
        let same = matches!((fs::canonicalize(input), fs::canonicalize(&out)), (Ok(a), Ok(b)) if a == b);
        if same {
            return Err(Error::SameFile);
        }
        if !opts.replace_output {
            return Err(Error::OutputExists(out));
        }
    }
    Ok(out)
}

/// Pages the visual check may look at (0 skips it).
pub const VISUAL_PAGES_MAX: u32 = 64;

/// Resolution bounds for a downsampling class: 36 ppi is already unreadable, 2400 is above any scanner.
const PPI: std::ops::RangeInclusive<u32> = 36..=2400;

fn validate(opts: &Options) -> Result<(), Error> {
    let bad = |m: String| Err(Error::InvalidOptions(m));
    for (name, c) in [("colour", &opts.images.color), ("gray", &opts.images.gray), ("monochrome", &opts.images.mono)] {
        // above_ppi 0 means "never downsample"; the target is then unused.
        if c.above_ppi != 0 {
            if !PPI.contains(&c.above_ppi) || !PPI.contains(&c.to_ppi) {
                return bad(format!("{name}: ppi must be between {} and {}", PPI.start(), PPI.end()));
            }
            if c.to_ppi > c.above_ppi {
                return bad(format!("{name}: target ppi {} is above the threshold {}", c.to_ppi, c.above_ppi));
            }
        }
        if matches!(c.codec, Codec::Jpeg | Codec::Jpx) && !(1..=100).contains(&c.quality) {
            return bad(format!("{name}: quality must be 1 to 100"));
        }
    }
    if opts.garbage > 4 {
        return bad("garbage level must be 0 to 4".into());
    }
    if opts.transparency.flatten && !flatten::PPI.contains(&opts.transparency.ppi) {
        return bad(format!("flatten: ppi must be between {} and {}", flatten::PPI.start(), flatten::PPI.end()));
    }
    if opts.visual_pages > VISUAL_PAGES_MAX {
        return bad(format!("the visual check covers 0 to {VISUAL_PAGES_MAX} pages"));
    }
    if opts.unembed_fonts.len() > 1000 || opts.unembed_fonts.iter().any(|n| n.is_empty() || n.len() > 256) {
        return bad("font names to unembed are not valid".into());
    }
    Ok(())
}

fn image_options(img: &Images) -> Option<ImageRewriteOptions> {
    let class = |c: &ImageClass, lossy: bool| {
        let method = match (c.codec, lossy) {
            (Codec::Jpeg, _) => Recompress::Jpeg,
            (Codec::Jpx, _) => Recompress::Jpx,
            (Codec::Flate, false) => Recompress::Flate,
            (Codec::Fax, _) if lossy => Recompress::Never,
            (Codec::Fax, false) => Recompress::Fax,
            // Retain, and Flate over a JPEG: re-encode only what gets resampled, in its own family.
            _ if c.above_ppi > 0 => Recompress::SameIfScaled,
            _ => Recompress::Never,
        };
        ImageClassOptions {
            threshold_ppi: c.above_ppi,
            target_ppi: c.to_ppi.min(c.above_ppi),
            bicubic: true,
            method,
            quality: if c.codec == Codec::Flate { 100 } else { c.quality as u32 },
        }
    };
    // A bitonal image is Fax, Flate or retained; "lossy" does not apply.
    let mono = |c: &ImageClass| {
        let mut o = class(c, false);
        if matches!(c.codec, Codec::Jpeg | Codec::Jpx) {
            o.method = Recompress::SameIfScaled;
        }
        o
    };
    let o = ImageRewriteOptions {
        color_lossy: class(&img.color, true),
        color_lossless: class(&img.color, false),
        gray_lossy: class(&img.gray, true),
        gray_lossless: class(&img.gray, false),
        bitonal: mono(&img.mono),
        always: !img.only_if_smaller,
    };
    let any = [o.color_lossy, o.color_lossless, o.gray_lossy, o.gray_lossless, o.bitonal]
        .iter()
        .any(|c| c.method != Recompress::Never);
    any.then_some(o)
}

/// `garbage` is the level to use (a run that took objects out never writes with 0: the orphans would stay).
fn write_options(opts: &Options, garbage: u8) -> PdfWriteOptions {
    let mut w = PdfWriteOptions::default();
    w.set_garbage_level(garbage as i32)
        .set_compress(opts.compress_streams)
        .set_compress_images(opts.compress_streams)
        .set_compress_fonts(opts.compress_streams)
        .set_use_objstms(opts.object_streams);
    w
}

// ---------------------------------------------------------------- the job

/// Audits a file without changing anything ("Audit space usage").
pub fn audit_file(path: &Path, password: Option<&str>) -> Result<Audit, Error> {
    let doc = open_pdf(path, password)?;
    audit::audit(&doc, fs::metadata(path)?.len())
}

/// The document is signed or certified (signature fields with a value count too, not only `/SigFlags`): a rewritten
/// copy invalidates them.
fn is_signed(doc: &mupdf::pdf::PdfDocument) -> bool {
    crate::outline::signed(doc)
}

/// [`audit_file`], and whether the document is signed (the sheet warns before Start).
pub fn audit_file_signed(path: &Path, password: Option<&str>) -> Result<(Audit, bool), Error> {
    let doc = open_pdf(path, password)?;
    let signed = is_signed(&doc);
    Ok((audit::audit(&doc, fs::metadata(path)?.len())?, signed))
}

/// Runs the whole job on the calling thread. `cancel` is polled between pages and stages; `progress`
/// is called often enough for a progress bar (per page while images are rewritten).
pub fn compress(
    input: &Path,
    opts: &Options,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report, Error> {
    let now = Instant::now();
    let mut run = Run { cancel, progress, t0: now, last: now, timings: Vec::new() };
    let mut warnings = Vec::new();
    run.check()?;
    validate(opts)?;
    run.emit(Stage::Open, 0.0, 0.0, 0, 0);

    let out_path = resolve_output(input, opts)?;
    let size_before = fs::metadata(input)?.len();
    let password = opts.password.as_deref();
    let mut doc = open_pdf(input, password)?;
    // The copy is a rewrite of every object: a certification does not survive it (a signature only warns, below).
    crate::outline::refuse_if_certified(&doc, "Compressing it").map_err(|e| Error::Refused(e.to_string()))?;
    let pages = doc.page_count()? as usize;
    run.lap("open");

    // 1. Where the bytes are now.
    let before = audit::audit(&doc, size_before)?;
    run.lap("audit");
    run.check()?;
    run.emit(Stage::Scan, 0.04, 0.0, 0, 0);

    let signed = is_signed(&doc);

    // 2. Extras the user asked to drop.
    let stripped = strip::strip(&mut doc, &opts.strip, pages)?;
    run.lap("strip");
    if stripped.links_unchecked {
        warnings.push("The named destinations could not be read, so links were not checked.".into());
    }
    if let Some(why) = &stripped.bookmarks_unchecked {
        warnings.push(format!("Invalid bookmarks were not removed: {why}."));
    }
    run.check()?;

    // 3. Fonts that are to be unembedded, before any pass that would copy them.
    let unembedded = if opts.unembed_fonts.is_empty() && !opts.unembed_all {
        Unembedded::default()
    } else {
        let u = fonts::unembed(&mut doc, &opts.unembed_fonts, opts.unembed_all)?;
        for (name, why) in &u.skipped {
            warnings.push(format!("{name} stays embedded: {why}."));
        }
        run.lap("unembed");
        u
    };
    // The pages those fonts are used on, found before anything rewrites the page resources.
    let restyled = fonts::pages_using(&doc, &unembedded.font_objects)?;
    run.check()?;

    // 4. Pages that use transparency become pictures, which the image pass below then treats like any other.
    let mut flattened = FlattenReport::default();
    if opts.transparency.flatten {
        let pages_hit = flatten::pages_using_transparency(&doc)?;
        if !pages_hit.is_empty() {
            let (cancel, progress) = (run.cancel, &mut *run.progress);
            let total = pages_hit.len();
            flattened = flatten::flatten(&mut doc, &pages_hit, opts.transparency.ppi, cancel, &mut |k, n| {
                progress(Progress {
                    stage: Stage::Flatten,
                    fraction: 0.04 + 0.12 * k as f32 / n.max(1) as f32,
                    page: k as u32,
                    pages: total as u32,
                });
            })?;
            for (page, why) in &flattened.skipped {
                warnings.push(format!("Page {page} uses transparency and was not flattened: {why}."));
            }
        }
        run.lap("flatten");
    }
    run.check()?;
    let flat = !flattened.pages.is_empty();

    // 5. Images. A document without any image (or a flattened page) is left to the writer alone.
    let (alternates, private) = strip::rewrite_losses(&doc)?;
    let mut image_stats = ImageStats::default();
    if (before.images > 0 || flat) && let Some(io) = image_options(&opts.images) {
        let (cancel, progress) = (run.cancel, &mut *run.progress);
        let mut cb = |pass: u32, page: usize, total: usize| {
            let (base, span) = match (pass, flat) {
                (0, false) => (0.06, 0.12),
                (0, true) => (0.16, 0.02),
                _ => (0.18, 0.5),
            };
            progress(Progress {
                stage: if pass == 0 { Stage::Scan } else { Stage::Images },
                fraction: base + span * page as f32 / total.max(1) as f32,
                page: page as u32,
                pages: total as u32,
            });
            cancel.load(Ordering::Relaxed)
        };
        match doc.rewrite_images(&io, &mut cb) {
            Ok(s) => image_stats = s.into(),
            Err(_) if cancel.load(Ordering::Relaxed) => return Err(Error::Cancelled),
            Err(e) => return Err(e.into()),
        }
        run.lap("images");
    }
    // The pass writes new objects for what it rewrites: alternates and a form's private data stay behind.
    if image_stats.replaced > 0 && alternates + private > 0 {
        let mut what = Vec::new();
        if alternates > 0 {
            what.push(format!("{alternates} image{} with alternates", if alternates == 1 { "" } else { "s" }));
        }
        if private > 0 {
            what.push(format!("{private} form{} with private data", if private == 1 { "" } else { "s" }));
        }
        warnings.push(format!("Rewritten images lost their alternates and private data ({}).", what.join(", ")));
    }
    run.check()?;

    // 6. Fonts.
    run.emit(Stage::Fonts, 0.68, 0.0, 0, 0);
    if opts.subset_fonts && before.fonts > unembedded.files {
        doc.subset_fonts()?;
        run.lap("fonts");
    }
    run.check()?;

    // 7. Write. JPEG 2000 needs PDF 1.5 (the writer raises the version itself for object streams).
    run.emit(Stage::Write, 0.72, 0.0, 0, 0);
    let jpx = [opts.images.color, opts.images.gray].iter().any(|c| c.codec == Codec::Jpx);
    if jpx && image_stats.replaced > 0 {
        let mut catalog = doc.catalog()?;
        let version = catalog.get_dict("Version")?.and_then(|v| v.as_name().ok());
        if version.is_none_or(|v| v.as_slice() < b"1.5".as_slice()) {
            catalog.dict_put("Version", mupdf::pdf::PdfObject::new_name("1.5")?)?;
        }
    }
    // Dropped extras, flattened pages and unembedded fonts change what the file shows: an embedded search index that
    // no longer matches would keep the old text, so it goes (one that still matches is kept).
    crate::search::catalog::embed::drop_if_stale(&doc)?;
    let tmp = TempFile::new(&out_path)?;
    // What the user asked to take out or flatten is never undone by the size rule below.
    let changed = stripped.any() || unembedded.files > 0 || flat;
    let garbage = if changed { opts.garbage.max(1) } else { opts.garbage };
    doc.save_with_options(&tmp.0.to_string_lossy(), write_options(opts, garbage))?;
    drop(doc);
    run.lap("write");
    run.check()?;

    // 8. Never worse, except that dropping what the user asked to drop is never undone.
    let written = fs::metadata(&tmp.0)?.len();
    let kept_original = written >= size_before && !changed;
    if written >= size_before && changed {
        warnings.push("The copy is larger than the original because objects were removed or flattened.".into());
    }
    if kept_original {
        fs::copy(input, &tmp.0)?;
    } else if signed {
        warnings.push("The document is signed; the signatures are invalid in the compressed copy.".into());
    }
    let size_after = if kept_original { size_before } else { written };
    publish(&tmp.0, &out_path, opts.replace_output)?;
    drop(tmp);
    let finish = |e: Error, p: &Path| {
        let _ = fs::remove_file(p);
        e
    };

    // 9. Reopen what was written: it must open, have the same pages, and look the same.
    run.emit(Stage::Verify, 0.9, 0.0, 0, 0);
    let mut verify = || -> Result<(Audit, Option<VisualCheck>), Error> {
        let out_doc = open_pdf(&out_path, password)?;
        let out_pages = out_doc.page_count()? as usize;
        if out_pages != pages {
            return Err(Error::Verify(format!("{out_pages} pages, expected {pages}")));
        }
        let after = audit::audit(&out_doc, size_after)?;
        let visual = if kept_original || opts.visual_pages == 0 {
            None
        } else {
            let orig = open_pdf(input, password)?;
            let (cancel, progress) = (run.cancel, &mut *run.progress);
            // Always look at the pages this run turned into pictures or gave another font.
            // A flattened page is a picture at the resolution it was flattened at: both sides are drawn at it.
            let flat_ppi = opts.transparency.ppi as f32;
            let mut must: Vec<visual::Must> = restyled.iter().map(|&page| visual::Must { page, ppi: 0.0 }).collect();
            must.extend(flattened.pages.iter().map(|&p| visual::Must { page: p as usize - 1, ppi: flat_ppi }));
            Some(visual::compare(&orig, &out_doc, opts.visual_pages as usize, &must, cancel, |k, n| {
                progress(Progress { stage: Stage::Verify, fraction: 0.9 + 0.1 * k as f32 / n as f32, page: k as u32, pages: n as u32 });
            })?)
        };
        Ok((after, visual))
    };
    let (after, visual) = match verify() {
        Ok(v) => v,
        Err(e) => return Err(finish(e, &out_path)),
    };
    run.lap("verify");
    run.emit(Stage::Verify, 1.0, 0.0, 0, 0);

    let saved_bytes = size_before.saturating_sub(size_after);
    Ok(Report {
        input: input.to_path_buf(),
        output: out_path,
        size_before,
        size_after,
        saved_bytes,
        saved_percent: if size_before == 0 { 0.0 } else { saved_bytes as f64 * 100.0 / size_before as f64 },
        kept_original,
        pages: pages as u32,
        before,
        after,
        images: image_stats,
        stripped,
        unembedded,
        flattened,
        visual,
        elapsed_ms: run.t0.elapsed().as_millis() as u64,
        timings: run.timings,
        warnings,
    })
}

/// A running job: poll nothing, just `cancel()` or `join()`.
pub struct Job {
    cancel: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<Result<Report, Error>>,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub fn join(self) -> Result<Report, Error> {
        self.handle.join().unwrap_or_else(|_| Err(Error::Verify("the compressor crashed".into())))
    }
}

/// Starts [`compress`] on its own thread. `progress` is called from that thread.
pub fn spawn(
    input: PathBuf,
    opts: Options,
    mut progress: impl FnMut(Progress) + Send + 'static,
) -> Job {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let handle = std::thread::Builder::new()
        .name("compress".into())
        .spawn(move || compress(&input, &opts, &flag, &mut progress))
        .expect("spawn compress thread");
    Job { cancel, handle }
}
