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

fn open_fixture(d: &Path, pages: usize) -> (std::sync::Arc<Engine>, u32, PathBuf) {
    let p = d.join("sample.pdf");
    std::fs::write(&p, build(&Fixture { pages, ..Default::default() })).unwrap();
    let eng = Engine::new(1, 1 << 20);
    let (doc, r) = eng.open_pending(&p.to_string_lossy());
    r.unwrap();
    (eng, doc, p)
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
