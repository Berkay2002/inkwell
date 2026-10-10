//! OCR benchmark: `ocrbench <windows|tesseract> <lang> <dpi> <runs> <workers> <out.json> <pdf>...`
//! Prints ms/page and writes the recognised words of every file as JSON for scripts/ocr-bench/score.py.

use inkwell_lib::ocr::backend::{Recognizer, TesseractCli, WinRtOcr};
use inkwell_lib::ocr::{Input, Options, recognize};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 8 {
        eprintln!("usage: ocrbench <windows|tesseract> <lang|-> <dpi> <runs> <workers> <out.json> <pdf>...");
        std::process::exit(2);
    }
    let backend: Box<dyn Recognizer> = match a[1].as_str() {
        "windows" => Box::new(WinRtOcr),
        "tesseract" => Box::new(TesseractCli::discover(None).expect("tesseract not found")),
        b => panic!("unknown backend {b}"),
    };
    let langs: Vec<String> = if a[2] == "-" { Vec::new() } else { a[2].split('+').map(String::from).collect() };
    let dpi: u32 = a[3].parse().unwrap();
    let runs: usize = a[4].parse().unwrap();
    let workers: usize = a[5].parse().unwrap();
    let mut results = Vec::new();
    for pdf in &a[7..] {
        let opts = Options { languages: langs.clone(), dpi, force: true, workers, ..Options::default() };
        let cancel = AtomicBool::new(false);
        let mut ms = Vec::new();
        let mut last = None;
        for _ in 0..runs.max(1) + 1 {
            let rec = recognize(&Input::Path(Path::new(pdf)), backend.as_ref(), &opts, &|_| {}, &cancel).unwrap();
            ms.push(rec.total_ms as f64 / rec.pages.len().max(1) as f64);
            last = Some(rec);
        }
        ms.remove(0); // warm-up
        ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = ms[ms.len() / 2];
        let rec = last.unwrap();
        let name = Path::new(pdf).file_stem().unwrap().to_string_lossy().into_owned();
        let words: usize = rec.pages.iter().map(|p| p.lines.iter().map(|l| l.len()).sum::<usize>()).sum();
        println!("{name:<16} {} {:>7.0} ms/page  {words} words  skew {:?}", backend.name(), med, rec.pages[0].skew_deg);
        results.push(json!({
            "case": name, "backend": backend.name(), "lang": a[2], "dpi": dpi, "ms_per_page": med,
            "pages": rec.pages.iter().map(|p| json!({"page": p.page, "status": p.status, "skew": p.skew_deg, "lines": p.words_on_page()})).collect::<Vec<_>>(),
        }));
    }
    std::fs::write(&a[6], serde_json::to_vec(&results).unwrap()).unwrap();
}
