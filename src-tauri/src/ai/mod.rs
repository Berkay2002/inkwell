//! The AI assistant: chat, summaries and cited answers about an open document, powered by the
//! user's own ChatGPT subscription through the locally logged-in Codex CLI (`codex app-server`).
//! No API keys, nothing persisted; text leaves the machine only on an explicit user action.

pub mod chat;
pub mod codex;
pub mod rpc;
pub mod text;

use crate::engine::{DocId, Engine};
pub use chat::Assistant;
use chat::{AskRequest, ChatEvent, Prepared, Status};
use std::sync::Arc;
use tauri::State;
use tauri::ipc::Channel;
use text::DocText;

type Ai<'a> = State<'a, Arc<Assistant>>;
type Eng<'a> = State<'a, Arc<Engine>>;

fn text_for(eng: &Arc<Engine>, ai: &Assistant, doc: DocId) -> Result<Arc<DocText>, String> {
    let rev = eng.doc_entry(doc).map_err(|e| e.to_string())?.generation();
    ai.doc_text(doc, rev, || text::extract(eng, doc).map_err(|e| e.to_string()))
}

async fn off_thread<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

/// Installed / signed in / models. Starts the codex server if it is not running yet.
#[tauri::command]
pub async fn ai_status(ai: Ai<'_>) -> Result<Status, String> {
    let ai = ai.inner().clone();
    off_thread(move || ai.status()).await
}

/// Reads the document's text (cached) and warms its chat thread, so the first question is quick.
#[tauri::command]
pub async fn ai_prepare(eng: Eng<'_>, ai: Ai<'_>, doc: DocId) -> Result<Prepared, String> {
    let (eng, ai) = (eng.inner().clone(), ai.inner().clone());
    off_thread(move || {
        let t = text_for(&eng, &ai, doc)?;
        let out = Prepared { pages: t.pages, chars: t.chars, enough: t.enough() };
        if out.enough {
            let ai = ai.clone();
            std::thread::spawn(move || ai.warm(doc));
        }
        Ok(out)
    })
    .await?
}

/// Starts a question; the answer streams through `on_event`. Returns once the turn is queued.
#[tauri::command]
pub fn ai_ask(eng: Eng<'_>, ai: Ai<'_>, doc: DocId, req: AskRequest, on_event: Channel<ChatEvent>) -> Result<(), String> {
    let (eng, ai) = (eng.inner().clone(), ai.inner().clone());
    let loader = {
        let ai = ai.clone();
        move || text_for(&eng, &ai, doc)
    };
    ai.ask(
        doc,
        req,
        loader,
        Arc::new(move |e| {
            let _ = on_event.send(e);
        }),
    )
}

#[tauri::command]
pub fn ai_cancel(ai: Ai<'_>, doc: DocId) {
    ai.cancel(doc);
}

/// The document was closed: drop its chat thread and cached text.
#[tauri::command]
pub fn ai_close(ai: Ai<'_>, doc: DocId) {
    ai.close_doc(doc);
}
