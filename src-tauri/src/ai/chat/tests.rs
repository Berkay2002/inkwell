//! Transport, streaming, errors and restart against `fake_codex.mjs` (Node), plus one live smoke
//! test against the real Codex behind `INKWELL_LIVE_CODEX=1`.

use super::*;
use crate::ai::text::{Chunk, Line, SelAction};
use crate::engine::Engine;
use crate::engine::fixture::{self, Fixture};
use std::sync::mpsc;

mod live;

const DOC: DocId = 7;

fn fake_with(mode: &str, flag: Option<&str>, idle: Duration) -> Arc<Assistant> {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ai/chat/fake_codex.mjs");
    let mut args = vec![script.to_string(), mode.to_string()];
    args.extend(flag.map(str::to_string));
    let spec = SpawnSpec { program: "node".into(), args, cwd: std::env::temp_dir() };
    Assistant::with_spec(Box::new(move || Some(spec.clone())), idle)
}

fn fake(mode: &str) -> Arc<Assistant> {
    fake_with(mode, None, Duration::from_secs(15))
}

/// Like `fake`, with a short request timeout.
fn fake_call_timeout(mode: &str, call: Duration) -> Arc<Assistant> {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ai/chat/fake_codex.mjs");
    let spec = SpawnSpec { program: "node".into(), args: vec![script.to_string(), mode.to_string()], cwd: std::env::temp_dir() };
    Assistant::with_timeouts(Box::new(move || Some(spec.clone())), Duration::from_secs(15), call)
}

fn chunk(id: &str, page: u32, text: &str) -> Chunk {
    Chunk {
        id: id.into(),
        page,
        text: text.into(),
        lines: vec![Line { b: [50.0, 100.0, 500.0, 112.0], t: text.into() }],
    }
}

fn contract_rev(rev: u32) -> Arc<DocText> {
    let chunks = vec![
        chunk("p1.1", 0, "Either party may terminate this agreement with thirty days written notice to the other party."),
        chunk("p2.1", 1, "The supplier shall deliver the goods within sixty days of the signing of this agreement."),
    ];
    let chars = chunks.iter().map(|c| c.text.len()).sum();
    Arc::new(DocText { rev, pages: 2, chunks, chars })
}

fn contract() -> Arc<DocText> {
    contract_rev(0)
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

fn start_doc(a: &Arc<Assistant>, doc: DocId, req: AskRequest, text: Arc<DocText>) -> mpsc::Receiver<ChatEvent> {
    let (tx, rx) = mpsc::channel();
    let sink: Sink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    a.ask(doc, req, move || Ok(text), Arc::new(FakeHost), sink).unwrap();
    rx
}

/// The engine side of a document without an engine: a title, a one-entry outline, a tiny "PNG".
struct FakeHost;

impl DocHost for FakeHost {
    fn meta(&self) -> Vec<(&'static str, String)> {
        vec![("title", "Master Services Agreement".into())]
    }
    fn outline(&self) -> Result<Vec<crate::engine::OutlineItem>, String> {
        Ok(vec![crate::engine::OutlineItem { title: "Termination".into(), page: Some(0), uri: None, children: Vec::new() }])
    }
    fn page_png(&self, _: u32, _: u32) -> Result<Vec<u8>, String> {
        Ok(vec![0x89, b'P', b'N', b'G'])
    }
}

fn start(a: &Arc<Assistant>, req: AskRequest, text: Arc<DocText>) -> mpsc::Receiver<ChatEvent> {
    start_doc(a, DOC, req, text)
}

/// Blocks until the answer has started streaming.
fn first_delta(rx: &mpsc::Receiver<ChatEvent>) {
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).expect("a delta") {
            ChatEvent::Delta { .. } => return,
            ChatEvent::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
}

fn finish(rx: &mpsc::Receiver<ChatEvent>) -> Vec<ChatEvent> {
    let mut v = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("an event within 30 s");
        let last = matches!(e, ChatEvent::Done { .. } | ChatEvent::Error { .. } | ChatEvent::Cancelled);
        v.push(e);
        if last {
            return v;
        }
    }
}

fn run(a: &Arc<Assistant>, req: AskRequest, text: Arc<DocText>) -> Vec<ChatEvent> {
    finish(&start(a, req, text))
}

fn stderr(a: &Assistant) -> String {
    a.server.lock().unwrap().as_ref().map(|s| s.client.stderr_tail()).unwrap_or_default()
}

fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
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

#[test]
fn streams_the_answer_and_resolves_the_citation() {
    let a = fake("ok");
    let ev = run(&a, ask_req("How do I end the agreement?"), contract());
    let ChatEvent::Done { answer, citations, followups, first_token_ms, usage, chunks_sent, chunks_total, .. } = ev.last().unwrap() else {
        panic!("not done: {:?}", ev.last());
    };
    // the deltas, concatenated, are exactly the final answer (escapes, accents, emoji, backslash)
    assert_eq!(&deltas(&ev), answer);
    assert!(answer.contains("\"quoted\" line, café 😀 and a \\ backslash [1]"), "{answer}");
    assert!(ev.iter().filter(|e| matches!(e, ChatEvent::Delta { .. })).count() > 3, "must stream in pieces");
    assert!(first_token_ms.is_some());
    assert_eq!(followups.len(), 3);
    assert_eq!(usage.as_ref().unwrap()["inputTokens"], 100);
    assert_eq!((*chunks_sent, *chunks_total), (2, 2));
    assert_eq!(citations.len(), 1);
    assert_eq!((citations[0].n, citations[0].page, citations[0].exact), (1, Some(0), true));
    assert_eq!(citations[0].rects, vec![[50.0, 100.0, 500.0, 112.0]]);
    let err = stderr(&a);
    assert!(err.contains("thread/start thr-1 sandbox=read-only approval=never ephemeral=true envs=[]"), "{err}");
    assert!(err.contains("effort=low") && err.contains("schema=yes"), "{err}");
}

#[test]
fn a_follow_up_reuses_the_thread_and_sends_no_excerpt_twice() {
    let a = fake("ok");
    assert!(matches!(run(&a, ask_req("one?"), contract()).last(), Some(ChatEvent::Done { .. })));
    assert!(matches!(run(&a, ask_req("two?"), contract()).last(), Some(ChatEvent::Done { .. })));
    let err = stderr(&a);
    assert_eq!(err.matches("thread/start").count(), 1, "{err}");
    assert!(err.contains("turn thr-1 turn-1") && err.contains("excerpts=2"), "{err}");
    assert!(err.contains("turn thr-1 turn-2") && err.contains("excerpts=0"), "{err}");
}

#[test]
fn a_failed_turn_says_to_log_in() {
    let a = fake("fail");
    let ev = run(&a, ask_req("hi?"), contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, hint } => {
            assert_eq!(hint.as_deref(), Some("login"));
            assert!(message.contains("codex login"), "{message}");
        }
        e => panic!("{e:?}"),
    }
}

#[test]
fn server_death_is_reported_and_the_next_question_restarts_it() {
    let flag = std::env::temp_dir().join(format!("inkwell-fake-die-{}", std::process::id()));
    let _ = std::fs::remove_file(&flag);
    let a = fake_with("die-once", Some(flag.to_str().unwrap()), Duration::from_secs(15));
    let ev = run(&a, ask_req("first?"), contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, .. } => assert!(message.contains("stopped unexpectedly"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(a.server_pid().is_none(), "dead server must be dropped");

    let ev = run(&a, ask_req("second?"), contract());
    assert!(matches!(ev.last(), Some(ChatEvent::Done { .. })), "{:?}", ev.last());
    assert!(a.server_pid().is_some());
    // a new process means a new thread, so the excerpts go out again
    let err = stderr(&a);
    assert!(err.contains("thread/start thr-1") && err.contains("excerpts=2"), "{err}");
    let _ = std::fs::remove_file(&flag);
}

#[test]
fn cancel_interrupts_the_running_turn() {
    let a = fake("slow");
    let rx = start(&a, ask_req("write a lot"), contract());
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            ChatEvent::Delta { .. } => break,
            ChatEvent::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    a.cancel(DOC);
    let ev = finish(&rx);
    assert!(matches!(ev.last(), Some(ChatEvent::Cancelled)), "{:?}", ev.last());
    // and the chat is usable again
    assert!(wait_for(|| !a.session(DOC).busy.load(Ordering::SeqCst)));
}

#[test]
fn server_requests_are_refused_without_blocking_the_turn() {
    let a = fake("approval");
    let ev = run(&a, ask_req("go"), contract());
    assert!(matches!(ev.last(), Some(ChatEvent::Done { .. })), "{:?}", ev.last());
    assert!(stderr(&a).contains("approval answered with error"));
}

#[test]
fn silence_times_out_and_the_server_is_killed() {
    let a = fake_with("hang", None, Duration::from_millis(700));
    let ev = run(&a, ask_req("hello?"), contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, .. } => assert!(message.contains("stopped responding"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(a.server_pid().is_none());
}

#[test]
fn closing_the_document_unsubscribes_its_thread() {
    let a = fake("ok");
    run(&a, ask_req("q?"), contract());
    let client = a.server.lock().unwrap().as_ref().unwrap().client.clone();
    a.close_doc(DOC);
    assert!(wait_for(|| client.stderr_tail().contains("unsubscribed thr-1")), "{}", client.stderr_tail());
    assert!(a.texts.lock().unwrap().is_empty());
}

#[test]
fn closing_one_document_mid_turn_leaves_the_server_and_other_chats_alone() {
    let a = fake("slow");
    let rx_a = start_doc(&a, 1, ask_req("a?"), contract());
    let rx_b = start_doc(&a, 2, ask_req("b?"), contract());
    first_delta(&rx_a);
    first_delta(&rx_b);
    let pid = a.server_pid().expect("server running");

    a.close_doc(1);
    let ev = finish(&rx_a);
    assert!(matches!(ev.last(), Some(ChatEvent::Cancelled)), "closed tab ends cancelled, not failed: {:?}", ev.last());

    let ev = finish(&rx_b);
    assert!(matches!(ev.last(), Some(ChatEvent::Done { .. })), "the other document's turn must finish: {:?}", ev.last());
    assert_eq!(a.server_pid(), Some(pid), "the shared server must survive");
    assert!(!stderr(&a).contains("dying"), "{}", stderr(&a));
    // and it still serves new questions without a restart
    let ev = run(&a, ask_req("again?"), contract());
    assert!(matches!(ev.last(), Some(ChatEvent::Done { .. })), "{:?}", ev.last());
    assert_eq!(a.server_pid(), Some(pid));
    a.close_doc(2);
    a.close_doc(DOC);
}

#[test]
fn clearing_a_chat_mid_turn_also_spares_the_server() {
    // the UI's "Clear chat" is `ai_close` on the same document, then a fresh turn later
    let a = fake("slow");
    let rx = start(&a, ask_req("a?"), contract());
    first_delta(&rx);
    let pid = a.server_pid().unwrap();
    a.close_doc(DOC);
    assert!(matches!(finish(&rx).last(), Some(ChatEvent::Cancelled)));
    assert_eq!(a.server_pid(), Some(pid));
    assert!(wait_for(|| !a.session(DOC).busy.load(Ordering::SeqCst)));
}

#[test]
fn warming_a_document_does_not_make_the_first_question_wait_or_fail() {
    let a = fake("slow-start");
    let warm = {
        let a = a.clone();
        std::thread::spawn(move || a.warm(DOC, 0))
    };
    assert!(wait_for(|| a.server_pid().is_some()));
    std::thread::sleep(Duration::from_millis(150)); // warm is now inside its slow thread/start
    let ev = run(&a, ask_req("quick question?"), contract()); // `ask` would panic on "still answering"
    assert!(matches!(ev.last(), Some(ChatEvent::Done { .. })), "{:?}", ev.last());
    warm.join().unwrap();
    assert_eq!(stderr(&a).matches("thread/start").count(), 1, "{}", stderr(&a));
}

#[test]
fn a_hung_request_replaces_the_server() {
    let a = fake_call_timeout("stall-start", Duration::from_millis(2500));
    let ev = run(&a, ask_req("hello?"), contract());
    match ev.last().unwrap() {
        ChatEvent::Error { message, .. } => assert!(message.contains("did not answer in time"), "{message}"),
        e => panic!("{e:?}"),
    }
    assert!(a.server_pid().is_none(), "a server that stopped answering must be dropped");
}

#[test]
fn an_edited_document_gets_a_new_thread() {
    let a = fake("ok");
    assert!(matches!(run(&a, ask_req("one?"), contract_rev(0)).last(), Some(ChatEvent::Done { .. })));
    assert!(matches!(run(&a, ask_req("two?"), contract_rev(1)).last(), Some(ChatEvent::Done { .. })));
    let client = a.server.lock().unwrap().as_ref().unwrap().client.clone();
    assert!(wait_for(|| client.stderr_tail().contains("unsubscribed thr-1")), "{}", client.stderr_tail());
    let err = stderr(&a);
    assert_eq!(err.matches("thread/start").count(), 2, "{err}");
    // the excerpts go out again on the new thread
    assert!(err.contains("turn thr-2 turn-2") && err.contains("excerpts=2"), "{err}");
}

#[cfg(windows)]
#[test]
fn killing_the_server_takes_its_children_with_it() {
    let running = |pid: u32| {
        let o = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        String::from_utf8_lossy(&o.stdout).contains(&pid.to_string())
    };
    let a = fake("grandchild");
    let _ = a.status(); // starts the server
    let client = a.server.lock().unwrap().as_ref().unwrap().client.clone();
    assert!(wait_for(|| client.stderr_tail().contains("grandchild ")), "{}", client.stderr_tail());
    let tail = client.stderr_tail();
    let gc: u32 = tail.lines().find_map(|l| l.strip_prefix("grandchild ")).unwrap().trim().parse().unwrap();
    assert!(running(gc), "grandchild {gc} should be running");
    client.kill();
    let mut gone = false;
    for _ in 0..40 {
        if !running(gc) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(gone, "grandchild {gc} outlived the server");
}

#[test]
fn selection_actions_are_plain_markdown_without_a_schema() {
    let a = fake("ok");
    let req = AskRequest {
        task: Task::Selection { action: SelAction::Explain, text: "indemnify and hold harmless".into(), page: 1, style: None },
        ..ask_req("")
    };
    let ev = run(&a, req, contract());
    let ChatEvent::Done { answer, citations, .. } = ev.last().unwrap() else { panic!("{:?}", ev.last()) };
    assert!(answer.starts_with("Plain **markdown** reply."));
    assert!(citations.is_empty());
    assert_eq!(&deltas(&ev), answer);
    assert!(stderr(&a).contains("schema=no"));
}

#[test]
fn a_document_without_text_never_reaches_codex() {
    let a = fake("ok");
    let empty = Arc::new(DocText { rev: 0, pages: 4, chunks: vec![chunk("p1.1", 0, "A1")], chars: 2 });
    let ev = run(&a, ask_req("summarize"), empty);
    match ev.last().unwrap() {
        ChatEvent::Error { hint, message } => {
            assert_eq!(hint.as_deref(), Some("noText"));
            assert!(message.contains("OCR"));
        }
        e => panic!("{e:?}"),
    }
    assert!(a.server_pid().is_none(), "no process for a document the model could not read");
}

#[test]
fn a_missing_codex_is_reported_with_an_install_hint() {
    let a = Assistant::with_spec(Box::new(|| None), Duration::from_secs(1));
    let st = a.status();
    assert!(!st.installed && !st.logged_in);
    let ev = run(&a, ask_req("hi"), contract());
    assert!(matches!(ev.last(), Some(ChatEvent::Error { hint: Some(h), .. }) if h == "install"), "{:?}", ev.last());
}

#[test]
fn status_reports_account_and_visible_models() {
    let st = fake("ok").status();
    assert!(st.installed && st.logged_in, "{st:?}");
    assert_eq!(st.account.as_deref(), Some("tester@example.com"));
    assert_eq!(st.plan.as_deref(), Some("plus"));
    assert_eq!(st.models.len(), 1, "hidden models are not offered");
    assert_eq!(st.models[0].efforts, ["low", "medium", "high"]);
    assert!(st.models[0].is_default);
    let out = fake("loggedout").status();
    assert!(out.installed && !out.logged_in);
}

#[test]
fn two_documents_stream_over_one_server() {
    let a = fake("ok");
    let (tx1, rx1) = mpsc::channel();
    let (tx2, rx2) = mpsc::channel();
    let s1: Sink = Arc::new(move |e| {
        let _ = tx1.send(e);
    });
    let s2: Sink = Arc::new(move |e| {
        let _ = tx2.send(e);
    });
    let t = contract();
    let (t1, t2) = (t.clone(), t);
    a.ask(1, ask_req("a?"), move || Ok(t1), Arc::new(FakeHost), s1).unwrap();
    a.ask(2, ask_req("b?"), move || Ok(t2), Arc::new(FakeHost), s2).unwrap();
    assert!(matches!(finish(&rx1).last(), Some(ChatEvent::Done { .. })));
    assert!(matches!(finish(&rx2).last(), Some(ChatEvent::Done { .. })));
    let err = stderr(&a);
    assert!(err.contains("thr-1") && err.contains("thr-2"), "{err}");
    a.close_doc(1);
    a.close_doc(2);
}

// ---- dynamic tools ------------------------------------------------------------------------------

fn tool_server(script: &str) -> Arc<Assistant> {
    fake(&format!("tools={script}"))
}

fn activity(ev: &[ChatEvent]) -> Vec<(String, ActState)> {
    ev.iter()
        .filter_map(|e| match e {
            ChatEvent::Activity { tool, state, .. } => Some((tool.clone(), *state)),
            _ => None,
        })
        .collect()
}

fn states(ev: &[ChatEvent], tool: &str) -> Vec<ActState> {
    activity(ev).into_iter().filter(|(t, _)| t == tool).map(|(_, s)| s).collect()
}

/// Like `finish`, answering every Apply / Reject card with `allow(tool)`.
fn finish_deciding(a: &Arc<Assistant>, rx: &mpsc::Receiver<ChatEvent>, allow: impl Fn(&str) -> bool) -> Vec<ChatEvent> {
    let mut v = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("an event within 30 s");
        if let ChatEvent::Activity { id, tool, state: ActState::Waiting, .. } = &e {
            a.decide(DOC, id, allow(tool));
        }
        let last = matches!(e, ChatEvent::Done { .. } | ChatEvent::Error { .. } | ChatEvent::Cancelled);
        v.push(e);
        if last {
            return v;
        }
    }
}

/// Registers a test-only edit tool and a destructive one; each counts how often it really ran.
fn edit_tools(a: &Assistant) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let make = |name: &str, class: Class| {
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        a.register(Arc::new(Tool {
            name: name.into(),
            description: "test only".into(),
            schema: json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            class,
            label: |_| "Change the test document".into(),
            run: Arc::new(move |_, _| {
                h.fetch_add(1, Ordering::SeqCst);
                Ok(tools::Output::text("changed"))
            }),
        }));
        hits
    };
    (make("test_edit", Class::Edit), make("test_destroy", Class::Destructive))
}

fn done(ev: &[ChatEvent]) -> &ChatEvent {
    let e = ev.last().unwrap();
    assert!(matches!(e, ChatEvent::Done { .. }), "{e:?}");
    e
}

#[test]
fn codex_gets_the_experimental_capability_and_the_tool_list() {
    let a = fake("ok");
    done(&run(&a, ask_req("hi?"), contract()));
    let err = stderr(&a);
    assert!(err.contains("initialize experimentalApi=true"), "{err}");
    assert!(
        err.contains(
            "tools=get_document_info,get_outline,get_text,search,get_selection,get_page_image,go_to_page,show_source"
        ),
        "{err}"
    );
}

#[test]
fn a_read_tool_serves_text_and_its_passages_can_be_cited() {
    let a = tool_server(r#"[{"tool":"get_text","arguments":{"pages":[2]}}]"#);
    a.set_inline_max(10); // header only: the text has to come through the tool
    let ev = run(&a, ask_req("When is delivery?"), contract());
    let ChatEvent::Done { answer, citations, chunks_sent, .. } = done(&ev) else { unreachable!() };
    assert_eq!(&deltas(&ev), answer, "commentary between tool calls must not reach the answer");
    assert_eq!(*chunks_sent, 1, "only the passage the tool returned");
    assert_eq!((citations[0].page, citations[0].exact), (Some(1), true));
    assert_eq!(citations[0].chunk.as_deref(), Some("p2.1"));
    assert_eq!(activity(&ev), [("get_text".into(), ActState::Running), ("get_text".into(), ActState::Done)]);
    let label = ev.iter().find_map(|e| match e {
        ChatEvent::Activity { label, .. } => Some(label.as_str()),
        _ => None,
    });
    assert_eq!(label, Some("Read p. 2"));
    let err = stderr(&a);
    assert!(err.contains("excerpts=0"), "no text in the prompt: {err}");
    assert!(err.contains("tool-reply get_text success=true images=0 :: <<<DOCUMENT --- page 2 --- [p2.1] The supplier"), "{err}");
}

#[test]
fn short_documents_are_inlined_and_long_ones_are_read_through_tools() {
    let a = fake("ok");
    done(&run(&a, ask_req("short?"), contract()));
    assert!(stderr(&a).contains("excerpts=2"), "{}", stderr(&a));
    let b = fake("ok");
    b.set_inline_max(10);
    done(&run(&b, ask_req("long?"), contract()));
    assert!(stderr(&b).contains("excerpts=0"), "{}", stderr(&b));
}

#[test]
fn ask_mode_waits_for_apply_and_a_rejection_changes_nothing() {
    let a = tool_server(r#"[{"tool":"test_edit"}]"#);
    let (edits, _) = edit_tools(&a);
    let ev = finish_deciding(&a, &start(&a, ask_req("go"), contract()), |_| false);
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 0);
    assert_eq!(states(&ev, "test_edit"), [ActState::Waiting, ActState::Rejected]);
    assert!(stderr(&a).contains("tool-reply test_edit success=false images=0 :: The user rejected this change"), "{}", stderr(&a));
    // the same question again, this time applied
    let ev = finish_deciding(&a, &start(&a, ask_req("go again"), contract()), |_| true);
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 1);
    assert_eq!(states(&ev, "test_edit"), [ActState::Waiting, ActState::Running, ActState::Done]);
}

/// A person takes a while: a card that waits longer than the silence limit must not end the turn.
#[test]
fn a_slow_apply_outlives_the_silence_watchdog() {
    let a = fake_with(r#"tools=[{"tool":"test_edit"}]"#, None, Duration::from_secs(2));
    let (edits, _) = edit_tools(&a);
    let rx = start(&a, ask_req("go"), contract());
    let mut ev = Vec::new();
    loop {
        let e = rx.recv_timeout(Duration::from_secs(30)).expect("an event within 30 s");
        if let ChatEvent::Activity { id, state: ActState::Waiting, .. } = &e {
            std::thread::sleep(Duration::from_secs(6)); // three times the silence limit
            a.decide(DOC, id, true);
        }
        let last = matches!(e, ChatEvent::Done { .. } | ChatEvent::Error { .. } | ChatEvent::Cancelled);
        ev.push(e);
        if last {
            break;
        }
    }
    done(&ev);
    assert_eq!(edits.load(Ordering::SeqCst), 1);
    assert_eq!(states(&ev, "test_edit"), [ActState::Waiting, ActState::Running, ActState::Done]);
}

#[test]
fn auto_mode_applies_edits_but_asks_before_destructive_calls() {
    let a = tool_server(r#"[{"tool":"test_edit"},{"tool":"test_destroy"}]"#);
    let (edits, destroys) = edit_tools(&a);
    let req = AskRequest { permissions: Mode::Auto, ..ask_req("go") };
    let ev = finish_deciding(&a, &start(&a, req, contract()), |t| {
        assert_eq!(t, "test_destroy", "an edit must not ask in auto mode");
        false
    });
    done(&ev);
    assert_eq!((edits.load(Ordering::SeqCst), destroys.load(Ordering::SeqCst)), (1, 0));
    assert_eq!(states(&ev, "test_edit"), [ActState::Running, ActState::Done]);
    assert_eq!(states(&ev, "test_destroy"), [ActState::Waiting, ActState::Rejected]);
}

#[test]
fn full_mode_runs_everything_without_asking() {
    let a = tool_server(r#"[{"tool":"test_edit"},{"tool":"test_destroy"}]"#);
    let (edits, destroys) = edit_tools(&a);
    let req = AskRequest { permissions: Mode::Full, ..ask_req("go") };
    let ev = finish_deciding(&a, &start(&a, req, contract()), |_| panic!("full mode must not ask"));
    done(&ev);
    assert_eq!((edits.load(Ordering::SeqCst), destroys.load(Ordering::SeqCst)), (1, 1));
    assert!(activity(&ev).iter().all(|(_, s)| *s != ActState::Waiting));
}

#[test]
fn read_tools_never_wait_in_any_mode() {
    let a = tool_server(r#"[{"tool":"search","arguments":{"query":"terminate"}}]"#);
    let ev = finish_deciding(&a, &start(&a, ask_req("find"), contract()), |_| panic!("a read tool must not ask"));
    done(&ev);
    assert_eq!(states(&ev, "search"), [ActState::Running, ActState::Done]);
    assert!(stderr(&a).contains("1 match on 1 page for “terminate”"), "{}", stderr(&a));
}

#[test]
fn cancelling_while_a_card_waits_rejects_it_and_ends_the_turn() {
    let a = tool_server(r#"[{"tool":"test_edit"}]"#);
    let (edits, _) = edit_tools(&a);
    let rx = start(&a, ask_req("go"), contract());
    loop {
        match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            ChatEvent::Activity { state: ActState::Waiting, .. } => break,
            ChatEvent::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    a.cancel(DOC);
    let ev = finish(&rx);
    assert!(matches!(ev.last(), Some(ChatEvent::Cancelled)), "{:?}", ev.last());
    assert_eq!(edits.load(Ordering::SeqCst), 0);
    assert!(states(&ev, "test_edit").contains(&ActState::Rejected));
}

#[test]
fn an_unknown_tool_is_refused_and_the_turn_goes_on() {
    let a = tool_server(r#"[{"tool":"delete_everything","arguments":{}}]"#);
    let ev = run(&a, ask_req("go"), contract());
    done(&ev);
    assert_eq!(states(&ev, "delete_everything"), [ActState::Failed]);
    assert!(stderr(&a).contains("tool-reply delete_everything success=false images=0 :: Unknown tool"), "{}", stderr(&a));
}

#[test]
fn a_tool_error_reaches_the_model_as_text() {
    let a = tool_server(r#"[{"tool":"get_text","arguments":{"pages":[99]}}]"#);
    let ev = run(&a, ask_req("go"), contract());
    done(&ev);
    assert_eq!(states(&ev, "get_text"), [ActState::Running, ActState::Failed]);
    assert!(stderr(&a).contains("success=false images=0 :: Page 99 does not exist: the document has 2 pages."), "{}", stderr(&a));
}

#[test]
fn a_call_that_arrives_after_the_tab_closed_is_refused_and_spares_the_server() {
    let a = tool_server(r#"[{"tool":"get_text","arguments":{"pages":[1]},"delay":700}]"#);
    let rx = start(&a, ask_req("go"), contract());
    loop {
        if matches!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), ChatEvent::Status { phase: "thinking" }) {
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(150));
    let pid = a.server_pid().unwrap();
    a.close_doc(DOC);
    let ev = finish(&rx);
    assert!(matches!(ev.last(), Some(ChatEvent::Cancelled)), "{:?}", ev.last());
    assert!(wait_for(|| stderr(&a).contains("tool-reply get_text error:the document is not open any more")), "{}", stderr(&a));
    assert_eq!(a.server_pid(), Some(pid));
}

#[test]
fn navigation_tools_move_the_view_and_a_made_up_quote_is_refused() {
    let a = tool_server(
        r#"[{"tool":"go_to_page","arguments":{"page":2}},
            {"tool":"show_source","arguments":{"page":1,"quote":"terminate this agreement with thirty days"}},
            {"tool":"show_source","arguments":{"page":2,"quote":"a sentence that is nowhere in the file"}}]"#,
    );
    let ev = run(&a, ask_req("show me"), contract());
    done(&ev);
    let ui: Vec<&UiAction> = ev
        .iter()
        .filter_map(|e| match e {
            ChatEvent::Ui { action } => Some(action),
            _ => None,
        })
        .collect();
    assert_eq!(ui.len(), 2, "{ui:?}");
    assert_eq!(*ui[0], UiAction::GoTo { page: 1 });
    assert!(matches!(ui[1], UiAction::Show { page: 0, rects, .. } if *rects == vec![[50.0, 100.0, 500.0, 112.0]]), "{:?}", ui[1]);
    let s = states(&ev, "show_source");
    assert_eq!(s, [ActState::Running, ActState::Done, ActState::Running, ActState::Failed]);
    assert!(stderr(&a).contains("That quote does not appear on page 2"), "{}", stderr(&a));
}

#[test]
fn a_page_image_travels_as_an_input_image() {
    let a = tool_server(r#"[{"tool":"get_page_image","arguments":{"page":1}}]"#);
    done(&run(&a, ask_req("what does it look like"), contract()));
    assert!(stderr(&a).contains("tool-reply get_page_image success=true images=1"), "{}", stderr(&a));
}

#[test]
fn the_selection_and_the_page_in_view_ride_along() {
    let a = tool_server(r#"[{"tool":"get_selection"}]"#);
    let req = AskRequest {
        selection: Some(Selected { text: "thirty days written notice".into(), page: 0 }),
        current_page: Some(1),
        ..ask_req("what does this mean?")
    };
    done(&run(&a, req, contract()));
    let err = stderr(&a);
    assert!(err.contains("tool-reply get_selection success=true images=0 :: The user selected this text on page 1: <<<SELECTION thirty days"), "{err}");
}

/// The registered tools and the replies we build keep the shape of the schema this build was
/// written against (codex 0.159.0); `live_codex_protocol_matches_the_pin` checks the installed CLI.
#[test]
fn dynamic_tool_json_matches_the_pinned_codex_schema() {
    let snap: Value = serde_json::from_str(include_str!("dynamic_tools.schema.json")).unwrap();
    let keys = |v: &Value| {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    };
    let required = |v: &Value| {
        let mut k: Vec<String> = v["required"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        k.sort();
        k
    };
    // the specs of thread/start
    let spec_schema = &snap["DynamicToolSpec"]["oneOf"][0];
    assert_eq!(spec_schema["properties"]["type"]["enum"], json!(["function"]));
    let specs = tools::specs(&tools::builtin());
    for s in specs.as_array().unwrap() {
        let k = keys(s);
        assert_eq!(k, required(spec_schema), "a function spec has exactly the required fields: {k:?}");
        assert!(spec_schema["properties"].as_object().unwrap().keys().all(|p| p != "name" || s.get("name").is_some()));
        assert_eq!(s["type"], "function");
        assert_eq!(s["inputSchema"]["type"], "object");
    }
    // the replies of item/tool/call
    let resp_schema = &snap["DynamicToolCallResponse"];
    let png = tools::Output { text: "t".into(), image: Some(vec![1]), ..Default::default() };
    for r in [tools::response(&Ok(png)), tools::response(&Err("e".into()))] {
        assert_eq!(keys(&r), required(resp_schema));
        for item in r["contentItems"].as_array().unwrap() {
            let ty = item["type"].as_str().unwrap();
            let def = resp_schema["definitions"]["DynamicToolCallOutputContentItem"]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["properties"]["type"]["enum"][0] == ty)
                .unwrap_or_else(|| panic!("codex no longer has a content item of type {ty}"));
            assert_eq!(keys(item), required(def));
        }
    }
    // the request we read
    let call = &snap["DynamicToolCallParams"];
    for field in ["threadId", "turnId", "callId", "tool", "arguments"] {
        assert!(call["properties"].get(field).is_some(), "DynamicToolCallParams lost `{field}`");
    }
    assert_eq!(snap["InitializeCapabilities.experimentalApi"]["type"], "boolean");
    assert_eq!(crate::ai::rpc::TOOL_CALL, "item/tool/call");
}

/// `INKWELL_LIVE_CODEX=1 cargo test --lib live_codex_protocol -- --nocapture`: the installed CLI still
/// speaks the pinned dynamic-tool protocol. Fails loudly (with the diff) when codex was updated.
#[test]
fn live_codex_protocol_matches_the_pin() {
    if std::env::var("INKWELL_LIVE_CODEX").as_deref() != Ok("1") {
        return;
    }
    let exe = crate::ai::codex::resolve_binary().expect("codex installed");
    let snap: Value = serde_json::from_str(include_str!("dynamic_tools.schema.json")).unwrap();
    let ver = std::process::Command::new(&exe).arg("--version").output().unwrap();
    let ver = String::from_utf8_lossy(&ver.stdout).trim().to_string();
    let pinned = snap["codex_version"].as_str().unwrap();
    println!("codex: {ver} (pinned {pinned})");
    let dir = std::env::temp_dir().join(format!("inkwell-codex-schema-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let o = std::process::Command::new(&exe)
        .args(["app-server", "generate-json-schema", "--experimental", "--out"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let read = |p: &str| -> Value {
        let mut v: Value = serde_json::from_slice(&std::fs::read(dir.join(p)).unwrap()).unwrap();
        v.as_object_mut().unwrap().remove("$schema");
        v
    };
    let start = read("v2/ThreadStartParams.json");
    let init = read("v1/InitializeParams.json");
    let checks = [
        ("DynamicToolCallParams", read("DynamicToolCallParams.json")),
        ("DynamicToolCallResponse", read("DynamicToolCallResponse.json")),
        ("DynamicToolSpec", start["definitions"]["DynamicToolSpec"].clone()),
        ("DynamicToolNamespaceTool", start["definitions"]["DynamicToolNamespaceTool"].clone()),
        ("ThreadStartParams.dynamicTools", start["properties"]["dynamicTools"].clone()),
        ("InitializeCapabilities.experimentalApi", init["definitions"]["InitializeCapabilities"]["properties"]["experimentalApi"].clone()),
    ];
    let mut bad = Vec::new();
    for (name, now) in checks {
        if now != snap[name] {
            bad.push(name);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        bad.is_empty() && ver.contains(pinned),
        "codex {ver} differs from the pin {pinned} in {bad:?}: re-check ai/tools.rs and src/ai/chat/dynamic_tools.schema.json, then update the pin"
    );
}

#[test]
fn extract_reads_every_page_of_a_pdf_into_chunks() {
    let dir = std::env::temp_dir().join("inkwell-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ai-extract.pdf");
    std::fs::write(&path, fixture::build(&Fixture { pages: 3, needle_every: 2, ..Default::default() })).unwrap();
    let eng = Engine::new(2, 16 << 20);
    let id = eng.open(path.to_str().unwrap()).unwrap().doc;
    let t = text::extract(&eng, id).unwrap();
    assert_eq!(t.pages, 3);
    assert!(t.enough());
    assert!(t.chunks.iter().any(|c| c.id == "p3.1" && c.text.contains("Inkwell fixture page 3")), "{:?}", t.chunks.iter().map(|c| &c.id).collect::<Vec<_>>());
    let needle: Vec<u32> = t.chunks.iter().filter(|c| c.text.contains("needle")).map(|c| c.page).collect();
    assert_eq!(needle, vec![0, 2]);
}

/// `INKWELL_LIVE_CODEX=1 cargo test --lib live_codex -- --nocapture`: the real CLI, a 40-page fixture.
#[test]
fn live_codex_smoke() {
    if std::env::var("INKWELL_LIVE_CODEX").as_deref() != Ok("1") {
        return;
    }
    let dir = std::env::temp_dir().join("inkwell-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ai-live-40.pdf");
    std::fs::write(&path, fixture::build(&Fixture { pages: 40, needle_every: 7, chapter_every: 10, ..Default::default() })).unwrap();
    let eng = Engine::new(4, 64 << 20);
    let id = eng.open(path.to_str().unwrap()).unwrap().doc;
    let t0 = Instant::now();
    let text = Arc::new(text::extract(&eng, id).unwrap());
    println!("extract: {} chunks, {} chars, {} ms", text.chunks.len(), text.chars, t0.elapsed().as_millis());

    let a = Assistant::new();
    let st = a.status();
    println!("status: {st:?}");
    assert!(st.installed && st.logged_in, "codex must be installed and logged in");

    let show = |label: &str, ev: &[ChatEvent]| {
        for e in ev {
            if let ChatEvent::Activity { tool, label, state, detail, .. } = e {
                println!("  tool {tool}: {label} [{state:?}] {detail:?}");
            }
        }
        match ev.last().unwrap() {
        ChatEvent::Done { answer, citations, followups, first_token_ms, total_ms, prep_ms, chunks_sent, chunks_total, usage } => {
            println!(
                "{label}: first token {:?} ms, total {total_ms} ms (prep {prep_ms} ms), chunks {chunks_sent}/{chunks_total}, usage {usage:?}",
                first_token_ms
            );
            println!("  answer: {}", answer.replace('\n', "\n          "));
            println!("  citations: {citations:#?}\n  followups: {followups:?}");
        }
        e => panic!("{label}: {e:?}"),
        }
    };

    let req = |task| AskRequest {
        task,
        name: "live-40.pdf".into(),
        model: None,
        effort: Some("low".into()),
        current_page: Some(0),
        selection: None,
        permissions: Mode::Ask,
    };
    // the real engine behind the tools: real title, outline and page images
    let go =|r: AskRequest| {
        let (tx, rx) = mpsc::channel();
        let sink: Sink = Arc::new(move |e| {
            let _ = tx.send(e);
        });
        let (text, host) = (text.clone(), Arc::new(crate::ai::EngineHost { eng: eng.clone(), doc: id }));
        a.ask(DOC, r, move || Ok(text), host, sink).unwrap();
        finish(&rx)
    };
    let first = go(req(Task::SummarizeDocument));
    show("summarize", &first);

    let ev = go(req(Task::Ask { question: "On which pages does the word needle appear? Cite each.".into() }));
    show("needle", &ev);
    let ChatEvent::Done { citations, answer, .. } = ev.last().unwrap() else { unreachable!() };
    assert!(!answer.is_empty());
    assert!(citations.iter().any(|c| c.page.is_some() && !c.rects.is_empty()), "at least one citation must highlight lines");
    // 40 pages is over the inline threshold: the model has to read through the tools (the second
    // question may be answered from what the first one already read, so count both turns)
    assert!(
        first.iter().chain(&ev).any(|e| matches!(e, ChatEvent::Activity { tool, state: ActState::Done, .. } if tool == "search" || tool == "get_text")),
        "the model never called search or get_text"
    );
    assert!(citations.iter().filter(|c| c.exact).all(|c| !c.rects.is_empty()), "a verified source outlines its lines");
    a.close_doc(DOC);
}
