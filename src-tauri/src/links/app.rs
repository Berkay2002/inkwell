//! Tauri commands for links. The UI is the user; the assistant calls the functions in `super`.
//! Nothing here opens a web address by itself: `link_open_url` is what the UI calls after the
//! person said yes in its confirmation, and it still passes the address gate.

use super::{LinkAudit, LinkEdited, LinkOp, PageLinks, UrlPreview};
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

/// Every link of the document counted, and the broken ones listed.
#[tauri::command]
pub async fn links_audit(eng: Eng<'_>, doc: DocId) -> Result<LinkAudit> {
    let eng = eng.inner().clone();
    blocking(move || super::audit(&eng, doc)).await
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

/// The path of the PDF a file link names, for the confirmation to show in full. No token is issued: the
/// file is only named here. A name that is not a PDF found from the document is refused with the reason.
#[tauri::command]
pub fn link_file_path(eng: Eng<'_>, doc: DocId, name: String) -> Result<String> {
    let path = eng.doc_entry(doc)?.path();
    Ok(super::resolve_file_link(&path, &name)?.to_string_lossy().into_owned())
}

/// The PDF a file link names, as an open token, for the tab to open it. Call it only after the person
/// confirmed what `link_file_path` showed: the token is what lets the file be opened.
#[tauri::command]
pub fn link_file_ref(window: Window, eng: Eng<'_>, doc: DocId, name: String) -> Result<FileRef> {
    let path = eng.doc_entry(doc)?.path();
    let found = super::resolve_file_link(&path, &name)?;
    Ok(window.state::<Paths>().issue_open(window.label(), found.to_string_lossy().into_owned()))
}

/// Hands a gated address to the system. Call it only after the person confirmed.
#[tauri::command]
pub fn link_open_url(url: String) -> Result<()> {
    let url = super::gate_url(&url)?;
    open_with_system(&url).map_err(|e| EngineError::Io(e.to_string()))
}

#[cfg(windows)]
pub(crate) fn open_with_system(url: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut std::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> *mut std::ffi::c_void;
    }
    let wide = |s: &str| std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(url));
    // SW_SHOWNORMAL = 1. A result above 32 is success. The address has passed the gate: https, http or mailto.
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
    if (r as usize) > 32 { Ok(()) } else { Err(std::io::Error::other(format!("the system could not open the address (code {})", r as usize))) }
}

#[cfg(target_os = "macos")]
fn open_with_system(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open").arg(url).spawn().map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_with_system(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ())
}
