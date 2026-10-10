//! One warm `codex app-server` shared by every open document; one ephemeral thread per document
//! chat. A turn = document excerpts (only those not sent before) + the task, streamed back as
//! events. The server is started lazily, restarted after a crash, and never blocks on approvals.

use super::rpc::{CLOSED, Client, RpcError, SpawnSpec};
use super::text::{self, Citation, DocText, Task};
use crate::engine::DocId;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// No notification for this long and the server is considered hung.
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);

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
    /// 0-based page in view; its text is always included when the document is sent in part.
    pub current_page: Option<u32>,
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
}

#[derive(Default)]
struct Session {
    busy: AtomicBool,
    cancel: AtomicBool,
    /// (client, thread, turn) of the turn in flight, for interrupt.
    active: Mutex<Option<(Arc<Client>, String, Option<String>)>>,
    state: Mutex<SessionState>,
}

pub struct Assistant {
    spec: Box<dyn Fn() -> Option<SpawnSpec> + Send + Sync>,
    server: Mutex<Option<Server>>,
    next_gen: AtomicU64,
    sessions: Mutex<HashMap<DocId, Arc<Session>>>,
    texts: Mutex<HashMap<DocId, Arc<DocText>>>,
    idle_timeout: Duration,
}

impl Assistant {
    pub fn new() -> Arc<Self> {
        Self::with_spec(Box::new(super::codex::spawn_spec), IDLE_TIMEOUT)
    }

    pub fn with_spec(spec: Box<dyn Fn() -> Option<SpawnSpec> + Send + Sync>, idle_timeout: Duration) -> Arc<Self> {
        Arc::new(Self {
            spec,
            server: Mutex::new(None),
            next_gen: AtomicU64::new(0),
            sessions: Mutex::new(HashMap::new()),
            texts: Mutex::new(HashMap::new()),
            idle_timeout,
        })
    }

    pub fn server_pid(&self) -> Option<u32> {
        self.server.lock().unwrap().as_ref().filter(|s| s.client.is_alive()).map(|s| s.client.pid)
    }

    fn session(&self, doc: DocId) -> Arc<Session> {
        self.sessions.lock().unwrap().entry(doc).or_default().clone()
    }

    /// The running server, started (and initialized) on first use or after it died.
    fn ensure_server(&self) -> Result<(Arc<Client>, u64), Failure> {
        let mut g = self.server.lock().unwrap();
        if let Some(s) = g.as_ref().filter(|s| s.client.is_alive()) {
            return Ok((s.client.clone(), s.epoch));
        }
        let spec = (self.spec)().ok_or_else(|| Failure {
            message: "The Codex CLI is not installed.".into(),
            hint: Some("install"),
        })?;
        let client = Client::spawn(&spec)?;
        let init = json!({ "clientInfo": { "name": "inkwell", "title": "Inkwell", "version": env!("CARGO_PKG_VERSION") } });
        let r = client.call("initialize", init, CALL_TIMEOUT).and_then(|_| client.notify("initialized", None));
        if let Err(e) = r {
            client.kill();
            return Err(e.into());
        }
        let epoch = self.next_gen.fetch_add(1, Ordering::SeqCst) + 1;
        *g = Some(Server { client: client.clone(), epoch });
        Ok((client, epoch))
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
        match client.call("account/read", json!({ "refreshToken": false }), CALL_TIMEOUT) {
            Ok(v) => {
                if let Some(a) = v.get("account").filter(|a| !a.is_null()) {
                    st.logged_in = true;
                    st.account = a.get("email").and_then(Value::as_str).map(str::to_string);
                    st.plan = a.get("planType").and_then(Value::as_str).map(str::to_string);
                }
            }
            Err(e) => st.error = Some(e.to_string()),
        }
        match client.call("model/list", json!({}), CALL_TIMEOUT) {
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
    pub fn warm(&self, doc: DocId) {
        let Ok((client, epoch)) = self.ensure_server() else { return };
        let session = self.session(doc);
        if session.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.ensure_thread(&client, epoch, &session, None);
        session.busy.store(false, Ordering::SeqCst);
    }

    pub fn close_doc(&self, doc: DocId) {
        self.texts.lock().unwrap().remove(&doc);
        let Some(s) = self.sessions.lock().unwrap().remove(&doc) else { return };
        s.cancel.store(true, Ordering::SeqCst);
        let client = self.server.lock().unwrap().as_ref().map(|s| s.client.clone());
        let thread = s.state.lock().unwrap().thread.take();
        if let (Some(client), Some((_, tid))) = (client, thread) {
            client.unsubscribe(&tid);
            // best effort, off the caller's thread: the server drops the thread now
            std::thread::spawn(move || {
                let _ = client.call("thread/unsubscribe", json!({ "threadId": tid }), Duration::from_secs(5));
            });
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
    ) -> Result<String, Failure> {
        let mut st = session.state.lock().unwrap();
        if let Some((g, id)) = &st.thread {
            if *g == epoch && client.is_alive() {
                return Ok(id.clone());
            }
        }
        let mut p = json!({
            "ephemeral": true,
            "sandbox": "read-only",
            "approvalPolicy": "never",
            "cwd": std::env::temp_dir().to_string_lossy(),
            "baseInstructions": text::BASE_INSTRUCTIONS,
        });
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            p["model"] = json!(m);
        }
        let r = client.call("thread/start", p, CALL_TIMEOUT)?;
        let id = r
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or_else(|| Failure::new("codex returned no thread id"))?
            .to_string();
        st.rx = Some(client.subscribe(&id));
        st.thread = Some((epoch, id.clone()));
        st.sent.clear();
        st.intro_sent = false;
        Ok(id)
    }

    // ---- turns ----------------------------------------------------------------------------

    /// Starts a turn on its own thread and returns at once; everything arrives through `sink`.
    /// `load` produces the document text (cached after the first call).
    pub fn ask(
        self: &Arc<Self>,
        doc: DocId,
        req: AskRequest,
        load: impl FnOnce() -> Result<Arc<DocText>, String> + Send + 'static,
        sink: Sink,
    ) -> Result<(), String> {
        let session = self.session(doc);
        if session.busy.swap(true, Ordering::SeqCst) {
            return Err("The assistant is still answering the previous question.".into());
        }
        session.cancel.store(false, Ordering::SeqCst);
        let me = self.clone();
        std::thread::Builder::new()
            .name("ai-turn".into())
            .spawn(move || {
                let ev = match me.run_turn(&session, req, load, &sink) {
                    Ok(ev) => ev,
                    Err(f) => ChatEvent::Error { message: f.message, hint: f.hint.map(str::to_string) },
                };
                *session.active.lock().unwrap() = None;
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
        let thread = self.ensure_thread(&client, epoch, session, req.model.as_deref())?;

        // what to send: the retrieval for this task minus what the model already has
        let (input, fresh_ids, chunks_total, chunks_sent) = {
            let st = session.state.lock().unwrap();
            let idx = if req.task.cited() {
                text::pick(&text, req.task.query(), &req.task.focus_pages(req.current_page), text::BUDGET_CHARS)
            } else {
                Vec::new()
            };
            let partial = idx.len() < text.chunks.len();
            let fresh: Vec<&text::Chunk> = idx.iter().map(|&i| &text.chunks[i]).filter(|c| !st.sent.contains(&c.id)).collect();
            let input = text::build_turn(&req.task, &req.name, text.pages, &fresh, !st.intro_sent, partial);
            let ids: Vec<String> = fresh.iter().map(|c| c.id.clone()).collect();
            let n = fresh.len();
            // drop notifications left over from an earlier turn
            if let Some(rx) = &st.rx {
                while rx.try_recv().is_ok() {}
            }
            (input, ids, text.chunks.len(), n)
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
        let started = client.call("turn/start", params, CALL_TIMEOUT).map_err(|e| {
            if matches!(e, RpcError::Timeout) {
                self.drop_server(&client);
            }
            Failure::from(e)
        })?;
        let turn_id = started.pointer("/turn/id").and_then(Value::as_str).unwrap_or_default().to_string();
        {
            let mut st = session.state.lock().unwrap();
            st.intro_sent = true;
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
        let outcome = self.stream(&rx, &req, &turn_id, t0, sink);
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
            chunks_sent,
            chunks_total,
            usage,
        })
    }

    fn stream(&self, rx: &Receiver<Value>, req: &AskRequest, turn_id: &str, t0: Instant, sink: &Sink) -> Result<Streamed, Failure> {
        let cited = req.task.cited();
        let mut raw = String::new();
        let mut shown = 0usize;
        let mut final_text: Option<String> = None;
        let mut first_ms = None;
        let mut usage = None;
        loop {
            let msg = match rx.recv_timeout(self.idle_timeout) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(Failure { message: "Codex stopped responding.".into(), hint: Some("dead") });
                }
                Err(RecvTimeoutError::Disconnected) => return Err(Failure { message: String::new(), hint: Some("dead") }),
            };
            let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
            let p = &msg["params"];
            // a notification of another turn of the same thread (late tail of an earlier one)
            let other_turn = |id: Option<&str>| id.is_some_and(|i| !turn_id.is_empty() && i != turn_id);
            match method {
                CLOSED => return Err(Failure { message: String::new(), hint: Some("dead") }),
                "item/agentMessage/delta" if !other_turn(p.get("turnId").and_then(Value::as_str)) => {
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
                    if let Some(t) = p.pointer("/item/text").and_then(Value::as_str) {
                        final_text = Some(t.to_string());
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
}

enum Streamed {
    Completed { text: String, first_ms: Option<u64>, usage: Option<Value> },
    Interrupted,
}

fn interrupt(s: &Session) {
    let a = s.active.lock().unwrap().clone();
    if let Some((client, thread, Some(turn))) = a {
        std::thread::spawn(move || {
            let _ = client.call("turn/interrupt", json!({ "threadId": thread, "turnId": turn }), Duration::from_secs(10));
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
