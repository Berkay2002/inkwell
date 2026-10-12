//! The custom stamp library and placing a stamp: sources, caps, a hostile index, and the edit paths.

use super::*;
use crate::annot::{Kind, Patch, Style, add_one, delete_many, edit_one, list_all};
use crate::engine::edit::EditSource;
use crate::engine::fixture::{self, Fixture};
use crate::engine::{Engine, EngineError};
use crate::outline::tests::{dir, doc_mdp, open_bytes, raw_pdf, sig_value};

fn user() -> EditSource {
    EditSource::User
}

/// A `w x h` PNG: the left half opaque red, the right half fully transparent.
pub(crate) fn half_red(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, _| if x < w / 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 0]) });
    encode_png(&img).unwrap()
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for b in data {
        c ^= *b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

pub(crate) fn write(d: &Path, name: &str, bytes: &[u8]) -> String {
    let p = d.join(name);
    std::fs::write(&p, bytes).unwrap();
    p.to_string_lossy().into_owned()
}

/// A library folder with one stamp (`half_red(200, 100)`), and its picture.
pub(crate) fn one_stamp(d: &Path) -> (PathBuf, Stamp, Vec<u8>) {
    let lib = d.join("lib");
    let src = write(d, "src.png", &half_red(200, 100));
    let s = create(&lib, &Source { path: &src, page: None }, "Approved by me", "Mine").unwrap();
    let (_, png) = picture(&lib, &s.id).unwrap();
    (lib, s, png)
}

fn err(r: Result<impl std::fmt::Debug>) -> String {
    match r {
        Err(EngineError::Invalid(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn pixels(eng: &Engine, id: crate::engine::DocId) -> (Vec<u8>, usize, usize) {
    let bytes = eng.unsaved_bytes(id).unwrap().expect("an edit is pending");
    let doc = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let pix = doc.load_page(0).unwrap().to_pixmap(&Matrix::IDENTITY, &Colorspace::device_rgb(), false, true).unwrap();
    (pix.samples().to_vec(), pix.stride() as usize, pix.n() as usize)
}

fn at(p: &(Vec<u8>, usize, usize), x: usize, y: usize) -> [u8; 3] {
    let i = y * p.1 + x * p.2;
    [p.0[i], p.0[i + 1], p.0[i + 2]]
}

// ---- the library ---------------------------------------------------------------------------

#[test]
fn a_png_becomes_a_normalised_stamp_with_its_alpha_and_none_of_the_source_bytes() {
    let d = dir("stamps", "png");
    // The picked file carries a text chunk (metadata); the stored picture must not.
    let mut src = half_red(300, 150);
    let chunk = b"tEXtSecret\0do not keep";
    let mut ext = ((chunk.len() - 4) as u32).to_be_bytes().to_vec();
    ext.extend_from_slice(chunk);
    ext.extend_from_slice(&crc32(chunk).to_be_bytes());
    src.splice(33..33, ext);
    let path = write(&d, "logo.png", &src);
    let lib = d.join("lib");
    let s = create(&lib, &Source { path: &path, page: None }, "  Logo \u{7}", "").unwrap();
    assert_eq!((s.name.as_str(), s.category.as_str(), s.width, s.height), ("Logo", "Custom", 300, 150));
    assert!(valid_id(&s.id));
    let (got, png) = picture(&lib, &s.id).unwrap();
    assert_eq!(got, s);
    let back = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(back.dimensions(), (300, 150));
    assert_eq!(back.get_pixel(10, 10).0, [255, 0, 0, 255]);
    assert_eq!(back.get_pixel(250, 10).0[3], 0, "transparency is kept");
    // Made by our encoder from the pixels: not the picked bytes, and the picked file is not touched.
    let stored = std::fs::read(png_path(&lib, &s.id)).unwrap();
    assert!(src.windows(6).any(|w| w == b"Secret") && !stored.windows(6).any(|w| w == b"Secret"));
    assert_eq!(std::fs::read(&path).unwrap(), src);
    assert!(thumbnail(&lib, &s.id).unwrap().len() > 8);
    // The library lists it, and nothing else is in the folder but its files.
    assert_eq!(list(&lib).stamps, vec![s.clone()]);
    assert_eq!(std::fs::read_dir(&lib).unwrap().count(), 3, "the picture, its thumbnail and the index");
}

/// A JPEG whose EXIF says "rotate 90 degrees clockwise" (orientation 6), 40 x 20 as stored.
fn rotated_jpeg() -> Vec<u8> {
    use image::{ExtendedColorType, ImageEncoder};
    let mut jpg = Vec::new();
    let rgb = vec![200u8; 40 * 20 * 3];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90).write_image(&rgb, 40, 20, ExtendedColorType::Rgb8).unwrap();
    let mut exif: Vec<u8> = b"Exif\0\0MM\0*\0\0\0\x08".to_vec();
    exif.extend([0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
    let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
    out.extend(((exif.len() + 2) as u16).to_be_bytes());
    out.extend(exif);
    out.extend(&jpg[2..]);
    out
}

#[test]
fn a_jpeg_is_turned_upright_by_its_exif_orientation() {
    let d = dir("stamps", "jpeg");
    let plain = write(&d, "a.jpg", &{
        use image::{ExtendedColorType, ImageEncoder};
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpg).write_image(&vec![90u8; 40 * 20 * 3], 40, 20, ExtendedColorType::Rgb8).unwrap();
        jpg
    });
    let lib = d.join("lib");
    let s = create(&lib, &Source { path: &plain, page: None }, "Plain", "").unwrap();
    assert_eq!((s.width, s.height), (40, 20));
    let rotated = write(&d, "b.jpg", &rotated_jpeg());
    let r = create(&lib, &Source { path: &rotated, page: None }, "Rotated", "").unwrap();
    assert_eq!((r.width, r.height), (20, 40), "orientation 6 swaps the sides");
}

#[test]
fn other_formats_go_through_mupdf() {
    let d = dir("stamps", "tiff");
    let mut tif = Vec::new();
    {
        use tiff::encoder::{TiffEncoder, colortype};
        let mut enc = TiffEncoder::new(Cursor::new(&mut tif)).unwrap();
        enc.write_image::<colortype::RGB8>(30, 10, &vec![40u8; 30 * 10 * 3]).unwrap();
    }
    let path = write(&d, "a.tif", &tif);
    let s = create(&d.join("lib"), &Source { path: &path, page: None }, "Scan", "").unwrap();
    assert_eq!((s.width, s.height), (30, 10));
}

#[test]
fn a_pdf_page_becomes_a_stamp_transparent_where_the_page_paints_nothing() {
    let d = dir("stamps", "pdf");
    let pdf = write(&d, "a.pdf", &fixture::build(&Fixture { pages: 2, lines: 3, ..Default::default() }));
    let lib = d.join("lib");
    let s = create(&lib, &Source { path: &pdf, page: Some(1) }, "Page two", "From PDF").unwrap();
    assert!(s.width.max(s.height) <= MAX_SIDE && s.width > 100, "{s:?}");
    let img = image::load_from_memory(&picture(&lib, &s.id).unwrap().1).unwrap().to_rgba8();
    assert!(img.pixels().any(|p| p[3] == 0), "unpainted parts of the page are transparent");
    assert!(img.pixels().any(|p| p[3] == 255), "the text is opaque");
    // The margins were cut: the page is 612 pt wide, which is 2550 px at 300 dpi.
    assert!(s.width < 2550, "{s:?}");
    assert!(err(create(&lib, &Source { path: &pdf, page: Some(5) }, "x", "")).contains("2 pages"));
}

#[test]
fn a_picture_that_mentions_pdf_in_its_metadata_is_still_a_picture() {
    let d = dir("stamps", "pdf-text-in-picture");
    let lib = d.join("lib");
    // The text is in a PNG text chunk before the pixels, and in a JPEG comment segment.
    let mut png = half_red(8, 4);
    let chunk_type_and_data = b"tEXtComment\0see %PDF-1.7 for details";
    let mut chunk = (chunk_type_and_data.len() as u32 - 4).to_be_bytes().to_vec();
    chunk.extend_from_slice(chunk_type_and_data);
    chunk.extend_from_slice(&crc32(chunk_type_and_data).to_be_bytes());
    png.splice(33..33, chunk);
    assert!(!is_pdf(&png) && png.windows(5).any(|w| w == b"%PDF-"));
    let s = create(&lib, &Source { path: &write(&d, "a.png", &png), page: None }, "Png", "").unwrap();
    assert_eq!((s.width, s.height), (8, 4));

    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpg).encode(&[200u8; 8 * 4 * 3], 8, 4, image::ExtendedColorType::Rgb8).unwrap();
    let com = b"%PDF-1.4 hidden";
    let mut seg = vec![0xff, 0xfe];
    seg.extend_from_slice(&(com.len() as u16 + 2).to_be_bytes());
    seg.extend_from_slice(com);
    jpg.splice(2..2, seg);
    assert!(!is_pdf(&jpg));
    let s = create(&lib, &Source { path: &write(&d, "a.jpg", &jpg), page: None }, "Jpeg", "").unwrap();
    assert_eq!((s.width, s.height), (8, 4));
    // A real PDF, with or without junk before its header, is still one.
    assert!(is_pdf(b"%PDF-1.7\n") && is_pdf(b"junk\n%PDF-1.4"));

    // A BMP whose pixel data holds the text: 4 x 2, 24 bits, rows of 12 bytes.
    let mut bmp = b"BM".to_vec();
    bmp.extend_from_slice(&(54u32 + 24).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&4i32.to_le_bytes());
    bmp.extend_from_slice(&2i32.to_le_bytes());
    bmp.extend_from_slice(&[1, 0, 24, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x13, 0x0b, 0, 0, 0x13, 0x0b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    bmp.extend_from_slice(b"%PDF-1.4 hidden!");
    bmp.resize(54 + 24, 200);
    assert!(!is_pdf(&bmp));
    let s = create(&lib, &Source { path: &write(&d, "a.bmp", &bmp), page: None }, "Bmp", "").unwrap();
    assert_eq!((s.width, s.height), (4, 2));
}

#[test]
fn a_page_with_a_huge_picture_is_refused_before_it_is_drawn() {
    let page = |objs: &[&str]| {
        let mut v = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /XObject << /Im 5 0 R /Fm 6 0 R >> >> >>".to_string(),
            "<< /Length 28 >>\nstream\nq 100 0 0 100 0 0 cm /Im Do Q\nendstream".to_string(),
        ];
        v.extend(objs.iter().map(|s| s.to_string()));
        raw_pdf(&v)
    };
    let image = |side: u32| format!("<< /Type /XObject /Subtype /Image /Width {side} /Height {side} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 64 >>\nstream\n{}\nendstream", "x".repeat(64));
    let plain_form = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 0 >>\nstream\n\nendstream";
    // 10000 x 10000 on the page itself.
    let e = err(render_pdf(&page(&[&image(10_000), plain_form]), None, RENDER_LIMIT));
    assert!(e.contains("too many pixels"), "{e}");
    // The same inside a form the page draws.
    let form = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Big 7 0 R >> >> /Length 0 >>\nstream\n\nendstream";
    let e = err(render_pdf(&page(&[&image(8), form, &image(10_000)]), None, RENDER_LIMIT));
    assert!(e.contains("too many pixels"), "{e}");
    // A small picture draws.
    render_pdf(&page(&[&image(8), plain_form]), None, RENDER_LIMIT).unwrap();
    // A size that cannot be read counts as over the cap: a real, a negative, a missing one, a name.
    for (w, h) in [("8.5", "8"), ("-8", "8"), ("/Big", "8"), ("8", "/Big")] {
        let im = format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 64 >>\nstream\n{}\nendstream", "x".repeat(64));
        let e = err(render_pdf(&page(&[&im, plain_form]), None, RENDER_LIMIT));
        assert!(e.contains("size cannot be read"), "{w} x {h}: {e}");
    }
    let no_width = format!("<< /Type /XObject /Subtype /Image /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 64 >>\nstream\n{}\nendstream", "x".repeat(64));
    assert!(err(render_pdf(&page(&[&no_width, plain_form]), None, RENDER_LIMIT)).contains("size cannot be read"));
}

#[test]
fn a_page_that_takes_too_long_to_draw_is_aborted() {
    // A page of 150 000 small squares: far more work than a zero time limit allows.
    let content = "0 0 5 5 re f\n".repeat(150_000);
    let pdf = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
    ]);
    assert!(err(render_pdf(&pdf, None, Duration::ZERO)).contains("too long"));
    // The same page within the real limit draws.
    assert!(render_pdf(&pdf, None, RENDER_LIMIT).is_ok());
    // A time limit does not disturb a page that is quick.
    let quick = fixture::build(&Fixture { pages: 1, lines: 3, ..Default::default() });
    assert!(render_pdf(&quick, None, RENDER_LIMIT).is_ok());
}

#[test]
fn the_caps_hold() {
    let d = dir("stamps", "caps");
    let lib = d.join("lib");
    let small = write(&d, "s.png", &half_red(4, 4));
    let mk = |name: &str| create(&lib, &Source { path: &small, page: None }, name, "");

    // Names and categories.
    assert!(mk(&"n".repeat(MAX_NAME)).is_ok());
    assert!(err(mk(&"n".repeat(MAX_NAME + 1))).contains("128"));
    assert!(err(create(&lib, &Source { path: &small, page: None }, "ok", &"c".repeat(MAX_CATEGORY + 1))).contains("64"));
    assert!(err(mk("  \t ")).contains("name"));
    // A multi-byte name is counted in characters.
    assert!(mk(&"\u{e9}".repeat(MAX_NAME)).is_ok());

    // A file over 20 MB is refused before it is read.
    let big = d.join("big.png");
    std::fs::File::create(&big).unwrap().set_len(MAX_SOURCE + 1).unwrap();
    assert!(err(create(&lib, &Source { path: &big.to_string_lossy(), page: None }, "big", "")).contains("20 MB"));

    // Too many pixels are refused before anything is decoded: a tiny PNG whose header says 9000 x 9000 (81
    // megapixels, under the decoder's own allocation limit, so it is our cap that answers), and the same in a TIFF.
    let mut bomb = half_red(4, 4);
    bomb[16..20].copy_from_slice(&9000u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&9000u32.to_be_bytes());
    let crc = crc32(&bomb[12..29]);
    bomb[29..33].copy_from_slice(&crc.to_be_bytes());
    assert!(err(create(&lib, &Source { path: &write(&d, "bomb.png", &bomb), page: None }, "bomb", "")).contains("megapixels"));

    let mut tif = Vec::new();
    {
        use tiff::encoder::{TiffEncoder, colortype};
        TiffEncoder::new(Cursor::new(&mut tif)).unwrap().write_image::<colortype::RGB8>(30, 10, &vec![40u8; 30 * 10 * 3]).unwrap();
    }
    assert_eq!(&tif[..4], b"II*\0", "little endian");
    let ifd = u32::from_le_bytes(tif[4..8].try_into().unwrap()) as usize;
    let entries = u16::from_le_bytes(tif[ifd..ifd + 2].try_into().unwrap()) as usize;
    let mut patched = 0;
    for e in 0..entries {
        let at = ifd + 2 + e * 12;
        let (tag, kind) = (u16::from_le_bytes(tif[at..at + 2].try_into().unwrap()), u16::from_le_bytes(tif[at + 2..at + 4].try_into().unwrap()));
        if tag == 256 || tag == 257 {
            match kind {
                3 => tif[at + 8..at + 10].copy_from_slice(&9000u16.to_le_bytes()),
                4 => tif[at + 8..at + 12].copy_from_slice(&9000u32.to_le_bytes()),
                k => panic!("unexpected tiff type {k}"),
            }
            patched += 1;
        }
    }
    assert_eq!(patched, 2);
    assert!(err(create(&lib, &Source { path: &write(&d, "bomb.tif", &tif), page: None }, "bomb", "")).contains("megapixels"));

    // A page of 14400 pt (the largest a viewer takes) is drawn at a scale that keeps it within 4096 px.
    let big = raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 14400 14400] /Contents 4 0 R >>".to_string(),
        "<< /Length 31 >>\nstream\n0 0 0 rg 100 100 9000 3000 re f\nendstream".to_string(),
    ]);
    let s = create(&lib, &Source { path: &write(&d, "huge.pdf", &big), page: None }, "huge", "").unwrap();
    assert!(s.width.max(s.height) <= MAX_SIDE && s.width > 1000, "{s:?}");

    // A long picture is scaled down to 4096 on its longest side.
    let wide = write(&d, "wide.png", &half_red(6000, 300));
    let s = create(&lib, &Source { path: &wide, page: None }, "wide", "").unwrap();
    assert_eq!((s.width, s.height), (4096, 205));

    // Not a picture.
    assert!(err(create(&lib, &Source { path: &write(&d, "x.png", b"hello"), page: None }, "x", "")).contains("not a picture"));

    // The 201st stamp.
    let have = list(&lib).stamps.len();
    for i in have..MAX_STAMPS {
        mk(&format!("s{i}")).unwrap();
    }
    assert_eq!(list(&lib).stamps.len(), MAX_STAMPS);
    assert!(err(mk("one more")).contains("200"));
    assert_eq!(std::fs::read_dir(&lib).unwrap().count(), MAX_STAMPS * 2 + 1, "a refused stamp leaves no files");
}

#[cfg(unix)]
#[test]
fn only_a_regular_file_is_read_as_a_source() {
    let d = dir("stamps", "special");
    let lib = d.join("lib");
    let mk = |p: &std::path::Path| create(&lib, &Source { path: &p.to_string_lossy(), page: None }, "x", "");
    // A symlink to a device that never ends, a directory, and a FIFO (opening it would block): refused, not read.
    let zero = d.join("zero.png");
    std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
    assert!(err(mk(&zero)).contains("not a file"));
    assert!(err(mk(&d)).contains("not a file"));
    let fifo = d.join("pipe.png");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    assert!(err(mk(&fifo)).contains("not a file"));
    assert!(!lib.join(INDEX).exists(), "nothing was made");
    // A link to a real picture still works.
    let real = write(&d, "real.png", &half_red(4, 4));
    let link = d.join("link.png");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(mk(&link).is_ok());
}

#[test]
fn rename_recategorise_and_delete() {
    let d = dir("stamps", "manage");
    let (lib, s, _) = one_stamp(&d);
    let r = update(&lib, &s.id, Some(" Renamed "), Some("Work")).unwrap();
    assert_eq!((r.name.as_str(), r.category.as_str()), ("Renamed", "Work"));
    assert_eq!(update(&lib, &s.id, None, Some("")).unwrap().category, "Custom");
    assert!(err(update(&lib, &s.id, Some(""), None)).contains("name"));
    assert!(err(update(&lib, &s.id, Some(&"x".repeat(129)), None)).contains("128"));
    assert_eq!(list(&lib).stamps[0].name, "Renamed");
    delete(&lib, &s.id).unwrap();
    assert!(list(&lib).stamps.is_empty());
    assert!(!png_path(&lib, &s.id).exists() && !thumb_path(&lib, &s.id).exists());
    assert!(err(delete(&lib, &s.id)).contains("not in the library"));
}

#[test]
fn an_id_that_is_not_one_we_made_never_reaches_the_file_system() {
    let d = dir("stamps", "ids");
    let (lib, s, _) = one_stamp(&d);
    std::fs::write(d.join("secret.png"), b"x").unwrap();
    for bad_id in ["../secret", "..", "", "ABCDEF0123456789", "0123456789abcde", "0123456789abcdef0", &format!("{}/x", s.id)] {
        assert!(picture(&lib, bad_id).is_err(), "{bad_id}");
        assert!(thumbnail(&lib, bad_id).is_err(), "{bad_id}");
        assert!(delete(&lib, bad_id).is_err(), "{bad_id}");
        assert!(update(&lib, bad_id, Some("x"), None).is_err(), "{bad_id}");
    }
    assert!(d.join("secret.png").exists());
    assert_eq!(list(&lib).stamps.len(), 1);
}

#[test]
fn write_atomic_replaces_the_file_whole_and_leaves_no_temp_file_behind() {
    let d = dir("stamps", "atomic");
    let f = d.join("a.json");
    write_atomic(&f, b"one").unwrap();
    write_atomic(&f, b"two, longer").unwrap();
    assert_eq!(std::fs::read(&f).unwrap(), b"two, longer");
    // A target that cannot be replaced (a folder) is an error, and the temp file does not stay.
    std::fs::create_dir(d.join("b.json")).unwrap();
    assert!(write_atomic(&d.join("b.json"), b"x").is_err());
    let mut names: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["a.json", "b.json"]);
}

/// The damaged indexes a writer set aside.
fn backups(lib: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(lib).unwrap().map(|e| e.unwrap().path()).filter(|p| p.to_string_lossy().ends_with(".bad")).collect();
    v.sort();
    v
}

#[test]
fn a_damaged_index_is_set_aside_by_a_writer_and_a_second_one_keeps_the_first_backup() {
    let d = dir("stamps", "backups");
    let lib = d.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let src = write(&d, "s.png", &half_red(4, 4));
    for (i, junk) in [&b"first damage"[..], b"second damage"].into_iter().enumerate() {
        std::fs::write(lib.join(INDEX), junk).unwrap();
        assert!(list(&lib).warning.is_some());
        assert_eq!(backups(&lib).len(), i, "listing sets nothing aside");
        create(&lib, &Source { path: &src, page: None }, "S", "").unwrap();
        let b = backups(&lib);
        assert_eq!(b.len(), i + 1, "one backup per damage");
        assert!(b.iter().any(|p| std::fs::read(p).unwrap() == junk));
    }
}

#[test]
fn a_write_never_uses_a_temp_file_another_writer_may_have() {
    let d = dir("stamps", "temp-names");
    let lib = d.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    // What the fixed names of an earlier version, or another writer, may hold.
    for f in ["stamps.tmp", "stamps.json.tmp"] {
        std::fs::write(lib.join(f), b"someone else's").unwrap();
    }
    let src = write(&d, "s.png", &half_red(4, 4));
    let s = create(&lib, &Source { path: &src, page: None }, "S", "").unwrap();
    for f in ["stamps.tmp", "stamps.json.tmp"] {
        assert_eq!(std::fs::read(lib.join(f)).unwrap(), b"someone else's", "{f}");
    }
    let mut names: Vec<String> = std::fs::read_dir(&lib).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, [format!("{}.png", s.id), format!("{}.t.png", s.id), "stamps.json".into(), "stamps.json.tmp".into(), "stamps.tmp".into()]);
    // Two writers at once (threads here) both land and the index stays whole.
    let (lib_ref, src_ref) = (&lib, &src);
    let ids: Vec<String> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..4).map(|i| sc.spawn(move || create(lib_ref, &Source { path: src_ref, page: None }, &format!("T{i}"), "").unwrap().id)).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(list(&lib).stamps.len(), 1 + ids.len());
}

#[test]
fn a_writer_deletes_temp_files_older_than_a_minute_and_only_those() {
    let d = dir("stamps", "stale-temp");
    let lib = d.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let aged = |name: &str, secs: u64| {
        std::fs::write(lib.join(name), b"x").unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(lib.join(name)).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs)).unwrap();
    };
    aged("stamps.json.0123456789abcdef.tmp", 3600);
    aged("0123456789abcdef.png.fedcba9876543210.tmp", 120);
    aged("stamps.json.1111111111111111.tmp", 5);
    aged("keep.txt", 3600);
    // Not a name of ours, so not ours to delete.
    aged("notes.tmp", 3600);
    // A reader leaves them; a writer clears the old ones.
    list(&lib);
    assert_eq!(std::fs::read_dir(&lib).unwrap().count(), 5);
    let src = write(&d, "s.png", &half_red(4, 4));
    let s = create(&lib, &Source { path: &src, page: None }, "S", "").unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&lib).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, [format!("{}.png", s.id), format!("{}.t.png", s.id), "keep.txt".into(), "notes.tmp".into(), "stamps.json".into(), "stamps.json.1111111111111111.tmp".into()]);
}

#[test]
fn a_hostile_index_reads_as_an_empty_or_trimmed_library_and_never_panics() {
    let d = dir("stamps", "hostile");
    let lib = d.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let index = |bytes: &[u8]| std::fs::write(lib.join(INDEX), bytes).unwrap();

    // Garbage, truncated JSON, the wrong shape, a binary blob.
    for junk in [&b"not json"[..], b"{\"stamps\": [{\"id\": ", b"[1,2,3]", b"{\"stamps\": 7}", &[0xff, 0xfe, 0, 1, 2]] {
        index(junk);
        let l = list(&lib);
        assert!(l.stamps.is_empty() && l.warning.is_some(), "{junk:?}");
        assert!(lib.join(INDEX).exists() && backups(&lib).is_empty(), "reading moves nothing");
    }

    // Deep nesting (past the parser's depth limit), and a file over the size cap.
    index(format!("{}{}", "[".repeat(100_000), "]".repeat(100_000)).as_bytes());
    assert!(list(&lib).warning.is_some());
    index(format!("{{\"stamps\": [], \"pad\": \"{}\"}}", "x".repeat(MAX_INDEX as usize)).as_bytes());
    assert!(list(&lib).warning.is_some());

    // Huge strings inside the cap are cut, the ids are checked, duplicates and missing pictures are dropped.
    let real = "0123456789abcdef";
    std::fs::write(png_path(&lib, real), b"png").unwrap();
    let entry = |id: &str, name: &str, w: u32| serde_json::json!({ "id": id, "name": name, "category": "c".repeat(5000), "width": w, "height": 10 });
    let doc = serde_json::json!({ "stamps": [
        entry(real, &"n".repeat(900_000), 10),
        entry(real, "duplicate", 10),
        entry("../../etc/passwd", "path", 10),
        entry("fedcba9876543210", "no picture", 10),
        entry("0000000000000001", "zero", 0),
        entry("0000000000000002", "too wide", 5000),
    ] });
    index(doc.to_string().as_bytes());
    let l = list(&lib);
    assert_eq!(l.stamps.len(), 1);
    assert_eq!((l.stamps[0].name.chars().count(), l.stamps[0].category.chars().count()), (MAX_NAME, MAX_CATEGORY));
    assert!(l.warning.is_some());

    // More than 200 entries: the first 200.
    let many: Vec<_> = (0..300)
        .map(|i| {
            let id = format!("{i:016x}");
            std::fs::write(png_path(&lib, &id), b"png").unwrap();
            serde_json::json!({ "id": id, "name": format!("s{i}"), "category": "c", "width": 1, "height": 1 })
        })
        .collect();
    index(serde_json::json!({ "stamps": many }).to_string().as_bytes());
    assert_eq!(list(&lib).stamps.len(), MAX_STAMPS);

    // A stamp whose picture file is not the size the index says is refused at place time.
    let (_, s, _) = one_stamp(&dir("stamps", "hostile-png"));
    let lib2 = d.join("lib2");
    std::fs::create_dir_all(&lib2).unwrap();
    std::fs::write(png_path(&lib2, &s.id), half_red(8, 8)).unwrap();
    std::fs::write(lib2.join(INDEX), serde_json::json!({ "stamps": [s.clone()] }).to_string()).unwrap();
    assert!(err(picture(&lib2, &s.id)).contains("damaged"));
}

#[cfg(unix)]
#[test]
fn an_unreadable_index_stops_a_write_and_is_never_stored_over() {
    use std::os::unix::fs::PermissionsExt;
    let d = dir("stamps", "unreadable");
    let (lib, s, _) = one_stamp(&d);
    let index = lib.join(INDEX);
    let before = std::fs::read(&index).unwrap();
    std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Running as root reads it anyway: the test then has nothing to show.
    if std::fs::File::open(&index).is_ok() {
        return;
    }
    let src = write(&d, "n.png", &half_red(4, 4));
    assert!(create(&lib, &Source { path: &src, page: None }, "new", "").is_err());
    assert!(update(&lib, &s.id, Some("x"), None).is_err());
    assert!(delete(&lib, &s.id).is_err());
    assert!(list(&lib).warning.is_some_and(|w| w.contains("could not be read")), "the list says so");
    std::fs::set_permissions(&index, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(std::fs::read(&index).unwrap(), before, "the index was not touched");
    assert!(png_path(&lib, &s.id).exists());
    assert_eq!(list(&lib).stamps, vec![s]);
}

#[test]
fn a_picture_that_is_briefly_missing_is_left_out_of_the_list_but_not_forgotten_by_a_write() {
    let d = dir("stamps", "missing");
    let (lib, s, _) = one_stamp(&d);
    let held = std::fs::read(png_path(&lib, &s.id)).unwrap();
    std::fs::remove_file(png_path(&lib, &s.id)).unwrap();
    assert!(list(&lib).stamps.is_empty());
    // Another stamp is made while the first one's picture is away: the first stays in the index.
    let src = write(&d, "n.png", &half_red(4, 4));
    let n = create(&lib, &Source { path: &src, page: None }, "second", "").unwrap();
    std::fs::write(png_path(&lib, &s.id), held).unwrap();
    let ids: Vec<_> = list(&lib).stamps.into_iter().map(|x| x.id).collect();
    assert_eq!(ids, vec![s.id, n.id]);
}

// ---- placing -------------------------------------------------------------------------------

fn plain() -> Vec<u8> {
    fixture::build(&Fixture { pages: 2, lines: 4, ..Default::default() })
}

fn sig_doc(catalog: &str, field: &str) -> Vec<u8> {
    raw_pdf(&[
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
        field.to_string(),
    ])
}

fn certified(p: u32) -> Vec<u8> {
    let perms = format!("/Perms << /DocMDP << /Type /Sig /Reference [ {} ] >> >>", doc_mdp(&format!("/TransformParams << /P {p} >>")));
    sig_doc(&perms, "<< >>")
}

fn signed() -> Vec<u8> {
    sig_doc("/AcroForm << /SigFlags 3 /Fields [4 0 R] >>", &format!("<< /FT /Sig /T (S) /Type /Annot /Subtype /Widget /Rect [0 0 0 0] {} >>", sig_value("")))
}

fn place1(eng: &Engine, id: crate::engine::DocId, s: &Stamp, png: &[u8], at: [f32; 2]) -> Result<i32> {
    place_one(eng, id, 0, s, png, at, "Alex", user()).map(|r| r.1)
}

#[test]
fn a_placed_stamp_keeps_its_picture_through_move_resize_save_reopen_and_undo() {
    let d = dir("stamps", "place");
    let (_lib, s, png) = one_stamp(&d);
    let (eng, id, path) = open_bytes(&d, "a.pdf", &plain());
    let base = eng.edit_state(id).unwrap();
    let before = eng.history(id).unwrap().len();
    let x = place1(&eng, id, &s, &png, [300.0, 400.0]).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), before + 1, "one undo step");

    let l = list_all(&eng, id).unwrap();
    assert_eq!(l.len(), 1);
    let a = &l[0];
    assert_eq!((a.xref, a.kind, a.icon.as_deref(), a.picture, a.author.as_str()), (x, Kind::Stamp, Some("Approved by me"), true, "Alex"));
    // 150 pt wide at the picture's 2:1, centred on the click.
    let want = [225.0, 362.5, 375.0, 437.5];
    assert!(a.rect.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01), "{:?}", a.rect);
    // It draws: the left half red, the right half (transparent in the picture) is the white page.
    let p = pixels(&eng, id);
    assert_eq!(at(&p, 250, 400), [255, 0, 0]);
    assert_eq!(at(&p, 350, 400), [255, 255, 255]);

    // Move (same size): the box is exactly what was asked for.
    edit_one(&eng, id, 0, x, &Patch { rect: Some([100.0, 100.0, 250.0, 175.0]), ..Default::default() }, "Alex", user()).unwrap();
    let p = pixels(&eng, id);
    assert_eq!(at(&p, 120, 140), [255, 0, 0]);
    assert_eq!(at(&p, 250, 400), [255, 255, 255], "gone from the old place");

    // Resize with a box of another shape: the picture keeps its 2:1, anchored where the sides did not move.
    edit_one(&eng, id, 0, x, &Patch { rect: Some([100.0, 100.0, 400.0, 160.0]), ..Default::default() }, "Alex", user()).unwrap();
    let a = list_all(&eng, id).unwrap().remove(0);
    let want = [100.0, 100.0, 220.0, 160.0];
    assert!(a.rect.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01), "{:?}", a.rect);
    assert!(a.picture);
    let p = pixels(&eng, id);
    assert_eq!(at(&p, 120, 130), [255, 0, 0]);
    assert_eq!(at(&p, 200, 130), [255, 255, 255]);

    // Save and reopen: the appearance is a real form XObject drawing one image with its soft mask.
    eng.save(id, None).unwrap();
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    let an = page.annotations().next().expect("the stamp is in the file");
    let n = an.object().get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
    assert!(n.is_stream().unwrap());
    assert_eq!(n.get_dict("Subtype").unwrap().unwrap().as_name().unwrap(), b"Form");
    let xo = n.get_dict("Resources").unwrap().unwrap().get_dict("XObject").unwrap().unwrap();
    assert_eq!(xo.dict_len().unwrap(), 1);
    let img = xo.get_dict("I").unwrap().unwrap();
    assert_eq!(img.get_dict("Subtype").unwrap().unwrap().as_name().unwrap(), b"Image");
    assert!(img.get_dict("SMask").unwrap().is_some(), "the picture's alpha is a soft mask");
    assert_eq!(picture_size(&an.object()), Some((200, 100)));
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let back = list_all(&eng2, id2).unwrap();
    assert_eq!((back.len(), back[0].picture, back[0].icon.as_deref()), (1, true, Some("Approved by me")));

    // Delete it; undo restores, undo again takes it away again, back to the opened state.
    delete_many(&eng, id, &[(0, x)], user()).unwrap();
    assert!(list_all(&eng, id).unwrap().is_empty());
    eng.undo(id).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().len(), 1);
    for _ in 0..3 {
        eng.undo(id).unwrap();
    }
    assert!(list_all(&eng, id).unwrap().is_empty());
    let s0 = eng.edit_state(id).unwrap();
    assert_eq!((s0.undo, s0.rev > base.rev), (None, true));
    eng.redo(id).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().len(), 1);
}

#[test]
fn a_stamp_is_clamped_to_the_page_and_a_small_page() {
    let page = Rect { x0: 0.0, y0: 0.0, x1: 612.0, y1: 792.0 };
    assert_eq!(place_rect([306.0, 396.0], 2.0, page), [231.0, 358.5, 381.0, 433.5]);
    // At a corner: moved inside the page, same size.
    assert_eq!(place_rect([0.0, 0.0], 2.0, page), [0.0, 0.0, 150.0, 75.0]);
    assert_eq!(place_rect([612.0, 792.0], 2.0, page), [462.0, 717.0, 612.0, 792.0]);
    // A page narrower than 150 pt, and one too short for the stamp's height.
    let narrow = Rect { x0: 0.0, y0: 0.0, x1: 100.0, y1: 500.0 };
    assert_eq!(place_rect([50.0, 250.0], 1.0, narrow), [0.0, 200.0, 100.0, 300.0]);
    let short = Rect { x0: 0.0, y0: 0.0, x1: 400.0, y1: 30.0 };
    assert_eq!(place_rect([200.0, 15.0], 0.5, short), [192.5, 0.0, 207.5, 30.0]);
    // A page under 1 pt (a small /UserUnit) puts the stamp at its corner and does not panic.
    let tiny = Rect { x0: 10.0, y0: 20.0, x1: 10.5, y1: 20.25 };
    let r = place_rect([10.2, 20.1], 2.0, tiny);
    assert!(r.iter().all(|v| v.is_finite()) && r[0] == 10.0 && r[1] == 20.0, "{r:?}");
    // A tall picture's default width still gives a tall stamp.
    let r = place_rect([300.0, 400.0], 0.25, page);
    assert_eq!((r[2] - r[0], r[3] - r[1]), (150.0, 600.0));
}

/// A `w x h` PNG whose top left quarter is red and top right quarter blue; the bottom half is transparent.
/// Two colours side by side tell a turn from a mirror, and the transparent half a turn from a transpose.
fn red_blue(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| match (x < w / 2, y < h / 2) {
        (true, true) => image::Rgba([255, 0, 0, 255]),
        (false, true) => image::Rgba([0, 0, 255, 255]),
        _ => image::Rgba([0, 0, 0, 0]),
    });
    encode_png(&img).unwrap()
}

/// The stamp as the page shows it: red top left, blue top right, page white below, inside `rect` (page space).
fn upright(p: &(Vec<u8>, usize, usize), rect: [f32; 4], what: &str) {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let px = |fx: f32, fy: f32| at(p, (rect[0] + w * fx) as usize, (rect[1] + h * fy) as usize);
    assert_eq!(px(0.25, 0.25), [255, 0, 0], "{what}: top left");
    assert_eq!(px(0.75, 0.25), [0, 0, 255], "{what}: top right");
    assert_eq!(px(0.25, 0.75), [255, 255, 255], "{what}: bottom left");
    assert_eq!(px(0.75, 0.75), [255, 255, 255], "{what}: bottom right");
}

#[test]
fn a_stamp_on_a_rotated_page_is_upright_and_stays_so_through_move_and_resize() {
    let d = dir("stamps", "rotated");
    let lib = d.join("lib");
    let src = write(&d, "rb.png", &red_blue(200, 100));
    let s = create(&lib, &Source { path: &src, page: None }, "Two colours", "").unwrap();
    let (_, png) = picture(&lib, &s.id).unwrap();
    // 100 and -90 are snapped by MuPDF to 90 and 270.
    for turn in [0, 90, 180, 270, 100, -90] {
        let pdf = raw_pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Rotate {turn} >>"),
        ]);
        let (eng, id, _) = open_bytes(&d, &format!("r{turn}.pdf"), &pdf);
        // At 90 and 270 the page shows 792 x 612; (500, 300) is inside it either way.
        let x = place1(&eng, id, &s, &png, [500.0, 300.0]).unwrap();
        let a = list_all(&eng, id).unwrap().remove(0);
        let r = a.rect;
        assert!(((r[2] - r[0]) - 150.0).abs() < 0.01 && ((r[3] - r[1]) - 75.0).abs() < 0.01, "{turn}: {r:?}");
        upright(&pixels(&eng, id), r, &format!("/Rotate {turn} placed"));

        // Move (same size).
        edit_one(&eng, id, 0, x, &Patch { rect: Some([100.0, 100.0, 250.0, 175.0]), ..Default::default() }, "Alex", user()).unwrap();
        let r = list_all(&eng, id).unwrap().remove(0).rect;
        assert!(r.iter().zip([100.0, 100.0, 250.0, 175.0]).all(|(g, w)| (g - w).abs() < 0.01), "{turn}: {r:?}");
        upright(&pixels(&eng, id), r, &format!("/Rotate {turn} moved"));

        // Resize into a box of another shape: the displayed 2:1 holds, the top left stays.
        edit_one(&eng, id, 0, x, &Patch { rect: Some([100.0, 100.0, 400.0, 160.0]), ..Default::default() }, "Alex", user()).unwrap();
        let r = list_all(&eng, id).unwrap().remove(0).rect;
        assert!(r.iter().zip([100.0, 100.0, 220.0, 160.0]).all(|(g, w)| (g - w).abs() < 0.01), "{turn}: {r:?}");
        upright(&pixels(&eng, id), r, &format!("/Rotate {turn} resized"));

        // Delete and undo it again: still the same picture.
        delete_many(&eng, id, &[(0, x)], user()).unwrap();
        eng.undo(id).unwrap();
        upright(&pixels(&eng, id), r, &format!("/Rotate {turn} after undo"));
    }
}

#[test]
fn fit_aspect_follows_the_handle_that_moved() {
    let r = |x0, y0, x1, y1| Rect { x0, y0, x1, y1 };
    let from = r(100.0, 100.0, 300.0, 200.0);
    let same = |a: Rect, b: Rect| (a.x0 - b.x0).abs() < 1e-3 && (a.y0 - b.y0).abs() < 1e-3 && (a.x1 - b.x1).abs() < 1e-3 && (a.y1 - b.y1).abs() < 1e-3;
    // South-east handle dragged wide: the top left stays.
    assert!(same(fit_aspect(from, r(100.0, 100.0, 500.0, 220.0), 2.0), r(100.0, 100.0, 340.0, 220.0)));
    // North-west handle: the bottom right stays.
    assert!(same(fit_aspect(from, r(0.0, 150.0, 300.0, 200.0), 2.0), r(200.0, 150.0, 300.0, 200.0)));
    // A move or a resize that is already in proportion is left as asked.
    assert!(same(fit_aspect(from, r(50.0, 60.0, 250.0, 160.0), 2.0), r(50.0, 60.0, 250.0, 160.0)));
    // Nothing to fit to: as asked.
    assert!(same(fit_aspect(from, r(0.0, 0.0, 10.0, 10.0), 0.0), r(0.0, 0.0, 10.0, 10.0)));
}

#[test]
fn certified_refuses_without_a_step_unless_it_allows_annotations_and_signed_stays_incremental() {
    let d = dir("stamps", "protection");
    let (_lib, s, png) = one_stamp(&d);
    for p in [1, 2] {
        let (eng, id, _) = open_bytes(&d, &format!("p{p}.pdf"), &certified(p));
        let before = eng.edit_state(id).unwrap();
        assert!(err(place1(&eng, id, &s, &png, [100.0, 100.0])).contains("certified"));
        let after = eng.edit_state(id).unwrap();
        assert_eq!((after.rev, after.undo, after.dirty), (before.rev, before.undo, before.dirty), "/P {p} left a step");
        let g = gate(&eng, id).unwrap();
        assert!(g.reason.as_deref().is_some_and(|m| m.contains("certified")), "{g:?}");
    }
    let (eng, id, _) = open_bytes(&d, "p3.pdf", &certified(3));
    assert!(gate(&eng, id).unwrap().reason.is_none());
    place1(&eng, id, &s, &png, [100.0, 100.0]).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().len(), 1);

    let bytes = signed();
    let (eng, id, path) = open_bytes(&d, "signed.pdf", &bytes);
    let g = gate(&eng, id).unwrap();
    assert!(g.reason.is_none() && g.signed);
    let x = place1(&eng, id, &s, &png, [100.0, 100.0]).unwrap();
    // Moving and then removing it still saves as an update after the signed bytes.
    edit_one(&eng, id, 0, x, &Patch { rect: Some([200.0, 200.0, 350.0, 275.0]), ..Default::default() }, "Alex", user()).unwrap();
    eng.save(id, None).unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(&bytes), "the signed bytes are not the start of the saved file");
    delete_many(&eng, id, &[(0, x)], user()).unwrap();
    eng.save(id, None).unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(&bytes));
}

#[test]
fn a_stamp_that_is_not_a_picture_is_not_flagged_and_is_left_to_mupdf() {
    let d = dir("stamps", "standard");
    let (eng, id, _) = open_bytes(&d, "a.pdf", &plain());
    let x = add_one(&eng, id, 0, &crate::annot::NewAnnot::Stamp { name: "Approved".into(), rect: [100.0, 100.0, 220.0, 150.0] }, &Style::default(), "A", user()).unwrap().1;
    assert!(!list_all(&eng, id).unwrap()[0].picture);
    edit_one(&eng, id, 0, x, &Patch { rect: Some([100.0, 100.0, 400.0, 130.0]), ..Default::default() }, "A", user()).unwrap();
    let r = list_all(&eng, id).unwrap()[0].rect;
    // MuPDF re-fits a text stamp inside the box itself; we add no proportion rule of our own.
    assert!(r[0] >= 100.0 && r[2] <= 400.0 && (r[1] - 100.0).abs() < 0.01 && (r[3] - 130.0).abs() < 0.01, "{r:?}");
}

#[test]
fn a_placed_custom_stamp_exports_to_xfdf_by_name_without_its_picture_and_reimports() {
    use crate::annot::xfdf::{export, import_one, parse};
    use crate::exchange::Format;
    let d = dir("stamps", "xfdf");
    let (_lib, s, png) = one_stamp(&d);
    let (eng, id, _) = open_bytes(&d, "a.pdf", &plain());
    place1(&eng, id, &s, &png, [300.0, 400.0]).unwrap();
    let before = list_all(&eng, id).unwrap();
    assert!(before[0].picture);

    for fmt in [Format::Xfdf, Format::Fdf] {
        let (bytes, done) = export(&eng, id, fmt, "a.pdf").unwrap();
        assert_eq!((done.count, done.left_out), (1, 0));
        let text = String::from_utf8_lossy(&bytes);
        // (An FDF name object writes the spaces as #20.)
        assert!(text.contains("Approved by me") || text.contains("Approved#20by#20me"), "the stamp is listed by its name: {text}");
        // The picture is not in the file: a few hundred bytes of text, no image data.
        assert!(bytes.len() < 4000 && !text.contains("PNG") && !text.contains("FlateDecode"), "{} bytes", bytes.len());

        // Back into the same document: the stamp is matched by name and updated, still the same picture.
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.items.len(), 1);
        let got = import_one(&eng, id, &parsed, user()).unwrap();
        assert!(got.summary.skipped.is_empty(), "{:?}", got.summary);
        let after = list_all(&eng, id).unwrap();
        assert_eq!(after.len(), 1, "no second stamp");
        assert!(after[0].picture, "the picture survives the update");
        assert_eq!(after[0].icon.as_deref(), Some("Approved by me"));

        // Into a document that has no such stamp: a stamp with that name is made (as MuPDF's own, not a picture).
        let (eng2, id2, _) = open_bytes(&dir("stamps", "xfdf-b"), "b.pdf", &plain());
        let got = import_one(&eng2, id2, &parsed, user()).unwrap();
        assert_eq!((got.summary.added, got.summary.updated), (1, 0), "{:?}", got.summary);
        let made = list_all(&eng2, id2).unwrap();
        assert_eq!((made.len(), made[0].kind, made[0].icon.as_deref(), made[0].picture), (1, Kind::Stamp, Some("Approved by me"), false));
    }
}
