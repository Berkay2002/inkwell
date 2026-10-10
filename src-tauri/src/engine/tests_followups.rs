//! Regressions for the phase 1 review's open follow-ups (docs/engine-rules.md).

use super::fixture::{self, Fixture};
use super::pool::class;
use super::*;
use std::sync::mpsc;
use std::time::Duration;

fn write_fixture(name: &str, f: &Fixture) -> String {
    let dir = std::env::temp_dir().join("inkwell-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, fixture::build(f)).unwrap();
    p.to_string_lossy().into_owned()
}

fn key(doc: DocId, rev: u32, page: u32, scale: u32) -> TileKey {
    TileKey { doc, rev, page, rot: 0, scale, tx: 0, ty: 0 }
}

/// Holds the engine's only worker until the returned sender is used (or dropped).
fn hold_worker(eng: &Arc<Engine>) -> (mpsc::Sender<()>, std::thread::JoinHandle<Result<()>>) {
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let eng = eng.clone();
    let busy = std::thread::spawn(move || {
        eng.call(class::VISIBLE, move |_| -> Result<()> {
            started_tx.send(()).unwrap();
            let _ = gate_rx.recv();
            Ok(())
        })
    });
    started_rx.recv().unwrap();
    (gate_tx, busy)
}

fn outcome_name(o: &TileOutcome) -> &'static str {
    match o {
        TileOutcome::Ready(_) => "ready",
        TileOutcome::Stale => "stale",
        TileOutcome::Failed(_) => "failed",
    }
}

/// A request for generation N still queued when N+1 lands must not be rendered from N+1's
/// content and cached (or served) as N.
#[test]
fn queued_request_for_an_old_generation_is_stale() {
    let eng = Engine::new(1, 64 << 20);
    let id = eng.open(&write_fixture("fu-stale-rev.pdf", &Fixture { pages: 2, ..Default::default() })).unwrap().doc;
    let (gate, busy) = hold_worker(&eng);
    let (tx, rx) = mpsc::channel();
    eng.request_tile(key(id, 0, 0, 1000), 0, 1, 0, Box::new(move |o| tx.send(outcome_name(&o)).unwrap()));
    eng.bump_generation(id).unwrap();
    gate.send(()).unwrap();
    busy.join().unwrap().unwrap();
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), "stale");
    assert_eq!(eng.stats().cache_tiles, 0, "an old-generation tile was cached");
}

/// A page that failed to load once (here: the file was briefly unreadable) loads on the next try.
#[test]
fn display_list_errors_are_not_cached() {
    let eng = Engine::new(2, 64 << 20);
    let path = write_fixture("fu-err.pdf", &Fixture { pages: 2, ..Default::default() });
    let good = std::fs::read(&path).unwrap();
    let id = eng.open(&path).unwrap().doc;
    let entry = eng.doc_entry(id).unwrap();
    // Replicas reopen from the file at the next generation; make that file unreadable for now.
    for s in &entry.slots {
        *s.lock() = None;
    }
    let rev = eng.bump_generation(id).unwrap();
    std::fs::write(&path, b"%PDF-1.4 broken").unwrap();
    assert!(eng.render_tile(&entry, &key(id, rev, 0, 500)).is_err());
    std::fs::write(&path, &good).unwrap();
    eng.render_tile(&entry, &key(id, rev, 0, 500)).expect("the first failure was cached");
}

/// A thumbnail job that the main view also asks for runs ahead of other thumbnail work.
#[test]
fn joining_main_view_request_raises_the_priority() {
    let eng = Engine::new(1, 64 << 20);
    let id = eng.open(&write_fixture("fu-prio.pdf", &Fixture { pages: 2, ..Default::default() })).unwrap().doc;
    let (gate, busy) = hold_worker(&eng);
    let order = Arc::new(Mutex::new(Vec::new()));

    // Queued: a thumbnail tile, then other interactive work that would normally run first.
    let (tx, rx) = mpsc::channel();
    let (o, t) = (order.clone(), tx.clone());
    eng.request_tile(key(id, 0, 0, 300), 1, 1, 0, Box::new(move |_| {
        o.lock().push("tile");
        t.send(()).unwrap();
    }));
    let other = std::thread::spawn({
        let (eng, o) = (eng.clone(), order.clone());
        move || {
            eng.call(class::INTERACTIVE, move |_| -> Result<()> {
                o.lock().push("other");
                Ok(())
            })
        }
    });
    while eng.pool.pending() < 2 {
        std::thread::sleep(Duration::from_millis(5));
    }
    // The main view now wants the same tile.
    eng.request_tile(key(id, 0, 0, 300), 0, 1, 0, Box::new(move |_| tx.send(()).unwrap()));
    gate.send(()).unwrap();
    busy.join().unwrap().unwrap();
    other.join().unwrap().unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(*order.lock(), vec!["tile", "other"]);
}

/// A render that finishes while `close` runs must not leave its tile in the cache.
#[test]
fn close_racing_a_render_leaves_no_orphan_tiles() {
    let eng = Engine::new(1, 256 << 20);
    let path = write_fixture("fu-orphan.pdf", &Fixture { pages: 1, ..Default::default() });
    for _ in 0..8 {
        let id = eng.open(&path).unwrap().doc;
        let (tx, rx) = mpsc::channel();
        eng.request_tile(key(id, 0, 0, 4000), 0, 1, 0, Box::new(move |o| tx.send(outcome_name(&o)).unwrap()));
        // Both the finished render and `close` now queue up on the cache lock; either may win it.
        let guard = eng.tiles.lock();
        let closer = std::thread::spawn({
            let eng = eng.clone();
            move || eng.close(id)
        });
        std::thread::sleep(Duration::from_millis(150));
        drop(guard);
        closer.join().unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(eng.stats().cache_tiles, 0, "a tile of a closed document stayed cached");
    }
}
