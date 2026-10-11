//! Poster, booklet creep and Save as PDF. Pure layout first, then the writer on generated fixtures,
//! read back with MuPDF (page counts, text, forms, pixels) and compared with the preview.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mupdf::pdf::PdfDocument;
use mupdf::{Colorspace, Matrix};

use super::layout::*;
use super::*;
use crate::engine::fixture::{build, Fixture};

const A4: (f64, f64) = (595.28, 841.89);
const MM: f64 = 72.0 / 25.4;

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn poster(scale: f64, overlap_mm: f64, cut_marks: bool, labels: bool) -> Layout {
    Layout::Poster { scale, overlap: overlap_mm * MM, cut_marks, labels }
}

fn lay(sizes: &[(f64, f64)], l: &Layout) -> Result<Vec<Sheet>, LayoutError> {
    let pages: Vec<u32> = (0..sizes.len() as u32).collect();
    layout(sizes, &pages, A4, Orientation::Portrait, l)
}

// ---------------------------------------------------------------- poster

/// The part of the scaled page a tile shows, `[x0, y0, x1, y1]` (scaled-page space, y down).
fn window(p: &Placement) -> [f64; 4] {
    [p.clip[0] - p.x, p.clip[1] - p.y, p.clip[2] - p.x, p.clip[3] - p.y]
}

#[test]
fn a_poster_cuts_the_enlarged_page_into_overlapping_tiles_in_reading_order() {
    let overlap = 6.0 * MM;
    let sheets = lay(&[A4], &poster(200.0, 6.0, false, false)).unwrap();
    // 1190.6 x 1683.8 pt on a 559.3 x 805.9 pt printable area with 17 pt of overlap: 3 x 3
    assert_eq!(sheets.len(), 9);
    let tiles: Vec<_> = sheets.iter().map(|s| s.tile.unwrap()).collect();
    assert!(tiles.iter().enumerate().all(|(i, t)| (t.col, t.row, t.cols, t.rows) == ((i % 3) as u32, (i / 3) as u32, 3, 3)));
    assert!(sheets.iter().all(|s| s.placed.len() == 1 && near(s.placed[0].scale, 2.0) && near(s.w, A4.0)));
    // across: neighbours share exactly `overlap` of the page, measured on the sheet
    for row in 0..3 {
        for col in 0..2 {
            let (a, b) = (window(&sheets[row * 3 + col].placed[0]), window(&sheets[row * 3 + col + 1].placed[0]));
            assert!(near(a[2] - b[0], overlap), "{} vs {overlap}", a[2] - b[0]);
        }
    }
    // down
    for col in 0..3 {
        for row in 0..2 {
            let (a, b) = (window(&sheets[row * 3 + col].placed[0]), window(&sheets[(row + 1) * 3 + col].placed[0]));
            assert!(near(a[3] - b[1], overlap));
        }
    }
    // the tiles cover the page: first window starts at its corner, the last reaches the far corner
    assert!(near(window(&sheets[0].placed[0])[0], 0.0) && near(window(&sheets[8].placed[0])[2], A4.0 * 2.0));
    assert!(near(window(&sheets[8].placed[0])[3], A4.1 * 2.0));
    // nothing is drawn outside the page or the printable area
    for s in &sheets {
        let c = s.placed[0].clip;
        assert!(c[0] >= MARGIN - 1e-9 && c[1] >= MARGIN - 1e-9 && c[2] <= s.w - MARGIN + 1e-9 && c[3] <= s.h - MARGIN + 1e-9);
    }
}

#[test]
fn poster_cut_marks_sit_on_the_edges_of_the_overlap_strip() {
    let sheets = lay(&[A4], &poster(200.0, 6.0, true, true)).unwrap();
    let mid = 4; // column 1, row 1
    let (here, left, right, up, down) = (&sheets[mid], &sheets[mid - 1], &sheets[mid + 1], &sheets[mid - 3], &sheets[mid + 3]);
    let at = |s: &Sheet, f: fn(&[f64; 4]) -> f64| -> Vec<f64> {
        let mut v: Vec<f64> = s.lines.iter().map(|l| f(l)).collect();
        v.sort_by(f64::total_cmp);
        v.dedup_by(|a, b| near(*a, *b));
        v
    };
    // where the neighbours' windows start and end, in this sheet's coordinates
    let wx = |s: &Sheet| window(&s.placed[0]);
    let (px, py) = (here.placed[0].x, here.placed[0].y);
    let want_x = [px + wx(left)[2], px + wx(right)[0]];
    let want_y = [py + wx(up)[3], py + wx(down)[1]];
    let vertical: Vec<f64> = at(here, |l| if l[0] == l[2] { l[0] } else { f64::NAN }).into_iter().filter(|v| !v.is_nan()).collect();
    let horizontal: Vec<f64> = at(here, |l| if l[1] == l[3] { l[1] } else { f64::NAN }).into_iter().filter(|v| !v.is_nan()).collect();
    assert_eq!(vertical.len(), 2);
    assert!(near(vertical[0], want_x[0]) && near(vertical[1], want_x[1]), "{vertical:?} vs {want_x:?}");
    assert_eq!(horizontal.len(), 2);
    assert!(near(horizontal[0], want_y[0]) && near(horizontal[1], want_y[1]));
    // ticks stay in the margin; a corner tile has marks for its two neighbours only
    assert!(here.lines.iter().all(|l| (l[0] == l[2] && (l[3] <= MARGIN || l[1] >= here.h - MARGIN)) || (l[1] == l[3] && (l[2] <= MARGIN || l[0] >= here.w - MARGIN))));
    assert_eq!(sheets[0].lines.len(), 2 * 2);
    // labels name the tile and the page
    assert_eq!(here.labels.len(), 1);
    assert_eq!(here.labels[0].text, "Page 1, row 2 of 3, column 2 of 3");
    assert!(here.labels[0].y > 0.0 && here.labels[0].y < MARGIN);
    // and without the options there is nothing in the margin
    let plain = lay(&[A4], &poster(200.0, 6.0, false, false)).unwrap();
    assert!(plain.iter().all(|s| s.lines.is_empty() && s.labels.is_empty()));
}

#[test]
fn a_poster_that_fits_one_sheet_is_one_tile_and_scale_one_hundred_needs_two_by_two() {
    let small = lay(&[(300.0, 400.0)], &poster(100.0, 0.0, true, false)).unwrap();
    assert_eq!(small.len(), 1);
    assert!(small[0].lines.is_empty());
    // an A4 page is larger than the printable area of an A4 sheet
    assert_eq!(lay(&[A4], &poster(100.0, 6.0, false, false)).unwrap().len(), 4);
    // a landscape page turns the sheet when the orientation is automatic
    let wide = layout(&[(842.0, 595.0)], &[0], A4, Orientation::Auto, &poster(100.0, 0.0, false, false)).unwrap();
    assert!(wide.iter().all(Sheet::landscape));
}

#[test]
fn poster_limits_are_checked_in_points_before_anything_is_built() {
    assert!(lay(&[A4], &poster(0.0, 6.0, false, false)).is_err());
    assert!(lay(&[A4], &poster(f64::NAN, 6.0, false, false)).is_err());
    assert!(lay(&[A4], &poster(100.0, 51.0, false, false)).is_err());
    assert!(lay(&[A4], &poster(100.0, -1.0, false, false)).is_err());
    assert!(lay(&[A4], &poster(100.0, f64::NAN, false, false)).is_err());
    assert!(lay(&[A4], &poster(100.0, 50.0, false, false)).is_ok());
    // a tile of 72 x 72 pt (36 pt printable) cannot overlap by half or more
    let tiny = layout(&[A4], &[0], (72.0, 72.0), Orientation::Portrait, &poster(100.0, 20.0 / MM, false, false));
    assert!(tiny.is_err());
    // the cap counts sheets per job, over all pages: 3 pages of 324 tiles pass, 4 do not
    let big = poster(1600.0, 6.0, false, false);
    assert_eq!(lay(&[A4; 3], &big).unwrap().len(), 3 * 324);
    let err = lay(&[A4; 4], &big).unwrap_err().to_string();
    assert!(err.contains("1024"), "{err}");
    // one page at the largest scale is refused, not built
    assert!(lay(&[A4], &poster(10_000.0, 0.0, false, false)).is_err());
}

// ---------------------------------------------------------------- creep

fn booklet_fronts(pages: usize, creep_mm: f64) -> Vec<Sheet> {
    let sizes = vec![(612.0, 792.0); pages];
    let pages: Vec<u32> = (0..pages as u32).collect();
    let l = Layout::Booklet { sides: BookletSides::FrontOnly, binding: Binding::Left, creep: creep_mm * MM };
    layout(&sizes, &pages, (612.0, 792.0), Orientation::Auto, &l).unwrap()
}

#[test]
fn creep_moves_inner_sheets_toward_the_spine_and_never_across_it() {
    let creep = 2.0 * MM;
    let sheets = booklet_fronts(16, 2.0); // 4 sheets of paper, 2 pages a side
    assert_eq!(sheets.len(), 4);
    let spine = sheets[0].w / 2.0;
    let left = |k: usize| sheets[k].placed[0];
    let right = |k: usize| sheets[k].placed[1];
    let shifts: Vec<f64> = (0..4).map(|k| left(k).x - left(0).x).collect();
    // outermost unshifted, innermost by `creep`, evenly between
    assert!(near(shifts[0], 0.0) && near(shifts[3], creep));
    assert!(shifts.windows(2).all(|w| w[1] > w[0]));
    assert!(near(shifts[1], creep / 3.0) && near(shifts[2], creep * 2.0 / 3.0));
    // the right page moves the same distance the other way
    assert!((0..4).all(|k| near(right(0).x - right(k).x, shifts[k])));
    // nothing, clip included, crosses the fold
    for s in &sheets {
        for (i, p) in s.placed.iter().enumerate() {
            let w = 612.0 * p.scale;
            if i == 0 {
                assert!(p.x + w <= spine + 1e-6 && p.clip[2] <= spine + 1e-6);
            } else {
                assert!(p.x >= spine - 1e-6 && p.clip[0] >= spine - 1e-6);
            }
        }
    }
    // no creep leaves every sheet where it was; one sheet of paper has nothing to creep
    let flat = booklet_fronts(16, 0.0);
    assert!(flat.iter().all(|s| near(s.placed[0].x, flat[0].placed[0].x)));
    let one = booklet_fronts(4, 3.0);
    assert!(near(one[0].placed[0].x, booklet_fronts(4, 0.0)[0].placed[0].x));
}

#[test]
fn creep_beyond_three_millimetres_is_refused() {
    let l = |mm: f64| Layout::Booklet { sides: BookletSides::Both, binding: Binding::Left, creep: mm * MM };
    let go = |l: &Layout| layout(&[(612.0, 792.0); 8], &(0..8).collect::<Vec<_>>(), (612.0, 792.0), Orientation::Auto, l);
    assert!(go(&l(3.0)).is_ok());
    assert!(go(&l(3.5)).is_err());
    assert!(go(&l(-0.5)).is_err());
    assert!(go(&l(f64::NAN)).is_err());
}

// ---------------------------------------------------------------- band skipping

#[test]
fn bands_with_only_margin_marks_still_count_as_ink() {
    // At 1 px per point a 12 px band at the top is margin: the page clip starts at 18.
    let mut sheet = Sheet { w: 595.0, h: 842.0, placed: vec![Placement { page: 0, x: 18.0, y: 18.0, scale: 1.0, rotate: false, clip: [18.0, 18.0, 577.0, 824.0] }], ..Sheet::default() };
    assert!(!render::band_has_ink(&sheet, 1.0, 0, 12));
    sheet.lines.push([100.0, 2.0, 100.0, 16.0]);
    assert!(render::band_has_ink(&sheet, 1.0, 0, 12));
    sheet.lines.clear();
    sheet.labels.push(Label { x: 200.0, y: 12.0, text: "Page 1, row 1 of 2, column 1 of 2".into() });
    assert!(render::band_has_ink(&sheet, 1.0, 0, 12));
    // a band far from both is empty again at the bottom margin
    assert!(!render::band_has_ink(&sheet, 1.0, 830, 842));
}

// ---------------------------------------------------------------- Save as PDF

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-print-layout-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open_bytes(d: &Path, name: &str, bytes: &[u8]) -> (std::sync::Arc<Engine>, u32, PathBuf) {
    let p = d.join(name);
    std::fs::write(&p, bytes).unwrap();
    let eng = Engine::new(1, 1 << 20);
    let (doc, r) = eng.open_pending(&p.to_string_lossy());
    r.unwrap();
    (eng, doc, p)
}

fn open_fixture(d: &Path, pages: usize) -> (std::sync::Arc<Engine>, u32, PathBuf) {
    open_bytes(d, "sample.pdf", &build(&Fixture { pages, ..Default::default() }))
}

/// A PDF from object bodies, `objs[i]` being object `i + 1` (the catalog is object 1), with a real xref.
fn raw_pdf(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).bytes());
    out
}

fn stream(dict: &str, content: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{content}\nendstream", content.len())
}

const HELV: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

/// A one-page Letter PDF with Helvetica as /F1 (object 4), the page's `content` (object 5) and
/// `extra` objects from number 6 on. `page` and `resources` add entries to the page and its
/// resources, `catalog` to the catalog.
fn one_page(content: &str, page: &str, resources: &str, catalog: &str, extra: &[String]) -> Vec<u8> {
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> {resources} >> /Contents 5 0 R {page} >>"
        ),
        HELV.to_string(),
        stream("", content),
    ];
    objs.extend_from_slice(extra);
    raw_pdf(&objs)
}

/// Some stream of the file at `path` (decoded) holds `needle`.
fn streams_mention(path: &Path, needle: &str) -> bool {
    let d = PdfDocument::open(path.to_string_lossy().as_ref()).unwrap();
    (1..d.count_objects().unwrap() as i32).any(|i| {
        let Ok(o) = d.new_indirect(i, 0) else { return false };
        o.is_stream().unwrap_or(false) && o.read_stream().is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle.as_bytes()))
    })
}

/// The first form XObject drawn on page `i` of the file at `path`.
fn first_form(p: &PdfDocument, i: i32) -> mupdf::pdf::PdfObject {
    let res = p.find_page(i).unwrap().get_dict("Resources").unwrap().unwrap();
    res.get_dict("XObject").unwrap().unwrap().get_dict_val(0).unwrap().unwrap()
}

/// The words of page 0 of `path` as (text, x0, y0).
fn words(path: &Path) -> Vec<(String, f64, f64)> {
    let d = mupdf::Document::open(path.to_string_lossy().as_ref()).unwrap();
    let t = d.load_page(0).unwrap().to_text_page(mupdf::TextPageFlags::empty()).unwrap();
    t.words().iter().map(|w| (w.text.clone(), f64::from(w.bounds.x0), f64::from(w.bounds.y0))).collect()
}

fn settings(layout: Layout) -> Settings {
    Settings { paper: Paper { id: None, width: 612.0, height: 792.0 }, layout, orientation: Orientation::Portrait, ..Settings::default() }
}

fn save(eng: &Engine, doc: u32, s: &Settings, out: &Path) -> Result<pdf::Saved, PrintError> {
    pdf::save(eng, doc, s, out, &AtomicBool::new(false), &mut |_| {})
}

/// The fixture page numbers drawn on page `i` of `doc`, in reading order ("Inkwell fixture page N").
fn titles(doc: &mupdf::Document, i: i32) -> Vec<u32> {
    let words = doc.load_page(i).unwrap().to_text_page(mupdf::TextPageFlags::empty()).unwrap().words();
    let mut found: Vec<(i32, f32, u32)> = words
        .windows(4)
        .filter(|w| w[0].text == "Inkwell" && w[1].text == "fixture" && w[2].text == "page")
        .filter_map(|w| Some(((w[0].bounds.y0 / 10.0).round() as i32, w[0].bounds.x0, w[3].text.parse().ok()?)))
        .collect();
    // reading order: rows top to bottom, each left to right
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    found.into_iter().map(|(_, _, n)| n).collect()
}

fn gray(path: &Path, i: i32, scale: f32) -> (usize, usize, Vec<u8>) {
    let d = mupdf::Document::open(path.to_string_lossy().as_ref()).unwrap();
    let pix = d.load_page(i).unwrap().to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_gray(), false, true).unwrap();
    (pix.width() as usize, pix.height() as usize, pix.samples().to_vec())
}

fn dark(path: &Path, i: i32) -> usize {
    gray(path, i, 1.0).2.iter().filter(|v| **v < 64).count()
}

#[test]
fn n_up_writes_one_page_per_sheet_with_every_page_a_form() {
    let d = dir("nup");
    let (eng, doc, src) = open_fixture(&d, 8);
    let before = std::fs::read(&src).unwrap();
    let rev = eng.doc_entry(doc).unwrap().generation();
    let out = d.join("out.pdf");
    let s = settings(Layout::Multiple { per_sheet: 4, order: PageOrder::Horizontal, border: true, auto_rotate: true });
    let saved = save(&eng, doc, &s, &out).unwrap();
    assert_eq!((saved.pages, saved.sheets), (8, 2));
    let o = mupdf::Document::open(out.to_string_lossy().as_ref()).unwrap();
    assert_eq!(o.page_count().unwrap(), 2);
    assert_eq!(titles(&o, 0), [1, 2, 3, 4]);
    assert_eq!(titles(&o, 1), [5, 6, 7, 8]);
    // small text and hairline borders are anti-aliased grey
    assert!(gray(&out, 0, 1.0).2.iter().filter(|v| **v < 200).count() > 500);
    // vector: every page's resources hold forms, and nothing is an image
    let p = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    for i in 0..2 {
        let res = p.find_page(i).unwrap().get_dict("Resources").unwrap().unwrap();
        let xo = res.get_dict("XObject").unwrap().unwrap();
        assert_eq!(xo.dict_len().unwrap(), 4);
        for k in 0..xo.dict_len().unwrap() {
            let sub = xo.get_dict_val(k as i32).unwrap().unwrap().get_dict("Subtype").unwrap().unwrap();
            assert_eq!(sub.as_name().unwrap(), b"Form");
        }
    }
    // the open document is untouched
    assert_eq!(std::fs::read(&src).unwrap(), before);
    assert_eq!(eng.doc_entry(doc).unwrap().generation(), rev);
}

#[test]
fn a_booklet_of_six_is_two_sheets_of_two_sides_in_saddle_order() {
    let d = dir("booklet");
    let (eng, doc, _) = open_fixture(&d, 6);
    let out = d.join("out.pdf");
    let s = settings(Layout::Booklet { sides: BookletSides::Both, binding: Binding::Left, creep: 0.0 });
    let saved = save(&eng, doc, &s, &out).unwrap();
    assert_eq!(saved.sheets, 4);
    let o = mupdf::Document::open(out.to_string_lossy().as_ref()).unwrap();
    // padded to 8: blanks are pages 7 and 8; left to right on each side
    let sides: Vec<Vec<u32>> = (0..4).map(|i| titles(&o, i)).collect();
    assert_eq!(sides, [vec![1], vec![2], vec![6, 3], vec![4, 5]]);
    // landscape, as printed
    assert!(o.load_page(0).unwrap().bounds().unwrap().width() > o.load_page(0).unwrap().bounds().unwrap().height());
}

#[test]
fn two_sided_padding_keeps_a_blank_back_the_size_of_the_front() {
    let d = dir("padding");
    let (eng, doc, _) = open_fixture(&d, 3);
    let out = d.join("out.pdf");
    let s = Settings { duplex: Duplex::LongEdge, ..settings(Layout::default()) };
    let saved = save(&eng, doc, &s, &out).unwrap();
    assert_eq!(saved.sheets, 4);
    let o = mupdf::Document::open(out.to_string_lossy().as_ref()).unwrap();
    assert_eq!(o.page_count().unwrap(), 4);
    assert_eq!(dark(&out, 3), 0);
    assert!(near(f64::from(o.load_page(3).unwrap().bounds().unwrap().width()), 612.0));
    // copies and collation are for the printer: the file has one of each
    let many = Settings { copies: 5, collate: false, ..settings(Layout::default()) };
    assert_eq!(save(&eng, doc, &many, &d.join("many.pdf")).unwrap().sheets, 3);
}

#[test]
fn a_poster_file_has_one_form_per_page_shared_by_every_tile() {
    let d = dir("poster");
    let (eng, doc, _) = open_fixture(&d, 1);
    let out = d.join("out.pdf");
    let s = settings(poster(200.0, 6.0, true, true));
    let info = plan_info(&eng, doc, &s).unwrap();
    assert_eq!(info.sheets, 9);
    assert_eq!(info.faces[4].tile.map(|t| (t.col, t.row, t.cols, t.rows)), Some((1, 1, 3, 3)));
    let saved = save(&eng, doc, &s, &out).unwrap();
    assert_eq!(saved.sheets, 9);
    let p = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    assert_eq!(p.page_count().unwrap(), 9);
    let mut forms = std::collections::BTreeSet::new();
    for i in 0..9 {
        let xo = p.find_page(i).unwrap().get_dict("Resources").unwrap().unwrap().get_dict("XObject").unwrap().unwrap();
        assert_eq!(xo.dict_len().unwrap(), 1);
        forms.insert(xo.get_dict_val(0).unwrap().unwrap().as_indirect().unwrap());
    }
    assert_eq!(forms.len(), 1);
    // the preview draws the marks and the label in the margin too (the printer's bands skip empty ones)
    let img = image::load_from_memory(&preview_png(&eng, doc, &s, 4, 842).unwrap()).unwrap().to_luma8();
    let ink = |rows: std::ops::Range<u32>| rows.flat_map(|y| (0..img.width()).map(move |x| (x, y))).filter(|&(x, y)| img.get_pixel(x, y).0[0] < 160).count();
    assert!(ink(0..16) > 20, "label and ticks in the top margin");
    assert!(ink(img.height() - 16..img.height()) > 4, "ticks in the bottom margin");
    // the label is real text on the tile
    let o = mupdf::Document::open(out.to_string_lossy().as_ref()).unwrap();
    let text = o.load_page(4).unwrap().to_text_page(mupdf::TextPageFlags::empty()).unwrap().to_text().unwrap();
    assert!(text.contains("row 2 of 3, column 2 of 3"), "{text}");
}

#[test]
fn the_file_is_the_preview_page_for_page() {
    let d = dir("preview-match");
    let (eng, doc, _) = open_fixture(&d, 8);
    let cases = [
        // turned pages: 2 per sheet on a portrait sheet
        settings(Layout::Multiple { per_sheet: 2, order: PageOrder::Horizontal, border: true, auto_rotate: true }),
        settings(Layout::Multiple { per_sheet: 4, order: PageOrder::VerticalReversed, border: false, auto_rotate: true }),
        settings(poster(150.0, 6.0, true, true)),
        settings(Layout::Booklet { sides: BookletSides::Both, binding: Binding::Right, creep: 2.0 * MM }),
    ];
    for (n, s) in cases.iter().enumerate() {
        let out = d.join(format!("out{n}.pdf"));
        let saved = save(&eng, doc, s, &out).unwrap();
        for k in [0u32, saved.sheets - 1] {
            let png = preview_png(&eng, doc, s, k, 800).unwrap();
            let img = image::load_from_memory(&png).unwrap().to_luma8();
            let side = plan(&eng, doc, s).unwrap().sheets[k as usize].clone();
            let scale = 800.0 / side.w.max(side.h);
            let (w, h, px) = gray(&out, k as i32, scale as f32);
            assert!((w as i64 - i64::from(img.width())).abs() <= 1 && (h as i64 - i64::from(img.height())).abs() <= 1, "case {n} side {k}");
            let (cw, ch) = (w.min(img.width() as usize), h.min(img.height() as usize));
            let mut diff = 0u64;
            for y in 0..ch {
                for x in 0..cw {
                    diff += u64::from(px[y * w + x].abs_diff(img.get_pixel(x as u32, y as u32).0[0]));
                }
            }
            let mean = diff as f64 / (cw * ch) as f64;
            assert!(mean < 3.0, "case {n} side {k}: mean difference {mean}");
        }
    }
}

#[test]
fn markups_are_in_the_file_only_when_asked_for() {
    use crate::annot::{add_one, Kind, NewAnnot, Style};
    use crate::engine::edit::EditSource;
    let d = dir("markups");
    let (eng, doc, _) = open_fixture(&d, 1);
    // an unsaved filled square
    let shape = NewAnnot::Shape { shape: Kind::Rectangle, points: vec![[100.0, 100.0], [300.0, 300.0]] };
    let style = Style { color: Some([0.0, 0.0, 0.0]), fill: Some([0.0, 0.0, 0.0]), ..Style::default() };
    add_one(&eng, doc, 0, &shape, &style, "tester", EditSource::User).unwrap();
    let base = settings(Layout::Size { mode: SizeMode::Actual, percent: 100.0 });
    let make = |content, name: &str| {
        let out = d.join(name);
        save(&eng, doc, &Settings { content, ..base.clone() }, &out).unwrap();
        out
    };
    let with = make(Content::DocumentAndMarkups, "with.pdf");
    let without = make(Content::Document, "without.pdf");
    let forms = make(Content::FormFieldsOnly, "forms.pdf");
    assert!(dark(&with, 0) > dark(&without, 0) + 20_000, "{} vs {}", dark(&with, 0), dark(&without, 0));
    assert!(dark(&without, 0) > 200);
    // the fixture has no form fields: nothing but paper
    assert_eq!(dark(&forms, 0), 0);
    // and the document still has its unsaved markup
    assert!(eng.doc_entry(doc).unwrap().generation() > 0);
}

#[test]
fn the_open_file_cannot_be_the_target_and_cancel_writes_nothing() {
    let d = dir("refusals");
    let (eng, doc, src) = open_fixture(&d, 2);
    let before = std::fs::read(&src).unwrap();
    let s = settings(Layout::default());
    let err = save(&eng, doc, &s, &src).unwrap_err().to_string();
    assert!(err.contains("open in Inkwell"), "{err}");
    assert_eq!(std::fs::read(&src).unwrap(), before);
    let out = d.join("never.pdf");
    let r = pdf::save(&eng, doc, &s, &out, &AtomicBool::new(true), &mut |_| {});
    assert!(matches!(r, Err(PrintError::Cancelled)));
    assert!(!out.exists());
    // a bad range is an error, not an empty file
    let bad = Settings { range: Range::Pages { text: "9".into() }, ..s };
    assert!(save(&eng, doc, &bad, &d.join("bad.pdf")).is_err());
    assert!(!d.join("bad.pdf").exists());
}

// ------------------------------------------------------- review follow-ups

#[test]
fn pages_not_exported_leave_no_comments_in_a_shared_resources_dict() {
    let d = dir("shared-annots");
    // two pages sharing one Resources object; the comment is on page 2
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 >>".to_string(),
        "<< /Font << /F1 6 0 R >> >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources 3 0 R /Contents 7 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources 3 0 R /Contents 8 0 R /Annots [9 0 R] >>".to_string(),
        HELV.to_string(),
        stream("", "BT /F1 24 Tf 72 700 Td (PAGEONE) Tj ET"),
        stream("", "BT /F1 24 Tf 72 700 Td (PAGETWO) Tj ET"),
        "<< /Type /Annot /Subtype /Square /Rect [100 100 300 300] /F 4 /C [1 0 0] /AP << /N 10 0 R >> >>".to_string(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Resources << /Font << /F1 6 0 R >> >>", "BT /F1 18 Tf 10 100 Td (SECRETNOTE) Tj ET"),
    ];
    let (eng, doc, _) = open_bytes(&d, "shared.pdf", &raw_pdf(&objs));
    let base = Settings { content: Content::DocumentAndMarkups, ..settings(Layout::default()) };
    let export = |range: &str, name: &str| {
        let out = d.join(name);
        save(&eng, doc, &Settings { range: Range::Pages { text: range.into() }, ..base.clone() }, &out).unwrap();
        out
    };
    // control: the comment is in the file when its page is exported
    let two = export("2", "two.pdf");
    assert!(streams_mention(&two, "SECRETNOTE"));
    // page 1 alone: nothing of page 2's comment, in any object or in the page's forms
    let one = export("1", "one.pdf");
    assert!(streams_mention(&one, "PAGEONE"));
    assert!(!streams_mention(&one, "SECRETNOTE"));
    let p = PdfDocument::open(one.to_string_lossy().as_ref()).unwrap();
    assert_eq!(p.page_count().unwrap(), 1);
    let form = first_form(&p, 0);
    if let Some(xo) = form.get_dict("Resources").unwrap().and_then(|r| r.get_dict("XObject").unwrap()) {
        for k in 0..xo.dict_len().unwrap() {
            let key = xo.get_dict_key(k as i32).unwrap().unwrap().as_name().unwrap();
            assert!(!key.starts_with(b"Annot"), "{}", String::from_utf8_lossy(&key));
        }
    }
}

#[test]
fn resources_inherited_from_the_page_tree_do_not_carry_an_unused_pages_comments() {
    let d = dir("inherited-annots");
    // the pages have no /Resources of their own: the /Pages node holds them
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources << /Font << /F1 5 0 R >> >> /MediaBox [0 0 612 792] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Annots [8 0 R] >>".to_string(),
        HELV.to_string(),
        stream("", "BT /F1 24 Tf 72 700 Td (PAGEONE) Tj ET"),
        stream("", "BT /F1 24 Tf 72 700 Td (PAGETWO) Tj ET"),
        "<< /Type /Annot /Subtype /Square /Rect [100 100 300 300] /F 4 /C [1 0 0] /AP << /N 9 0 R >> >>".to_string(),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >>", "BT /F1 18 Tf 10 100 Td (SECRETNOTE) Tj ET"),
    ];
    let (eng, doc, _) = open_bytes(&d, "inherited.pdf", &raw_pdf(&objs));
    let base = Settings { content: Content::DocumentAndMarkups, ..settings(Layout::default()) };
    let export = |range: &str, name: &str| {
        let out = d.join(name);
        save(&eng, doc, &Settings { range: Range::Pages { text: range.into() }, ..base.clone() }, &out).unwrap();
        out
    };
    assert!(streams_mention(&export("2", "two.pdf"), "SECRETNOTE"));
    let one = export("1", "one.pdf");
    assert!(streams_mention(&one, "PAGEONE"));
    assert!(!streams_mention(&one, "SECRETNOTE"));
}

#[test]
fn layers_that_are_off_stay_off_in_the_file() {
    let d = dir("layers");
    let bytes = one_page(
        "/OC /L1 BDC 0 g 36 36 300 300 re f EMC /OC /L2 BDC 0 g 400 600 100 100 re f EMC",
        "",
        "/Properties << /L1 6 0 R /L2 7 0 R >>",
        "/OCProperties << /OCGs [6 0 R 7 0 R] /D << /OFF [6 0 R] >> >>",
        &["<< /Type /OCG /Name (Hidden) >>".to_string(), "<< /Type /OCG /Name (Shown) >>".to_string()],
    );
    let (eng, doc, _) = open_bytes(&d, "layers.pdf", &bytes);
    let out = d.join("out.pdf");
    let s = settings(Layout::Size { mode: SizeMode::Actual, percent: 100.0 });
    save(&eng, doc, &s, &out).unwrap();
    // only the 100 x 100 square of the layer that is on
    let ink = dark(&out, 0);
    assert!((9_000..11_000).contains(&ink), "{ink}");
    // and the file carries the configuration, so another viewer agrees
    let o = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    let props = o.catalog().unwrap().get_dict("OCProperties").unwrap().expect("OCProperties");
    assert_eq!(props.get_dict("D").unwrap().unwrap().get_dict("OFF").unwrap().unwrap().len().unwrap(), 1);
}

#[test]
fn a_pages_transparency_group_comes_along_on_its_form() {
    let d = dir("group");
    let bytes = one_page("0.5 g 100 100 200 200 re f", "/Group << /S /Transparency /CS /DeviceRGB >>", "", "", &[]);
    let (eng, doc, _) = open_bytes(&d, "group.pdf", &bytes);
    let out = d.join("out.pdf");
    save(&eng, doc, &settings(Layout::default()), &out).unwrap();
    let o = PdfDocument::open(out.to_string_lossy().as_ref()).unwrap();
    let group = first_form(&o, 0).get_dict("Group").unwrap().expect("the form keeps the page's /Group");
    assert_eq!(group.get_dict("S").unwrap().unwrap().as_name().unwrap(), b"Transparency");
}

#[test]
fn a_turned_page_with_an_offset_crop_box_lands_where_the_preview_puts_it() {
    let d = dir("rotate-crop");
    let text = "BT /F1 20 Tf 120 120 Td (ALPHA) Tj ET BT /F1 20 Tf 300 450 Td (OMEGA) Tj ET";
    let page = "/Rotate 90 /CropBox [100 100 400 500]";
    // the same page with a big black square outside the crop box (inside the media box)
    let with = one_page(&format!("0 g 20 20 70 70 re f {text}"), page, "", "", &[]);
    let without = one_page(text, page, "", "", &[]);
    let (eng, doc, src) = open_bytes(&d, "with.pdf", &with);
    let (eng2, doc2, _) = open_bytes(&d, "without.pdf", &without);
    let s = settings(Layout::Size { mode: SizeMode::Actual, percent: 100.0 });
    let (a, b) = (d.join("a.pdf"), d.join("b.pdf"));
    save(&eng, doc, &s, &a).unwrap();
    save(&eng2, doc2, &s, &b).unwrap();
    // the crop box clips: outside it nothing is drawn
    assert_eq!(dark(&a, 0), dark(&b, 0));
    // both words moved by the same amount: the page was placed whole, at scale one, turned as shown
    let from = words(&src);
    let to = words(&a);
    let shift = |t: &str| {
        let f = from.iter().find(|w| w.0 == t).unwrap_or_else(|| panic!("{t} in the source: {from:?}"));
        let o = to.iter().find(|w| w.0 == t).unwrap_or_else(|| panic!("{t} in the file: {to:?}"));
        (o.1 - f.1, o.2 - f.2)
    };
    let (alpha, omega) = (shift("ALPHA"), shift("OMEGA"));
    assert!((alpha.0 - omega.0).abs() < 0.5 && (alpha.1 - omega.1).abs() < 0.5, "{alpha:?} {omega:?}");
    // a 400 x 300 page (300 x 400 crop, turned) centred on 612 x 792
    assert!((alpha.0 - 106.0).abs() < 1.5 && (alpha.1 - 246.0).abs() < 1.5, "{alpha:?}");
    // and it is what the preview draws
    let png = preview_png(&eng, doc, &s, 0, 792).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_luma8();
    let (w, h, px) = gray(&a, 0, 1.0);
    let mut diff = 0u64;
    for y in 0..h.min(img.height() as usize) {
        for x in 0..w.min(img.width() as usize) {
            diff += u64::from(px[y * w + x].abs_diff(img.get_pixel(x as u32, y as u32).0[0]));
        }
    }
    assert!(diff as f64 / ((w * h) as f64) < 3.0);
}

/// One page, `media` points wide and high, with the given page entries and a 200 x 200 square (user units) at 100, 100.
fn sized_page(media: (u32, u32), page: &str, content: &str) -> Vec<u8> {
    raw_pdf(&[
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents 4 0 R {page} >>", media.0, media.1),
        stream("", content),
    ])
}

#[test]
fn a_page_with_a_user_unit_prints_at_its_real_size() {
    let d = dir("user-unit");
    // /UserUnit 2: a 612 x 792 box in user units is a 1224 x 1584 pt page, and the square is 400 pt across
    let unit = sized_page((612, 792), "/UserUnit 2", "0 g 100 100 200 200 re f");
    // the same page written in points
    let plain = sized_page((1224, 1584), "", "2 0 0 2 0 0 cm 0 g 100 100 200 200 re f");
    let (eng, doc, _) = open_bytes(&d, "unit.pdf", &unit);
    let (eng2, doc2, _) = open_bytes(&d, "plain.pdf", &plain);
    let s = settings(Layout::default());
    let (a, b) = (d.join("a.pdf"), d.join("b.pdf"));
    save(&eng, doc, &s, &a).unwrap();
    save(&eng2, doc2, &s, &b).unwrap();
    let (ink_a, ink_b) = (dark(&a, 0), dark(&b, 0));
    assert!(ink_b > 1_000 && ink_a.abs_diff(ink_b) * 100 <= ink_b, "{ink_a} vs {ink_b}");
    // the sheet is the paper, and the page was fitted whole: the square covers the same share of it as in the twin
    let o = mupdf::Document::open(a.to_string_lossy().as_ref()).unwrap();
    assert!(near(f64::from(o.load_page(0).unwrap().bounds().unwrap().width()), 612.0));
    // and it is what the preview draws
    let png = preview_png(&eng, doc, &s, 0, 792).unwrap();
    let img = image::load_from_memory(&png).unwrap().to_luma8();
    let (w, h, px) = gray(&a, 0, 1.0);
    let mut diff = 0u64;
    for y in 0..h.min(img.height() as usize) {
        for x in 0..w.min(img.width() as usize) {
            diff += u64::from(px[y * w + x].abs_diff(img.get_pixel(x as u32, y as u32).0[0]));
        }
    }
    assert!(diff as f64 / ((w * h) as f64) < 3.0);
}

#[test]
fn form_fields_only_keeps_the_widgets_and_drops_page_and_markup() {
    let d = dir("fields");
    let bytes = one_page(
        "BT /F1 24 Tf 72 700 Td (BODYTEXT) Tj ET",
        "/Annots [6 0 R 7 0 R]",
        "",
        "/AcroForm << /Fields [6 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> >>",
        &[
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (f1) /V (WIDGETVALUE) /DA (/Helv 12 Tf 0 g) /Rect [50 600 250 630] /F 4 /P 3 0 R /MK << /BC [0 0 0] >> >>"
                .to_string(),
            "<< /Type /Annot /Subtype /Square /Rect [200 300 400 500] /F 4 /C [0 0 0] /IC [0 0 0] /AP << /N 9 0 R >> >>".to_string(),
            // object 8 is not used: the square's appearance is object 9
            "<< >>".to_string(),
            stream("/Type /XObject /Subtype /Form /BBox [0 0 200 200]", "0 g 0 0 200 200 re f"),
        ],
    );
    let (eng, doc, _) = open_bytes(&d, "fields.pdf", &bytes);
    let base = settings(Layout::Size { mode: SizeMode::Actual, percent: 100.0 });
    let make = |content, name: &str| {
        let out = d.join(name);
        save(&eng, doc, &Settings { content, ..base.clone() }, &out).unwrap();
        out
    };
    let forms = make(Content::FormFieldsOnly, "forms.pdf");
    let all = make(Content::DocumentAndMarkups, "all.pdf");
    let text = |p: &Path| words(p).into_iter().map(|w| w.0).collect::<Vec<_>>().join(" ");
    assert!(text(&forms).contains("WIDGETVALUE"), "{}", text(&forms));
    assert!(!text(&forms).contains("BODYTEXT"));
    assert!(dark(&forms, 0) < 2_000, "the square is not in the form-only file: {}", dark(&forms, 0));
    assert!(text(&all).contains("WIDGETVALUE") && text(&all).contains("BODYTEXT"));
    assert!(dark(&all, 0) > 38_000);
}

#[test]
fn a_document_that_forbids_copying_is_refused_before_a_file_is_made() {
    use crate::engine::edit::EditSource;
    use crate::protect::{Changes, Cipher, Printing, ProtectSpec};
    let d = dir("no-copy");
    let (eng, doc, _) = open_fixture(&d, 2);
    let spec = ProtectSpec {
        open_password: None,
        permissions_password: Some("owner".into()),
        printing: Printing::High,
        changes: Changes::Comments,
        copy: false,
        accessibility: true,
        cipher: Cipher::Aes256,
    };
    eng.protect(doc, spec, None, EditSource::User).unwrap();
    let locked = d.join("locked.pdf");
    eng.save_recorded(doc, Some(&locked.to_string_lossy())).unwrap();
    // reopened from disk the way a user would: printing is allowed, copying is not
    let eng2 = Engine::new(1, 1 << 20);
    let doc2 = eng2.open(&locked.to_string_lossy()).unwrap().doc;
    let out = d.join("out.pdf");
    let err = save(&eng2, doc2, &settings(Layout::default()), &out).unwrap_err();
    assert!(matches!(err, PrintError::Render(render::RenderError::NoCopy)), "{err}");
    assert!(!out.exists());
    // the printer path is not affected: the preview still draws
    assert!(preview_png(&eng2, doc2, &settings(Layout::default()), 0, 400).is_ok());
}
