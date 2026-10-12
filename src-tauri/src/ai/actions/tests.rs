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

pub(in crate::ai) struct Host {
    pub(in crate::ai) eng: Arc<Engine>,
    pub(in crate::ai) doc: DocId,
    pub(in crate::ai) edited: AtomicBool,
    pub(in crate::ai) lane: bool,
}

impl DocHost for Host {
    fn meta(&self) -> Vec<(&'static str, String)> {
        self.eng.metadata(self.doc).unwrap_or_default()
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

pub(in crate::ai) fn open_bytes(test: &str, bytes: Vec<u8>) -> Host {
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
pub(in crate::ai) fn fixture_doc(test: &str) -> Host {
    open_bytes(test, fixture::build(&Fixture { pages: 3, lines: 4, needle_every: 2, ..Default::default() }))
}

fn call(h: &Host, name: &str, args: Value) -> Result<Output, String> {
    let t = tools().into_iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"));
    let text = DocText { rev: 0, pages: 0, chunks: Vec::new(), chars: 0 };
    let c = Ctx { text: &text, host: h, name: "a.pdf", current_page: 0, selection: None, cancel: &crate::ai::tools::NEVER, progress: &crate::ai::tools::quiet };
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

pub(in crate::ai) fn steps(h: &Host) -> usize {
    h.eng.history(h.doc).unwrap().len()
}

pub(in crate::ai) fn rev(h: &Host) -> u32 {
    h.eng.edit_state(h.doc).unwrap().rev
}

fn marks(h: &Host) -> Vec<crate::redact::app::MarkDto> {
    list_marks(&h.eng, h.doc).unwrap()
}

/// The digest `redact_list_marks` gives for the document's marks now.
fn digest(h: &Host) -> String {
    crate::redact::app::marks_digest(&marks(h))
}

fn needle_hits(h: &Host) -> usize {
    let q = FindQuery::Text { needle: "needle".into(), case_sensitive: false, whole_word: false };
    crate::redact::app::find(&h.eng, h.doc, &q).unwrap().len()
}

pub(in crate::ai) fn sign(h: &Host) {
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
    // the card names everything that goes with the marks: every option is on by default and
    // comments are among them
    let label = (t.label)(&json!({ "expected_marks": 3, "pages": [1, 2, 3] }));
    assert_eq!(label, "Permanently redact 3 marks on pp. 1-3, delete ALL comments and markup, strip metadata and all hidden data");
    let keep = (t.label)(&json!({ "expected_marks": 1, "pages": [4], "sanitize": { "annotations": false, "metadata": false } }));
    assert_eq!(
        keep,
        "Permanently redact 1 mark on p. 4, strip hidden layers, hidden text, scripts and actions, attached files, bookmarks, keep metadata, comments"
    );
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
    let d = digest(&h);
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 5, "pages": [1, 3], "marks_digest": d }));
    assert!(e.contains("not what you listed"), "{e}");
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1], "marks_digest": d }));
    assert!(e.contains("not what you listed"), "{e}");
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3] }));
    assert!(e.contains("marks_digest"), "a digest is required: {e}");
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3], "marks_digest": "0000000000000000" }));
    assert!(e.contains("not what you listed"), "{e}");
    assert_eq!(needle_hits(&h), 2, "a refused apply changed nothing");

    let o = ok(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3], "marks_digest": d }));
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
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 1, "pages": [1], "marks_digest": "x" }));
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
    // a range or list that names no page is never "every page": a lone `to`, a `from` that is no number, a reversed range
    for a in [json!({ "to": 2 }), json!({ "from": "x" }), json!({ "from": 3, "to": 1 }), json!({ "pages": [] }), json!({ "pages": "1" })] {
        let mut rotate = json!({ "action": "rotate", "degrees": 90 });
        rotate.as_object_mut().unwrap().extend(a.as_object().unwrap().clone());
        assert!(err_of(&h, "organize_pages", rotate).contains("name no page"), "{a}");
        assert!(err_of(&h, "delete_pages", a.clone()).contains("name no page"), "{a}");
    }
    assert!(err_of(&h, "organize_pages", json!({ "action": "move", "to": 2 })).contains("name no page"));
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

// ---- signed documents and hosts without a lane -------------------------------------------------

#[test]
fn a_signed_document_refuses_what_would_break_the_signature() {
    let h = fixture_doc("signed");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    let d = digest(&h);
    sign(&h);
    let (r, s) = (rev(&h), steps(&h));
    for (tool, args) in [
        ("redact_apply", json!({ "expected_marks": 2, "pages": [1, 3], "marks_digest": d })),
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
        ("redact_apply", json!({ "expected_marks": 1, "pages": [1], "marks_digest": "x" })),
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
    for edit in ["redact_find_and_mark", "redact_mark_area", "add_comment", "fill_form_fields", "organize_pages"] {
        assert_eq!(class(edit), Class::Edit, "{edit}");
    }
    assert_eq!(class("delete_pages"), Class::Destructive);
    // the user's marks are not for text in a document to delete unasked, in Auto mode either
    assert_eq!(class("redact_unmark"), Class::Destructive);
    assert_eq!(class("redact_apply"), Class::Permanent);
    // the model is told it can act, never that it cannot
    let prompt = crate::ai::text::BASE_INSTRUCTIONS;
    assert!(prompt.contains("You CAN change the document"));
    assert!(!prompt.contains("cannot redact") && !prompt.contains("can't redact"));
}

// ---- certification, field locks and the pre-approval check ------------------------------------

/// Installs a signed signature field (and, with `p`, a certification of that level); `lock` is
/// the field's `/Lock` dictionary body, e.g. `/Action /All`.
pub(in crate::ai) fn certify(h: &Host, p: Option<i32>, lock: Option<&str>) {
    h.eng
        .apply_custom(h.doc, "Certify".into(), EditSource::User, false, |d| {
            let refs = match p {
                Some(p) => format!(
                    "/Reference [ << /Type /SigRef /TransformMethod /DocMDP /TransformParams << /Type /TransformParams /P {p} /V /1.2 >> >> ]"
                ),
                None => String::new(),
            };
            let sig = d.new_object_from_str(&format!("<< /Type /Sig /Filter /Adobe.PPKLite /ByteRange [0 0 0 0] /Contents <00> {refs} >>"))?;
            let mut field = d.new_object_from_str("<< /FT /Sig /T (Signature1) /Type /Annot /Subtype /Widget /Rect [0 0 0 0] >>")?;
            field.dict_put("V", sig.clone())?;
            if let Some(l) = lock {
                let lock = d.new_object_from_str(&format!("<< /Type /SigFieldLock {l} >>"))?;
                field.dict_put("Lock", lock)?;
            }
            let field = d.add_object(&field)?;
            // a signature widget sits on a page, as in a real file: the form scan reads widgets from the pages
            let mut page = d.find_page(0)?;
            let annots = match page.get_dict("Annots")? {
                Some(a) => a,
                None => {
                    let a = d.new_object_from_str("[]")?;
                    page.dict_put("Annots", a.clone())?;
                    a
                }
            };
            let mut annots = annots;
            annots.array_push(field.clone())?;
            let form = match d.catalog()?.get_dict("AcroForm")? {
                Some(f) => f,
                None => {
                    // an indirect object, as in a real file, so adding to its array is a change of it
                    let f = d.add_object(&d.new_object_from_str("<< /Fields [] >>")?)?;
                    d.catalog()?.dict_put("AcroForm", f.clone())?;
                    f
                }
            };
            let mut fields = form.get_dict("Fields")?.expect("a Fields array");
            fields.array_push(field)?;
            // no SigFlags: the signed field alone must be enough to count as signed
            if p.is_some() {
                let mut perms = d.new_object_from_str("<< >>")?;
                perms.dict_put("DocMDP", sig)?;
                d.catalog()?.dict_put("Perms", perms)?;
            }
            Ok(vec![0])
        })
        .unwrap();
}

fn card(h: &Host, tool: &str, args: Value) -> Result<Option<String>, String> {
    let t = tools().into_iter().find(|t| t.name == tool).unwrap();
    let text = DocText { rev: 0, pages: 0, chunks: Vec::new(), chars: 0 };
    let c = Ctx { text: &text, host: h, name: "a.pdf", current_page: 0, selection: None, cancel: &crate::ai::tools::NEVER, progress: &crate::ai::tools::quiet };
    (t.check.as_ref().unwrap_or_else(|| panic!("{tool} has no pre-approval check")))(&c, &args)
}

#[test]
fn a_signed_field_counts_even_without_sigflags_and_is_refused_before_the_card() {
    let h = fixture_doc("sigfield");
    certify(&h, None, None);
    let (r, s) = (rev(&h), steps(&h));
    // the card is never shown for a change the signature forbids
    for (tool, args) in [
        ("organize_pages", json!({ "action": "rotate", "pages": [1], "degrees": 90 })),
        ("delete_pages", json!({ "pages": [1] })),
    ] {
        let e = card(&h, tool, args.clone()).unwrap_err();
        assert!(e.contains("digitally signed"), "{tool}: {e}");
        assert!(err_of(&h, tool, args).contains("digitally signed"), "{tool}");
    }
    assert_eq!((rev(&h), steps(&h)), (r, s));
    // an uncertified signature still lets markup in
    assert!(card(&h, "add_comment", json!({ "kind": "note", "page": 1, "x": 0.1, "y": 0.1, "text": "a" })).is_ok());
}

#[test]
fn a_signed_file_that_had_to_be_repaired_is_refused_before_the_card() {
    // a signed file whose cross-reference table is gone: MuPDF rebuilds it, and the engine takes no edit of it
    let src = fixture_doc("repaired-src");
    certify(&src, None, None);
    src.eng.save(src.doc, None).unwrap();
    let path = src.eng.doc_entry(src.doc).unwrap().path().to_string();
    let mut b = std::fs::read(path).unwrap();
    let at = b.windows(10).rposition(|w| w == b"startxref\n").unwrap() + 10;
    let end = at + b[at..].iter().position(|&c| c == b'\n').unwrap();
    b.splice(at..end, *b"99999999");
    let h = open_bytes("repaired", b);
    let (r, s) = (rev(&h), steps(&h));
    for (tool, args) in [
        ("organize_pages", json!({ "action": "rotate", "pages": [1], "degrees": 90 })),
        ("add_comment", json!({ "kind": "note", "page": 1, "x": 0.1, "y": 0.1, "text": "a" })),
    ] {
        let e = card(&h, tool, args.clone()).unwrap_err();
        assert!(e.contains("repaired") && e.contains("save a copy"), "{tool}: {e}");
        assert!(err_of(&h, tool, args).contains("repaired"), "{tool}");
    }
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

#[test]
fn a_certification_limits_comments_and_form_filling_to_what_it_allows() {
    // P=1: nothing may change; P=2: forms but no comments; P=3: forms and comments
    for (p, comments, forms) in [(1, false, false), (2, false, true), (3, true, true)] {
        let h = with_field(&format!("certified-{p}"));
        certify(&h, Some(p), None);
        let c = card(&h, "add_comment", json!({ "kind": "note", "page": 1, "x": 0.1, "y": 0.1, "text": "a" }));
        assert_eq!(c.is_ok(), comments, "P={p} comments: {c:?}");
        let f = card(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] }));
        assert_eq!(f.is_ok(), forms, "P={p} forms: {f:?}");
        if !comments {
            let e = err_of(&h, "add_comment", json!({ "kind": "note", "page": 1, "x": 0.1, "y": 0.1, "text": "a" }));
            assert!(e.contains("certified") && e.contains("save a copy"), "{e}");
            let e = err_of(&h, "redact_find_and_mark", json!({ "text": "needle" }));
            assert!(e.contains("certified"), "marking is an annotation too: {e}");
        }
        if !forms {
            let e = err_of(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] }));
            assert!(e.contains("certified") && e.contains("save a copy"), "{e}");
            assert_eq!(name_value(&h), "");
        } else {
            ok(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] }));
            assert_eq!(name_value(&h), "A");
        }
    }
}

#[test]
fn a_password_field_is_listed_without_its_value_and_is_never_filled() {
    let h = with_field("forms-password");
    ok(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "hunter2" }] }));
    let f = forms::list_all(&h.eng, h.doc).unwrap().fields.into_iter().find(|f| f.name == "Full name").unwrap();
    let patch = forms::FieldPatch { password: Some(true), ..Default::default() };
    forms::edit_one(&h.eng, h.doc, f.page, f.xref, &patch, EditSource::User).unwrap();
    let l = ok(&h, "list_form_fields", json!({}));
    assert!(l.text.contains("[password field]"), "{}", l.text);
    assert!(!l.text.contains("hunter2"), "the secret reached the model: {}", l.text);
    let (r, s) = (rev(&h), steps(&h));
    let e = err_of(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "x" }] }));
    assert!(e.contains("password field"), "{e}");
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

#[test]
fn a_query_that_matches_everywhere_is_refused_and_can_be_stopped() {
    let h = open_bytes("mark-cap", fixture::build(&Fixture { pages: 501, lines: 4, needle_every: 1, ..Default::default() }));
    let (r, s) = (rev(&h), steps(&h));
    let e = err_of(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    assert!(e.contains("More than 500 matches"), "{e}");
    assert_eq!((rev(&h), steps(&h)), (r, s), "a refused search marks nothing");
    // a stop request ends the search with nothing marked
    let t = tools().into_iter().find(|t| t.name == "redact_find_and_mark").unwrap();
    let text = DocText { rev: 0, pages: 0, chunks: Vec::new(), chars: 0 };
    let stop = std::sync::atomic::AtomicBool::new(true);
    let c = Ctx { text: &text, host: &h, name: "a.pdf", current_page: 0, selection: None, cancel: &stop, progress: &crate::ai::tools::quiet };
    let e = (t.run)(&c, &json!({ "text": "needle", "pages": [1, 2, 3] })).err().unwrap();
    assert!(e.contains("Stopped by the user"), "{e}");
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

#[test]
fn a_page_list_over_the_cap_is_an_error_and_not_cut() {
    let h = fixture_doc("pages-cap");
    let many: Vec<i64> = (1..=600).collect();
    let e = err_of(&h, "redact_find_and_mark", json!({ "text": "needle", "pages": many.clone() }));
    assert!(e.contains("500"), "{e}");
    // get_text is in the reading registry
    let t = crate::ai::tools::builtin().into_iter().find(|t| t.name == "get_text").unwrap();
    let text = DocText { rev: 0, pages: 0, chunks: Vec::new(), chars: 0 };
    let c = Ctx { text: &text, host: &h, name: "a.pdf", current_page: 0, selection: None, cancel: &crate::ai::tools::NEVER, progress: &crate::ai::tools::quiet };
    let e = (t.run)(&c, &json!({ "pages": many })).err().unwrap();
    assert!(e.contains("500"), "{e}");
}

#[test]
fn a_field_a_signature_locked_is_not_filled() {
    let h = with_field("locked");
    certify(&h, Some(3), Some("/Action /Include /Fields [(Full name)]"));
    let e = card(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] })).unwrap_err();
    assert!(e.contains("locked by a signature") && e.contains("Full name"), "{e}");
    assert!(err_of(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] })).contains("locked"));
    assert_eq!(name_value(&h), "");

    // /Exclude: every field but the listed ones is frozen
    let h = with_field("locked-exclude");
    certify(&h, Some(3), Some("/Action /Exclude /Fields [(Other)]"));
    assert!(card(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] })).is_err());
    let h = with_field("locked-exclude-ok");
    certify(&h, Some(3), Some("/Action /Exclude /Fields [(Full name)]"));
    assert!(card(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "A" }] })).is_ok());
}

#[test]
fn a_signed_document_that_needs_a_full_rewrite_is_refused_for_every_change() {
    let h = fixture_doc("rewrite");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    let d = digest(&h);
    ok(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3], "marks_digest": d }));
    // a redaction makes every later save a full rewrite; with a signature on top, no change may go in
    sign(&h);
    let e = err_of(&h, "add_comment", json!({ "kind": "note", "page": 1, "x": 0.1, "y": 0.1, "text": "a" }));
    assert!(e.contains("repair") || e.contains("rewrite"), "{e}");
}

#[test]
fn the_apply_card_shows_the_marked_text_and_refuses_what_is_not_the_reviewed_list() {
    let h = fixture_doc("apply-card");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    let d = digest(&h);
    let detail = card(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3], "marks_digest": d })).unwrap().unwrap();
    assert!(detail.to_lowercase().contains("needle") && detail.contains("1, 3"), "{detail}");
    // the list changed after review: the same call is refused before any card
    ok(&h, "redact_mark_area", json!({ "page": 2, "left": 0.1, "top": 0.1, "right": 0.5, "bottom": 0.2 }));
    let e = card(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3], "marks_digest": d })).unwrap_err();
    assert!(e.contains("not what you listed"), "{e}");
    // the listing carries the digest the apply needs
    let listed = ok(&h, "redact_list_marks", json!({}));
    assert!(listed.text.contains(&format!("marks_digest: {}", digest(&h))), "{}", listed.text);
}

#[test]
fn the_apply_card_names_what_else_goes_and_unknown_options_are_refused() {
    let h = fixture_doc("apply-opts");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    let d = digest(&h);
    let e = card(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3], "marks_digest": d, "sanitize": { "everything": true } })).unwrap_err();
    assert!(e.contains("no option \"everything\""), "{e}");
    let e = err_of(&h, "redact_apply", json!({ "expected_marks": 2, "pages": [1, 3], "marks_digest": d, "sanitize": { "metadata": "no" } }));
    assert!(e.contains("true or false"), "{e}");
    assert_eq!(needle_hits(&h), 2);
}

#[test]
fn the_fill_card_shows_each_field_and_its_value_and_hidden_fields_are_not_offered() {
    let h = with_field("fill-card");
    let c = card(&h, "fill_form_fields", json!({ "fields": [{ "field": "Full name", "value": "SE45 5000 0000 0583 9825 7466" }] })).unwrap().unwrap();
    assert!(c.contains("Full name = “SE45 5000 0000 0583 9825 7466”"), "{c}");
    // not by id either: an id that is not listed is not a field
    let e = err_of(&h, "fill_form_fields", json!({ "fields": [{ "field": "p1#999999", "value": "x" }] }));
    assert!(e.contains("no form field"), "{e}");
}

#[test]
fn unmarking_with_both_marks_and_all_is_refused_and_a_long_regex_too() {
    let h = fixture_doc("unmark-both");
    ok(&h, "redact_find_and_mark", json!({ "text": "needle" }));
    let (r, s) = (rev(&h), steps(&h));
    let id = {
        let m = &marks(&h)[0];
        format!("p{}#{}", m.page + 1, m.xref)
    };
    let e = err_of(&h, "redact_unmark", json!({ "all": true, "marks": [id] }));
    assert!(e.contains("not both"), "{e}");
    let label = (tools().into_iter().find(|t| t.name == "redact_unmark").unwrap().label)(&json!({ "all": true, "marks": ["p1#1"] }));
    assert_eq!(label, "Remove all redaction marks");
    let long = "a".repeat(201);
    let e = err_of(&h, "redact_find_and_mark", json!({ "regex": long }));
    assert!(e.contains("too long"), "{e}");
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

#[test]
fn the_comment_card_says_how_many_matches_and_what_the_note_says() {
    let h = fixture_doc("comment-card");
    let args = json!({ "kind": "highlight", "find": "needle", "text": "check" });
    let t = tools().into_iter().find(|t| t.name == "add_comment").unwrap();
    assert_eq!((t.label)(&args), "Highlight every match of “needle” with the note “check”");
    assert_eq!(card(&h, "add_comment", args).unwrap().unwrap(), "2 matches on pp. 1, 3");
    let first = json!({ "kind": "highlight", "find": "needle", "first_only": true });
    assert_eq!((t.label)(&first), "Highlight the first match of “needle”");
    assert_eq!(card(&h, "add_comment", first).unwrap().unwrap(), "1 match on p. 1");
    assert_eq!((t.label)(&json!({ "kind": "reply", "parent": "p1#2", "text": "Agreed" })), "Reply to a comment: “Agreed”");
    let org = tools().into_iter().find(|t| t.name == "organize_pages").unwrap();
    assert_eq!((org.label)(&json!({ "action": "insert_blank" })), "Insert a blank page at the end");
}

// ---- the coverage contract ----------------------------------------------------------------------

/// `src/lib/aiCoverage.json`: which of the app's tools the assistant covers. The same file is
/// checked against the app's tool ids by `aiCoverage.test.ts`.
#[test]
fn the_coverage_file_lists_exactly_the_registry_and_maps_to_real_tools() {
    let file: Value = serde_json::from_str(include_str!("../../../../src/lib/aiCoverage.json")).unwrap();
    let all = crate::ai::tools::builtin();
    let mut registry: Vec<String> = all.iter().map(|t| t.name.clone()).collect();
    registry.sort_unstable();
    let mut listed: Vec<String> = file["registry"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    listed.sort_unstable();
    assert_eq!(listed, registry, "src/lib/aiCoverage.json `registry` must list exactly the tools in ai::tools::builtin()");
    assert!(registry.len() <= 40, "{} tools: keep the list coarse", registry.len());
    for (id, name) in file["tools"].as_object().unwrap() {
        assert!(registry.iter().any(|n| n == name.as_str().unwrap()), "{id} maps to {name}, which is not a tool");
    }
    for (id, why) in file["excluded"].as_object().unwrap() {
        assert!(why.as_str().unwrap().trim().len() > 10, "{id} needs a reason");
        assert!(file["tools"].get(id).is_none(), "{id} is both covered and excluded");
    }
}

// ---- markup by pattern or regex -----------------------------------------------------------------

#[test]
fn every_date_is_highlighted_in_one_step_by_regex_and_a_literal_date_works_too() {
    let h = open_bytes("highlight-dates", SAMPLE.to_vec());
    let t = tools().into_iter().find(|t| t.name == "add_comment").unwrap();
    let dates = json!({ "kind": "highlight", "regex": r"\d{4}-\d{2}-\d{2}" });
    assert_eq!((t.label)(&dates), r"Highlight every match of /\d{4}-\d{2}-\d{2}/");
    let c = card(&h, "add_comment", dates.clone()).unwrap().unwrap();
    assert!(c.starts_with("2 matches on"), "{c}");
    let s = steps(&h);
    let o = ok(&h, "add_comment", dates);
    assert!(o.text.contains("2 highlights"), "{}", o.text);
    assert_eq!(steps(&h), s + 1, "all of them are one undo step");
    assert_edited(&o);
    // one date by its text (the sample wraps it over two lines) and a validated pattern
    ok(&h, "add_comment", json!({ "kind": "underline", "find": "2026-01-15" }));
    assert!(ok(&h, "add_comment", json!({ "kind": "squiggly", "pattern": "ssn" })).text.contains("5 matches"));
    // exactly one way to say what to mark
    assert!(err_of(&h, "add_comment", json!({ "kind": "highlight" })).contains("Give one of"));
    assert!(err_of(&h, "add_comment", json!({ "kind": "highlight", "find": "a", "regex": "b" })).contains("only one"));
    assert!(err_of(&h, "add_comment", json!({ "kind": "highlight", "regex": "(" })).contains("compile"));
    assert!(err_of(&h, "add_comment", json!({ "kind": "highlight", "regex": "zebra-[0-9]+" })).contains("does not appear"));
}
