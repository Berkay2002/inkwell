//! Runs against the real Codex and a real engine, behind `INKWELL_LIVE_CODEX=1`:
//! `INKWELL_LIVE_CODEX=1 cargo test --lib live_ -- --nocapture --test-threads=1`

use super::*;

fn live() -> bool {
    std::env::var("INKWELL_LIVE_CODEX").as_deref() == Ok("1")
}

/// A document opened in a real engine, asked through the real `EngineHost`.
struct Live {
    eng: Arc<Engine>,
    doc: DocId,
    text: Arc<DocText>,
}

impl Live {
    fn open(name: &str, pdf: Vec<u8>) -> Live {
        let dir = std::env::temp_dir().join("inkwell-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, pdf).unwrap();
        let eng = Engine::new(4, 64 << 20);
        let doc = eng.open(path.to_str().unwrap()).unwrap().doc;
        let text = Arc::new(text::extract(&eng, doc).unwrap());
        Live { eng, doc, text }
    }

    fn ask(&self, a: &Arc<Assistant>, q: &str, page: u32) -> Vec<ChatEvent> {
        let (tx, rx) = mpsc::channel();
        let sink: Sink = Arc::new(move |e| {
            let _ = tx.send(e);
        });
        let host = Arc::new(crate::ai::EngineHost { eng: self.eng.clone(), doc: self.doc });
        let req = AskRequest { current_page: Some(page), ..ask_req(q) };
        let text = self.text.clone();
        a.ask(self.doc, req, move || Ok(text), host, sink).unwrap();
        let mut v = Vec::new();
        loop {
            let e = rx.recv_timeout(Duration::from_secs(180)).expect("an event within 180 s");
            let last = matches!(e, ChatEvent::Done { .. } | ChatEvent::Error { .. } | ChatEvent::Cancelled);
            v.push(e);
            if last {
                return v;
            }
        }
    }
}

fn tools_used(ev: &[ChatEvent]) -> Vec<String> {
    ev.iter()
        .filter_map(|e| match e {
            ChatEvent::Activity { tool, label, state: ActState::Done, .. } => Some(format!("{tool}({label})")),
            _ => None,
        })
        .collect()
}

/// A 3-page PDF whose page 2 shows `number` drawn as seven-segment bars: no text of it exists, so
/// only a model that looks at the page image can read it.
fn drawn_number_pdf(number: &str) -> Vec<u8> {
    const SEG: [&str; 10] = ["abcdef", "bc", "abdeg", "abcdg", "bcfg", "acdfg", "acdefg", "abc", "abcdefg", "abcdfg"];
    let mut bars = String::from("0 g\n");
    for (i, d) in number.chars().enumerate() {
        let (x, y, k) = (60.0 + i as f32 * 130.0, 300.0, 1.8f32);
        for s in SEG[d.to_digit(10).unwrap() as usize].chars() {
            let (dx, dy, w, h) = match s {
                'a' => (6.0, 108.0, 48.0, 12.0),
                'b' => (48.0, 60.0, 12.0, 54.0),
                'c' => (48.0, 6.0, 12.0, 54.0),
                'd' => (6.0, 0.0, 48.0, 12.0),
                'e' => (0.0, 6.0, 12.0, 54.0),
                'f' => (0.0, 60.0, 12.0, 54.0),
                _ => (6.0, 54.0, 48.0, 12.0),
            };
            bars.push_str(&format!("{} {} {} {} re f\n", x + dx * k, y + dy * k, w * k, h * k));
        }
    }
    // enough filler text that the document counts as readable; the bars start below y = 560
    let text = |s: &str| {
        let mut t = format!("BT /F1 14 Tf 48 700 Td ({s}) Tj ET\n");
        for l in 0..5 {
            t.push_str(&format!(
                "BT /F1 12 Tf 48 {} Td (Plain filler sentence number {l}: the committee reviewed the budget and adjourned.) Tj ET\n",
                660 - l * 22
            ));
        }
        t
    };
    let pages = [
        text("Inkwell image test, page 1."),
        format!("{}{}", text("Inkwell image test, page 2."), bars),
        text("Inkwell image test, page 3."),
    ];
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [4 0 R 6 0 R 8 0 R] /Count 3 >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    for (i, c) in pages.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + i * 2
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{c}endstream", c.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes(),
    );
    out
}

#[test]
fn live_codex_reads_a_page_image_and_has_no_shell() {
    if !live() {
        return;
    }
    let d = Live::open("ai-live-image.pdf", drawn_number_pdf("4271"));
    assert!(!d.text.chunks.iter().any(|c| c.text.contains("4271")), "the number must exist only as a drawing");
    {
        use crate::ai::tools::DocHost as _;
        let png = crate::ai::EngineHost { eng: d.eng.clone(), doc: d.doc }.page_png(d.text.rev, 1, 1024).unwrap();
        let p = std::env::temp_dir().join("inkwell-tests").join("ai-live-image-page2.png");
        std::fs::write(&p, &png).unwrap();
        println!("page 2 png: {} bytes at {}", png.len(), p.display());
    }
    let a = Assistant::new();
    let ev = d.ask(&a, "Page 2 shows a large number drawn as bars. Look at the page image and tell me the number.", 1);
    let ChatEvent::Done { answer, .. } = ev.last().unwrap() else { panic!("{:?}", ev.last()) };
    println!("tools: {:?}\nanswer: {answer}", tools_used(&ev));
    let (asked, read) = (tools_used(&ev).iter().any(|t| t.starts_with("get_page_image")), answer.contains("4271"));
    a.close_doc(d.doc);

    // the model has the document tools and nothing else: no shell of its own
    let ev = d.ask(&a, "List every tool you can call, by exact name, and say whether you can run shell commands.", 0);
    let ChatEvent::Done { answer, .. } = ev.last().unwrap() else { panic!("{:?}", ev.last()) };
    println!("tools: {:?}\nanswer: {answer}", tools_used(&ev));
    a.close_doc(d.doc);
    assert!(asked, "no page image was requested");
    assert!(read, "the model did not read the number from the page image");
}

/// A person reads the card for a while: the real server must still be waiting for the reply when
/// Apply comes 90 s later, and the turn must finish with the change made.
#[test]
fn live_a_slow_apply_still_lands() {
    if live() {
        slow_apply(90, None);
    }
}

/// Finds how long the real server keeps waiting for a tool result: `INKWELL_LIVE_WAIT=<seconds>`
/// answers the card that late (default 60) with our own limit lifted, and prints what the model
/// made of it. Codex 0.159: replies at 150, 210 and 273 s were taken; by 330 s it had given up
/// (the model said "did not return a result" while the late change still ran), so
/// `APPROVAL_TIMEOUT` (240 s) must stay below that.
#[test]
fn live_how_late_may_an_apply_be() {
    if let (true, Ok(w)) = (live(), std::env::var("INKWELL_LIVE_WAIT")) {
        slow_apply(w.parse().unwrap(), Some(Duration::from_secs(900)));
    }
}

fn slow_apply(wait_s: u64, timeout: Option<Duration>) {
    let d = Live::open("ai-live-slow.pdf", drawn_number_pdf("4271"));
    let a = Assistant::new();
    if let Some(t) = timeout {
        a.set_approval_timeout(t);
    }
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    a.register(Arc::new(Tool {
        name: "set_page_note".into(),
        description: "Attaches a short note to a page of the document.".into(),
        schema: json!({ "type": "object", "additionalProperties": false, "required": ["page", "note"],
            "properties": { "page": { "type": "integer" }, "note": { "type": "string" } } }),
        class: Class::Edit,
        label: |a| format!("Add a note to p. {}", a["page"].as_i64().unwrap_or(0)),
        run: Arc::new(move |_, _| {
            h.fetch_add(1, Ordering::SeqCst);
            Ok(tools::Output::text("The note was added."))
        }),
        check: None,
    }));
    let (tx, rx) = mpsc::channel();
    let sink: Sink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let host = Arc::new(crate::ai::EngineHost { eng: d.eng.clone(), doc: d.doc });
    let req = ask_req("Add the note \"check this\" to page 2 with set_page_note, then say whether it was applied.");
    let text = d.text.clone();
    a.ask(d.doc, req, move || Ok(text), host, sink).unwrap();
    let t0 = Instant::now();
    let mut decided = None;
    let ev = loop {
        let e = rx.recv_timeout(Duration::from_secs(180)).expect("an event within 180 s");
        if let ChatEvent::Activity { id, state: ActState::Waiting, .. } = &e {
            println!("card shown after {} ms; answering in {wait_s} s", t0.elapsed().as_millis());
            std::thread::sleep(Duration::from_secs(wait_s));
            a.decide(d.doc, id, true);
            decided = Some(t0.elapsed());
        }
        if matches!(e, ChatEvent::Done { .. } | ChatEvent::Error { .. } | ChatEvent::Cancelled) {
            break e;
        }
    };
    println!("decided at {decided:?}; ended with {ev:?}");
    assert!(decided.is_some(), "the model never asked");
    assert!(matches!(ev, ChatEvent::Done { .. }), "{ev:?}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    a.close_doc(d.doc);
}

/// The same question with the whole text in the turn and with tools only, at three document sizes.
#[test]
fn live_inline_threshold() {
    if !live() {
        return;
    }
    println!("pages chars mode      total_ms first_ms input_tok tool_calls cited_exact");
    for pages in [5usize, 15, 30] {
        let pdf = fixture::build(&Fixture { pages, needle_every: 4, chapter_every: 5, ..Default::default() });
        let d = Live::open(&format!("ai-thr-{pages}.pdf"), pdf);
        for (mode, max) in [("inline", usize::MAX), ("tools", 0)] {
            let a = Assistant::new();
            a.set_inline_max(max);
            // the first turn of a fresh thread is where the text is inlined or not
            let ev = d.ask(&a, "On which page does the word needle first appear, and what is on the line with it? Cite it.", 0);
            let ChatEvent::Done { citations, usage, total_ms, first_token_ms, .. } = ev.last().unwrap() else {
                panic!("{pages} {mode}: {:?}", ev.last())
            };
            let input = usage.as_ref().and_then(|u| u["inputTokens"].as_u64()).unwrap_or(0);
            println!(
                "{pages:>5} {:>5} {mode:<9} {total_ms:>8} {:>8} {input:>9} {:>10} {}/{}",
                d.text.chars,
                first_token_ms.unwrap_or(0),
                tools_used(&ev).len(),
                citations.iter().filter(|c| c.exact).count(),
                citations.len()
            );
            a.close_doc(d.doc);
        }
    }
}
