//! Tauri commands for links. The UI is the user; the assistant calls the functions in `super`.
//! Nothing here opens a web address by itself: `link_open_url` is what the UI calls after the
//! person said yes in its confirmation, and it still passes the address gate.

use super::{LinkEdited, LinkOp, PageLinks, UrlPreview};
use crate::engine::edit::EditSource;
use crate::engine::{DocId, Engine, EngineError};
use crate::paths::{FileRef, Paths};
use tauri::{Manager, Window};
use tauri_plugin_dialog::DialogExt;

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

/// The links of `page` (0-based), with the `rev` they belong to.
#[tauri::command]
pub async fn links_list(eng: Eng<'_>, doc: DocId, page: u32) -> Result<PageLinks> {
    let eng = eng.inner().clone();
    blocking(move || super::list(&eng, doc, page)).await
}

/// Applies one change as listed at `rev` (`Stale` if the document moved on). A file target
/// names an open token.
#[tauri::command]
pub async fn links_apply(window: Window, eng: Eng<'_>, doc: DocId, rev: u32, mut op: LinkOp) -> Result<LinkEdited> {
    let reg = window.state::<Paths>();
    super::resolve_files(&mut op, |t| reg.open(window.label(), t))?;
    let eng = eng.inner().clone();
    blocking(move || super::apply(&eng, doc, rev, &op, EditSource::User)).await
}

/// How many links "from web addresses" would make (and the first of them), changing nothing.
#[tauri::command]
pub async fn links_urls(eng: Eng<'_>, doc: DocId) -> Result<UrlPreview> {
    let eng = eng.inner().clone();
    blocking(move || super::preview_urls(&eng, doc)).await
}

/// An address as it would be linked, or why it can't be.
#[tauri::command]
pub fn link_check_url(url: String) -> Result<String> {
    super::gate_url(&url)
}

/// Choose a file a link opens: any file, as an open token.
#[tauri::command]
pub async fn link_pick_file(window: Window) -> Result<Option<FileRef>> {
    blocking(move || {
        let picked = window.dialog().file().set_parent(&window).set_title("Choose the file the link opens").blocking_pick_file();
        Ok(picked.and_then(|f| f.into_path().ok()).map(|p| window.state::<Paths>().issue_open(window.label(), p.to_string_lossy().into_owned())))
    })
    .await
}

/// Hands a gated address to the system. Call it only after the person confirmed.
#[tauri::command]
pub fn link_open_url(url: String) -> Result<()> {
    let url = super::gate_url(&url)?;
    #[cfg(windows)]
    let status = std::process::Command::new("rundll32").arg("url.dll,FileProtocolHandler").arg(&url).spawn();
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = std::process::Command::new("xdg-open").arg(&url).spawn();
    status.map(|_| ()).map_err(|e| EngineError::Io(e.to_string()))
}
