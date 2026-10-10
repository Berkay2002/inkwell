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
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("inkwell-estimate-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
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

/// `full` is the audit of the whole file. Unencrypted files only; the caller may fall back to nothing.
pub fn estimate(input: &Path, password: Option<&str>, full: &Audit, cancel: &AtomicBool) -> Result<Vec<Estimate>, Error> {
    let presets = [Preset::Smallest, Preset::Balanced, Preset::HighQuality];
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
    check(cancel)?;

    let scratch = Scratch::new()?;
    let tmp_dir = scratch.0.clone();
    let base = TempFile::new(&tmp_dir.join("sample.pdf"))?;
    let mut shrink = vec![(1.0, 1.0, 1.0); presets.len()];
    let mut sample_img = 0u64;

    if !keep.is_empty() && full_img > 0 {
        let mut s = doc;
        let others: Vec<usize> = (0..pages as usize).filter(|p| !keep.contains(p)).collect();
        if !others.is_empty() {
            s.delete_pages(others)?;
        }
        // Written as the original was (no optimisation), so the presets' structural savings show too.
        let mut plain = PdfWriteOptions::default();
        plain.set_garbage_level(1);
        s.save_with_options(&base.0.to_string_lossy(), plain)?;
        drop(s);
        check(cancel)?;
        let before = measure(&base.0)?;
        sample_img = before.bytes(Category::Images);
        let b_font = before.bytes(Category::Fonts);
        let b_img = sample_img.max(1) as f64;
        let b_rest = before.total.saturating_sub(sample_img + b_font).max(1) as f64;

        // 2. Each preset on the sample.
        for (k, &preset) in presets.iter().enumerate() {
            check(cancel)?;
            let opts = Options::preset(preset);
            let mut d = open_pdf(&base.0, None)?;
            if let Some(io) = image_options(&opts.images) {
                let mut cb = |_: u32, _: usize, _: usize| cancel.load(Ordering::Relaxed);
                if let Err(e) = d.rewrite_images(&io, &mut cb) {
                    return Err(if cancel.load(Ordering::Relaxed) { Error::Cancelled } else { e.into() });
                }
            }
            if opts.subset_fonts && before.fonts > 0 {
                d.subset_fonts()?;
            }
            let out = TempFile::new(&tmp_dir.join("out.pdf"))?;
            d.save_with_options(&out.0.to_string_lossy(), write_options(&opts))?;
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
    let rest = full.total.saturating_sub(full_img + full_font) as f64;
    // One or two sampled pages say little about how the others differ: the range opens up.
    let spread = if keep.len() >= 4 { 0.4 } else { 1.0 };
    Ok(presets
        .iter()
        .zip(&shrink)
        .map(|(&preset, &(r_img, r_font, r_rest))| {
            let img_out = full_img as f64 * r_img;
            let mid = img_out + full_font as f64 * r_font + rest * r_rest;
            let u = spread * (1.0 - coverage) * img_out;
            let clamp = |v: f64| v.clamp(1.0, full.total as f64) as u64;
            Estimate { preset, bytes: clamp(mid), low: clamp(mid - 0.6 * u), high: clamp(mid + u) }
        })
        .collect())
}
