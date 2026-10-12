//! Visual page diff: cached page renders and a pixel diff with changed regions as boxes.
//!
//! `render_page` opens its own MuPDF document from the bytes (contexts are per thread, so call it from
//! `spawn_blocking`); renders are cached in a thread-safe, size-bounded LRU keyed by
//! (BLAKE3 of the bytes, page, scale). `diff_rasters` is pure pixel work on two cached renders.

use super::types::{DiffError, Rect4, Result, Side};
use lru::LruCache;
use mupdf::pdf::PdfDocument;
use mupdf::{Colorspace, Cookie, Device, Matrix, Pixmap};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Duration;

/// Budget of the render cache in bytes of RGBA.
const CACHE_BYTES: usize = 256 << 20;
/// Per-channel difference above which a pixel counts as changed (anti-aliasing tolerant).
const THRESHOLD: i32 = 32;
/// A pixel this different (3x3 means) counts towards a region being real; a region needs `MIN_STRONG` of them.
const STRONG: i32 = 48;
/// Minimum difference of the 3x3 means for a pixel to count at all.
const BLUR_MIN: i32 = 24;
const MIN_STRONG: u32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BytesId(pub [u8; 32]);

impl BytesId {
    pub fn of(bytes: &[u8]) -> Self {
        BytesId(*blake3::hash(bytes).as_bytes())
    }
}

/// How long a fitted render (a thumbnail) may take before the page is given up on.
const FIT_LIMIT: Duration = Duration::from_secs(6);

/// Largest render, in pixels (160 MB of RGBA).
pub const MAX_PIXELS: f32 = 40e6;

/// A rendered page: tightly packed RGBA, opaque white paper.
#[derive(Debug)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// The scale actually used: lower than asked when the page would exceed `MAX_PIXELS`.
    pub scale: f32,
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
    cached((id, page, (scale * 1000.0).round() as u32), bytes, page, Size::Scale(scale), password)
}

/// A page drawn so its longest side is `side` pixels (whatever the page's own size): what a thumbnail wants.
pub fn render_page_fit(id: BytesId, bytes: &[u8], page: u32, side: u32) -> Result<Arc<Raster>> {
    // The high bit tells a fitted render from a scaled one in the cache key.
    cached((id, page, (1 << 31) | side), bytes, page, Size::Side(side as f32), None)
}

/// How big a render is asked for: a scale (pixels per point) or a longest side in pixels.
#[derive(Clone, Copy)]
enum Size {
    Scale(f32),
    Side(f32),
}

fn cached(key: Key, bytes: &[u8], page: u32, size: Size, password: Option<&str>) -> Result<Arc<Raster>> {
    if let Some(r) = cache().lock().lru.get(&key) {
        return Ok(r.clone());
    }
    // Panics inside MuPDF become an error, as in `compare`.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(bytes, page, size, password)))
        .unwrap_or(Err(DiffError::Panic))?;
    let r = Arc::new(r);
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

fn render(bytes: &[u8], page: u32, size: Size, password: Option<&str>) -> Result<Raster> {
    let doc: PdfDocument = super::snap::open_doc(bytes, Side::A, password)?;
    let n = doc.page_count()?;
    if page as i32 >= n {
        return Err(DiffError::BadPage { page });
    }
    let p = doc.load_page(page as i32)?;
    let b = p.bounds()?;
    let area = ((b.x1 - b.x0) * (b.y1 - b.y0)).max(1.0);
    let scale = match size {
        Size::Scale(s) => s,
        Size::Side(side) => side / (b.x1 - b.x0).max(b.y1 - b.y0).max(1.0),
    };
    // 0.99: the pixmap rounds its edges outward.
    let scale = scale.min((MAX_PIXELS / area).sqrt() * 0.99);
    let m = Matrix::new_scale(scale, scale);
    let pix = match size {
        Size::Scale(_) => p.to_pixmap(&m, &Colorspace::device_rgb(), false, true)?,
        Size::Side(_) => draw_within(&p, &m, FIT_LIMIT)?,
    };
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
    Ok(Raster { width: w as u32, height: h as u32, scale, rgba })
}

/// What `Page::to_pixmap` makes (white paper, opaque, annotations and fields drawn), with a cookie so a watchdog
/// can abort a page that takes longer than `limit`: a hostile page cannot hold the caller for good.
fn draw_within(page: &mupdf::Page, m: &Matrix, limit: Duration) -> Result<Pixmap> {
    let area = page.bounds()?.transform(m).round();
    let mut pix = Pixmap::new_with_rect(&Colorspace::device_rgb(), area, false)?;
    pix.clear_with(255)?;
    let cookie = Cookie::new()?;
    let timed_out = AtomicBool::new(false);
    let (done, finished) = mpsc::channel::<()>();
    let (timed, handle) = (&timed_out, cookie.abort_handle());
    let run = std::thread::scope(|s| {
        s.spawn(move || {
            if finished.recv_timeout(limit) == Err(mpsc::RecvTimeoutError::Timeout) {
                timed.store(true, Ordering::SeqCst);
                handle.abort();
            }
        });
        let run = Device::from_pixmap(&pix).and_then(|dev| page.run_with_cookie(&dev, m, &cookie));
        drop(done);
        run
    });
    if timed_out.load(Ordering::SeqCst) {
        return Err(DiffError::Slow);
    }
    run?;
    Ok(pix)
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
    visual_diff_hinted(a, a_page, b, b_page, scale, &[])
}

/// `hints`: rects (page points) of semantic changes; regions touching one are kept however faint.
pub fn visual_diff_hinted(a: &[u8], a_page: u32, b: &[u8], b_page: u32, scale: f32, hints: &[Rect4]) -> Result<VisualDiff> {
    let (ia, ib) = (BytesId::of(a), BytesId::of(b));
    let (mut ra, mut rb) = (render_page(ia, a, a_page, scale)?, render_page(ib, b, b_page, scale)?);
    if ra.scale != rb.scale {
        // One page hit the pixel cap: compare both at the smaller scale.
        let s = ra.scale.min(rb.scale);
        (ra, rb) = (render_page(ia, a, a_page, s)?, render_page(ib, b, b_page, s)?);
    }
    Ok(diff_rasters_hinted(ra, rb, hints))
}

/// Pixel diff of two renders made at the same scale (no semantic hints: see [`diff_rasters_hinted`]).
pub fn diff_rasters(before: Arc<Raster>, after: Arc<Raster>, _requested_scale: f32) -> VisualDiff {
    diff_rasters_hinted(before, after, &[])
}

/// A pixel differs only if it is not explained by a sub-pixel shift: its 3x3 neighbourhood must differ too. Regions then need `MIN_STRONG` strongly changed pixels to count, unless they
/// touch a `hints` rect (a semantic change, page points). Boxes use the rasters' effective scale.
pub fn diff_rasters_hinted(before: Arc<Raster>, after: Arc<Raster>, hints: &[Rect4]) -> VisualDiff {
    let scale = before.scale.min(after.scale);
    let (w, h) = (before.width.max(after.width) as usize, before.height.max(after.height) as usize);
    // both on a white canvas of the larger size
    let canvas = |r: &Raster| -> Vec<u8> {
        if r.width as usize == w && r.height as usize == h {
            return r.rgba.clone();
        }
        let mut v = vec![255u8; w * h * 4];
        for y in 0..r.height as usize {
            let n = r.width as usize * 4;
            v[y * w * 4..y * w * 4 + n].copy_from_slice(&r.rgba[y * n..(y + 1) * n]);
        }
        v
    };
    let (ca, cb) = (canvas(&before), canvas(&after));
    let delta = |p: &[u8], q: &[u8]| (0..3).map(|c| (p[c] as i32 - q[c] as i32).abs()).max().unwrap_or(0);
    // 3x3 mean at (x, y): a sub-pixel shift of an edge barely moves it, a real change does
    let blur = |img: &[u8], x: usize, y: usize| {
        let mut sum = [0i32; 3];
        let (mut n, ys, xs) = (0, y.saturating_sub(1)..=(y + 1).min(h - 1), x.saturating_sub(1)..=(x + 1).min(w - 1));
        for yy in ys {
            for xx in xs.clone() {
                for (c, s) in sum.iter_mut().enumerate() {
                    *s += img[(yy * w + xx) * 4 + c] as i32;
                }
                n += 1;
            }
        }
        sum.map(|s| s / n)
    };
    let mut mask = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            if delta(&ca[i..i + 4], &cb[i..i + 4]) <= THRESHOLD {
                continue;
            }
            let (p, q) = (blur(&ca, x, y), blur(&cb, x, y));
            let d = (0..3).map(|c| (p[c] - q[c]).abs()).max().unwrap_or(0);
            if d > BLUR_MIN {
                mask[y * w + x] = if d >= STRONG { 2 } else { 1 };
            }
        }
    }
    // isolated single pixels are rendering noise
    let near = |x: usize, y: usize| {
        (y.saturating_sub(1)..=(y + 1).min(h - 1)).any(|yy| (x.saturating_sub(1)..=(x + 1).min(w - 1)).any(|xx| (xx, yy) != (x, y) && mask[yy * w + xx] != 0))
    };
    let lonely: Vec<usize> = (0..w * h).filter(|&i| mask[i] != 0 && !near(i % w, i / w)).collect();
    for i in lonely {
        mask[i] = 0;
    }
    let strong: Vec<u8> = mask.iter().map(|&m| (m == 2) as u8).collect();
    let mask: Vec<u8> = mask.iter().map(|&m| (m != 0) as u8).collect();

    let r = ((2.0 * scale).round() as usize).max(1);
    let grown = dilate(&mask, w, h, r);
    let mut boxes_px = components(&grown, w, h);
    // join regions closer than a few pixels, then pad
    let gap = (6.0 * scale).round() as i64;
    merge_boxes(&mut boxes_px, gap);
    let pad = (2.0 * scale).round() as i64;
    let hint_px: Vec<[f32; 4]> = hints.iter().map(|r| [r[0] * scale, r[1] * scale, r[2] * scale, r[3] * scale]).collect();
    let mut changed = 0u32;
    let mut kept_mask = vec![0u8; w * h];
    let mut boxes_out: Vec<[u32; 4]> = Vec::new();
    for b in boxes_px {
        let (mut n_strong, mut n_all) = (0u32, 0u32);
        for y in b[1] as usize..b[3] as usize {
            for x in b[0] as usize..b[2] as usize {
                n_strong += strong[y * w + x] as u32;
                n_all += mask[y * w + x] as u32;
            }
        }
        let hinted = hint_px.iter().any(|r| r[0] < b[2] as f32 && r[2] > b[0] as f32 && r[1] < b[3] as f32 && r[3] > b[1] as f32);
        if n_strong < MIN_STRONG && !hinted {
            continue;
        }
        changed += n_all;
        for y in b[1] as usize..b[3] as usize {
            for x in b[0] as usize..b[2] as usize {
                kept_mask[y * w + x] = grown[y * w + x];
            }
        }
        boxes_out.push([
            (b[0] as i64 - pad).max(0) as u32,
            (b[1] as i64 - pad).max(0) as u32,
            (b[2] as i64 + pad).min(w as i64) as u32,
            (b[3] as i64 + pad).min(h as i64) as u32,
        ]);
    }
    let boxes_px = boxes_out;
    let boxes = boxes_px.iter().map(|b| [b[0] as f32 / scale, b[1] as f32 / scale, b[2] as f32 / scale, b[3] as f32 / scale]).collect();
    let mask = kept_mask.into_iter().map(|v| if v != 0 { 255 } else { 0 }).collect();
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
