//! Text editing through the edit lane: round trips through save, fallback fonts, undo, reflow.

use super::*;
use crate::engine::Engine;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join("textedit").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-page PDF: `fonts` are `(resource name, font dict)`, `extra` more objects (numbered from 6).
fn pdf(content: &str, fonts: &[(&str, &str)], extra: &[&str]) -> Vec<u8> {
    let font_objs: Vec<String> = fonts.iter().map(|(_, d)| d.to_string()).collect();
    let first_font = 5 + extra.len() + 1;
    let res: String = fonts.iter().enumerate().map(|(i, (n, _))| format!("/{n} {} 0 R ", first_font + i)).collect();
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << {res}>> >> /Contents 4 0 R >>").into_bytes(),
        stream(content.as_bytes()),
        b"<< >>".to_vec(),
    ];
    objs.extend(extra.iter().map(|e| e.as_bytes().to_vec()));
    objs.extend(font_objs.into_iter().map(String::into_bytes));
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

fn stream(data: &[u8]) -> Vec<u8> {
    let mut s = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nendstream");
    s
}

const HELV: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

fn open(d: &Path, bytes: &[u8]) -> (Arc<Engine>, DocId, String) {
    let p = d.join("a.pdf");
    std::fs::write(&p, bytes).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

/// The page's text as MuPDF extracts it, lines joined with spaces.
fn text(eng: &Engine, id: DocId) -> String {
    eng.page_text(id, 0).unwrap().iter().map(|l| l.t.trim().to_string()).collect::<Vec<_>>().join(" ")
}

fn rev(eng: &Engine, id: DocId) -> u32 {
    eng.edit_state(id).unwrap().rev
}

fn set_text(eng: &Engine, id: DocId, index: u32, t: &str) -> TextEdited {
    let change = BlockChange { text: Some(t.into()), ..Default::default() };
    edit(eng, id, rev(eng, id), 0, index, &change, EditSource::User).unwrap()
}

const TWO_BLOCKS: &str = "BT /F1 24 Tf 72 700 Td (Quarterly report) Tj ET\n\
                          BT /F1 12 Tf 72 640 Td 14 TL (The first line of the body) Tj T* (and its second line.) Tj ET";

#[test]
fn lists_blocks_with_style() {
    let d = dir("list");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let tb = blocks(&eng, id, 0).unwrap();
    assert_eq!(tb.blocks.len(), 2, "{:?}", tb.blocks);
    let h = &tb.blocks[0];
    assert_eq!((h.text.as_str(), h.font.as_str(), h.size.round(), h.editable), ("Quarterly report", "Helvetica", 24.0, true));
    let body = &tb.blocks[1];
    assert_eq!(body.text, "The first line of the body and its second line.");
    assert_eq!(body.lines.len(), 2);
    // Page space: y grows downwards, the heading is above the body.
    assert!(h.rect[1] < body.rect[1] && (h.rect[0] - 72.0).abs() < 1.0, "{:?}", h.rect);
    assert!((792.0 - 700.0 - h.rect[3]).abs() < 8.0, "baseline near the box bottom: {:?}", h.rect);
}

#[test]
fn replace_round_trips_through_save_and_reopen() {
    let d = dir("roundtrip");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let r = set_text(&eng, id, 0, "Annual summary");
    assert!(r.fallback.is_none());
    assert_eq!(r.state.undo.as_deref(), Some("Edit text on p. 1"));
    let t = text(&eng, id);
    assert!(t.contains("Annual summary") && !t.contains("Quarterly"), "{t}");
    assert!(t.contains("The first line of the body"), "the other block stays: {t}");
    eng.save(id, None).unwrap();
    eng.close(id);

    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let t = text(&eng2, id2);
    assert!(t.starts_with("Annual summary"), "{t}");
    assert!(!t.contains("Quarterly"), "the old text is gone: {t}");
    let tb = blocks(&eng2, id2, 0).unwrap();
    assert_eq!(tb.blocks[0].text, "Annual summary");
    assert_eq!(tb.blocks[0].size.round(), 24.0);
    // And the bytes really have no old string.
    let bytes = std::fs::read(&path).unwrap();
    let doc = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let c = doc.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap();
    let data = if c.is_array().unwrap() {
        (0..c.len().unwrap() as i32).flat_map(|i| c.get_array(i).unwrap().unwrap().read_stream().unwrap()).collect()
    } else {
        c.read_stream().unwrap()
    };
    assert!(!data.windows(9).any(|w| w == b"Quarterly"));
}

#[test]
fn undo_restores_the_original_text() {
    let d = dir("undo");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let before = text(&eng, id);
    set_text(&eng, id, 1, "Replaced body.");
    assert!(text(&eng, id).contains("Replaced body."));
    let s = eng.undo(id).unwrap();
    assert!(!s.dirty);
    assert_eq!(text(&eng, id), before);
    eng.redo(id).unwrap();
    assert!(text(&eng, id).contains("Replaced body."));
}

#[test]
fn reflows_to_the_block_width() {
    let d = dir("reflow");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let old = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    let long = "This much longer paragraph replaces the body and has to wrap over several lines of the same width as before.";
    set_text(&eng, id, 1, long);
    let tb = blocks(&eng, id, 0).unwrap();
    let b = &tb.blocks[1];
    assert_eq!(b.text, long);
    assert!(b.lines.len() >= 4, "{} lines", b.lines.len());
    let width = old.rect[2] - old.rect[0];
    for l in &b.lines {
        assert!(l[2] - l[0] <= width + 1.0, "line {l:?} wider than {width}");
        assert!((l[0] - old.rect[0]).abs() < 1.0, "left aligned");
    }
    // Same line spacing as before (14 pt).
    assert!(((b.lines[1][1] - b.lines[0][1]) - 14.0).abs() < 0.5);
    let joined: String = eng.page_text(id, 0).unwrap().iter().skip(1).map(|l| l.t.trim().to_string()).collect::<Vec<_>>().join(" ");
    assert_eq!(joined, long);
}

#[test]
fn subset_font_falls_back_for_missing_glyphs() {
    let d = dir("subset");
    // A subset of Helvetica with glyphs only for the letters of "Hello" (and the space).
    let mut widths = vec!["0"; 96];
    for c in [b' ', b'H', b'e', b'l', b'o'] {
        widths[(c - 32) as usize] = "556";
    }
    let font = format!(
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+Helvetica /FirstChar 32 /LastChar 127 /Widths [{}] /Encoding /WinAnsiEncoding >>",
        widths.join(" ")
    );
    let (eng, id, path) = open(&d, &pdf("BT /F1 18 Tf 72 700 Td (Hello) Tj ET", &[("F1", &font)], &[]));
    // Letters the subset has: kept in the font.
    assert!(set_text(&eng, id, 0, "Hole").fallback.is_none());
    // A letter it lacks: a standard font, and the result says why.
    let r = set_text(&eng, id, 0, "Zebra");
    let fb = r.fallback.expect("fallback");
    assert_eq!((fb.font.as_str(), fb.embedded), ("Helvetica", false));
    assert!(fb.reason.contains("subset") && fb.reason.contains("'Z'"), "{}", fb.reason);
    assert_eq!(text(&eng, id), "Zebra");
    // Beyond WinAnsi: an embedded face.
    let r = set_text(&eng, id, 0, "Ωμέγα Привет");
    assert!(r.fallback.as_ref().is_some_and(|f| f.embedded), "{:?}", r.fallback);
    assert_eq!(text(&eng, id), "Ωμέγα Привет");
    eng.save(id, None).unwrap();
    eng.close(id);
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert_eq!(text(&eng2, id2), "Ωμέγα Привет");
}

#[test]
fn font_without_a_space_glyph_places_words_apart() {
    let d = dir("nospace");
    // A TeX-like font: no code shows a space.
    let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding << /Type /Encoding /Differences [32 /visiblespace] >> >>";
    let (eng, id, _) = open(&d, &pdf("BT /F1 14 Tf 72 700 Td (Word) Tj ET", &[("F1", font)], &[]));
    let r = set_text(&eng, id, 0, "Two words here");
    assert!(r.fallback.is_none(), "{:?}", r.fallback);
    assert_eq!(text(&eng, id), "Two words here");
}

#[test]
fn size_colour_alignment_move_and_resize() {
    let d = dir("format");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let b0 = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    let ch = BlockChange { size: Some(10.0), color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    let r = edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    assert_eq!(r.state.undo.as_deref(), Some("Format text on p. 1"));
    let b = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert_eq!((b.size.round(), b.color), (10.0, [1.0, 0.0, 0.0]));
    assert_eq!(b.text, b0.text);

    let ch = BlockChange { offset: Some([50.0, 100.0]), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    let m = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert!((m.rect[0] - b.rect[0] - 50.0).abs() < 0.5 && (m.rect[1] - b.rect[1] - 100.0).abs() < 0.5, "{:?} -> {:?}", b.rect, m.rect);

    let ch = BlockChange { width: Some(80.0), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    let w = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert!(w.lines.len() > m.lines.len());
    assert!(w.lines.iter().all(|l| l[2] - l[0] <= 80.5), "{:?}", w.lines);
    assert_eq!(w.text, b0.text);

    let ch = BlockChange { align: Some(Align::Right), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    let r = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert_eq!(r.align, Align::Right);
    let right = r.lines[0][2];
    assert!(r.lines.iter().all(|l| (l[2] - right).abs() < 0.5), "{:?}", r.lines);

    // Every change was its own step.
    for _ in 0..4 {
        eng.undo(id).unwrap();
    }
    assert!(!eng.edit_state(id).unwrap().dirty);
}

#[test]
fn centred_text_stays_centred() {
    let d = dir("centre");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let ch = BlockChange { align: Some(Align::Center), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    let b = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    // Read back as centred (the detector sees the lines share a centre).
    assert_eq!(b.align, Align::Center);
    let centre = (b.rect[0] + b.rect[2]) / 2.0;
    set_text(&eng, id, 1, "Short");
    let n = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert!(((n.rect[0] + n.rect[2]) / 2.0 - centre).abs() < 1.0, "{:?} vs centre {centre}", n.rect);
}

#[test]
fn stale_rev_is_refused() {
    let d = dir("stale");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let r0 = rev(&eng, id);
    set_text(&eng, id, 0, "New");
    let ch = BlockChange { text: Some("Again".into()), ..Default::default() };
    assert!(matches!(edit(&eng, id, r0, 0, 0, &ch, EditSource::User), Err(EngineError::Stale)));
}

#[test]
fn reports_what_cannot_be_edited() {
    let d = dir("issues");
    let type3 = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
                 /CharProcs << /a 6 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] >>";
    let cid = "<< /Type /Font /Subtype /Type0 /BaseFont /Mystery /Encoding /Identity-H \
               /DescendantFonts [<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Mystery /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 500 >>] >>";
    let glyph = "<< /Length 28 >>\nstream\n1000 0 d0 0 0 1000 1000 re f\nendstream";
    let mut content = String::from(
        "BT /F2 20 Tf 72 700 Td (aaa) Tj ET\nBT /F3 20 Tf 72 600 Td <00410042> Tj ET\nBT /F1 12 Tf 3 Tr 72 500 Td (hidden ocr) Tj ET\n",
    );
    let (eng, id, _) = open(&d, &pdf(&content, &[("F1", HELV), ("F2", type3), ("F3", cid)], &[glyph]));
    let tb = blocks(&eng, id, 0).unwrap();
    let issues: Vec<Option<Issue>> = tb.blocks.iter().map(|b| b.issue).collect();
    assert_eq!(issues, vec![Some(Issue::Type3), Some(Issue::NoUnicode), Some(Issue::Invisible)], "{:?}", tb.blocks);
    assert!(tb.blocks.iter().all(|b| !b.editable && b.reason.is_some()));
    let ch = BlockChange { text: Some("x".into()), ..Default::default() };
    assert!(matches!(edit(&eng, id, rev(&eng, id), 0, 0, &ch, EditSource::User), Err(EngineError::Invalid(_))));

    // Text converted to outlines: many glyph-sized filled curves, no text.
    content.clear();
    for i in 0..120 {
        let (x, y) = (50 + (i % 40) * 12, 700 - (i / 40) * 20);
        content.push_str(&format!("{x} {y} m {} {} {} {} {} {y} c f\n", x + 2, y + 10, x + 8, y + 10, x + 10));
    }
    let d = dir("outlines");
    let (eng, id, _) = open(&d, &pdf(&content, &[("F1", HELV)], &[]));
    let tb = blocks(&eng, id, 0).unwrap();
    assert!(tb.blocks.is_empty());
    assert_eq!(tb.areas.iter().map(|a| a.issue).collect::<Vec<_>>(), vec![Issue::Outlines]);
}
