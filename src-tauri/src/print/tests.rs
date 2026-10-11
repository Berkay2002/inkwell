//! Layout, page choice and sequencing are pure; the end-to-end tests print through Microsoft Print to PDF
//! into a scratch file and read the result back. They never address a real printer: a machine without
//! that driver skips them with a note.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::layout::*;
use super::*;
use crate::engine::fixture::{build, Fixture};

const LETTER: (f64, f64) = (612.0, 792.0);
const A4: (f64, f64) = (595.28, 841.89);

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.5
}

// ---------------------------------------------------------------- pure

#[test]
fn fit_centres_and_scales_to_the_printable_area() {
    // a Letter page on A4: width-limited
    let sheets = layout(&[LETTER], &[0], A4, Orientation::Auto, &Layout::default()).unwrap();
    assert_eq!(sheets.len(), 1);
    let (s, p) = (&sheets[0], sheets[0].placed[0]);
    assert!(near(s.w, A4.0) && near(s.h, A4.1));
    let fit = ((A4.0 - 36.0) / 612.0).min((A4.1 - 36.0) / 792.0);
    assert!(near(p.scale, fit));
    assert!(near(p.x, (A4.0 - 612.0 * fit) / 2.0) && near(p.y, (A4.1 - 792.0 * fit) / 2.0));
    assert!(!p.rotate);
}

#[test]
fn size_modes_actual_shrink_custom() {
    let big = (1224.0, 1584.0);
    let mk = |mode, percent| Layout::Size { mode, percent };
    let at = |sz, l: Layout| layout(&[sz], &[0], LETTER, Orientation::Auto, &l).unwrap()[0].placed[0].scale;
    assert!(near(at(big, mk(SizeMode::Actual, 100.0)), 1.0));
    assert!(at(big, mk(SizeMode::Shrink, 100.0)) < 1.0);
    // a small page is not enlarged by Shrink, but is by Fit
    assert!(near(at((306.0, 396.0), mk(SizeMode::Shrink, 100.0)), 1.0));
    assert!(at((306.0, 396.0), mk(SizeMode::Fit, 100.0)) > 1.0);
    assert!(near(at(LETTER, mk(SizeMode::Custom, 50.0)), 0.5));
    assert!(layout(&[LETTER], &[0], LETTER, Orientation::Auto, &mk(SizeMode::Custom, 0.0)).is_err());
}

#[test]
fn auto_orientation_turns_the_sheet_for_a_landscape_page() {
    let sheets = layout(&[LETTER, (792.0, 612.0)], &[0, 1], LETTER, Orientation::Auto, &Layout::default()).unwrap();
    assert!(!sheets[0].landscape() && sheets[1].landscape());
    let forced = layout(&[LETTER], &[0], LETTER, Orientation::Landscape, &Layout::default()).unwrap();
    assert!(forced[0].landscape());
    let portrait = layout(&[(792.0, 612.0)], &[0], LETTER, Orientation::Portrait, &Layout::default()).unwrap();
    assert!(!portrait[0].landscape());
}

#[test]
fn multiple_fills_cells_in_the_chosen_order() {
    let sizes = vec![LETTER; 5];
    let pages: Vec<u32> = (0..5).collect();
    let mk = |order| Layout::Multiple { per_sheet: 4, order, border: true, auto_rotate: true };
    let sheets = layout(&sizes, &pages, LETTER, Orientation::Auto, &mk(PageOrder::Horizontal)).unwrap();
    assert_eq!(sheets.len(), 2);
    assert_eq!(sheets[0].placed.len(), 4);
    assert_eq!(sheets[1].placed.len(), 1);
    assert_eq!(sheets[0].borders.len(), 4);
    let xy = |s: &Sheet| s.placed.iter().map(|p| ((p.x > 306.0) as u8, (p.y > 396.0) as u8)).collect::<Vec<_>>();
    assert_eq!(xy(&sheets[0]), [(0, 0), (1, 0), (0, 1), (1, 1)]);
    let v = layout(&sizes, &pages, LETTER, Orientation::Auto, &mk(PageOrder::Vertical)).unwrap();
    assert_eq!(xy(&v[0]), [(0, 0), (0, 1), (1, 0), (1, 1)]);
    let r = layout(&sizes, &pages, LETTER, Orientation::Auto, &mk(PageOrder::HorizontalReversed)).unwrap();
    assert_eq!(xy(&r[0]), [(1, 0), (0, 0), (1, 1), (0, 1)]);
    // every cell is inside the printable area and clips to its cell
    for p in &sheets[0].placed {
        assert!(p.clip[0] >= 18.0 - 0.01 && p.clip[2] <= 594.01 && p.clip[1] >= 17.99 && p.clip[3] <= 774.01);
        assert!(p.scale < 0.5);
    }
}

#[test]
fn two_per_sheet_uses_landscape_paper_and_rotates_portrait_pages_when_asked() {
    let l = Layout::Multiple { per_sheet: 2, order: PageOrder::Horizontal, border: false, auto_rotate: false };
    let s = layout(&[LETTER, LETTER], &[0, 1], LETTER, Orientation::Auto, &l).unwrap();
    assert_eq!(s.len(), 1);
    assert!(s[0].landscape());
    assert!(s[0].placed[0].x < s[0].placed[1].x && near(s[0].placed[0].y, s[0].placed[1].y));
    // a landscape page in a portrait cell is turned when auto-rotate is on
    let l = Layout::Multiple { per_sheet: 4, order: PageOrder::Horizontal, border: false, auto_rotate: true };
    let wide = (792.0, 612.0);
    let s = layout(&[wide; 4], &[0, 1, 2, 3], LETTER, Orientation::Portrait, &l).unwrap();
    // 2x2 cells of a portrait sheet are portrait-ish (a bit taller than wide): landscape pages turn
    assert!(s[0].placed.iter().all(|p| p.rotate));
}

#[test]
fn booklet_imposes_saddle_stitch_order() {
    let sizes = vec![LETTER; 8];
    let pages: Vec<u32> = (0..8).collect();
    let l = Layout::Booklet { sides: BookletSides::Both, binding: Binding::Left, creep: 0.0 };
    let sheets = layout(&sizes, &pages, LETTER, Orientation::Auto, &l).unwrap();
    assert_eq!(sheets.len(), 4);
    assert!(sheets.iter().all(Sheet::landscape));
    let order: Vec<Vec<u32>> = sheets.iter().map(|s| s.placed.iter().map(|p| p.page + 1).collect()).collect();
    assert_eq!(order, [vec![8, 1], vec![2, 7], vec![6, 3], vec![4, 5]]);
    // fronts only
    let l = Layout::Booklet { sides: BookletSides::FrontOnly, binding: Binding::Left, creep: 0.0 };
    assert_eq!(layout(&sizes, &pages, LETTER, Orientation::Auto, &l).unwrap().len(), 2);
    // right binding swaps the halves; a 5-page booklet pads to 8 with blanks
    let l = Layout::Booklet { sides: BookletSides::Both, binding: Binding::Right, creep: 0.0 };
    let five = layout(&sizes[..5], &[0, 1, 2, 3, 4], LETTER, Orientation::Auto, &l).unwrap();
    assert_eq!(five.len(), 4);
    let firsts: Vec<usize> = five.iter().map(|s| s.placed.len()).collect();
    assert_eq!(firsts, [1, 1, 1, 2]);
}

#[test]
fn rotated_placement_matrix_maps_the_top_edge_to_the_left_edge() {
    // a 100 x 200 page turned counter-clockwise at scale 1 with its corner at (10, 20)
    let p = Placement { page: 0, x: 10.0, y: 20.0, scale: 1.0, rotate: true, clip: [0.0; 4] };
    let m = p.matrix((0.0, 0.0), 100.0);
    let map = |u: f64, v: f64| (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]);
    // top-left (0,0) goes to the bottom-left, top-right (100,0) to the bottom... of the turned page:
    // the page is now 200 wide and 100 tall, spanning x 10..210, y 20..120
    assert_eq!(map(0.0, 0.0), (10.0, 120.0));
    assert_eq!(map(100.0, 0.0), (10.0, 20.0));
    assert_eq!(map(0.0, 200.0), (210.0, 120.0));
}

#[test]
fn page_choice_follows_acrobat() {
    let all = |r: Range, s, rev| select_pages(10, &r, s, rev);
    assert_eq!(all(Range::All, Subset::All, false).unwrap().len(), 10);
    assert_eq!(all(Range::Current { page: 3 }, Subset::All, false).unwrap(), [3]);
    let r = Range::Pages { text: "2-4, 8-".into() };
    assert_eq!(all(r.clone(), Subset::All, false).unwrap(), [1, 2, 3, 7, 8, 9]);
    // odd and even count among the chosen pages: the first chosen is "odd"
    assert_eq!(all(r.clone(), Subset::Odd, false).unwrap(), [1, 3, 8]);
    assert_eq!(all(r.clone(), Subset::Even, false).unwrap(), [2, 7, 9]);
    assert_eq!(all(r, Subset::All, true).unwrap(), [9, 8, 7, 3, 2, 1]);
    assert!(all(Range::Pages { text: "0".into() }, Subset::All, false).is_err());
    assert!(all(Range::Pages { text: "11".into() }, Subset::All, false).is_err());
    assert!(all(Range::Pages { text: "x".into() }, Subset::All, false).is_err());
    assert!(all(Range::Current { page: 10 }, Subset::All, false).is_err());
    // one page, even: nothing left
    assert_eq!(all(Range::Current { page: 0 }, Subset::Even, false), Err(LayoutError::NoPages));
}

#[test]
fn sequence_applies_copies_collation_and_two_sided() {
    let v = |a: &[Option<usize>]| a.to_vec();
    let s = |i| Some(i);
    assert_eq!(sequence(3, 1, true, false), [s(0), s(1), s(2)]);
    assert_eq!(sequence(2, 2, true, false), [s(0), s(1), s(0), s(1)]);
    assert_eq!(sequence(2, 2, false, false), [s(0), s(0), s(1), s(1)]);
    // two-sided: 3 sides is 2 papers, the last back blank; collated copies each start on a new paper
    assert_eq!(v(&sequence(3, 2, true, true)), [s(0), s(1), s(2), None, s(0), s(1), s(2), None]);
    // uncollated two-sided repeats whole papers, never splitting a front from its back
    assert_eq!(v(&sequence(4, 2, false, true)), [s(0), s(1), s(0), s(1), s(2), s(3), s(2), s(3)]);
    assert_eq!(sequence(1, 5000, true, false).len(), 999);
}

#[test]
fn paper_beyond_the_pdf_page_limit_is_rejected() {
    let l = Layout::default();
    assert!(layout(&[LETTER], &[0], (1e7, 1e7), Orientation::Auto, &l).is_err());
    assert!(layout(&[LETTER], &[0], (612.0, 14401.0), Orientation::Auto, &l).is_err());
    assert!(layout(&[LETTER], &[0], (f64::NAN, 792.0), Orientation::Auto, &l).is_err());
    assert!(layout(&[LETTER], &[0], (14400.0, 14400.0), Orientation::Auto, &l).is_ok());
}

#[test]
fn a_booklet_of_one_side_is_not_printed_two_sided() {
    let booklet = |sides| Layout::Booklet { sides, binding: Binding::Left, creep: 0.0 };
    let both = Settings { duplex: Duplex::LongEdge, layout: booklet(BookletSides::Both), ..Settings::default() };
    assert!(both.two_sided());
    for sides in [BookletSides::FrontOnly, BookletSides::BackOnly] {
        let one = Settings { layout: booklet(sides), ..both.clone() };
        assert!(!one.two_sided(), "{sides:?}");
    }
    // the same settings with an ordinary layout stay two-sided
    assert!(Settings { layout: Layout::default(), ..both }.two_sided());
}

#[test]
fn plan_info_counts_the_sides_sent() {
    let d = dir("sides");
    let (eng, doc) = open_fixture(&d, 3);
    let s = Settings { copies: 2, duplex: Duplex::LongEdge, ..Settings::default() };
    let info = plan_info(&eng, doc, &s).unwrap();
    // 3 sides make 2 papers with a blank back, twice
    assert_eq!((info.sheets, info.sides, info.papers), (3, 8, 4));
    // Save as PDF has one of each, padded to whole papers: 3 sides become 4 pages
    assert_eq!(info.pdf_sides, 4);
    let simplex = Settings { duplex: Duplex::Off, ..s };
    let info = plan_info(&eng, doc, &simplex).unwrap();
    assert_eq!((info.sides, info.papers, info.pdf_sides), (6, 6, 3));
}

#[cfg(windows)]
#[test]
fn the_devices_paper_replaces_an_unchosen_one_only_when_it_differs() {
    let a4 = Paper { id: None, width: A4.0, height: A4.1 };
    assert_eq!(device_paper(&a4, (595.0, 842.0)), None);
    // either orientation of the same paper is the same paper
    assert_eq!(device_paper(&a4, (842.0, 595.0)), None);
    let d = device_paper(&a4, (612.0, 792.0)).unwrap();
    assert_eq!((d.id, d.width, d.height), (None, 612.0, 792.0));
    // a chosen paper (a driver id) is never replaced; nor is nonsense from the device
    assert_eq!(device_paper(&Paper { id: Some(9), ..a4 }, (612.0, 792.0)), None);
    assert_eq!(device_paper(&a4, (0.0, 0.0)), None);
}

// ---------------------------------------------------------------- engine and printer

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-print-tests").join(test);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open_fixture(d: &Path, pages: usize) -> (std::sync::Arc<Engine>, u32) {
    let p = d.join("sample.pdf");
    std::fs::write(&p, build(&Fixture { pages, ..Default::default() })).unwrap();
    let eng = Engine::new(1, 1 << 20);
    let (doc, r) = eng.open_pending(&p.to_string_lossy());
    r.unwrap();
    (eng, doc)
}

fn png_size(b: &[u8]) -> (u32, u32) {
    image::load_from_memory(b).unwrap().to_rgb8().dimensions()
}

#[test]
fn preview_is_the_sheet_at_the_asked_size_and_shows_the_page() {
    let d = dir("preview");
    let (eng, doc) = open_fixture(&d, 3);
    let s = Settings { paper: Paper { id: None, width: A4.0, height: A4.1 }, ..Settings::default() };
    let info = plan_info(&eng, doc, &s).unwrap();
    assert_eq!((info.pages, info.sheets, info.papers), (3, 3, 3));
    assert!(near(info.faces[0].width, A4.0) && near(info.faces[0].height, A4.1));
    let png = preview_png(&eng, doc, &s, 0, 800).unwrap();
    let (w, h) = png_size(&png);
    assert_eq!(h, 800);
    assert!(near(w as f64, 800.0 * A4.0 / A4.1));
    let img = image::load_from_memory(&png).unwrap().to_luma8();
    let dark = img.pixels().filter(|p| p.0[0] < 128).count();
    assert!(dark > 200, "the fixture's text should be drawn ({dark} dark pixels)");
    // the printable margin stays paper-white
    assert!(img.pixels().take(w as usize * 4).all(|p| p.0[0] == 255));
    // a sheet past the end is an error, not a panic
    assert!(preview_png(&eng, doc, &s, 3, 800).is_err());
    // forms only: the fixture has none, so nothing but paper
    let f = Settings { content: Content::FormFieldsOnly, ..s.clone() };
    let blank = image::load_from_memory(&preview_png(&eng, doc, &f, 0, 400).unwrap()).unwrap().to_luma8();
    assert!(blank.pixels().all(|p| p.0[0] == 255));
    // grayscale previews are gray
    let g = Settings { grayscale: true, ..s };
    let rgb = image::load_from_memory(&preview_png(&eng, doc, &g, 0, 400).unwrap()).unwrap().to_rgb8();
    assert!(rgb.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]));
}

#[test]
fn n_up_preview_draws_one_sheet_for_four_pages() {
    let d = dir("nup");
    let (eng, doc) = open_fixture(&d, 4);
    let s = Settings {
        paper: Paper { id: None, width: LETTER.0, height: LETTER.1 },
        layout: Layout::Multiple { per_sheet: 4, order: PageOrder::Horizontal, border: true, auto_rotate: true },
        ..Settings::default()
    };
    let info = plan_info(&eng, doc, &s).unwrap();
    assert_eq!((info.sheets, info.faces[0].pages.clone()), (1, vec![1, 2, 3, 4]));
    let img = image::load_from_memory(&preview_png(&eng, doc, &s, 0, 792).unwrap()).unwrap().to_luma8();
    // page borders are drawn: a dark column at the left edge of the first cell
    let x = 18;
    assert!((100..200).any(|y| img.get_pixel(x, y).0[0] < 100));
}

#[test]
fn unsaved_markups_print_by_content_mode_and_survive_save_and_reopen() {
    use crate::annot::{add_one, Kind, NewAnnot, Style};
    use crate::engine::edit::EditSource;
    let d = dir("markup");
    let (eng, doc) = open_fixture(&d, 1);
    let shape = NewAnnot::Shape { shape: Kind::Rectangle, points: vec![[100.0, 100.0], [300.0, 300.0]] };
    let style = Style { color: Some([0.0, 0.0, 0.0]), fill: Some([0.0, 0.0, 0.0]), ..Style::default() };
    add_one(&eng, doc, 0, &shape, &style, "tester", EditSource::User).unwrap();
    let base = Settings { paper: Paper { id: None, width: LETTER.0, height: LETTER.1 }, layout: Layout::Size { mode: SizeMode::Actual, percent: 100.0 }, ..Settings::default() };
    let px = |s: &Settings, eng: &Engine, doc| image::load_from_memory(&preview_png(eng, doc, s, 0, 612).unwrap()).unwrap().to_luma8();
    let dark = |img: &image::GrayImage| img.pixels().filter(|p| p.0[0] < 64).count();
    // the unsaved square is on the sheet only when markups are asked for
    let with = px(&base, &eng, doc);
    let without = px(&Settings { content: Content::Document, ..base.clone() }, &eng, doc);
    assert!(dark(&with) > dark(&without) + 20_000, "{} vs {}", dark(&with), dark(&without));
    // saved, closed and opened again, the same sheet comes out
    eng.save(doc, None).unwrap();
    let path = eng.doc_entry(doc).unwrap().path();
    eng.close(doc);
    let (doc2, r) = eng.open_pending(&path);
    r.unwrap();
    assert_eq!(px(&base, &eng, doc2).into_raw(), with.into_raw());
}

#[cfg(windows)]
mod printer {
    use super::*;
    use crate::print::win;

    const PDF_PRINTER: &str = "Microsoft Print to PDF";

    fn available() -> bool {
        // Opt-in: these talk to the Windows spooler. INKWELL_PRINT_TESTS=1 runs them.
        if std::env::var_os("INKWELL_PRINT_TESTS").is_none() {
            eprintln!("skipped: set INKWELL_PRINT_TESTS=1 to print through {PDF_PRINTER}");
            return false;
        }
        let ok = win::printers().iter().any(|p| p.name == PDF_PRINTER);
        if !ok {
            eprintln!("skipped: {PDF_PRINTER} is not installed");
        }
        ok
    }

    /// Prints `pages` fixture pages under `s` into a file and returns the page sizes of the result.
    fn print(test: &str, pages: usize, s: Settings) -> Vec<(f32, f32)> {
        let d = dir(test);
        let (eng, doc) = open_fixture(&d, pages);
        let out = d.join("out.pdf");
        let s = Settings { printer: Some(PDF_PRINTER.into()), ..s };
        let mut last = 0;
        let r = run(&eng, doc, &s, "inkwell-test", Some(out.clone()), &AtomicBool::new(false), &mut |p| {
            assert!(p.done >= last && p.done <= p.total);
            last = p.done;
        })
        .unwrap();
        assert_eq!(r.printer, PDF_PRINTER);
        assert_eq!(last, r.sides);
        let doc = mupdf::Document::open(out.to_str().unwrap()).unwrap();
        let n = doc.page_count().unwrap();
        (0..n)
            .map(|i| {
                let b = doc.load_page(i).unwrap().bounds().unwrap();
                (b.width(), b.height())
            })
            .collect()
    }

    fn paper(id: u16, (w, h): (f64, f64)) -> Paper {
        Paper { id: Some(id), width: w, height: h }
    }

    #[test]
    fn lists_the_pdf_printer_and_its_papers() {
        if !available() {
            return;
        }
        let list = win::printers();
        assert!(list.iter().any(|p| p.name == PDF_PRINTER && p.to_file));
        assert_eq!(list.iter().filter(|p| p.is_default).count(), 1.min(list.len()));
        let caps = win::capabilities(PDF_PRINTER).expect("capabilities");
        let a4 = caps.papers.iter().find(|p| p.id == 9).expect("A4");
        assert!(near(a4.width, 595.3) && near(a4.height, 841.9), "{a4:?}");
        assert!(caps.papers.iter().any(|p| p.id == 1), "Letter");
    }

    #[test]
    fn prints_every_page_at_the_paper_size() {
        if !available() {
            return;
        }
        let sizes = print("all", 3, Settings { paper: paper(9, A4), ..Settings::default() });
        assert_eq!(sizes.len(), 3);
        for (w, h) in sizes {
            assert!((w - 595.0).abs() < 3.0 && (h - 842.0).abs() < 3.0, "{w}x{h}");
        }
    }

    #[test]
    fn range_copies_collate_and_reverse_set_the_page_count() {
        if !available() {
            return;
        }
        let base = Settings { paper: paper(1, LETTER), ..Settings::default() };
        let r = Range::Pages { text: "2-3".into() };
        assert_eq!(print("range", 4, Settings { range: r.clone(), ..base.clone() }).len(), 2);
        assert_eq!(print("copies", 4, Settings { range: r.clone(), copies: 3, ..base.clone() }).len(), 6);
        assert_eq!(print("odd", 5, Settings { subset: Subset::Odd, ..base.clone() }).len(), 3);
        assert_eq!(print("rev", 3, Settings { reverse: true, collate: false, copies: 2, ..base }).len(), 6);
    }

    #[test]
    fn layouts_and_orientation_set_the_sheet_sizes() {
        if !available() {
            return;
        }
        let base = Settings { paper: paper(1, LETTER), ..Settings::default() };
        let land = print("landscape", 2, Settings { orientation: Orientation::Landscape, ..base.clone() });
        assert_eq!(land.len(), 2);
        assert!(land.iter().all(|(w, h)| w > h && (w - 792.0).abs() < 3.0), "{land:?}");
        let up = print(
            "4up",
            8,
            Settings { layout: Layout::Multiple { per_sheet: 4, order: PageOrder::Horizontal, border: false, auto_rotate: true }, ..base.clone() },
        );
        assert_eq!(up.len(), 2);
        assert!(up.iter().all(|(w, h)| h > w));
        let book = print(
            "booklet",
            8,
            Settings { layout: Layout::Booklet { sides: BookletSides::Both, binding: Binding::Left, creep: 0.0 }, ..base },
        );
        assert_eq!(book.len(), 4);
        assert!(book.iter().all(|(w, h)| w > h));
    }

    #[test]
    fn cancel_before_the_first_sheet_prints_nothing() {
        if !available() {
            return;
        }
        let d = dir("cancel");
        let (eng, doc) = open_fixture(&d, 3);
        let out = d.join("out.pdf");
        let s = Settings { printer: Some(PDF_PRINTER.into()), paper: paper(9, A4), ..Settings::default() };
        let r = run(&eng, doc, &s, "t", Some(out.clone()), &AtomicBool::new(true), &mut |_| {});
        assert!(matches!(r, Err(PrintError::Cancelled)));
        assert!(!out.exists());
    }

    #[test]
    fn an_unknown_printer_is_an_error_not_a_crash() {
        let d = dir("noprinter");
        let (eng, doc) = open_fixture(&d, 1);
        let s = Settings { printer: Some("No Such Printer 9".into()), ..Settings::default() };
        let r = run(&eng, doc, &s, "t", None, &AtomicBool::new(false), &mut |_| {});
        assert!(matches!(r, Err(PrintError::Printer(_))));
    }
}
