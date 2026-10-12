//! Predicted output size per preset, from a sample instead of fixed ratios.
//!
//! Pages spread over the file's image sizes (the largest, then evenly down the ranking) are copied into a small sample file (about 2.5 Mpx of
//! images at most). Each preset's real image and font settings run on that sample, and the audited
//! image and font bytes before and after give the shrink factors, which are applied to the whole
//! file's image and font bytes. Images outside the sample (the smaller ones) are the uncertainty:
//! the answer is a range that widens with their share, and collapses when the sample covers the file.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use serde::Serialize;

use super::audit::{audit, Audit, Category};
use super::{image_options, open_pdf, write_options, Error, Options, Preset, TempFile};

/// Pixels of images in the sample, at most (recompression time follows pixels, not file bytes: about
/// 0.4 s per megapixel for the three presets together); and the most pages it may span.
const SAMPLE_PIXELS: u64 = 2_500_000;
const SAMPLE_PAGES: usize = 8;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub preset: Preset,
    /// Most likely size of the compressed file, bytes.
    pub bytes: u64,
    /// The range it is expected to fall in (`low == high` when the sample covered every image).
    pub low: u64,
    pub high: u64,
}

/// A private scratch folder in the system temp dir, removed on drop (cancel and errors included).
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Result<Self, Error> {
        use std::hash::{BuildHasher, Hasher};
        // Unpredictable name, and `create_dir` (not `create_dir_all`) fails on anything already there,
        // so a pre-planted folder or link in the shared temp dir is never used.
        loop {
            let nonce = std::collections::hash_map::RandomState::new().build_hasher().finish();
            let dir = std::env::temp_dir().join(format!("inkwell-estimate-{}-{nonce:016x}", std::process::id()));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) { Err(Error::Cancelled) } else { Ok(()) }
}

/// Pixels of the images a page draws directly (inherited resources included).
fn page_image_pixels(doc: &PdfDocument, i: i32) -> u64 {
    let Ok(page) = doc.find_page(i) else { return 0 };
    let Ok(Some(res)) = page.get_dict_inheritable("Resources") else { return 0 };
    let Ok(Some(xo)) = res.get_dict("XObject") else { return 0 };
    let Ok(it) = xo.dict_iter() else { return 0 };
    let mut sum = 0;
    for (_, v) in it.flatten() {
        if !v.is_stream().unwrap_or(false) {
            continue;
        }
        let image = v.get_dict("Subtype").ok().flatten().and_then(|s| s.as_name().ok()).is_some_and(|n| n == b"Image");
        if image {
            let dim = |k: &str| v.get_dict(k).ok().flatten().and_then(|l| l.as_int().ok()).unwrap_or(0).max(0) as u64;
            sum += dim("Width") * dim("Height");
        }
    }
    sum
}

fn measure(path: &Path) -> Result<Audit, Error> {
    let doc = open_pdf(path, None)?;
    audit(&doc, std::fs::metadata(path)?.len())
}

/// The three presets. `full` is the audit of the whole file. Unencrypted files only; the caller may
/// fall back to nothing.
pub fn estimate(input: &Path, password: Option<&str>, full: &Audit, cancel: &AtomicBool) -> Result<Vec<Estimate>, Error> {
    let variants: Vec<Options> = [Preset::Smallest, Preset::Balanced, Preset::HighQuality].map(Options::preset).into();
    estimate_many(input, password, full, &variants, cancel)
}

/// One set of Advanced settings.
pub fn estimate_custom(input: &Path, password: Option<&str>, full: &Audit, opts: &Options, cancel: &AtomicBool) -> Result<Estimate, Error> {
    let mut v = estimate_many(input, password, full, std::slice::from_ref(opts), cancel)?;
    let mut e = v.pop().ok_or_else(|| Error::Verify("no estimate".into()))?;
    e.preset = Preset::Custom;
    Ok(e)
}

/// What the full file's audit already says about a discard option: the bytes of the category that
/// holds what it removes. Exact where the audit has a category for it; options without one (actions,
/// alternate images, private data) are small and counted as nothing.
fn discarded(full: &Audit, opts: &Options) -> u64 {
    let s = &opts.strip;
    let mut b = 0;
    if s.thumbnails {
        b += full.bytes(Category::Thumbnails);
    }
    if s.embedded_files {
        b += full.bytes(Category::EmbeddedFiles);
    }
    if s.bookmarks {
        b += full.bytes(Category::Bookmarks);
    }
    if s.metadata {
        b += full.bytes(Category::Metadata);
    }
    if s.links {
        b += full.bytes(Category::Links);
    }
    b
}

/// Bytes the unembed list takes out of the file's fonts (stored size of the files, exact).
fn unembed_bytes(full: &Audit, opts: &Options) -> u64 {
    full.font_list
        .iter()
        .filter(|f| f.can_unembed && (opts.unembed_all || opts.unembed_fonts.contains(&f.name)))
        .map(|f| f.bytes)
        .sum()
}

fn estimate_many(
    input: &Path,
    password: Option<&str>,
    full: &Audit,
    variants: &[Options],
    cancel: &AtomicBool,
) -> Result<Vec<Estimate>, Error> {
    let full_img = full.bytes(Category::Images);
    let full_font = full.bytes(Category::Fonts);

    // 1. The sample file: the image-heaviest pages, largest first.
    let doc = open_pdf(input, password)?;
    let pages = doc.page_count()?;
    let mut ranked: Vec<(u64, i32)> = Vec::new();
    for i in 0..pages {
        if i % 16 == 0 {
            check(cancel)?;
        }
        let b = page_image_pixels(&doc, i);
        if b > 0 {
            ranked.push((b, i));
        }
    }
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    // The largest page first, then evenly spaced ranks: big and small images both show up.
    let want = ranked.len().min(SAMPLE_PAGES);
    let (mut keep, mut got) = (Vec::new(), 0u64);
    for k in 0..want {
        let (b, i) = ranked[k * ranked.len() / want];
        // The two heaviest pages always go in unless one alone is over the budget: a single page says
        // nothing about how the others compress.
        let must = keep.len() < 2 && b <= SAMPLE_PIXELS;
        if !keep.is_empty() && !must && got + b > SAMPLE_PIXELS {
            continue;
        }
        keep.push(i as usize);
        got += b;
    }
    // No images at all: pages spread over the file stand in, so fonts and structure still get measured.
    if keep.is_empty() && pages > 0 {
        keep = super::visual::sample_pages(pages as usize, SAMPLE_PAGES);
    }
    // Pages that use transparency: a flatten run is not simulated (it is a rendering), its cost is a range.
    let flatten_pages = if variants.iter().any(|o| o.transparency.flatten) {
        super::flatten::pages_using_transparency(&doc)?.len() as u64
    } else {
        0
    };
    let (page_w, page_h) = doc.find_page(0).ok().and_then(|p| page_size(&p)).unwrap_or((612.0, 792.0));
    let page_px = |ppi: u32| (page_w * ppi as f64 / 72.0) * (page_h * ppi as f64 / 72.0);
    check(cancel)?;

    let scratch = Scratch::new()?;
    let tmp_dir = scratch.0.clone();
    let base = TempFile::new(&tmp_dir.join("sample.pdf"))?;
    let mut shrink = vec![(1.0, 1.0, 1.0); variants.len()];
    let mut sample_img = 0u64;

    if !keep.is_empty() && (full_img > 0 || full_font > 0) {
        let mut s = doc;
        let others: Vec<usize> = (0..pages as usize).filter(|p| !keep.contains(p)).collect();
        if !others.is_empty() {
            s.delete_pages(others)?;
        }
        // Without an /ID no save has one to renew: MuPDF writes a random second half each time, up to 16 bytes
        // shorter now and then, and that noise would land in the ratios (pdf-write.c `change_identity`).
        // shortcut: an encrypted file keeps its /ID (the key of the older forms comes from it) and the rare
        // jitter; upgrade if a case shows up.
        let mut trailer = s.trailer()?;
        if trailer.get_dict("Encrypt")?.is_none() {
            trailer.dict_delete("ID")?;
        }
        // Written as the original was (no optimisation), so the settings' structural savings show too.
        let mut plain = PdfWriteOptions::default();
        plain.set_garbage_level(1);
        s.save_with_options(&base.0.to_string_lossy(), plain)?;
        drop(s);
        check(cancel)?;
        let before = measure(&base.0)?;
        sample_img = if full_img > 0 { before.bytes(Category::Images) } else { 0 };
        let b_font = before.bytes(Category::Fonts);
        let b_img = sample_img.max(1) as f64;
        let b_rest = before.total.saturating_sub(sample_img + b_font).max(1) as f64;

        // 2. Each variant's image, font and write settings on the sample.
        for (k, opts) in variants.iter().enumerate() {
            check(cancel)?;
            let mut d = open_pdf(&base.0, None)?;
            if let Some(io) = image_options(&opts.images) {
                let mut cb = |_: u32, _: usize, _: usize| cancel.load(Ordering::Relaxed);
                if let Err(e) = super::rewrite_images(&mut d, &io, &mut cb) {
                    return Err(if cancel.load(Ordering::Relaxed) { Error::Cancelled } else { e.into() });
                }
            }
            if opts.subset_fonts && before.fonts > 0 {
                d.subset_fonts()?;
            }
            let out = TempFile::new(&tmp_dir.join("out.pdf"))?;
            d.save_with_options(&out.0.to_string_lossy(), write_options(opts, opts.garbage))?;
            drop(d);
            let after = measure(&out.0)?;
            let r_img = (after.bytes(Category::Images) as f64 / b_img).min(1.0);
            let r_font = if b_font > 0 { (after.bytes(Category::Fonts) as f64 / b_font as f64).min(1.0) } else { 1.0 };
            let a_rest = after.total.saturating_sub(after.bytes(Category::Images) + after.bytes(Category::Fonts));
            shrink[k] = (r_img, r_font, (a_rest as f64 / b_rest).min(1.0));
        }
    }
    drop(base);

    // 3. Extrapolate. What the sample did not see (the images it skipped) is the uncertainty: the
    // range is a share of the image output that grows with the unseen part, and is exact at full coverage.
    let coverage = if full_img == 0 { 1.0 } else { (sample_img as f64 / full_img as f64).min(1.0) };
    // One or two sampled pages say little about how the others differ: the range opens up.
    let spread = if keep.len() >= 4 { 0.4 } else { 1.0 };
    Ok(variants
        .iter()
        .zip(&shrink)
        .map(|(opts, &(r_img, r_font, r_rest))| {
            let gone = discarded(full, opts).min(full.total) as f64;
            let unembedded = unembed_bytes(full, opts).min(full_font) as f64;
            let rest = full.total.saturating_sub(full_img + full_font) as f64 - gone.min(full.total.saturating_sub(full_img + full_font) as f64);
            let img_out = full_img as f64 * r_img;
            let font_out = (full_font as f64 - unembedded) * r_font;
            let mut mid = img_out + font_out + rest.max(0.0) * r_rest;
            let mut u = spread * (1.0 - coverage) * img_out;
            // Each flattened page costs a picture: JPEG 2000 / JPEG of a text page runs 0.15 to 0.5 byte
            // per pixel at the Balanced settings.
            if opts.transparency.flatten && flatten_pages > 0 {
                let px = page_px(opts.transparency.ppi) * flatten_pages as f64;
                mid += px * 0.15;
                u += px * 0.35;
            }
            let clamp = |v: f64| v.clamp(1.0, full.total as f64) as u64;
            Estimate { preset: opts.preset, bytes: clamp(mid), low: clamp(mid - 0.6 * u), high: clamp(mid + u) }
        })
        .collect())
}

/// Width and height of a page's crop box (or media box) in points.
fn page_size(page: &mupdf::pdf::PdfObject) -> Option<(f64, f64)> {
    let b = page.get_dict_inheritable("CropBox").ok().flatten().or_else(|| page.get_dict_inheritable("MediaBox").ok().flatten())?;
    let n = |i: i32| b.get_array(i).ok().flatten().and_then(|v| v.as_float().ok()).map(f64::from);
    Some(((n(2)? - n(0)?).abs(), (n(3)? - n(1)?).abs()))
}
