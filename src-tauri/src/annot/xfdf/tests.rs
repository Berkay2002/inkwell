//! Comment export and import: every type through XFDF and FDF, absolute coordinates, update by name,
//! replies, skipped entries, hostile files, certified and signed documents.

use super::*;
use crate::annot::{AnnotDto, add_one, list_all, reply_one, state_one};
use crate::engine::fixture::{self, Fixture};
use crate::outline::tests::{dir, open_bytes, raw_pdf};
use std::sync::Arc;

fn fresh(test: &str) -> (Arc<Engine>, DocId, String) {
    let d = dir("xfdf", test);
    open_bytes(&d, "a.pdf", &fixture::build(&Fixture { pages: 2, lines: 4, ..Default::default() }))
}

fn every_type() -> Vec<NewAnnot> {
    let q = |x: f32, y: f32| [x, y, x + 120.0, y, x, y + 14.0, x + 120.0, y + 14.0];
    vec![
        NewAnnot::Markup { markup: Kind::Highlight, quads: vec![q(72.0, 100.0)] },
        NewAnnot::Markup { markup: Kind::Underline, quads: vec![q(72.0, 130.0)] },
        NewAnnot::Markup { markup: Kind::StrikeOut, quads: vec![q(72.0, 160.0)] },
        NewAnnot::Markup { markup: Kind::Squiggly, quads: vec![q(72.0, 190.0)] },
        NewAnnot::Note { at: [400.0, 100.0], text: "Is this the final rent? <5 & \"6\">".into() },
        NewAnnot::FreeText { rect: [300.0, 300.0, 460.0, 340.0], text: "Check with the landlord".into(), font_size: Some(12.0) },
        NewAnnot::Ink { strokes: vec![vec![[100.0, 400.0], [120.0, 380.0], [150.0, 410.0]], vec![[160.0, 400.0], [170.0, 420.0]]] },
        NewAnnot::Shape { shape: Kind::Rectangle, points: vec![[72.0, 450.0], [200.0, 500.0]] },
        NewAnnot::Shape { shape: Kind::Ellipse, points: vec![[220.0, 450.0], [300.0, 500.0]] },
        NewAnnot::Shape { shape: Kind::Line, points: vec![[72.0, 530.0], [200.0, 560.0]] },
        NewAnnot::Shape { shape: Kind::Arrow, points: vec![[220.0, 530.0], [300.0, 560.0]] },
        NewAnnot::Shape { shape: Kind::Polygon, points: vec![[320.0, 450.0], [400.0, 460.0], [380.0, 520.0]] },
        NewAnnot::Stamp { name: "Approved".into(), rect: [400.0, 600.0, 520.0, 650.0] },
    ]
}

fn add(eng: &Engine, id: DocId, page: u32, what: &NewAnnot) -> i32 {
    add_one(eng, id, page, what, &Style::default(), "Alex Johnson", EditSource::User).unwrap().1
}

fn raw_rect(eng: &Engine, id: DocId, page: u32, xref: i32) -> Vec<f32> {
    eng.read_custom(id, |d| {
        let (_p, a) = find(d, page, xref)?;
        Ok(floats(&a.object().get_dict("Rect")?.expect("/Rect"), 8, &Budget::new()))
    })
    .unwrap()
}

fn close(a: &[f32], b: &[f32], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: {a:?} vs {b:?}");
    for (x, y) in a.iter().zip(b) {
        assert!((x - y).abs() < 1.5, "{what}: {a:?} vs {b:?}");
    }
}

fn same(a: &AnnotDto, b: &AnnotDto) {
    let what = format!("{:?}", a.kind);
    assert_eq!((a.kind, a.page, &a.author, &a.contents, &a.created, &a.modified), (b.kind, b.page, &b.author, &b.contents, &b.created, &b.modified), "{what}");
    close(&a.rect, &b.rect, &format!("{what} rect"));
    assert_eq!((a.quads.len(), a.points.len(), a.strokes.len(), &a.icon), (b.quads.len(), b.points.len(), b.strokes.len(), &b.icon), "{what}");
    for (p, q) in a.points.iter().zip(&b.points) {
        close(p, q, &format!("{what} point"));
    }
    for (p, q) in a.quads.iter().zip(&b.quads) {
        close(p, q, &format!("{what} quad"));
    }
    assert_eq!(a.color.is_some(), b.color.is_some(), "{what} colour");
}

#[test]
fn every_comment_type_round_trips_through_xfdf_and_fdf_as_one_undo_step() {
    for (fmt, ext) in [(Format::Xfdf, "xfdf"), (Format::Fdf, "fdf")] {
        let (a, ida, _) = fresh(&format!("rt-{ext}-a"));
        for w in every_type() {
            add(&a, ida, 0, &w);
        }
        let (bytes, done) = export(&a, ida, fmt, "a.pdf").unwrap();
        assert_eq!((done.count, done.left_out), (13, 0), "{ext}");
        let want = list_all(&a, ida).unwrap();

        let parsed = parse(&bytes).unwrap();
        assert_eq!((parsed.items.len(), parsed.unknown, parsed.over), (13, 0, 0), "{ext}");
        let (b, idb, _) = fresh(&format!("rt-{ext}-b"));
        let steps = b.history(idb).unwrap().len();
        let got = import_one(&b, idb, &parsed, EditSource::User).unwrap();
        assert_eq!((got.summary.added, got.summary.updated, got.signed), (13, 0, false), "{ext}: {:?}", got.summary);
        assert!(got.summary.skipped.is_empty(), "{ext}: {:?}", got.summary.skipped);
        assert_eq!(b.history(idb).unwrap().len(), steps + 1, "{ext}: one undo step");

        let have = list_all(&b, idb).unwrap();
        assert_eq!(have.len(), 13, "{ext}");
        for (x, y) in want.iter().zip(&have) {
            same(x, y);
        }
        b.undo(idb).unwrap();
        assert!(list_all(&b, idb).unwrap().is_empty(), "{ext}: undo takes all of them away");
    }
}

#[test]
fn geometry_is_pdf_user_space_in_the_files_and_maps_back_through_the_page() {
    let (eng, id, _) = fresh("abs");
    // The DTO is page space (y down): this quad is 72..192 across and 100..114 down, so in user space
    // of a 792 pt page it is 72..192 and 678..692 (MuPDF pads the /Rect a little around it).
    let x = add(&eng, id, 0, &NewAnnot::Markup { markup: Kind::Highlight, quads: vec![[72.0, 100.0, 192.0, 100.0, 72.0, 114.0, 192.0, 114.0]] });
    let raw = raw_rect(&eng, id, 0, x);
    let (bytes, _) = export(&eng, id, Format::Xfdf, "a.pdf").unwrap();
    let parsed = parse(&bytes).unwrap();
    close(&parsed.items[0].rect.unwrap(), &raw, "the file carries the /Rect as stored");
    assert!(raw[1] > 670.0 && raw[3] < 700.0 && raw[3] > 685.0, "{raw:?} is in user space (y up), not page space");
    close(&parsed.items[0].quads, &[72.0, 692.0, 192.0, 692.0, 72.0, 678.0, 192.0, 678.0], "exported QuadPoints");
    assert_eq!(parsed.items[0].page, 0);

    // A file written by hand, in user space, on page 2.
    let file = br##"<?xml version="1.0"?><xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <square page="1" rect="100,600,200,700" name="sq-1" title="Pat" color="#FF0000" date="D:20260102030405Z"><contents>Hand</contents></square>
        </annots></xfdf>"##;
    let (b, idb, _) = fresh("abs-b");
    let r = import_one(&b, idb, &parse(file).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.added, 1);
    let d = list_all(&b, idb).unwrap();
    assert_eq!((d[0].page, d[0].author.as_str(), d[0].contents.as_str(), d[0].modified.as_str()), (1, "Pat", "Hand", "D:20260102030405Z"));
    close(&raw_rect(&b, idb, 1, d[0].xref), &[100.0, 600.0, 200.0, 700.0], "imported raw /Rect");
    close(&d[0].rect, &[100.0, 92.0, 200.0, 192.0], "page-space rect");
}

#[test]
fn import_updates_by_name_in_place_adds_the_rest_and_keeps_replies_and_states() {
    let (a, ida, _) = fresh("upd");
    let note = add(&a, ida, 0, &NewAnnot::Note { at: [100.0, 100.0], text: "Is this the final rent?".into() });
    let reply = reply_one(&a, ida, 0, note, "Yes, signed today", "Sam", EditSource::User).unwrap().1;
    state_one(&a, ida, 0, note, "Completed", "Alex Johnson", EditSource::User).unwrap();
    let before = list_all(&a, ida).unwrap();
    let (bytes, done) = export(&a, ida, Format::Xfdf, "a.pdf").unwrap();
    assert_eq!(done.count, before.len() + 1, "the state is a comment of its own in the file");

    // Into the same document: everything is matched by name, nothing is added.
    let r = import_one(&a, ida, &parse(&bytes).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.added, 0, "{:?}", r.summary);
    assert!(r.summary.updated >= 2, "{:?}", r.summary);
    let after = list_all(&a, ida).unwrap();
    assert_eq!(after.iter().map(|d| d.xref).collect::<Vec<_>>(), before.iter().map(|d| d.xref).collect::<Vec<_>>());

    // Changed text of the file lands on the same comment, whose reply stays under it.
    let changed = String::from_utf8(bytes.clone()).unwrap().replace("Is this the final rent?", "Changed");
    let r = import_one(&a, ida, &parse(changed.as_bytes()).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.added, 0);
    let now = list_all(&a, ida).unwrap();
    assert_eq!(now.iter().find(|d| d.xref == note).unwrap().contents, "Changed");
    assert_eq!(now.iter().find(|d| d.xref == reply).unwrap().parent, Some(note));

    // Into another document: the reply is made under the new parent, the state resolves it.
    let (b, idb, _) = fresh("upd-b");
    let r = import_one(&b, idb, &parse(&bytes).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.updated, 0);
    let have = list_all(&b, idb).unwrap();
    let n = have.iter().find(|d| d.kind == Kind::Note && d.parent.is_none()).expect("note");
    let rep = have.iter().find(|d| d.contents == "Yes, signed today").expect("reply");
    assert_eq!((rep.parent, rep.author.as_str()), (Some(n.xref), "Sam"));
    assert!(n.resolved, "the review state came along");
}

#[test]
fn a_page_out_of_range_and_types_the_files_cannot_carry_are_skipped_and_counted() {
    let file = br##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <square page="9" rect="100,600,200,700" name="far"/>
        <square page="-1" rect="100,600,200,700" name="neg"/>
        <caret page="0" rect="100,600,110,610" name="c"/>
        <fileattachment page="0" rect="100,600,110,610" name="f"/>
        <sound page="0" rect="100,600,110,610" name="s"/>
        <square page="0" rect="100,600,200,700" name="ok"/>
        </annots></xfdf>"##;
    let (eng, id, _) = fresh("skip");
    let r = import_one(&eng, id, &parse(file).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.added, 1);
    assert_eq!(r.summary.skipped.iter().map(|s| s.count).sum::<usize>(), 5, "{:?}", r.summary.skipped);
    assert_eq!(list_all(&eng, id).unwrap().len(), 1);
}

#[test]
fn hostile_files_are_refused_or_read_harmlessly() {
    let head = r#"<?xml version="1.0"?>"#;
    let laughs = format!(
        "{head}<!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY l1 \"&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;\"><!ENTITY l2 \"&l1;&l1;&l1;&l1;&l1;&l1;&l1;&l1;&l1;&l1;\">]>\
         <xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><annots><text page=\"0\" rect=\"1,1,2,2\"><contents>&l2;</contents></text></annots></xfdf>"
    );
    let external = format!(
        "{head}<!DOCTYPE x [<!ENTITY e SYSTEM \"file:///etc/passwd\">]><xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><annots><text page=\"0\" rect=\"1,1,2,2\"><contents>&e;</contents></text></annots></xfdf>"
    );
    let deep = format!("{head}<xfdf>{}{}</xfdf>", "<a>".repeat(5000), "</a>".repeat(5000));
    for (what, f) in [("billion laughs", laughs), ("external entity", external), ("deep nesting", deep)] {
        assert!(matches!(parse(f.as_bytes()), Err(EngineError::Invalid(_))), "{what}");
    }

    // Truncated and damaged files never panic: every prefix and a few byte flips either parse or fail.
    let (eng, id, _) = fresh("hostile");
    for w in every_type() {
        add(&eng, id, 0, &w);
    }
    for fmt in [Format::Xfdf, Format::Fdf] {
        let (bytes, _) = export(&eng, id, fmt, "a.pdf").unwrap();
        for cut in (0..bytes.len()).step_by(37) {
            let _ = parse(&bytes[..cut]);
        }
        for at in (0..bytes.len()).step_by(53) {
            let mut b = bytes.clone();
            b[at] ^= 0xA5;
            let _ = parse(&b);
        }
    }
    for junk in [&b"%FDF-1.2\n\x00\xff\xfe garbage"[..], b"%FDF-1.2\n1 0 obj << /FDF << /Annots 99 0 R >> >> endobj\ntrailer << /Root 1 0 R >>", b"", b"\x00\x01\x02"] {
        let _ = parse(junk);
    }
}

#[test]
fn an_fdf_with_javascript_and_file_references_is_read_without_them() {
    let fdf = b"%FDF-1.2\n\
        1 0 obj\n<< /FDF << /Annots [2 0 R] /F (C:\\\\Windows\\\\evil.exe) /JavaScript << /Before (app.alert\\(1\\)) /After (evil_after) >> >> >>\nendobj\n\
        2 0 obj\n<< /Type /Annot /Subtype /Square /Page 0 /Rect [100 600 200 700] /NM (x1) /Contents (hi) \
        /AA << /E << /S /JavaScript /JS (evil_aa) >> >> /A << /S /JavaScript /JS (evil_a) >> >>\nendobj\n\
        trailer\n<< /Root 1 0 R >>\n%%EOF\n";
    let (eng, id, _) = fresh("fdf-js");
    let r = import_one(&eng, id, &parse(fdf).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.added, 1);
    let bytes = eng.unsaved_bytes(id).unwrap().expect("pending edit");
    let text = String::from_utf8_lossy(&bytes);
    for s in ["evil", "JavaScript", "/AA"] {
        assert!(!text.contains(s), "{s} reached the document");
    }
    close(&raw_rect(&eng, id, 0, list_all(&eng, id).unwrap()[0].xref), &[100.0, 600.0, 200.0, 700.0], "rect");
}

fn one_page_pdf(catalog_extra: &str, page_extra: &str, more: &[&str]) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {page_extra} >>"),
    ];
    objs.extend(more.iter().map(|s| s.to_string()));
    raw_pdf(&objs)
}

const ONE: &[u8] = br##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><square page="0" rect="100,600,200,700" name="n1"><contents>One</contents></square></annots></xfdf>"##;

#[test]
fn a_certified_document_refuses_import_without_a_history_step_but_exports() {
    let cat = "/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 2 >> >> ] >> >>";
    let d = dir("xfdf", "certified");
    let (eng, id, _) = open_bytes(&d, "c.pdf", &one_page_pdf(cat, "", &[]));
    let before = eng.edit_state(id).unwrap();
    let steps = eng.history(id).unwrap().len();
    match import_one(&eng, id, &parse(ONE).unwrap(), EditSource::User) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
        other => panic!("{:?}", other.map(|r| r.summary)),
    }
    let after = eng.edit_state(id).unwrap();
    assert_eq!((after.rev, after.undo, after.dirty), (before.rev, before.undo, before.dirty));
    assert_eq!(eng.history(id).unwrap().len(), steps);
    assert!(export(&eng, id, Format::Xfdf, "c.pdf").is_ok(), "export is read-only");
}

#[test]
fn a_signed_document_takes_comments_as_an_update_after_the_signature() {
    let cat = "/AcroForm << /SigFlags 3 /Fields [4 0 R] >>";
    let sig = "<< /FT /Sig /T (Signature1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>";
    let d = dir("xfdf", "signed");
    let (eng, id, path) = open_bytes(&d, "s.pdf", &one_page_pdf(cat, "", &[sig]));
    let original = std::fs::read(&path).unwrap();
    let r = import_one(&eng, id, &parse(ONE).unwrap(), EditSource::User).unwrap();
    assert!(r.signed);
    assert_eq!(r.summary.added, 1);
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the signed bytes are kept as a prefix");
}

#[test]
fn a_rotated_page_with_an_offset_crop_box_still_gets_the_file_s_user_space_numbers() {
    let d = dir("xfdf", "rotated");
    let (eng, id, _) = open_bytes(&d, "r.pdf", &one_page_pdf("", "/Rotate 90 /CropBox [50 40 562 700]", &[]));
    let r = import_one(&eng, id, &parse(ONE).unwrap(), EditSource::User).unwrap();
    assert_eq!(r.summary.added, 1);
    let x = list_all(&eng, id).unwrap()[0].xref;
    close(&raw_rect(&eng, id, 0, x), &[100.0, 600.0, 200.0, 700.0], "raw /Rect on a rotated page");
    let (bytes, _) = export(&eng, id, Format::Xfdf, "r.pdf").unwrap();
    close(&parse(&bytes).unwrap().items[0].rect.unwrap(), &[100.0, 600.0, 200.0, 700.0], "and back out");
}

#[test]
fn measurements_are_left_out_of_a_file_and_are_neither_made_nor_changed_by_one() {
    use crate::measure::{Kind as MKind, add_one as measure};
    let (a, ida, _) = fresh("measure");
    add(&a, ida, 0, &NewAnnot::Note { at: [100.0, 100.0], text: "keep".into() });
    let (_, m, _) = measure(&a, ida, 0, MKind::Distance, &[[100.0, 300.0], [300.0, 300.0]], None, "Alex", EditSource::User).unwrap();
    reply_one(&a, ida, 0, m, "Check the scale", "Sam", EditSource::User).unwrap();
    for fmt in [Format::Xfdf, Format::Fdf] {
        let (bytes, done) = export(&a, ida, fmt, "a.pdf").unwrap();
        assert_eq!((done.count, done.left_out), (1, 2), "the measurement and its reply are counted, not dropped silently");
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("Check the scale") && !text.contains("Measure"), "{fmt:?}");
    }

    // A measurement in a file (FDF with /Measure, XFDF with a dimension intent) is skipped and counted.
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [ \
        << /Subtype /Line /Page 0 /Rect [100 500 300 520] /L [100 510 300 510] /NM (m1) /IT /LineDimension \
           /Measure << /Type /Measure /Subtype /RL /R (1 in = 1 ft) /X [<< /Type /NumberFormat /U (ft) /C 1 >>] >> >> \
        << /Subtype /Text /Page 0 /Rect [100 100 120 120] /NM (n1) /Contents (kept) >> ] >> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    let xfdf = br##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots>
        <line page="0" rect="100,500,300,520" start="100,510" end="300,510" intent="LineDimension" name="m2"/>
        <polygon page="0" rect="100,500,300,520" name="m3" intent="PolygonDimension"><vertices>100,500;300,500;200,520</vertices></polygon>
        <text page="0" rect="100,100,120,120" name="n2"><contents>kept</contents></text></annots></xfdf>"##;
    for (what, file, skipped) in [("fdf", &fdf[..], 1), ("xfdf", &xfdf[..], 2)] {
        let (b, idb, _) = fresh(&format!("measure-in-{what}"));
        let r = import_one(&b, idb, &parse(file).unwrap(), EditSource::User).unwrap();
        assert_eq!(r.summary.added, 1, "{what}: {:?}", r.summary);
        assert_eq!(r.summary.skipped.iter().map(|s| s.count).sum::<usize>(), skipped, "{what}: {:?}", r.summary.skipped);
        let list = list_all(&b, idb).unwrap();
        assert!(list.iter().all(|d| d.kind == Kind::Note), "{what}: no line or polygon was made");
    }

    // A measurement of the document that a file's comment has the name of is not updated.
    let (b, idb, _) = fresh("measure-name");
    let (_, mx, _) = measure(&b, idb, 0, MKind::Distance, &[[100.0, 300.0], [300.0, 300.0]], None, "Alex", EditSource::User).unwrap();
    b.apply_custom(idb, "Name".into(), EditSource::User, false, |d| {
        let (_p, ann) = find(d, 0, mx)?;
        put_str(&ann, "NM", "m-doc")?;
        Ok(vec![0])
    })
    .unwrap();
    let before = raw_rect(&b, idb, 0, mx);
    let file = br##"<xfdf xmlns="http://ns.adobe.com/xfdf/"><annots><line page="0" rect="10,10,50,50" start="10,10" end="50,50" name="m-doc"><contents>moved</contents></line></annots></xfdf>"##;
    let r = import_one(&b, idb, &parse(file).unwrap(), EditSource::User).unwrap();
    assert_eq!((r.summary.added, r.summary.updated), (0, 0), "{:?}", r.summary);
    assert_eq!(raw_rect(&b, idb, 0, mx), before);
}

/// An FDF whose annotations are objects 10.. and `shared` is object 2, with `extra` objects after them.
fn fdf_sharing(n: usize, annot: impl Fn(usize) -> String, objects: &[String]) -> Vec<u8> {
    let refs: String = (0..n).map(|i| format!("{} 0 R ", 10 + i)).collect();
    let mut f = format!("%FDF-1.2\n1 0 obj\n<< /FDF << /Annots [{refs}] >> >>\nendobj\n");
    for o in objects {
        f.push_str(o);
    }
    for i in 0..n {
        f.push_str(&format!("{} 0 obj\n{}\nendobj\n", 10 + i, annot(i)));
    }
    f.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    f.into_bytes()
}

#[test]
fn one_string_or_ink_array_shared_by_many_comments_is_refused_by_the_byte_budget() {
    let text = |i: usize, r: &str| format!("<< /Subtype /Text /Page 0 /Rect [100 600 120 620] /NM (n{i}) /Contents {r} >>");
    let big = format!("2 0 obj\n({})\nendobj\n", "x".repeat(900_000));
    // 200 references to one 900 kB string would be 180 MB of copies.
    let f = fdf_sharing(200, |i| text(i, "2 0 R"), std::slice::from_ref(&big));
    assert!(matches!(parse(&f), Err(EngineError::Invalid(m)) if m.contains("same data")));
    // A few are fine and read in full.
    let ok = parse(&fdf_sharing(5, |i| text(i, "2 0 R"), std::slice::from_ref(&big))).unwrap();
    assert_eq!(ok.items.len(), 5);
    assert!(ok.items.iter().all(|c| c.contents.len() == 900_000));

    // One ink list of 1024 strokes, each the same 65 536 numbers, on 100 comments.
    let nums = "1 ".repeat(65_536);
    let strokes: String = "6 0 R ".repeat(1024);
    let objs = [format!("5 0 obj\n[{strokes}]\nendobj\n"), format!("6 0 obj\n[{nums}]\nendobj\n")];
    let f = fdf_sharing(100, |i| format!("<< /Subtype /Ink /Page 0 /Rect [0 0 50 50] /NM (i{i}) /InkList 5 0 R >>"), &objs);
    assert!(matches!(parse(&f), Err(EngineError::Invalid(m)) if m.contains("same data")));
}
