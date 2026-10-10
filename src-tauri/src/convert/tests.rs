//! Export and create, end to end on scratch files (the fixtures are built here, never the user's files).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::create::{self, CreateRequest, CreateSource, PageSize, RequestSource};
use super::export::{ExportKind, ExportRequest, ImageFormat, Target};
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
