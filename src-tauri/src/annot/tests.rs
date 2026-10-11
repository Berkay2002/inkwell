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
fn the_default_author_is_the_login_name() {
    let env = |pairs: &'static [(&str, &str)]| move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.to_owned());
    assert_eq!(os_user(env(&[("USERNAME", " berka "), ("USER", "x")])), "berka");
    assert_eq!(os_user(env(&[("USERNAME", "  "), ("USER", "bob")])), "bob");
    assert_eq!(os_user(env(&[])), "");
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

// ---- listing without loading every page --------------------------------------------------------

/// Objects numbered from 1 in order (1 is the catalog).
fn pdf_of(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Objects 1 and 2 (catalog, page tree) for pages that are objects `first..first + n`, then `rest`.
fn pages_and(n: usize, rest: Vec<String>, page: impl Fn(usize) -> String) -> Vec<u8> {
    let first = 3 + rest.len();
    let kids: String = (0..n).map(|i| format!("{} 0 R ", first + i)).collect();
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>")];
    objs.extend(rest);
    objs.extend((0..n).map(|i| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {} >>", page(i))));
    pdf_of(&objs)
}

fn open_raw(test: &str, bytes: Vec<u8>) -> (Arc<Engine>, DocId) {
    let p = dir(test).join("a.pdf");
    std::fs::write(&p, bytes).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    (eng, id)
}

const NOTE: &str = "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (shared) /T (Ada) >>";
const SQUARE: &str = "<< /Type /Annot /Subtype /Square /Rect [40 40 90 90] /Contents (box) >>";

#[test]
fn pages_that_share_an_annots_array_list_each_annotation_once() {
    // 3 note, 4 square, 5 the array, pages 6..=8
    let (eng, id) = open_raw("shared", pages_and(3, vec![NOTE.into(), SQUARE.into(), "[3 0 R 4 0 R]".into()], |_| "/Annots 5 0 R".into()));
    let l = list_status(&eng, id).unwrap();
    assert_eq!(l.items.iter().map(|a| (a.page, a.xref, a.kind)).collect::<Vec<_>>(), vec![(0, 3, Kind::Note), (0, 4, Kind::Rectangle)]);
    assert!(!l.incomplete);
}

#[test]
fn an_annotation_listed_by_two_pages_goes_to_the_page_its_p_names_or_else_the_first() {
    // 3 note (/P is page 3 = object 7), 4 square (no /P); pages are objects 5, 6, 7
    let note = "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (a) /P 7 0 R >>";
    let pages = ["/Annots [3 0 R 4 0 R]", "/Annots [4 0 R]", "/Annots [3 0 R]"];
    let (eng, id) = open_raw("owner", pages_and(3, vec![note.into(), SQUARE.into()], |i| pages[i].into()));
    let got: Vec<_> = list_all(&eng, id).unwrap().iter().map(|a| (a.page, a.xref)).collect();
    assert_eq!(got, vec![(0, 4), (2, 3)]);
}

/// What `list` was before it skipped pages: every page loaded, every annotation listed.
fn list_by_loading_every_page(doc: &PdfDocument) -> Vec<AnnotDto> {
    let mut all = Vec::new();
    for p in 0..doc.page_count().unwrap() as u32 {
        let page = doc.load_pdf_page(p as i32).unwrap();
        for a in page.annotations() {
            if a.r#type().is_ok_and(is_comment) {
                all.push(dto(p, &a).unwrap());
            }
        }
    }
    let mut out: Vec<AnnotDto> = Vec::new();
    let mut states: Vec<(i32, String)> = Vec::new();
    for d in all {
        match (&d.state, d.parent) {
            (Some(s), Some(parent)) => states.push((parent, s.clone())),
            _ => out.push(d),
        }
    }
    for (parent, s) in states {
        if let Some(d) = out.iter_mut().find(|d| d.xref == parent) {
            d.resolved = s == "Completed";
            d.state = Some(s);
        }
    }
    out
}

fn same_as_loading_every_page(eng: &Engine, id: DocId) -> usize {
    let want = eng.read_custom(id, |d| Ok(list_by_loading_every_page(d))).unwrap();
    let got = list_status(eng, id).unwrap();
    assert_eq!(got.items, want);
    assert!(!got.incomplete);
    want.len()
}

#[test]
fn a_normal_file_lists_what_loading_every_page_lists() {
    let (eng, id, _) = open("equal-engine");
    let mut notes = Vec::new();
    for (i, (_, what)) in every_type().into_iter().enumerate() {
        let x = add1(&eng, id, (i % 2) as u32, what);
        if matches!(what_kind(&eng, id, x), Kind::Note) {
            notes.push((i % 2, x));
        }
    }
    let (page, parent) = notes[0];
    reply_one(&eng, id, page as u32, parent, "agreed", "Sam", EditSource::User).unwrap();
    state_one(&eng, id, page as u32, parent, "Completed", "Sam", EditSource::User).unwrap();
    assert!(same_as_loading_every_page(&eng, id) >= 13);

    // Links, a popup, a widget, a redaction mark and an inline dictionary are not comments (or not
    // shared): the same list either way.
    let objs = vec![
        "<< /Type /Annot /Subtype /Link /Rect [0 0 20 20] /A << /S /URI /URI (https://example.com) >> >>".to_string(),
        "<< /Type /Annot /Subtype /Popup /Rect [0 0 20 20] >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /Rect [0 0 20 20] >>".to_string(),
        "<< /Type /Annot /Subtype /Redact /Rect [0 0 20 20] >>".to_string(),
        NOTE.to_string(),
        "<< /Type /Annot /Subtype /Highlight /Rect [10 10 90 30] /QuadPoints [10 30 90 30 10 10 90 10] >>".to_string(),
    ];
    let pages = [
        "/Annots [3 0 R 4 0 R 5 0 R 6 0 R 7 0 R << /Type /Annot /Subtype /Square /Rect [1 1 5 5] /Contents (inline) >>]",
        "",
        "/Annots [8 0 R 3 0 R]",
        "/Annots [<< /Type /Annot /Subtype /Circle /Rect [1 1 5 5] >> 8 0 R]",
    ];
    let (eng, id) = open_raw("equal-raw", pages_and(4, objs, |i| pages[i].into()));
    // The note is on page 0 only; the highlight on pages 2 and 3 is one object (listed once, on page 2:
    // loading every page lists it twice).
    let got = list_status(&eng, id).unwrap().items;
    assert_eq!(got.iter().map(|a| (a.page, a.kind)).collect::<Vec<_>>(), vec![(0, Kind::Note), (0, Kind::Rectangle), (2, Kind::Highlight), (3, Kind::Ellipse)]);
    let every = eng.read_custom(id, |d| Ok(list_by_loading_every_page(d))).unwrap();
    assert_eq!(every.len(), got.len() + 1);
    assert!(got.iter().all(|a| every.contains(a)));
}

fn what_kind(eng: &Engine, id: DocId, xref: i32) -> Kind {
    list_all(eng, id).unwrap().into_iter().find(|a| a.xref == xref).map(|a| a.kind).unwrap()
}

#[test]
fn two_thousand_pages_sharing_twenty_thousand_annotations_list_fast() {
    let list: String = (0..20_000).map(|_| "4 0 R ").collect();
    // 3 note, 4 square, 5 the array of 20 000 references to the square
    let bytes = pages_and(2000, vec![NOTE.into(), SQUARE.into(), format!("[3 0 R {list}]")], |_| "/Annots 5 0 R".into());
    let (eng, id) = open_raw("hostile", bytes);
    let t = std::time::Instant::now();
    let l = list_status(&eng, id).unwrap();
    let took = t.elapsed();
    eprintln!("annot list, 2000 pages x 20 000 shared annotations: {took:?}");
    assert_eq!(l.items.len(), 2);
    assert!(!l.incomplete);
}

#[test]
fn the_listing_stops_at_its_annotation_budget_and_says_so() {
    // Two pages with an array of 30 000 distinct annotations each: the second is over the budget.
    let n = 30_000;
    let mut rest: Vec<String> = (0..2 * n).map(|_| SQUARE.to_string()).collect();
    for k in 0..2 {
        rest.push(format!("[{}]", (0..n).map(|i| format!("{} 0 R ", 3 + k * n + i)).collect::<String>()));
    }
    let arrays = 3 + 2 * n;
    let (eng, id) = open_raw("budget", pages_and(2, rest, |i| format!("/Annots {} 0 R", arrays + i)));
    let l = list_status(&eng, id).unwrap();
    assert_eq!(l.items.len(), n);
    assert!(l.items.iter().all(|a| a.page == 0));
    assert!(l.incomplete);
}

#[test]
fn a_p_that_names_a_page_sharing_the_array_does_not_drop_the_annotation() {
    // 3 note whose /P is the last page (object 7), 4 the array every page shares; pages are objects 5, 6, 7
    let note = "<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (a) /P 7 0 R >>";
    let (eng, id) = open_raw("shared-p", pages_and(3, vec![note.into(), "[3 0 R]".into()], |_| "/Annots 4 0 R".into()));
    let got: Vec<_> = list_all(&eng, id).unwrap().iter().map(|a| (a.page, a.xref)).collect();
    assert_eq!(got, vec![(0, 3)]);
}

#[test]
fn a_reversed_document_whose_annotations_keep_their_old_p_lists_everything_fast() {
    // 100 pages of 250 squares, every /P naming the page at the mirrored position: a later page that does
    // not list it for the first half. Asking that page for its array per annotation went past the scan cap.
    let (pages, per) = (100usize, 250usize);
    let first = 3 + pages * per;
    let square = |i: usize| format!("<< /Type /Annot /Subtype /Square /Rect [40 40 90 90] /P {} 0 R >>", first + pages - 1 - i);
    let rest: Vec<String> = (0..pages).flat_map(|i| (0..per).map(move |_| i)).map(square).collect();
    let bytes = pages_and(pages, rest, |i| {
        format!("/Annots [{}]", (0..per).map(|j| format!("{} 0 R ", 3 + i * per + j)).collect::<String>())
    });
    let (eng, id) = open_raw("stale-p", bytes);
    let t = std::time::Instant::now();
    let l = list_status(&eng, id).unwrap();
    let took = t.elapsed();
    eprintln!("annot list, {pages} pages x {per} annotations with stale /P: {took:?}");
    assert_eq!(l.items.len(), pages * per);
    assert!(l.items.iter().enumerate().all(|(n, a)| a.page == (n / per) as u32));
    assert!(!l.incomplete);
}

#[test]
fn links_do_not_count_against_the_annotation_budget() {
    // One page whose array holds 50 001 links and one note: only the note is loaded as an annotation.
    let link = "<< /Type /Annot /Subtype /Link /Rect [0 0 20 20] >>";
    let n = 50_001;
    // 3 note, 4 link, 5 the array
    let list: String = (0..n).map(|_| "4 0 R ").collect();
    let (eng, id) = open_raw("links", pages_and(1, vec![NOTE.into(), link.into(), format!("[3 0 R {list}]")], |_| "/Annots 5 0 R".into()));
    let l = list_status(&eng, id).unwrap();
    assert_eq!(l.items.len(), 1);
    assert!(!l.incomplete);
}

/// 2000 pages with a direct `/Annots` of 1001 references to object 4 (a square): 2 002 000 entries,
/// all but the first a repeat. Objects: 3 note, 4 square.
fn repeated_square_file() -> Vec<u8> {
    let refs = "4 0 R ".repeat(1001);
    pages_and(2000, vec![NOTE.into(), SQUARE.into()], |_| format!("/Annots [{refs}]"))
}

#[test]
fn repeated_entries_past_the_scan_budget_leave_pages_out_and_say_so() {
    let (eng, id) = open_raw("repeats", repeated_square_file());
    let l = list_status(&eng, id).unwrap();
    assert_eq!(l.items.iter().map(|a| (a.page, a.xref)).collect::<Vec<_>>(), vec![(0, 4)]);
    assert!(l.incomplete);
    // Walking every page meets 1001 repeats on each: the cap is passed on the last two.
    let mut walk = AnnotWalk::default();
    eng.read_custom(id, |d| {
        for p in 0..2000 {
            let _ = walk.page(d, p, Which::Comments)?;
        }
        Ok(())
    })
    .unwrap();
    assert!(walk.incomplete && walk.scanned > MAX_SCANNED);
}

#[test]
fn unshared_links_do_not_use_the_scan_budget_whatever_the_page_count() {
    // 300 pages x 200 links of their own, and a note on every page: 60 000 entries met once each.
    let (pages, per) = (300usize, 200usize);
    let link = "<< /Type /Annot /Subtype /Link /Rect [0 0 20 20] >>";
    let mut rest: Vec<String> = (0..pages).map(|_| NOTE.to_string()).collect();
    rest.extend((0..pages * per).map(|_| link.to_string()));
    let bytes = pages_and(pages, rest, |i| {
        let links: String = (0..per).map(|j| format!("{} 0 R ", 3 + pages + i * per + j)).collect();
        format!("/Annots [{} 0 R {links}]", 3 + i)
    });
    let (eng, id) = open_raw("unshared-links", bytes);
    let (n, scanned, incomplete) = eng
        .read_custom(id, |d| {
            let mut walk = AnnotWalk::default();
            let mut n = 0;
            for p in 0..d.page_count()? as u32 {
                n += walk.page(d, p, Which::Comments)?.map_or(0, |f| f.nums.len());
            }
            Ok((n, walk.scanned, walk.incomplete))
        })
        .unwrap();
    assert_eq!((n, scanned, incomplete), (pages, 0, false));
    let l = list_status(&eng, id).unwrap();
    assert_eq!(l.items.len(), pages);
    assert!(!l.incomplete);
}

#[test]
fn a_page_listed_many_times_in_kids_is_walked_once() {
    // 2000 /Kids entries naming one page object whose direct /Annots holds 2000 inline squares
    let square = "<< /Type /Annot /Subtype /Square /Rect [40 40 90 90] /Contents (box) >>";
    let kids = "3 0 R ".repeat(2000);
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count 2000 >>"),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [{}] >>", vec![square; 2000].join(" ")),
    ];
    // MuPDF refuses a /Count that is not below the number of objects
    objs.extend((0..2000).map(|_| "null".to_string()));
    let (eng, id) = open_raw("dup-kids", pdf_of(&objs));
    let t = std::time::Instant::now();
    let l = list_status(&eng, id).unwrap();
    let took = t.elapsed();
    eprintln!("annot list, one page listed 2000 times with 2000 inline annotations: {took:?}");
    assert_eq!(l.items.len(), 2000);
    assert!(l.items.iter().all(|a| a.page == 0));
    assert!(!l.incomplete);
}

#[test]
fn direct_entries_are_charged_against_the_scan_budget() {
    // 2000 pages, each with 1001 inline entries: nothing to share, so every one is charged
    let (eng, id) = open_raw("direct", pages_and(2000, Vec::new(), |_| format!("/Annots [{}]", "0 ".repeat(1001))));
    let l = list_status(&eng, id).unwrap();
    assert!(l.items.is_empty());
    assert!(l.incomplete);
}
