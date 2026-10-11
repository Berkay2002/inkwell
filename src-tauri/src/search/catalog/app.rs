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
pub async fn catalog_list(window: Window) -> Result<Vec<Meta>> {
    let root = root(&window)?;
    blocking(move || Ok(super::list(&root))).await
}

#[tauri::command]
pub async fn catalog_delete(window: Window, jobs: State<'_, Arc<Jobs>>, id: String) -> Result<()> {
    let root = root(&window)?;
    if jobs.busy.lock().unwrap().contains(&id) {
        return Err(EngineError::Invalid("this catalog is being built; cancel that first".into()));
    }
    blocking(move || super::delete(&root, &id)).await
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
    let out = blocking(move || {
        let ev = on_event.clone();
        let r = super::build(&dir, full, &cancel, &move |done, total| {
            let _ = ev.send(BuildEvent::Progress { job, done, total });
        });
        if let Ok(summary) = &r {
            if let Ok(catalog) = super::load_meta(&dir) {
                let _ = on_event.send(BuildEvent::Done { job, summary: summary.clone(), catalog });
            }
        }
        r
    })
    .await;
    j2.live.lock().unwrap().remove(&job);
    j2.busy.lock().unwrap().remove(&id2);
    out
}

#[tauri::command]
pub fn catalog_cancel(window: Window, jobs: State<'_, Arc<Jobs>>, job: u64) {
    if let Some((w, c)) = jobs.live.lock().unwrap().get(&job) {
        if w == window.label() {
            c.store(true, Ordering::SeqCst);
        }
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
