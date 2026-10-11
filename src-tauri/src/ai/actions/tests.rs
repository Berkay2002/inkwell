//! The assistant's changing tools on real engine documents (temporary copies, never a user's file):
//! what each one does, that it is one undo step of the turn, and that a call that changes nothing,
//! or must not touch a signed document, is an `Err` the model can read.

use super::*;
use crate::ai::text::DocText;
use crate::ai::tools::{DocHost, Mode};
use crate::engine::edit::EditSource;
use crate::engine::fixture::{self, Fixture};
use crate::engine::{DocId, Engine, OutlineItem};
use crate::redact::app::list_marks;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

const SAMPLE: &[u8] = include_bytes!("../../../../docs/study/samples/redact-source.pdf");

struct Host {
    eng: Arc<Engine>,
    doc: DocId,
    edited: AtomicBool,
    lane: bool,
}

impl DocHost for Host {
    fn meta(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
    fn outline(&self) -> Result<Vec<OutlineItem>, String> {
        Ok(Vec::new())
    }
    fn page_png(&self, _: u32, _: u32, _: u32) -> Result<Vec<u8>, String> {
        Err("no renderer".into())
    }
    fn lane(&self) -> Option<Lane<'_>> {
        self.lane.then(|| Lane {
            eng: &self.eng,
            doc: self.doc,
            source: EditSource::Assistant { turn: "t1".into() },
            edited: &self.edited,
        })
    }
}

fn open_bytes(test: &str, bytes: Vec<u8>) -> Host {
    let d: PathBuf = std::env::temp_dir().join("inkwell-tests").join(format!("ai-actions-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("a.pdf");
    std::fs::write(&p, bytes).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let doc = eng.open(&p.to_string_lossy()).unwrap().doc;
    Host { eng, doc, edited: AtomicBool::new(false), lane: true }
}

/// Three pages; "needle" is on pages 1 and 3.
fn fixture_doc(test: &str) -> Host {
    open_bytes(test, fixture::build(&Fixture { pages: 3, lines: 4, needle_every: 2, ..Default::default() }))
}

fn call(h: &Host, name: &str, args: Value) -> Result<Output, String> {
    let t = tools().into_iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"));
    let text = DocText { rev: 0, pages: 0, chunks: Vec::new(), chars: 0 };
    let c = Ctx { text: &text, host: h, name: "a.pdf", current_page: 0, selection: None };
    (t.run)(&c, &args)
}

fn ok(h: &Host, name: &str, args: Value) -> Output {
    call(h, name, args).unwrap_or_else(|e| panic!("{name} failed: {e}"))
}

fn err_of(h: &Host, name: &str, args: Value) -> String {
    match call(h, name, args) {
        Ok(o) => panic!("{name} should have failed, said: {}", o.text),
        Err(e) => e,
    }
}

fn steps(h: &Host) -> usize {
    h.eng.history(h.doc).unwrap().len()
}

fn rev(h: &Host) -> u32 {
    h.eng.edit_state(h.doc).unwrap().rev
}

fn marks(h: &Host) -> Vec<crate::redact::app::MarkDto> {
    list_marks(&h.eng, h.doc).unwrap()
}

fn needle_hits(h: &Host) -> usize {
    let q = FindQuery::Text { needle: "needle".into(), case_sensitive: false, whole_word: false };
    crate::redact::app::find(&h.eng, h.doc, &q).unwrap().len()
}

fn sign(h: &Host) {
    h.eng
        .apply_custom(h.doc, "Sign".into(), EditSource::User, false, |d| {
            let f = d.new_object_from_str("<< /Fields [] /SigFlags 3 >>")?;
            d.catalog()?.dict_put("AcroForm", f)?;
            Ok(vec![0])
        })
        .unwrap();
}

fn assert_edited(o: &Output) {
    assert!(matches!(o.ui, Some(UiAction::Edited { .. })), "{:?}", o.ui);
}

// ---- redaction ---------------------------------------------------------------------------------

#[test]
fn marking_by_text_marks_every_match_as_one_undo_step() {
    let h = fixture_doc("mark-text");
    let before = steps(&h);
    let o = ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    assert_edited(&o);
    assert!(o.text.contains("Redact panel"), "{}", o.text);
    assert!(o.text.contains("p. 1") && o.text.contains("p. 3"), "{}", o.text);
    assert_eq!(marks(&h).iter().map(|m| m.page).collect::<Vec<_>>(), [0, 2]);
    assert_eq!(steps(&h), before + 1, "one call is one step");
    assert!(h.edited.load(std::sync::atomic::Ordering::SeqCst));
    let last = h.eng.history(h.doc).unwrap().pop().unwrap();
    assert_eq!(last.source, EditSource::Assistant { turn: "t1".into() });

    h.eng.undo(h.doc).unwrap();
    assert!(marks(&h).is_empty(), "undo takes the marks back");
}

#[test]
fn marking_can_be_limited_to_pages_and_use_a_regex() {
    let h = fixture_doc("mark-pages");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle", "pages": [3] }));
    assert_eq!(marks(&h).iter().map(|m| m.page).collect::<Vec<_>>(), [2]);
    let o = ok(&h, "redact_find_and_mark", json!({ "regex": r"line \d of page 2" }));
    assert!(o.text.contains("p. 2"), "{}", o.text);
    assert_eq!(marks(&h).len(), 1 + 4, "the needle on page 3 and the four lines of page 2");
}

#[test]
fn marking_by_pattern_finds_the_sample_ssns_and_does_not_echo_them() {
    let h = open_bytes("mark-pattern", SAMPLE.to_vec());
    let o = ok(&h, "redact_find_and_mark", json!({ "pattern": "ssn" }));
    assert_eq!(marks(&h).len(), 5);
    assert!(!o.text.contains("123-45-6789"), "a matched value is masked: {}", o.text);
    assert_edited(&o);
}

#[test]
fn a_marking_that_changes_nothing_is_an_error() {
    let h = fixture_doc("mark-noop");
    let e = err_of(&h, "redact_find_and_mark", json!({ "text": "zebra-not-there" }));
    assert!(e.contains("No match"), "{e}");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    let (r, s) = (rev(&h), steps(&h));
    let e = err_of(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    assert!(e.contains("already marked"), "{e}");
    assert_eq!((rev(&h), steps(&h)), (r, s), "nothing was pushed");
    // bad input is explained, not guessed at
    assert!(err_of(&h, "redact_find_and_mark", json!({})).contains("one of"));
    assert!(err_of(&h, "redact_find_and_mark", json!({ "text": "a", "pattern": "email" })).contains("only one"));
    assert!(err_of(&h, "redact_find_and_mark", json!({ "pattern": "zipcode" })).contains("Unknown pattern"));
    assert!(err_of(&h, "redact_find_and_mark", json!({ "regex": "(" })).contains("compile"));
    assert!(err_of(&h, "redact_find_and_mark", json!({ "text": "needle", "pages": [9] })).contains("does not exist"));
}

#[test]
fn an_area_is_marked_listed_and_unmarked() {
    let h = fixture_doc("area");
    let area = json!({ "page": 2, "left": 0.1, "top": 0.1, "right": 0.5, "bottom": 0.2 });
    let o = ok(&h, "redact_mark_area", area.clone());
    assert_edited(&o);
    let m = marks(&h);
    assert_eq!((m.len(), m[0].page), (1, 1));
    let l = ok(&h, "redact_list_marks", json!({}));
    let id = format!("p2#{}", m[0].xref);
    assert!(l.text.contains(&id), "{}", l.text);
    assert!(l.ui.is_none(), "listing is a read");
    // the same area again, and an empty or off-page one, change nothing
    assert!(err_of(&h, "redact_mark_area", area).contains("already"));
    assert!(err_of(&h, "redact_mark_area", json!({ "page": 2, "left": 0.5, "top": 0.1, "right": 0.1, "bottom": 0.2 })).contains("empty"));
    assert!(err_of(&h, "redact_mark_area", json!({ "page": 9, "left": 0.1, "top": 0.1, "right": 0.5, "bottom": 0.2 })).contains("does not exist"));
    assert!(err_of(&h, "redact_mark_area", json!({ "page": 1, "left": 0.1, "top": 0.1, "right": 1.5, "bottom": 0.2 })).contains("between 0 and 1"));

    assert!(err_of(&h, "redact_unmark", json!({ "marks": ["p2#99999"] })).contains("no mark"));
    ok(&h, "redact_unmark", json!({ "marks": [id] }));
    assert!(marks(&h).is_empty());
    assert!(err_of(&h, "redact_unmark", json!({ "all": true })).contains("no redaction marks"));
}

#[test]
fn applying_asks_in_every_mode_and_the_card_names_what_is_redacted() {
    let t = tools().into_iter().find(|t| t.name == "redact_apply").unwrap();
    assert_eq!(t.class, Class::Permanent);
    for m in [Mode::Ask, Mode::Auto, Mode::Full] {
        assert!(m.needs_approval(t.class), "{m:?} must ask before a redaction");
    }
    let label = (t.label)(&json!({ "expected_marks": 3, "pages": [1, 2, 3] }));
    assert_eq!(label, "Permanently redact 3 marks on pp. 1-3");
    assert_eq!((t.label)(&json!({ "expected_marks": 1, "pages": [4] })), "Permanently redact 1 mark on p. 4");
    // marking and listing never wait in Auto
    for t in tools().into_iter().filter(|t| t.class != Class::Permanent && t.class != Class::Destructive) {
        assert!(!Mode::Auto.needs_approval(t.class), "{}", t.name);
    }
}

#[test]
fn apply_removes_the_text_and_one_undo_brings_the_turn_back() {
    let h = fixture_doc("apply");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    assert_eq!(needle_hits(&h), 2);

    // what the user was shown must be what is there
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 5, "pages": [1, 3] }));
    assert!(e.contains("not what you listed"), "{e}");
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1] }));
    assert!(e.contains("not what you listed"), "{e}");
    assert_eq!(needle_hits(&h), 2, "a refused apply changed nothing");

    let o = ok(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3] }));
    match &o.ui {
        Some(UiAction::Edited { redacted: Some(r), .. }) => {
            assert_eq!(r["marks"], 2);
            assert!(r["verify"]["leaks"].as_array().unwrap().is_empty(), "{r}");
        }
        other => panic!("{other:?}"),
    }
    assert!(o.text.contains("Checked"), "{}", o.text);
    assert_eq!(needle_hits(&h), 0, "the text is gone");
    assert!(marks(&h).is_empty());
    assert!(h.eng.edit_state(h.doc).unwrap().redacted);

    // marking and applying were one turn: one undo takes both back
    h.eng.undo(h.doc).unwrap();
    assert_eq!(needle_hits(&h), 2, "undo restores the text");
}

#[test]
fn applying_without_marks_is_an_error() {
    let h = fixture_doc("apply-none");
    let (r, s) = (rev(&h), steps(&h));
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 1, "pages": [1] }));
    assert!(e.contains("nothing marked"), "{e}");
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

// ---- comments ----------------------------------------------------------------------------------

#[test]
fn highlights_notes_and_replies_are_one_step_each() {
    let h = fixture_doc("comments");
    let s0 = steps(&h);
    let o = ok(&h, "add_comment", json!({ "kind": "highlight", "find": "needle", "text": "check this" }));
    assert_edited(&o);
    assert_eq!(steps(&h), s0 + 1, "two highlights, one step");
    let all = annot::list_all(&h.eng, h.doc).unwrap();
    assert_eq!(all.iter().filter(|a| a.kind == Kind::Highlight).count(), 2);
    assert!(all.iter().all(|a| a.contents == "check this"));

    ok(&h, "add_comment", json!({ "kind": "underline", "find": "needle", "pages": [3], "first_only": true }));
    ok(&h, "add_comment", json!({ "kind": "strikeout", "find": "QUICK", "pages": [1] }));
    let o = ok(&h, "add_comment", json!({ "kind": "note", "page": 1, "x": 0.5, "y": 0.5, "text": "Is this right?" }));
    assert_eq!(steps(&h), s0 + 4);
    let note = annot::list_all(&h.eng, h.doc).unwrap().into_iter().find(|a| a.kind == Kind::Note).unwrap();
    assert!(o.text.contains("note"));
    let id = format!("p1#{}", note.xref);
    assert!(ok(&h, "list_comments", json!({})).text.contains(&id));

    ok(&h, "add_comment", json!({ "kind": "reply", "parent": id, "text": "Yes." }));
    let all = annot::list_all(&h.eng, h.doc).unwrap();
    assert!(all.iter().any(|a| a.parent == Some(note.xref) && a.contents == "Yes."));
    assert!(all.iter().any(|a| a.author == AUTHOR));
}

#[test]
fn a_comment_that_cannot_be_made_is_an_error_and_changes_nothing() {
    let h = fixture_doc("comments-err");
    let (r, s) = (rev(&h), steps(&h));
    assert!(err_of(&h, "add_comment", json!({ "kind": "highlight", "find": "not in the file" })).contains("does not appear"));
    assert!(err_of(&h, "add_comment", json!({ "kind": "highlight" })).contains("\"find\""));
    assert!(err_of(&h, "add_comment", json!({ "kind": "note", "page": 1, "x": 0.1, "y": 0.1 })).contains("\"text\""));
    assert!(err_of(&h, "add_comment", json!({ "kind": "reply", "parent": "p1#9999", "text": "x" })).contains("no comment"));
    assert!(err_of(&h, "add_comment", json!({ "kind": "reply", "parent": "nonsense", "text": "x" })).contains("not a comment id"));
    assert!(err_of(&h, "add_comment", json!({ "kind": "scribble" })).contains("Unknown kind"));
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

// ---- forms -------------------------------------------------------------------------------------

fn with_field(test: &str) -> Host {
    let h = fixture_doc(test);
    let f = forms::NewField {
        kind: FieldKind::Text,
        rect: [72.0, 100.0, 272.0, 120.0],
        name: Some("Full name".into()),
        group: None,
        export: None,
        caption: None,
        options: None,
    };
    forms::add_one(&h.eng, h.doc, 0, &f, EditSource::User).unwrap();
    h
}

fn name_value(h: &Host) -> String {
    forms::list_all(&h.eng, h.doc).unwrap().fields.iter().find(|f| f.name == "Full name").unwrap().value.clone()
}

#[test]
fn form_fields_are_listed_and_filled_in_one_step() {
    let h = with_field("forms");
    let l = ok(&h, "list_form_fields", json!({}));
    assert!(l.text.contains("\"Full name\""), "{}", l.text);
    let s0 = steps(&h);
    let o = ok(&h, "fill_form_fields", json!({ "fields": [{ "field": "full name", "value": "Alex Johnson" }] }));
    assert_edited(&o);
    assert_eq!(name_value(&h), "Alex Johnson");
    assert_eq!(steps(&h), s0 + 1);
    h.eng.undo(h.doc).unwrap();
    assert_eq!(name_value(&h), "");
}

#[test]
fn filling_is_all_or_nothing_and_a_no_op_is_an_error() {
    let h = with_field("forms-err");
    ok(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] }));
    let (r, s) = (rev(&h), steps(&h));
    assert!(err_of(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] })).contains("already"));
    let e = err_of(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "B" }, { "field": "Nope", "value": "x" }] }));
    assert!(e.contains("no form field"), "{e}");
    assert_eq!(name_value(&h), "A");
    assert_eq!((rev(&h), steps(&h)), (r, s));
    assert!(err_of(&h, "fill_form_fields", json!({ "fields": [] })).contains("fields"));
}

#[test]
fn a_document_without_a_form_says_so() {
    let h = fixture_doc("no-form");
    assert!(ok(&h, "list_form_fields", json!({})).text.contains("no form fields"));
    assert!(err_of(&h, "fill_form_fields", json!({ "fields": [{ "field": "a", "value": "b" }] })).contains("no form fields"));
}

// ---- pages -------------------------------------------------------------------------------------

#[test]
fn pages_are_rotated_moved_inserted_and_deleted_one_step_each() {
    let h = fixture_doc("pages");
    let s0 = steps(&h);
    ok(&h, "organize_pages", json!({ "action": "rotate", "pages": [1], "degrees": 90 }));
    assert_eq!(h.eng.edit_state(h.doc).unwrap().sizes[0], [792.0, 612.0]);
    ok(&h, "organize_pages", json!({ "action": "move", "pages": [1], "to": 4 }));
    assert_eq!(h.eng.edit_state(h.doc).unwrap().sizes[2], [792.0, 612.0], "the rotated page is last now");
    ok(&h, "organize_pages", json!({ "action": "insert_blank", "at": 1 }));
    assert_eq!(h.eng.edit_state(h.doc).unwrap().page_count, 4);
    let o = ok(&h, "delete_pages", json!({ "pages": [1, 2] }));
    assert!(o.text.contains("pp. 1-2"), "{}", o.text);
    assert_eq!(h.eng.edit_state(h.doc).unwrap().page_count, 2);
    assert_eq!(steps(&h), s0 + 4);

    // the whole turn is one undo unit
    h.eng.undo(h.doc).unwrap();
    assert_eq!(h.eng.edit_state(h.doc).unwrap().page_count, 3);
}

#[test]
fn page_changes_that_do_nothing_or_are_wrong_are_errors() {
    let h = fixture_doc("pages-err");
    let (r, s) = (rev(&h), steps(&h));
    assert!(err_of(&h, "organize_pages", json!({ "action": "rotate", "pages": [1], "degrees": 360 })).contains("not a turn"));
    assert!(err_of(&h, "organize_pages", json!({ "action": "rotate", "pages": [9], "degrees": 90 })).contains("does not exist"));
    assert!(err_of(&h, "organize_pages", json!({ "action": "move", "pages": [1], "to": 1 })).contains("exactly as they are"));
    assert!(err_of(&h, "organize_pages", json!({ "action": "move", "pages": [1], "to": 9 })).contains("from 1 to 4"));
    assert!(err_of(&h, "organize_pages", json!({ "action": "spin" })).contains("Unknown action"));
    assert!(err_of(&h, "delete_pages", json!({ "pages": [1, 2, 3] })).contains("at least one page"));
    assert!(err_of(&h, "delete_pages", json!({})).contains("pages"));
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

// ---- signed documents and hosts without a lane -------------------------------------------------

#[test]
fn a_signed_document_refuses_what_would_break_the_signature() {
    let h = fixture_doc("signed");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    sign(&h);
    let (r, s) = (rev(&h), steps(&h));
    for (tool, args) in [
        ("redact_apply", json!({ "expected_marks": 2, "pages": [1, 3] })),
        ("organize_pages", json!({ "action": "rotate", "pages": [1], "degrees": 90 })),
        ("delete_pages", json!({ "pages": [1] })),
    ] {
        let e = err_of(&h, tool, args);
        assert!(e.contains("digitally signed") && e.contains("save a copy"), "{tool}: {e}");
    }
    assert_eq!((rev(&h), steps(&h)), (r, s));
    // markup goes in as an incremental update and keeps the signature valid
    ok(&h, "add_comment", json!({ "kind": "highlight", "find": "needle", "pages": [1] }));
}

#[test]
fn without_an_edit_lane_nothing_is_changed() {
    let mut h = fixture_doc("no-lane");
    h.lane = false;
    for (tool, args) in [
        ("redact_find_and_mark", json!({ "text": "needle" })),
        ("redact_apply", json!({ "expected_marks": 1, "pages": [1] })),
        ("add_comment", json!({ "kind": "note", "page": 1, "x": 0.1, "y": 0.1, "text": "a" })),
        ("organize_pages", json!({ "action": "rotate", "pages": [1], "degrees": 90 })),
        ("delete_pages", json!({ "pages": [1] })),
    ] {
        assert!(err_of(&h, tool, args).contains("cannot be changed from here"), "{tool}");
    }
}

// ---- the registry ------------------------------------------------------------------------------

#[test]
fn the_registry_names_are_unique_and_the_classes_are_honest() {
    let all = crate::ai::tools::builtin();
    let mut names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    let n = names.len();
    names.dedup();
    assert_eq!(names.len(), n, "duplicate tool names");
    let class = |name: &str| all.iter().find(|t| t.name == name).unwrap().class;
    for read in ["redact_list_marks", "list_comments", "list_form_fields"] {
        assert_eq!(class(read), Class::Read, "{read}");
    }
    for edit in ["redact_find_and_mark", "redact_mark_area", "redact_unmark", "add_comment", "fill_form_fields", "organize_pages"] {
        assert_eq!(class(edit), Class::Edit, "{edit}");
    }
    assert_eq!(class("delete_pages"), Class::Destructive);
    assert_eq!(class("redact_apply"), Class::Permanent);
    // the model is told it can act, never that it cannot
    let prompt = crate::ai::text::BASE_INSTRUCTIONS;
    assert!(prompt.contains("You CAN change the document"));
    assert!(!prompt.contains("cannot redact") && !prompt.contains("can't redact"));
}
