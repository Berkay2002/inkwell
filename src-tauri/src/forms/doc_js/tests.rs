//! Document JavaScript on generated files: listing, bounds, edits through the edit lane (one undo step each, saved,
//! reopened), the syntax check, the capability scan, and that none of it runs a script.

use super::*;
use crate::outline::tests::{dir, open_bytes, raw_pdf};
use std::sync::Arc;

const USER: EditSource = EditSource::User;

/// Catalog with 3 named scripts (one in a stream), an open action chain ending in a GoTo, WS and DP actions, page O and
/// C actions, a text field with K and C scripts, a link and a bookmark with scripts.
fn fixture() -> Vec<String> {
    vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 5 0 R >> /OpenAction 10 0 R /AA << /WS 11 0 R /DP 12 0 R >> /AcroForm << /Fields [21 0 R] >> /Outlines 30 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA << /O 13 0 R /C 14 0 R >> /Annots [21 0 R 22 0 R] >>".into(),
        "<< >>".into(),
        "<< /Names [(alpha) 6 0 R (beta) 7 0 R (gamma) 8 0 R] >>".into(),
        "<< /S /JavaScript /JS (var a = 1;) >>".into(),
        "<< /S /JavaScript /JS 9 0 R >>".into(),
        "<< /S /JavaScript /JS (for (var i = 0; i < 3; i++) { app.alert(\"x\"); } app.launchURL(\"https://x\"); this.submitForm(\"u\");) >>".into(),
        "<< /Length 19 >>\nstream\nvar fromStream = 2;\nendstream".into(),
        "<< /S /JavaScript /JS (app.alert(\"hi\");) /Next 15 0 R >>".into(),
        "<< /S /JavaScript /JS (this.print({bUI: false});) >>".into(),
        "<< /S /JavaScript /JS (console.println(\"p\"); util.printf(\"%d\", 1);) >>".into(),
        "<< /S /JavaScript /JS (var open = 1;) >>".into(),
        "<< /S /JavaScript /JS (var close = 1;) >>".into(),
        "<< /S /JavaScript /JS (var after = 1;) /Next 16 0 R >>".into(),
        "<< /S /GoTo /D [3 0 R /Fit] >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Total) /Rect [10 10 100 30] /P 3 0 R /AA << /K << /S /JavaScript /JS (event.rc = true;) >> /C << /S /JavaScript /JS (event.value = 1;) >> >> >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [0 0 50 50] /A << /S /JavaScript /JS (app.mailDoc();) >> >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< >>".into(),
        "<< /Type /Outlines /First 31 0 R /Last 31 0 R /Count 1 >>".into(),
        "<< /Title (Go) /Parent 30 0 R /A << /S /JavaScript /JS (exportDataObject({cName: \"x\"});) >> >>".into(),
    ]
}

fn open(test: &str, objs: &[String]) -> (Arc<Engine>, DocId, String) {
    let d = dir("doc-js", test);
    open_bytes(&d, "a.pdf", &raw_pdf(objs))
}

fn reopen(path: &str) -> (Arc<Engine>, DocId) {
    let e = Engine::new(2, 64 << 20);
    let i = e.open(path).unwrap().doc;
    (e, i)
}

fn steps(l: &DocJs, slot: &str) -> Vec<Step> {
    l.doc_actions.iter().chain(&l.page_actions).find(|s| s.id == slot).unwrap_or_else(|| panic!("no slot {slot}")).steps.clone()
}

fn set(eng: &Engine, id: DocId, at: &str, source: &str) -> Applied {
    apply(eng, id, &Op::SetSource { id: at.into(), source: source.into() }, USER).unwrap()
}

fn text_of(eng: &Engine, id: DocId, at: &str) -> String {
    source_of(eng, id, at).unwrap().text
}

#[test]
fn a_file_is_listed_with_every_place_its_scripts_are() {
    let (eng, id, _) = open("list", &fixture());
    let l = list_all(&eng, id).unwrap();
    assert_eq!(l.named.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["alpha", "beta", "gamma"]);
    assert_eq!(l.named[0].id, "n:alpha");
    assert_eq!(l.named[0].meta.size, "var a = 1;".len() as u64);
    // The stream-held script is read as text too.
    assert_eq!(text_of(&eng, id, "n:beta"), "var fromStream = 2;");
    // The open action is a chain: JavaScript, JavaScript, then a GoTo, in order.
    let open = steps(&l, "d:open");
    assert_eq!(open.iter().map(|s| (s.id.as_str(), s.javascript)).collect::<Vec<_>>(), [("d:open:0", true), ("d:open:1", true), ("d:open:2", false)]);
    assert_eq!(open[2].what, "Go to a page");
    assert_eq!(text_of(&eng, id, "d:open:1"), "var after = 1;");
    // All six document slots are there, the unset ones empty.
    assert_eq!(l.doc_actions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["d:open", "d:WC", "d:WS", "d:DS", "d:WP", "d:DP"]);
    assert_eq!(steps(&l, "d:WS").len(), 1);
    assert_eq!(steps(&l, "d:DP").len(), 1);
    assert!(steps(&l, "d:WC").is_empty());
    assert_eq!(text_of(&eng, id, "d:WS:0"), "this.print({bUI: false});");
    // Page actions, 0-based page in the id.
    assert_eq!(l.page_actions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["p:0:O", "p:0:C"]);
    assert_eq!(l.page_actions[0].page, Some(0));
    assert_eq!(text_of(&eng, id, "p:0:C:0"), "var close = 1;");
    // The field's K and C, the link and the bookmark are counted, not listed as scripts to edit.
    assert_eq!(l.counts, Counts { fields: 1, field_scripts: 2, links: 1, bookmarks: 1 });
    assert!(!l.incomplete);
}

#[test]
fn the_risky_call_scan_finds_each_call_and_where_it_is() {
    let (eng, id, _) = open("risks", &fixture());
    let l = list_all(&eng, id).unwrap();
    let cap = |k: &str| l.capabilities.iter().find(|c| c.key == k).unwrap_or_else(|| panic!("no capability {k}: {:?}", l.capabilities));
    assert_eq!(cap("launchUrl").places, ["Document script \"gamma\""]);
    assert_eq!(cap("submitForm").places, ["Document script \"gamma\""]);
    assert_eq!(cap("alertLoop").places, ["Document script \"gamma\""]);
    assert_eq!(cap("print").places, ["Document will save"]);
    assert_eq!(cap("mail").places, ["Link on page 1: action"]);
    assert_eq!(cap("exportData").places, ["Bookmark \"Go\""]);
    // No script imports data, and a println or printf is not a print.
    assert!(l.capabilities.iter().all(|c| c.key != "importData"));
    assert_eq!(l.named[2].meta.risks, ["launchUrl", "submitForm", "alertLoop"]);
    assert!(steps(&l, "d:DP")[0].meta.risks.is_empty());
}

#[test]
fn every_token_is_found_and_look_alikes_are_not() {
    let hit = |src: &str, key: &str| assert!(risks_of(src).iter().any(|k| k == key), "{key} not found in {src}");
    hit("app.launchURL('http://a', true);", "launchUrl");
    hit("this.getURL ('http://a');", "launchUrl");
    hit("this.submitForm({cURL: 'http://a'});", "submitForm");
    hit("this.mailDoc({bUI: false});", "mail");
    hit("this.mailForm(true, 'a@b.c');", "mail");
    hit("this.print({bUI: false});", "print");
    hit("print ()", "print");
    hit("this.exportDataObject({cName: 'a'});", "exportData");
    hit("this.saveAs('x.pdf');", "exportData");
    hit("this.importDataObject('a');", "importData");
    hit("this.importAnFDF('a');", "importData");
    hit("while (true) { app.alert('a'); }", "alertLoop");
    hit("do { app.alert('a'); } while (x);", "alertLoop");
    hit("app.setInterval('a()', 10);", "alertLoop");
    for src in [
        "console.println('print(1)'.length);",
        "util.printf('%d', 3);",
        "myprint(1); reprint(2);",
        "app.alert('once');",
        "var launchURLs = 1; // submitForms",
    ] {
        let r = risks_of(src);
        // A string that mentions print( is only found by a token scan, which is an approximation: it says so.
        if src.starts_with("console.println") {
            assert_eq!(r, ["print"], "{src}");
        } else {
            assert!(r.is_empty(), "{src}: {r:?}");
        }
    }
}

#[test]
fn a_hostile_name_tree_and_huge_scripts_are_bounded() {
    let big = "x".repeat(10 << 20);
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 5 0 R >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
        "<< >>".into(),
        // A node that is its own kid, and a second kid that points back at the first.
        "<< /Kids [5 0 R 6 0 R] /Names [(small) 7 0 R (huge) 8 0 R (stream) 9 0 R] >>".into(),
        "<< /Kids [5 0 R] /Names [(again) 7 0 R] >>".into(),
        "<< /S /JavaScript /JS (var s = 1;) >>".into(),
        format!("<< /S /JavaScript /JS ({big}) >>"),
        "<< /S /JavaScript /JS 10 0 R >>".into(),
        format!("<< /Length {} >>\nstream\n{big}\nendstream", big.len()),
    ];
    let (eng, id, _) = open("hostile", &objs);
    let t = std::time::Instant::now();
    let l = list_all(&eng, id).unwrap();
    assert!(t.elapsed().as_secs() < 20);
    let names: Vec<_> = l.named.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["again", "huge", "small", "stream"]);
    let huge = l.named.iter().find(|n| n.name == "huge").unwrap();
    assert!(huge.meta.too_large && huge.meta.size == 0, "{huge:?}");
    let st = l.named.iter().find(|n| n.name == "stream").unwrap();
    assert!(st.meta.too_large, "{st:?}");
    // Shown as too large, not truncated silently.
    let s = source_of(&eng, id, "n:huge").unwrap();
    assert!(s.too_large && s.text.is_empty());
    assert_eq!(text_of(&eng, id, "n:small"), "var s = 1;");
    // The scan's total is bounded: many 1 MiB-ish scripts leave the rest unread and say so.
    let one = "y".repeat(900 << 10);
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 5 0 R >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
        "<< >>".to_string(),
    ];
    let n = 14;
    let names: String = (0..n).map(|i| format!("(s{i:02}) {} 0 R ", 6 + i)).collect();
    objs.push(format!("<< /Names [{names}] >>"));
    objs.extend((0..n).map(|_| format!("<< /S /JavaScript /JS ({one}) >>")));
    let (eng, id, _) = open("budget", &objs);
    let l = list_all(&eng, id).unwrap();
    assert!(l.incomplete);
    assert!(l.named.iter().any(|n| n.meta.unread));
    assert!(l.named.iter().any(|n| n.meta.size > 0));
}

#[test]
fn a_syntax_error_is_reported_with_its_line_and_column() {
    assert_eq!(check("var a = 1;\nif (a) { a++; }\n"), Check { ok: true, ..Check::default() });
    let c = check("var a = 1;\nvar b = (2 +;\nvar c = 3;");
    assert!(!c.ok);
    assert_eq!(c.line, Some(2), "{c:?}");
    assert!(c.column.is_some_and(|c| c >= 1), "{c:?}");
    assert!(c.message.as_deref().is_some_and(|m| !m.is_empty()));
    // Top-level `return` is not valid in a document script.
    assert!(!check("return 1;").ok);
    let big = check(&"a;".repeat(MAX_SCRIPT_BYTES));
    assert!(big.ok && big.too_large);
}

#[test]
fn checking_compiles_and_never_runs() {
    // A script that would raise an alert, write a global and loop forever is only compiled.
    let c = check("var __ran = 1; app.alert('x'); while (true) {}");
    assert!(c.ok, "{c:?}");
}

#[test]
fn every_edit_is_one_undo_step_that_survives_save_and_reopen() {
    let (eng, id, path) = open("edits", &fixture());
    let before = list_all(&eng, id).unwrap();
    let steps_before = eng.history(id).unwrap().len();
    let check_step = |op: Op, expect: &dyn Fn(&DocJs)| {
        let was = list_all(&eng, id).unwrap();
        let n = eng.history(id).unwrap().len();
        apply(&eng, id, &op, USER).unwrap();
        assert_eq!(eng.history(id).unwrap().len(), n + 1, "one step for {op:?}");
        expect(&list_all(&eng, id).unwrap());
        eng.undo(id).unwrap();
        assert_eq!(list_all(&eng, id).unwrap(), was, "undo restores {op:?}");
        eng.redo(id).unwrap();
    };
    check_step(Op::Add { name: None, source: "var n = 1;".into() }, &|l| assert!(l.named.iter().any(|n| n.name == "Script 1")));
    check_step(Op::Add { name: Some("  mine ".into()), source: String::new() }, &|l| assert!(l.named.iter().any(|n| n.name == "mine")));
    check_step(Op::Rename { id: "n:alpha".into(), name: "omega".into() }, &|l| {
        assert!(l.named.iter().any(|n| n.name == "omega") && l.named.iter().all(|n| n.name != "alpha"));
    });
    check_step(Op::SetSource { id: "n:omega".into(), source: "var a = 2;".into() }, &|l| {
        assert_eq!(l.named.iter().find(|n| n.name == "omega").unwrap().meta.size, 10);
    });
    check_step(Op::Delete { id: "n:beta".into() }, &|l| assert!(l.named.iter().all(|n| n.name != "beta")));
    check_step(Op::SetSource { id: "d:WC:0".into(), source: "var closing = 1;".into() }, &|l| assert_eq!(steps(l, "d:WC").len(), 1));
    check_step(Op::SetSource { id: "p:0:O:0".into(), source: "var o2 = 1;".into() }, &|l| assert_eq!(steps(l, "p:0:O").len(), 1));
    check_step(Op::Delete { id: "d:WS:0".into() }, &|l| assert!(steps(l, "d:WS").is_empty()));
    check_step(Op::Delete { id: "d:open".into() }, &|l| assert!(steps(l, "d:open").is_empty()));
    // Everything above is redone: save, reopen and read it back.
    assert_eq!(eng.history(id).unwrap().len(), steps_before + 9);
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    let after = list_all(&eng2, id2).unwrap();
    assert_eq!(after.named.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Script 1", "gamma", "mine", "omega"]);
    assert_eq!(text_of(&eng2, id2, "n:omega"), "var a = 2;");
    assert_eq!(text_of(&eng2, id2, "d:WC:0"), "var closing = 1;");
    assert_eq!(text_of(&eng2, id2, "p:0:O:0"), "var o2 = 1;");
    assert!(steps(&after, "d:WS").is_empty() && steps(&after, "d:open").is_empty());
    // The rest is as it was.
    assert_eq!(after.counts, before.counts);
    assert_eq!(steps(&after, "d:DP"), steps(&before, "d:DP"));
}

#[test]
fn script_names_are_unique_and_the_default_is_script_n() {
    let (eng, id, _) = open("names", &fixture());
    let a = apply(&eng, id, &Op::Add { name: None, source: String::new() }, USER).unwrap();
    assert_eq!(a.id.as_deref(), Some("n:Script 1"));
    let b = apply(&eng, id, &Op::Add { name: None, source: String::new() }, USER).unwrap();
    assert_eq!(b.id.as_deref(), Some("n:Script 2"));
    let steps_now = eng.history(id).unwrap().len();
    // Refused: a name taken, an empty name, a rename onto another script. None is a step.
    assert!(apply(&eng, id, &Op::Add { name: Some("alpha".into()), source: String::new() }, USER).is_err());
    assert!(apply(&eng, id, &Op::Add { name: Some("   ".into()), source: "x".into() }, USER).is_ok_and(|a| a.id.as_deref() == Some("n:Script 3")));
    assert!(apply(&eng, id, &Op::Rename { id: "n:alpha".into(), name: "gamma".into() }, USER).is_err());
    assert!(apply(&eng, id, &Op::Rename { id: "n:alpha".into(), name: "".into() }, USER).is_err());
    assert_eq!(eng.history(id).unwrap().len(), steps_now + 1);
    // The same name, the same text and clearing an empty slot change nothing.
    let n = eng.history(id).unwrap().len();
    apply(&eng, id, &Op::Rename { id: "n:alpha".into(), name: "alpha".into() }, USER).unwrap();
    set(&eng, id, "n:alpha", "var a = 1;");
    apply(&eng, id, &Op::Delete { id: "d:WC".into() }, USER).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), n);
}

#[test]
fn a_script_keeps_what_follows_it_and_non_ascii_text_survives() {
    let (eng, id, path) = open("chain", &fixture());
    // Replacing the head of the open chain keeps its /Next.
    set(&eng, id, "d:open:0", "var head = 'h\u{e9}llo \u{2713} \u{65e5}\u{672c}';");
    let l = list_all(&eng, id).unwrap();
    assert_eq!(steps(&l, "d:open").len(), 3);
    // A step that is not JavaScript is not replaced from inside a chain.
    assert!(apply(&eng, id, &Op::SetSource { id: "d:open:2".into(), source: "x".into() }, USER).is_err());
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(text_of(&eng2, id2, "d:open:0"), "var head = 'h\u{e9}llo \u{2713} \u{65e5}\u{672c}';");
    assert_eq!(text_of(&eng2, id2, "d:open:1"), "var after = 1;");
    // A name with non-ASCII text is kept too.
    apply(&eng2, id2, &Op::Add { name: Some("r\u{e4}ksm\u{f6}rg\u{e5}s".into()), source: "var s = 1;".into() }, USER).unwrap();
    eng2.save(id2, None).unwrap();
    let (eng3, id3) = reopen(&path);
    assert!(list_all(&eng3, id3).unwrap().named.iter().any(|n| n.name == "r\u{e4}ksm\u{f6}rg\u{e5}s"));
    assert_eq!(text_of(&eng3, id3, "n:r\u{e4}ksm\u{f6}rg\u{e5}s"), "var s = 1;");
}

#[test]
fn an_open_action_that_is_not_javascript_is_shown_cleared_or_replaced() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /OpenAction [3 0 R /Fit] >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
    ];
    let (eng, id, _) = open("openaction", &objs);
    let open = steps(&list_all(&eng, id).unwrap(), "d:open");
    assert_eq!(open.len(), 1);
    assert!(!open[0].javascript);
    assert_eq!(open[0].what, "Go to a page");
    // Cleared.
    apply(&eng, id, &Op::Delete { id: "d:open".into() }, USER).unwrap();
    assert!(steps(&list_all(&eng, id).unwrap(), "d:open").is_empty());
    eng.undo(id).unwrap();
    // Or replaced by JavaScript.
    set(&eng, id, "d:open:0", "app.alert('hi');");
    let open = steps(&list_all(&eng, id).unwrap(), "d:open");
    assert!(open[0].javascript);
    assert_eq!(text_of(&eng, id, "d:open:0"), "app.alert('hi');");
}

#[test]
fn clearing_the_last_action_removes_the_empty_aa_dictionary() {
    let (eng, id, path) = open("aa", &fixture());
    apply(&eng, id, &Op::Delete { id: "d:WS".into() }, USER).unwrap();
    apply(&eng, id, &Op::Delete { id: "d:DP".into() }, USER).unwrap();
    apply(&eng, id, &Op::Delete { id: "p:0:O".into() }, USER).unwrap();
    apply(&eng, id, &Op::Delete { id: "p:0:C".into() }, USER).unwrap();
    eng.save(id, None).unwrap();
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    assert!(doc.catalog().unwrap().get_dict("AA").unwrap().is_none());
    assert!(doc.find_page(0).unwrap().get_dict("AA").unwrap().is_none());
}

#[test]
fn replaced_and_deleted_text_is_not_left_in_the_saved_file() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 4 0 R >> /AA << /WS 5 0 R >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
        "<< /Names [(one) 6 0 R (two) 7 0 R] >>".into(),
        "<< /S /JavaScript /JS (var SECRETSAVE = 1;) >>".into(),
        "<< /S /JavaScript /JS (var SECRETONE = 1;) >>".into(),
        "<< /S /JavaScript /JS (var SECRETTWO = 1;) >>".into(),
    ];
    let (eng, id, path) = open("privacy", &objs);
    set(&eng, id, "n:one", "var replaced = 1;");
    apply(&eng, id, &Op::Delete { id: "n:two".into() }, USER).unwrap();
    apply(&eng, id, &Op::Delete { id: "d:WS".into() }, USER).unwrap();
    eng.save(id, None).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let has = |needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
    for secret in ["SECRETONE", "SECRETTWO", "SECRETSAVE"] {
        assert!(!has(secret), "{secret} is still in the saved file");
    }
    assert!(has("replaced"));
}

#[test]
fn listing_reading_checking_and_editing_never_run_a_script() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 4 0 R >> /OpenAction 6 0 R /AcroForm << /Fields [7 0 R] >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [7 0 R] >>".into(),
        "<< /Names [(init) 5 0 R] >>".into(),
        "<< /S /JavaScript /JS (console.println(\"RAN-NAMED\"); app.alert(\"named\"); var __trace = 1;) >>".into(),
        "<< /S /JavaScript /JS (console.println(\"RAN-OPEN\"); app.alert(\"open\");) >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (T) /Rect [10 10 100 30] /P 3 0 R /V (a) /AA << /F << /S /JavaScript /JS (event.value = event.value + \"!\";) >> >> >>".into(),
    ];
    let (eng, id, _) = open("notrace", &objs);
    let (scripts, _) = eng.doc_scripts(id).unwrap();
    list_all(&eng, id).unwrap();
    text_of(&eng, id, "n:init");
    text_of(&eng, id, "d:open:0");
    check("console.println('RAN-CHECK'); app.alert('x');");
    set(&eng, id, "n:init", "console.println(\"RAN-EDIT\"); var __trace = 2;");
    apply(&eng, id, &Op::Add { name: None, source: "app.alert('added');".into() }, USER).unwrap();
    apply(&eng, id, &Op::Rename { id: "n:init".into(), name: "init2".into() }, USER).unwrap();
    apply(&eng, id, &Op::Delete { id: "d:open".into() }, USER).unwrap();
    eng.undo(id).unwrap();
    assert!(scripts.log().is_empty(), "{:?}", scripts.log_text());
    assert_eq!(scripts.has_scripts(), None, "nothing built the script engine");
    // The forms runtime is what runs document scripts, at its own policy: here it does, which shows the log can see one.
    let f = crate::forms::list_all(&eng, id).unwrap();
    assert_eq!(f.fields.len(), 1);
    assert!(scripts.log_text().iter().any(|l| l.contains("RAN-EDIT")), "{:?}", scripts.log_text());
}

#[test]
fn a_name_tree_cut_short_by_a_cap_is_reported_and_not_edited() {
    // 11 nested /Kids nodes: the walk stops at depth 8, so the scripts below were never seen
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 5 0 R >> >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into(),
        "<< >>".into(),
    ];
    let depth = 11;
    for i in 0..depth {
        let kids = if i + 1 < depth { format!("/Kids [{} 0 R]", 6 + i) } else { String::new() };
        objs.push(format!("<< {kids} /Names [(k{i:02}) {} 0 R] >>", 5 + depth + i));
    }
    objs.extend((0..depth).map(|i| format!("<< /S /JavaScript /JS (var v{i} = {i};) >>")));
    let (eng, id, _) = open("cut-tree", &objs);
    let l = list_all(&eng, id).unwrap();
    assert!(l.incomplete);
    assert!(l.named.len() < depth);
    let seen = l.named.len();
    for op in [
        Op::Add { name: Some("new".into()), source: "var n;".into() },
        Op::Rename { id: "n:k00".into(), name: "other".into() },
        Op::SetSource { id: "n:k00".into(), source: "var z;".into() },
        Op::Delete { id: "n:k00".into() },
    ] {
        let e = apply(&eng, id, &op, USER).unwrap_err();
        assert!(format!("{e:?}").contains("too large or too deeply nested"), "{e:?}");
    }
    // nothing was written: still the same entries, and reading still works
    assert_eq!(list_all(&eng, id).unwrap().named.len(), seen);
    assert_eq!(text_of(&eng, id, "n:k00"), "var v0 = 0;");
}

#[test]
fn an_action_shared_by_several_pages_is_not_changed_under_the_others() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA << /O 5 0 R >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA << /O 5 0 R >> >>".into(),
        "<< /S /JavaScript /JS (var shared = 1;) /Next 6 0 R >>".into(),
        "<< /S /JavaScript /JS (var after = 1;) >>".into(),
    ];
    let (eng, id, _) = open("shared-action", &objs);
    set(&eng, id, "p:0:O:0", "var mine = 2;");
    assert_eq!(text_of(&eng, id, "p:0:O:0"), "var mine = 2;");
    assert_eq!(text_of(&eng, id, "p:1:O:0"), "var shared = 1;");
    // what followed the head still follows it, on both pages
    assert_eq!(text_of(&eng, id, "p:0:O:1"), "var after = 1;");
    assert_eq!(text_of(&eng, id, "p:1:O:1"), "var after = 1;");
}

#[test]
fn a_listing_of_streams_that_fail_to_read_is_still_bounded() {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 5 0 R >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
        "<< >>".to_string(),
    ];
    let n = 40;
    let names: String = (0..n).map(|i| format!("(s{i:02}) {} 0 R ", 6 + i)).collect();
    objs.push(format!("<< /Names [{names}] >>"));
    objs.extend((0..n).map(|i| format!("<< /S /JavaScript /JS {} 0 R >>", 6 + n + i)));
    // a stream whose decode parameters make MuPDF refuse it (columns overflow): reading it is an error
    objs.extend((0..n).map(|_| "<< /Length 5 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns 2000000000 /Colors 4 /BitsPerComponent 16 >> >>\nstream\nhello\nendstream".to_string()));
    let (eng, id, _) = open("failing-streams", &objs);
    let l = list_all(&eng, id).unwrap();
    // each failed read is charged: the budget (8 MiB at 1 MiB a read) runs out well before 40
    assert!(l.incomplete);
    assert!(l.named.iter().any(|n| n.meta.unread), "{:?}", l.named.iter().map(|n| (n.meta.unread, n.meta.size)).collect::<Vec<_>>());
}

/// A one-page file with a named script and a page action, a signature field with a value, and `perms` in the catalog.
fn signed_objs(perms: &str) -> Vec<String> {
    vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Names << /JavaScript 5 0 R >> /AcroForm << /SigFlags 3 /Fields [6 0 R] >> {perms} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA << /O 7 0 R >> >>".into(),
        "<< >>".into(),
        "<< /Names [(alpha) 8 0 R] >>".into(),
        "<< /FT /Sig /T (Signature1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>".into(),
        "<< /S /JavaScript /JS (var open = 1;) >>".into(),
        "<< /S /JavaScript /JS (var a = 1;) >>".into(),
    ]
}

#[test]
fn editing_a_signed_file_saves_an_update_and_warns() {
    let (eng, id, path) = open("signed", &signed_objs(""));
    let l = list_all(&eng, id).unwrap();
    assert!(l.signed && !l.certified);
    let original = std::fs::read(&path).unwrap();
    let warned = |a: Applied| assert!(a.warning.is_some_and(|w| w.contains("signature")), "an edit of a signed file warns");
    warned(apply(&eng, id, &Op::SetSource { id: "n:alpha".into(), source: "var b = 2;".into() }, USER).unwrap());
    warned(apply(&eng, id, &Op::Delete { id: "p:0:O:0".into() }, USER).unwrap());
    // the same text again is no edit and no warning
    assert!(apply(&eng, id, &Op::SetSource { id: "n:alpha".into(), source: "var b = 2;".into() }, USER).unwrap().warning.is_none());
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the signed bytes are kept: the edit is an appended update");
    let (eng2, id2) = reopen(&path);
    assert_eq!(text_of(&eng2, id2, "n:alpha"), "var b = 2;");
    assert!(list_all(&eng2, id2).unwrap().page_actions.is_empty());
}

#[test]
fn a_certified_file_takes_no_script_edit_at_any_level() {
    for p in 1..=3 {
        let perms = format!("/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P {p} >> >> ] >> >>");
        let (eng, id, _) = open(&format!("certified-{p}"), &signed_objs(&perms));
        assert!(list_all(&eng, id).unwrap().certified, "P {p}");
        let steps = eng.history(id).unwrap().len();
        for op in [
            Op::Add { name: Some("new".into()), source: "var n;".into() },
            Op::Rename { id: "n:alpha".into(), name: "other".into() },
            Op::SetSource { id: "n:alpha".into(), source: "var z;".into() },
            Op::SetSource { id: "p:0:O:0".into(), source: "var z;".into() },
            Op::Delete { id: "n:alpha".into() },
            Op::Delete { id: "p:0:O:0".into() },
        ] {
            let e = apply(&eng, id, &op, USER).unwrap_err();
            assert!(format!("{e:?}").contains("certified"), "P {p}, {op:?}: {e:?}");
        }
        assert_eq!(eng.history(id).unwrap().len(), steps, "P {p}: no step was made");
        assert_eq!(text_of(&eng, id, "n:alpha"), "var a = 1;");
    }
}

#[test]
fn an_aa_dictionary_shared_by_two_pages_is_not_changed_under_the_other() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA 5 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA 5 0 R >>".into(),
        "<< /O 6 0 R /C 7 0 R >>".into(),
        "<< /S /JavaScript /JS (var open = 1;) >>".into(),
        "<< /S /JavaScript /JS (var close = 1;) >>".into(),
    ];
    let (eng, id, path) = open("shared-aa", &objs);
    set(&eng, id, "p:0:O:0", "var mine = 2;");
    assert_eq!(text_of(&eng, id, "p:0:O:0"), "var mine = 2;");
    assert_eq!(text_of(&eng, id, "p:0:C:0"), "var close = 1;", "the page's other events came along");
    assert_eq!(text_of(&eng, id, "p:1:O:0"), "var open = 1;");
    apply(&eng, id, &Op::Delete { id: "p:0:C".into() }, USER).unwrap();
    let l = list_all(&eng, id).unwrap();
    assert!(l.page_actions.iter().all(|s| s.id != "p:0:C"));
    assert_eq!(text_of(&eng, id, "p:1:C:0"), "var close = 1;");
    assert_eq!(text_of(&eng, id, "p:1:O:0"), "var open = 1;");
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(text_of(&eng2, id2, "p:0:O:0"), "var mine = 2;");
    assert_eq!(text_of(&eng2, id2, "p:1:C:0"), "var close = 1;");
}

#[test]
fn a_next_chain_shared_by_two_actions_is_copied_up_to_the_edited_step() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA << /O 5 0 R >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA << /O 6 0 R >> >>".into(),
        "<< /S /JavaScript /JS (var head0 = 1;) /Next 7 0 R >>".into(),
        "<< /S /JavaScript /JS (var head1 = 1;) /Next 7 0 R >>".into(),
        "<< /S /JavaScript /JS (var tail = 1;) /Next 8 0 R >>".into(),
        "<< /S /JavaScript /JS (var end = 1;) >>".into(),
    ];
    let (eng, id, path) = open("shared-next", &objs);
    let steps = eng.history(id).unwrap().len();
    set(&eng, id, "p:0:O:1", "var tail2 = 2;");
    let page = |p: u32| (0..3).filter_map(|i| source_of(&eng, id, &format!("p:{p}:O:{i}")).ok().map(|s| s.text)).collect::<Vec<_>>();
    assert_eq!(page(0), ["var head0 = 1;", "var tail2 = 2;", "var end = 1;"]);
    assert_eq!(page(1), ["var head1 = 1;", "var tail = 1;", "var end = 1;"]);
    // a step further down: both links above it are copied
    set(&eng, id, "p:1:O:2", "var end2 = 3;");
    assert_eq!(page(1), ["var head1 = 1;", "var tail = 1;", "var end2 = 3;"]);
    assert_eq!(page(0), ["var head0 = 1;", "var tail2 = 2;", "var end = 1;"]);
    // one undo step each, and the copies survive a save
    assert_eq!(eng.history(id).unwrap().len(), steps + 2);
    eng.save(id, None).unwrap();
    let (eng2, id2) = reopen(&path);
    assert_eq!(text_of(&eng2, id2, "p:0:O:1"), "var tail2 = 2;");
    assert_eq!(text_of(&eng2, id2, "p:1:O:2"), "var end2 = 3;");
}

#[test]
fn a_step_in_a_next_array_is_edited_on_a_copy_of_the_head() {
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA << /O 5 0 R >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /AA << /O 5 0 R >> >>".into(),
        "<< /S /JavaScript /JS (var h = 1;) /Next [6 0 R 7 0 R] >>".into(),
        "<< /S /JavaScript /JS (var a = 1;) >>".into(),
        "<< /S /JavaScript /JS (var b = 1;) >>".into(),
    ];
    let (eng, id, _) = open("next-array", &objs);
    set(&eng, id, "p:0:O:2", "var b2 = 2;");
    let texts = |p: u32| (0..3).map(|i| text_of(&eng, id, &format!("p:{p}:O:{i}"))).collect::<Vec<_>>();
    assert_eq!(texts(0), ["var h = 1;", "var a = 1;", "var b2 = 2;"]);
    assert_eq!(texts(1), ["var h = 1;", "var a = 1;", "var b = 1;"]);
}
