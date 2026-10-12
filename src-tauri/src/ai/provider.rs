//! What the assistant panel talks to: one provider per backend, one chosen per question.
//!
//! * `Assistant` (`chat.rs`) is Codex: ChatGPT subscription, one warm `codex app-server`.
//! * `ClaudeProvider` (`claude/`) is Claude Code: Claude subscription, one `claude` child per chat.
//!
//! Both turn what their CLI writes into the same `ChatEvent`s and answer tool calls through the
//! same gate (`gate.rs`) and registry (`tools.rs`), so the UI, the permission modes and every tool
//! are the same whichever is chosen. The `Hub` below owns both and routes by the id the UI sends.

use super::chat::{AskRequest, Assistant, Sink, Status};
use super::claude::ClaudeProvider;
use super::text::DocText;
use super::tools::{DocHost, Mode, Tool};
use crate::engine::DocId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    #[default]
    Codex,
    Claude,
}

/// Produces the document's text (cached by the hub); runs on the turn's own thread.
pub type Loader = Box<dyn FnOnce() -> Result<Arc<DocText>, String> + Send + 'static>;

pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    /// Installed / signed in / models / account. May start the CLI, so call it off the UI thread.
    fn status(&self) -> Status;
    /// Gets a document's chat ready so the first question is quick (a no-op where there is nothing to do).
    fn warm(&self, doc: DocId, rev: u32);
    /// Starts a turn on its own thread and returns at once; everything arrives through `sink`.
    fn ask(
        self: Arc<Self>,
        doc: DocId,
        req: AskRequest,
        load: Loader,
        host: Arc<dyn DocHost>,
        sink: Sink,
    ) -> Result<(), String>;
    /// Stops the turn in flight (it ends as `Cancelled`).
    fn cancel(&self, doc: DocId);
    /// The user's Apply / Reject for a waiting tool call. Unknown or settled ids are ignored.
    fn decide(&self, doc: DocId, call_id: &str, allow: bool);
    /// The permission mode changed while a turn runs: the very next call obeys it.
    fn set_permissions(&self, doc: DocId, mode: Mode);
    /// The tab was closed: everything started for it goes.
    fn close_doc(&self, doc: DocId);
}

impl Provider for Assistant {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn status(&self) -> Status {
        Assistant::status(self)
    }

    fn warm(&self, doc: DocId, rev: u32) {
        Assistant::warm(self, doc, rev)
    }

    fn ask(
        self: Arc<Self>,
        doc: DocId,
        req: AskRequest,
        load: Loader,
        host: Arc<dyn DocHost>,
        sink: Sink,
    ) -> Result<(), String> {
        Assistant::ask(&self, doc, req, load, host, sink)
    }

    fn cancel(&self, doc: DocId) {
        Assistant::cancel(self, doc)
    }

    fn decide(&self, doc: DocId, call_id: &str, allow: bool) {
        Assistant::decide(self, doc, call_id, allow)
    }

    fn set_permissions(&self, doc: DocId, mode: Mode) {
        Assistant::set_permissions(self, doc, mode)
    }

    fn close_doc(&self, doc: DocId) {
        Assistant::close_doc(self, doc)
    }
}

/// Both providers and the document text they share.
pub struct Hub {
    codex: Arc<Assistant>,
    claude: Arc<ClaudeProvider>,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        let codex = Assistant::new();
        let claude = ClaudeProvider::new(codex.tool_registry());
        Arc::new(Self { codex, claude })
    }

    /// For tests: the given providers instead of the real CLIs.
    pub fn with(codex: Arc<Assistant>, claude: Arc<ClaudeProvider>) -> Arc<Self> {
        Arc::new(Self { codex, claude })
    }

    pub fn provider(&self, id: ProviderId) -> Arc<dyn Provider> {
        match id {
            ProviderId::Codex => self.codex.clone(),
            ProviderId::Claude => self.claude.clone(),
        }
    }

    pub fn codex(&self) -> &Arc<Assistant> {
        &self.codex
    }

    pub fn claude(&self) -> &Arc<ClaudeProvider> {
        &self.claude
    }

    /// Adds a tool for both providers (they share one registry).
    pub fn register(&self, tool: Arc<Tool>) {
        self.codex.register(tool);
    }

    /// Cached text of a document at edit generation `rev`; one cache for both providers.
    pub fn doc_text(
        &self,
        doc: DocId,
        rev: u32,
        load: impl FnOnce() -> Result<DocText, String>,
    ) -> Result<Arc<DocText>, String> {
        self.codex.doc_text(doc, rev, load)
    }

    /// Cancel, decide, permissions and close reach both: a document has a session in at most one
    /// of them that matters, and the other finds nothing under the id and does nothing.
    pub fn cancel(&self, doc: DocId) {
        self.codex.cancel(doc);
        self.claude.cancel(doc);
    }

    pub fn decide(&self, doc: DocId, call_id: &str, allow: bool) {
        self.codex.decide(doc, call_id, allow);
        self.claude.decide(doc, call_id, allow);
    }

    pub fn set_permissions(&self, doc: DocId, mode: Mode) {
        self.codex.set_permissions(doc, mode);
        self.claude.set_permissions(doc, mode);
    }

    /// Also drops the cached text.
    pub fn close_doc(&self, doc: DocId) {
        self.claude.close_doc(doc);
        self.codex.close_doc(doc);
    }
}
