//! History storage benchmark: `cargo run --release --bin history_bench`.
//! Bytes stored per version for FastCDC (16/64 KiB), object-level dedup and whole-file zstd over
//! realistic save sequences, plus the real store's commit time. Numbers go into docs/history.md.

use inkwell_lib::engine::fixture::{self, Fixture};
use inkwell_lib::history::{Author, Kind, NewVersion, Store};
use mupdf::Rect;
use mupdf::pdf::{PdfDocument, PdfWriteOptions};
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

#[derive(Clone, Copy)]
enum Save {
    Incremental,
    Full,
    FullGarbageCompress,
}

/// `n` successive saves, each adding one annotation.
fn edits(base: &[u8], n: usize, how: Save) -> Vec<Vec<u8>> {
    let mut out = vec![base.to_vec()];
    for i in 0..n {
        let doc = PdfDocument::from_bytes(out.last().unwrap()).unwrap();
        let pages = doc.page_count().unwrap();
        let mut page = doc.load_pdf_page(i as i32 % pages).unwrap();
        let y = 40.0 + (i % 20) as f32 * 30.0;
        page.add_text_annotation(Rect::new(40.0, y, 60.0, y + 20.0), &format!("review note {i}")).unwrap();
        let mut o = PdfWriteOptions::default();
        match how {
            Save::Incremental => {
                o.set_incremental(true);
            }
            Save::Full => {}
            Save::FullGarbageCompress => {
                o.set_garbage_level(3).set_compress(true);
            }
        }
        let mut buf = Vec::new();
        doc.write_to_with_options(&mut buf, o).unwrap();
        out.push(buf);
    }
    out
}

fn cdc(b: &[u8], avg: u32) -> Vec<&[u8]> {
    fastcdc::v2020::FastCDC::new(b, avg / 4, avg, avg * 4).map(|c| &b[c.offset..c.offset + c.length]).collect()
}

/// One unit per `N G obj ... endobj` (cut after each `endobj`), the rest (xref, trailer) as units.
fn objects(b: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + 6 <= b.len() {
        if &b[i..i + 6] == b"endobj" {
            let mut end = i + 6;
            while end < b.len() && (b[end] == b'\r' || b[end] == b'\n') {
                end += 1;
            }
            out.push(&b[start..end]);
            start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    if start < b.len() {
        out.push(&b[start..]);
    }
    out
}

/// What the store spends on one stored unit: flag + zstd-or-raw + nonce + tag.
fn unit_cost(u: &[u8]) -> u64 {
    let z = zstd::bulk::compress(u, 3).unwrap().len().min(u.len());
    (1 + z + 24 + 16) as u64
}

/// Manifest size for `n` chunk refs (JSON list, zstd, sealed), measured on real-looking ids.
fn manifest_cost(units: &[&[u8]]) -> u64 {
    let mut s = String::from("{\"chunks\":[");
    for u in units {
        s.push_str(&format!("[\"{}\",{}],", blake3::hash(u).to_hex(), u.len()));
    }
    s.push_str("]}");
    unit_cost(s.as_bytes()) + 300
}

struct Row {
    first: u64,
    rest: u64,
    secs: f64,
}

fn run(seq: &[Vec<u8>], split: impl Fn(&[u8]) -> Vec<&[u8]>, whole: bool) -> Row {
    let mut seen = HashSet::new();
    let mut costs = Vec::new();
    let t = Instant::now();
    for v in seq {
        let c = if whole {
            if seen.insert(*blake3::hash(v).as_bytes()) { unit_cost(v) } else { 0 }
        } else {
            let units = split(v);
            let mut c = manifest_cost(&units);
            for u in &units {
                if seen.insert(*blake3::hash(u).as_bytes()) {
                    c += unit_cost(u);
                }
            }
            c
        };
        costs.push(c);
    }
    Row { first: costs[0], rest: costs[1..].iter().sum(), secs: t.elapsed().as_secs_f64() }
}

fn kb(b: u64) -> String {
    format!("{:.1}", b as f64 / 1024.0)
}

fn main() {
    let dir = std::env::temp_dir().join("inkwell-history-bench");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Copies only: never read the user's files in place.
    let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../samples");
    let copy = |name: &str| {
        let to = dir.join(name);
        std::fs::copy(samples.join(name), &to).unwrap();
        std::fs::read(&to).unwrap()
    };

    let fx = fixture::build(&Fixture { pages: 200, chapter_every: 20, needle_every: 0, lines: 40 });
    let img = copy("compress-source.pdf");
    let n = 10;
    let mut seqs: Vec<(String, Vec<Vec<u8>>)> = Vec::new();
    for (name, base) in [("fixture 200p", &fx), ("compress-source (images)", &img)] {
        seqs.push((format!("{name}: incremental x{n}"), edits(base, n, Save::Incremental)));
        seqs.push((format!("{name}: full rewrite x{n}"), edits(base, n, Save::Full)));
        seqs.push((format!("{name}: rewrite garbage+compress x{n}"), edits(base, n, Save::FullGarbageCompress)));
    }
    let pair = |src: &str, outs: &[&str]| {
        let mut v = vec![copy(src)];
        v.extend(outs.iter().map(|o| copy(o)));
        v
    };
    seqs.push(("Acrobat edit: edit-source -> edit-out-*".into(), pair("edit-source.pdf", &["edit-out-01-typed-ZZ.pdf", "edit-out-02-addtext.pdf"])));
    seqs.push((
        "Acrobat redact: redact-source -> red-out-*".into(),
        pair("redact-source.pdf", &["red-out-01-apply-nosanitize.pdf", "red-out-02-apply-sanitize.pdf", "red-out-03-sanitize-only.pdf"]),
    ));
    let opt: Vec<String> = std::fs::read_dir(&samples)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("opt-out-"))
        .collect();
    let opt: Vec<&str> = opt.iter().map(String::as_str).collect();
    seqs.push(("Acrobat optimize: opt-in-01 -> opt-out-*".into(), pair("opt-in-01.pdf", &opt)));
    seqs.push(("Acrobat form: form-source -> frm-out-*".into(), pair("form-source.pdf", &["frm-out-01-autodetect.pdf"])));

    println!("Bytes stored: first version (KiB) / mean per later version (KiB). Logical = mean file size.\n");
    println!("| sequence | logical KiB | CDC 16 KiB | CDC 64 KiB | object-level | whole-file zstd |");
    println!("|---|---|---|---|---|---|");
    let mut totals = [0u64; 4];
    let mut secs = [0f64; 4];
    let mut mb = 0f64;
    for (name, seq) in &seqs {
        let later = (seq.len() - 1) as u64;
        let rows = [
            run(seq, |b| cdc(b, 16 << 10), false),
            run(seq, |b| cdc(b, 64 << 10), false),
            run(seq, objects, false),
            run(seq, |_| vec![], true),
        ];
        let logical = seq.iter().map(|v| v.len() as u64).sum::<u64>() / seq.len() as u64;
        mb += seq.iter().map(|v| v.len() as f64).sum::<f64>() / 1e6;
        let cells: Vec<String> = rows.iter().map(|r| format!("{} / {}", kb(r.first), kb(r.rest / later))).collect();
        println!("| {name} | {} | {} |", kb(logical), cells.join(" | "));
        for (i, r) in rows.iter().enumerate() {
            totals[i] += r.first + r.rest;
            secs[i] += r.secs;
        }
    }
    println!("| **total stored, all sequences (KiB)** | | {} |", totals.map(kb).join(" | "));
    println!(
        "| split+hash+compress, ms per MB | | {} |",
        secs.map(|s| format!("{:.1}", s * 1000.0 / mb)).join(" | ")
    );

    // The real store (FastCDC default, encryption, fsync) on the first two sequences of each base.
    println!("\nReal store commit (default chunking, XChaCha20-Poly1305, temp+fsync+rename):\n");
    println!("| sequence | stored after first (KiB/version) | commit ms per MB |");
    println!("|---|---|---|");
    for (name, seq) in seqs.iter().take(6) {
        let root = dir.join("store").join(name.len().to_string() + &seq[0].len().to_string());
        let src = dir.join("src.pdf");
        std::fs::write(&src, &seq[0]).unwrap();
        let s = Store::open(&root).unwrap();
        let doc = s.attach(&src).unwrap();
        let first = s.usage().unwrap().total;
        let t = Instant::now();
        for b in &seq[1..] {
            let meta = NewVersion {
                author: Author::User,
                kind: Kind::Save,
                label: "Save".into(),
                summary: vec![],
                name: None,
                pages_touched: vec![],
            };
            s.commit(doc, b, meta).unwrap();
        }
        let el = t.elapsed().as_secs_f64();
        let later_mb = seq[1..].iter().map(|v| v.len() as f64).sum::<f64>() / 1e6;
        let per = (s.usage().unwrap().total - first) / (seq.len() - 1) as u64;
        println!("| {name} | {} | {:.1} |", kb(per), el * 1000.0 / later_mb);
    }
}
