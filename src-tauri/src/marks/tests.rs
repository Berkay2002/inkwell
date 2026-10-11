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
    let content = "BT /F1 12 Tf 250 400 Td (Body) Tj ET";
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
