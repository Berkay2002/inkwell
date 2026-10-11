//! Search across files: every PDF in a folder (optionally its sub-folders) or a list of files the user picked.
//!
//! Runs on the engine's background workers, two files at a time, so the page tiles of the open document stay
//! first in line. One file is open at a time per worker and dropped as soon as it has been read; what is kept
//! is bounded (a file lists at most `PER_FILE` hits, a search at most `TOTAL` listed hits, and counts go on).
//! Cancelling ends it between two pages. Paths come in as tokens (paths.rs); the events carry the path for
//! display and the command turns it into an open token for the window.

use super::{find_in_page, Hit, Query, SearchOpts};
use crate::engine::Engine;
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
    /// A file with at least one hit. `hits` counts them all, `items` lists the first few.
    #[serde(rename_all = "camelCase")]
    File {
        search: u64,
        index: u32,
        path: String,
        /// An open token for `path`, issued by the command that forwards the event (empty from the worker).
        token: String,
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
    Done { search: u64, files: u32, matched: u32, hits: u32, skipped: u32, cancelled: bool, capped: bool },
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

/// The PDFs under `root` (sorted by path within each folder; sub-folders after the files of their parent),
/// at most `MAX_FILES`; `true` when the cap cut the list. Links to folders are not followed.
pub fn list_pdfs(root: &Path, recursive: bool) -> (Vec<PathBuf>, bool) {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut capped = false;
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let p = e.path();
            if t.is_dir() {
                dirs.push(p);
            } else if t.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
                files.push(p);
            }
        }
        files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
        for f in files {
            if out.len() >= MAX_FILES {
                capped = true;
                break;
            }
            out.push(f);
        }
        if recursive {
            dirs.sort_by_key(|p| std::cmp::Reverse(p.to_string_lossy().to_lowercase()));
            stack.extend(dirs);
        }
        if capped {
            break;
        }
    }
    (out, capped)
}

/// The searches that are running, so one can be cancelled by id.
#[derive(Default)]
pub struct FolderSearches {
    next: AtomicU64,
    live: Mutex<HashMap<u64, Arc<AtomicBool>>>,
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
    listed: AtomicU32,
    skipped: AtomicU32,
    in_flight: AtomicU32,
    capped: bool,
    sink: Arc<dyn Fn(FilesEvent) + Send + Sync>,
    owner: Arc<FolderSearches>,
}

impl FolderSearches {
    /// Starts a search over `files`; events go to `sink` (from worker threads). Returns the search id.
    pub fn start(
        self: &Arc<Self>,
        eng: &Arc<Engine>,
        files: Vec<PathBuf>,
        capped: bool,
        query: &str,
        opts: SearchOpts,
        sink: Arc<dyn Fn(FilesEvent) + Send + Sync>,
    ) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let total = files.len() as u32;
        let Some(q) = Query::new(query, opts).filter(|_| total > 0) else {
            sink(FilesEvent::Done { search: id, files: total, matched: 0, hits: 0, skipped: 0, cancelled: false, capped });
            return id;
        };
        self.live.lock().unwrap().insert(id, cancel.clone());
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
            in_flight: AtomicU32::new(0),
            capped,
            sink,
            owner: self.clone(),
        });
        for k in 0..PARALLEL.min(total as usize) {
            run.in_flight.fetch_add(1, Ordering::SeqCst);
            let r = run.clone();
            eng.spawn_background(k as u32, move |_| worker(r));
        }
        id
    }

    /// Ends search `id` between two pages; its `Done` event says it was cancelled.
    pub fn cancel(&self, id: u64) {
        if let Some(c) = self.live.lock().unwrap().get(&id) {
            c.store(true, Ordering::SeqCst);
        }
    }
}

/// Takes files off the queue until it is empty or the search is cancelled; the last worker out says `Done`.
fn worker(run: Arc<Run>) {
    // Make sure this thread's MuPDF context exists before any document it will drop.
    let _ = mupdf::Context::get();
    loop {
        if run.cancel.load(Ordering::SeqCst) {
            break;
        }
        let Some((index, path)) = run.queue.lock().unwrap().pop_front() else { break };
        let room = TOTAL.saturating_sub(run.listed.load(Ordering::Relaxed) as usize);
        match search_file(&path, &run.q, &run.cancel, room) {
            Ok(r) => {
                if r.hits > 0 {
                    run.matched.fetch_add(1, Ordering::Relaxed);
                    run.hits.fetch_add(r.hits, Ordering::Relaxed);
                    run.listed.fetch_add(r.items.len() as u32, Ordering::Relaxed);
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    (run.sink)(FilesEvent::File {
                        search: run.id,
                        index,
                        path: path.to_string_lossy().into_owned(),
                        token: String::new(),
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
    if run.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
        let cancelled = run.cancel.load(Ordering::SeqCst);
        run.owner.live.lock().unwrap().remove(&run.id);
        (run.sink)(FilesEvent::Done {
            search: run.id,
            files: run.total,
            matched: run.matched.load(Ordering::Relaxed),
            hits: run.hits.load(Ordering::Relaxed),
            skipped: run.skipped.load(Ordering::Relaxed),
            cancelled,
            capped: run.capped,
        });
    }
}
