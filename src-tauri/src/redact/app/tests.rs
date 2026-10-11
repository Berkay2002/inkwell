//! The app wiring: marks as undo steps, Apply through edit-core, and the save of the verified bytes.

use super::*;
use crate::engine::fixture::{self, Fixture};
use std::path::PathBuf;
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join("redact-app").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fixture copy with the word "needle" on some pages, opened.
fn open(test: &str) -> (PathBuf, String, Arc<Engine>, DocId) {
    let d = dir(test);
    let path = d.join("a.pdf");
    std::fs::write(&path, fixture::build(&Fixture { pages: 3, lines: 4, needle_every: 2, ..Default::default() })).unwrap();
    let path = path.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (d, path, eng, id)
}

fn needle() -> FindQuery {
    FindQuery::Text { needle: "needle".into(), case_sensitive: false, whole_word: true }
}

fn last_label(eng: &Engine, id: DocId) -> String {
    eng.history(id).unwrap().last().unwrap().label.clone()
}

#[test]
fn marks_are_undo_steps() {
    let (_d, _p, eng, id) = open("marks");
    let hits = find(&eng, id, &needle()).unwrap();
    assert!(hits.len() >= 2, "{hits:?}");
    let s = mark_hits(&eng, id, &hits, EditSource::User).unwrap();
    assert_eq!(s.rev, 1);
    assert_eq!(last_label(&eng, id), format!("Mark {} items for redaction", hits.len()));
    mark_area(&eng, id, 1, [300.0, 300.0, 100.0, 100.0], EditSource::User).unwrap();
    mark_pages(&eng, id, &[2], EditSource::Assistant { turn: "t4".into() }).unwrap();
    let marks = list_marks(&eng, id).unwrap();
    assert_eq!(marks.len(), hits.len() + 2);
    assert_eq!(marks.iter().filter(|m| m.whole_page).count(), 1);

    let m = &marks[0];
    unmark(&eng, id, &[(m.page, m.xref)], EditSource::User).unwrap();
    assert_eq!(list_marks(&eng, id).unwrap().len(), marks.len() - 1);
    assert!(unmark(&eng, id, &[(m.page, m.xref)], EditSource::User).is_err(), "an unknown mark is an error");
    eng.undo(id).unwrap();
    assert_eq!(list_marks(&eng, id).unwrap().len(), marks.len());
    // Marking is not redacting: the text is all still there.
    assert_eq!(find(&eng, id, &needle()).unwrap().len(), hits.len());
}

#[test]
fn apply_saves_the_verified_bytes_beside_the_original() {
    let (d, path, eng, id) = open("apply");
    let before = std::fs::read(&path).unwrap();
    let hits = find(&eng, id, &needle()).unwrap();
    mark_hits(&eng, id, &hits, EditSource::User).unwrap();
    let out = redacted_path(&path);
    assert_eq!(PathBuf::from(&out), d.join("a_Redacted.pdf"));

    let r = apply(&eng, id, Sanitize::default(), Some(out.clone()), EditSource::User).unwrap();
    let ApplyOutcome::Applied { state, marks, verify, saved_to, .. } = r else { panic!("refused: {r:?}") };
    assert_eq!(marks, hits.len());
    assert!(verify.is_clean());
    assert_eq!(saved_to.as_deref(), Some(out.as_str()));
    assert!(!state.dirty);
    assert_eq!(state.path, out, "the document now lives in the redacted copy");
    assert_eq!(last_label(&eng, id), format!("Redact {} items", hits.len()));
    assert_eq!(std::fs::read(&path).unwrap(), before, "the original is untouched");

    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&out).unwrap().doc;
    assert!(find(&eng2, id2, &needle()).unwrap().is_empty());
    assert!(list_marks(&eng2, id2).unwrap().is_empty());
    assert!(apply(&eng, id, Sanitize::default(), None, EditSource::User).is_err(), "nothing left to apply");
}

#[test]
fn save_after_redaction_writes_exactly_the_verified_bytes() {
    let (d, _p, eng, id) = open("exact");
    let hits = find(&eng, id, &needle()).unwrap();
    mark_hits(&eng, id, &hits, EditSource::User).unwrap();
    assert!(!hits.is_empty());
    let mut verified = Vec::new();
    eng.apply_redaction(id, "Redact".into(), EditSource::User, |doc| {
        let rep = crate::redact::apply(doc, &ApplyOptions::default()).map_err(err)?;
        // A trailing comment no rewrite would produce: only an exact write keeps it.
        verified = [rep.bytes.as_slice(), b"\n% verified\n"].concat();
        Ok((rep.pages.iter().map(|&p| p as u32).collect(), verified.clone()))
    })
    .unwrap();
    assert!(!verified.is_empty());
    let out = d.join("out.pdf").to_string_lossy().into_owned();
    eng.save(id, Some(&out)).unwrap();
    assert!(std::fs::read(&out).unwrap() == verified, "save as wrote other bytes");
    // A plain save over it (the doc's path now) goes through backup and replace, same bytes.
    eng.save(id, None).unwrap();
    assert!(std::fs::read(&out).unwrap() == verified, "save wrote other bytes");
}

/// The fixture with "needle" on pages 1 and 3, encrypted with user password "secret".
fn encrypted(test: &str) -> (PathBuf, String) {
    use mupdf::pdf::{Encryption, PdfWriteOptions};
    let d = dir(test);
    let path = d.join("s.pdf").to_string_lossy().into_owned();
    let src = PdfDocument::from_bytes(&fixture::build(&Fixture { pages: 3, lines: 4, needle_every: 2, ..Default::default() })).unwrap();
    let mut o = PdfWriteOptions::default();
    o.set_encryption(Encryption::Aes256).set_user_password("secret").set_owner_password("owner");
    src.save_with_options(&path, o).unwrap();
    (d, path)
}

#[test]
fn encrypted_input_is_verified_with_its_password_and_stays_encrypted() {
    let (d, path) = encrypted("encrypted");
    let eng = Engine::new(2, 64 << 20);
    let (id, _) = eng.open_pending(&path);
    eng.authenticate(id, "secret").unwrap();
    let hits = find(&eng, id, &needle()).unwrap();
    assert!(hits.len() >= 2, "{hits:?}");
    mark_hits(&eng, id, &hits, EditSource::User).unwrap();

    let out = d.join("s_Redacted.pdf").to_string_lossy().into_owned();
    let r = apply(&eng, id, Sanitize::default(), Some(out.clone()), EditSource::User).unwrap();
    let ApplyOutcome::Applied { verify, .. } = r else { panic!("not applied: {r:?}") };
    assert!(verify.is_clean());

    let mut doc = mupdf::Document::open(out.as_str()).unwrap();
    assert!(doc.needs_password().unwrap(), "the redacted copy lost its encryption");
    assert!(!doc.authenticate("wrong").unwrap());
    let eng2 = Engine::new(2, 64 << 20);
    let (id2, _) = eng2.open_pending(&out);
    eng2.authenticate(id2, "secret").unwrap();
    assert!(find(&eng2, id2, &needle()).unwrap().is_empty(), "redacted text readable with the password");
    assert_eq!(eng2.doc_entry(id2).unwrap().page_count(), 3);
}

#[test]
fn encrypted_output_without_its_password_fails_closed() {
    let (_d, path) = encrypted("encrypted_nopw");
    let mut doc = mupdf::Document::open(path.as_str()).unwrap();
    assert!(doc.authenticate("secret").unwrap());
    let mut doc = PdfDocument::try_from(doc).unwrap();
    let hits = crate::redact::find(&doc, &Query::Text { needle: "needle".into(), case_sensitive: false, whole_word: true }).unwrap();
    crate::redact::mark_hits(&mut doc, &hits, &MarkStyle::default()).unwrap();
    // No password to check the written copy with: refuse, never report it clean.
    let r = crate::redact::apply(&mut doc, &ApplyOptions::default());
    assert!(matches!(r, Err(RedactError::NeedsPassword)), "{r:?}");
    assert!(matches!(err(RedactError::NeedsPassword), EngineError::NeedsPassword));
}

#[test]
fn a_failed_apply_changes_nothing() {
    let (_d, _p, eng, id) = open("fail");
    assert!(apply(&eng, id, Sanitize::default(), None, EditSource::User).is_err(), "no marks");
    let hits = find(&eng, id, &needle()).unwrap();
    let s = mark_hits(&eng, id, &hits, EditSource::User).unwrap();
    // A closure error rolls the lane back: the marks are still there, unapplied.
    let r = eng.apply_redaction(id, "Redact".into(), EditSource::User, |_| Err(EngineError::Invalid("x".into())));
    assert!(r.is_err());
    assert_eq!(eng.edit_state(id).unwrap().rev, s.rev);
    assert_eq!(list_marks(&eng, id).unwrap().len(), hits.len());
}

#[test]
fn command_args_parse_as_the_ui_sends_them() {
    let q: FindQuery = serde_json::from_str(r#"{"kind":"pattern","pattern":"email"}"#).unwrap();
    assert!(matches!(q, FindQuery::Pattern { pattern: Pattern::Email }));
    let q: FindQuery = serde_json::from_str(r#"{"kind":"text","needle":"x","caseSensitive":true,"wholeWord":false}"#).unwrap();
    assert!(matches!(q, FindQuery::Text { case_sensitive: true, .. }));
    let w: MarkWhat = serde_json::from_str(r#"{"kind":"area","page":1,"area":[1,2,3,4]}"#).unwrap();
    assert!(matches!(w, MarkWhat::Area { page: 1, .. }));
    let a: ApplyArgs = serde_json::from_str(r#"{"sanitize":{"bookmarks":false}}"#).unwrap();
    assert!(!a.sanitize.bookmarks && a.sanitize.metadata && !a.save);
    // No path ever comes from the webview: an unknown "saveTo" is not a field.
    let a: ApplyArgs = serde_json::from_str(r#"{"save":true,"saveTo":"C:/x.pdf"}"#).unwrap();
    assert!(a.save);
    let o: ApplyOutcome = ApplyOutcome::Refused { leaks: vec![] };
    assert_eq!(serde_json::to_value(&o).unwrap()["status"], "refused");
}

#[test]
fn sanitize_preview_leaves_the_document() {
    let (_d, _p, eng, id) = open("preview");
    let hits = find(&eng, id, &needle()).unwrap();
    let s = mark_hits(&eng, id, &hits, EditSource::User).unwrap();
    let p = preview_sanitize(&eng, id, &Sanitize::default()).unwrap();
    assert_eq!(p.annotations, 0, "the marks themselves are not comments");
    assert_eq!(eng.edit_state(id).unwrap().rev, s.rev);
    assert_eq!(list_marks(&eng, id).unwrap().len(), hits.len());
}

#[test]
fn text_marks_keep_their_label() {
    let (d, path, eng, id) = open("labels");
    let hits = find(&eng, id, &needle()).unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|h| !h.before.is_empty() || !h.after.is_empty()), "hits carry context: {hits:?}");
    mark_hits(&eng, id, &hits, EditSource::User).unwrap();
    mark_area(&eng, id, 1, [10.0, 10.0, 50.0, 50.0], EditSource::User).unwrap();
    let labels = |e: &Engine, i| { let mut l: Vec<_> = list_marks(e, i).unwrap().into_iter().map(|m| m.label).collect(); l.sort(); l };
    let mut want: Vec<Option<String>> = hits.iter().map(|h| Some(h.text.clone())).chain([None]).collect();
    want.sort();
    assert_eq!(labels(&eng, id), want);

    eng.undo(id).unwrap();
    assert_eq!(labels(&eng, id), want[1..]);
    eng.redo(id).unwrap();
    assert_eq!(labels(&eng, id), want);

    // Save and reopen: the label is in the file.
    let saved = d.join("saved.pdf").to_string_lossy().into_owned();
    eng.save(id, Some(&saved)).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&saved).unwrap().doc;
    assert_eq!(labels(&eng2, id2), want);

    // Security: after Apply the label (the redacted text) is nowhere in the output.
    let out = redacted_path(&path);
    let r = apply(&eng, id, Sanitize::default(), Some(out.clone()), EditSource::User).unwrap();
    let ApplyOutcome::Applied { verify, .. } = r else { panic!("refused: {r:?}") };
    assert!(verify.is_clean());
    let bytes = std::fs::read(&out).unwrap();
    for h in &hits {
        assert!(!bytes.windows(h.text.len()).any(|w| w == h.text.as_bytes()), "label {:?} left in the output", h.text);
    }
}
