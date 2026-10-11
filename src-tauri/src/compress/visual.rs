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
    pub worst_page: usize,
    pub per_page: Vec<PageDiff>,
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

fn render(doc: &Document, page: usize) -> Result<mupdf::Pixmap, Error> {
    let p = doc.load_page(page as i32)?;
    Ok(p.to_pixmap(&Matrix::new_scale(SCALE, SCALE), &Colorspace::device_rgb(), false, false)?)
}

/// Compares `limit` evenly spread pages of two already-open documents, and the pages in `must` besides.
pub(crate) fn compare(
    a: &Document,
    b: &Document,
    limit: usize,
    must: &[usize],
    cancel: &AtomicBool,
    mut tick: impl FnMut(usize, usize),
) -> Result<VisualCheck, Error> {
    let total = a.page_count()? as usize;
    let picks = picks_for(total, limit, must);
    let (mut max_all, mut sum_abs, mut sum_sq, mut count) = (0u8, 0f64, 0f64, 0f64);
    let mut per_page = Vec::with_capacity(picks.len());
    let mut worst = (0usize, 0.0f64);

    for (k, &page) in picks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        tick(k, picks.len());
        let (pa, pb) = (render(a, page)?, render(b, page)?);
        let same_shape = pa.width() == pb.width() && pa.height() == pb.height() && pa.n() == pb.n();
        let (mut pmax, mut pabs, mut psq, mut pn) = (0u8, 0f64, 0f64, 0f64);
        if same_shape {
            let row = pa.width() as usize * pa.n() as usize;
            let (sa, sb) = (pa.samples(), pb.samples());
            let stride = pa.stride() as usize;
            for y in 0..pa.height() as usize {
                let (ra, rb) = (&sa[y * stride..y * stride + row], &sb[y * stride..y * stride + row]);
                for (&x, &y) in ra.iter().zip(rb) {
                    let d = x.abs_diff(y);
                    pmax = pmax.max(d);
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
        }
        max_all = max_all.max(pmax);
        sum_abs += pabs;
        sum_sq += psq;
        count += pn;
        let mean = if pn == 0.0 { 0.0 } else { pabs / pn };
        if mean >= worst.1 {
            worst = (page, mean);
        }
        per_page.push(PageDiff { page, max: pmax, mean, psnr: psnr(psq, pn) });
    }
    Ok(VisualCheck {
        pages_checked: picks.len(),
        pages_total: total,
        max_diff: max_all,
        mean_diff: if count == 0.0 { 0.0 } else { sum_abs / count },
        psnr: psnr(sum_sq, count),
        worst_page: worst.0,
        per_page,
    })
}
