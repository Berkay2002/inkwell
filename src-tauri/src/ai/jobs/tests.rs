//! The job tools against a real engine and generated documents in a temporary folder: what is
//! written where (never over anything), what the model is told, and what is refused.

use super::*;
use crate::ai::actions::tests::{Host, fixture_doc, rev, sign, steps};
use crate::ai::text::DocText;
use crate::ai::tools::{NEVER, builtin, quiet};

fn ctx_run<R>(h: &Host, f: impl FnOnce(&Ctx) -> R) -> R {
    let text = DocText { rev: 0, pages: 0, chunks: Vec::new(), chars: 0 };
    let c = Ctx { text: &text, host: h, name: "a.pdf", current_page: 0, selection: None, cancel: &NEVER, progress: &quiet };
    f(&c)
}

fn tool_named(name: &str) -> Arc<Tool> {
    builtin().into_iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"))
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

fn folder(h: &Host) -> PathBuf {
    PathBuf::from(h.eng.doc_entry(h.doc).unwrap().path()).parent().unwrap().to_path_buf()
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// An unsaved change the assistant makes, so the document differs from its file.
fn make_dirty(h: &Host) {
    ok(h, "organize_pages", json!({ "action": "rotate", "pages": [1], "degrees": 90 }));
}

#[test]
fn the_open_documents_are_listed_with_this_one_marked() {
    let h = fixture_doc("jobs-list");
    let o = ok(&h, "list_open_documents", json!({}));
    assert!(o.text.starts_with("1 document open"), "{}", o.text);
    assert!(o.text.contains("a.pdf") && o.text.contains("3 pages") && o.text.contains("this document"), "{}", o.text);
}

#[test]
fn comparing_with_the_saved_file_lists_the_unsaved_changes() {
    let h = fixture_doc("jobs-compare");
    // nothing unsaved: nothing to compare with
    assert!(err_of(&h, "compare_documents", json!({})).contains("no unsaved changes"));
    make_dirty(&h);
    let o = ok(&h, "compare_documents", json!({}));
    assert!(o.text.contains("change"), "{}", o.text);
    assert!(o.text.contains("<<<DOCUMENT"), "document-derived lines are a data block: {}", o.text);
    // itself, a document that is not open, and nonsense are errors that say what to do
    assert!(err_of(&h, "compare_documents", json!({ "with": h.doc })).contains("this document"));
    assert!(err_of(&h, "compare_documents", json!({ "with": 999 })).contains("list_open_documents"));
    assert!(err_of(&h, "compare_documents", json!({ "with": "the other one" })).contains("\"with\""));
}

#[test]
fn compress_makes_a_copy_next_to_the_document_and_never_over_a_file() {
    let h = fixture_doc("jobs-compress");
    let dir = folder(&h);
    let before = std::fs::read(dir.join("a.pdf")).unwrap();
    let c = card(&h, "compress_document", json!({})).unwrap().unwrap();
    assert!(c.contains("a (compressed).pdf") && c.contains("original is not changed"), "{c}");
    let o = ok(&h, "compress_document", json!({ "preset": "smallest" }));
    assert!(o.text.contains("a (compressed).pdf") && o.text.contains("original"), "{}", o.text);
    assert!(dir.join("a (compressed).pdf").is_file(), "{:?}", names(&dir));
    // a second run takes the next free name
    ok(&h, "compress_document", json!({}));
    assert!(dir.join("a (compressed) (2).pdf").is_file(), "{:?}", names(&dir));
    assert_eq!(std::fs::read(dir.join("a.pdf")).unwrap(), before, "the original is not touched");
    assert!(err_of(&h, "compress_document", json!({ "preset": "tiny" })).contains("Unknown preset"));
}

#[test]
fn compress_wants_the_saved_file_and_says_so() {
    let h = fixture_doc("jobs-compress-dirty");
    make_dirty(&h);
    let e = err_of(&h, "compress_document", json!({}));
    assert!(e.contains("unsaved changes") && e.contains("save"), "{e}");
    assert_eq!(card(&h, "compress_document", json!({})).unwrap_err(), e, "refused before the card");
    assert!(!names(&folder(&h)).iter().any(|n| n.contains("compressed")));
}

#[test]
fn export_writes_new_files_and_folders_with_free_names() {
    let h = fixture_doc("jobs-export");
    let dir = folder(&h);
    let o = ok(&h, "export_document", json!({ "format": "text" }));
    assert!(o.text.contains("a.txt"), "{}", o.text);
    let first = std::fs::read_to_string(dir.join("a.txt")).unwrap();
    assert!(first.contains("Inkwell fixture page 1"), "{first}");
    ok(&h, "export_document", json!({ "format": "text", "pages": [2] }));
    assert!(dir.join("a (2).txt").is_file(), "{:?}", names(&dir));
    assert!(std::fs::read_to_string(dir.join("a (2).txt")).unwrap().contains("page 2"));

    let o = ok(&h, "export_document", json!({ "format": "images", "dpi": 72 }));
    assert!(o.text.contains("3 pages") && o.text.contains("a (images)"), "{}", o.text);
    assert_eq!(std::fs::read_dir(dir.join("a (images)")).unwrap().count(), 3);
    assert!(err_of(&h, "export_document", json!({ "format": "images", "dpi": 5 })).contains("dpi"));
    assert!(err_of(&h, "export_document", json!({ "format": "pdf" })).contains("Unknown format"));
    assert!(err_of(&h, "export_document", json!({})).contains("required"));
    assert!(err_of(&h, "export_document", json!({ "format": "text", "pages": [9] })).contains("does not exist"));
    // the card names what is written before anything is
    let c = card(&h, "export_document", json!({ "format": "html" })).unwrap().unwrap();
    assert!(c.contains("a.html") && c.contains("nothing is overwritten"), "{c}");
}

#[test]
fn a_blank_pdf_is_created_as_a_separate_document() {
    let h = fixture_doc("jobs-create");
    let (r, s) = (rev(&h), steps(&h));
    let o = ok(&h, "create_blank_pdf", json!({ "pages": 2, "page_size": "a4" }));
    assert!(o.text.contains("2 pages"), "{}", o.text);
    assert_eq!((rev(&h), steps(&h)), (r, s), "this document is not changed");
    assert!(err_of(&h, "create_blank_pdf", json!({ "pages": 0 })).contains("1 to 500"));
    assert!(err_of(&h, "create_blank_pdf", json!({ "page_size": "folio" })).contains("page_size"));
}

#[test]
fn recognizing_text_on_pages_that_have_text_changes_nothing() {
    let h = fixture_doc("jobs-ocr");
    let (r, s) = (rev(&h), steps(&h));
    // the fixture's pages have text: nothing is recognized (or the engine is not there): an error, no step
    let e = err_of(&h, "ocr_document", json!({ "pages": [1, 2] }));
    assert!(!e.is_empty());
    assert_eq!((rev(&h), steps(&h)), (r, s), "no undo step for a no-op");
    assert!(err_of(&h, "ocr_document", json!({ "engine": "abbyy" })).contains("Unknown engine"));
    assert!(err_of(&h, "ocr_document", json!({ "languages": ["en US; rm"] })).contains("short codes"));
    assert!(err_of(&h, "ocr_document", json!({ "pages": [9] })).contains("does not exist"));
    assert!(err_of(&h, "ocr_document", json!({ "dpi": 600 })).contains("no option"));
}

#[test]
fn a_conversion_makes_a_copy_and_leaves_the_document_alone() {
    let h = fixture_doc("jobs-convert");
    let (r, s) = (rev(&h), steps(&h));
    let dir = folder(&h);
    let before = std::fs::read(dir.join("a.pdf")).unwrap();
    let files = names(&dir).len();
    match call(&h, "convert_to_standard", json!({ "profile": "pdfa-2b", "lang": "en-US" })) {
        Ok(o) => {
            assert!(o.text.contains("original is unchanged"), "{}", o.text);
            assert_eq!(names(&dir).len(), files + 1, "{:?}", names(&dir));
        }
        // what the safe fixes cannot settle says so and writes nothing
        Err(e) => {
            assert!(!e.is_empty());
            assert_eq!(names(&dir).len(), files, "{e}");
        }
    }
    assert_eq!(std::fs::read(dir.join("a.pdf")).unwrap(), before);
    assert_eq!((rev(&h), steps(&h)), (r, s));
    assert!(err_of(&h, "convert_to_standard", json!({ "profile": "pdfa-9" })).contains("Unknown profile"));
}

#[test]
fn recognizing_text_on_a_signed_document_is_refused_before_the_card() {
    let h = fixture_doc("jobs-signed");
    sign(&h);
    let (r, s) = (rev(&h), steps(&h));
    let e = card(&h, "ocr_document", json!({})).unwrap_err();
    assert!(e.contains("digitally signed") && e.contains("save a copy"), "{e}");
    assert!(err_of(&h, "ocr_document", json!({})).contains("digitally signed"));
    assert_eq!((rev(&h), steps(&h)), (r, s));
}

#[test]
fn the_job_tools_are_honest_about_their_class() {
    for read in ["list_open_documents", "compare_documents"] {
        assert_eq!(tool_named(read).class, Class::Read, "{read}");
    }
    // they write files or change the document: never silent in ask mode
    for edit in ["compress_document", "ocr_document", "export_document", "convert_to_standard", "create_blank_pdf"] {
        assert_eq!(tool_named(edit).class, Class::Edit, "{edit}");
    }
}
