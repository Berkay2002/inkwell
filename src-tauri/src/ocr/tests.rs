//! OCR tests. The writer, skipping, cancel and coordinate tests use a stub recognizer with a fixed
//! word list so they do not depend on recognition accuracy; the end-to-end tests run the real
//! backends on scans rendered in-test from vector text (no binaries in the repo).

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mupdf::pdf::PdfDocument;
use mupdf::{Quad, TextPageFlags};

use super::backend::{Bitmap, RawLine, RawPage, RawWord, Recognizer, TesseractCli};
use super::order::reading_order;
#[cfg(windows)]
use super::backend::WinRtOcr;
use super::*;

// ---------------------------------------------------------------------------------------------
// PDF builders
// ---------------------------------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)")
}

enum Pg {
    /// Vector text, Helvetica 14 pt, one line per entry starting at (72, 720), 30 pt apart.
    Text(Vec<String>),
    /// Image-only page: grey bitmap placed over the whole `w` x `h` pt page.
    Scan { w: f32, h: f32, bmp: Bitmap, rotate: i32, sloppy_cm: bool },
}

fn build(pages: &[Pg]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offs: Vec<usize> = Vec::new();
    let first = 4usize; // 1 catalog, 2 pages, 3 font
    let mut next = first;
    let mut kids = String::new();
    let mut objs: Vec<(usize, Vec<u8>)> = Vec::new();
    for p in pages {
        let page_no = next;
        kids.push_str(&format!("{page_no} 0 R "));
        match p {
            Pg::Text(lines) => {
                let mut c = String::from("BT /F1 14 Tf\n");
                for (i, l) in lines.iter().enumerate() {
                    c.push_str(&format!("1 0 0 1 72 {} Tm ({}) Tj\n", 720 - 30 * i, esc(l)));
                }
                c.push_str("ET\n");
                objs.push((
                    page_no,
                    format!(
                        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                        page_no + 1
                    )
                    .into_bytes(),
                ));
                objs.push((page_no + 1, stream(&format!("<< /Length {} >>", c.len()), c.as_bytes())));
                next += 2;
            }
            Pg::Scan { w, h, bmp, rotate, sloppy_cm } => {
                let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
                z.write_all(&bmp.pixels).unwrap();
                let z = z.finish().unwrap();
                // A scanner-style stream: sets the CTM and never restores it.
                let c = if *sloppy_cm { format!("{w} 0 0 {h} 0 0 cm /Im0 Do\n") } else { format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q\n") };
                objs.push((
                    page_no,
                    format!(
                        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Rotate {rotate} \
                         /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                        page_no + 1,
                        page_no + 2
                    )
                    .into_bytes(),
                ));
                objs.push((
                    page_no + 1,
                    stream(
                        &format!(
                            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray \
                             /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>",
                            bmp.w,
                            bmp.h,
                            z.len()
                        ),
                        &z,
                    ),
                ));
                objs.push((page_no + 2, stream(&format!("<< /Length {} >>", c.len()), c.as_bytes())));
                next += 3;
            }
        }
    }
    let mut all: Vec<(usize, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes()),
        (3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()),
    ];
    all.extend(objs);
    for (n, body) in &all {
        while offs.len() < *n {
            offs.push(0);
        }
        offs[*n - 1] = out.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offs.len() + 1).as_bytes());
    for o in &offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offs.len() + 1).as_bytes());
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = dict.as_bytes().to_vec();
    v.extend_from_slice(b"\nstream\n");
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

fn scan_bitmap(lines: &[&str], dpi: u32) -> Bitmap {
    let text = build(&[Pg::Text(lines.iter().map(|s| s.to_string()).collect())]);
    let doc = PdfDocument::from_bytes(&text).unwrap();
    render(&doc, 0, dpi, 20000).unwrap().0
}

/// Rasterises one line set at `dpi` and wraps it as a scan page (no text in the result).
fn scan_of(lines: &[&str], dpi: u32) -> Pg {
    Pg::Scan { w: 612.0, h: 792.0, bmp: scan_bitmap(lines, dpi), rotate: 0, sloppy_cm: false }
}

/// The same page fed to the scanner turned clockwise by `deg`: a landscape (or upside-down) image.
fn turned_scan_of(lines: &[&str], dpi: u32, deg: u16) -> Pg {
    let (w, h) = if deg % 180 == 90 { (792.0, 612.0) } else { (612.0, 792.0) };
    Pg::Scan { w, h, bmp: scan_bitmap(lines, dpi).rotated(deg), rotate: 0, sloppy_cm: false }
}

fn write_tmp(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join("inkwell-ocr-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn page_text(path: &std::path::Path, page: i32) -> String {
    let doc = PdfDocument::open(path.to_string_lossy().as_ref()).unwrap();
    let p = doc.load_page(page).unwrap();
    p.to_text_page(TextPageFlags::empty()).unwrap().to_text().unwrap()
}

fn search(path: &std::path::Path, page: i32, needle: &str) -> Vec<Quad> {
    let doc = PdfDocument::open(path.to_string_lossy().as_ref()).unwrap();
    doc.load_page(page).unwrap().search(needle, 16).unwrap().iter().cloned().collect()
}

fn centre(q: &Quad) -> (f32, f32) {
    ((q.ul.x + q.ur.x + q.ll.x + q.lr.x) / 4.0, (q.ul.y + q.ur.y + q.ll.y + q.lr.y) / 4.0)
}

// ---------------------------------------------------------------------------------------------
// Stub recognizer
// ---------------------------------------------------------------------------------------------

struct Stub {
    /// (text, [x0, y0, x1, y1]) in bitmap pixels; one line per entry of `lines`.
    lines: Vec<Vec<(&'static str, [f32; 4])>>,
    angle: Option<f32>,
    /// What `osd_rotation` answers (a stand-in for Tesseract's detector).
    osd: Option<u16>,
    delay: Duration,
    calls: AtomicUsize,
}

impl Stub {
    fn new(lines: Vec<Vec<(&'static str, [f32; 4])>>) -> Self {
        Self { lines, angle: None, osd: None, delay: Duration::ZERO, calls: AtomicUsize::new(0) }
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
    fn osd_rotation(&self, _bmp: &Bitmap) -> Option<u16> {
        self.osd
    }
    fn recognize(&self, _bmp: &Bitmap, _languages: &[String]) -> Result<RawPage, OcrError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        Ok(RawPage {
            lines: self
                .lines
                .iter()
                .map(|l| RawLine { words: l.iter().map(|(t, r)| RawWord { text: t.to_string(), rect: *r }).collect() })
                .collect(),
            angle_deg: self.angle,
        })
    }
}

/// Three words on one line, then one on a second; coordinates are points because tests use dpi 72.
fn stub_words() -> Vec<Vec<(&'static str, [f32; 4])>> {
    vec![
        vec![("Invoice", [72.0, 100.0, 120.0, 114.0]), ("Total:", [126.0, 100.0, 166.0, 114.0]), ("$1,234.56", [172.0, 100.0, 240.0, 114.0])],
        vec![("jane.tester@example.com", [72.0, 140.0, 250.0, 154.0]), ("Åke", [260.0, 140.0, 290.0, 154.0])],
    ]
}

fn opts72() -> Options {
    Options { dpi: 72, workers: 2, detect_orientation: false, ..Options::default() }
}

fn no_progress(_: Progress) {}

fn run_stub(pages: &[Pg], stub: &Stub, opts: &Options, name: &str) -> (PathBuf, Report) {
    let input = write_tmp(&format!("{name}-in.pdf"), &build(pages));
    let output = std::env::temp_dir().join("inkwell-ocr-tests").join(format!("{name}-out.pdf"));
    let _ = std::fs::remove_file(&output);
    let cancel = AtomicBool::new(false);
    let report = ocr_file(&input, &output, stub, opts, &no_progress, &cancel).unwrap();
    (output, report)
}

fn blank_scan(w: f32, h: f32) -> Pg {
    Pg::Scan { w, h, bmp: Bitmap { w: 8, h: 8, pixels: vec![255; 64] }, rotate: 0, sloppy_cm: false }
}

// ---------------------------------------------------------------------------------------------
// Writer, coordinates, spaces (stub)
// ---------------------------------------------------------------------------------------------

#[test]
fn text_has_spaces_and_whole_tokens() {
    let (out, rep) = run_stub(&[blank_scan(612.0, 792.0)], &Stub::new(stub_words()), &opts72(), "spaces");
    assert_eq!(rep.pages_recognized, 1);
    assert_eq!(rep.words, 5);
    let t = page_text(&out, 0);
    // Reading order, real spaces, `$1,234.56` stays one token (Acrobat wrote `$1 ,234.56`).
    let lines: Vec<_> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    assert_eq!(lines, ["Invoice Total: $1,234.56", "jane.tester@example.com Åke"], "{t:?}");
    assert_eq!(search(&out, 0, "$1,234.56").len(), 1);
    assert_eq!(search(&out, 0, "Åke").len(), 1);
}

#[test]
fn search_hits_sit_on_the_word_boxes() {
    let (out, _) = run_stub(&[blank_scan(612.0, 792.0)], &Stub::new(stub_words()), &opts72(), "positions");
    for (needle, b) in [("Invoice", [72.0, 100.0, 120.0, 114.0]), ("Total:", [126.0, 100.0, 166.0, 114.0]), ("jane.tester@example.com", [72.0, 140.0, 250.0, 154.0])] {
        let hits = search(&out, 0, needle);
        assert_eq!(hits.len(), 1, "{needle}");
        let (cx, cy) = centre(&hits[0]);
        let (ex, ey) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        assert!((cx - ex).abs() < 3.0 && (cy - ey).abs() < 4.0, "{needle}: hit centre ({cx},{cy}) vs box ({ex},{ey})");
    }
}

#[test]
fn page_boxes_and_image_bytes_are_untouched() {
    let scan = scan_of(&["Image bytes must survive", "the text layer is appended"], 150);
    let input_bytes = build(&[scan]);
    let input = write_tmp("bytes-in.pdf", &input_bytes);
    let output = input.with_file_name("bytes-out.pdf");
    let stub = Stub::new(stub_words());
    let opts = Options { dpi: 72, workers: 1, detect_orientation: false, ..Options::default() };
    ocr_file(&input, &output, &stub, &opts, &no_progress, &AtomicBool::new(false)).unwrap();
    let out_bytes = std::fs::read(&output).unwrap();
    // The incremental update leaves every original byte (image stream included) where it was.
    assert!(out_bytes.starts_with(&input_bytes), "output must append to the input file");
    assert!(out_bytes.len() > input_bytes.len());

    let a = PdfDocument::open(input.to_string_lossy().as_ref()).unwrap();
    let b = PdfDocument::open(output.to_string_lossy().as_ref()).unwrap();
    let (pa, pb) = (a.load_pdf_page(0).unwrap(), b.load_pdf_page(0).unwrap());
    assert_eq!(pa.media_box().unwrap(), pb.media_box().unwrap());
    assert_eq!(pa.crop_box().unwrap(), pb.crop_box().unwrap());
    let raw = |p: &mupdf::pdf::PdfPage| {
        let im = p.resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap();
        (im.as_indirect().unwrap(), im.read_raw_stream().unwrap())
    };
    assert_eq!(raw(&pa), raw(&pb));
}

#[test]
fn layer_ignores_a_ctm_the_scanner_left_behind() {
    let mut sloppy = blank_scan(612.0, 792.0);
    if let Pg::Scan { sloppy_cm, .. } = &mut sloppy {
        *sloppy_cm = true;
    }
    let (out, _) = run_stub(&[sloppy], &Stub::new(stub_words()), &opts72(), "sloppy");
    let hits = search(&out, 0, "Invoice");
    assert_eq!(hits.len(), 1);
    let (cx, cy) = centre(&hits[0]);
    assert!((cx - 96.0).abs() < 3.0 && (cy - 107.0).abs() < 4.0, "({cx},{cy})");
}

#[test]
fn rotated_pages_get_words_where_the_viewer_shows_them() {
    // /Rotate 90 on a 400 x 300 pt page: displayed 300 x 400. The stub's boxes are in displayed space.
    let mut p = blank_scan(400.0, 300.0);
    if let Pg::Scan { rotate, .. } = &mut p {
        *rotate = 90;
    }
    let (out, _) = run_stub(&[p], &Stub::new(stub_words()), &opts72(), "rotated");
    let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    let b = doc.load_page(0).unwrap().bounds().unwrap();
    assert_eq!((b.x1 - b.x0, b.y1 - b.y0), (300.0, 400.0));
    let hits = search(&out, 0, "Invoice");
    assert_eq!(hits.len(), 1);
    let (cx, cy) = centre(&hits[0]);
    assert!((cx - 96.0).abs() < 3.0 && (cy - 107.0).abs() < 4.0, "({cx},{cy})");
    assert_eq!(page_text(&out, 0).split_whitespace().count(), 5);
}

#[test]
fn skewed_pages_follow_the_engine_angle() {
    let mut stub = Stub::new(stub_words());
    stub.angle = Some(2.0);
    let (out, rep) = run_stub(&[blank_scan(612.0, 792.0)], &stub, &opts72(), "skew");
    assert_eq!(rep.pages[0].skew_deg, Some(2.0));
    // Text frame -> page: rotate about the page centre by +2 degrees (y down, clockwise).
    let (s, c) = 2.0f32.to_radians().sin_cos();
    let (dx, dy) = (96.0 - 306.0, 107.0 - 396.0);
    let (ex, ey) = (306.0 + dx * c - dy * s, 396.0 + dx * s + dy * c);
    let hits = search(&out, 0, "Invoice");
    assert_eq!(hits.len(), 1);
    let (cx, cy) = centre(&hits[0]);
    assert!((cx - ex).abs() < 3.0 && (cy - ey).abs() < 4.0, "({cx},{cy}) vs ({ex},{ey})");
}

#[test]
fn pages_with_text_are_skipped_unless_forced() {
    let pages = [Pg::Text(vec!["A page that already has plenty of real text on it".into()]), blank_scan(612.0, 792.0)];
    let stub = Stub::new(stub_words());
    let (out, rep) = run_stub(&pages, &stub, &opts72(), "skip");
    assert_eq!((rep.pages_skipped_text, rep.pages_recognized), (1, 1));
    assert_eq!(stub.calls.load(Ordering::SeqCst), 1);
    assert!(page_text(&out, 0).contains("plenty of real text") && !page_text(&out, 0).contains("Invoice"));
    assert!(page_text(&out, 1).contains("Invoice"));

    let stub = Stub::new(stub_words());
    let forced = Options { force: true, ..opts72() };
    let (out, rep) = run_stub(&pages, &stub, &forced, "skip-forced");
    assert_eq!(rep.pages_recognized, 2);
    assert!(page_text(&out, 0).contains("Invoice"));
}

#[test]
fn page_range_limits_the_work() {
    let pages: Vec<Pg> = (0..5).map(|_| blank_scan(612.0, 792.0)).collect();
    let stub = Stub::new(stub_words());
    let opts = Options { pages: Some(parse_page_range("2,4-", 5).unwrap()), ..opts72() };
    let (out, rep) = run_stub(&pages, &stub, &opts, "range");
    assert_eq!(rep.pages_recognized, 3);
    assert_eq!(rep.pages.iter().map(|p| p.page).collect::<Vec<_>>(), [1, 3, 4]);
    for (i, want) in [false, true, false, true, true].into_iter().enumerate() {
        assert_eq!(page_text(&out, i as i32).contains("Invoice"), want, "page {i}");
    }
    assert!(parse_page_range("0", 5).is_err() && parse_page_range("3-9", 5).is_err() && parse_page_range("x", 5).is_err());
}

#[test]
fn forcing_again_replaces_our_layer() {
    let pages = [blank_scan(612.0, 792.0)];
    let stub = Stub::new(stub_words());
    let forced = Options { force: true, workers: 1, ..opts72() };
    let (first, _) = run_stub(&pages, &stub, &forced, "twice-a");
    let second = first.with_file_name("twice-b.pdf");
    let rep = ocr_file(&first, &second, &stub, &forced, &no_progress, &AtomicBool::new(false)).unwrap();
    assert_eq!(rep.pages_recognized, 1);
    assert_eq!(search(&second, 0, "Invoice").len(), 1, "a second layer would double every hit");
}

#[test]
fn a_hundred_pages_stay_cancellable_and_report_progress() {
    let pages: Vec<Pg> = (0..100).map(|_| blank_scan(612.0, 792.0)).collect();
    let input = write_tmp("hundred-in.pdf", &build(&pages));
    let output = input.with_file_name("hundred-out.pdf");
    let _ = std::fs::remove_file(&output);
    let mut stub = Stub::new(stub_words());
    stub.delay = Duration::from_millis(40);
    let opts = Options { workers: 4, ..opts72() };
    let cancel = Arc::new(AtomicBool::new(false));
    let seen: Arc<Mutex<Vec<u32>>> = Arc::default();
    let t = Instant::now();
    let r = {
        let (cancel2, seen2) = (cancel.clone(), seen.clone());
        let progress = move |p: Progress| {
            seen2.lock().unwrap().push(p.done);
            assert_eq!(p.total, 100);
            if p.done == 10 {
                cancel2.store(true, Ordering::Relaxed);
            }
        };
        ocr_file(&input, &output, &stub, &opts, &progress, &cancel)
    };
    assert!(matches!(r, Err(OcrError::Cancelled)));
    // Four workers finish the pages they are on (at most 4 more), nothing else starts.
    let calls = stub.calls.load(Ordering::SeqCst);
    assert!((10..=14).contains(&calls), "calls = {calls}");
    assert!(t.elapsed() < Duration::from_secs(2), "cancel took {:?}", t.elapsed());
    assert!(!output.exists(), "a cancelled run must not leave an output");
    assert!(!output.with_extension("ocr-tmp").exists());
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), calls);
}

#[test]
fn a_full_hundred_pages_finish_with_parallel_workers() {
    let pages: Vec<Pg> = (0..100).map(|_| blank_scan(612.0, 792.0)).collect();
    let mut stub = Stub::new(stub_words());
    stub.delay = Duration::from_millis(20);
    let t = Instant::now();
    let (out, rep) = run_stub(&pages, &stub, &Options { workers: 4, ..opts72() }, "hundred-full");
    assert_eq!(rep.pages_recognized, 100);
    // 100 x 20 ms is 2 s serially.
    assert!(t.elapsed() < Duration::from_millis(1800), "{:?}", t.elapsed());
    let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    assert_eq!(doc.page_count().unwrap(), 100);
    assert!(page_text(&out, 99).contains("jane.tester@example.com"));
}

#[test]
fn output_equals_input_is_refused_and_bad_pages_are_reported() {
    let input = write_tmp("same.pdf", &build(&[blank_scan(612.0, 792.0)]));
    let stub = Stub::new(stub_words());
    let r = ocr_file(&input, &input, &stub, &opts72(), &no_progress, &AtomicBool::new(false));
    assert!(matches!(r, Err(OcrError::Io(_))));
    let o = Options { pages: Some(vec![7]), ..opts72() };
    let r = recognize(&Input::Path(&input), &stub, &o, &no_progress, &AtomicBool::new(false));
    assert!(matches!(r, Err(OcrError::BadPage(7))));
}

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

#[test]
fn page_ranges_parse() {
    assert_eq!(parse_page_range("1-3, 5 ;8-", 9).unwrap(), [0, 1, 2, 4, 7, 8]);
    assert_eq!(parse_page_range("-2", 9).unwrap(), [0, 1]);
    assert_eq!(parse_page_range("3,3,1", 9).unwrap(), [0, 2]);
}

#[test]
fn identifiers_keep_their_ascii_hyphens() {
    assert_eq!(normalize("INV\u{2014}2026\u{2014}0042".into()), "INV-2026-0042");
    assert_eq!(normalize("A\u{2212}7".into()), "A-7");
    assert_eq!(normalize("word\u{2014}word".into()), "word\u{2014}word"); // prose em dash stays
}

#[test]
fn tesseract_tsv_becomes_lines() {
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
        1\t1\t0\t0\t0\t0\t0\t0\t100\t100\t-1\t\n\
        5\t1\t1\t1\t1\t1\t10\t20\t30\t12\t96.5\tHello\n\
        5\t1\t1\t1\t1\t2\t45\t20\t40\t12\t91.0\t$1,234.56\n\
        5\t1\t1\t1\t2\t1\t10\t40\t30\t12\t90.0\tnext\n";
    let p = backend::parse_tsv(tsv);
    assert_eq!(p.lines.len(), 2);
    assert_eq!(p.lines[0].words[1].text, "$1,234.56");
    assert_eq!(p.lines[0].words[1].rect, [45.0, 20.0, 85.0, 32.0]);
}

// ---------------------------------------------------------------------------------------------
// Real backends, end to end
// ---------------------------------------------------------------------------------------------

const SCAN_LINES: [&str; 5] = [
    "Scanned document test page.",
    "The quick brown fox jumps over the lazy dog.",
    "Invoice 20260042 Total: $1,234.56",
    "Contact: jane.tester@example.com",
    "Arsredovisning for foretaget",
];

fn real_backends() -> Vec<(Box<dyn Recognizer>, &'static str)> {
    let mut v: Vec<(Box<dyn Recognizer>, &'static str)> = Vec::new();
    #[cfg(windows)]
    if WinRtOcr.languages().map(|l| l.iter().any(|t| t.starts_with("en"))).unwrap_or(false) {
        v.push((Box::new(WinRtOcr), "en-US"));
    }
    if let Some(t) = TesseractCli::discover(None) {
        if t.languages().map(|l| l.iter().any(|x| x == "en")).unwrap_or(false) {
            v.push((Box::new(t), "en"));
        }
    }
    v
}

#[test]
fn real_backends_make_a_scan_searchable() {
    let backends = real_backends();
    if backends.is_empty() {
        eprintln!("skipped: no OCR backend available");
        return;
    }
    for (b, lang) in backends {
        let name = format!("e2e-{}", b.name());
        let input = write_tmp(&format!("{name}-in.pdf"), &build(&[scan_of(&SCAN_LINES, 300)]));
        let output = input.with_file_name(format!("{name}-out.pdf"));
        let _ = std::fs::remove_file(&output);
        let opts = Options { languages: vec![lang.into()], workers: 1, ..Options::default() };
        let rep = ocr_file(&input, &output, b.as_ref(), &opts, &no_progress, &AtomicBool::new(false)).unwrap();
        assert_eq!(rep.pages_recognized, 1, "{}", b.name());

        let lines: Vec<String> = page_text(&output, 0).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        assert_eq!(&lines[..4], &SCAN_LINES[..4], "{}: text must extract in reading order with correct spaces", b.name());
        assert_eq!(search(&output, 0, "$1,234.56").len(), 1, "{}", b.name());
        // The "quick" hit lands where the vector original has it.
        let truth = {
            let src = write_tmp("e2e-src.pdf", &build(&[Pg::Text(SCAN_LINES.iter().map(|s| s.to_string()).collect())]));
            search(&src, 0, "quick")
        };
        let got = search(&output, 0, "quick");
        assert_eq!((got.len(), truth.len()), (1, 1));
        let (a, c) = (centre(&got[0]), centre(&truth[0]));
        assert!((a.0 - c.0).abs() < 6.0 && (a.1 - c.1).abs() < 8.0, "{}: {a:?} vs {c:?}", b.name());
    }
}

/// The study's one-page scan: what Acrobat 26 produced is the reference.
#[test]
fn study_sample_compared_with_acrobat() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/study/samples");
    let (scan, acrobat) = (dir.join("ocr-in-01.pdf"), dir.join("ocr-out-01-english.pdf"));
    assert!(scan.exists() && acrobat.exists(), "study samples are committed; missing: {dir:?}");
    for (b, lang) in real_backends() {
        let out = std::env::temp_dir().join("inkwell-ocr-tests").join(format!("study-{}.pdf", b.name()));
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        let opts = Options { languages: vec![lang.into()], workers: 1, ..Options::default() };
        ocr_file(&scan, &out, b.as_ref(), &opts, &no_progress, &AtomicBool::new(false)).unwrap();
        let ours = page_text(&out, 0);
        let theirs = page_text(&acrobat, 0);
        eprintln!("[{}] ours   : {:?}", b.name(), ours.split_whitespace().collect::<Vec<_>>().join(" "));
        eprintln!("[{}] acrobat: {:?}", b.name(), theirs.split_whitespace().collect::<Vec<_>>().join(" "));
        assert!(ours.contains("$1,234.56"), "{}: token kept whole", b.name());
        assert!(ours.contains("jane.tester@example.com") && ours.contains("20260042"));
        // Same words as Acrobat apart from its broken `$1 ,234.56`.
        let norm = |s: &str| s.replace("$1 ,", "$1,").split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(norm(&ours), norm(&theirs), "{}", b.name());
        // Image stream is the scan's original bytes (Acrobat re-encoded it and grew the page).
        let orig = std::fs::read(&scan).unwrap();
        assert!(std::fs::read(&out).unwrap().starts_with(&orig));
    }
}

// ---------------------------------------------------------------------------------------------
// Orientation
// ---------------------------------------------------------------------------------------------

#[test]
fn bitmap_rotation_is_clockwise() {
    // 3 x 2 image: a b c / d e f
    let b = Bitmap { w: 3, h: 2, pixels: vec![1, 2, 3, 4, 5, 6] };
    assert_eq!((b.rotated(90).w, b.rotated(90).h, b.rotated(90).pixels), (2, 3, vec![4, 1, 5, 2, 6, 3]));
    assert_eq!(b.rotated(180).pixels, vec![6, 5, 4, 3, 2, 1]);
    assert_eq!((b.rotated(270).w, b.rotated(270).pixels), (2, vec![3, 6, 2, 5, 1, 4]));
    assert_eq!(b.rotated(90).rotated(270).pixels, b.pixels);
}

#[test]
fn a_sideways_scan_gets_its_words_mapped_back_to_the_page() {
    // Landscape 400 x 300 pt image whose text must be turned 90 degrees clockwise to read.
    // The stub reads the turned raster (300 x 400) and reports words in that frame.
    let mut stub = Stub::new(stub_words());
    stub.osd = Some(90);
    let opts = Options { detect_orientation: true, ..opts72() };
    let (out, rep) = run_stub(&[blank_scan(400.0, 300.0)], &stub, &opts, "sideways");
    assert_eq!(rep.pages[0].rotation, 90);
    assert_eq!(stub.calls.load(Ordering::SeqCst), 2, "one read to find out, one turned");
    // Turned frame (x, y) -> page (y, h - x) with h = 300: "Invoice" centre (96, 107) -> (107, 204).
    let hits = search(&out, 0, "Invoice");
    assert_eq!(hits.len(), 1);
    let (cx, cy) = centre(&hits[0]);
    assert!((cx - 107.0).abs() < 4.0 && (cy - 204.0).abs() < 8.0, "({cx},{cy})");
    // The page itself is untouched.
    let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    let b = doc.load_page(0).unwrap().bounds().unwrap();
    assert_eq!((b.x1 - b.x0, b.y1 - b.y0), (400.0, 300.0));
    assert_eq!(page_text(&out, 0).split_whitespace().count(), 5);
}

#[test]
fn an_upright_page_is_not_turned() {
    let stub = Stub::new(stub_words());
    let opts = Options { detect_orientation: true, ..opts72() };
    let (_, rep) = run_stub(&[blank_scan(612.0, 792.0)], &stub, &opts, "upright-detect");
    assert_eq!(rep.pages[0].rotation, 0);
}

#[test]
fn osd_output_is_parsed() {
    let osd = "Page number: 0\nOrientation in degrees: 90\nRotate: 270\nOrientation confidence: 11.26\n";
    assert_eq!(backend::parse_osd(osd), Some(270));
    assert_eq!(backend::parse_osd("Rotate: 90\nOrientation confidence: 0.4\n"), None);
    assert_eq!(backend::parse_osd("Too few characters. Skipping this page"), None);
}

#[test]
fn real_backends_read_scans_fed_sideways() {
    let backends = real_backends();
    if backends.is_empty() {
        eprintln!("skipped: no OCR backend available");
        return;
    }
    // "quick" on the upright 612 x 792 page, from the vector original.
    let truth = {
        let src = write_tmp("turn-src.pdf", &build(&[Pg::Text(SCAN_LINES.iter().map(|s| s.to_string()).collect())]));
        centre(&search(&src, 0, "quick")[0])
    };
    for (b, lang) in backends {
        for deg in [90u16, 180, 270] {
            let name = format!("turn{deg}-{}", b.name());
            let input = write_tmp(&format!("{name}-in.pdf"), &build(&[turned_scan_of(&SCAN_LINES, 200, deg)]));
            let output = input.with_file_name(format!("{name}-out.pdf"));
            let _ = std::fs::remove_file(&output);
            let opts = Options { languages: vec![lang.into()], workers: 1, ..Options::default() };
            let rep = ocr_file(&input, &output, b.as_ref(), &opts, &no_progress, &AtomicBool::new(false)).unwrap();
            eprintln!("{} fed turned {deg}: rotation {} in {} ms", b.name(), rep.pages[0].rotation, rep.pages[0].ms);
            // The scan was turned clockwise by `deg`; the engine's turn undoes it.
            assert_eq!(rep.pages[0].rotation, 360 - deg, "{} {deg}", b.name());
            let words: Vec<String> = page_text(&output, 0).split_whitespace().map(String::from).collect();
            for w in ["quick", "$1,234.56", "jane.tester@example.com", "Scanned"] {
                assert!(words.iter().any(|x| x == w), "{} {deg}: {w} missing from {words:?}", b.name());
            }
            let (w, h) = (612.0, 792.0);
            let expect = match deg {
                90 => (h - truth.1, truth.0),
                180 => (w - truth.0, h - truth.1),
                _ => (truth.1, w - truth.0),
            };
            let hits = search(&output, 0, "quick");
            assert_eq!(hits.len(), 1, "{} {deg}", b.name());
            let c = centre(&hits[0]);
            assert!((c.0 - expect.0).abs() < 8.0 && (c.1 - expect.1).abs() < 8.0, "{} {deg}: {c:?} vs {expect:?}", b.name());
            // Words in reading order of the upright page.
            let want: Vec<&str> = SCAN_LINES[..4].iter().flat_map(|l| l.split_whitespace()).collect();
            let got: Vec<&str> = words.iter().map(String::as_str).collect();
            assert_eq!(&got[..want.len()], &want[..], "{} {deg}", b.name());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Reading order
// ---------------------------------------------------------------------------------------------

fn w(text: &str, x: f32, y: f32) -> Word {
    Word { text: text.into(), bbox: [x, y, x + 6.0 * text.len() as f32, y + 10.0] }
}

fn texts(lines: &[Vec<Word>]) -> Vec<String> {
    lines.iter().map(|l| l.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")).collect()
}

#[test]
fn a_table_the_engine_returns_by_column_is_read_by_row() {
    // What Windows OCR gives for a ruled table: one "line" per cell stack, column after column.
    let col = |x: f32, cells: &[&str]| -> Vec<Vec<Word>> {
        cells.iter().enumerate().map(|(i, t)| t.split(' ').enumerate().map(|(j, p)| w(p, x + 40.0 * j as f32, 100.0 + 22.0 * i as f32)).collect()).collect()
    };
    let mut lines = Vec::new();
    lines.extend(col(72.0, &["Item", "A-100", "B-220"]));
    lines.extend(col(216.0, &["Description", "Hex bolt M8", "Steel bracket"]));
    lines.extend(col(360.0, &["Qty", "120", "18"]));
    let got = texts(&reading_order(lines));
    assert_eq!(got, ["Item Description Qty", "A-100 Hex bolt M8 120", "B-220 Steel bracket 18"]);
}

#[test]
fn text_columns_are_read_one_after_the_other() {
    let sentence = |x: f32, y: f32, tag: &str| -> Vec<Word> { (0..8).map(|i| w(&format!("{tag}{i}"), x + 42.0 * i as f32, y)).collect() };
    // Interleaved as a row-by-row engine would return them; a full-width title on top.
    let mut lines = vec![vec![w("Title", 72.0, 60.0), w("of", 120.0, 60.0), w("page", 150.0, 60.0)]];
    for row in 0..4 {
        let y = 100.0 + 14.0 * row as f32;
        lines.push(sentence(72.0, y, &format!("L{row}_")));
        lines.push(sentence(450.0, y, &format!("R{row}_")));
    }
    let got = texts(&reading_order(lines));
    assert_eq!(got[0], "Title of page");
    assert!(got[1].starts_with("L0_0") && got[4].starts_with("L3_0"), "{got:?}");
    assert!(got[5].starts_with("R0_0") && got[8].starts_with("R3_0"), "{got:?}");
}

#[test]
fn a_single_column_keeps_its_line_order() {
    let lines: Vec<Vec<Word>> = (0..6).map(|i| (0..9).map(|j| w(&format!("w{i}{j}"), 72.0 + 40.0 * j as f32, 100.0 + 14.0 * i as f32)).collect()).collect();
    let before = texts(&lines);
    assert_eq!(texts(&reading_order(lines)), before);
}

// ---------------------------------------------------------------------------------------------
// Languages and Tesseract discovery
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn windows_language_priority_falls_through_to_an_installed_pack() {
    let packs = WinRtOcr.languages().unwrap();
    eprintln!("installed Windows OCR packs: {packs:?}");
    assert!(!packs.is_empty());
    let bmp = scan_bitmap(&SCAN_LINES, 200);
    // First requested pack is not installed: the next one is used instead of failing.
    let have = packs.iter().find(|p| p.starts_with("en")).expect("en pack").clone();
    let raw = WinRtOcr.recognize(&bmp, &["xx-XX".to_string(), have]).unwrap();
    assert!(raw.lines.iter().flat_map(|l| &l.words).any(|w| w.text == "quick"));
    assert!(matches!(WinRtOcr.recognize(&bmp, &["xx-XX".to_string()]), Err(OcrError::LanguageUnavailable(_))));
}

#[test]
fn tesseract_is_searched_in_a_fixed_order_and_never_on_path() {
    let env = |k: &str| -> Option<std::ffi::OsString> {
        match k {
            "INKWELL_TESSERACT" => Some(r"D:\env\tesseract.exe".into()),
            "ProgramFiles" => Some(r"C:\Program Files".into()),
            "ProgramFiles(x86)" => Some(r"C:\Program Files (x86)".into()),
            "PATH" => Some(r"C:\evil".into()),
            _ => None,
        }
    };
    let got = TesseractCli::candidates(Some(std::path::Path::new(r"D:\setting\tesseract.exe")), &env);
    let got: Vec<String> = got.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    assert_eq!(
        got,
        [
            r"D:\setting\tesseract.exe",
            r"D:\env\tesseract.exe",
            r"C:\Program Files\Tesseract-OCR\tesseract.exe",
            r"C:\Program Files (x86)\Tesseract-OCR\tesseract.exe",
        ]
    );
    // Nothing configured and no environment: nothing to try (in particular no PATH, no cwd).
    assert!(TesseractCli::candidates(None, &|_| None).is_empty());
    assert!(TesseractCli::candidates(Some(std::path::Path::new("")), &|_| None).is_empty());
}
