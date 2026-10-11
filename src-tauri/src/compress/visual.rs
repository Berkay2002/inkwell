//! Visual check: render the same pages of two files and measure how far apart the pixels are.

use std::sync::atomic::{AtomicBool, Ordering};

use mupdf::{Colorspace, Document, Matrix};
use serde::Serialize;

use super::Error;

/// 96 ppi: what a viewer shows at 100 % zoom on a standard-DPI Windows screen.
const SCALE: f32 = 96.0 / 72.0;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDiff {
    pub page: usize,
    /// Largest per-channel difference, 0..=255.
    pub max: u8,
    /// Mean per-channel difference.
    pub mean: f64,
    pub psnr: f64,
    /// The difference 99.9 % of this page's samples stay at or under (0..=255).
    pub p999: u8,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualCheck {
    pub pages_checked: usize,
    pub pages_total: usize,
    pub max_diff: u8,
    pub mean_diff: f64,
    /// Over all checked pages together. 99.0 = identical.
    pub psnr: f64,
    /// The difference 99.9 % of all checked samples stay at or under: a few stray pixels (the edge of a
    /// resampled glyph) do not move it, a changed line of text does. The result screen's verdict uses it.
    pub p999_diff: u8,
    /// Zero-based; the page with the highest [`badness`], which is what the verdict is made of.
    pub worst_page: usize,
    pub per_page: Vec<PageDiff>,
}

/// The result screen calls a copy "different" at a PSNR under this ...
pub const RED_PSNR: f64 = 30.0;
/// ... or a 99.9th-percentile difference above this (of 255).
pub const RED_P999: f64 = 64.0;

/// How far a page is into "different": 1.0 is the threshold of either test, so the page named worst is
/// the one that trips the verdict hardest, whichever of the two does it.
fn badness(p999: u8, psnr: f64) -> f64 {
    (p999 as f64 / RED_P999).max(RED_PSNR / psnr.max(0.1))
}

/// Smallest difference d such that `fraction` of the samples are at or under d.
fn percentile(hist: &[u64; 256], fraction: f64) -> u8 {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return 0;
    }
    let need = (total as f64 * fraction).ceil() as u64;
    let mut seen = 0u64;
    for (d, &n) in hist.iter().enumerate() {
        seen += n;
        if seen >= need {
            return d as u8;
        }
    }
    255
}

impl VisualCheck {
    /// What the result screen calls "Looks different": the same two tests as `visualVerdict` in
    /// src/lib/optimizer.ts.
    pub fn looks_different(&self) -> bool {
        self.psnr < RED_PSNR || self.p999_diff as f64 > RED_P999
    }
}

fn psnr(sum_sq: f64, samples: f64) -> f64 {
    if sum_sq == 0.0 || samples == 0.0 {
        return 99.0;
    }
    (10.0 * (255.0f64 * 255.0 / (sum_sq / samples)).log10()).min(99.0)
}

/// Up to `limit` page indices spread evenly over `total` pages (first and last included).
pub(crate) fn sample_pages(total: usize, limit: usize) -> Vec<usize> {
    if total <= limit || limit < 2 {
        return (0..total.min(limit.max(1))).collect();
    }
    (0..limit).map(|i| i * (total - 1) / (limit - 1)).collect()
}

/// A page the check has to look at because the run changed it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Must {
    /// Zero-based.
    pub page: usize,
    /// Resolution to render this page at on both sides; 0 for the usual 96 ppi. A flattened page is a
    /// picture drawn at its own resolution: seen at any other, resampling alone would be reported as change.
    pub ppi: f32,
}

/// Most pages added to the sample because the run changed them (flattened, or set in an unembedded font).
const MUST_MAX: usize = 24;

/// The evenly spread sample plus `must` (zero-based; spread down to [`MUST_MAX`] when there are more).
pub(crate) fn picks_for(total: usize, limit: usize, must: &[usize]) -> Vec<usize> {
    let mut picks = sample_pages(total, limit);
    let mut m: Vec<usize> = must.iter().copied().filter(|&p| p < total).collect();
    m.sort_unstable();
    m.dedup();
    if m.len() > MUST_MAX {
        m = (0..MUST_MAX).map(|i| m[i * (m.len() - 1) / (MUST_MAX - 1)]).collect();
    }
    picks.extend(m);
    picks.sort_unstable();
    picks.dedup();
    picks
}

fn render(doc: &Document, page: usize, scale: f32) -> Result<mupdf::Pixmap, Error> {
    let p = doc.load_page(page as i32)?;
    Ok(p.to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), false, false)?)
}

/// Compares `limit` evenly spread pages of two already-open documents, and the pages in `must` besides.
pub(crate) fn compare(
    a: &Document,
    b: &Document,
    limit: usize,
    must: &[Must],
    cancel: &AtomicBool,
    mut tick: impl FnMut(usize, usize),
) -> Result<VisualCheck, Error> {
    let total = a.page_count()? as usize;
    let wanted: Vec<usize> = must.iter().map(|m| m.page).collect();
    let picks = picks_for(total, limit, &wanted);
    // The pages the run changed are compared at their own resolution (a flattened page at the one it was
    // flattened at); a picked page that is in the list twice takes the first resolution that is set.
    let scale_of = |page: usize| {
        must.iter().find(|m| m.page == page && m.ppi > 0.0).map_or(SCALE, |m| m.ppi / 72.0)
    };
    let (mut max_all, mut sum_abs, mut sum_sq, mut count) = (0u8, 0f64, 0f64, 0f64);
    let mut per_page = Vec::with_capacity(picks.len());
    let mut hist_all = [0u64; 256];
    let mut worst = (picks.first().copied().unwrap_or(0), -1.0f64);

    for (k, &page) in picks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        tick(k, picks.len());
        let scale = scale_of(page);
        let (pa, pb) = (render(a, page, scale)?, render(b, page, scale)?);
        let same_shape = pa.width() == pb.width() && pa.height() == pb.height() && pa.n() == pb.n();
        let (mut pmax, mut pabs, mut psq, mut pn) = (0u8, 0f64, 0f64, 0f64);
        let mut hist = [0u64; 256];
        if same_shape {
            let row = pa.width() as usize * pa.n() as usize;
            let (sa, sb) = (pa.samples(), pb.samples());
            let stride = pa.stride() as usize;
            for y in 0..pa.height() as usize {
                let (ra, rb) = (&sa[y * stride..y * stride + row], &sb[y * stride..y * stride + row]);
                for (&x, &y) in ra.iter().zip(rb) {
                    let d = x.abs_diff(y);
                    pmax = pmax.max(d);
                    hist[d as usize] += 1;
                    pabs += d as f64;
                    psq += (d as f64) * (d as f64);
                }
                pn += row as f64;
            }
        } else {
            // A different page size is as wrong as it gets.
            pmax = 255;
            pabs = 255.0;
            psq = 255.0 * 255.0;
            pn = 1.0;
            hist[255] = 1;
        }
        for (all, h) in hist_all.iter_mut().zip(&hist) {
            *all += h;
        }
        let (p999, page_psnr) = (percentile(&hist, 0.999), psnr(psq, pn));
        max_all = max_all.max(pmax);
        sum_abs += pabs;
        sum_sq += psq;
        count += pn;
        let mean = if pn == 0.0 { 0.0 } else { pabs / pn };
        let bad = badness(p999, page_psnr);
        if bad > worst.1 {
            worst = (page, bad);
        }
        per_page.push(PageDiff { page, max: pmax, mean, psnr: page_psnr, p999 });
    }
    Ok(VisualCheck {
        pages_checked: picks.len(),
        pages_total: total,
        max_diff: max_all,
        mean_diff: if count == 0.0 { 0.0 } else { sum_abs / count },
        psnr: psnr(sum_sq, count),
        p999_diff: percentile(&hist_all, 0.999),
        worst_page: worst.0,
        per_page,
    })
}
