//! Engine benchmark: `cargo run --release --bin bench [pages]`.
//! Prints the numbers the phase-1 performance targets are judged on.

use inkwell_lib::engine::cache::TileKey;
use inkwell_lib::engine::fixture::{self, Fixture};
use inkwell_lib::engine::{Engine, SearchEvent};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn working_set_mb() -> f64 {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb);
        c.WorkingSetSize as f64 / (1024.0 * 1024.0)
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    let pages: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
    let dir = std::env::temp_dir().join("inkwell-bench");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("fixture-{pages}.pdf"));
    let spec = Fixture { pages, chapter_every: 25, needle_every: 50, lines: 40 };
    std::fs::write(&path, fixture::build(&spec)).unwrap();
    let path = path.to_string_lossy().into_owned();
    println!("fixture: {pages} pages, {:.1} MB, {path}", std::fs::metadata(&path).unwrap().len() as f64 / 1e6);

    let budget_mb = 128usize;
    let workers = Engine::default_workers();
    let eng = Engine::new(workers, budget_mb << 20);
    println!("workers: {workers}, tile cache budget: {budget_mb} MB, baseline RSS {:.0} MB", working_set_mb());

    if std::env::var("BENCH_BREAKDOWN").is_ok() {
        let t = Instant::now();
        let doc = mupdf::Document::open(path.as_str()).unwrap();
        println!("  breakdown: Document::open {:.1} ms", ms(t.elapsed()));
        let t = Instant::now();
        let page = doc.load_page(0).unwrap();
        println!("  breakdown: load_page {:.1} ms", ms(t.elapsed()));
        let t = Instant::now();
        let list = page.to_display_list(true).unwrap();
        println!("  breakdown: to_display_list {:.1} ms", ms(t.elapsed()));
        let t = Instant::now();
        let pix = mupdf::Pixmap::new_with_w_h(&mupdf::Colorspace::device_rgb(), 512, 512, true).unwrap();
        let dev = mupdf::Device::from_pixmap(&pix).unwrap();
        list.run(&dev, &mupdf::Matrix::new_scale(1.5, 1.5), mupdf::rect::Rect::INF).unwrap();
        println!("  breakdown: run {:.1} ms", ms(t.elapsed()));
    }

    // 1. Cold open to first pixels (everything the UI waits on before page 1 is visible).
    let t0 = Instant::now();
    let info = eng.open(&path).unwrap();
    let t_open = t0.elapsed();
    let entry = eng.doc_entry(info.doc).unwrap();
    let key = |page: u32, tx: u16, ty: u16, scale: u32| TileKey { doc: info.doc, rev: 0, page, rot: 0, scale, tx, ty };
    let tile = eng.render_tile(&entry, &key(0, 0, 0, 1500)).unwrap();
    let t_first = t0.elapsed();
    println!(
        "cold open: open={:.1} ms, first tile ({}x{}) done at {:.1} ms",
        ms(t_open),
        tile.w,
        tile.h,
        ms(t_first)
    );

    for (label, k) in [("same page, tile 2", key(0, 1, 0, 1500)), ("next page, tile 1", key(1, 0, 0, 1500))] {
        let t = Instant::now();
        eng.render_tile(&entry, &k).unwrap();
        println!("  warm: {label} {:.1} ms", ms(t.elapsed()));
    }

    // 2. Page-size scan for the whole document.
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let t0 = Instant::now();
    eng.scan_sizes(info.doc, Arc::new(move |start, sizes| {
        let _ = tx.lock().unwrap().send((start, sizes.len()));
    }))
    .unwrap();
    let mut got = 0;
    while got < pages {
        got += rx.recv_timeout(Duration::from_secs(60)).unwrap().1;
    }
    println!("page-size scan: {:.1} ms for {pages} pages", ms(t0.elapsed()));

    // 3. Scroll simulation: stream tiles for every page in order, as fast as the pool allows,
    //    at a retina-ish scale (2 tiles/page). Sample cache size and RSS on the way.
    let (dtx, drx) = mpsc::channel();
    let t0 = Instant::now();
    let mut sent = 0usize;
    let scale = 2000;
    for p in 0..pages as u32 {
        for (tx_, ty_) in [(0u16, 0u16), (1, 0), (0, 1), (1, 1)] {
            let dtx = dtx.clone();
            eng.request_tile(key(p, tx_, ty_, scale), 0, 1, p * 4, Box::new(move |_| {
                let _ = dtx.send(());
            }));
            sent += 1;
        }
    }
    let mut done = 0usize;
    let (mut max_cache, mut max_rss) = (0usize, 0f64);
    while done < sent {
        drx.recv_timeout(Duration::from_secs(120)).unwrap();
        done += 1;
        if done % 64 == 0 {
            max_cache = max_cache.max(eng.stats().cache_bytes);
            max_rss = max_rss.max(working_set_mb());
        }
    }
    let el = t0.elapsed();
    let s = eng.stats();
    println!(
        "tile stream: {sent} tiles in {:.0} ms = {:.0} tiles/s ({:.2} ms/tile/worker); \
         cache max {:.0} MB (budget {budget_mb} MB), RSS max {:.0} MB",
        ms(el),
        sent as f64 / el.as_secs_f64(),
        s.render_ms_total as f64 / s.rendered.max(1) as f64,
        max_cache as f64 / 1048576.0,
        max_rss
    );

    // 4. Full-document search.
    let (stx, srx) = mpsc::channel();
    let stx = Mutex::new(stx);
    let t0 = Instant::now();
    eng.search(info.doc, "needle".into(), Arc::new(move |e| {
        let _ = stx.lock().unwrap().send(e);
    }))
    .unwrap();
    let mut first_hit = None;
    loop {
        match srx.recv_timeout(Duration::from_secs(60)).unwrap() {
            SearchEvent::Hits { .. } => {
                first_hit.get_or_insert(t0.elapsed());
            }
            SearchEvent::Done { hits, pages, .. } => {
                println!(
                    "search 'needle': {hits} hits over {pages} pages in {:.0} ms (first hit {:.0} ms)",
                    ms(t0.elapsed()),
                    first_hit.map(ms).unwrap_or(0.0)
                );
                break;
            }
        }
    }
    println!("final RSS {:.0} MB", working_set_mb());
}
