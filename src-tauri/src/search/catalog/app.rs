//! Tauri commands for catalogs. The webview names a catalog by its id and a folder by an open token
//! (`pick_search_folder`); it never sends or receives a path to read or write.

use super::embed::{self, Embedded};
use super::{Estimate, Meta, Stale, Summary};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::paths::Paths;
use crate::search::files::{FilesEvent, FolderSearches};
use crate::search::SearchOpts;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::ipc::Channel;
use tauri::{Manager, State, Window};

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = State<'a, Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// Builds that are running (cancellable by job id, for the window that started them) and the catalogs being
/// built, so one catalog is never built twice at once.
#[derive(Default)]
pub struct Jobs {
    next: AtomicU64,
    live: Mutex<HashMap<u64, (String, Arc<AtomicBool>)>>,
    busy: Mutex<HashSet<String>>,
}

impl Jobs {
    /// Runs `f` with catalog `id` held, so a build and a delete (or two builds) of one catalog never overlap;
    /// refused when it is held. Released when `f` returns, whatever it returns.
    pub fn guarded<R>(&self, id: &str, f: impl FnOnce() -> Result<R>) -> Result<R> {
        let _held = self.hold(id).ok_or_else(|| EngineError::Invalid("this catalog is being built or removed; wait for that first".into()))?;
        f()
    }

    fn hold(&self, id: &str) -> Option<Hold<'_>> {
        self.busy.lock().unwrap().insert(id.to_string()).then(|| Hold(self, id.to_string()))
    }
}

/// A catalog held by [`Jobs::hold`], released when dropped.
struct Hold<'a>(&'a Jobs, String);

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        self.0.busy.lock().unwrap().remove(&self.1);
    }
}

/// Every catalog under `root` forgets the file at `path` (see [`super::forget`]). One that is being built or
/// removed does it when it is free (`forget_deferred`); until then a search hides the file (it checks it against
/// the disk). Returns the rows dropped now.
pub fn forget_all(root: &std::path::Path, jobs: &Jobs, path: &std::path::Path) -> u32 {
    let mut n = 0;
    for m in super::list(root) {
        let Some(_held) = jobs.hold(&m.id) else {
            // Written into the catalog's folder: a quit before it is free does not lose it (`replay_pending`).
            // When the list cannot be kept it marks the catalog for a wipe, done by whoever frees it (never here:
            // the holder would write the old words back).
            if let Err(e) = super::defer_forget(&root.join(&m.id), path) {
                eprintln!("inkwell catalog {}: could not keep the file to forget: {e}", m.id);
            }
            // The holder may have finished between the failed hold and the write above: look again now.
            forget_deferred(jobs, &m.id, &root.join(&m.id));
            continue;
        };
        match super::forget(&root.join(&m.id), path) {
            Ok(k) => n += k,
            Err(e) => {
                // Tried again at the next hold (the line below already does once).
                eprintln!("inkwell catalog {}: {e}", m.id);
                let _ = super::defer_forget(&root.join(&m.id), path);
            }
        }
        drop(_held);
        // What was saved while it was held (a build, another forget) is done now, not at some later build.
        forget_deferred(jobs, &m.id, &root.join(&m.id));
    }
    n
}

/// Catalog `id` (in `dir`) is free again: the files saved while it was busy are forgotten now.
pub(super) fn forget_deferred(jobs: &Jobs, id: &str, dir: &std::path::Path) {
    // What failed this call: not tried again here (the next hold does), or the loop below would never end.
    let mut failed: Vec<PathBuf> = Vec::new();
    loop {
        {
            // Held first: when it is not free the list is left as it is, for whoever frees it.
            let Some(_held) = jobs.hold(id) else { return };
            match super::pending(dir) {
                // A forget that could not be kept (list unreadable, unwritable or past its bound): nothing says
                // which file, so nothing stays. Done here, under the hold, after whatever else held the catalog
                // is finished.
                super::Waiting::Wipe => {
                    if let Err(e) = super::wipe_pending(dir) {
                        eprintln!("inkwell catalog {id}: {e}");
                        return;
                    }
                }
                super::Waiting::Paths(p) if p.is_empty() => return,
                super::Waiting::Paths(p) => {
                    // Only what was forgotten comes off the list: a failure is tried again at the next hold.
                    let mut done = Vec::new();
                    for path in p {
                        match super::forget(dir, &path) {
                            Ok(_) => done.push(path),
                            Err(e) => {
                                eprintln!("inkwell catalog {id}: {e}");
                                failed.push(path);
                            }
                        }
                    }
                    if let Err(e) = super::settle_pending(dir, &done) {
                        // The list still names what was forgotten: looking again would find the same, so the next
                        // hold does (a forget is safe to repeat).
                        eprintln!("inkwell catalog {id}: {e}");
                        return;
                    }
                }
            }
            #[cfg(test)]
            super::tests::while_held(dir);
        }
        // A forget that found the catalog held (this one) wrote its path and left: it is done here, now that the
        // hold is released, not at some later build.
        match super::pending(dir) {
            super::Waiting::Paths(p) if p.iter().all(|x| failed.contains(x)) => return,
            _ => {}
        }
    }
}

/// Startup, and whenever the catalogs are listed: every catalog with paths waiting (a quit while it was being
/// built) forgets them now.
pub fn replay_pending(root: &std::path::Path, jobs: &Jobs) {
    for m in super::list(root) {
        let dir = root.join(&m.id);
        // Looked at without the hold, so listing never makes a build or a delete wait or be refused.
        if !matches!(super::pending(&dir), super::Waiting::Paths(p) if p.is_empty()) {
            forget_deferred(jobs, &m.id, &dir);
        }
    }
}

/// [`replay_pending`] for the app's own catalogs folder.
pub fn replay_pending_at_startup(app: &tauri::AppHandle) {
    let Ok((data, _)) = crate::recents::data_dir(app) else { return };
    replay_pending(&data.join("catalogs"), &app.state::<Arc<Jobs>>());
}

/// A document with redactions was saved over `path`: no catalog keeps its old text.
pub fn forget_saved(app: &tauri::AppHandle, path: &str) {
    let Ok((data, _)) = crate::recents::data_dir(app) else { return };
    forget_all(&data.join("catalogs"), &app.state::<Arc<Jobs>>(), std::path::Path::new(path));
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BuildEvent {
    #[serde(rename_all = "camelCase")]
    Started { job: u64 },
    #[serde(rename_all = "camelCase")]
    Progress { job: u64, done: u32, total: u32 },
    #[serde(rename_all = "camelCase")]
    Done { job: u64, summary: Summary, catalog: Meta },
}

fn root(window: &Window) -> Result<PathBuf> {
    let (dir, _) = crate::recents::data_dir(window.app_handle()).map_err(EngineError::Io)?;
    let r = dir.join("catalogs");
    std::fs::create_dir_all(&r).map_err(|e| EngineError::Io(e.to_string()))?;
    Ok(r)
}

fn dir_of(window: &Window, id: &str) -> Result<PathBuf> {
    if !super::valid_id(id) {
        return Err(EngineError::Invalid("unknown catalog".into()));
    }
    Ok(root(window)?.join(id))
}

fn folder_of(window: &Window, token: &str) -> Result<PathBuf> {
    let p = PathBuf::from(window.state::<Paths>().open(window.label(), token)?);
    if !p.is_dir() {
        return Err(EngineError::Io(format!("{} is not a folder", p.display())));
    }
    Ok(p)
}

/// What indexing the folder the token names would take, from a quick listing.
#[tauri::command]
pub async fn catalog_estimate(window: Window, folder: String, recursive: bool) -> Result<Estimate> {
    let dir = folder_of(&window, &folder)?;
    blocking(move || Ok(super::estimate(&dir, recursive, &AtomicBool::new(false)))).await
}

/// A new, empty catalog for the folder the token names. Build it with `catalog_build`.
#[tauri::command]
pub async fn catalog_create(window: Window, folder: String, name: String, recursive: bool) -> Result<Meta> {
    let dir = folder_of(&window, &folder)?;
    let root = root(&window)?;
    blocking(move || {
        let fallback = dir.file_name().map_or_else(|| "Catalog".to_string(), |n| n.to_string_lossy().into_owned());
        let name = if name.trim().is_empty() { fallback } else { name };
        super::create(&root, &name, &dir.to_string_lossy(), recursive)
    })
    .await
}

#[tauri::command]
pub async fn catalog_list(window: Window, jobs: State<'_, Arc<Jobs>>) -> Result<Vec<Meta>> {
    let root = root(&window)?;
    let jobs = jobs.inner().clone();
    blocking(move || {
        replay_pending(&root, &jobs);
        Ok(super::list(&root))
    })
    .await
}

#[tauri::command]
pub async fn catalog_delete(window: Window, jobs: State<'_, Arc<Jobs>>, id: String) -> Result<()> {
    let root = root(&window)?;
    // The id is held for the whole delete, so a build cannot start on a folder that is being removed.
    let jobs = jobs.inner().clone();
    blocking(move || jobs.guarded(&id, || super::delete(&root, &id))).await
}

/// Whether a refresh would change anything (counts and modified times; no file is opened).
#[tauri::command]
pub async fn catalog_stale(window: Window, id: String) -> Result<Stale> {
    let dir = dir_of(&window, &id)?;
    blocking(move || super::check(&dir, &AtomicBool::new(false))).await
}

/// Builds (`full`) or refreshes a catalog; events (`started` first, then progress, then done) go to `on_event`.
/// Returns the summary. Cancel with `catalog_cancel(job)`; a cancelled build leaves the old index in place.
#[tauri::command]
pub async fn catalog_build(window: Window, jobs: State<'_, Arc<Jobs>>, id: String, full: bool, on_event: Channel<BuildEvent>) -> Result<Summary> {
    let dir = dir_of(&window, &id)?;
    let jobs = jobs.inner().clone();
    if !jobs.busy.lock().unwrap().insert(id.clone()) {
        return Err(EngineError::Invalid("this catalog is already being built".into()));
    }
    let job = jobs.next.fetch_add(1, Ordering::SeqCst) + 1;
    let cancel = Arc::new(AtomicBool::new(false));
    jobs.live.lock().unwrap().insert(job, (window.label().to_string(), cancel.clone()));
    let _ = on_event.send(BuildEvent::Started { job });
    let (j2, id2) = (jobs.clone(), id.clone());
    blocking(move || {
        let ev = on_event.clone();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let r = super::build(&dir, full, &cancel, &move |done, total| {
                let _ = ev.send(BuildEvent::Progress { job, done, total });
            });
            if let Ok(summary) = &r
                && let Ok(catalog) = super::load_meta(&dir)
            {
                let _ = on_event.send(BuildEvent::Done { job, summary: summary.clone(), catalog });
            }
            r
        }));
        // Free again (even after a panic), and what was saved meanwhile is forgotten now, here and not on the
        // async thread.
        j2.live.lock().unwrap().remove(&job);
        j2.busy.lock().unwrap().remove(&id2);
        forget_deferred(&j2, &id2, &dir);
        r.unwrap_or(Err(EngineError::Panic))
    })
    .await
}

#[tauri::command]
pub fn catalog_cancel(window: Window, jobs: State<'_, Arc<Jobs>>, job: u64) {
    if let Some((w, c)) = jobs.live.lock().unwrap().get(&job)
        && w == window.label()
    {
        c.store(true, Ordering::SeqCst);
    }
}

/// Searches a catalog with the folder search's events; results open through `search_file_token` and the search
/// cancels with `cancel_search_files`. Returns the search id.
#[tauri::command]
pub async fn catalog_search(
    window: Window,
    searches: State<'_, Arc<FolderSearches>>,
    id: String,
    query: String,
    opts: Option<SearchOpts>,
    on_event: Channel<FilesEvent>,
) -> Result<u64> {
    let dir = dir_of(&window, &id)?;
    let searches = searches.inner().clone();
    let win = window.label().to_string();
    blocking(move || {
        super::search(&dir, &query, opts.unwrap_or_default(), &searches, &win, &|e| {
            let _ = on_event.send(e);
        })
    })
    .await
}

/// What the open document carries of an embedded index.
#[tauri::command]
pub async fn index_embedded(eng: Eng<'_>, doc: DocId) -> Result<Embedded> {
    let eng = eng.inner().clone();
    blocking(move || embed::embedded(&eng, doc)).await
}

/// Writes the search index of the open document into it (one undo step; Save keeps it).
#[tauri::command]
pub async fn index_embed(eng: Eng<'_>, doc: DocId) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || embed::embed(&eng, doc, EditSource::User)).await
}

#[tauri::command]
pub async fn index_unembed(eng: Eng<'_>, doc: DocId) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || embed::unembed(&eng, doc, EditSource::User)).await
}
