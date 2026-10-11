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
    // Latin-1 letters as octal escapes (the page font is WinAnsi); everything else as it is.
    s.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .chars()
        .map(|c| if (0x80..=0xFF).contains(&(c as u32)) { format!("\\{:03o}", c as u32) } else { c.to_string() })
        .collect()
}

enum Pg {
    /// Vector text, Helvetica 14 pt, one line per entry starting at (72, 720), 30 pt apart.
    Text(Vec<String>),
    /// Image-only page: grey bitmap placed over the whole `w` x `h` pt page.
    Scan { w: f32, h: f32, bmp: Bitmap, rotate: i32, sloppy_cm: bool },
    /// A scan stored as JPEG or as 1-bit CCITT G4 (the bitmap is thresholded at 128).
    Enc { w: f32, h: f32, bmp: Bitmap, enc: Enc },
}

#[derive(Clone, Copy)]
enum Enc {
    Jpeg,
    G4,
}

fn build(pages: &[Pg]) -> Vec<u8> {
    build_ext(pages, "", "")
}

/// `catalog_extra` goes into the catalog dictionary, `scan_fonts` into the resources of scan pages.
fn build_ext(pages: &[Pg], catalog_extra: &str, scan_fonts: &str) -> Vec<u8> {
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
                         /Resources << /XObject << /Im0 {} 0 R >> {scan_fonts} >> /Contents {} 0 R >>",
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
            Pg::Enc { w, h, bmp, enc } => {
                let (dict, data) = match enc {
                    Enc::Jpeg => {
                        use image::{ExtendedColorType, ImageEncoder};
                        let mut out = Vec::new();
                        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95)
                            .write_image(&bmp.pixels, bmp.w, bmp.h, ExtendedColorType::L8)
                            .unwrap();
                        ("/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode".to_string(), out)
                    }
                    Enc::G4 => {
                        let (packed, stride) = raster::pack_bilevel(&bmp.pixels, bmp.w as usize, bmp.h as usize, 128);
                        let g4 = mupdf::Buffer::ccitt_g4(&packed, bmp.w, bmp.h, stride).unwrap();
                        (
                            format!(
                                "/ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode \
                                 /DecodeParms << /K -1 /Columns {} /Rows {} >>",
                                bmp.w, bmp.h
                            ),
                            g4,
                        )
                    }
                };
                let c = format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q\n");
                objs.push((
                    page_no,
                    format!(
                        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                        page_no + 1,
                        page_no + 2
                    )
                    .into_bytes(),
                ));
                objs.push((
                    page_no + 1,
                    stream(
                        &format!(
                            "<< /Type /XObject /Subtype /Image /Width {} /Height {} {dict} /Length {} >>",
                            bmp.w,
                            bmp.h,
                            data.len()
                        ),
                        &data,
                    ),
                ));
                objs.push((page_no + 2, stream(&format!("<< /Length {} >>", c.len()), c.as_bytes())));
                next += 3;
            }
        }
    }
    let mut all: Vec<(usize, Vec<u8>)> = vec![
        (1, format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>").into_bytes()),
        (2, format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes()),
        (3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec()),
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
    /// Every recognition fails with this message.
    fail: Option<&'static str>,
    /// Every recognition panics.
    panics: bool,
    /// Words (by text) the stub reports with a low confidence; when set, the others get a high one.
    low: Vec<&'static str>,
}

impl Stub {
    fn new(lines: Vec<Vec<(&'static str, [f32; 4])>>) -> Self {
        Self { lines, angle: None, osd: None, delay: Duration::ZERO, calls: AtomicUsize::new(0), fail: None, panics: false, low: Vec::new() }
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
    fn osd_rotation(&self, _bmp: &Bitmap, _cancel: &AtomicBool) -> Option<u16> {
        self.osd
    }
    fn recognize(&self, _bmp: &Bitmap, _languages: &[String], _cancel: &AtomicBool) -> Result<RawPage, OcrError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        if self.panics {
            panic!("stub recognizer panics");
        }
        if let Some(m) = self.fail {
            return Err(OcrError::Backend(m.into()));
        }
        Ok(RawPage {
            lines: self
                .lines
                .iter()
                .map(|l| RawLine {
                    words: l
                        .iter()
                        .map(|(t, r)| RawWord {
                            text: t.to_string(),
                            rect: *r,
                            conf: if self.low.is_empty() { None } else { Some(if self.low.contains(t) { 30.0 } else { 95.0 }) },
                        })
                        .collect(),
                })
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
    let _ = std::fs::remove_file(&output);
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
    let _ = std::fs::remove_file(&second);
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
// Review fixes: text, files, failures
// ---------------------------------------------------------------------------------------------

/// A scratch directory of its own, so leftovers of one test cannot be blamed on another.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("inkwell-ocr-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn contents_len(path: &std::path::Path, page: i32) -> usize {
    let doc = PdfDocument::open(path.to_string_lossy().as_ref()).unwrap();
    let p = doc.load_pdf_page(page).unwrap();
    p.object().get_dict("Contents").unwrap().unwrap().len().unwrap()
}

#[test]
fn characters_outside_the_bmp_and_rtl_scripts_survive_extraction_and_search() {
    let lines = vec![
        vec![("A\u{1D400}B\u{1F600}", [72.0, 100.0, 160.0, 114.0]), ("\u{E000}\u{1F600}x", [170.0, 100.0, 230.0, 114.0])],
        // Arabic on its own line (marhaba), then a word with digits and Latin letters inside it.
        vec![("\u{645}\u{631}\u{62D}\u{628}\u{627}", [72.0, 140.0, 140.0, 154.0])],
        vec![("\u{645}\u{631}\u{62D}\u{628}\u{627}2024abc", [72.0, 180.0, 200.0, 194.0])],
    ];
    let (out, rep) = run_stub(&[blank_scan(612.0, 792.0)], &Stub::new(lines), &opts72(), "astral");
    assert_eq!(rep.pages_recognized, 1);
    let t = page_text(&out, 0);
    assert!(t.contains("A\u{1D400}B\u{1F600}"), "astral characters were split or lost: {t:?}");
    // A real private-use character keeps its identity next to the codes handed out for emoji.
    assert!(t.contains("\u{E000}\u{1F600}x"), "{t:?}");
    // Logical order comes back out: search and copy of an Arabic word work.
    assert!(t.contains("\u{645}\u{631}\u{62D}\u{628}\u{627}"), "{t:?}");
    // Inside a mixed word the digits and Latin letters stay readable runs (MuPDF returns visual order).
    assert!(t.contains("2024abc"), "{t:?}");
    // MuPDF reports an RTL hit as one quad per letter; they all sit on the two word boxes.
    let hits = search(&out, 0, "\u{645}\u{631}\u{62D}\u{628}\u{627}");
    assert!(!hits.is_empty());
    for h in &hits {
        let (cx, cy) = centre(h);
        let on_word = |y0: f32, x1: f32| (y0..y0 + 14.0).contains(&cy) && (72.0..x1).contains(&cx);
        assert!(on_word(140.0, 140.0) || on_word(180.0, 200.0), "hit at {cx},{cy}");
    }
    assert!(!t.chars().any(|c| c == '\u{FFFD}'), "{t:?}");
    assert_eq!(search(&out, 0, "A\u{1D400}B\u{1F600}").len(), 1);
}

#[test]
fn right_to_left_lines_and_columns_are_read_from_the_right() {
    let ar = |s: &str, x: f32, y: f32| Word { text: s.into(), bbox: [x, y, x + 40.0, y + 10.0], conf: None };
    // One Arabic line, engine order left to right: logical order starts at the rightmost word.
    let line = vec![ar("\u{62B}\u{62B}\u{62B}", 100.0, 100.0), ar("\u{62A}\u{62A}\u{62A}", 160.0, 100.0), ar("\u{627}\u{627}\u{627}", 220.0, 100.0)];
    let out = reading_order(vec![line]);
    assert_eq!(texts(&out), ["\u{627}\u{627}\u{627} \u{62A}\u{62A}\u{62A} \u{62B}\u{62B}\u{62B}"]);
    // Latin lines are unaffected.
    let latin = vec![w("one", 100.0, 100.0), w("two", 160.0, 100.0)];
    assert_eq!(texts(&reading_order(vec![latin])), ["one two"]);
    // Two Arabic columns of three-word lines: the right column first.
    let col = |x: f32, tag: &str| -> Vec<Vec<Word>> {
        (0..4).map(|i| (0..3).map(|j| ar(&format!("{tag}{tag}{i}{j}"), x + 45.0 * j as f32, 100.0 + 14.0 * i as f32)).collect()).collect()
    };
    let out = reading_order([col(60.0, "\u{644}"), col(400.0, "\u{631}")].concat());
    let flat: Vec<String> = out.iter().flatten().map(|wd| wd.text.clone()).collect();
    assert!(flat[0].contains('\u{631}') && flat[flat.len() - 1].contains('\u{644}'), "{flat:?}");
    // Within a right-column line the rightmost word comes first.
    assert_eq!(flat[..3], ["\u{631}\u{631}02", "\u{631}\u{631}01", "\u{631}\u{631}00"], "{flat:?}");
}

#[test]
fn output_is_never_written_over_an_existing_file_or_the_input() {
    let dir = scratch("overwrite");
    let input = dir.join("in.pdf");
    let bytes = build(&[blank_scan(612.0, 792.0)]);
    std::fs::write(&input, &bytes).unwrap();
    let stub = Stub::new(stub_words());
    let go = |out: &std::path::Path| ocr_file(&input, out, &stub, &opts72(), &no_progress, &AtomicBool::new(false));

    // An existing output stays as it was.
    let existing = dir.join("out.pdf");
    std::fs::write(&existing, b"precious").unwrap();
    assert!(matches!(go(&existing), Err(OcrError::OutputExists(_))));
    assert_eq!(std::fs::read(&existing).unwrap(), b"precious");

    // The input by another spelling: `.`/`..` segments, and case on Windows.
    assert!(matches!(go(&dir.join(".").join("in.pdf")), Err(OcrError::Io(_))));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    assert!(matches!(go(&dir.join("sub").join("..").join("in.pdf")), Err(OcrError::Io(_))));
    #[cfg(windows)]
    assert!(matches!(go(&dir.join("IN.PDF")), Err(OcrError::Io(_))));
    assert_eq!(std::fs::read(&input).unwrap(), bytes);

    // Nothing but the two files and the empty directory is left behind.
    assert_eq!(files_in(&dir), ["in.pdf", "out.pdf", "sub"]);
}

#[test]
fn publish_refuses_to_replace_and_temp_files_remove_themselves() {
    let dir = scratch("publish");
    let out = dir.join("o.pdf");
    std::fs::write(&out, b"mine").unwrap();
    let a = TempFile::beside(&out).unwrap();
    let b = TempFile::beside(&out).unwrap();
    assert_ne!(a.0, b.0);
    std::fs::write(&a.0, b"new").unwrap();
    assert!(matches!(publish(&a.0, &out), Err(OcrError::OutputExists(_))));
    assert_eq!(std::fs::read(&out).unwrap(), b"mine");
    std::fs::remove_file(&out).unwrap();
    publish(&a.0, &out).unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"new");
    drop((a, b));
    assert_eq!(files_in(&dir), ["o.pdf"]);
}

#[test]
fn a_run_where_every_page_fails_is_an_error_and_leaves_nothing() {
    for (name, stub) in [
        ("failing", Stub { fail: Some("engine broke"), ..Stub::new(stub_words()) }),
        ("panicking", Stub { panics: true, ..Stub::new(stub_words()) }),
    ] {
        let dir = scratch(name);
        let input = dir.join("in.pdf");
        std::fs::write(&input, build(&[blank_scan(612.0, 792.0), blank_scan(612.0, 792.0)])).unwrap();
        let out = dir.join("out.pdf");
        let r = ocr_file(&input, &out, &stub, &opts72(), &no_progress, &AtomicBool::new(false));
        assert!(matches!(r, Err(OcrError::Backend(_))), "{name}: {r:?}");
        assert_eq!(files_in(&dir), ["in.pdf"], "{name}");
    }
}

#[test]
fn a_missing_language_fails_once_before_any_page_is_read() {
    let stub = Stub::new(stub_words());
    let input = write_tmp("lang-in.pdf", &build(&[blank_scan(612.0, 792.0), blank_scan(612.0, 792.0)]));
    let o = Options { languages: vec!["xx-XX".into()], ..opts72() };
    let r = recognize(&Input::Path(&input), &stub, &o, &no_progress, &AtomicBool::new(false));
    assert!(matches!(r, Err(OcrError::LanguageUnavailable(_))));
    assert_eq!(stub.calls.load(Ordering::SeqCst), 0);
    // Several languages: one installed pack is enough, the missing ones are skipped.
    let o = Options { languages: vec!["xx-XX".into(), "en-GB".into()], ..opts72() };
    assert!(recognize(&Input::Path(&input), &stub, &o, &no_progress, &AtomicBool::new(false)).is_ok());
}

#[test]
fn byte_input_is_read_once_and_no_scratch_file_is_left() {
    let bytes = build(&(0..8).map(|_| blank_scan(612.0, 792.0)).collect::<Vec<_>>());
    let stub = Stub::new(stub_words());
    let o = Options { workers: 4, ..opts72() };
    let rec = recognize(&Input::Bytes(&bytes), &stub, &o, &no_progress, &AtomicBool::new(false)).unwrap();
    assert_eq!(rec.pages.len(), 8);
    assert!(rec.pages.iter().all(|p| matches!(p.status, PageStatus::Recognized)));
    let mine = format!(".inkwell-ocr-input.pdf.inkwell-ocr-{}-", std::process::id());
    assert!(files_in(&std::env::temp_dir()).iter().all(|n| !n.starts_with(&mine)));
}

#[test]
fn forcing_again_does_not_stack_wrapper_streams() {
    let dir = scratch("stack");
    let first_in = dir.join("a.pdf");
    std::fs::write(&first_in, build(&[blank_scan(612.0, 792.0)])).unwrap();
    let stub = Stub::new(stub_words());
    let forced = Options { force: true, workers: 1, ..opts72() };
    let mut src = first_in.clone();
    let mut lens = Vec::new();
    for n in 1..=3 {
        let out = dir.join(format!("r{n}.pdf"));
        ocr_file(&src, &out, &stub, &forced, &no_progress, &AtomicBool::new(false)).unwrap();
        lens.push(contents_len(&out, 0));
        assert_eq!(search(&out, 0, "Invoice").len(), 1);
        src = out;
    }
    assert_eq!(lens[0], lens[1], "{lens:?}");
    assert_eq!(lens[1], lens[2], "{lens:?}");
}

#[test]
fn our_font_name_is_not_taken_when_the_page_already_uses_it() {
    let dir = scratch("fontname");
    let input = dir.join("in.pdf");
    // The page already has an unrelated font under the name we would pick.
    let scan = Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: 8, h: 8, pixels: vec![255; 64] }, rotate: 0, sloppy_cm: false };
    std::fs::write(&input, build_ext(&[scan], "", "/Font << /InkOcrF 3 0 R >>")).unwrap();
    let out = dir.join("out.pdf");
    let rep = ocr_file(&input, &out, &Stub::new(stub_words()), &opts72(), &no_progress, &AtomicBool::new(false)).unwrap();
    assert_eq!(rep.pages_recognized, 1);
    let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    let fonts = page.object().get_dict_inheritable("Resources").unwrap().unwrap().get_dict("Font").unwrap().unwrap();
    let base = |n: &str| fonts.get_dict(n).unwrap().unwrap().get_dict("BaseFont").unwrap().unwrap().as_name().unwrap().to_vec();
    assert_eq!(base("InkOcrF"), b"Helvetica", "the page's own font must be left alone");
    assert_eq!(base("InkOcrF1"), b"InkOcrGlyphless");
    assert_eq!(search(&out, 0, "Invoice").len(), 1);
}

#[test]
fn a_signed_document_is_copied_with_a_warning() {
    let dir = scratch("signed");
    let input = dir.join("in.pdf");
    let scan = Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: 8, h: 8, pixels: vec![255; 64] }, rotate: 0, sloppy_cm: false };
    std::fs::write(&input, build_ext(&[scan], "/AcroForm << /Fields [] /SigFlags 3 >>", "")).unwrap();
    let out = dir.join("out.pdf");
    let rep = ocr_file(&input, &out, &Stub::new(stub_words()), &opts72(), &no_progress, &AtomicBool::new(false)).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("signed")), "{:?}", rep.warnings);
    // A file MuPDF had to repair cannot be saved incrementally: full save, other warning.
    let mut broken = build_ext(
        &[Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: 8, h: 8, pixels: vec![255; 64] }, rotate: 0, sloppy_cm: false }],
        "/AcroForm << /Fields [] /SigFlags 3 >>",
        "",
    );
    let at = broken.windows(9).rposition(|w| w == b"startxref").unwrap() + 10;
    broken[at] = b'9'; // the offset now points past the real table
    let rin = dir.join("repaired.pdf");
    std::fs::write(&rin, &broken).unwrap();
    let rout = dir.join("repaired-out.pdf");
    let rep = ocr_file(&rin, &rout, &Stub::new(stub_words()), &opts72(), &no_progress, &AtomicBool::new(false)).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("invalid")), "{:?}", rep.warnings);
    assert!(!std::fs::read(&rout).unwrap().starts_with(&broken), "a full save rewrites the file");
    assert_eq!(search(&rout, 0, "Invoice").len(), 1);
    // An unsigned file carries none.
    let plain = dir.join("plain.pdf");
    std::fs::write(&plain, build(&[blank_scan(612.0, 792.0)])).unwrap();
    let rep = ocr_file(&plain, &dir.join("plain-out.pdf"), &Stub::new(stub_words()), &opts72(), &no_progress, &AtomicBool::new(false)).unwrap();
    assert!(rep.warnings.is_empty());
}

#[test]
fn a_child_process_is_killed_on_timeout_and_on_cancel() {
    use super::backend::run_child;
    #[cfg(windows)]
    let sleeper = || {
        let mut c = std::process::Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        c
    };
    #[cfg(not(windows))]
    let sleeper = || {
        let mut c = std::process::Command::new("sleep");
        c.arg("30");
        c
    };
    let t = Instant::now();
    let r = run_child(sleeper(), Vec::new(), Duration::from_millis(300), &AtomicBool::new(false));
    assert!(matches!(&r, Err(OcrError::Backend(m)) if m.contains("timed out")), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(10));

    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            cancel.store(true, Ordering::SeqCst);
        });
        run_child(sleeper(), Vec::new(), Duration::from_secs(60), &cancel)
    });
    assert!(matches!(r, Err(OcrError::Cancelled)), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_cancelled_run_writes_nothing() {
    let dir = scratch("cancelled");
    let input = dir.join("in.pdf");
    std::fs::write(&input, build(&[blank_scan(612.0, 792.0)])).unwrap();
    let out = dir.join("out.pdf");
    let r = ocr_file(&input, &out, &Stub::new(stub_words()), &opts72(), &no_progress, &AtomicBool::new(true));
    assert!(matches!(r, Err(OcrError::Cancelled)), "{r:?}");
    assert_eq!(files_in(&dir), ["in.pdf"]);
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
        let _ = std::fs::remove_file(&out);
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
    Word { text: text.into(), bbox: [x, y, x + 6.0 * text.len() as f32, y + 10.0], conf: None }
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
    let raw = WinRtOcr.recognize(&bmp, &["xx-XX".to_string(), have], &AtomicBool::new(false)).unwrap();
    assert!(raw.lines.iter().flat_map(|l| &l.words).any(|w| w.text == "quick"));
    assert!(matches!(WinRtOcr.recognize(&bmp, &["xx-XX".to_string()], &AtomicBool::new(false)), Err(OcrError::LanguageUnavailable(_))));
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

// ---------------------------------------------------------------------------------------------
// Cleaned-up style
// ---------------------------------------------------------------------------------------------

use super::raster::{self, Raster};

fn raster_of(b: &Bitmap) -> Raster {
    Raster { w: b.w as usize, h: b.h as usize, n: 1, px: b.pixels.clone() }
}

/// The page as a scanner that was fed the sheet crooked would see it: tilted clockwise by `deg`.
fn tilted(bmp: &Bitmap, deg: f64) -> Bitmap {
    let r = raster_of(bmp);
    let back = clean::turn(-deg, (r.w as f64 / 2.0, r.h as f64 / 2.0));
    Bitmap { w: bmp.w, h: bmp.h, pixels: raster::warp(&r, &back, &[255]).px }
}

const TWELVE_LINES: [&str; 12] = [
    "Invoice Total: $1,234.56 for the month of March",
    "jane.tester@example.com wrote about the quarterly review",
    "another line of ordinary text that fills the width",
    "and one more line of plain text to give rows weight",
    "the quick brown fox jumps over the lazy dog again",
    "pack my box with five dozen liquor jugs please",
    "sphinx of black quartz judge my vow of silence",
    "how vexingly quick daft zebras jump over fences",
    "a wizard's job is to vex chumps quickly in fog",
    "the five boxing wizards jump quickly over it",
    "waltz bad nymph for quick jigs vex everyone",
    "glib jocks quiz nymph to vex dwarf quickly",
];

fn stub_with_angle(deg: f32) -> Stub {
    let mut s = Stub::new(stub_words());
    s.angle = Some(deg);
    s
}

fn cleaned() -> Options {
    Options { style: Style::Cleaned, ..opts72() }
}

/// [`super::apply_styled`] with nobody watching and nothing cancelled.
fn apply_styled(doc: &mut PdfDocument, results: &[PageResult], opts: &Options) -> Result<ApplyReport, OcrError> {
    super::apply_styled(doc, results, opts, &no_progress, &AtomicBool::new(false))
}

/// The first page's `/Im0` in a written file: its filter, size and decoded samples.
fn page_image(path: &std::path::Path, page: i32) -> (String, u32, u32, Vec<u8>) {
    let doc = PdfDocument::open(path.to_string_lossy().as_ref()).unwrap();
    let p = doc.load_pdf_page(page).unwrap();
    let im = p.resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap();
    let filter = im.get_dict("Filter").unwrap().and_then(|f| f.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned()).unwrap_or_default();
    let n = |k: &str| im.get_dict(k).unwrap().unwrap().as_int().unwrap() as u32;
    (filter, n("Width"), n("Height"), im.read_stream().unwrap())
}

fn unpack_bits(data: &[u8], w: u32, h: u32) -> Raster {
    let stride = (w as usize).div_ceil(8);
    let mut px = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            px.push(if data[y * stride + x / 8] >> (7 - x % 8) & 1 == 1 { 255 } else { 0 });
        }
    }
    Raster { w: w as usize, h: h as usize, n: 1, px }
}

fn recognized_page(page: u32, skew: Option<f32>) -> PageResult {
    PageResult {
        page,
        status: PageStatus::Recognized,
        lines: vec![vec![Word { text: "Invoice".into(), bbox: [72.0, 100.0, 120.0, 114.0], conf: None }]],
        skew_deg: skew,
        rotation: 0,
        page_size: [612.0, 792.0],
        ms: 0,
    }
}

#[test]
fn ccitt_g4_round_trips_through_the_reader() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let (packed, stride) = raster::pack_bilevel(&bmp.pixels, bmp.w as usize, bmp.h as usize, 128);
    let g4 = mupdf::Buffer::ccitt_g4(&packed, bmp.w, bmp.h, stride).unwrap();
    assert!(g4.len() < packed.len() / 4, "G4 should be much smaller than raw bits ({} vs {})", g4.len(), packed.len());
    let (out, rep) = {
        let pages = [Pg::Enc { w: 612.0, h: 792.0, bmp: Bitmap { w: bmp.w, h: bmp.h, pixels: bmp.pixels.clone() }, enc: Enc::G4 }];
        run_stub(&pages, &Stub::new(stub_words()), &opts72(), "g4-roundtrip")
    };
    assert_eq!(rep.pages_recognized, 1);
    let (filter, w, h, data) = page_image(&out, 0);
    assert_eq!(filter, "CCITTFaxDecode");
    assert_eq!(unpack_bits(&data, w, h).px, raster_of(&bmp).px.iter().map(|&v| if v >= 128 { 255 } else { 0 }).collect::<Vec<u8>>());
}

#[test]
fn cleaned_style_straightens_the_picture_and_the_text_layer_stops_following_a_tilt() {
    let upright = scan_bitmap(&TWELVE_LINES, 100);
    let crooked = tilted(&upright, 2.0);
    let before = raster_of(&crooked);
    let pages = [Pg::Scan { w: 612.0, h: 792.0, bmp: crooked, rotate: 0, sloppy_cm: false }];
    let (out, rep) = run_stub(&pages, &stub_with_angle(2.0), &cleaned(), "clean-skew");
    let c = rep.clean.as_ref().expect("a cleaned run reports what it did");
    assert_eq!((c.pages_cleaned, c.pages_deskewed, c.pages_left), (1, 1, 0), "{c:?}");
    let (filter, w, h, data) = page_image(&out, 0);
    assert_eq!((filter.as_str(), w as usize, h as usize), ("FlateDecode", before.w, before.h));
    let after = Raster { w: w as usize, h: h as usize, n: 1, px: data };
    assert!(raster::row_sharpness(&after, 0.0) > raster::row_sharpness(&before, 0.0) * 1.05, "rows must line up better");
    // The layer sits on the boxes as the engine gave them: the picture is straight now.
    let hits = search(&out, 0, "Invoice");
    assert_eq!(hits.len(), 1);
    let (cx, cy) = centre(&hits[0]);
    assert!((cx - 96.0).abs() < 3.0 && (cy - 107.0).abs() < 4.0, "({cx},{cy})");
    // And a plain searchable run on the same file still tilts the layer instead.
    let (plain, _) = run_stub(&pages, &stub_with_angle(2.0), &opts72(), "clean-skew-plain");
    let (px, py) = centre(&search(&plain, 0, "Invoice")[0]);
    assert!((py - 107.0).abs() > 4.0 || (px - 96.0).abs() > 3.0, "searchable keeps the tilt ({px},{py})");
    assert_eq!(page_image(&plain, 0).3, before.px, "searchable leaves the picture alone");
}

#[test]
fn cleaned_style_keeps_a_straight_clean_page_exactly() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let pixels = bmp.pixels.clone();
    let pages = [Pg::Scan { w: 612.0, h: 792.0, bmp, rotate: 0, sloppy_cm: false }];
    let (out, rep) = run_stub(&pages, &Stub::new(stub_words()), &cleaned(), "clean-nothing");
    let c = rep.clean.as_ref().unwrap();
    assert_eq!((c.pages_cleaned, c.pages_clean), (0, 1), "{c:?}");
    assert_eq!(page_image(&out, 0).3, pixels);
    assert_eq!(search(&out, 0, "Invoice").len(), 1, "it still gets its text layer");
}

#[test]
fn cleaned_style_removes_isolated_specks_and_nothing_else() {
    let clean_bmp = scan_bitmap(&TWELVE_LINES, 100);
    let mut dirty = Bitmap { w: clean_bmp.w, h: clean_bmp.h, pixels: clean_bmp.pixels.clone() };
    // Dust in the left margin (the text starts about 100 px in).
    for i in 0..15 {
        dirty.pixels[(60 + i * 53) * dirty.w as usize + 20 + (i % 3) * 9] = 0;
    }
    let pages = [Pg::Scan { w: 612.0, h: 792.0, bmp: dirty, rotate: 0, sloppy_cm: false }];
    let (out, rep) = run_stub(&pages, &Stub::new(stub_words()), &cleaned(), "clean-specks");
    let c = rep.clean.as_ref().unwrap();
    assert_eq!((c.pages_cleaned, c.pages_deskewed, c.specks_removed), (1, 0, 15), "{c:?}");
    assert_eq!(page_image(&out, 0).3, clean_bmp.pixels, "only the dust is gone");
}

#[test]
fn a_jpeg_scan_stays_a_jpeg_and_a_bilevel_scan_stays_ccitt() {
    let upright = scan_bitmap(&TWELVE_LINES, 100);
    let crooked = tilted(&upright, 2.0);
    let before = raster_of(&crooked);
    for (enc, name, filter) in [(Enc::Jpeg, "clean-jpeg", "DCTDecode"), (Enc::G4, "clean-g4", "CCITTFaxDecode")] {
        let pages = [Pg::Enc { w: 612.0, h: 792.0, bmp: Bitmap { w: crooked.w, h: crooked.h, pixels: crooked.pixels.clone() }, enc }];
        let (out, rep) = run_stub(&pages, &stub_with_angle(2.0), &cleaned(), name);
        let c = rep.clean.as_ref().unwrap();
        assert_eq!((c.pages_cleaned, c.pages_deskewed), (1, 1), "{name}: {c:?}");
        let (f, w, h, data) = page_image(&out, 0);
        assert_eq!(f, filter, "{name}");
        assert_eq!((w, h), (crooked.w, crooked.h));
        let after = if matches!(enc, Enc::G4) { unpack_bits(&data, w, h) } else { Raster { w: w as usize, h: h as usize, n: 1, px: data } };
        assert!(raster::row_sharpness(&after, 0.0) > raster::row_sharpness(&before, 0.0) * 1.05, "{name}: rows must line up better");
    }
}

#[test]
fn a_picture_shared_with_another_page_is_not_touched() {
    let crooked = tilted(&scan_bitmap(&TWELVE_LINES, 100), 2.0);
    let pages = [
        Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: crooked.w, h: crooked.h, pixels: crooked.pixels.clone() }, rotate: 0, sloppy_cm: false },
        Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: crooked.w, h: crooked.h, pixels: crooked.pixels.clone() }, rotate: 0, sloppy_cm: false },
    ];
    let mut doc = PdfDocument::from_bytes(&build(&pages)).unwrap();
    let im = |doc: &PdfDocument, n: i32| {
        doc.load_pdf_page(n).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap()
    };
    // Page 2 draws page 1's picture.
    let shared = im(&doc, 0);
    let mut xo = doc.load_pdf_page(1).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap();
    xo.dict_put("Im0", shared.clone()).unwrap();
    let (id, bytes) = (shared.as_indirect().unwrap(), shared.read_raw_stream().unwrap());
    assert_eq!(im(&doc, 1).as_indirect().unwrap(), id);

    let rep = apply_styled(&mut doc, &[recognized_page(0, Some(2.0))], &cleaned()).unwrap();
    assert_eq!(rep.clean.as_ref().unwrap().pages_deskewed, 1);
    let (a, b) = (im(&doc, 0), im(&doc, 1));
    assert_ne!(a.as_indirect().unwrap(), id, "page 1 got its own picture");
    assert_eq!(b.as_indirect().unwrap(), id, "page 2 still draws the original object");
    assert_eq!(b.read_raw_stream().unwrap(), bytes, "and its bytes are as they were");
}

#[test]
fn a_page_with_a_redaction_mark_and_a_signed_document_are_left_alone() {
    let crooked = tilted(&scan_bitmap(&TWELVE_LINES, 100), 2.0);
    let scan = || Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: crooked.w, h: crooked.h, pixels: crooked.pixels.clone() }, rotate: 0, sloppy_cm: false };

    let mut doc = PdfDocument::from_bytes(&build(&[scan()])).unwrap();
    let before = doc.load_pdf_page(0).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap().as_indirect().unwrap();
    let annot = doc.new_object_from_str("<< /Type /Annot /Subtype /Redact /Rect [10 10 100 100] >>").unwrap();
    let annot = doc.add_object(&annot).unwrap();
    let mut arr = doc.new_array().unwrap();
    arr.array_push(annot).unwrap();
    doc.load_pdf_page(0).unwrap().object().dict_put("Annots", arr).unwrap();
    let rep = apply_styled(&mut doc, &[recognized_page(0, Some(2.0))], &cleaned()).unwrap();
    let c = rep.clean.unwrap();
    assert_eq!((c.pages_cleaned, c.pages_left), (0, 1));
    assert!(c.notes[0].contains("redaction"), "{:?}", c.notes);
    let after = doc.load_pdf_page(0).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap().as_indirect().unwrap();
    assert_eq!(after, before);

    let dir = scratch("clean-signed");
    let input = dir.join("in.pdf");
    std::fs::write(&input, build_ext(&[scan()], "/AcroForm << /Fields [] /SigFlags 3 >>", "")).unwrap();
    let out = dir.join("out.pdf");
    let err = ocr_file(&input, &out, &stub_with_angle(2.0), &cleaned(), &no_progress, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(err.to_string(), SIGNED);
    assert!(!out.exists(), "nothing is written");
    // The searchable style still works on it (with its warning, as before).
    assert!(ocr_file(&input, &out, &stub_with_angle(2.0), &opts72(), &no_progress, &AtomicBool::new(false)).is_ok());
}

#[test]
fn a_page_with_several_pictures_or_a_stencil_is_left_as_it_is() {
    // Two scans side by side on one page are not "the scan": nothing is straightened.
    let crooked = tilted(&scan_bitmap(&TWELVE_LINES, 100), 2.0);
    let mut doc = PdfDocument::from_bytes(&build(&[Pg::Scan {
        w: 612.0,
        h: 792.0,
        bmp: Bitmap { w: crooked.w, h: crooked.h, pixels: crooked.pixels.clone() },
        rotate: 0,
        sloppy_cm: false,
    }]))
    .unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    // Draw the same picture a second time, small.
    let contents = page.object().get_dict("Contents").unwrap().unwrap();
    let extra = b"\nq 100 0 0 100 10 10 cm /Im0 Do Q\n";
    let mut data = contents.read_stream().unwrap();
    data.extend_from_slice(extra);
    let buf = mupdf::Buffer::from_bytes(&data).unwrap();
    let new = doc.add_stream(&buf, None, false).unwrap();
    page.object().dict_put("Contents", new).unwrap();
    let rep = apply_styled(&mut doc, &[recognized_page(0, Some(2.0))], &cleaned()).unwrap();
    let c = rep.clean.unwrap();
    assert_eq!((c.pages_cleaned, c.pages_left), (0, 1), "{c:?}");
    assert!(c.notes[0].contains("more than one picture"), "{:?}", c.notes);
}

// ---------------------------------------------------------------------------------------------
// Editable-text style
// ---------------------------------------------------------------------------------------------

fn editable() -> Options {
    Options { style: Style::Editable, ..opts72() }
}

fn scan_page(bmp: &Bitmap) -> Pg {
    Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: bmp.w, h: bmp.h, pixels: bmp.pixels.clone() }, rotate: 0, sloppy_cm: false }
}

/// What a flawless engine would return for a scan made from `lines`: each word with the box of its
/// ink, in points (the scan is 100 dpi, the page 72). Segmentation comes from the vector page the
/// scan was rendered from, the boxes are then pulled in to the dark pixels like Tesseract's.
fn ink_words(lines: &[&'static str], bmp: &Bitmap) -> Vec<Vec<(&'static str, [f32; 4])>> {
    let vector = build(&[Pg::Text(lines.iter().map(|s| s.to_string()).collect())]);
    let doc = PdfDocument::from_bytes(&vector).unwrap();
    let words = doc.load_page(0).unwrap().to_text_page(TextPageFlags::empty()).unwrap().words();
    let k = bmp.w as f32 / 612.0;
    let (w, h) = (bmp.w as usize, bmp.h as usize);
    let mut it = words.iter();
    lines
        .iter()
        .map(|l| {
            l.split_whitespace()
                .map(|t| {
                    let word = it.next().expect("the vector page has a word for every word");
                    assert_eq!(word.text, t);
                    let (x0, y0) = ((word.bounds.x0 * k).floor().max(0.0) as usize, (word.bounds.y0 * k).floor().max(0.0) as usize);
                    let (x1, y1) = (((word.bounds.x1 * k).ceil() as usize).min(w), ((word.bounds.y1 * k).ceil() as usize).min(h));
                    let (mut ax, mut ay, mut bx, mut by) = (usize::MAX, usize::MAX, 0, 0);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            if bmp.pixels[y * w + x] < 128 {
                                (ax, ay, bx, by) = (ax.min(x), ay.min(y), bx.max(x + 1), by.max(y + 1));
                            }
                        }
                    }
                    assert!(ax != usize::MAX, "no ink in the box of {t:?}");
                    (t, [ax as f32 / k, ay as f32 / k, bx as f32 / k, by as f32 / k])
                })
                .collect()
        })
        .collect()
}

struct Ch {
    c: char,
    x: f32,
    y: f32,
    size: f32,
    font: String,
}

/// Every character of a page as extracted: its origin (page space, y down), size and font name.
fn chars_of(path: &std::path::Path, page: i32) -> Vec<Ch> {
    let doc = PdfDocument::open(path.to_string_lossy().as_ref()).unwrap();
    let tp = doc.load_page(page).unwrap().to_text_page(TextPageFlags::empty()).unwrap();
    let mut out = Vec::new();
    for b in tp.blocks() {
        for l in b.lines() {
            for ch in l.chars() {
                if let Some(c) = ch.char() {
                    let o = ch.origin();
                    out.push(Ch { c, x: o.x, y: o.y, size: ch.size(), font: ch.font().map(|f| f.name().to_string()).unwrap_or_default() });
                }
            }
        }
    }
    out
}

fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn ink_count(r: &Raster, below: u8) -> usize {
    r.px.iter().filter(|&&v| v < below).count()
}

fn page_raster(path: &std::path::Path, page: i32) -> Raster {
    let (_, w, h, data) = page_image(path, page);
    Raster { w: w as usize, h: h as usize, n: 1, px: data }
}

fn word_results(words: &[Vec<(&'static str, [f32; 4])>], page: u32, skew: Option<f32>, conf: Option<f32>) -> PageResult {
    PageResult {
        page,
        status: PageStatus::Recognized,
        lines: words.iter().map(|l| l.iter().map(|(t, b)| Word { text: t.to_string(), bbox: *b, conf }).collect()).collect(),
        skew_deg: skew,
        rotation: 0,
        page_size: [612.0, 792.0],
        ms: 0,
    }
}

#[test]
fn editable_text_is_real_searchable_text_within_two_points_of_the_scan() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let (out, rep) = run_stub(&[scan_page(&bmp)], &Stub::new(words.clone()), &editable(), "editable-clean");
    let e = rep.editable.as_ref().expect("an editable run reports what it did");
    let total: usize = TWELVE_LINES.iter().map(|l| l.split_whitespace().count()).sum();
    assert_eq!((e.pages_converted, e.pages_kept, e.words_kept, e.words_converted as usize), (1, 0, 0, total), "{e:?}");

    // Real text, in order, findable.
    let text = normalized(&page_text(&out, 0));
    for l in TWELVE_LINES {
        assert!(text.contains(l), "{l:?} is missing from {text:?}");
    }
    assert_eq!(search(&out, 0, "quarterly review").len(), 1);

    // Every character sits on its line's baseline and every word starts where the scan's did.
    let chars = chars_of(&out, 0);
    let flat: Vec<&Ch> = chars.iter().filter(|c| !c.c.is_whitespace()).collect();
    let mut k = 0;
    for (i, line) in words.iter().enumerate() {
        let baseline = 72.0 + 30.0 * i as f32;
        for (t, b) in line {
            for (j, expected) in t.chars().enumerate() {
                let c = flat[k];
                k += 1;
                assert_eq!(c.c, expected, "line {i} word {t:?}");
                assert!((c.y - baseline).abs() <= 2.0, "{t:?}: baseline {} vs {baseline}", c.y);
                assert!((c.size - 14.0).abs() <= 2.1, "{t:?}: size {}", c.size);
                if j == 0 {
                    assert!((c.x - b[0]).abs() <= 2.0, "{t:?}: x {} vs {}", c.x, b[0]);
                }
            }
        }
    }
    assert_eq!(k, flat.len());
    assert!(flat.iter().all(|c| c.font.contains("NotoSans") && !c.font.contains("Mono")), "{:?}", flat[0].font);

    // The scan under the text is gone, and what is on the page still reads.
    assert!(ink_count(&page_raster(&out, 0), 200) <= 30, "the picture still carries the text");
    let doc = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    let shown = render(&doc, 0, 72, 4000).unwrap().0;
    assert!(shown.pixels.iter().filter(|&&v| v < 128).count() > 1500, "the text is drawn");
}

#[test]
fn editable_text_follows_a_tilted_scan_and_leaves_no_ink_behind() {
    let upright = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &upright);
    let crooked = tilted(&upright, 2.0);
    let mut stub = Stub::new(words.clone());
    stub.angle = Some(2.0);
    let (out, rep) = run_stub(&[scan_page(&crooked)], &stub, &editable(), "editable-skew");
    let e = rep.editable.as_ref().unwrap();
    assert_eq!((e.pages_converted, e.words_kept), (1, 0), "{e:?}");
    let turn = clean::turn(2.0, (306.0, 396.0));
    let chars = chars_of(&out, 0);
    let flat: Vec<&Ch> = chars.iter().filter(|c| !c.c.is_whitespace()).collect();
    let mut k = 0;
    for (i, line) in words.iter().enumerate() {
        for (t, b) in line {
            let (ex, ey) = turn.apply(f64::from(b[0]), f64::from(72.0 + 30.0 * i as f32));
            let c = flat[k];
            assert!((f64::from(c.x) - ex).abs() <= 2.0 && (f64::from(c.y) - ey).abs() <= 2.0, "{t:?}: ({}, {}) vs ({ex}, {ey})", c.x, c.y);
            k += t.chars().count();
        }
    }
    assert!(ink_count(&page_raster(&out, 0), 200) <= 60, "the tilted text is painted over");
}

#[test]
fn words_the_engine_doubts_or_that_sit_on_a_picture_stay_in_the_image_and_stay_searchable() {
    let mut bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let k = bmp.w as f32 / 612.0;
    let w = bmp.w as usize;
    // "quick" of line 4 sits on a patch of noise: a photograph, not paper (the patch stops short of
    // the neighbouring words).
    let q = words[4][1].1;
    let (x0, y0, x1, y1) = ((q[0] * k) as usize - 2, (q[1] * k) as usize - 8, (q[2] * k) as usize + 2, (q[3] * k) as usize + 8);
    for y in y0..y1 {
        for x in x0..x1 {
            bmp.pixels[y * w + x] = (60 + (x * 37 + y * 91) % 150) as u8;
        }
    }
    let mut stub = Stub::new(words.clone());
    stub.low = vec!["brown"];
    let (out, rep) = run_stub(&[scan_page(&bmp)], &stub, &editable(), "editable-unsure");
    let e = rep.editable.as_ref().unwrap();
    assert_eq!((e.pages_converted, e.words_kept), (1, 2), "{e:?}");
    let mut why: Vec<(String, &str)> = e.unsure.iter().map(|u| (u.text.clone(), u.why)).collect();
    why.sort();
    assert_eq!(why, vec![("brown".to_string(), "low-confidence"), ("quick".to_string(), "picture")]);
    let brown = e.unsure.iter().find(|u| u.text == "brown").unwrap();
    assert!((brown.bbox[0] - words[4][2].1[0]).abs() < 1.0 && (brown.bbox[1] - words[4][2].1[1]).abs() < 1.0, "{:?}", brown.bbox);

    // Still findable (invisible text), and still in the picture.
    assert_eq!(search(&out, 0, "quick brown").len(), 1);
    let im = page_raster(&out, 0);
    let ink_in = |b: [f32; 4]| {
        let mut n = 0;
        for y in (b[1] * k) as usize..(b[3] * k) as usize {
            for x in (b[0] * k) as usize..(b[2] * k) as usize {
                n += usize::from(im.px[y * im.w + x] < 128);
            }
        }
        n
    };
    assert!(ink_in(words[4][2].1) > 30, "the doubted word is still in the picture");
    assert_eq!(ink_in(words[4][3].1), 0, "a sure word is gone from it");
    assert!(ink_in(words[4][1].1) > 100, "the noise patch is untouched");
    // The sure words are text.
    assert!(normalized(&page_text(&out, 0)).contains("fox jumps over the lazy dog again"));
}

#[test]
fn an_image_shared_with_another_page_keeps_its_pixels_when_one_page_becomes_text() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let mut doc = PdfDocument::from_bytes(&build(&[scan_page(&bmp), scan_page(&bmp)])).unwrap();
    let im = |doc: &PdfDocument, n: i32| doc.load_pdf_page(n).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap();
    let shared = im(&doc, 0);
    let mut xo = doc.load_pdf_page(1).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap();
    xo.dict_put("Im0", shared.clone()).unwrap();
    let (id, bytes) = (shared.as_indirect().unwrap(), shared.read_raw_stream().unwrap());

    let rep = apply_styled(&mut doc, &[word_results(&words, 0, None, None)], &editable()).unwrap();
    let e = rep.editable.as_ref().unwrap();
    assert_eq!((e.pages_converted, e.pages_kept), (1, 0), "{e:?}");
    assert_ne!(im(&doc, 0).as_indirect().unwrap(), id, "page 1 got a picture of its own");
    assert_eq!(im(&doc, 1).as_indirect().unwrap(), id, "page 2 still draws the original object");
    assert_eq!(im(&doc, 1).read_raw_stream().unwrap(), bytes, "with its bytes as they were");
}

#[test]
fn editable_text_leaves_signed_documents_and_redaction_pages_alone() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let mut doc = PdfDocument::from_bytes(&build(&[scan_page(&bmp)])).unwrap();
    let picture = |doc: &PdfDocument| doc.load_pdf_page(0).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap().as_indirect().unwrap();
    let before = picture(&doc);
    let annot = doc.new_object_from_str("<< /Type /Annot /Subtype /Redact /Rect [10 10 100 100] >>").unwrap();
    let annot = doc.add_object(&annot).unwrap();
    let mut arr = doc.new_array().unwrap();
    arr.array_push(annot).unwrap();
    doc.load_pdf_page(0).unwrap().object().dict_put("Annots", arr).unwrap();
    let rep = apply_styled(&mut doc, &[word_results(&words, 0, None, None)], &editable()).unwrap();
    let e = rep.editable.unwrap();
    assert_eq!((e.pages_converted, e.pages_kept, e.words_converted), (0, 1, 0), "{e:?}");
    assert!(e.notes[0].contains("redaction"), "{:?}", e.notes);
    assert_eq!(picture(&doc), before, "its picture is the one it had");
    assert!(rep.words > 0, "it still gets the invisible layer");

    let dir = scratch("editable-signed");
    let input = dir.join("in.pdf");
    std::fs::write(&input, build_ext(&[scan_page(&bmp)], "/AcroForm << /Fields [] /SigFlags 3 >>", "")).unwrap();
    let out = dir.join("out.pdf");
    let err = ocr_file(&input, &out, &Stub::new(words), &editable(), &no_progress, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(err.to_string(), SIGNED);
    assert!(!out.exists());
}

#[test]
fn the_font_choice_is_honoured() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let mono = Options { font: FontPref::Mono, ..editable() };
    let (out, rep) = run_stub(&[scan_page(&bmp)], &Stub::new(words), &mono, "editable-mono");
    assert_eq!(rep.editable.as_ref().unwrap().pages_converted, 1);
    let chars = chars_of(&out, 0);
    assert!(chars.iter().filter(|c| !c.c.is_whitespace()).all(|c| c.font.contains("Mono")), "{:?}", chars[0].font);
    assert!(normalized(&page_text(&out, 0)).contains("Invoice Total: $1,234.56 for the month of March"));
}

#[test]
fn editable_text_can_be_changed_with_the_edit_tool() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let (out, _) = run_stub(&[scan_page(&bmp)], &Stub::new(words), &editable(), "editable-edit");
    let eng = crate::engine::Engine::new(2, 64 << 20);
    let id = eng.open(&out.to_string_lossy()).unwrap().doc;
    let listed = crate::textedit::blocks(&eng, id, 0).unwrap();
    let b = listed.blocks.iter().find(|b| b.text.contains("jane.tester@example.com wrote")).unwrap_or_else(|| panic!("{:?}", listed.blocks.iter().map(|b| &b.text).collect::<Vec<_>>()));
    assert!(b.editable, "{:?}", b.reason);
    let change = crate::textedit::BlockChange { text: Some(b.text.replace("example.com", "example.org")), ..Default::default() };
    crate::textedit::edit(&eng, id, listed.rev, 0, b.index, &change, crate::engine::edit::EditSource::User).unwrap();
    let now = normalized(&eng.page_text(id, 0).unwrap().iter().map(|l| l.t.clone()).collect::<Vec<_>>().join(" "));
    assert!(now.contains("jane.tester@example.org wrote about the quarterly review"), "{now}");
    assert!(!now.contains("example.com"));
}

/// Swedish with English in one run: the languages Windows OCR cannot combine, read by Tesseract with
/// `swe+eng`. Skips (with the reason) where Tesseract or the Swedish pack is not installed.
#[test]
fn swedish_and_english_are_read_together_by_tesseract() {
    let Some(t) = TesseractCli::discover(None) else {
        eprintln!("skipped: Tesseract is not installed");
        return;
    };
    let have = t.languages().unwrap_or_default();
    if !["sv", "en"].iter().all(|l| have.iter().any(|x| x == l)) {
        eprintln!("skipped: Tesseract has no sv+en tessdata (has {have:?})");
        return;
    }
    let lines = ["Årsredovisning och räkenskapsåret", "Vi går på möte efter lunch", "The invoice total is paid in full"];
    let input = write_tmp("swe-in.pdf", &build(&[scan_of(&lines, 300)]));
    let output = input.with_file_name("swe-out.pdf");
    let _ = std::fs::remove_file(&output);
    let opts = Options { languages: vec!["sv".into(), "en".into()], workers: 1, ..Options::default() };
    let rep = ocr_file(&input, &output, &t, &opts, &no_progress, &AtomicBool::new(false)).unwrap();
    assert_eq!(rep.pages_recognized, 1);
    let text = page_text(&output, 0);
    for want in ["räkenskapsåret", "går", "möte", "invoice"] {
        assert!(text.to_lowercase().contains(want), "{want:?} missing in {text:?}");
    }
    assert_eq!(search(&output, 0, "möte").len(), 1);
}

#[test]
fn an_opaque_soft_mask_does_not_stop_the_picture_styles_but_a_real_one_does() {
    let crooked = tilted(&scan_bitmap(&TWELVE_LINES, 100), 2.0);
    let (w, h) = (crooked.w as usize, crooked.h as usize);
    for (opaque, cleaned_pages) in [(true, 1u32), (false, 0)] {
        let page = Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: crooked.w, h: crooked.h, pixels: crooked.pixels.clone() }, rotate: 0, sloppy_cm: false };
        let mut doc = PdfDocument::from_bytes(&build(&[page])).unwrap();
        let mut im = doc.load_pdf_page(0).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap();
        let mut alpha = vec![255u8; w * h];
        if !opaque {
            alpha[w * 5 + 5] = 0;
        }
        let dict = doc
            .new_object_from_str(&format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 >>"))
            .unwrap();
        let mask = doc.add_stream(&mupdf::Buffer::from_bytes(&alpha).unwrap(), Some(&dict), false).unwrap();
        im.dict_put("SMask", mask).unwrap();
        let rep = apply_styled(&mut doc, &[recognized_page(0, Some(2.0))], &cleaned()).unwrap();
        let c = rep.clean.unwrap();
        assert_eq!((c.pages_cleaned, c.pages_left), (cleaned_pages, 1 - cleaned_pages), "opaque={opaque}: {c:?}");
        if !opaque {
            assert!(c.notes[0].contains("mask"), "{:?}", c.notes);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Signatures, existing text, cancel (review fixes)
// ---------------------------------------------------------------------------------------------

/// A signature field with a value and no `/SigFlags`: only the field tree shows the file is signed.
const SIGNED_FIELD: &str = "/AcroForm << /Fields [ << /FT /Sig /T (Sig1) /V << /Type /Sig /Filter /Adobe.PPKLite \
     /SubFilter /adbe.pkcs7.detached /Contents <00> >> >> ] >>";
/// A certification: no change is allowed, not even a layer of invisible text.
const CERTIFIED_DOC: &str = "/Perms << /DocMDP << /Type /Sig /Filter /Adobe.PPKLite /Contents <00> \
     /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 1 >> >> ] >> >>";

#[test]
fn a_signed_field_counts_as_signed_even_without_the_flag() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let dir = scratch("signed-field");
    let input = dir.join("in.pdf");
    std::fs::write(&input, build_ext(&[scan_page(&bmp)], SIGNED_FIELD, "")).unwrap();
    let out = dir.join("out.pdf");
    let cancel = AtomicBool::new(false);
    for opts in [cleaned(), editable()] {
        let err = ocr_file(&input, &out, &Stub::new(words.clone()), &opts, &no_progress, &cancel).unwrap_err();
        assert_eq!(err.to_string(), SIGNED, "{:?}", opts.style);
        assert!(!out.exists(), "nothing is written");
    }
    // The searchable style appends its layer after the signed bytes and says so.
    let rep = ocr_file(&input, &out, &Stub::new(words), &opts72(), &no_progress, &cancel).unwrap();
    assert!(rep.warnings.iter().any(|w| w.contains("signed")), "{:?}", rep.warnings);
    let bytes = std::fs::read(&out).unwrap();
    let original = std::fs::read(&input).unwrap();
    assert!(bytes.starts_with(&original), "the signed revision is untouched: the file only grew");
}

#[test]
fn a_certified_document_takes_no_style_at_all() {
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let dir = scratch("certified");
    let input = dir.join("in.pdf");
    std::fs::write(&input, build_ext(&[scan_page(&bmp)], CERTIFIED_DOC, "")).unwrap();
    let out = dir.join("out.pdf");
    let cancel = AtomicBool::new(false);
    for style in [Style::Searchable, Style::Cleaned, Style::Editable] {
        let opts = Options { style, ..opts72() };
        let err = ocr_file(&input, &out, &Stub::new(words.clone()), &opts, &no_progress, &cancel).unwrap_err();
        assert_eq!(err.to_string(), CERTIFIED, "{style:?}");
        assert!(!out.exists(), "{style:?}: nothing is written");
        let err = preview(&Input::Path(&input), 0, &Stub::new(words.clone()), &opts, 400, &cancel).err().expect("refused");
        assert_eq!(err.to_string(), CERTIFIED, "preview {style:?}");
    }
}

#[test]
fn cleaning_a_page_that_already_has_text_does_not_move_its_picture() {
    // A scan that already carries a text layer, read again because "skip pages with text" is off:
    // that layer was placed on the tilted picture, so straightening the picture would put it
    // beside its words. The picture keeps its geometry.
    let crooked = tilted(&scan_bitmap(&TWELVE_LINES, 100), 2.0);
    let before = raster_of(&crooked);
    let pages = [Pg::Scan { w: 612.0, h: 792.0, bmp: crooked, rotate: 0, sloppy_cm: false }];
    let (first, _) = run_stub(&pages, &stub_with_angle(2.0), &opts72(), "clean-has-text-1");
    let out = std::env::temp_dir().join("inkwell-ocr-tests").join("clean-has-text-2.pdf");
    let _ = std::fs::remove_file(&out);
    let again = Options { force: true, ..cleaned() };
    let rep = ocr_file(&first, &out, &stub_with_angle(2.0), &again, &no_progress, &AtomicBool::new(false)).unwrap();
    let c = rep.clean.as_ref().unwrap();
    assert_eq!((c.pages_deskewed, c.pages_cleaned), (0, 0), "{c:?}");
    assert_eq!(page_image(&out, 0).3, before.px, "the picture is the one it was");
    // The same picture without a layer is straightened (see the skew test above).
    let (_, plain) = run_stub(&pages, &stub_with_angle(2.0), &cleaned(), "clean-has-text-3");
    assert_eq!(plain.clean.as_ref().unwrap().pages_deskewed, 1);
}

#[test]
fn a_cancel_between_pages_stops_the_picture_styles_and_reports_each_page() {
    let crooked = tilted(&scan_bitmap(&TWELVE_LINES, 100), 2.0);
    let page = || Pg::Scan { w: 612.0, h: 792.0, bmp: Bitmap { w: crooked.w, h: crooked.h, pixels: crooked.pixels.clone() }, rotate: 0, sloppy_cm: false };
    let image_of = |doc: &PdfDocument, p: i32| {
        doc.load_pdf_page(p).unwrap().resources().unwrap().get_dict("XObject").unwrap().unwrap().get_dict("Im0").unwrap().unwrap().as_indirect().unwrap()
    };
    let results = [recognized_page(0, Some(2.0)), recognized_page(1, Some(2.0))];

    // Cleaned: Cancel is pressed once the first page is written; the second is not touched.
    let mut doc = PdfDocument::from_bytes(&build(&[page(), page()])).unwrap();
    let second = image_of(&doc, 1);
    let cancel = AtomicBool::new(false);
    let seen = Mutex::new(Vec::new());
    let on = |p: Progress| {
        seen.lock().unwrap().push((p.done, p.total, p.writing));
        cancel.store(true, Ordering::Relaxed);
    };
    let err = super::apply_styled(&mut doc, &results, &cleaned(), &on, &cancel).unwrap_err();
    assert!(matches!(err, OcrError::Cancelled), "{err:?}");
    assert_eq!(*seen.lock().unwrap(), vec![(1, 2, true)], "one page was written before the stop");
    assert_eq!(image_of(&doc, 1), second, "the second picture is as it was");

    // Editable: a run nobody cancels reports every page in order, one that is cancelled stops.
    let bmp = scan_bitmap(&TWELVE_LINES, 100);
    let words = ink_words(&TWELVE_LINES, &bmp);
    let results = [word_results(&words, 0, None, None), word_results(&words, 1, None, None)];
    let mut doc = PdfDocument::from_bytes(&build(&[scan_page(&bmp), scan_page(&bmp)])).unwrap();
    let seen = Mutex::new(Vec::new());
    let all = |p: Progress| seen.lock().unwrap().push((p.done, p.total, p.writing));
    super::apply_styled(&mut doc, &results, &editable(), &all, &AtomicBool::new(false)).unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![(1, 2, true), (2, 2, true)]);

    let mut doc = PdfDocument::from_bytes(&build(&[scan_page(&bmp), scan_page(&bmp)])).unwrap();
    let cancel = AtomicBool::new(false);
    let stop_after_first = |p: Progress| {
        if p.done == 1 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let err = super::apply_styled(&mut doc, &results, &editable(), &stop_after_first, &cancel).unwrap_err();
    assert!(matches!(err, OcrError::Cancelled), "{err:?}");
}

#[test]
fn a_jpeg_is_written_back_at_about_the_quality_it_had() {
    use image::{ExtendedColorType, ImageEncoder, codecs::jpeg::JpegEncoder};
    let (w, h) = (64u32, 64u32);
    let px: Vec<u8> = (0..w * h).map(|i| ((i * 7 + (i / w) * 13) % 251) as u8).collect();
    let encode = |q: u8| {
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, q).write_image(&px, w, h, ExtendedColorType::L8).unwrap();
        out
    };
    for q in [50u8, 60, 75, 85, 90, 95] {
        let got = clean::jpeg_quality(&encode(q)).expect("a quality is read") as i32;
        assert!((got - q as i32).abs() <= 3, "wrote {q}, read {got}");
    }
    assert_eq!(clean::jpeg_quality(b"not a jpeg"), None);
    assert_eq!(clean::jpeg_quality(&encode(80)[..12]), None, "a cut stream has no table to read");
}
