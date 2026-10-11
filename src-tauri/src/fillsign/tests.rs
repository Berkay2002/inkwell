//! Fill & Sign items through the edit lane (one undo step each, kept by Save and reopen),
//! flattening, and the encrypted saved-signature store. Signatures here are drawn by the test.

use super::store::{self, Kind, Source, Store};
use super::{Mark, NewItem, Spec, add_one, flatten_one, list_all};
use crate::annot::{self, NewAnnot, Style};
use crate::engine::edit::EditSource;
use crate::engine::fixture::{self, Fixture};
use crate::engine::{DocId, Engine};
use std::path::PathBuf;
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("fillsign-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(test: &str) -> (Arc<Engine>, DocId, String) {
    let p = dir(test).join("a.pdf");
    std::fs::write(&p, fixture::build(&Fixture { pages: 2, lines: 4, ..Default::default() })).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

/// A dark diagonal stroke on transparency, as the webview hands over a drawn signature.
fn drawn_png() -> Vec<u8> {
    let mut img = image::RgbaImage::new(120, 40);
    for x in 0..120 {
        for t in 0..3 {
            img.put_pixel(x, (x / 3 + t).min(39), image::Rgba([31, 42, 68, 255]));
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

fn item(page: u32, spec: Spec) -> NewItem {
    NewItem { page, spec, color: None }
}

fn every_kind() -> Vec<NewItem> {
    let mark = |mark, x| item(0, Spec::Mark { mark, at: [x, 200.0], size: None });
    vec![
        item(0, Spec::Text { at: [72.0, 120.0], text: "Alex Johnson".into(), font_size: Some(14.0), width: None }),
        mark(Mark::Check, 100.0),
        mark(Mark::Cross, 130.0),
        mark(Mark::Dot, 160.0),
        mark(Mark::Circle, 190.0),
        mark(Mark::Line, 240.0),
        item(1, Spec::Image { kind: Kind::Signature, rect: [300.0, 600.0, 420.0, 640.0], saved_id: None, png_base64: None }),
        item(1, Spec::Image { kind: Kind::Initials, rect: [450.0, 600.0, 490.0, 630.0], saved_id: None, png_base64: None }),
    ]
}

#[test]
fn every_item_is_one_undo_step_and_survives_save_and_reopen() {
    let (eng, id, path) = open("items");
    let png = drawn_png();
    let base = eng.history(id).unwrap().len();
    for (n, it) in every_kind().iter().enumerate() {
        let img = matches!(it.spec, Spec::Image { .. }).then_some(&png[..]);
        add_one(&eng, id, it, img, EditSource::User).unwrap();
        assert_eq!(eng.history(id).unwrap().len(), base + n + 1);
    }
    // A comment is not a Fill & Sign item.
    annot::add_one(&eng, id, 0, &NewAnnot::Note { at: [400.0, 400.0], text: "note".into() }, &Style::default(), "T", EditSource::User).unwrap();
    let kinds = |eng: &Engine, id| list_all(eng, id).unwrap().into_iter().map(|i| i.item).collect::<Vec<_>>();
    let want = ["text", "check", "cross", "dot", "circle", "line", "signature", "initials"];
    assert_eq!(kinds(&eng, id), want);
    // ... and the comments list holds only the note.
    let comments = annot::list_all(&eng, id).unwrap();
    assert_eq!(comments.len(), 1, "{comments:?}");
    let items = list_all(&eng, id).unwrap();
    assert_eq!(items[0].annot.contents, "Alex Johnson");
    assert_eq!(items[6].annot.page, 1);
    assert!((items[6].annot.rect[0] - 300.0).abs() < 0.5 && (items[6].annot.rect[3] - 640.0).abs() < 0.5, "{:?}", items[6].annot.rect);

    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert_eq!(kinds(&eng2, id2), want);

    eng.undo(id).unwrap(); // the note
    eng.undo(id).unwrap(); // the initials
    assert_eq!(kinds(&eng, id).len(), 7);
}

#[test]
fn empty_text_and_a_missing_image_are_refused() {
    let (eng, id, _) = open("refused");
    let text = item(0, Spec::Text { at: [72.0, 120.0], text: "  ".into(), font_size: None, width: None });
    assert!(add_one(&eng, id, &text, None, EditSource::User).is_err());
    let sig = item(0, Spec::Image { kind: Kind::Signature, rect: [72.0, 72.0, 200.0, 120.0], saved_id: None, png_base64: None });
    assert!(add_one(&eng, id, &sig, None, EditSource::User).is_err());
    assert!(flatten_one(&eng, id, None, EditSource::User).is_err(), "nothing to flatten");
}

/// Dark pixels inside `r` (page points) on page `p` at 1:1.
fn ink(bytes: &[u8], p: i32, r: [usize; 4]) -> usize {
    let page = mupdf::Document::from_bytes(bytes, "application/pdf").unwrap().load_page(p).unwrap();
    let pix = page.to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_gray(), false, true).unwrap();
    let (w, s) = (pix.width() as usize, pix.samples());
    (r[1]..r[3]).flat_map(|y| (r[0]..r[2]).map(move |x| (x, y))).filter(|&(x, y)| s[y * w + x] < 128).count()
}

#[test]
fn flattening_bakes_the_items_and_keeps_comments() {
    let (eng, id, path) = open("flatten");
    let png = drawn_png();
    for it in every_kind() {
        let img = matches!(it.spec, Spec::Image { .. }).then_some(&png[..]);
        add_one(&eng, id, &it, img, EditSource::User).unwrap();
    }
    annot::add_one(&eng, id, 0, &NewAnnot::Note { at: [400.0, 400.0], text: "keep me".into() }, &Style::default(), "T", EditSource::User).unwrap();
    let before = eng.current_bytes(id).unwrap();

    // Page 2 only first: page 1's items stay items.
    flatten_one(&eng, id, Some(&[1]), EditSource::User).unwrap();
    assert!(list_all(&eng, id).unwrap().iter().all(|i| i.annot.page == 0));
    flatten_one(&eng, id, None, EditSource::User).unwrap();
    assert!(list_all(&eng, id).unwrap().is_empty());
    let notes = annot::list_all(&eng, id).unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].contents, "keep me");

    eng.save(id, None).unwrap();
    let after = std::fs::read(&path).unwrap();
    // Still drawn, now as page content: the signature box and the check have ink.
    let sig = [300, 600, 420, 640];
    assert!(ink(&before, 1, sig) > 40);
    assert!(ink(&after, 1, sig) > 40, "the signature was baked");
    assert!(ink(&after, 0, [92, 192, 108, 208]) > 3, "the check was baked");

    // One undo restores the page-1 items.
    eng.undo(id).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().len(), 6);
}

#[test]
fn saved_signatures_are_encrypted_bound_and_capped() {
    let d = dir("store");
    let st = Store::new(d.clone());
    assert!(st.list().unwrap().is_empty());
    let png = store::normalize(&drawn_png(), true, false).unwrap();
    let a = st.add(Kind::Signature, Source::Drawn, None, png.clone()).unwrap();
    let b = st.add(Kind::Initials, Source::Typed, Some("Caveat".into()), png.clone()).unwrap();
    let all = Store::new(d.clone()).list().unwrap();
    assert_eq!(all.len(), 2);
    let got = st.get(&a.id).unwrap();
    assert_eq!((got.kind, got.source, got.png == png), (Kind::Signature, Source::Drawn, true));
    assert_eq!(st.get(&b.id).unwrap().font.as_deref(), Some("Caveat"));

    // At rest: no PNG signature, no JSON, and the key is DPAPI-wrapped, not raw.
    let raw = std::fs::read(d.join(format!("{}.rec", a.id))).unwrap();
    assert!(!raw.windows(4).any(|w| w == b"\x89PNG") && !raw.windows(6).any(|w| w == b"source"));
    assert!(std::fs::read(d.join("key.dpapi")).unwrap().len() > 32);

    // A record renamed or relabelled fails its check and is left out of the list.
    let mut relabelled = raw.clone();
    relabelled[0] = std::fs::read(d.join(format!("{}.rec", b.id))).unwrap()[0];
    std::fs::write(d.join(format!("{}.rec", a.id)), &relabelled).unwrap();
    assert!(st.get(&a.id).is_err());
    std::fs::write(d.join(format!("{}.rec", a.id)), &raw).unwrap();
    std::fs::copy(d.join(format!("{}.rec", a.id)), d.join(format!("{}.rec", "0".repeat(32)))).unwrap();
    assert!(st.get(&"0".repeat(32)).is_err());
    assert_eq!(st.list().unwrap().len(), 2);

    // Ids are file names: only 32 hex characters get near the disk.
    assert!(st.get("../key").is_err() && st.remove("..\\key.dpapi").is_err());
    st.remove(&a.id).unwrap();
    assert!(st.get(&a.id).is_err());

    for _ in 0..store::MAX_PER_KIND {
        st.add(Kind::Initials, Source::Drawn, None, png.clone()).unwrap_or_else(|_| b.clone());
    }
    assert!(st.add(Kind::Initials, Source::Drawn, None, png.clone()).is_err(), "capped");
    st.add(Kind::Signature, Source::Drawn, None, png).unwrap();
}

#[test]
fn pictures_are_normalized() {
    // JPEG only where allowed; a photo of paper has its white made transparent.
    let mut img = image::RgbImage::from_pixel(40, 20, image::Rgb([250, 250, 248]));
    for x in 5..35 {
        img.put_pixel(x, 10, image::Rgb([20, 20, 30]));
    }
    let mut jpg = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut jpg), image::ImageFormat::Jpeg).unwrap();
    assert!(store::normalize(&jpg, true, false).is_err());
    let png = store::normalize(&jpg, false, true).unwrap();
    let out = image::load_from_memory(&png).unwrap().into_rgba8();
    assert!(out.get_pixel(1, 1).0[3] < 20, "paper is transparent");
    assert!(out.get_pixel(20, 10).0[3] > 200, "ink stays");
    assert_eq!(store::png_size(&png), [40, 20]);
    assert!(store::normalize(b"not an image", false, false).is_err());
    assert!(store::decode_png("data:image/png;base64,!!!").is_err());
}
