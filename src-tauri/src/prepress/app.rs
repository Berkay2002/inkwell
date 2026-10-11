//! Tauri commands for print production, and the tile route: `inkwell://.../p/{doc}/{page}/{rot}/{scale}/{tx}/{ty}?v={view}&g={rev}&e=..&r=..&s=..`
//! is the page tile route with a registered view (plates on or off, coverage) added. The UI is the
//! user; the assistant calls the functions in `super`.

use super::boxes::BoxSpec;
use super::ink::{DEFAULT_LIMIT, DEFAULT_MAX_PX, doc_ink};
use super::{ConvertReport, ConvertSpec, InkReport, Inspected, IntentSource, MarksSpec, PageSeps, Result, View, add_printer_marks, convert_colors, inspect_at, page_separations, render_view_tile, set_boxes};
use crate::a11y::Progress;
use crate::engine::cache::{Tile, TileKey};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::pool::class;
use crate::engine::{DocId, Engine, EngineError, TileOutcome};
use crate::paths::Paths;
use lru::LruCache;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::Window;

type Eng<'a> = tauri::State<'a, Arc<Engine>>;
type Jobs<'a> = tauri::State<'a, Arc<crate::tools::Jobs>>;
type Pre<'a> = tauri::State<'a, Arc<Prepress>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// Tiles of the registered views are kept up to this many bytes.
const TILE_BUDGET: usize = 48 << 20;

struct TileCache {
    map: LruCache<(TileKey, u32), Tile>,
    bytes: usize,
}

impl TileCache {
    fn insert(&mut self, k: (TileKey, u32), t: Tile) {
        self.bytes += t.rgba.len();
        if let Some(old) = self.map.put(k, t) {
            self.bytes -= old.rgba.len();
        }
        while self.bytes > TILE_BUDGET && self.map.len() > 1 {
            if let Some((_, t)) = self.map.pop_lru() {
                self.bytes -= t.rgba.len();
            }
        }
    }
}

struct Views {
    next: u32,
    by_id: HashMap<u32, (DocId, View)>,
}

/// The registered views and what is cached for them.
pub struct Prepress {
    views: Mutex<Views>,
    seps: Mutex<HashMap<(DocId, u32, u32), Arc<PageSeps>>>,
    tiles: Mutex<TileCache>,
}

impl Default for Prepress {
    fn default() -> Self {
        Prepress {
            views: Mutex::new(Views { next: 1, by_id: HashMap::new() }),
            seps: Mutex::new(HashMap::new()),
            tiles: Mutex::new(TileCache { map: LruCache::unbounded(), bytes: 0 }),
        }
    }
}

impl Prepress {
    /// An id for `view` of `doc`, to put in tile URLs. The same view gets the same id.
    pub fn register(&self, doc: DocId, view: View) -> u32 {
        let mut v = self.views.lock();
        if let Some((id, _)) = v.by_id.iter().find(|(_, (d, w))| *d == doc && *w == view) {
            return *id;
        }
        let id = v.next;
        v.next += 1;
        v.by_id.insert(id, (doc, view));
        id
    }

    fn view(&self, id: u32, doc: DocId) -> Option<View> {
        self.views.lock().by_id.get(&id).filter(|(d, _)| *d == doc).map(|(_, v)| v.clone())
    }

    /// Forgets a closed document's views and tiles.
    pub fn drop_doc(&self, doc: DocId) {
        self.views.lock().by_id.retain(|_, (d, _)| *d != doc);
        self.seps.lock().retain(|k, _| k.0 != doc);
        let mut t = self.tiles.lock();
        let keys: Vec<_> = t.map.iter().filter(|(k, _)| k.0.doc == doc).map(|(k, _)| *k).collect();
        for k in keys {
            if let Some(old) = t.map.pop(&k) {
                t.bytes -= old.rgba.len();
            }
        }
    }

    fn page_seps(&self, eng: &Engine, key: &TileKey) -> Result<Arc<PageSeps>> {
        let k = (key.doc, key.rev, key.page);
        if let Some(s) = self.seps.lock().get(&k) {
            return Ok(s.clone());
        }
        let s = Arc::new(page_separations(eng, key.doc, key.page)?);
        // only for the generation it was read at
        if eng.doc_entry(key.doc)?.generation() != key.rev {
            return Err(EngineError::Stale);
        }
        let mut m = self.seps.lock();
        m.retain(|(d, r, _), _| *d != key.doc || *r == key.rev);
        m.insert(k, s.clone());
        Ok(s)
    }

    fn render(&self, eng: &Engine, key: &TileKey, view_id: u32, view: &View) -> Result<Tile> {
        let seps = self.page_seps(eng, key)?;
        let tile = render_view_tile(eng, &seps, key, view)?;
        if eng.doc_entry(key.doc)?.generation() != key.rev {
            return Err(EngineError::Stale);
        }
        self.tiles.lock().insert((*key, view_id), tile.clone());
        Ok(tile)
    }

    /// Serves one tile; the same contract as `Engine::request_tile` (stale when the document moved on).
    pub fn request_tile(self: &Arc<Self>, eng: &Arc<Engine>, key: TileKey, view_id: u32, epoch: u64, rank: u32, respond: Box<dyn FnOnce(TileOutcome) + Send>) {
        let entry = match eng.doc_entry(key.doc) {
            Ok(e) => e,
            Err(e) => return respond(TileOutcome::Failed(e.to_string())),
        };
        if key.rev != entry.generation() {
            return respond(TileOutcome::Stale);
        }
        let Some(view) = self.view(view_id, key.doc) else { return respond(TileOutcome::Failed("unknown view".into())) };
        if let Some(t) = self.tiles.lock().map.get(&(key, view_id)) {
            return respond(TileOutcome::Ready(t.clone()));
        }
        let me = self.clone();
        eng.spawn(class::VISIBLE, epoch, rank, move |eng| {
            let out = match catch_unwind(AssertUnwindSafe(|| me.render(eng, &key, view_id, &view))) {
                Ok(Ok(t)) => TileOutcome::Ready(t),
                Ok(Err(EngineError::Stale)) => TileOutcome::Stale,
                Ok(Err(e)) => TileOutcome::Failed(e.to_string()),
                Err(_) => TileOutcome::Failed(EngineError::Panic.to_string()),
            };
            respond(out)
        });
    }
}

/// `/p/{doc}/{page}/{rot}/{scale}/{tx}/{ty}?v={view}&g={rev}&e={epoch}&r={rank}` -> the tile key, the
/// view id, the epoch and the rank.
pub fn parse_tile_request(uri: &tauri::http::Uri) -> Option<(TileKey, u32, u64, u32)> {
    let mut parts = uri.path().trim_start_matches('/').split('/');
    if parts.next()? != "p" {
        return None;
    }
    let mut n = || parts.next()?.parse::<u32>().ok();
    let mut key = TileKey { doc: n()?, rev: 0, page: n()?, rot: n()? as u8, scale: n()?, tx: n()? as u16, ty: n()? as u16 };
    let (mut view, mut epoch, mut rank) = (0u32, 0u64, 0u32);
    for kv in uri.query().unwrap_or("").split('&') {
        match kv.split_once('=') {
            Some(("v", v)) => view = v.parse().unwrap_or(0),
            Some(("e", v)) => epoch = v.parse().unwrap_or(0),
            Some(("r", v)) => rank = v.parse().unwrap_or(0),
            Some(("g", v)) => key.rev = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    Some((key, view, epoch, rank))
}

/// The inks of a page.
#[tauri::command]
pub async fn prepress_separations(eng: Eng<'_>, doc: DocId, page: u32) -> Result<PageSeps> {
    let eng = eng.inner().clone();
    blocking(move || page_separations(&eng, doc, page)).await
}

/// Registers a view; the id goes in the tile URLs of the `p` route.
#[tauri::command]
pub fn prepress_view(pre: Pre<'_>, eng: Eng<'_>, doc: DocId, view: View) -> Result<u32> {
    eng.doc_entry(doc)?;
    Ok(pre.register(doc, view))
}

/// Ink coverage and TAC of the pages (all when `pages` is absent), `on_progress` once per page.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn prepress_ink(
    eng: Eng<'_>,
    jobs: Jobs<'_>,
    job: u64,
    doc: DocId,
    pages: Option<Vec<u32>>,
    limit: Option<f32>,
    trim_only: Option<bool>,
    on_progress: Channel<Progress>,
) -> Result<Vec<InkReport>> {
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        doc_ink(&eng, doc, pages.as_deref(), limit.unwrap_or(DEFAULT_LIMIT), DEFAULT_MAX_PX, trim_only.unwrap_or(true), &mut progress, &guard.cancel)
    })
    .await
}

/// What the page paints at `(x, y)` (page points from the top-left), topmost first.
#[tauri::command]
pub async fn prepress_inspect(eng: Eng<'_>, doc: DocId, page: u32, x: f32, y: f32) -> Result<Vec<Inspected>> {
    let eng = eng.inner().clone();
    blocking(move || inspect_at(&eng, doc, page, x, y, 8)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Converted {
    pub state: EditState,
    pub report: ConvertReport,
}

/// Converts colours to CMYK as one undo step. `profile` is an open token (from `pick_open`) of a CMYK
/// ICC profile, for `intent: "profile"`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn prepress_convert(
    window: Window,
    eng: Eng<'_>,
    reg: tauri::State<'_, Paths>,
    jobs: Jobs<'_>,
    job: u64,
    doc: DocId,
    spec: ConvertSpec,
    profile: Option<String>,
    on_progress: Channel<Progress>,
) -> Result<Converted> {
    let bytes = match (spec.intent, profile) {
        (IntentSource::Profile, Some(token)) => {
            let path = reg.open(window.label(), &token)?;
            // a profile is a few hundred kilobytes; a file far beyond that is not one
            if std::fs::metadata(&path).map_err(|e| EngineError::Io(e.to_string()))?.len() > 64 << 20 {
                return Err(EngineError::Invalid("The file is too large for an ICC profile".into()));
            }
            Some(std::fs::read(&path).map_err(|e| EngineError::Io(e.to_string()))?)
        }
        _ => None,
    };
    let eng = eng.inner().clone();
    let guard = jobs.inner().begin(job);
    blocking(move || {
        let mut progress = |p: Progress| {
            let _ = on_progress.send(p);
        };
        let (state, report) = convert_colors(&eng, doc, &spec, bytes.as_deref(), EditSource::User, &mut progress, &guard.cancel)?;
        Ok(Converted { state, report })
    })
    .await
}

/// Sets a trim, bleed, art or crop box.
#[tauri::command]
pub async fn prepress_boxes(eng: Eng<'_>, doc: DocId, spec: BoxSpec) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || set_boxes(&eng, doc, &spec, EditSource::User)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Marked {
    pub state: EditState,
    pub notes: Vec<String>,
}

/// Adds printer's marks on an enlarged media box.
#[tauri::command]
pub async fn prepress_marks(eng: Eng<'_>, doc: DocId, spec: MarksSpec) -> Result<Marked> {
    let eng = eng.inner().clone();
    blocking(move || {
        let (state, notes) = add_printer_marks(&eng, doc, &spec, EditSource::User)?;
        Ok(Marked { state, notes })
    })
    .await
}
