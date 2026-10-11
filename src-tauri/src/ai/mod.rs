//! The AI assistant: chat, summaries and cited answers about an open document, powered by the
//! user's own ChatGPT subscription through the locally logged-in Codex CLI (`codex app-server`).
//! No API keys, nothing persisted; text leaves the machine only on an explicit user action.

pub mod actions;
pub mod chat;
pub mod codex;
pub mod jobs;
pub mod rpc;
pub mod text;
pub mod toolbox;
pub mod tools;

use crate::engine::edit::EditSource;
use crate::engine::{DocId, Engine, OutlineItem, pool::class};
pub use chat::Assistant;
use chat::{AskRequest, ChatEvent, Prepared, Status};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{Manager, State};
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

/// The host of one turn: the engine side plus the edit lane the action tools change the document
/// through. Every change of the turn is made as `EditSource::Assistant { turn }`, so the whole turn
/// is one undo unit; once one landed, page images are rendered from the document as it is now.
struct ActingHost {
    inner: EngineHost,
    turn: String,
    edited: AtomicBool,
    /// The window the question came from: files the assistant makes are offered to it as open tokens.
    app: tauri::AppHandle,
    window: String,
}

impl ActingHost {
    fn new(eng: Arc<Engine>, doc: DocId, app: tauri::AppHandle, window: String) -> Self {
        static TURNS: AtomicU64 = AtomicU64::new(1);
        let n = TURNS.fetch_add(1, Ordering::Relaxed);
        ActingHost { inner: EngineHost { eng, doc }, turn: format!("assistant-{n}"), edited: AtomicBool::new(false), app, window }
    }
}

impl tools::DocHost for ActingHost {
    fn meta(&self) -> Vec<(&'static str, String)> {
        self.inner.meta()
    }

    fn outline(&self) -> Result<Vec<OutlineItem>, String> {
        self.inner.outline()
    }

    fn page_png(&self, rev: u32, page: u32, max_px: u32) -> Result<Vec<u8>, String> {
        let rev = if self.edited.load(Ordering::SeqCst) {
            self.inner.eng.doc_entry(self.inner.doc).map_err(|e| e.to_string())?.generation()
        } else {
            rev
        };
        self.inner.page_png(rev, page, max_px)
    }

    fn lane(&self) -> Option<tools::Lane<'_>> {
        Some(tools::Lane {
            eng: &self.inner.eng,
            doc: self.inner.doc,
            source: EditSource::Assistant { turn: self.turn.clone() },
            edited: &self.edited,
        })
    }

    fn offer_file(&self, path: &str) -> Option<String> {
        let paths = self.app.try_state::<crate::paths::Paths>()?;
        Some(paths.issue_open(&self.window, path.to_string()).token)
    }

    fn tesseract(&self) -> Option<std::path::PathBuf> {
        self.app.try_state::<Arc<crate::tools::paths::ToolPaths>>()?.tesseract()
    }
}

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
            let (ai, rev) = (ai.clone(), t.rev);
            std::thread::spawn(move || ai.warm(doc, rev));
        }
        Ok(out)
    })
    .await?
}

/// Starts a question; the answer streams through `on_event`. Returns once the turn is queued.
#[tauri::command]
pub fn ai_ask(
    window: tauri::Window,
    eng: Eng<'_>,
    ai: Ai<'_>,
    doc: DocId,
    req: AskRequest,
    on_event: Channel<ChatEvent>,
) -> Result<(), String> {
    let (eng, ai) = (eng.inner().clone(), ai.inner().clone());
    let host = Arc::new(ActingHost::new(eng.clone(), doc, window.app_handle().clone(), window.label().to_string()));
    let loader = {
        let ai = ai.clone();
        move || text_for(&eng, &ai, doc)
    };
    ai.ask(
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

/// The document was closed: drop its chat thread and cached text.
#[tauri::command]
pub async fn ai_close(ai: Ai<'_>, doc: DocId) -> Result<(), String> {
    let ai = ai.inner().clone();
    off_thread(move || ai.close_doc(doc)).await
}
