//! Batch tests. Everything runs on scratch folders under the temp dir; recognition uses a stub engine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mupdf::pdf::PdfDocument;
use mupdf::{Document, TextPageFlags};
use serde_json::json;

use super::action::*;
use super::run::{Event, Report, Status, run};
use super::steps::Env;
use super::store::{self, Store, StoreError};
use crate::compress::Preset;
use crate::engine::fixture::{self, Fixture};
use crate::marks::{Bates, HeaderFooter, Watermark};
use crate::ocr::OcrError;
use crate::ocr::backend::{Bitmap, RawLine, RawPage, RawWord, Recognizer};
use crate::redact::{Pattern, Sanitize};
use crate::tools::BackendChoice;

// ---------------------------------------------------------------- fixtures

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("batch-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Every entry of `d` (files and folders), by name.
fn names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// A PDF whose pages carry the given text lines (Helvetica, one line each from the top).
fn text_pdf(pages: &[&[&str]]) -> Vec<u8> {
    let n = pages.len();
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 4 + i * 2)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>"),
        // Lines are 28 pt apart: a glyph's box is taller than its size, and a redaction mark on a line
        // takes the glyphs of a line closer than that with it.
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 126 /Widths [278 278 355 556 556 889 667 191 333 333 389 584 278 333 278 278 556 556 556 556 556 556 556 556 556 556 278 278 584 584 584 556 1015 667 667 722 722 667 611 778 722 278 500 667 556 833 722 778 667 778 722 667 611 722 667 944 667 667 611 278 278 278 469 556 333 556 556 500 556 556 278 556 556 222 222 500 222 833 556 556 556 556 333 500 278 556 500 722 500 500 500 334 260 334 584] >>"),
    ];
    for (i, lines) in pages.iter().enumerate() {
        let mut c = String::new();
        for (k, l) in lines.iter().enumerate() {
            c.push_str(&format!("BT /F1 12 Tf 72 {} Td ({}) Tj ET\n", 700 - 28 * k, l.replace('(', "\\(").replace(')', "\\)")));
        }
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + i * 2
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
    }
    assemble(&objs, "")
}

fn assemble(objs: &[String], trailer_extra: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offs {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
    out
}

fn put(d: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = d.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn sample(d: &Path, name: &str, pages: usize) -> PathBuf {
    put(d, name, &fixture::build(&Fixture { pages, ..Fixture::default() }))
}

fn digest(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}

fn text_of(p: &Path) -> String {
    let doc = Document::open(p.to_string_lossy().as_ref()).unwrap();
    (0..doc.page_count().unwrap())
        .map(|i| doc.load_page(i).unwrap().to_text_page(TextPageFlags::empty()).unwrap().to_text().unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn pages_of(p: &Path) -> i32 {
    Document::open(p.to_string_lossy().as_ref()).unwrap().page_count().unwrap()
}

struct Stub {
    /// Runs in every recognition (a cancel that lands mid-step).
    during: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Recognizer for Stub {
    fn name(&self) -> &'static str {
        "stub"
    }
    fn languages(&self) -> Result<Vec<String>, OcrError> {
        Ok(vec!["en-US".into()])
    }
    fn max_dimension(&self) -> u32 {
        10_000
    }
    fn recognize(&self, _bmp: &Bitmap, _l: &[String], cancel: &AtomicBool) -> Result<RawPage, OcrError> {
        if let Some(f) = self.during.lock().unwrap().as_ref() {
            f();
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(OcrError::Cancelled);
        }
        let words = [("Invoice", [72.0, 100.0, 120.0, 114.0]), ("Total", [126.0, 100.0, 166.0, 114.0])];
        Ok(RawPage {
            lines: vec![RawLine { words: words.iter().map(|(t, r)| RawWord { text: t.to_string(), rect: *r }).collect() }],
            angle_deg: None,
        })
    }
}

fn env() -> Env {
    let mut e = Env::bare();
    e.date = crate::marks::Date { year: 2026, month: 10, day: 11 };
    e.recognizer = Some(Arc::new(Stub { during: Mutex::new(None) }));
    e
}

fn action(name: &str, steps: Vec<Step>) -> Action {
    Action { name: name.into(), steps, ..Action::default() }
}

fn footer(text: &str) -> Step {
    let mut hf = HeaderFooter::default();
    hf.text[4] = text.into();
    Step::HeaderFooter(hf)
}

fn go(a: &Action, files: &[PathBuf], folder: Option<&Path>) -> Report {
    run(a, files, folder, &env(), &AtomicBool::new(false), &mut |_| {}).unwrap()
}

fn issues_of(a: &Action) -> Vec<(Option<usize>, &'static str)> {
    a.validate(&env()).into_iter().map(|i| (i.step, i.code)).collect()
}

// ---------------------------------------------------------------- schema and store

fn every_step() -> Vec<Step> {
    vec![
        Step::Compress(CompressStep { preset: Preset::Smallest }),
        Step::Ocr(OcrStep { languages: vec!["en-US".into()], force: true, backend: BackendChoice::Tesseract }),
        footer("Page <<1>> of <<n>>"),
        Step::Watermark(Watermark { text: "DRAFT".into(), ..Watermark::default() }),
        Step::Bates(BatesStep { bates: Bates { prefix: "ACME-".into(), ..Bates::default() }, continue_across_files: false }),
        Step::Sanitize(Sanitize::default()),
        Step::Redact(RedactStep {
            queries: vec![
                RedactQuery::Pattern { pattern: Pattern::Email },
                RedactQuery::Text { text: "secret".into(), case_sensitive: true, whole_word: true },
                RedactQuery::Regex { regex: r"\d{4}".into(), case_sensitive: false },
            ],
            overlay_text: Some("REDACTED".into()),
            sanitize: Sanitize::NONE,
        }),
        Step::FlattenForms(FlattenFormsStep {}),
        Step::ExportImages(ExportImagesStep { dpi: 96, ..ExportImagesStep::default() }),
        Step::ExportText(ExportTextStep { layout: true }),
        Step::SetProperties(PropertiesStep { title: Some("T".into()), author: Some(String::new()), ..PropertiesStep::default() }),
        Step::Protect(ProtectStep { printing: crate::protect::Printing::High, ..ProtectStep::default() }),
    ]
}

#[test]
fn schema_round_trips_every_step() {
    let a = Action {
        id: "round-trip".into(),
        name: "All of it".into(),
        description: "Every step kind".into(),
        steps: every_step(),
        output: OutputRule::Folder { suffix: " (done)".into() },
        on_error: OnError::Stop,
        built_in: false,
    };
    let bytes = store::to_file(&a).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["format"], "inkwell-batch-action");
    assert_eq!(v["version"], 1);
    assert_eq!(v["action"]["steps"][0]["kind"], "compress");
    assert_eq!(v["action"]["steps"][4]["kind"], "bates");
    assert_eq!(v["action"]["output"]["kind"], "folder");
    assert_eq!(v["action"]["onError"], "stop");
    assert_eq!(store::from_file(&bytes).unwrap(), a);
    // Reading what was written and writing it again changes nothing.
    assert_eq!(store::to_file(&store::from_file(&bytes).unwrap()).unwrap(), bytes);
}

#[test]
fn a_file_missing_fields_gets_the_defaults() {
    let file = json!({ "format": "inkwell-batch-action", "version": 1, "action": {
        "name": "Minimal", "steps": [ { "kind": "compress" }, { "kind": "headerFooter", "text": ["", "", "", "", "<<1>>", ""] } ] } });
    let a = store::from_file(file.to_string().as_bytes()).unwrap();
    assert_eq!(a.steps[0], Step::Compress(CompressStep { preset: Preset::Balanced }));
    assert_eq!(a.output, OutputRule::default());
    assert_eq!(a.on_error, OnError::Skip);
    assert!(!a.built_in);
    assert!(a.validate(&env()).is_empty(), "{:?}", a.validate(&env()));
}

#[test]
fn versions_and_foreign_files_are_refused() {
    let wrap = |v: serde_json::Value| json!({ "format": "inkwell-batch-action", "version": v, "action": { "name": "x", "steps": [] } }).to_string();
    assert_eq!(store::from_file(wrap(json!(2)).as_bytes()).unwrap_err(), StoreError::TooNew(2));
    assert_eq!(store::from_file(wrap(json!(999)).as_bytes()).unwrap_err(), StoreError::TooNew(999));
    for bad in [json!(0), json!("1"), json!(null), json!(-1)] {
        assert!(matches!(store::from_file(wrap(bad).as_bytes()), Err(StoreError::Invalid(_))));
    }
    let nover = json!({ "format": "inkwell-batch-action", "action": { "name": "x" } }).to_string();
    assert!(matches!(store::from_file(nover.as_bytes()), Err(StoreError::Invalid(_))));
    let other = json!({ "format": "something-else", "version": 1, "action": {} }).to_string();
    assert!(matches!(store::from_file(other.as_bytes()), Err(StoreError::Invalid(_))));
    assert!(matches!(store::from_file(b"not json"), Err(StoreError::Invalid(_))));
    assert!(matches!(store::from_file(&vec![b' '; (store::MAX_FILE + 1) as usize]), Err(StoreError::Invalid(_))));
}

#[test]
fn passwords_are_never_written() {
    let a = action(
        "Locked",
        vec![Step::Protect(ProtectStep {
            open_password: Some("open-sesame-123".into()),
            permissions_password: Some("owner-secret-456".into()),
            ..ProtectStep::default()
        })],
    );
    let bytes = store::to_file(&a).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(!text.contains("open-sesame") && !text.contains("owner-secret") && !text.contains("Password"), "{text}");
    let back = store::from_file(&bytes).unwrap();
    let Step::Protect(p) = &back.steps[0] else { panic!() };
    assert!(p.open_password.is_none() && p.permissions_password.is_none());
    // Debug output hides them too.
    assert!(!format!("{a:?}").contains("open-sesame"));

    let d = dir("passwords");
    let s = Store::new(&d);
    let saved = s.save(a).unwrap();
    for f in names(&d) {
        let t = std::fs::read_to_string(d.join(f)).unwrap();
        assert!(!t.contains("open-sesame") && !t.contains("owner-secret"));
    }
    // A stored Protect step cannot run until the password is typed again.
    assert!(issues_of(&s.get(&saved.id).unwrap()).contains(&(Some(0), "invalid-step")));
}

#[test]
fn the_store_saves_lists_gets_and_deletes() {
    let d = dir("store").join("actions");
    let s = Store::new(&d);
    let builtin_count = s.list().len();
    assert_eq!(builtin_count, 4);
    assert!(s.list().iter().all(|a| a.built_in));

    let saved = s.save(action("  Zebra  ", vec![footer("<<1>>")])).unwrap();
    assert!(saved.id.starts_with("a-") && store::id_ok(&saved.id));
    assert_eq!(saved.name, "Zebra");
    let other = s.save(action("apple", vec![footer("<<1>>")])).unwrap();
    let list = s.list();
    assert_eq!(list.len(), builtin_count + 2);
    assert_eq!(list[builtin_count].name, "apple", "saved actions come after the built-ins, by name");
    assert_eq!(s.get(&saved.id).unwrap(), saved);

    // Saving under the same id replaces it.
    let mut edited = saved.clone();
    edited.description = "changed".into();
    s.save(edited).unwrap();
    assert_eq!(s.get(&saved.id).unwrap().description, "changed");
    assert_eq!(s.list().len(), builtin_count + 2);

    s.delete(&other.id).unwrap();
    assert_eq!(s.get(&other.id).unwrap_err(), StoreError::NotFound);
    assert_eq!(s.delete(&other.id).unwrap_err(), StoreError::NotFound);
    assert!(names(&d).iter().all(|n| !n.ends_with(".tmp")));
}

#[test]
fn the_store_refuses_bad_ids_built_ins_and_unnamed_actions() {
    let d = dir("store-refuse");
    let s = Store::new(d.join("actions"));
    for id in ["../escape", "a/b", "A", "x.json", "-lead", "has space", &"x".repeat(65), "a\\b"] {
        let mut a = action("x", vec![footer("<<1>>")]);
        a.id = id.into();
        assert!(matches!(s.save(a), Err(StoreError::Invalid(_))), "{id}");
        assert!(matches!(s.get(id), Err(StoreError::NotFound)), "{id}");
        assert!(matches!(s.delete(id), Err(StoreError::NotFound)), "{id}");
    }
    assert!(!d.join("escape.json").exists());

    let builtin = s.list().remove(0);
    assert!(builtin.built_in);
    assert_eq!(s.save(builtin.clone()).unwrap_err(), StoreError::BuiltIn);
    assert_eq!(s.delete(&builtin.id).unwrap_err(), StoreError::BuiltIn);
    assert_eq!(s.get(&builtin.id).unwrap(), builtin);

    assert!(matches!(s.save(action("   ", vec![])), Err(StoreError::Invalid(_))));
    // An unreadable step is not stored: the action would lose it silently.
    let mut a = action("Broken", vec![]);
    a.steps.push(Step::Invalid { name: "wat".into(), why: "no".into() });
    assert!(matches!(s.save(a), Err(StoreError::Invalid(_))));
}

#[test]
fn import_and_export_round_trip_and_never_replace() {
    let d = dir("import");
    let s = Store::new(d.join("actions"));
    let a = s.save(action("Mine", vec![footer("<<1>>"), Step::Compress(CompressStep::default())])).unwrap();
    let bytes = store::to_file(&a).unwrap();

    let imported = s.import(&bytes).unwrap();
    assert_ne!(imported.id, a.id, "an import gets its own id");
    assert_eq!(imported.name, "Mine (imported)");
    assert_eq!(imported.steps, a.steps);
    assert_eq!(s.get(&a.id).unwrap(), a, "the original is untouched");
    assert!(!imported.built_in);

    // A file that says it is a built-in, or names an id, is still just a new action.
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["action"]["builtIn"] = json!(true);
    v["action"]["id"] = json!("builtin-make-searchable");
    let again = s.import(v.to_string().as_bytes()).unwrap();
    assert!(!again.built_in && !again.id.starts_with("builtin"));
    assert_eq!(s.list().iter().filter(|x| x.id == "builtin-make-searchable").count(), 1);

    assert!(matches!(s.import(b"{}"), Err(StoreError::Invalid(_))));
    let newer = json!({ "format": "inkwell-batch-action", "version": 7, "action": { "name": "n" } }).to_string();
    assert_eq!(s.import(newer.as_bytes()).unwrap_err(), StoreError::TooNew(7));
}

#[test]
fn an_unknown_step_survives_the_file_and_fails_validation() {
    let file = json!({ "format": "inkwell-batch-action", "version": 1, "action": { "name": "Future", "steps": [
        { "kind": "compress" }, { "kind": "teleport", "where": "moon" }, { "kind": "removeSecurity" },
        { "kind": "redact", "queries": "oops" }, { "nokind": 1 } ] } });
    let a = store::from_file(file.to_string().as_bytes()).unwrap();
    assert_eq!(a.steps.len(), 5);
    let codes = issues_of(&a);
    assert_eq!(
        codes,
        vec![(Some(1), "unknown-step"), (Some(2), "forbidden-step"), (Some(3), "unknown-step"), (Some(4), "unknown-step")]
    );
    let msgs: Vec<String> = a.validate(&env()).into_iter().map(|i| i.message).collect();
    assert!(msgs[0].contains("teleport"), "{msgs:?}");
    assert!(msgs[1].contains("Removing security is not allowed"), "{msgs:?}");
    assert!(msgs[2].contains("redact"), "{msgs:?}");
}

#[test]
fn the_four_built_ins_validate_and_have_unique_ids() {
    let b = store::builtins();
    assert_eq!(b.len(), 4);
    let ids: std::collections::HashSet<_> = b.iter().map(|a| a.id.clone()).collect();
    assert_eq!(ids.len(), 4);
    for a in &b {
        assert!(a.built_in && store::id_ok(&a.id), "{}", a.id);
        assert!(a.validate(&env()).is_empty(), "{}: {:?}", a.name, a.validate(&env()));
        // Each one survives its own file format.
        assert_eq!(store::from_file(&store::to_file(a).unwrap()).unwrap().steps, a.steps);
    }
    let names: Vec<_> = b.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["Prepare for email", "Make searchable", "Publish with watermark", "Redact emails and phone numbers"]);
}

// ---------------------------------------------------------------- validation

#[test]
fn validation_names_what_is_wrong_before_any_file_is_touched() {
    let mut a = action("", vec![]);
    assert_eq!(issues_of(&a), vec![(None, "name"), (None, "no-steps")]);

    a = action("x".repeat(81).as_str(), vec![footer("<<1>>")]);
    assert_eq!(issues_of(&a), vec![(None, "name")]);

    a = action("ok", vec![footer("<<1>>")]);
    a.output = OutputRule::NextToSource { suffix: String::new() };
    assert_eq!(issues_of(&a), vec![(None, "output")], "a copy next to its source needs an ending");
    a.output = OutputRule::NextToSource { suffix: "/../x".into() };
    assert_eq!(issues_of(&a), vec![(None, "output")]);
    a.output = OutputRule::Folder { suffix: String::new() };
    assert!(a.validate(&env()).is_empty(), "a folder result may keep the name");

    // Steps with settings that cannot work.
    let bad = vec![
        Step::HeaderFooter(HeaderFooter::default()),
        Step::Watermark(Watermark::default()),
        Step::Watermark(Watermark {
            text: "x".into(),
            picture: Some(crate::marks::Picture { file: "tok".into(), page: 0, password: None }),
            ..Watermark::default()
        }),
        Step::Bates(BatesStep { bates: Bates { digits: 2, ..Bates::default() }, continue_across_files: true }),
        Step::Redact(RedactStep::default()),
        Step::Redact(RedactStep { queries: vec![RedactQuery::Regex { regex: "(".into(), case_sensitive: false }], ..RedactStep::default() }),
        Step::Redact(RedactStep { queries: vec![RedactQuery::Text { text: "  ".into(), case_sensitive: false, whole_word: false }], ..RedactStep::default() }),
        Step::ExportImages(ExportImagesStep { dpi: 5000, ..ExportImagesStep::default() }),
        Step::SetProperties(PropertiesStep::default()),
        Step::Compress(CompressStep { preset: Preset::Custom }),
        Step::Protect(ProtectStep::default()),
    ];
    let a = action("bad", bad);
    let got = issues_of(&a);
    for i in 0..a.steps.len() {
        // The second watermark step also repeats the first.
        assert!(got.contains(&(Some(i), "invalid-step")), "step {i}: {got:?}");
    }
    let m = a.validate(&env());
    assert!(m[0].message.contains("header or footer"), "{}", m[0].message);
    assert!(m[2].message.contains("Use text"), "{}", m[2].message);
}

#[test]
fn ordering_rules_protect_last_and_no_repeated_stamps() {
    let protect = Step::Protect(ProtectStep { open_password: Some("pw-one-2".into()), ..ProtectStep::default() });
    let a = action("o", vec![protect.clone(), Step::Compress(CompressStep::default())]);
    assert_eq!(issues_of(&a), vec![(Some(0), "order")]);
    let a = action("o", vec![Step::Compress(CompressStep::default()), protect.clone()]);
    assert!(a.validate(&env()).is_empty());
    let a = action("o", vec![footer("a<<1>>"), footer("b<<1>>")]);
    assert_eq!(issues_of(&a), vec![(Some(1), "order")]);
}

#[test]
fn an_engine_that_is_not_there_fails_validation_not_the_run() {
    let a = action("ocr", vec![Step::Ocr(OcrStep { backend: BackendChoice::Tesseract, ..OcrStep::default() })]);
    let mut bare = Env::bare();
    bare.tesseract = None;
    let issues = a.validate(&bare);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!((issues[0].step, issues[0].code), (Some(0), "unavailable"));
    assert!(issues[0].message.contains("Tesseract"), "{}", issues[0].message);

    let d = dir("unavailable");
    let f = sample(&d, "a.pdf", 1);
    let err = run(&a, &[f.clone()], None, &bare, &AtomicBool::new(false), &mut |_| {}).unwrap_err();
    assert!(matches!(err, super::BatchError::Invalid(_)));
    assert_eq!(names(&d), ["a.pdf"], "nothing was written");
    // With an engine present the same action is fine.
    assert!(a.validate(&env()).is_empty());
}

#[test]
fn a_run_needs_files_and_a_folder_when_the_rule_says_so() {
    let d = dir("inputs");
    let f = sample(&d, "a.pdf", 1);
    let a = action("x", vec![footer("<<1>>")]);
    let cancel = AtomicBool::new(false);
    assert!(matches!(run(&a, &[], None, &env(), &cancel, &mut |_| {}), Err(super::BatchError::Input(_))));
    let mut b = a.clone();
    b.output = OutputRule::Folder { suffix: String::new() };
    assert!(matches!(run(&b, &[f.clone()], None, &env(), &cancel, &mut |_| {}), Err(super::BatchError::Input(_))));
    assert!(matches!(run(&b, &[f.clone()], Some(&d.join("nope")), &env(), &cancel, &mut |_| {}), Err(super::BatchError::Input(_))));
    assert_eq!(names(&d), ["a.pdf"]);
}

// ---------------------------------------------------------------- runs

#[test]
fn a_three_file_run_with_a_bad_file_skips_it_and_goes_on() {
    let d = dir("skip");
    let a = sample(&d, "a.pdf", 2);
    let bad = put(&d, "bad.pdf", b"this is not a pdf at all");
    let c = sample(&d, "c.pdf", 3);
    let before: Vec<_> = [&a, &bad, &c].iter().map(|p| digest(p)).collect();

    let mut act = action("Number", vec![footer("Page <<1>> of <<n>>"), Step::SetProperties(PropertiesStep { title: Some("Batch".into()), ..Default::default() })]);
    act.output = OutputRule::NextToSource { suffix: " (done)".into() };
    let mut events = Vec::new();
    let r = run(&act, &[a.clone(), bad.clone(), c.clone()], None, &env(), &AtomicBool::new(false), &mut |e| events.push(e)).unwrap();

    assert_eq!((r.ok, r.skipped, r.errors), (2, 0, 1));
    assert!(!r.cancelled && !r.stopped);
    let st: Vec<_> = r.files.iter().map(|f| f.status).collect();
    assert_eq!(st, [Status::Ok, Status::Error, Status::Ok]);
    assert!(r.files[1].outputs.is_empty());

    for (i, name, pages) in [(0usize, "a (done).pdf", 2), (2, "c (done).pdf", 3)] {
        let f = &r.files[i];
        let out = d.join(name);
        assert_eq!(f.outputs.len(), 1);
        assert_eq!(Path::new(&f.outputs[0].path), out);
        assert_eq!(f.size_before, std::fs::metadata(&f.input).unwrap().len());
        assert_eq!(f.size_after, Some(std::fs::metadata(&out).unwrap().len()));
        assert_eq!(pages_of(&out), pages);
        assert_eq!(f.steps_done, 2);
        let t = text_of(&out);
        assert!(t.contains(&format!("Page 1 of {pages}")), "{t}");
        let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
        let title = doc.trailer().unwrap().get_dict("Info").unwrap().unwrap().get_dict("Title").unwrap().unwrap().as_string().unwrap();
        assert_eq!(title, "Batch");
    }
    assert_eq!(names(&d), ["a (done).pdf", "a.pdf", "bad.pdf", "c (done).pdf", "c.pdf"], "no staging left behind");
    assert_eq!([&a, &bad, &c].iter().map(|p| digest(p)).collect::<Vec<_>>(), before, "sources are untouched");

    // Events: started, then per file started / steps / done, then finished.
    assert!(matches!(events.first(), Some(Event::Started { files: 3 })));
    assert!(matches!(events.last(), Some(Event::Finished { ok: 2, skipped: 0, errors: 1, cancelled: false })));
    let done: Vec<usize> = events.iter().filter_map(|e| if let Event::FileDone { index, .. } = e { Some(*index) } else { None }).collect();
    assert_eq!(done, [0, 1, 2]);
    let steps: Vec<_> = events.iter().filter_map(|e| if let Event::Step { index, step, steps, label } = e { Some((*index, *step, *steps, label.clone())) } else { None }).collect();
    assert_eq!(steps[0], (0, 0, 2, "Add header and footer".to_string()));
    assert_eq!(steps[1], (0, 1, 2, "Set document properties".to_string()));
    assert_eq!(steps.iter().filter(|s| s.0 == 1).count(), 0, "the bad file fails before its first step");
    assert!(r.files[1].reason.as_deref().unwrap().contains("not a PDF"), "{:?}", r.files[1].reason);
}

#[test]
fn stop_mode_leaves_the_files_after_a_failure_alone() {
    let d = dir("stop");
    let a = sample(&d, "a.pdf", 1);
    let bad = put(&d, "bad.pdf", b"%PDF-1.4 garbage");
    let c = sample(&d, "c.pdf", 1);
    let mut act = action("Number", vec![footer("<<1>>")]);
    act.on_error = OnError::Stop;
    let r = go(&act, &[a, bad, c.clone()], None);
    let st: Vec<_> = r.files.iter().map(|f| f.status).collect();
    assert_eq!(st, [Status::Ok, Status::Error, Status::Skipped]);
    assert!(r.stopped && !r.cancelled);
    assert!(r.files[2].reason.as_deref().unwrap().contains("stopped after an error in bad.pdf"));
    assert_eq!((r.ok, r.skipped, r.errors), (1, 1, 1));
    assert_eq!(names(&d), ["a (batch).pdf", "a.pdf", "bad.pdf", "c.pdf"]);
}

#[test]
fn a_cancel_between_files_keeps_the_finished_ones_whole_and_leaves_nothing_else() {
    let d = dir("cancel");
    let files: Vec<_> = (0..3).map(|i| sample(&d, &format!("f{i}.pdf"), 2)).collect();
    let act = action("Number", vec![footer("<<1>>"), Step::Compress(CompressStep::default())]);
    let cancel = AtomicBool::new(false);
    // The cancel lands while the second file is on its first step.
    let r = run(&act, &files, None, &env(), &cancel, &mut |e| {
        if let Event::Step { index: 1, step: 0, .. } = e {
            cancel.store(true, Ordering::SeqCst);
        }
    })
    .unwrap();
    assert!(r.cancelled);
    let st: Vec<_> = r.files.iter().map(|f| f.status).collect();
    assert_eq!(st, [Status::Ok, Status::Skipped, Status::Skipped]);
    assert!(r.files.iter().skip(1).all(|f| f.reason.as_deref() == Some("cancelled")));
    assert_eq!(names(&d), ["f0 (batch).pdf", "f0.pdf", "f1.pdf", "f2.pdf"], "no partial output, no staging folder");
    assert_eq!(pages_of(&d.join("f0 (batch).pdf")), 2);
}

#[test]
fn a_cancel_in_the_middle_of_a_step_drops_that_files_work() {
    let d = dir("cancel-mid");
    let scans: Vec<_> = (0..2).map(|i| put(&d, &format!("scan{i}.pdf"), &blank(2))).collect();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let mut e = env();
    e.recognizer = Some(Arc::new(Stub { during: Mutex::new(Some(Box::new(move || flag.store(true, Ordering::SeqCst)))) }));
    let act = action("OCR", vec![Step::SetProperties(PropertiesStep { title: Some("t".into()), ..Default::default() }), Step::Ocr(OcrStep::default())]);
    let r = run(&act, &scans, None, &e, &cancel, &mut |_| {}).unwrap();
    assert!(r.cancelled);
    assert_eq!(r.files.iter().map(|f| f.status).collect::<Vec<_>>(), [Status::Skipped, Status::Skipped]);
    assert_eq!(names(&d), ["scan0.pdf", "scan1.pdf"], "the properties step's staged file is gone too");
}

#[test]
fn nothing_is_ever_written_over_an_existing_file() {
    let d = dir("never-overwrite");
    let a = sample(&d, "a.pdf", 1);
    let taken = put(&d, "a (batch).pdf", b"someone else's file");
    let act = action("Number", vec![footer("<<1>>")]);

    let r1 = go(&act, &[a.clone()], None);
    let r2 = go(&act, &[a.clone()], None);
    assert_eq!(Path::new(&r1.files[0].outputs[0].path), d.join("a (batch) (2).pdf"));
    assert_eq!(Path::new(&r2.files[0].outputs[0].path), d.join("a (batch) (3).pdf"));
    assert_eq!(digest(&taken), b"someone else's file");

    // A folder result with no ending would land on the source itself: it gets a number instead.
    let mut b = act.clone();
    b.output = OutputRule::Folder { suffix: String::new() };
    let before = digest(&a);
    let r3 = go(&b, &[a.clone()], Some(&d));
    assert_eq!(Path::new(&r3.files[0].outputs[0].path), d.join("a (2).pdf"));
    assert_eq!(digest(&a), before);

    // The same file listed twice is processed once.
    let r4 = go(&act, &[a.clone(), a.clone()], None);
    assert_eq!(r4.files[0].status, Status::Ok);
    assert_eq!((r4.files[1].status, r4.files[1].reason.as_deref()), (Status::Skipped, Some("listed more than once")));
}

#[test]
fn a_folder_rule_writes_there_and_only_there() {
    let d = dir("folder");
    let out = d.join("out");
    std::fs::create_dir(&out).unwrap();
    let src = d.join("in");
    std::fs::create_dir(&src).unwrap();
    let a = sample(&src, "a.pdf", 1);
    let mut act = action("Number", vec![footer("<<1>>")]);
    act.output = OutputRule::Folder { suffix: " - numbered".into() };
    let r = go(&act, &[a], Some(&out));
    assert_eq!(names(&out), ["a - numbered.pdf"]);
    assert_eq!(names(&src), ["a.pdf"]);
    assert_eq!(Path::new(&r.files[0].outputs[0].path), out.join("a - numbered.pdf"));
}

#[test]
fn a_file_open_in_the_app_is_skipped_with_the_reason() {
    let d = dir("open");
    let a = sample(&d, "a.pdf", 1);
    let b = sample(&d, "b.pdf", 1);
    let open = a.clone();
    let mut e = env();
    e.is_open = Arc::new(move |p| p == open);
    let act = action("Number", vec![footer("<<1>>")]);
    let r = run(&act, &[a, b], None, &e, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(r.files[0].status, Status::Skipped);
    assert!(r.files[0].reason.as_deref().unwrap().contains("open in Inkwell"));
    assert_eq!(r.files[1].status, Status::Ok);
    assert_eq!(names(&d), ["a.pdf", "b (batch).pdf", "b.pdf"]);
    // A skip is not an error: stop mode goes on.
    let mut stop = act.clone();
    stop.on_error = OnError::Stop;
    let r = run(&stop, &[d.join("a.pdf"), d.join("b.pdf")], None, &e, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert!(!r.stopped);
    assert_eq!(r.files[1].status, Status::Ok);
}

/// A PDF with a signature field and the "signatures exist" flag.
fn signed_pdf() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /SigFlags 3 >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R] /Contents 4 0 R /Resources << >> >>".to_string(),
        "<< /Length 0 >>\nstream\n\nendstream".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Sig1) /Rect [10 10 100 40] /P 3 0 R /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /Contents <0000> /ByteRange [0 0 0 0] >> >>".to_string(),
    ];
    assemble(&objs, "")
}

#[test]
fn a_signed_file_is_refused_for_steps_that_would_break_the_signature() {
    let d = dir("signed");
    let s = put(&d, "signed.pdf", &signed_pdf());
    let plain = sample(&d, "plain.pdf", 1);
    let before = digest(&s);
    for step in [
        footer("<<1>>"),
        Step::Compress(CompressStep::default()),
        Step::Sanitize(Sanitize::default()),
        Step::Watermark(Watermark { text: "X".into(), ..Watermark::default() }),
        Step::FlattenForms(FlattenFormsStep {}),
        Step::SetProperties(PropertiesStep { title: Some("t".into()), ..Default::default() }),
        Step::Redact(RedactStep { queries: vec![RedactQuery::Pattern { pattern: Pattern::Email }], ..RedactStep::default() }),
        Step::Protect(ProtectStep { open_password: Some("pw-one-2".into()), ..ProtectStep::default() }),
        Step::Ocr(OcrStep::default()),
    ] {
        let label = step.label();
        // The refusal comes before any step runs: it holds even when an export would come first.
        let act = action("a", vec![Step::ExportText(ExportTextStep::default()), step]);
        let r = go(&act, &[s.clone(), plain.clone()], None);
        assert_eq!(r.files[0].status, Status::Skipped, "{label}");
        let why = r.files[0].reason.clone().unwrap();
        assert!(why.starts_with("signed:") && why.contains(&label), "{why}");
        assert!(r.files[0].outputs.is_empty(), "{label}");
        assert_eq!(r.files[1].status, Status::Ok, "{label}: other files run");
        assert!(!names(&d).iter().any(|n| n.starts_with("signed ")), "{label}: {:?}", names(&d));
        // clean up outputs of the plain file for the next round
        for n in names(&d) {
            if n != "signed.pdf" && n != "plain.pdf" {
                let _ = std::fs::remove_dir_all(d.join(&n));
                let _ = std::fs::remove_file(d.join(&n));
            }
        }
    }
    assert_eq!(digest(&s), before);

    // Steps that only read a signed file are fine.
    let act = action("read", vec![Step::ExportText(ExportTextStep::default())]);
    let r = go(&act, &[s.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    assert_eq!(digest(&s), before);
    assert!(names(&d).contains(&"signed (batch).txt".to_string()), "{:?}", names(&d));
}

fn blank(n: usize) -> Vec<u8> {
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 3 + i)).collect();
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>")];
    for _ in 0..n {
        objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string());
    }
    assemble(&objs, "")
}

// ---------------------------------------------------------------- the steps

#[test]
fn header_footer_and_watermark_are_stamped_and_updated_not_doubled() {
    let d = dir("marks");
    let f = sample(&d, "a.pdf", 2);
    let act = action(
        "Stamp",
        vec![footer("Doc <<Page 1 of n>> <<yyyy-mm-dd>>"), Step::Watermark(Watermark { text: "SECRET".into(), ..Watermark::default() })],
    );
    let r = go(&act, &[f.clone()], None);
    let out = PathBuf::from(&r.files[0].outputs[0].path);
    let t = text_of(&out);
    assert!(t.contains("Doc Page 1 of 2 2026-10-11") && t.contains("Doc Page 2 of 2"), "{t}");
    assert!(t.contains("SECRET"));

    // Running the action over its own output replaces the stamps.
    let r2 = go(&act, &[out.clone()], None);
    let t2 = text_of(Path::new(&r2.files[0].outputs[0].path));
    assert_eq!(t2.matches("Doc Page 1 of 2").count(), 1, "{t2}");
    assert_eq!(t2.matches("SECRET").count(), 2, "one per page: {t2}");
}

#[test]
fn bates_numbers_continue_across_files_or_restart() {
    let d = dir("bates");
    let files = vec![sample(&d, "a.pdf", 3), sample(&d, "b.pdf", 2)];
    let bates = |cont: bool| BatesStep { bates: Bates { prefix: "X-".into(), digits: 4, start: 10, ..Bates::default() }, continue_across_files: cont };

    let r = go(&action("B", vec![Step::Bates(bates(true))]), &files, None);
    let a = text_of(Path::new(&r.files[0].outputs[0].path));
    let b = text_of(Path::new(&r.files[1].outputs[0].path));
    assert!(a.contains("X-0010") && a.contains("X-0012") && !a.contains("X-0013"), "{a}");
    assert!(b.contains("X-0013") && b.contains("X-0014") && !b.contains("X-0015"), "{b}");
    assert!(r.files[0].notes[0].contains("numbered X-0010 to X-0012"), "{:?}", r.files[0].notes);

    // A failed file does not use up numbers.
    let bad = put(&d, "bad.pdf", b"nope");
    let r = go(&action("B", vec![Step::Bates(bates(true))]), &[files[0].clone(), bad, files[1].clone()], None);
    assert_eq!(r.files[1].status, Status::Error);
    assert!(text_of(Path::new(&r.files[2].outputs[0].path)).contains("X-0013"));

    let r = go(&action("B", vec![Step::Bates(bates(false))]), &files, None);
    assert!(text_of(Path::new(&r.files[1].outputs[0].path)).contains("X-0010"));
}

#[test]
fn bates_that_run_out_of_digits_fail_the_file_not_the_run() {
    let d = dir("bates-digits");
    let big = sample(&d, "big.pdf", 5);
    let small = sample(&d, "small.pdf", 1);
    let step = Step::Bates(BatesStep { bates: Bates { digits: 3, start: 998, ..Bates::default() }, continue_across_files: false });
    let r = go(&action("B", vec![step]), &[big, small], None);
    assert_eq!(r.files[0].status, Status::Error);
    assert!(r.files[0].reason.as_deref().unwrap().contains("digits"), "{:?}", r.files[0].reason);
    assert_eq!(r.files[1].status, Status::Ok);
}

#[test]
fn properties_are_set_and_removed() {
    let d = dir("props");
    let f = sample(&d, "a.pdf", 1);
    let set = Step::SetProperties(PropertiesStep {
        title: Some("Årsrapport".into()),
        author: Some("Berkay".into()),
        subject: Some("S".into()),
        keywords: Some("k1, k2".into()),
        ..Default::default()
    });
    let r = go(&action("P", vec![set]), &[f], None);
    let out = PathBuf::from(&r.files[0].outputs[0].path);
    let info = |p: &Path, key: &str| {
        let doc = PdfDocument::open(p.to_string_lossy().as_ref()).unwrap();
        let i = doc.trailer().unwrap().get_dict("Info").unwrap().unwrap();
        i.get_dict(key).unwrap().map(|v| v.as_string().unwrap_or_default())
    };
    assert_eq!(info(&out, "Author").as_deref(), Some("Berkay"));
    assert_eq!(info(&out, "Keywords").as_deref(), Some("k1, k2"));
    assert!(info(&out, "Title").is_some());
    let doc = Document::open(out.to_string_lossy().as_ref()).unwrap();
    assert_eq!(doc.metadata(mupdf::document::MetadataName::Title).unwrap(), "Årsrapport");

    let clear = Step::SetProperties(PropertiesStep { author: Some(String::new()), ..Default::default() });
    let r = go(&action("P", vec![clear]), &[out.clone()], None);
    let out2 = PathBuf::from(&r.files[0].outputs[0].path);
    assert_eq!(info(&out2, "Author"), None);
    assert_eq!(info(&out2, "Keywords").as_deref(), Some("k1, k2"), "what the step does not name stays");
}

#[test]
fn sanitize_removes_metadata_and_a_clean_file_passes_through() {
    let d = dir("sanitize");
    let f = sample(&d, "a.pdf", 1);
    let with_meta = go(
        &action("M", vec![Step::SetProperties(PropertiesStep { author: Some("Someone".into()), ..Default::default() })]),
        &[f.clone()],
        None,
    );
    let tagged = PathBuf::from(&with_meta.files[0].outputs[0].path);
    let r = go(&action("S", vec![Step::Sanitize(Sanitize { metadata: true, ..Sanitize::NONE })]), &[tagged.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok);
    let out = PathBuf::from(&r.files[0].outputs[0].path);
    let doc = Document::open(out.to_string_lossy().as_ref()).unwrap();
    assert_eq!(doc.metadata(mupdf::document::MetadataName::Author).unwrap(), "");
    assert!(r.files[0].notes[0].contains("removed"), "{:?}", r.files[0].notes);

    // Nothing to remove: the output is a byte copy of the source.
    let r = go(&action("S", vec![Step::Sanitize(Sanitize { bookmarks: true, ..Sanitize::NONE })]), &[f.clone()], None);
    assert_eq!(digest(Path::new(&r.files[0].outputs[0].path)), digest(&f));
    assert!(r.files[0].notes[0].contains("nothing to remove"));
}

#[test]
fn redaction_by_pattern_removes_the_text_and_verifies() {
    let d = dir("redact");
    let f = put(
        &d,
        "contact.pdf",
        &text_pdf(&[
            &["Write to jane.tester@example.com for details.", "Call +46 70 123 45 67 after nine."],
            &["Nothing sensitive on this page, only the word keep."],
        ]),
    );
    let before = digest(&f);
    let act = store::builtins().into_iter().find(|a| a.id == "builtin-redact-emails-and-phones").unwrap();
    let r = go(&act, &[f.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    let out = PathBuf::from(&r.files[0].outputs[0].path);
    assert_eq!(out, d.join("contact (redacted).pdf"));
    let t = text_of(&out);
    assert!(!t.contains("jane.tester") && !t.contains("example.com") && !t.contains("123 45 67"), "{t}");
    assert!(t.contains("for details") && t.contains("keep"), "{t}");
    let raw = String::from_utf8_lossy(&digest(&out)).into_owned();
    assert!(!raw.contains("jane.tester"));
    assert!(r.files[0].notes[0].contains("redacted"), "{:?}", r.files[0].notes);
    assert_eq!(digest(&f), before);

    // A file with nothing to find is copied unchanged, and says so.
    let clean = put(&d, "clean.pdf", &text_pdf(&[&["Nothing to see."]]));
    let r = go(&act, &[clean.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok);
    assert_eq!(digest(Path::new(&r.files[0].outputs[0].path)), digest(&clean));
    assert!(r.files[0].notes[0].contains("nothing found"), "{:?}", r.files[0].notes);
}

#[test]
fn redaction_by_text_and_regex_with_an_overlay() {
    let d = dir("redact-text");
    let f = put(&d, "t.pdf", &text_pdf(&[&["Account 4711 is held.", "Owner: Mr Hidden Person, retired.", "Reference ZX-9921 is public."]]));
    let act = action(
        "R",
        vec![Step::Redact(RedactStep {
            queries: vec![
                RedactQuery::Text { text: "hidden person".into(), case_sensitive: false, whole_word: true },
                RedactQuery::Regex { regex: r"\b\d{4}\b".into(), case_sensitive: false },
            ],
            overlay_text: Some("X".into()),
            sanitize: Sanitize::NONE,
        })],
    );
    let r = go(&act, &[f], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    let t = text_of(Path::new(&r.files[0].outputs[0].path));
    assert!(!t.contains("4711") && !t.contains("Hidden") && !t.contains("Person"), "{t}");
    assert!(t.contains("is held") && t.contains("Owner: Mr") && t.contains("retired") && t.contains("ZX-") && !t.contains("9921"), "{t}");
}

#[test]
fn compress_runs_and_a_file_that_cannot_shrink_is_copied() {
    let d = dir("compress");
    let f = sample(&d, "a.pdf", 3);
    let r = go(&action("C", vec![Step::Compress(CompressStep { preset: Preset::Smallest })]), &[f.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    let out = PathBuf::from(&r.files[0].outputs[0].path);
    assert_eq!(pages_of(&out), 3);
    assert_eq!(text_of(&out), text_of(&f));
    assert_eq!(r.files[0].size_after, Some(std::fs::metadata(&out).unwrap().len()));
    assert_eq!(names(&d), ["a (batch).pdf", "a.pdf"]);
}

#[test]
fn ocr_adds_a_text_layer_to_scans_and_leaves_text_pages() {
    let d = dir("ocr");
    let scan = put(&d, "scan.pdf", &blank(2));
    let texty = sample(&d, "texty.pdf", 1);
    let act = action("OCR", vec![Step::Ocr(OcrStep::default())]);
    let r = go(&act, &[scan.clone(), texty.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    assert!(text_of(Path::new(&r.files[0].outputs[0].path)).contains("Invoice"));
    assert!(r.files[0].notes[0].contains("2 page(s) recognized"));
    // Pages that already have text: nothing to do, a copy comes out.
    assert_eq!(r.files[1].status, Status::Ok);
    assert_eq!(digest(Path::new(&r.files[1].outputs[0].path)), digest(&texty));
}

#[test]
fn protect_writes_an_encrypted_copy_and_a_batch_cannot_open_it_again() {
    let d = dir("protect");
    let f = sample(&d, "a.pdf", 2);
    let act = action(
        "P",
        vec![
            footer("<<1>>"),
            Step::Protect(ProtectStep {
                open_password: Some("open-pw-123".into()),
                permissions_password: Some("owner-pw-456".into()),
                printing: crate::protect::Printing::Low,
                changes: crate::protect::Changes::None,
                ..ProtectStep::default()
            }),
        ],
    );
    let r = go(&act, &[f.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    let out = PathBuf::from(&r.files[0].outputs[0].path);
    let mut doc = Document::open(out.to_string_lossy().as_ref()).unwrap();
    assert!(doc.needs_password().unwrap());
    assert!(!doc.authenticate("wrong").unwrap());
    assert!(doc.authenticate("open-pw-123").unwrap());
    assert_eq!(doc.page_count().unwrap(), 2);
    assert!(!String::from_utf8_lossy(&digest(&out)).contains("open-pw-123"));

    // The protected file needs its password: a batch has none, so it skips it.
    let r = go(&action("again", vec![footer("<<1>>")]), &[out.clone()], None);
    assert_eq!(r.files[0].status, Status::Skipped);
    assert_eq!(r.files[0].reason.as_deref(), Some("needs a password"));
}

#[test]
fn a_file_whose_owner_restricted_copying_is_skipped() {
    let d = dir("restricted");
    let f = sample(&d, "a.pdf", 1);
    let act = action(
        "lock",
        vec![Step::Protect(ProtectStep {
            permissions_password: Some("owner-pw-456".into()),
            printing: crate::protect::Printing::High,
            changes: crate::protect::Changes::None,
            copy: false,
            ..ProtectStep::default()
        })],
    );
    let locked = PathBuf::from(&go(&act, &[f], None).files[0].outputs[0].path);
    let r = go(&action("n", vec![footer("<<1>>")]), &[locked.clone()], None);
    assert_eq!(r.files[0].status, Status::Skipped, "{:?}", r.files[0].reason);
    assert!(r.files[0].reason.as_deref().unwrap().contains("restricted"));
    let r = go(&action("t", vec![Step::ExportText(ExportTextStep::default())]), &[locked], None);
    assert_eq!(r.files[0].status, Status::Skipped, "an export would write the text out unprotected too");
}

#[test]
fn exports_go_next_to_the_result_under_names_that_are_free() {
    let d = dir("exports");
    let f = sample(&d, "doc.pdf", 3);
    std::fs::create_dir(d.join("doc (batch) (images)")).unwrap();
    let act = action(
        "Export",
        vec![
            footer("Stamp <<1>>"),
            Step::ExportImages(ExportImagesStep { dpi: 36, ..ExportImagesStep::default() }),
            Step::ExportText(ExportTextStep::default()),
        ],
    );
    let r = go(&act, &[f.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    assert_eq!(names(&d), ["doc (batch) (images)", "doc (batch) (images) (2)", "doc (batch).pdf", "doc (batch).txt", "doc.pdf"]);
    let imgs = d.join("doc (batch) (images) (2)");
    assert_eq!(names(&imgs), ["doc-1.png", "doc-2.png", "doc-3.png"]);
    assert!(names(&d.join("doc (batch) (images)")).is_empty(), "an existing folder is left alone");
    // Exports see the file as the steps before them left it.
    let txt = std::fs::read_to_string(d.join("doc (batch).txt")).unwrap();
    assert!(txt.contains("Stamp 1") && txt.contains("Stamp 3"), "{txt}");
    assert_eq!(r.files[0].outputs.len(), 3);
    assert_eq!(r.files[0].steps_done, 3);
}

#[test]
fn an_export_only_action_makes_no_pdf() {
    let d = dir("export-only");
    let f = sample(&d, "doc.pdf", 2);
    let act = action("Text", vec![Step::ExportText(ExportTextStep { layout: false })]);
    assert!(!act.writes_pdf());
    let r = go(&act, &[f.clone()], None);
    assert_eq!(names(&d), ["doc (batch).txt", "doc.pdf"]);
    assert_eq!(r.files[0].outputs.len(), 1);
    assert!(r.files[0].size_after.unwrap() > 0);
}

/// A form with one text field and its appearance, so flattening has something to bake.
fn form_pdf() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R] /Resources << /Font << /Helv 3 0 R >> >> >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Berkay) /DA (/Helv 12 Tf 0 g) /Rect [72 700 272 720] /P 4 0 R /F 4 >>".to_string(),
    ];
    assemble(&objs, "")
}

#[test]
fn flatten_turns_form_fields_into_page_content() {
    let d = dir("flatten");
    let f = put(&d, "form.pdf", &form_pdf());
    let pdf = PdfDocument::open(f.to_string_lossy().as_ref()).unwrap();
    assert_eq!(crate::forms::list(&pdf).unwrap().fields.len(), 1);
    drop(pdf);
    let r = go(&action("F", vec![Step::FlattenForms(FlattenFormsStep {})]), &[f.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    let out = PathBuf::from(&r.files[0].outputs[0].path);
    let flat = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    assert_eq!(crate::forms::list(&flat).unwrap().fields.len(), 0);
    assert!(text_of(&out).contains("Berkay"), "the value is page content now: {:?}", text_of(&out));

    // No fields: nothing to do.
    let plain = sample(&d, "plain.pdf", 1);
    let r = go(&action("F", vec![Step::FlattenForms(FlattenFormsStep {})]), &[plain.clone()], None);
    assert_eq!(digest(Path::new(&r.files[0].outputs[0].path)), digest(&plain));
}

#[test]
fn the_built_in_actions_run_end_to_end() {
    let d = dir("builtins");
    let text = put(&d, "mail.pdf", &text_pdf(&[&["Contact: someone@example.org, tel +46 8 123 456 78"]]));
    let scan = put(&d, "scan.pdf", &blank(1));
    for id in ["builtin-prepare-for-email", "builtin-publish-with-watermark", "builtin-redact-emails-and-phones"] {
        let a = store::builtins().into_iter().find(|a| a.id == id).unwrap();
        let r = go(&a, &[text.clone()], None);
        assert_eq!(r.files[0].status, Status::Ok, "{id}: {:?}", r.files[0].reason);
        assert_eq!(r.files[0].outputs.len(), 1, "{id}");
    }
    let wm = names(&d).into_iter().find(|n| n.contains("(published)")).unwrap();
    assert!(text_of(&d.join(wm)).contains("CONFIDENTIAL"));
    let red = names(&d).into_iter().find(|n| n.contains("(redacted)")).unwrap();
    assert!(!text_of(&d.join(red)).contains("someone@example.org"));

    let a = store::builtins().into_iter().find(|a| a.id == "builtin-make-searchable").unwrap();
    let r = go(&a, &[scan], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    assert!(text_of(Path::new(&r.files[0].outputs[0].path)).contains("Invoice"));
}

#[test]
fn civil_dates_from_days() {
    use super::steps::civil_from_days;
    let d = |n| {
        let x = civil_from_days(n);
        (x.year, x.month, x.day)
    };
    assert_eq!(d(0), (1970, 1, 1));
    assert_eq!(d(59), (1970, 3, 1));
    assert_eq!(d(11_016), (2000, 2, 29));
    assert_eq!(d(20_372), (2025, 10, 11));
    assert_eq!(d(-1), (1969, 12, 31));
}




// ---------------------------------------------------------------- review round: restrictions, sanitize, languages, leftovers

/// `f` protected with an owner password only: it opens without a password, with these rights.
fn guarded(d: &Path, name: &str, changes: crate::protect::Changes, printing: crate::protect::Printing) -> PathBuf {
    let f = sample(d, name, 1);
    let lock = action(
        "lock",
        vec![Step::Protect(ProtectStep {
            permissions_password: Some("owner-pw-456".into()),
            printing,
            changes,
            ..ProtectStep::default()
        })],
    );
    let out = PathBuf::from(&go(&lock, &[f], None).files[0].outputs[0].path);
    let to = d.join(format!("{name}.locked.pdf"));
    std::fs::rename(&out, &to).unwrap();
    to
}

fn security(p: &Path) -> crate::protect::SecurityInfo {
    let doc = PdfDocument::open(p.to_string_lossy().as_ref()).unwrap();
    crate::protect::read_security(&doc, None).unwrap()
}

#[test]
fn protect_cannot_replace_an_owner_password_the_batch_does_not_have() {
    use crate::protect::{Changes, Printing};
    let d = dir("owner-guard");
    // Copying allowed, printing forbidden: an owner password guards that.
    let locked = guarded(&d, "a", Changes::AllButExtract, Printing::None);
    assert!(!security(&locked).unlocked);
    let before = digest(&locked);
    let again = action("again", vec![Step::Protect(ProtectStep { open_password: Some("pw-one-2".into()), printing: Printing::High, ..ProtectStep::default() })]);
    let r = go(&again, &[locked.clone()], None);
    assert_eq!(r.files[0].status, Status::Skipped);
    assert!(r.files[0].reason.as_deref().unwrap().contains("owner password"), "{:?}", r.files[0].reason);
    assert!(r.files[0].outputs.is_empty());
    assert_eq!(digest(&locked), before);
    // A file with no owner password is the batch's to protect.
    let plain = sample(&d, "plain.pdf", 1);
    assert_eq!(go(&again, &[plain], None).files[0].status, Status::Ok);
}

#[test]
fn a_source_that_forbids_changes_gets_no_stamps_but_may_be_read() {
    use crate::protect::{Changes, Printing};
    let d = dir("no-modify");
    // "Pages" lets pages be assembled, copying is on, but the content may not be edited.
    let locked = guarded(&d, "a", Changes::Pages, Printing::High);
    let s = security(&locked);
    assert!(!s.rights.modify && s.rights.assemble && s.rights.copy, "{:?}", s.rights);
    for step in [
        footer("<<1>>"),
        Step::Watermark(Watermark { text: "X".into(), ..Watermark::default() }),
        Step::FlattenForms(FlattenFormsStep {}),
        Step::SetProperties(PropertiesStep { title: Some("t".into()), ..Default::default() }),
        Step::Compress(CompressStep::default()),
        Step::Sanitize(Sanitize::default()),
    ] {
        let label = step.label();
        let r = go(&action("n", vec![step]), &[locked.clone()], None);
        assert_eq!(r.files[0].status, Status::Skipped, "{label}");
        assert!(r.files[0].reason.as_deref().unwrap().contains("does not allow changes"), "{label}: {:?}", r.files[0].reason);
    }
    assert_eq!(names(&d).len(), 2, "{:?}", names(&d));
    let r = go(&action("t", vec![Step::ExportText(ExportTextStep::default())]), &[locked], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
}

/// Two pages: the first has text with an email, a picture and a form field, and the author is an email
/// too; the second is a scan. Every writing step has something to do on it.
fn rich_pdf() -> Vec<u8> {
    let hex = "40A0F0".repeat(64 * 64);
    let content = "BT /F1 12 Tf 72 700 Td (Mail jane.tester@example.com now) Tj ET\nq 100 0 0 100 72 500 cm /Im1 Do Q";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [8 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 3 0 R >> >> >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R 9 0 R] /Count 2 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R /Annots [8 0 R] /Resources << /Font << /F1 3 0 R /Helv 3 0 R >> /XObject << /Im1 7 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Author (author@example.org) /Title (Rich) >>".to_string(),
        format!("<< /Type /XObject /Subtype /Image /Width 64 /Height 64 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length {} >>\nstream\n{hex}>\nendstream", hex.len() + 1),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Berkay) /DA (/Helv 12 Tf 0 g) /Rect [72 400 272 420] /P 4 0 R /F 4 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
    ];
    assemble(&objs, "/Info 6 0 R")
}

#[test]
fn a_result_keeps_the_restrictions_of_a_source_it_may_change() {
    use crate::protect::{Changes, Printing};
    let d = dir("keep-rights");
    let rich = put(&d, "rich.pdf", &rich_pdf());
    let lock = action(
        "lock",
        vec![Step::Protect(ProtectStep {
            permissions_password: Some("owner-pw-456".into()),
            printing: Printing::None,
            changes: Changes::AllButExtract,
            ..ProtectStep::default()
        })],
    );
    let locked = PathBuf::from(&go(&lock, &[rich.clone()], None).files[0].outputs[0].path);
    let src = security(&locked);
    assert!(src.encrypted && !src.unlocked && src.rights.printing == Printing::None);
    let before = digest(&locked);

    // (step, the note that proves it did its work and did not just copy the file; the stamps leave none, the changed bytes show theirs)
    let steps = vec![
        (footer("<<1>>"), ""),
        (Step::Watermark(Watermark { text: "X".into(), ..Watermark::default() }), ""),
        (Step::Bates(BatesStep::default()), "numbered"),
        (Step::SetProperties(PropertiesStep { title: Some("t".into()), ..Default::default() }), "properties set"),
        (Step::Sanitize(Sanitize { metadata: true, ..Sanitize::NONE }), "removed"),
        (Step::Compress(CompressStep::default()), "smaller"),
        (Step::Ocr(OcrStep::default()), "1 page(s) recognized"),
        (Step::FlattenForms(FlattenFormsStep {}), "1 field(s) flattened"),
        (
            Step::Redact(RedactStep { queries: vec![RedactQuery::Pattern { pattern: Pattern::Email }], sanitize: Sanitize::NONE, ..RedactStep::default() }),
            "redacted",
        ),
    ];
    for (step, proof) in steps {
        let label = step.label();
        let r = go(&action("k", vec![step]), &[locked.clone()], None);
        let f = &r.files[0];
        assert_eq!(f.status, Status::Ok, "{label}: {:?}", f.reason);
        assert!(proof.is_empty() || f.notes.iter().any(|n| n.contains(proof)), "{label} did not do its work: {:?}", f.notes);
        let out = Path::new(&f.outputs[0].path);
        assert_ne!(digest(out), before, "{label}");
        // Still locked, still with the original owner password and rights.
        let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
        let s = crate::protect::read_security(&doc, None).unwrap();
        assert!(s.encrypted && !s.unlocked && s.rights.printing == Printing::None && s.rights == src.rights, "{label}: {s:?}");
        let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
        assert!(crate::protect::read_security(&doc, Some("owner-pw-456")).unwrap().unlocked, "{label}: the owner password still works");
        let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
        assert!(!crate::protect::read_security(&doc, Some("not-the-password")).unwrap().unlocked, "{label}");
    }
    assert_eq!(digest(&locked), before);
}

#[test]
fn redact_with_nothing_found_still_sanitizes_and_warns_about_blind_pages() {
    let d = dir("redact-nohits");
    // The address is only in the properties, not on a page.
    let f = put(&d, "t.pdf", &text_pdf(&[&["Nothing to find here."]]));
    let tagged = PathBuf::from(
        &go(&action("m", vec![Step::SetProperties(PropertiesStep { author: Some("a@example.com".into()), ..Default::default() })]), &[f], None).files[0]
            .outputs[0]
            .path,
    );
    let act = action(
        "R",
        vec![Step::Redact(RedactStep {
            queries: vec![RedactQuery::Pattern { pattern: Pattern::Email }],
            sanitize: Sanitize { metadata: true, ..Sanitize::NONE },
            ..RedactStep::default()
        })],
    );
    let r = go(&act, &[tagged.clone()], None);
    assert_eq!(r.files[0].status, Status::Ok);
    let out = PathBuf::from(&r.files[0].outputs[0].path);
    assert_ne!(digest(&out), digest(&tagged));
    assert!(!String::from_utf8_lossy(&digest(&out)).contains("a@example.com"));
    assert!(r.files[0].notes[0].contains("nothing found on the pages"), "{:?}", r.files[0].notes);
    assert!(r.files[0].warnings.is_empty());

    // A scan has no text to search: the file is processed, and the report says so.
    let scan = put(&d, "scan.pdf", &blank(3));
    let r = go(&act, &[scan], None);
    assert_eq!(r.files[0].status, Status::Ok);
    assert_eq!(r.files[0].warnings.len(), 1, "{:?}", r.files[0].warnings);
    assert!(r.files[0].warnings[0].contains("3 page(s) have no text") && r.files[0].warnings[0].contains("Make searchable"));
}

#[test]
fn ocr_languages_are_checked_one_by_one_and_by_shape() {
    let ocr = |langs: &[&str]| action("o", vec![Step::Ocr(OcrStep { languages: langs.iter().map(|s| s.to_string()).collect(), ..OcrStep::default() })]);
    assert!(ocr(&["en-US"]).validate(&env()).is_empty());
    assert!(ocr(&["en"]).validate(&env()).is_empty(), "the primary language is what counts");
    assert!(ocr(&[]).validate(&env()).is_empty());
    // One installed tag does not excuse another that is not.
    let got = ocr(&["en-US", "sv-SE"]).validate(&env());
    assert_eq!((got[0].step, got[0].code), (Some(0), "unavailable"), "{got:?}");
    assert!(got[0].message.contains("sv-SE"));
    for bad in ["../../x", "en/US", "e", "en-", "-US", "en US", "en-US\\x", "waytoolongtag", "en-us-\u{e5}", ""] {
        let got = ocr(&["en-US", bad]).validate(&env());
        assert_eq!((got[0].step, got[0].code), (Some(0), "invalid-step"), "{bad:?}: {got:?}");
    }
    assert_eq!(ocr(&["en"; 9]).validate(&env())[0].code, "invalid-step");
    assert!(super::steps::language_tag_ok("sr-Latn-RS") && super::steps::language_tag_ok("eng") && super::steps::language_tag_ok("zh_CN"));
}

#[test]
fn signatures_are_found_by_flag_certification_and_signed_field() {
    let d = dir("signed-kinds");
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R] /Resources << >> >>";
    let pdf = |catalog_extra: &str, widget: &str| {
        assemble(
            &[
                format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
                "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
                page.to_string(),
                "<< >>".to_string(),
                widget.to_string(),
            ],
            "",
        )
    };
    let sig = |v: &str| format!("<< /Type /Annot /Subtype /Widget /FT /Sig /T (S) /Rect [0 0 50 20] /P 3 0 R {v} >>");
    let value = "/V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /Contents <00> /ByteRange [0 0 0 0] >>";
    let cases = [
        ("flag", pdf("/AcroForm << /Fields [5 0 R] /SigFlags 1 >>", &sig("")), true),
        ("valued", pdf("/AcroForm << /Fields [5 0 R] >>", &sig(value)), true),
        ("certified", pdf("/AcroForm << /Fields [5 0 R] >> /Perms << /DocMDP 5 0 R >>", &sig("")), true),
        ("empty-field", pdf("/AcroForm << /Fields [5 0 R] >>", &sig("")), false),
    ];
    let act = action("n", vec![footer("<<1>>")]);
    for (name, bytes, signed) in cases {
        let f = put(&d, &format!("{name}.pdf"), &bytes);
        let r = go(&act, &[f], None);
        let want = if signed { Status::Skipped } else { Status::Ok };
        assert_eq!(r.files[0].status, want, "{name}: {:?}", r.files[0].reason);
        if signed {
            assert!(r.files[0].reason.as_deref().unwrap().starts_with("signed:"), "{name}");
        }
    }
}

#[test]
fn a_file_whose_form_cannot_be_scanned_is_left_alone_as_possibly_signed() {
    // A signature field with a value, no SigFlags, no DocMDP, on 2000 pages that each repeat its
    // widget 1001 times: past the form scan budget, so the field list errors.
    let d = dir("signed-unscannable");
    let refs = "3 0 R ".repeat(1001);
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [3 0 R] >> >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count 2000 >>", (0..2000).map(|i| format!("{} 0 R ", 4 + i)).collect::<String>()),
        "<< /Type /Annot /Subtype /Widget /FT /Sig /T (S) /Rect [0 0 50 20] /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /Contents <00> /ByteRange [0 0 0 0] >> >>".to_string(),
    ];
    objs.extend((0..2000).map(|_| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{refs}] /Resources << >> >>")));
    let f = put(&d, "big.pdf", &assemble(&objs, ""));
    let r = go(&action("n", vec![footer("<<1>>")]), &[f], None);
    assert_eq!(r.files[0].status, Status::Skipped, "{:?}", r.files[0].reason);
    assert!(r.files[0].reason.as_deref().unwrap().starts_with("could not check whether the file is signed:"), "{:?}", r.files[0].reason);
}

#[test]
fn a_file_whose_form_is_too_large_to_check_for_a_certification_is_left_alone_as_possibly_signed() {
    // No SigFlags and no signature at all, but 20 001 entries in /Fields: past what the certification walk reads, so
    // the answer is "could not check", not "not signed".
    let d = dir("certification-unchecked");
    let objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{}] >> >>", "3 0 R ".repeat(20_001)),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /FT /Tx /T (t) >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> >>".to_string(),
    ];
    let f = put(&d, "wide.pdf", &assemble(&objs, ""));
    let r = go(&action("n", vec![footer("<<1>>")]), &[f], None);
    assert_eq!(r.files[0].status, Status::Skipped, "{:?}", r.files[0].reason);
    assert!(r.files[0].reason.as_deref().unwrap().starts_with("could not check whether the file is signed:"), "{:?}", r.files[0].reason);
}

#[test]
fn an_imported_name_stays_within_the_limit() {
    let d = dir("long-name");
    let s = Store::new(d.join("actions"));
    let long = "n".repeat(MAX_NAME);
    let a = s.save(action(&long, vec![footer("<<1>>")])).unwrap();
    let imported = s.import(&store::to_file(&a).unwrap()).unwrap();
    assert!(imported.name.chars().count() <= 80, "{}", imported.name.chars().count());
    assert!(imported.name.ends_with(" (imported)"));
    assert_ne!(imported.name, a.name);
    assert!(imported.validate(&env()).iter().all(|i| i.code != "name"));
}

#[test]
fn files_are_written_through_a_temp_file_nobody_else_has() {
    let d = dir("atomic");
    let target = d.join("out.json");
    // A file that happens to have the old fixed temp name is not touched.
    let bystander = put(&d, "out.json.tmp", b"mine");
    store::write_file_atomic(&target, b"{}").unwrap();
    store::write_file_atomic(&target, b"{\"a\":1}").unwrap();
    assert_eq!(digest(&target), b"{\"a\":1}");
    assert_eq!(digest(&bystander), b"mine");
    assert_eq!(names(&d), ["out.json", "out.json.tmp"]);
}

#[test]
fn one_job_id_cannot_run_twice() {
    let jobs = Arc::new(crate::tools::Jobs::default());
    let first = jobs.try_begin(7).expect("free id");
    assert!(jobs.try_begin(7).is_none(), "a running id is refused");
    assert!(jobs.try_begin(8).is_some());
    jobs.cancel(7);
    assert!(first.cancel.load(Ordering::SeqCst));
    drop(first);
    let again = jobs.try_begin(7).expect("free again once the job ended");
    assert!(!again.cancel.load(Ordering::SeqCst), "a new job does not inherit the old cancel");
}

#[test]
fn a_sweep_removes_what_a_dead_run_left_and_nothing_else() {
    let d = dir("sweep");
    let journal = d.join("state").join("run-dirs.txt");
    let work = d.join("work");
    std::fs::create_dir(&work).unwrap();
    let f = sample(&work, "a.pdf", 1);

    // A normal run leaves nothing, and writes its folder and ending into the journal.
    let mut e = env();
    e.journal = Some(journal.clone());
    run(&action("n", vec![footer("<<1>>")]), &[f.clone()], None, &e, &AtomicBool::new(false), &mut |_| {}).unwrap();
    let j = std::fs::read_to_string(&journal).unwrap();
    assert_eq!(j, format!("{}\t (batch)\n", work.display()));
    assert_eq!(names(&work), ["a (batch).pdf", "a.pdf"]);
    assert!(!journal.with_extension("lock").exists(), "the lock is released");

    // What a crash leaves: a staging folder with a claims list, an empty claimed file, a finished result.
    let stale = work.join(".inkwell-convert-1-0-deadbeef");
    std::fs::create_dir(&stale).unwrap();
    let empty = put(&work, "a (batch) (2).pdf", b"");
    let whole = work.join("a (batch).pdf");
    let someone = put(&work, "keep.pdf", b"x");
    let claims = format!("+{}\n+{}\n+{}\n", empty.display(), whole.display(), someone.display());
    std::fs::write(stale.join("claims.txt"), claims).unwrap();
    std::fs::write(stale.join("step-0.pdf"), b"partial").unwrap();
    let fresh = work.join(".inkwell-convert-1-1-cafe");
    std::fs::create_dir(&fresh).unwrap();
    let not_staging = work.join(".inkwell-convert-notes");
    std::fs::create_dir(&not_staging).unwrap();

    // Young folders are a live run's: nothing happens to them.
    assert_eq!(super::run::sweep_for_test(&journal, std::time::Duration::from_secs(3600)), 0);
    assert!(stale.exists() && empty.exists());
    // Once old enough, the folder goes with the empty claim; whole files and other folders stay.
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(super::run::sweep_for_test(&journal, std::time::Duration::from_millis(5)), 2);
    assert!(!stale.exists() && !fresh.exists() && !empty.exists());
    assert!(whole.exists() && someone.exists() && f.exists() && not_staging.exists());
    // A folder that is gone leaves the journal.
    std::fs::remove_dir_all(&work).unwrap();
    super::run::sweep_for_test(&journal, std::time::Duration::from_millis(5));
    assert_eq!(std::fs::read_to_string(&journal).unwrap().trim(), "");
}

#[test]
fn a_planted_claims_list_cannot_make_the_sweep_delete_other_files() {
    let d = dir("sweep-planted");
    let journal = d.join("run-dirs.txt");
    let work = d.join("work");
    let elsewhere = d.join("elsewhere");
    std::fs::create_dir_all(work.join("sub")).unwrap();
    std::fs::create_dir(&elsewhere).unwrap();
    sample(&work, "a.pdf", 1);
    std::fs::write(&journal, format!("{}\t (batch)\n", work.display())).unwrap();

    let outside = put(&elsewhere, "a (batch).pdf", b"");
    let unrelated = put(&work, "notes.txt", b"");
    let wrong_stem = put(&work, "other (batch).pdf", b"");
    let wrong_ending = put(&work, "a (final).pdf", b"");
    let wrong_shape = put(&work, "a (batch) copy.pdf", b"");
    let in_sub = put(&work.join("sub"), "a (batch).pdf", b"");
    let empty_dir = work.join("a (batch) (images)");
    std::fs::create_dir(&empty_dir).unwrap();
    let published_empty = put(&work, "a (batch).txt", b"");
    let genuine = put(&work, "a (batch) (2).pdf", b"");

    let stale = work.join(".inkwell-convert-9-9-abc123");
    std::fs::create_dir(&stale).unwrap();
    let mut claims = String::new();
    for p in [&outside, &unrelated, &wrong_stem, &wrong_ending, &wrong_shape, &in_sub, &empty_dir, &genuine] {
        claims.push_str(&format!("+{}\n", p.display()));
    }
    claims.push_str("+a (batch) (3).pdf\n+../elsewhere/a (batch).pdf\n");
    claims.push_str(&format!("+{}\n={}\n", published_empty.display(), published_empty.display()));
    std::fs::write(stale.join("claims.txt"), claims).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(super::run::sweep_for_test(&journal, std::time::Duration::from_millis(5)), 1);
    assert!(!stale.exists());
    for p in [&outside, &unrelated, &wrong_stem, &wrong_ending, &wrong_shape, &in_sub, &published_empty] {
        assert!(p.exists(), "{} must stay", p.display());
    }
    // An empty folder is removed only when it is a claim that fits; this one is, and it is empty.
    assert!(!empty_dir.exists());
    // The one claim that fits goes.
    assert!(!genuine.exists());
}

#[test]
fn journal_lines_are_added_without_losing_a_concurrent_writer() {
    let d = dir("journal");
    let journal = d.join("run-dirs.txt");
    let f = sample(&d, "a.pdf", 1);
    let mut e = env();
    e.journal = Some(journal.clone());
    let e = Arc::new(e);
    let act = Arc::new(action("n", vec![footer("<<1>>")]));
    let threads: Vec<_> = (0..4)
        .map(|k| {
            let (e, act, f) = (e.clone(), act.clone(), f.clone());
            let sub = d.join(format!("t{k}"));
            std::fs::create_dir(&sub).unwrap();
            let copy = sub.join("a.pdf");
            std::fs::copy(&f, &copy).unwrap();
            std::thread::spawn(move || {
                run(&act, &[copy], None, &e, &AtomicBool::new(false), &mut |_| {}).unwrap();
            })
        })
        .collect();
    let sweeper = {
        let journal = journal.clone();
        std::thread::spawn(move || {
            for _ in 0..20 {
                super::run::sweep_for_test(&journal, std::time::Duration::from_secs(3600));
            }
        })
    };
    threads.into_iter().for_each(|t| t.join().unwrap());
    sweeper.join().unwrap();
    let lines = std::fs::read_to_string(&journal).unwrap();
    assert_eq!(lines.lines().count(), 4, "{lines}");
}

#[test]
fn a_file_that_stops_reports_how_many_steps_finished() {
    let d = dir("steps-done");
    let f = sample(&d, "a.pdf", 1);
    let cancel = AtomicBool::new(false);
    let act = action("n", vec![footer("<<1>>"), Step::SetProperties(PropertiesStep { title: Some("t".into()), ..Default::default() }), Step::Compress(CompressStep::default())]);
    let r = run(&act, &[f.clone()], None, &env(), &cancel, &mut |e| {
        if let Event::Step { step: 2, .. } = e {
            cancel.store(true, Ordering::SeqCst);
        }
    })
    .unwrap();
    // The cancel lands during the third step, after two had finished.
    assert_eq!(r.files[0].status, Status::Skipped);
    assert_eq!(r.files[0].steps_done, 2, "{:?}", r.files[0]);
}

#[test]
fn an_image_folder_is_published_as_one_rename() {
    let d = dir("folder-atomic");
    let f = sample(&d, "doc.pdf", 3);
    // A non-empty folder of the same name is never entered; an empty one is not reused either.
    let busy = d.join("doc (batch) (images)");
    std::fs::create_dir(&busy).unwrap();
    std::fs::write(busy.join("mine.txt"), b"x").unwrap();
    let act = action("i", vec![Step::ExportImages(ExportImagesStep { dpi: 36, ..Default::default() })]);
    let r = go(&act, &[f], None);
    assert_eq!(r.files[0].status, Status::Ok, "{:?}", r.files[0].reason);
    assert_eq!(names(&busy), ["mine.txt"]);
    assert_eq!(names(&d.join("doc (batch) (images) (2)")), ["doc-1.png", "doc-2.png", "doc-3.png"]);
    assert_eq!(names(&d), ["doc (batch) (images)", "doc (batch) (images) (2)", "doc.pdf"]);
}
