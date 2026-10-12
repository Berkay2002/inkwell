use super::*;
use crate::engine::edit::{EditSource, page_ranges, rewrite_opts, write_doc};
use crate::engine::fixture::{self, Fixture};
use crate::engine::{Engine, EngineError};
use mupdf::pdf::PdfDocument;
use mupdf::{Colorspace, Matrix, Rect, TextPageFlags};
use std::path::{Path, PathBuf};

const DATE: Date = Date { year: 2026, month: 10, day: 11 };

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("marks-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// One page, `extra` page keys (`/Rotate 90`, a crop box…), the word "Body" in the middle.
fn one_page(extra: &str) -> Vec<u8> {
    page_with(extra, "BT /F1 12 Tf 250 400 Td (Body) Tj ET")
}

/// One page with `content` as its only stream (Helvetica is /F1).
fn page_with(extra: &str, content: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {extra} /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
    out
}

/// One page whose content is `streams`, one stream each, in order (Helvetica is /F1).
fn page_with_streams(extra: &str, streams: &[&str]) -> Vec<u8> {
    let refs: Vec<String> = (0..streams.len()).map(|i| format!("{} 0 R", i + 5)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {extra} /Resources << /Font << /F1 4 0 R >> >> /Contents [{}] >>", refs.join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    objs.extend(streams.iter().map(|c| format!("<< /Length {} >>\nstream\n{c}\nendstream", c.len())));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
    out
}

fn req(mark: Mark, pages: Vec<u32>) -> AddRequest {
    AddRequest { mark, pages, replace: false, date: DATE }
}

fn hf(slot: usize, text: &str) -> HeaderFooter {
    let mut h = HeaderFooter::default();
    h.text[slot] = text.into();
    h
}

/// Writes the document and reads it back, as a save and reopen does.
fn reopened(doc: &PdfDocument) -> PdfDocument {
    let bytes = write_doc(doc, rewrite_opts(), 1 << 16).unwrap();
    PdfDocument::from_bytes(&bytes).unwrap()
}

fn words(doc: &PdfDocument, page: i32) -> Vec<mupdf::text_page::TextWord> {
    doc.load_page(page).unwrap().to_text_page(TextPageFlags::empty()).unwrap().words()
}

fn word(doc: &PdfDocument, text: &str) -> Rect {
    words(doc, 0).into_iter().find(|w| w.text == text).unwrap_or_else(|| panic!("no word {text:?} in {:?}", words(doc, 0))).bounds
}

fn near(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 4.0, "{what}: {a} is not near {b}");
}

fn text_of(doc: &PdfDocument, page: i32) -> String {
    doc.load_page(page).unwrap().to_text_page(TextPageFlags::empty()).unwrap().to_text().unwrap()
}

/// Share of dark pixels of the page at 72 dpi (greyscale), for "something was drawn here".
fn ink(doc: &PdfDocument, page: i32) -> usize {
    let pm = doc.load_page(page).unwrap().to_pixmap(&Matrix::new_scale(1.0, 1.0), &Colorspace::device_gray(), false, false).unwrap();
    pm.samples().iter().filter(|&&v| v < 200).count()
}

fn coloured(doc: &PdfDocument, page: i32) -> usize {
    let pm = doc.load_page(page).unwrap().to_pixmap(&Matrix::new_scale(1.0, 1.0), &Colorspace::device_gray(), false, false).unwrap();
    pm.samples().iter().filter(|&&v| v < 250).count()
}

#[test]
fn tokens_expand_like_acrobats() {
    let d = DATE;
    assert_eq!(expand("<<1>>", 3, 12, d), "3");
    assert_eq!(expand("Page <<1>> of <<n>>", 3, 12, d), "Page 3 of 12");
    assert_eq!(expand("<<Page 1 of n>>", 3, 12, d), "Page 3 of 12");
    assert_eq!(expand("<<1 of n>> · <<1/n>>", 3, 12, d), "3 of 12 · 3/12");
    assert_eq!(expand("<<mm/dd/yyyy>>", 1, 1, d), "10/11/2026");
    assert_eq!(expand("<<d/m/yy>>", 1, 1, d), "11/10/26");
    assert_eq!(expand("<<yyyy-mm-dd>>", 1, 1, d), "2026-10-11");
    assert_eq!(expand("<<mmmm d, yyyy>>", 1, 1, d), "October 11, 2026");
    // unknown tokens and an unclosed one stay as typed
    assert_eq!(expand("<<what>> <<1", 1, 1, d), "<<what>> <<1");
}

#[test]
fn bates_numbers_pad_and_run_out() {
    let b = Bates { prefix: "ACME-".into(), suffix: "-X".into(), digits: 6, start: 41, ..Bates::default() };
    assert_eq!(bates_text(&b, 41), "ACME-000041-X");
    assert!(bates_fits(3, 1, 999));
    assert!(!bates_fits(3, 1, 1000));
    assert!(!bates_fits(3, 998, 3));
    assert!(bates_fits(15, 1, 1_000_000));
    assert_eq!(page_ranges(&[0, 1, 2, 5]), "pp. 1-3, 6");
}

#[test]
fn text_lands_where_asked_on_every_rotation_and_box() {
    // (page keys, display width, display height)
    let cases = [
        ("", 612.0, 792.0),
        ("/Rotate 90", 792.0, 612.0),
        ("/Rotate 180", 612.0, 792.0),
        ("/Rotate 270", 792.0, 612.0),
        ("/CropBox [50 60 450 700]", 400.0, 640.0),
        ("/Rotate 90 /CropBox [50 60 450 700]", 640.0, 400.0),
    ];
    for (extra, w, h) in cases {
        let mut doc = PdfDocument::from_bytes(&one_page(extra)).unwrap();
        let mut m = HeaderFooter::default();
        m.text = ["TL".into(), "TC".into(), "TR".into(), "BL".into(), "BC".into(), "BR".into()];
        add(&mut doc, &req(Mark::HeaderFooter(m.clone()), vec![0])).unwrap();
        let doc = reopened(&doc);
        let [top, bottom, left, right] = m.margins;
        let (tl, tc, tr) = (word(&doc, "TL"), word(&doc, "TC"), word(&doc, "TR"));
        let (bl, bc, br) = (word(&doc, "BL"), word(&doc, "BC"), word(&doc, "BR"));
        let ctx = |s: &str| format!("{extra:?} {s}");
        near(tl.x0, left as f32, &ctx("TL left"));
        near(bl.x0, left as f32, &ctx("BL left"));
        near(tr.x1, (w - right) as f32, &ctx("TR right"));
        near(br.x1, (w - right) as f32, &ctx("BR right"));
        near((tc.x0 + tc.x1) / 2.0, (w / 2.0) as f32, &ctx("TC centre"));
        near((bc.x0 + bc.x1) / 2.0, (w / 2.0) as f32, &ctx("BC centre"));
        // y is down in MuPDF's page space
        near(tl.y0, top as f32, &ctx("top"));
        near(bl.y1, (h - bottom) as f32, &ctx("bottom"));
        // the page reads the same: the body is still where it was, and first
        assert!(text_of(&doc, 0).trim_start().starts_with("Body"), "{extra:?}: {:?}", text_of(&doc, 0));
        let b = word(&doc, "Body");
        assert!(b.x0 > 0.0 && b.y0 > 0.0, "{extra:?}");
    }
}

#[test]
fn marks_are_artifacts_in_their_own_streams() {
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(5, "Page <<1>> of <<n>>")), vec![0])).unwrap();
    add(
        &mut doc,
        &req(Mark::Watermark(Watermark { text: "DRAFT".into(), behind: false, ..Watermark::default() }), vec![0]),
    )
    .unwrap();
    add(&mut doc, &req(Mark::Background(Background::default()), vec![0])).unwrap();
    let doc = reopened(&doc);
    let page = doc.find_page(0).unwrap();
    let list = page.get_dict("Contents").unwrap().unwrap();
    // background, wrap open, original, wrap close, header/footer, watermark
    assert_eq!(list.len().unwrap(), 6);
    let mut tags = Vec::new();
    for i in 0..6 {
        let s = list.get_array(i).unwrap().unwrap();
        tags.push(s.get_dict("PCMark").unwrap().and_then(|v| v.as_name().ok()).map(|n| String::from_utf8(n).unwrap()));
    }
    assert_eq!(
        tags,
        [Some("Background".into()), Some("Wrap".into()), None, Some("Wrap".into()), Some("HeaderFooter".into()), Some("Watermark".into())]
    );
    let hfs = list.get_array(4).unwrap().unwrap().read_stream().unwrap();
    let hfs = String::from_utf8_lossy(&hfs).into_owned();
    assert!(hfs.contains("/Artifact <</Type /Pagination /Subtype /Footer /Attached [/Bottom] /PCMark /HeaderFooter>> BDC"), "{hfs}");
    assert!(hfs.contains("(Page 1 of 1) Tj"), "{hfs}");
    let wm = String::from_utf8_lossy(&list.get_array(5).unwrap().unwrap().read_stream().unwrap()).into_owned();
    assert!(wm.contains("/Subtype /Watermark"), "{wm}");
    assert!(wm.contains("/IWGS50 gs"), "{wm}");
}

#[test]
fn update_replaces_and_remove_restores_the_page() {
    let original = PdfDocument::from_bytes(&one_page("")).unwrap();
    let before = text_of(&original, 0);
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(0, "First")), vec![0])).unwrap();
    // Update: same kind again, replacing
    let mut r = req(Mark::HeaderFooter(hf(0, "Second")), vec![0]);
    r.replace = true;
    add(&mut doc, &r).unwrap();
    let text = text_of(&doc, 0);
    assert!(text.contains("Second") && !text.contains("First"), "{text:?}");
    assert_eq!(info(&doc).unwrap().kinds.len(), 1);
    assert_eq!(doc.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap().len().unwrap(), 4);

    // Remove: the page is as it was, the wrapper gone
    assert_eq!(remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), 1);
    assert_eq!(info(&doc).unwrap(), Info::default());
    assert_eq!(text_of(&doc, 0), before);
    let contents = doc.find_page(0).unwrap().get_dict("Contents").unwrap().unwrap();
    assert_eq!(contents.len().unwrap_or(1), 1);
    // removing again finds nothing
    assert_eq!(remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), 0);
}

#[test]
fn only_the_asked_kind_and_pages_are_removed() {
    let bytes = fixture::build(&Fixture { pages: 3, lines: 2, ..Default::default() });
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(3, "HF")), vec![0, 1, 2])).unwrap();
    add(&mut doc, &req(Mark::Watermark(Watermark { text: "WM".into(), ..Watermark::default() }), vec![1])).unwrap();
    let i = info(&doc).unwrap();
    assert_eq!(i.kinds.iter().find(|k| k.kind == MarkKind::HeaderFooter).unwrap().pages, [0, 1, 2]);
    assert_eq!(i.kinds.iter().find(|k| k.kind == MarkKind::Watermark).unwrap().pages, [1]);
    remove(&mut doc, MarkKind::HeaderFooter, &[1]).unwrap();
    let i = info(&doc).unwrap();
    assert_eq!(i.kinds.iter().find(|k| k.kind == MarkKind::HeaderFooter).unwrap().pages, [0, 2]);
    // the watermark on page 2 survives its page's header being removed
    assert_eq!(i.kinds.iter().find(|k| k.kind == MarkKind::Watermark).unwrap().pages, [1]);
}

#[test]
fn page_numbers_count_from_the_start_number_over_the_selection() {
    let bytes = fixture::build(&Fixture { pages: 4, lines: 2, ..Default::default() });
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    let mut m = hf(4, "<<Page 1 of n>>");
    m.start_number = 7;
    // odd pages only: pages 1 and 3 (0-based 0 and 2)
    add(&mut doc, &req(Mark::HeaderFooter(m), vec![0, 2])).unwrap();
    assert!(text_of(&doc, 0).contains("Page 7 of 4"), "{:?}", text_of(&doc, 0));
    assert!(text_of(&doc, 2).contains("Page 8 of 4"), "{:?}", text_of(&doc, 2));
    assert!(!text_of(&doc, 1).contains("Page "), "{:?}", text_of(&doc, 1));
}

#[test]
fn settings_round_trip_through_the_file() {
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    let mut m = hf(1, "Café <<1>> — draft");
    m.style = Style { family: Family::Times, bold: true, italic: true, size: 14.0, color: [0.2, 0.4, 0.6], underline: true };
    m.margins = [20.0, 30.0, 40.0, 50.0];
    m.start_number = 5;
    add(&mut doc, &req(Mark::HeaderFooter(m.clone()), vec![0])).unwrap();
    let doc = reopened(&doc);
    let got = info(&doc).unwrap();
    assert_eq!(got.kinds.len(), 1);
    let mark: Mark = serde_json::from_value(got.kinds[0].mark.clone().unwrap()).unwrap();
    assert_eq!(mark, Mark::HeaderFooter(m));
    // and the text is drawn with its accent and the dash (WinAnsi)
    assert!(text_of(&doc, 0).contains("Café 5 — draft"), "{:?}", text_of(&doc, 0));
}

#[test]
fn bad_requests_are_refused_before_anything_is_drawn() {
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    let refuse = |doc: &mut PdfDocument, mark: Mark, pages: Vec<u32>| -> String {
        match add(doc, &req(mark, pages)) {
            Err(EngineError::Invalid(m)) => m,
            Err(EngineError::BadPage(p)) => format!("bad page {p}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    };
    assert!(refuse(&mut doc, Mark::HeaderFooter(HeaderFooter::default()), vec![0]).contains("Type the header"));
    assert!(refuse(&mut doc, Mark::HeaderFooter(hf(0, "日本語")), vec![0]).contains("can't draw"));
    assert_eq!(refuse(&mut doc, Mark::HeaderFooter(hf(0, "x")), vec![3]), "bad page 3");
    assert!(refuse(&mut doc, Mark::HeaderFooter(hf(0, "x")), vec![]).contains("no pages"));
    let mut neg = hf(0, "x");
    neg.margins[0] = -1.0;
    assert!(refuse(&mut doc, Mark::HeaderFooter(neg), vec![0]).contains("margins"));
    assert!(refuse(&mut doc, Mark::Watermark(Watermark::default()), vec![0]).contains("watermark text"));
    let hidden = Watermark { text: "x".into(), show_on_screen: false, show_in_print: false, ..Watermark::default() };
    assert!(refuse(&mut doc, Mark::Watermark(hidden), vec![0]).contains("screen"));
    let few = Bates { digits: 3, start: 999, ..Bates::default() };
    // one page at 999 fits; 2 pages would not (checked on a two-page file below)
    assert!(add(&mut doc, &req(Mark::Bates(few), vec![0])).is_ok());
}

#[test]
fn nothing_is_half_drawn_when_a_later_page_fails() {
    let bytes = fixture::build(&Fixture { pages: 2, lines: 2, ..Default::default() });
    let mut doc = PdfDocument::from_bytes(&bytes).unwrap();
    // 2 pages from 999 with 3 digits runs out on the second: refused up front
    let b = Bates { digits: 3, start: 999, ..Bates::default() };
    let e = add(&mut doc, &req(Mark::Bates(b), vec![0, 1])).unwrap_err();
    assert!(matches!(e, EngineError::Invalid(ref m) if m.contains("digits")), "{e:?}");
    assert_eq!(info(&doc).unwrap(), Info::default());
}

#[test]
fn watermark_draws_and_its_screen_toggle_hides_it() {
    let blank = PdfDocument::from_bytes(&one_page("")).unwrap();
    let base = ink(&blank, 0);
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    let wm = Watermark { text: "CONFIDENTIAL".into(), style: Style { color: [0.0; 3], ..Watermark::default().style }, opacity: 1.0, ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(wm.clone()), vec![0])).unwrap();
    assert!(ink(&doc, 0) > base + 400, "{} vs {}", ink(&doc, 0), base);
    let drawn = ink(&doc, 0);

    // not on screen, in print only: the viewer's render hides it
    let mut hidden = PdfDocument::from_bytes(&one_page("")).unwrap();
    add(&mut hidden, &req(Mark::Watermark(Watermark { show_on_screen: false, show_in_print: true, ..wm }), vec![0])).unwrap();
    assert!(ink(&hidden, 0) < drawn, "{} vs {drawn}", ink(&hidden, 0));
    assert!(ink(&hidden, 0) <= base + 10, "{} vs {base}", ink(&hidden, 0));
    // a layer exists for it, and goes with the mark
    let cat = hidden.catalog().unwrap();
    assert!(cat.get_dict("OCProperties").unwrap().is_some());
    remove(&mut hidden, MarkKind::Watermark, &[0]).unwrap();
    let props = hidden.catalog().unwrap().get_dict("OCProperties").unwrap().unwrap();
    assert_eq!(props.get_dict("OCGs").unwrap().unwrap().len().unwrap(), 0);
}

#[test]
fn background_colour_fills_the_page_behind_the_text() {
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    add(&mut doc, &req(Mark::Background(Background { color: [0.5, 0.5, 0.5], ..Background::default() }), vec![0])).unwrap();
    // the whole page is grey (value 128 < 200), and the body text is still read
    assert!(ink(&doc, 0) > 612 * 792 * 9 / 10);
    assert!(text_of(&doc, 0).contains("Body"));
    assert!(coloured(&doc, 0) > 612 * 792 * 9 / 10);
}

/// A PNG of a solid square, for picture marks.
fn png(path: &Path) {
    let img = image::RgbImage::from_pixel(40, 20, image::Rgb([0, 0, 0]));
    img.save_with_format(path, image::ImageFormat::Png).unwrap();
}

#[test]
fn picture_marks_from_an_image_and_from_a_pdf_page() {
    let d = dir("picture");
    let p = d.join("logo.png");
    png(&p);
    let other = d.join("stamp.pdf");
    std::fs::write(&other, one_page("/Rotate 90")).unwrap();

    let blank = ink(&PdfDocument::from_bytes(&one_page("")).unwrap(), 0);
    for (file, page) in [(p.to_string_lossy().into_owned(), 0), (other.to_string_lossy().into_owned(), 0)] {
        let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
        let w = Watermark {
            picture: Some(Picture { file: file.clone(), page, password: None }),
            opacity: 1.0,
            rotation: 0.0,
            scale: 0.5,
            ..Watermark::default()
        };
        add(&mut doc, &req(Mark::Watermark(w), vec![0])).unwrap();
        let doc = reopened(&doc);
        // the page of the other file has only a few words, drawn at half size
        assert!(ink(&doc, 0) > blank + 10, "{file}: {} vs {blank}", ink(&doc, 0));
        // the image became one XObject on the page, named by its object number
        let res = doc.load_pdf_page(0).unwrap().resources().unwrap();
        assert!(res.get_dict("XObject").unwrap().is_some());
    }
    // a file that is neither is refused
    std::fs::write(d.join("x.png"), b"not a picture").unwrap();
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    let w = Watermark { picture: Some(Picture { file: d.join("x.png").to_string_lossy().into_owned(), page: 0, password: None }), ..Watermark::default() };
    assert!(matches!(add(&mut doc, &req(Mark::Watermark(w), vec![0])), Err(EngineError::Invalid(_))));
}

#[test]
fn edit_core_steps_undo_save_and_reopen() {
    let d = dir("core");
    let src = d.join("a.pdf");
    std::fs::write(&src, fixture::build(&Fixture { pages: 3, lines: 2, ..Default::default() })).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(src.to_str().unwrap()).unwrap().doc;

    let m = req(Mark::HeaderFooter(hf(5, "ZZMARK <<1>>")), vec![0, 1, 2]);
    let s = eng.apply_custom(id, "Add header and footer to pp. 1-3".into(), EditSource::User, false, |doc| add(doc, &m)).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Add header and footer to pp. 1-3"));
    assert!(s.dirty);
    assert!(eng.page_text(id, 1).unwrap().iter().any(|l| l.t.contains("ZZMARK 2")));
    assert!(eng.read_custom(id, |doc| info(doc)).unwrap().kinds.iter().any(|k| k.kind == MarkKind::HeaderFooter));

    // one undo takes all three pages back; redo puts them again
    let s = eng.undo(id).unwrap();
    assert!(s.undo.is_none());
    assert!(!eng.page_text(id, 1).unwrap().iter().any(|l| l.t.contains("ZZMARK")));
    eng.redo(id).unwrap();
    assert!(eng.page_text(id, 2).unwrap().iter().any(|l| l.t.contains("ZZMARK 3")));

    // save, close, reopen: the marks are in the file and Update can read them
    let out = d.join("b.pdf");
    eng.save(id, Some(out.to_str().unwrap())).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(out.to_str().unwrap()).unwrap().doc;
    assert!(eng2.page_text(id2, 0).unwrap().iter().any(|l| l.t.contains("ZZMARK 1")));
    let i = eng2.read_custom(id2, |doc| info(doc)).unwrap();
    assert_eq!(i.kinds.len(), 1);
    assert_eq!(i.kinds[0].pages, [0, 1, 2]);

    // update on the reopened file, then remove it: the saved bytes no longer hold the old text
    let mut up = req(Mark::HeaderFooter(hf(5, "YYNEW <<1>>")), vec![0, 1, 2]);
    up.replace = true;
    eng2.apply_removing(id2, "Update header and footer on pp. 1-3".into(), EditSource::User, |doc| add(doc, &up)).unwrap();
    eng2.apply_removing(id2, "Remove header and footer".into(), EditSource::User, |doc| remove(doc, MarkKind::HeaderFooter, &[0, 1, 2]).map(|_| vec![0, 1, 2])).unwrap();
    let c = d.join("c.pdf");
    eng2.save(id2, Some(c.to_str().unwrap())).unwrap();
    let bytes = std::fs::read(&c).unwrap();
    let hay = String::from_utf8_lossy(&bytes);
    assert!(!hay.contains("ZZMARK") && !hay.contains("YYNEW"));
    let doc = PdfDocument::open(c.to_str().unwrap()).unwrap();
    assert_eq!(info(&doc).unwrap(), Info::default());
}

#[test]
fn bates_runs_across_files_and_never_overwrites() {
    let d = dir("bates");
    let a = d.join("a.pdf");
    let b = d.join("b.pdf");
    std::fs::write(&a, fixture::build(&Fixture { pages: 3, lines: 2, ..Default::default() })).unwrap();
    std::fs::write(&b, fixture::build(&Fixture { pages: 2, lines: 2, ..Default::default() })).unwrap();
    let out = d.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let bates = Bates { prefix: "CASE-".into(), digits: 6, start: 98, ..Bates::default() };
    let files = vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
    let done = bates_files(&files, &bates, DATE, &out, &|_| Ok(())).unwrap();
    assert_eq!(done.len(), 2);
    assert_eq!((done[0].first, done[0].last, done[0].pages), (98, 100, 3));
    assert_eq!((done[1].first, done[1].last, done[1].pages), (101, 102, 2));
    let second = PdfDocument::open(&done[1].output).unwrap();
    assert!(text_of(&second, 1).contains("CASE-000102"), "{:?}", text_of(&second, 1));
    let first = PdfDocument::open(&done[0].output).unwrap();
    assert!(text_of(&first, 0).contains("CASE-000098"));
    // the originals are untouched
    assert_eq!(info(&PdfDocument::open(a.to_str().unwrap()).unwrap()).unwrap(), Info::default());
    // a second run writes new names
    let again = bates_files(&files, &bates, DATE, &out, &|_| Ok(())).unwrap();
    assert_ne!(again[0].output, done[0].output);
    assert!(Path::new(&done[0].output).exists());
    // running out of digits writes nothing
    let few = Bates { digits: 3, start: 998, ..bates };
    let before = std::fs::read_dir(&out).unwrap().count();
    assert!(matches!(bates_files(&files, &few, DATE, &out, &|_| Ok(())), Err(EngineError::Invalid(_))));
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), before);
}

#[test]
fn bates_refuses_an_output_that_is_open_before_writing_anything() {
    let d = dir("bates-open");
    let one = d.join("one");
    let two = d.join("two");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::create_dir_all(&two).unwrap();
    // two inputs with the same file name, from different folders
    let a = one.join("same.pdf");
    let b = two.join("same.pdf");
    std::fs::write(&a, fixture::build(&Fixture { pages: 1, lines: 2, ..Default::default() })).unwrap();
    std::fs::write(&b, fixture::build(&Fixture { pages: 1, lines: 2, ..Default::default() })).unwrap();
    let out = d.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let files = vec![a.to_string_lossy().into_owned(), b.to_string_lossy().into_owned()];
    let bates = Bates::default();
    // the second copy is "open": the run stops before the first is written
    let second = out.join("same - Bates (2).pdf");
    let refuse = |p: &Path| {
        if p == second { Err(EngineError::Invalid("that file is open in Inkwell; close it first".into())) } else { Ok(()) }
    };
    assert!(matches!(bates_files(&files, &bates, DATE, &out, &refuse), Err(EngineError::Invalid(_))));
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "nothing written");
    // without the refusal, same-named inputs get distinct copies within one run
    let done = bates_files(&files, &bates, DATE, &out, &|_| Ok(())).unwrap();
    assert_ne!(done[0].output, done[1].output);
    assert!(second.exists());
}

// ---- review fixes: shared resources, unbalanced q/Q, inline marks, resource clean-up -----------------

/// A document with `pages` pages whose content is `content`. `shared` puts one indirect Resources
/// object under every page; otherwise the pages have none at all (and no inherited ones).
fn many(pages: usize, content: &str, shared: bool) -> Vec<u8> {
    // 1 catalog, 2 pages, 3 font, 4 shared resources, then (page, content) pairs from 5.
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", (0..pages).map(|i| format!("{} 0 R", 5 + i * 2)).collect::<Vec<_>>().join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Font << /F1 3 0 R >> >>".into(),
    ];
    for i in 0..pages {
        let res = if shared { "/Resources 4 0 R" } else { "" };
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {res} /Contents {} 0 R >>", 6 + i * 2));
        objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
    out
}

fn category_names(doc: &PdfDocument, page: i32, category: &str) -> Vec<String> {
    let p = doc.find_page(page).unwrap();
    let Some(res) = p.get_dict_inheritable("Resources").unwrap() else { return vec![] };
    let Some(cat) = res.get_dict(category).unwrap() else { return vec![] };
    (0..cat.dict_len().unwrap())
        .filter_map(|k| cat.get_dict_key(k as i32).unwrap().and_then(|n| n.as_name().ok()))
        .map(|n| String::from_utf8_lossy(&n).into_owned())
        .collect()
}

fn group_count(doc: &PdfDocument) -> usize {
    let catalog = doc.catalog().unwrap();
    catalog
        .get_dict("OCProperties")
        .unwrap()
        .and_then(|p| p.get_dict("OCGs").unwrap())
        .map(|a| a.len().unwrap())
        .unwrap_or(0)
}

#[test]
fn marks_leave_shared_resources_alone_and_keep_each_pages_layer() {
    let mut doc = PdfDocument::from_bytes(&many(3, "BT /F1 12 Tf 250 400 Td (Body) Tj ET", true)).unwrap();
    // pages 1-3 share one Resources object; a print-only watermark on pages 1-2
    let print_only = Watermark { text: "PRINT".into(), show_on_screen: false, show_in_print: true, ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(print_only), vec![0, 1])).unwrap();
    // page 3 shares the object and was not touched: it has no marks' resources
    assert_eq!(category_names(&doc, 2, "Font"), vec!["F1".to_string()]);
    assert!(category_names(&doc, 2, "Properties").is_empty());
    // a second, screen-only watermark on page 2 only
    let screen_only = Watermark { text: "SCREEN".into(), show_on_screen: true, show_in_print: false, ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(screen_only), vec![1])).unwrap();
    assert_eq!(group_count(&doc), 2);
    let p0 = category_names(&doc, 0, "Properties");
    let p1 = category_names(&doc, 1, "Properties");
    assert_eq!(p0.len(), 1, "{p0:?}");
    assert_eq!(p1.len(), 2, "{p1:?}");
    assert!(p0.iter().all(|n| p1.contains(n)), "page 1's layer is still page 2's first one");
    // taking the watermarks off page 2 leaves page 1's layer and its group alone
    remove(&mut doc, MarkKind::Watermark, &[1]).unwrap();
    let doc = reopened(&doc);
    assert_eq!(category_names(&doc, 0, "Properties"), p0);
    assert!(category_names(&doc, 1, "Properties").is_empty());
    assert_eq!(group_count(&doc), 1, "the screen-only group went, the print-only one stays");
    // (a print-only layer is hidden on screen, so its text is not in the page text either way)
    assert!(text_of(&doc, 0).contains("Body") && text_of(&doc, 1).contains("Body"));
    assert!(!text_of(&doc, 1).contains("SCREEN"));
}

#[test]
fn a_page_with_no_resources_gets_its_own() {
    let mut doc = PdfDocument::from_bytes(&many(1, "0 0 m 10 10 l S", false)).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(5, "Footer")), vec![0])).unwrap();
    let w = Watermark { text: "COPY".into(), opacity: 0.5, ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(w), vec![0])).unwrap();
    let doc = reopened(&doc);
    assert!(text_of(&doc, 0).contains("Footer") && text_of(&doc, 0).contains("COPY"));
}

#[test]
fn marks_start_from_a_clean_state_after_unbalanced_content() {
    // the page's own content opens a q and a translation and never closes them
    let unclosed = "q 1 0 0 1 100 100 cm 1 0 0 rg BT /F1 12 Tf 250 400 Td (Body) Tj ET";
    let mut doc = PdfDocument::from_bytes(&many(1, unclosed, true)).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(0, "TL")), vec![0])).unwrap();
    let doc = reopened(&doc);
    let tl = word(&doc, "TL");
    near(tl.x0, 72.0, "TL left, not shifted by the page's own cm");
    near(tl.y0, 36.0, "TL top");
    // and the page's own content is still where it was
    near(word(&doc, "Body").x0, 350.0, "Body");

    // a stray Q in the page's content does not pop the wrapper
    let stray = "Q Q BT /F1 12 Tf 250 400 Td (Body) Tj ET";
    let mut doc = PdfDocument::from_bytes(&many(1, stray, true)).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(0, "TL")), vec![0])).unwrap();
    let doc = reopened(&doc);
    near(word(&doc, "TL").x0, 72.0, "TL left after stray Q");
    // the wrapper balances: its streams hold as many q as Q
    let page = doc.find_page(0).unwrap();
    let mut q = 0i64;
    for s in super::apply::contents_list(&page).unwrap() {
        for op in crate::textedit::content::parse(&s.read_stream().unwrap()) {
            match op.op.as_slice() {
                b"q" => q += 1,
                b"Q" => q -= 1,
                _ => {}
            }
        }
    }
    assert_eq!(q, 0);
}

/// What autotagging and text editing do to a page: all its streams become one stream of their own.
fn merge_streams(doc: &mut PdfDocument, index: u32) {
    let mut page = doc.find_page(index as i32).unwrap();
    let mut data = Vec::new();
    for s in super::apply::contents_list(&page).unwrap() {
        data.extend(s.read_stream().unwrap());
        data.push(b'\n');
    }
    let dict = doc.new_dict().unwrap();
    let merged = doc.add_stream(&mupdf::Buffer::from_bytes(&data).unwrap(), Some(&dict), false).unwrap();
    page.dict_put("Contents", merged).unwrap();
}

#[test]
fn marks_are_found_and_removed_after_the_streams_were_rewritten() {
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(5, "Footer text")), vec![0])).unwrap();
    add(&mut doc, &req(Mark::Watermark(Watermark { text: "SECRET".into(), ..Watermark::default() }), vec![0])).unwrap();
    merge_streams(&mut doc, 0);
    let before = info(&doc).unwrap();
    let kinds: Vec<MarkKind> = before.kinds.iter().map(|k| k.kind).collect();
    assert!(kinds.contains(&MarkKind::HeaderFooter) && kinds.contains(&MarkKind::Watermark), "{kinds:?}");
    assert_eq!(remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), 1);
    let doc = reopened(&doc);
    let text = text_of(&doc, 0);
    assert!(!text.contains("Footer text") && text.contains("SECRET") && text.contains("Body"), "{text:?}");
    let after = info(&doc).unwrap();
    assert_eq!(after.kinds.len(), 1);
    assert_eq!(after.kinds[0].kind, MarkKind::Watermark);
}

#[test]
fn removing_a_mark_takes_its_resources_with_it() {
    let mut doc = PdfDocument::from_bytes(&many(2, "BT /F1 12 Tf 250 400 Td (Body) Tj ET", true)).unwrap();
    let w = Watermark { text: "COPY".into(), opacity: 0.5, ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(w), vec![0])).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(5, "Footer")), vec![0])).unwrap();
    assert!(category_names(&doc, 0, "ExtGState").iter().any(|n| n.starts_with("IWGS")));
    remove(&mut doc, MarkKind::Watermark, &[0]).unwrap();
    // the footer still draws with its font; the opacity state is gone
    assert!(category_names(&doc, 0, "Font").iter().any(|n| n.starts_with("IWH")));
    assert!(!category_names(&doc, 0, "ExtGState").iter().any(|n| n.starts_with("IWGS")));
    remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap();
    assert_eq!(category_names(&doc, 0, "Font"), vec!["F1".to_string()]);
    let doc = reopened(&doc);
    assert!(text_of(&doc, 0).contains("Body"));
}

#[test]
fn the_file_remembers_a_picture_by_its_name_only() {
    let d = dir("picture-name");
    let p = d.join("logo.png");
    png(&p);
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    let w = Watermark { picture: Some(Picture { file: p.to_string_lossy().into_owned(), page: 0, password: Some("secret".into()) }), ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(w), vec![0])).unwrap();
    let bytes = write_doc(&doc, rewrite_opts(), 1 << 16).unwrap();
    let hay = String::from_utf8_lossy(&bytes);
    assert!(!hay.contains("inkwell-tests") && !hay.contains(&*d.to_string_lossy()), "the folder is not written");
    assert!(!hay.contains("secret"));
    let found = info(&PdfDocument::from_bytes(&bytes).unwrap()).unwrap();
    let mark = found.kinds[0].mark.clone().unwrap();
    assert_eq!(mark["picture"]["file"], "logo.png");
}

// ---- shrink to fit the margins ----------------------------------------------------------------------

/// Words in the four corners of the user-space box `[x0, y0, x1, y1]`, 12 pt, a little inside it.
fn corners(b: [f64; 4]) -> String {
    let [x0, y0, x1, y1] = b;
    let at = |x: f64, y: f64, t: &str| format!("BT /F1 12 Tf {x} {y} Td ({t}) Tj ET\n");
    [at(x0 + 4.0, y0 + 5.0, "LL"), at(x0 + 4.0, y1 - 15.0, "UL"), at(x1 - 24.0, y0 + 5.0, "LR"), at(x1 - 24.0, y1 - 15.0, "UR")].concat()
}

/// A header and a footer text, so both bands are used.
fn top_and_bottom(shrink: bool) -> HeaderFooter {
    let mut h = HeaderFooter::default();
    h.text[1] = "TC".into();
    h.text[4] = "BC".into();
    h.shrink = shrink;
    h
}

fn inside(b: Rect, bounds: [f32; 4], what: &str) {
    let [l, t, r, d] = bounds;
    assert!(b.x0 >= l - 2.0 && b.x1 <= r + 2.0 && b.y0 >= t - 2.0 && b.y1 <= d + 2.0, "{what}: {b:?} is not inside {bounds:?}");
}

/// Dark pixels of the page at 72 dpi inside `[x0, y0, x1, y1]` (page pixels, y down).
fn ink_in(doc: &PdfDocument, page: i32, [x0, y0, x1, y1]: [usize; 4]) -> usize {
    let pm = doc.load_page(page).unwrap().to_pixmap(&Matrix::new_scale(1.0, 1.0), &Colorspace::device_gray(), false, false).unwrap();
    let (w, n) = (pm.width() as usize, pm.n() as usize);
    let s = pm.samples();
    let mut count = 0;
    for y in y0..y1.min(pm.height() as usize) {
        for x in x0..x1.min(w) {
            if s[(y * w + x) * n] < 200 {
                count += 1;
            }
        }
    }
    count
}

/// How many of the page's streams' bytes say `needle`.
fn stream_count(doc: &PdfDocument, page: i32, needle: &str) -> usize {
    let p = doc.find_page(page).unwrap();
    super::apply::contents_list(&p)
        .unwrap()
        .iter()
        .map(|s| String::from_utf8_lossy(&s.read_stream().unwrap()).matches(needle).count())
        .sum()
}

#[test]
fn shrink_puts_the_content_between_the_bands_on_every_rotation_and_box() {
    // (page keys, the visible box in user space, display width, display height)
    let cases = [
        ("", [0.0, 0.0, 612.0, 792.0], 612.0, 792.0),
        ("/Rotate 90", [0.0, 0.0, 612.0, 792.0], 792.0, 612.0),
        ("/Rotate 270", [0.0, 0.0, 612.0, 792.0], 792.0, 612.0),
        ("/CropBox [50 60 450 700]", [50.0, 60.0, 450.0, 700.0], 400.0, 640.0),
        ("/Rotate 90 /CropBox [50 60 450 700]", [50.0, 60.0, 450.0, 700.0], 640.0, 400.0),
    ];
    for (extra, visible, w, h) in cases {
        let mut doc = PdfDocument::from_bytes(&page_with(extra, &corners(visible))).unwrap();
        let unshrunk = reopened(&doc);
        add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
        let doc = reopened(&doc);
        // the top and bottom bands: margin + one line; across, the content is the page scaled about its centre
        let scale = (h - 96.0) / h;
        let l = (w * (1.0 - scale) / 2.0) as f32;
        let band: [f32; 4] = [l, 48.0, w as f32 - l, (h - 48.0) as f32];
        for name in ["LL", "UL", "LR", "UR"] {
            inside(word(&doc, name), band, &format!("{extra:?} {name}"));
        }
        // it kept the page size and the text above and below does not touch the content
        assert_eq!(doc.load_page(0).unwrap().bounds().unwrap().width() as f64, w, "{extra:?}");
        assert!(word(&doc, "TC").y1 <= word(&doc, "UL").y0 + 1.0, "{extra:?}: header over the content");
        // (word boxes carry the font's ascent and descent; the content sits right at the band's edge)
        assert!(word(&doc, "BC").y0 >= word(&doc, "LL").y1 - 4.0, "{extra:?}: footer over the content");
        // never up: a corner word that already fits does not grow
        let before = word(&unshrunk, "UL");
        let after = word(&doc, "UL");
        assert!(after.x1 - after.x0 <= before.x1 - before.x0 + 0.01, "{extra:?}: scaled up");
        // the page's own text is still read first
        assert!(!text_of(&doc, 0).trim_start().starts_with("TC"), "{extra:?}");
    }
}

#[test]
fn shrink_with_nothing_to_clear_does_not_scale_up() {
    // margins of 0 and one line of text at the bottom: the box is the page less 12 pt.
    let mut h = HeaderFooter::default();
    h.text[4] = "BC".into();
    h.margins = [0.0, 0.0, 0.0, 0.0];
    h.shrink = true;
    let mut doc = PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
    let before = word(&reopened(&doc), "UR");
    add(&mut doc, &req(Mark::HeaderFooter(h), vec![0])).unwrap();
    let doc = reopened(&doc);
    // scaled by (792 - 12) / 792 and no more: width is not a constraint with margins of 0
    let ratio = (word(&doc, "UR").x1 - word(&doc, "UR").x0) / (before.x1 - before.x0);
    assert!((0.97..=0.995).contains(&ratio), "{ratio}");
    // the word boxes include the font's ascent and descent (the footer's sits half off the page
    // with a margin of 0), so allow for them; the glyphs are 8 pt apart
    assert!(word(&doc, "BC").y0 >= word(&doc, "LL").y1 - 4.0);
}

#[test]
fn only_the_top_and_bottom_bands_drive_the_scale() {
    // a footer only, with the default 72 pt left and right margins: the scale is (792 - 48) / 792
    let footer = |margins: [f64; 4]| {
        let mut h = HeaderFooter::default();
        h.text[4] = "BC".into();
        h.shrink = true;
        h.margins = margins;
        h
    };
    let width_of_ur = |m: [f64; 4]| {
        let mut doc = PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
        let before = word(&reopened(&doc), "UR");
        add(&mut doc, &req(Mark::HeaderFooter(footer(m)), vec![0])).unwrap();
        let doc = reopened(&doc);
        let (ul, ur) = (word(&doc, "UL"), word(&doc, "UR"));
        // scaled about the page's centre: UL started 4 pt from the left edge
        let s = 744.0 / 792.0;
        near(ul.x0, (306.0 - s * 302.0) as f32, "centred across the page");
        (ur.x1 - ur.x0) / (before.x1 - before.x0)
    };
    let default = width_of_ur([36.0, 36.0, 72.0, 72.0]);
    let wide = width_of_ur([36.0, 36.0, 0.0, 0.0]);
    let narrow = width_of_ur([36.0, 36.0, 200.0, 200.0]);
    assert!((default - 744.0 / 792.0).abs() < 0.01, "{default}: not 0.94 (the left and right margins must not shrink it)");
    assert!((default - wide).abs() < 0.001 && (default - narrow).abs() < 0.001, "{default} {wide} {narrow}");
}

#[test]
fn shrink_clips_to_the_visible_area() {
    // a big word outside the crop box but inside the media box
    let content = "BT /F1 36 Tf 10 10 Td (OUTSIDE) Tj ET BT /F1 12 Tf 200 300 Td (IN) Tj ET";
    let src = page_with("/CropBox [100 100 500 700]", content);
    let plain = PdfDocument::from_bytes(&src).unwrap();
    assert_eq!(ink_in(&plain, 0, [0, 0, 70, 600]), 0, "outside the crop box nothing shows");
    let mut doc = PdfDocument::from_bytes(&src).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    let doc = reopened(&doc);
    // scaled in without the clip it would land in the left margin (x < 72)
    assert_eq!(ink_in(&doc, 0, [0, 0, 70, 600]), 0, "what the crop box hid is still hidden");
    assert!(text_of(&doc, 0).contains("IN"));
}

#[test]
fn shrink_survives_unbalanced_content_and_a_stray_q() {
    for content in ["Q Q ", "q 1 0 0 1 100 100 cm "] {
        let src = page_with("", &format!("{content}{}", corners([0.0, 0.0, 612.0, 792.0])));
        let mut doc = PdfDocument::from_bytes(&src).unwrap();
        add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
        let doc = reopened(&doc);
        if content.starts_with('Q') {
            let l = (612.0 * (1.0 - 696.0 / 792.0) / 2.0) as f32;
            inside(word(&doc, "UL"), [l, 48.0, 612.0 - l, 744.0], "a stray Q does not undo the shrink");
            inside(word(&doc, "LR"), [l, 48.0, 612.0 - l, 744.0], "a stray Q does not undo the shrink");
        }
        // the marks are where they were asked for either way
        near(word(&doc, "TC").y0, 36.0, "TC top");
    }
}

#[test]
fn update_keeps_the_shrink_and_unticking_it_gives_the_page_back() {
    let src = page_with("", &corners([0.0, 0.0, 612.0, 792.0]));
    let mut doc = PdfDocument::from_bytes(&src).unwrap();
    let original = reopened(&doc);
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    let first = word(&reopened(&doc), "UL");
    // the file remembers it asked for the shrink
    let spec = info(&doc).unwrap().kinds[0].mark.clone().unwrap();
    assert_eq!(spec["shrink"], true);

    let mut changed = top_and_bottom(true);
    changed.text[1] = "NEW".into();
    let mut r = req(Mark::HeaderFooter(changed.clone()), vec![0]);
    r.replace = true;
    add(&mut doc, &r).unwrap();
    let again = reopened(&doc);
    let second = word(&again, "UL");
    near(second.x0, first.x0, "scaled once, not twice (x)");
    near(second.y0, first.y0, "scaled once, not twice (y)");
    assert_eq!(stream_count(&again, 0, "PCMark /Shrink"), 1);

    // Update with the box unticked: the content is where it was
    changed.shrink = false;
    let mut r = req(Mark::HeaderFooter(changed), vec![0]);
    r.replace = true;
    add(&mut doc, &r).unwrap();
    let off = reopened(&doc);
    assert_eq!(stream_count(&off, 0, "PCMark /Shrink"), 0);
    let (a, b) = (word(&off, "UL"), word(&original, "UL"));
    near(a.x0, b.x0, "unshrunk x");
    near(a.y0, b.y0, "unshrunk y");
}

#[test]
fn removing_the_mark_takes_the_cm_and_the_wrapper_out() {
    let content = corners([0.0, 0.0, 612.0, 792.0]);
    let mut doc = PdfDocument::from_bytes(&page_with("", &content)).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    let wm = Watermark { text: "COPY".into(), ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(wm), vec![0])).unwrap();
    assert_eq!(stream_count(&doc, 0, "PCMark /Shrink"), 1);
    let shrunk = word(&reopened(&doc), "UL");

    // a mark of another kind on top leaves the header's shrink alone
    remove(&mut doc, MarkKind::Watermark, &[0]).unwrap();
    near(word(&reopened(&doc), "UL").y0, shrunk.y0, "the watermark's removal does not undo the shrink");
    assert_eq!(stream_count(&doc, 0, "PCMark /Shrink"), 1);

    // the last mark that asked: the page content is the original bytes again, in the original stream
    assert_eq!(remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), 1);
    let page = doc.find_page(0).unwrap();
    let list = super::apply::contents_list(&page).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].read_stream().unwrap(), content.as_bytes());
    assert_eq!(info(&doc).unwrap(), Info::default());
}

#[test]
fn two_kinds_share_the_page_and_each_removal_recomputes_it() {
    let mut doc = PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    let both = word(&reopened(&doc), "UL");
    let b = Bates { shrink: true, ..Bates::default() };
    add(&mut doc, &req(Mark::Bates(b), vec![0])).unwrap();
    // the Bates number is in the bottom band the footer already keeps clear
    near(word(&reopened(&doc), "UL").y0, both.y0, "same bands, same place");
    // the header and footer go: the Bates number's bottom band alone is left (no top band any more)
    remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap();
    let bates_only = reopened(&doc);
    assert!(word(&bates_only, "UL").y0 < both.y0 - 4.0, "the top band is no longer kept clear: the content moved up");
    inside(word(&bates_only, "LL"), [0.0, 0.0, 612.0, 792.0 - 48.0], "bottom band still kept");
    // the Bates mark goes too: the original page
    remove(&mut doc, MarkKind::Bates, &[0]).unwrap();
    assert_eq!(stream_count(&doc, 0, "PCMark"), 0);
    near(word(&reopened(&doc), "UL").x0, 4.0, "original");
}

#[test]
fn a_watermark_added_updated_and_removed_leaves_the_shrink_alone() {
    let mut doc = PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    let shrunk = word(&reopened(&doc), "UL");
    let same = |doc: &PdfDocument, step: &str| {
        let w = word(&reopened(doc), "UL");
        assert!((w.x0 - shrunk.x0).abs() < 0.01 && (w.y0 - shrunk.y0).abs() < 0.01 && (w.x1 - shrunk.x1).abs() < 0.01, "{step}: {w:?} is not {shrunk:?}");
    };
    for behind in [false, true] {
        let wm = Watermark { text: "DRAFT".into(), behind, ..Watermark::default() };
        add(&mut doc, &req(Mark::Watermark(wm.clone()), vec![0])).unwrap();
        same(&doc, "added");
        let mut r = req(Mark::Watermark(wm), vec![0]);
        r.replace = true;
        add(&mut doc, &r).unwrap();
        same(&doc, "updated");
        remove(&mut doc, MarkKind::Watermark, &[0]).unwrap();
        same(&doc, "removed");
    }
    // and the header still gives the page back
    remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap();
    near(word(&reopened(&doc), "UL").x0, 4.0, "original");
}

#[test]
fn a_shrink_is_found_again_after_the_streams_were_merged() {
    let mut doc = PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    let first = word(&reopened(&doc), "UL");
    merge_streams(&mut doc, 0);
    // Update on the merged page: the old section is cut, not scaled on top of
    let mut r = req(Mark::HeaderFooter(top_and_bottom(true)), vec![0]);
    r.replace = true;
    add(&mut doc, &r).unwrap();
    let again = reopened(&doc);
    near(word(&again, "UL").y0, first.y0, "scaled once");
    // the merged stream's own section was cut: one in the page, in the wrapper
    assert_eq!(stream_count(&again, 0, "PCMark /Shrink"), 1);
    remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap();
    assert_eq!(stream_count(&doc, 0, "PCMark /Shrink"), 0);
    near(word(&reopened(&doc), "UL").x0, 4.0, "original");
}

#[test]
fn margins_that_leave_no_room_are_refused() {
    let mut doc = PdfDocument::from_bytes(&one_page("")).unwrap();
    let mut h = top_and_bottom(true);
    h.margins = [400.0, 400.0, 72.0, 72.0];
    match add(&mut doc, &req(Mark::HeaderFooter(h), vec![0])) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("too little room"), "{m}"),
        other => panic!("{other:?}"),
    }
}

// ---- marks other apps made ---------------------------------------------------------------------------

const FOREIGN: &str = "BT /F1 12 Tf 250 400 Td (Body) Tj ET\n\
/Artifact <</Type /Pagination /Subtype /Header /Attached [/Top]>> BDC\n\
BT /F1 10 Tf 72 760 Td (ForeignHeader) Tj ET\n\
EMC\n\
/Artifact <</Type /Pagination /Subtype /Footer>> BDC\n\
q BT /F1 10 Tf 72 20 Td /Span <</Lang (en-US)>> BDC (ForeignFooter) Tj EMC ET Q\n\
EMC\n\
/Artifact <</Type /Layout /Subtype /Header>> BDC\n\
BT /F1 10 Tf 72 700 Td (LayoutKeep) Tj ET\n\
EMC\n\
/Artifact <</Type /Pagination /Subtype /LineNum>> BDC\n\
BT /F1 10 Tf 20 600 Td (LineKeep) Tj ET\n\
EMC\n";

fn foreign_of(doc: &PdfDocument, kind: MarkKind) -> Option<ForeignInfo> {
    info(doc).unwrap().foreign.into_iter().find(|f| f.kind == kind)
}

#[test]
fn another_apps_header_is_found_and_cut_and_the_body_stays() {
    let mut doc = PdfDocument::from_bytes(&page_with("", FOREIGN)).unwrap();
    let i = info(&doc).unwrap();
    assert!(i.kinds.is_empty(), "not ours: no Update for it");
    assert_eq!(foreign_of(&doc, MarkKind::HeaderFooter).unwrap().pages, [0]);
    assert!(foreign_of(&doc, MarkKind::HeaderFooter).unwrap().stuck.is_empty());
    assert!(foreign_of(&doc, MarkKind::Watermark).is_none() && foreign_of(&doc, MarkKind::Background).is_none());

    // both the header and the footer (one with marked content nested in it) go, nothing else
    assert_eq!(remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), (2, 0));
    let doc = reopened(&doc);
    let text = text_of(&doc, 0);
    assert!(text.contains("Body") && text.contains("LayoutKeep") && text.contains("LineKeep"), "{text:?}");
    assert!(!text.contains("ForeignHeader") && !text.contains("ForeignFooter"), "{text:?}");
    assert_eq!(stream_count(&doc, 0, "/Type /Layout"), 1);
    // the Layout and LineNum sections are in the streams byte for byte, nested BDC and all
    let layout = "/Artifact <</Type /Layout /Subtype /Header>> BDC\nBT /F1 10 Tf 72 700 Td (LayoutKeep) Tj ET\nEMC";
    assert_eq!(stream_count(&doc, 0, layout), 1);
    assert!(foreign_of(&doc, MarkKind::HeaderFooter).is_none());
    // removing again finds nothing
    let mut doc = doc;
    assert_eq!(remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), (0, 0));
}

#[test]
fn a_stream_full_of_unclosed_sections_is_read_in_one_pass() {
    // 50,000 openers and no EMC: each must not scan to the end (this would be 10^9 steps)
    let hostile = "/Artifact <</Type /Pagination /Subtype /Header>> BDC\n".repeat(50_000);
    let started = std::time::Instant::now();
    let (found, _) = super::fit::foreign_sections(hostile.as_bytes(), 0);
    assert_eq!(found.len(), 50_000);
    assert!(found.iter().all(|f| !f.clean));
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
}

#[test]
fn another_apps_watermark_and_background_are_their_own_rows() {
    let content = "BT /F1 12 Tf 250 400 Td (Body) Tj ET\n\
/Artifact <</Type /Pagination /Subtype /Watermark>> BDC BT /F1 40 Tf 100 300 Td (SECRET) Tj ET EMC\n\
/Artifact <</Type /Background>> BDC q 1 0 0 rg 0 0 100 100 re f Q EMC\n";
    let mut doc = PdfDocument::from_bytes(&page_with("", content)).unwrap();
    assert!(foreign_of(&doc, MarkKind::HeaderFooter).is_none());
    assert!(foreign_of(&doc, MarkKind::Watermark).is_some() && foreign_of(&doc, MarkKind::Background).is_some());
    assert_eq!(remove_foreign(&mut doc, MarkKind::Watermark, &[0]).unwrap(), (1, 0));
    assert!(!text_of(&doc, 0).contains("SECRET") && text_of(&doc, 0).contains("Body"));
    assert!(foreign_of(&doc, MarkKind::Background).is_some(), "the background was not the asked kind");
}

#[test]
fn a_section_that_cannot_be_cut_cleanly_is_left_and_reported() {
    let cases = [
        // no end in this stream
        ("no EMC", "/Artifact <</Type /Pagination /Subtype /Header>> BDC BT /F1 10 Tf 72 760 Td (Stuck) Tj ET\n"),
        // a q that is never closed inside it would leak into what follows
        ("open q", "/Artifact <</Type /Pagination /Subtype /Header>> BDC q BT /F1 10 Tf 72 760 Td (Stuck) Tj ET EMC\n"),
        // a cm outside q...Q would move what follows
        ("bare cm", "/Artifact <</Type /Pagination /Subtype /Header>> BDC 1 0 0 1 5 5 cm BT /F1 10 Tf 72 760 Td (Stuck) Tj ET EMC\n"),
        // a text object that ends outside
        ("open BT", "/Artifact <</Type /Pagination /Subtype /Header>> BDC BT /F1 10 Tf 72 760 Td (Stuck) Tj EMC ET\n"),
    ];
    for (what, bad) in cases {
        let mut doc = PdfDocument::from_bytes(&page_with("", &format!("BT /F1 12 Tf 250 400 Td (Body) Tj ET\n{bad}"))).unwrap();
        let row = foreign_of(&doc, MarkKind::HeaderFooter).unwrap_or_else(|| panic!("{what}: not listed"));
        assert_eq!((row.pages.clone(), row.stuck.clone()), (vec![0], vec![0]), "{what}");
        assert_eq!(remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), (0, 1), "{what}");
        assert!(text_of(&doc, 0).contains("Stuck"), "{what}: it was left in the page");
        assert!(foreign_of(&doc, MarkKind::HeaderFooter).is_some(), "{what}: still listed");
    }
    // one that is clean next to one that is not: the clean one goes, the other is reported
    let both = format!(
        "{FOREIGN}/Artifact <</Type /Pagination /Subtype /Watermark>> BDC q BT /F1 10 Tf 72 500 Td (WM) Tj ET\n"
    );
    let mut doc = PdfDocument::from_bytes(&page_with("", &both)).unwrap();
    assert_eq!(remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), (2, 0));
    assert_eq!(remove_foreign(&mut doc, MarkKind::Watermark, &[0]).unwrap(), (0, 1));
}

#[test]
fn our_marks_and_another_apps_do_not_mix() {
    let mut doc = PdfDocument::from_bytes(&page_with("", FOREIGN)).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(hf(5, "Ours <<1>>")), vec![0])).unwrap();
    let i = info(&doc).unwrap();
    assert_eq!(i.kinds.len(), 1);
    assert_eq!(foreign_of(&doc, MarkKind::HeaderFooter).unwrap().pages, [0]);
    // removing ours leaves the other app's; removing the other app's leaves ours
    remove(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap();
    assert!(text_of(&doc, 0).contains("ForeignHeader") && !text_of(&doc, 0).contains("Ours"));
    add(&mut doc, &req(Mark::HeaderFooter(hf(5, "Ours <<1>>")), vec![0])).unwrap();
    remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap();
    assert!(!text_of(&doc, 0).contains("ForeignHeader") && text_of(&doc, 0).contains("Ours 1"));
}

#[test]
fn an_annotated_page_is_listed_so_the_shrink_can_say_its_comments_stay() {
    let doc = PdfDocument::from_bytes(&page_with("/Annots [ << /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (n) >> ]", "")).unwrap();
    assert_eq!(info(&doc).unwrap().annotated, [0]);
    let plain = PdfDocument::from_bytes(&one_page("")).unwrap();
    assert!(info(&plain).unwrap().annotated.is_empty());
}

// ---- signed and certified documents -------------------------------------------------------------------

fn signed_with(content: &str, certified: bool) -> Vec<u8> {
    use crate::outline::tests::{doc_mdp, raw_pdf, sig_value};
    let perms = if certified {
        format!("/Perms << /DocMDP << /Type /Sig /Reference [ {} ] >> >>", doc_mdp("/TransformParams << /P 1 >>"))
    } else {
        String::new()
    };
    raw_pdf(&[
        format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /SigFlags 3 /Fields [6 0 R] >> {perms} >>"),
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!("<< /FT /Sig /T (S) /Type /Annot /Subtype /Widget /Rect [0 0 0 0] {} >>", sig_value("")),
    ])
}

#[test]
fn a_certified_document_refuses_the_shrink_and_the_removal_of_another_apps_marks() {
    let mut doc = PdfDocument::from_bytes(&signed_with(FOREIGN, true)).unwrap();
    assert!(info(&doc).unwrap().certified);
    for e in [
        add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap_err(),
        remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap_err(),
    ] {
        assert!(matches!(&e, EngineError::Invalid(m) if m.contains("certified")), "{e:?}");
    }
    assert!(text_of(&doc, 0).contains("ForeignHeader"), "nothing was changed");
}

#[test]
fn a_signed_document_keeps_its_signed_bytes_through_a_shrink_and_a_removal() {
    use crate::outline::tests::open_bytes;
    let d = dir("signed-fit");
    let bytes = signed_with(FOREIGN, false);
    let (eng, id, path) = open_bytes(&d, "a.pdf", &bytes);
    assert!(eng.read_custom(id, |d| Ok(info(d)?.signed)).unwrap());
    let m = req(Mark::HeaderFooter(top_and_bottom(true)), vec![0]);
    eng.apply_custom(id, "Add header and footer to p. 1".into(), EditSource::User, false, |doc| add(doc, &m)).unwrap();
    eng.apply_removing(id, "Remove header and footer from another app".into(), EditSource::User, |doc| {
        remove_foreign(doc, MarkKind::HeaderFooter, &[0])?;
        Ok(vec![0])
    })
    .unwrap();
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > bytes.len() && saved.starts_with(&bytes), "the signed bytes are the start of the saved file");
    let doc = PdfDocument::from_bytes(&saved).unwrap();
    assert!(!text_of(&doc, 0).contains("ForeignHeader"));
    assert_eq!(stream_count(&doc, 0, "PCMark /Shrink"), 1);
}

#[test]
fn a_stream_of_unclosed_shrink_sections_is_read_in_one_pass() {
    // 50,000 openers and one EMC: only the last opener is closed, and none scans to the end
    let hostile = "/Artifact <</PCMark /Shrink /HeaderFooter [1 1 1 1]>> BDC\n".repeat(50_000) + "EMC\n";
    let started = std::time::Instant::now();
    let found = super::fit::shrink_sections(hostile.as_bytes());
    assert_eq!(found.len(), 1);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
}

#[test]
fn on_a_merged_page_removing_one_kind_leaves_the_others_shrink() {
    let page = || PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
    let bates = Bates { shrink: true, ..Bates::default() };
    // what each kind alone does to the page
    let mut hf_only = page();
    add(&mut hf_only, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    let mut bates_only = page();
    add(&mut bates_only, &req(Mark::Bates(bates.clone()), vec![0])).unwrap();
    let (hf_ul, bates_ul) = (word(&reopened(&hf_only), "UL"), word(&reopened(&bates_only), "UL"));
    assert!((hf_ul.y0 - bates_ul.y0).abs() > 10.0, "the two differ, or the test says nothing");

    for (removed, left, expect) in [(MarkKind::Bates, MarkKind::HeaderFooter, hf_ul), (MarkKind::HeaderFooter, MarkKind::Bates, bates_ul)] {
        let mut doc = page();
        add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
        add(&mut doc, &req(Mark::Bates(bates.clone()), vec![0])).unwrap();
        merge_streams(&mut doc, 0);
        assert!(remove(&mut doc, removed, &[0]).unwrap() >= 1);
        let after = reopened(&doc);
        near(word(&after, "UL").y0, expect.y0, &format!("the {left:?} shrink stays"));
        assert_eq!(stream_count(&doc, 0, "PCMark /Shrink"), 1, "one section, not two");
        // and the last one out gives the page back
        assert!(remove(&mut doc, left, &[0]).unwrap() >= 1);
        assert_eq!(stream_count(&doc, 0, "PCMark /Shrink"), 0);
        near(word(&reopened(&doc), "UL").x0, 4.0, "original");
    }
}

#[test]
fn bands_read_from_a_file_are_kept_inside_the_page() {
    // a merged page (no tagged wrapper) with a /Shrink section that has a negative and a huge band
    let content = format!(
        "q %Inkwell\n/Artifact <</PCMark /Shrink /Bates [-50 0 0 0] /Watermark [0 999999999 0 0]>> BDC\n1 0 0 1 0 0 cm\nEMC\n{}Q %Inkwell\n",
        corners([0.0, 0.0, 612.0, 792.0])
    );
    let mut doc = PdfDocument::from_bytes(&page_with("", &content)).unwrap();
    // the huge band is clamped to the page: it leaves no room, and says so, instead of poisoning the cm
    match add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("too little room"), "{m}"),
        other => panic!("{other:?}"),
    }
    // without it, the negative band is read as 0
    let content = content.replace("/Watermark [0 999999999 0 0]", "");
    let mut doc = PdfDocument::from_bytes(&page_with("", &content)).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    assert_eq!(stream_count(&doc, 0, "/Bates [0 0 0 0]"), 1);
    assert_eq!(stream_count(&doc, 0, "-50"), 0);
}

#[test]
fn a_section_inside_a_text_object_of_the_page_is_left_when_it_moves_the_text() {
    // cutting the header would move "Body": its Td is relative to the header's
    let inside_text = "BT /F1 12 Tf /Artifact <</Type /Pagination>> BDC 72 750 Td (Hdr) Tj EMC 0 -700 Td (Body) Tj ET\n";
    let mut doc = PdfDocument::from_bytes(&page_with("", inside_text)).unwrap();
    let found = foreign_of(&doc, MarkKind::HeaderFooter).unwrap();
    assert_eq!((found.pages.clone(), found.stuck.clone()), (vec![0], vec![0]));
    assert_eq!(remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), (0, 1));
    assert!(text_of(&reopened(&doc), 0).contains("Hdr"));

    // a section that only draws (its own BT...ET, nothing inherited) inside the page's text is as good as outside
    let drawn = "BT /F1 12 Tf 72 500 Td (Before) Tj /Artifact <</Type /Pagination>> BDC EMC (After) Tj ET\n";
    let mut doc = PdfDocument::from_bytes(&page_with("", drawn)).unwrap();
    assert_eq!(remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), (1, 0));
}

#[test]
fn an_update_with_shrink_is_refused_on_a_merged_page_with_marks_on_top() {
    let mut doc = PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
    let wm = Watermark { text: "DRAFT".into(), behind: false, ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(wm), vec![0])).unwrap();
    merge_streams(&mut doc, 0);
    let mut r = req(Mark::HeaderFooter(top_and_bottom(true)), vec![0]);
    r.replace = true;
    match add(&mut doc, &r) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("rewritten after marks were added"), "{m}"),
        other => panic!("{other:?}"),
    }
    // the same Update without a shrink is fine (nothing on top is scaled), and so is a page without marks on top
    let mut r = req(Mark::HeaderFooter(top_and_bottom(false)), vec![0]);
    r.replace = true;
    add(&mut doc, &r).unwrap();
    near(word(&reopened(&doc), "UL").x0, 4.0, "the shrink went with the update");
}

#[test]
fn a_text_object_that_runs_on_into_the_next_stream_counts_as_inside_text() {
    // the first stream leaves a BT open; the header is in the second, whose Td is relative to it
    let open = "BT /F1 12 Tf 72 750 Td (A) Tj\n";
    let header = "/Artifact <</Type /Pagination /Subtype /Header>> BDC 0 -20 Td (Hdr) Tj EMC (B) Tj ET\n";
    let mut doc = PdfDocument::from_bytes(&page_with_streams("", &[open, header])).unwrap();
    let found = foreign_of(&doc, MarkKind::HeaderFooter).unwrap();
    assert_eq!((found.pages, found.stuck), (vec![0], vec![0]));
    assert_eq!(remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), (0, 1));
    assert!(text_of(&reopened(&doc), 0).contains("Hdr"));

    // control: the first stream closes its text object, so the second one's own BT...ET is not inside it
    let closed = "BT /F1 12 Tf 72 750 Td (A) Tj ET\n";
    let own = "/Artifact <</Type /Pagination /Subtype /Header>> BDC BT /F1 10 Tf 72 700 Td (Hdr) Tj ET EMC\n";
    let mut doc = PdfDocument::from_bytes(&page_with_streams("", &[closed, own])).unwrap();
    assert!(foreign_of(&doc, MarkKind::HeaderFooter).unwrap().stuck.is_empty());
    assert_eq!(remove_foreign(&mut doc, MarkKind::HeaderFooter, &[0]).unwrap(), (1, 0));
    assert!(!text_of(&reopened(&doc), 0).contains("Hdr"));
}

#[test]
fn a_shrink_is_refused_where_inline_marks_would_scale_with_the_page() {
    let refused = |doc: &mut PdfDocument| match add(doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])) {
        Err(EngineError::Invalid(m)) => assert!(m.contains("rewritten after marks were added"), "{m}"),
        other => panic!("{other:?}"),
    };
    // a background inline before the page's own content: nothing wraps it yet
    let mut doc = PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
    add(&mut doc, &req(Mark::Background(Background { color: [0.9, 0.9, 0.5], ..Background::default() }), vec![0])).unwrap();
    merge_streams(&mut doc, 0);
    refused(&mut doc);
    // the same page takes a header and footer that does not shrink
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(false)), vec![0])).unwrap();

    // a watermark on top, merged; a later mark without a shrink puts a wrapper around the stream
    let mut doc = PdfDocument::from_bytes(&page_with("", &corners([0.0, 0.0, 612.0, 792.0]))).unwrap();
    let wm = Watermark { text: "DRAFT".into(), behind: false, ..Watermark::default() };
    add(&mut doc, &req(Mark::Watermark(wm), vec![0])).unwrap();
    merge_streams(&mut doc, 0);
    add(&mut doc, &req(Mark::Bates(Bates::default()), vec![0])).unwrap();
    refused(&mut doc);
    // and once the watermark is removed there is nothing left to scale
    remove(&mut doc, MarkKind::Watermark, &[0]).unwrap();
    add(&mut doc, &req(Mark::HeaderFooter(top_and_bottom(true)), vec![0])).unwrap();
}
