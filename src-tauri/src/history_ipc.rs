//! IPC for the version history panel and Compare mode. One `Arc<Store>` per process, opened lazily;
//! each open document gets a binding (engine doc id -> history doc + session guard).
//! History errors are logged and shown in the panel; they never fail an open.

use crate::diff::{self, BytesId, CompareOptions, Comparison, VisualDiff};
use crate::engine::edit::{Saved, Turn};
use crate::engine::{DocId as EngDoc, Engine};
use crate::paths::Paths;
use crate::history::{self, Author, HistoryError, Kind, NewVersion, Resolution, SessionGuard, Store, Target, VersionId, VersionInfo};
use lru::LruCache;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::ipc::{Channel, Response};
use tauri::{AppHandle, Emitter, Manager, State, Window};

type Hist<'a> = State<'a, Arc<History>>;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// The open hook is still running.
    Loading,
    Ready,
    /// Another Inkwell window holds the store.
    Unavailable,
    /// "Never keep history for this file".
    Off,
    /// Asks "same document as before?".
    Ambiguous,
    /// History was deleted for this open document.
    None,
    Error,
}

struct Bound {
    path: PathBuf,
    hdoc: Option<history::DocId>,
    /// Dropping it removes the session marker.
    guard: Option<SessionGuard>,
    status: Status,
    message: Option<String>,
    recovery: Option<VersionInfo>,
    candidates: Vec<history::DocId>,
}

impl Bound {
    fn new(path: &Path) -> Self {
        Bound { path: path.to_path_buf(), hdoc: None, guard: None, status: Status::Loading, message: None, recovery: None, candidates: vec![] }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    id: String,
    /// Newest version of the candidate's history (to say which document it was).
    last: Option<VersionInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateOut {
    status: Status,
    message: Option<String>,
    recovery: Option<VersionInfo>,
    candidates: Vec<Candidate>,
}

/// What a command reads PDF bytes from.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Source {
    Version { id: String },
    /// The file of the document on disk now.
    Current,
    /// An open token from `pick_open` (paths.rs); commands turn it into `File` with `resolve` before the work starts.
    Picked { token: String },
    /// What `Picked` resolves to. Never read from the webview.
    #[serde(skip_deserializing)]
    File { path: PathBuf },
}

impl Source {
    /// Turns a token into the file it names for `window`; unknown, spent or other-window tokens are refused.
    pub fn resolve(self, reg: &Paths, window: &str) -> Result<Source, String> {
        match self {
            Source::Picked { token } => Ok(Source::File { path: reg.open(window, &token).map_err(err)?.into() }),
            s => Ok(s),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOut {
    path: String,
    version: VersionInfo,
}

/// How often open documents with new unsaved edits are autosaved.
const AUTOSAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(120);

/// What edit-core's hooks remember per open document.
#[derive(Default)]
struct Marks {
    /// A redaction landed since the open: the next save that includes it purges the older versions.
    redaction_pending: bool,
    /// The assistant turn last recorded as a checkpoint.
    checkpointed: Option<String>,
    /// The engine generation of the last autosave (or of the last look that found nothing unsaved).
    autosaved_rev: u32,
}

/// A finished comparison with the bytes it read: whoever draws on the result must use these very bytes.
pub struct Compared {
    pub cmp: Arc<Comparison>,
    pub a: Arc<(BytesId, Vec<u8>)>,
    pub b: Arc<(BytesId, Vec<u8>)>,
}

pub struct History {
    root: PathBuf,
    store: Mutex<Option<Arc<Store>>>,
    docs: Mutex<HashMap<EngDoc, Bound>>,
    marks: Mutex<HashMap<EngDoc, Marks>>,
    /// The only way a UI-supplied file reaches `load`.
    /// Recently read sources, so paging through a version does not decrypt it once per page.
    bytes: Mutex<LruCache<String, Arc<(BytesId, Vec<u8>)>>>,
    /// MuPDF work of the diff module, one thread at a time.
    diff_lock: Mutex<()>,
    /// The latest comparisons by (bytes of A, bytes of B).
    compared: Mutex<LruCache<(BytesId, BytesId), Arc<Comparison>>>,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl History {
    pub fn new() -> Self {
        let root = std::env::var_os("INKWELL_HISTORY_ROOT").map(PathBuf::from).unwrap_or_else(history::default_root);
        Self::with_root(root)
    }

    pub fn with_root(root: PathBuf) -> Self {
        History {
            root,
            store: Mutex::new(None),
            docs: Mutex::new(HashMap::new()),
            marks: Mutex::new(HashMap::new()),
            bytes: Mutex::new(LruCache::new(NonZeroUsize::new(6).unwrap())),
            diff_lock: Mutex::new(()),
            compared: Mutex::new(LruCache::new(NonZeroUsize::new(4).unwrap())),
        }
    }

    /// Opened on first use; a failure (a second window holds the lock) is retried on the next call.
    fn store(&self) -> Result<Arc<Store>, HistoryError> {
        let mut g = self.store.lock();
        if let Some(s) = g.as_ref() {
            return Ok(s.clone());
        }
        let s = Arc::new(Store::open(&self.root)?);
        *g = Some(s.clone());
        Ok(s)
    }

    /// Called after a document opened: registers it now, resolves it in the background, then tells the UI.
    pub fn spawn_open(self: &Arc<Self>, app: AppHandle, doc: EngDoc, path: String) {
        self.docs.lock().insert(doc, Bound::new(Path::new(&path)));
        let me = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            me.bind(doc, Path::new(&path));
            let _ = app.emit("history-changed", doc);
        });
    }

    /// Resolves the file to a history (attach or known), starts the session. Never fails.
    pub fn bind(&self, doc: EngDoc, path: &Path) {
        let mut b = Bound::new(path);
        match self.store() {
            Ok(store) => {
                if let Err(e) = self.resolve(&store, path, &mut b) {
                    b.set_error(&e);
                }
            }
            Err(e) => b.set_error(&e),
        }
        let mut docs = self.docs.lock();
        // Closed while resolving: the guard drops here and the marker goes with it.
        if docs.contains_key(&doc) {
            docs.insert(doc, b);
        }
    }

    fn resolve(&self, store: &Arc<Store>, path: &Path, b: &mut Bound) -> Result<(), HistoryError> {
        match store.doc_for_file(path)? {
            Resolution::Known { doc } => self.known(store, doc, path, b),
            Resolution::New => {
                let doc = store.attach(path)?;
                b.guard = Some(store.begin_session(doc)?);
                b.hdoc = Some(doc);
                b.status = Status::Ready;
                Ok(())
            }
            Resolution::Ambiguous { candidates } => {
                b.candidates = candidates;
                b.status = Status::Ambiguous;
                Ok(())
            }
            Resolution::Excluded => {
                b.status = Status::Off;
                Ok(())
            }
        }
    }

    fn known(&self, store: &Arc<Store>, doc: history::DocId, path: &Path, b: &mut Bound) -> Result<(), HistoryError> {
        // Order matters: the recovery check must come before check_external and begin_session.
        b.recovery = store.pending_recovery(doc)?;
        store.check_external(doc, path)?;
        b.guard = Some(store.begin_session(doc)?);
        b.hdoc = Some(doc);
        b.status = Status::Ready;
        Ok(())
    }

    /// The document was closed (or is about to be reopened): drops the session.
    pub fn forget(&self, doc: EngDoc) {
        self.docs.lock().remove(&doc);
        self.marks.lock().remove(&doc);
        // Decrypted versions must not outlive the binding that was allowed to read them.
        self.bytes.lock().clear();
    }

    fn bound(&self, doc: EngDoc) -> Result<(Arc<Store>, history::DocId, PathBuf), String> {
        let (hdoc, path) = {
            let docs = self.docs.lock();
            let b = docs.get(&doc).ok_or("this document is not open")?;
            (b.hdoc.ok_or_else(|| b.message.clone().unwrap_or_else(|| "history is not available for this file".into()))?, b.path.clone())
        };
        Ok((self.store().map_err(err)?, hdoc, path))
    }

    pub fn state(&self, doc: EngDoc) -> StateOut {
        let (status, message, recovery, cands) = match self.docs.lock().get(&doc) {
            Some(b) => (b.status, b.message.clone(), b.recovery.clone(), b.candidates.clone()),
            None => (Status::None, None, None, vec![]),
        };
        let store = if cands.is_empty() { None } else { self.store().ok() };
        let candidates = cands
            .into_iter()
            .map(|c| Candidate {
                id: c.to_string(),
                last: store.as_ref().and_then(|s| s.versions(c).ok()).and_then(|v| v.into_iter().next()),
            })
            .collect();
        StateOut { status, message, recovery, candidates }
    }

    pub fn versions(&self, doc: EngDoc) -> Result<Vec<VersionInfo>, String> {
        let (store, hdoc, _) = self.bound(doc)?;
        store.versions(hdoc).map_err(err)
    }

    /// Records the file on disk if it changed outside Inkwell (window focus).
    pub fn check(&self, doc: EngDoc) -> Result<Option<VersionInfo>, String> {
        let (store, hdoc, path) = self.bound(doc)?;
        store.check_external(hdoc, &path).map_err(err)
    }

    /// Commits the restore as a new version on top and returns what the caller must write to the file.
    /// The binding is dropped (the caller reopens the file, which binds it again).
    pub fn prepare_restore(&self, doc: EngDoc, version: &str) -> Result<(PathBuf, Vec<u8>, VersionInfo), String> {
        let (store, hdoc, path) = self.bound(doc)?;
        let v: VersionId = version.parse().map_err(err)?;
        // Whatever is on disk now becomes a version first: a restore never loses a change made outside.
        store.check_external(hdoc, &path).map_err(err)?;
        let info = store.restore(hdoc, v, Author::User).map_err(err)?;
        let bytes = store.read(hdoc, info.id).map_err(err)?;
        self.forget(doc);
        Ok((path, bytes, info))
    }

    pub fn confirm_same(&self, doc: EngDoc, candidate: Option<String>) -> Result<(), String> {
        let path = {
            let docs = self.docs.lock();
            let b = docs.get(&doc).ok_or("this document is not open")?;
            // Only answers a question that was asked; the binding stays as it was when refused.
            if b.status != Status::Ambiguous {
                return Err("this document has no pending question about its history".into());
            }
            let c = match &candidate {
                Some(c) => Some(c.parse::<history::DocId>().map_err(err)?),
                None => None,
            };
            if c.is_some_and(|c| !b.candidates.contains(&c)) {
                return Err("that history was not offered for this document".into());
            }
            b.path.clone()
        };
        let store = self.store().map_err(err)?;
        let mut b = Bound::new(&path);
        let r = match candidate {
            Some(c) => c.parse().map_err(err).and_then(|c: history::DocId| {
                store.confirm_same(c, &path).map_err(err)?;
                self.known(&store, c, &path, &mut b).map_err(err)
            }),
            None => self.resolve_new(&store, &path, &mut b).map_err(err),
        };
        if let Err(e) = &r {
            b.status = Status::Error;
            b.message = Some(e.clone());
        }
        self.docs.lock().insert(doc, b);
        r
    }

    fn resolve_new(&self, store: &Arc<Store>, path: &Path, b: &mut Bound) -> Result<(), HistoryError> {
        let doc = store.attach(path)?;
        b.guard = Some(store.begin_session(doc)?);
        b.hdoc = Some(doc);
        b.status = Status::Ready;
        Ok(())
    }

    pub fn delete(&self, doc: EngDoc) -> Result<(), String> {
        let (store, hdoc, path) = self.bound(doc)?;
        self.forget(doc);
        store.delete_history(hdoc).map_err(err)?;
        let mut b = Bound::new(&path);
        b.status = Status::None;
        self.docs.lock().insert(doc, b);
        Ok(())
    }

    pub fn never_keep(&self, doc: EngDoc, never: bool) -> Result<(), String> {
        let (path, hdoc) = {
            let docs = self.docs.lock();
            let b = docs.get(&doc).ok_or("this document is not open")?;
            (b.path.clone(), b.hdoc)
        };
        let store = self.store().map_err(err)?;
        // Drop the session marker before the directory is shredded.
        self.docs.lock().insert(doc, Bound::new(&path));
        let target = hdoc.map_or_else(|| Target::Path(path.clone()), Target::Doc);
        match store.set_never_keep(target, never) {
            Ok(()) => {}
            Err(HistoryError::Ambiguous(candidates)) => {
                // Let the UI ask which history this file is, then try again.
                if let Some(b) = self.docs.lock().get_mut(&doc) {
                    b.candidates = candidates;
                    b.status = Status::Ambiguous;
                }
                return Err("Choose which history this file belongs to first.".into());
            }
            Err(e) => {
                self.bind(doc, &path);
                return Err(err(e));
            }
        }
        if never {
            self.docs.lock().get_mut(&doc).map(|b| b.status = Status::Off);
        } else {
            self.bind(doc, &path);
        }
        Ok(())
    }

    // ---- edit-core hooks (docs/history.md, "Integration points") ------------------------------

    /// A save over the document's own file: records the bytes written. When they include a redaction
    /// made since the open, `redactions_applied` records them instead and the versions from before
    /// it are destroyed (unless the user chose to keep pre-redaction history).
    pub fn saved(&self, doc: EngDoc, s: &Saved) -> Result<VersionInfo, String> {
        let (store, hdoc, _) = self.bound(doc)?;
        let meta = |label: &str| NewVersion {
            author: Author::User,
            kind: Kind::Save,
            label: label.into(),
            summary: s.summary.clone(),
            name: None,
            pages_touched: s.pages.clone(),
        };
        let pending = self.marks.lock().get(&doc).is_some_and(|m| m.redaction_pending);
        if !(pending && s.redacts) {
            return store.commit(hdoc, &s.bytes, meta("Saved")).map_err(err);
        }
        let keep = store.keeps_pre_redaction(hdoc).map_err(err)?;
        let v = store.redactions_applied(hdoc, keep, &s.bytes, meta("Applied redactions")).map_err(err)?;
        self.marks.lock().entry(doc).or_default().redaction_pending = false;
        // decrypted bytes of purged versions must not stay readable from the cache
        self.bytes.lock().clear();
        Ok(v)
    }

    /// Engine redaction hook: a redaction landed in `doc` (in memory; the purge waits for the save).
    pub fn on_redactions_applied(&self, doc: EngDoc) {
        self.marks.lock().entry(doc).or_default().redaction_pending = true;
    }

    /// An assistant turn ended with changes: one checkpoint per turn, even when the bytes equal the head.
    pub fn checkpoint(&self, doc: EngDoc, t: Turn) -> Result<Option<VersionInfo>, String> {
        if self.marks.lock().get(&doc).is_some_and(|m| m.checkpointed.as_deref() == Some(t.turn.as_str())) {
            return Ok(None);
        }
        let (store, hdoc, _) = self.bound(doc)?;
        let meta = NewVersion {
            author: Author::Assistant { turn_id: t.turn.clone() },
            kind: Kind::Checkpoint,
            label: t.label,
            summary: t.summary,
            name: None,
            pages_touched: t.pages,
        };
        let v = store.commit(hdoc, &t.bytes, meta).map_err(err)?;
        self.marks.lock().entry(doc).or_default().checkpointed = Some(t.turn);
        Ok(Some(v))
    }

    /// Autosaves each bound document whose unsaved content changed since the last look. Returns the
    /// documents that got a version.
    pub fn autosave_all(&self, eng: &Engine) -> Vec<EngDoc> {
        let docs: Vec<EngDoc> = self.docs.lock().iter().filter(|(_, b)| b.hdoc.is_some()).map(|(d, _)| *d).collect();
        let mut out = vec![];
        for doc in docs {
            let Ok(rev) = eng.doc_entry(doc).map(|e| e.generation()) else { continue };
            if self.marks.lock().get(&doc).is_some_and(|m| m.autosaved_rev == rev) {
                continue;
            }
            let r = (|| {
                let Some(bytes) = eng.unsaved_bytes(doc).map_err(err)? else { return Ok(false) };
                let (store, hdoc, _) = self.bound(doc)?;
                store.autosave(hdoc, &bytes).map_err(err)?;
                Ok::<_, String>(true)
            })();
            match r {
                Ok(recorded) => {
                    self.marks.lock().entry(doc).or_default().autosaved_rev = rev;
                    if recorded {
                        out.push(doc);
                    }
                }
                Err(e) => eprintln!("inkwell history: autosave: {e}"),
            }
        }
        out
    }

    /// The autosave timer: every `AUTOSAVE_EVERY`, for the life of the app.
    pub fn spawn_autosave(self: &Arc<Self>, app: AppHandle, eng: Arc<Engine>) {
        let me = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(AUTOSAVE_EVERY);
            for doc in me.autosave_all(&eng) {
                let _ = app.emit("history-changed", doc);
            }
        });
    }

    pub fn save_copy(&self, doc: EngDoc, version: &str, dest: &Path) -> Result<(), String> {
        let (store, hdoc, _) = self.bound(doc)?;
        store.save_copy(hdoc, version.parse().map_err(err)?, dest, true).map_err(err)
    }

    pub fn name(&self, doc: EngDoc, version: &str, name: Option<String>) -> Result<VersionInfo, String> {
        let (store, hdoc, _) = self.bound(doc)?;
        store.name(hdoc, version.parse().map_err(err)?, name).map_err(err)
    }

    /// The bytes behind a source (decrypted history stays in memory only).
    /// Authorization comes first and never depends on the cache: a version needs a binding for `doc`
    /// (looked up from the engine id, failing closed), `Current` the bound document's own file, a file
    /// resolved from a `paths.rs` token. Nothing else names a file.
    fn load(&self, doc: Option<EngDoc>, src: &Source) -> Result<Arc<(BytesId, Vec<u8>)>, String> {
        enum Origin {
            Version(Arc<Store>, history::DocId, VersionId),
            File(PathBuf),
        }
        let file_key = |p: &Path| -> Result<String, String> {
            let m = std::fs::metadata(p).map_err(err)?;
            let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            Ok(format!("f:{}:{t}:{}", p.display(), m.len()))
        };
        let (key, origin) = match src {
            Source::Version { id } => {
                let (store, hdoc, _) = self.bound(doc.ok_or("no document")?)?;
                let v: VersionId = id.parse().map_err(err)?;
                // per history document: two documents never share an entry
                (format!("v:{hdoc}:{v}"), Origin::Version(store, hdoc, v))
            }
            Source::Current => {
                let (_, _, p) = self.bound(doc.ok_or("no document")?)?;
                (file_key(&p)?, Origin::File(p))
            }
            Source::File { path } => (file_key(path)?, Origin::File(path.clone())),
            Source::Picked { .. } => return Err("unresolved file token".into()),
        };
        if let Some(v) = self.bytes.lock().get(&key) {
            return Ok(v.clone());
        }
        let bytes = match origin {
            Origin::Version(store, hdoc, v) => store.read(hdoc, v).map_err(err)?,
            Origin::File(p) => std::fs::read(p).map_err(err)?,
        };
        let v = Arc::new((BytesId::of(&bytes), bytes));
        self.bytes.lock().put(key, v.clone());
        Ok(v)
    }

    pub fn compare(&self, doc: Option<EngDoc>, a: &Source, b: &Source) -> Result<Comparison, String> {
        self.compare_job(doc, a, b, &AtomicBool::new(false), &mut |_| {}).map(|j| (*j.cmp).clone())
    }

    /// The comparison of two sources, from the cache when these exact bytes were compared a moment ago
    /// (the report and the view ask for the same one). `cancel` is polled between pages. The bytes come back with
    /// it: whoever draws on the result (the report) must use the very bytes that were compared.
    pub fn compare_job(&self, doc: Option<EngDoc>, a: &Source, b: &Source, cancel: &AtomicBool, on: &mut dyn FnMut(diff::Progress)) -> Result<Compared, String> {
        let (a, b) = (self.load(doc, a)?, self.load(doc, b)?);
        let key = (a.0, b.0);
        if let Some(c) = self.compared.lock().get(&key) {
            return Ok(Compared { cmp: c.clone(), a, b });
        }
        let _g = self.diff_lock.lock();
        // another job may have finished this very comparison while this one waited for the lock
        if let Some(c) = self.compared.lock().get(&key) {
            return Ok(Compared { cmp: c.clone(), a, b });
        }
        let c = Arc::new(diff::compare_with(&a.1, &b.1, &CompareOptions::default(), cancel, on).map_err(err)?);
        self.compared.lock().put(key, c.clone());
        Ok(Compared { cmp: c, a, b })
    }

    /// The comparison of two sources as a PDF report. Returns its bytes; the caller writes them. The report draws
    /// the pages from the same bytes the comparison read, even if a file changed on disk in between.
    pub fn report(&self, doc: Option<EngDoc>, a: &Source, b: &Source, names: (&str, &str), date: &str, cancel: &AtomicBool, on: &mut dyn FnMut(diff::Progress)) -> Result<Vec<u8>, String> {
        let job = self.compare_job(doc, a, b, cancel, on)?;
        let _g = self.diff_lock.lock();
        let input = diff::report::ReportInput { a: &job.a.1, b: &job.b.1, a_name: names.0, b_name: names.1, date };
        diff::report::build(&input, &job.cmp, cancel, on).map_err(err)
    }

    /// The report, written to `dest`. A job cancelled any time before the file is replaced leaves `dest` as it was.
    pub fn report_to(&self, doc: Option<EngDoc>, a: &Source, b: &Source, names: (&str, &str), date: &str, dest: &Path, cancel: &AtomicBool, on: &mut dyn FnMut(diff::Progress)) -> Result<(), String> {
        let bytes = self.report(doc, a, b, names, date, cancel, on)?;
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(err(diff::DiffError::Cancelled));
        }
        write_replace(dest, &bytes).map_err(|e| format!("could not write {}: {e}", dest.display()))
    }

    pub fn render(&self, doc: Option<EngDoc>, src: &Source, page: u32, scale: f32) -> Result<Vec<u8>, String> {
        let s = self.load(doc, src)?;
        let _g = self.diff_lock.lock();
        diff::render_page(s.0, &s.1, page, scale).map_err(err)?.to_png().map_err(err)
    }

    pub fn visual_diff(&self, doc: Option<EngDoc>, a: &Source, ap: u32, b: &Source, bp: u32, scale: f32, hints: &[diff::Rect4]) -> Result<VisualDiff, String> {
        let (a, b) = (self.load(doc, a)?, self.load(doc, b)?);
        let _g = self.diff_lock.lock();
        diff::visual_diff_hinted(&a.1, ap, &b.1, bp, scale, hints).map_err(err)
    }
}

impl Bound {
    fn set_error(&mut self, e: &HistoryError) {
        eprintln!("inkwell history: {e}");
        self.status = if matches!(e, HistoryError::Locked) { Status::Unavailable } else { Status::Error };
        self.message = Some(e.to_string());
    }
}

/// Writes `bytes` to `path` through a temp file in the same folder (the file itself is the destination:
/// nothing decrypted lands anywhere else).
pub fn write_replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let tmp = dir.join(format!(".{}.restore-{}", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

// ---- commands -----------------------------------------------------------------------------------

async fn off<R: Send + 'static>(f: impl FnOnce() -> Result<R, String> + Send + 'static) -> Result<R, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|_| "history task failed".to_string())?
}

#[tauri::command]
pub fn history_state(h: Hist<'_>, doc: EngDoc) -> StateOut {
    h.state(doc)
}

#[tauri::command]
pub async fn history_versions(h: Hist<'_>, doc: EngDoc) -> Result<Vec<VersionInfo>, String> {
    let h = h.inner().clone();
    off(move || h.versions(doc)).await
}

/// Window focus: records a change made outside Inkwell. Returns the new version, if any.
#[tauri::command]
pub async fn history_check(h: Hist<'_>, doc: EngDoc) -> Result<Option<VersionInfo>, String> {
    let h = h.inner().clone();
    off(move || h.check(doc)).await
}

async fn restore_with(eng: Arc<Engine>, h: Arc<History>, doc: EngDoc, version: String) -> Result<RestoreOut, String> {
    off(move || {
        // Unsaved edits are about to be discarded: keep them as an autosave version first.
        if let Some(bytes) = eng.unsaved_bytes(doc).map_err(err)? {
            let (store, hdoc, _) = h.bound(doc)?;
            store.autosave(hdoc, &bytes).map_err(err)?;
        }
        let (path, bytes, info) = h.prepare_restore(doc, &version)?;
        // Engine rule 5: release every replica of the old file, then rename over it.
        eng.close(doc);
        write_replace(&path, &bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
        Ok(RestoreOut { path: path.to_string_lossy().into_owned(), version: info })
    })
    .await
}

/// Makes `version` the newest version and rewrites the file. The UI then reopens the file.
#[tauri::command]
pub async fn history_restore(h: Hist<'_>, eng: State<'_, Arc<Engine>>, doc: EngDoc, version: String) -> Result<RestoreOut, String> {
    restore_with(eng.inner().clone(), h.inner().clone(), doc, version).await
}

/// "Recover unsaved changes": restores the autosave found at open.
#[tauri::command]
pub async fn history_recover(h: Hist<'_>, eng: State<'_, Arc<Engine>>, doc: EngDoc) -> Result<RestoreOut, String> {
    let v = h.state(doc).recovery.ok_or("nothing to recover")?.id.to_string();
    restore_with(eng.inner().clone(), h.inner().clone(), doc, v).await
}

/// Writes `version` to the file of save token `to` (from `pick_save`); returns its path, now a known one.
#[tauri::command]
pub async fn history_save_copy(window: Window, h: Hist<'_>, reg: State<'_, Paths>, doc: EngDoc, version: String, to: String) -> Result<String, String> {
    let dest = reg.take_save(window.label(), &to).map_err(err)?;
    let h = h.inner().clone();
    off(move || {
        h.save_copy(doc, &version, Path::new(&dest))?;
        window.state::<Paths>().learn(&dest);
        Ok(dest)
    })
    .await
}

/// After an assistant turn: records it as a checkpoint if the document ends with its changes.
#[tauri::command]
pub async fn history_checkpoint(app: AppHandle, h: Hist<'_>, eng: State<'_, Arc<Engine>>, doc: EngDoc) -> Result<Option<VersionInfo>, String> {
    let (h, eng) = (h.inner().clone(), eng.inner().clone());
    off(move || {
        let Some(turn) = eng.assistant_turn(doc).map_err(err)? else { return Ok(None) };
        let v = h.checkpoint(doc, turn)?;
        if v.is_some() {
            let _ = app.emit("history-changed", doc);
        }
        Ok(v)
    })
    .await
}

#[tauri::command]
pub async fn history_name(h: Hist<'_>, doc: EngDoc, version: String, name: Option<String>) -> Result<VersionInfo, String> {
    let h = h.inner().clone();
    off(move || h.name(doc, &version, name)).await
}

#[tauri::command]
pub async fn history_confirm_same(app: AppHandle, h: Hist<'_>, doc: EngDoc, candidate: Option<String>) -> Result<(), String> {
    let h = h.inner().clone();
    let r = off(move || h.confirm_same(doc, candidate)).await;
    let _ = app.emit("history-changed", doc);
    r
}

#[tauri::command]
pub async fn history_delete(h: Hist<'_>, doc: EngDoc) -> Result<(), String> {
    let h = h.inner().clone();
    off(move || h.delete(doc)).await
}

#[tauri::command]
pub async fn history_never_keep(h: Hist<'_>, doc: EngDoc, never: bool) -> Result<(), String> {
    let h = h.inner().clone();
    off(move || h.never_keep(doc, never)).await
}

#[tauri::command]
pub async fn history_compare(window: Window, h: Hist<'_>, reg: State<'_, Paths>, doc: Option<EngDoc>, a: Source, b: Source) -> Result<Comparison, String> {
    let (a, b) = (a.resolve(&reg, window.label())?, b.resolve(&reg, window.label())?);
    let h = h.inner().clone();
    off(move || h.compare(doc, &a, &b)).await
}

/// Compare mode's comparison: as `history_compare`, with progress and a cancel (`tool_cancel(job)`, the id
/// is the caller's). A cancelled run fails with "cancelled".
#[tauri::command]
pub async fn history_compare_run(
    window: Window,
    h: Hist<'_>,
    reg: State<'_, Paths>,
    jobs: State<'_, Arc<crate::tools::Jobs>>,
    job: u64,
    doc: Option<EngDoc>,
    a: Source,
    b: Source,
    on_progress: Channel<diff::Progress>,
) -> Result<Comparison, String> {
    let (a, b) = (a.resolve(&reg, window.label())?, b.resolve(&reg, window.label())?);
    let h = h.inner().clone();
    let guard = jobs.inner().begin(job);
    off(move || {
        h.compare_job(doc, &a, &b, &guard.cancel, &mut |p| {
            let _ = on_progress.send(p);
        })
        .map(|j| (*j.cmp).clone())
    })
    .await
}

/// Writes the comparison report to the file of save token `to` (from `pick_save`); returns its path.
/// `names` and `date` are display text for the report's header.
#[tauri::command]
pub async fn history_compare_report(
    window: Window,
    h: Hist<'_>,
    eng: State<'_, Arc<Engine>>,
    reg: State<'_, Paths>,
    jobs: State<'_, Arc<crate::tools::Jobs>>,
    job: u64,
    doc: Option<EngDoc>,
    a: Source,
    b: Source,
    names: (String, String),
    date: String,
    to: String,
    on_progress: Channel<diff::Progress>,
) -> Result<String, String> {
    let (a, b) = (a.resolve(&reg, window.label())?, b.resolve(&reg, window.label())?);
    let dest = reg.take_save(window.label(), &to).map_err(err)?;
    // The report replaces what is at `dest`: never a document that is open (a later save would corrupt it).
    eng.check_not_open(&dest, None).map_err(err)?;
    let h = h.inner().clone();
    let guard = jobs.inner().begin(job);
    off(move || {
        h.report_to(doc, &a, &b, (&names.0, &names.1), &date, Path::new(&dest), &guard.cancel, &mut |p| {
            let _ = on_progress.send(p);
        })?;
        window.state::<Paths>().learn(&dest);
        Ok(dest)
    })
    .await
}

/// One page as a PNG (`scale` 1 = one pixel per point). Over IPC only: no temp file.
#[tauri::command]
pub async fn history_render(window: Window, h: Hist<'_>, reg: State<'_, Paths>, doc: Option<EngDoc>, source: Source, page: u32, scale: f32) -> Result<Response, String> {
    let source = source.resolve(&reg, window.label())?;
    let h = h.inner().clone();
    off(move || h.render(doc, &source, page, scale.clamp(0.25, 4.0))).await.map(Response::new)
}

/// Changed regions between two pages (boxes in page points); the PNGs come from `history_render`.
#[tauri::command]
pub async fn history_visual_diff(
    window: Window,
    h: Hist<'_>,
    reg: State<'_, Paths>,
    doc: Option<EngDoc>,
    a: Source,
    a_page: u32,
    b: Source,
    b_page: u32,
    scale: f32,
    hints: Vec<diff::Rect4>,
) -> Result<VisualDiff, String> {
    let (a, b) = (a.resolve(&reg, window.label())?, b.resolve(&reg, window.label())?);
    let h = h.inner().clone();
    off(move || h.visual_diff(doc, &a, a_page, &b, b_page, scale.clamp(0.25, 4.0), &hints)).await
}

// The history store needs DPAPI: Windows only.
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::history::Kind;

    fn sample(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("docs").join("study").join("samples").join(name)
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join("inkwell-history-ipc-tests").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Builds the scenario `scripts/history-shots.mjs` photographs, from copies of the study samples:
    /// `INKWELL_SEED_DIR=<dir> cargo test --lib seed_screens -- --ignored`. Writes `<dir>/lease.pdf` (three versions:
    /// import (named), assistant checkpoint, save) and `<dir>/notes.pdf` (a crash with an unsaved autosave)
    /// with the store in `<dir>/store`. Never touches anything outside `<dir>`.
    #[test]
    #[ignore]
    fn seed_screens() {
        let dir = PathBuf::from(std::env::var("INKWELL_SEED_DIR").expect("INKWELL_SEED_DIR"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Arc::new(Store::open(dir.join("store")).unwrap());
        let meta = |author, kind, label: &str, summary: &[&str], pages: &[u32]| history::NewVersion {
            author,
            kind,
            label: label.into(),
            summary: summary.iter().map(|s| s.to_string()).collect(),
            name: None,
            pages_touched: pages.to_vec(),
        };
        let read = |n: &str| std::fs::read(sample(n)).unwrap();

        let lease = dir.join("lease.pdf");
        std::fs::write(&lease, read("edit-source.pdf")).unwrap();
        let doc = store.attach(&lease).unwrap();
        let first = store.versions(doc).unwrap().remove(0);
        store.name(doc, first.id, Some("Signed by tenant".into())).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let asst = Author::Assistant { turn_id: "t1".into() };
        store
            .commit(doc, &read("edit-out-02-addtext.pdf"), meta(asst, Kind::Checkpoint, "Filled tenant details", &["Added text on page 1"], &[0]))
            .unwrap();
        // same content plus a trailing comment, so it is a distinct (but change-free) version
        let mut last = read("edit-out-02-addtext.pdf");
        last.extend_from_slice(b"
%saved
");
        store.commit(doc, &last, meta(Author::User, Kind::Save, "Saved", &[], &[0, 1])).unwrap();
        // the file as it is on disk now: the last version (rewritten in place, so its file id is kept)
        std::fs::write(&lease, &last).unwrap();

        let notes = dir.join("notes.pdf");
        std::fs::write(&notes, read("form-source.pdf")).unwrap();
        let n = store.attach(&notes).unwrap();
        store.commit(n, &read("form-source.pdf"), meta(Author::User, Kind::Save, "Saved", &[], &[0])).unwrap();
        let guard = store.begin_session(n).unwrap();
        store.autosave(n, &read("frm-out-01-autodetect.pdf")).unwrap();
        // the "crash": the session marker stays behind
        std::mem::forget(guard);
        for d in [doc, n] {
            for v in store.versions(d).unwrap() {
                eprintln!("seed {d}: {} {:?} {:?}", v.label, v.kind, v.author);
            }
        }
        let probe = |a: &str, b: &str| {
            let (a, b) = (read(a), read(b));
            let c = diff::compare(&a, &b, &CompareOptions::default()).unwrap();
            eprintln!("compare {} changes, {} pages", c.changes.len(), c.pages.b_pages);
            for ch in &c.changes {
                eprintln!("  {:?} p{:?}/{:?} {}", ch.kind, ch.a_page, ch.b_page, ch.label);
            }
        };
        probe("edit-source.pdf", "edit-out-02-addtext.pdf");
        probe("form-source.pdf", "frm-out-01-autodetect.pdf");
        probe("org-out-01-ops.pdf", "org-out-02-insert.pdf");
        probe("red-in-01.pdf", "red-out-02-apply-sanitize.pdf");
        drop(store);
    }

    fn bound_doc(h: &History, doc: EngDoc, file: &Path) {
        h.docs.lock().insert(doc, Bound::new(file));
        h.bind(doc, file);
        assert_eq!(h.state(doc).status, Status::Ready);
    }

    /// An engine document on a copy of `sample`, bound to a history in `dir/store`.
    fn opened(dir: &Path, sample_name: &str) -> (History, Arc<Engine>, EngDoc, PathBuf) {
        let file = dir.join("doc.pdf");
        std::fs::copy(sample(sample_name), &file).unwrap();
        let eng = Engine::new(1, 1 << 20);
        let doc = match eng.open_pending(&file.to_string_lossy()) {
            (doc, Ok(_)) => doc,
            (_, Err(e)) => panic!("{e:?}"),
        };
        let h = History::with_root(dir.join("store"));
        bound_doc(&h, doc, &file);
        (h, eng, doc, file)
    }

    fn rotate(eng: &Engine, doc: EngDoc, source: crate::engine::edit::EditSource) {
        eng.apply_from(doc, crate::engine::edit::EditOp::Rotate { pages: vec![0], quarter_turns: 1 }, source).unwrap();
    }

    /// Save -> a Save version of the bytes written; a save including a redaction -> `redactions_applied`.
    #[test]
    fn saves_commit_and_a_saved_redaction_purges() {
        let dir = tmp("wire-save");
        let (h, eng, doc, file) = opened(&dir, "edit-source.pdf");
        rotate(&eng, doc, Default::default());
        let (_, s) = eng.save_recorded(doc, None).unwrap();
        let s = s.unwrap();
        assert_eq!(s.bytes, std::fs::read(&file).unwrap());
        let v = h.saved(doc, &s).unwrap();
        assert_eq!((v.kind, v.label.as_str()), (Kind::Save, "Saved"));
        assert_eq!(v.pages_touched, vec![0]);
        assert_eq!(h.versions(doc).unwrap().len(), 2);
        assert!(eng.save_recorded(doc, None).unwrap().1.is_none(), "nothing to save, nothing recorded");

        // a redaction lands (the engine hook), then the save replaces every older version
        eng.apply_custom(doc, "Redact".into(), Default::default(), true, |_| Ok(vec![0])).unwrap();
        h.on_redactions_applied(doc);
        let s = eng.save_recorded(doc, None).unwrap().1.unwrap();
        assert!(s.redacts);
        assert_eq!(h.saved(doc, &s).unwrap().label, "Applied redactions");
        assert_eq!(h.versions(doc).unwrap().len(), 1);
        // later saves are plain commits again
        rotate(&eng, doc, Default::default());
        let s = eng.save_recorded(doc, None).unwrap().1.unwrap();
        assert_eq!(h.saved(doc, &s).unwrap().label, "Saved");
        assert_eq!(h.versions(doc).unwrap().len(), 2);
    }

    /// An assistant turn that changed the document -> one Checkpoint with the turn's label and pages.
    #[test]
    fn assistant_turns_checkpoint_once() {
        let dir = tmp("wire-turn");
        let (h, eng, doc, _) = opened(&dir, "edit-source.pdf");
        let turn = crate::engine::edit::EditSource::Assistant { turn: "t1".into() };
        assert!(eng.assistant_turn(doc).unwrap().is_none());
        rotate(&eng, doc, turn.clone());
        rotate(&eng, doc, turn);
        let t = eng.assistant_turn(doc).unwrap().unwrap();
        assert_eq!((t.turn.as_str(), t.summary.len(), t.pages.clone()), ("t1", 2, vec![0]));
        assert!(t.label.ends_with("and 1 more"), "{}", t.label);
        let v = h.checkpoint(doc, t).unwrap().unwrap();
        assert_eq!(v.kind, Kind::Checkpoint);
        assert!(matches!(&v.author, Author::Assistant { turn_id } if turn_id == "t1"));
        assert!(h.checkpoint(doc, eng.assistant_turn(doc).unwrap().unwrap()).unwrap().is_none(), "once per turn");
        rotate(&eng, doc, Default::default());
        assert!(eng.assistant_turn(doc).unwrap().is_none(), "the user edited after the turn");
        assert_eq!(h.versions(doc).unwrap().len(), 2);
    }

    /// The autosave tick records unsaved content once per change, never a clean document.
    #[test]
    fn autosave_records_new_unsaved_edits_only() {
        let dir = tmp("wire-autosave");
        let (h, eng, doc, _) = opened(&dir, "edit-source.pdf");
        assert!(h.autosave_all(&eng).is_empty(), "clean");
        rotate(&eng, doc, Default::default());
        assert_eq!(h.autosave_all(&eng), vec![doc]);
        assert!(h.autosave_all(&eng).is_empty(), "nothing new since");
        let s = eng.save_recorded(doc, None).unwrap().1.unwrap();
        h.saved(doc, &s).unwrap();
        assert!(h.autosave_all(&eng).is_empty(), "saved");
        rotate(&eng, doc, Default::default());
        assert_eq!(h.autosave_all(&eng), vec![doc]);
        let kinds: Vec<Kind> = h.versions(doc).unwrap().into_iter().map(|v| v.kind).collect();
        // the save of the autosaved bytes turned that autosave into the Save
        assert_eq!(kinds, vec![Kind::Autosave, Kind::Save, Kind::Import]);
    }

    /// Save a copy writes only to a save token's file, once, and the path becomes known.
    #[test]
    fn save_copy_goes_through_a_save_token() {
        let dir = tmp("wire-copy");
        let (h, _eng, doc, _) = opened(&dir, "edit-source.pdf");
        let v = h.versions(doc).unwrap()[0].id.to_string();
        let reg = Paths::default();
        let dest = dir.join("copy.pdf").to_string_lossy().into_owned();
        let t = reg.issue_save("main", dest.clone());
        assert!(reg.take_save("main", &dest).is_err(), "a raw path is not a token");
        let to = reg.take_save("main", &t.token).unwrap();
        h.save_copy(doc, &v, Path::new(&to)).unwrap();
        assert!(std::fs::metadata(&dest).unwrap().len() > 0);
        assert!(reg.take_save("main", &t.token).is_err(), "used once");
    }

    /// Every history command acts only on a document the engine id is bound to; paths come only from picks.
    #[test]
    fn refuses_what_is_not_bound() {
        let dir = tmp("refuse");
        let (f1, f2) = (dir.join("one.pdf"), dir.join("two.pdf"));
        std::fs::copy(sample("edit-source.pdf"), &f1).unwrap();
        std::fs::copy(sample("form-source.pdf"), &f2).unwrap();
        let h = History::with_root(dir.join("store"));
        bound_doc(&h, 1, &f1);
        bound_doc(&h, 2, &f2);
        let v1 = h.versions(1).unwrap()[0].id.to_string();
        let v1s = Source::Version { id: v1.clone() };

        // never-opened engine doc: everything is refused
        assert!(h.versions(99).is_err());
        assert!(h.check(99).is_err());
        assert!(h.name(99, &v1, None).is_err());
        assert!(h.delete(99).is_err());
        assert!(h.prepare_restore(99, &v1).is_err());
        assert!(h.compare(Some(99), &v1s, &Source::Current).is_err());
        assert!(h.render(Some(99), &Source::Current, 0, 0.5).is_err());
        assert!(h.visual_diff(Some(99), &v1s, 0, &Source::Current, 0, 1.0, &[]).is_err());
        // a version needs a document; so does the current file
        assert!(h.render(None, &v1s, 0, 0.5).is_err());
        assert!(h.render(None, &Source::Current, 0, 0.5).is_err());
        // another document's version id is not readable through this document
        assert!(h.render(Some(2), &v1s, 0, 0.5).is_err());
        assert!(h.render(Some(1), &v1s, 0, 0.5).is_ok());
        // fail closed: a binding without a history (error / off) reads nothing, cached or not
        h.docs.lock().insert(3, Bound::new(&f1));
        assert!(h.render(Some(3), &v1s, 0, 0.5).is_err());
        assert!(h.render(Some(3), &Source::Current, 0, 0.5).is_err());
        // a closed document loses access even though its bytes were cached a moment ago
        h.forget(1);
        assert!(h.render(Some(1), &v1s, 0, 0.5).is_err());
        assert!(h.versions(1).is_err());
    }

    /// The UI cannot name a path: only open tokens issued to this window resolve, and an unresolved one reads nothing.
    #[test]
    fn only_token_files_are_read() {
        let dir = tmp("picks");
        let (f1, f2) = (dir.join("one.pdf"), dir.join("two.pdf"));
        std::fs::copy(sample("edit-source.pdf"), &f1).unwrap();
        std::fs::copy(sample("edit-out-02-addtext.pdf"), &f2).unwrap();
        let h = History::with_root(dir.join("store"));
        let reg = Paths::default();
        let (t1, t2) = (reg.issue_open("main", f1.to_string_lossy().into_owned()), reg.issue_open("main", f2.to_string_lossy().into_owned()));
        for bad in ["", "../one.pdf", f1.to_str().unwrap(), "pick-999", t1.token.as_str()] {
            let s = Source::Picked { token: bad.into() };
            // unresolved sources read nothing; the token only works for its own window
            assert!(h.render(None, &s, 0, 0.5).is_err(), "{bad}");
            if bad != t1.token {
                assert!(s.clone().resolve(&reg, "main").is_err(), "{bad}");
            }
        }
        assert!(Source::Picked { token: t1.token.clone() }.resolve(&reg, "other").is_err());
        // the old wire form with a raw path (and the internal File form) do not parse
        assert!(serde_json::from_str::<Source>(r#"{"kind":"file","path":"C:\x.pdf"}"#).is_err());
        let (a, b) = (Source::Picked { token: t1.token }.resolve(&reg, "main").unwrap(), Source::Picked { token: t2.token }.resolve(&reg, "main").unwrap());
        assert!(!h.compare(None, &a, &b).unwrap().identical);
        assert!(!h.render(None, &a, 0, 0.5).unwrap().is_empty());
    }

    /// "Same document?" accepts only a history the document was offered.
    #[test]
    fn confirm_same_refuses_foreign_histories() {
        let dir = tmp("confirm");
        let (f1, f2) = (dir.join("one.pdf"), dir.join("two.pdf"));
        std::fs::copy(sample("edit-source.pdf"), &f1).unwrap();
        std::fs::copy(sample("form-source.pdf"), &f2).unwrap();
        let h = History::with_root(dir.join("store"));
        bound_doc(&h, 1, &f1);
        let foreign = h.docs.lock().get(&1).unwrap().hdoc.unwrap().to_string();
        bound_doc(&h, 2, &f2);
        // not offered (the document is Ready, no candidates)
        assert!(h.confirm_same(2, Some(foreign)).is_err());
        assert!(h.confirm_same(2, Some("nonsense".into())).is_err());
        assert!(h.confirm_same(77, None).is_err());
        // the document is still bound to its own history
        assert_eq!(h.versions(2).unwrap().len(), 1);
    }

    /// Open hook: New -> attach (Import), reopen -> Known (no new version), outside change -> External,
    /// restore writes the old bytes back, a second store on the same root is Unavailable.
    #[test]
    fn open_hook_and_restore() {
        let dir = tmp("hook");
        let file = dir.join("doc.pdf");
        std::fs::copy(sample("edit-source.pdf"), &file).unwrap();
        let first = std::fs::read(&file).unwrap();

        let h = History::with_root(dir.join("store"));
        h.docs.lock().insert(1, Bound::new(&file));
        h.bind(1, &file);
        assert_eq!(h.state(1).status, Status::Ready);
        let v = h.versions(1).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, Kind::Import);

        // a second window on the same root: history unavailable, not a crash
        let h2 = History::with_root(dir.join("store"));
        h2.docs.lock().insert(7, Bound::new(&file));
        h2.bind(7, &file);
        assert_eq!(h2.state(7).status, Status::Unavailable);
        assert!(h2.state(7).message.is_some());

        // close + reopen: the same history, nothing new recorded
        h.forget(1);
        h.docs.lock().insert(2, Bound::new(&file));
        h.bind(2, &file);
        assert_eq!(h.state(2).status, Status::Ready);
        assert_eq!(h.versions(2).unwrap().len(), 1);

        // changed outside Inkwell while closed: recorded on the next open
        h.forget(2);
        let other = std::fs::read(sample("edit-out-02-addtext.pdf")).unwrap();
        std::fs::write(&file, &other).unwrap();
        h.docs.lock().insert(3, Bound::new(&file));
        h.bind(3, &file);
        let v = h.versions(3).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].kind, Kind::External);

        // compare the two versions: a change is found
        let a = Source::Version { id: v[1].id.to_string() };
        let b = Source::Current;
        let c = h.compare(Some(3), &a, &b).unwrap();
        assert!(!c.identical);
        assert!(!h.render(Some(3), &a, 0, 0.5).unwrap().is_empty());

        // restore: the file holds the first bytes again, history has a Restore on top
        let (path, bytes, info) = h.prepare_restore(3, &v[1].id.to_string()).unwrap();
        write_replace(&path, &bytes).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), first);
        assert!(matches!(info.kind, Kind::Restore { .. }));
        assert_eq!(h.state(3).status, Status::None); // binding dropped until the UI reopens the file
        h.docs.lock().insert(4, Bound::new(&file));
        h.bind(4, &file);
        assert_eq!(h.state(4).status, Status::Ready);
        assert_eq!(h.versions(4).unwrap().len(), 3);

        // never keep: off; turning it on again attaches a fresh history
        h.never_keep(4, true).unwrap();
        assert_eq!(h.state(4).status, Status::Off);
        h.never_keep(4, false).unwrap();
        assert_eq!(h.state(4).status, Status::Ready);
    }

    /// The comparison report: made from a version and the file, written through a save token, reopens as a PDF with
    /// the counts in it; a cancelled run writes nothing; the second comparison of the same bytes is the cached one.
    #[test]
    fn compare_report_round_trip() {
        let dir = tmp("wire-report");
        let (h, eng, doc, _) = opened(&dir, "edit-source.pdf");
        rotate(&eng, doc, Default::default());
        let s = eng.save_recorded(doc, None).unwrap().1.unwrap();
        h.saved(doc, &s).unwrap();
        let first = h.versions(doc).unwrap().into_iter().last().unwrap().id.to_string();
        let (a, b) = (Source::Version { id: first }, Source::Current);
        let no = AtomicBool::new(false);

        let c1 = h.compare_job(Some(doc), &a, &b, &no, &mut |_| {}).unwrap();
        let c2 = h.compare_job(Some(doc), &a, &b, &no, &mut |_| {}).unwrap();
        assert!(Arc::ptr_eq(&c1.cmp, &c2.cmp), "same bytes, same comparison");
        assert!(Arc::ptr_eq(&c1.a, &c2.a) && Arc::ptr_eq(&c1.b, &c2.b), "and the same bytes come back with it");
        assert!(!c1.cmp.identical);

        let reg = Paths::default();
        let dest = dir.join("report.pdf").to_string_lossy().into_owned();
        let t = reg.issue_save("main", dest.clone());
        let to = reg.take_save("main", &t.token).unwrap();
        let mut stages = Vec::new();
        let bytes = h.report(Some(doc), &a, &b, ("before.pdf", "after.pdf"), "11 Oct 2026", &no, &mut |p| stages.push(p.stage)).unwrap();
        write_replace(Path::new(&to), &bytes).unwrap();
        let again = mupdf::pdf::PdfDocument::open(&dest).unwrap();
        assert!(again.page_count().unwrap() >= 1);
        let text = again.load_page(0).unwrap().to_text_page(mupdf::TextPageFlags::empty()).unwrap().to_text().unwrap();
        assert!(text.contains("Comparison report") && text.contains("before.pdf") && text.contains("1 difference"), "{text}");

        // cancelled: nothing is written
        let flag = AtomicBool::new(true);
        let h2 = History::with_root(dir.join("store2"));
        let dest2 = dir.join("cancelled.pdf");
        let (fa, fb) = (dir.join("a.pdf"), dir.join("b.pdf"));
        std::fs::copy(sample("edit-source.pdf"), &fa).unwrap();
        std::fs::copy(sample("edit-out-02-addtext.pdf"), &fb).unwrap();
        let r = h2.report_to(None, &Source::File { path: fa.clone() }, &Source::File { path: fb.clone() }, ("a", "b"), "d", &dest2, &flag, &mut |_| {});
        assert!(r.unwrap_err().contains("cancelled"));
        assert!(!dest2.exists());

        // cancelled while the report was being made (here: at its very last progress report, right before the file
        // is serialised and written): nothing is written either, and an old file at the destination is left alone
        let old = dir.join("existing.pdf");
        std::fs::write(&old, b"old contents").unwrap();
        let late = AtomicBool::new(false);
        // (the comparison itself is cached by now, so only the report's own progress reaches the callback)
        h2.compare_job(None, &Source::File { path: fa.clone() }, &Source::File { path: fb.clone() }, &no, &mut |_| {}).unwrap();
        let r = h2.report_to(None, &Source::File { path: fa }, &Source::File { path: fb }, ("a", "b"), "d", &old, &late, &mut |p| {
            if p.stage == diff::Stage::Pages && p.done == p.total {
                late.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        assert!(r.unwrap_err().contains("cancelled"));
        assert_eq!(std::fs::read(&old).unwrap(), b"old contents");
    }

    #[test]
    fn a_cancel_of_a_finished_job_is_dropped_and_early_ones_are_bounded() {
        use std::sync::atomic::Ordering::Relaxed;
        let jobs = Arc::new(crate::tools::Jobs::default());
        // a cancel that beat its job is held for it; one that comes after the job finished is not
        jobs.cancel(41);
        let g = jobs.begin(41);
        assert!(g.cancel.load(Relaxed));
        drop(g);
        jobs.cancel(41);
        assert!(!jobs.begin(41).cancel.load(Relaxed), "a finished job's cancel must not reach a later job with the same id");
        // ids nobody ever began cannot pile up
        for id in 1000..1500 {
            jobs.cancel(id);
        }
        assert!(!jobs.begin(1000).cancel.load(Relaxed), "only the last few early cancels are kept");
        assert!(jobs.begin(1499).cancel.load(Relaxed));
    }
}
