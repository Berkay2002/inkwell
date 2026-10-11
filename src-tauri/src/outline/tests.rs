//! Bookmarks through the edit lane: list, add, rename, nest, move, delete, destination, style,
//! from headings, undo and a round trip through save.

use super::*;
use crate::engine::fixture::{self, Fixture};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) fn dir(group: &str, test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(group).join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A PDF with one page per content stream (Helvetica as /F1, 612 x 792).
pub(crate) fn pdf_with(pages: &[&str]) -> Vec<u8> {
    let n = pages.len();
    let kids = (0..n).map(|i| format!("{} 0 R", 4 + i * 2)).collect::<Vec<_>>().join(" ");
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>").into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
    ];
    for (i, c) in pages.iter().enumerate() {
        objs.push(
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>", 5 + i * 2)
                .into_bytes(),
        );
        let mut s = format!("<< /Length {} >>\nstream\n", c.len()).into_bytes();
        s.extend_from_slice(c.as_bytes());
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

pub(crate) fn open_bytes(d: &Path, name: &str, bytes: &[u8]) -> (Arc<Engine>, DocId, String) {
    let p = d.join(name);
    std::fs::write(&p, bytes).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

fn open_fixture(d: &Path, f: &Fixture) -> (Arc<Engine>, DocId, String) {
    open_bytes(d, "a.pdf", &fixture::build(f))
}

fn rev(eng: &Engine, id: DocId) -> u32 {
    eng.edit_state(id).unwrap().rev
}

fn go(eng: &Engine, id: DocId, op: BookmarkOp) -> BookmarkEdited {
    apply(eng, id, rev(eng, id), &op, EditSource::User).unwrap_or_else(|e| panic!("{op:?} failed: {e:?}"))
}

fn titles(items: &[Bookmark]) -> Vec<String> {
    items.iter().map(|b| b.title.clone()).collect()
}

/// `/Count` of the outline root and of every item, from the file's own objects.
fn counts(eng: &Engine, id: DocId) -> Vec<(String, i64)> {
    eng.read_custom(id, |d| {
        fn walk(sh: &Shape, key: Option<i32>, out: &mut Vec<(String, i64)>) {
            for id in sh.kid_ids(key) {
                let k = &sh.refs[id];
                let c = number(&k.get_dict("Count").unwrap()).unwrap_or(0.0) as i64;
                out.push((string_of(k.get_dict("Title").unwrap()), c));
                walk(sh, Some(*id), out);
            }
        }
        let r = root(d)?.unwrap();
        let sh = shape(&r)?;
        let mut out = vec![("<root>".to_string(), number(&r.get_dict("Count")?).unwrap_or(0.0) as i64)];
        walk(&sh, None, &mut out);
        Ok(out)
    })
    .unwrap()
}

fn count_of(c: &[(String, i64)], title: &str) -> i64 {
    c.iter().find(|(t, _)| t == title).unwrap().1
}

fn at_of(b: &Bookmark) -> Option<[f32; 2]> {
    match &b.dest {
        Dest::Page { at, .. } => *at,
        _ => None,
    }
}

#[test]
fn lists_the_outline_of_a_document() {
    let d = dir("outline", "list");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 6, chapter_every: 2, ..Default::default() });
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["Chapter 1", "Chapter 2", "Chapter 3"]);
    assert_eq!(b.count, 3);
    assert_eq!(b.rev, rev(&eng, id));
    assert_eq!(b.items[1].dest, Dest::Page { page: 2, at: Some([0.0, 0.0]) });
    assert!(!b.truncated);
}

#[test]
fn a_document_without_an_outline_lists_nothing() {
    let d = dir("outline", "none");
    let (eng, id, _) = open_fixture(&d, &Fixture::default());
    assert!(list(&eng, id).unwrap().items.is_empty());
}

#[test]
fn adds_renames_and_deletes_each_as_one_undo_step() {
    let d = dir("outline", "steps");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 6, chapter_every: 2, ..Default::default() });
    let r0 = rev(&eng, id);
    let added = go(&eng, id, BookmarkOp::Add { parent: None, index: Some(1), title: "  Summary  ".into(), page: 4, at: Some([72.0, 100.0]) });
    assert_eq!(added.state.undo.as_deref(), Some("Add bookmark \"Summary\""));
    assert!(rev(&eng, id) > r0);
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["Chapter 1", "Summary", "Chapter 2", "Chapter 3"]);
    let s = &b.items[1];
    assert_eq!(s.id, added.id.unwrap());
    match &s.dest {
        Dest::Page { page, at: Some([x, y]) } => {
            assert_eq!(*page, 4);
            assert!((x - 72.0).abs() < 0.1 && (y - 100.0).abs() < 0.1, "{x} {y}");
        }
        other => panic!("{other:?}"),
    }

    go(&eng, id, BookmarkOp::Rename { id: s.id, title: "Overview".into() });
    assert_eq!(titles(&list(&eng, id).unwrap().items)[1], "Overview");
    let gone = go(&eng, id, BookmarkOp::Delete { ids: vec![s.id] });
    assert_eq!(gone.count, 1);
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["Chapter 1", "Chapter 2", "Chapter 3"]);

    // Three changes: three undo steps back.
    eng.undo(id).unwrap();
    assert_eq!(titles(&list(&eng, id).unwrap().items)[1], "Overview");
    eng.undo(id).unwrap();
    assert_eq!(titles(&list(&eng, id).unwrap().items)[1], "Summary");
    eng.undo(id).unwrap();
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["Chapter 1", "Chapter 2", "Chapter 3"]);
    eng.redo(id).unwrap();
    assert_eq!(titles(&list(&eng, id).unwrap().items)[1], "Summary");
}

#[test]
fn a_document_without_an_outline_gets_one() {
    let d = dir("outline", "first");
    let (eng, id, _) = open_fixture(&d, &Fixture::default());
    go(&eng, id, BookmarkOp::Add { parent: None, index: None, title: "Start".into(), page: 0, at: None });
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["Start"]);
    assert_eq!(b.items[0].dest, Dest::Page { page: 0, at: None });
    assert_eq!(counts(&eng, id), [("<root>".to_string(), 1), ("Start".to_string(), 0)]);
}

#[test]
fn nests_moves_and_keeps_the_counts_right() {
    let d = dir("outline", "nest");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 8, chapter_every: 2, ..Default::default() });
    let b = list(&eng, id).unwrap();
    let (c1, c2, c3, c4) = (b.items[0].id, b.items[1].id, b.items[2].id, b.items[3].id);
    // Chapter 2 and 3 go under Chapter 1; Chapter 4 under Chapter 2.
    go(&eng, id, BookmarkOp::Move { id: c2, parent: Some(c1), index: 0 });
    go(&eng, id, BookmarkOp::Move { id: c3, parent: Some(c1), index: 1 });
    go(&eng, id, BookmarkOp::Move { id: c4, parent: Some(c2), index: 0 });
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["Chapter 1"]);
    assert_eq!(titles(&b.items[0].children), ["Chapter 2", "Chapter 3"]);
    assert_eq!(titles(&b.items[0].children[0].children), ["Chapter 4"]);
    assert!(b.items[0].open, "a group that took members is shown open");
    assert_eq!(b.count, 4);
    // Everything is open: three items below the first, one below the second.
    let c = counts(&eng, id);
    assert_eq!(c[0], ("<root>".into(), 4));
    assert_eq!(count_of(&c, "Chapter 1"), 3);
    assert_eq!(count_of(&c, "Chapter 2"), 1);

    // Collapse Chapter 1: the sign flips, its size stays, the root counts only what is shown.
    go(&eng, id, BookmarkOp::SetOpen { id: c1, open: false });
    let c = counts(&eng, id);
    assert_eq!(c[0].1, 1);
    assert_eq!(count_of(&c, "Chapter 1"), -3);
    assert!(!list(&eng, id).unwrap().items[0].open);

    // A bookmark can't go into itself or under what is under it.
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::Move { id: c1, parent: Some(c4), index: 0 }, EditSource::User);
    assert!(matches!(r, Err(EngineError::Invalid(_))), "{r:?}");
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::Move { id: c2, parent: Some(c2), index: 0 }, EditSource::User);
    assert!(matches!(r, Err(EngineError::Invalid(_))), "{r:?}");

    // Deleting a group takes what is under it.
    let gone = go(&eng, id, BookmarkOp::Delete { ids: vec![c1] });
    assert_eq!(gone.count, 4);
    assert!(list(&eng, id).unwrap().items.is_empty());
    assert_eq!(counts(&eng, id), [("<root>".to_string(), 0)]);
}

#[test]
fn deleting_nested_picks_counts_each_item_once() {
    let d = dir("outline", "delete-nested");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 6, chapter_every: 2, ..Default::default() });
    let b = list(&eng, id).unwrap();
    let (c1, c2) = (b.items[0].id, b.items[1].id);
    go(&eng, id, BookmarkOp::Move { id: c2, parent: Some(c1), index: 0 });
    let gone = go(&eng, id, BookmarkOp::Delete { ids: vec![c1, c2] });
    assert_eq!(gone.count, 2);
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["Chapter 3"]);
}

#[test]
fn sets_the_destination_and_the_style() {
    let d = dir("outline", "style");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 4, chapter_every: 2, ..Default::default() });
    let c1 = list(&eng, id).unwrap().items[0].id;
    go(&eng, id, BookmarkOp::SetDest { id: c1, page: 3, at: Some([50.0, 400.0]) });
    go(&eng, id, BookmarkOp::SetStyle { id: c1, bold: Some(true), italic: Some(true), color: Some([0.8, 0.2, 0.1]), reset_color: None });
    let b = &list(&eng, id).unwrap().items[0];
    assert!(b.bold && b.italic);
    assert_eq!(b.color.map(|c| c.map(|v| (v * 10.0).round() as i32)), Some([8, 2, 1]));
    match &b.dest {
        Dest::Page { page: 3, at: Some([x, y]) } => assert!((x - 50.0).abs() < 0.1 && (y - 400.0).abs() < 0.1),
        other => panic!("{other:?}"),
    }
    go(&eng, id, BookmarkOp::SetStyle { id: c1, bold: Some(false), italic: None, color: None, reset_color: Some(true) });
    let b = &list(&eng, id).unwrap().items[0];
    assert!(!b.bold && b.italic && b.color.is_none());
}

#[test]
fn a_stale_change_is_refused_and_changes_nothing() {
    let d = dir("outline", "stale");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 4, chapter_every: 2, ..Default::default() });
    let old = rev(&eng, id);
    go(&eng, id, BookmarkOp::Add { parent: None, index: None, title: "New".into(), page: 0, at: None });
    let r = apply(&eng, id, old, &BookmarkOp::Add { parent: None, index: None, title: "Late".into(), page: 0, at: None }, EditSource::User);
    assert!(matches!(r, Err(EngineError::Stale)), "{r:?}");
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["Chapter 1", "Chapter 2", "New"]);
    // An id that is no bookmark (a page object) is refused too.
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::Rename { id: 5, title: "x".into() }, EditSource::User);
    assert!(r.is_err(), "{r:?}");
}

#[test]
fn titles_keep_their_characters_through_save_and_reopen() {
    let d = dir("outline", "unicode");
    let (eng, id, path) = open_fixture(&d, &Fixture { pages: 2, ..Default::default() });
    go(&eng, id, BookmarkOp::Add { parent: None, index: None, title: "Överblick — 概要 ✓".into(), page: 1, at: Some([10.0, 20.0]) });
    go(&eng, id, BookmarkOp::Add { parent: None, index: None, title: "Plain".into(), page: 0, at: None });
    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let b = list(&eng2, id2).unwrap();
    assert_eq!(titles(&b.items), ["Överblick — 概要 ✓", "Plain"]);
    assert_eq!(b.items[0].dest, Dest::Page { page: 1, at: Some([10.0, 20.0]) });
    // And MuPDF's own reader (the sidebar) sees the same.
    let side = eng2.outline(id2).unwrap();
    assert_eq!(side[0].title, "Överblick — 概要 ✓");
    assert_eq!(side[0].page, Some(1));
}

#[test]
fn empty_names_and_pages_out_of_range_are_refused() {
    let d = dir("outline", "bad");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 2, ..Default::default() });
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::Add { parent: None, index: None, title: "   ".into(), page: 0, at: None }, EditSource::User);
    assert!(matches!(r, Err(EngineError::Invalid(_))));
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::Add { parent: None, index: None, title: "X".into(), page: 9, at: None }, EditSource::User);
    assert!(matches!(r, Err(EngineError::BadPage(9))));
    // Nothing was added by the failed ones, and the document is still clean.
    assert!(list(&eng, id).unwrap().items.is_empty());
    assert!(!eng.edit_state(id).unwrap().dirty);
}

#[test]
fn bookmarks_from_headings_predict_then_make_the_same_count() {
    let d = dir("outline", "headings");
    // Body 11 pt; 24 pt chapter titles, 16 pt section titles; a small running head on each page.
    let page = |chapter: Option<&str>, section: &str| {
        let mut s = String::from("BT /F1 9 Tf 72 760 Td (Annual report) Tj ET\n");
        let mut y = 700;
        if let Some(c) = chapter {
            s.push_str(&format!("BT /F1 24 Tf 72 {y} Td ({c}) Tj ET\n"));
            y -= 40;
        }
        s.push_str(&format!("BT /F1 16 Tf 72 {y} Td ({section}) Tj ET\n"));
        y -= 30;
        for i in 0..8 {
            s.push_str(&format!(
                "BT /F1 11 Tf 72 {} Td (Body text of the page, a sentence that goes on for a while, number {i}.) Tj ET\n",
                y - i * 16
            ));
        }
        s
    };
    let pages = [page(Some("Chapter One"), "First section"), page(None, "Second section"), page(Some("Chapter Two"), "Third section")];
    let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
    let (eng, id, _) = open_bytes(&d, "h.pdf", &pdf_with(&refs));
    let p = preview_headings(&eng, id).unwrap();
    assert_eq!(p.source, "size", "an untagged file is guessed from the size of the text");
    assert_eq!(p.count, 5, "{p:?}");
    assert_eq!(p.levels, 2);
    assert_eq!(p.rows[0], HeadingRow { title: "Chapter One".into(), page: 0, level: 1 });
    let made = go(&eng, id, BookmarkOp::FromHeadings { replace: false });
    assert_eq!(made.count, p.count);
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["Chapter One", "Chapter Two"]);
    assert_eq!(titles(&b.items[0].children), ["First section", "Second section"]);
    assert_eq!(titles(&b.items[1].children), ["Third section"]);
    assert!(at_of(&b.items[1]).is_some());
    // One undo step for all of them.
    eng.undo(id).unwrap();
    assert!(list(&eng, id).unwrap().items.is_empty());
}

#[test]
fn a_page_text_with_no_headings_makes_no_bookmarks() {
    let d = dir("outline", "no-headings");
    let body = "BT /F1 11 Tf 72 700 Td (Just body text, all the same size.) Tj ET";
    let (eng, id, _) = open_bytes(&d, "n.pdf", &pdf_with(&[body]));
    assert_eq!(preview_headings(&eng, id).unwrap().count, 0);
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::FromHeadings { replace: false }, EditSource::User);
    assert!(matches!(r, Err(EngineError::Invalid(_))));
}

#[test]
fn text_decoding_follows_the_pdf_encodings() {
    assert_eq!(decode_text(b"Plain"), "Plain");
    assert_eq!(decode_text(&[0xFE, 0xFF, 0x00, 0x41, 0x4E, 0x2D]), "A中");
    assert_eq!(decode_text(&[0xFF, 0xFE, 0x41, 0x00]), "A");
    assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, 0xC3, 0xA9]), "é");
    // PDFDocEncoding: bullet and the euro sign, Latin-1 for the rest.
    assert_eq!(decode_text(&[0x80, 0xA0, 0xE9]), "•€é");
}

#[test]
fn a_bookmark_to_a_missing_named_destination_is_broken() {
    let d = dir("outline", "broken");
    let (eng, id, _p) = open_fixture(&d, &Fixture { pages: 2, chapter_every: 1, ..Default::default() });
    let first = list(&eng, id).unwrap().items[0].id;
    eng.apply_custom(id, "Break bookmark".into(), EditSource::User, false, |d| {
        let mut it = walked(d)?.1.obj(first)?;
        it.dict_put("Dest", d.new_object_from_str("(gone)")?)?;
        Ok(Vec::new())
    })
    .unwrap();
    match &list(&eng, id).unwrap().items[0].dest {
        Dest::Broken { what } => assert!(what.contains("gone"), "{what}"),
        other => panic!("{other:?}"),
    }
}

/// A one-page PDF with `objs` as objects 1.. (a hand-made xref).
pub(crate) fn raw_pdf(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7
".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj
{body}
endobj
", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref
0 {}
0000000000 65535 f 
", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n 
").as_bytes());
    }
    out.extend_from_slice(format!("trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF
", objs.len() + 1).as_bytes());
    out
}

#[test]
fn a_tagged_file_names_its_headings_by_its_tags() {
    let d = dir("outline", "tagged-headings");
    // "Body text goes here." is set at the body size, so only the tags make it a heading.
    let content = "/P <</MCID 0>> BDC BT /F1 24 Tf 72 700 Td (Chapter One) Tj ET EMC
                   /P <</MCID 1>> BDC BT /F1 12 Tf 72 600 Td (Body text goes here.) Tj ET EMC
                   /P <</MCID 2>> BDC BT /F1 12 Tf 72 500 Td (More body text in the same size.) Tj ET EMC
                   /P <</MCID 3>> BDC BT /F1 12 Tf 72 400 Td (Part A) Tj ET EMC";
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 6 0 R /MarkInfo << /Marked true >> >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".to_string(),
        format!("<< /Length {} >>
stream
{content}
endstream", content.len()),
        "<< /Type /StructTreeRoot /K 7 0 R >>".to_string(),
        "<< /Type /StructElem /S /Document /P 6 0 R /K [8 0 R 9 0 R 10 0 R 11 0 R] >>".to_string(),
        "<< /Type /StructElem /S /H1 /P 7 0 R /Pg 4 0 R /K 0 >>".to_string(),
        "<< /Type /StructElem /S /H2 /P 7 0 R /Pg 4 0 R /K 1 >>".to_string(),
        "<< /Type /StructElem /S /P /P 7 0 R /Pg 4 0 R /K 2 >>".to_string(),
        "<< /Type /StructElem /S /H /P 7 0 R /Pg 4 0 R /K 3 >>".to_string(),
    ];
    let (eng, id, _) = open_bytes(&d, "tagged.pdf", &raw_pdf(&objs));
    let p = preview_headings(&eng, id).unwrap();
    assert_eq!(p.source, "tags");
    let got: Vec<(String, u8)> = p.rows.iter().map(|r| (r.title.clone(), r.level)).collect();
    assert_eq!(
        got,
        [("Chapter One".to_string(), 1), ("Body text goes here.".to_string(), 2), ("Part A".to_string(), 1)]
    );
    let e = go(&eng, id, BookmarkOp::FromHeadings { replace: false });
    assert_eq!(e.count, 3);
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["Chapter One", "Part A"]);
    assert_eq!(titles(&b.items[0].children), ["Body text goes here."], "a level 2 heading nests under the level 1 before it");
}

// ---- a damaged or hostile outline ----

/// A one-page file with the outline items `items` (objects 7..; `/Outlines` is 4, the page 5) and a heading on the page.
fn outline_pdf(root: &str, items: &[String]) -> Vec<u8> {
    outline_pdf_with("", root, items)
}

/// `outline_pdf` with `catalog` more keys in the document catalog (built whole, so the xref stays exact).
fn outline_pdf_with(catalog: &str, root: &str, items: &[String]) -> Vec<u8> {
    let content = "BT /F1 24 Tf 72 700 Td (Chapter One) Tj ET BT /F1 12 Tf 72 600 Td (Plain body text of the document, long enough.) Tj ET BT /F1 12 Tf 72 560 Td (More plain body text in the same size here.) Tj ET";
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [5 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        root.to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 6 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
    ];
    objs.extend(items.iter().cloned());
    raw_pdf(&objs)
}

/// An outline item with `links` (`/Parent`, `/Next`, ...): every item goes to the page (object 5).
fn item_obj(title: &str, links: &str) -> String {
    format!("<< /Title ({title}) /Dest [5 0 R /Fit] {links} >>")
}

/// The object number `key` of object `num` refers to.
fn link_of(eng: &Engine, id: DocId, num: i32, key: &str) -> Option<i32> {
    eng.read_custom(id, |d| Ok(d.new_indirect(num, 0)?.get_dict(key)?.and_then(|o| o.as_indirect().ok()))).unwrap()
}

fn count_key(eng: &Engine, id: DocId, num: i32) -> Option<i64> {
    eng.read_custom(id, |d| Ok(number(&d.new_indirect(num, 0)?.get_dict("Count")?).map(|c| c as i64))).unwrap()
}

/// Every kind of change, tried on `real`: each returns (none hangs), refuses, and none changes the document.
fn assert_every_edit_is_refused(eng: &Engine, id: DocId, real: i32, expect: &str) {
    let before = eng.edit_state(id).unwrap();
    let ops = [
        BookmarkOp::Add { parent: None, index: None, title: "C".into(), page: 0, at: None },
        BookmarkOp::Add { parent: Some(real), index: None, title: "C".into(), page: 0, at: None },
        BookmarkOp::Delete { ids: vec![real] },
        BookmarkOp::Move { id: real, parent: None, index: 0 },
        BookmarkOp::FromHeadings { replace: true },
        BookmarkOp::FromHeadings { replace: false },
        BookmarkOp::Rename { id: real, title: "R".into() },
        BookmarkOp::SetOpen { id: real, open: false },
    ];
    for op in ops {
        match apply(eng, id, rev(eng, id), &op, EditSource::User) {
            Err(EngineError::Invalid(m)) => assert!(m.contains(expect), "{op:?}: {m}"),
            other => panic!("{op:?}: {other:?}"),
        }
    }
    let after = eng.edit_state(id).unwrap();
    assert_eq!((after.rev, after.undo), (before.rev, before.undo), "nothing was changed");
}

#[test]
fn outline_items_shared_by_two_parents_are_met_once_and_left_as_they_are() {
    let d = dir("outline", "shared");
    // A (7) and B (8) both have X (9) as their first child; X has Y (10) after it.
    let (eng, id, _) = open_bytes(
        &d,
        "shared.pdf",
        &outline_pdf(
            "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 4 >>",
            &[
                item_obj("A", "/Parent 4 0 R /Next 8 0 R /First 9 0 R /Last 10 0 R /Count 2"),
                item_obj("B", "/Parent 4 0 R /Prev 7 0 R /First 9 0 R /Last 10 0 R /Count 2"),
                item_obj("X", "/Parent 7 0 R /Next 10 0 R"),
                item_obj("Y", "/Parent 7 0 R /Prev 9 0 R"),
            ],
        ),
    );
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["A", "B"]);
    assert_eq!(titles(&b.items[0].children), ["X", "Y"], "the first parent to reach them has them");
    assert!(b.items[1].children.is_empty(), "an item already met is not listed again");
    assert_eq!(b.count, 4);
    assert!(b.truncated, "the list says something was left out");
    // Relinking either parent would cut the other's list: nothing is changed, and nothing hangs.
    assert_every_edit_is_refused(&eng, id, 7, "more than one place");
}

#[test]
fn a_long_chain_of_items_that_all_point_at_each_other_does_not_hang() {
    let d = dir("outline", "bomb");
    // Item i (from 7) has item i+1 as its first child and as its next one: 2^n paths for a naive walk.
    let n = 40;
    let items: Vec<String> = (0..n)
        .map(|i| {
            let me = 7 + i;
            if i + 1 < n {
                item_obj(&format!("I{i}"), &format!("/Parent 4 0 R /First {} 0 R /Next {} 0 R", me + 1, me + 1))
            } else {
                item_obj(&format!("I{i}"), "/Parent 4 0 R")
            }
        })
        .collect();
    let (eng, id, _) = open_bytes(&d, "bomb.pdf", &outline_pdf("<< /Type /Outlines /First 7 0 R /Last 7 0 R >>", &items));
    let b = list(&eng, id).unwrap();
    assert_eq!(b.count, n as u32, "every item is listed once");
    assert_every_edit_is_refused(&eng, id, 9, "more than one place");
}

#[test]
fn a_cyclic_outline_is_listed_and_left_as_it_is() {
    let d = dir("outline", "cycle");
    // 7 is its own first child and its own next item; 8 is under 7 and points back at it.
    let (eng, id, _) = open_bytes(
        &d,
        "cycle.pdf",
        &outline_pdf(
            "<< /Type /Outlines /First 7 0 R /Last 7 0 R >>",
            &[item_obj("Loop", "/Parent 4 0 R /First 7 0 R /Next 7 0 R"), item_obj("Orphan", "/Parent 7 0 R /Next 7 0 R")],
        ),
    );
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["Loop"]);
    assert!(b.items[0].children.is_empty());
    assert_every_edit_is_refused(&eng, id, 7, "more than one place");
}

#[test]
fn a_chain_that_loops_back_into_itself_is_mended_by_an_edit() {
    let d = dir("outline", "loop-back");
    // A (7) -> B (8) -> A again: the same chain comes back to its start, so nothing is lost by cutting it.
    let (eng, id, _) = open_bytes(
        &d,
        "loopback.pdf",
        &outline_pdf(
            "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>",
            &[item_obj("A", "/Parent 4 0 R /Next 8 0 R"), item_obj("B", "/Parent 4 0 R /Prev 7 0 R /Next 7 0 R")],
        ),
    );
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["A", "B"]);
    go(&eng, id, BookmarkOp::Rename { id: 7, title: "Alpha".into() });
    let c = go(&eng, id, BookmarkOp::Add { parent: None, index: None, title: "C".into(), page: 0, at: None }).id.unwrap();
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["Alpha", "B", "C"]);
    // The cycle is gone: B goes on to the new item and the last one has no /Next.
    assert_eq!(link_of(&eng, id, 8, "Next"), Some(c));
    assert_eq!(link_of(&eng, id, c, "Next"), None);
}

#[test]
fn a_wrong_parent_does_not_misplace_a_move() {
    let d = dir("outline", "wrong-parent-move");
    // A (7), B (8), C (9) are all under the root, but B says its parent is A.
    let (eng, id, _) = open_bytes(
        &d,
        "wp.pdf",
        &outline_pdf(
            "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 3 >>",
            &[
                item_obj("A", "/Parent 4 0 R /Next 8 0 R"),
                item_obj("B", "/Parent 7 0 R /Prev 7 0 R /Next 9 0 R"),
                item_obj("C", "/Parent 4 0 R /Prev 8 0 R"),
            ],
        ),
    );
    go(&eng, id, BookmarkOp::Move { id: 8, parent: Some(9), index: 0 });
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items), ["A", "C"]);
    assert_eq!(titles(&b.items[1].children), ["B"]);
    // The links of the file itself: the root, A, C and B.
    assert_eq!(link_of(&eng, id, 4, "First"), Some(7));
    assert_eq!(link_of(&eng, id, 4, "Last"), Some(9));
    assert_eq!(link_of(&eng, id, 7, "Next"), Some(9));
    assert_eq!(link_of(&eng, id, 7, "First"), None, "A never had B as a child, and has none now");
    assert_eq!(link_of(&eng, id, 9, "Prev"), Some(7));
    assert_eq!(link_of(&eng, id, 9, "First"), Some(8));
    assert_eq!(link_of(&eng, id, 9, "Last"), Some(8));
    assert_eq!(link_of(&eng, id, 8, "Parent"), Some(9));
    assert_eq!(link_of(&eng, id, 8, "Prev"), None);
    assert_eq!(link_of(&eng, id, 8, "Next"), None);
    assert_eq!(count_key(&eng, id, 4), Some(3), "A, C and B (C is open now)");
    assert_eq!(count_key(&eng, id, 9), Some(1));
}

#[test]
fn a_wrong_parent_does_not_misplace_a_delete() {
    let d = dir("outline", "wrong-parent-delete");
    let (eng, id, _) = open_bytes(
        &d,
        "wpd.pdf",
        &outline_pdf(
            "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>",
            &[item_obj("A", "/Parent 4 0 R /Next 8 0 R"), item_obj("B", "/Parent 7 0 R /Prev 7 0 R")],
        ),
    );
    let e = go(&eng, id, BookmarkOp::Delete { ids: vec![8] });
    assert_eq!(e.count, 1);
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["A"]);
    assert_eq!(link_of(&eng, id, 4, "First"), Some(7));
    assert_eq!(link_of(&eng, id, 4, "Last"), Some(7), "the root's last item is A now");
    assert_eq!(link_of(&eng, id, 7, "Next"), None);
    assert_eq!(count_key(&eng, id, 4), Some(1));
}

#[test]
fn a_move_into_a_descendant_is_refused_even_when_the_parents_are_wrong() {
    let d = dir("outline", "wrong-parent-cycle");
    // X (8) is a child of A (7) in the chains, but says its parent is the root.
    let (eng, id, _) = open_bytes(
        &d,
        "wpc.pdf",
        &outline_pdf(
            "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 2 >>",
            &[item_obj("A", "/Parent 4 0 R /First 8 0 R /Last 8 0 R /Count 1"), item_obj("X", "/Parent 4 0 R")],
        ),
    );
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::Move { id: 7, parent: Some(8), index: 0 }, EditSource::User);
    assert!(matches!(r, Err(EngineError::Invalid(_))), "{r:?}");
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["A"]);
}

#[test]
fn an_item_that_is_not_in_the_outline_is_stale() {
    let d = dir("outline", "not-in-outline");
    // 8 is an item object, but nothing links to it.
    let (eng, id, _) = open_bytes(
        &d,
        "nio.pdf",
        &outline_pdf(
            "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 1 >>",
            &[item_obj("A", "/Parent 4 0 R"), item_obj("Lost", "/Parent 4 0 R")],
        ),
    );
    for op in [
        BookmarkOp::Delete { ids: vec![8] },
        BookmarkOp::Move { id: 8, parent: None, index: 0 },
        BookmarkOp::Rename { id: 8, title: "x".into() },
    ] {
        let r = apply(&eng, id, rev(&eng, id), &op, EditSource::User);
        assert!(matches!(r, Err(EngineError::Stale)), "{op:?}: {r:?}");
    }
}

#[test]
fn a_change_that_changes_nothing_makes_no_undo_step() {
    let d = dir("outline", "noop");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 6, chapter_every: 2, ..Default::default() });
    let before = eng.edit_state(id).unwrap();
    let b = list(&eng, id).unwrap();
    let first = b.items[0].clone();
    let ops = [
        BookmarkOp::Rename { id: first.id, title: format!("  {}  ", first.title) },
        BookmarkOp::SetStyle { id: first.id, bold: Some(first.bold), italic: Some(first.italic), color: None, reset_color: None },
        BookmarkOp::Move { id: first.id, parent: None, index: 0 },
        BookmarkOp::Delete { ids: vec![] },
    ];
    for op in ops {
        let e = apply(&eng, id, rev(&eng, id), &op, EditSource::User).unwrap();
        assert_eq!(e.state.rev, before.rev, "{op:?}");
        assert_eq!(e.state.undo, before.undo, "{op:?}");
        assert_eq!(e.state.dirty, before.dirty, "{op:?}");
    }
    // A real change still is one.
    let e = go(&eng, id, BookmarkOp::Rename { id: first.id, title: "Renamed".into() });
    assert_ne!(e.state.rev, before.rev);
}

#[test]
fn a_certified_document_keeps_its_bookmarks() {
    let d = dir("outline", "certified");
    // The catalog has a `/Perms /DocMDP` entry whose `/P` is 1.
    let pdf = outline_pdf_with(
        "/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 1 >> >> ] >> >>",
        "<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 1 >>",
        &[item_obj("A", "/Parent 4 0 R")],
    );
    let (eng, id, _) = open_bytes(&d, "cert.pdf", &pdf);
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::Rename { id: 7, title: "B".into() }, EditSource::User);
    match r {
        Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["A"]);
}

/// `certified` and `signed` of a file whose catalog carries `catalog` and whose objects 7, 8, ... are `objs`.
fn certification_of(name: &str, catalog: &str, objs: &[&str]) -> (Option<i32>, bool) {
    let (certified, signed) = checked_certification_of(name, catalog, objs);
    (certified.unwrap(), signed)
}

/// As [`certification_of`], with `certified`'s own result (an error is an answer here, `signed` still gives one).
fn checked_certification_of(name: &str, catalog: &str, objs: &[&str]) -> (std::result::Result<Option<i32>, EngineError>, bool) {
    let d = dir("outline", name);
    let items: Vec<String> = objs.iter().map(|o| o.to_string()).collect();
    let pdf = outline_pdf_with(catalog, "<< /Type /Outlines /Count 0 >>", &items);
    let (eng, id, _) = open_bytes(&d, "c.pdf", &pdf);
    eng.read_custom(id, |doc| Ok((certified(doc), signed(doc)))).unwrap()
}

/// A signature value whose `/Reference` holds `refs` (raw dictionaries).
fn sig_value(refs: &str) -> String {
    format!("/V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> /Reference [ {refs} ] >>")
}

fn doc_mdp(params: &str) -> String {
    format!("<< /Type /SigRef /TransformMethod /DocMDP {params} >>")
}

#[test]
fn a_signature_field_that_certifies_the_document_is_seen_like_the_catalog_entry() {
    let form = "/AcroForm << /SigFlags 3 /Fields [7 0 R] >>";
    let field = |refs: &str| format!("<< /FT /Sig /T (S) {} >>", sig_value(refs));
    // The catalog's entry: P read, default 2, one that makes no sense is 2 as well.
    let cat = |p: &str| format!("/Perms << /DocMDP << /Type /Sig /Reference [ {} ] >> >>", doc_mdp(p));
    assert_eq!(certification_of("cat-1", &cat("/TransformParams << /P 1 >>"), &[]), (Some(1), true));
    assert_eq!(certification_of("cat-default", &cat(""), &[]), (Some(2), true));
    assert_eq!(certification_of("cat-null", "/Perms << /DocMDP null >>", &[]).0, None);
    // The field's own reference: P 3, no /P (2). A /P that is there and is not 1, 2 or 3 is the strictest (1).
    let p3 = field(&doc_mdp("/TransformParams << /P 3 >>"));
    assert_eq!(certification_of("field-3", form, &[&p3]), (Some(3), true));
    let none = field(&doc_mdp("/TransformParams << /Type /TransformParams >>"));
    assert_eq!(certification_of("field-default", form, &[&none]).0, Some(2));
    assert_eq!(certification_of("field-no-params", form, &[&field(&doc_mdp(""))]).0, Some(2));
    for (name, p) in [("99", "99"), ("0", "0"), ("name", "/One"), ("string", "(1)"), ("fraction", "2.5"), ("array", "[2]")] {
        let wild = field(&doc_mdp(&format!("/TransformParams << /P {p} >>")));
        assert_eq!(certification_of(&format!("field-p-{name}"), form, &[&wild]).0, Some(1), "/P {p}");
    }
    let null_p = field(&doc_mdp("/TransformParams << /P null >>"));
    assert_eq!(certification_of("field-p-null", form, &[&null_p]).0, Some(2));
    let cat_bad = |p: &str| format!("/Perms << /DocMDP << /Type /Sig /Reference [ {} ] >> >>", doc_mdp(&format!("/TransformParams << /P {p} >>")));
    assert_eq!(certification_of("cat-p-0", &cat_bad("0"), &[]).0, Some(1));
    // Several references: the strictest, in whatever order they come.
    let two = field(&format!("{} {}", doc_mdp("/TransformParams << /P 3 >>"), doc_mdp("/TransformParams << /P 1 >>")));
    assert_eq!(certification_of("field-two", form, &[&two]).0, Some(1));
    // A reference that is not a DocMDP certifies nothing.
    let fmdp = field("<< /Type /SigRef /TransformMethod /FieldMDP /TransformParams << /Action /All >> >>");
    assert_eq!(certification_of("field-fieldmdp", form, &[&fmdp]), (None, true));
    // Both exist: the lower P counts.
    let both = format!("{form} {}", cat("/TransformParams << /P 1 >>"));
    assert_eq!(certification_of("both", &both, &[&p3]).0, Some(1));
    // Strictest of the two, either way round: the catalog's 3 over a field's 1 is 1.
    let p1 = field(&doc_mdp("/TransformParams << /P 1 >>"));
    let both = format!("{form} {}", cat("/TransformParams << /P 3 >>"));
    assert_eq!(certification_of("both-field-stricter", &both, &[&p1]).0, Some(1));
    // A signature with a value and no DocMDP is signed, not certified; an empty field is neither.
    let plain = format!("<< /FT /Sig /T (S) /V << /Type /Sig /ByteRange [0 10 20 30] /Contents <00> >> >>");
    assert_eq!(certification_of("plain", form, &[&plain]), (None, true));
    assert_eq!(certification_of("empty", "/AcroForm << /Fields [7 0 R] >>", &["<< /FT /Sig /T (S) >>"]), (None, false));
}

#[test]
fn the_signature_type_and_value_are_found_through_the_field_tree() {
    // /FT on the parent, the value on a kid of a kid.
    let v = sig_value(&doc_mdp("/TransformParams << /P 3 >>"));
    let objs = ["<< /FT /Sig /T (P) /Kids [8 0 R] >>".to_string(), "<< /Parent 7 0 R /Kids [9 0 R] >>".to_string(), format!("<< /Parent 8 0 R /T (K) {v} >>")];
    let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
    assert_eq!(certification_of("tree", "/AcroForm << /Fields [7 0 R] >>", &objs), (Some(3), true));
    // /V on the parent, /FT only on the kid: the kid is a signature with the parent's value.
    let objs = [format!("<< /T (P) {v} /Kids [8 0 R] >>"), "<< /Parent 7 0 R /FT /Sig /T (K) >>".to_string()];
    let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
    assert_eq!(certification_of("inherited-value", "/AcroForm << /Fields [7 0 R] >>", &objs), (Some(3), true));
    // A kid with no value of its own under two parents that each carry one: the DocMDP of the second is not missed,
    // whichever parent is read first.
    let approval = "/V << /Type /Sig /ByteRange [0 10 20 30] /Contents <00> >>";
    let certifying = sig_value(&doc_mdp("/TransformParams << /P 1 >>"));
    for (name, fields) in [("two-parents-a", "[7 0 R 8 0 R]"), ("two-parents-b", "[8 0 R 7 0 R]")] {
        let objs = [
            format!("<< /T (A) {approval} /Kids [9 0 R] >>"),
            format!("<< /T (B) {certifying} /Kids [9 0 R] >>"),
            "<< /FT /Sig /T (K) >>".to_string(),
        ];
        let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
        assert_eq!(certification_of(name, &format!("/AcroForm << /Fields {fields} >>"), &objs), (Some(1), true), "{name}");
    }
    // Not a signature field: a text field's /V is not looked at.
    let text = format!("<< /FT /Tx /T (T) {v} >>");
    assert_eq!(certification_of("text-field", "/AcroForm << /Fields [7 0 R] >>", &[&text]), (None, false));
}

#[test]
fn a_signature_of_a_shape_no_writer_makes_is_read_toward_refusing() {
    let form = "/AcroForm << /Fields [7 0 R] >>";
    let certifying = doc_mdp("/TransformParams << /P 1 >>");
    // /V that is not a dictionary: a value, so signed, and no DocMDP to read.
    for (name, v) in [("string", "(text)"), ("int", "12"), ("name", "/Sig")] {
        let obj = format!("<< /FT /Sig /T (S) /V {v} >>");
        assert_eq!(certification_of(&format!("v-{name}"), form, &[&obj]), (None, true), "/V {v}");
    }
    // /Reference that is one dictionary where an array belongs: read as the one entry it would be.
    let lone = format!("<< /FT /Sig /T (S) /V << /Type /Sig /Contents <00> /Reference {certifying} >> >>");
    assert_eq!(certification_of("reference-dict", form, &[&lone]), (Some(1), true));
    // /Reference that is neither: nothing to read, still signed.
    let junk = "<< /FT /Sig /T (S) /V << /Type /Sig /Contents <00> /Reference (x) >> >>";
    assert_eq!(certification_of("reference-string", form, &[junk]), (None, true));
    // More references than are read: the strictest answer, not the first 16.
    let many = format!("<< /FT /Sig /T (S) /V << /Type /Sig /Contents <00> /Reference [ {} {certifying} ] >> >>", "<< /TransformMethod /FieldMDP >> ".repeat(16));
    assert_eq!(certification_of("reference-many", form, &[&many]), (Some(1), true));
}

/// A form whose first field (object 7) is a certifying signature (`/P 1`), then `n` references to one plain text field
/// (object 8), and `kids` of those under a parent (object 9) listed last.
fn crowded_form(name: &str, n: usize, kids: usize) -> (std::result::Result<Option<i32>, EngineError>, bool) {
    let certifying = format!("<< /FT /Sig /T (S) {} >>", sig_value(&doc_mdp("/TransformParams << /P 1 >>")));
    let parent = format!("<< /T (P) /Kids [{}] >>", "8 0 R ".repeat(kids));
    let fields = format!("[7 0 R {}{}]", "8 0 R ".repeat(n), if kids > 0 { "9 0 R" } else { "" });
    checked_certification_of(name, &format!("/AcroForm << /Fields {fields} >>"), &[&certifying, "<< /FT /Tx /T (t) >>", &parent])
}

#[test]
fn a_walk_that_hits_its_caps_refuses_instead_of_answering_not_certified() {
    // Exactly at the cap: fine, and the signature is found.
    assert_eq!(crowded_form("cap-exact", 19_999, 0).0.unwrap(), Some(1));
    // One field past the cap in /Fields: an error, and signed (nothing else says so).
    let (c, signed) = crowded_form("cap-fields", 20_000, 0);
    assert!(matches!(&c, Err(EngineError::Invalid(m)) if m.contains("certified")), "{c:?}");
    assert!(signed);
    // The nodes of the walk add up past the cap: the certifying field at the front is the last one popped.
    let (c, signed) = crowded_form("cap-budget", 1, 19_998);
    assert!(matches!(&c, Err(EngineError::Invalid(m)) if m.contains("certified")), "{c:?}");
    assert!(signed);
    // A caller that refuses on a certified document refuses on that too.
    let d = dir("outline", "cap-caller");
    let certifying = format!("<< /FT /Sig /T (S) {} >>", sig_value(&doc_mdp("/TransformParams << /P 1 >>")));
    let parent = format!("<< /T (P) /Kids [{}] >>", "8 0 R ".repeat(19_999));
    let pdf = outline_pdf_with("/AcroForm << /Fields [7 0 R 9 0 R] >>", "<< /Type /Outlines /Count 0 >>", &[certifying, "<< /FT /Tx /T (t) >>".into(), parent]);
    let (eng, id, _) = open_bytes(&d, "big.pdf", &pdf);
    match apply(&eng, id, rev(&eng, id), &BookmarkOp::Rename { id: 7, title: "B".into() }, EditSource::User) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_field_shared_by_parents_that_hold_their_value_inline_is_read_under_each() {
    // Both parents are direct (no object number), so only what each one certifies tells their states apart.
    let approval = "/V << /Type /Sig /ByteRange [0 10 20 30] /Contents <00> >>";
    let certifying = sig_value(&doc_mdp("/TransformParams << /P 1 >>"));
    for (name, a, b) in [("inline-ab", approval, certifying.as_str()), ("inline-ba", certifying.as_str(), approval)] {
        let form = format!("/AcroForm << /Fields [ << /T (A) {a} /Kids [7 0 R] >> << /T (B) {b} /Kids [7 0 R] >> ] >>");
        assert_eq!(certification_of(name, &form, &["<< /FT /Sig /T (K) >>"]), (Some(1), true), "{name}");
    }
}

#[test]
fn a_direct_value_that_loops_back_to_its_field_ends_the_walk() {
    // 7 -> a direct kid with a direct /V -> 7: every lap is the same state, so it is read once (and it is an answer,
    // not the error a walk that never ran out of new states would end in).
    let v = sig_value(&doc_mdp("/TransformParams << /P 3 >>"));
    let looped = format!("<< /FT /Sig /T (A) /Kids [ << /T (B) {v} /Kids [7 0 R] >> ] >>");
    assert_eq!(certification_of("direct-loop", "/AcroForm << /Fields [7 0 R] >>", &[&looped]), (Some(3), true));
}

#[test]
fn a_hostile_field_tree_is_walked_once_and_reads_nothing_it_should_not() {
    let v = sig_value(&doc_mdp("/TransformParams << /P 1 >>"));
    // 7 -> 8 -> 7 -> ..., a field that is its own kid, a kid that is not a dictionary, and a valued signature after them.
    let objs = [
        "<< /FT /Sig /T (A) /Parent 8 0 R /Kids [8 0 R 7 0 R 42 0 R 5 /Name (str)] >>".to_string(),
        "<< /Parent 7 0 R /Kids [7 0 R 8 0 R 9 0 R] >>".to_string(),
        format!("<< /Parent 8 0 R /T (B) {v} >>"),
    ];
    let objs: Vec<&str> = objs.iter().map(String::as_str).collect();
    assert_eq!(certification_of("loop", "/AcroForm << /Fields [7 0 R 7 0 R 8 0 R] >>", &objs), (Some(1), true));
    let looped = ["<< /FT /Sig /Kids [8 0 R] >>", "<< /Kids [7 0 R] >>"];
    assert_eq!(certification_of("loop-only", "/AcroForm << /Fields [7 0 R] >>", &looped), (None, false));
    // /Fields that is not an array, and an AcroForm that is not a dictionary.
    assert_eq!(certification_of("fields-name", "/AcroForm << /Fields /Nope >>", &[]).0, None);
    assert_eq!(certification_of("acroform-int", "/AcroForm 12", &[]).0, None);
}

#[test]
fn bookmarks_follow_their_pages_through_insert_move_and_delete() {
    use crate::engine::edit::EditOp;
    let d = dir("outline", "page-ops");
    let (eng, id, _) = open_fixture(&d, &Fixture { pages: 6, chapter_every: 2, ..Default::default() });
    let pages = |eng: &Engine| -> Vec<Option<u32>> {
        list(eng, id)
            .unwrap()
            .items
            .iter()
            .map(|b| match b.dest {
                Dest::Page { page, .. } => Some(page),
                _ => None,
            })
            .collect()
    };
    assert_eq!(pages(&eng), [Some(0), Some(2), Some(4)]);
    eng.apply(id, EditOp::InsertBlank { at: 0, like: 0 }).unwrap();
    assert_eq!(pages(&eng), [Some(1), Some(3), Some(5)], "a page inserted before them moves them down");
    eng.apply(id, EditOp::Move { pages: vec![0], to: 7 }).unwrap();
    assert_eq!(pages(&eng), [Some(0), Some(2), Some(4)], "and moved to the end moves them back");
    eng.apply(id, EditOp::Move { pages: vec![4, 5], to: 0 }).unwrap();
    assert_eq!(pages(&eng), [Some(2), Some(4), Some(0)], "a bookmark goes with its page");
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    assert_eq!(pages(&eng), [Some(1), Some(3), Some(0)]);
    // The page of a bookmark is deleted: the bookmark stays and says why it goes nowhere.
    eng.apply(id, EditOp::Delete { pages: vec![1] }).unwrap();
    let b = list(&eng, id).unwrap();
    assert_eq!(titles(&b.items).len(), 3);
    assert!(matches!(&b.items[0].dest, Dest::Broken { .. }), "{:?}", b.items[0].dest);
    // The rest still edits and nests.
    go(&eng, id, BookmarkOp::Rename { id: b.items[0].id, title: "Gone".into() });
}

/// An outline in a file that carries a signature field with a value (its bytes are what the signature covers).
fn signed_outline() -> Vec<u8> {
    outline_pdf_with(
        "/AcroForm << /SigFlags 3 /Fields [9 0 R] >>",
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>",
        &[
            item_obj("A", "/Parent 4 0 R /Next 8 0 R"),
            item_obj("B", "/Parent 4 0 R /Prev 7 0 R"),
            "<< /FT /Sig /T (Signature1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>"
                .to_string(),
        ],
    )
}

#[test]
fn deleting_bookmarks_of_a_signed_document_keeps_the_signed_bytes() {
    for replace_headings in [false, true] {
        let d = dir("outline", if replace_headings { "signed-headings" } else { "signed-delete" });
        let (eng, id, path) = open_bytes(&d, "signed.pdf", &signed_outline());
        assert!(eng.read_custom(id, |doc| Ok(signed(doc))).unwrap(), "the signature is seen");
        let original = std::fs::read(&path).unwrap();
        if replace_headings {
            go(&eng, id, BookmarkOp::FromHeadings { replace: true });
        } else {
            go(&eng, id, BookmarkOp::Delete { ids: vec![8] });
        }
        eng.save(id, None).unwrap();
        let saved = std::fs::read(&path).unwrap();
        // An incremental update: every byte the signature covers is where it was.
        assert!(saved.len() > original.len() && saved.starts_with(&original), "replace_headings={replace_headings}: the file was rewritten");
        // And the change is there when it is read again.
        let eng2 = Engine::new(2, 64 << 20);
        let id2 = eng2.open(&path).unwrap().doc;
        let t = titles(&list(&eng2, id2).unwrap().items);
        assert_eq!(t, if replace_headings { vec!["Chapter One".to_string()] } else { vec!["A".to_string()] });
    }
}

#[test]
fn an_unsigned_document_is_still_rewritten_without_what_was_deleted() {
    let d = dir("outline", "unsigned-delete");
    let pdf = outline_pdf(
        "<< /Type /Outlines /First 7 0 R /Last 8 0 R /Count 2 >>",
        &[item_obj("A", "/Parent 4 0 R /Next 8 0 R"), item_obj("Secret title", "/Parent 4 0 R /Prev 7 0 R")],
    );
    let (eng, id, path) = open_bytes(&d, "plain.pdf", &pdf);
    assert!(!eng.read_custom(id, |doc| Ok(signed(doc))).unwrap());
    go(&eng, id, BookmarkOp::Delete { ids: vec![8] });
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("Secret title"), "a deleted bookmark's title does not stay in the file");
}

/// A signature whose `/Reference` is `refs`, as an object that the reads below can be made to fail on.
fn sig_object(refs: &str) -> PdfObject {
    let pdf = outline_pdf_with(&format!("/Perms << /DocMDP << /Type /Sig /Reference {refs} >> >>"), "<< /Type /Outlines /Count 0 >>", &[]);
    let doc = PdfDocument::from_bytes(&pdf).unwrap();
    doc.catalog().unwrap().get_dict("Perms").unwrap().unwrap().get_dict("DocMDP").unwrap().unwrap()
}

#[test]
fn a_signature_reference_that_cannot_be_read_counts_as_certifying() {
    // MuPDF repairs a damaged file on its own, so a file cannot make a read fail: the reads are made to.
    let sig = sig_object("[ << /TransformMethod /DocMDP /TransformParams << /P 3 >> >> ]");
    let ok = |o: &PdfObject, k: &str| o.get_dict(k);
    assert_eq!(reference_p_with(&sig, &ok), Some(3));
    for failing in ["Reference", "TransformMethod", "TransformParams", "P"] {
        let get = |o: &PdfObject, k: &str| if k == failing { Err(mupdf::Error::InvalidPdfDocument) } else { o.get_dict(k) };
        assert_eq!(reference_p_with(&sig, &get), Some(1), "{failing} unreadable");
    }
    // An entry that is not a DocMDP is not made strict by being unreadable elsewhere: only what is read counts.
    let other = sig_object("[ << /TransformMethod /FieldMDP /TransformParams << /P 3 >> >> ]");
    let get = |o: &PdfObject, k: &str| if k == "TransformParams" { Err(mupdf::Error::InvalidPdfDocument) } else { o.get_dict(k) };
    assert_eq!(reference_p_with(&other, &get), None);
}
