//! The toolbox tools against a real engine and a generated document: what lands, in how many undo
//! steps, what is refused before the card, and what the model is told.

use super::*;
use crate::ai::actions::tests::{Host, fixture_doc, rev, sign, steps};
use crate::ai::text::DocText;
use crate::engine::edit::EditSource;
use crate::ai::tools::{NEVER, quiet};

fn ctx_run<R>(h: &Host, f: impl FnOnce(&Ctx) -> R) -> R {
    let text = DocText { rev: 0, pages: 0, chunks: Vec::new(), chars: 0 };
    let c = Ctx { text: &text, host: h, name: "a.pdf", current_page: 0, selection: None, cancel: &NEVER, progress: &quiet };
    f(&c)
}

fn tool_named(name: &str) -> Arc<Tool> {
    tools().into_iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"))
}

fn call(h: &Host, name: &str, args: Value) -> Result<Output, String> {
    let t = tool_named(name);
    ctx_run(h, |c| (t.run)(c, &args))
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

/// What the pre-approval check says: `Err` refuses before any card.
fn card(h: &Host, name: &str, args: Value) -> Result<Option<String>, String> {
    let t = tool_named(name);
    match &t.check {
        Some(check) => ctx_run(h, |c| check(c, &args)),
        None => Ok(None),
    }
}

fn marked_pages(h: &Host, kind: MarkKind) -> Vec<u32> {
    let info = h.eng.read_custom(h.doc, |d| crate::marks::info(d)).unwrap();
    info.kinds.iter().find(|k| k.kind == kind).map(|k| k.pages.clone()).unwrap_or_default()
}

fn title(h: &Host) -> String {
    h.eng.metadata(h.doc).unwrap().into_iter().find(|(k, _)| *k == "title").map(|(_, v)| v).unwrap_or_default()
}

// ---- page marks -----------------------------------------------------------------------------------

#[test]
fn a_watermark_goes_on_every_page_in_one_step_and_comes_off_in_one() {
    let h = fixture_doc("tb-watermark");
    let s = steps(&h);
    let o = ok(&h, "page_marks", json!({ "kind": "watermark", "text": "DRAFT", "opacity": 0.3 }));
    assert!(o.text.contains("Added the watermark on all pages"), "{}", o.text);
    assert!(matches!(o.ui, Some(UiAction::Edited { .. })));
    assert_eq!(marked_pages(&h, MarkKind::Watermark), vec![0, 1, 2]);
    assert_eq!(steps(&h), s + 1);
    // adding again is refused rather than doubling it
    let e = err_of(&h, "page_marks", json!({ "kind": "watermark", "text": "DRAFT" }));
    assert!(e.contains("already") && e.contains("update"), "{e}");
    ok(&h, "page_marks", json!({ "kind": "watermark", "action": "update", "text": "FINAL", "pages": [2] }));
    assert_eq!(marked_pages(&h, MarkKind::Watermark), vec![0, 1, 2]);
    let before = steps(&h);
    ok(&h, "page_marks", json!({ "kind": "watermark", "action": "remove" }));
    assert_eq!(marked_pages(&h, MarkKind::Watermark), Vec::<u32>::new());
    assert_eq!(steps(&h), before + 1);
    let e = err_of(&h, "page_marks", json!({ "kind": "watermark", "action": "remove" }));
    assert!(e.contains("no watermark"), "{e}");
}

#[test]
fn page_marks_say_what_they_will_do_and_refuse_nonsense() {
    let h = fixture_doc("tb-marks-args");
    let (r, s) = (rev(&h), steps(&h));
    let t = tool_named("page_marks");
    assert_eq!((t.label)(&json!({ "kind": "watermark", "pages": [1, 2, 3] })), "Add watermark to pp. 1-3");
    assert_eq!((t.label)(&json!({ "kind": "bates", "action": "remove" })), "Remove Bates numbering from all pages");
    let detail = card(&h, "page_marks", json!({ "kind": "header_footer", "bottom_center": "Page <<1 of n>>", "top_right": "ACME" })).unwrap().unwrap();
    assert!(detail.contains("top right: “ACME”") && detail.contains("bottom center: “Page <<1 of n>>”") && detail.contains("all pages"), "{detail}");
    assert!(card(&h, "page_marks", json!({ "kind": "bates", "prefix": "ABC-", "digits": 4 })).unwrap().unwrap().contains("ABC-0001"));
    for (args, said) in [
        (json!({}), "kind"),
        (json!({ "kind": "stamp" }), "Unknown kind"),
        (json!({ "kind": "watermark" }), "text"),
        (json!({ "kind": "watermark", "text": "X", "colour": "red" }), "no option"),
        (json!({ "kind": "watermark", "text": "X", "color": "red" }), "#cc0000"),
        (json!({ "kind": "watermark", "text": "X", "opacity": 7 }), "between"),
        (json!({ "kind": "header_footer" }), "at least one place"),
        (json!({ "kind": "background" }), "color"),
        (json!({ "kind": "bates", "digits": 2 }), "3 to 15"),
        (json!({ "kind": "bates", "digits": 3, "start": 999 }), "run past"),
        (json!({ "kind": "bates", "position": "middle" }), "Unknown position"),
        (json!({ "kind": "watermark", "text": "X", "pages": [9] }), "does not exist"),
        (json!({ "kind": "watermark", "text": "X", "action": "paint" }), "Unknown action"),
    ] {
        let e = err_of(&h, "page_marks", args.clone());
        assert!(e.contains(said), "{args}: {e}");
    }
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

#[test]
fn header_footer_and_bates_numbers_land_on_the_chosen_pages() {
    let h = fixture_doc("tb-hf");
    ok(&h, "page_marks", json!({ "kind": "header_footer", "bottom_center": "Page <<1 of n>>", "pages": [1, 2] }));
    assert_eq!(marked_pages(&h, MarkKind::HeaderFooter), vec![0, 1]);
    ok(&h, "page_marks", json!({ "kind": "bates", "prefix": "ACME-", "digits": 5, "start": 100, "position": "top_right" }));
    assert_eq!(marked_pages(&h, MarkKind::Bates), vec![0, 1, 2]);
    ok(&h, "page_marks", json!({ "kind": "background", "color": "#fff7d6" }));
    assert_eq!(marked_pages(&h, MarkKind::Background), vec![0, 1, 2]);
}

#[test]
fn a_signed_document_is_refused_before_the_card_for_every_toolbox_change() {
    let h = fixture_doc("tb-signed");
    sign(&h);
    let (r, s) = (rev(&h), steps(&h));
    for (tool, args) in [
        ("page_marks", json!({ "kind": "watermark", "text": "DRAFT" })),
        ("set_document_properties", json!({ "title": "T" })),
        ("sanitize_document", json!({})),
        ("fix_accessibility", json!({ "fixes": [{ "fix": "tab_order" }] })),
    ] {
        let e = card(&h, tool, args.clone()).unwrap_err();
        assert!(e.contains("digitally signed") && e.contains("save a copy"), "{tool}: {e}");
        assert!(err_of(&h, tool, args).contains("digitally signed"), "{tool}");
    }
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

// ---- properties and sanitize ---------------------------------------------------------------------

#[test]
fn properties_are_set_once_and_a_repeat_is_an_error() {
    let h = fixture_doc("tb-props");
    let s = steps(&h);
    let t = tool_named("set_document_properties");
    assert_eq!((t.label)(&json!({ "title": "Annual report", "author": "" })), "Set the document title “Annual report”, clear the author");
    ok(&h, "set_document_properties", json!({ "title": "Annual report 2026", "author": "Berkay" }));
    assert_eq!(steps(&h), s + 1);
    assert_eq!(title(&h), "Annual report 2026");
    let e = err_of(&h, "set_document_properties", json!({ "title": "Annual report 2026" }));
    assert!(e.contains("already have those values"), "{e}");
    assert!(err_of(&h, "set_document_properties", json!({})).contains("at least one"));
    assert!(err_of(&h, "set_document_properties", json!({ "creator": "x" })).contains("no option"));
    assert!(err_of(&h, "set_document_properties", json!({ "title": 7 })).contains("must be text"));
    assert_eq!(steps(&h), s + 1);
}

/// The XMP packet as text (empty when the document has none).
fn xmp_of(h: &Host) -> String {
    h.eng.read_custom(h.doc, |d| Ok(crate::a11y::matterhorn::xmp(&d.catalog()?)?.unwrap_or_default())).unwrap()
}

#[test]
fn properties_reach_the_xmp_packet_too_and_a_cleared_one_leaves_it() {
    let h = fixture_doc("tb-props-xmp");
    // a packet that names an old title and author, as an export from another application leaves it
    h.eng
        .apply_custom(h.doc, "Add XMP".into(), EditSource::User, false, |d| {
            let xml = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">                <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">                <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Old title</rdf:li></rdf:Alt></dc:title>                <dc:creator><rdf:Seq><rdf:li>Old author</rdf:li></rdf:Seq></dc:creator>                </rdf:Description></rdf:RDF></x:xmpmeta>";
            let mut stm = d.add_stream(&mupdf::Buffer::from_bytes(xml.as_bytes())?, None, false)?;
            stm.dict_put("Type", mupdf::pdf::PdfObject::new_name("Metadata")?)?;
            stm.dict_put("Subtype", mupdf::pdf::PdfObject::new_name("XML")?)?;
            d.catalog()?.dict_put("Metadata", stm)?;
            Ok(Vec::new())
        })
        .unwrap();
    ok(&h, "set_document_properties", json!({ "title": "New title", "author": "" }));
    let x = xmp_of(&h);
    assert!(x.contains("New title") && !x.contains("Old title"), "{x}");
    assert!(!x.contains("Old author"), "a cleared author stays in the packet: {x}");
}

#[test]
fn a_certified_document_takes_no_property_change_and_no_sanitize_and_a_signed_one_no_rewrite() {
    let h = fixture_doc("tb-cert-props");
    crate::ai::actions::tests::certify(&h, Some(2), None);
    let (r, s) = (rev(&h), steps(&h));
    for (tool, args) in [("set_document_properties", json!({ "title": "T" })), ("sanitize_document", json!({}))] {
        // the assistant's own guard answers first; the closure's `refuse_if_certified` is the backstop
        // (`engine/tests_certified.rs` runs the same writes without it)
        let e = err_of(&h, tool, args);
        assert!(e.contains("certified") || e.contains("digitally signed"), "{tool}: {e}");
    }
    assert_eq!((rev(&h), steps(&h)), (r, s));
    // signed but not certified: a sanitize is a full rewrite, which would void the signature
    let h = fixture_doc("tb-signed-sanitize");
    sign(&h);
    let s = steps(&h);
    let e = err_of(&h, "sanitize_document", json!({}));
    assert!(e.contains("signed"), "{e}");
    assert_eq!(steps(&h), s);
}

#[test]
fn sanitize_names_what_goes_and_a_clean_document_is_an_error() {
    let h = fixture_doc("tb-sanitize");
    let t = tool_named("sanitize_document");
    assert_eq!(t.class, Class::Destructive);
    assert_eq!(
        (t.label)(&json!({})),
        "Remove hidden information: metadata, hidden layers, scripts and actions, attached files"
    );
    assert_eq!(
        (t.label)(&json!({ "annotations": true, "metadata": false })),
        "Remove hidden information: hidden layers, scripts and actions, attached files, ALL comments and markup"
    );
    assert!(card(&h, "sanitize_document", json!({ "hiddenLayers": "yes" })).unwrap_err().contains("true or false"));
    assert!(card(&h, "sanitize_document", json!({ "everything": true })).unwrap_err().contains("no option"));
    let off = json!({ "metadata": false, "hiddenLayers": false, "scriptsActions": false, "embeddedFiles": false });
    assert!(card(&h, "sanitize_document", off).unwrap_err().contains("nothing would be removed"));
    // nothing to remove: an error and no undo step
    let (r, s) = (rev(&h), steps(&h));
    // a fixture has no metadata of the kinds asked for until one is set
    let first = call(&h, "sanitize_document", json!({}));
    if let Err(e) = &first {
        assert!(e.contains("nothing of the kinds"), "{e}");
        assert_eq!((rev(&h), steps(&h)), (r, s));
    }
    ok(&h, "set_document_properties", json!({ "title": "Secret plans", "author": "Berkay" }));
    let o = ok(&h, "sanitize_document", json!({}));
    assert!(o.text.contains("metadata entries"), "{}", o.text);
    assert_eq!(title(&h), "");
}

// ---- accessibility and preflight ----------------------------------------------------------------------

#[test]
fn the_accessibility_check_lists_failures_and_fixes_land_as_one_step() {
    let h = fixture_doc("tb-a11y");
    let o = ok(&h, "check_accessibility", json!({}));
    assert!(o.text.starts_with("Accessibility check: 3 pages"), "{}", o.text);
    assert!(o.text.contains("[title]") && o.text.contains("fix_accessibility can repair this: title"), "{}", o.text);
    let s = steps(&h);
    let o = ok(&h, "fix_accessibility", json!({ "fixes": [{ "fix": "title", "title": "Quarterly report" }, { "fix": "language", "lang": "en-US" }] }));
    assert!(o.text.contains("Applied:"), "{}", o.text);
    assert_eq!(steps(&h), s + 1);
    assert_eq!(title(&h), "Quarterly report");
    let o = ok(&h, "check_accessibility", json!({}));
    assert!(!o.text.contains("[title]") && !o.text.contains("[primary-language]"), "{}", o.text);
    // arguments are checked
    for (args, said) in [
        (json!({}), "fixes"),
        (json!({ "fixes": [{ "fix": "paint" }] }), "Unknown fix"),
        (json!({ "fixes": [{ "fix": "title" }] }), "needs \"title\""),
        (json!({ "fixes": [{ "fix": "language" }] }), "needs \"lang\""),
        (json!({ "fixes": [{ "fix": "figure_alt", "alt": "A cat" }] }), "xref"),
        (json!({ "fixes": [{ "fix": "figure_alt", "xref": 5 }] }), "alt"),
    ] {
        let e = err_of(&h, "fix_accessibility", args.clone());
        assert!(e.contains(said), "{args}: {e}");
    }
    let t = tool_named("fix_accessibility");
    assert_eq!((t.label)(&json!({ "fixes": [{ "fix": "tab_order" }] })), "Set tab order to structure order");
    let detail = card(&h, "fix_accessibility", json!({ "fixes": [{ "fix": "figure_alt", "xref": 7, "alt": "A red car" }] })).unwrap().unwrap();
    assert!(detail.contains("figure 7: “A red car”"), "{detail}");
}

#[test]
fn the_preflight_check_reports_a_profile_and_rejects_unknown_ones() {
    let h = fixture_doc("tb-preflight");
    let o = ok(&h, "check_preflight", json!({ "profile": "pdfa-2b" }));
    assert!(o.text.starts_with("PDF/A-2b check of 3 pages:"), "{}", o.text);
    assert!(err_of(&h, "check_preflight", json!({ "profile": "pdfa-9" })).contains("Unknown profile"));
    assert!(err_of(&h, "check_preflight", json!({})).contains("profile"));
    assert_eq!(tool_named("check_preflight").class, Class::Read);
}

// ---- opening a tool ----------------------------------------------------------------------------------

#[test]
fn open_tool_opens_only_known_tools_and_changes_nothing() {
    let h = fixture_doc("tb-open");
    let (r, s) = (rev(&h), steps(&h));
    let o = ok(&h, "open_tool", json!({ "tool": "protect" }));
    assert_eq!(o.ui, Some(UiAction::OpenTool { tool: "protect".into() }));
    assert!(err_of(&h, "open_tool", json!({ "tool": "sign" })).contains("no tool"));
    assert!(err_of(&h, "open_tool", json!({})).contains("required"));
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

#[test]
fn the_toolbox_classes_are_honest() {
    let class = |n: &str| tool_named(n).class;
    for read in ["check_accessibility", "check_preflight", "open_tool"] {
        assert_eq!(class(read), Class::Read, "{read}");
    }
    for edit in ["page_marks", "set_document_properties", "fix_accessibility"] {
        assert_eq!(class(edit), Class::Edit, "{edit}");
    }
    assert_eq!(class("sanitize_document"), Class::Destructive);
    // every tool's schema is an object the provider accepts
    for t in tools() {
        assert_eq!(t.schema["type"], "object", "{}", t.name);
        assert!(!t.description.is_empty() && t.description.len() < 1500, "{}", t.name);
    }
}

// ---- editing text -------------------------------------------------------------------------------------

#[test]
fn editing_text_replaces_one_place_in_one_step_and_says_when_it_cannot() {
    let h = fixture_doc("tb-edit-text");
    let s = steps(&h);
    let t = tool_named("edit_text");
    assert_eq!((t.label)(&json!({ "page": 1, "find": "fixture", "replace": "sample" })), "Replace “fixture” with “sample” on p. 1");
    assert_eq!((t.label)(&json!({ "page": 2, "find": "dog", "replace": "" })), "Delete “dog” on p. 2");
    let c = card(&h, "edit_text", json!({ "page": 1, "find": "fixture", "replace": "sample" })).unwrap().unwrap();
    assert!(c.contains("p. 1") && c.contains("fixture") && c.contains("sample"), "{c}");
    let o = ok(&h, "edit_text", json!({ "page": 1, "find": "fixture", "replace": "sample" }));
    assert!(o.text.contains("Replaced"), "{}", o.text);
    assert!(matches!(o.ui, Some(UiAction::Edited { .. })));
    assert_eq!(steps(&h), s + 1);
    let b = textedit::blocks(&h.eng, h.doc, 0).unwrap();
    assert!(b.blocks.iter().any(|b| b.text.contains("Inkwell sample page 1")), "{:?}", b.blocks.iter().map(|b| &b.text).collect::<Vec<_>>());
    // not there, ambiguous, the same, or off the document: an error and no step
    let s = steps(&h);
    for (args, said) in [
        (json!({ "page": 1, "find": "no such words", "replace": "x" }), "not in the editable text"),
        (json!({ "page": 1, "find": "line", "replace": "row" }), "times"),
        (json!({ "page": 1, "find": "sample", "replace": "sample" }), "same"),
        (json!({ "page": 7, "find": "sample", "replace": "x" }), "does not exist"),
        (json!({ "page": 1, "find": "sample" }), "\"replace\" is required"),
        (json!({ "page": 1, "find": "sample", "replace": "x", "bold": true }), "no option"),
    ] {
        let e = err_of(&h, "edit_text", args.clone());
        assert!(e.contains(said), "{args}: {e}");
    }
    assert_eq!(steps(&h), s);
}

#[test]
fn editing_text_in_a_signed_document_is_refused_before_the_card() {
    let h = fixture_doc("tb-edit-text-signed");
    sign(&h);
    let (r, s) = (rev(&h), steps(&h));
    let args = json!({ "page": 1, "find": "fixture", "replace": "x" });
    let e = card(&h, "edit_text", args.clone()).unwrap_err();
    assert!(e.contains("digitally signed"), "{e}");
    assert!(err_of(&h, "edit_text", args).contains("digitally signed"));
    assert_eq!((rev(&h), steps(&h)), (r, s));
}
