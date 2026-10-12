//! Ink coverage: per plane and in total, the highest total area coverage (TAC) of a page and where
//! it is, and the regions above the limit.

use super::render::{Planes, paint_planes};
use super::{DEFAULT_TAC_LIMIT, PageSeps, Result, bad, page_separations};
use crate::a11y::Progress;
use crate::engine::{DocId, Engine};
use crate::engine::view_matrix;
use mupdf::IRect;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

/// How one plane is inked on a page.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaneInk {
    pub name: String,
    /// The densest pixel, per cent.
    pub max: f32,
    /// Average over the page, per cent.
    pub mean: f32,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InkReport {
    pub page: u32,
    pub planes: Vec<PlaneInk>,
    /// Highest total area coverage of any pixel, per cent (400 = four full inks).
    pub max_tac: f32,
    /// Where it is, in page points from the top-left of the page as shown.
    pub max_at: [f32; 2],
    pub tac_limit: f32,
    /// `max_tac` is above the limit.
    pub over_limit: bool,
    /// The part of the page above the limit, 0 to 1.
    pub over_limit_area: f32,
    /// Boxes `[x0, y0, x1, y1]` (page points, top-left origin) around the areas above the limit,
    /// at most 64.
    pub regions: Vec<[f32; 4]>,
    /// There were more than 64 regions; the rest are not listed.
    pub regions_truncated: bool,
    pub spots: Vec<String>,
}

/// The ink at one point: per plane and in total, per cent.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InkAt {
    pub page: u32,
    pub planes: Vec<PlaneValue>,
    pub total: f32,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaneValue {
    pub name: String,
    pub value: f32,
}

/// The ink of every plane at `(x, y)` (page points from the top-left of the page as shown), read from one
/// pixel painted with overprint simulated, so it is what prints there and not what the topmost object says.
pub fn ink_at(eng: &Engine, seps: &PageSeps, doc: DocId, page: u32, x: f32, y: f32) -> Result<InkAt> {
    if !(x.is_finite() && y.is_finite()) {
        return Err(bad("The point is not a number"));
    }
    let entry = eng.doc_entry(doc)?;
    let rev = entry.generation();
    let list = eng.page_list(&entry, rev, page)?;
    // two pixels per point: a pixel is a quarter of a point square
    const SCALE: f32 = 2.0;
    let (m, w, h) = view_matrix(list.bounds(), 0, SCALE);
    let px = ((x * SCALE).floor() as i32).clamp(0, w - 1);
    let py = ((y * SCALE).floor() as i32).clamp(0, h - 1);
    let p = paint_planes(&list, seps, true, &m, IRect { x0: px, y0: py, x1: px + 1, y1: py + 1 })?;
    if entry.generation() != rev {
        return Err(crate::engine::EngineError::Stale);
    }
    let name = |i: usize| if i < 4 { super::PROCESS[i].to_string() } else { seps.spots[i - 4].name.clone() };
    let planes: Vec<PlaneValue> = p.data.iter().take(p.n).enumerate().map(|(i, v)| PlaneValue { name: name(i), value: *v as f32 * 100.0 / 255.0 }).collect();
    let total = planes.iter().map(|v| v.value).sum();
    Ok(InkAt { page, planes, total })
}

/// Grid used to report the regions above the limit.
const GRID: usize = 32;
const MAX_REGIONS: usize = 64;

/// Measures page `page`. The page is painted so that its longer side is `max_px` pixels (at most
/// twice natural size), which bounds the work; a hairline thinner than a pixel is averaged with
/// its paper. With `trim_only` the measure stops at the TrimBox (when the page has one), so printer's
/// marks in the slug do not count.
pub fn ink_report(eng: &Engine, doc: DocId, page: u32, limit: f32, max_px: u32, trim_only: bool) -> Result<InkReport> {
    if !(limit.is_finite() && (0.0..=1000.0).contains(&limit)) {
        return Err(bad("The ink limit must be between 0 and 1000 per cent"));
    }
    let seps = page_separations(eng, doc, page)?;
    let entry = eng.doc_entry(doc)?;
    let rev = entry.generation();
    let list = eng.page_list(&entry, rev, page)?;
    let b = list.bounds();
    let long = b.width().max(b.height()).max(1.0);
    let scale = (max_px.max(64) as f32 / long).min(2.0);
    let (m, w, h) = view_matrix(b, 0, scale);
    let planes = paint_planes(&list, &seps, true, &m, IRect { x0: 0, y0: 0, x1: w, y1: h })?;
    let clip = if trim_only { trim_pixels(eng, doc, page, scale, w, h)? } else { None };
    Ok(measure(&planes, &seps, page, limit, scale, clip))
}

/// The page's TrimBox as a pixel rectangle `(x0, y0, x1, y1)` of the painted page.
fn trim_pixels(eng: &Engine, doc: DocId, page: u32, scale: f32, w: i32, h: i32) -> Result<Option<(usize, usize, usize, usize)>> {
    let r = eng.read_custom(doc, |pdf| {
        let p = mupdf::pdf::PdfPage::try_from(pdf.load_page(page as i32)?)?;
        let Some(t) = super::boxes::stated(&pdf.find_page(page as i32)?, super::BoxKind::Trim)? else { return Ok(None) };
        let m = p.ctm()?;
        Ok(Some(mupdf::Rect::new(t[0], t[1], t[2], t[3]).transform(&m)))
    })?;
    Ok(r.map(|r| {
        let c = |v: f32, max: i32| ((v * scale).round().max(0.0) as usize).min(max as usize);
        (c(r.x0, w), c(r.y0, h), c(r.x1, w), c(r.y1, h))
    }))
}

fn measure(p: &Planes, seps: &PageSeps, page: u32, limit: f32, scale: f32, clip: Option<(usize, usize, usize, usize)>) -> InkReport {
    let (cx0, cy0, cx1, cy1) = clip.unwrap_or((0, 0, p.w, p.h));
    let mut max = vec![0u8; p.n];
    let mut sum = vec![0u64; p.n];
    let (mut max_tac, mut at, mut over) = (0u32, (0usize, 0usize), 0usize);
    let mut cells = vec![false; GRID * GRID];
    let limit_raw = (limit * 255.0 / 100.0).floor() as u32;
    for y in cy0..cy1 {
        for x in cx0..cx1 {
            let px = &p.data[(y * p.w + x) * p.n..][..p.n];
            let mut tot = 0u32;
            for (i, v) in px.iter().enumerate() {
                tot += *v as u32;
                sum[i] += *v as u64;
                if *v > max[i] {
                    max[i] = *v;
                }
            }
            if tot > max_tac {
                max_tac = tot;
                at = (x, y);
            }
            if tot > limit_raw {
                over += 1;
                cells[(y * GRID / p.h.max(1)) * GRID + x * GRID / p.w.max(1)] = true;
            }
        }
    }
    let px_count = ((cx1 - cx0) * (cy1 - cy0)).max(1) as f64;
    let name = |i: usize| if i < 4 { super::PROCESS[i].to_string() } else { seps.spots[i - 4].name.clone() };
    let planes = (0..p.n)
        .map(|i| PlaneInk { name: name(i), max: max[i] as f32 * 100.0 / 255.0, mean: (sum[i] as f64 * 100.0 / 255.0 / px_count) as f32 })
        .collect();
    let (cw, ch) = (p.w as f32 / scale / GRID as f32, p.h as f32 / scale / GRID as f32);
    let mut regions: Vec<[f32; 4]> = Vec::new();
    for gy in 0..GRID {
        let mut gx = 0;
        while gx < GRID {
            if !cells[gy * GRID + gx] {
                gx += 1;
                continue;
            }
            let start = gx;
            while gx < GRID && cells[gy * GRID + gx] {
                gx += 1;
            }
            let r = [start as f32 * cw, gy as f32 * ch, gx as f32 * cw, (gy + 1) as f32 * ch];
            // runs directly under a run of the same extent grow it instead of adding a box
            match regions.last_mut() {
                Some(last) if (last[0] - r[0]).abs() < 0.01 && (last[2] - r[2]).abs() < 0.01 && (last[3] - r[1]).abs() < 0.01 => last[3] = r[3],
                _ => regions.push(r),
            }
        }
    }
    let regions_truncated = regions.len() > MAX_REGIONS;
    regions.truncate(MAX_REGIONS);
    let max_tac = max_tac as f32 * 100.0 / 255.0;
    InkReport {
        page,
        planes,
        max_tac,
        max_at: [at.0 as f32 / scale, at.1 as f32 / scale],
        tac_limit: limit,
        over_limit: max_tac > limit,
        over_limit_area: over as f32 / px_count as f32,
        regions,
        regions_truncated,
        spots: seps.spots.iter().map(|s| s.name.clone()).collect(),
    }
}

/// [`ink_report`] for every page (or the ones given), one progress event each; a cancel stops it.
pub fn doc_ink(
    eng: &Engine,
    doc: DocId,
    pages: Option<&[u32]>,
    limit: f32,
    max_px: u32,
    trim_only: bool,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Vec<InkReport>> {
    let count = eng.doc_entry(doc)?.page_count();
    let list: Vec<u32> = match pages {
        Some(p) => p.to_vec(),
        None => (0..count).collect(),
    };
    let mut out = Vec::with_capacity(list.len());
    for (i, &p) in list.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(bad("cancelled"));
        }
        out.push(ink_report(eng, doc, p, limit, max_px, trim_only)?);
        progress(Progress { done: i as u32 + 1, pages: list.len() as u32, stage: "ink" });
    }
    Ok(out)
}

pub const DEFAULT_MAX_PX: u32 = 1600;
pub const DEFAULT_LIMIT: f32 = DEFAULT_TAC_LIMIT;
