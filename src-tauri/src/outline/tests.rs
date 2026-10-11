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
    apply(eng, id, rev(eng, id), &op, EditSource::User).unwrap()
}

fn titles(items: &[Bookmark]) -> Vec<String> {
    items.iter().map(|b| b.title.clone()).collect()
}

/// `/Count` of the outline root and of every item, from the file's own objects.
fn counts(eng: &Engine, id: DocId) -> Vec<(String, i64)> {
    eng.read_custom(id, |d| {
        fn walk(parent: &PdfObject, out: &mut Vec<(String, i64)>) {
            for k in kids(parent).unwrap() {
                let c = number(&k.get_dict("Count").unwrap()).unwrap_or(0.0) as i64;
                out.push((string_of(k.get_dict("Title").unwrap()), c));
                walk(&k, out);
            }
        }
        let r = root(d)?.unwrap();
        let mut out = vec![("<root>".to_string(), number(&r.get_dict("Count")?).unwrap_or(0.0) as i64)];
        walk(&r, &mut out);
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
        let mut it = item(d, first)?;
        it.dict_put("Dest", d.new_object_from_str("(gone)")?)?;
        Ok(Vec::new())
    })
    .unwrap();
    match &list(&eng, id).unwrap().items[0].dest {
        Dest::Broken { what } => assert!(what.contains("gone"), "{what}"),
        other => panic!("{other:?}"),
    }
}
