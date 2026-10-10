//! Edit lane, undo/redo, save, and the Organize ops.

use super::*;
use crate::engine::fixture::{self, Fixture, PAGE_H, PAGE_W};
use crate::engine::{Engine, TileOutcome, cache::TileKey};
use parking_lot::Mutex;
use std::sync::mpsc;
use std::time::Duration;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn fixture_at(d: &Path, name: &str, f: &Fixture) -> String {
    let p = d.join(name);
    std::fs::write(&p, fixture::build(f)).unwrap();
    p.to_string_lossy().into_owned()
}

fn pages(n: usize) -> Fixture {
    Fixture { pages: n, lines: 2, ..Default::default() }
}

/// The fixture page number printed on each page (1-based), 0 for a page without it.
fn labels(eng: &Engine, id: DocId) -> Vec<u32> {
    let n = eng.doc_entry(id).unwrap().page_count();
    (0..n)
        .map(|p| {
            eng.page_text(id, p)
                .unwrap()
                .iter()
                .find_map(|l| l.t.strip_prefix("Inkwell fixture page ").and_then(|s| s.trim().parse().ok()))
                .unwrap_or(0)
        })
        .collect()
}

fn reopen(path: &str) -> (Arc<Engine>, DocId) {
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(path).unwrap().doc;
    (eng, id)
}

const UPRIGHT: [f32; 2] = [PAGE_W, PAGE_H];
const TURNED: [f32; 2] = [PAGE_H, PAGE_W];

#[test]
fn rotate_undo_redo() {
    let d = dir("rotate");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;

    let s = eng.apply(id, EditOp::Rotate { pages: vec![1], quarter_turns: 1 }).unwrap();
    assert_eq!(s.rev, 1);
    assert_eq!(s.sizes, vec![UPRIGHT, TURNED, UPRIGHT]);
    assert!(s.dirty);
    assert_eq!((s.undo.as_deref(), s.redo.as_deref()), (Some("Rotate p. 2 by 90°"), None));
    // Tiles of the new generation show it: 792 x 612 at scale 1.
    let entry = eng.doc_entry(id).unwrap();
    let t = eng.render_tile(&entry, &TileKey { doc: id, rev: 1, page: 1, rot: 0, scale: 1000, tx: 1, ty: 1 }).unwrap();
    assert_eq!((t.w, t.h), (792 - 512, 612 - 512));

    let s = eng.undo(id).unwrap();
    assert_eq!((s.rev, s.dirty, s.redo.as_deref()), (2, false, Some("Rotate p. 2 by 90°")));
    assert_eq!(s.sizes, vec![UPRIGHT; 3]);
    let s = eng.redo(id).unwrap();
    assert_eq!((s.rev, s.sizes[1]), (3, TURNED));
    // Nothing more to redo: no new generation.
    assert_eq!(eng.redo(id).unwrap().rev, 3);
    // A full turn changes nothing and records no step.
    assert_eq!(eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 4 }).unwrap().rev, 3);
    // Counter-clockwise turns back.
    let s = eng.apply(id, EditOp::Rotate { pages: vec![1, 2], quarter_turns: -1 }).unwrap();
    assert_eq!(s.sizes, vec![UPRIGHT, UPRIGHT, TURNED]);
}

#[test]
fn delete_move_insert_and_undo_everything() {
    let d = dir("organize");
    let path = fixture_at(&d, "a.pdf", &pages(5));
    let other = fixture_at(&d, "b.pdf", &Fixture { pages: 3, lines: 6, ..Default::default() });
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;

    eng.apply(id, EditOp::Delete { pages: vec![1, 3] }).unwrap();
    assert_eq!(labels(&eng, id), vec![1, 3, 5]);
    // Move page 5 to the front, then 1 and 3 to the end.
    eng.apply(id, EditOp::Move { pages: vec![2], to: 0 }).unwrap();
    assert_eq!(labels(&eng, id), vec![5, 1, 3]);
    eng.apply(id, EditOp::Move { pages: vec![1, 2], to: 0 }).unwrap();
    assert_eq!(labels(&eng, id), vec![1, 3, 5]);
    eng.apply(id, EditOp::Move { pages: vec![0], to: 3 }).unwrap();
    assert_eq!(labels(&eng, id), vec![3, 5, 1]);
    // Dropping a page right where it is is not a step.
    let rev = eng.edit_state(id).unwrap().rev;
    assert_eq!(eng.apply(id, EditOp::Move { pages: vec![1], to: 2 }).unwrap().rev, rev);

    let s = eng.apply(id, EditOp::InsertBlank { at: 1, like: 0 }).unwrap();
    assert_eq!(labels(&eng, id), vec![3, 0, 5, 1]);
    assert_eq!(s.sizes[1], UPRIGHT);
    assert!(eng.page_text(id, 1).unwrap().is_empty());

    let s = eng.apply(id, EditOp::InsertFile { at: 4, path: other.clone(), pages: Some(vec![2, 0]), password: None }).unwrap();
    assert_eq!(s.page_count, 6);
    assert_eq!(labels(&eng, id), vec![3, 0, 5, 1, 1, 3]);
    // The inserted pages are b.pdf's (6 body lines), in its own order.
    assert_eq!(eng.page_text(id, 4).unwrap().len(), 7);
    assert_eq!(eng.page_text(id, 5).unwrap().len(), 7);

    // Undo all the way back to the file as opened.
    let mut s = eng.edit_state(id).unwrap();
    while s.undo.is_some() {
        s = eng.undo(id).unwrap();
    }
    assert!(!s.dirty);
    assert_eq!(labels(&eng, id), vec![1, 2, 3, 4, 5]);
    // And forward again.
    while s.redo.is_some() {
        s = eng.redo(id).unwrap();
    }
    assert_eq!(labels(&eng, id), vec![3, 0, 5, 1, 1, 3]);

    // An edit after an undo drops the redo branch.
    eng.undo(id).unwrap();
    let s = eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 2 }).unwrap();
    assert_eq!(s.redo, None);
    assert_eq!(s.page_count, 4);

    // Invalid ops change nothing.
    let rev = s.rev;
    assert!(eng.apply(id, EditOp::Delete { pages: vec![0, 1, 2, 3] }).is_err());
    assert!(eng.apply(id, EditOp::Delete { pages: vec![9] }).is_err());
    assert!(eng.apply(id, EditOp::Move { pages: vec![], to: 0 }).is_err());
    assert!(eng.apply(id, EditOp::InsertFile { at: 0, path: d.join("missing.pdf").to_string_lossy().into(), pages: None, password: None }).is_err());
    assert_eq!(eng.edit_state(id).unwrap().rev, rev);
    assert_eq!(labels(&eng, id), vec![3, 0, 5, 1]);
}

#[test]
fn save_then_reopen_keeps_order_rotation_and_count() {
    let d = dir("save");
    let path = fixture_at(&d, "a.pdf", &pages(4));
    let original = std::fs::read(&path).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    // Render first so replicas hold the file open, as in the app.
    let entry = eng.doc_entry(id).unwrap();
    eng.render_tile(&entry, &TileKey { doc: id, rev: 0, page: 0, rot: 0, scale: 500, tx: 0, ty: 0 }).unwrap();

    // Only changes and additions: saved incrementally, the original bytes untouched at the front.
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    let s = eng.save(id, None).unwrap();
    assert!(!s.dirty);
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "not an incremental save");
    {
        let pdf = PdfDocument::open(path.as_str()).unwrap();
        assert_eq!(pdf.load_pdf_page(0).unwrap().rotation().unwrap(), 90);
    }

    // A delete: the file is rewritten and the deleted page is gone from it.
    eng.apply(id, EditOp::Move { pages: vec![3], to: 1 }).unwrap();
    eng.apply(id, EditOp::Delete { pages: vec![2] }).unwrap();
    assert!(eng.edit_state(id).unwrap().dirty);
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(!saved.starts_with(&original), "a delete must rewrite the file");
    let pdf = PdfDocument::open(path.as_str()).unwrap();
    let page_objects = (1..pdf.xref_len().unwrap() as i32)
        .filter_map(|i| pdf.xref_object(i).ok().flatten())
        .filter(|o| o.get_dict("Type").ok().flatten().and_then(|t| t.as_name().ok()).is_some_and(|t| t == b"Page"))
        .count();
    assert_eq!(page_objects, 3, "the deleted page is still in the file");
    drop(pdf);

    let (eng2, id2) = reopen(&path);
    assert_eq!(labels(&eng2, id2), vec![1, 4, 3]);
    assert_eq!(eng2.doc_entry(id2).unwrap().page_count(), 3);
    let first = eng2.page_text(id2, 0).unwrap();
    assert!(first[0].b[2] > PAGE_W, "page 1 lost its rotation");

    // Saving again with nothing changed writes nothing.
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    eng.save(id, None).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before);
    // Undo after saving makes the document dirty again.
    assert!(eng.undo(id).unwrap().dirty);
}

#[test]
fn save_as_moves_the_document_and_leaves_the_original() {
    let d = dir("saveas");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let original = std::fs::read(&path).unwrap();
    let copy = d.join("copy.pdf").to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    eng.apply(id, EditOp::Delete { pages: vec![0] }).unwrap();
    let s = eng.save(id, Some(&copy)).unwrap();
    assert_eq!((s.path.as_str(), s.dirty), (copy.as_str(), false));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let (eng2, id2) = reopen(&copy);
    assert_eq!(labels(&eng2, id2), vec![2, 3]);
    // Save as with no edits at all copies the document.
    let b = eng2.save(id2, Some(&d.join("copy2.pdf").to_string_lossy())).unwrap();
    assert_eq!(b.page_count, 2);
    assert_eq!(reopen(&b.path).0.doc_entry(1).unwrap().page_count(), 2);
}

#[test]
fn failed_save_never_touches_the_original() {
    let d = dir("savefail");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let original = std::fs::read(&path).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();

    // The file cannot be replaced (read-only): the save fails, the file and folder are as before.
    let mut perm = std::fs::metadata(&path).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&path, perm.clone()).unwrap();
    assert!(eng.save(id, None).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let names: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names.len(), 1, "temporary file left behind: {names:?}");
    assert!(eng.edit_state(id).unwrap().dirty);
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&path, perm).unwrap();

    // A target folder that does not exist.
    assert!(eng.save(id, Some(&d.join("nope").join("x.pdf").to_string_lossy())).is_err());
    // Now it works, and the document still renders from memory afterwards.
    eng.save(id, None).unwrap();
    assert_eq!(labels(&eng, id), vec![1, 3]);
}

#[test]
fn extract_split_merge() {
    let d = dir("extract");
    let path = fixture_at(&d, "book.pdf", &pages(5));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;

    let out = d.join("picked.pdf").to_string_lossy().into_owned();
    let s = eng.extract(id, &[3, 1], &out, false).unwrap();
    assert_eq!((s.page_count, s.dirty), (5, false));
    let (e2, i2) = reopen(&out);
    assert_eq!(labels(&e2, i2), vec![2, 4]);
    // Extract with delete: one undo step.
    let s = eng.extract(id, &[0], &d.join("first.pdf").to_string_lossy(), true).unwrap();
    assert_eq!((s.page_count, s.undo.as_deref()), (4, Some("Delete p. 1")));
    eng.undo(id).unwrap();

    let r = eng.split(id, 2).unwrap();
    let names: Vec<_> = r.files.iter().map(|f| Path::new(f).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["book_Part1.pdf", "book_Part2.pdf", "book_Part3.pdf"]);
    let counts: Vec<_> = r.files.iter().map(|f| reopen(f).0.doc_entry(1).unwrap().page_count()).collect();
    assert_eq!(counts, [2, 2, 1]);
    let (e3, i3) = reopen(&r.files[2]);
    assert_eq!(labels(&e3, i3), vec![5]);
    // A second split never overwrites the first.
    let first = std::fs::read(&r.files[0]).unwrap();
    let r2 = eng.split(id, 3).unwrap();
    assert!(r2.files[0].ends_with("book_Part1 (2).pdf"), "{:?}", r2.files);
    assert_eq!(std::fs::read(&r.files[0]).unwrap(), first);

    let merged = d.join("merged.pdf").to_string_lossy().into_owned();
    assert_eq!(eng.merge(&[out.clone(), r.files[2].clone(), path.clone()], &merged).unwrap(), 8);
    let (e4, i4) = reopen(&merged);
    assert_eq!(labels(&e4, i4), vec![2, 4, 5, 1, 2, 3, 4, 5]);
    // Merging into a file that is open is refused.
    assert!(eng.merge(&[out.clone()], &path).is_err());
}

#[test]
fn history_is_one_buffer_and_grows_by_the_changes() {
    // 200 pages, 50 edits of every kind.
    let d = dir("memory");
    let path = fixture_at(&d, "big.pdf", &Fixture { pages: 200, ..Default::default() });
    let base = std::fs::metadata(&path).unwrap().len() as usize;
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    for i in 0..50u32 {
        let op = match i % 5 {
            0 => EditOp::Rotate { pages: vec![i], quarter_turns: 1 },
            1 => EditOp::Move { pages: vec![i, i + 50], to: 150 },
            2 => EditOp::InsertBlank { at: i, like: i + 1 },
            3 => EditOp::Delete { pages: vec![i + 7] },
            _ => EditOp::Rotate { pages: (0..100).collect(), quarter_turns: 1 },
        };
        eng.apply(id, op).unwrap();
    }
    let entry = eng.doc_entry(id).unwrap();
    let lane = entry.editor.lock();
    let ed = lane.as_ref().unwrap();
    assert_eq!(ed.steps.len(), 51);
    let shared = &ed.steps[50].snap.bytes;
    assert!(ed.steps.iter().all(|s| Arc::ptr_eq(&s.snap.bytes, shared)), "a step holds its own copy");
    let extra = shared.len() - base;
    println!("200-page file: {base} bytes; history of 50 edits adds {extra} bytes ({:.1} KiB/edit)", extra as f64 / 50.0 / 1024.0);
    assert!(extra < 50 * 64 << 10, "history grew by {extra} bytes");
}

/// Renders in flight while edits land never come back with another generation's content.
#[test]
fn renders_during_edits_match_their_generation() {
    let d = dir("concurrent");
    let path = fixture_at(&d, "a.pdf", &pages(6));
    let eng = Engine::new(4, 256 << 20);
    let id = eng.open(&path).unwrap().doc;
    // Page 0's size per generation, from what each edit reported.
    let truth = Arc::new(Mutex::new(std::collections::HashMap::from([(0u32, UPRIGHT)])));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(u32, u32, u32)>();
    let ready = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let renderers: Vec<_> = (0..3)
        .map(|r| {
            let (eng, stop, tx, ready) = (eng.clone(), stop.clone(), tx.clone(), ready.clone());
            std::thread::spawn(move || {
                let entry = eng.doc_entry(id).unwrap();
                let mut n = 0u32;
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    let rev = entry.generation();
                    // A fresh scale each time, so most requests miss the cache and really render.
                    let scale = 200 + (n * 3 + r) % 300;
                    let (tx, ready) = (tx.clone(), ready.clone());
                    eng.request_tile(TileKey { doc: id, rev, page: 0, rot: 0, scale, tx: 0, ty: 0 }, 0, 1, 0, Box::new(move |o| {
                        if let TileOutcome::Ready(t) = o {
                            let _ = tx.send((rev, scale, t.w * 10000 + t.h));
                            ready.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        }
                    }));
                    n += 1;
                    std::thread::sleep(Duration::from_micros(300));
                }
            })
        })
        .collect();

    for i in 0..40 {
        let s = match i % 4 {
            0 | 1 => eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap(),
            2 => eng.undo(id).unwrap(),
            _ => eng.apply(id, EditOp::Move { pages: vec![0], to: 6 }).and_then(|_| eng.undo(id)).unwrap(),
        };
        truth.lock().insert(s.rev, s.sizes[0]);
        // Let a couple of renders of this generation finish (and others race the next edit).
        let (seen, t0) = (ready.load(std::sync::atomic::Ordering::SeqCst), std::time::Instant::now());
        while ready.load(std::sync::atomic::Ordering::SeqCst) < seen + 2 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // The move produced a generation too: it showed page 2 at index 0.
        if i % 4 == 3 {
            truth.lock().insert(s.rev - 1, UPRIGHT);
        }
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    for r in renderers {
        r.join().unwrap();
    }
    drop(tx);
    let truth = truth.lock();
    let mut checked = 0;
    while let Ok((rev, scale, wh)) = rx.recv_timeout(Duration::from_secs(10)) {
        let size = truth[&rev];
        let s = scale as f32 / 1000.0;
        let want = (size[0] * s).ceil() as u32 * 10000 + (size[1] * s).ceil() as u32;
        assert_eq!(wh, want, "generation {rev} rendered with another generation's page");
        checked += 1;
    }
    println!("checked {checked} tiles across {} generations", truth.len());
    assert!(checked >= 80, "only {checked} renders completed");
}

#[test]
fn history_lists_steps_and_undoes_an_assistant_turn_at_once() {
    let d = dir("history");
    let path = fixture_at(&d, "a.pdf", &pages(5));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    let turn = |t: &str| EditSource::Assistant { turn: t.into() };

    eng.apply(id, EditOp::Rotate { pages: vec![1, 2, 4], quarter_turns: 1 }).unwrap();
    eng.apply_from(id, EditOp::Delete { pages: vec![0] }, turn("t1")).unwrap();
    eng.apply_from(id, EditOp::Move { pages: vec![0], to: 4 }, turn("t1")).unwrap();
    let s = eng.apply_from(id, EditOp::InsertBlank { at: 1, like: 0 }, turn("t2")).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Insert blank page as p. 2"));

    let h = eng.history(id).unwrap();
    let rows: Vec<_> = h.iter().map(|e| (e.label.as_str(), e.pages.clone(), e.current)).collect();
    assert_eq!(
        rows,
        vec![
            ("Opened", vec![], false),
            ("Rotate pp. 2-3, 5 by 90°", vec![1, 2, 4], false),
            ("Delete p. 1", vec![0], false),
            ("Move p. 1 to position 5", vec![3], false),
            ("Insert blank page as p. 2", vec![1], true),
        ]
    );
    assert_eq!(h[2].source, turn("t1"));
    assert!(h[0].saved && h.windows(2).all(|w| w[0].at_ms <= w[1].at_ms));

    // Turn t2, then all of turn t1 in one call.
    let s = eng.undo(id).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Delete p. 1 and 1 more"));
    let s = eng.undo(id).unwrap();
    assert_eq!(labels(&eng, id), vec![1, 2, 3, 4, 5]);
    assert_eq!((s.undo.as_deref(), s.redo.as_deref()), (Some("Rotate pp. 2-3, 5 by 90°"), Some("Delete p. 1 and 1 more")));
    eng.redo(id).unwrap();
    assert_eq!(labels(&eng, id), vec![3, 4, 5, 2]);

    // Restore to any entry, forward or back; the history itself is unchanged.
    assert_eq!(eng.restore(id, 4).unwrap().page_count, 5);
    let s = eng.restore(id, 0).unwrap();
    assert!(!s.dirty);
    assert_eq!(labels(&eng, id), vec![1, 2, 3, 4, 5]);
    assert_eq!(eng.history(id).unwrap().len(), 5);
    assert!(eng.restore(id, 5).is_err());
}

#[test]
fn a_redaction_never_leaves_the_old_bytes_behind() {
    let d = dir("redact");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let original = std::fs::read(&path).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    let hooked = Arc::new(std::sync::atomic::AtomicU32::new(u32::MAX));
    let h = hooked.clone();
    set_redaction_hook(move |doc| h.store(doc, std::sync::atomic::Ordering::SeqCst));

    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    // A failing custom edit, halfway through: rolled back, no step.
    let before = eng.edit_state(id).unwrap();
    let r = eng.apply_custom(id, "Redact".into(), EditSource::User, true, |doc| {
        doc.delete_page(1)?;
        Err(EngineError::Invalid("leaked".into()))
    });
    assert!(r.is_err());
    assert_eq!(eng.edit_state(id).unwrap(), before);
    assert_eq!(labels(&eng, id), vec![1, 2, 3]);

    // Stand-in for the redaction engine: remove page 2.
    let s = eng
        .apply_custom(id, "Redact p. 2".into(), EditSource::User, true, |doc| {
            doc.delete_page(1)?;
            Ok(vec![1])
        })
        .unwrap();
    assert_eq!(labels(&eng, id), vec![1, 3]);
    assert!(s.redacted);
    assert_eq!(hooked.load(std::sync::atomic::Ordering::SeqCst), id);
    {
        let entry = eng.doc_entry(id).unwrap();
        let lane = entry.editor.lock();
        let ed = lane.as_ref().unwrap();
        let snap = &ed.step().snap;
        assert!(!snap.bytes[..snap.len].starts_with(&original), "the redaction step is an incremental update");
    }
    // Later edits stay cheap, but saving rewrites.
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    eng.save(id, None).unwrap();
    assert!(!std::fs::read(&path).unwrap().starts_with(&original));

    // Purge what came before the redaction: no step holds the unredacted buffer any more.
    eng.undo(id).unwrap();
    let s = eng.forget_history(id).unwrap();
    assert!(s.redacted && s.undo.is_none() && s.redo.is_some());
    let h = eng.history(id).unwrap();
    assert_eq!(h.len(), 2);
    assert!(h[0].label.starts_with("Redact"));
    let entry = eng.doc_entry(id).unwrap();
    let lane = entry.editor.lock();
    assert!(lane.as_ref().unwrap().steps.iter().all(|s| !s.snap.bytes.starts_with(&original)));
}

// ---- pre-merge review regressions ----------------------------------------------------------

#[test]
fn review_edit_after_a_full_rewrite_save() {
    let d = dir("review_rewrite_then_edit");
    let path = fixture_at(&d, "a.pdf", &pages(4));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    eng.save(id, None).unwrap();
    // The full rewrite must not leave the authoritative document unusable for the next edit.
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    eng.apply(id, EditOp::Move { pages: vec![2], to: 0 }).unwrap();
    assert_eq!(labels(&eng, id), vec![4, 1, 3]);
    eng.save(id, None).unwrap();
    let (e2, i2) = reopen(&path);
    assert_eq!(labels(&e2, i2), vec![4, 1, 3]);
    assert!(e2.page_text(i2, 1).unwrap()[0].b[2] > PAGE_W, "rotation lost");
    eng.undo(id).unwrap();
    assert_eq!(labels(&eng, id), vec![1, 3, 4]);
}

#[test]
fn review_saving_a_password_protected_file_keeps_it_encrypted() {
    use mupdf::pdf::Encryption;
    let d = dir("review_encrypted");
    let path = d.join("s.pdf").to_string_lossy().into_owned();
    {
        let src = PdfDocument::from_bytes(&fixture::build(&pages(3))).unwrap();
        let mut o = PdfWriteOptions::default();
        o.set_encryption(Encryption::Aes256).set_user_password("secret").set_owner_password("owner");
        src.save_with_options(&path, o).unwrap();
    }
    for (i, op) in [EditOp::Rotate { pages: vec![0], quarter_turns: 1 }, EditOp::Delete { pages: vec![1] }].into_iter().enumerate() {
        let eng = Engine::new(2, 64 << 20);
        let (id, _) = eng.open_pending(&path);
        eng.authenticate(id, "secret").unwrap();
        eng.apply(id, op).unwrap();
        eng.save(id, None).unwrap();
        let mut doc = Document::open(path.as_str()).unwrap();
        assert!(doc.needs_password().unwrap(), "save {i} wrote the file without its password");
        assert!(doc.authenticate("secret").unwrap());
    }
}

#[test]
fn review_a_panicking_custom_edit_is_rolled_back() {
    let d = dir("review_panic");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        eng.apply_custom(id, "Redact".into(), EditSource::User, true, |doc| {
            doc.delete_page(1)?;
            panic!("redaction engine bug");
        })
    }));
    assert!(matches!(r, Ok(Err(_))), "panic escaped the edit lane");
    // The next, unrelated edit must not carry the half-done redaction with it.
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    assert_eq!(labels(&eng, id), vec![1, 2, 3]);
    // A huge turn count is not an overflow.
    let s = eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: i32::MAX }).unwrap();
    assert_eq!(s.sizes[0], UPRIGHT);
}

#[test]
fn review_forget_history_keeps_the_rewrite_flags() {
    let d = dir("review_forget");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let original = std::fs::read(&path).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    eng.forget_history(id).unwrap();
    eng.save(id, None).unwrap();
    assert!(!std::fs::read(&path).unwrap().starts_with(&original), "deleted page content kept in the saved file");
}

#[test]
fn review_text_and_search_of_an_old_generation_are_stale() {
    let d = dir("review_text_rev");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    eng.apply(id, EditOp::Delete { pages: vec![0] }).unwrap();
    // A caller that read rev 0 asks for page 0's text: it must not get rev 1's page 0.
    assert!(matches!(eng.page_text_at(id, 0, 0), Err(EngineError::Stale)));
    assert!(eng.page_text_at(id, 1, 0).is_ok());
    let entry = eng.doc_entry(id).unwrap();
    assert!(matches!(eng.search_page_at(&entry, Some(0), 0, "fixture"), Err(EngineError::Stale)));
}

#[cfg(windows)]
#[test]
fn save_keeps_the_files_streams_and_follows_links() {
    let d = dir("replace");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    // An alternate data stream (Windows keeps e.g. the download zone there) survives the save.
    std::fs::write(format!("{path}:inkwell-note"), b"keep me").unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    eng.apply(id, EditOp::Delete { pages: vec![0] }).unwrap();
    eng.save(id, None).unwrap();
    assert_eq!(std::fs::read(format!("{path}:inkwell-note")).unwrap(), b"keep me");
    assert_eq!(labels(&reopen(&path).0, 1), vec![2, 3]);
    let leftovers: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).filter(|n| n != "a.pdf").collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Through a symbolic link (needs Developer Mode or admin; skipped otherwise): the link stays.
    let link = d.join("link.pdf");
    if std::os::windows::fs::symlink_file(&path, &link).is_ok() {
        let link = link.to_string_lossy().into_owned();
        let eng = Engine::new(2, 64 << 20);
        let id = eng.open(&link).unwrap().doc;
        eng.apply(id, EditOp::Delete { pages: vec![0] }).unwrap();
        eng.save(id, None).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(labels(&reopen(&path).0, 1), vec![3]);
    } else {
        println!("symlink test skipped: no privilege");
    }
}

#[test]
fn copying_over_in_place_when_the_folder_takes_no_new_files() {
    let d = dir("copyover");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let new = fixture::build(&pages(2));
    copy_over(Path::new(&path), &new, 2, None).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), new);
    // Bytes that do not read back are never copied over the file.
    assert!(copy_over(Path::new(&path), b"%PDF-1.7 junk", 2, None).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), new);
}

#[test]
fn stale_save_temps_are_removed_on_open() {
    let d = dir("stale");
    let path = fixture_at(&d, "a.pdf", &pages(1));
    let old = d.join(".a.pdf.1.abc.inkwell-tmp");
    let fresh = d.join(".a.pdf.2.def.inkwell-tmp");
    let other = d.join(".b.pdf.1.abc.inkwell-tmp");
    for f in [&old, &fresh, &other] {
        std::fs::write(f, b"x").unwrap();
    }
    let hours_ago = std::time::SystemTime::now() - Duration::from_secs(2 * 3600);
    for f in [&old, &other] {
        std::fs::File::options().write(true).open(f).unwrap().set_modified(hours_ago).unwrap();
    }
    Engine::new(1, 1 << 20).open(&path).unwrap();
    assert!(!old.exists(), "a stale temp file stayed");
    assert!(fresh.exists(), "a save in progress lost its temp file");
    assert!(other.exists(), "another file's temp was removed");
}

/// Two blank pages. Page 1: a link to page 2 (explicit), a link to page 2 through a named
/// destination, a web link, the text field "name" (= "Ada"), and the first widget of the field
/// "addr" (= "Main St"), whose second widget is on page 2.
fn linked_source() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Names << /Dests 12 0 R >> \
         /AcroForm << /Fields [7 0 R 8 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 9 0 R >> >> >> >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [5 0 R 6 0 R 13 0 R 7 0 R 10 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [11 0 R] >>",
        "<< /Type /Annot /Subtype /Link /Rect [72 700 200 720] /Dest [4 0 R /Fit] >>",
        "<< /Type /Annot /Subtype /Link /Rect [72 670 200 690] /A << /S /GoTo /D (second) >> >>",
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V (Ada) /Rect [72 600 300 620] /P 3 0 R >>",
        "<< /FT /Tx /T (addr) /V (Main St) /Kids [10 0 R 11 0 R] >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        "<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [72 560 300 580] /P 3 0 R >>",
        "<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [72 560 300 580] /P 4 0 R >>",
        "<< /Names [(second) [4 0 R /XYZ 0 792 0]] >>",
        "<< /Type /Annot /Subtype /Link /Rect [72 640 200 660] /A << /S /URI /URI (https://example.com/) >> >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Per page: its links (the page index they go to, -1 for a web link) and its widgets' field names.
fn links_and_widgets(path: &str) -> Vec<(Vec<i32>, Vec<String>)> {
    let doc = PdfDocument::open(path).unwrap();
    (0..doc.page_count().unwrap())
        .map(|p| {
            let page = doc.find_page(p).unwrap();
            let (mut links, mut widgets) = (Vec::new(), Vec::new());
            if let Some(annots) = page.get_dict("Annots").unwrap() {
                for a in annots.array_iter().unwrap() {
                    let a = a.unwrap().resolve().unwrap().unwrap();
                    match name_of(a.get_dict("Subtype").unwrap()).as_slice() {
                        b"Link" => links.push(match a.get_dict("Dest").unwrap() {
                            Some(d) => doc.lookup_page_number(&d.get_array(0).unwrap().unwrap()).unwrap(),
                            None => -1,
                        }),
                        b"Widget" => {
                            let mut f = a.clone();
                            while f.get_dict("T").unwrap().is_none() {
                                f = f.get_dict("Parent").unwrap().expect("a widget outside any field");
                            }
                            // The widget must be on this very page.
                            let on = doc.lookup_page_number(&a.get_dict("P").unwrap().unwrap()).unwrap();
                            assert_eq!(on, p);
                            widgets.push(f.get_dict("T").unwrap().unwrap().as_string().unwrap());
                        }
                        _ => {}
                    }
                }
            }
            (links, widgets)
        })
        .collect()
}

/// The root fields of the form: name, value, number of widgets.
fn form_fields(path: &str) -> Vec<(String, String, usize)> {
    let doc = PdfDocument::open(path).unwrap();
    let Some(form) = doc.catalog().unwrap().get_dict("AcroForm").unwrap() else { return vec![] };
    form.get_dict("Fields")
        .unwrap()
        .unwrap()
        .array_iter()
        .unwrap()
        .map(|f| {
            let f = f.unwrap();
            let s = |k: &str| f.get_dict(k).unwrap().map(|v| v.as_string().unwrap()).unwrap_or_default();
            let kids = f.get_dict("Kids").unwrap().map(|k| k.len().unwrap()).unwrap_or(1);
            (s("T"), s("V"), kids)
        })
        .collect()
}

#[test]
fn inserted_pages_keep_their_links_and_form_fields() {
    let d = dir("insertlinks");
    let path = fixture_at(&d, "a.pdf", &pages(3));
    let src = d.join("form.pdf");
    std::fs::write(&src, linked_source()).unwrap();
    let src = src.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;

    eng.apply(id, EditOp::InsertFile { at: 1, path: src.clone(), pages: None, password: None }).unwrap();
    eng.save(id, None).unwrap();
    let pages = links_and_widgets(&path);
    assert_eq!(pages.len(), 5);
    // Both links to the source's page 2 now go to its copy, page index 2.
    assert_eq!(pages[1], (vec![2, 2, -1], vec!["name".to_string(), "addr".to_string()]));
    assert_eq!(pages[2], (vec![], vec!["addr".to_string()]));
    assert_eq!(form_fields(&path), [("name".into(), "Ada".into(), 1), ("addr".into(), "Main St".into(), 2)]);

    // A second copy: its fields are renamed so they keep values of their own.
    eng.apply(id, EditOp::InsertFile { at: 5, path: src.clone(), pages: None, password: None }).unwrap();
    eng.save(id, None).unwrap();
    let names: Vec<String> = form_fields(&path).into_iter().map(|f| f.0).collect();
    assert_eq!(names, ["name", "addr", "name (2)", "addr (2)"]);
    assert_eq!(links_and_widgets(&path)[5].0, vec![6, 6, -1]);

    // Extracting the first copy takes its links and fields along.
    let out = d.join("part.pdf").to_string_lossy().into_owned();
    eng.extract(id, &[1, 2], &out, false).unwrap();
    assert_eq!(links_and_widgets(&out)[0].0, vec![1, 1, -1]);
    assert_eq!(form_fields(&out), [("name".into(), "Ada".into(), 1), ("addr".into(), "Main St".into(), 2)]);

    // Undo takes the form back with the pages.
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    eng.save(id, None).unwrap();
    assert!(form_fields(&path).is_empty());
}

#[test]
fn a_link_whose_page_stays_behind_is_dropped() {
    let d = dir("insertone");
    let path = fixture_at(&d, "a.pdf", &pages(2));
    let src = d.join("form.pdf");
    std::fs::write(&src, linked_source()).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    let src = src.to_string_lossy().into_owned();
    eng.apply(id, EditOp::InsertFile { at: 0, path: src, pages: Some(vec![0]), password: None }).unwrap();
    eng.save(id, None).unwrap();
    // The web link stays; the widgets stay, "addr" with only the widget that came along.
    assert_eq!(links_and_widgets(&path)[0], (vec![-1], vec!["name".to_string(), "addr".to_string()]));
    assert_eq!(form_fields(&path), [("name".into(), "Ada".into(), 1), ("addr".into(), "Main St".into(), 1)]);
}
