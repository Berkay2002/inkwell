//! Search across files: every PDF in a folder (optionally its sub-folders) or a list of files the user picked.
//!
//! Runs on its own two threads per search (below normal priority on Windows), outside the engine's pool: the
//! pool's background lane is one job wide and shared with in-document search, the page-size scan and the AI
//! reads, which a search over hundreds of files would otherwise starve. One file is open at a time per thread
//! and dropped as soon as it has been read; what is kept is bounded (a file lists at most `PER_FILE` hits, a
//! search at most `TOTAL` listed hits, and counts go on). Cancelling ends it between two pages, and also the
//! walk over a folder. Files that are only in the cloud (OneDrive placeholders) are not read, so a search never
//! starts a download; they are counted. Paths come in as tokens (paths.rs); the events carry the path for
//! display, and a result's open token is issued when the result is opened (`FolderSearches::path_of`).

use super::{find_in_page, Hit, Query, SearchOpts};
use mupdf::Document;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Hits listed per file.
pub const PER_FILE: usize = 100;
/// Hits listed in one search; after this only counts are kept.
pub const TOTAL: usize = 5000;
/// Files looked at in one search.
pub const MAX_FILES: usize = 20_000;
/// Directory entries looked at while listing a folder (files, sub-folders, whatever else); after this the
/// listing stops and says it was cut.
pub const MAX_VISITED: usize = 200_000;
/// Files read at the same time.
const PARALLEL: usize = 2;

/// One hit and its page (0-based).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PageHit {
    pub page: u32,
    #[serde(flatten)]
    pub hit: Hit,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FilesEvent {
    /// The search exists (before the folder is listed): this id cancels it.
    #[serde(rename_all = "camelCase")]
    Started { search: u64 },
    /// A file with at least one hit. `hits` counts them all, `items` lists the first few.
    #[serde(rename_all = "camelCase")]
    File {
        search: u64,
        index: u32,
        path: String,
        name: String,
        page_count: u32,
        hits: u32,
        pages: u32,
        truncated: bool,
        items: Vec<PageHit>,
    },
    /// After each file: how far the search is.
    #[serde(rename_all = "camelCase")]
    Progress { search: u64, scanned: u32, total: u32 },
    #[serde(rename_all = "camelCase")]
    Done {
        search: u64,
        files: u32,
        matched: u32,
        hits: u32,
        skipped: u32,
        /// Files left unread because they are only in the cloud.
        cloud_only: u32,
        cancelled: bool,
        capped: bool,
    },
}

/// What one file gave.
#[derive(Debug, Default)]
pub struct FileResult {
    pub page_count: u32,
    pub hits: u32,
    pub pages: u32,
    pub truncated: bool,
    pub items: Vec<PageHit>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Skip {
    /// Needs a password (or is damaged): nothing is read from it.
    Unreadable,
    Cancelled,
}

/// Searches one file; `cancel` is looked at between pages. `list` is how many hits may still be listed.
pub fn search_file(path: &Path, q: &Query, cancel: &AtomicBool, list: usize) -> Result<FileResult, Skip> {
    let doc = Document::open(path.to_string_lossy().as_ref()).map_err(|_| Skip::Unreadable)?;
    if doc.needs_password().unwrap_or(true) {
        return Err(Skip::Unreadable);
    }
    let n = doc.page_count().map_err(|_| Skip::Unreadable)?.max(0) as u32;
    let mut r = FileResult { page_count: n, ..Default::default() };
    let cap = PER_FILE.min(list);
    for p in 0..n {
        if cancel.load(Ordering::Relaxed) {
            return Err(Skip::Cancelled);
        }
        let Ok(hits) = find_in_page(&doc, p, q) else { continue };
        if hits.is_empty() {
            continue;
        }
        r.pages += 1;
        r.hits += hits.len() as u32;
        for h in hits {
            if r.items.len() < cap {
                r.items.push(PageHit { page: p, hit: h });
            } else {
                r.truncated = true;
            }
        }
    }
    if q.opts().bookmarks {
        for (p, h) in super::bookmark_hits(&doc, q).unwrap_or_default() {
            r.hits += 1;
            if r.items.len() < cap {
                r.items.push(PageHit { page: p, hit: h });
            } else {
                r.truncated = true;
            }
        }
        r.items.sort_by_key(|i| i.page);
    }
    Ok(r)
}

/// What a walk over a folder found.
#[derive(Debug, Default)]
pub struct Listing {
    pub files: Vec<PathBuf>,
    /// The walk was cut short (`MAX_FILES` files or `MAX_VISITED` entries).
    pub capped: bool,
    /// PDFs that are only in the cloud and were left out.
    pub cloud_only: u32,
    pub cancelled: bool,
}

/// `true` for a file that is only a placeholder (OneDrive and the like): reading it would download it.
pub fn is_cloud_only(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE
        m.file_attributes() & (0x0040_0000 | 0x0004_0000 | 0x0000_1000) != 0
    }
    #[cfg(not(windows))]
    {
        let _ = m;
        false
    }
}

/// The PDFs under `root` (sorted by path within each folder; sub-folders after the files of their parent), at
/// most `MAX_FILES`, looking at most `MAX_VISITED` entries. Links to folders are not followed; cloud-only files
/// are not read and cloud-only folders are not entered. `cancel` is looked at between folders and every few
/// hundred entries.
pub fn list_pdfs(root: &Path, recursive: bool, cancel: &AtomicBool) -> Listing {
    let mut out = Listing::default();
    let mut visited = 0usize;
    let mut stack = vec![root.to_path_buf()];
    'walk: while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            visited += 1;
            if visited > MAX_VISITED {
                out.capped = true;
                break 'walk;
            }
            if visited % 256 == 0 && cancel.load(Ordering::Relaxed) {
                out.cancelled = true;
                break 'walk;
            }
            let Ok(t) = e.file_type() else { continue };
            let p = e.path();
            if t.is_dir() {
                // Listing a cloud-only folder would fetch it.
                if e.metadata().is_ok_and(|m| is_cloud_only(&m)) {
                    continue;
                }
                dirs.push(p);
            } else if t.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
                if e.metadata().is_ok_and(|m| is_cloud_only(&m)) {
                    out.cloud_only += 1;
                } else {
                    files.push(p);
                }
            }
        }
        files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
        for f in files {
            if out.files.len() >= MAX_FILES {
                out.capped = true;
                break 'walk;
            }
            out.files.push(f);
        }
        if recursive {
            dirs.sort_by_key(|p| std::cmp::Reverse(p.to_string_lossy().to_lowercase()));
            stack.extend(dirs);
        }
    }
    out
}

/// Picked files, in order, without those that are only in the cloud; at most `MAX_FILES`.
pub fn list_picked(paths: Vec<PathBuf>) -> Listing {
    let mut out = Listing::default();
    for p in paths {
        if std::fs::metadata(&p).is_ok_and(|m| is_cloud_only(&m)) {
            out.cloud_only += 1;
        } else if out.files.len() >= MAX_FILES {
            out.capped = true;
        } else {
            out.files.push(p);
        }
    }
    out
}

/// The searches that are running, so one can be cancelled by id, and the files with hits of each window's latest
/// one, so a result can be opened (its open token is issued then, not for every file in the list). A window has
/// one search at a time, and can only reach its own.
#[derive(Default)]
pub struct FolderSearches {
    next: AtomicU64,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Searches that have not ended: the window they belong to and their cancel flag.
    live: HashMap<u64, (String, Arc<AtomicBool>)>,
    /// Per window: its latest search and the files of it that had hits (index to path).
    found: HashMap<String, (u64, HashMap<u32, PathBuf>)>,
}

struct Run {
    id: u64,
    q: Query,
    cancel: Arc<AtomicBool>,
    queue: Mutex<VecDeque<(u32, PathBuf)>>,
    total: u32,
    scanned: AtomicU32,
    matched: AtomicU32,
    hits: AtomicU32,
    /// Hits listed so far, across files (reserved before a file's items are sent).
    listed: AtomicU32,
    skipped: AtomicU32,
    cloud_only: u32,
    in_flight: AtomicU32,
    capped: bool,
    sink: Arc<dyn Fn(FilesEvent) + Send + Sync>,
    owner: Arc<FolderSearches>,
    window: String,
}

impl FolderSearches {
    /// A new search id for `window`, cancellable from now on (before the files are listed). The window's previous
    /// search is cancelled and its files forgotten.
    pub fn begin(&self, window: &str) -> (u64, Arc<AtomicBool>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let mut s = self.state.lock().unwrap();
        let previous = s.found.get(window).map(|f| f.0);
        if let Some((_, c)) = previous.and_then(|p| s.live.get(&p)) {
            c.store(true, Ordering::SeqCst);
        }
        s.live.insert(id, (window.to_owned(), cancel.clone()));
        s.found.insert(window.to_owned(), (id, HashMap::new()));
        (id, cancel)
    }

    /// `begin` and `run` in one step, for a list of files that is already known.
    pub fn start(
        self: &Arc<Self>,
        window: &str,
        files: Vec<PathBuf>,
        capped: bool,
        query: &str,
        opts: SearchOpts,
        sink: Arc<dyn Fn(FilesEvent) + Send + Sync>,
    ) -> u64 {
        let (id, cancel) = self.begin(window);
        self.run(id, cancel, files, capped, 0, query, opts, sink);
        id
    }

    /// Runs search `id` (from `begin`) over `files`; events go to `sink` (from the search's threads).
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        self: &Arc<Self>,
        id: u64,
        cancel: Arc<AtomicBool>,
        files: Vec<PathBuf>,
        capped: bool,
        cloud_only: u32,
        query: &str,
        opts: SearchOpts,
        sink: Arc<dyn Fn(FilesEvent) + Send + Sync>,
    ) {
        let total = files.len() as u32;
        let cancelled = cancel.load(Ordering::SeqCst);
        let Some(q) = Query::new(query, opts).filter(|_| total > 0 && !cancelled) else {
            self.state.lock().unwrap().live.remove(&id);
            sink(FilesEvent::Done { search: id, files: total, matched: 0, hits: 0, skipped: 0, cloud_only, cancelled, capped });
            return;
        };
        let threads = PARALLEL.min(total as usize);
        let window = self.state.lock().unwrap().live.get(&id).map(|l| l.0.clone()).unwrap_or_default();
        let run = Arc::new(Run {
            id,
            q,
            cancel,
            queue: Mutex::new(files.into_iter().enumerate().map(|(i, p)| (i as u32, p)).collect()),
            total,
            scanned: AtomicU32::new(0),
            matched: AtomicU32::new(0),
            hits: AtomicU32::new(0),
            listed: AtomicU32::new(0),
            skipped: AtomicU32::new(0),
            cloud_only,
            in_flight: AtomicU32::new(threads as u32),
            capped,
            sink,
            owner: self.clone(),
            window,
        });
        for k in 0..threads {
            let r = run.clone();
            let spawned = std::thread::Builder::new().name(format!("inkwell-search-{k}")).spawn(move || worker(r));
            if spawned.is_err() {
                // No thread for this one: it leaves at once (the last to leave says Done).
                leave(&run);
            }
        }
    }

    /// Ends search `id` of `window` between two pages (or in the middle of listing a folder); its `Done` event
    /// says it was cancelled. Another window's search is not touched.
    pub fn cancel(&self, window: &str, id: u64) {
        if let Some((w, c)) = self.state.lock().unwrap().live.get(&id) {
            if w == window {
                c.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Notes that file `index` of search `id` had hits. Ignored unless `id` is still `window`'s latest search: a
    /// cancelled search finishing its last file must not write into the next one's list.
    pub fn record(&self, window: &str, id: u64, index: u32, path: PathBuf) {
        if let Some((latest, files)) = self.state.lock().unwrap().found.get_mut(window) {
            if *latest == id {
                files.insert(index, path);
            }
        }
    }

    /// The path of file `index` of search `id`, if that search is `window`'s latest and listed the file.
    pub fn path_of(&self, window: &str, id: u64, index: u32) -> Option<PathBuf> {
        let s = self.state.lock().unwrap();
        match s.found.get(window) {
            Some((latest, files)) if *latest == id => files.get(&index).cloned(),
            _ => None,
        }
    }
}

/// Lowers this thread's priority a little, so tile rendering and typing win against a long search.
fn be_polite() {
    #[cfg(windows)]
    // SAFETY: plain calls on the current thread's pseudo handle.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// One thread of the search is done (its work ran out, it was cancelled, or it panicked); the last says `Done`.
fn leave(run: &Arc<Run>) {
    if run.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
        let cancelled = run.cancel.load(Ordering::SeqCst);
        run.owner.state.lock().unwrap().live.remove(&run.id);
        (run.sink)(FilesEvent::Done {
            search: run.id,
            files: run.total,
            matched: run.matched.load(Ordering::Relaxed),
            hits: run.hits.load(Ordering::Relaxed),
            skipped: run.skipped.load(Ordering::Relaxed),
            cloud_only: run.cloud_only,
            cancelled,
            capped: run.capped,
        });
    }
}

/// Calls `leave` however the thread ends, so a panic cannot leave a search without its `Done`.
struct Leave(Arc<Run>);

impl Drop for Leave {
    fn drop(&mut self) {
        leave(&self.0);
    }
}

/// Takes files off the queue until it is empty or the search is cancelled.
fn worker(run: Arc<Run>) {
    let _leave = Leave(run.clone());
    be_polite();
    // Make sure this thread's MuPDF context exists before any document it will drop.
    let _ = mupdf::Context::get();
    loop {
        if run.cancel.load(Ordering::SeqCst) {
            break;
        }
        let Some((index, path)) = run.queue.lock().unwrap().pop_front() else { break };
        let room = TOTAL.saturating_sub(run.listed.load(Ordering::Relaxed) as usize);
        // A file that makes MuPDF panic is skipped like a damaged one.
        let one = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| search_file(&path, &run.q, &run.cancel, room)))
            .unwrap_or(Err(Skip::Unreadable));
        match one {
            Ok(mut r) => {
                if r.hits > 0 {
                    // Reserve the listing room atomically: two threads must not both fill the last of it.
                    let want = r.items.len() as u32;
                    let total = TOTAL as u32;
                    let before = run
                        .listed
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |c| Some(c + want.min(total.saturating_sub(c))))
                        .unwrap_or(0);
                    let kept = want.min(total.saturating_sub(before)) as usize;
                    if kept < r.items.len() {
                        r.items.truncate(kept);
                        r.truncated = true;
                    }
                    run.matched.fetch_add(1, Ordering::Relaxed);
                    run.hits.fetch_add(r.hits, Ordering::Relaxed);
                    run.owner.record(&run.window, run.id, index, path.clone());
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    (run.sink)(FilesEvent::File {
                        search: run.id,
                        index,
                        path: path.to_string_lossy().into_owned(),
                        name,
                        page_count: r.page_count,
                        hits: r.hits,
                        pages: r.pages,
                        truncated: r.truncated || r.items.len() < r.hits as usize,
                        items: r.items,
                    });
                }
            }
            Err(Skip::Unreadable) => {
                run.skipped.fetch_add(1, Ordering::Relaxed);
            }
            Err(Skip::Cancelled) => break,
        }
        let scanned = run.scanned.fetch_add(1, Ordering::SeqCst) + 1;
        (run.sink)(FilesEvent::Progress { search: run.id, scanned, total: run.total });
    }
}
