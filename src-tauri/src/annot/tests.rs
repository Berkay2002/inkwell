//! Every annotation type through the edit lane: one undo step each, saved, reopened and read back
//! with a fresh MuPDF, with appearance streams for other viewers.

use super::*;
use crate::engine::edit::EditOp;
use crate::engine::fixture::{self, Fixture};
use std::path::PathBuf;
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("annot-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(test: &str) -> (Arc<Engine>, DocId, String) {
    let d = dir(test);
    let p = d.join("a.pdf");
    std::fs::write(&p, fixture::build(&Fixture { pages: 2, lines: 4, ..Default::default() })).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

fn kinds(eng: &Engine, id: DocId) -> Vec<(Kind, u32)> {
    list_all(eng, id).unwrap().iter().map(|a| (a.kind, a.page)).collect()
}

fn add1(eng: &Engine, id: DocId, page: u32, what: NewAnnot) -> i32 {
    add_one(eng, id, page, &what, &Style::default(), "Alex Johnson", EditSource::User).unwrap().1
}

fn every_type() -> Vec<(Kind, NewAnnot)> {
    let q = |x: f32, y: f32| [x, y, x + 120.0, y, x, y + 14.0, x + 120.0, y + 14.0];
    vec![
        (Kind::Highlight, NewAnnot::Markup { markup: Kind::Highlight, quads: vec![q(72.0, 100.0)] }),
        (Kind::Underline, NewAnnot::Markup { markup: Kind::Underline, quads: vec![q(72.0, 130.0)] }),
        (Kind::StrikeOut, NewAnnot::Markup { markup: Kind::StrikeOut, quads: vec![q(72.0, 160.0)] }),
        (Kind::Squiggly, NewAnnot::Markup { markup: Kind::Squiggly, quads: vec![q(72.0, 190.0)] }),
        (Kind::Note, NewAnnot::Note { at: [400.0, 100.0], text: "Is this the final rent?".into() }),
        (Kind::FreeText, NewAnnot::FreeText { rect: [300.0, 300.0, 460.0, 340.0], text: "Check with the landlord".into(), font_size: Some(12.0) }),
        (Kind::Ink, NewAnnot::Ink { strokes: vec![vec![[100.0, 400.0], [120.0, 380.0], [150.0, 410.0]], vec![[160.0, 400.0], [170.0, 420.0]]] }),
        (Kind::Rectangle, NewAnnot::Shape { shape: Kind::Rectangle, points: vec![[72.0, 450.0], [200.0, 500.0]] }),
        (Kind::Ellipse, NewAnnot::Shape { shape: Kind::Ellipse, points: vec![[220.0, 450.0], [300.0, 500.0]] }),
        (Kind::Line, NewAnnot::Shape { shape: Kind::Line, points: vec![[72.0, 530.0], [200.0, 560.0]] }),
        (Kind::Arrow, NewAnnot::Shape { shape: Kind::Arrow, points: vec![[220.0, 530.0], [300.0, 560.0]] }),
        (Kind::Polygon, NewAnnot::Shape { shape: Kind::Polygon, points: vec![[320.0, 450.0], [400.0, 460.0], [380.0, 520.0]] }),
        (Kind::Stamp, NewAnnot::Stamp { name: "Approved".into(), rect: [400.0, 600.0, 520.0, 650.0] }),
    ]
}

#[test]
fn every_type_is_one_undo_step_and_survives_save_and_reopen() {
    let (eng, id, path) = open("types");
    let all = every_type();
    let base = eng.edit_state(id).unwrap().rev;
    for (i, (_, what)) in all.iter().enumerate() {
        let before = eng.history(id).unwrap().len();
        add1(&eng, id, 0, what.clone());
        assert_eq!(eng.history(id).unwrap().len(), before + 1, "one step for {what:?}");
        assert_eq!(kinds(&eng, id).len(), i + 1);
    }
    assert_eq!(eng.edit_state(id).unwrap().rev, base + all.len() as u32);
    let want: Vec<Kind> = all.iter().map(|a| a.0).collect();
    let got: Vec<Kind> = kinds(&eng, id).iter().map(|k| k.0).collect();
    assert_eq!(got, want);

    // Undo is one annotation at a time.
    eng.undo(id).unwrap();
    assert_eq!(kinds(&eng, id).len(), all.len() - 1);
    eng.redo(id).unwrap();
    assert_eq!(kinds(&eng, id).len(), all.len());

    eng.save(id, None).unwrap();
    let (eng2, id2) = {
        let e = Engine::new(2, 64 << 20);
        let i = e.open(&path).unwrap().doc;
        (e, i)
    };
    let back = list_all(&eng2, id2).unwrap();
    assert_eq!(back.iter().map(|a| a.kind).collect::<Vec<_>>(), want);
    assert!(back.iter().all(|a| a.author == "Alex Johnson" && a.created.starts_with("D:20") && a.modified.starts_with("D:20")));
    assert_eq!(back[4].contents, "Is this the final rent?");
    assert_eq!(back[5].contents, "Check with the landlord");
    assert_eq!(back[6].strokes.len(), 2);
    assert_eq!(back[11].points.len(), 3);

    // A fresh MuPDF sees the same subtypes, and every one carries an appearance stream.
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let page = doc.load_pdf_page(0).unwrap();
    let mut n = 0;
    for a in page.annotations() {
        n += 1;
        let o = a.object();
        assert!(o.get_dict("AP").unwrap().is_some(), "annotation {n} has no /AP");
    }
    assert_eq!(n, all.len());
}

#[test]
fn appearances_draw_on_the_page() {
    let (eng, id, _) = open("draw");
    add1(&eng, id, 0, NewAnnot::Markup { markup: Kind::Highlight, quads: vec![[72.0, 100.0, 272.0, 100.0, 72.0, 130.0, 272.0, 130.0]] });
    add1(&eng, id, 0, NewAnnot::Stamp { name: "Approved".into(), rect: [300.0, 500.0, 480.0, 560.0] });
    let bytes = eng.unsaved_bytes(id).unwrap().expect("an edit is pending");
    let doc = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let pix = doc
        .load_page(0)
        .unwrap()
        .to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_rgb(), false, true)
        .unwrap();
    let (w, n) = (pix.width() as usize, pix.n() as usize);
    let px = |x: usize, y: usize| {
        let i = (y * w + x) * n;
        pix.samples()[i..i + 3].to_vec()
    };
    let hi = px(150, 115);
    assert!(hi[2] < 200, "highlight area is not white: {hi:?}");
    let stamp = (500..560).flat_map(|y| (300..480).map(move |x| (x, y))).filter(|&(x, y)| px(x, y) != vec![255, 255, 255]).count();
    assert!(stamp > 100, "the stamp draws something ({stamp} px)");
}

#[test]
fn a_text_box_is_not_filled_with_its_text_colour() {
    let (eng, id, _) = open("freetext");
    add1(&eng, id, 0, NewAnnot::FreeText { rect: [300.0, 500.0, 480.0, 560.0], text: "Initial each page".into(), font_size: Some(12.0) });
    let a = list_all(&eng, id).unwrap().pop().unwrap();
    assert!(a.color.is_some_and(|c| c[0] < 0.2), "the dto carries the text colour: {:?}", a.color);
    let bytes = eng.unsaved_bytes(id).unwrap().expect("an edit is pending");
    let doc = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let pix = doc.load_page(0).unwrap().to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_rgb(), false, true).unwrap();
    let (w, n) = (pix.width() as usize, pix.n() as usize);
    let dark = (500..560)
        .flat_map(|y| (300..480).map(move |x| (x, y)))
        .filter(|&(x, y)| pix.samples()[(y * w + x) * n] < 100)
        .count();
    assert!(dark < 180 * 60 / 4, "the box is not a dark block ({dark} dark px)");
}

#[test]
fn named_and_date_stamps_draw() {
    for (i, name) in ["Draft", "Confidential", "11 Oct 2026"].iter().enumerate() {
        let (eng, id, _) = open(&format!("stamp{i}"));
        add1(&eng, id, 0, NewAnnot::Stamp { name: (*name).into(), rect: [300.0, 500.0, 480.0, 560.0] });
        let bytes = eng.unsaved_bytes(id).unwrap().unwrap();
        let doc = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
        let pix = doc.load_page(0).unwrap().to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_rgb(), false, true).unwrap();
        let (w, n) = (pix.width() as usize, pix.n() as usize);
        let drawn = (500..560)
            .flat_map(|y| (300..480).map(move |x| (x, y)))
            .filter(|&(x, y)| pix.samples()[(y * w + x) * n..][..3] != [255, 255, 255])
            .count();
        assert!(drawn > 100, "{name}: {drawn} px");
    }
}

#[test]
fn edit_move_resize_style_delete() {
    let (eng, id, _) = open("edit");
    let ink = add1(&eng, id, 0, NewAnnot::Ink { strokes: vec![vec![[100.0, 400.0], [200.0, 450.0]]] });
    let before = list_all(&eng, id).unwrap()[0].clone();
    let steps = eng.history(id).unwrap().len();
    // Move by (+50, +20): geometry follows, not just /Rect.
    let r = before.rect;
    let patch = Patch { rect: Some([r[0] + 50.0, r[1] + 20.0, r[2] + 50.0, r[3] + 20.0]), ..Default::default() };
    edit_one(&eng, id, 0, ink, &patch, "Alex Johnson", EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    let moved = list_all(&eng, id).unwrap()[0].clone();
    let (a, b) = (before.strokes[0][0], moved.strokes[0][0]);
    assert!((b[0] - a[0] - 50.0).abs() < 0.01 && (b[1] - a[1] - 20.0).abs() < 0.01, "{a:?} -> {b:?}");

    // Colour, opacity and thickness.
    let patch = Patch { style: Style { color: Some([1.0, 0.0, 0.0]), opacity: Some(0.5), width: Some(4.0), fill: None }, ..Default::default() };
    edit_one(&eng, id, 0, ink, &patch, "Alex Johnson", EditSource::User).unwrap();
    let s = list_all(&eng, id).unwrap()[0].clone();
    assert_eq!(s.color, Some([1.0, 0.0, 0.0]));
    assert!((s.opacity - 0.5).abs() < 0.01 && (s.width - 4.0).abs() < 0.01);

    // Resize the box: the stroke scales with it.
    let r = s.rect;
    let patch = Patch { rect: Some([r[0], r[1], r[0] + (r[2] - r[0]) * 2.0, r[1] + (r[3] - r[1]) * 2.0]), ..Default::default() };
    edit_one(&eng, id, 0, ink, &patch, "", EditSource::User).unwrap();
    let big = list_all(&eng, id).unwrap()[0].clone();
    assert!((big.strokes[0][1][0] - big.strokes[0][0][0]) > (s.strokes[0][1][0] - s.strokes[0][0][0]) * 1.9);

    delete_many(&eng, id, &[(0, ink)], EditSource::User).unwrap();
    assert!(list_all(&eng, id).unwrap().is_empty());
    eng.undo(id).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().len(), 1);
}

#[test]
fn replies_keep_irt_and_state_resolves() {
    let (eng, id, path) = open("reply");
    let note = add1(&eng, id, 1, NewAnnot::Note { at: [300.0, 100.0], text: "Is this the final rent?".into() });
    let steps = eng.history(id).unwrap().len();
    let (_, r) = reply_one(&eng, id, 1, note, "Yes, confirmed by mail.", "Maria Lind", EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 1);
    let all = list_all(&eng, id).unwrap();
    assert_eq!(all.len(), 2);
    let rep = all.iter().find(|a| a.xref == r).unwrap();
    assert_eq!((rep.parent, rep.author.as_str(), rep.contents.as_str()), (Some(note), "Maria Lind", "Yes, confirmed by mail."));
    assert!(!all.iter().find(|a| a.xref == note).unwrap().resolved);

    state_one(&eng, id, 1, note, "Completed", "Alex Johnson", EditSource::User).unwrap();
    let all = list_all(&eng, id).unwrap();
    assert_eq!(all.len(), 2, "the state reply is folded into its parent");
    let n = all.iter().find(|a| a.xref == note).unwrap();
    assert!(n.resolved && n.state.as_deref() == Some("Completed"));
    state_one(&eng, id, 1, note, "None", "Alex Johnson", EditSource::User).unwrap();
    assert!(!list_all(&eng, id).unwrap().iter().find(|a| a.xref == note).unwrap().resolved);
    assert!(state_one(&eng, id, 1, note, "Bogus", "x", EditSource::User).is_err());

    // Replies are hidden on the page, and deleting the parent takes them along.
    eng.save(id, None).unwrap();
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let page = doc.load_pdf_page(1).unwrap();
    let mut irt = 0;
    for a in page.annotations() {
        let o = a.object();
        if o.get_dict("IRT").unwrap().is_some() {
            irt += 1;
            assert_eq!(o.get_dict("RT").unwrap().unwrap().as_name().unwrap(), b"R");
            assert!(a.flags().unwrap().contains(mupdf::pdf::AnnotationFlags::IS_HIDDEN));
        }
    }
    assert_eq!(irt, 3);
    delete_many(&eng, id, &[(1, note)], EditSource::User).unwrap();
    assert!(list_all(&eng, id).unwrap().is_empty());
}

#[test]
fn a_page_edit_after_annotating_keeps_them() {
    let (eng, id, _) = open("organize");
    add1(&eng, id, 1, NewAnnot::Note { at: [100.0, 100.0], text: "x".into() });
    eng.apply(id, EditOp::Move { pages: vec![1], to: 0 }).unwrap();
    assert_eq!(kinds(&eng, id), vec![(Kind::Note, 0)]);
}
