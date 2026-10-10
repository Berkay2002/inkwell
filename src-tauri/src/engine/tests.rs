use super::fixture::{self, Fixture, PAGE_H, PAGE_W};
use super::*;
use std::sync::mpsc;

fn write_fixture(name: &str, f: &Fixture) -> String {
    let dir = std::env::temp_dir().join("inkwell-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, fixture::build(f)).unwrap();
    p.to_string_lossy().into_owned()
}

fn key(doc: DocId, page: u32, rot: u8, scale: u32, tx: u16, ty: u16) -> TileKey {
    TileKey { doc, rev: 0, page, rot, scale, tx, ty }
}

fn px(t: &Tile, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * t.w + x) * 4) as usize;
    t.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn open_reports_page_count_and_size() {
    let eng = Engine::new(2, 64 << 20);
    let path = write_fixture("open.pdf", &Fixture { pages: 3, ..Default::default() });
    let info = eng.open(&path).unwrap();
    assert_eq!(info.page_count, 3);
    assert_eq!(info.first_page, [PAGE_W, PAGE_H]);
}

#[test]
fn renders_edge_tiles_with_expected_size_and_content() {
    let eng = Engine::new(2, 64 << 20);
    let path = write_fixture("render.pdf", &Fixture { pages: 1, ..Default::default() });
    let id = eng.open(&path).unwrap().doc;
    let entry = eng.doc_entry(id).unwrap();

    // 612x792 at 1.0: a 2x2 grid whose right/bottom tiles are partial.
    let t00 = eng.render_tile(&entry, &key(id, 0, 0, 1000, 0, 0)).unwrap();
    assert_eq!((t00.w, t00.h), (512, 512));
    let t11 = eng.render_tile(&entry, &key(id, 0, 0, 1000, 1, 1)).unwrap();
    assert_eq!((t11.w, t11.h), (100, 280));
    assert!(eng.render_tile(&entry, &key(id, 0, 0, 1000, 2, 0)).is_err());

    // The tinted band sits at PDF y 700..756, i.e. top-left y 36..92; page 0 is rgb(0, 115, 230).
    let [r, g, b, a] = px(&t00, 480, 60);
    assert!(r < 8 && (g as i32 - 115).abs() < 8 && (b as i32 - 230).abs() < 8 && a == 255, "{r} {g} {b} {a}");
    // Below the text everything is paper white.
    assert_eq!(px(&t11, 50, 250), [255, 255, 255, 255]);
    // Text rendered: some dark pixel in the body area.
    let dark = (0..512u32).any(|x| (100..400).any(|y| px(&t00, x, y)[0] < 80));
    assert!(dark, "no text pixels found");
}

#[test]
fn rotation_swaps_dimensions() {
    let eng = Engine::new(2, 64 << 20);
    let path = write_fixture("rot.pdf", &Fixture { pages: 1, ..Default::default() });
    let id = eng.open(&path).unwrap().doc;
    let entry = eng.doc_entry(id).unwrap();
    // Rotated 90 degrees: 792 wide x 612 tall at scale 1.
    let t = eng.render_tile(&entry, &key(id, 0, 1, 1000, 1, 1)).unwrap();
    assert_eq!((t.w, t.h), (792 - 512, 612 - 512));
    // The band (top of the upright page) moves to the right edge: right column is tinted.
    let t = eng.render_tile(&entry, &key(id, 0, 1, 1000, 1, 0)).unwrap();
    let [r, _, b, _] = px(&t, 215, 300);
    assert!(r < 8 && b > 200, "band not on the right edge for 90deg: {r} {b}");
}

#[test]
fn extracts_text_lines_inside_the_page() {
    let eng = Engine::new(2, 64 << 20);
    let path = write_fixture("text.pdf", &Fixture { pages: 2, ..Default::default() });
    let id = eng.open(&path).unwrap().doc;
    let lines = eng.page_text(id, 1).unwrap();
    assert!(lines.iter().any(|l| l.t.contains("Inkwell fixture page 2")), "{lines:?}");
    assert!(lines.len() >= 13);
    for l in &lines {
        assert!(l.b[0] >= 0.0 && l.b[2] <= PAGE_W && l.b[1] >= 0.0 && l.b[3] <= PAGE_H, "{l:?}");
        assert!(l.b[2] > l.b[0] && l.b[3] > l.b[1]);
    }
}

#[test]
fn search_streams_hits_per_page_and_finishes() {
    let eng = Engine::new(4, 64 << 20);
    let f = Fixture { pages: 40, needle_every: 7, ..Default::default() };
    let path = write_fixture("search.pdf", &f);
    let id = eng.open(&path).unwrap().doc;

    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let search = eng
        .search(id, "needle".into(), Arc::new(move |e| {
            let _ = tx.lock().send(e);
        }))
        .unwrap();

    let mut pages = vec![];
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
            SearchEvent::Hits { search: s, page, rects } => {
                assert_eq!(s, search);
                assert_eq!(rects.len(), 1);
                let r = rects[0];
                assert!(r[2] > r[0] && r[3] > r[1] && r[2] <= PAGE_W);
                pages.push(page);
            }
            SearchEvent::Done { hits, pages: n, .. } => {
                assert_eq!((hits, n), (6, 40));
                break;
            }
        }
    }
    pages.sort();
    assert_eq!(pages, vec![0, 7, 14, 21, 28, 35]);
}

#[test]
fn outline_maps_chapters_to_pages() {
    let eng = Engine::new(2, 64 << 20);
    let path = write_fixture("outline.pdf", &Fixture { pages: 6, chapter_every: 2, ..Default::default() });
    let id = eng.open(&path).unwrap().doc;
    let o = eng.outline(id).unwrap();
    let got: Vec<_> = o.iter().map(|i| (i.title.clone(), i.page)).collect();
    assert_eq!(
        got,
        vec![
            ("Chapter 1".into(), Some(0)),
            ("Chapter 2".into(), Some(2)),
            ("Chapter 3".into(), Some(4))
        ]
    );
}

#[test]
fn password_flow() {
    // Encrypt the fixture with MuPDF itself.
    let dir = std::env::temp_dir().join("inkwell-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("secret.pdf").to_string_lossy().into_owned();
    {
        use mupdf::pdf::{Encryption, PdfDocument, PdfWriteOptions};
        let src = PdfDocument::from_bytes(&fixture::build(&Fixture::default())).unwrap();
        let mut o = PdfWriteOptions::default();
        o.set_encryption(Encryption::Aes256).set_user_password("secret").set_owner_password("owner");
        src.save_with_options(&out, o).unwrap();
    }

    let eng = Engine::new(2, 64 << 20);
    let (id, r) = eng.open_pending(&out);
    assert!(matches!(r, Err(EngineError::NeedsPassword)));
    assert!(matches!(eng.authenticate(id, "nope"), Err(EngineError::BadPassword)));
    let info = eng.authenticate(id, "secret").unwrap();
    assert_eq!(info.page_count, 3);
    // Workers (other threads) can open it with the stored password and render.
    let entry = eng.doc_entry(id).unwrap();
    let e2 = eng.clone();
    let tile = eng.call(class::VISIBLE, move |e| e.render_tile(&entry, &key(id, 0, 0, 1000, 0, 0)));
    drop(e2);
    assert_eq!(tile.unwrap().w, 512);
}

#[test]
fn tile_cache_stays_within_budget_and_dedupes() {
    // Budget of ~8 full tiles (1 MiB each).
    let eng = Engine::new(4, 8 << 20);
    let f = Fixture { pages: 60, ..Default::default() };
    let path = write_fixture("cache.pdf", &f);
    let id = eng.open(&path).unwrap().doc;

    let (tx, rx) = mpsc::channel();
    let n = 60;
    for p in 0..n {
        let tx = tx.clone();
        eng.request_tile(key(id, p, 0, 1000, 0, 0), 0, 1, p, Box::new(move |o| {
            let _ = tx.send(matches!(o, TileOutcome::Ready(_)));
        }));
    }
    // Same tile asked for again while queued/cached must not render twice.
    for _ in 0..5 {
        let tx = tx.clone();
        eng.request_tile(key(id, 0, 0, 1000, 0, 0), 0, 1, 0, Box::new(move |o| {
            let _ = tx.send(matches!(o, TileOutcome::Ready(_)));
        }));
    }
    for _ in 0..(n + 5) {
        assert!(rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap());
    }
    let s = eng.stats();
    assert!(s.cache_bytes <= (8 << 20) + (1 << 20), "cache grew to {} bytes", s.cache_bytes);
    assert!(s.rendered <= n as u64 + 1, "rendered {} for {} distinct tiles", s.rendered, n);
}

#[test]
fn stale_epochs_are_skipped() {
    let eng = Engine::new(1, 64 << 20);
    let path = write_fixture("stale.pdf", &Fixture { pages: 4, ..Default::default() });
    let id = eng.open(&path).unwrap().doc;

    // Block the single worker so both requests are queued, then check the old epoch is dropped.
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let _busy = std::thread::spawn({
        let eng = eng.clone();
        move || {
            eng.call(class::VISIBLE, move |_| -> Result<()> {
                started_tx.send(()).unwrap();
                gate_rx.recv().unwrap();
                Ok(())
            })
        }
    });
    started_rx.recv().unwrap();

    let (tx, rx) = mpsc::channel();
    let t1 = tx.clone();
    eng.request_tile(key(id, 0, 0, 1000, 0, 0), 0, 1, 0, Box::new(move |o| {
        let _ = t1.send(("old", matches!(o, TileOutcome::Stale)));
    }));
    eng.request_tile(key(id, 1, 0, 1000, 0, 0), 0, 2, 0, Box::new(move |o| {
        let _ = tx.send(("new", matches!(o, TileOutcome::Ready(_))));
    }));
    gate_tx.send(()).unwrap();
    let mut got = vec![rx.recv().unwrap(), rx.recv().unwrap()];
    got.sort();
    assert_eq!(got, vec![("new", true), ("old", true)]);
}

/// Bounding box (device px) of the blue header band, found by scanning every tile of the page.
fn band_bbox(eng: &Engine, id: DocId, rot: u8, scale: u32) -> (u32, u32, u32, u32) {
    let entry = eng.doc_entry(id).unwrap();
    let s = scale as f32 / 1000.0;
    let (pw, ph) = if rot % 2 == 0 { (PAGE_W, PAGE_H) } else { (PAGE_H, PAGE_W) };
    let (w, h) = ((pw * s).ceil() as u32, (ph * s).ceil() as u32);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for ty in 0..h.div_ceil(512) {
        for tx in 0..w.div_ceil(512) {
            let t = eng.render_tile(&entry, &key(id, 0, rot, scale, tx as u16, ty as u16)).unwrap();
            for y in (0..t.h).step_by(4) {
                for x in (0..t.w).step_by(4) {
                    let p = px(&t, x, y);
                    if p[2] > 200 && p[0] < 30 {
                        let (gx, gy) = (tx * 512 + x, ty * 512 + y);
                        x0 = x0.min(gx);
                        y0 = y0.min(gy);
                        x1 = x1.max(gx);
                        y1 = y1.max(gy);
                    }
                }
            }
        }
    }
    (x0, y0, x1, y1)
}

#[test]
fn view_rotation_is_clockwise_and_scale_culls_in_device_space() {
    let eng = Engine::new(2, 64 << 20);
    let path = write_fixture("probe.pdf", &Fixture { pages: 1, ..Default::default() });
    let id = eng.open(&path).unwrap().doc;
    // Band in page space: x 36..576, top-left y 36..92 (a 540 x 56 pt bar).
    let near = |a: u32, b: f32| (a as f32 - b).abs() <= 8.0;
    for (scale, k) in [(1000u32, 1.0f32), (2000, 2.0)] {
        let (x0, y0, x1, y1) = band_bbox(&eng, id, 0, scale);
        assert!(near(x0, 36.0 * k) && near(x1, 576.0 * k) && near(y0, 36.0 * k) && near(y1, 92.0 * k), "rot0 x{k}: {x0} {y0} {x1} {y1}");
    }
    // 90 deg clockwise: the top edge becomes the right edge (page height 792 is the new width).
    let (x0, y0, x1, y1) = band_bbox(&eng, id, 1, 1000);
    assert!(near(x0, 792.0 - 92.0) && near(x1, 792.0 - 36.0) && near(y0, 36.0) && near(y1, 576.0), "rot1: {x0} {y0} {x1} {y1}");
    let (x0, y0, x1, y1) = band_bbox(&eng, id, 2, 1000);
    assert!(near(x0, 36.0) && near(x1, 576.0) && near(y0, 792.0 - 92.0) && near(y1, 792.0 - 36.0), "rot2: {x0} {y0} {x1} {y1}");
    let (x0, y0, x1, y1) = band_bbox(&eng, id, 3, 1000);
    assert!(near(x0, 36.0) && near(x1, 92.0) && near(y0, 36.0) && near(y1, 576.0), "rot3: {x0} {y0} {x1} {y1}");
}
/// Renames `from` to `to`, retrying for a moment: a worker finishing its last operation on a
/// just-closed document may still hold the file for a few milliseconds.
fn rename_soon(from: &str, to: &str) -> std::io::Result<()> {
    let mut last = Ok(());
    for _ in 0..60 {
        last = std::fs::rename(from, to);
        if last.is_ok() {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    last
}

#[test]
fn close_releases_the_file() {
    let eng = Engine::new(4, 64 << 20);
    let path = write_fixture("close.pdf", &Fixture { pages: 5, ..Default::default() });
    let id = eng.open(&path).unwrap().doc;
    let entry = eng.doc_entry(id).unwrap();
    // Render on a worker so the document has been used from more than one thread.
    let (e2, en2) = (eng.clone(), entry.clone());
    eng.call(class::VISIBLE, move |_| e2.render_tile(&en2, &key(id, 1, 0, 1000, 0, 0)).map(|_| ())).unwrap();
    eng.render_tile(&entry, &key(id, 2, 0, 1000, 0, 0)).unwrap();
    drop(entry);

    eng.close(id);
    let moved = format!("{path}.moved");
    std::fs::rename(&path, &moved).expect("file still open after close");
    std::fs::rename(&moved, &path).unwrap();
}

#[test]
fn close_with_queued_work_never_reopens_the_file() {
    let eng = Engine::new(1, 64 << 20);
    let path = write_fixture("close-queued.pdf", &Fixture { pages: 300, ..Default::default() });
    let id = eng.open(&path).unwrap().doc;

    // Hold the only worker, queue a page-size scan and a tile, close, then let them run.
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let busy = std::thread::spawn({
        let eng = eng.clone();
        move || {
            eng.call(class::VISIBLE, move |_| -> Result<()> {
                started_tx.send(()).unwrap();
                gate_rx.recv().unwrap();
                Ok(())
            })
        }
    });
    started_rx.recv().unwrap();
    eng.scan_sizes(id, Arc::new(|_, _| {})).unwrap();
    let (tx, rx) = mpsc::channel();
    eng.request_tile(key(id, 3, 0, 1000, 0, 0), 0, 1, 0, Box::new(move |o| {
        let _ = tx.send(matches!(o, TileOutcome::Failed(_) | TileOutcome::Stale));
    }));

    eng.close(id);
    gate_tx.send(()).unwrap();
    busy.join().unwrap().unwrap();
    assert!(rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(), "tile for a closed document was served");

    let moved = format!("{path}.moved");
    rename_soon(&path, &moved).expect("a queued job reopened the closed document");
    std::fs::rename(&moved, &path).unwrap();
}

#[test]
fn close_evicts_only_that_documents_display_lists() {
    let eng = Engine::new(2, 64 << 20);
    let a = eng.open(&write_fixture("lists-a.pdf", &Fixture { pages: 3, ..Default::default() })).unwrap().doc;
    let b = eng.open(&write_fixture("lists-b.pdf", &Fixture { pages: 3, ..Default::default() })).unwrap().doc;
    for id in [a, b] {
        let entry = eng.doc_entry(id).unwrap();
        for page in 0..2 {
            eng.render_tile(&entry, &key(id, page, 0, 1000, 0, 0)).unwrap();
        }
    }
    assert_eq!(eng.stats().lists, 4);
    eng.close(a);
    assert_eq!(eng.stats().lists, 2);
    // The other document still renders from its cached lists.
    let entry = eng.doc_entry(b).unwrap();
    eng.render_tile(&entry, &key(b, 0, 0, 1000, 0, 0)).unwrap();
    assert!(eng.doc_entry(a).is_err());
}

#[test]
fn old_generations_are_never_served() {
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&write_fixture("gen.pdf", &Fixture { pages: 2, ..Default::default() })).unwrap().doc;
    let ask = |k: TileKey| {
        let (tx, rx) = mpsc::channel();
        eng.request_tile(k, 0, 1, 0, Box::new(move |o| {
            let _ = tx.send(match o {
                TileOutcome::Ready(_) => "ready",
                TileOutcome::Stale => "stale",
                TileOutcome::Failed(_) => "failed",
            });
        }));
        rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap()
    };
    let old = key(id, 0, 0, 1000, 0, 0);
    assert_eq!(ask(old), "ready");
    assert_eq!(eng.bump_generation(id).unwrap(), 1);
    // The cached tile and any display list belong to generation 0 and must not answer for 1.
    assert_eq!(ask(old), "stale");
    let new = TileKey { rev: 1, ..old };
    assert_eq!(ask(new), "ready");
    assert_eq!(eng.stats().rendered, 2);
}

#[test]
fn rejects_files_that_are_not_pdfs() {
    let eng = Engine::new(1, 8 << 20);
    let dir = std::env::temp_dir().join("inkwell-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("fake.pdf");
    std::fs::write(&fake, b"just some text, not a document").unwrap();
    let fake = fake.to_string_lossy().into_owned();
    assert!(matches!(eng.open(&fake), Err(EngineError::NotPdf)));
    assert!(matches!(eng.open_pending(&fake).1, Err(EngineError::NotPdf)));
    assert!(eng.open(dir.join("does-not-exist.pdf").to_str().unwrap()).is_err());
}

#[test]
fn many_threads_share_one_document() {
    let eng = Engine::new(4, 64 << 20);
    let id = eng.open(&write_fixture("shared.pdf", &Fixture { pages: 24, ..Default::default() })).unwrap().doc;
    let entry = eng.doc_entry(id).unwrap();
    let threads: Vec<_> = (0..8u32)
        .map(|t| {
            let (eng, entry) = (eng.clone(), entry.clone());
            std::thread::spawn(move || {
                for p in 0..24 {
                    let page = (p + t * 3) % 24;
                    let tile = eng.render_tile(&entry, &key(id, page, 0, 1000, 0, 0)).unwrap();
                    assert_eq!((tile.w, tile.h), (512, 512));
                    eng.page_text(id, page).unwrap();
                    eng.search_page(&entry, page, "fixture").unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

/// Does N cold pages in parallel finish faster than serially? `INKWELL_PROBE_PDF=... cargo test --release --lib probe_parallel -- --ignored --nocapture`.
#[test]
#[ignore]
fn probe_parallel() {
    use std::time::Instant;
    let path = std::env::var("INKWELL_PROBE_PDF").expect("INKWELL_PROBE_PDF");
    let eng = Engine::new(1, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    let entry = eng.doc_entry(id).unwrap();
    let t = Instant::now();
    for p in 20..24u32 {
        eng.render_tile(&entry, &key(id, p, 0, 370, 0, 0)).unwrap();
    }
    println!("serial 4 previews: {:?}", t.elapsed());
    let t = Instant::now();
    std::thread::scope(|s| {
        for p in 30..34u32 {
            let (eng, entry) = (&eng, &entry);
            s.spawn(move || {
                let t = Instant::now();
                eng.render_tile(entry, &key(id, p, 0, 370, 0, 0)).unwrap();
                println!("  parallel p{p}: {:?}", t.elapsed());
            });
        }
    });
    println!("parallel 4 previews: {:?}", t.elapsed());
}
