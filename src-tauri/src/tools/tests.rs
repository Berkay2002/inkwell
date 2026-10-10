//! Tests for the UI-facing compress and OCR operations. OCR uses a stub recogniser (accuracy is the
//! engine's tests' business); everything runs on scratch copies, never on the study samples in place.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::engine::edit::EditOp;
use crate::ocr::backend::{Bitmap, RawLine, RawPage, RawWord};

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tools-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn files(d: &Path) -> Vec<String> {
    let mut v: Vec<String> =
        std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// `n` pages without any content: no text, so OCR treats them as scans.
fn blank_pdf(n: usize) -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offs = Vec::new();
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 3 + i)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>"),
    ];
    for _ in 0..n {
        objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string());
    }
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

struct Stub {
    /// Runs once, inside the first recognition (an edit that lands mid-run).
    during: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    delay_ms: u64,
}

impl Stub {
    fn new() -> Self {
        Stub { during: Mutex::new(None), delay_ms: 0 }
    }
}

impl Recognizer for Stub {
    fn name(&self) -> &'static str {
        "stub"
    }
    fn languages(&self) -> Result<Vec<String>, OcrError> {
        Ok(vec!["en-US".into()])
    }
    fn max_dimension(&self) -> u32 {
        10000
    }
    fn recognize(&self, _bmp: &Bitmap, _l: &[String], _c: &AtomicBool) -> Result<RawPage, OcrError> {
        if let Some(f) = self.during.lock().unwrap().take() {
            f();
        }
        std::thread::sleep(std::time::Duration::from_millis(self.delay_ms));
        let words = [("Invoice", [72.0, 100.0, 120.0, 114.0]), ("Total", [126.0, 100.0, 166.0, 114.0])];
        Ok(RawPage {
            lines: vec![RawLine { words: words.iter().map(|(t, r)| RawWord { text: t.to_string(), rect: *r }).collect() }],
            angle_deg: None,
        })
    }
}

fn open(d: &Path, name: &str, pages: usize) -> (Arc<Engine>, DocId, String) {
    let p = d.join(name);
    std::fs::write(&p, blank_pdf(pages)).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

fn req(doc: DocId) -> OcrRequest {
    OcrRequest {
        doc,
        pages: None,
        languages: vec![],
        force: false,
        backend: BackendChoice::Windows,
        save_as: false,
    }
}

fn text_of(eng: &Engine, id: DocId, page: u32) -> String {
    eng.page_text(id, page).unwrap().iter().map(|l| l.t.as_str()).collect::<Vec<_>>().join(" ")
}

#[test]
fn ocr_in_place_is_one_undo_step_and_survives_save_and_reopen() {
    let d = dir("inplace");
    let (eng, id, path) = open(&d, "scan.pdf", 3);
    assert_eq!(text_of(&eng, id, 0), "");

    let out = run_ocr(&eng, &req(id), None, &Stub::new(), &|_| {}, &AtomicBool::new(false)).unwrap();
    let st = out.state.unwrap();
    assert_eq!(out.report.pages_recognized, 3);
    assert_eq!(st.undo.as_deref(), Some("Recognize text (3 pages)"));
    assert!(st.dirty);
    assert!(text_of(&eng, id, 2).contains("Invoice"));

    // One step: a single undo removes the text of every page.
    let st = eng.undo(id).unwrap();
    assert!(!st.dirty);
    assert!((0..3).all(|p| text_of(&eng, id, p).is_empty()));
    eng.redo(id).unwrap();
    assert!(text_of(&eng, id, 0).contains("Invoice"));

    eng.save(id, None).unwrap();
    drop(eng);
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert!((0..3).all(|p| text_of(&eng2, id2, p).contains("Invoice")));
}

#[test]
fn ocr_progress_only_grows_and_ends_at_the_total() {
    let d = dir("progress");
    let (eng, id, _) = open(&d, "scan.pdf", 12);
    let seen = Mutex::new(Vec::new());
    let stub = Stub { delay_ms: 5, ..Stub::new() };
    run_ocr(&eng, &req(id), None, &stub, &|p| seen.lock().unwrap().push(p.done), &AtomicBool::new(false)).unwrap();
    let seen = seen.into_inner().unwrap();
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    assert_eq!(seen.last(), Some(&12));
}

#[test]
fn cancelled_ocr_changes_nothing_and_writes_nothing() {
    let d = dir("cancel");
    let (eng, id, _) = open(&d, "scan.pdf", 40);
    let cancel = AtomicBool::new(false);
    let stub = Stub { delay_ms: 30, ..Stub::new() };

    // In place: cancelled after the first page.
    let r = run_ocr(&eng, &req(id), None, &stub, &|_| cancel.store(true, Ordering::Relaxed), &cancel);
    assert_eq!(r.unwrap_err().0, "cancelled");
    let st = eng.edit_state(id).unwrap();
    assert!(!st.dirty && st.undo.is_none());

    // Save as: no output, no scratch files left beside it.
    let out_dir = dir("cancel-out");
    let out = out_dir.join("scan (ocr).pdf");
    cancel.store(false, Ordering::Relaxed);
    let r = run_ocr(
        &eng,
        &OcrRequest { save_as: true, ..req(id) },
        Some(&out),
        &stub,
        &|_| cancel.store(true, Ordering::Relaxed),
        &cancel,
    );
    assert_eq!(r.unwrap_err().0, "cancelled");
    assert!(files(&out_dir).is_empty(), "{:?}", files(&out_dir));
}

#[test]
fn an_edit_that_lands_during_recognition_makes_the_run_stale() {
    let d = dir("stale");
    let (eng, id, _) = open(&d, "scan.pdf", 3);
    let stub = Stub::new();
    let e2 = eng.clone();
    *stub.during.lock().unwrap() = Some(Box::new(move || {
        e2.apply(id, EditOp::Delete { pages: vec![0] }).unwrap();
    }));
    let r = run_ocr(&eng, &req(id), None, &stub, &|_| {}, &AtomicBool::new(false));
    assert_eq!(r.unwrap_err().0, "stale");
    // Only the user's delete is in the history.
    assert_eq!(eng.edit_state(id).unwrap().undo.as_deref(), Some("Delete p. 1"));
}

#[test]
fn save_as_new_file_leaves_the_document_alone_and_never_overwrites() {
    let d = dir("saveas");
    let (eng, id, path) = open(&d, "scan.pdf", 2);
    // A pending edit is part of the new file.
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    let out_path = d.join("scan (ocr).pdf");
    let out = out_path.to_string_lossy().into_owned();
    let r = OcrRequest { save_as: true, ..req(id) };

    let done = run_ocr(&eng, &r, Some(&out_path), &Stub::new(), &|_| {}, &AtomicBool::new(false)).unwrap();
    assert!(done.state.is_none());
    assert_eq!(done.output.as_deref(), Some(out.as_str()));
    assert_eq!(done.report.pages_recognized, 1);
    let probe = Engine::new(1, 16 << 20);
    let pid = probe.open(&out).unwrap();
    assert_eq!(pid.page_count, 1);
    assert!(text_of(&probe, pid.doc, 0).contains("Invoice"));
    // The open document still has no text and the original file is untouched.
    assert!(text_of(&eng, id, 0).is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), blank_pdf(2));

    // A name confirmed in the dialog may replace an existing file (here a stale one; Windows needs
    // it closed first)...
    probe.close(pid.doc);
    std::fs::write(&out_path, b"stale").unwrap();
    run_ocr(&eng, &r, Some(&out_path), &Stub::new(), &|_| {}, &AtomicBool::new(false)).unwrap();
    assert!(text_of(&probe, probe.open(&out).unwrap().doc, 0).contains("Invoice"));
    // ...but never the document's own file, even though the source is an in-memory snapshot.
    let own = run_ocr(&eng, &r, Some(Path::new(&path)), &Stub::new(), &|_| {}, &AtomicBool::new(false));
    assert!(own.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), blank_pdf(2));
    assert!(!files(&d).iter().any(|f| f.contains("inkwell-ocr")), "{:?}", files(&d));
}

#[test]
fn a_page_range_limits_the_pages_and_text_pages_are_skipped() {
    let d = dir("range");
    let (eng, id, _) = open(&d, "scan.pdf", 4);
    let r = OcrRequest { pages: Some("2-3".into()), ..req(id) };
    let out = run_ocr(&eng, &r, None, &Stub::new(), &|_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.report.pages_recognized, 2);
    assert_eq!(out.state.unwrap().undo.as_deref(), Some("Recognize text (2 pages)"));
    assert!(text_of(&eng, id, 0).is_empty() && text_of(&eng, id, 1).contains("Invoice"));

    // Same pages again: they have text now, so nothing is written and no undo step is added.
    let out = run_ocr(&eng, &r, None, &Stub::new(), &|_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(out.report.pages_skipped_text, 2);
    assert_eq!(out.state.unwrap().undo.as_deref(), Some("Recognize text (2 pages)"));
    assert_eq!(eng.history(id).unwrap().len(), 2);

    let bad = OcrRequest { pages: Some("9".into()), ..req(id) };
    assert!(run_ocr(&eng, &bad, None, &Stub::new(), &|_| {}, &AtomicBool::new(false)).is_err());
}

#[test]
fn tesseract_is_only_found_where_discover_looks() {
    let d = dir("tess");
    let missing = d.join("nope").join("tesseract.exe");
    // Not on PATH, not in the working directory: a missing configured path is an error code.
    if TesseractCli::discover(Some(&missing)).is_none() {
        let e = make_backend(BackendChoice::Tesseract, Some(&missing)).err().unwrap();
        assert_eq!(e.0, "tesseract-not-found");
    }
    // The path is run as a program: only something named tesseract is accepted.
    assert!(configured_tesseract(Some(Path::new("C:\\Windows\\System32\\cmd.exe"))).is_none());
    assert!(configured_tesseract(Some(Path::new("C:\\T\\Tesseract.EXE"))).is_some());
}

// ---------------------------------------------------------------- compress

fn sample_doc(d: &Path) -> (Arc<Engine>, DocId, String) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/study/samples/opt-in-01.pdf");
    let to = d.join("opt-in-01.pdf");
    std::fs::copy(src, &to).unwrap();
    let path = to.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

fn creq(doc: DocId) -> CompressRequest {
    CompressRequest { doc, preset: Preset::Smallest, custom: None, choose_output: false }
}

#[test]
fn compress_reports_monotonic_progress_writes_next_to_the_input_and_refuses_to_overwrite() {
    let d = dir("compress");
    let (eng, id, _) = sample_doc(&d);
    let mut fractions = Vec::new();
    let rep = run_compress(&eng, &creq(id), None, &AtomicBool::new(false), &mut |p| fractions.push(p.fraction)).unwrap();
    assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{fractions:?}");
    assert_eq!(fractions.last().copied(), Some(1.0));
    assert!(rep.size_after < rep.size_before && !rep.kept_original);
    assert_eq!(rep.output, d.join("opt-in-01 (compressed).pdf"));
    assert_eq!(files(&d), ["opt-in-01 (compressed).pdf", "opt-in-01.pdf"]);

    let before = std::fs::read(&rep.output).unwrap();
    let again = run_compress(&eng, &creq(id), None, &AtomicBool::new(false), &mut |_| {});
    assert_eq!(again.unwrap_err().0, "output-exists");
    assert_eq!(std::fs::read(&rep.output).unwrap(), before);
    assert_eq!(files(&d).len(), 2);
    // The audit now knows the default name is taken.
    assert!(audit_for_ui(&eng, id).unwrap().output_exists);
}

#[test]
fn a_chosen_output_is_used_and_cancelled_compress_leaves_no_file() {
    let d = dir("compress-cancel");
    let (eng, id, _) = sample_doc(&d);
    let cancel = AtomicBool::new(false);
    let r = run_compress(&eng, &creq(id), None, &cancel, &mut |_| cancel.store(true, Ordering::Relaxed));
    assert_eq!(r.unwrap_err().0, "cancelled");
    assert_eq!(files(&d), ["opt-in-01.pdf"]);

    let mut custom = Options::preset(Preset::Balanced);
    custom.images.color.to_ppi = 5;
    let bad = CompressRequest { custom: Some(custom), ..creq(id) };
    let r = run_compress(&eng, &bad, None, &AtomicBool::new(false), &mut |_| {});
    assert_eq!(r.unwrap_err().0, "invalid-options");
    assert_eq!(files(&d), ["opt-in-01.pdf"]);

    let chosen = d.join("small.pdf");
    let rep = run_compress(&eng, &creq(id), Some(chosen.clone()), &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(rep.output, chosen);

    // A name confirmed in the dialog may replace an existing file, never the input.
    let again = run_compress(&eng, &creq(id), Some(chosen.clone()), &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(again.output, chosen);
    assert_eq!(files(&d), ["opt-in-01.pdf", "small.pdf"]);
    let input = d.join("opt-in-01.pdf");
    let before = std::fs::read(&input).unwrap();
    let r = run_compress(&eng, &creq(id), Some(input.clone()), &AtomicBool::new(false), &mut |_| {});
    assert!(r.unwrap_err().0.contains("overwrite the input"));
    assert_eq!(std::fs::read(&input).unwrap(), before);
}

#[test]
fn an_output_in_the_webview_json_is_ignored() {
    let o: Options = serde_json::from_str(r#"{"output":"C:\\x.pdf","replaceOutput":true,"password":"p"}"#).unwrap();
    assert!(o.output.is_none() && !o.replace_output);
    let r: CompressRequest = serde_json::from_str(r#"{"doc":1,"custom":{"output":"C:\\x.pdf","replaceOutput":true}}"#).unwrap();
    let c = r.custom.unwrap();
    assert!(c.output.is_none() && !c.replace_output);
}

#[test]
fn the_audit_predicts_smaller_files_for_smaller_presets() {
    let d = dir("audit");
    let (eng, id, _) = sample_doc(&d);
    let a = audit_for_ui(&eng, id).unwrap();
    assert!(!a.output_exists);
    let p = |x: Preset| a.predictions.iter().find(|p| p.preset == x).unwrap().bytes;
    assert!(p(Preset::Smallest) < p(Preset::Balanced));
    assert!(p(Preset::Balanced) < p(Preset::HighQuality));
    assert!(p(Preset::HighQuality) < a.size);
}

/// The webview may only name documents (by id), never files: requests have no path fields and
/// refuse unknown ones, and no command of this module takes a string or a path.
#[test]
fn the_commands_take_no_raw_path_from_the_webview() {
    let bad = r#"{"doc":1,"path":"C:\x.pdf"}"#;
    assert!(serde_json::from_str::<CompressRequest>(bad).is_err());
    assert!(serde_json::from_str::<CompressRequest>(r#"{"doc":1,"output":"C:\x.pdf"}"#).is_err());
    assert!(serde_json::from_str::<OcrRequest>(r#"{"doc":1,"saveAs":"C:\x.pdf"}"#).is_err());
    assert!(serde_json::from_str::<OcrRequest>(r#"{"doc":1,"tesseractPath":"C:\t.exe"}"#).is_err());
    assert!(serde_json::from_str::<OcrRequest>(r#"{"doc":1,"saveAs":true}"#).is_ok());

    let lib = include_str!("../lib.rs");
    let mut seen = 0;
    for name in ["compress_audit", "compress_start", "tool_cancel", "ocr_languages", "ocr_choose_tesseract", "ocr_clear_tesseract", "ocr_start"] {
        let at = lib.find(&format!("fn {name}(")).unwrap_or_else(|| panic!("no command {name}"));
        let rest = &lib[at..];
        let end = [rest.find(") ->"), rest.find(") {")].into_iter().flatten().min().unwrap();
        let sig = &rest[..end];
        for bad in [": String", "PathBuf", ": &str", "Vec<String>", "Option<String>"] {
            assert!(!sig.contains(bad), "{name} takes {bad}: {sig}");
        }
        seen += 1;
    }
    assert_eq!(seen, 7);
}

#[test]
fn a_cancel_before_the_job_registers_is_not_lost() {
    let jobs = Arc::new(Jobs::default());
    jobs.cancel(7);
    let g = jobs.begin(7);
    assert!(g.cancel.load(Ordering::Relaxed));
    drop(g);
    assert!(!jobs.begin(7).cancel.load(Ordering::Relaxed));
}
