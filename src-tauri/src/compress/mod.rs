//! Compress / optimize: input path + options -> a new PDF + a report.
//!
//! Standalone on purpose. It reads the file on disk with its own MuPDF documents, never goes through the
//! engine's replicas, and never writes to the input: the caller saves pending edits first and opens the
//! output afterwards. Everything runs on the calling thread (MuPDF contexts are per thread), so use
//! [`spawn`] from the UI.
//!
//! Pipeline: open -> audit -> strip extras -> downsample/recompress images -> subset fonts -> write
//! (garbage collect, dedupe, object streams, Flate) -> keep the original when nothing got smaller ->
//! reopen, audit and render-compare the result.
//!
//! Gaps against Acrobat, by design of the engine underneath: MuPDF cannot linearize ("Fast Web View"),
//! has no JBIG2 encoder (monochrome images use CCITT G4 or Flate) and does not unembed fonts.

mod audit;
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
    /// Pack objects and the xref into compressed streams (PDF 1.5).
    pub object_streams: bool,
    /// 0 none, 1 drop unused, 2 + renumber, 3 + merge identical objects, 4 + merge identical streams.
    pub garbage: u8,
    pub strip: Strip,
    /// Pages rendered before/after for the visual check (spread evenly; 0 = skip the check).
    pub visual_pages: u32,
    pub password: Option<String>,
    /// Where to write; default `<name> (compressed).pdf` next to the input. Never overwritten.
    pub output: Option<PathBuf>,
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
            object_streams: true,
            garbage: 4,
            strip,
            visual_pages: 8,
            password: None,
            output: None,
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

/// Gives `tmp`'s content the name `out` without ever replacing a file that is already there. A hard link
/// does it atomically; file systems without links (FAT32, exFAT, some network shares) get an exclusive
/// create plus a copy. The caller removes `tmp` afterwards.
fn publish(tmp: &Path, out: &Path) -> Result<(), Error> {
    use std::io::ErrorKind::AlreadyExists;
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
        return Err(if same { Error::SameFile } else { Error::OutputExists(out) });
    }
    Ok(out)
}

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

fn write_options(opts: &Options) -> PdfWriteOptions {
    let mut w = PdfWriteOptions::default();
    w.set_garbage_level(opts.garbage as i32)
        .set_compress(true)
        .set_compress_images(true)
        .set_compress_fonts(true)
        .set_use_objstms(opts.object_streams);
    w
}

// ---------------------------------------------------------------- the job

/// Audits a file without changing anything ("Audit space usage").
pub fn audit_file(path: &Path, password: Option<&str>) -> Result<Audit, Error> {
    let doc = open_pdf(path, password)?;
    audit::audit(&doc, fs::metadata(path)?.len())
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
    let pages = doc.page_count()? as usize;
    run.lap("open");

    // 1. Where the bytes are now.
    let before = audit::audit(&doc, size_before)?;
    run.lap("audit");
    run.check()?;
    run.emit(Stage::Scan, 0.04, 0.0, 0, 0);

    let signed = doc.catalog()?.get_dict("AcroForm")?.and_then(|f| f.get_dict("SigFlags").ok().flatten())
        .and_then(|f| f.as_int().ok())
        .is_some_and(|f| f & 1 != 0);

    // 2. Extras the user asked to drop.
    let stripped = strip::strip(&mut doc, &opts.strip, pages)?;
    run.lap("strip");
    run.check()?;

    // 3. Images. A document without any image is left to the writer alone.
    let mut image_stats = ImageStats::default();
    if before.images > 0 && let Some(io) = image_options(&opts.images) {
        let (cancel, progress) = (run.cancel, &mut *run.progress);
        let mut cb = |pass: u32, page: usize, total: usize| {
            let (base, span) = if pass == 0 { (0.06, 0.12) } else { (0.18, 0.5) };
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
    run.check()?;

    // 4. Fonts.
    run.emit(Stage::Fonts, 0.68, 0.0, 0, 0);
    if opts.subset_fonts && before.fonts > 0 {
        doc.subset_fonts()?;
        run.lap("fonts");
    }
    run.check()?;

    // 5. Write. JPEG 2000 needs PDF 1.5 (the writer raises the version itself for object streams).
    run.emit(Stage::Write, 0.72, 0.0, 0, 0);
    let jpx = [opts.images.color, opts.images.gray].iter().any(|c| c.codec == Codec::Jpx);
    if jpx && image_stats.replaced > 0 {
        let mut catalog = doc.catalog()?;
        let version = catalog.get_dict("Version")?.and_then(|v| v.as_name().ok());
        if version.is_none_or(|v| v.as_slice() < b"1.5".as_slice()) {
            catalog.dict_put("Version", mupdf::pdf::PdfObject::new_name("1.5")?)?;
        }
    }
    let tmp = TempFile::new(&out_path)?;
    doc.save_with_options(&tmp.0.to_string_lossy(), write_options(opts))?;
    drop(doc);
    run.lap("write");
    run.check()?;

    // 6. Never worse, except that dropping what the user asked to drop is never undone.
    let written = fs::metadata(&tmp.0)?.len();
    let kept_original = written >= size_before && !stripped.any();
    if written >= size_before && stripped.any() {
        warnings.push("The copy is larger than the original because extras were removed.".into());
    }
    if kept_original {
        fs::copy(input, &tmp.0)?;
    } else if signed {
        warnings.push("The document is signed; the signatures are invalid in the compressed copy.".into());
    }
    let size_after = if kept_original { size_before } else { written };
    publish(&tmp.0, &out_path)?;
    drop(tmp);
    let finish = |e: Error, p: &Path| {
        let _ = fs::remove_file(p);
        e
    };

    // 7. Reopen what was written: it must open, have the same pages, and look the same.
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
            Some(visual::compare(&orig, &out_doc, opts.visual_pages as usize, cancel, |k, n| {
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
