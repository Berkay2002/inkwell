//! Document service around MuPDF.
//!
//! Threading model: every worker thread lazily opens its *own* `fz_document` per `DocId`
//! (thread-local handle), so no MuPDF object is ever shared between threads except
//! `DisplayList`, which MuPDF documents as safe to run concurrently. A page is interpreted once
//! into a cached display list, then any number of workers rasterise tiles from it in parallel.

pub mod cache;
pub mod fixture;
pub mod pool;

use cache::{Tile, TileCache, TileKey};
use lru::LruCache;
use mupdf::{
    Colorspace, Device, DisplayList, Document, Matrix, MetadataName, Pixmap,
    rect::{IRect, Rect},
    text_page::{TextBlockType, TextPageFlags},
};
use parking_lot::{Mutex, RwLock};
use pool::{Job, Pool, Prio, class};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

pub type DocId = u32;
/// Tile edge in device pixels.
pub const TILE: u32 = 512;
/// Independent epoch counters per document: the main view, the thumbnail strip, spare.
pub const STREAMS: usize = 4;

#[derive(Debug, thiserror::Error, Clone)]
pub enum EngineError {
    #[error("{0}")]
    Mupdf(String),
    #[error("document is not open")]
    NoDoc,
    #[error("password required")]
    NeedsPassword,
    #[error("wrong password")]
    BadPassword,
    #[error("page {0} is out of range")]
    BadPage(u32),
    #[error("render failed unexpectedly")]
    Panic,
}

impl From<mupdf::Error> for EngineError {
    fn from(e: mupdf::Error) -> Self {
        EngineError::Mupdf(e.to_string())
    }
}

impl Serialize for EngineError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        // The UI matches on these codes.
        let code = match self {
            EngineError::NeedsPassword => "needs-password".to_string(),
            EngineError::BadPassword => "bad-password".to_string(),
            other => other.to_string(),
        };
        s.serialize_str(&code)
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenInfo {
    pub doc: DocId,
    pub path: String,
    pub page_count: u32,
    pub title: Option<String>,
    pub first_page: [f32; 2],
}

#[derive(Serialize, Clone, Debug)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<u32>,
    pub uri: Option<String>,
    pub children: Vec<OutlineItem>,
}

/// One line of page text, in unrotated page space (points, origin top-left).
#[derive(Serialize, Clone, Debug)]
pub struct TextLine {
    pub b: [f32; 4],
    pub s: f32,
    pub t: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SearchEvent {
    #[serde(rename_all = "camelCase")]
    Hits { search: u64, page: u32, rects: Vec<[f32; 4]> },
    #[serde(rename_all = "camelCase")]
    Done { search: u64, hits: u32, pages: u32 },
}

pub enum TileOutcome {
    Ready(Tile),
    /// The view moved on before a worker got to this tile.
    Stale,
    Failed(String),
}

type TileResponder = Box<dyn FnOnce(TileOutcome) + Send>;

pub struct DocEntry {
    pub id: DocId,
    pub path: String,
    password: Mutex<Option<String>>,
    page_count: AtomicU32,
    epochs: [AtomicU64; STREAMS],
    search_gen: AtomicU64,
    pub sizes: RwLock<Vec<[f32; 2]>>,
}

impl DocEntry {
    pub fn page_count(&self) -> u32 {
        self.page_count.load(Ordering::Relaxed)
    }
}

struct ListSlot(OnceLock<std::result::Result<Arc<DisplayList>, EngineError>>);

struct Inflight {
    prio: Arc<Prio>,
    waiters: Vec<TileResponder>,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EngineStats {
    pub cache_bytes: usize,
    pub cache_tiles: usize,
    pub cache_budget: usize,
    pub lists: usize,
    pub queued: usize,
    pub rendered: u64,
    pub render_ms_total: u64,
    pub hits: u64,
    pub misses: u64,
}

thread_local! {
    static HANDLES: RefCell<HashMap<DocId, Rc<Document>>> = RefCell::new(HashMap::new());
    static SEEN_CLOSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub struct Engine {
    docs: RwLock<HashMap<DocId, Arc<DocEntry>>>,
    next_id: AtomicU32,
    pool: Pool,
    tiles: Mutex<TileCache>,
    lists: Mutex<LruCache<(DocId, u32), Arc<ListSlot>>>,
    inflight: Mutex<HashMap<TileKey, Inflight>>,
    closes: AtomicU64,
    rendered: AtomicU64,
    render_us: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl Engine {
    pub fn new(workers: usize, cache_budget: usize) -> Arc<Self> {
        Arc::new(Self {
            docs: RwLock::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            pool: Pool::new(workers.max(1)),
            tiles: Mutex::new(TileCache::new(cache_budget)),
            lists: Mutex::new(LruCache::new(NonZeroUsize::new(48).unwrap())),
            inflight: Mutex::new(HashMap::new()),
            closes: AtomicU64::new(0),
            rendered: AtomicU64::new(0),
            render_us: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        })
    }

    /// Worker count that leaves the UI thread and WebView room on this machine.
    pub fn default_workers() -> usize {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        (n / 2).clamp(2, 8)
    }

    // ---- documents ------------------------------------------------------------------------

    fn entry(&self, id: DocId) -> Result<Arc<DocEntry>> {
        self.docs.read().get(&id).cloned().ok_or(EngineError::NoDoc)
    }

    /// Opens `path`. Returns `NeedsPassword` (with the document kept registered under the id in
    /// the error-adjacent `OpenInfo` flow) via [`Engine::open_pending`].
    pub fn open(&self, path: &str) -> Result<OpenInfo> {
        let entry = self.register(path);
        match self.finish_open(&entry) {
            Ok(info) => Ok(info),
            Err(e) => {
                self.close(entry.id);
                Err(e)
            }
        }
    }

    /// Like `open`, but keeps the registration when a password is needed so the UI can
    /// `authenticate` against the returned id.
    pub fn open_pending(&self, path: &str) -> (DocId, Result<OpenInfo>) {
        let entry = self.register(path);
        let r = self.finish_open(&entry);
        if let Err(e) = &r {
            if !matches!(e, EngineError::NeedsPassword) {
                self.close(entry.id);
            }
        }
        (entry.id, r)
    }

    fn register(&self, path: &str) -> Arc<DocEntry> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let entry = Arc::new(DocEntry {
            id,
            path: path.to_string(),
            password: Mutex::new(None),
            page_count: AtomicU32::new(0),
            epochs: Default::default(),
            search_gen: AtomicU64::new(0),
            sizes: RwLock::new(Vec::new()),
        });
        self.docs.write().insert(id, entry.clone());
        entry
    }

    pub fn authenticate(&self, id: DocId, password: &str) -> Result<OpenInfo> {
        let entry = self.entry(id)?;
        *entry.password.lock() = Some(password.to_string());
        // Drop any handle this thread cached before the password was known.
        HANDLES.with(|h| h.borrow_mut().remove(&id));
        let r = self.finish_open(&entry);
        if r.is_err() {
            *entry.password.lock() = None;
        }
        r
    }

    fn finish_open(&self, entry: &Arc<DocEntry>) -> Result<OpenInfo> {
        let (count, first, title) = self.with_doc(entry, |doc| {
            let count = doc.page_count()?.max(0) as u32;
            if count == 0 {
                return Err(EngineError::Mupdf("document has no pages".into()));
            }
            let b = doc.load_page(0)?.bounds()?;
            let title = doc.metadata(MetadataName::Title).ok().filter(|t| !t.trim().is_empty());
            Ok((count, [b.width(), b.height()], title))
        })?;
        entry.page_count.store(count, Ordering::SeqCst);
        *entry.sizes.write() = vec![first; count as usize];
        Ok(OpenInfo {
            doc: entry.id,
            path: entry.path.clone(),
            page_count: count,
            title,
            first_page: first,
        })
    }

    pub fn close(&self, id: DocId) {
        self.docs.write().remove(&id);
        self.tiles.lock().drop_doc(id);
        self.lists.lock().clear();
        self.closes.fetch_add(1, Ordering::SeqCst);
    }

    /// Runs `f` with this thread's handle for the document, opening (and authenticating) it on
    /// first use.
    fn with_doc<R>(&self, entry: &DocEntry, f: impl FnOnce(&Document) -> Result<R>) -> Result<R> {
        // Create this thread's MuPDF context before HANDLES so it is destroyed after the documents.
        let _ = mupdf::Context::get();
        let closes = self.closes.load(Ordering::Relaxed);
        if SEEN_CLOSES.with(|c| c.replace(closes)) != closes {
            let live = self.docs.read();
            HANDLES.with(|h| h.borrow_mut().retain(|id, _| live.contains_key(id)));
        }
        let cached = HANDLES.with(|h| h.borrow().get(&entry.id).cloned());
        let doc = match cached {
            Some(d) => d,
            None => {
                let mut d = Document::open(entry.path.as_str())?;
                if d.needs_password()? {
                    let pw = entry.password.lock().clone();
                    match pw {
                        None => return Err(EngineError::NeedsPassword),
                        Some(pw) => {
                            if !d.authenticate(&pw)? {
                                return Err(EngineError::BadPassword);
                            }
                        }
                    }
                }
                let d = Rc::new(d);
                HANDLES.with(|h| h.borrow_mut().insert(entry.id, d.clone()));
                d
            }
        };
        f(&doc)
    }

    /// Runs `f` on a pool worker and blocks for the result. For commands that want the
    /// worker-local document handle.
    pub fn call<R: Send + 'static>(
        self: &Arc<Self>,
        class: u8,
        f: impl FnOnce(&Engine) -> R + Send + 'static,
    ) -> R {
        let (tx, rx) = std::sync::mpsc::channel();
        let eng = self.clone();
        self.pool.submit(Job {
            class,
            prio: Prio::new(u64::MAX, 0),
            run: Box::new(move || {
                let _ = tx.send(f(&eng));
            }),
        });
        rx.recv().expect("worker dropped result")
    }

    // ---- pages ----------------------------------------------------------------------------

    fn page_list(&self, entry: &DocEntry, page: u32) -> Result<Arc<DisplayList>> {
        if page >= entry.page_count() {
            return Err(EngineError::BadPage(page));
        }
        let slot = {
            let mut lists = self.lists.lock();
            lists
                .get_or_insert((entry.id, page), || Arc::new(ListSlot(OnceLock::new())))
                .clone()
        };
        slot.0
            .get_or_init(|| {
                self.with_doc(entry, |doc| {
                    let list = doc.load_page(page as i32)?.to_display_list(true)?;
                    Ok(Arc::new(list))
                })
            })
            .clone()
    }

    /// Computes page sizes in chunks and hands each chunk to `sink(start, sizes)`.
    pub fn scan_sizes(
        self: &Arc<Self>,
        id: DocId,
        sink: Arc<dyn Fn(u32, Vec<[f32; 2]>) + Send + Sync>,
    ) -> Result<()> {
        let entry = self.entry(id)?;
        const CHUNK: u32 = 128;
        let n = entry.page_count();
        for (ci, start) in (0..n).step_by(CHUNK as usize).enumerate() {
            let eng = self.clone();
            let entry = entry.clone();
            let sink = sink.clone();
            self.pool.submit(Job {
                class: class::BACKGROUND,
                prio: Prio::new(0, ci as u32),
                run: Box::new(move || {
                    let end = (start + CHUNK).min(n);
                    let sizes = eng
                        .with_doc(&entry, |doc| {
                            let mut v = Vec::with_capacity((end - start) as usize);
                            for p in start..end {
                                let b = doc.load_page(p as i32)?.bounds()?;
                                v.push([b.width(), b.height()]);
                            }
                            Ok(v)
                        })
                        .unwrap_or_default();
                    if sizes.is_empty() {
                        return;
                    }
                    {
                        let mut all = entry.sizes.write();
                        for (i, s) in sizes.iter().enumerate() {
                            if let Some(slot) = all.get_mut(start as usize + i) {
                                *slot = *s;
                            }
                        }
                    }
                    sink(start, sizes);
                }),
            });
        }
        Ok(())
    }

    pub fn outline(&self, id: DocId) -> Result<Vec<OutlineItem>> {
        fn conv(o: &mupdf::Outline) -> OutlineItem {
            OutlineItem {
                title: o.title.clone(),
                page: o.dest.map(|d| d.loc.page_number),
                uri: o.uri.clone(),
                children: o.down.iter().map(conv).collect(),
            }
        }
        let entry = self.entry(id)?;
        self.with_doc(&entry, |doc| Ok(doc.outlines()?.iter().map(conv).collect()))
    }

    pub fn page_text(&self, id: DocId, page: u32) -> Result<Vec<TextLine>> {
        let entry = self.entry(id)?;
        let list = self.page_list(&entry, page)?;
        let origin = list.bounds();
        let tp = list.to_text_page(TextPageFlags::empty())?;
        let mut out = Vec::new();
        for block in tp.blocks() {
            if block.r#type() != TextBlockType::Text {
                continue;
            }
            for line in block.lines() {
                let mut t = String::new();
                let mut size = 0.0f32;
                for ch in line.chars() {
                    if size == 0.0 {
                        size = ch.size();
                    }
                    if let Some(c) = ch.char() {
                        t.push(c);
                    }
                }
                if t.trim().is_empty() {
                    continue;
                }
                let b = line.bounds();
                out.push(TextLine {
                    b: [b.x0 - origin.x0, b.y0 - origin.y0, b.x1 - origin.x0, b.y1 - origin.y0],
                    s: size,
                    t,
                });
            }
        }
        Ok(out)
    }

    // ---- tiles ----------------------------------------------------------------------------

    /// Entry point for the tile protocol. `respond` is called exactly once, possibly on a
    /// worker thread, possibly before this returns (cache hit).
    pub fn request_tile(
        self: &Arc<Self>,
        key: TileKey,
        stream: usize,
        epoch: u64,
        rank: u32,
        respond: TileResponder,
    ) {
        let entry = match self.entry(key.doc) {
            Ok(e) => e,
            Err(e) => return respond(TileOutcome::Failed(e.to_string())),
        };
        let stream = stream.min(STREAMS - 1);
        entry.epochs[stream].fetch_max(epoch, Ordering::SeqCst);

        if let Some(t) = self.tiles.lock().get(&key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return respond(TileOutcome::Ready(t));
        }
        self.misses.fetch_add(1, Ordering::Relaxed);

        let prio = {
            let mut inflight = self.inflight.lock();
            if let Some(inf) = inflight.get_mut(&key) {
                inf.prio.epoch.fetch_max(epoch, Ordering::SeqCst);
                inf.prio.rank.store(rank, Ordering::Relaxed);
                inf.waiters.push(respond);
                return;
            }
            let prio = Prio::new(epoch, rank);
            inflight.insert(key, Inflight { prio: prio.clone(), waiters: vec![respond] });
            prio
        };

        let eng = self.clone();
        let job_prio = prio.clone();
        self.pool.submit(Job {
            class: class::VISIBLE,
            prio,
            run: Box::new(move || eng.run_tile(entry, key, stream, job_prio)),
        });
    }

    fn run_tile(&self, entry: Arc<DocEntry>, key: TileKey, stream: usize, prio: Arc<Prio>) {
        let stale = prio.epoch.load(Ordering::SeqCst) < entry.epochs[stream].load(Ordering::SeqCst);
        let outcome = if stale {
            TileOutcome::Stale
        } else {
            let started = std::time::Instant::now();
            match catch_unwind(AssertUnwindSafe(|| self.render_tile(&entry, &key))) {
                Ok(Ok(tile)) => {
                    self.rendered.fetch_add(1, Ordering::Relaxed);
                    self.render_us.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                    self.tiles.lock().insert(key, tile.clone());
                    TileOutcome::Ready(tile)
                }
                Ok(Err(e)) => TileOutcome::Failed(e.to_string()),
                Err(_) => TileOutcome::Failed(EngineError::Panic.to_string()),
            }
        };
        let waiters = self.inflight.lock().remove(&key).map(|i| i.waiters).unwrap_or_default();
        for w in waiters {
            w(match &outcome {
                TileOutcome::Ready(t) => TileOutcome::Ready(t.clone()),
                TileOutcome::Stale => TileOutcome::Stale,
                TileOutcome::Failed(e) => TileOutcome::Failed(e.clone()),
            });
        }
    }

    /// Synchronous render, used by the worker and by tests/benchmarks.
    pub fn render_tile(&self, entry: &DocEntry, key: &TileKey) -> Result<Tile> {
        let list = self.page_list(entry, key.page)?;
        let scale = key.scale as f32 / 1000.0;
        let (m, pw, ph) = view_matrix(list.bounds(), key.rot, scale);
        let x0 = key.tx as i32 * TILE as i32;
        let y0 = key.ty as i32 * TILE as i32;
        if x0 >= pw || y0 >= ph {
            return Err(EngineError::Mupdf("tile outside page".into()));
        }
        let rect = IRect { x0, y0, x1: (x0 + TILE as i32).min(pw), y1: (y0 + TILE as i32).min(ph) };
        let (w, h) = (rect.width() as usize, rect.height() as usize);

        let cs = Colorspace::device_rgb();
        let mut pix = Pixmap::new_with_rect(&cs, rect, true)?;
        pix.clear_with(0xff)?;
        {
            let dev = Device::from_pixmap_with_clip(&pix, rect)?;
            // MuPDF takes the cull rectangle in device space (after the ctm).
            let area = Rect::new(x0 as f32, y0 as f32, rect.x1 as f32, rect.y1 as f32);
            list.run(&dev, &m, area)?;
        }
        let stride = pix.stride() as usize;
        let samples = pix.samples();
        let rgba = if stride == w * 4 {
            samples[..w * h * 4].to_vec()
        } else {
            let mut v = Vec::with_capacity(w * h * 4);
            for row in 0..h {
                v.extend_from_slice(&samples[row * stride..row * stride + w * 4]);
            }
            v
        };
        Ok(Tile { w: w as u32, h: h as u32, rgba: Arc::new(rgba) })
    }

    pub fn doc_entry(&self, id: DocId) -> Result<Arc<DocEntry>> {
        self.entry(id)
    }

    pub fn set_cache_budget(&self, bytes: usize) {
        self.tiles.lock().set_budget(bytes);
    }

    pub fn stats(&self) -> EngineStats {
        let t = self.tiles.lock();
        EngineStats {
            cache_bytes: t.bytes(),
            cache_tiles: t.len(),
            cache_budget: 0,
            lists: self.lists.lock().len(),
            queued: self.pool.pending(),
            rendered: self.rendered.load(Ordering::Relaxed),
            render_ms_total: self.render_us.load(Ordering::Relaxed) / 1000,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    // ---- search ---------------------------------------------------------------------------

    /// Starts a search over every page. Supersedes any search already running on the document.
    /// Returns the search id carried by every event.
    pub fn search(
        self: &Arc<Self>,
        id: DocId,
        query: String,
        sink: Arc<dyn Fn(SearchEvent) + Send + Sync>,
    ) -> Result<u64> {
        let entry = self.entry(id)?;
        let sid = entry.search_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let query = query.trim().to_string();
        let n = entry.page_count();
        if query.is_empty() {
            sink(SearchEvent::Done { search: sid, hits: 0, pages: n });
            return Ok(sid);
        }
        const CHUNK: u32 = 16;
        let chunks = n.div_ceil(CHUNK) as usize;
        let remaining = Arc::new(AtomicUsize::new(chunks));
        let total_hits = Arc::new(AtomicU32::new(0));
        for ci in 0..chunks {
            let eng = self.clone();
            let entry = entry.clone();
            let sink = sink.clone();
            let query = query.clone();
            let remaining = remaining.clone();
            let total_hits = total_hits.clone();
            self.pool.submit(Job {
                class: class::BACKGROUND,
                prio: Prio::new(0, ci as u32),
                run: Box::new(move || {
                    let start = ci as u32 * CHUNK;
                    let end = (start + CHUNK).min(n);
                    for p in start..end {
                        if entry.search_gen.load(Ordering::SeqCst) != sid {
                            return;
                        }
                        let rects = eng.search_page(&entry, p, &query).unwrap_or_default();
                        if !rects.is_empty() {
                            total_hits.fetch_add(rects.len() as u32, Ordering::Relaxed);
                            sink(SearchEvent::Hits { search: sid, page: p, rects });
                        }
                    }
                    if remaining.fetch_sub(1, Ordering::SeqCst) == 1 {
                        sink(SearchEvent::Done {
                            search: sid,
                            hits: total_hits.load(Ordering::Relaxed),
                            pages: n,
                        });
                    }
                }),
            });
        }
        Ok(sid)
    }

    pub fn cancel_search(&self, id: DocId) {
        if let Ok(e) = self.entry(id) {
            e.search_gen.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn search_page(&self, entry: &DocEntry, page: u32, needle: &str) -> Result<Vec<[f32; 4]>> {
        self.with_doc(entry, |doc| {
            let p = doc.load_page(page as i32)?;
            let origin = p.bounds()?;
            let quads = p.search(needle, 512)?;
            Ok(quads
                .iter()
                .map(|q| {
                    let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
                    let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
                    let min = |a: [f32; 4]| a.into_iter().fold(f32::INFINITY, f32::min);
                    let max = |a: [f32; 4]| a.into_iter().fold(f32::NEG_INFINITY, f32::max);
                    [min(xs) - origin.x0, min(ys) - origin.y0, max(xs) - origin.x0, max(ys) - origin.y0]
                })
                .collect())
        })
    }
}

/// Matrix taking page space to device pixels for the given view rotation and scale, plus the
/// resulting page size in device pixels.
pub fn view_matrix(bounds: Rect, rot: u8, scale: f32) -> (Matrix, i32, i32) {
    let mut m = Matrix::new_translate(-bounds.x0, -bounds.y0);
    m.concat(Matrix::new_rotate((rot % 4) as f32 * 90.0));
    let r = Rect::new(0.0, 0.0, bounds.width(), bounds.height()).transform(&m);
    m.concat(Matrix::new_translate(-r.x0, -r.y0));
    m.concat(Matrix::new_scale(scale, scale));
    let w = (r.width() * scale).ceil().max(1.0) as i32;
    let h = (r.height() * scale).ceil().max(1.0) as i32;
    (m, w, h)
}

#[cfg(test)]
mod tests;
