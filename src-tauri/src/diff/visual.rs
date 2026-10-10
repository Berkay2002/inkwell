//! Visual page diff: cached page renders and a pixel diff with changed regions as boxes.
//!
//! `render_page` opens its own MuPDF document from the bytes (contexts are per thread, so call it from
//! `spawn_blocking`); renders are cached in a thread-safe, size-bounded LRU keyed by
//! (BLAKE3 of the bytes, page, scale). `diff_rasters` is pure pixel work on two cached renders.

use super::types::{DiffError, Rect4, Result, Side};
use lru::LruCache;
use mupdf::pdf::PdfDocument;
use mupdf::{Colorspace, Matrix};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::{Arc, OnceLock};

/// Budget of the render cache in bytes of RGBA.
const CACHE_BYTES: usize = 256 << 20;
/// Per-channel difference above which a pixel counts as changed (anti-aliasing tolerant).
const THRESHOLD: i32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BytesId(pub [u8; 32]);

impl BytesId {
    pub fn of(bytes: &[u8]) -> Self {
        BytesId(*blake3::hash(bytes).as_bytes())
    }
}

/// A rendered page: tightly packed RGBA, opaque white paper.
#[derive(Debug)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let cs = Colorspace::device_rgb();
        let mut pix = mupdf::Pixmap::new_with_w_h(&cs, self.width as i32, self.height as i32, true)?;
        let stride = pix.stride() as usize;
        let row = self.width as usize * 4;
        let dst = pix.samples_mut();
        for y in 0..self.height as usize {
            dst[y * stride..y * stride + row].copy_from_slice(&self.rgba[y * row..(y + 1) * row]);
        }
        let mut out = Vec::new();
        pix.write_to(&mut out, mupdf::ImageFormat::PNG)?;
        Ok(out)
    }
}

type Key = (BytesId, u32, u32);

struct Cache {
    lru: LruCache<Key, Arc<Raster>>,
    bytes: usize,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Cache { lru: LruCache::unbounded(), bytes: 0 }))
}

pub fn clear_cache() {
    let mut c = cache().lock();
    c.lru.clear();
    c.bytes = 0;
}

pub fn render_page(id: BytesId, bytes: &[u8], page: u32, scale: f32) -> Result<Arc<Raster>> {
    render_page_pw(id, bytes, page, scale, None)
}

pub fn render_page_pw(id: BytesId, bytes: &[u8], page: u32, scale: f32, password: Option<&str>) -> Result<Arc<Raster>> {
    let key = (id, page, (scale * 1000.0).round() as u32);
    if let Some(r) = cache().lock().lru.get(&key) {
        return Ok(r.clone());
    }
    let r = Arc::new(render(bytes, page, scale, password)?);
    let mut c = cache().lock();
    if let Some(old) = c.lru.put(key, r.clone()) {
        c.bytes -= old.rgba.len();
    }
    c.bytes += r.rgba.len();
    while c.bytes > CACHE_BYTES && c.lru.len() > 1 {
        if let Some((_, old)) = c.lru.pop_lru() {
            c.bytes -= old.rgba.len();
        }
    }
    Ok(r)
}

fn render(bytes: &[u8], page: u32, scale: f32, password: Option<&str>) -> Result<Raster> {
    let doc: PdfDocument = super::snap::open_doc(bytes, Side::A, password)?;
    let n = doc.page_count()?;
    if page as i32 >= n {
        return Err(DiffError::BadPage { page });
    }
    let p = doc.load_page(page as i32)?;
    let pix = p.to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), false, true)?;
    let (w, h, n) = (pix.width() as usize, pix.height() as usize, pix.n() as usize);
    let stride = pix.stride() as usize;
    let src = pix.samples();
    let mut rgba = vec![255u8; w * h * 4];
    for y in 0..h {
        let row = &src[y * stride..y * stride + w * n];
        let out = &mut rgba[y * w * 4..(y + 1) * w * 4];
        if n >= 3 {
            for (o, s) in out.chunks_exact_mut(4).zip(row.chunks_exact(n)) {
                o[..3].copy_from_slice(&s[..3]);
            }
        } else {
            for (o, s) in out.chunks_exact_mut(4).zip(row.chunks_exact(n)) {
                o[..3].fill(s[0]);
            }
        }
    }
    Ok(Raster { width: w as u32, height: h as u32, rgba })
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VisualDiff {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    #[serde(skip)]
    pub before: Arc<Raster>,
    #[serde(skip)]
    pub after: Arc<Raster>,
    /// Changed regions in page space (points), merged and padded: draw as rounded boxes.
    pub boxes: Vec<Rect4>,
    /// The same regions in pixels `[x0, y0, x1, y1)`.
    pub boxes_px: Vec<[u32; 4]>,
    /// width x height, 255 where a pixel differs (dilated), else 0.
    #[serde(skip)]
    pub mask: Vec<u8>,
    pub changed_pixels: u32,
}

pub fn visual_diff(a: &[u8], a_page: u32, b: &[u8], b_page: u32, scale: f32) -> Result<VisualDiff> {
    let ra = render_page(BytesId::of(a), a, a_page, scale)?;
    let rb = render_page(BytesId::of(b), b, b_page, scale)?;
    Ok(diff_rasters(ra, rb, scale))
}

pub fn diff_rasters(before: Arc<Raster>, after: Arc<Raster>, scale: f32) -> VisualDiff {
    let (w, h) = (before.width.max(after.width) as usize, before.height.max(after.height) as usize);
    let mut mask = vec![0u8; w * h];
    let mut changed = 0u32;
    let differs = |p: &[u8], q: &[u8]| (0..3).any(|c| (p[c] as i32 - q[c] as i32).abs() > THRESHOLD);
    const WHITE: [u8; 4] = [255; 4];
    if before.width == after.width && before.height == after.height {
        for (m, (p, q)) in mask.iter_mut().zip(before.rgba.chunks_exact(4).zip(after.rgba.chunks_exact(4))) {
            if p != q && differs(p, q) {
                *m = 1;
                changed += 1;
            }
        }
    } else {
        // pages of different size: compare on a white canvas of the larger size
        let px = |r: &Raster, x: usize, y: usize| -> [u8; 4] {
            if x < r.width as usize && y < r.height as usize {
                let i = (y * r.width as usize + x) * 4;
                [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2], 255]
            } else {
                WHITE
            }
        };
        for y in 0..h {
            for x in 0..w {
                if differs(&px(&before, x, y), &px(&after, x, y)) {
                    mask[y * w + x] = 1;
                    changed += 1;
                }
            }
        }
    }

    let r = ((2.0 * scale).round() as usize).max(1);
    let mask = dilate(&mask, w, h, r);
    let mut boxes_px = components(&mask, w, h);
    // join regions closer than a few pixels, then pad
    let gap = (6.0 * scale).round() as i64;
    merge_boxes(&mut boxes_px, gap);
    let pad = (2.0 * scale).round() as i64;
    let boxes_px: Vec<[u32; 4]> = boxes_px
        .into_iter()
        .map(|b| {
            [
                (b[0] as i64 - pad).max(0) as u32,
                (b[1] as i64 - pad).max(0) as u32,
                (b[2] as i64 + pad).min(w as i64) as u32,
                (b[3] as i64 + pad).min(h as i64) as u32,
            ]
        })
        .collect();
    let boxes = boxes_px.iter().map(|b| [b[0] as f32 / scale, b[1] as f32 / scale, b[2] as f32 / scale, b[3] as f32 / scale]).collect();
    let mask = mask.into_iter().map(|v| if v != 0 { 255 } else { 0 }).collect();
    VisualDiff { width: w as u32, height: h as u32, scale, before, after, boxes, boxes_px, mask, changed_pixels: changed }
}

/// Square dilation by `r` pixels (separable: row sums, then a sliding column window).
fn dilate(m: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut hz = vec![0u8; w * h];
    let mut ps = vec![0u32; w + 1];
    for y in 0..h {
        let row = &m[y * w..(y + 1) * w];
        for x in 0..w {
            ps[x + 1] = ps[x] + row[x] as u32;
        }
        let out = &mut hz[y * w..(y + 1) * w];
        for x in 0..w {
            out[x] = (ps[(x + r + 1).min(w)] - ps[x.saturating_sub(r)] > 0) as u8;
        }
    }
    let mut out = vec![0u8; w * h];
    if h == 0 {
        return out;
    }
    let mut cnt = vec![0u32; w];
    let add = |cnt: &mut Vec<u32>, y: usize, sign: i32| {
        for x in 0..w {
            let v = hz[y * w + x] as i32 * sign;
            cnt[x] = (cnt[x] as i32 + v) as u32;
        }
    };
    for y in 0..=r.min(h - 1) {
        add(&mut cnt, y, 1);
    }
    for y in 0..h {
        if y > 0 {
            if y + r < h {
                add(&mut cnt, y + r, 1);
            }
            if y > r {
                add(&mut cnt, y - r - 1, -1);
            }
        }
        for x in 0..w {
            out[y * w + x] = (cnt[x] > 0) as u8;
        }
    }
    out
}

/// Bounding boxes `[x0, y0, x1, y1)` of 4-connected regions.
fn components(m: &[u8], w: usize, h: usize) -> Vec<[u32; 4]> {
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    for start in 0..w * h {
        if m[start] == 0 || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        seen[start] = true;
        stack.push(start as u32);
        while let Some(i) = stack.pop() {
            let i = i as usize;
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
            let mut push = |j: usize| {
                if m[j] != 0 && !seen[j] {
                    seen[j] = true;
                    stack.push(j as u32);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
        out.push([x0 as u32, y0 as u32, x1 as u32, y1 as u32]);
    }
    out
}

/// Repeatedly unions boxes that are within `gap` pixels of each other.
fn merge_boxes(boxes: &mut Vec<[u32; 4]>, gap: i64) {
    // ponytail: O(n^2) per pass; a page with thousands of separate regions (noise, photos) is slow-ish, bucket by grid if it matters
    loop {
        let mut merged = false;
        let mut i = 0;
        while i < boxes.len() {
            let mut j = i + 1;
            while j < boxes.len() {
                let (a, b) = (boxes[i], boxes[j]);
                let near = (a[0] as i64) <= b[2] as i64 + gap
                    && (b[0] as i64) <= a[2] as i64 + gap
                    && (a[1] as i64) <= b[3] as i64 + gap
                    && (b[1] as i64) <= a[3] as i64 + gap;
                if near {
                    boxes[i] = [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
                    boxes.swap_remove(j);
                    merged = true;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
        if !merged {
            break;
        }
    }
    boxes.sort_by_key(|b| (b[1], b[0]));
}
