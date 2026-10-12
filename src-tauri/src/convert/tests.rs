//! Export and create, end to end on scratch files (the fixtures are built here, never the user's files).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::create::{self, CreateRequest, CreateSource, PageSize, RequestSource};
use super::export::{ExportKind, ExportRequest, ImageFormat, Target};
use super::flow;
use super::*;
use crate::engine::fixture::{build, Fixture};
use crate::engine::Engine;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-convert-tests").join(test);
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// An engine with a 3-page fixture (612 x 792 pt) open; returns the document id.
fn open_fixture(d: &Path, pages: usize) -> (std::sync::Arc<Engine>, u32, PathBuf) {
    let p = d.join("sample.pdf");
    fs::write(&p, build(&Fixture { pages, ..Default::default() })).unwrap();
    let eng = Engine::new(1, 1 << 20);
    let (doc, r) = eng.open_pending(&p.to_string_lossy());
    r.unwrap();
    (eng, doc, p)
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

fn run_export(eng: &Engine, doc: u32, kind: ExportKind, pages: Option<&str>, target: Target) -> Result<ExportOutput, Error> {
    let req = ExportRequest { doc, kind, pages: pages.map(String::from), save_to: String::new() };
    export(eng, &req, target, &no_cancel(), &mut |_| {})
}

use super::export::{export, ExportOutput};

fn dims(p: &str) -> (u32, u32) {
    if p.ends_with(".tif") {
        let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(fs::read(p).unwrap())).unwrap();
        d.dimensions().unwrap()
    } else {
        image::image_dimensions(p).unwrap()
    }
}

fn near(a: u32, b: u32) -> bool {
    a.abs_diff(b) <= 1
}

// ---------------------------------------------------------------- export

#[test]
fn page_images_match_dpi_times_page_size_in_every_format() {
    let d = dir("images");
    let (eng, doc, _) = open_fixture(&d, 3);
    for (format, ext) in [(ImageFormat::Png, "png"), (ImageFormat::Jpeg, "jpg"), (ImageFormat::Tiff, "tif")] {
        let out = d.join(ext);
        fs::create_dir_all(&out).unwrap();
        let kind = ExportKind::Images { format, dpi: 144, gray: false, quality: 85 };
        let r = run_export(&eng, doc, kind, None, Target::Folder(out.clone())).unwrap();
        assert_eq!(r.files.len(), 3, "{ext}");
        assert_eq!(names(&out), ["sample-1.".to_string() + ext, "sample-2.".to_string() + ext, "sample-3.".to_string() + ext]);
        for f in &r.files {
            let (w, h) = dims(f);
            // 612 x 792 pt at 144 dpi
            assert!(near(w, 1224) && near(h, 1584), "{f}: {w}x{h}");
        }
        assert!(r.bytes > 0);
    }
}

#[test]
fn page_range_dpi_and_grayscale_are_honoured() {
    let d = dir("range");
    let (eng, doc, _) = open_fixture(&d, 5);
    let out = d.join("o");
    fs::create_dir_all(&out).unwrap();
    let kind = ExportKind::Images { format: ImageFormat::Png, dpi: 72, gray: true, quality: 90 };
    let r = run_export(&eng, doc, kind, Some("2, 4-5"), Target::Folder(out.clone())).unwrap();
    assert_eq!(names(&out), ["sample-2.png", "sample-4.png", "sample-5.png"]);
    assert_eq!(r.pages, 3);
    assert_eq!(dims(&r.files[0]), (612, 792));
    // PNG colour type 0 = greyscale (byte 25 of the file)
    assert_eq!(fs::read(&r.files[0]).unwrap()[25], 0);

    // a second run next to the first one does not overwrite it
    let again = run_export(&eng, doc, ExportKind::Images { format: ImageFormat::Png, dpi: 72, gray: true, quality: 90 }, Some("2"), Target::Folder(out.clone())).unwrap();
    assert!(again.files[0].ends_with("sample (2)-2.png"), "{:?}", again.files);
    assert_eq!(names(&out).len(), 4);

    let bad = run_export(&eng, doc, ExportKind::Images { format: ImageFormat::Png, dpi: 72, gray: false, quality: 90 }, Some("9"), Target::Folder(out.clone()));
    assert!(matches!(bad, Err(Error::Invalid(_))));
    let low = run_export(&eng, doc, ExportKind::Images { format: ImageFormat::Png, dpi: 10, gray: false, quality: 90 }, None, Target::Folder(out));
    assert!(matches!(low, Err(Error::Invalid(_))));
}

#[test]
fn text_plain_and_layout_contain_the_page_text() {
    let d = dir("text");
    let (eng, doc, _) = open_fixture(&d, 3);
    for layout in [false, true] {
        let to = d.join(format!("out-{layout}.txt"));
        let r = run_export(&eng, doc, ExportKind::Text { layout }, Some("2-3"), Target::File(to.clone())).unwrap();
        assert_eq!(r.files, [to.to_string_lossy().into_owned()]);
        let t = fs::read_to_string(&to).unwrap();
        assert!(t.contains("Inkwell fixture page 2"), "layout={layout}: {t}");
        assert!(t.contains("Inkwell fixture page 3"));
        assert!(!t.contains("fixture page 1"), "page 1 was not asked for");
        assert!(t.contains("The quick brown fox jumps over the lazy dog, line 0 of page 2"));
        assert!(t.contains('\x0c'), "pages are separated by a form feed");
    }
}

#[test]
fn html_and_xhtml_are_one_document_with_every_page() {
    let d = dir("html");
    let (eng, doc, _) = open_fixture(&d, 3);
    for (xhtml, ext) in [(false, "html"), (true, "xhtml")] {
        let to = d.join(format!("out.{ext}"));
        run_export(&eng, doc, ExportKind::Html { xhtml }, None, Target::File(to.clone())).unwrap();
        let t = fs::read_to_string(&to).unwrap();
        assert_eq!(t.matches("<body").count(), 1, "{ext}: one body");
        assert_eq!(t.matches("</body>").count(), 1);
        assert!(t.contains("<title>sample</title>"));
        for p in 1..=3 {
            assert!(t.contains(&format!("fixture page {p}")), "{ext} page {p}");
        }
    }
}

/// The bytes of `name` inside a stored/deflated zip, read from the central directory.
fn unzip(zip: &[u8], name: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let le32 = |at: usize| u32::from_le_bytes(zip[at..at + 4].try_into().unwrap()) as usize;
    let le16 = |at: usize| u16::from_le_bytes(zip[at..at + 2].try_into().unwrap()) as usize;
    let eocd = zip.len() - 22;
    let (mut at, count) = (le32(eocd + 16), le16(eocd + 10));
    for _ in 0..count {
        let (method, size, local, nlen, elen, clen) = (le16(at + 10), le32(at + 20), le32(at + 42), le16(at + 28), le16(at + 30), le16(at + 32));
        if &zip[at + 46..at + 46 + nlen] == name.as_bytes() {
            let start = local + 30 + le16(local + 26) + le16(local + 28);
            let body = &zip[start..start + size];
            return Some(if method == 8 {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(body).read_to_end(&mut out).ok()?;
                out
            } else {
                body.to_vec()
            });
        }
        at += 46 + nlen + elen + clen;
    }
    None
}

#[test]
fn word_export_is_a_docx_with_the_text() {
    let d = dir("docx");
    let (eng, doc, _) = open_fixture(&d, 2);
    let to = d.join("out.docx");
    run_export(&eng, doc, ExportKind::Word, None, Target::File(to.clone())).unwrap();
    let z = fs::read(&to).unwrap();
    assert!(z.starts_with(b"PK\x03\x04"));
    let xml = String::from_utf8(unzip(&z, "word/document.xml").expect("document.xml")).unwrap();
    assert!(xml.contains("Inkwell fixture page 1") && xml.contains("Inkwell fixture page 2"), "{xml}");
    assert!(unzip(&z, "[Content_Types].xml").is_some());
}

// ---------------------------------------------------------------- rtf

#[derive(Debug, PartialEq)]
enum Tok {
    Open,
    Close,
    Word(String, Option<i32>),
    Symbol(char),
    Text(char),
}

/// A small RTF tokenizer: groups, control words with their numeric parameter, control symbols
/// (`\\`, `\{`, `\}`, `\*`, `\'hh` as the byte's char) and plain characters (CR/LF are not text).
fn rtf_tokens(s: &str) -> Vec<Tok> {
    let c: Vec<char> = s.chars().collect();
    let (mut i, mut out) = (0, Vec::new());
    while i < c.len() {
        match c[i] {
            '{' => out.push(Tok::Open),
            '}' => out.push(Tok::Close),
            '\r' | '\n' => {}
            '\\' => {
                i += 1;
                let n = c[i];
                if n.is_ascii_alphabetic() {
                    let start = i;
                    while i < c.len() && c[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let name: String = c[start..i].iter().collect();
                    let ds = i;
                    if i < c.len() && c[i] == '-' {
                        i += 1;
                    }
                    while i < c.len() && c[i].is_ascii_digit() {
                        i += 1;
                    }
                    let num = c[ds..i].iter().collect::<String>().parse::<i32>().ok();
                    if i < c.len() && c[i] == ' ' {
                        i += 1; // the delimiter space belongs to the word
                    }
                    out.push(Tok::Word(name, num));
                    continue;
                } else if n == '\'' {
                    let h: String = c[i + 1..i + 3].iter().collect();
                    out.push(Tok::Text(u8::from_str_radix(&h, 16).unwrap() as char));
                    i += 2;
                } else {
                    out.push(Tok::Symbol(n));
                }
            }
            ch => out.push(Tok::Text(ch)),
        }
        i += 1;
    }
    out
}

/// The braces balance and nothing is left open.
fn balanced(toks: &[Tok]) -> bool {
    let mut depth = 0i32;
    for t in toks {
        match t {
            Tok::Open => depth += 1,
            Tok::Close => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

/// The document's text as a reader shows it: `\uN?` decoded (UTF-16 units joined), `\tab`, `\line` and
/// `\par` as characters; the font, colour, style, info and picture groups left out.
fn rtf_plain(toks: &[Tok]) -> String {
    let mut out = String::new();
    let mut units: Vec<u16> = Vec::new();
    let mut skip: Vec<bool> = Vec::new();
    let mut skip_fallback = false;
    let flush = |out: &mut String, units: &mut Vec<u16>| {
        out.push_str(&String::from_utf16(units).unwrap());
        units.clear();
    };
    let mut k = 0;
    while k < toks.len() {
        let hidden = skip.last().copied().unwrap_or(false);
        match &toks[k] {
            Tok::Open => {
                let kill = matches!(toks.get(k + 1), Some(Tok::Word(w, _)) if ["fonttbl", "colortbl", "stylesheet", "info", "pict"].contains(&w.as_str()));
                skip.push(hidden || kill);
            }
            Tok::Close => {
                skip.pop();
            }
            _ if hidden => {}
            Tok::Word(w, Some(n)) if w == "u" => {
                units.push(*n as i16 as u16);
                skip_fallback = true;
            }
            Tok::Text(_) if skip_fallback => skip_fallback = false,
            other => {
                flush(&mut out, &mut units);
                match other {
                    Tok::Word(w, _) if w == "tab" => out.push('\t'),
                    Tok::Word(w, _) if w == "line" || w == "par" || w == "page" => out.push('\n'),
                    Tok::Symbol(c @ ('\\' | '{' | '}')) => out.push(*c),
                    Tok::Text(c) => out.push(*c),
                    _ => {}
                }
            }
        }
        k += 1;
    }
    flush(&mut out, &mut units);
    out
}

fn has_word(toks: &[Tok], name: &str) -> bool {
    toks.iter().any(|t| matches!(t, Tok::Word(w, _) if w == name))
}

fn word_count(toks: &[Tok], name: &str) -> usize {
    toks.iter().filter(|t| matches!(t, Tok::Word(w, _) if w == name)).count()
}

fn model_block(t: &str, x: f64, y: f64, size: f64, bold: bool, color: [f64; 3]) -> flow::Block {
    flow::Block { text: t.into(), rect: [x, y, x + 60.0, y + size], size, bold, italic: !bold, color }
}

fn model_png() -> Vec<u8> {
    use image::ImageEncoder;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(&vec![200u8; 4 * 3 * 3], 4, 3, image::ExtendedColorType::Rgb8).unwrap();
    png
}

/// Two pages: a heading, a sub-heading, body text, a 3 x 3 table and a PNG on the first.
fn model_pages() -> Vec<flow::Page> {
    let mut blocks = vec![
        model_block("Årsrapport 日本語 😀 \\{x}", 72.0, 700.0, 24.0, true, [0.0; 3]),
        model_block("Overview", 72.0, 660.0, 14.0, true, [0.8, 0.1, 0.1]),
        model_block("Body text, long enough to be the main text size of this page ok.\tTabbed.", 72.0, 600.0, 11.0, false, [0.0; 3]),
    ];
    for r in 0..3 {
        for c in 0..3 {
            blocks.push(model_block(&format!("r{r}c{c}"), 72.0 + 100.0 * c as f64, 500.0 - 20.0 * r as f64, 11.0, r == 0, [0.0; 3]));
        }
    }
    let p1 = flow::Page { width: 612.0, height: 792.0, blocks, images: vec![flow::Image { ext: "png", bytes: model_png(), rect: [72.0, 300.0, 272.0, 400.0] }] };
    let p2 = flow::Page { width: 612.0, height: 792.0, blocks: vec![model_block("Second page", 72.0, 700.0, 11.0, false, [0.0, 0.0, 1.0])], images: vec![] };
    vec![p1, p2]
}

#[test]
fn rtf_escapes_follow_the_signed_16_bit_rule() {
    use super::rtf::rtf;
    let page = flow::Page {
        width: 612.0,
        height: 792.0,
        blocks: vec![model_block("å日😀\\{}\u{1}", 72.0, 700.0, 11.0, false, [0.0; 3])],
        images: vec![],
    };
    let r = rtf(&[page], "t");
    // U+00E5 = 229, U+65E5 = 26085, U+1F600 = D83D DE00 = -10179 -8704; the C0 control is dropped.
    assert!(r.text.contains("\\u229?\\u26085?\\u-10179?\\u-8704?\\\\\\{\\}\\par"), "{}", r.text);
    assert!(r.text.is_ascii());
}

#[test]
fn rtf_keeps_text_formatting_table_picture_and_page_break() {
    use super::rtf::rtf;
    let r = rtf(&model_pages(), "Report {1}");
    assert_eq!(r.skipped_images, 0);
    assert!(r.text.is_ascii());
    let toks = rtf_tokens(&r.text);
    assert!(balanced(&toks), "{}", r.text);
    assert!(matches!(toks.first(), Some(Tok::Open)) && matches!(toks.get(1), Some(Tok::Word(w, Some(1))) if w == "rtf"));
    for w in ["ansi", "ansicpg", "deff", "fonttbl", "colortbl", "stylesheet", "paperw", "margl", "trowd", "cellx", "cell", "row", "intbl", "pngblip", "pagebb", "b", "i"] {
        assert!(has_word(&toks, w), "missing \\{w}\n{}", r.text);
    }
    assert!(r.text.contains("\\paperw12240\\paperh15840\\margl1440\\margr1440\\margt1440\\margb1440"), "{}", r.text);
    // 24 pt heading: Heading 1 by style and directly, 48 half-points, bold; the red 14 pt line is Heading 2.
    assert!(r.text.contains("\\s1\\sb240\\keepn\\outlinelevel0\\b\\fs48"), "{}", r.text);
    assert!(r.text.contains("\\s2\\sb200\\keepn\\outlinelevel1\\b\\fs28\\cf2"), "{}", r.text);
    assert!(r.text.contains("\\red204\\green26\\blue26;") && r.text.contains("\\red0\\green0\\blue255;"));
    // The table: three rows of three cells, the picture 4 x 3 px placed 200 x 100 pt (4000 x 2000 twips).
    assert_eq!(word_count(&toks, "trowd"), 3);
    assert_eq!(word_count(&toks, "row"), 3);
    assert_eq!(word_count(&toks, "cellx"), 9);
    assert!(r.text.contains("\\picw4\\pich3\\picwgoal4000\\pichgoal2000"), "{}", r.text);
    // The break is on the next page's first paragraph: no paragraph of its own, no \page.
    assert!(r.text.contains("{\\pard\\plain\\pagebb\\sa120\\fs22\\i\\cf"), "{}", r.text);
    assert_eq!(word_count(&toks, "pagebb"), 1);
    assert!(!has_word(&toks, "page"));
    assert!(!r.text.contains("\\pagebb\\par}"), "no empty paragraph for a break");
    assert!(r.text.contains("\\cellx"));
    // Hex of the PNG signature right after the picture header.
    assert!(r.text.contains("89504e470d0a1a0a"));
    let plain = rtf_plain(&toks);
    for want in ["Årsrapport 日本語 😀 \\{x}", "Overview", "Tabbed.", "\t", "r0c0", "r2c2", "Second page"] {
        assert!(plain.contains(want), "{want:?} not in {plain:?}");
    }
    assert!(plain.contains("Body text, long enough to be the main text size of this page ok.\tTabbed."));
    assert!(!plain.contains("Report"), "the title stays in \\info");
}

#[test]
fn rtf_grey_text_keeps_its_colour_and_a_blank_page_stays_a_page() {
    use super::rtf::rtf;
    let grey = 153.0 / 255.0;
    let page = |blocks| flow::Page { width: 612.0, height: 792.0, blocks, images: vec![] };
    let r = rtf(
        &[
            page(vec![model_block("Grey", 72.0, 700.0, 11.0, false, [grey; 3])]),
            page(vec![]),
            page(vec![model_block("Last", 72.0, 700.0, 11.0, false, [0.0; 3])]),
        ],
        "t",
    );
    // #999999 is colour table entry 1 (the border grey is the same colour), not black.
    assert!(r.text.contains("\\fs22\\i\\cf1 Grey"), "{}", r.text);
    assert!(r.text.contains("{\\colortbl;\\red153\\green153\\blue153;}"), "{}", r.text);
    // Page 2 is empty: its own paragraph holds the break, page 3's text carries the next one.
    let toks = rtf_tokens(&r.text);
    assert_eq!(word_count(&toks, "pagebb"), 2, "{}", r.text);
    assert!(r.text.contains("{\\pard\\plain\\pagebb\\par}\n{\\pard\\plain\\pagebb\\sa120\\fs22\\i Last"), "{}", r.text);
}

#[test]
fn rtf_a_table_after_a_page_break_is_a_separate_table_and_the_break_is_not_in_a_row() {
    use super::rtf::rtf;
    let grid = |y: f64| -> Vec<flow::Block> {
        (0..6).map(|i| model_block(&format!("t{i}"), 72.0 + 100.0 * (i % 3) as f64, y - 20.0 * (i / 3) as f64, 11.0, false, [0.0; 3])).collect()
    };
    let page = |blocks| flow::Page { width: 612.0, height: 792.0, blocks, images: vec![] };
    let r = rtf(&[page(grid(700.0)), page(grid(700.0))], "t");
    let toks = rtf_tokens(&r.text);
    assert_eq!(word_count(&toks, "trowd"), 4, "{}", r.text);
    // Between the first table's last \row and the second \trowd: a paragraph outside any table with the break.
    assert!(r.text.contains("\\row\n{\\pard\\plain\\pagebb\\par}\n\\trowd"), "{}", r.text);
    // No \pagebb inside a row: every row text, from \trowd to \row, is free of it.
    for row in r.text.split("\\trowd").skip(1) {
        let row = &row[..row.find("\\row").unwrap()];
        assert!(!row.contains("pagebb") && row.contains("\\intbl"), "{row}");
    }
    assert_eq!(word_count(&toks, "pagebb"), 1);
    assert!(balanced(&toks));
}

#[test]
fn rtf_pictures_over_the_cap_are_skipped_and_counted() {
    use super::rtf::{rtf, MAX_IMAGE_BYTES};
    let mut big = model_png();
    big.resize(MAX_IMAGE_BYTES - 10, 0); // a valid header, the rest is padding after IEND
    let im = |bytes: Vec<u8>| flow::Image { ext: "png", bytes, rect: [72.0, 300.0, 272.0, 400.0] };
    let page = flow::Page { width: 612.0, height: 792.0, blocks: vec![], images: vec![im(big), im(model_png()), im(b"not a png".to_vec())] };
    let r = rtf(&[page], "t");
    // The first fits, the small one would push past the cap (hex of 32 MB is written once), the text is not a PNG.
    assert_eq!(word_count(&rtf_tokens(&r.text), "pngblip"), 1, "one picture is kept");
    assert_eq!(r.skipped_images, 2);
}

#[test]
fn rtf_export_writes_one_parsable_file_with_the_text() {
    let d = dir("rtf");
    let (eng, doc, _) = open_fixture(&d, 2);
    let to = d.join("out.rtf");
    let out = run_export(&eng, doc, ExportKind::Rtf, None, Target::File(to.clone())).unwrap();
    assert_eq!(out.files.len(), 1);
    assert!(out.warnings.is_empty());
    assert_eq!(ExportKind::Rtf.extension(), "rtf");
    assert!(!ExportKind::Rtf.is_folder());
    let text = fs::read_to_string(&to).unwrap();
    assert!(text.starts_with("{\\rtf1\\ansi\\ansicpg1252\\deff0"));
    let toks = rtf_tokens(&text);
    assert!(balanced(&toks));
    let plain = rtf_plain(&toks);
    assert!(plain.contains("Inkwell fixture page 1") && plain.contains("Inkwell fixture page 2"), "{plain:?}");
    assert_eq!(word_count(&toks, "pagebb"), 1, "one break between two pages");
    // macOS ships textutil: a real RTF reader must agree on the text (skipped where it is absent).
    if let Ok(o) = std::process::Command::new("textutil").args(["-convert", "txt", "-stdout"]).arg(&to).output() {
        if o.status.success() {
            let t = String::from_utf8_lossy(&o.stdout);
            assert!(t.contains("Inkwell fixture page 1") && t.contains("Inkwell fixture page 2"), "{t:?}");
        }
    }
}

// ---------------------------------------------------------------- create

fn png(w: u32, h: u32, shade: u8, path: &Path) {
    let buf: Vec<u8> = (0..w * h * 3).map(|i| if i % 3 == 0 { shade } else { 128 }).collect();
    image::save_buffer(path, &buf, w, h, image::ColorType::Rgb8).unwrap();
}

fn make(source: CreateSource, size: PageSize, margin: f32) -> Result<create::CreateOutput, Error> {
    let req = CreateRequest { source: RequestSource::Clipboard, page_size: size, margin, font_size: None };
    create::create(&source, &req, &no_cancel(), &mut |_| {})
}

fn page_sizes(pdf: &str) -> Vec<(f32, f32)> {
    let d = mupdf::Document::open(pdf).unwrap();
    (0..d.page_count().unwrap()).map(|i| {
        let b = d.load_page(i).unwrap().bounds().unwrap();
        (b.width(), b.height())
    }).collect()
}

#[test]
fn three_images_make_three_pages_of_the_right_size() {
    let d = dir("three");
    let files: Vec<PathBuf> = [(200, 100), (100, 300), (150, 150)]
        .iter()
        .enumerate()
        .map(|(i, &(w, h))| {
            let p = d.join(format!("img{i}.png"));
            png(w, h, 40 * i as u8, &p);
            p
        })
        .collect();

    // Fit: the page is the image (MuPDF assumes 96 dpi when the file states none: 0.75 pt per pixel)
    let r = make(CreateSource::Images(files.clone()), PageSize::Fit, 0.0).unwrap();
    assert_eq!(r.pages, 3);
    assert_eq!(page_sizes(&r.path), [(150.0, 75.0), (75.0, 225.0), (112.5, 112.5)]);
    assert!(r.path.contains("inkwell-new"));
    assert!(r.path.ends_with("img0.pdf"));
    create::discard_scratch(Path::new(&r.path));

    // margins grow the page
    let r = make(CreateSource::Images(files.clone()), PageSize::Fit, 10.0).unwrap();
    assert_eq!(page_sizes(&r.path)[0], (170.0, 95.0));
    create::discard_scratch(Path::new(&r.path));

    // A4: portrait, landscape for the wide image; small images keep their size and sit centred
    let r = make(CreateSource::Images(files), PageSize::A4, 36.0).unwrap();
    assert_eq!(r.pages, 3);
    assert_eq!(page_sizes(&r.path), [(842.0, 595.0), (595.0, 842.0), (595.0, 842.0)]);
    // the picture is on the page
    let doc = mupdf::Document::open(&r.path).unwrap();
    let pix = doc.load_page(1).unwrap().to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_rgb(), false, true).unwrap();
    let c = pix.pixel(297, 421).unwrap();
    assert_ne!(c.components(), [255, 255, 255], "the image is not drawn");
    create::discard_scratch(Path::new(&r.path));
}

#[test]
fn a_jpeg_is_embedded_as_it_is() {
    let d = dir("jpeg");
    let p = d.join("photo.jpg");
    let buf: Vec<u8> = (0..64 * 48 * 3).map(|i| (i % 251) as u8).collect();
    image::save_buffer(&p, &buf, 64, 48, image::ColorType::Rgb8).unwrap();
    let r = make(CreateSource::Images(vec![p.clone()]), PageSize::Fit, 0.0).unwrap();
    let pdf = fs::read(&r.path).unwrap();
    let jpg = fs::read(&p).unwrap();
    assert!(pdf.windows(jpg.len()).any(|w| w == &jpg[..]), "the JPEG bytes are in the PDF untouched");
    create::discard_scratch(Path::new(&r.path));
}

#[test]
fn a_multi_page_tiff_gives_a_page_per_image() {
    let d = dir("tiff");
    let p = d.join("scan.tif");
    {
        use tiff::encoder::{colortype, TiffEncoder};
        let mut f = fs::File::create(&p).unwrap();
        let mut enc = TiffEncoder::new(&mut f).unwrap();
        enc.write_image::<colortype::Gray8>(40, 20, &vec![200u8; 800]).unwrap();
        enc.write_image::<colortype::RGB8>(30, 30, &vec![90u8; 30 * 30 * 3]).unwrap();
        enc.write_image::<colortype::Gray8>(10, 50, &vec![10u8; 500]).unwrap();
    }
    let r = make(CreateSource::Images(vec![p]), PageSize::Fit, 0.0).unwrap();
    assert_eq!(r.pages, 3);
    assert_eq!(page_sizes(&r.path), [(40.0, 20.0), (30.0, 30.0), (10.0, 50.0)]);
    create::discard_scratch(Path::new(&r.path));
}

#[test]
fn text_becomes_wrapped_pages_with_turkish_letters_intact() {
    let d = dir("text-pdf");
    let p = d.join("notes.txt");
    let line = "Işık ğüşöç İstanbul çocuk the quick brown fox jumps over the lazy dog. ";
    // cp1254 bytes, as an old Windows editor writes them
    let mut text = String::new();
    for i in 0..400 {
        text.push_str(&format!("{i}: {line}\n"));
    }
    fs::write(&p, text.as_bytes()).unwrap();
    let r = make(CreateSource::Text(p.clone()), PageSize::A4, 0.0).unwrap();
    assert!(r.pages >= 4, "{} pages", r.pages);
    assert!(page_sizes(&r.path).iter().all(|&s| s == (595.0, 842.0)));
    let doc = mupdf::Document::open(&r.path).unwrap();
    let t = doc.load_page(0).unwrap().to_text_page(mupdf::text_page::TextPageFlags::empty()).unwrap().to_text().unwrap();
    assert!(t.contains("Işık ğüşöç İstanbul"), "{t}");
    create::discard_scratch(Path::new(&r.path));
}

#[test]
fn blank_pdf_and_limits() {
    let r = make(CreateSource::Blank(3), PageSize::Letter, 0.0).unwrap();
    assert_eq!(r.pages, 3);
    assert_eq!(page_sizes(&r.path), [(612.0, 792.0); 3]);
    create::discard_scratch(Path::new(&r.path));
    assert!(matches!(make(CreateSource::Blank(0), PageSize::A4, 0.0), Err(Error::Invalid(_))));
    assert!(matches!(make(CreateSource::Images(vec![]), PageSize::A4, 0.0), Err(Error::Invalid(_))));
    assert!(matches!(make(CreateSource::Blank(1), PageSize::A4, 500.0), Err(Error::Invalid(_))));
    let d = dir("notimage");
    let p = d.join("x.png");
    fs::write(&p, b"this is not a picture").unwrap();
    let e = make(CreateSource::Images(vec![p]), PageSize::Fit, 0.0).unwrap_err();
    assert!(e.to_string().contains("x.png"), "{e}");
}

#[test]
fn clipboard_pixels_become_a_page() {
    let mut doc = mupdf::pdf::PdfDocument::new();
    let req = CreateRequest { source: RequestSource::Clipboard, page_size: PageSize::Fit, margin: 0.0, font_size: None };
    // 2 x 1 pixels, the second one transparent: composited over white
    let rgba = [255u8, 0, 0, 255, 0, 0, 255, 0];
    create::rgba_page(&mut doc, 2, 1, &rgba, &req).unwrap();
    assert_eq!(doc.page_count().unwrap(), 1);
}

#[test]
fn text_decoding_follows_the_bom_and_the_code_page() {
    assert_eq!(create::decode_text(b"\xEF\xBB\xBFabc"), "abc");
    assert_eq!(create::decode_text(b"\xFF\xFEa\0b\0"), "ab");
    assert_eq!(create::decode_text("åäö".as_bytes()), "åäö");
    // windows-1254: Ğ ş ı
    assert_eq!(create::decode_text(b"\xD0\xFE\xFD"), "Ğşı");
    // windows-1252: the same bytes beside á are Icelandic letters
    assert_eq!(create::decode_text(b"\xD0\xE1"), "Ðá");
}

// ---------------------------------------------------------------- cancel

#[test]
fn a_cancelled_export_leaves_nothing_behind() {
    let d = dir("cancel");
    let (eng, doc, _) = open_fixture(&d, 6);
    let out = d.join("o");
    fs::create_dir_all(&out).unwrap();
    for kind in [
        ExportKind::Images { format: ImageFormat::Png, dpi: 72, gray: false, quality: 90 },
        ExportKind::Images { format: ImageFormat::Jpeg, dpi: 72, gray: false, quality: 90 },
    ] {
        // cancelled before it starts
        let flag = AtomicBool::new(true);
        let req = ExportRequest { doc, kind: kind.clone(), pages: None, save_to: String::new() };
        let r = export(&eng, &req, Target::Folder(out.clone()), &flag, &mut |_| {});
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(names(&out).is_empty(), "{:?}", names(&out));

        // cancelled after the third page was written
        let flag = AtomicBool::new(false);
        let r = export(&eng, &req, Target::Folder(out.clone()), &flag, &mut |p: Progress| {
            if p.done == 3 {
                flag.store(true, Ordering::Relaxed);
            }
        });
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(names(&out).is_empty(), "files left: {:?}", names(&out));
    }
    // a single file is not there either
    let to = d.join("t.txt");
    let flag = AtomicBool::new(false);
    let req = ExportRequest { doc, kind: ExportKind::Text { layout: true }, pages: None, save_to: String::new() };
    let r = export(&eng, &req, Target::File(to.clone()), &flag, &mut |p: Progress| {
        if p.done == 2 {
            flag.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(r, Err(Error::Cancelled)));
    assert!(!to.exists());
    assert!(names(&d).iter().all(|n| !n.starts_with(".inkwell-convert")), "{:?}", names(&d));
}

#[test]
fn a_cancelled_create_leaves_no_file() {
    let d = dir("cancel-create");
    let files: Vec<PathBuf> = (0..4)
        .map(|i| {
            let p = d.join(format!("{i}.png"));
            png(50, 50, 10, &p);
            p
        })
        .collect();
    let before: Vec<String> = fs::read_dir(create::scratch_root()).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    let flag = AtomicBool::new(false);
    let req = CreateRequest { source: RequestSource::Clipboard, page_size: PageSize::Fit, margin: 0.0, font_size: None };
    let r = create::create(&CreateSource::Images(files), &req, &flag, &mut |p: Progress| {
        if p.done == 2 {
            flag.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(r, Err(Error::Cancelled)));
    let after: Vec<String> = fs::read_dir(create::scratch_root()).map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    // other tests create scratch folders at the same time: only a leftover of this job would hold "0.pdf"
    for n in after.iter().filter(|n| !before.contains(n)) {
        let inner = names(&create::scratch_root().join(n));
        assert!(!inner.iter().any(|f| f == "0.pdf" || f.ends_with(".tmp")), "{n}: {inner:?}");
    }
}

#[test]
fn discard_only_touches_the_scratch_folder() {
    let d = dir("discard");
    let stray = d.join("keep.pdf");
    fs::write(&stray, b"x").unwrap();
    assert!(!create::discard_scratch(&stray));
    assert!(stray.exists());
    let r = make(CreateSource::Blank(1), PageSize::A4, 0.0).unwrap();
    assert!(create::discard_scratch(Path::new(&r.path)));
    assert!(!Path::new(&r.path).exists());
}

#[test]
fn file_names_are_made_safe() {
    assert_eq!(safe_stem("a/b:c?"), "a_b_c_");
    assert_eq!(safe_stem("  "), "document");
    assert_eq!(safe_stem("name."), "name");
}
