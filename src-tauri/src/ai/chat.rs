//! One warm `codex app-server` shared by every open document; one ephemeral thread per document
//! chat. A turn = a small header (title, outline, page in view, selection; the whole text only for a
//! short document) + the task, streamed back as events. The model pulls what it needs through
//! Inkwell's dynamic tools (`tools.rs`), whose calls are answered here, in the turn's stream loop,
//! under the user's permission mode. The server is started lazily and restarted after a crash.

use super::rpc::{CLOSED, Client, RpcError, SpawnSpec, TOOL_CALL};
use super::text::{self, Citation, DocText, Task};
use super::tools::{self, Class, Ctx, DocHost, Mode, Selected, Tool, UiAction};
use crate::engine::DocId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// No notification for this long and the server is considered hung.
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
/// An Apply / Reject card nobody answered is a rejection after this long. Below what the real
/// server waits for a tool result: codex 0.159 still took a reply at 273 s and had given up by
/// 330 s (live test `live_how_late_may_an_apply_be`), and a change must never run after the
/// model was told it did not happen.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(240);
/// Header lines of the outline sent with the first turn of a long document.
const HEADER_OUTLINE_LINES: usize = 60;

/// How far a tool call got; the activity trail and the approval card render it.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActState {
    /// Needs the user's Apply / Reject.
    Waiting,
    Running,
    Done,
    Failed,
    Rejected,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatEvent {
    /// "reading" (extracting text), "connecting" (starting codex), "thinking".
    Status { phase: &'static str },
    /// Growth of the visible answer; append it.
    Delta { text: String },
    #[serde(rename_all = "camelCase")]
    Done {
        answer: String,
        citations: Vec<Citation>,
        followups: Vec<String>,
        /// From the ask until the first answer text.
        first_token_ms: Option<u64>,
        total_ms: u64,
        /// From the ask until the turn was handed to codex (text extraction, startup, retrieval).
        prep_ms: u64,
        chunks_sent: usize,
        chunks_total: usize,
        usage: Option<Value>,
    },
    /// A tool call of this turn, at each step of its life (same `id` throughout).
    Activity { id: String, tool: String, class: Class, label: String, state: ActState, detail: Option<String> },
    /// The model moved the user's view (go_to_page, show_source).
    Ui { action: UiAction },
    Error { message: String, hint: Option<String> },
    Cancelled,
}

pub type Sink = Arc<dyn Fn(ChatEvent) + Send + Sync>;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskRequest {
    pub task: Task,
    /// File name, for the prompt.
    pub name: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// 0-based page in view.
    pub current_page: Option<u32>,
    /// The text selected in the viewer, if any.
    #[serde(default)]
    pub selection: Option<Selected>,
    /// What the model may change without asking (`ask` when absent).
    #[serde(default)]
    pub permissions: Mode,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub installed: bool,
    pub logged_in: bool,
    pub account: Option<String>,
    pub plan: Option<String>,
    pub models: Vec<ModelInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    pub pages: u32,
    pub chars: usize,
    pub enough: bool,
}

struct Failure {
    message: String,
    hint: Option<&'static str>,
}

impl Failure {
    fn new(m: impl Into<String>) -> Self {
        Self { message: m.into(), hint: None }
    }
}

impl From<RpcError> for Failure {
    fn from(e: RpcError) -> Self {
        let message = match &e {
            RpcError::Closed(_) => "Codex stopped unexpectedly. Ask again to restart it.".to_string(),
            RpcError::Timeout => "Codex did not answer in time.".to_string(),
            other => other.to_string(),
        };
        Failure { message, hint: None }
    }
}

struct Server {
    client: Arc<Client>,
    epoch: u64,
}

#[derive(Default)]
struct SessionState {
    /// (server generation, thread id) of the live thread.
    thread: Option<(u64, String)>,
    rx: Option<Receiver<Value>>,
    /// Excerpt ids the model already has in this thread.
    sent: HashSet<String>,
    intro_sent: bool,
    /// Text revision the thread's excerpts came from; an edit starts a new thread.
    rev: u32,
}

#[derive(Default)]
struct Session {
    busy: AtomicBool,
    cancel: AtomicBool,
    /// The tab is gone: nothing started for it may outlive it.
    closed: AtomicBool,
    /// (client, thread, turn) of the turn in flight, for interrupt.
    active: Mutex<Option<(Arc<Client>, String, Option<String>)>>,
    /// Serialises `thread/start` so `state` is never held across an RPC.
    start: Mutex<()>,
    state: Mutex<SessionState>,
    /// Tool calls waiting for Apply / Reject, by call id.
    approvals: Mutex<HashMap<String, Sender<bool>>>,
    /// Threads of sub-agents the model started (`spawn_agent` cannot be switched off): Stop and
    /// close unsubscribe them too, which makes the server unload them.
    children: Mutex<Vec<String>>,
    /// The permission mode every tool call is checked against. Set when a question starts and by
    /// `set_permissions`, so tightening it mid-turn applies to the very next call.
    mode: Mutex<Mode>,
}

/// How a call that needed the user's say ended.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Apply,
    Reject,
    /// No answer within the limit: final, a late Apply is ignored.
    TimedOut,
    /// The turn was stopped or the tab closed while the card waited.
    Stopped,
}

pub struct Assistant {
    spec: Box<dyn Fn() -> Option<SpawnSpec> + Send + Sync>,
    /// The dynamic tools every thread is started with (`register` adds to them).
    tools: RwLock<Vec<Arc<Tool>>>,
    /// Documents with at most this many characters go into the first turn whole.
    inline_max: AtomicUsize,
    /// How long a change card may wait for Apply / Reject, in ms.
    approval_ms: AtomicU64,
    /// Only ever locked briefly: never across a spawn or an RPC.
    server: Mutex<Option<Server>>,
    /// Serialises starting a server.
    starting: Mutex<()>,
    next_gen: AtomicU64,
    sessions: Mutex<HashMap<DocId, Arc<Session>>>,
    texts: Mutex<HashMap<DocId, Arc<DocText>>>,
    idle_timeout: Duration,
    call_timeout: Duration,
}

impl Assistant {
    pub fn new() -> Arc<Self> {
        Self::with_spec(Box::new(super::codex::spawn_spec), IDLE_TIMEOUT)
    }

    pub fn with_spec(spec: Box<dyn Fn() -> Option<SpawnSpec> + Send + Sync>, idle_timeout: Duration) -> Arc<Self> {
        Self::with_timeouts(spec, idle_timeout, CALL_TIMEOUT)
    }

    pub fn with_timeouts(
        spec: Box<dyn Fn() -> Option<SpawnSpec> + Send + Sync>,
        idle_timeout: Duration,
        call_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            spec,
            tools: RwLock::new(tools::builtin()),
            inline_max: AtomicUsize::new(text::INLINE_CHARS),
            approval_ms: AtomicU64::new(APPROVAL_TIMEOUT.as_millis() as u64),
            server: Mutex::new(None),
            starting: Mutex::new(()),
            next_gen: AtomicU64::new(0),
            sessions: Mutex::new(HashMap::new()),
            texts: Mutex::new(HashMap::new()),
            idle_timeout,
            call_timeout,
        })
    }

    /// Adds a tool. Threads already started do not get it, so register before the first question.
    pub fn register(&self, tool: Arc<Tool>) {
        self.tools.write().unwrap().push(tool);
    }

    /// Overrides the short-document threshold (`text::INLINE_CHARS`); used to measure it.
    pub fn set_inline_max(&self, chars: usize) {
        self.inline_max.store(chars, Ordering::SeqCst);
    }

    /// Overrides how long a card may wait (`APPROVAL_TIMEOUT`); tests and the live limit probe.
    pub fn set_approval_timeout(&self, d: Duration) {
        self.approval_ms.store(d.as_millis() as u64, Ordering::SeqCst);
    }

    /// Changes the permission mode of the document's turn in flight (and of the next one, which
    /// brings its own). A call already waiting for the user keeps waiting; the next call is checked
    /// against the new mode, so tightening full -> ask takes effect at once.
    pub fn set_permissions(&self, doc: DocId, mode: Mode) {
        if let Some(s) = self.sessions.lock().unwrap().get(&doc) {
            *s.mode.lock().unwrap() = mode;
        }
    }

    /// The user's answer to a waiting tool call. Unknown or already settled ids are ignored.
    pub fn decide(&self, doc: DocId, call_id: &str, allow: bool) {
        let Some(s) = self.sessions.lock().unwrap().get(&doc).cloned() else { return };
        if let Some(tx) = s.approvals.lock().unwrap().remove(call_id) {
            let _ = tx.send(allow);
        }
    }

    pub fn server_pid(&self) -> Option<u32> {
        self.server.lock().unwrap().as_ref().filter(|s| s.client.is_alive()).map(|s| s.client.pid)
    }

    fn session(&self, doc: DocId) -> Arc<Session> {
        self.sessions.lock().unwrap().entry(doc).or_default().clone()
    }

    fn live_server(&self) -> Option<(Arc<Client>, u64)> {
        let g = self.server.lock().unwrap();
        g.as_ref().filter(|s| s.client.is_alive()).map(|s| (s.client.clone(), s.epoch))
    }

    /// The running server, started (and initialized) on first use or after it died. The `server`
    /// lock is not held while codex starts, so `close_doc` never waits behind a slow start.
    fn ensure_server(&self) -> Result<(Arc<Client>, u64), Failure> {
        if let Some(s) = self.live_server() {
            return Ok(s);
        }
        let _starting = self.starting.lock().unwrap();
        if let Some(s) = self.live_server() {
            return Ok(s); // another thread started it while this one waited
        }
        let spec = (self.spec)().ok_or_else(|| Failure {
            message: "The Codex CLI is not installed.".into(),
            hint: Some("install"),
        })?;
        let client = Client::spawn(&spec)?;
        // `experimentalApi` is what lets `thread/start` carry `dynamicTools` (codex refuses it otherwise).
        let init = json!({
            "clientInfo": { "name": "inkwell", "title": "Inkwell", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "experimentalApi": true },
        });
        let r = client.call("initialize", init, self.call_timeout).and_then(|_| client.notify("initialized", None));
        if let Err(e) = r {
            client.kill();
            return Err(e.into());
        }
        let epoch = self.next_gen.fetch_add(1, Ordering::SeqCst) + 1;
        *self.server.lock().unwrap() = Some(Server { client: client.clone(), epoch });
        Ok((client, epoch))
    }

    /// A request whose timeout means the server is hung: it is replaced, not retried.
    fn rpc(&self, client: &Arc<Client>, method: &str, params: Value) -> Result<Value, RpcError> {
        let r = client.call(method, params, self.call_timeout);
        if matches!(r, Err(RpcError::Timeout)) {
            self.drop_server(client);
        }
        r
    }

    fn drop_server(&self, client: &Arc<Client>) {
        client.kill();
        let mut g = self.server.lock().unwrap();
        if g.as_ref().is_some_and(|s| Arc::ptr_eq(&s.client, client)) {
            *g = None;
        }
    }

    // ---- status ---------------------------------------------------------------------------

    pub fn status(&self) -> Status {
        let mut st = Status { installed: true, logged_in: false, account: None, plan: None, models: Vec::new(), error: None };
        let (client, _) = match self.ensure_server() {
            Ok(c) => c,
            Err(f) => {
                st.installed = f.hint != Some("install");
                st.error = Some(f.message);
                return st;
            }
        };
        match self.rpc(&client, "account/read", json!({ "refreshToken": false })) {
            Ok(v) => {
                if let Some(a) = v.get("account").filter(|a| !a.is_null()) {
                    st.logged_in = true;
                    st.account = a.get("email").and_then(Value::as_str).map(str::to_string);
                    st.plan = a.get("planType").and_then(Value::as_str).map(str::to_string);
                }
            }
            Err(e) => st.error = Some(e.to_string()),
        }
        match self.rpc(&client, "model/list", json!({})) {
            Ok(v) => {
                st.models = v
                    .get("data")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(model_info).collect())
                    .unwrap_or_default();
            }
            // the UI falls back to the server's default model
            Err(_) => {}
        }
        st
    }

    // ---- documents ------------------------------------------------------------------------

    /// Cached text of a document at edit generation `rev` (kept until `close_doc` or an edit),
    /// loaded with `load` on first use.
    pub fn doc_text(
        &self,
        doc: DocId,
        rev: u32,
        load: impl FnOnce() -> Result<DocText, String>,
    ) -> Result<Arc<DocText>, String> {
        if let Some(t) = self.texts.lock().unwrap().get(&doc).filter(|t| t.rev == rev) {
            return Ok(t.clone());
        }
        let t = Arc::new(load()?);
        self.texts.lock().unwrap().insert(doc, t.clone());
        Ok(t)
    }

    /// Warms the document's thread (the first `thread/start` is slow) so the first question is quick.
    /// `rev` is the text revision the excerpts will come from. It does not take `busy`: a question
    /// asked meanwhile waits for the same `thread/start` instead of being refused.
    pub fn warm(&self, doc: DocId, rev: u32) {
        let session = self.session(doc);
        if session.busy.load(Ordering::SeqCst) {
            return; // a turn is running; it has its thread
        }
        let Ok((client, epoch)) = self.ensure_server() else { return };
        let _ = self.ensure_thread(&client, epoch, &session, None, rev);
    }

    /// The tab was closed. A turn in flight is interrupted (and ends as `Cancelled`); the shared
    /// server and every other document's chat are left alone.
    pub fn close_doc(&self, doc: DocId) {
        self.texts.lock().unwrap().remove(&doc);
        let Some(s) = self.sessions.lock().unwrap().remove(&doc) else { return };
        s.closed.store(true, Ordering::SeqCst);
        s.cancel.store(true, Ordering::SeqCst);
        interrupt(&s); // before the unsubscribe: the turn's stream reads the cancel flag, not a dead server
        let client = self.live_server().map(|(c, _)| c);
        let thread = s.state.lock().unwrap().thread.take();
        if let (Some(client), Some((_, tid))) = (client, thread) {
            release_thread(&client, tid);
        }
    }

    pub fn cancel(&self, doc: DocId) {
        let Some(s) = self.sessions.lock().unwrap().get(&doc).cloned() else { return };
        s.cancel.store(true, Ordering::SeqCst);
        interrupt(&s);
    }

    fn ensure_thread(
        &self,
        client: &Arc<Client>,
        epoch: u64,
        session: &Session,
        model: Option<&str>,
        rev: u32,
    ) -> Result<String, Failure> {
        // `start` serialises thread starts; `state` is only held briefly, never across the RPC.
        let _start = session.start.lock().unwrap();
        let stale = {
            let st = session.state.lock().unwrap();
            match &st.thread {
                Some((g, id)) if *g == epoch && client.is_alive() && st.rev == rev => return Ok(id.clone()),
                // same server, but the document text changed: the old excerpts are wrong
                Some((g, id)) if *g == epoch && client.is_alive() => Some(id.clone()),
                _ => None,
            }
        };
        if let Some(id) = stale {
            release_thread(client, id);
        }
        let mut p = json!({
            "ephemeral": true,
            "sandbox": "read-only",
            "approvalPolicy": "never",
            // No environment: drops apply_patch and any file/exec access; dynamic tools still work (codex 0.159).
            "environments": [],
            "cwd": std::env::temp_dir().to_string_lossy(),
            "baseInstructions": text::BASE_INSTRUCTIONS,
            "dynamicTools": tools::specs(&self.tools.read().unwrap()),
        });
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            p["model"] = json!(m);
        }
        let r = self.rpc(client, "thread/start", p)?;
        let id = r
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| Failure::new("codex returned no thread id"))?
            .to_string();
        if session.closed.load(Ordering::SeqCst) {
            release_thread(client, id); // the tab went away while the thread was starting
            return Err(Failure::new("The document was closed."));
        }
        let rx = client.subscribe(&id);
        let mut st = session.state.lock().unwrap();
        st.rx = Some(rx);
        st.thread = Some((epoch, id.clone()));
        st.sent.clear();
        st.intro_sent = false;
        st.rev = rev;
        Ok(id)
    }

    // ---- turns ----------------------------------------------------------------------------

    /// Starts a turn on its own thread and returns at once; everything arrives through `sink`.
    /// `load` produces the document text (cached after the first call); `host` is the engine side
    /// the tools need (outline, page images, metadata).
    pub fn ask(
        self: &Arc<Self>,
        doc: DocId,
        req: AskRequest,
        load: impl FnOnce() -> Result<Arc<DocText>, String> + Send + 'static,
        host: Arc<dyn DocHost>,
        sink: Sink,
    ) -> Result<(), String> {
        let session = self.session(doc);
        if session.busy.swap(true, Ordering::SeqCst) {
            return Err("The assistant is still answering the previous question.".into());
        }
        session.cancel.store(false, Ordering::SeqCst);
        // before the turn thread exists: a `set_permissions` that follows can never be overwritten
        *session.mode.lock().unwrap() = req.permissions;
        let me = self.clone();
        std::thread::Builder::new()
            .name("ai-turn".into())
            .spawn(move || {
                let ev = match me.run_turn(&session, req, load, &*host, &sink) {
                    Ok(ev) => ev,
                    Err(f) => ChatEvent::Error { message: f.message, hint: f.hint.map(str::to_string) },
                };
                *session.active.lock().unwrap() = None;
                session.approvals.lock().unwrap().clear();
                session.busy.store(false, Ordering::SeqCst);
                sink(ev);
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn run_turn(
        &self,
        session: &Arc<Session>,
        req: AskRequest,
        load: impl FnOnce() -> Result<Arc<DocText>, String>,
        host: &dyn DocHost,
        sink: &Sink,
    ) -> Result<ChatEvent, Failure> {
        let t0 = Instant::now();
        sink(ChatEvent::Status { phase: "reading" });
        let text = load().map_err(Failure::new)?;
        if !text.enough() {
            return Err(Failure {
                message: "This file isn't supported for AI features because it doesn't contain enough text. \
                          If it is a scan, run OCR first."
                    .into(),
                hint: Some("noText"),
            });
        }
        if session.cancel.load(Ordering::SeqCst) {
            return Ok(ChatEvent::Cancelled);
        }
        if self.server_pid().is_none() {
            sink(ChatEvent::Status { phase: "connecting" });
        }
        let (client, epoch) = self.ensure_server()?;
        let thread = self.ensure_thread(&client, epoch, session, req.model.as_deref(), text.rev)?;
        if session.cancel.load(Ordering::SeqCst) {
            return Ok(ChatEvent::Cancelled);
        }

        // What to send: the header on a thread's first question (plus the whole text of a short
        // document); everything else the model reads through the tools.
        let selection_task = matches!(req.task, Task::Selection { .. });
        let first = !session.state.lock().unwrap().intro_sent && !selection_task;
        let inline = first && text.chars <= self.inline_max.load(Ordering::SeqCst);
        let header = if first { header_text(host) } else { String::new() };
        let (input, fresh_ids, sent_before) = {
            let st = session.state.lock().unwrap();
            let chunks: Vec<&text::Chunk> = if inline { text.chunks.iter().collect() } else { Vec::new() };
            let info = text::TurnInfo {
                name: &req.name,
                pages: text.pages,
                first,
                header: &header,
                inline: &chunks,
                viewing: req.current_page,
                selection: req.selection.as_ref().map(|s| (s.text.as_str(), s.page)),
            };
            let input = text::build_turn(&req.task, &info);
            let ids: Vec<String> = chunks.iter().map(|c| c.id.clone()).collect();
            // drop notifications left over from an earlier turn; a tool call among them would leave
            // codex waiting for ever
            if let Some(rx) = &st.rx {
                while let Ok(m) = rx.try_recv() {
                    if m.get("method").and_then(Value::as_str) == Some(TOOL_CALL) {
                        if let Some(id) = m.get("id") {
                            client.refuse(id, "that turn is over");
                        }
                    }
                }
            }
            (input, ids, st.sent.len())
        };

        let mut params = json!({
            "threadId": thread,
            "input": [{ "type": "text", "text": input }],
        });
        if let Some(e) = req.effort.as_deref().filter(|e| !e.is_empty()) {
            params["effort"] = json!(e);
        }
        if let Some(m) = req.model.as_deref().filter(|m| !m.is_empty()) {
            params["model"] = json!(m);
        }
        if req.task.cited() {
            params["outputSchema"] = text::schema();
        }
        *session.active.lock().unwrap() = Some((client.clone(), thread.clone(), None));
        sink(ChatEvent::Status { phase: "thinking" });
        let started = self.rpc(&client, "turn/start", params)?;
        let turn_id = started.pointer("/turn/id").and_then(Value::as_str).unwrap_or_default().to_string();
        {
            let mut st = session.state.lock().unwrap();
            st.intro_sent |= first;
            st.sent.extend(fresh_ids);
        }
        if let Some(a) = session.active.lock().unwrap().as_mut() {
            a.2 = Some(turn_id.clone());
        }
        // a cancel that raced the start
        if session.cancel.load(Ordering::SeqCst) {
            interrupt(session);
        }
        let prep_ms = t0.elapsed().as_millis() as u64;

        // ---- stream --------------------------------------------------------------------
        let rx = session.state.lock().unwrap().rx.take().ok_or_else(|| Failure::new("no event stream"))?;
        let ctx = TurnCtx { req: &req, text: &text, host };
        let outcome = self.stream(&rx, &client, session, &ctx, &turn_id, t0, sink);
        session.state.lock().unwrap().rx = Some(rx);
        let (final_text, first_ms, usage) = match outcome {
            Ok(Streamed::Completed { text, first_ms, usage }) => (text, first_ms, usage),
            Ok(Streamed::Interrupted) => return Ok(ChatEvent::Cancelled),
            Err(f) => {
                if f.hint == Some("dead") {
                    self.drop_server(&client);
                    let message = if f.message.is_empty() {
                        "Codex stopped unexpectedly. Ask again to restart it.".to_string()
                    } else {
                        format!("{} Ask again to restart it.", f.message)
                    };
                    return Err(Failure::new(message));
                }
                return Err(f);
            }
        };

        // ---- answer --------------------------------------------------------------------
        let sent = session.state.lock().unwrap().sent.clone();
        let (answer, citations, followups) = match text::parse_reply(&final_text).filter(|_| req.task.cited()) {
            Some(r) => {
                let mut cs: Vec<Citation> = r.citations.iter().map(|c| text::resolve(c, &text, &sent)).collect();
                cs.sort_by_key(|c| c.n);
                let f = r.followups.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(4).collect();
                (r.answer, cs, f)
            }
            None => (final_text, Vec::new(), Vec::new()),
        };
        Ok(ChatEvent::Done {
            answer,
            citations,
            followups,
            first_token_ms: first_ms,
            total_ms: t0.elapsed().as_millis() as u64,
            prep_ms,
            chunks_sent: sent.len().saturating_sub(sent_before),
            chunks_total: text.chunks.len(),
            usage,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn stream(
        &self,
        rx: &Receiver<Value>,
        client: &Client,
        session: &Session,
        ctx: &TurnCtx,
        turn_id: &str,
        t0: Instant,
        sink: &Sink,
    ) -> Result<Streamed, Failure> {
        let cancelled = || session.cancel.load(Ordering::SeqCst);
        let cited = ctx.req.task.cited();
        let mut raw = String::new();
        let mut shown = 0usize;
        let mut final_text: Option<String> = None;
        let mut first_ms = None;
        let mut usage = None;
        // Messages the model wrote between tool calls ("Let me look that up"): not part of the answer.
        let mut commentary: HashSet<String> = HashSet::new();
        let mut page = ctx.req.current_page.unwrap_or(0);
        loop {
            let msg = match rx.recv_timeout(self.idle_timeout) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(Failure { message: "Codex stopped responding.".into(), hint: Some("dead") });
                }
                // The sink is removed when the document is closed (or its thread replaced): that is a
                // cancel, never a reason to kill the server every tab shares.
                Err(RecvTimeoutError::Disconnected) => {
                    return if cancelled() {
                        Ok(Streamed::Interrupted)
                    } else if !client.is_alive() {
                        Err(Failure { message: String::new(), hint: Some("dead") })
                    } else {
                        Err(Failure::new("The answer stream ended unexpectedly. Ask again."))
                    };
                }
            };
            let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
            let p = &msg["params"];
            // a notification of another turn of the same thread (late tail of an earlier one)
            let other_turn = |id: Option<&str>| id.is_some_and(|i| !turn_id.is_empty() && i != turn_id);
            let item_id = p.get("itemId").and_then(Value::as_str).unwrap_or_default();
            match method {
                CLOSED if cancelled() => return Ok(Streamed::Interrupted),
                CLOSED => return Err(Failure { message: String::new(), hint: Some("dead") }),
                TOOL_CALL => {
                    let Some(id) = msg.get("id") else { continue };
                    if other_turn(p.get("turnId").and_then(Value::as_str)) {
                        client.refuse(id, "that turn is over");
                    } else {
                        self.tool_call(id, p, client, session, ctx, &mut page, sink);
                    }
                }
                "item/started" | "item/completed"
                    if p.pointer("/item/type").and_then(Value::as_str) == Some("collabAgentToolCall") =>
                {
                    let ids = p.pointer("/item/receiverThreadIds").and_then(Value::as_array);
                    let mut kids = session.children.lock().unwrap();
                    for id in ids.into_iter().flatten().filter_map(Value::as_str) {
                        if !kids.iter().any(|k| k == id) {
                            kids.push(id.to_string());
                        }
                    }
                }
                "item/started" if p.pointer("/item/type").and_then(Value::as_str) == Some("agentMessage") => {
                    if p.pointer("/item/phase").and_then(Value::as_str) == Some("commentary") {
                        if let Some(id) = p.pointer("/item/id").and_then(Value::as_str) {
                            commentary.insert(id.to_string());
                        }
                    }
                }
                "item/agentMessage/delta" if !other_turn(p.get("turnId").and_then(Value::as_str)) && !commentary.contains(item_id) => {
                    raw.push_str(p.get("delta").and_then(Value::as_str).unwrap_or_default());
                    let visible = if cited { text::partial_answer(&raw) } else { raw.clone() };
                    if visible.len() > shown {
                        // `visible` only ever grows by appending
                        let grown = visible[shown..].to_string();
                        shown = visible.len();
                        first_ms.get_or_insert(t0.elapsed().as_millis() as u64);
                        sink(ChatEvent::Delta { text: grown });
                    }
                }
                "item/completed" if p.pointer("/item/type").and_then(Value::as_str) == Some("agentMessage") => {
                    let id = p.pointer("/item/id").and_then(Value::as_str).unwrap_or_default();
                    let is_commentary = commentary.contains(id) || p.pointer("/item/phase").and_then(Value::as_str) == Some("commentary");
                    if !is_commentary {
                        if let Some(t) = p.pointer("/item/text").and_then(Value::as_str) {
                            final_text = Some(t.to_string());
                        }
                    }
                }
                "thread/tokenUsage/updated" => usage = p.pointer("/tokenUsage/last").cloned(),
                "turn/completed" if !other_turn(p.pointer("/turn/id").and_then(Value::as_str)) => {
                    let status = p.pointer("/turn/status").and_then(Value::as_str).unwrap_or_default();
                    return match status {
                        "completed" => {
                            let text = final_text.unwrap_or(raw);
                            if text.trim().is_empty() {
                                Err(Failure::new("Codex returned an empty answer."))
                            } else {
                                Ok(Streamed::Completed { text, first_ms, usage })
                            }
                        }
                        "interrupted" => Ok(Streamed::Interrupted),
                        _ => Err(turn_failure(p.pointer("/turn/error"))),
                    };
                }
                _ => {}
            }
        }
    }

    // ---- tools ----------------------------------------------------------------------------

    /// Answers one `item/tool/call`: finds the tool, applies the permission mode (waiting for the
    /// user when it needs to), runs it and replies with its result. Every outcome, a refusal
    /// included, is a reply the model can read; the turn is never left hanging.
    #[allow(clippy::too_many_arguments)]
    fn tool_call(
        &self,
        rpc_id: &Value,
        p: &Value,
        client: &Client,
        session: &Session,
        ctx: &TurnCtx,
        page: &mut u32,
        sink: &Sink,
    ) {
        let call_id = p.get("callId").and_then(Value::as_str).unwrap_or_default().to_string();
        let name = p.get("tool").and_then(Value::as_str).unwrap_or_default().to_string();
        let args = match p.get("arguments") {
            // some models hand the arguments over as a JSON string
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
            Some(v) => v.clone(),
            None => Value::Null,
        };
        let args = if args.is_null() { json!({}) } else { args };
        let reply = |r: &Result<tools::Output, String>| {
            let _ = client.respond(rpc_id, tools::response(r));
        };
        let tool = self.tools.read().unwrap().iter().find(|t| t.name == name).cloned();
        let Some(tool) = tool else {
            let why = format!("Unknown tool \"{name}\".");
            sink(ChatEvent::Activity {
                id: call_id,
                tool: name,
                class: Class::Read,
                label: "Called an unknown tool".into(),
                state: ActState::Failed,
                detail: Some(why.clone()),
            });
            reply(&Err(why));
            return;
        };
        let label = (tool.label)(&args);
        let act = |state: ActState, detail: Option<String>| {
            sink(ChatEvent::Activity {
                id: call_id.clone(),
                tool: name.clone(),
                class: tool.class,
                label: label.clone(),
                state,
                detail,
            });
        };
        if session.closed.load(Ordering::SeqCst) || session.cancel.load(Ordering::SeqCst) {
            act(ActState::Failed, Some("stopped".into()));
            reply(&Err("The request was stopped.".into()));
            return;
        }
        // read now, not at the start of the turn: the user may have tightened it since
        let mode = *session.mode.lock().unwrap();
        if mode.needs_approval(tool.class) {
            // registered before the card is announced: the answer may come back at once
            let (tx, rx) = mpsc::channel();
            session.approvals.lock().unwrap().insert(call_id.clone(), tx);
            act(ActState::Waiting, None);
            let (why, text) = match self.await_decision(session, &call_id, rx) {
                Decision::Apply => (None, None),
                Decision::Reject => (
                    None,
                    Some("The user rejected this change, so nothing was changed. Do not try it again; \
                          say what you would have done and ask how they want to proceed."),
                ),
                Decision::TimedOut => (
                    Some("no answer in time"),
                    Some("The user did not answer in time, so the change was NOT made and cannot be made any more. \
                          Do not try it again unless they ask; say what you would have done."),
                ),
                Decision::Stopped => (Some("stopped"), Some("The request was stopped, so nothing was changed.")),
            };
            if let Some(text) = text {
                act(ActState::Rejected, why.map(str::to_string));
                reply(&Err(text.into()));
                return;
            }
        }
        act(ActState::Running, None);
        let tc = Ctx {
            text: ctx.text,
            host: ctx.host,
            name: &ctx.req.name,
            current_page: *page,
            selection: ctx.req.selection.as_ref(),
        };
        let out = catch_unwind(AssertUnwindSafe(|| (tool.run)(&tc, &args)))
            .unwrap_or_else(|_| Err("The tool failed unexpectedly.".into()));
        match &out {
            Ok(o) => {
                session.state.lock().unwrap().sent.extend(o.seen.iter().cloned());
                if let Some(ui) = &o.ui {
                    if let Some(p) = ui.page() {
                        *page = p;
                    }
                    sink(ChatEvent::Ui { action: ui.clone() });
                }
                act(ActState::Done, None);
            }
            Err(e) => act(ActState::Failed, Some(e.clone())),
        }
        reply(&out);
    }

    /// Waits for the user's Apply / Reject. A cancel, a closed tab and the timeout each end it, and
    /// the call is final then: the pending entry is removed, so a late Apply finds nothing to answer.
    fn await_decision(&self, session: &Session, call_id: &str, rx: Receiver<bool>) -> Decision {
        let deadline = Instant::now() + Duration::from_millis(self.approval_ms.load(Ordering::SeqCst));
        let decision = loop {
            match rx.recv_timeout(Duration::from_millis(150)) {
                Ok(true) => break Decision::Apply,
                Ok(false) => break Decision::Reject,
                Err(RecvTimeoutError::Timeout) => {
                    if session.cancel.load(Ordering::SeqCst) || session.closed.load(Ordering::SeqCst) {
                        break Decision::Stopped;
                    }
                    if Instant::now() > deadline {
                        break Decision::TimedOut;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break Decision::Stopped,
            }
        };
        // `decide` takes the sender out of this map: once it is gone (or the receiver dropped), a
        // late answer is a no-op. Removing under the lock settles a decide racing the timeout.
        let mut pending = session.approvals.lock().unwrap();
        pending.remove(call_id);
        // an Apply that slipped in between the last poll and the removal came too late: still final
        decision
    }
}

/// What a turn's tool calls work against.
struct TurnCtx<'a> {
    req: &'a AskRequest,
    text: &'a DocText,
    host: &'a dyn DocHost,
}

/// Title and outline for the first message of a thread.
fn header_text(host: &dyn DocHost) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    if let Some((_, t)) = host.meta().into_iter().find(|(k, _)| *k == "title") {
        let _ = writeln!(s, "Title: {t}");
    }
    if let Ok(items) = host.outline() {
        if !items.is_empty() {
            let (lines, cut) = tools::outline_lines(&items, HEADER_OUTLINE_LINES);
            let _ = write!(s, "Outline:\n{lines}{}", if cut { "(more entries: use get_outline)\n" } else { "" });
        }
    }
    s
}

enum Streamed {
    Completed { text: String, first_ms: Option<u64>, usage: Option<Value> },
    Interrupted,
}

/// Stops listening to a thread and tells the server to drop it (best effort, off the caller's thread).
fn release_thread(client: &Arc<Client>, thread: String) {
    client.unsubscribe(&thread);
    let client = client.clone();
    std::thread::spawn(move || {
        let _ = client.call("thread/unsubscribe", json!({ "threadId": thread }), Duration::from_secs(5));
    });
}

fn interrupt(s: &Session) {
    let a = s.active.lock().unwrap().clone();
    let kids = std::mem::take(&mut *s.children.lock().unwrap());
    if let Some((client, thread, turn)) = a {
        std::thread::spawn(move || {
            if let Some(turn) = turn {
                let _ = client.call("turn/interrupt", json!({ "threadId": thread, "turnId": turn }), Duration::from_secs(10));
            }
            for k in kids {
                let _ = client.call("thread/unsubscribe", json!({ "threadId": k }), Duration::from_secs(5));
            }
        });
    }
}

fn turn_failure(err: Option<&Value>) -> Failure {
    let msg = err.and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("The request failed.").to_string();
    let info = err.and_then(|e| e.get("codexErrorInfo")).and_then(Value::as_str).unwrap_or_default();
    match info {
        "unauthorized" => Failure {
            message: "You are not signed in to ChatGPT. Run `codex login` in a terminal, then try again.".into(),
            hint: Some("login"),
        },
        "usageLimitExceeded" | "rateLimitExceeded" => Failure {
            message: format!("ChatGPT usage limit reached. {msg}"),
            hint: Some("limit"),
        },
        "contextWindowExceeded" => Failure { message: "The document excerpts were too large for the model.".into(), hint: None },
        _ => Failure { message: msg, hint: None },
    }
}

fn model_info(m: &Value) -> Option<ModelInfo> {
    if m.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let id = m.get("model").or_else(|| m.get("id")).and_then(Value::as_str)?.to_string();
    Some(ModelInfo {
        name: m.get("displayName").and_then(Value::as_str).unwrap_or(&id).to_string(),
        efforts: m
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|e| e.get("reasoningEffort").and_then(Value::as_str).map(str::to_string)).collect())
            .unwrap_or_default(),
        default_effort: m.get("defaultReasoningEffort").and_then(Value::as_str).map(str::to_string),
        is_default: m.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
        id,
    })
}

#[cfg(test)]
mod tests;
