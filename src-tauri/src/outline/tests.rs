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
    let content = "BT /F1 24 Tf 72 700 Td (Chapter One) Tj ET BT /F1 12 Tf 72 600 Td (Plain body text of the document, long enough.) Tj ET BT /F1 12 Tf 72 560 Td (More plain body text in the same size here.) Tj ET";
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>".to_string(),
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

#[test]
fn outline_items_shared_by_two_parents_are_met_once_and_edits_return() {
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
    assert_eq!(titles(&b.items[0].children), ["X", "Y"]);
    assert!(b.items[1].children.is_empty(), "an item already met is not listed again");
    assert_eq!(b.count, 4);
    go(&eng, id, BookmarkOp::Add { parent: None, index: None, title: "C".into(), page: 0, at: None });
    go(&eng, id, BookmarkOp::Delete { ids: vec![8] });
    go(&eng, id, BookmarkOp::Move { id: 9, parent: None, index: 0 });
    go(&eng, id, BookmarkOp::FromHeadings { replace: true });
    let after = list(&eng, id).unwrap();
    assert_eq!(titles(&after.items), ["Chapter One"]);
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
    go(&eng, id, BookmarkOp::Add { parent: Some(7), index: None, title: "New".into(), page: 0, at: None });
    go(&eng, id, BookmarkOp::Move { id: 8, parent: Some(20), index: 0 });
    go(&eng, id, BookmarkOp::Delete { ids: vec![9, 30] });
    go(&eng, id, BookmarkOp::FromHeadings { replace: false });
    assert!(list(&eng, id).unwrap().count > 0);
}

#[test]
fn a_cyclic_outline_is_listed_and_edited() {
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
    go(&eng, id, BookmarkOp::Rename { id: 7, title: "Looped".into() });
    go(&eng, id, BookmarkOp::Add { parent: Some(7), index: None, title: "Inside".into(), page: 0, at: None });
    let e = go(&eng, id, BookmarkOp::Delete { ids: vec![7] });
    assert_eq!(e.count, 2, "the item and the one made under it");
    assert!(list(&eng, id).unwrap().items.is_empty());
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
    let pdf = outline_pdf("<< /Type /Outlines /First 7 0 R /Last 7 0 R /Count 1 >>", &[item_obj("A", "/Parent 4 0 R")]);
    // The catalog gets a `/Perms /DocMDP` entry whose `/P` is 1; the xref offsets move and MuPDF rebuilds them.
    let text = String::from_utf8_lossy(&pdf).replace(
        "/Outlines 4 0 R >>",
        "/Outlines 4 0 R /Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 1 >> >> ] >> >> >>",
    );
    let (eng, id, _) = open_bytes(&d, "cert.pdf", text.as_bytes());
    let r = apply(&eng, id, rev(&eng, id), &BookmarkOp::Rename { id: 7, title: "B".into() }, EditSource::User);
    match r {
        Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(titles(&list(&eng, id).unwrap().items), ["A"]);
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
