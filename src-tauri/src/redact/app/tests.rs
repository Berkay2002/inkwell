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
    preview_sanitize(&eng, id, &Sanitize::default()).unwrap();
    assert_eq!(eng.edit_state(id).unwrap().rev, s.rev);
    assert_eq!(list_marks(&eng, id).unwrap().len(), hits.len());
}
