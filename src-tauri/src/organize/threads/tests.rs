//! Article threads through the edit lane: every step keeps the ring, /F, /T and the pages' /B in line
//! (`check` runs after each one), damaged files are reported without a panic, and what is written
//! reads back from a saved file.

use super::*;
use crate::engine::edit::{EditOp, EditSource};
use crate::engine::fixture::{self, Fixture};
use crate::engine::{DocId, Engine, EngineError};
use crate::organize::{
    thread_add_bead, thread_create, thread_delete, thread_delete_bead, thread_move_bead, thread_reorder, thread_set_info, threads_get,
};
use std::path::PathBuf;
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("threads-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(test: &str, pages: usize) -> (Arc<Engine>, DocId, PathBuf) {
    let d = dir(test);
    let p = d.join("a.pdf");
    std::fs::write(&p, fixture::build(&Fixture { pages, lines: 4, ..Default::default() })).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    (eng, id, d)
}

fn reopen(eng: &Engine, id: DocId) -> (Arc<Engine>, DocId) {
    let s = eng.save(id, None).unwrap();
    let e = Engine::new(2, 64 << 20);
    let i = e.open(&s.path).unwrap().doc;
    (e, i)
}

const USER: EditSource = EditSource::User;
const R: [f32; 4] = [50.0, 60.0, 250.0, 300.0];

fn list(eng: &Engine, id: DocId) -> Vec<Thread> {
    threads_get(eng, id).unwrap()
}

/// The object numbers a page's /B lists.
fn b_ids(page: &PdfObject) -> Vec<i32> {
    let Some(b) = get(page, "B") else { return Vec::new() };
    (0..b.len().unwrap() as i32).filter_map(|i| indirect_num(&b.get_array(i).unwrap()?)).collect()
}

/// The invariants of every thread and every page, read from the raw objects.
fn check(eng: &Engine, id: DocId) {
    eng.read_custom(id, |d| {
        let pages = Pages::of(d)?;
        assert!(pages.nums.iter().all(|n| *n > 0), "every page is an indirect object");
        let threads = thread_objs(d)?;
        let ids: Vec<i32> = threads.iter().map(|(i, _)| *i).collect();
        for (tid, t) in &threads {
            let dict = t.resolve()?.unwrap();
            let ring = walk(&dict, *tid, &pages, &mut MAX_TOTAL_BEADS.clone())?;
            assert_eq!(ring.end, End::Closed, "ring of {tid} closes");
            assert!(!ring.beads.is_empty());
            assert!(ring.damage().is_none(), "{tid}: {:?}", ring.damage());
            assert_eq!(get(&dict, "F").and_then(|f| indirect_num(&f)), Some(ring.beads[0].id), "/F is the first bead");
            let n = ring.beads.len();
            for (i, b) in ring.beads.iter().enumerate() {
                let bd = b.obj.resolve()?.unwrap();
                assert_eq!(get(&bd, "T").and_then(|x| indirect_num(&x)), Some(*tid), "/T on every bead");
                assert_eq!(get(&bd, "N").and_then(|x| indirect_num(&x)), Some(ring.beads[(i + 1) % n].id));
                assert_eq!(get(&bd, "V").and_then(|x| indirect_num(&x)), Some(ring.beads[(i + n - 1) % n].id));
                let page = d.find_page(b.page.unwrap() as i32)?;
                assert_eq!(b_ids(&page).iter().filter(|x| **x == b.id).count(), 1, "bead {} listed once on its page", b.id);
            }
            // this thread's beads on a page are listed in ring order
            for p in 0..pages.nums.len() as u32 {
                let listed: Vec<i32> = b_ids(&d.find_page(p as i32)?).into_iter().filter(|x| ring.beads.iter().any(|b| b.id == *x)).collect();
                let order: Vec<i32> = ring.beads.iter().filter(|b| b.page == Some(p)).map(|b| b.id).collect();
                assert_eq!(listed, order, "reading order on page {p}");
            }
        }
        // nothing on a page belongs to an article that is gone, or to another page
        for p in 0..pages.nums.len() as i32 {
            for bead in b_ids(&d.find_page(p)?) {
                let obj = d.new_indirect(bead, 0)?.resolve()?.unwrap();
                let t = get(&obj, "T").and_then(|t| indirect_num(&t)).unwrap();
                assert!(ids.contains(&t), "page {p} lists a bead of an article that is not in /Threads");
                assert_eq!(get(&obj, "P").and_then(|x| indirect_num(&x)), Some(pages.nums[p as usize]));
            }
        }
        Ok(())
    })
    .unwrap();
}

/// The bead objects of a thread, in ring order (raw, for tampering).
fn beads_of(d: &PdfDocument, tid: i32) -> Vec<PdfObject> {
    let pages = Pages::of(d).unwrap();
    let (_, t) = thread_objs(d).unwrap().into_iter().find(|(i, _)| *i == tid).unwrap();
    walk(&t.resolve().unwrap().unwrap(), tid, &pages, &mut MAX_TOTAL_BEADS.clone()).unwrap().beads.into_iter().map(|b| b.obj).collect()
}

fn tamper(eng: &Engine, id: DocId, f: impl FnOnce(&mut PdfDocument) -> Result<Vec<u32>>) {
    eng.apply_custom(id, "tamper".into(), USER, false, f).unwrap();
}

/// Two articles over four pages: A has beads on pages 0, 0 and 1; B has beads on pages 2 and 3.
fn two_threads(eng: &Engine, id: DocId) -> (i32, i32) {
    thread_create(eng, id, "Story A", 0, [40.0, 50.0, 280.0, 700.0], USER).unwrap();
    let a = list(eng, id)[0].id;
    thread_add_bead(eng, id, a, None, 0, [320.0, 50.0, 560.0, 700.0], USER).unwrap();
    thread_add_bead(eng, id, a, None, 1, [40.0, 50.0, 560.0, 300.0], USER).unwrap();
    thread_create(eng, id, "", 2, [40.0, 100.0, 300.0, 400.0], USER).unwrap();
    let b = list(eng, id)[1].id;
    thread_add_bead(eng, id, b, None, 3, [40.0, 100.0, 300.0, 400.0], USER).unwrap();
    (a, b)
}

#[test]
fn reads_a_two_thread_document() {
    let (eng, id, _d) = open("read", 4);
    assert!(list(&eng, id).is_empty());
    let (a, b) = two_threads(&eng, id);
    check(&eng, id);
    let l = list(&eng, id);
    assert_eq!(l.len(), 2);
    assert_eq!((l[0].id, l[1].id), (a, b));
    assert_eq!(l[0].title, "Story A");
    assert_eq!(l[1].title, "Article 2", "no /I title: the fallback by position");
    assert_eq!(l[0].beads.iter().map(|b| b.page).collect::<Vec<_>>(), [0, 0, 1]);
    assert_eq!(l[1].beads.iter().map(|b| b.page).collect::<Vec<_>>(), [2, 3]);
    assert!(l.iter().all(|t| t.damaged.is_none()));
    // the rectangle comes back as it was given (an upright page: page space is y down)
    for (got, want) in l[0].beads[0].rect.iter().zip([40.0, 50.0, 280.0, 700.0]) {
        assert!((got - want).abs() < 0.01, "{got} vs {want}");
    }
    // saved and opened again
    let (e2, i2) = reopen(&eng, id);
    check(&e2, i2);
    assert_eq!(list(&e2, i2), l);
}

#[test]
fn every_step_keeps_the_invariants_and_undo_restores_the_structure() {
    let (eng, id, _d) = open("steps", 4);
    let (a, b) = two_threads(&eng, id);
    let mut snap = list(&eng, id);
    let mut step = |name: &str, f: &dyn Fn() -> Result<crate::engine::edit::EditState>, eng: &Engine| {
        let s = f().unwrap();
        check(eng, id);
        let now = list(eng, id);
        assert_ne!(now, snap, "{name} changed something");
        assert_eq!(s.undo.is_some(), true);
        eng.undo(id).unwrap();
        check(eng, id);
        assert_eq!(list(eng, id), snap, "undo of {name} restores the structure");
        eng.redo(id).unwrap();
        check(eng, id);
        assert_eq!(list(eng, id), now, "redo of {name}");
        snap = now;
    };
    let l = list(&eng, id);
    let a_beads: Vec<i32> = l[0].beads.iter().map(|b| b.id).collect();
    step("insert after the first bead", &|| thread_add_bead(&eng, id, a, Some(a_beads[0]), 1, [10.0, 10.0, 100.0, 100.0], USER), &eng);
    step("append", &|| thread_add_bead(&eng, id, b, None, 0, [300.0, 300.0, 500.0, 500.0], USER), &eng);
    step("resize", &|| thread_move_bead(&eng, id, a, a_beads[1], 0, [320.0, 50.0, 560.0, 400.0], USER), &eng);
    step("move to another page", &|| thread_move_bead(&eng, id, a, a_beads[2], 3, [40.0, 50.0, 560.0, 300.0], USER), &eng);
    step("delete a bead", &|| thread_delete_bead(&eng, id, a, a_beads[0], USER), &eng);
    step(
        "properties",
        &|| thread_set_info(&eng, id, b, &Info { title: "Grön fältet \u{2013} Ärende".into(), author: "Berkay".into(), ..Info::default() }, USER),
        &eng,
    );
    step("reorder", &|| thread_reorder(&eng, id, b, 0, USER), &eng);
    step("delete the article", &|| thread_delete(&eng, id, a, USER), &eng);
    let l = list(&eng, id);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].title, "Grön fältet \u{2013} Ärende");
    assert_eq!(l[0].info.author, "Berkay");
    // the whole thing survives a save
    let (e2, i2) = reopen(&eng, id);
    check(&e2, i2);
    assert_eq!(list(&e2, i2), l);
}

#[test]
fn deleting_the_last_bead_deletes_the_article_and_a_no_op_is_no_step() {
    let (eng, id, _d) = open("last", 2);
    let s = thread_create(&eng, id, "One", 0, R, USER).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Start article One"));
    let t = list(&eng, id).remove(0);
    // the same place and the same properties: no step
    let s = thread_move_bead(&eng, id, t.id, t.beads[0].id, 0, t.beads[0].rect, USER).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Start article One"));
    let s = thread_set_info(&eng, id, t.id, &Info { title: " One ".into(), ..Info::default() }, USER).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Start article One"));
    let s = thread_delete_bead(&eng, id, t.id, t.beads[0].id, USER).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Delete article One"));
    assert!(list(&eng, id).is_empty());
    check(&eng, id);
    eng.read_custom(id, |d| {
        assert!(get(&d.catalog()?, "Threads").is_none(), "the key goes with the last article");
        assert!(get(&d.find_page(0)?, "B").is_none());
        Ok(())
    })
    .unwrap();
}

#[test]
fn input_is_checked() {
    let (eng, id, _d) = open("input", 2);
    assert!(thread_create(&eng, id, "x", 5, R, USER).is_err(), "a page that is not there");
    assert!(thread_create(&eng, id, "x", 0, [10.0, 10.0, 11.0, 11.0], USER).is_err(), "too small");
    assert!(thread_create(&eng, id, "x", 0, [f32::NAN, 0.0, 100.0, 100.0], USER).is_err());
    assert!(list(&eng, id).is_empty(), "a refused step leaves nothing behind");
    // outside the page is clamped to it
    thread_create(&eng, id, "x", 0, [-50.0, -50.0, 5000.0, 100.0], USER).unwrap();
    let t = list(&eng, id).remove(0);
    for (got, want) in t.beads[0].rect.iter().zip([0.0, 0.0, 612.0, 100.0]) {
        assert!((got - want).abs() < 0.01, "{:?}", t.beads[0].rect);
    }
    assert!(thread_add_bead(&eng, id, t.id + 1000, None, 0, R, USER).is_err(), "an article that is not there");
    assert!(thread_add_bead(&eng, id, t.id, Some(t.id), 0, R, USER).is_err(), "a bead that is not there");
    check(&eng, id);
}

#[test]
fn rotated_and_cropped_pages_round_trip() {
    let (eng, id, _d) = open("rotate", 2);
    // a crop box that does not start at the origin, and the page turned
    tamper(&eng, id, |d| {
        let page = d.find_page(0)?;
        let mut p = page.share();
        let mut cb = d.new_array()?;
        for v in [30.0, 40.0, 530.0, 740.0] {
            cb.array_push(PdfObject::new_real(v)?)?;
        }
        p.dict_put("CropBox", cb)?;
        Ok(vec![0])
    });
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    for rect in [[10.0, 20.0, 200.0, 120.0], [300.0, 300.0, 480.0, 450.0]] {
        thread_create(&eng, id, "r", 0, rect, USER).unwrap();
        let t = list(&eng, id).pop().unwrap();
        for (got, want) in t.beads[0].rect.iter().zip(rect) {
            assert!((got - want).abs() < 0.05, "{:?} vs {rect:?}", t.beads[0].rect);
        }
        check(&eng, id);
    }
    // the stored /R is in the page's own space: a bead drawn near the shown top-left is not at the user-space origin
    let user = eng
        .read_custom(id, |d| {
            let t = thread_objs(d)?.remove(0);
            let ring = walk(&t.1.resolve()?.unwrap(), t.0, &Pages::of(d)?, &mut MAX_TOTAL_BEADS.clone())?;
            Ok(ring.beads[0].user.unwrap())
        })
        .unwrap();
    assert!(user[0] >= 30.0 && user[2] <= 530.0 && user[1] >= 40.0 && user[3] <= 740.0, "inside the crop box: {user:?}");
}

#[test]
fn damaged_threads_are_reported_without_a_panic() {
    // each flaw: how to make it, what the readable prefix is
    type Make = fn(&mut PdfDocument, &[PdfObject], &PdfObject) -> Result<()>;
    let cases: Vec<(&str, Make, usize)> = vec![
        ("the ring does not close", |_, b, _| b[2].share().dict_delete("N").map_err(Into::into), 3),
        ("a bead of another thread", |d, b, other| {
            let mut x = b[1].share();
            x.dict_put("T", other.share())?;
            let _ = d;
            Ok(())
        }, 1),
        ("a bead on a non-page", |d, b, _| {
            // object 3 of the fixture is the font
            b[2].share().dict_put("P", d.new_indirect(3, 0)?).map_err(Into::into)
        }, 2),
        ("the chain loops into its middle", |_, b, _| b[2].share().dict_put("N", b[1].share()).map_err(Into::into), 3),
        ("an empty rectangle", |d, b, _| {
            let mut r = d.new_array()?;
            for v in [10.0, 10.0, 10.0, 10.0] {
                r.array_push(PdfObject::new_real(v)?)?;
            }
            b[1].share().dict_put("R", r).map_err(Into::into)
        }, 1),
        ("the links back are wrong", |_, b, _| b[1].share().dict_put("V", b[2].share()).map_err(Into::into), 3),
        ("a chain that is a ring of one with a broken link", |_, b, _| b[0].share().dict_put("N", PdfObject::new_int(7)?).map_err(Into::into), 1),
    ];
    for (n, (what, make, prefix)) in cases.into_iter().enumerate() {
        let (eng, id, _d) = open(&format!("damaged{n}"), 3);
        let (a, b) = {
            thread_create(&eng, id, "A", 0, [10.0, 10.0, 200.0, 200.0], USER).unwrap();
            let a = list(&eng, id)[0].id;
            thread_add_bead(&eng, id, a, None, 1, [10.0, 10.0, 200.0, 200.0], USER).unwrap();
            thread_add_bead(&eng, id, a, None, 2, [10.0, 10.0, 200.0, 200.0], USER).unwrap();
            thread_create(&eng, id, "B", 1, [300.0, 10.0, 500.0, 200.0], USER).unwrap();
            (a, list(&eng, id)[1].id)
        };
        eng.apply_custom(id, "tamper".into(), USER, false, |d| {
            let beads = beads_of(d, a);
            let other = thread_objs(d)?.into_iter().find(|(i, _)| *i == b).unwrap().1;
            make(d, &beads, &other).map(|_| vec![])
        })
        .unwrap();
        let l = list(&eng, id);
        assert_eq!(l.len(), 2, "{what}");
        assert!(l[0].damaged.is_some(), "{what}: reported damaged");
        assert_eq!(l[0].beads.len(), if what.starts_with("the links") { 3 } else { prefix }, "{what}: the readable prefix");
        assert!(l[1].damaged.is_none(), "{what}: the other article is untouched");
        // only properties and delete are offered on a damaged article
        let first = l[0].beads.first().map(|x| x.id);
        let bad_step = thread_add_bead(&eng, id, a, None, 0, R, USER);
        assert!(bad_step.is_err(), "{what}: no new bead on a damaged article");
        if let Some(f) = first {
            assert!(thread_move_bead(&eng, id, a, f, 0, R, USER).is_err());
            assert!(thread_delete_bead(&eng, id, a, f, USER).is_err());
        }
        assert!(thread_reorder(&eng, id, a, 1, USER).is_err(), "{what}: a damaged article is not moved");
        thread_set_info(&eng, id, a, &Info { title: "Kept".into(), ..Info::default() }, USER).unwrap();
        assert_eq!(list(&eng, id)[0].title, "Kept");
        thread_delete(&eng, id, a, USER).unwrap();
        check(&eng, id);
        assert_eq!(list(&eng, id).len(), 1);
        // the article's beads are gone from every page, those past the break too
        eng.read_custom(id, |d| {
            for p in 0..3 {
                for bead in b_ids(&d.find_page(p)?) {
                    let o = d.new_indirect(bead, 0)?.resolve()?.unwrap();
                    assert_eq!(get(&o, "T").and_then(|t| indirect_num(&t)), Some(b), "{what}");
                }
            }
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn a_ring_of_10001_beads_is_capped() {
    let (eng, id, _d) = open("cap", 1);
    thread_create(&eng, id, "Big", 0, R, USER).unwrap();
    let tid = list(&eng, id)[0].id;
    tamper(&eng, id, |d| {
        let (_, t) = thread_objs(d)?.into_iter().next().unwrap();
        let first = beads_of(d, tid).remove(0);
        let page = d.new_indirect(Pages::of(d)?.nums[0], 0)?;
        let total = MAX_BEADS + 1;
        let mut refs = vec![first];
        for _ in 1..total {
            let mut b = d.new_dict()?;
            put_name(&mut b, "Type", "Bead")?;
            b.dict_put("T", t.share())?;
            b.dict_put("P", page.share())?;
            let mut r = d.new_array()?;
            for v in [10.0, 10.0, 90.0, 90.0] {
                r.array_push(PdfObject::new_real(v)?)?;
            }
            b.dict_put("R", r)?;
            refs.push(d.add_object(&b)?);
        }
        for i in 0..total {
            let mut b = refs[i].share();
            b.dict_put("N", refs[(i + 1) % total].share())?;
            b.dict_put("V", refs[(i + total - 1) % total].share())?;
        }
        Ok(vec![])
    });
    let t = list(&eng, id).remove(0);
    assert_eq!(t.beads.len(), MAX_BEADS);
    assert!(t.damaged.as_deref().is_some_and(|m| m.contains("more than 10000")), "{:?}", t.damaged);
    assert!(thread_add_bead(&eng, id, tid, None, 0, R, USER).is_err());
    thread_delete(&eng, id, tid, USER).unwrap();
    assert!(list(&eng, id).is_empty());
}

#[test]
fn a_loop_of_threads_and_duplicates_in_the_array_are_read_once() {
    let (eng, id, _d) = open("dup", 1);
    thread_create(&eng, id, "A", 0, R, USER).unwrap();
    tamper(&eng, id, |d| {
        let (_, t) = thread_objs(d)?.remove(0);
        let mut arr = get(&d.catalog()?, "Threads").unwrap();
        arr.array_push(t.share())?;
        arr.array_push(PdfObject::new_int(3)?)?;
        arr.array_push(d.new_dict()?)?;
        Ok(vec![])
    });
    assert_eq!(list(&eng, id).len(), 1);
}

#[test]
fn deleting_pages_unlinks_their_beads() {
    let (eng, id, _d) = open("pages", 4);
    let (a, b) = two_threads(&eng, id);
    // page 1 carries the last bead of A: the article stays with two beads
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    check(&eng, id);
    let l = list(&eng, id);
    assert_eq!(l.len(), 2);
    assert_eq!(l[0].id, a);
    assert_eq!(l[0].beads.len(), 2);
    assert!(l[0].beads.iter().all(|x| x.page == 0));
    assert_eq!(l[1].beads.iter().map(|x| x.page).collect::<Vec<_>>(), [1, 2], "B's pages shifted down");
    assert!(l.iter().all(|t| t.damaged.is_none()));
    // the first bead of a thread on a deleted page: /F moves to the next
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    check(&eng, id);
    let l = list(&eng, id);
    assert_eq!(l[1].id, b);
    assert_eq!(l[1].beads.len(), 1);
    // every bead of an article on deleted pages: the article goes
    eng.apply(id, EditOp::Delete { pages: vec![0] }).unwrap();
    check(&eng, id);
    let l = list(&eng, id);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].id, b);
    // undo brings them all back
    eng.undo(id).unwrap();
    check(&eng, id);
    assert_eq!(list(&eng, id).len(), 2);
    // and the save keeps what is left
    let (e2, i2) = reopen(&eng, id);
    check(&e2, i2);
}

#[test]
fn moving_and_inserting_pages_keeps_the_beads() {
    let (eng, id, _d) = open("move", 4);
    two_threads(&eng, id);
    let before = list(&eng, id);
    eng.apply(id, EditOp::Move { pages: vec![0], to: 4 }).unwrap();
    check(&eng, id);
    let l = list(&eng, id);
    assert_eq!(l[0].beads.iter().map(|b| b.page).collect::<Vec<_>>(), [3, 3, 0], "A follows its pages");
    assert_eq!(l[0].beads[0].rect, before[0].beads[0].rect);
    eng.apply(id, EditOp::InsertBlank { at: 0, like: 0 }).unwrap();
    check(&eng, id);
    assert_eq!(list(&eng, id)[1].beads.iter().map(|b| b.page).collect::<Vec<_>>(), [2, 3]);
}

#[test]
fn extracting_and_splitting_leave_the_beads_behind() {
    let (eng, id, d) = open("extract", 4);
    two_threads(&eng, id);
    let out = d.join("part.pdf");
    eng.extract(id, &[0, 1], &out.to_string_lossy(), false).unwrap();
    let e2 = Engine::new(2, 64 << 20);
    let i2 = e2.open(&out.to_string_lossy()).unwrap().doc;
    assert!(list(&e2, i2).is_empty(), "no /Threads in the extract");
    e2.read_custom(i2, |d| {
        for p in 0..2 {
            assert!(get(&d.find_page(p)?, "B").is_none(), "no /B on page {p}");
        }
        Ok(())
    })
    .unwrap();
    check(&e2, i2);
    // extracting with "delete after" goes through the delete step
    let out2 = d.join("gone.pdf");
    eng.extract(id, &[2, 3], &out2.to_string_lossy(), true).unwrap();
    check(&eng, id);
    let l = list(&eng, id);
    assert_eq!(l.len(), 1, "article B had no beads left");
}

// ---- review fixes -----------------------------------------------------------------------------

/// B's first bead object (raw), for tampering.
fn first_bead(eng: &Engine, id: DocId, tid: i32) -> PdfObject {
    let mut out = None;
    eng.read_custom(id, |d| {
        out = Some(beads_of(d, tid).remove(0));
        Ok(())
    })
    .unwrap();
    out.unwrap()
}

#[test]
fn a_damaged_article_that_runs_into_another_leaves_the_other_alone() {
    // A's last bead links on into B's ring. B's beads carry /T = B, so the walk of A stops at them.
    for delete_page_instead in [false, true] {
        let (eng, id, _d) = open(&format!("into-b-{delete_page_instead}"), 4);
        let (a, b) = two_threads(&eng, id);
        let b1 = first_bead(&eng, id, b);
        tamper(&eng, id, |d| {
            let mut last = beads_of(d, a).remove(2);
            last.dict_put("N", b1.share())?;
            Ok(vec![])
        });
        let l = list(&eng, id);
        assert!(l[0].damaged.is_some());
        assert_eq!(l[0].beads.len(), 3, "A's own beads are the readable prefix");
        assert!(l[1].damaged.is_none());
        if delete_page_instead {
            // page 2 holds B's first bead: B is mended, A (damaged) is not rewritten
            eng.apply(id, EditOp::Delete { pages: vec![2] }).unwrap();
            let l = list(&eng, id);
            assert_eq!(l.len(), 2);
            assert!(l[0].damaged.is_some(), "A is still damaged, not silently repaired");
            assert_eq!(l[0].beads.len(), 3);
            assert!(l[1].damaged.is_none());
            assert_eq!(l[1].beads.len(), 1);
            eng.read_custom(id, |d| {
                let pages = Pages::of(d)?;
                let (_, t) = thread_objs(d)?.remove(1);
                let ring = walk(&t.resolve()?.unwrap(), b, &pages, &mut MAX_TOTAL_BEADS.clone())?;
                assert_eq!((ring.end, ring.beads.len()), (End::Closed, 1), "B's ring is closed and whole");
                Ok(())
            })
            .unwrap();
        } else {
            thread_delete(&eng, id, a, USER).unwrap();
            check(&eng, id);
            let l = list(&eng, id);
            assert_eq!(l.len(), 1);
            assert_eq!(l[0].id, b);
            assert_eq!(l[0].beads.len(), 2, "B keeps both beads, and both stay on their pages' /B");
        }
    }
}

#[test]
fn deleting_a_damaged_article_keeps_the_beads_of_a_ring_it_runs_into_even_without_a_t() {
    let (eng, id, _d) = open("into-b-no-t", 4);
    let (a, b) = two_threads(&eng, id);
    let b1 = first_bead(&eng, id, b);
    tamper(&eng, id, |d| {
        // B's beads have no /T (allowed on read); A's last bead runs into them
        for mut x in beads_of(d, b) {
            x.dict_delete("T")?;
        }
        let mut last = beads_of(d, a).remove(2);
        last.dict_put("N", b1.share())?;
        Ok(vec![])
    });
    assert!(list(&eng, id)[0].damaged.is_some());
    let on_pages = |eng: &Engine| -> Vec<Vec<i32>> {
        let mut out = Vec::new();
        eng.read_custom(id, |d| {
            for p in 2..4 {
                out.push(b_ids(&d.find_page(p)?));
            }
            Ok(())
        })
        .unwrap();
        out
    };
    let before = on_pages(&eng);
    assert!(before.iter().all(|p| p.len() == 1));
    thread_delete(&eng, id, a, USER).unwrap();
    assert_eq!(on_pages(&eng), before, "B's beads are still listed on their pages");
    let l = list(&eng, id);
    assert_eq!((l.len(), l[0].id, l[0].beads.len()), (1, b, 2));
}

#[test]
fn rectangles_on_a_page_with_no_usable_size_are_refused_not_a_panic() {
    let (eng, id, _d) = open("size", 2);
    tamper(&eng, id, |d| {
        let mut page = d.find_page(0)?;
        let mut r = d.new_array()?;
        for v in [0.0, 0.0, 0.0, 0.0] {
            r.array_push(PdfObject::new_real(v)?)?;
        }
        page.dict_put("MediaBox", r)?;
        Ok(vec![])
    });
    // MuPDF may substitute a default size for it; either way the answer is an error or a bead inside the page
    match thread_create(&eng, id, "Odd", 0, R, USER) {
        Ok(_) => {
            let l = list(&eng, id);
            assert!(l[0].damaged.is_none());
        }
        Err(e) => assert!(!e.to_string().is_empty()),
    }
    // NaN and infinity never reach the clamp
    assert!(thread_create(&eng, id, "Odd", 1, [f32::NAN, 0.0, 10.0, 10.0], USER).is_err());
    assert!(thread_create(&eng, id, "Odd", 1, [0.0, 0.0, f32::INFINITY, 10.0], USER).is_err());
}

#[test]
fn a_page_that_is_not_an_indirect_object_holds_no_beads() {
    let (eng, id, _d) = open("direct-page", 3);
    tamper(&eng, id, |d| {
        // the first page becomes a direct dictionary in the page tree's /Kids
        let root = get(&d.catalog()?, "Pages").unwrap();
        let mut kids = get(&root, "Kids").unwrap();
        let page = kids.get_array(0)?.unwrap().resolve()?.unwrap();
        kids.array_put(0, page)?;
        Ok(vec![])
    });
    let mut direct = false;
    eng.read_custom(id, |d| {
        direct = Pages::of(d)?.nums[0] == 0;
        Ok(())
    })
    .unwrap();
    if !direct {
        // this MuPDF build hands back the page by reference anyway: nothing to refuse
        return;
    }
    assert!(thread_create(&eng, id, "No", 0, R, USER).is_err(), "no /P 0 0 R is ever written");
    thread_create(&eng, id, "Yes", 1, R, USER).unwrap();
    let l = list(&eng, id);
    assert_eq!((l.len(), l[0].beads.len(), l[0].beads[0].page, l[0].damaged.is_none()), (1, 1, 1, true));
    eng.read_custom(id, |d| {
        let pages = Pages::of(d)?;
        let bead = beads_of(d, l[0].id).remove(0).resolve()?.unwrap();
        assert_eq!(get(&bead, "P").and_then(|p| indirect_num(&p)), Some(pages.nums[1]));
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_thousand_articles_sharing_one_big_ring_are_read_within_the_total_budget() {
    let (eng, id, _d) = open("budget", 1);
    thread_create(&eng, id, "Big", 0, R, USER).unwrap();
    let tid = list(&eng, id)[0].id;
    tamper(&eng, id, |d| {
        let (_, t) = thread_objs(d)?.into_iter().next().unwrap();
        let mut first = beads_of(d, tid).remove(0);
        first.dict_delete("T")?;
        let page = d.new_indirect(Pages::of(d)?.nums[0], 0)?;
        let total = 6_000;
        let mut refs = vec![first.share()];
        for _ in 1..total {
            let mut b = d.new_dict()?;
            put_name(&mut b, "Type", "Bead")?;
            b.dict_put("P", page.share())?;
            let mut r = d.new_array()?;
            for v in [10.0, 10.0, 90.0, 90.0] {
                r.array_push(PdfObject::new_real(v)?)?;
            }
            b.dict_put("R", r)?;
            refs.push(d.add_object(&b)?);
        }
        for i in 0..total {
            let mut b = refs[i].share();
            b.dict_put("N", refs[(i + 1) % total].share())?;
            b.dict_put("V", refs[(i + total - 1) % total].share())?;
        }
        // thirty more articles that all start at the same bead
        let mut arr = get(&d.catalog()?, "Threads").unwrap();
        for _ in 0..30 {
            let mut th = d.new_dict()?;
            put_name(&mut th, "Type", "Thread")?;
            th.dict_put("F", first.share())?;
            arr.array_push(d.add_object(&th)?)?;
        }
        let _ = t;
        Ok(vec![])
    });
    let l = list(&eng, id);
    assert_eq!(l.len(), 31);
    assert!(l[0].damaged.is_none() && l[0].beads.len() == 6_000);
    let last = l.last().unwrap();
    assert!(last.damaged.as_deref().is_some_and(|m| m.contains("too many")), "{:?}", last.damaged);
    let visited: usize = l.iter().map(|t| t.beads.len()).sum();
    assert!(visited <= MAX_TOTAL_BEADS, "{visited} beads read in all");
    // and an edit under the budget still works
    assert!(thread_delete(&eng, id, tid, USER).is_ok());
}

#[test]
fn deleting_or_moving_an_article_keeps_the_entries_it_does_not_list() {
    let (eng, id, _d) = open("array", 1);
    thread_create(&eng, id, "First", 0, R, USER).unwrap();
    thread_create(&eng, id, "Second", 0, R, USER).unwrap();
    let ids: Vec<i32> = list(&eng, id).iter().map(|t| t.id).collect();
    // a direct entry in between, and more than MAX_THREADS articles after: only the first 1000 are listed
    tamper(&eng, id, |d| {
        let mut arr = get(&d.catalog()?, "Threads").unwrap();
        let mut direct = d.new_dict()?;
        put_name(&mut direct, "Type", "Thread")?;
        arr.array_push(direct)?;
        for _ in 0..MAX_THREADS {
            let mut th = d.new_dict()?;
            put_name(&mut th, "Type", "Thread")?;
            arr.array_push(d.add_object(&th)?)?;
        }
        Ok(vec![])
    });
    let len = |eng: &Engine| -> usize {
        let mut n = 0;
        eng.read_custom(id, |d| {
            n = get(&d.catalog()?, "Threads").unwrap().len()?;
            Ok(())
        })
        .unwrap();
        n
    };
    let before = len(&eng);
    assert_eq!(before, 2 + 1 + MAX_THREADS);
    assert_eq!(list(&eng, id).len(), MAX_THREADS);
    thread_reorder(&eng, id, ids[0], 1, USER).unwrap();
    assert_eq!(len(&eng), before, "nothing dropped by a reorder");
    assert_eq!(list(&eng, id)[1].id, ids[0]);
    thread_delete(&eng, id, ids[1], USER).unwrap();
    assert_eq!(len(&eng), before - 1, "only the deleted article left the array");
}

#[test]
fn properties_applied_unchanged_are_no_step() {
    let (eng, id, _d) = open("props-same", 1);
    thread_create(&eng, id, "", 0, R, USER).unwrap();
    let t = list(&eng, id).remove(0);
    let before = eng.edit_state(id).unwrap().undo;
    // a form that opened with the listed fallback ("Article 1") and was applied as it was: the UI sends
    // nothing; the same info through the engine is no step either
    thread_set_info(&eng, id, t.id, &t.info, USER).unwrap();
    assert_eq!(eng.edit_state(id).unwrap().undo, before);
    // only the author edited, the title field left empty (what the UI sends for an untitled article): the article
    // gets an author and no /Title, so it is still listed by position
    let author = Info { author: "Berkay".into(), ..Info::default() };
    thread_set_info(&eng, id, t.id, &author, USER).unwrap();
    let t = list(&eng, id).remove(0);
    assert_eq!((t.info.title.as_str(), t.info.author.as_str(), t.title.as_str()), ("", "Berkay", "Article 1"));
}

#[test]
fn a_scan_that_runs_out_of_budget_keeps_the_rest_of_a_pages_beads() {
    let (eng, id, _d) = open("strip-budget", 4);
    let (a, b) = two_threads(&eng, id);
    let b1 = first_bead(&eng, id, b);
    let b1_id = indirect_num(&b1).unwrap();
    // page 0 lists A's two beads and then B's first one
    let on_page0 = |eng: &Engine| -> Vec<i32> {
        let mut out = Vec::new();
        eng.read_custom(id, |d| {
            out = b_ids(&d.find_page(0)?);
            Ok(())
        })
        .unwrap();
        out
    };
    tamper(&eng, id, |d| {
        let mut arr = get(&d.find_page(0)?, "B").unwrap();
        arr.array_push(b1.share())?;
        Ok(vec![])
    });
    let before = on_page0(&eng);
    assert_eq!(before.len(), 3);
    assert_eq!(before[2], b1_id);
    // a budget of one: the first entry is looked at (it is A's, so it goes), the others are not looked at
    tamper(&eng, id, |d| {
        let pages = Pages::of(d)?;
        let touched = strip_listed(d, &pages, a, &mut 1)?;
        assert_eq!(touched, [0]);
        Ok(vec![])
    });
    assert_eq!(on_page0(&eng), before[1..], "everything after the budget is kept");
    // and with room for all of them every bead of A goes and B's stays
    tamper(&eng, id, |d| {
        let pages = Pages::of(d)?;
        strip_listed(d, &pages, a, &mut 100)?;
        Ok(vec![])
    });
    assert_eq!(on_page0(&eng), [b1_id]);
}

/// Opens the fixture with `/Perms` and/or an `/AcroForm` added to the catalog (what a certified or signed file has).
fn protected(test: &str, perms: Option<&str>, acroform: Option<&str>) -> (Arc<Engine>, DocId, PathBuf) {
    let (eng, id, d) = open(test, 3);
    tamper(&eng, id, |d| {
        let mut cat = d.catalog()?;
        if let Some(p) = perms {
            cat.dict_put("Perms", d.new_object_from_str(p)?)?;
        }
        if let Some(a) = acroform {
            cat.dict_put("AcroForm", d.new_object_from_str(a)?)?;
        }
        Ok(vec![])
    });
    (eng, id, d)
}

#[test]
fn a_certified_document_takes_no_article_edit() {
    for p in [1, 2, 3] {
        let perms = format!("<< /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P {p} >> >> ] >> >>");
        // the article exists before the document is certified, so every kind of edit has something to refuse
        let (eng, id, _d) = open(&format!("certified{p}"), 3);
        thread_create(&eng, id, "A", 0, R, USER).unwrap();
        thread_create(&eng, id, "B", 1, R, USER).unwrap();
        let (a, b) = (list(&eng, id)[0].id, list(&eng, id)[1].id);
        let bead = list(&eng, id)[0].beads[0].id;
        tamper(&eng, id, |d| {
            let mut cat = d.catalog()?;
            cat.dict_put("Perms", d.new_object_from_str(&perms)?)?;
            Ok(vec![])
        });
        let before = (eng.edit_state(id).unwrap().undo, list(&eng, id).len());
        let refused: Vec<(&str, Result<_>)> = vec![
            ("create", thread_create(&eng, id, "C", 2, R, USER)),
            ("add", thread_add_bead(&eng, id, a, None, 1, R, USER)),
            ("move", thread_move_bead(&eng, id, a, bead, 0, [60.0, 70.0, 260.0, 310.0], USER)),
            ("delete bead", thread_delete_bead(&eng, id, a, bead, USER)),
            ("properties", thread_set_info(&eng, id, a, &Info { author: "X".into(), ..Info::default() }, USER)),
            ("reorder", thread_reorder(&eng, id, b, 0, USER)),
            ("delete", thread_delete(&eng, id, a, USER)),
        ];
        for (what, r) in refused {
            match r {
                Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "P {p} {what}: {m}"),
                other => panic!("P {p} {what}: {:?}", other.map(|s| s.undo)),
            }
        }
        assert_eq!((eng.edit_state(id).unwrap().undo, list(&eng, id).len()), before, "P {p}: no step recorded");
        check(&eng, id);
    }
}

#[test]
fn a_signed_document_is_changed_after_its_signed_bytes() {
    // a signature field with a value (the signed revision), not certified
    let (eng, id, d) = protected("signed", None, Some("<< /SigFlags 3 >>"));
    assert!(eng.read_custom(id, |d| Ok(crate::outline::signed(d))).unwrap());
    assert!(eng.read_custom(id, |d| Ok(crate::outline::certified(d)?.is_none())).unwrap());
    // save once so the file on disk is the signed revision, then edit and save again in place
    let path = d.join("a.pdf");
    eng.save(id, None).unwrap();
    let original = std::fs::read(&path).unwrap();
    thread_create(&eng, id, "A", 0, R, USER).unwrap();
    let a = list(&eng, id)[0].id;
    thread_add_bead(&eng, id, a, None, 1, R, USER).unwrap();
    thread_delete_bead(&eng, id, a, list(&eng, id)[0].beads[1].id, USER).unwrap();
    thread_delete(&eng, id, a, USER).unwrap();
    thread_create(&eng, id, "B", 2, R, USER).unwrap();
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "an update after the signed revision, not a rewrite");
    let (eng2, id2) = reopen(&eng, id);
    assert_eq!(list(&eng2, id2).len(), 1);
}
