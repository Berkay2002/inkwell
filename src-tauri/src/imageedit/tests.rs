//! Image editing through the edit lane: every operation, save and reopen, undo, no orphans.

use super::*;
use crate::engine::Engine;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join("imageedit").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 4 × 2 RGB pixels, each a different colour: row 0 is red, green, blue, white; row 1 black, grey, yellow, cyan.
const PIXELS: [u8; 24] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 128, 128, 128, 255, 255, 0, 0, 255, 255];

/// A PDF of `pages` pages. Page 1 draws image `Im1` at (100,500)-(300,600) in user space, an
/// inline 2 × 2 grey image at (400,600)-(450,650), and a line of text; other pages draw `Im1` too
/// when `shared`.
fn pdf(pages: usize, shared: bool) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = Vec::new();
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", 5 + i * 2)).collect();
    objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objs.push(format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>").into_bytes());
    let mut img = format!(
        "<< /Type /XObject /Subtype /Image /Width 4 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n",
        PIXELS.len()
    )
    .into_bytes();
    img.extend_from_slice(&PIXELS);
    img.extend_from_slice(b"\nendstream");
    objs.push(img);
    objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    for i in 0..pages {
        objs.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im1 3 0 R >> /Font << /F1 4 0 R >> >> /Contents {} 0 R >>",
                6 + i * 2
            )
            .into_bytes(),
        );
        let mut c: Vec<u8> = if i == 0 || shared { b"q 200 0 0 100 100 500 cm /Im1 Do Q\n".to_vec() } else { Vec::new() };
        if i == 0 {
            c.extend_from_slice(b"q 50 0 0 50 400 600 cm BI /W 2 /H 2 /CS /G /BPC 8 ID \x10\x60\xa0\xf0 EI Q\n");
        }
        c.extend_from_slice(format!("BT /F1 12 Tf 72 72 Td (Page {}) Tj ET\n", i + 1).as_bytes());
        let mut s = format!("<< /Length {} >>\nstream\n", c.len()).into_bytes();
        s.extend_from_slice(&c);
        s.extend_from_slice(b"\nendstream");
        objs.push(s);
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn open(d: &Path, bytes: &[u8]) -> (Arc<Engine>, DocId, String) {
    let p = d.join("a.pdf");
    std::fs::write(&p, bytes).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

fn list(eng: &Engine, id: DocId) -> Vec<ImageBlock> {
    blocks(eng, id, 0).unwrap().images
}

fn change(eng: &Engine, id: DocId, index: Option<u32>, c: ImageChange) -> ImageEdited {
    let rev = eng.edit_state(id).unwrap().rev;
    edit(eng, id, rev, 0, index, &c, EditSource::User).unwrap()
}

fn near(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05)
}

/// Where unit-square point (u, v) of image `b` lands in page space (page height 792, no rotation).
fn at(b: &ImageBlock, u: f32, v: f32) -> (f32, f32) {
    let m = b.matrix;
    (m[0] * u + m[2] * v + m[4], 792.0 - (m[1] * u + m[3] * v + m[5]))
}

/// The image XObjects a saved file holds: (object number, width, height, filter, has SMask).
fn image_objects(path: &str) -> Vec<(i32, i32, i32, Vec<String>, bool)> {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    let n = doc.count_objects().unwrap() as i32;
    let mut out = Vec::new();
    let mut smasks = HashSet::new();
    for i in 1..n {
        let Ok(o) = doc.new_indirect(i, 0) else { continue };
        let Ok(Some(r)) = o.resolve() else { continue };
        if !r.is_dict().unwrap_or(false) || get(&r, "Subtype").and_then(|s| s.as_name().ok()).as_deref() != Some(b"Image") {
            continue;
        }
        if let Some(s) = get(&r, "SMask") {
            smasks.insert(s.as_indirect().unwrap_or(0));
        }
        let int = |k: &str| get(&r, k).and_then(|v| v.as_int().ok()).unwrap_or(0);
        out.push((i, int("Width"), int("Height"), pdf_names(get(&r, "Filter")), get(&r, "SMask").is_some()));
    }
    // Soft masks are images too, but not ones a page draws.
    out.retain(|o| !smasks.contains(&o.0));
    out
}

fn jpeg(d: &Path) -> (String, Vec<u8>) {
    let img = image::RgbImage::from_fn(16, 8, |x, y| image::Rgb([(x * 16) as u8, (y * 32) as u8, 128]));
    let p = d.join("photo.jpg");
    img.save(&p).unwrap();
    (p.to_string_lossy().into_owned(), std::fs::read(&p).unwrap())
}

fn png_alpha(d: &Path) -> String {
    let img = image::RgbaImage::from_fn(8, 8, |x, _| image::Rgba([0, 0, 255, (x * 32) as u8]));
    let p = d.join("logo.png");
    img.save(&p).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn lists_xobject_and_inline_images() {
    let d = dir("list");
    let (eng, id, _) = open(&d, &pdf(1, false));
    let l = list(&eng, id);
    assert_eq!(l.len(), 2);
    let (a, b) = (&l[0], &l[1]);
    assert_eq!((a.kind, a.width, a.height, a.color_space.as_str(), a.bits_per_component), (ImageKind::XObject, 4, 2, "DeviceRGB", 8));
    assert_eq!((a.name.as_deref(), a.xref, a.editable), (Some("Im1"), Some(3), true));
    assert!(near(a.rect, [100.0, 192.0, 300.0, 292.0]), "{:?}", a.rect);
    assert_eq!(a.matrix, [200.0, 0.0, 0.0, 100.0, 100.0, 500.0]);
    assert_eq!((b.kind, b.width, b.height, b.color_space.as_str()), (ImageKind::Inline, 2, 2, "DeviceGray"));
    assert!(near(b.rect, [400.0, 142.0, 450.0, 192.0]), "{:?}", b.rect);
}

#[test]
fn move_round_trips_through_save_and_undo() {
    let d = dir("move");
    let (eng, id, path) = open(&d, &pdf(1, false));
    let r = change(&eng, id, Some(0), ImageChange::Move { dx: 10.0, dy: 20.0 });
    assert_eq!(r.state.undo.as_deref(), Some("Move image on p. 1"));
    assert!(near(list(&eng, id)[0].rect, [110.0, 212.0, 310.0, 312.0]));
    // Inline images move the same way.
    change(&eng, id, Some(1), ImageChange::Move { dx: -100.0, dy: 0.0 });
    assert!(near(list(&eng, id)[1].rect, [300.0, 142.0, 350.0, 192.0]));
    eng.undo(id).unwrap();
    assert!(near(list(&eng, id)[1].rect, [400.0, 142.0, 450.0, 192.0]));
    eng.save(id, None).unwrap();
    eng.close(id);
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert!(near(list(&eng2, id2)[0].rect, [110.0, 212.0, 310.0, 312.0]));
    eng2.undo(id2).unwrap();
    assert_eq!(image_objects(&path).len(), 1, "the file holds one image");
}

#[test]
fn resize_with_and_without_aspect() {
    let d = dir("resize");
    let (eng, id, _) = open(&d, &pdf(1, false));
    change(&eng, id, Some(0), ImageChange::Resize { rect: [50.0, 50.0, 150.0, 250.0], keep_aspect: false });
    assert!(near(list(&eng, id)[0].rect, [50.0, 50.0, 150.0, 250.0]));
    // Kept proportions (1:2 now): as large as fits from the top left.
    change(&eng, id, Some(0), ImageChange::Resize { rect: [0.0, 0.0, 300.0, 300.0], keep_aspect: true });
    assert!(near(list(&eng, id)[0].rect, [0.0, 0.0, 150.0, 300.0]), "{:?}", list(&eng, id)[0].rect);
}

#[test]
fn rotate_and_flip_about_the_centre() {
    let d = dir("rotate");
    let (eng, id, _) = open(&d, &pdf(1, false));
    change(&eng, id, Some(0), ImageChange::Rotate { quarter_turns: 1 });
    let b = list(&eng, id)[0].clone();
    // 200 × 100 turns into 100 × 200 about the centre (200, 242).
    assert!(near(b.rect, [150.0, 142.0, 250.0, 342.0]), "{:?}", b.rect);
    // A quarter turn clockwise: the image's top left corner is now at the top right.
    let (x, y) = at(&b, 0.0, 1.0);
    assert!((x - 250.0).abs() < 0.05 && (y - 142.0).abs() < 0.05, "({x}, {y})");
    change(&eng, id, Some(0), ImageChange::Rotate { quarter_turns: -1 });
    let b = list(&eng, id)[0].clone();
    assert!(near(b.rect, [100.0, 192.0, 300.0, 292.0]));
    assert!(b.matrix.iter().zip([200.0, 0.0, 0.0, 100.0, 100.0, 500.0]).all(|(a, e)| (a - e).abs() < 0.01), "{:?}", b.matrix);
    change(&eng, id, Some(0), ImageChange::Rotate { quarter_turns: 2 });
    let (x, y) = at(&list(&eng, id)[0], 0.0, 1.0);
    assert!((x - 300.0).abs() < 0.05 && (y - 292.0).abs() < 0.05, "upside down: ({x}, {y})");
    change(&eng, id, Some(0), ImageChange::Rotate { quarter_turns: 2 });
    change(&eng, id, Some(0), ImageChange::Flip { horizontal: true });
    let b = list(&eng, id)[0].clone();
    assert!(near(b.rect, [100.0, 192.0, 300.0, 292.0]));
    let (x, y) = at(&b, 0.0, 1.0);
    assert!((x - 300.0).abs() < 0.05 && (y - 192.0).abs() < 0.05, "mirrored: ({x}, {y})");
    change(&eng, id, Some(0), ImageChange::Flip { horizontal: false });
    let (x, y) = at(&list(&eng, id)[0], 0.0, 1.0);
    assert!((x - 300.0).abs() < 0.05 && (y - 292.0).abs() < 0.05, "and flipped: ({x}, {y})");
}

#[test]
fn delete_leaves_no_orphan() {
    let d = dir("delete");
    let (eng, id, path) = open(&d, &pdf(1, false));
    change(&eng, id, Some(0), ImageChange::Delete);
    let l = list(&eng, id);
    assert_eq!((l.len(), l[0].kind), (1, ImageKind::Inline));
    change(&eng, id, Some(0), ImageChange::Delete);
    assert!(list(&eng, id).is_empty());
    eng.save(id, None).unwrap();
    assert!(image_objects(&path).is_empty(), "{:?}", image_objects(&path));
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.windows(3).any(|w| w == b"Im1"), "the page no longer names the image");
    assert_eq!(bytes.windows(9).filter(|w| *w == b"startxref").count(), 1, "a full rewrite");
    // Undo brings it back.
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    assert_eq!(list(&eng, id).len(), 2);
}

#[test]
fn an_image_another_page_draws_stays() {
    let d = dir("shared");
    let (eng, id, path) = open(&d, &pdf(2, true));
    change(&eng, id, Some(0), ImageChange::Delete);
    eng.save(id, None).unwrap();
    assert_eq!(image_objects(&path).len(), 1);
    assert_eq!(blocks(&eng, id, 1).unwrap().images.len(), 1, "page 2 still draws it");
}

#[test]
fn replace_with_jpeg_passes_it_through() {
    let d = dir("jpeg");
    let (file, data) = jpeg(&d);
    let (eng, id, path) = open(&d, &pdf(1, false));
    change(&eng, id, Some(0), ImageChange::Replace { file });
    let b = list(&eng, id)[0].clone();
    assert_eq!((b.width, b.height, b.filter.as_slice()), (16, 8, &["DCTDecode".to_string()][..]));
    // 16:8 = the old box's 2:1: it fills it.
    assert!(near(b.rect, [100.0, 192.0, 300.0, 292.0]), "{:?}", b.rect);
    eng.save(id, None).unwrap();
    let imgs = image_objects(&path);
    assert_eq!(imgs.len(), 1, "the old image is gone: {imgs:?}");
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let raw = doc.new_indirect(imgs[0].0, 0).unwrap().read_raw_stream().unwrap();
    assert_eq!(raw, data, "the JPEG bytes are untouched");
    // A JPEG is decoded for a pixel crop (and stored losslessly).
    change(&eng, id, Some(0), ImageChange::Crop { rect: [100.0, 192.0, 200.0, 292.0], pixels: true });
    let b = list(&eng, id)[0].clone();
    assert_eq!((b.width, b.height, b.filter.as_slice()), (8, 8, &["FlateDecode".to_string()][..]));
    eng.save(id, None).unwrap();
    assert_eq!(image_objects(&path).len(), 1);
}

#[test]
fn replace_with_png_alpha_makes_an_smask() {
    let d = dir("png");
    let file = png_alpha(&d);
    let (eng, id, path) = open(&d, &pdf(1, false));
    change(&eng, id, Some(0), ImageChange::Replace { file });
    let b = list(&eng, id)[0].clone();
    assert!(b.masked);
    // Square into a 2:1 box: centred, 100 × 100.
    assert!(near(b.rect, [150.0, 192.0, 250.0, 292.0]), "{:?}", b.rect);
    eng.save(id, None).unwrap();
    let imgs = image_objects(&path);
    assert_eq!(imgs.len(), 1);
    assert!(imgs[0].4, "has an /SMask");
    // Deleted: the image and its mask go.
    change(&eng, id, Some(0), ImageChange::Delete);
    eng.save(id, None).unwrap();
    assert_eq!(all_images(&path), 0);
}

/// Every image object in a saved file, soft masks included.
fn all_images(path: &str) -> usize {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    (1..doc.count_objects().unwrap() as i32)
        .filter_map(|i| doc.new_indirect(i, 0).ok()?.resolve().ok()?)
        .filter(|r| r.is_dict().unwrap_or(false) && get(r, "Subtype").and_then(|s| s.as_name().ok()).as_deref() == Some(b"Image"))
        .count()
}

#[test]
fn crop_with_a_clip_and_with_pixels() {
    let d = dir("crop");
    let (eng, id, path) = open(&d, &pdf(1, false));
    // The right half.
    change(&eng, id, Some(0), ImageChange::Crop { rect: [200.0, 192.0, 300.0, 292.0], pixels: false });
    let b = list(&eng, id)[0].clone();
    assert!(near(b.rect, [200.0, 192.0, 300.0, 292.0]) && near(b.full_rect, [100.0, 192.0, 300.0, 292.0]), "{b:?}");
    eng.undo(id).unwrap();
    // The top right quarter as pixels: columns 2-3 of row 0 (blue, white).
    change(&eng, id, Some(0), ImageChange::Crop { rect: [200.0, 192.0, 300.0, 242.0], pixels: true });
    let b = list(&eng, id)[0].clone();
    assert_eq!((b.width, b.height), (2, 1));
    assert!(near(b.rect, [200.0, 192.0, 300.0, 242.0]) && near(b.full_rect, b.rect), "{b:?}");
    eng.save(id, None).unwrap();
    let imgs = image_objects(&path);
    assert_eq!(imgs.len(), 1, "the uncropped image is gone: {imgs:?}");
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let samples = doc.new_indirect(imgs[0].0, 0).unwrap().read_stream().unwrap();
    assert_eq!(samples, vec![0, 0, 255, 255, 255, 255]);
    // Inline images take a clip only.
    let rev = eng.edit_state(id).unwrap().rev;
    let e = edit(&eng, id, rev, 0, Some(1), &ImageChange::Crop { rect: [400.0, 142.0, 425.0, 192.0], pixels: true }, EditSource::User);
    assert!(matches!(e, Err(EngineError::Invalid(_))));
    change(&eng, id, Some(1), ImageChange::Crop { rect: [400.0, 142.0, 425.0, 192.0], pixels: false });
    assert!(near(list(&eng, id)[1].rect, [400.0, 142.0, 425.0, 192.0]));
}

#[test]
fn add_a_new_image() {
    let d = dir("add");
    let file = png_alpha(&d);
    // Content that leaves its CTM scaled: the new image must not inherit it.
    let mut bytes = pdf(1, false);
    let doc = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let mut c = doc.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap();
    let mut data = c.read_stream().unwrap();
    data.extend_from_slice(b"q 2 0 0 2 0 0 cm\n");
    c.write_stream_buffer(&Buffer::from_bytes(&data).unwrap()).unwrap();
    bytes.clear();
    doc.write_to(&mut bytes).unwrap();
    let (eng, id, path) = open(&d, &bytes);
    let r = change(&eng, id, None, ImageChange::Add { file, rect: [300.0, 400.0, 500.0, 500.0], keep_aspect: true });
    assert_eq!(r.state.undo.as_deref(), Some("Add image on p. 1"));
    let l = list(&eng, id);
    assert_eq!(l.len(), 3);
    // An 8 × 8 image in 200 × 100: 100 × 100, centred.
    assert!(near(l[2].rect, [350.0, 400.0, 450.0, 500.0]), "{:?}", l[2].rect);
    // Upright: its top left pixel at the top left.
    let (x, y) = at(&l[2], 0.0, 1.0);
    assert!((x - 350.0).abs() < 0.05 && (y - 400.0).abs() < 0.05, "({x}, {y})");
    // The old images did not move.
    assert!(near(l[0].rect, [100.0, 192.0, 300.0, 292.0]));
    eng.save(id, None).unwrap();
    assert_eq!(image_objects(&path).len(), 2);
    eng.undo(id).unwrap();
    assert_eq!(list(&eng, id).len(), 2);
}

#[test]
fn stale_rev_and_bad_requests_are_refused() {
    let d = dir("stale");
    let (eng, id, _) = open(&d, &pdf(1, false));
    let r0 = eng.edit_state(id).unwrap().rev;
    change(&eng, id, Some(0), ImageChange::Move { dx: 1.0, dy: 1.0 });
    let e = edit(&eng, id, r0, 0, Some(0), &ImageChange::Delete, EditSource::User);
    assert!(matches!(e, Err(EngineError::Stale)));
    let rev = eng.edit_state(id).unwrap().rev;
    assert!(matches!(edit(&eng, id, rev, 0, Some(9), &ImageChange::Delete, EditSource::User), Err(EngineError::Invalid(_))));
    assert!(matches!(edit(&eng, id, rev, 0, None, &ImageChange::Delete, EditSource::User), Err(EngineError::Invalid(_))));
    // Nothing changed by the refusals.
    assert_eq!(eng.edit_state(id).unwrap().rev, rev);
}
