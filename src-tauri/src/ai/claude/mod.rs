//! Claude as the assistant: the user's own installed `claude` (Claude Code), signed in with their own
//! Claude plan. No API keys, nothing of Anthropic's is read, copied, refreshed, stored or printed by
//! Inkwell; the login stays where `claude login` put it and only the CLI ever touches it.
//!
//! One long-lived `claude -p --input-format stream-json` child per open document chat. Its own tools
//! are off (`--tools ""`) and it sees no user settings, plugins, skills or hooks; it gets Inkwell's
//! tools as an MCP server on 127.0.0.1 (`mcp.rs`, bearer token in a private config file), and every
//! call goes through the same gate as Codex's (`gate.rs`): the same permission modes, the same
//! Apply / Reject cards. What the child writes is mapped to the same `ChatEvent`s.
//!
//! Flags verified against Claude Code 2.1.296 (see docs/ai-claude.md).

mod exe;
mod mcp;
mod private;
mod proc;
#[cfg(test)]
mod tests;

use super::chat::{AskRequest, ChatEvent, ModelInfo, Sink, Status, TurnCtx, header_text};
use super::gate::{self, Gate};
use super::provider::{Loader, Provider, ProviderId};
use super::text::{self, Citation, DocText, Task};
use super::tools::{self, DocHost, Mode, Tool};
use crate::engine::DocId;
use mcp::{Handler, McpServer};
use proc::{CLOSED, Proc};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

/// No event from the child for this long (while no tool call is in flight) and it is considered hung.
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
/// An interrupt the child has not answered by now is a kill.
const INTERRUPT_GRACE: Duration = Duration::from_secs(5);
/// Same limit as Codex's approval card (see `chat.rs`).
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(240);
const MODELS_TTL: Duration = Duration::from_secs(600);
/// Efforts Claude Code accepts; anything else (a Codex leftover such as "minimal") is not passed.
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// The window after which Claude Code compacts the conversation itself (`--autocompact`, which takes
/// "auto" or 100k to 1M). Inkwell's questions are small; a long chat is cut down early instead of
/// dragging its whole history into every turn.
const AUTOCOMPACT: &str = "100k";

/// A model id that may reach the command line: the empty string and "default" mean the CLI's own
/// choice (`Ok(None)`); anything else must be an id of the CLI's own list, or plain letters, digits
/// and `. _ - [ ]` that does not start with a dash. The id comes from the webview, so it is never
/// trusted to be a value rather than an option (`--model --bare` would be both).
fn accepted_model(m: &str, known: &[ModelInfo]) -> Result<Option<String>, String> {
    if m.is_empty() || m == "default" {
        return Ok(None);
    }
    let plain = m.len() <= 100
        && m.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && m.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '[' | ']'));
    let listed = !m.starts_with('-') && !m.chars().any(|c| c.is_whitespace() || c.is_control()) && known.iter().any(|k| k.id == m);
    if plain || listed {
        Ok(Some(m.to_string()))
    } else {
        Err("That model name is not one Claude Code offers. Pick a model from the list.".into())
    }
}

/// Claude Code's conversation ids are UUIDs.
fn is_session_id(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(&parts).all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// How to start the CLI: the program, then these arguments, then Inkwell's. (A test starts `node`
/// with a script here.)
#[derive(Debug, Clone)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
}

struct Failure {
    message: String,
    hint: Option<&'static str>,
}

impl Failure {
    fn new(m: impl Into<String>) -> Self {
        Self { message: m.into(), hint: None }
    }
    fn hinted(m: impl Into<String>, hint: &'static str) -> Self {
        Self { message: m.into(), hint: Some(hint) }
    }
}

/// What the MCP handler works against: the turn in flight.
struct ActiveTurn {
    req: AskRequest,
    text: Arc<DocText>,
    host: Arc<dyn DocHost>,
    sink: Sink,
    /// The page in view; `go_to_page` and `show_source` move it within the turn.
    page: Mutex<u32>,
    /// Tool calls running or waiting for the user: the idle timer pauses while there are any.
    pending: AtomicUsize,
}

struct Running {
    proc: Arc<Proc>,
    mcp: McpServer,
    /// The private folder of this child: its working directory and its MCP config.
    dir: PathBuf,
    model: Option<String>,
    effort: Option<String>,
}

impl Running {
    fn close(self) {
        self.mcp.stop();
        self.proc.kill();
        // the token file first; the folder is the child's working directory, which Windows keeps
        // until the process is really gone, so removing it is retried off this thread
        let _ = std::fs::remove_file(self.dir.join("mcp.json"));
        let dir = self.dir.clone();
        std::thread::spawn(move || {
            for _ in 0..60 {
                if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
    }
}

#[derive(Default)]
struct State {
    running: Option<Running>,
    /// Claude Code's id of the conversation, for `--resume` after the child went away.
    session_id: Option<String>,
    /// Excerpt ids the model already has in this conversation.
    sent: HashSet<String>,
    intro_sent: bool,
    /// Text revision the conversation's excerpts came from.
    rev: u32,
    /// The document changed (an edit) since the model last read it: the next question says so.
    stale: bool,
}

impl State {
    /// An edit changes the text but not the conversation: the model keeps its memory of the chat,
    /// loses the excerpts (the ids may mean something else now) and is told to read again.
    fn note_rev(&mut self, rev: u32) {
        if self.rev != rev {
            self.stale |= self.intro_sent;
            self.sent.clear();
            self.rev = rev;
        }
    }
}

#[derive(Default)]
struct Session {
    busy: AtomicBool,
    cancel: AtomicBool,
    /// The tab is gone: nothing started for it may outlive it.
    closed: AtomicBool,
    approvals: Mutex<HashMap<String, Sender<bool>>>,
    mode: Mutex<Mode>,
    turn: Mutex<Option<Arc<ActiveTurn>>>,
    /// Serialises starting a child.
    start: Mutex<()>,
    state: Mutex<State>,
    /// The running child's output, taken by the turn that is streaming it.
    rx: Mutex<Option<Receiver<Value>>>,
    /// When an interrupt was sent for the turn in flight.
    interrupted: Mutex<Option<Instant>>,
}

/// Where the CLI is, given the user's own choice of path (the setting).
type FindCli = Box<dyn Fn(Option<&Path>) -> Option<Launch> + Send + Sync>;

pub struct ClaudeProvider {
    find: FindCli,
    environment: Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
    /// Tests only: added to the child's environment after the filter.
    extra_env: Vec<(String, String)>,
    tools: Arc<RwLock<Vec<Arc<Tool>>>>,
    approval_ms: Arc<AtomicU64>,
    inline_max: AtomicUsize,
    interrupt_grace_ms: AtomicU64,
    idle_timeout: Duration,
    sessions: Mutex<HashMap<DocId, Arc<Session>>>,
    models: Mutex<Option<(Instant, Vec<ModelInfo>)>>,
    /// The user's own choice of executable (a setting); `None` = look for it.
    chosen: Mutex<Option<PathBuf>>,
    /// Whether the executable last looked at knows `--autocompact` (an older CLI would refuse to
    /// start with it), by program path, size and modification time (an update asks again).
    autocompact: Mutex<Option<(ExeKey, bool)>>,
}

/// Which build of an executable a cached answer is about.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExeKey {
    path: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl ExeKey {
    fn of(path: &Path) -> Self {
        let meta = std::fs::metadata(path).ok();
        Self {
            path: path.to_path_buf(),
            len: meta.as_ref().map_or(0, |m| m.len()),
            modified: meta.and_then(|m| m.modified().ok()),
        }
    }
}

impl ClaudeProvider {
    /// `tools` is the registry both providers serve (`Assistant::tool_registry`).
    pub fn new(tools: Arc<RwLock<Vec<Arc<Tool>>>>) -> Arc<Self> {
        let find: FindCli =
            Box::new(|chosen| exe::resolve(chosen).map(|program| Launch { program, args: Vec::new() }));
        Arc::new(Self::build(tools, find, Box::new(real_environment), Vec::new(), IDLE_TIMEOUT))
    }

    /// For tests: starts `launch` instead of the real CLI, with `environment` as the user's. The
    /// `FAKE_CLAUDE*` entries are the fake's own settings and go to the child past the filter.
    pub fn with_launch(
        tools: Arc<RwLock<Vec<Arc<Tool>>>>,
        launch: Launch,
        environment: Vec<(String, String)>,
        idle_timeout: Duration,
    ) -> Arc<Self> {
        let find: FindCli = Box::new(move |_| Some(launch.clone()));
        let (extra, user): (Vec<_>, Vec<_>) = environment.into_iter().partition(|(k, _)| k.starts_with("FAKE_CLAUDE"));
        Arc::new(Self::build(tools, find, Box::new(move || user.clone()), extra, idle_timeout))
    }

    fn build(
        tools: Arc<RwLock<Vec<Arc<Tool>>>>,
        find: FindCli,
        environment: Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
        extra_env: Vec<(String, String)>,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            find,
            environment,
            extra_env,
            tools,
            approval_ms: Arc::new(AtomicU64::new(APPROVAL_TIMEOUT.as_millis() as u64)),
            inline_max: AtomicUsize::new(text::INLINE_CHARS),
            interrupt_grace_ms: AtomicU64::new(INTERRUPT_GRACE.as_millis() as u64),
            idle_timeout,
            sessions: Mutex::new(HashMap::new()),
            models: Mutex::new(None),
            chosen: Mutex::new(None),
            autocompact: Mutex::new(None),
        }
    }

    /// The `claude` the user picked, if any. `None` means Inkwell looks for it itself.
    pub fn executable(&self) -> Option<PathBuf> {
        self.chosen.lock().unwrap().clone()
    }

    /// Makes a file the user picked in the file dialog the `claude` to run, once it is vetted: a
    /// local file named `claude`, which answers `--version` as Claude Code. Nothing the webview
    /// sends reaches this; the pick and the check both happen in Rust.
    pub fn choose_executable(&self, picked: &Path) -> Result<PathBuf, String> {
        let program = exe::vet_choice(picked)?;
        let l = Launch { program: program.clone(), args: Vec::new() };
        let out = run_capture(&l, &["--version"], &self.env_for_child(), Duration::from_secs(10))
            .map_err(|_| "That file did not start.".to_string())?;
        if !exe::is_claude_version(&out) {
            return Err("That file is not Claude Code: it does not answer --version as Claude Code.".into());
        }
        self.set_chosen(Some(program.clone()));
        Ok(program)
    }

    /// Goes back to looking for `claude` on the PATH and in its usual folders.
    pub fn clear_executable(&self) {
        self.set_chosen(None);
    }

    /// Restores a path saved by an earlier run. It is vetted again but not started.
    pub fn restore_executable(&self, saved: &Path) -> bool {
        match exe::vet_choice(saved) {
            Ok(p) => {
                self.set_chosen(Some(p));
                true
            }
            Err(_) => false,
        }
    }

    fn set_chosen(&self, path: Option<PathBuf>) {
        *self.chosen.lock().unwrap() = path;
        *self.models.lock().unwrap() = None;
        *self.autocompact.lock().unwrap() = None;
    }

    pub fn set_approval_timeout(&self, d: Duration) {
        self.approval_ms.store(d.as_millis() as u64, Ordering::SeqCst);
    }

    /// Overrides how long an interrupt may go unanswered before the child is killed.
    pub fn set_interrupt_grace(&self, d: Duration) {
        self.interrupt_grace_ms.store(d.as_millis() as u64, Ordering::SeqCst);
    }

    /// Overrides the short-document threshold (`text::INLINE_CHARS`).
    pub fn set_inline_max(&self, chars: usize) {
        self.inline_max.store(chars, Ordering::SeqCst);
    }

    fn launch(&self) -> Option<Launch> {
        let chosen = self.chosen.lock().unwrap().clone();
        (self.find)(chosen.as_deref())
    }

    fn session(&self, doc: DocId) -> Arc<Session> {
        self.sessions.lock().unwrap().entry(doc).or_default().clone()
    }

    /// The names of the variables in the user's environment that would make Claude Code use
    /// something other than their plan.
    fn forced(&self) -> Option<String> {
        let f = exe::forcing(&(self.environment)());
        (!f.is_empty()).then(|| exe::refusal(&f))
    }

    fn env_for_child(&self) -> Vec<(String, String)> {
        let mut env = exe::child_env(&(self.environment)());
        env.extend(self.extra_env.iter().cloned());
        // The CLI's own limit for one MCP call. It must outlast the approval card, or a change could
        // run after the model was told it had failed.
        let ms = (self.approval_ms.load(Ordering::SeqCst) + 600_000).max(900_000);
        env.retain(|(k, _)| !k.eq_ignore_ascii_case("MCP_TOOL_TIMEOUT"));
        env.push(("MCP_TOOL_TIMEOUT".into(), ms.to_string()));
        env
    }

    // ---- status ---------------------------------------------------------------------------

    fn status_impl(&self) -> Status {
        let mut st = Status {
            installed: false,
            logged_in: false,
            account: None,
            plan: None,
            models: Vec::new(),
            error: None,
            version: None,
        };
        let Some(l) = self.launch() else {
            st.error = Some("Claude Code is not installed.".into());
            return st;
        };
        st.installed = true;
        if let Some(why) = self.forced() {
            st.error = Some(why);
            return st;
        }
        let env = self.env_for_child();
        st.version = run_capture(&l, &["--version"], &env, Duration::from_secs(10))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // JSON is its default output; it exits 1 when signed out and still prints `{"loggedIn": false}`
        match run_capture(&l, &["auth", "status"], &env, Duration::from_secs(15)) {
            Ok(out) => match serde_json::from_str::<Value>(out.trim()) {
                Ok(v) => {
                    st.logged_in = v.get("loggedIn").and_then(Value::as_bool).unwrap_or(false);
                    if st.logged_in {
                        st.account = v.get("email").and_then(Value::as_str).map(str::to_string);
                        st.plan = v.get("subscriptionType").and_then(Value::as_str).map(str::to_string);
                    }
                }
                Err(_) => st.error = Some("Claude Code's sign-in status could not be read.".into()),
            },
            Err(e) => st.error = Some(e),
        }
        if st.logged_in {
            st.models = self.models_for(&l, &env);
        }
        st
    }

    /// The model list (cached): asked of the CLI with a control request, which makes no API call.
    fn models_for(&self, l: &Launch, env: &[(String, String)]) -> Vec<ModelInfo> {
        if let Some((at, m)) = &*self.models.lock().unwrap() {
            if at.elapsed() < MODELS_TTL && !m.is_empty() {
                return m.clone();
            }
        }
        let found = probe_models(l, env).unwrap_or_default();
        let list = if found.is_empty() { fallback_models() } else { found };
        *self.models.lock().unwrap() = Some((Instant::now(), list.clone()));
        list
    }

    // ---- the child --------------------------------------------------------------------------

    /// Whether this CLI lists `--autocompact` in its own `--help` (asked once per executable).
    fn knows_autocompact(&self, l: &Launch, env: &[(String, String)]) -> bool {
        let key = ExeKey::of(&l.program);
        if let Some((k, ok)) = &*self.autocompact.lock().unwrap() {
            if *k == key {
                return *ok;
            }
        }
        let ok = run_capture(l, &["--help"], env, Duration::from_secs(10)).is_ok_and(|h| h.contains("--autocompact"));
        *self.autocompact.lock().unwrap() = Some((key, ok));
        ok
    }

    fn args_for(
        &self,
        cfg: &Path,
        model: Option<&str>,
        effort: Option<&str>,
        resume: Option<&str>,
        autocompact: bool,
    ) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            // none of Claude Code's own tools: it can only do what Inkwell's server offers
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        a.push(cfg.to_string_lossy().into_owned());
        for s in [
            "--allowedTools",
            "mcp__inkwell__*",
            // no user, project or local settings: no hooks, no plugins, no CLAUDE.md of the user's
            "--setting-sources",
            "",
            "--disable-slash-commands",
            // the permission mode is left at its default; anything that would still prompt is a
            // denial, never a wait nobody can answer
            "--permission-prompts",
            "none",
            "--system-prompt",
            text::BASE_INSTRUCTIONS,
        ] {
            a.push(s.to_string());
        }
        // `--flag=value`: a value can never be read as another option. The model was checked by
        // `accepted_model` and the conversation id comes from the CLI's own init event.
        if let Some(m) = model.filter(|m| !m.is_empty() && *m != "default") {
            a.push(format!("--model={m}"));
        }
        if let Some(e) = effort.filter(|e| EFFORTS.contains(e)) {
            a.push(format!("--effort={e}"));
        }
        if let Some(r) = resume.filter(|r| is_session_id(r)) {
            a.push(format!("--resume={r}"));
        }
        if autocompact {
            a.extend(["--autocompact".into(), AUTOCOMPACT.to_string()]);
        }
        a
    }

    /// The tool handler the MCP server calls: the turn in flight, through the shared gate.
    fn handler(&self, session: &Arc<Session>) -> Handler {
        let weak: Weak<Session> = Arc::downgrade(session);
        let tools = self.tools.clone();
        let approval_ms = self.approval_ms.clone();
        let counter = Arc::new(AtomicU64::new(0));
        Arc::new(move |name: &str, args: Value, tool_use: Option<String>| {
            let Some(sess) = weak.upgrade() else { return Err("The document is not open any more.".to_string()) };
            let Some(turn) = sess.turn.lock().unwrap().clone() else {
                return Err("No question is being answered right now, so nothing was run.".to_string());
            };
            let call_id = tool_use.unwrap_or_else(|| format!("call-{}", counter.fetch_add(1, Ordering::SeqCst) + 1));
            turn.pending.fetch_add(1, Ordering::SeqCst);
            struct Leave<'a>(&'a AtomicUsize);
            impl Drop for Leave<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _leave = Leave(&turn.pending);
            let mut page = *turn.page.lock().unwrap();
            let g = Gate {
                tools: &tools,
                approvals: &sess.approvals,
                mode: &sess.mode,
                cancel: &sess.cancel,
                closed: &sess.closed,
                approval_ms: approval_ms.load(Ordering::SeqCst),
            };
            let ctx = TurnCtx { req: &turn.req, text: &turn.text, host: &*turn.host };
            let out = gate::run_call(&g, &call_id, name, &args, &ctx, &mut page, &turn.sink);
            *turn.page.lock().unwrap() = page;
            if let Ok(o) = &out {
                sess.state.lock().unwrap().sent.extend(o.seen.iter().cloned());
            }
            out
        })
    }

    /// The session's child, started (or restarted) when there is none, it died, or the model or effort
    /// changed. A restart continues the conversation with `--resume`.
    fn ensure_child(
        &self,
        session: &Arc<Session>,
        model: Option<&str>,
        effort: Option<&str>,
        rev: u32,
    ) -> Result<Arc<Proc>, Failure> {
        let _start = session.start.lock().unwrap();
        if session.closed.load(Ordering::SeqCst) {
            return Err(Failure::new("The document is not open any more."));
        }
        let mut st = session.state.lock().unwrap();
        st.note_rev(rev);
        if let Some(r) = &st.running {
            if r.proc.is_alive() && r.model.as_deref() == model && r.effort.as_deref() == effort {
                return Ok(r.proc.clone());
            }
        }
        if let Some(old) = st.running.take() {
            old.close();
        }
        *session.rx.lock().unwrap() = None;
        let Some(l) = self.launch() else {
            return Err(Failure::hinted("Claude Code is not installed.", "install"));
        };
        if let Some(why) = self.forced() {
            return Err(Failure::hinted(why, "env"));
        }
        // a new folder with a random name that only the owner can enter (see `private.rs`)
        let dir = private::private_dir_in(&std::env::temp_dir())
            .map_err(|e| Failure::new(format!("Could not prepare Claude: {e}")))?;
        let handler = self.handler(session);
        let mcp = McpServer::start(self.tools.clone(), handler).map_err(|e| {
            let _ = std::fs::remove_dir_all(&dir);
            Failure::new(format!("Could not start Inkwell's tool server: {e}"))
        })?;
        // the token goes to the child through this file, never through a command line
        let cfg = dir.join("mcp.json");
        let body = json!({ "mcpServers": { "inkwell": {
            "type": "http",
            "url": mcp.url(),
            "headers": { "Authorization": format!("Bearer {}", mcp.token) },
        } } });
        if let Err(e) = private::write_new_private(&cfg, &serde_json::to_vec(&body).expect("serializable")) {
            mcp.stop();
            let _ = std::fs::remove_dir_all(&dir);
            return Err(Failure::new(format!("Could not prepare Claude: {e}")));
        }
        let mut args = l.args.clone();
        let env = self.env_for_child();
        let autocompact = self.knows_autocompact(&l, &env);
        args.extend(self.args_for(&cfg, model, effort, st.session_id.as_deref(), autocompact));
        match Proc::spawn(&l.program, &args, &env, &dir) {
            Ok((proc, rx)) => {
                *session.rx.lock().unwrap() = Some(rx);
                st.running = Some(Running {
                    proc: proc.clone(),
                    mcp,
                    dir,
                    model: model.map(str::to_string),
                    effort: effort.map(str::to_string),
                });
                Ok(proc)
            }
            Err(e) => {
                mcp.stop();
                let _ = std::fs::remove_dir_all(&dir);
                Err(Failure::new(e))
            }
        }
    }

    // ---- turns ----------------------------------------------------------------------------

    fn run_turn(
        &self,
        session: &Arc<Session>,
        req: AskRequest,
        load: Loader,
        host: Arc<dyn DocHost>,
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
        if let Some(why) = self.forced() {
            return Err(Failure::hinted(why, "env"));
        }
        let known = self.models.lock().unwrap().as_ref().map(|(_, m)| m.clone()).unwrap_or_default();
        let model = accepted_model(req.model.as_deref().unwrap_or(""), &known).map_err(Failure::new)?;
        let effort = req.effort.clone().filter(|e| !e.is_empty());
        let resumed = session.state.lock().unwrap().session_id.is_some();
        sink(ChatEvent::Status { phase: "connecting" });
        let proc = self.ensure_child(session, model.as_deref(), effort.as_deref(), text.rev)?;
        if session.cancel.load(Ordering::SeqCst) {
            return Ok(ChatEvent::Cancelled);
        }

        // What to send: the header on a conversation's first question (plus the whole text of a
        // short document); everything else the model reads through the tools.
        let selection_task = matches!(req.task, Task::Selection { .. });
        let first = !session.state.lock().unwrap().intro_sent && !selection_task;
        let inline = first && text.chars <= self.inline_max.load(Ordering::SeqCst);
        // the engine is asked outside the state lock
        let header = if first { header_text(&*host) } else { String::new() };
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
            let input = if st.stale { format!("{}\n{input}", text::CHANGED_NOTE) } else { input };
            let ids: Vec<String> = chunks.iter().map(|c| c.id.clone()).collect();
            (input, ids, st.sent.len())
        };
        let fresh = fresh_ids;

        let turn = Arc::new(ActiveTurn {
            page: Mutex::new(req.current_page.unwrap_or(0)),
            req,
            text: text.clone(),
            host,
            sink: sink.clone(),
            pending: AtomicUsize::new(0),
        });
        *session.turn.lock().unwrap() = Some(turn.clone());
        *session.interrupted.lock().unwrap() = None;
        let rx = session.rx.lock().unwrap().take().ok_or_else(|| Failure::new("Claude Code is not running. Ask again."))?;
        // anything left from an earlier turn is not part of this one
        while let Ok(m) = rx.try_recv() {
            if m.get("type").and_then(Value::as_str) == Some(CLOSED) {
                *session.rx.lock().unwrap() = Some(rx);
                *session.turn.lock().unwrap() = None;
                self.drop_child(session);
                return Err(Failure::new("Claude Code stopped unexpectedly. Ask again to restart it."));
            }
        }
        let sent_ok = proc.send(&json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": input }] },
        }));
        {
            let mut st = session.state.lock().unwrap();
            st.intro_sent |= first;
            st.sent.extend(fresh);
            st.stale = false;
        }
        let prep_ms = t0.elapsed().as_millis() as u64;
        sink(ChatEvent::Status { phase: "thinking" });
        let outcome = if sent_ok {
            self.stream(&rx, &proc, session, &turn, t0, sink)
        } else {
            Err(Failure::new("Claude Code stopped unexpectedly. Ask again to restart it."))
        };
        *session.turn.lock().unwrap() = None;
        *session.rx.lock().unwrap() = Some(rx);
        let streamed = match outcome {
            Ok(Streamed::Interrupted) => return Ok(ChatEvent::Cancelled),
            Ok(Streamed::Completed(c)) => c,
            Err(f) => {
                // after these the child may still be writing the turn we gave up on
                if matches!(f.hint, Some("dead" | "login" | "env")) {
                    self.drop_child(session);
                }
                // a resumed conversation the CLI no longer has: the next question starts a new one
                // (with its header and excerpts again)
                if resumed && f.hint == Some("dead") {
                    let mut st = session.state.lock().unwrap();
                    st.session_id = None;
                    st.sent.clear();
                    st.intro_sent = false;
                }
                return Err(f);
            }
        };

        let sent = session.state.lock().unwrap().sent.clone();
        let (answer, citations, followups): (String, Vec<Citation>, Vec<String>) =
            match text::parse_reply(&streamed.text).filter(|_| turn.req.task.cited()) {
                Some(r) => {
                    let mut cs: Vec<Citation> = r.citations.iter().map(|c| text::resolve(c, &text, &sent)).collect();
                    cs.sort_by_key(|c| c.n);
                    let f = r.followups.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(4).collect();
                    (r.answer, cs, f)
                }
                None => (streamed.text, Vec::new(), Vec::new()),
            };
        Ok(ChatEvent::Done {
            answer,
            citations,
            followups,
            first_token_ms: streamed.first_ms,
            total_ms: t0.elapsed().as_millis() as u64,
            prep_ms,
            chunks_sent: sent.len().saturating_sub(sent_before),
            chunks_total: text.chunks.len(),
            usage: streamed.usage,
        })
    }

    /// Forgets a child that is gone or no good; its conversation id is kept for `--resume`.
    fn drop_child(&self, session: &Session) {
        let old = session.state.lock().unwrap().running.take();
        if let Some(r) = old {
            r.close();
        }
        *session.rx.lock().unwrap() = None;
    }

    fn stream(
        &self,
        rx: &Receiver<Value>,
        proc: &Arc<Proc>,
        session: &Arc<Session>,
        turn: &Arc<ActiveTurn>,
        t0: Instant,
        sink: &Sink,
    ) -> Result<Streamed, Failure> {
        let cited = turn.req.task.cited();
        let cancelled = || session.cancel.load(Ordering::SeqCst);
        // text of the assistant message being written; a message that ends in a tool call is not the answer
        let mut raw = String::new();
        let mut shown = 0usize;
        let mut first_ms: Option<u64> = None;
        let mut last_event = Instant::now();
        let mut assistant_error: Option<String> = None;
        let mut limited = false;
        loop {
            if cancelled() {
                let mut at = session.interrupted.lock().unwrap();
                let sent_at = *at;
                if sent_at.is_none() {
                    proc.send(&interrupt_request());
                    *at = Some(Instant::now());
                }
                drop(at);
                // an interrupt the child does not answer is a kill; the next question starts it again
                let grace = Duration::from_millis(self.interrupt_grace_ms.load(Ordering::SeqCst));
                if sent_at.is_some_and(|t| t.elapsed() > grace) {
                    self.drop_child(session);
                    return Ok(Streamed::Interrupted);
                }
            }
            let msg = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    // a call waiting for the user (or running) is not silence, and the quiet that
                    // follows it gets the whole limit again
                    if turn.pending.load(Ordering::SeqCst) > 0 {
                        last_event = Instant::now();
                    } else if last_event.elapsed() > self.idle_timeout {
                        return Err(Failure { message: "Claude stopped responding.".into(), hint: Some("dead") });
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return if cancelled() {
                        Ok(Streamed::Interrupted)
                    } else {
                        Err(Failure {
                            message: "Claude Code stopped unexpectedly. Ask again to restart it.".into(),
                            hint: Some("dead"),
                        })
                    };
                }
            };
            last_event = Instant::now();
            let kind = msg.get("type").and_then(Value::as_str).unwrap_or_default();
            // output of a sub-agent is not this conversation's
            if !msg.get("parent_tool_use_id").map_or(true, Value::is_null) {
                continue;
            }
            match kind {
                CLOSED => {
                    if cancelled() {
                        return Ok(Streamed::Interrupted);
                    }
                    let tail = proc.stderr_tail();
                    let tail = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string();
                    let message = if tail.is_empty() {
                        "Claude Code stopped unexpectedly. Ask again to restart it.".to_string()
                    } else {
                        format!("Claude Code stopped: {tail}")
                    };
                    return Err(Failure { message, hint: Some("dead") });
                }
                "system" => {
                    if msg.get("subtype").and_then(Value::as_str) == Some("init") {
                        if let Some(id) = msg.get("session_id").and_then(Value::as_str) {
                            session.state.lock().unwrap().session_id = Some(id.to_string());
                        }
                        // OAuth reports "none"; any other source means something else would be billed
                        if let Some(src) = msg.get("apiKeySource").and_then(Value::as_str) {
                            if src != "none" && src != "oauth" {
                                return Err(Failure::hinted(
                                    "Claude Code is set to use an API key, not your Claude plan. Inkwell only uses \
                                     your plan (`claude login`), so it stopped.",
                                    "env",
                                ));
                            }
                        }
                    }
                }
                "stream_event" => {
                    let ev = &msg["event"];
                    match ev.get("type").and_then(Value::as_str) {
                        Some("message_start") => {
                            raw.clear();
                            shown = 0;
                        }
                        // a message that ends in a tool call was a remark ("let me look"), not the answer
                        Some("message_delta")
                            if ev.pointer("/delta/stop_reason").and_then(Value::as_str) == Some("tool_use") =>
                        {
                            if shown > 0 {
                                sink(ChatEvent::Reset);
                            }
                            raw.clear();
                            shown = 0;
                            first_ms = None;
                        }
                        Some("content_block_delta") if ev.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta") => {
                            raw.push_str(ev.pointer("/delta/text").and_then(Value::as_str).unwrap_or_default());
                            let visible = if cited { text::partial_answer(&raw) } else { raw.clone() };
                            if visible.len() > shown {
                                // `visible` only ever grows by appending
                                let grown = visible[shown..].to_string();
                                shown = visible.len();
                                first_ms.get_or_insert(t0.elapsed().as_millis() as u64);
                                sink(ChatEvent::Delta { text: grown });
                            }
                        }
                        _ => {}
                    }
                }
                "assistant" => {
                    if let Some(e) = msg.get("error").and_then(Value::as_str) {
                        assistant_error = Some(e.to_string());
                    }
                }
                "rate_limit_event" => {
                    limited = msg.pointer("/rate_limit_info/status").and_then(Value::as_str) == Some("rejected");
                }
                "result" => return finish(&msg, assistant_error.as_deref(), limited, cancelled(), first_ms),
                _ => {}
            }
        }
    }
}

struct Completed {
    text: String,
    first_ms: Option<u64>,
    usage: Option<Value>,
}

enum Streamed {
    Completed(Completed),
    Interrupted,
}

fn interrupt_request() -> Value {
    json!({ "type": "control_request", "request_id": "inkwell-interrupt", "request": { "subtype": "interrupt" } })
}

/// The `result` event of a turn: its answer, or why there is none.
fn finish(r: &Value, assistant_error: Option<&str>, limited: bool, cancelled: bool, first_ms: Option<u64>) -> Result<Streamed, Failure> {
    let is_error = r.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let subtype = r.get("subtype").and_then(Value::as_str).unwrap_or_default();
    let result = r.get("result").and_then(Value::as_str).unwrap_or_default();
    if cancelled && (is_error || subtype.starts_with("error")) {
        return Ok(Streamed::Interrupted);
    }
    if is_error || subtype.starts_with("error") {
        let lower = result.to_ascii_lowercase();
        if assistant_error == Some("authentication_failed") || lower.contains("not logged in") || lower.contains("/login") {
            return Err(Failure::hinted(
                "You are not signed in to Claude. Run `claude login` in a terminal, then try again.",
                "login",
            ));
        }
        if limited
            || matches!(assistant_error, Some("rate_limit"))
            || r.get("api_error_status").and_then(Value::as_i64) == Some(429)
            || lower.contains("usage limit")
            || lower.contains("rate limit")
        {
            let tail = if result.trim().is_empty() { String::new() } else { format!(" {}", result.trim()) };
            return Err(Failure::hinted(format!("Your Claude usage limit is reached.{tail}"), "limit"));
        }
        let message = if result.trim().is_empty() { "The request failed.".to_string() } else { result.trim().to_string() };
        return Err(Failure::new(message));
    }
    if result.trim().is_empty() {
        return Err(Failure::new("Claude returned an empty answer."));
    }
    let u = &r["usage"];
    let usage = json!({
        "inputTokens": u.get("input_tokens"),
        "outputTokens": u.get("output_tokens"),
        "cacheReadInputTokens": u.get("cache_read_input_tokens"),
        "cacheCreationInputTokens": u.get("cache_creation_input_tokens"),
        "turns": r.get("num_turns"),
        "durationMs": r.get("duration_ms"),
        "durationApiMs": r.get("duration_api_ms"),
    });
    Ok(Streamed::Completed(Completed { text: result.to_string(), first_ms, usage: Some(usage) }))
}

/// The user's environment as the process has it (a variable that is not valid Unicode is read lossily).
fn real_environment() -> Vec<(String, String)> {
    std::env::vars_os()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .collect()
}

// ---- small CLI calls ----------------------------------------------------------------------------

/// Runs `claude <extra>` to the end (at most `limit`) and returns its stdout. The exit code is not an
/// error: `claude auth status` exits 1 when signed out and prints its answer all the same.
fn run_capture(l: &Launch, extra: &[&str], env: &[(String, String)], limit: Duration) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(&l.program);
    cmd.args(&l.args)
        .args(extra)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let program = l.program.clone();
    std::thread::spawn(move || {
        let _ = tx.send(cmd.output().map_err(|e| format!("could not start {}: {e}", program.display())));
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(o)) => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Claude Code did not answer in time.".into()),
    }
}

/// The CLI's own model list, from a short-lived child answering the `initialize` control request.
fn probe_models(l: &Launch, env: &[(String, String)]) -> Option<Vec<ModelInfo>> {
    let dir = std::env::temp_dir();
    let mut args = l.args.clone();
    args.extend(
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--tools",
            "",
            "--strict-mcp-config",
            "--setting-sources",
            "",
            "--disable-slash-commands",
            "--no-session-persistence",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    let (proc, rx) = Proc::spawn(&l.program, &args, env, &dir).ok()?;
    proc.send(&json!({ "type": "control_request", "request_id": "inkwell-init", "request": { "subtype": "initialize" } }));
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut out = None;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(m) if m.get("type").and_then(Value::as_str) == Some("control_response") => {
                out = m.pointer("/response/response/models").and_then(Value::as_array).map(|a| a.iter().filter_map(model_info).collect());
                break;
            }
            Ok(m) if m.get("type").and_then(Value::as_str) == Some(CLOSED) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    proc.kill();
    out
}

fn model_info(m: &Value) -> Option<ModelInfo> {
    let id = m.get("value").and_then(Value::as_str)?.to_string();
    Some(ModelInfo {
        name: m.get("displayName").and_then(Value::as_str).unwrap_or(&id).to_string(),
        efforts: m
            .get("supportedEffortLevels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        default_effort: None,
        is_default: id == "default",
        id,
    })
}

/// When the CLI could not be asked: the aliases it has always accepted.
fn fallback_models() -> Vec<ModelInfo> {
    [("default", "Default"), ("sonnet", "Sonnet"), ("opus", "Opus"), ("haiku", "Haiku")]
        .iter()
        .map(|(id, name)| ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
            efforts: EFFORTS.iter().map(|e| e.to_string()).collect(),
            default_effort: None,
            is_default: *id == "default",
        })
        .collect()
}

// ---- the provider ---------------------------------------------------------------------------------

impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn status(&self) -> Status {
        self.status_impl()
    }

    /// Nothing to warm: a child costs a second to start and a model change restarts it anyway.
    fn warm(&self, _doc: DocId, _rev: u32) {}

    fn ask(
        self: Arc<Self>,
        doc: DocId,
        req: AskRequest,
        load: Loader,
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
            .name("ai-claude-turn".into())
            .spawn(move || {
                let ev = match me.run_turn(&session, req, load, host, &sink) {
                    Ok(ev) => ev,
                    Err(f) => ChatEvent::Error { message: f.message, hint: f.hint.map(str::to_string) },
                };
                *session.turn.lock().unwrap() = None;
                session.approvals.lock().unwrap().clear();
                session.busy.store(false, Ordering::SeqCst);
                sink(ev);
            })
            .map_err(|e| {
                self.session(doc).busy.store(false, Ordering::SeqCst);
                e.to_string()
            })?;
        Ok(())
    }

    fn cancel(&self, doc: DocId) {
        let Some(s) = self.sessions.lock().unwrap().get(&doc).cloned() else { return };
        s.cancel.store(true, Ordering::SeqCst);
        if s.busy.load(Ordering::SeqCst) {
            let proc = s.state.lock().unwrap().running.as_ref().map(|r| r.proc.clone());
            if let Some(p) = proc {
                let mut at = s.interrupted.lock().unwrap();
                if at.is_none() {
                    p.send(&interrupt_request());
                    *at = Some(Instant::now());
                }
            }
        }
    }

    fn decide(&self, doc: DocId, call_id: &str, allow: bool) {
        let Some(s) = self.sessions.lock().unwrap().get(&doc).cloned() else { return };
        if let Some(tx) = s.approvals.lock().unwrap().remove(call_id) {
            let _ = tx.send(allow);
        }
    }

    fn set_permissions(&self, doc: DocId, mode: Mode) {
        if let Some(s) = self.sessions.lock().unwrap().get(&doc) {
            *s.mode.lock().unwrap() = mode;
        }
    }

    /// The tab was closed: the child, its tool server and its folder go with it.
    fn close_doc(&self, doc: DocId) {
        let Some(s) = self.sessions.lock().unwrap().remove(&doc) else { return };
        s.closed.store(true, Ordering::SeqCst);
        s.cancel.store(true, Ordering::SeqCst);
        let old = s.state.lock().unwrap().running.take();
        if let Some(r) = old {
            r.close();
        }
    }
}
