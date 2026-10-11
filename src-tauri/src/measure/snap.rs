//! Snapping: the nearest point of the page's vector paths to a point, so the UI can snap a measurement to a drawing.
//!
//! The paths come from running the page's contents (not its annotations, so a measurement does not snap to
//! itself) through a MuPDF device. Everything is in page space, as the viewer shows the page.
//!
//! The rule: an end point (a corner or path end) or a midpoint of a segment within the radius wins, the nearer of the
//! two (an end point on a tie); with none, the nearest point on any segment within the radius ("line"). Curves count
//! by their end points, their t = 0.5 point as the midpoint, and twelve straight pieces for "line".
//!
//! A page is read once per revision ([`SnapCache`]; concurrent first requests share one read) and never without
//! limits: at most [`MAX_SEGMENTS`] segments and [`TIME_LIMIT`] of reading, the second enforced by a watchdog thread that
//! aborts the MuPDF run whatever it is busy with (text, images, one huge path). Past either limit the index keeps what
//! it has (`truncated`).

use super::calc::{P2, dist};
use crate::engine::EngineError;
use crate::engine::{DocId, Engine};
use lru::LruCache;
use mupdf::pdf::PdfDocument;
use mupdf::{ColorParams, Colorspace, StrokeState};
use mupdf::{Cookie, Device, Matrix, NativeDevice, Path, PathWalker};
use parking_lot::Mutex;
use serde::Serialize;
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// Segments kept per page (a curve is 12). About 8 MB of index at the limit.
pub const MAX_SEGMENTS: usize = 100_000;
pub const TIME_LIMIT: Duration = Duration::from_millis(1500);

type Result<T> = std::result::Result<T, EngineError>;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SnapKind {
    Endpoint,
    Midpoint,
    Line,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SnapHit {
    pub kind: SnapKind,
    pub x: f32,
    pub y: f32,
    /// From the point asked about.
    pub distance: f32,
}

/// The segments and anchor points of one page, built once and asked many times (the UI snaps on every mouse move).
#[derive(Default, Debug)]
pub struct SnapIndex {
    segments: Vec<(P2, P2)>,
    ends: Vec<P2>,
    mids: Vec<P2>,
    /// The page had more than the limits allow; snapping covers only what was read first.
    pub truncated: bool,
}

impl SnapIndex {
    /// Reads page `page` (0-based) of `doc`, within the limits.
    pub fn build(doc: &PdfDocument, page: u32) -> Result<SnapIndex> {
        Self::build_within(doc, page, TIME_LIMIT)
    }

    pub(crate) fn build_within(doc: &PdfDocument, page: u32, limit: Duration) -> Result<SnapIndex> {
        let p = doc.load_pdf_page(page as i32)?;
        let cookie = Rc::new(Cookie::new()?);
        let collector = Rc::new(RefCell::new(Collector { cookie: cookie.clone(), index: SnapIndex::default() }));
        let device = Device::from_native(collector.clone())?;
        // The watchdog waits for the run to finish; if the limit passes first it aborts the run.
        let timed_out = AtomicBool::new(false);
        let (done, finished) = mpsc::channel::<()>();
        let (timed, handle) = (&timed_out, cookie.abort_handle());
        let run = std::thread::scope(|s| {
            s.spawn(move || {
                if finished.recv_timeout(limit) == Err(RecvTimeoutError::Timeout) {
                    timed.store(true, Ordering::SeqCst);
                    handle.abort();
                }
            });
            let run = p.run_contents_with_cookie(&device, &Matrix::IDENTITY, &cookie);
            drop(done);
            run
        });
        drop(device);
        let mut index = std::mem::take(&mut collector.borrow_mut().index);
        index.truncated |= timed_out.load(Ordering::SeqCst);
        // An aborted run may report an error; what was read before the limit is what we keep.
        if let Err(e) = run
            && !index.truncated
        {
            return Err(e.into());
        }
        Ok(index)
    }

    /// The best snap within `radius` of `(x, y)`, or `None`.
    pub fn snap(&self, x: f32, y: f32, radius: f32) -> Option<SnapHit> {
        let at = [f64::from(x), f64::from(y)];
        let r = f64::from(radius);
        let nearest = |pts: &[P2]| pts.iter().map(|p| (dist(at, *p), *p)).filter(|(d, _)| *d <= r).min_by(|a, b| a.0.total_cmp(&b.0));
        let hit = |kind, d: f64, p: P2| SnapHit { kind, x: p[0] as f32, y: p[1] as f32, distance: d as f32 };
        match (nearest(&self.ends), nearest(&self.mids)) {
            (Some((de, pe)), Some((dm, pm))) => return Some(if dm < de { hit(SnapKind::Midpoint, dm, pm) } else { hit(SnapKind::Endpoint, de, pe) }),
            (Some((d, p)), None) => return Some(hit(SnapKind::Endpoint, d, p)),
            (None, Some((d, p))) => return Some(hit(SnapKind::Midpoint, d, p)),
            (None, None) => {}
        }
        self.segments
            .iter()
            .map(|&(a, b)| {
                let q = closest(a, b, at);
                (dist(at, q), q)
            })
            .filter(|(d, _)| *d <= r)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(d, p)| hit(SnapKind::Line, d, p))
    }
}

/// The point of the segment `a`-`b` nearest to `p`.
fn closest(a: P2, b: P2, p: P2) -> P2 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-12 {
        return a;
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    [a[0] + t * dx, a[1] + t * dy]
}

struct Collector {
    index: SnapIndex,
    cookie: Rc<Cookie>,
}

struct Walk<'a> {
    ctm: &'a Matrix,
    out: &'a mut SnapIndex,
    start: Option<P2>,
    cur: Option<P2>,
}

impl Walk<'_> {
    fn map(&self, x: f32, y: f32) -> P2 {
        let (x, y) = self.ctm.transform_xy(x, y);
        [f64::from(x), f64::from(y)]
    }

    /// Room for `n` more segments; past the limit the page is marked truncated and nothing more is kept.
    fn room(&mut self, n: usize) -> bool {
        if self.out.segments.len() + n > MAX_SEGMENTS {
            self.out.truncated = true;
            return false;
        }
        true
    }

    fn line(&mut self, a: P2, b: P2) {
        if dist(a, b) < 1e-6 || !self.room(1) {
            return;
        }
        self.out.segments.push((a, b));
        self.out.ends.extend([a, b]);
        self.out.mids.push([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    }
}

impl PathWalker for Walk<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.start = Some(p);
        self.cur = Some(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        if let Some(c) = self.cur {
            self.line(c, p);
        }
        self.cur = Some(p);
    }

    fn curve_to(&mut self, cx1: f32, cy1: f32, cx2: f32, cy2: f32, ex: f32, ey: f32) {
        let (c1, c2, e) = (self.map(cx1, cy1), self.map(cx2, cy2), self.map(ex, ey));
        let Some(s) = self.cur else { return };
        if !self.room(12) {
            self.cur = Some(e);
            return;
        }
        let at = |t: f64| {
            let u = 1.0 - t;
            let f = |i: usize| u * u * u * s[i] + 3.0 * u * u * t * c1[i] + 3.0 * u * t * t * c2[i] + t * t * t * e[i];
            [f(0), f(1)]
        };
        let mut prev = s;
        for i in 1..=12 {
            let p = at(f64::from(i) / 12.0);
            self.out.segments.push((prev, p));
            prev = p;
        }
        self.out.ends.extend([s, e]);
        self.out.mids.push(at(0.5));
        self.cur = Some(e);
    }

    fn close(&mut self) {
        if let (Some(c), Some(s)) = (self.cur, self.start) {
            self.line(c, s);
            self.cur = Some(s);
        }
    }
}

impl Collector {
    fn take_path(&mut self, path: &Path, ctm: &Matrix) {
        if self.index.truncated {
            self.cookie.abort_shared();
            return;
        }
        let mut w = Walk { ctm, out: &mut self.index, start: None, cur: None };
        let _ = path.walk(&mut w);
    }
}

#[allow(unused_variables)]
impl NativeDevice for Collector {
    fn fill_path(&mut self, path: &Path, even_odd: bool, cmt: Matrix, color_space: &Colorspace, color: &[f32], alpha: f32, cp: ColorParams) {
        self.take_path(path, &cmt);
    }

    fn stroke_path(&mut self, path: &Path, stroke_state: &StrokeState, cmt: Matrix, color_space: &Colorspace, color: &[f32], alpha: f32, cp: ColorParams) {
        self.take_path(path, &cmt);
    }
}

/// Page indexes kept between calls, by (document, revision, page), so a mouse move does not read the page again. The
/// revision is the edit state's `rev`: it only grows, so an entry of an old one is never asked for again.
pub struct SnapCache {
    map: Mutex<LruCache<(DocId, u32, u32), Arc<SnapIndex>>>,
    /// Pages read so far (what the cache saved is not counted).
    pub builds: AtomicUsize,
}

impl Default for SnapCache {
    fn default() -> Self {
        SnapCache { map: Mutex::new(LruCache::new(NonZeroUsize::new(4).expect("4 is not zero"))), builds: AtomicUsize::new(0) }
    }
}

impl SnapCache {
    /// The index of `page` as the document is now (built on the edit lane the first time). Callers that all missed
    /// before the first read finished look again once they hold the lane, so one read serves them all.
    pub fn index(&self, eng: &Engine, doc: DocId, page: u32) -> Result<Arc<SnapIndex>> {
        let key = (doc, eng.edit_state(doc)?.rev, page);
        if let Some(i) = self.map.lock().get(&key) {
            return Ok(i.clone());
        }
        eng.read_custom(doc, |d| {
            if let Some(i) = self.map.lock().get(&key) {
                return Ok(i.clone());
            }
            self.builds.fetch_add(1, Ordering::SeqCst);
            let built = Arc::new(SnapIndex::build(d, page)?);
            self.map.lock().put(key, built.clone());
            Ok(built)
        })
    }
}
