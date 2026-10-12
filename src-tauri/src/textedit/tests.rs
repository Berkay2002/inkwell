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
    pdf_in(content, fonts, extra, "")
}

/// [`pdf`] with `catalog` entries added to the document catalog.
fn pdf_in(content: &str, fonts: &[(&str, &str)], extra: &[&str], catalog: &str) -> Vec<u8> {
    let font_objs: Vec<String> = fonts.iter().map(|(_, d)| d.to_string()).collect();
    let first_font = 5 + extra.len() + 1;
    let res: String = fonts.iter().enumerate().map(|(i, (n, _))| format!("/{n} {} 0 R ", first_font + i)).collect();
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
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
fn a_block_after_other_text_in_its_object_reopens_it_first() {
    let d = dir("split");
    let content = "BT /F1 12 Tf 72 700 Td (Keep this) Tj 0 -60 Td (Edit me) Tj ET";
    let (eng, id, path) = open(&d, &pdf(content, &[("F1", HELV)], &[]));
    assert_eq!(blocks(&eng, id, 0).unwrap().blocks.len(), 2);
    set_text(&eng, id, 1, "Edited");
    assert_eq!(text(&eng, id), "Keep this Edited");
    eng.save(id, None).unwrap();
    let doc = mupdf::pdf::PdfDocument::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
    let data = doc.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap().read_stream().unwrap();
    let ops = content::parse(&data);
    let names: Vec<&[u8]> = ops.iter().map(|o| o.op.as_slice()).collect();
    // After the inserted block: ET … Q, then BT before any text state operator.
    let q = names.iter().rposition(|n| *n == b"Q").unwrap();
    let bt = q + names[q..].iter().position(|n| *n == b"BT").unwrap();
    assert!(names[q..bt].iter().all(|n| !matches!(*n, b"Tf" | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Ts" | b"Tr")), "{names:?}");
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

const HELV_BOLD: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>";

/// The rect of the first block whose text is `t`.
fn rect_of(eng: &Engine, id: DocId, t: &str) -> [f32; 4] {
    let tb = blocks(eng, id, 0).unwrap();
    tb.blocks.iter().find(|b| b.text == t).unwrap_or_else(|| panic!("no block {t:?} in {:?}", tb.blocks)).rect
}

fn index_of(eng: &Engine, id: DocId, t: &str) -> u32 {
    blocks(eng, id, 0).unwrap().blocks.iter().find(|b| b.text == t).unwrap().index
}

fn same(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

#[test]
fn runs_after_an_edited_run_keep_their_place() {
    let d = dir("mixed");
    // Three blocks in one text object (far enough apart on the line not to be one block).
    let content = "BT /F1 12 Tf 72 700 Td (Hello) Tj 80 0 Td /F2 12 Tf (bold) Tj 80 0 Td /F1 12 Tf (world) Tj ET";
    let (eng, id, _) = open(&d, &pdf(content, &[("F1", HELV), ("F2", HELV_BOLD)], &[]));
    let (bold, world) = (rect_of(&eng, id, "bold"), rect_of(&eng, id, "world"));
    assert!(bold[0] > 100.0, "{bold:?}");
    set_text(&eng, id, index_of(&eng, id, "Hello"), "Hi");
    assert!(same(rect_of(&eng, id, "bold"), bold), "bold moved: {:?} -> {:?}", bold, rect_of(&eng, id, "bold"));
    assert!(same(rect_of(&eng, id, "world"), world));
    // Edited after text in its object (the object is split there): the rest stays too.
    set_text(&eng, id, index_of(&eng, id, "bold"), "BOLD");
    assert!(same(rect_of(&eng, id, "world"), world), "world moved: {:?} -> {:?}", world, rect_of(&eng, id, "world"));
    assert!((rect_of(&eng, id, "BOLD")[0] - bold[0]).abs() < 0.01);
}

#[test]
fn removed_quote_operators_keep_their_line_moves() {
    let d = dir("quotes");
    let fonts = [("F1", HELV), ("F2", HELV_BOLD)];
    // ' as the first operator of its object, then in the middle (the object is split), then ".
    let cases = [
        "BT /F1 12 Tf 30 TL 72 730 Td /F2 12 Tf (edit me) ' /F1 12 Tf (keep two) ' ET",
        "BT /F1 12 Tf 30 TL 72 700 Td (keep one) Tj /F2 12 Tf (edit me) ' /F1 12 Tf (keep two) ' ET",
        "BT /F1 12 Tf 30 TL 72 700 Td (keep one) Tj /F2 12 Tf 3 1 (edit me) \" /F1 12 Tf (keep two) ' ET",
    ];
    for (n, content) in cases.iter().enumerate() {
        let d = d.join(n.to_string());
        std::fs::create_dir_all(&d).unwrap();
        let (eng, id, _) = open(&d, &pdf(content, &fonts, &[]));
        let keep = rect_of(&eng, id, "keep two");
        let edit = rect_of(&eng, id, "edit me");
        set_text(&eng, id, index_of(&eng, id, "edit me"), "edited");
        assert!(same(rect_of(&eng, id, "keep two"), keep), "case {n}: {:?} -> {:?}", keep, rect_of(&eng, id, "keep two"));
        assert!((rect_of(&eng, id, "edited")[1] - edit[1]).abs() < 0.5, "case {n}: the edited line stays on its line");
    }
}

#[test]
fn saved_file_has_no_trace_of_the_old_text() {
    let d = dir("gone");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let before = std::fs::read(&path).unwrap();
    assert!(before.windows(16).any(|w| w == b"Quarterly report"));
    set_text(&eng, id, 0, "Annual summary");
    eng.save(id, None).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.windows(9).any(|w| w == b"Quarterly"), "the old text is still in the saved bytes");
    // A full rewrite, not the original plus an update.
    assert_eq!(bytes.windows(9).filter(|w| *w == b"startxref").count(), 1);
    assert!(text(&eng, id).starts_with("Annual summary"));
}

#[test]
fn text_on_a_hidden_layer_is_not_editable() {
    let d = dir("hidden");
    let content = "BT /F1 12 Tf 72 700 Td (Visible) Tj ET\n/OC /L1 BDC BT /F1 12 Tf 72 600 Td (Secret) Tj ET EMC\n\
                   /OC /L2 BDC BT /F1 12 Tf 72 500 Td (Shown layer) Tj ET EMC";
    let mut bytes = pdf(content, &[("F1", HELV)], &["<< /Type /OCG /Name (Hidden) >>", "<< /Type /OCG /Name (Shown) >>"]);
    // Wire the layers in: page /Properties and the catalog's /OCProperties (L1 off).
    let doc = mupdf::pdf::PdfDocument::from_bytes(&bytes).unwrap();
    let mut res = doc.find_page(0).unwrap().get_dict("Resources").unwrap().unwrap();
    res.dict_put("Properties", doc.new_object_from_str("<< /L1 6 0 R /L2 7 0 R >>").unwrap()).unwrap();
    doc.catalog()
        .unwrap()
        .dict_put("OCProperties", doc.new_object_from_str("<< /OCGs [6 0 R 7 0 R] /D << /OFF [6 0 R] >> >>").unwrap())
        .unwrap();
    bytes.clear();
    doc.write_to(&mut bytes).unwrap();
    let (eng, id, _) = open(&d, &bytes);
    let tb = blocks(&eng, id, 0).unwrap();
    let issue = |t: &str| tb.blocks.iter().find(|b| b.text == t).map(|b| (b.editable, b.issue));
    assert_eq!(issue("Visible"), Some((true, None)));
    assert_eq!(issue("Secret"), Some((false, Some(Issue::Hidden))));
    assert_eq!(issue("Shown layer"), Some((true, None)));
}

#[test]
fn study_sample_hidden_layer_is_flagged() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/study/samples/redact-source.pdf");
    let d = dir("hidden-sample");
    let (eng, id, _) = open(&d, &std::fs::read(src).unwrap());
    let tb = blocks(&eng, id, 0).unwrap();
    let secret = tb.blocks.iter().find(|b| b.text.contains("HIDDEN-LAYER-SECRET")).expect("the hidden layer's text is listed");
    assert_eq!((secret.editable, secret.issue), (false, Some(Issue::Hidden)));
    assert!(tb.blocks.iter().any(|b| b.editable), "the visible text stays editable");
}

fn add_box(eng: &Engine, id: DocId, t: &str, at: [f32; 2], width: Option<f32>) -> EditState {
    let new = NewTextBox { text: t.into(), size: 14.0, color: [0.0, 0.0, 0.0], at, width };
    add(eng, id, rev(eng, id), 0, &new, EditSource::User).unwrap()
}

#[test]
fn line_spacing_bold_and_italic_are_changes_of_their_own() {
    let d = dir("spacing");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let b0 = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert!((b0.line_spacing.unwrap() - 14.0 / 12.0).abs() < 0.05, "{:?}", b0.line_spacing);
    assert_eq!(blocks(&eng, id, 0).unwrap().blocks[0].line_spacing, None, "one line has no spacing");

    let ch = BlockChange { line_spacing: Some(2.0), ..Default::default() };
    let r = edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    assert_eq!(r.state.undo.as_deref(), Some("Format text on p. 1"));
    assert!(r.fallback.is_none());
    let b = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert!((b.line_spacing.unwrap() - 2.0).abs() < 0.05, "{:?}", b.line_spacing);
    assert_eq!(b.text, b0.text);
    let bad = BlockChange { line_spacing: Some(9.0), ..Default::default() };
    assert!(edit(&eng, id, rev(&eng, id), 0, 1, &bad, EditSource::User).is_err());

    let ch = BlockChange { bold: Some(true), italic: Some(true), ..Default::default() };
    let r = edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    let fb = r.fallback.expect("a restyled block is set in a standard face");
    assert_eq!((fb.font.as_str(), fb.embedded), ("Helvetica-BoldOblique", false), "{fb:?}");
    assert!(fb.reason.contains("bold italic"), "{}", fb.reason);
    let b = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert!(b.bold && b.italic, "{b:?}");
    assert_eq!(b.text, b0.text);

    eng.save(id, None).unwrap();
    eng.close(id);
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let again = blocks(&eng2, id2, 0).unwrap().blocks[1].clone();
    assert!(again.bold && again.italic && (again.line_spacing.unwrap() - 2.0).abs() < 0.05, "{again:?}");
}

#[test]
fn adds_a_text_box_that_undoes_and_survives_save_and_reopen() {
    let d = dir("add");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let s = add_box(&eng, id, "Paid in full", [100.0, 400.0], None);
    assert_eq!(s.undo.as_deref(), Some("Add text on p. 1"));
    assert!(s.dirty);
    let tb = blocks(&eng, id, 0).unwrap();
    assert_eq!(tb.blocks.len(), 3, "{:?}", tb.blocks);
    let n = tb.blocks.iter().find(|b| b.text == "Paid in full").expect("the new block is listed");
    assert!((n.rect[0] - 100.0).abs() < 1.0 && (n.rect[1] - 400.0).abs() < 4.0, "{:?}", n.rect);
    assert_eq!((n.font.as_str(), n.size.round(), n.editable), ("Helvetica", 14.0, true));
    assert!(text(&eng, id).contains("Quarterly report"), "the page keeps its text: {}", text(&eng, id));

    // The new block is an ordinary one: it can be edited.
    let i = n.index;
    set_text(&eng, id, i, "Paid in full on 3 May");
    assert!(text(&eng, id).contains("Paid in full on 3 May"));
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    assert_eq!(blocks(&eng, id, 0).unwrap().blocks.len(), 2);
    assert!(!eng.edit_state(id).unwrap().dirty);
    eng.redo(id).unwrap();

    eng.save(id, None).unwrap();
    eng.close(id);
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let t = eng2.page_text(id2, 0).unwrap().iter().map(|l| l.t.trim().to_string()).collect::<Vec<_>>().join(" ");
    assert!(t.contains("Paid in full") && t.contains("Quarterly report"), "{t}");
}

#[test]
fn an_added_box_wraps_to_its_width_and_takes_other_scripts() {
    let d = dir("add-wrap");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    add_box(&eng, id, "The deposit is returned within thirty days of the end of the lease.", [72.0, 300.0], Some(150.0));
    let b = blocks(&eng, id, 0).unwrap().blocks.into_iter().find(|b| b.text.starts_with("The deposit")).unwrap();
    assert!(b.lines.len() >= 3 && b.lines.iter().all(|l| l[2] - l[0] <= 150.5), "{:?}", b.lines);
    // Greek needs more than WinAnsi: the standard face is embedded.
    add_box(&eng, id, "Ευχαριστώ", [72.0, 500.0], None);
    assert!(text(&eng, id).contains("Ευχαριστώ"), "{}", text(&eng, id));
    assert!(
        add(
            &eng,
            id,
            rev(&eng, id),
            0,
            &NewTextBox { text: "  ".into(), size: 12.0, color: [0.0; 3], at: [0.0, 0.0], width: None },
            EditSource::User
        )
        .is_err()
    );
    assert!(
        add(
            &eng,
            id,
            rev(&eng, id) + 1,
            0,
            &NewTextBox { text: "x".into(), size: 12.0, color: [0.0; 3], at: [0.0, 0.0], width: None },
            EditSource::User
        )
        .is_err()
    );
}

#[test]
fn adding_text_again_does_not_nest_another_wrapper() {
    let d = dir("add-twice");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    for (i, t) in ["First note", "Second note", "Third note"].iter().enumerate() {
        add_box(&eng, id, t, [72.0, 300.0 + 40.0 * i as f32], None);
    }
    eng.save(id, None).unwrap();
    // The page's contents: the original stream, one `q` before and one `Q` after it, one stream per box.
    let doc = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let contents = doc.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap();
    assert_eq!(contents.len().unwrap(), 1 + 2 + 3);
    let t = text(&eng, id);
    assert!(t.contains("First note") && t.contains("Second note") && t.contains("Third note") && t.contains("Quarterly report"), "{t}");
}

#[test]
fn text_added_to_a_rotated_page_reads_upright() {
    let d = dir("add-rotated");
    let bytes = String::from_utf8(pdf(TWO_BLOCKS, &[("F1", HELV)], &[]))
        .unwrap()
        .replace("/MediaBox [0 0 612 792]", "/MediaBox [0 0 612 792] /Rotate 90");
    let (eng, id, _) = open(&d, bytes.as_bytes());
    // The page as shown is 792 wide and 612 tall.
    add_box(&eng, id, "Rotated note", [200.0, 100.0], None);
    let b = blocks(&eng, id, 0).unwrap().blocks.into_iter().find(|b| b.text == "Rotated note").expect("listed");
    assert!(b.rect[2] - b.rect[0] > b.rect[3] - b.rect[1], "a horizontal line in page space: {:?}", b.rect);
    assert!((b.rect[0] - 200.0).abs() < 1.5 && (b.rect[1] - 100.0).abs() < 4.0, "{:?}", b.rect);
}

// ---- runs, bundled faces, font list, reflow ----------------------------------------------------

fn reopen(path: &str) -> (Arc<Engine>, DocId) {
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(path).unwrap().doc;
    (eng, id)
}

/// The operators of page 1's content in a saved file.
fn saved_ops(path: &str) -> Vec<content::Op> {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    let c = doc.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap();
    let data: Vec<u8> = if c.is_array().unwrap() {
        (0..c.len().unwrap() as i32)
            .flat_map(|i| [c.get_array(i).unwrap().unwrap().read_stream().unwrap(), b"\n".to_vec()].concat())
            .collect()
    } else {
        c.read_stream().unwrap()
    };
    content::parse(&data)
}

/// Every font in a saved file: (/BaseFont, font program length, /FontFile key).
fn saved_fonts(path: &str) -> Vec<(String, usize, String)> {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    let mut out = Vec::new();
    for i in 1..doc.count_objects().unwrap() as i32 {
        let Ok(Some(o)) = doc.new_indirect(i, 0).unwrap().resolve() else { continue };
        if !o.is_dict().unwrap_or(false)
            || o.get_dict("Type").ok().flatten().and_then(|t| t.as_name().ok()).as_deref() != Some(b"FontDescriptor")
        {
            continue;
        }
        let name = String::from_utf8_lossy(&o.get_dict("FontName").unwrap().unwrap().as_name().unwrap()).into_owned();
        for key in ["FontFile", "FontFile2", "FontFile3"] {
            if let Some(f) = o.get_dict(key).unwrap() {
                out.push((name.clone(), f.read_stream().unwrap().len(), key.to_string()));
            }
        }
    }
    out
}

const MIXED: &str = "BT /F1 12 Tf 72 700 Td [(W) 120 (orld) -40 (s)] TJ ( and ) Tj /F2 12 Tf (bold) Tj /F1 12 Tf ( words) Tj ET";

fn run_change(text: Option<&str>, source: Option<usize>) -> RunChange {
    RunChange { text: text.map(str::to_string), source, ..Default::default() }
}

#[test]
fn a_block_holds_runs_and_keeps_kerning_of_unchanged_ones() {
    let d = dir("runs");
    let (eng, id, path) = open(&d, &pdf(MIXED, &[("F1", HELV), ("F2", HELV_BOLD)], &[]));
    let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert_eq!(b.text, "Worlds and bold words");
    let runs: Vec<(&str, &str)> = b.runs.iter().map(|r| (r.text.as_str(), r.font.as_str())).collect();
    assert_eq!(runs, [("Worlds and ", "Helvetica"), ("bold", "Helvetica-Bold"), (" words", "Helvetica")]);
    // Only the bold run changes.
    let change = BlockChange {
        runs: Some(vec![run_change(None, Some(0)), run_change(Some("strong"), Some(1)), run_change(None, Some(2))]),
        ..Default::default()
    };
    let r = edit(&eng, id, rev(&eng, id), 0, 0, &change, EditSource::User).unwrap();
    assert!(r.fallback.is_none(), "{:?}", r.fallbacks);
    assert_eq!(r.state.undo.as_deref(), Some("Edit text on p. 1"));
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert_eq!(text(&eng, id), "Worlds and strong words");
    let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    let runs: Vec<(&str, &str)> = b.runs.iter().map(|r| (r.text.as_str(), r.font.as_str())).collect();
    assert_eq!(runs, [("Worlds and ", "Helvetica"), ("strong", "Helvetica-Bold"), (" words", "Helvetica")]);
    // The unchanged run's kerning is in the saved bytes: TJ [(W) 120 (orld) -40 (s …)].
    let ops = saved_ops(&path);
    let kerned = ops.iter().any(|o| {
        o.is("TJ") && matches!(o.operands.first(), Some(content::Obj::Array(a)) if a.contains(&content::Obj::Int(120)) && a.contains(&content::Obj::Int(-40)))
    });
    assert!(kerned, "{:?}", ops.iter().filter(|o| o.is("TJ")).collect::<Vec<_>>());
}

#[test]
fn runs_take_their_own_size_colour_and_font() {
    let d = dir("run-styles");
    let (eng, id, path) = open(&d, &pdf(MIXED, &[("F1", HELV), ("F2", HELV_BOLD)], &[]));
    let fonts = fonts(&eng, id).unwrap();
    let mono = fonts.iter().find(|f| f.id == "bundled:NotoSansMono-Regular").expect("the bundled mono face is offered");
    let bold_doc = fonts.iter().find(|f| f.kind == "document" && f.name == "Helvetica-Bold").expect("the document's bold font is offered");
    let runs = vec![
        RunChange { text: Some("Big ".into()), source: Some(0), size: Some(18.0), color: Some([1.0, 0.0, 0.0]), ..Default::default() },
        RunChange { text: Some("plain ".into()), source: Some(0), font: Some(bold_doc.id.clone()), ..Default::default() },
        RunChange { text: Some("code".into()), source: Some(0), font: Some(mono.id.clone()), ..Default::default() },
        RunChange { text: Some(" italic".into()), source: Some(0), italic: Some(true), ..Default::default() },
    ];
    let r = edit(&eng, id, rev(&eng, id), 0, 0, &BlockChange { runs: Some(runs), ..Default::default() }, EditSource::User).unwrap();
    // Italic Helvetica is not in the document: the standard face, reported.
    assert!(r.fallbacks.iter().any(|f| f.font == "Helvetica-Oblique"), "{:?}", r.fallbacks);
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert_eq!(text(&eng, id), "Big plain code italic");
    let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    let got: Vec<(String, f32, [f32; 3], bool, bool)> =
        b.runs.iter().map(|r| (r.font.clone(), r.size.round(), r.color, r.bold, r.italic)).collect();
    assert_eq!(got[0], ("Helvetica".to_string(), 18.0, [1.0, 0.0, 0.0], false, false));
    assert_eq!((got[1].0.as_str(), got[1].3), ("Helvetica-Bold", true));
    assert!(got[2].0.ends_with("+NotoSansMono-Regular"), "{got:?}");
    assert_eq!((got[3].0.as_str(), got[3].4), ("Helvetica-Oblique", true));
    // The embedded mono subset holds four glyphs and .notdef: a few kilobytes.
    let mono = saved_fonts(&path).into_iter().find(|f| f.0.ends_with("NotoSansMono-Regular")).expect("embedded");
    assert!(mono.1 < 8 << 10 && mono.2 == "FontFile2", "{mono:?}");
}

#[test]
fn cjk_and_greek_text_is_set_in_bundled_subsets() {
    let d = dir("cjk");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let r = set_text(&eng, id, 0, "你好，世界。안녕하세요 Ωμέγα");
    let fonts: Vec<(&str, bool)> = r.fallbacks.iter().map(|f| (f.font.as_str(), f.embedded)).collect();
    assert!(fonts.contains(&("NotoSans-Regular", true)) && fonts.contains(&("NotoSansCJKsc-Regular", true)), "{fonts:?}");
    // Bold CJK is drawn by stroking the outlines (the bundled CJK face has one weight).
    let runs = vec![RunChange { text: Some("太字".into()), source: Some(0), bold: Some(true), ..Default::default() }];
    edit(&eng, id, rev(&eng, id), 0, 1, &BlockChange { runs: Some(runs), ..Default::default() }, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    let t = text(&eng, id);
    assert!(t.starts_with("你好，世界。안녕하세요 Ωμέγα"), "{t}");
    assert!(t.contains("太字"), "{t}");
    let embedded = saved_fonts(&path);
    let cjk: Vec<_> = embedded.iter().filter(|f| f.0.ends_with("+NotoSansCJKsc-Regular")).collect();
    let latin = embedded.iter().find(|f| f.0.ends_with("+NotoSans-Regular")).expect("Noto Sans subset");
    assert!(!cjk.is_empty() && cjk.iter().all(|f| f.2 == "FontFile3" && f.1 < 24 << 10), "{cjk:?}");
    assert!(latin.2 == "FontFile2" && latin.1 < 12 << 10, "{latin:?}");
    assert!(saved_ops(&path).iter().any(|o| o.is("Tr") && o.num(0) == Some(2.0)), "synthetic bold");
    // The new blocks can be edited again (their embedded subsets read back).
    let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert!(b.editable, "{b:?}");
}

#[test]
fn an_added_box_takes_cjk_text() {
    let d = dir("add-cjk");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    add_box(&eng, id, "受付番号 1234", [72.0, 500.0], None);
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert!(text(&eng, id).contains("受付番号 1234"), "{}", text(&eng, id));
}

#[test]
fn the_font_list_offers_document_standard_and_bundled_faces() {
    let d = dir("font-list");
    let (eng, id, _) = open(&d, &pdf(MIXED, &[("F1", HELV), ("F2", HELV_BOLD)], &[]));
    let list = fonts(&eng, id).unwrap();
    let doc: Vec<&str> = list.iter().filter(|f| f.kind == "document").map(|f| f.name.as_str()).collect();
    assert_eq!(doc, ["Helvetica", "Helvetica-Bold"]);
    assert!(list.iter().filter(|f| f.kind == "document").all(|f| f.id.starts_with("doc:") && f.usable && f.pages == [0]));
    assert_eq!(list.iter().filter(|f| f.kind == "standard").count(), 12);
    let bundled: Vec<&str> = list.iter().filter(|f| f.kind == "bundled").map(|f| f.id.as_str()).collect();
    assert_eq!(bundled.len(), 7);
    assert!(bundled.contains(&"bundled:NotoSansCJK-Regular"));
    // Every bundled id is accepted by a rewrite.
    let runs =
        vec![RunChange { text: Some("x".into()), source: Some(0), font: Some("bundled:NotoSans-BoldItalic".into()), ..Default::default() }];
    assert!(edit(&eng, id, rev(&eng, id), 0, 0, &BlockChange { runs: Some(runs), ..Default::default() }, EditSource::User).is_ok());
    let bad = vec![RunChange { text: Some("x".into()), font: Some("bundled:Nope".into()), ..Default::default() }];
    assert!(edit(&eng, id, rev(&eng, id), 0, 0, &BlockChange { runs: Some(bad), ..Default::default() }, EditSource::User).is_err());
}

#[test]
fn justified_paragraphs_rewrap_and_stay_justified() {
    let d = dir("justify");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let long = "Justified text spreads the free width of every line but the last over its spaces, so both edges line up.";
    let change = BlockChange { text: Some(long.into()), align: Some(Align::Justify), width: Some(160.0), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &change, EditSource::User).unwrap();
    let check = |b: &TextBlockDto| {
        assert_eq!(b.align, Align::Justify, "{b:?}");
        let n = b.lines.len();
        assert!(n >= 4, "{n} lines");
        let right = b.lines[0][2];
        assert!(b.lines[..n - 1].iter().all(|l| (l[2] - right).abs() < 0.6 && (l[0] - b.lines[0][0]).abs() < 0.01), "{:?}", b.lines);
        assert!(b.lines[n - 1][2] < right - 5.0, "the last line is not stretched");
    };
    let b = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    check(&b);
    // Another text change keeps the justification and the width, and re-wraps.
    let longer = format!("{long} A second sentence makes the paragraph longer still.");
    set_text(&eng, id, 1, &longer);
    let b2 = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    check(&b2);
    assert!(b2.lines.len() > b.lines.len());
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    let b3 = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    check(&b3);
    let t = text(&eng, id);
    assert!(t.replace(' ', "").contains(&longer.replace(' ', "")), "{t}");
}

/// The names in page 1's /Font dict of a saved file.
fn saved_font_names(path: &str) -> Vec<String> {
    let doc = mupdf::pdf::PdfDocument::open(path).unwrap();
    let fonts = doc.find_page(0).unwrap().get_dict_inheritable("Resources").unwrap().unwrap().get_dict("Font").unwrap().unwrap();
    (0..fonts.dict_len().unwrap() as i32)
        .map(|i| String::from_utf8_lossy(&fonts.get_dict_key(i).unwrap().unwrap().as_name().unwrap()).into_owned())
        .collect()
}

#[test]
fn word_spacing_of_the_source_is_not_counted_twice() {
    let d = dir("tw");
    // Two lines justified with Tw: both 100 pt wide (each 64.704 pt of glyphs and two spaces).
    let content = "BT /F1 12 Tf 17.648 Tw 14 TL 72 700 Td (aaa bbb ccc) Tj T* (ccc bbb aaa) Tj ET";
    let (eng, id, path) = open(&d, &pdf(content, &[("F1", HELV)], &[]));
    let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert_eq!(b.align, Align::Justify, "{b:?}");
    assert!(b.lines.iter().all(|l| (l[2] - l[0] - 100.0).abs() < 0.1), "{:?}", b.lines);
    // A colour change re-sets the kept runs with Tw 0: the free width goes back over the spaces.
    let ch = BlockChange { color: Some([0.0, 0.0, 1.0]), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 0, &ch, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert_eq!(text(&eng, id), "aaa bbb ccc ccc bbb aaa");
    let a = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    // Without Tw the first line takes four words, spread to the full 100 pt.
    assert_eq!(a.lines.len(), 2, "{:?}", a.lines);
    assert!((a.lines[0][0] - 72.0).abs() < 0.1 && (a.lines[0][2] - 172.0).abs() < 0.6, "{:?}", a.lines);
    // Right-aligned, the lines end at the box's right edge.
    let ch = BlockChange { align: Some(Align::Right), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 0, &ch, EditSource::User).unwrap();
    let r = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert!(r.lines.iter().all(|l| (l[2] - 172.0).abs() < 0.6), "{:?}", r.lines);
}

#[test]
fn a_gap_read_as_a_space_keeps_its_width() {
    let d = dir("gap");
    let content = "BT /F1 12 Tf 72 700 Td [(Hello) -600 (world)] TJ ET";
    let (eng, id, path) = open(&d, &pdf(content, &[("F1", HELV)], &[]));
    let before = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert_eq!(before.text, "Hello world");
    let ch = BlockChange { color: Some([1.0, 0.0, 0.0]), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 0, &ch, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    let after = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert_eq!(after.text, "Hello world");
    assert!((after.rect[2] - before.rect[2]).abs() < 0.01, "{:?} -> {:?}", before.rect, after.rect);
    // Still a gap, not Helvetica's space: -250 for the space and -350 left of it.
    let shows =
        |o: &content::Op| matches!(o.operands.first(), Some(content::Obj::Array(a)) if a.iter().any(|x| matches!(x, content::Obj::Str(_))));
    let tj: Vec<_> = saved_ops(&path).into_iter().filter(|o| o.is("TJ") && shows(o)).collect();
    let nums: f64 = tj
        .iter()
        .flat_map(|o| match o.operands.first() {
            Some(content::Obj::Array(a)) => a.iter().filter_map(content::Obj::as_f64).collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .sum();
    assert!((nums + 600.0).abs() < 0.01, "{tj:?}");
}

#[test]
fn cjk_only_text_falls_back_to_the_cjk_face_alone() {
    let d = dir("cjk-only");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let r = set_text(&eng, id, 0, "你好 世界");
    let fonts: Vec<&str> = r.fallbacks.iter().map(|f| f.font.as_str()).collect();
    assert_eq!(fonts, ["NotoSansCJKsc-Regular"]);
    assert!(r.fallbacks[0].reason.contains("Helvetica"), "{:?}", r.fallbacks);
    eng.save(id, None).unwrap();
    let embedded = saved_fonts(&path);
    assert!(embedded.iter().all(|f| !f.0.contains("+NotoSans-")), "no empty Noto Sans subset: {embedded:?}");
    assert_eq!(embedded.len(), 1, "{embedded:?}");
}

#[test]
fn edits_again_leave_no_unused_fonts_behind() {
    let d = dir("no-orphan-fonts");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    for t in ["你好", "世界 Ωμέγα", "再见"] {
        set_text(&eng, id, 0, t);
    }
    // Italic Helvetica is not on the page: a standard face, added once and reused.
    for t in ["one", "two"] {
        let runs = vec![RunChange { text: Some(t.into()), source: Some(0), italic: Some(true), ..Default::default() }];
        edit(&eng, id, rev(&eng, id), 0, 1, &BlockChange { runs: Some(runs), ..Default::default() }, EditSource::User).unwrap();
    }
    eng.save(id, None).unwrap();
    eng.close(id);
    let mut names = saved_font_names(&path);
    names.sort();
    assert_eq!(names, ["F1", "IwEd0", "IwNoto0"], "only what the content sets");
    let embedded = saved_fonts(&path);
    assert_eq!(embedded.len(), 1, "one CJK subset, the last one: {embedded:?}");
    let (eng, id) = reopen(&path);
    assert_eq!(text(&eng, id), "再见 two");
}

#[test]
fn characters_sharing_a_glyph_keep_their_own_text() {
    let d = dir("shared-glyph");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    // U+301C WAVE DASH and U+FF5E FULLWIDTH TILDE are one glyph in the CJK face; U+03A9 and
    // U+2126 (ohm) one in Noto Sans.
    set_text(&eng, id, 0, "〜～ ΩΩ");
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert!(text(&eng, id).starts_with("〜～ ΩΩ"), "{}", text(&eng, id));
    assert_eq!(blocks(&eng, id, 0).unwrap().blocks[0].text, "〜～ ΩΩ");
}

#[test]
fn korean_without_spaces_wraps_between_syllables() {
    let d = dir("hangul");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let ko = "대한민국의수도는서울특별시입니다그리고부산은항구도시입니다";
    let ch = BlockChange { text: Some(ko.into()), width: Some(120.0), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    let b = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert!(b.lines.len() >= 3, "{:?}", b.lines);
    assert!(b.lines.iter().all(|l| l[2] - l[0] <= 120.5), "{:?}", b.lines);
    assert_eq!(b.text.replace(' ', ""), ko);
}

#[test]
fn a_line_under_larger_text_clears_it() {
    let d = dir("big-run");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let runs = vec![
        RunChange { text: Some("small ".into()), source: Some(0), size: Some(12.0), ..Default::default() },
        RunChange { text: Some("BIG\n".into()), source: Some(0), size: Some(48.0), ..Default::default() },
        RunChange { text: Some("body text".into()), source: Some(0), size: Some(12.0), ..Default::default() },
    ];
    edit(&eng, id, rev(&eng, id), 0, 0, &BlockChange { runs: Some(runs), ..Default::default() }, EditSource::User).unwrap();
    let lines = eng.page_text(id, 0).unwrap();
    let big = lines.iter().find(|l| l.t.contains("BIG")).unwrap_or_else(|| panic!("{lines:?}"));
    let body = lines.iter().find(|l| l.t.contains("body")).unwrap_or_else(|| panic!("{lines:?}"));
    assert!(body.b[1] >= big.b[3] - 0.5, "the body line overlaps the big text: {big:?} {body:?}");
}

#[test]
fn a_moved_block_stays_put_when_edited_again() {
    let d = dir("move-then-edit");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let ch = BlockChange { offset: Some([50.0, 100.0]), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &ch, EditSource::User).unwrap();
    let moved = rect_of(&eng, id, "The first line of the body and its second line.");
    set_text(&eng, id, index_of(&eng, id, "The first line of the body and its second line."), "Edited after the move");
    let r = rect_of(&eng, id, "Edited after the move");
    assert!((r[0] - moved[0]).abs() < 0.5 && (r[1] - moved[1]).abs() < 0.5, "{moved:?} -> {r:?}");
    // And again, with a colour change, then through save and reopen.
    let ch = BlockChange { color: Some([0.0, 0.5, 0.0]), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, index_of(&eng, id, "Edited after the move"), &ch, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    let r = rect_of(&eng, id, "Edited after the move");
    assert!((r[0] - moved[0]).abs() < 0.5 && (r[1] - moved[1]).abs() < 0.5, "{moved:?} -> {r:?}");
}

#[test]
fn an_added_box_stays_put_when_edited() {
    let d = dir("add-then-edit");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    add_box(&eng, id, "Added note", [300.0, 400.0], None);
    let added = rect_of(&eng, id, "Added note");
    assert!((added[0] - 300.0).abs() < 1.0 && (added[1] - 400.0).abs() < 4.0, "{added:?}");
    set_text(&eng, id, index_of(&eng, id, "Added note"), "Added note, edited");
    let r = rect_of(&eng, id, "Added note, edited");
    assert!((r[0] - added[0]).abs() < 0.5 && (r[1] - added[1]).abs() < 0.5, "{added:?} -> {r:?}");
    set_text(&eng, id, index_of(&eng, id, "Added note, edited"), "Edited twice");
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    let r = rect_of(&eng, id, "Edited twice");
    assert!((r[0] - added[0]).abs() < 0.5 && (r[1] - added[1]).abs() < 0.5, "{added:?} -> {r:?}");
    // The q/Q nesting does not grow with each edit.
    let ops = saved_ops(&path);
    let mut depth: i32 = 0;
    let mut max = 0;
    for o in &ops {
        if o.is("q") {
            depth += 1;
            max = max.max(depth);
        } else if o.is("Q") {
            depth -= 1;
        }
    }
    assert!(max <= 3, "q depth {max}");
}

#[test]
fn moving_a_block_again_and_again_does_not_nest_deeper() {
    let d = dir("many-moves");
    let (eng, id, path) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let t = "The first line of the body and its second line.";
    let start = rect_of(&eng, id, t);
    for _ in 0..40 {
        let ch = BlockChange { offset: Some([2.5, -3.0]), ..Default::default() };
        edit(&eng, id, rev(&eng, id), 0, index_of(&eng, id, t), &ch, EditSource::User).unwrap();
    }
    // An added box moved too: its placement cm takes the moves.
    add_box(&eng, id, "Box", [300.0, 300.0], None);
    let boxed = rect_of(&eng, id, "Box");
    for _ in 0..30 {
        let ch = BlockChange { offset: Some([1.0, 1.0]), ..Default::default() };
        edit(&eng, id, rev(&eng, id), 0, index_of(&eng, id, "Box"), &ch, EditSource::User).unwrap();
    }
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    let r = rect_of(&eng, id, t);
    assert!((r[0] - start[0] - 100.0).abs() < 0.5 && (r[1] - start[1] + 120.0).abs() < 0.5, "{start:?} -> {r:?}");
    let b = rect_of(&eng, id, "Box");
    assert!((b[0] - boxed[0] - 30.0).abs() < 0.5 && (b[1] - boxed[1] - 30.0).abs() < 0.5, "{boxed:?} -> {b:?}");
    let mut depth: i32 = 0;
    let mut max = 0;
    for o in &saved_ops(&path) {
        if o.is("q") {
            depth += 1;
            max = max.max(depth);
        } else if o.is("Q") {
            depth -= 1;
        }
    }
    assert!(max <= 3, "q depth {max}");
}

// ---- underline, strikeout, superscript, subscript ----------------------------------------------

/// The operators of page 1's content as the document is now.
fn live_ops(eng: &Engine, id: DocId) -> Vec<content::Op> {
    eng.read_custom(id, |d| {
        let p = d.find_page(0)?;
        let data: Vec<u8> = text::page_streams(&p)?.into_iter().flat_map(|(_, d)| [d, b"\n".to_vec()].concat()).collect();
        Ok(content::parse(&data))
    })
    .unwrap()
}

/// Our decorations in `ops`: (strikeout, `re` operands).
fn decorations(ops: &[content::Op]) -> Vec<(bool, Vec<f64>)> {
    let mut out = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let Some(content::Obj::Dict(d)) = op.is("BDC").then(|| op.operands.get(1)).flatten() else { continue };
        let Some((_, kind)) = d.iter().find(|(k, _)| k == b"IWDeco") else { continue };
        let end = ops[i..].iter().position(|o| o.is("EMC")).map_or(ops.len(), |p| i + p);
        let re = ops[i..end].iter().find(|o| o.is("re")).expect("a decoration draws a rectangle");
        out.push((kind.as_name() == Some(b"S".as_slice()), re.operands.iter().filter_map(content::Obj::as_f64).collect()));
    }
    out
}

fn change_runs(eng: &Engine, id: DocId, runs: Vec<RunChange>) -> TextEdited {
    let change = BlockChange { runs: Some(runs), ..Default::default() };
    edit(eng, id, rev(eng, id), 0, 0, &change, EditSource::User).unwrap()
}

fn run_with(text: Option<&str>, source: usize, f: impl FnOnce(&mut RunChange)) -> RunChange {
    let mut r = run_change(text, Some(source));
    f(&mut r);
    r
}

fn runs_of(eng: &Engine, id: DocId) -> Vec<RunDto> {
    blocks(eng, id, 0).unwrap().blocks[0].runs.clone()
}

#[test]
fn superscript_and_subscript_round_trip_and_toggle_back() {
    let d = dir("rise");
    let (eng, id, path) = open(&d, &pdf("BT /F1 12 Tf 72 700 Td (H2O and x2) Tj ET", &[("F1", HELV)], &[]));
    let sub = |r: &mut RunChange| r.rise = Some(Some(Rise::Sub));
    let sup = |r: &mut RunChange| r.rise = Some(Some(Rise::Super));
    change_runs(
        &eng,
        id,
        vec![
            run_with(Some("H"), 0, |_| {}),
            run_with(Some("2"), 0, sub),
            run_with(Some("O and x"), 0, |_| {}),
            run_with(Some("2"), 0, sup),
        ],
    );
    let r = runs_of(&eng, id);
    let shape: Vec<(&str, Option<Rise>, f32)> = r.iter().map(|r| (r.text.as_str(), r.rise, (r.size * 10.0).round() / 10.0)).collect();
    assert_eq!(
        shape,
        [("H", None, 12.0), ("2", Some(Rise::Sub), 7.8), ("O and x", None, 12.0), ("2", Some(Rise::Super), 7.8)]
    );
    // One line, one block: the raised runs stay on their line.
    let tb = blocks(&eng, id, 0).unwrap();
    assert_eq!((tb.blocks.len(), tb.blocks[0].lines.len(), tb.blocks[0].text.as_str()), (1, 1, "H2O and x2"));
    assert!((tb.blocks[0].size - 12.0).abs() < 0.01, "the block's own size is the body text's");
    // Ts is written: -0.15 and +0.33 of the 12 pt base.
    eng.save(id, None).unwrap();
    let ts: Vec<f64> = saved_ops(&path).iter().filter(|o| o.is("Ts")).filter_map(|o| o.num(0)).collect();
    assert!(ts.contains(&-1.8) && ts.contains(&3.96), "{ts:?}");
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert_eq!(runs_of(&eng, id).iter().map(|r| r.rise).collect::<Vec<_>>(), [None, Some(Rise::Sub), None, Some(Rise::Super)]);
    // Back to the line: the base size returns.
    change_runs(
        &eng,
        id,
        (0..4).map(|i| run_with(None, i, |r| if i % 2 == 1 { r.rise = Some(None) })).collect(),
    );
    let r = runs_of(&eng, id);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!((r[0].text.as_str(), r[0].rise, r[0].size.round()), ("H2O and x2", None, 12.0));
}

#[test]
fn a_rise_from_a_file_made_without_us_is_read_and_kept() {
    let d = dir("foreign-rise");
    let content = "BT /F1 12 Tf 72 700 Td (E = mc) Tj /F1 8 Tf 4 Ts (2) Tj 0 Ts /F1 12 Tf ( and H) Tj /F1 8 Tf -3 Ts (2) Tj 0 Ts /F1 12 Tf (O) Tj ET";
    let (eng, id, path) = open(&d, &pdf(content, &[("F1", HELV)], &[]));
    let tb = blocks(&eng, id, 0).unwrap();
    assert_eq!((tb.blocks.len(), tb.blocks[0].lines.len()), (1, 1), "{:?}", tb.blocks);
    let rises: Vec<Option<Rise>> = tb.blocks[0].runs.iter().map(|r| r.rise).collect();
    assert_eq!(rises, [None, Some(Rise::Super), None, Some(Rise::Sub), None]);
    assert_eq!(tb.blocks[0].runs[1].size, 8.0);
    // Another run's text changes: the raised runs keep their own Ts and size.
    change_runs(
        &eng,
        id,
        vec![
            run_with(Some("E = MC"), 0, |_| {}),
            run_with(None, 1, |_| {}),
            run_with(None, 2, |_| {}),
            run_with(None, 3, |_| {}),
            run_with(None, 4, |_| {}),
        ],
    );
    eng.save(id, None).unwrap();
    let ts: Vec<f64> = saved_ops(&path).iter().filter(|o| o.is("Ts")).filter_map(|o| o.num(0)).collect();
    assert!(ts.contains(&4.0) && ts.contains(&-3.0), "{ts:?}");
    eng.close(id);
    let (eng, id) = reopen(&path);
    let r = runs_of(&eng, id);
    assert_eq!((r.len(), r[1].size, r[1].rise), (5, 8.0, Some(Rise::Super)));
}

#[test]
fn a_paragraph_that_starts_with_a_footnote_mark_keeps_its_line_gap() {
    let d = dir("footnote");
    let content = "BT /F1 8 Tf 72 700 Td 4 Ts (1) Tj 0 Ts /F1 12 Tf ( First line of the note) Tj 0 -14 Td (second line of it.) Tj ET";
    let (eng, id, _) = open(&d, &pdf(content, &[("F1", HELV)], &[]));
    let first = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert_eq!((first.lines.len(), first.size), (2, 12.0), "the block's own size is the body text's: {first:?}");
    let gap = first.line_spacing.unwrap();
    for colour in [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.5, 0.0]] {
        let change = BlockChange { color: Some(colour), ..Default::default() };
        edit(&eng, id, rev(&eng, id), 0, 0, &change, EditSource::User).unwrap();
        let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
        assert!((b.line_spacing.unwrap() - gap).abs() < 0.01, "{gap} -> {:?}", b.line_spacing);
        assert_eq!(b.runs[0].rise, Some(Rise::Super));
    }
}

#[test]
fn underline_and_strikeout_are_drawn_and_read_back() {
    let d = dir("deco");
    let (eng, id, path) = open(&d, &pdf("BT /F1 12 Tf 72 700 Td (Hello big world) Tj ET", &[("F1", HELV)], &[]));
    let r = change_runs(
        &eng,
        id,
        vec![
            run_with(Some("Hello "), 0, |_| {}),
            run_with(Some("big"), 0, |r| r.underline = Some(true)),
            run_with(Some(" "), 0, |_| {}),
            run_with(Some("world"), 0, |r| r.strike = Some(true)),
        ],
    );
    assert_eq!(r.state.undo.as_deref(), Some("Format text on p. 1"));
    let runs = runs_of(&eng, id);
    let flags: Vec<(&str, bool, bool)> = runs.iter().map(|r| (r.text.as_str(), r.underline, r.strike)).collect();
    assert_eq!(flags, [("Hello ", false, false), ("big", true, false), (" ", false, false), ("world", false, true)]);
    let decos = decorations(&live_ops(&eng, id));
    assert_eq!(decos.len(), 2, "{decos:?}");
    // "big" at 12 pt in Helvetica is 0.556 + 0.222 + 0.556 em wide; the line is 0.05 em thick.
    let (strike, re) = &decos[0];
    assert!(!strike && (re[2] - 16.0).abs() < 0.2 && (re[3] - 0.6).abs() < 0.01, "{re:?}");
    assert!(decos[1].0);
    // Only our own tagged rectangles read back, and they survive save and reopen.
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    let flags: Vec<(bool, bool)> = runs_of(&eng, id).iter().map(|r| (r.underline, r.strike)).collect();
    assert_eq!(flags, [(false, false), (true, false), (false, false), (false, true)]);
    // The rectangle is not text, and a plain line another producer drew is not guessed at.
    assert_eq!(text(&eng, id), "Hello big world");
    let plain = "BT /F1 12 Tf 72 700 Td (Hello) Tj ET\n72 698 30 0.6 re f";
    let (eng2, id2, _) = open(&dir("deco-foreign"), &pdf(plain, &[("F1", HELV)], &[]));
    assert!(!runs_of(&eng2, id2)[0].underline);
}

#[test]
fn a_decorated_block_moved_and_reflowed_keeps_one_decoration_per_line() {
    let d = dir("deco-reflow");
    let body = "The quick brown fox jumps over the lazy dog and keeps running far away from the farm";
    let (eng, id, _) = open(&d, &pdf(&format!("BT /F1 12 Tf 72 700 Td ({body}) Tj ET"), &[("F1", HELV)], &[]));
    let apply = |c: BlockChange| edit(&eng, id, rev(&eng, id), 0, 0, &c, EditSource::User).unwrap();
    apply(BlockChange { underline: Some(true), width: Some(180.0), ..Default::default() });
    let lines = |e: &Engine| blocks(e, id, 0).unwrap().blocks[0].lines.len();
    let n = lines(&eng);
    assert!(n >= 3, "wrapped into {n} lines");
    assert_eq!(decorations(&live_ops(&eng, id)).len(), n, "one underline per line");
    // Moved, reflowed wider, narrower, struck out too, moved again: never a leftover.
    apply(BlockChange { offset: Some([30.0, -40.0]), ..Default::default() });
    assert_eq!(decorations(&live_ops(&eng, id)).len(), n);
    apply(BlockChange { width: Some(320.0), ..Default::default() });
    let wide = lines(&eng);
    assert!(wide < n);
    assert_eq!(decorations(&live_ops(&eng, id)).len(), wide);
    apply(BlockChange { width: Some(150.0), strike: Some(true), ..Default::default() });
    let narrow = lines(&eng);
    let all = decorations(&live_ops(&eng, id));
    assert_eq!((all.len(), all.iter().filter(|d| d.0).count()), (narrow * 2, narrow), "{narrow} lines, underlined and struck");
    apply(BlockChange { offset: Some([-10.0, 20.0]), ..Default::default() });
    assert_eq!(decorations(&live_ops(&eng, id)).len(), narrow * 2);
    let runs = runs_of(&eng, id);
    assert!(runs.iter().all(|r| r.underline && r.strike), "the moved block still reads decorated: {runs:?}");
    // Turned off, then on again: no rectangle is left behind by either.
    apply(BlockChange { underline: Some(false), strike: Some(false), ..Default::default() });
    assert!(decorations(&live_ops(&eng, id)).is_empty());
    // Emptied: nothing of it is left.
    apply(BlockChange { underline: Some(true), ..Default::default() });
    assert_eq!(decorations(&live_ops(&eng, id)).len(), narrow);
    apply(BlockChange { text: Some(String::new()), ..Default::default() });
    assert!(decorations(&live_ops(&eng, id)).is_empty(), "a block's decorations go with it");
    // Undo brings the decorated block back, whole.
    eng.undo(id).unwrap();
    assert_eq!(decorations(&live_ops(&eng, id)).len(), narrow);
}

#[test]
fn a_justified_line_is_decorated_to_its_full_width() {
    let d = dir("deco-justify");
    let body = "The quick brown fox jumps over the lazy dog and keeps running far away from the farm and then some more";
    let (eng, id, _) = open(&d, &pdf(&format!("BT /F1 12 Tf 72 700 Td ({body}) Tj ET"), &[("F1", HELV)], &[]));
    let c = BlockChange { underline: Some(true), width: Some(200.0), align: Some(Align::Justify), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 0, &c, EditSource::User).unwrap();
    let ops = live_ops(&eng, id);
    let decos = decorations(&ops);
    let lines = blocks(&eng, id, 0).unwrap().blocks[0].lines.len();
    assert_eq!(decos.len(), lines);
    // Every line but the last fills the 200 pt (the rectangle runs from its first glyph to its last).
    for (_, re) in &decos[..lines - 1] {
        assert!((re[2] - 200.0).abs() < 1.5, "{re:?}");
    }
    assert!(decos[lines - 1].1[2] < 200.0);
}

#[test]
fn a_formatting_only_change_of_runs_is_labelled_format_text() {
    let d = dir("deco-label");
    let (eng, id, _) = open(&d, &pdf("BT /F1 12 Tf 72 700 Td (Hello big world) Tj ET", &[("F1", HELV)], &[]));
    let r = change_runs(
        &eng,
        id,
        vec![
            run_with(Some("Hello "), 0, |_| {}),
            run_with(Some("big"), 0, |r| r.underline = Some(true)),
            run_with(Some(" world"), 0, |_| {}),
        ],
    );
    assert_eq!(r.state.undo.as_deref(), Some("Format text on p. 1"));
    let r = change_runs(&eng, id, vec![run_with(Some("Hello"), 0, |_| {}), run_with(Some(" there"), 2, |_| {})]);
    assert_eq!(r.state.undo.as_deref(), Some("Edit text on p. 1"));
}

#[test]
fn rise_in_a_change_reads_null_as_clear_and_missing_as_keep() {
    let c: BlockChange = serde_json::from_str(r#"{"rise":null}"#).unwrap();
    assert_eq!(c.rise, Some(None));
    let c: BlockChange = serde_json::from_str(r#"{"rise":"super","underline":true}"#).unwrap();
    assert_eq!((c.rise, c.underline), (Some(Some(Rise::Super)), Some(true)));
    let c: BlockChange = serde_json::from_str(r#"{"runs":[{"source":0,"rise":null},{"source":1}]}"#).unwrap();
    let runs = c.runs.unwrap();
    assert_eq!((runs[0].rise, runs[1].rise), (Some(None), None));
    assert_eq!(serde_json::from_str::<BlockChange>("{}").unwrap(), BlockChange::default());
}

const CERTIFIED: &str =
    "/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 2 >> >> ] >> >>";
const SIG_FIELD: &str = "<< /FT /Sig /T (Signature1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>";

#[test]
fn a_certified_document_refuses_decoration_and_says_why() {
    let d = dir("deco-certified");
    let (eng, id, _) = open(&d, &pdf_in("BT /F1 12 Tf 72 700 Td (Hello) Tj ET", &[("F1", HELV)], &[], CERTIFIED));
    let tb = blocks(&eng, id, 0).unwrap();
    assert!(tb.locked.as_deref().is_some_and(|m| m.contains("certified")), "{:?}", tb.locked);
    let before = rev(&eng, id);
    for c in [
        BlockChange { underline: Some(true), ..Default::default() },
        BlockChange { rise: Some(Some(Rise::Super)), ..Default::default() },
        BlockChange { runs: Some(vec![run_with(None, 0, |r| r.strike = Some(true))]), ..Default::default() },
    ] {
        match edit(&eng, id, before, 0, 0, &c, EditSource::User) {
            Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(rev(&eng, id), before, "nothing was recorded");
    assert!(decorations(&live_ops(&eng, id)).is_empty());
    // An ordinary document reports no lock and no signature.
    let (eng, id, _) = open(&dir("deco-plain"), &pdf("BT /F1 12 Tf 72 700 Td (Hello) Tj ET", &[("F1", HELV)], &[]));
    let tb = blocks(&eng, id, 0).unwrap();
    assert_eq!((tb.locked, tb.signed), (None, false));
}

#[test]
fn decorating_a_signed_document_is_saved_after_the_signature() {
    let d = dir("deco-signed");
    let catalog = "/AcroForm << /SigFlags 3 /Fields [6 0 R] >>";
    let (eng, id, path) = open(&d, &pdf_in("BT /F1 12 Tf 72 700 Td (Hello big) Tj ET", &[("F1", HELV)], &[SIG_FIELD], catalog));
    assert!(blocks(&eng, id, 0).unwrap().signed);
    let original = std::fs::read(&path).unwrap();
    let c = BlockChange { underline: Some(true), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 0, &c, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the signed bytes are not rewritten");
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert!(runs_of(&eng, id)[0].underline);
}

#[test]
fn a_tagged_group_that_is_not_ours_is_left_alone() {
    let d = dir("deco-foreign-tag");
    // Tagged like a decoration, but it holds text: not a rectangle of ours, so no edit removes it.
    let content = "BT /F1 12 Tf 72 700 Td (Hello) Tj ET\n/Span <</IWDeco /U>> BDC 72 698 30 0.6 re f BT /F1 12 Tf 72 600 Td (Keep me) Tj ET EMC";
    let (eng, id, _) = open(&d, &pdf(content, &[("F1", HELV)], &[]));
    let hello = blocks(&eng, id, 0).unwrap().blocks.iter().find(|b| b.text == "Hello").unwrap().index;
    let c = BlockChange { underline: Some(true), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, hello, &c, EditSource::User).unwrap();
    assert!(text(&eng, id).contains("Keep me"), "{}", text(&eng, id));
    let ops = live_ops(&eng, id);
    assert!(ops.iter().any(|o| o.is("BDC") && matches!(o.operands.get(1), Some(content::Obj::Dict(_)))));
    // Only the one underline we drew counts as ours (the foreign group is not read as a decoration).
    assert_eq!(decorations(&ops).len(), 2, "the foreign group is still there, and one underline of ours");
}

#[test]
fn a_page_of_tagged_groups_is_read_and_edited_fast() {
    let d = dir("deco-flood");
    // 12,000 complete groups of ours (more than a page is read for) and 50,000 that never close.
    let group = "/Span <</IWDeco /U>> BDC q 1 0 0 1 400 100 cm 0 g 0 0 5 0.6 re f Q EMC\n";
    let content = format!(
        "BT /F1 12 Tf 72 700 Td (Hello) Tj ET\n{}{}",
        group.repeat(12_000),
        "/Span <</IWDeco /U>> BDC ".repeat(50_000)
    );
    let (eng, id, _) = open(&d, &pdf(&content, &[("F1", HELV)], &[]));
    let started = std::time::Instant::now();
    let tb = blocks(&eng, id, 0).unwrap();
    assert_eq!(tb.blocks.len(), 1);
    let c = BlockChange { underline: Some(true), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 0, &c, EditSource::User).unwrap();
    assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
    assert!(text(&eng, id).contains("Hello"));
}

#[test]
fn a_rise_past_most_of_the_size_is_a_position_not_a_script() {
    let d = dir("rise-position");
    // 100 pt of `Ts` on 12 pt text: text moved with the rise operator, not a superscript.
    let (eng, id, _) = open(&d, &pdf("BT /F1 12 Tf 72 400 Td 100 Ts (Moved by rise) Tj ET", &[("F1", HELV)], &[]));
    let at = |eng: &Engine| {
        let b = &blocks(eng, id, 0).unwrap().blocks[0];
        assert_eq!(b.runs[0].rise, None, "read as no rise");
        b.rect
    };
    let before = at(&eng);
    // The baseline is at 400 + 100 up: 292 from the top of a 792 pt page.
    assert!(before[1] <= 292.0 && 292.0 <= before[3], "{before:?}");
    set_text(&eng, id, 0, "Moved by rise, edited");
    let after = at(&eng);
    assert!((after[1] - before[1]).abs() < 1.0 && (after[3] - before[3]).abs() < 1.0, "{before:?} -> {after:?}");
    assert!(text(&eng, id).contains("edited"));
}

#[test]
fn a_rise_that_is_not_a_number_the_file_can_hold_is_no_rise() {
    let d = dir("rise-inf");
    // A literal that overflows to infinity as a double.
    let content = format!("BT /F1 12 Tf 72 400 Td {} Ts (Hello) Tj ET", "9".repeat(400));
    let (eng, id, path) = open(&d, &pdf(&content, &[("F1", HELV)], &[]));
    let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert_eq!(b.runs[0].rise, None);
    assert!(b.rect.iter().all(|v| v.is_finite()), "{:?}", b.rect);
    set_text(&eng, id, 0, "Hello again");
    let c = BlockChange { rise: Some(Some(Rise::Super)), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 0, &c, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    let ops = saved_ops(&path);
    // `inf` would not parse as a number: it would come back as an operator of its own.
    assert!(!ops.iter().any(|o| o.op.eq_ignore_ascii_case(b"inf") || o.op.eq_ignore_ascii_case(b"nan")));
    let ts: Vec<f64> = ops.iter().filter(|o| o.is("Ts")).filter_map(|o| o.num(0)).collect();
    assert!(!ts.is_empty() && ts.iter().all(|v| v.is_finite() && v.abs() < 1e5), "{ts:?}");
}

// ---- Find & Replace ----------------------------------------------------------------------------

use super::replace::{replace, replace_within, Limits, ReplaceAt, ReplaceFrom, ReplaceRequest, Replaced};
use crate::search::{Query, SearchOpts};

/// A PDF from whole objects (numbered from 1; object 1 is the catalog).
fn raw_pdf(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Pages with the given contents, all with the same fonts (`F1` Helvetica, `F2` bold, objects 3 and 4); the
/// page and content objects follow from 5, then `extra`.
fn pages_pdf(contents: &[&str]) -> Vec<u8> {
    let n = contents.len();
    let kids: String = (0..n).map(|i| format!("{} 0 R ", 5 + i * 2)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {n} >>"),
        HELV.to_string(),
        HELV_BOLD.to_string(),
    ];
    // Objects 5..: page, content, page, content ...
    for (i, c) in contents.iter().enumerate() {
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
            6 + i * 2
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len()));
    }
    raw_pdf(&objs)
}

fn all_text(eng: &Engine, id: DocId) -> String {
    let n = eng.edit_state(id).unwrap().page_count;
    (0..n).map(|p| eng.page_text(id, p).unwrap().iter().map(|l| l.t.trim().to_string()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" | ")
}

fn req(find: &str, with: &str, opts: SearchOpts) -> ReplaceRequest {
    ReplaceRequest { find: find.into(), with: with.into(), opts, at: None, from: None }
}

fn do_replace(eng: &Engine, id: DocId, r: &ReplaceRequest) -> Result<Replaced> {
    replace(eng, id, rev(eng, id), r, EditSource::User)
}

const CATS: &str = "BT /F1 12 Tf 72 700 Td (The cat and the Cat sat by a concat.) Tj ET";

#[test]
fn replace_all_is_one_undo_step_across_pages_and_survives_save() {
    let d = dir("replace-all");
    let bytes = pages_pdf(&[CATS, "BT /F1 12 Tf 72 700 Td (A cat on page two.) Tj ET\nBT /F1 12 Tf 72 600 Td (Second cat block) Tj ET"]);
    let (eng, id, path) = open(&d, &bytes);
    let before = all_text(&eng, id);
    let r = do_replace(&eng, id, &req("cat", "dog", SearchOpts::default())).unwrap();
    // Case-insensitive, parts of words too: cat, Cat, concat, cat, cat.
    assert_eq!((r.replaced, r.skipped_count, r.stopped.as_deref()), (5, 0, None), "{r:?}");
    assert_eq!(r.state.undo.as_deref(), Some("Replace \"cat\" with \"dog\""));
    assert_eq!(all_text(&eng, id), "The dog and the dog sat by a condog. | A dog on page two. Second dog block");
    // One undo restores both pages.
    let u = eng.undo(id).unwrap();
    assert_eq!(all_text(&eng, id), before, "{u:?}");
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert_eq!(all_text(&eng, id), "The dog and the dog sat by a condog. | A dog on page two. Second dog block");
}

#[test]
fn replace_follows_case_and_whole_word_options() {
    let d = dir("replace-opts");
    let (eng, id, _) = open(&d, &pages_pdf(&[CATS]));
    let case = SearchOpts { case_sensitive: true, ..Default::default() };
    let r = do_replace(&eng, id, &req("cat", "dog", case)).unwrap();
    // "cat" lower case, also inside "concat"; not "Cat".
    assert_eq!(r.replaced, 2, "{r:?}");
    assert_eq!(all_text(&eng, id), "The dog and the Cat sat by a condog.");
    eng.undo(id).unwrap();
    let whole = SearchOpts { whole_words: true, ..Default::default() };
    let r = do_replace(&eng, id, &req("cat", "dog", whole)).unwrap();
    assert_eq!(r.replaced, 2, "{r:?}");
    assert_eq!(all_text(&eng, id), "The dog and the dog sat by a concat.");
}

#[test]
fn a_replacement_that_contains_the_find_text_is_set_once() {
    let d = dir("replace-self");
    let (eng, id, _) = open(&d, &pages_pdf(&["BT /F1 12 Tf 72 700 Td (a cat, a cat) Tj ET"]));
    let r = do_replace(&eng, id, &req("cat", "cat cat", SearchOpts::default())).unwrap();
    assert_eq!((r.replaced, r.inserted), (2, 2));
    assert_eq!(all_text(&eng, id), "a cat cat, a cat cat");
    // A second run replaces what the first one wrote, once more each.
    let r = do_replace(&eng, id, &req("a", "aa", SearchOpts::default())).unwrap();
    assert_eq!(all_text(&eng, id), "aa caat caat, aa caat caat", "{r:?}");
}

#[test]
fn replace_one_changes_only_the_current_match() {
    let d = dir("replace-one");
    let (eng, id, path) = open(&d, &pages_pdf(&["BT /F1 12 Tf 72 700 Td (red fox and red hen) Tj ET\nBT /F1 12 Tf 72 600 Td (another red one) Tj ET"]));
    let q = Query::new("red", SearchOpts::default()).unwrap();
    let hits = eng.read_custom(id, |d| crate::search::find_in_page(d, 0, &q)).unwrap();
    assert_eq!(hits.len(), 3);
    for pick in [1usize, 2] {
        // The second and third match; the list is as it is now (each replacement is one step).
        let hits = eng.read_custom(id, |d| crate::search::find_in_page(d, 0, &q)).unwrap();
        let at = ReplaceAt { page: 0, rect: hits[pick.min(hits.len() - 1)].rects[0] };
        let r = do_replace(&eng, id, &ReplaceRequest { at: Some(at), ..req("red", "blue", SearchOpts::default()) }).unwrap();
        assert_eq!((r.replaced, r.skipped_count), (1, 0), "{r:?}");
        assert_eq!(r.state.undo.as_deref(), Some("Replace text on p. 1"));
        if pick == 1 {
            assert_eq!(all_text(&eng, id), "red fox and blue hen another red one");
        }
    }
    // After the second replacement the list is [red, red]; picking index 1 again hit the last one.
    assert_eq!(all_text(&eng, id), "red fox and blue hen another blue one");
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert_eq!(all_text(&eng, id), "red fox and blue hen another blue one");
}

#[test]
fn a_match_across_runs_takes_the_style_of_the_run_it_starts_in() {
    let d = dir("replace-runs");
    let (eng, id, _) = open(&d, &pages_pdf(&[MIXED]));
    // "and bold" starts in the regular run and ends in the bold one.
    let r = do_replace(&eng, id, &req("and bold", "plus", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced, 1, "{r:?}");
    let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    assert_eq!(b.text, "Worlds plus words");
    let fonts: Vec<(&str, &str)> = b.runs.iter().map(|r| (r.text.as_str(), r.font.as_str())).collect();
    // The text between the two halves is one style again, so it is one run.
    assert_eq!(fonts, [("Worlds plus words", "Helvetica")], "{fonts:?}");
    // A match inside the bold run only changes that run.
    eng.undo(id).unwrap();
    do_replace(&eng, id, &req("bold", "heavy", SearchOpts::default())).unwrap();
    let b = blocks(&eng, id, 0).unwrap().blocks[0].clone();
    let fonts: Vec<(&str, &str)> = b.runs.iter().map(|r| (r.text.as_str(), r.font.as_str())).collect();
    assert_eq!(fonts, [("Worlds and ", "Helvetica"), ("heavy", "Helvetica-Bold"), (" words", "Helvetica")]);
}

#[test]
fn matches_edit_text_refuses_are_skipped_with_their_reason_and_page() {
    let d = dir("replace-skips");
    let type3 = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
                 /CharProcs << /a 7 0 R >> /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] >>";
    let glyph = "<< /Length 28 >>\nstream\n1000 0 d0 0 0 1000 1000 re f\nendstream".to_string();
    // Page 1: one editable "aaa x" block and a Type 3 "aaa"; page 2: invisible OCR text "aaa".
    let c1 = "BT /F1 12 Tf 72 700 Td (aaa x) Tj ET\nBT /F3 20 Tf 72 500 Td (aaa) Tj ET";
    let c2 = "BT /F1 12 Tf 3 Tr 72 500 Td (aaa hidden) Tj ET";
    // Objects: 1-4 catalog, pages, fonts; 5-8 the two pages and their contents; 9 the glyph; 10 the Type 3 font.
    let mut objs = pages_pdf_objs(&[c1, c2]);
    objs[4] = objs[4].replace("/F2 4 0 R", "/F2 4 0 R /F3 10 0 R");
    objs.push(glyph);
    objs.push(type3.replace("/a 7 0 R", "/a 9 0 R"));
    let (eng, id, _) = open(&d, &raw_pdf(&objs));
    let r = do_replace(&eng, id, &req("aaa", "bbb", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced, 1, "{r:?}");
    assert_eq!(r.skipped_count, 2, "{r:?}");
    let by_page: Vec<(u32, bool)> = r.skipped.iter().map(|s| (s.page, s.reason.starts_with("skipped: "))).collect();
    assert_eq!(by_page, [(0, true), (1, true)], "{r:?}");
    assert!(r.skipped[0].reason.contains("Type 3"), "{:?}", r.skipped[0]);
    assert!(r.skipped[1].reason.contains("invisible"), "{:?}", r.skipped[1]);
    // The editable block changed, nothing else did.
    assert!(all_text(&eng, id).starts_with("bbb x"), "{}", all_text(&eng, id));
    // A run that only meets refused text is no step at all.
    let before = rev(&eng, id);
    let r = do_replace(&eng, id, &req("hidden", "shown", SearchOpts::default())).unwrap();
    assert_eq!((r.replaced, r.skipped_count, rev(&eng, id)), (0, 1, before));
}

/// The objects of `pages_pdf`, for tests that add to them.
fn pages_pdf_objs(contents: &[&str]) -> Vec<String> {
    let bytes = pages_pdf(contents);
    let s = String::from_utf8(bytes).unwrap();
    let mut objs = Vec::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find(" 0 obj\n") {
        let after = &rest[i + 7..];
        let end = after.find("\nendobj\n").unwrap();
        objs.push(after[..end].to_string());
        rest = &after[end + 8..];
    }
    objs
}

#[test]
fn a_match_across_two_blocks_and_one_in_a_form_are_skipped() {
    let d = dir("replace-across");
    let form = "<< /Type /XObject /Subtype /Form /BBox [0 0 200 50] /Resources << /Font << /F1 3 0 R >> >> /Length 40 >>\nstream\nBT /F1 12 Tf 10 10 Td (stamp over) Tj ET\nendstream".to_string();
    let c = "BT /F1 12 Tf 72 700 Td (the first over) Tj ET\nBT /F1 12 Tf 72 500 Td (over the second) Tj ET\nq 1 0 0 1 72 300 cm /Fm1 Do Q";
    let mut objs = pages_pdf_objs(&[c]);
    // Object 7 is the form: the page's content is 6, the next free number after the page (5) and content (6).
    objs[4] = objs[4].replace("/F2 4 0 R >>", "/F2 4 0 R >> /XObject << /Fm1 7 0 R >>");
    objs.push(form);
    let (eng, id, _) = open(&d, &raw_pdf(&objs));
    // The two matches in the page's own blocks are replaced; the one in the form is not (shared by pages).
    let r = do_replace(&eng, id, &req("over", "OVER", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced, 2, "{r:?}");
    assert_eq!(r.skipped_count, 1, "{r:?}");
    assert!(r.skipped[0].reason.contains("form"), "{:?}", r.skipped);
    let r = do_replace(&eng, id, &req("over over", "x", SearchOpts::default())).unwrap();
    // "first OVER" and "OVER the second" now read "OVER OVER" across the gap: in no one block.
    assert_eq!((r.replaced, r.skipped_count), (0, 1), "{r:?}");
    assert!(r.skipped[0].reason.contains("two text blocks"), "{:?}", r.skipped);
    assert!(all_text(&eng, id).contains("the first OVER") && all_text(&eng, id).contains("OVER the second"));
}

#[test]
fn a_certified_document_is_refused_and_a_signed_one_is_saved_after_the_signature() {
    let d = dir("replace-certified");
    let (eng, id, _) = open(&d, &pdf_in("BT /F1 12 Tf 72 700 Td (Hello Hello) Tj ET", &[("F1", HELV)], &[], CERTIFIED));
    let before = rev(&eng, id);
    match do_replace(&eng, id, &req("Hello", "Bye", SearchOpts::default())) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(rev(&eng, id), before, "nothing was recorded");
    assert!(text(&eng, id).contains("Hello Hello"));

    let d = dir("replace-signed");
    let catalog = "/AcroForm << /SigFlags 3 /Fields [6 0 R] >>";
    let (eng, id, path) = open(&d, &pdf_in("BT /F1 12 Tf 72 700 Td (Hello Hello) Tj ET", &[("F1", HELV)], &[SIG_FIELD], catalog));
    let original = std::fs::read(&path).unwrap();
    let r = do_replace(&eng, id, &req("Hello", "Bye", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced, 2);
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "the signed bytes are not rewritten");
    eng.close(id);
    let (eng, id) = reopen(&path);
    assert_eq!(text(&eng, id), "Bye Bye");
}

#[test]
fn an_empty_find_and_a_replacement_with_a_line_break_are_refused() {
    let d = dir("replace-refused");
    let (eng, id, _) = open(&d, &pages_pdf(&[CATS]));
    let before = rev(&eng, id);
    for r in [req("", "x", SearchOpts::default()), req("   ", "x", SearchOpts::default()), req("cat", "a\nb", SearchOpts::default())] {
        assert!(matches!(do_replace(&eng, id, &r), Err(EngineError::Invalid(_))), "{r:?}");
    }
    // A stale rev is refused like any text edit.
    assert!(matches!(replace(&eng, id, before + 7, &req("cat", "x", SearchOpts::default()), EditSource::User), Err(EngineError::Stale)));
    assert_eq!(rev(&eng, id), before);
    // Replacing with nothing deletes the match; a block it would empty is left alone and listed.
    let r = do_replace(&eng, id, &req(" and the cat", "", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced, 1);
    let r = do_replace(&eng, id, &req("The cat sat by a concat.", "", SearchOpts::default())).unwrap();
    assert_eq!((r.replaced, r.skipped_count), (0, 1), "{r:?}");
}

#[test]
fn replace_one_on_a_rotated_page_finds_its_block() {
    let d = dir("replace-rotated");
    let bytes = String::from_utf8(pages_pdf(&["BT /F1 12 Tf 72 700 Td (north east north) Tj ET"]))
        .unwrap()
        .replace("/MediaBox [0 0 612 792]", "/MediaBox [0 0 612 792] /Rotate 90");
    let (eng, id, _) = open(&d, bytes.as_bytes());
    let q = Query::new("north", SearchOpts::default()).unwrap();
    let hits = eng.read_custom(id, |d| crate::search::find_in_page(d, 0, &q)).unwrap();
    assert_eq!(hits.len(), 2);
    let at = ReplaceAt { page: 0, rect: hits[1].rects[0] };
    let r = do_replace(&eng, id, &ReplaceRequest { at: Some(at), ..req("north", "south", SearchOpts::default()) }).unwrap();
    assert_eq!((r.replaced, r.skipped_count), (1, 0), "{r:?}");
    assert_eq!(all_text(&eng, id), "north east south");
}

#[test]
fn a_run_is_capped_stops_with_a_reason_and_is_still_one_step() {
    let d = dir("replace-cap");
    let line = format!("BT /F1 8 Tf 20 700 Td ({}) Tj ET", "x ".repeat(700));
    let (eng, id, _) = open(&d, &pages_pdf(&[&line, &line, &line]));
    let before = all_text(&eng, id);
    let r = do_replace(&eng, id, &req("x", "y", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced as usize, super::replace::MAX_REPLACEMENTS, "{:?}", (r.replaced, &r.stopped));
    assert!(r.stopped.as_deref().is_some_and(|s| s.contains("2000")), "{:?}", r.stopped);
    assert_eq!(r.state.undo.as_deref(), Some("Replace \"x\" with \"y\""));
    assert!(all_text(&eng, id).contains('y'));
    eng.undo(id).unwrap();
    assert_eq!(all_text(&eng, id), before);
}

#[test]
fn a_block_with_a_line_end_hyphen_is_left_alone_unless_a_match_covers_it() {
    let d = dir("replace-hyphen");
    let two = "BT /F1 12 Tf 72 700 Td (A well-) Tj ET\nBT /F1 12 Tf 72 686 Td (known fox and a fox) Tj ET";
    let (eng, id, _) = open(&d, &pages_pdf(&[two]));
    let (before, text) = (rev(&eng, id), all_text(&eng, id));
    // "fox" twice in the block: rewriting it would turn "well-known" into "wellknown".
    let r = do_replace(&eng, id, &req("fox", "dog", SearchOpts::default())).unwrap();
    assert_eq!((r.replaced, r.skipped_count, rev(&eng, id)), (0, 2, before), "{r:?}");
    assert_eq!(r.skipped[0].reason, "skipped: a hyphen at the end of a line in the block would be lost");
    assert_eq!(all_text(&eng, id), text);
    // A match across the hyphen replaces it with the replacement.
    let r = do_replace(&eng, id, &req("wellknown", "famous", SearchOpts::default())).unwrap();
    assert_eq!((r.replaced, r.skipped_count), (1, 0), "{r:?}");
    assert!(all_text(&eng, id).starts_with("A famous"), "{}", all_text(&eng, id));
}

#[test]
fn a_block_whose_text_and_find_hits_disagree_is_left_alone_and_listed() {
    let d = dir("replace-uneven");
    // Find matches "well-known" across the break; the block text, with the hyphen dropped, does not.
    let two = "BT /F1 12 Tf 72 700 Td (A well-) Tj ET\nBT /F1 12 Tf 72 686 Td (known fox) Tj ET";
    let (eng, id, _) = open(&d, &pages_pdf(&[two]));
    let before = rev(&eng, id);
    let r = do_replace(&eng, id, &req("well-known", "famous", SearchOpts::default())).unwrap();
    assert_eq!((r.replaced, r.skipped_count, rev(&eng, id)), (0, 1, before), "{r:?}");
    assert!(r.skipped[0].reason.contains("reads differently"), "{r:?}");
}

#[test]
fn a_whole_word_at_a_block_edge_that_find_does_not_list_is_left_alone() {
    let d = dir("replace-uneven-edge");
    // "cat" + "s" on one line: Find reads "cats", the first block's text ends in the whole word "cat".
    let edge = "BT /F1 12 Tf 72 700 Td (the cat) Tj ET\nBT /F1 12 Tf 108.024 700 Td (s here) Tj ET";
    let (eng, id, _) = open(&d, &pages_pdf(&[edge]));
    let before = rev(&eng, id);
    let whole = SearchOpts { whole_words: true, ..Default::default() };
    let r = do_replace(&eng, id, &req("cat", "dog", whole)).unwrap();
    assert_eq!((r.replaced, r.skipped_count, rev(&eng, id)), (0, 1, before), "{r:?}");
    assert!(r.skipped[0].reason.contains("reads differently"), "{r:?}");
}

#[test]
fn replace_all_stops_between_blocks_when_its_time_is_up() {
    let d = dir("replace-budget");
    let two = "BT /F1 12 Tf 72 700 Td (first cat) Tj ET\nBT /F1 12 Tf 72 500 Td (second cat) Tj ET";
    let (eng, id, _) = open(&d, &pages_pdf(&[two]));
    let r = req("cat", "dog", SearchOpts::default());
    // No time at all: the block that was started is done (a run always gets somewhere), then it stops.
    let first = replace_within(&eng, id, rev(&eng, id), &r, EditSource::User, Limits { budget: std::time::Duration::ZERO, ..Limits::default() }).unwrap();
    assert_eq!((first.replaced, first.skipped_count), (1, 0), "{first:?}");
    assert!(first.stopped.as_deref().is_some_and(|s| s.starts_with("Stopped at page 1")), "{first:?}");
    assert_eq!(all_text(&eng, id), "first cat second dog");
    // The next run carries on with what is left.
    let next = do_replace(&eng, id, &r).unwrap();
    assert_eq!((next.replaced, next.stopped), (1, None));
    assert_eq!(all_text(&eng, id), "first dog second dog");
}

#[test]
fn a_block_cut_by_the_cap_is_checked_for_hyphens_after_the_cut() {
    let d = dir("replace-cap-hyphen");
    // The block at the top has "lk" at its start and across the dropped hyphen of "well-" "known"; below it
    // is one more "lk". With room for two, the lower one is done and the top block is cut to its first match,
    // which would lose the hyphen: the block is left alone, and the next run still has it to do.
    let two = "BT /F1 12 Tf 72 700 Td (lk well-) Tj ET\nBT /F1 12 Tf 72 686 Td (known) Tj ET\nBT /F1 12 Tf 72 400 Td (lk) Tj ET";
    let (eng, id, _) = open(&d, &pages_pdf(&[two]));
    let mut r = req("lk", "XX", SearchOpts::default());
    let cap = Limits { max: 2, ..Limits::default() };
    let out = replace_within(&eng, id, rev(&eng, id), &r, EditSource::User, cap).unwrap();
    assert_eq!((out.replaced, out.skipped_count), (1, 1), "{out:?}");
    assert!(out.stopped.is_some() && out.skipped[0].reason.contains("hyphen"), "{out:?}");
    assert_eq!(out.resume, Some(ReplaceFrom { page: 0, upto: Some(1) }), "{out:?}");
    assert_eq!(all_text(&eng, id), "lk well- known XX");
    // With room, the match across the hyphen covers it and the block is replaced.
    r.from = out.resume;
    let out = replace_within(&eng, id, rev(&eng, id), &r, EditSource::User, Limits::default()).unwrap();
    assert_eq!((out.replaced, out.skipped_count), (2, 0), "{out:?}");
}

#[test]
fn going_on_after_a_stop_replaces_each_match_once_even_when_the_replacement_holds_the_find_text() {
    let d = dir("replace-resume");
    let page0 = "BT /F1 12 Tf 72 700 Td (first cat) Tj ET\nBT /F1 12 Tf 72 500 Td (second cat) Tj ET";
    let page1 = "BT /F1 12 Tf 72 700 Td (third cat) Tj ET";
    let (eng, id, _) = open(&d, &pages_pdf(&[page0, page1]));
    let mut r = req("cat", "cats", SearchOpts::default());
    let none = Limits { budget: std::time::Duration::ZERO, ..Limits::default() };
    // No time: one block per run (the last first), then it says where to go on from.
    let mut seen = Vec::new();
    for _ in 0..3 {
        let out = replace_within(&eng, id, rev(&eng, id), &r, EditSource::User, none).unwrap();
        assert_eq!(out.replaced, 1, "{out:?}");
        seen.push(out.resume);
        r.from = out.resume;
    }
    // Stopped on page 1 with a block to do, then at the start of page 2, then done.
    assert_eq!(seen, vec![Some(ReplaceFrom { page: 0, upto: Some(1) }), Some(ReplaceFrom { page: 1, upto: None }), None]);
    assert_eq!(all_text(&eng, id), "first cats second cats | third cats");
}

#[test]
fn a_block_the_cap_would_cut_is_left_whole_for_the_next_run() {
    let d = dir("replace-resume-cap");
    let (eng, id, _) = open(&d, &pages_pdf(&["BT /F1 12 Tf 72 700 Td (cat cat cat) Tj ET\nBT /F1 12 Tf 72 500 Td (cat) Tj ET"]));
    let mut r = req("cat", "cats", SearchOpts::default());
    // Room for 2: the last block (one match) is done; the first has three and is not started.
    let two = Limits { max: 2, ..Limits::default() };
    let out = replace_within(&eng, id, rev(&eng, id), &r, EditSource::User, two).unwrap();
    assert_eq!((out.replaced, out.skipped_count), (1, 0), "{out:?}");
    assert_eq!(out.resume, Some(ReplaceFrom { page: 0, upto: Some(1) }), "{out:?}");
    assert_eq!(all_text(&eng, id), "cat cat cat cats");
    // The next run does it whole: each "cat" is replaced exactly once.
    r.from = out.resume;
    let out = replace_within(&eng, id, rev(&eng, id), &r, EditSource::User, Limits::default()).unwrap();
    assert_eq!((out.replaced, out.resume), (3, None), "{out:?}");
    assert_eq!(all_text(&eng, id), "cats cats cats cats");
}

#[test]
fn one_block_over_the_cap_is_part_replaced_and_the_rest_listed() {
    let d = dir("replace-over-cap");
    let (eng, id, _) = open(&d, &pages_pdf(&["BT /F1 12 Tf 72 700 Td (cat cat cat) Tj ET"]));
    let two = Limits { max: 2, ..Limits::default() };
    let out = replace_within(&eng, id, rev(&eng, id), &req("cat", "cats", SearchOpts::default()), EditSource::User, two).unwrap();
    assert_eq!((out.replaced, out.skipped_count), (2, 1), "{out:?}");
    assert!(out.skipped[0].reason.contains("more matches than the 2"), "{out:?}");
    // It is done with: going on does not start it again.
    assert_eq!(out.resume, Some(ReplaceFrom { page: 0, upto: Some(0) }), "{out:?}");
    assert_eq!(all_text(&eng, id), "cats cats cat");
}

/// Three lines of different widths in one block: the short ones would join the widest if the block were re-wrapped.
const THREE_LINES: &str = "BT /F1 12 Tf 72 700 Td 14 TL (One cat) Tj T* (Two) Tj T* (Three long hats here) Tj ET";

fn page_lines(eng: &Engine, id: DocId) -> Vec<String> {
    eng.page_text(id, 0).unwrap().iter().map(|l| l.t.trim().to_string()).collect()
}

/// The first block's line boxes: one block of `n` lines is the whole check that nothing joined or split.
fn line_boxes(eng: &Engine, id: DocId) -> Vec<[f32; 4]> {
    let tb = blocks(eng, id, 0).unwrap();
    assert_eq!(tb.blocks.len(), 1, "{:?}", tb.blocks);
    tb.blocks[0].lines.clone()
}

#[test]
fn edit_text_on_an_untouched_block_reflows_its_lines_to_the_widest() {
    // What Edit text sends for one changed word: the block's text with spaces, no line breaks.
    let d = dir("edit-joins");
    let (eng, id, _) = open(&d, &pdf(THREE_LINES, &[("F1", HELV)], &[]));
    assert_eq!(line_boxes(&eng, id).len(), 3);
    set_text(&eng, id, 0, "One dog Two Three long hats here");
    assert!(line_boxes(&eng, id).len() < 3, "{:?}", page_lines(&eng, id));
}

#[test]
fn replace_all_keeps_the_lines_of_a_block_it_changes() {
    let d = dir("replace-keeps-lines");
    let (eng, id, path) = open(&d, &pages_pdf(&[THREE_LINES]));
    let before = line_boxes(&eng, id);
    assert_eq!(before.len(), 3);
    let r = do_replace(&eng, id, &req("cat", "dog", SearchOpts::default())).unwrap();
    assert_eq!((r.replaced, r.skipped_count), (1, 0), "{r:?}");
    let check = |eng: &Engine, id: DocId| {
        assert_eq!(page_lines(eng, id), ["One dog", "Two", "Three long hats here"]);
        let after = line_boxes(eng, id);
        assert_eq!(after.len(), 3);
        for (a, b) in after.iter().zip(&before) {
            assert!((a[1] - b[1]).abs() < 0.5 && (a[0] - b[0]).abs() < 0.5, "{after:?} vs {before:?}");
        }
    };
    check(&eng, id);
    eng.save(id, None).unwrap();
    eng.close(id);
    let (eng, id) = reopen(&path);
    check(&eng, id);
}

#[test]
fn a_longer_replacement_wraps_only_its_own_line() {
    let d = dir("replace-grows-one-line");
    let (eng, id, _) = open(&d, &pages_pdf(&[THREE_LINES]));
    let before = line_boxes(&eng, id);
    // Fits the block's width: that line grows, the others stay where they were.
    do_replace(&eng, id, &req("Two", "Two and a half", SearchOpts::default())).unwrap();
    assert_eq!(page_lines(&eng, id), ["One cat", "Two and a half", "Three long hats here"]);
    let after = line_boxes(&eng, id);
    assert_eq!(after.len(), 3);
    assert!(after.iter().zip(&before).all(|(a, b)| (a[1] - b[1]).abs() < 0.5), "{after:?}");
    // Wider than the block: its overflow wraps onto a line of its own, and the lines below keep theirs.
    eng.undo(id).unwrap();
    do_replace(&eng, id, &req("cat", "cat on a big mat", SearchOpts::default())).unwrap();
    let lines = page_lines(&eng, id);
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines[0].starts_with("One cat on"), "{lines:?}");
    assert_eq!(&lines[2..], ["Two", "Three long hats here"], "{lines:?}");
    assert_eq!(line_boxes(&eng, id).len(), 4);
}

#[test]
fn a_match_across_a_line_end_drops_only_that_break() {
    let d = dir("replace-across-lines");
    let (eng, id, _) = open(&d, &pages_pdf(&[THREE_LINES]));
    let r = do_replace(&eng, id, &req("cat Two", "pet", SearchOpts::default())).unwrap();
    assert_eq!((r.replaced, r.skipped_count), (1, 0), "{r:?}");
    assert_eq!(page_lines(&eng, id), ["One pet", "Three long hats here"]);
}

#[test]
fn a_justified_block_stays_justified_when_one_word_is_replaced() {
    let d = dir("replace-justified");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let long = "Justified text spreads the free width of every line but the last over its spaces, so both edges line up.";
    let change = BlockChange { text: Some(long.into()), align: Some(Align::Justify), width: Some(160.0), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &change, EditSource::User).unwrap();
    let before = blocks(&eng, id, 0).unwrap().blocks[1].lines.clone();
    let n = before.len();
    assert!(n >= 4, "{n} lines");
    // Same length, so no line changes its own width by more than a glyph or two.
    let r = do_replace(&eng, id, &req("both", "each", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced, 1, "{r:?}");
    let b = blocks(&eng, id, 0).unwrap().blocks[1].clone();
    assert_eq!(b.lines.len(), n, "{:?}", page_lines(&eng, id));
    let right = b.lines[0][2];
    assert!(b.lines[..n - 1].iter().all(|l| (l[2] - right).abs() < 0.6), "{:?}", b.lines);
    assert!(b.lines[n - 1][2] < right - 5.0, "the last line is not stretched");
    assert!(b.text.contains("each edges"), "{}", b.text);
}

#[test]
fn lines_set_in_different_styles_keep_their_breaks_too() {
    let d = dir("replace-keeps-mixed-lines");
    let mixed = "BT /F1 12 Tf 72 700 Td 14 TL (One cat) Tj T* /F2 12 Tf (Bold two) Tj T* /F1 12 Tf (Three long hats here) Tj ET";
    let (eng, id, _) = open(&d, &pages_pdf(&[mixed]));
    assert_eq!(line_boxes(&eng, id).len(), 3);
    let r = do_replace(&eng, id, &req("cat", "dog", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced, 1, "{r:?}");
    assert_eq!(page_lines(&eng, id), ["One dog", "Bold two", "Three long hats here"]);
    // A replacement that starts and ends in a space beside a line end: still no panic, nothing misplaced.
    let r = do_replace(&eng, id, &req("dog", " big dog ", SearchOpts::default())).unwrap();
    assert_eq!(r.replaced, 1, "{r:?}");
    assert_eq!(page_lines(&eng, id), ["One big dog", "Bold two", "Three long hats here"]);
}

#[test]
fn skipped_matches_carry_the_words_around_them() {
    let d = dir("replace-skip-context");
    let two = "BT /F1 12 Tf 72 700 Td (A well-) Tj ET\nBT /F1 12 Tf 72 686 Td (known fox and a fox) Tj ET";
    let (eng, id, _) = open(&d, &pages_pdf(&[two]));
    let r = do_replace(&eng, id, &req("fox", "dog", SearchOpts::default())).unwrap();
    let rows: Vec<(&str, &str, &str)> = r.skipped.iter().map(|s| (s.before.as_str(), s.text.as_str(), s.after.as_str())).collect();
    assert_eq!(rows, [("A wellknown ", "fox", " and a fox"), ("fox and a ", "fox", "")], "{r:?}");
}

#[test]
fn a_word_replaced_by_nothing_at_a_line_end_leaves_no_blank_line_and_keeps_the_other_breaks() {
    let d = dir("replace-keeps-lines-emptied");
    let (eng, id, _) = open(&d, &pages_pdf(&[THREE_LINES]));
    do_replace(&eng, id, &req("cat", "", SearchOpts::default())).unwrap();
    assert_eq!(page_lines(&eng, id), ["One", "Two", "Three long hats here"]);
    eng.undo(id).unwrap();
    // A whole line emptied: its neighbours stay lines of their own.
    do_replace(&eng, id, &req("Two", "", SearchOpts::default())).unwrap();
    assert_eq!(page_lines(&eng, id), ["One cat", "Three long hats here"]);
    // The same in a block of lines in different styles.
    let mixed = "BT /F1 12 Tf 72 700 Td 14 TL (One cat) Tj T* /F2 12 Tf (Bold two) Tj T* /F1 12 Tf (Three long hats here) Tj ET";
    let (eng, id, _) = open(&dir("replace-keeps-lines-emptied-mixed"), &pages_pdf(&[mixed]));
    do_replace(&eng, id, &req("Bold two", "", SearchOpts::default())).unwrap();
    assert_eq!(page_lines(&eng, id), ["One cat", "Three long hats here"]);
}

#[test]
fn what_spills_over_from_a_longer_justified_line_goes_on_to_the_next_and_the_block_stays_justified() {
    let d = dir("replace-justified-spill");
    let (eng, id, _) = open(&d, &pdf(TWO_BLOCKS, &[("F1", HELV)], &[]));
    let long = "Justified text spreads the free width of every line but the last over its spaces, so both edges line up.";
    let change = BlockChange { text: Some(long.into()), align: Some(Align::Justify), width: Some(160.0), ..Default::default() };
    edit(&eng, id, rev(&eng, id), 0, 1, &change, EditSource::User).unwrap();
    let check = |eng: &Engine| {
        let b = blocks(eng, id, 0).unwrap().blocks[1].clone();
        let n = b.lines.len();
        assert_eq!(b.align, Align::Justify, "{:?}", b.lines);
        let right = b.lines[0][2];
        assert!(b.lines[..n - 1].iter().all(|l| (l[2] - right).abs() < 0.6), "{:?}", b.lines);
        assert!(b.lines[n - 1][2] < right - 5.0, "the last line is not stretched");
        n
    };
    let n = check(&eng);
    do_replace(&eng, id, &req("spreads", "spreads very far and wide", SearchOpts::default())).unwrap();
    assert!(check(&eng) >= n);
    // Another word replaced later: the block is still read, and set again, as justified.
    do_replace(&eng, id, &req("edges", "sides", SearchOpts::default())).unwrap();
    check(&eng);
    assert!(blocks(&eng, id, 0).unwrap().blocks[1].text.contains("very far and wide"));
}
