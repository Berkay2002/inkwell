//! The job tools against a real engine and generated documents in a temporary folder: what is
//! written where (never over anything), what the model is told, and what is refused.

use super::*;
use crate::ai::actions::tests::{Host, fixture_doc, rev, sign, steps};
use crate::ai::text::DocText;
use crate::ai::tools::{Mode, NEVER, builtin, quiet};

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
fn comparing_with_another_open_document_asks_every_time_and_names_it() {
    use crate::engine::fixture::{self, Fixture};
    let h = fixture_doc("jobs-compare-other");
    // the other document is opened the way the user would: a file of its own in the same folder
    let other = folder(&h).join("Older version.pdf");
    std::fs::write(&other, fixture::build(&Fixture { pages: 2, lines: 4, needle_every: 2, ..Default::default() })).unwrap();
    h.eng.open(&other.to_string_lossy()).unwrap();

    // reading another document's content is asked about in every mode, Full too
    let t = tool_named("compare_with_open_document");
    assert_eq!(t.class, Class::Permanent);
    for m in [Mode::Ask, Mode::Auto, Mode::Full] {
        assert!(m.needs_approval(t.class), "{m:?}");
    }
    // the card names the file as the model asked for it, found or not (found by name, any case, when it runs)
    let c = card(&h, "compare_with_open_document", json!({ "document": "older VERSION.pdf" })).unwrap().unwrap();
    assert!(c.contains("older VERSION.pdf"), "{c}");
    let o = ok(&h, "compare_with_open_document", json!({ "document": "Older version.pdf" }));
    assert!(o.text.contains("Older version.pdf") && o.text.contains("change"), "{}", o.text);
    assert!(o.text.contains("<<<DOCUMENT"), "document-derived lines are a data block: {}", o.text);

    // the card is the same whether or not a document of that name is open, so names cannot be guessed before the
    // user is asked; the job then reports the miss, without saying what else is open
    let hit = card(&h, "compare_with_open_document", json!({ "document": "Older version.pdf" })).unwrap().unwrap();
    let miss = card(&h, "compare_with_open_document", json!({ "document": "secret-plans.pdf" })).unwrap().unwrap();
    assert_eq!(hit.replace("Older version.pdf", "X"), miss.replace("secret-plans.pdf", "X"), "{hit} / {miss}");
    let e = err_of(&h, "compare_with_open_document", json!({ "document": "secret-plans.pdf" }));
    assert!(e.contains("No other open document") && !e.contains("Older version"), "{e}");
    assert!(err_of(&h, "compare_with_open_document", json!({ "document": "a.pdf" })).contains("No other open document"), "this document is not another one");
    assert!(card(&h, "compare_with_open_document", json!({})).unwrap_err().contains("document"));
    // the listing tool is gone: names of open documents come from the user
    assert!(builtin().iter().all(|t| t.name != "list_open_documents"));
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
    // another document is not this tool's: it says which one to use, whatever is asked
    assert!(err_of(&h, "compare_documents", json!({ "with": h.doc })).contains("compare_with_open_document"));
    assert!(err_of(&h, "compare_documents", json!({ "with": "the other one" })).contains("compare_with_open_document"));
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
    // a range or list that names no page is not "every page"
    for bad in [json!({ "from": 3, "to": 1 }), json!({ "pages": ["x"] }), json!({ "pages": [] }), json!({ "pages": "1-2" })] {
        let mut a = json!({ "format": "text" });
        a.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
        assert!(err_of(&h, "export_document", a.clone()).contains("name no page"), "{a}");
    }
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
    assert_eq!(tool_named("compare_documents").class, Class::Read);
    // they write files or change the document: never silent in ask mode
    for edit in ["compress_document", "ocr_document", "export_document", "convert_to_standard", "create_blank_pdf"] {
        assert_eq!(tool_named(edit).class, Class::Edit, "{edit}");
    }
}

#[test]
fn a_file_that_appears_at_the_chosen_name_is_never_replaced() {
    use std::sync::atomic::AtomicBool;
    let h = fixture_doc("jobs-no-replace");
    let dir = folder(&h);
    let cancel = AtomicBool::new(false);
    let theirs = b"someone else's file".to_vec();

    // compress: the name was free when it was picked and is taken by the time the output is published
    let taken = dir.join("a (compressed).pdf");
    std::fs::write(&taken, &theirs).unwrap();
    let req = CompressRequest { doc: h.doc, preset: crate::compress::Preset::Smallest, custom: None, save_to: None };
    let e = run_compress_new(&h.eng, &req, taken.clone(), &cancel, &mut |_| {}).unwrap_err();
    assert_eq!(e.0, "output-exists");
    assert_eq!(std::fs::read(&taken).unwrap(), theirs);

    // export, a single file
    let taken = dir.join("a.txt");
    std::fs::write(&taken, &theirs).unwrap();
    let req = ExportRequest { doc: h.doc, kind: ExportKind::Text { layout: false }, pages: None, save_to: String::new() };
    let e = convert::export_new(&h.eng, &req, Target::File(taken.clone()), &cancel, &mut |_| {}).unwrap_err();
    assert_eq!(e.to_string(), "output-exists");
    assert_eq!(std::fs::read(&taken).unwrap(), theirs);

    // conversion
    let taken = dir.join("a (PDF-A-2b).pdf");
    std::fs::write(&taken, &theirs).unwrap();
    let c = crate::preflight::Convert { groups: vec![crate::preflight::FixGroup::Metadata], lang: Some("en-US".into()) };
    let r = crate::preflight::convert_to_new_file(&h.eng, h.doc, crate::preflight::Profile::Pdfa2b, &c, &taken.to_string_lossy(), &mut |_| {}, &cancel);
    assert_eq!(r.err().map(|e| e.to_string()).as_deref(), Some("output-exists"));
    assert_eq!(std::fs::read(&taken).unwrap(), theirs);
    // a folder that already has the name is not filled: the export takes the next free one
    let folder = dir.join("a (images)");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("keep.txt"), b"mine").unwrap();
    ok(&h, "export_document", json!({ "format": "images", "dpi": 72, "pages": [1] }));
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "files were put into a folder that was there already");
    assert!(dir.join("a (images) (2)").is_dir(), "{:?}", names(&dir));
    // none of them left a stray temporary file behind
    let left = names(&dir);
    assert!(left.iter().all(|n| !n.contains(".tmp") && !n.contains("partial")), "{left:?}");
}

#[test]
fn converting_a_protected_document_is_refused_before_the_card_and_names_its_output() {
    let h = fixture_doc("jobs-convert-protected");
    // the card names the file the copy would be written to
    let text = card(&h, "convert_to_standard", json!({ "profile": "pdfa-2b", "lang": "en-US" })).unwrap().unwrap();
    assert!(text.contains("(PDF-A-2b).pdf") && text.contains("original is not changed"), "{text}");
    // a permissions password only: the file opens, but a converted copy would be written without it
    let spec = crate::protect::ProtectSpec {
        open_password: None,
        permissions_password: Some("owner-pw".into()),
        printing: Default::default(),
        changes: Default::default(),
        copy: true,
        accessibility: true,
        cipher: Default::default(),
    };
    h.eng.protect_and_save(h.doc, spec, None, None).unwrap();
    let dir = folder(&h);
    let files = names(&dir);
    let args = json!({ "profile": "pdfa-2b", "lang": "en-US" });
    let e = card(&h, "convert_to_standard", args.clone()).unwrap_err();
    assert!(e.contains("password"), "{e}");
    assert!(err_of(&h, "convert_to_standard", args).contains("password"));
    assert_eq!(names(&dir), files, "nothing was written");
}

#[test]
fn exporting_a_protected_document_is_refused_before_the_card() {
    let h = fixture_doc("jobs-export-protected");
    let args = json!({ "format": "text" });
    assert!(card(&h, "export_document", args.clone()).unwrap().is_some());
    // a permissions password only: the file opens and may be copied from, but an export is written without it
    let spec = crate::protect::ProtectSpec {
        open_password: None,
        permissions_password: Some("owner-pw".into()),
        printing: Default::default(),
        changes: Default::default(),
        copy: true,
        accessibility: true,
        cipher: Default::default(),
    };
    h.eng.protect_and_save(h.doc, spec, None, None).unwrap();
    let dir = folder(&h);
    let files = names(&dir);
    for a in [args, json!({ "format": "images", "dpi": 72 })] {
        let e = card(&h, "export_document", a.clone()).unwrap_err();
        assert!(e.contains("password protected") && e.contains("Export tool"), "{e}");
        assert!(err_of(&h, "export_document", a).contains("password protected"));
    }
    assert_eq!(names(&dir), files, "nothing was written");
}
