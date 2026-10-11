//! The protect commands. Setting or removing security is one edit step; Save or Save as (with a
//! `pick_save` token, through `save_document`) writes it.

use super::{ProtectSpec, SecurityInfo};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use tauri::State;

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = State<'a, std::sync::Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or_else(|_| Err(EngineError::Invalid("the task crashed".into())))
}

/// What the document is protected with, as Save would write it.
#[tauri::command]
pub async fn security_info(eng: Eng<'_>, doc: DocId) -> Result<SecurityInfo> {
    let eng = eng.inner().clone();
    blocking(move || eng.security_info(doc)).await
}

/// `permissions_password`: only to change the security of a file that has restrictions.
#[tauri::command]
pub async fn protect_apply(eng: Eng<'_>, doc: DocId, spec: ProtectSpec, permissions_password: Option<String>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || eng.protect(doc, spec, permissions_password.as_deref(), EditSource::User)).await
}

#[tauri::command]
pub async fn remove_security(eng: Eng<'_>, doc: DocId, permissions_password: Option<String>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || eng.remove_security(doc, permissions_password.as_deref(), EditSource::User)).await
}
