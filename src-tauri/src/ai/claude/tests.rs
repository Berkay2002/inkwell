//! The Claude provider against `fake_claude.mjs` (Node), which speaks the stream-json protocol of
//! Claude Code 2.1.296 and calls Inkwell's tools over the MCP server like the real CLI does. The
//! real CLI is only run by hand (see docs/ai-claude.md).

use super::*;
use crate::ai::chat::ActState;
use crate::ai::text::{Chunk, Line, SelAction};
use crate::ai::tools::{Class, Output};
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::sync::mpsc;

const DOC: DocId = 7;

fn script() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/src/ai/claude/fake_claude.mjs")
}

/// What a clean user environment looks like to the child, plus `extra`.
fn env_with(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = ["PATH", "Path", "SystemRoot", "windir", "TEMP", "TMP", "HOME", "USERPROFILE"]
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|val| (k.to_string(), val)))
        .collect();
    v.extend(extra.iter().map(|(k, val)| (k.to_string(), val.to_string())));
    v
}

fn log_file(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("inkwell-fake-claude-{}-{tag}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(format!("{}.trace", p.display()));
    p
}

struct Rig {
    p: Arc<ClaudeProvider>,
    log: PathBuf,
}

fn rig_with(mode: &str, idle: Duration, extra_env: &[(&str, &str)]) -> Rig {
    static N: AtomicU64 = AtomicU64::new(0);
    let log = log_file(&N.fetch_add(1, Ordering::SeqCst).to_string());
    let mut extra: Vec<(&str, &str)> = vec![("FAKE_CLAUDE_LOG", log.to_str().unwrap())];
    extra.extend_from_slice(extra_env);
    let launch = Launch { program: "node".into(), args: vec![script().to_string(), mode.to_string()] };
    let tools = Arc::new(RwLock::new(tools::builtin()));
    Rig { p: ClaudeProvider::with_launch(tools, launch, env_with(&extra), idle), log }
}

fn rig(mode: &str) -> Rig {
    rig_with(mode, Duration::from_secs(15), &[])
}

impl Rig {
    /// One entry per start of the fake: its arguments and the names of its environment.
    fn spawns(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// The turn-level spawns (not the `--version` / `auth status` / model probes).
    fn chats(&self) -> Vec<Value> {
        self.spawns()
            .into_iter()
            .filter(|s| s["args"].as_array().is_some_and(|a| a.iter().any(|x| x == "--input-format")))
            .filter(|s| s["args"].as_array().is_some_and(|a| a.iter().any(|x| x == "--mcp-config")))
            .collect()
    }

    fn stderr(&self) -> String {
        let s = self.p.session(DOC);
        let st = s.state.lock().unwrap();
        st.running.as_ref().map(|r| r.proc.stderr_tail()).unwrap_or_default()
    }

    fn pid(&self) -> Option<u32> {
        self.p.session(DOC).state.lock().unwrap().running.as_ref().map(|r| r.proc.pid)
    }

    fn alive(&self) -> bool {
        self.p.session(DOC).state.lock().unwrap().running.as_ref().is_some_and(|r| r.proc.is_alive())
    }

    fn register_edit_tools(&self) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let make = |name: &str, class: Class| {
            let hits = Arc::new(AtomicUsize::new(0));
            let h = hits.clone();
            self.p.tools.write().unwrap().push(Arc::new(Tool {
                name: name.into(),
                description: "test only".into(),
                schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
                class,
                label: |_| "Change the test document".into(),
                run: Arc::new(move |_, _| {
                    h.fetch_add(1, Ordering::SeqCst);
                    Ok(Output::text("changed"))
                }),
                check: None,
            }));
            hits
        };
        (make("test_edit", Class::Edit), make("test_destroy", Class::Destructive))
    }

    fn start(&self, req: AskRequest, text: Arc<DocText>) -> mpsc::Receiver<ChatEvent> {
        let (tx, rx) = mpsc::channel();
        let sink: Sink = Arc::new(move |e| {
            let _ = tx.send(e);
        });
        self.p.clone().ask(DOC, req, Box::new(move || Ok(text)), Arc::new(FakeHost), sink).unwrap();
        rx
    }

    fn run(&self, req: AskRequest, text: Arc<DocText>) -> Vec<ChatEvent> {
        let ev = finish(&self.start(req, text));
        self.explain_error(&ev);
        ev
    }

    /// A turn that ended in an error says what the fake saw (its `.trace` file) in the test output.
    fn explain_error(&self, ev: &[ChatEvent]) {
        if let Some(ChatEvent::Error { message, .. }) = ev.last() {
            let trace = std::fs::read_to_string(format!("{}.trace", self.log.display())).unwrap_or_default();
            eprintln!("turn failed with {message:?}; the fake's trace:\n{trace}");
        }
    }

    /// Like `finish`, answering every Apply / Reject card with `allow(tool)`.
    fn finish_deciding(&self, rx: &mpsc::Receiver<ChatEvent>, allow: impl Fn(&str) -> bool) -> Vec<ChatEvent> {
        let mut v = Vec::new();
        loop {
            let e = rx.recv_timeout(Duration::from_secs(30)).expect("an event within 30 s");
            if let ChatEvent::Activity { id, tool, state: ActState::Waiting, .. } = &e {
                self.p.decide(DOC, id, allow(tool));
            }
            let last = is_last(&e);
            v.push(e);
            if last {
                self.explain_error(&v);
                return v;
            }
        }
    }
}

fn is_last(e: &ChatEvent) -> bool {
    matches!(e, ChatEvent::Done { .. } | ChatEvent::Error { .. } | ChatEvent::Cancelled)
}

fn finish(rx: &mpsc::Receiver<ChatEvent>) -> Vec<ChatEvent> {
    let mut v = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("an event within 30 s");
        let last = is_last(&e);
        v.push(e);
        if last {
            return v;
        }
    }
}

fn first_delta(rx: &mpsc::Receiver<ChatEvent>) {
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).expect("a delta") {
            ChatEvent::Delta { .. } => return,
            ChatEvent::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
}

fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..150 {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

fn deltas(ev: &[ChatEvent]) -> String {
    ev.iter()
        .filter_map(|e| match e {
            ChatEvent::Delta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn states(ev: &[ChatEvent], tool: &str) -> Vec<ActState> {
    ev.iter()
        .filter_map(|e| match e {
            ChatEvent::Activity { tool: t, state, .. } if t == tool => Some(*state),
            _ => None,
        })
        .collect()
}

fn done(ev: &[ChatEvent]) -> &ChatEvent {
    let e = ev.last().unwrap();
    assert!(matches!(e, ChatEvent::Done { .. }), "{e:?}");
    e
}

fn chunk(id: &str, page: u32, text: &str) -> Chunk {
    Chunk {
        id: id.into(),
        page,
        text: text.into(),
        lines: vec![Line { b: [50.0, 100.0, 500.0, 112.0], t: text.into() }],
    }
}

fn contract() -> Arc<DocText> {
    let chunks = vec![
        chunk("p1.1", 0, "Either party may terminate this agreement with thirty days written notice to the other party."),
        chunk("p2.1", 1, "The supplier shall deliver the goods within sixty days of the signing of this agreement."),
    ];
    let chars = chunks.iter().map(|c| c.text.len()).sum();
    Arc::new(DocText { rev: 0, pages: 2, chunks, chars })
}

fn ask_req(q: &str) -> AskRequest {
    AskRequest {
        task: Task::Ask { question: q.into() },
        name: "contract.pdf".into(),
        model: None,
        effort: Some("low".into()),
        current_page: Some(0),
        selection: None,
        permissions: Mode::Ask,
    }
}

fn select_req() -> AskRequest {
    AskRequest {
        task: Task::Selection { action: SelAction::Explain, text: "thirty days".into(), page: 0, style: None },
        ..ask_req("")
    }
}

struct FakeHost;

impl DocHost for FakeHost {
    fn meta(&self) -> Vec<(&'static str, String)> {
        vec![("title", "Master Services Agreement".into())]
    }
    fn outline(&self) -> Result<Vec<crate::engine::OutlineItem>, String> {
        Ok(vec![crate::engine::OutlineItem { title: "Termination".into(), page: Some(0), uri: None, children: Vec::new() }])
    }
    fn page_png(&self, _: u32, _: u32, _: u32) -> Result<Vec<u8>, String> {
        Ok(vec![0x89, b'P', b'N', b'G'])
    }
}

fn args_of(spawn: &Value) -> Vec<String> {
    spawn["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect()
}

/// The value of `flag`, written `--flag value` or `--flag=value`.
fn after(args: &[String], flag: &str) -> Option<String> {
    let eq = format!("{flag}=");
    if let Some(v) = args.iter().find_map(|a| a.strip_prefix(&eq)) {
        return Some(v.to_string());
    }
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

const SESSION: &str = "fa4e0001-0000-4000-8000-000000000001";

// ---- answers ---------------------------------------------------------------------------------------

#[test]
fn streams_the_answer_and_resolves_the_citation() {
    let r = rig("ok");
    let ev = r.run(ask_req("How do I end the agreement?"), contract());
    let ChatEvent::Done { answer, citations, first_token_ms, usage, chunks_sent, chunks_total, .. } = done(&ev) else {
        unreachable!()
    };
    assert_eq!(&deltas(&ev), answer, "the deltas, concatenated, are exactly the answer");
    assert!(answer.contains("\"quoted\" line, café 😀 and a \\ backslash [1]"), "{answer}");
    assert!(ev.iter().filter(|e| matches!(e, ChatEvent::Delta { .. })).count() > 3, "must stream in pieces");
    assert!(first_token_ms.is_some());
    let u = usage.as_ref().unwrap();
    assert_eq!((u["inputTokens"].as_u64(), u["outputTokens"].as_u64(), u["turns"].as_u64()), (Some(100), Some(20), Some(1)));
    assert_eq!(u["cacheReadInputTokens"], 7);
    assert_eq!((*chunks_sent, *chunks_total), (2, 2));
    assert_eq!((citations[0].n, citations[0].page, citations[0].exact), (1, Some(0), true));
    assert_eq!(citations[0].rects, vec![[50.0, 100.0, 500.0, 112.0]]);
    let err = r.stderr();
    assert!(err.contains("effort=low") && err.contains("excerpts=2") && err.contains("first=true"), "{err}");
}

#[test]
fn a_selection_task_answers_in_plain_markdown() {
    let r = rig("ok");
    let ev = r.run(select_req(), contract());
    let ChatEvent::Done { answer, citations, .. } = done(&ev) else { unreachable!() };
    assert!(answer.starts_with("Plain **markdown** reply."), "{answer}");
    assert!(citations.is_empty());
    assert_eq!(&deltas(&ev), answer);
}

#[test]
fn a_follow_up_reuses_the_child_and_sends_no_excerpt_twice() {
    let r = rig("ok");
    done(&r.run(ask_req("one?"), contract()));
    let pid = r.pid();
    done(&r.run(ask_req("two?"), contract()));
    assert_eq!(r.pid(), pid, "same child");
    assert_eq!(r.chats().len(), 1);
    let err = r.stderr();
    assert!(err.contains("turn 1") && err.contains("excerpts=2"), "{err}");
    assert!(err.contains("turn 2") && err.contains("excerpts=0") && err.contains("first=false"), "{err}");
}

#[test]
fn a_new_model_restarts_the_child_and_resumes_the_conversation() {
    let r = rig("ok");
    done(&r.run(ask_req("one?"), contract()));
    let mut req = ask_req("two?");
    req.model = Some("sonnet".into());
    done(&r.run(req, contract()));
    let chats = r.chats();
    assert_eq!(chats.len(), 2, "{chats:?}");
    let (a, b) = (args_of(&chats[0]), args_of(&chats[1]));
    assert!(!a.contains(&"--resume".to_string()), "{a:?}");
    assert_eq!(after(&b, "--resume").as_deref(), Some(SESSION), "{b:?}");
    assert_eq!(after(&b, "--model").as_deref(), Some("sonnet"));
    // values are attached to their flag, so none of them can be taken for another option
    assert!(b.iter().any(|a| a == "--model=sonnet") && b.iter().any(|a| *a == format!("--resume={SESSION}")), "{b:?}");
    assert!(!b.iter().any(|a| a == "--model" || a == "--resume" || a == "--effort"), "{b:?}");
    // the model still has the excerpts: the resumed conversation is not sent them again
    assert!(r.stderr().contains("excerpts=0"), "{}", r.stderr());
}

fn model(id: &str) -> Result<Option<String>, String> {
    accepted_model(id, &fallback_models())
}

#[test]
fn only_a_model_id_that_cannot_be_an_option_reaches_the_command_line() {
    assert_eq!(model(""), Ok(None));
    assert_eq!(model("default"), Ok(None));
    for ok in ["sonnet", "haiku", "opus", "claude-opus-4-1", "claude-haiku-5-5", "sonnet[1m]", "opus.4_5"] {
        assert_eq!(model(ok), Ok(Some(ok.to_string())), "{ok}");
    }
    for bad in [
        "--bare",
        "-p",
        "--model",
        "--dangerously-skip-permissions",
        "sonnet --bare",
        "sonnet\n--bare",
        "so\"net",
        " sonnet",
        "--setting-sources=local",
        "[1m]",
        &"a".repeat(101),
    ] {
        assert!(model(bad).is_err(), "{bad:?} must be refused");
    }
    // an id of the CLI's own list is accepted, but still never one that starts with a dash
    let one = |id: &str| vec![ModelInfo { id: id.into(), name: "x".into(), efforts: vec![], default_effort: None, is_default: false }];
    assert_eq!(accepted_model("my.model:1", &one("my.model:1")), Ok(Some("my.model:1".into())));
    assert!(accepted_model("--bare", &one("--bare")).is_err());
}

#[test]
fn a_model_id_from_the_webview_is_refused_before_anything_starts() {
    let r = rig("ok");
    let mut req = ask_req("one?");
    req.model = Some("--bare".into());
    let ev = r.run(req, contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, .. } => assert!(message.contains("not one Claude Code offers"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(r.chats().is_empty(), "no child was started: {:?}", r.chats());
}

#[test]
fn a_conversation_id_that_is_not_a_uuid_is_not_resumed() {
    let r = rig("ok");
    r.p.session(DOC).state.lock().unwrap().session_id = Some("--bare".into());
    done(&r.run(ask_req("one?"), contract()));
    let a = args_of(&r.chats()[0]);
    assert!(!a.iter().any(|x| x.starts_with("--resume") || x == "--bare"), "{a:?}");
}

#[test]
fn a_program_that_is_not_claude_code_is_never_chosen() {
    let dir = std::env::temp_dir().join(format!("inkwell-choose-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let r = rig("ok");
    // named like the CLI but not it: whatever it prints for --version, that is not Claude Code
    let named = dir.join(if cfg!(windows) { "claude.exe" } else { "claude" });
    std::fs::copy(std::env::current_exe().unwrap(), &named).unwrap();
    let e = r.p.choose_executable(&named).unwrap_err();
    assert!(e.contains("not Claude Code") || e.contains("did not start"), "{e}");
    // a name that is not the CLI's is refused before it is started
    let other = dir.join(if cfg!(windows) { "notepad.exe" } else { "bash" });
    std::fs::copy(std::env::current_exe().unwrap(), &other).unwrap();
    assert!(r.p.choose_executable(&other).unwrap_err().contains("must be named"));
    assert!(r.p.choose_executable(Path::new("claude.exe")).is_err(), "a relative path");
    assert!(r.p.choose_executable(Path::new(r"\\evil\share\claude.exe")).is_err(), "a network path");
    assert_eq!(r.p.executable(), None, "nothing was chosen");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An edit (the assistant's own included) must not make it forget the conversation: the same child
/// goes on, its excerpts are dropped from the books and the next question says to read again.
#[test]
fn an_edited_document_keeps_the_conversation_and_says_the_document_changed() {
    let r = rig("ok");
    done(&r.run(ask_req("one?"), contract()));
    let pid = r.pid();
    let mut t = contract();
    Arc::get_mut(&mut t).unwrap().rev = 1;
    done(&r.run(ask_req("two?"), t.clone()));
    assert_eq!((r.pid(), r.chats().len()), (pid, 1), "same child");
    let err = r.stderr();
    assert!(err.contains("turn 1") && err.contains("changed=false"), "{err}");
    let second = err.lines().find(|l| l.contains("turn 2")).unwrap();
    assert!(second.contains("changed=true") && second.contains("excerpts=0"), "{second}");
    done(&r.run(ask_req("three?"), t));
    let third = r.stderr().lines().find(|l| l.contains("turn 3")).unwrap().to_string();
    assert!(third.contains("changed=false"), "said once: {third}");
}

// ---- what the child is given -------------------------------------------------------------------------

#[test]
fn the_conversation_is_compacted_by_the_cli_at_100k_when_it_knows_the_flag() {
    let r = rig("ok");
    done(&r.run(ask_req("hi?"), contract()));
    let a = args_of(&r.chats()[0]);
    assert_eq!(after(&a, "--autocompact").as_deref(), Some("100k"), "{a:?}");
    // asked once per executable, not once per child
    let helps = r.spawns().iter().filter(|s| s["args"][0] == "--help").count();
    assert_eq!(helps, 1);
}

#[test]
fn an_older_cli_without_the_flag_is_started_without_it() {
    let r = rig_with("ok", Duration::from_secs(15), &[("FAKE_CLAUDE_OLD", "1")]);
    done(&r.run(ask_req("hi?"), contract()));
    let a = args_of(&r.chats()[0]);
    assert!(!a.contains(&"--autocompact".to_string()), "{a:?}");
}

#[test]
fn the_child_has_no_tools_of_its_own_no_settings_and_no_credentials_on_its_command_line() {
    let r = rig("ok");
    done(&r.run(ask_req("hi?"), contract()));
    let a = args_of(&r.chats()[0]);
    assert_eq!(after(&a, "--tools").as_deref(), Some(""), "{a:?}");
    assert!(a.contains(&"--strict-mcp-config".to_string()));
    assert_eq!(after(&a, "--setting-sources").as_deref(), Some(""));
    assert_eq!(after(&a, "--allowedTools").as_deref(), Some("mcp__inkwell__*"));
    assert!(a.contains(&"--disable-slash-commands".to_string()));
    assert!(!a.contains(&"--bare".to_string()), "never --bare");
    assert!(after(&a, "--system-prompt").unwrap().contains("Do not narrate your tool use"));
    let cfg = after(&a, "--mcp-config").unwrap();
    assert!(std::path::Path::new(&cfg).is_absolute() && cfg.ends_with("mcp.json"), "{cfg}");
    // the bearer token is in the private file, never in an argument
    let body = std::fs::read_to_string(&cfg).unwrap();
    let token = body.split("Bearer ").nth(1).unwrap().split('"').next().unwrap().to_string();
    assert_eq!(token.len(), 64, "{body}");
    assert!(a.iter().all(|x| !x.contains(&token)), "{a:?}");
    assert!(body.contains("127.0.0.1"));
    // the working directory is the child's own private folder, not the document's
    let cwd = spawn_cwd(&r.chats()[0]);
    let name = Path::new(&cwd).file_name().unwrap().to_str().unwrap();
    let hex = name.strip_prefix("inkwell-claude-").unwrap_or_else(|| panic!("{cwd}"));
    assert!(hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()), "an unguessable name: {name}");
    assert_eq!(Path::new(&cfg).parent().unwrap().file_name().unwrap(), name, "mcp.json is in that folder");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &str| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&cwd), mode(&cfg)), (0o700, 0o600), "only the owner can enter or read");
    }
}

fn spawn_cwd(s: &Value) -> String {
    s["cwd"].as_str().unwrap_or_default().to_string()
}

#[test]
fn claude_session_and_credential_variables_never_reach_the_child() {
    let r = rig_with(
        "ok",
        Duration::from_secs(15),
        &[
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "outer"),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "x"),
            ("ANTHROPIC_MODEL", "something"),
            ("CLAUDE_CONFIG_DIR", "D:\\cfg"),
            ("UNRELATED", "kept"),
        ],
    );
    done(&r.run(ask_req("hi?"), contract()));
    let env: Vec<String> = r.chats()[0]["env"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()).collect();
    for gone in ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_MESSAGING_TOKEN", "ANTHROPIC_MODEL"] {
        assert!(!env.iter().any(|k| k.eq_ignore_ascii_case(gone)), "{gone} leaked: {env:?}");
    }
    assert!(env.contains(&"CLAUDE_CONFIG_DIR".to_string()), "the user's own config dir is kept");
    // the environment is an allowlist: a variable nobody named stays out, one the program needs stays in
    assert!(!env.iter().any(|k| k.eq_ignore_ascii_case("UNRELATED")), "UNRELATED leaked: {env:?}");
    assert!(env.iter().any(|k| k.eq_ignore_ascii_case("PATH")), "PATH is allowlisted: {env:?}");
    assert!(env.contains(&"MCP_TOOL_TIMEOUT".to_string()));
}

#[test]
fn a_forced_api_key_or_token_is_refused_and_nothing_is_started() {
    for var in ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_OAUTH_TOKEN"] {
        let r = rig_with("ok", Duration::from_secs(15), &[(var, "sk-very-secret")]);
        let ev = r.run(ask_req("hi?"), contract());
        match ev.last().unwrap() {
            ChatEvent::Error { message, hint } => {
                assert_eq!(hint.as_deref(), Some("env"));
                assert!(message.contains(var), "{message}");
                assert!(!message.contains("sk-very-secret"), "the value must never be printed");
            }
            e => panic!("{var}: {e:?}"),
        }
        assert!(r.spawns().is_empty(), "{var}: no child may start");
        let st = r.p.status();
        assert!(st.installed && !st.logged_in);
        assert!(st.error.as_deref().is_some_and(|e| e.contains(var) && !e.contains("sk-very-secret")), "{st:?}");
        assert!(r.spawns().is_empty(), "status must not start it either");
    }
}

#[test]
fn an_init_event_that_names_an_api_key_source_stops_the_turn() {
    let r = rig("apikey");
    let ev = r.run(ask_req("hi?"), contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, hint } => {
            assert_eq!(hint.as_deref(), Some("env"));
            assert!(message.contains("API key"), "{message}");
        }
        e => panic!("{e:?}"),
    }
}

// ---- status ---------------------------------------------------------------------------------------

#[test]
fn status_reports_version_account_plan_and_the_clis_own_model_list() {
    let r = rig("ok");
    let st = r.p.status();
    assert!(st.installed && st.logged_in, "{st:?}");
    assert_eq!(st.version.as_deref(), Some("9.9.9 (Fake Claude Code)"));
    assert_eq!((st.account.as_deref(), st.plan.as_deref()), (Some("tester@example.com"), Some("max")));
    let ids: Vec<&str> = st.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["default", "sonnet", "claude-haiku-4-5-20251001"]);
    assert!(st.models[0].is_default && !st.models[1].is_default);
    assert_eq!(st.models[1].efforts, ["low", "medium", "high"]);
    assert_eq!(st.models[0].name, "Default (recommended)");
}

#[test]
fn signed_out_says_to_run_claude_login() {
    let r = rig("loggedout");
    let st = r.p.status();
    assert!(st.installed && !st.logged_in && st.models.is_empty(), "{st:?}");
    assert!(st.error.is_none(), "signed out is a state, not an error: {st:?}");
    let ev = r.run(ask_req("hi?"), contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, hint } => {
            assert_eq!(hint.as_deref(), Some("login"));
            assert!(message.contains("claude login"), "{message}");
        }
        e => panic!("{e:?}"),
    }
}

#[test]
fn not_installed_is_reported_without_starting_anything() {
    let tools = Arc::new(RwLock::new(tools::builtin()));
    let find: FindCli = Box::new(|_| None);
    let p = Arc::new(ClaudeProvider::build(tools, find, Box::new(Vec::new), Vec::new(), Duration::from_secs(5)));
    let st = p.status();
    assert!(!st.installed && !st.logged_in, "{st:?}");
    assert!(st.error.as_deref().is_some_and(|e| e.contains("not installed")), "{st:?}");
    let (tx, rx) = mpsc::channel();
    let sink: Sink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    p.ask(DOC, ask_req("hi?"), Box::new(|| Ok(contract())), Arc::new(FakeHost), sink).unwrap();
    match finish(&rx).last().unwrap() {
        ChatEvent::Error { hint, .. } => assert_eq!(hint.as_deref(), Some("install")),
        e => panic!("{e:?}"),
    }
}

#[test]
fn a_usage_limit_is_reported_as_one() {
    let r = rig("limit");
    match r.run(ask_req("hi?"), contract()).last().unwrap() {
        ChatEvent::Error { message, hint } => {
            assert_eq!(hint.as_deref(), Some("limit"));
            assert!(message.contains("usage limit"), "{message}");
        }
        e => panic!("{e:?}"),
    }
}

// ---- tools through the MCP server -------------------------------------------------------------------

fn tool_rig(script: &str) -> Rig {
    rig(&format!("tools={script}"))
}

#[test]
fn the_whole_registry_is_served_and_a_read_tool_answers_through_it() {
    let r = tool_rig(r#"[{"tool":"get_text","arguments":{"pages":[2]}}]"#);
    r.register_edit_tools();
    r.p.set_inline_max(10); // header only: the text has to come through the tool
    let ev = r.run(ask_req("When is delivery?"), contract());
    let ChatEvent::Done { answer, citations, chunks_sent, usage, .. } = done(&ev) else { unreachable!() };
    assert_eq!(&deltas(&ev), answer, "a remark before a tool call must not reach the answer");
    assert_eq!(*chunks_sent, 1, "only the passage the tool returned");
    assert_eq!((citations[0].page, citations[0].exact), (Some(1), true));
    assert_eq!(citations[0].chunk.as_deref(), Some("p2.1"));
    assert_eq!(usage.as_ref().unwrap()["turns"], 2);
    let err = r.stderr();
    assert!(err.contains("excerpts=0"), "{err}");
    // the registry's own tools (the action tools sit between the reads and the two test tools) and the test ones
    let served = err.lines().find_map(|l| l.split("tools=").nth(1)).unwrap_or_default();
    assert!(
        served.starts_with("get_document_info,get_outline,get_text,search,get_selection,get_page_image,go_to_page,show_source,")
            && served.contains(",redact_apply,")
            && served.trim_end().ends_with("test_edit,test_destroy"),
        "every registered tool, edit and destructive ones included: {err}"
    );
    assert!(err.contains("tool-reply get_text isError=false images=0 :: <<<DOCUMENT --- page 2 --- [p2.1] The supplier"), "{err}");
    assert_eq!(states(&ev, "get_text"), [ActState::Running, ActState::Done]);
}

#[test]
fn a_remark_before_a_tool_call_in_a_plain_task_is_taken_back() {
    let r = tool_rig(r#"[{"tool":"get_text","arguments":{"pages":[1]}}]"#);
    let ev = r.run(select_req(), contract());
    let ChatEvent::Done { answer, .. } = done(&ev) else { unreachable!() };
    let at = ev.iter().position(|e| matches!(e, ChatEvent::Reset)).expect("a reset event");
    assert!(ev[..at].iter().any(|e| matches!(e, ChatEvent::Delta { text } if text.contains("Let me look"))));
    let after: String = ev[at..]
        .iter()
        .filter_map(|e| match e {
            ChatEvent::Delta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(&after, answer);
}

#[test]
fn ask_mode_waits_for_apply_and_a_rejection_changes_nothing() {
    let r = tool_rig(r#"[{"tool":"test_edit"}]"#);
    let (edits, _) = r.register_edit_tools();
    let ev = r.finish_deciding(&r.start(ask_req("go"), contract()), |_| false);
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 0);
    assert_eq!(states(&ev, "test_edit"), [ActState::Waiting, ActState::Rejected]);
    assert!(
        wait_for(|| r.stderr().contains("tool-reply test_edit isError=true images=0 :: The user rejected this change")),
        "{}",
        r.stderr()
    );
    // the same question again, this time applied
    let ev = r.finish_deciding(&r.start(ask_req("go again"), contract()), |_| true);
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 1);
    assert_eq!(states(&ev, "test_edit"), [ActState::Waiting, ActState::Running, ActState::Done]);
    assert!(wait_for(|| r.stderr().contains("tool-reply test_edit isError=false")), "{}", r.stderr());
}

#[test]
fn full_mode_runs_edits_unasked_but_destructive_ones_still_follow_the_mode() {
    let r = tool_rig(r#"[{"tool":"test_edit"},{"tool":"test_destroy"}]"#);
    let (edits, destroys) = r.register_edit_tools();
    let mut req = ask_req("go");
    req.permissions = Mode::Auto;
    let ev = r.finish_deciding(&r.start(req, contract()), |t| {
        assert_eq!(t, "test_destroy", "an edit must not ask in auto mode");
        false
    });
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 1);
    assert_eq!(destroys.load(Ordering::SeqCst), 0);
    assert_eq!(states(&ev, "test_edit"), [ActState::Running, ActState::Done]);
    assert_eq!(states(&ev, "test_destroy"), [ActState::Waiting, ActState::Rejected]);
}

#[test]
fn a_card_nobody_answers_is_a_rejection_after_the_limit() {
    let r = tool_rig(r#"[{"tool":"test_edit"}]"#);
    let (edits, _) = r.register_edit_tools();
    r.p.set_approval_timeout(Duration::from_millis(400));
    let ev = r.run(ask_req("go"), contract());
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 0);
    assert_eq!(states(&ev, "test_edit"), [ActState::Waiting, ActState::Rejected]);
    // a late Apply finds nothing to answer
    for e in &ev {
        if let ChatEvent::Activity { id, state: ActState::Waiting, .. } = e {
            r.p.decide(DOC, id, true);
        }
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(edits.load(Ordering::SeqCst), 0);
}

#[test]
fn a_request_without_the_token_gets_no_tool() {
    let r = tool_rig(r#"[{"noauth":true},{"tool":"get_document_info"}]"#);
    done(&r.run(ask_req("hi?"), contract()));
    assert!(wait_for(|| r.stderr().contains("noauth status=401")), "{}", r.stderr());
}

// ---- cancel, death, close -----------------------------------------------------------------------------

#[test]
fn cancel_interrupts_the_turn_and_the_child_stays_for_the_next_question() {
    let r = rig("slow");
    let rx = r.start(ask_req("write a lot"), contract());
    first_delta(&rx);
    let pid = r.pid();
    r.p.cancel(DOC);
    let ev = finish(&rx);
    assert!(matches!(ev.last(), Some(ChatEvent::Cancelled)), "{:?}", ev.last());
    assert!(wait_for(|| !r.p.session(DOC).busy.load(Ordering::SeqCst)));
    assert!(r.stderr().contains("interrupt received"), "{}", r.stderr());
    assert!(r.alive() && r.pid() == pid, "an interrupt is not a kill");
}

#[test]
fn a_child_that_ignores_the_interrupt_is_killed() {
    let r = rig("deaf");
    r.p.set_interrupt_grace(Duration::from_millis(400));
    let rx = r.start(ask_req("write a lot"), contract());
    first_delta(&rx);
    let proc = r.p.session(DOC).state.lock().unwrap().running.as_ref().unwrap().proc.clone();
    r.p.cancel(DOC);
    let ev = finish(&rx);
    assert!(matches!(ev.last(), Some(ChatEvent::Cancelled)), "{:?}", ev.last());
    assert!(wait_for(|| !proc.is_alive()), "the child must be gone");
    assert!(r.p.session(DOC).state.lock().unwrap().running.is_none());
}

#[test]
fn closing_the_tab_ends_the_child_and_its_tool_server() {
    let r = rig("ok");
    done(&r.run(ask_req("q?"), contract()));
    let (proc, port, dir) = {
        let s = r.p.session(DOC);
        let st = s.state.lock().unwrap();
        let run = st.running.as_ref().unwrap();
        (run.proc.clone(), run.mcp.port, run.dir.clone())
    };
    assert!(dir.join("mcp.json").exists());
    r.p.close_doc(DOC);
    assert!(wait_for(|| !proc.is_alive()), "the child must be gone");
    assert!(wait_for(|| TcpStream::connect(("127.0.0.1", port)).is_err()), "the tool server must be closed");
    assert!(wait_for(|| !dir.exists()), "the private folder is removed");
    assert!(r.p.sessions.lock().unwrap().is_empty());
}

#[test]
fn child_death_is_reported_and_the_next_question_restarts_and_resumes() {
    let flag = std::env::temp_dir().join(format!("inkwell-fake-claude-die-{}", std::process::id()));
    let _ = std::fs::remove_file(&flag);
    let r = rig(&format!("die-once:{}", flag.to_str().unwrap()));
    let ev = r.run(ask_req("first?"), contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, .. } => assert!(message.contains("stopped"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(wait_for(|| r.p.session(DOC).state.lock().unwrap().running.is_none()));
    let ev = r.run(ask_req("second?"), contract());
    done(&ev);
    let chats = r.chats();
    assert_eq!(chats.len(), 2);
    assert_eq!(after(&args_of(&chats[1]), "--resume").as_deref(), Some(SESSION));
    let _ = std::fs::remove_file(&flag);
}

#[test]
fn silence_times_out_and_the_child_is_dropped() {
    let r = rig_with("hang", Duration::from_millis(700), &[]);
    let ev = r.run(ask_req("hello?"), contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, .. } => assert!(message.contains("stopped responding"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(wait_for(|| r.p.session(DOC).state.lock().unwrap().running.is_none()));
}

#[test]
fn a_slow_apply_outlives_the_silence_watchdog() {
    let r = rig_with(r#"tools=[{"tool":"test_edit"}]"#, Duration::from_millis(800), &[]);
    let (edits, _) = r.register_edit_tools();
    let rx = r.start(ask_req("go"), contract());
    let mut ev = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        if let ChatEvent::Activity { id, state: ActState::Waiting, .. } = &e {
            // far longer than the idle limit
            std::thread::sleep(Duration::from_millis(2500));
            r.p.decide(DOC, id, true);
        }
        let last = is_last(&e);
        ev.push(e);
        if last {
            break;
        }
    }
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 1);
}

// ---- the tool server on its own ----------------------------------------------------------------------

fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    c.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    let _ = c.read_to_string(&mut resp);
    let status = resp.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
    (status, body)
}

#[test]
fn the_tool_server_serves_only_a_request_with_the_token_the_right_host_and_no_origin() {
    let tools = Arc::new(RwLock::new(tools::builtin()));
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let handler: Handler = Arc::new(move |name, args, id| {
        c.fetch_add(1, Ordering::SeqCst);
        Ok(Output::text(format!("{name} {args} {}", id.unwrap_or_default())))
    });
    let s = McpServer::start(tools, handler).unwrap();
    let port = s.port;
    let host = format!("127.0.0.1:{port}");
    let bearer = format!("Bearer {}", s.token);
    let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_text","arguments":"{\"pages\":[1]}","_meta":{"claudecode/toolUseId":"toolu_9"}}}"#;
    let good = [("Host", host.as_str()), ("Authorization", bearer.as_str())];

    assert_eq!(http(port, "POST", "/mcp", &[("Host", &host)], call).0, 401, "no token");
    assert_eq!(http(port, "POST", "/mcp", &[("Host", &host), ("Authorization", "Bearer nope")], call).0, 401, "wrong token");
    assert_eq!(http(port, "POST", "/mcp", &[("Host", "evil.example"), ("Authorization", &bearer)], call).0, 403, "other host");
    assert_eq!(
        http(port, "POST", "/mcp", &[("Host", &host), ("Authorization", &bearer), ("Origin", "http://localhost:1420")], call).0,
        403,
        "a web page"
    );
    assert_eq!(http(port, "POST", "/other", &good, call).0, 404);
    assert_eq!(http(port, "GET", "/mcp", &good, "").0, 405);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing ran for a refused request");

    let (code, body) = http(port, "POST", "/mcp", &good, call);
    assert_eq!(code, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["isError"], false);
    assert_eq!(v["result"]["content"][0]["text"], r#"get_text {"pages":[1]} toolu_9"#, "string arguments are parsed");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let (_, body) = http(port, "POST", "/mcp", &good, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let v: Value = serde_json::from_str(&body).unwrap();
    let names: Vec<&str> = v["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), tools::builtin().len());
    assert!(v["result"]["tools"][0]["inputSchema"].is_object());
    // a notification is accepted and answered with nothing
    let (code, _) = http(port, "POST", "/mcp", &good, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    assert_eq!(code, 202);

    s.stop();
    assert!(wait_for(|| TcpStream::connect(("127.0.0.1", port)).is_err()));
}

#[test]
fn a_tool_result_carries_the_page_picture_and_a_failure_is_an_error_result() {
    let mut o = Output::text("page 3");
    o.image = Some(vec![0x89, b'P', b'N', b'G']);
    let v = mcp::tool_result(&Ok(o));
    assert_eq!(v["isError"], false);
    assert_eq!(v["content"][0]["text"], "page 3");
    assert_eq!((v["content"][1]["type"].as_str(), v["content"][1]["mimeType"].as_str()), (Some("image"), Some("image/png")));
    assert_eq!(v["content"][1]["data"], "iVBORw==");
    let v = mcp::tool_result(&Err("no such page".into()));
    assert_eq!((v["isError"].as_bool(), v["content"][0]["text"].as_str()), (Some(true), Some("no such page")));
}

// ---- by hand: the installed `claude` --------------------------------------------------------------------

/// `cargo test --lib ai::claude::tests::live -- --ignored --nocapture`: the user's own installed and
/// signed-in `claude`, a short synthetic document (the provider only ever sees extracted text, never
/// the file), one answer, one approved tool call, one denied, one cancel. Costs a few turns of the plan.
#[test]
#[ignore = "uses the installed Claude Code and the user's plan"]
fn live_with_the_installed_claude() {
    let tools = Arc::new(RwLock::new(tools::builtin()));
    let edits = Arc::new(AtomicUsize::new(0));
    let h = edits.clone();
    tools.write().unwrap().push(Arc::new(Tool {
        name: "add_note".into(),
        description: "Adds a short note to the document. Takes no arguments.".into(),
        schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        class: Class::Edit,
        label: |_| "Add a note to the document".into(),
        run: Arc::new(move |_, _| {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(Output::text("The note was added."))
        }),
        check: None,
    }));
    let r = Rig { p: ClaudeProvider::new(tools), log: log_file("live") };
    let st = r.p.status();
    eprintln!("status: installed={} logged_in={} version={:?} plan={:?} error={:?}", st.installed, st.logged_in, st.version, st.plan, st.error);
    eprintln!("models: {:?}", st.models.iter().map(|m| (&m.id, &m.name, &m.efforts)).collect::<Vec<_>>());
    assert!(st.installed && st.logged_in, "{st:?}");
    // the installed CLI lists `--autocompact`, so every child below is started with `--autocompact 100k`
    // (a CLI that did not accept it would exit at once and the turns would fail)
    let l = r.p.launch().expect("claude found");
    assert!(r.p.knows_autocompact(&l, &r.p.env_for_child()), "--autocompact not listed by this claude");
    eprintln!("autocompact: listed by --help, passed as {AUTOCOMPACT}");

    let summary = |what: &str, ev: &[ChatEvent]| match ev.last().unwrap() {
        ChatEvent::Done { answer, usage, first_token_ms, total_ms, chunks_sent, .. } => eprintln!(
            "{what}: done in {total_ms} ms (first token {first_token_ms:?}), {chunks_sent} excerpts sent\n  answer: {answer:?}\n  usage: {usage:?}"
        ),
        e => eprintln!("{what}: {e:?}"),
    };

    let ev = r.run(ask_req("When must the supplier deliver the goods?"), contract());
    summary("answer", &ev);
    done(&ev);

    let tool_req = |q: &str| {
        let mut q = ask_req(q);
        q.permissions = Mode::Ask;
        q
    };
    let ev = r.finish_deciding(
        &r.start(tool_req("Call the add_note tool once, then reply with the single word: added."), contract()),
        |_| true,
    );
    summary("approved", &ev);
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 1);
    assert_eq!(states(&ev, "add_note"), [ActState::Waiting, ActState::Running, ActState::Done]);

    let ev = r.finish_deciding(
        &r.start(tool_req("Call the add_note tool once, then reply with the single word: added."), contract()),
        |_| false,
    );
    summary("denied", &ev);
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 1, "a denied call changes nothing");
    assert_eq!(states(&ev, "add_note"), [ActState::Waiting, ActState::Rejected]);

    let rx = r.start(ask_req("Write a very long, detailed essay (about 1500 words) on contract law."), contract());
    first_delta(&rx);
    let pid = r.pid();
    r.p.cancel(DOC);
    let ev = finish(&rx);
    summary("cancel", &ev);
    assert!(matches!(ev.last(), Some(ChatEvent::Cancelled)), "{:?}", ev.last());
    eprintln!("cancel: child pid {pid:?} alive afterwards: {}", r.alive());
    r.p.close_doc(DOC);
}
