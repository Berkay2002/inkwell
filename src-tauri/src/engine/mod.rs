//! Document service around MuPDF.
//!
//! Threading model: a file is parsed into a few `fz_document` replicas at most (`DOC_SLOTS`), each
//! behind its own mutex in the [`DocEntry`] and used by whichever worker holds the lock (each
//! thread has its own context cloned from the same base, which is what MuPDF requires). Replicas
//! are opened on demand: a lone user of a file only ever has one. Only page interpretation, text,
//! search and outline need a document; a page is interpreted once into a cached `DisplayList`,
//! which MuPDF documents as safe to run concurrently, and any number of workers rasterise
//! tiles from it in parallel. Replicas are not tied to threads, so closing a file frees them all.

pub mod cache;
pub mod edit;
pub mod fixture;
pub mod fonts;
pub mod pool;

use cache::{Tile, TileCache, TileKey};
use lru::LruCache;
use mupdf::{
    Colorspace, Device, DisplayList, Document, ImageFormat, Matrix, MetadataName, Pixmap,
    rect::{IRect, Rect},
    text_page::{TextBlockType, TextPageFlags},
};
use parking_lot::{Mutex, RwLock};
use crate::search::SearchOpts;
use pool::{Job, Pool, Prio, class};
use serde::Serialize;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use zeroize::Zeroizing;

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
    #[error("not a PDF file")]
    NotPdf,
    /// The document changed (an edit landed) while this request was waiting; ask again.
    #[error("stale")]
    Stale,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(String),
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
    Hits { search: u64, page: u32, hits: Vec<crate::search::Hit> },
    /// After each chunk of pages: how many of the `total` have been read.
    #[serde(rename_all = "camelCase")]
    Progress { search: u64, scanned: u32, total: u32 },
    /// `capped`: the search stopped at `search::DOC_CAP` hits; the ones listed are the first.
    #[serde(rename_all = "camelCase")]
    Done { search: u64, hits: u32, pages: u32, capped: bool },
}

pub enum TileOutcome {
    Ready(Tile),
    /// The view moved on before a worker got to this tile.
    Stale,
    Failed(String),
}

type TileResponder = Box<dyn FnOnce(TileOutcome) + Send>;

/// Parsed replicas per file. Interpreting a heavy page (large images) can take hundreds of
/// milliseconds; with one replica a screenful of cold pages would queue behind each other.
const DOC_SLOTS: usize = 4;

/// One parsed copy, and the edit generation it was opened at.
struct SharedDoc {
    doc: Document,
    rev: u32,
    /// The snapshot the copy reads in place (None: the file on disk). Declared after `doc` so
    /// the document is dropped first.
    _bytes: Option<Arc<Vec<u8>>>,
}

// SAFETY: a MuPDF document may move between threads as long as only one thread uses it at a time
// (its slot's mutex) and every thread works through its own context cloned from the same base
// (`mupdf::Context::get()`).
unsafe impl Send for SharedDoc {}

/// A snapshot of the document: the first `len` bytes of `bytes` are a complete PDF.
#[derive(Clone)]
struct Snap {
    bytes: Arc<Vec<u8>>,
    len: usize,
}

/// Where the current content of a document can be read from (see [`Engine::doc_source`]).
pub enum DocSource {
    Path(String),
    /// The first `len` bytes are a complete PDF.
    Bytes(Arc<Vec<u8>>, usize),
}

/// What one edit generation looks like. Swapped as a whole, so a page index is always read
/// together with the generation it belongs to.
struct Layout {
    /// Bumped by anything that changes the document's content; part of every tile and list key,
    /// so nothing rendered from the old content is served afterwards.
    rev: u32,
    page_count: u32,
    sizes: Vec<[f32; 2]>,
    /// Where replicas of this generation read from; None until the first edit (the file on disk).
    snap: Option<Snap>,
}

pub struct DocEntry {
    pub id: DocId,
    path: RwLock<String>,
    password: Mutex<Option<Zeroizing<String>>>,
    /// The open password of the file on disk after a save that set or removed security (`None` until
    /// then: the file is as it was opened and `password` unlocks it).
    file_password: Mutex<Option<Option<Zeroizing<String>>>>,
    epochs: [AtomicU64; STREAMS],
    search_gen: AtomicU64,
    layout: RwLock<Layout>,
    /// Set first thing on close: nothing may (re)open the file afterwards.
    closed: AtomicBool,
    slots: [Mutex<Option<SharedDoc>>; DOC_SLOTS],
    next_slot: AtomicUsize,
    /// The edit lane: the authoritative document and its history, created on the first edit.
    /// Every mutation holds this lock for its whole run, so edits to one file are serial.
    editor: Mutex<Option<edit::Editor>>,
}

impl DocEntry {
    pub fn page_count(&self) -> u32 {
        self.layout.read().page_count
    }

    pub fn generation(&self) -> u32 {
        self.layout.read().rev
    }

    pub fn path(&self) -> String {
        self.path.read().clone()
    }

    /// Page sizes (points, `/Rotate` applied) of the current generation.
    pub fn sizes(&self) -> Vec<[f32; 2]> {
        self.layout.read().sizes.clone()
    }
}

/// A display list being built or built. Errors are not kept: the next request tries again.
struct ListSlot(Mutex<Option<Arc<DisplayList>>>);

/// (document, edit generation, page)
type ListKey = (DocId, u32, u32);

struct Waiter {
    stream: usize,
    epoch: u64,
    respond: TileResponder,
}

struct Inflight {
    prio: Arc<Prio>,
    waiters: Vec<Waiter>,
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

pub struct Engine {
    docs: RwLock<HashMap<DocId, Arc<DocEntry>>>,
    next_id: AtomicU32,
    pool: Pool,
    tiles: Mutex<TileCache>,
    lists: Mutex<LruCache<ListKey, Arc<ListSlot>>>,
    inflight: Mutex<HashMap<TileKey, Inflight>>,
    rendered: AtomicU64,
    render_us: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl Engine {
    pub fn new(workers: usize, cache_budget: usize) -> Arc<Self> {
        fonts::install();
        Arc::new(Self {
            docs: RwLock::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            pool: Pool::new(workers.max(1)),
            tiles: Mutex::new(TileCache::new(cache_budget)),
            lists: Mutex::new(LruCache::new(NonZeroUsize::new(48).unwrap())),
            inflight: Mutex::new(HashMap::new()),
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
        check_pdf(path)?;
        edit::clean_stale_temps(std::path::Path::new(path));
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
        if let Err(e) = check_pdf(path) {
            return (0, Err(e));
        }
        edit::clean_stale_temps(std::path::Path::new(path));
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
            path: RwLock::new(path.to_string()),
            password: Mutex::new(None),
            file_password: Mutex::new(None),
            epochs: Default::default(),
            search_gen: AtomicU64::new(0),
            layout: RwLock::new(Layout { rev: 0, page_count: 0, sizes: Vec::new(), snap: None }),
            closed: AtomicBool::new(false),
            slots: Default::default(),
            next_slot: AtomicUsize::new(0),
            editor: Mutex::new(None),
        });
        self.docs.write().insert(id, entry.clone());
        entry
    }

    pub fn authenticate(&self, id: DocId, password: &str) -> Result<OpenInfo> {
        let entry = self.entry(id)?;
        *entry.password.lock() = Some(Zeroizing::new(password.to_string()));
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
        {
            let mut l = entry.layout.write();
            l.page_count = count;
            l.sizes = vec![first; count as usize];
        }
        Ok(OpenInfo {
            doc: entry.id,
            path: entry.path(),
            page_count: count,
            title,
            first_page: first,
        })
    }

    /// Releases the document and everything cached for it. The file handle is gone when this
    /// returns, unless a worker is mid-operation on the document, in which case that worker drops
    /// it as soon as it finishes (`with_doc`).
    pub fn close(&self, id: DocId) {
        let entry = self.docs.write().remove(&id);
        // Before the caches are emptied: a render finishing now checks `closed` under the cache
        // lock, so it either sees it or inserts before the purge below.
        if let Some(entry) = &entry {
            entry.closed.store(true, Ordering::SeqCst);
        }
        self.tiles.lock().drop_doc(id);
        {
            let mut lists = self.lists.lock();
            let stale: Vec<ListKey> = lists.iter().map(|(k, _)| *k).filter(|k| k.0 == id).collect();
            for k in stale {
                lists.pop(&k);
            }
        }
        if let Some(entry) = entry {
            // Busy replicas are dropped by their user when it sees `closed`.
            for slot in &entry.slots {
                if let Some(mut g) = slot.try_lock() {
                    *g = None;
                }
            }
            // A running edit drops it when it sees `closed`.
            if let Some(mut ed) = entry.editor.try_lock() {
                *ed = None;
            }
        }
    }

    /// Declares that the document's content changed: tiles and display lists rendered before
    /// this are never served again, and replicas opened before it are reopened on next use.
    /// Returns the new generation.
    pub fn bump_generation(&self, id: DocId) -> Result<u32> {
        let entry = self.entry(id)?;
        let mut l = entry.layout.write();
        l.rev += 1;
        Ok(l.rev)
    }

    /// A free replica, preferring one that is already open; blocks only when all are busy.
    fn lock_slot(entry: &DocEntry) -> parking_lot::MutexGuard<'_, Option<SharedDoc>> {
        let mut empty = None;
        for s in &entry.slots {
            if let Some(g) = s.try_lock() {
                if g.is_some() {
                    return g;
                }
                empty.get_or_insert(g);
            }
        }
        if let Some(g) = empty {
            return g;
        }
        let i = entry.next_slot.fetch_add(1, Ordering::Relaxed) % DOC_SLOTS;
        entry.slots[i].lock()
    }

    /// Runs `f` with a replica of the document, opening (and authenticating) one on first use.
    /// Keep `f` short so visible tiles are not starved.
    fn with_doc<R>(&self, entry: &DocEntry, f: impl FnOnce(&Document) -> Result<R>) -> Result<R> {
        self.with_doc_rev(entry, |doc, _| f(doc))
    }

    /// `with_doc`, also telling `f` which edit generation the replica holds.
    fn with_doc_rev<R>(&self, entry: &DocEntry, f: impl FnOnce(&Document, u32) -> Result<R>) -> Result<R> {
        // Make sure this thread's MuPDF context exists before any document it will drop.
        let _ = mupdf::Context::get();
        let mut slot = Self::lock_slot(entry);
        if entry.closed.load(Ordering::SeqCst) {
            *slot = None;
            return Err(EngineError::NoDoc);
        }
        let (rev, snap) = {
            let l = entry.layout.read();
            (l.rev, l.snap.clone())
        };
        if slot.as_ref().is_some_and(|d| d.rev != rev) {
            *slot = None;
        }
        if slot.is_none() {
            let mut d = match &snap {
                None => Document::open(entry.path().as_str())?,
                // SAFETY: snapshots are never mutated, and the slot keeps them alive past the document.
                Some(s) => unsafe { Document::from_shared_bytes(&s.bytes[..s.len], "application/pdf")? },
            };
            authenticate(&mut d, entry)?;
            *slot = Some(SharedDoc { doc: d, rev, _bytes: snap.map(|s| s.bytes) });
        }
        let r = f(&slot.as_ref().expect("opened above").doc, rev);
        if entry.closed.load(Ordering::SeqCst) {
            // closed while we were working: nobody else will drop it
            *slot = None;
        }
        r
    }

    /// Runs `f` on a pool worker and blocks for the result.
    pub fn call<R: Send + 'static>(
        self: &Arc<Self>,
        class: u8,
        f: impl FnOnce(&Engine) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let (tx, rx) = std::sync::mpsc::channel();
        let eng = self.clone();
        self.pool.submit(Job {
            prio: Prio::new(class, u64::MAX, 0),
            run: Box::new(move || {
                let _ = tx.send(f(&eng));
            }),
        });
        // A panicking job drops the sender; report it instead of taking the caller down.
        rx.recv().map_err(|_| EngineError::Panic)?
    }

    /// Queues `f` on a pool worker without waiting for it (print production's tiles, which are
    /// not plain page tiles but are scheduled with them).
    pub(crate) fn spawn(self: &Arc<Self>, class: u8, epoch: u64, rank: u32, f: impl FnOnce(&Engine) + Send + 'static) {
        let eng = self.clone();
        self.pool.submit(Job { prio: Prio::new(class, epoch, rank), run: Box::new(move || f(&eng)) });
    }

    // ---- pages ----------------------------------------------------------------------------

    pub(crate) fn page_list(&self, entry: &DocEntry, rev: u32, page: u32) -> Result<Arc<DisplayList>> {
        {
            let l = entry.layout.read();
            if l.rev != rev {
                return Err(EngineError::Stale);
            }
            if page >= l.page_count {
                return Err(EngineError::BadPage(page));
            }
        }
        let slot = {
            let mut lists = self.lists.lock();
            if entry.closed.load(Ordering::SeqCst) {
                return Err(EngineError::NoDoc);
            }
            lists
                .get_or_insert((entry.id, rev, page), || Arc::new(ListSlot(Mutex::new(None))))
                .clone()
        };
        // Held while building, so a second request for the same page waits for the first.
        let mut built = slot.0.lock();
        if let Some(list) = &*built {
            return Ok(list.clone());
        }
        let list = self.with_doc_rev(entry, |doc, held| {
            // The replica may already hold a newer generation than the one asked for.
            if held != rev {
                return Err(EngineError::Stale);
            }
            Ok(Arc::new(doc.load_page(page as i32)?.to_display_list(true)?))
        })?;
        *built = Some(list.clone());
        Ok(list)
    }

    /// Computes page sizes in chunks and hands each chunk to `sink(start, sizes)`.
    pub fn scan_sizes(
        self: &Arc<Self>,
        id: DocId,
        sink: Arc<dyn Fn(u32, Vec<[f32; 2]>) + Send + Sync>,
    ) -> Result<()> {
        let entry = self.entry(id)?;
        const CHUNK: u32 = 128;
        let (rev, n) = {
            let l = entry.layout.read();
            (l.rev, l.page_count)
        };
        for (ci, start) in (0..n).step_by(CHUNK as usize).enumerate() {
            let eng = self.clone();
            let entry = entry.clone();
            let sink = sink.clone();
            self.pool.submit(Job {
                prio: Prio::new(class::BACKGROUND, 0, ci as u32),
                run: Box::new(move || {
                    let end = (start + CHUNK).min(n);
                    // One lock per page, so a visible tile never waits for a whole chunk.
                    let mut sizes = Vec::with_capacity((end - start) as usize);
                    for p in start..end {
                        let size = eng.with_doc_rev(&entry, |doc, held| {
                            if held != rev {
                                return Err(EngineError::Stale);
                            }
                            let b = doc.load_page(p as i32)?.bounds()?;
                            Ok([b.width(), b.height()])
                        });
                        match size {
                            Ok(s) => sizes.push(s),
                            Err(_) => return,
                        }
                    }
                    // An edit since the scan started brought its own sizes.
                    let mut l = entry.layout.write();
                    if l.rev != rev {
                        return;
                    }
                    for (i, s) in sizes.iter().enumerate() {
                        if let Some(slot) = l.sizes.get_mut(start as usize + i) {
                            *slot = *s;
                        }
                    }
                    // Sent before an edit can publish, so the UI never gets them after its new sizes.
                    let _l = parking_lot::RwLockWriteGuard::downgrade(l);
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

    /// The document's non-empty info fields as (label, value), for the assistant.
    pub fn metadata(&self, id: DocId) -> Result<Vec<(&'static str, String)>> {
        let entry = self.entry(id)?;
        self.with_doc(&entry, |doc| {
            let fields = [
                ("title", MetadataName::Title),
                ("author", MetadataName::Author),
                ("subject", MetadataName::Subject),
                ("keywords", MetadataName::Keywords),
                ("creator", MetadataName::Creator),
                ("producer", MetadataName::Producer),
                ("created", MetadataName::CreationDate),
                ("modified", MetadataName::ModDate),
            ];
            Ok(fields
                .into_iter()
                .filter_map(|(k, n)| doc.metadata(n).ok().map(|v| (k, v.trim().to_string())))
                .filter(|(_, v)| !v.is_empty())
                .collect())
        })
    }

    /// One whole page as a PNG whose longer side is at most `max_px` (never scaled up past 2x), of
    /// edit generation `rev`: `Stale` when the document is already past it, or moves on while the
    /// page renders (docs/engine-rules.md rule 3), so an image never mixes with text of another state.
    pub fn page_png(&self, id: DocId, rev: u32, page: u32, max_px: u32) -> Result<Vec<u8>> {
        let entry = self.entry(id)?;
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        let list = self.page_list(&entry, rev, page)?;
        let b = list.bounds();
        let scale = (max_px as f32 / b.width().max(b.height()).max(1.0)).min(2.0);
        let (m, w, h) = view_matrix(b, 0, scale);
        let rect = IRect { x0: 0, y0: 0, x1: w, y1: h };
        let mut pix = Pixmap::new_with_rect(&Colorspace::device_rgb(), rect, false)?;
        pix.clear_with(0xff)?;
        {
            let dev = Device::from_pixmap(&pix)?;
            list.run(&dev, &m, Rect::new(0.0, 0.0, w as f32, h as f32))?;
        }
        let mut out = Vec::new();
        pix.write_to(&mut out, ImageFormat::PNG)?;
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        Ok(out)
    }

    pub fn page_text(&self, id: DocId, page: u32) -> Result<Vec<TextLine>> {
        let rev = self.entry(id)?.generation();
        self.page_text_at(id, rev, page)
    }

    /// `page_text` of generation `rev`: `Stale` once an edit has landed since.
    pub fn page_text_at(&self, id: DocId, rev: u32, page: u32) -> Result<Vec<TextLine>> {
        let entry = self.entry(id)?;
        let list = self.page_list(&entry, rev, page)?;
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
        if key.rev != entry.generation() {
            return respond(TileOutcome::Stale);
        }
        let stream = stream.min(STREAMS - 1);
        entry.epochs[stream].fetch_max(epoch, Ordering::SeqCst);
        // thumbnails (any stream but the main view) never delay the page being read
        let class = if stream == 0 { class::VISIBLE } else { class::INTERACTIVE };

        if let Some(t) = self.tiles.lock().get(&key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return respond(TileOutcome::Ready(t));
        }
        self.misses.fetch_add(1, Ordering::Relaxed);

        let prio = {
            let mut inflight = self.inflight.lock();
            if let Some(inf) = inflight.get_mut(&key) {
                // A main-view request joining a thumbnail job makes it a main-view job.
                inf.prio.class.fetch_min(class, Ordering::SeqCst);
                inf.prio.epoch.fetch_max(epoch, Ordering::SeqCst);
                inf.prio.rank.store(rank, Ordering::Relaxed);
                inf.waiters.push(Waiter { stream, epoch, respond });
                return;
            }
            let prio = Prio::new(class, epoch, rank);
            inflight.insert(key, Inflight { prio: prio.clone(), waiters: vec![Waiter { stream, epoch, respond }] });
            prio
        };

        let eng = self.clone();
        self.pool.submit(Job { prio, run: Box::new(move || eng.run_tile(entry, key)) });
    }

    fn run_tile(&self, entry: Arc<DocEntry>, key: TileKey) {
        // The same tile can be wanted by several streams (the page preview is both a thumbnail and
        // the main view's underlay): it is stale only once every one of them has moved on.
        let stale = {
            let mut inflight = self.inflight.lock();
            match inflight.get(&key) {
                None => return,
                Some(inf) => {
                    let all_stale = inf.waiters.iter().all(|w| w.epoch < entry.epochs[w.stream].load(Ordering::SeqCst));
                    if all_stale {
                        let waiters = inflight.remove(&key).map(|i| i.waiters).unwrap_or_default();
                        drop(inflight);
                        for w in waiters {
                            (w.respond)(TileOutcome::Stale);
                        }
                    }
                    all_stale
                }
            }
        };
        if stale {
            return;
        }
        let started = std::time::Instant::now();
        let outcome = match catch_unwind(AssertUnwindSafe(|| self.render_tile(&entry, &key))) {
            Ok(Ok(tile)) => {
                self.rendered.fetch_add(1, Ordering::Relaxed);
                self.render_us.fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                // Not for a document that was closed, or edited, while this rendered. Checked
                // under the cache lock, which `close` takes only after setting `closed`.
                let mut tiles = self.tiles.lock();
                if entry.closed.load(Ordering::SeqCst) {
                    TileOutcome::Failed(EngineError::NoDoc.to_string())
                } else if key.rev != entry.generation() {
                    TileOutcome::Stale
                } else {
                    tiles.insert(key, tile.clone());
                    TileOutcome::Ready(tile)
                }
            }
            Ok(Err(EngineError::Stale)) => TileOutcome::Stale,
            Ok(Err(e)) => TileOutcome::Failed(e.to_string()),
            Err(_) => TileOutcome::Failed(EngineError::Panic.to_string()),
        };
        let waiters = self.inflight.lock().remove(&key).map(|i| i.waiters).unwrap_or_default();
        for w in waiters {
            (w.respond)(match &outcome {
                TileOutcome::Ready(t) => TileOutcome::Ready(t.clone()),
                TileOutcome::Stale => TileOutcome::Stale,
                TileOutcome::Failed(e) => TileOutcome::Failed(e.clone()),
            });
        }
    }

    /// Synchronous render, used by the worker and by tests/benchmarks.
    pub fn render_tile(&self, entry: &DocEntry, key: &TileKey) -> Result<Tile> {
        let list = self.page_list(entry, key.rev, key.page)?;
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

    /// What the document currently shows, with the generation it belongs to (read together, so a
    /// page index taken from it is valid for that `rev`): the snapshot after an edit, the file
    /// on disk before the first one.
    pub fn doc_source(&self, id: DocId) -> Result<(u32, DocSource)> {
        let entry = self.entry(id)?;
        let l = entry.layout.read();
        let src = match &l.snap {
            Some(s) => DocSource::Bytes(s.bytes.clone(), s.len),
            None => DocSource::Path(entry.path()),
        };
        Ok((l.rev, src))
    }

    /// The password the user unlocked `id` with (redaction checks its encrypted output with it).
    pub(crate) fn password(&self, id: DocId) -> Result<Option<Zeroizing<String>>> {
        Ok(self.entry(id)?.password.lock().clone())
    }

    /// The password that opens the file on disk now: after Save wrote new password security it is the
    /// new open password, which the in-memory snapshots (`password`) do not share.
    pub(crate) fn file_password(&self, id: DocId) -> Result<Option<Zeroizing<String>>> {
        let entry = self.entry(id)?;
        let over = entry.file_password.lock().clone();
        Ok(match over {
            Some(p) => p,
            None => entry.password.lock().clone(),
        })
    }

    pub fn set_cache_budget(&self, bytes: usize) {
        self.tiles.lock().set_budget(bytes);
    }

    pub fn stats(&self) -> EngineStats {
        let t = self.tiles.lock();
        EngineStats {
            cache_bytes: t.bytes(),
            cache_tiles: t.len(),
            cache_budget: t.budget(),
            lists: self.lists.lock().len(),
            queued: self.pool.pending(),
            rendered: self.rendered.load(Ordering::Relaxed),
            render_ms_total: self.render_us.load(Ordering::Relaxed) / 1000,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    // ---- search ---------------------------------------------------------------------------

    /// Starts a search over every page with the default options. See [`Engine::search_with`].
    pub fn search(
        self: &Arc<Self>,
        id: DocId,
        query: String,
        sink: Arc<dyn Fn(SearchEvent) + Send + Sync>,
    ) -> Result<u64> {
        self.search_with(id, query, SearchOpts::default(), sink)
    }

    /// Starts a search over every page. Supersedes any search already running on the document.
    /// Returns the search id carried by every event.
    pub fn search_with(
        self: &Arc<Self>,
        id: DocId,
        query: String,
        opts: SearchOpts,
        sink: Arc<dyn Fn(SearchEvent) + Send + Sync>,
    ) -> Result<u64> {
        let entry = self.entry(id)?;
        let sid = entry.search_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let (rev, n) = {
            let l = entry.layout.read();
            (l.rev, l.page_count)
        };
        let Some(query) = crate::search::Query::new(&query, opts) else {
            sink(SearchEvent::Done { search: sid, hits: 0, pages: n, capped: false });
            return Ok(sid);
        };
        let query = Arc::new(query);
        const CHUNK: u32 = 16;
        let page_chunks = n.div_ceil(CHUNK) as usize;
        // One more job for the bookmarks, which belong to no page.
        let chunks = page_chunks + opts.bookmarks as usize;
        let remaining = Arc::new(AtomicUsize::new(chunks));
        let total_hits = Arc::new(AtomicU32::new(0));
        let capped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let scanned = Arc::new(AtomicU32::new(0));
        /// Takes up to `n` of the document-wide listing room; fewer (or none) means the cap was reached.
        fn reserve(total: &AtomicU32, n: usize) -> usize {
            let cap = crate::search::DOC_CAP as u32;
            let before = total.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| Some(c + (n as u32).min(cap.saturating_sub(c)))).unwrap_or(cap);
            (n as u32).min(cap.saturating_sub(before)) as usize
        }
        for ci in 0..chunks {
            let eng = self.clone();
            let entry = entry.clone();
            let sink = sink.clone();
            let query = query.clone();
            let remaining = remaining.clone();
            let total_hits = total_hits.clone();
            let capped = capped.clone();
            let scanned = scanned.clone();
            self.pool.submit(Job {
                prio: Prio::new(class::BACKGROUND, 0, ci as u32),
                run: Box::new(move || {
                    if ci >= page_chunks {
                        // Page numbers belong to `rev`: an edit ends the search (the UI searches again).
                        if entry.search_gen.load(Ordering::SeqCst) == sid {
                            let marks = eng.with_doc_rev(&entry, |doc, held| {
                                if held != rev {
                                    return Err(EngineError::Stale);
                                }
                                crate::search::bookmark_hits(doc, &query)
                            });
                            let mut by_page: std::collections::BTreeMap<u32, Vec<crate::search::Hit>> = Default::default();
                            for (p, h) in marks.unwrap_or_default() {
                                if p < n {
                                    by_page.entry(p).or_default().push(h);
                                }
                            }
                            for (page, mut hits) in by_page {
                                let kept = reserve(&total_hits, hits.len());
                                if kept < hits.len() {
                                    hits.truncate(kept);
                                    capped.store(true, Ordering::Relaxed);
                                }
                                if !hits.is_empty() {
                                    sink(SearchEvent::Hits { search: sid, page, hits });
                                }
                            }
                        }
                    } else {
                        let start = ci as u32 * CHUNK;
                        let end = (start + CHUNK).min(n);
                        for p in start..end {
                            if entry.search_gen.load(Ordering::SeqCst) != sid {
                                return;
                            }
                            if total_hits.load(Ordering::Relaxed) as usize >= crate::search::DOC_CAP {
                                capped.store(true, Ordering::Relaxed);
                                break;
                            }
                            // Page numbers belong to `rev`: an edit ends the search (the UI searches again).
                            let mut hits = match eng.search_hits_at(&entry, Some(rev), p, &query) {
                                Err(EngineError::Stale) => break,
                                r => r.unwrap_or_default(),
                            };
                            let kept = reserve(&total_hits, hits.len());
                            if kept < hits.len() {
                                hits.truncate(kept);
                                capped.store(true, Ordering::Relaxed);
                            }
                            if !hits.is_empty() {
                                sink(SearchEvent::Hits { search: sid, page: p, hits });
                            }
                        }
                        let done = scanned.fetch_add(end - start, Ordering::SeqCst) + (end - start);
                        sink(SearchEvent::Progress { search: sid, scanned: done, total: n });
                    }
                    if remaining.fetch_sub(1, Ordering::SeqCst) == 1 && entry.search_gen.load(Ordering::SeqCst) == sid {
                        sink(SearchEvent::Done {
                            search: sid,
                            hits: total_hits.load(Ordering::Relaxed),
                            pages: n,
                            capped: capped.load(Ordering::Relaxed),
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
        self.search_page_at(entry, None, page, needle)
    }

    /// `search_page` on generation `rev` (any if None): `Stale` once an edit has landed since.
    fn search_page_at(&self, entry: &DocEntry, rev: Option<u32>, page: u32, needle: &str) -> Result<Vec<[f32; 4]>> {
        let Some(q) = crate::search::Query::new(needle, SearchOpts::default()) else { return Ok(Vec::new()) };
        Ok(self.search_hits_at(entry, rev, page, &q)?.into_iter().flat_map(|h| h.rects).collect())
    }

    /// Every hit of `q` on `page` (text, and what the options add), on generation `rev` (any if None).
    fn search_hits_at(&self, entry: &DocEntry, rev: Option<u32>, page: u32, q: &crate::search::Query) -> Result<Vec<crate::search::Hit>> {
        self.with_doc_rev(entry, |doc, held| {
            if rev.is_some_and(|r| r != held) {
                return Err(EngineError::Stale);
            }
            crate::search::find_in_page(doc, page, q)
        })
    }
}

/// Unlocks `d` with the password the user gave for the entry, if it needs one.
fn authenticate(d: &mut Document, entry: &DocEntry) -> Result<()> {
    if d.needs_password()? {
        let pw = entry.password.lock().clone();
        match pw {
            None => return Err(EngineError::NeedsPassword),
            Some(pw) => {
                if d.authenticate(&pw)? {
                    return Ok(());
                }
                // MuPDF does no SASLprep, and the writer puts passwords in NFKC (Protect): a password
                // typed decomposed or full-width opens the file under its NFKC form. The form that
                // worked is kept, so every later read of the file uses it.
                match crate::protect::nfkc_form(&pw) {
                    Some(n) if d.authenticate(&n)? => *entry.password.lock() = Some(n),
                    _ => return Err(EngineError::BadPassword),
                }
            }
        }
    }
    Ok(())
}

/// Accepts a file only if `%PDF-` appears in its first KiB (where readers look for the header).
fn check_pdf(path: &str) -> Result<()> {
    use std::io::Read;
    let mut head = [0u8; 1024];
    let n = std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|e| EngineError::Mupdf(format!("cannot read file: {e}")))?;
    if head[..n].windows(5).any(|w| w == b"%PDF-") {
        Ok(())
    } else {
        Err(EngineError::NotPdf)
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

#[cfg(test)]
mod tests_followups;
