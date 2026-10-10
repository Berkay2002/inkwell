//! Transport, streaming, errors and restart against `fake_codex.mjs` (Node), plus one live smoke
//! test against the real Codex behind `INKWELL_LIVE_CODEX=1`.

use super::*;
use crate::ai::text::{Chunk, Line, SelAction};
use crate::engine::Engine;
use crate::engine::fixture::{self, Fixture};
use std::sync::mpsc;

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
    }
}

fn start_doc(a: &Arc<Assistant>, doc: DocId, req: AskRequest, text: Arc<DocText>) -> mpsc::Receiver<ChatEvent> {
    let (tx, rx) = mpsc::channel();
    let sink: Sink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    a.ask(doc, req, move || Ok(text), sink).unwrap();
    rx
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
    assert!(err.contains("thread/start thr-1 sandbox=read-only approval=never ephemeral=true"), "{err}");
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
    a.ask(1, ask_req("a?"), move || Ok(t1), s1).unwrap();
    a.ask(2, ask_req("b?"), move || Ok(t2), s2).unwrap();
    assert!(matches!(finish(&rx1).last(), Some(ChatEvent::Done { .. })));
    assert!(matches!(finish(&rx2).last(), Some(ChatEvent::Done { .. })));
    let err = stderr(&a);
    assert!(err.contains("thr-1") && err.contains("thr-2"), "{err}");
    a.close_doc(1);
    a.close_doc(2);
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
    std::fs::write(&path, fixture::build(&Fixture { pages: 40, needle_every: 7, ..Default::default() })).unwrap();
    let eng = Engine::new(4, 64 << 20);
    let id = eng.open(path.to_str().unwrap()).unwrap().doc;
    let t0 = Instant::now();
    let text = Arc::new(text::extract(&eng, id).unwrap());
    println!("extract: {} chunks, {} chars, {} ms", text.chunks.len(), text.chars, t0.elapsed().as_millis());

    let a = Assistant::new();
    let st = a.status();
    println!("status: {st:?}");
    assert!(st.installed && st.logged_in, "codex must be installed and logged in");

    let show = |label: &str, ev: &[ChatEvent]| match ev.last().unwrap() {
        ChatEvent::Done { answer, citations, followups, first_token_ms, total_ms, prep_ms, chunks_sent, chunks_total, usage } => {
            println!(
                "{label}: first token {:?} ms, total {total_ms} ms (prep {prep_ms} ms), chunks {chunks_sent}/{chunks_total}, usage {usage:?}",
                first_token_ms
            );
            println!("  answer: {}", answer.replace('\n', "\n          "));
            println!("  citations: {citations:#?}\n  followups: {followups:?}");
        }
        e => panic!("{label}: {e:?}"),
    };

    let req = |task| AskRequest { task, name: "live-40.pdf".into(), model: None, effort: Some("low".into()), current_page: Some(0) };
    let ev = run(&a, req(Task::SummarizeDocument), text.clone());
    show("summarize", &ev);

    let ev = run(&a, req(Task::Ask { question: "On which pages does the word needle appear? Cite each.".into() }), text.clone());
    show("needle", &ev);
    let ChatEvent::Done { citations, answer, .. } = ev.last().unwrap() else { unreachable!() };
    assert!(!answer.is_empty());
    assert!(citations.iter().any(|c| c.page.is_some() && !c.rects.is_empty()), "at least one citation must highlight lines");
    a.close_doc(id);
}
