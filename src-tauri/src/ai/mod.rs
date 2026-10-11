//! The AI assistant: chat, summaries and cited answers about an open document, powered by the
//! user's own subscription through the locally logged-in CLI of their choice: Codex (ChatGPT,
//! `codex app-server`) or Claude Code (`claude`, see `claude/`). No API keys, nothing persisted;
//! text leaves the machine only on an explicit user action.

pub mod chat;
pub mod claude;
pub mod codex;
mod gate;
pub mod provider;
pub mod rpc;
pub mod text;
pub mod tools;

use crate::engine::{DocId, Engine, OutlineItem, pool::class};
pub use chat::Assistant;
use chat::{AskRequest, ChatEvent, Prepared, Status};
pub use provider::{Hub, Provider, ProviderId};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use tauri::ipc::Channel;
use text::DocText;

/// The engine side of one open document, for the tools. A tab that closed meanwhile makes every
/// call fail with "document closed", which the model reads.
struct EngineHost {
    eng: Arc<Engine>,
    doc: DocId,
}

impl tools::DocHost for EngineHost {
    fn meta(&self) -> Vec<(&'static str, String)> {
        let doc = self.doc;
        self.eng.call(class::BACKGROUND, move |e| e.metadata(doc)).unwrap_or_default()
    }

    fn outline(&self) -> Result<Vec<OutlineItem>, String> {
        let doc = self.doc;
        self.eng.call(class::INTERACTIVE, move |e| e.outline(doc)).map_err(|e| e.to_string())
    }

    fn page_png(&self, rev: u32, page: u32, max_px: u32) -> Result<Vec<u8>, String> {
        let doc = self.doc;
        self.eng.call(class::INTERACTIVE, move |e| e.page_png(doc, rev, page, max_px)).map_err(|e| e.to_string())
    }
}

type Ai<'a> = State<'a, Arc<Hub>>;
type Eng<'a> = State<'a, Arc<Engine>>;

fn text_for(eng: &Arc<Engine>, hub: &Hub, doc: DocId) -> Result<Arc<DocText>, String> {
    let rev = eng.doc_entry(doc).map_err(|e| e.to_string())?.generation();
    hub.doc_text(doc, rev, || text::extract(eng, doc).map_err(|e| e.to_string()))
}

async fn off_thread<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Result<R, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

/// Installed / signed in / models of the chosen provider (Codex when none is named). Starts its CLI
/// if it is not running yet.
#[tauri::command]
pub async fn ai_status(ai: Ai<'_>, provider: Option<ProviderId>) -> Result<Status, String> {
    let p = ai.provider(provider.unwrap_or_default());
    off_thread(move || p.status()).await
}

/// Reads the document's text (cached) and warms its chat, so the first question is quick.
#[tauri::command]
pub async fn ai_prepare(eng: Eng<'_>, ai: Ai<'_>, doc: DocId, provider: Option<ProviderId>) -> Result<Prepared, String> {
    let (eng, ai) = (eng.inner().clone(), ai.inner().clone());
    off_thread(move || {
        let t = text_for(&eng, &ai, doc)?;
        let out = Prepared { pages: t.pages, chars: t.chars, enough: t.enough() };
        if out.enough {
            let (p, rev) = (ai.provider(provider.unwrap_or_default()), t.rev);
            std::thread::spawn(move || p.warm(doc, rev));
        }
        Ok(out)
    })
    .await?
}

/// Starts a question; the answer streams through `on_event`. Returns once the turn is queued.
#[tauri::command]
pub fn ai_ask(
    eng: Eng<'_>,
    ai: Ai<'_>,
    doc: DocId,
    req: AskRequest,
    provider: Option<ProviderId>,
    on_event: Channel<ChatEvent>,
) -> Result<(), String> {
    let (eng, ai) = (eng.inner().clone(), ai.inner().clone());
    let host = Arc::new(EngineHost { eng: eng.clone(), doc });
    let p = ai.provider(provider.unwrap_or_default());
    let loader: provider::Loader = {
        let ai = ai.clone();
        Box::new(move || text_for(&eng, &ai, doc))
    };
    p.ask(
        doc,
        req,
        loader,
        host,
        Arc::new(move |e| {
            let _ = on_event.send(e);
        }),
    )
}

#[tauri::command]
pub fn ai_cancel(ai: Ai<'_>, doc: DocId) {
    ai.cancel(doc);
}

/// Apply (`allow`) or Reject of the change card the assistant is waiting on.
#[tauri::command]
pub fn ai_tool_decision(ai: Ai<'_>, doc: DocId, call_id: String, allow: bool) {
    ai.decide(doc, &call_id, allow);
}

/// The user changed the permission mode while a turn runs: the very next tool call obeys it.
#[tauri::command]
pub fn ai_set_permissions(ai: Ai<'_>, doc: DocId, mode: tools::Mode) {
    ai.set_permissions(doc, mode);
}

/// The user's own path to `claude` (a setting); an empty string goes back to looking for it.
#[tauri::command]
pub fn ai_set_claude_path(ai: Ai<'_>, path: Option<String>) {
    ai.claude().set_executable(path.map(PathBuf::from));
}

/// The document was closed: drop its chats and cached text.
#[tauri::command]
pub async fn ai_close(ai: Ai<'_>, doc: DocId) -> Result<(), String> {
    let ai = ai.inner().clone();
    off_thread(move || ai.close_doc(doc)).await
}
