//! Generated PDFs: a spot colour page lists its spot and switching a separation changes pixels, overprint
//! is simulated, a 4 x 100% patch reads 400% TAC, the inspector names what is at a point, colour
//! conversion leaves no DeviceRGB, boxes validate, and printer's marks fall outside the trim box.

use super::app::Prepress;
use super::*;
use crate::a11y::Progress;
use crate::engine::cache::TileKey;
use crate::engine::edit::EditSource;
use crate::engine::{DocId, Engine, TileOutcome};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

// ---- fixtures -------------------------------------------------------------------------------------

/// Object numbers: 1 catalog, 2 pages, 3 Helvetica, page i at 10 + 2i with its content at 11 + 2i; extras from 100.
fn build(resources: &str, pages: &[(&str, &str)], extra: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut objs: Vec<(usize, Vec<u8>)> = vec![
        (1, b"<</Type/Catalog/Pages 2 0 R>>".to_vec()),
        (3, b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>".to_vec()),
    ];
    let kids: String = (0..pages.len()).map(|i| format!("{} 0 R ", 10 + 2 * i)).collect();
    objs.push((2, format!("<</Type/Pages/Kids[{kids}]/Count {}>>", pages.len()).into_bytes()));
    for (i, (entries, content)) in pages.iter().enumerate() {
        objs.push((
            10 + 2 * i,
            format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Resources<</Font<</F1 3 0 R>> {resources}>>/Contents {} 0 R {entries}>>", 11 + 2 * i).into_bytes(),
        ));
        objs.push((11 + 2 * i, stream("", content.as_bytes())));
    }
    objs.extend(extra.iter().cloned());
    objs.sort_by_key(|(n, _)| *n);
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let size = objs.last().map_or(1, |o| o.0) + 1;
    let mut offsets = vec![None; size];
    for (n, body) in &objs {
        offsets[*n] = Some(out.len());
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let mut t = format!("xref\n0 {size}\n");
    for o in &offsets {
        match o {
            Some(o) => {
                let _ = write!(t, "{o:010} 00000 n \n");
            }
            None => t.push_str("0000000000 65535 f \n"),
        }
    }
    let _ = write!(t, "trailer\n<</Size {size}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n");
    out.extend_from_slice(t.as_bytes());
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut v = format!("<<{dict}/Length {}>>\nstream\n", data.len()).into_bytes();
    v.extend_from_slice(data);
    v.extend_from_slice(b"\nendstream");
    v
}

const SPOT: &str = "/ColorSpace<</CS0[/Separation/PANTONE#20123/DeviceCMYK<</FunctionType 2/Domain[0 1]/C0[0 0 0 0]/C1[0 0.5 1 0]/N 1>>]>>\
 /ExtGState<</GS0<</Type/ExtGState/OP true/op true/OPM 1>>>>";

/// A page with a spot patch (50..200 x 500..650), a cyan patch (250..400 x 500..650) and a black patch that
/// overprints it (280..340 x 520..580). In a tile, y counts from the top: pdf y 650 is row 142.
const SPOT_PAGE: &str = "/CS0 cs 1 scn 50 500 150 150 re f\n1 0 0 0 k 250 500 150 150 re f\n/GS0 gs 0 0 0 1 k 280 520 60 60 re f\n";

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("prepress-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open(test: &str, bytes: &[u8]) -> (Arc<Engine>, DocId) {
    let p = dir(test).join("doc.pdf");
    std::fs::write(&p, bytes).unwrap();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&p.to_string_lossy()).unwrap().doc;
    (eng, id)
}

fn key(eng: &Engine, doc: DocId) -> TileKey {
    TileKey { doc, rev: eng.doc_entry(doc).unwrap().generation(), page: 0, rot: 0, scale: 1000, tx: 0, ty: 0 }
}

/// RGB of the pixel at (x, y) in the first tile of page 0.
fn pixel(eng: &Engine, doc: DocId, view: &View, x: usize, y: usize) -> [u8; 3] {
    let seps = page_separations(eng, doc, 0).unwrap();
    let t = render_view_tile(eng, &seps, &key(eng, doc), view).unwrap();
    let i = (y * t.w as usize + x) * 4;
    [t.rgba[i], t.rgba[i + 1], t.rgba[i + 2]]
}

fn only(process: [bool; 4], hide_spots: bool) -> View {
    View { process, hidden_spots: if hide_spots { vec!["PANTONE 123".into()] } else { vec![] }, ..View::default() }
}

fn no_progress() -> impl FnMut(Progress) {
    |_| {}
}

fn contents(eng: &Engine, doc: DocId, page: u32) -> String {
    eng.read_custom(doc, |pdf| {
        let p = pdf.find_page(page as i32)?;
        let mut s = String::new();
        let c = p.get_dict("Contents")?.unwrap();
        let list: Vec<_> = if c.is_array()? { (0..c.len()?).filter_map(|i| c.get_array(i as i32).ok().flatten()).collect() } else { vec![c] };
        for o in list {
            s.push_str(&String::from_utf8_lossy(&o.read_stream()?));
            s.push('\n');
        }
        Ok(s)
    })
    .unwrap()
}

fn boxes(eng: &Engine, doc: DocId, page: u32, kind: BoxKind) -> Option<[f32; 4]> {
    eng.read_custom(doc, |pdf| super::boxes::stated(&pdf.find_page(page as i32)?, kind)).unwrap()
}

// ---- separations and plates ----------------------------------------------------------------------

#[test]
fn a_spot_colour_page_lists_its_spot() {
    let (eng, doc) = open("list", &build(SPOT, &[("", SPOT_PAGE)], &[]));
    let s = page_separations(&eng, doc, 0).unwrap();
    assert_eq!(s.process, ["Cyan", "Magenta", "Yellow", "Black"]);
    assert_eq!(s.spots.len(), 1);
    assert_eq!(s.spots[0].name, "PANTONE 123");
    // the equivalent of (0, .5, 1, 0) on screen: orange
    let rgb = s.spots[0].rgb;
    assert!(rgb[0] > 200 && (100..200).contains(&rgb[1]) && rgb[2] < 80, "{rgb:?}");
    assert_eq!(s.spots[0].cmyk, [0, 50, 100, 0]);
    assert_eq!(s.plane_of("PANTONE 123"), Some(4));
    assert!(s.overprint, "the page sets overprint");
    // a page without spots lists none
    let (eng, doc) = open("list-plain", &build("", &[("", "1 0 0 0 k 0 0 10 10 re f")], &[]));
    assert!(page_separations(&eng, doc, 0).unwrap().spots.is_empty());
    assert!(matches!(page_separations(&eng, doc, 3), Err(crate::engine::EngineError::BadPage(3))));
}

#[test]
fn toggling_a_separation_changes_the_pixels_it_paints() {
    let (eng, doc) = open("plates", &build(SPOT, &[("", SPOT_PAGE)], &[]));
    let spot = (125, 217);
    let cyan = (325, 190);
    let all = View::default();
    let orange = page_separations(&eng, doc, 0).unwrap().spots[0].rgb;
    assert_eq!(pixel(&eng, doc, &all, spot.0, spot.1), orange, "the spot patch in its own colour");
    assert_eq!(pixel(&eng, doc, &all, cyan.0, cyan.1), [0, 255, 255]);
    assert_eq!(pixel(&eng, doc, &all, 5, 5), [255, 255, 255], "paper");
    // the spot off: its patch is paper, the cyan patch stays
    let no_spot = only([true; 4], true);
    assert_eq!(pixel(&eng, doc, &no_spot, spot.0, spot.1), [255, 255, 255]);
    assert_eq!(pixel(&eng, doc, &no_spot, cyan.0, cyan.1), [0, 255, 255]);
    // cyan off: its patch is paper, the spot stays
    let no_cyan = View { process: [false, true, true, true], ..View::default() };
    assert_eq!(pixel(&eng, doc, &no_cyan, cyan.0, cyan.1), [255, 255, 255]);
    assert_eq!(pixel(&eng, doc, &no_cyan, spot.0, spot.1), orange);
    // only the spot, only black
    assert_eq!(pixel(&eng, doc, &only([false; 4], false), spot.0, spot.1), orange);
    assert_eq!(pixel(&eng, doc, &only([false; 4], false), cyan.0, cyan.1), [255, 255, 255]);
    assert_eq!(pixel(&eng, doc, &only([false, false, false, true], true), 310, 242), [0, 0, 0]);
}

#[test]
fn overprint_keeps_the_plate_below_and_knockout_does_not() {
    let (eng, doc) = open("overprint", &build(SPOT, &[("", SPOT_PAGE)], &[]));
    // the black patch lies on the cyan one with overprint mode 1: the cyan plate shows through it
    let cyan_plate = only([true, false, false, false], true);
    assert_eq!(pixel(&eng, doc, &cyan_plate, 310, 242), [0, 255, 255], "overprinted: cyan stays under the black");
    let knockout = View { overprint: false, ..cyan_plate.clone() };
    assert_eq!(pixel(&eng, doc, &knockout, 310, 242), [255, 255, 255], "knocked out: the black replaced it");
    // with every plate on the black patch looks black either way
    assert_eq!(pixel(&eng, doc, &View::default(), 310, 242), [0, 0, 0]);
    // without overprint the spot is folded into CMYK: it still shows
    let folded = pixel(&eng, doc, &View { overprint: false, ..View::default() }, 125, 217);
    assert!(folded[0] > 200 && folded[2] < 80, "{folded:?}");
}

#[test]
fn a_coverage_tile_is_red_above_the_limit_and_white_on_paper() {
    let page = "1 1 1 1 k 100 500 100 100 re f\n0.2 0.2 0.2 0.2 k 300 500 100 100 re f\n";
    let (eng, doc) = open("heat", &build("", &[("", page)], &[]));
    let heat = View { mode: Mode::Coverage, ..View::default() };
    assert_eq!(pixel(&eng, doc, &heat, 150, 242), [214, 24, 28], "400% is over 300%");
    assert_eq!(pixel(&eng, doc, &heat, 5, 5), [255, 255, 255]);
    let low = pixel(&eng, doc, &heat, 350, 242);
    assert!(low != [214, 24, 28] && low != [255, 255, 255], "80% is a colour of the ramp: {low:?}");
    // a higher limit moves the patch under it
    assert_ne!(pixel(&eng, doc, &View { tac_limit: 400.0, ..heat }, 150, 242), [214, 24, 28]);
}

// ---- coverage ------------------------------------------------------------------------------------

#[test]
fn a_four_hundred_per_cent_patch_reads_four_hundred() {
    let rich = "1 1 1 1 k 100 500 100 100 re f\n";
    let (eng, doc) = open("tac", &build("", &[("", rich), ("", "0.5 0.5 0.5 0.5 k 100 500 100 100 re f\n"), ("", "")], &[]));
    let r = ink_report(&eng, doc, 0, 300.0, 1600, false).unwrap();
    assert!((r.max_tac - 400.0).abs() < 0.5, "max TAC {}", r.max_tac);
    assert!(r.over_limit);
    // the densest point is inside the patch (page points from the top-left: y 792 - 600 .. 792 - 500)
    assert!((100.0..=200.0).contains(&r.max_at[0]) && (192.0..=292.0).contains(&r.max_at[1]), "{:?}", r.max_at);
    assert!((r.over_limit_area - (100.0 * 100.0) / (612.0 * 792.0)).abs() < 0.002, "{}", r.over_limit_area);
    assert!(!r.regions.is_empty() && r.regions.iter().all(|b| b[0] >= 90.0 && b[2] <= 215.0 && b[1] >= 165.0 && b[3] <= 300.0), "{:?}", r.regions);
    for p in &r.planes {
        assert!((p.max - 100.0).abs() < 0.5, "{} {}", p.name, p.max);
    }
    // 4 x 50% is 200%: under the limit
    let half = ink_report(&eng, doc, 1, 300.0, 1600, false).unwrap();
    assert!((half.max_tac - 200.0).abs() < 1.0 && !half.over_limit && half.regions.is_empty());
    // an empty page has no ink
    let blank = ink_report(&eng, doc, 2, 300.0, 1600, false).unwrap();
    assert_eq!((blank.max_tac, blank.over_limit_area), (0.0, 0.0));
    // all pages, with progress and a cancel
    let mut seen = Vec::new();
    let all = doc_ink(&eng, doc, None, 300.0, 800, false, &mut |p| seen.push(p.done), &AtomicBool::new(false)).unwrap();
    assert_eq!((all.len(), seen), (3, vec![1, 2, 3]));
    assert!(doc_ink(&eng, doc, None, 300.0, 800, false, &mut no_progress(), &AtomicBool::new(true)).is_err());
}

#[test]
fn a_spot_adds_to_the_total_and_is_listed_per_plane() {
    let page = "0 0 0 1 k 100 500 100 100 re f\n/CS0 cs 1 scn 100 500 100 100 re f\n";
    let (eng, doc) = open("tac-spot", &build(SPOT, &[("", page)], &[]));
    let r = ink_report(&eng, doc, 0, 300.0, 1600, false).unwrap();
    // the spot paints over the black (no overprint set): knocked out, the spot alone is 100%
    assert!((r.max_tac - 100.0).abs() < 0.5, "{}", r.max_tac);
    assert_eq!(r.spots, ["PANTONE 123"]);
    assert_eq!(r.planes.len(), 5);
    assert_eq!(r.planes[4].name, "PANTONE 123");
    assert!((r.planes[4].max - 100.0).abs() < 0.5 && r.planes[3].max < 0.5);
}

// ---- the inspector -------------------------------------------------------------------------------

#[test]
fn the_inspector_names_the_colour_space_values_and_overprint() {
    let page = "0.2 0.4 0.6 rg 50 100 100 100 re f\n/CS0 cs 0.8 scn 200 100 100 100 re f\n/GS0 gs 1 0 0 0 k 350 100 100 100 re f\nBT /F1 24 Tf 0 g 50 400 Td (Hello) Tj ET\n";
    let (eng, doc) = open("inspect", &build(SPOT, &[("", page)], &[]));
    // rgb: page point (100, 792 - 150)
    let rgb = inspect_at(&eng, doc, 0, 100.0, 642.0, 8).unwrap();
    assert_eq!(rgb.len(), 1);
    assert_eq!((rgb[0].kind, rgb[0].model, rgb[0].color_space.as_str()), (ObjectKind::Fill, "rgb", "DeviceRGB"));
    assert_eq!(rgb[0].values, [0.2, 0.4, 0.6]);
    let c = rgb[0].cmyk.unwrap();
    assert!(c[0] > 50.0 && c[3] < 5.0, "{c:?}");
    assert!(rgb[0].ink.unwrap() > 100.0);
    assert!(!rgb[0].overprint);
    for (got, want) in rgb[0].bounds.iter().zip([50.0, 592.0, 150.0, 692.0]) {
        assert!((got - want).abs() <= 1.0, "{:?}", rgb[0].bounds);
    }
    // the spot patch
    let spot = inspect_at(&eng, doc, 0, 250.0, 642.0, 8).unwrap();
    assert_eq!((spot[0].model, spot[0].colorants.clone()), ("spot", vec!["PANTONE 123".to_string()]));
    assert_eq!(spot[0].values, [0.8]);
    assert!(!spot[0].overprint);
    // cmyk with overprint on
    let op = inspect_at(&eng, doc, 0, 400.0, 642.0, 8).unwrap();
    assert_eq!((op[0].model, op[0].overprint, op[0].overprint_mode), ("cmyk", true, true));
    assert_eq!(op[0].cmyk, Some([100.0, 0.0, 0.0, 0.0]));
    // text, with its font
    let t = inspect_at(&eng, doc, 0, 53.0, 383.0, 8).unwrap();
    assert!(t.iter().any(|o| o.kind == ObjectKind::Text && o.font.as_deref().is_some_and(|f| f.contains("Helvetica"))), "{t:?}");
    // nothing there, and outside the page
    assert!(inspect_at(&eng, doc, 0, 500.0, 50.0, 8).unwrap().is_empty());
    assert!(inspect_at(&eng, doc, 0, 700.0, 50.0, 8).is_err());
}

#[test]
fn the_inspector_lists_what_overlaps_topmost_first_and_honours_clips() {
    let page = "0 0 1 0 k 100 500 200 200 re f\n1 0 0 0 k 150 550 100 100 re f\nq 400 500 50 50 re W n 0 1 0 0 k 380 480 100 100 re f Q\n";
    let (eng, doc) = open("inspect-stack", &build("", &[("", page)], &[]));
    let hits = inspect_at(&eng, doc, 0, 200.0, 792.0 - 600.0, 8).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].cmyk, Some([100.0, 0.0, 0.0, 0.0]), "the cyan patch is on top");
    assert_eq!(hits[1].cmyk, Some([0.0, 0.0, 100.0, 0.0]));
    assert_eq!(inspect_at(&eng, doc, 0, 200.0, 792.0 - 600.0, 1).unwrap().len(), 1);
    // the magenta patch is clipped to 400..450 x 500..550
    assert_eq!(inspect_at(&eng, doc, 0, 420.0, 792.0 - 520.0, 8).unwrap().len(), 1);
    assert!(inspect_at(&eng, doc, 0, 470.0, 792.0 - 490.0, 8).unwrap().is_empty(), "outside the clip");
}

// ---- convert colours -----------------------------------------------------------------------------

const MIXED: &str = "0.2 0.4 0.6 rg 50 600 100 100 re f\n0.5 g 200 600 100 100 re f\n0 g 350 600 50 50 re f\n0 0 0 rg 420 600 50 50 re f\n\
/CS0 cs 0.8 scn 50 450 100 100 re f\n1 0 0 RG 4 w 200 450 m 300 550 l S\nBT /F1 24 Tf 0.1 0.7 0.3 rg 50 380 Td (Hello) Tj ET\n\
q 100 0 0 100 400 400 cm /Im0 Do Q\nq 1 0 0 1 300 250 cm 0 0 100 100 re W n /Sh0 sh Q\n";

fn mixed() -> Vec<u8> {
    let res = format!(
        "{SPOT} /XObject<</Im0 102 0 R>> /Shading<</Sh0 103 0 R>>"
    );
    let image: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 200, 200, 200];
    build(
        &res,
        &[("", MIXED)],
        &[
            (102, stream("/Type/XObject/Subtype/Image/Width 2/Height 2/ColorSpace/DeviceRGB/BitsPerComponent 8", &image)),
            (103, b"<</ShadingType 2/ColorSpace/DeviceRGB/Coords[0 0 100 0]/Function<</FunctionType 2/Domain[0 1]/C0[1 0 0]/C1[0 0 1]/N 1>>/Extend[true true]>>".to_vec()),
        ],
    )
}

fn spec(rgb: bool, gray: bool, spot: bool) -> ConvertSpec {
    ConvertSpec { pages: None, rgb, gray, spot, gray_to_k: true, preserve_black: true, intent: IntentSource::Device, condition: None }
}

fn convert(eng: &Engine, doc: DocId, s: &ConvertSpec, profile: Option<&[u8]>) -> Result<(crate::engine::edit::EditState, ConvertReport)> {
    convert_colors(eng, doc, s, profile, EditSource::User, &mut no_progress(), &AtomicBool::new(false))
}

/// A CMYK profile MuPDF itself ships (vendor/mupdf-sys/mupdf/source/fitz/icc/): `cmyk` is the one its own
/// DeviceCMYK conversions use, `ps_cmyk` a small different one.
fn cmyk_profile(name: &str) -> Vec<u8> {
    let path = format!("{}/../vendor/mupdf-sys/mupdf/source/fitz/icc/{name}.icc.h", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(path).unwrap();
    let body = &text[text.find('{').unwrap() + 1..text.rfind('}').unwrap()];
    body.split(',').filter_map(|t| u8::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok()).collect()
}

fn survey(eng: &Engine, doc: DocId) -> [bool; 3] {
    eng.read_custom(doc, |pdf| crate::preflight::device_colours(pdf, &mut no_progress(), &AtomicBool::new(false))).unwrap()
}

#[test]
fn conversion_leaves_no_device_rgb_and_is_one_undo_step() {
    let (eng, doc) = open("convert", &mixed());
    assert_eq!(survey(&eng, doc), [true, true, false], "before: gray and RGB are in use");
    let before = contents(&eng, doc, 0);
    assert!(before.contains(" rg") && before.contains(" RG"));
    let (state, report) = convert(&eng, doc, &spec(true, true, true), None).unwrap();
    assert_eq!(report.pages, [0]);
    assert_eq!(report.remaining, [false, false, true], "only DeviceCMYK is left");
    assert_eq!(state.undo.as_deref(), Some("Convert colours to CMYK"));
    let after = contents(&eng, doc, 0);
    for op in [" rg", " RG", " g\n", " G\n"] {
        assert!(!after.contains(op), "{op:?} is still in\n{after}");
    }
    assert!(after.contains(" k"), "{after}");
    // the images and the shading are CMYK too
    let spaces = eng
        .read_custom(doc, |pdf| {
            let page = pdf.find_page(0)?;
            let res = page.get_dict("Resources")?.unwrap();
            let mut names = Vec::new();
            for dict in ["XObject", "Shading"] {
                if let Some(d) = res.get_dict(dict)? {
                    for i in 0..d.dict_len()? {
                        let o = d.get_dict_val(i as i32)?.unwrap();
                        if let Some(cs) = o.get_dict("ColorSpace")? {
                            names.push(format!("{}", if cs.is_name()? { String::from_utf8_lossy(&cs.as_name()?).into_owned() } else { format!("{cs}") }));
                        }
                    }
                }
            }
            Ok(names)
        })
        .unwrap();
    assert!(!spaces.is_empty() && spaces.iter().all(|n| n == "DeviceCMYK"), "{spaces:?}");
    // the spot is process now: the page lists no separation
    assert!(page_separations(&eng, doc, 0).unwrap().spots.is_empty());
    // pure black became K alone
    assert!(after.contains("0 0 0 1 k"), "{after}");
    // and it undoes
    eng.undo(doc).unwrap();
    assert_eq!(contents(&eng, doc, 0), before);
    assert_eq!(page_separations(&eng, doc, 0).unwrap().spots.len(), 1);
    assert!(survey(&eng, doc)[1]);
}

#[test]
fn conversion_picks_what_it_converts_and_survives_a_save() {
    let (eng, doc) = open("convert-pick", &mixed());
    // rgb only: gray and the spot stay
    let (_, r) = convert(&eng, doc, &spec(true, false, false), None).unwrap();
    assert_eq!(r.remaining, [true, false, true]);
    assert_eq!(page_separations(&eng, doc, 0).unwrap().spots.len(), 1);
    // then the rest
    let (_, r) = convert(&eng, doc, &spec(false, true, true), None).unwrap();
    assert_eq!(r.remaining, [false, false, true]);
    eng.save(doc, None).unwrap();
    let path = eng.doc_entry(doc).unwrap().path();
    let reopened = mupdf::pdf::PdfDocument::open(&path).unwrap();
    let again = crate::preflight::device_colours(&reopened, &mut no_progress(), &AtomicBool::new(false)).unwrap();
    assert_eq!(again, [false, false, true]);
    // nothing picked is refused
    assert!(convert(&eng, doc, &spec(false, false, false), None).is_err());
}

#[test]
fn a_profile_converts_through_its_values_and_becomes_the_output_intent() {
    let profile = cmyk_profile("ps_cmyk");
    assert!(profile.len() > 4_000 && &profile[16..20] == b"CMYK");
    let rgb = "0.2 0.4 0.6 rg 50 600 100 100 re f\n";
    let (eng_a, a) = open("profile-a", &build("", &[("", rgb)], &[]));
    let (eng_b, b) = open("profile-b", &build("", &[("", rgb)], &[]));
    convert(&eng_a, a, &spec(true, false, false), None).unwrap();
    let with_profile = ConvertSpec { intent: IntentSource::Profile, condition: Some("Test CMYK".into()), ..spec(true, false, false) };
    let (_, r) = convert(&eng_b, b, &with_profile, Some(&profile)).unwrap();
    assert!(r.used_profile);
    let value = |eng: &Engine, doc: DocId| -> Vec<f32> {
        let c = contents(eng, doc, 0);
        let line = c.lines().find(|l| l.contains(" k ") && !l.starts_with("0 0 0 1")).unwrap_or_else(|| panic!("{c}")).to_string();
        line.split_whitespace().take(4).map(|v| v.parse().unwrap()).collect()
    };
    let (plain, icc) = (value(&eng_a, a), value(&eng_b, b));
    let diff: f32 = plain.iter().zip(&icc).map(|(x, y)| (x - y).abs()).sum();
    assert!(diff > 0.1, "the profile changes the values: {plain:?} vs {icc:?}");
    assert!(icc.iter().all(|v| (0.0..=1.0).contains(v)));
    // the intent is in the file: GTS_PDFX with a 4-component profile
    let (n, s, id) = eng_b
        .read_custom(b, |pdf| {
            let oi = pdf.catalog()?.get_dict("OutputIntents")?.unwrap().get_array(0)?.unwrap();
            let p = oi.get_dict("DestOutputProfile")?.unwrap();
            Ok((
                p.get_dict("N")?.unwrap().as_int()?,
                String::from_utf8_lossy(&oi.get_dict("S")?.unwrap().as_name()?).into_owned(),
                oi.get_dict("OutputConditionIdentifier")?.unwrap().as_string()?,
            ))
        })
        .unwrap();
    assert_eq!((n, s.as_str(), id.as_str()), (4, "GTS_PDFX", "Test CMYK"));
    // converting a second document "to the output intent" of the first uses the same values
    let (_, again) = convert(&eng_b, b, &ConvertSpec { intent: IntentSource::Document, ..spec(true, true, true) }, None).unwrap();
    assert!(again.used_profile);
    // a document without an output intent cannot, and a file that is not a CMYK profile is refused
    assert!(convert(&eng_a, a, &ConvertSpec { intent: IntentSource::Document, ..spec(true, true, true) }, None).is_err());
    assert!(convert(&eng_a, a, &with_profile, Some(&profile[..64])).is_err());
    assert!(convert(&eng_a, a, &with_profile, None).is_err());
    let mut rgb_profile = profile.clone();
    rgb_profile[16..20].copy_from_slice(b"RGB ");
    assert!(convert(&eng_a, a, &with_profile, Some(&rgb_profile)).is_err());
}

#[test]
fn a_transparency_group_in_rgb_is_set_to_cmyk() {
    let (eng, doc) = open("group", &build("", &[("/Group<</S/Transparency/CS/DeviceRGB>>", "0.2 0.4 0.6 rg 50 600 100 100 re f\n")], &[]));
    convert(&eng, doc, &spec(true, true, true), None).unwrap();
    let cs = eng
        .read_custom(doc, |pdf| Ok(String::from_utf8_lossy(&pdf.find_page(0)?.get_dict("Group")?.unwrap().get_dict("CS")?.unwrap().as_name()?).into_owned()))
        .unwrap();
    assert_eq!(cs, "DeviceCMYK");
}

// ---- boxes ---------------------------------------------------------------------------------------

fn box_spec(kind: BoxKind, from: BoxKind, margin: [f32; 4]) -> BoxSpec {
    BoxSpec { pages: None, kind, rect: None, from: Some(from), margin: Some(margin) }
}

#[test]
fn trim_and_bleed_boxes_are_set_and_checked() {
    let (eng, doc) = open("boxes", &build("", &[("", "0 0 0 1 k 40 40 100 100 re f")], &[]));
    let u = EditSource::User;
    // a 36-point margin inside the media box is the trim; 9 points outside that the bleed
    let s = set_boxes(&eng, doc, &box_spec(BoxKind::Trim, BoxKind::Media, [-36.0; 4]), u.clone()).unwrap();
    assert_eq!(s.undo.as_deref(), Some("Set trim box"));
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Trim), Some([36.0, 36.0, 576.0, 756.0]));
    set_boxes(&eng, doc, &box_spec(BoxKind::Bleed, BoxKind::Trim, [9.0; 4]), u.clone()).unwrap();
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Bleed), Some([27.0, 27.0, 585.0, 765.0]));
    // a bleed beyond the media box, a trim outside the bleed, an empty box and the media box are refused
    assert!(set_boxes(&eng, doc, &box_spec(BoxKind::Bleed, BoxKind::Trim, [40.0; 4]), u.clone()).is_err());
    assert!(set_boxes(&eng, doc, &box_spec(BoxKind::Trim, BoxKind::Media, [-10.0; 4]), u.clone()).is_err());
    assert!(set_boxes(&eng, doc, &BoxSpec { pages: None, kind: BoxKind::Art, rect: Some([10.0, 10.0, 10.0, 400.0]), from: None, margin: None }, u.clone()).is_err());
    assert!(set_boxes(&eng, doc, &BoxSpec { pages: None, kind: BoxKind::Media, rect: Some([0.0, 0.0, 100.0, 100.0]), from: None, margin: None }, u.clone()).is_err());
    assert!(set_boxes(&eng, doc, &BoxSpec { pages: Some(vec![4]), kind: BoxKind::Art, rect: Some([0.0, 0.0, 100.0, 100.0]), from: None, margin: None }, u.clone()).is_err());
    assert!(set_boxes(&eng, doc, &BoxSpec { pages: None, kind: BoxKind::Art, rect: None, from: None, margin: None }, u.clone()).is_err());
    // what was refused changed nothing, and an absolute rectangle works
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Trim), Some([36.0, 36.0, 576.0, 756.0]));
    set_boxes(&eng, doc, &BoxSpec { pages: None, kind: BoxKind::Art, rect: Some([100.0, 100.0, 500.0, 700.0]), from: None, margin: None }, u).unwrap();
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Art), Some([100.0, 100.0, 500.0, 700.0]));
    // the PDF/X checker sees the trim box
    let report = crate::preflight::check_doc(&eng, doc, crate::preflight::Profile::Pdfx4, &mut no_progress(), &AtomicBool::new(false)).unwrap();
    assert_eq!(report.status("page-boxes"), Some(crate::preflight::Status::Passed));
    // undo takes the art box away again
    eng.undo(doc).unwrap();
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Art), None);
}

// ---- printer's marks -----------------------------------------------------------------------------

/// Every vertex of the marks' path operators (`m`, `l`, `c`, `re`) in the page's last content stream.
fn mark_points(eng: &Engine, doc: DocId) -> Vec<(f32, f32)> {
    let all = contents(eng, doc, 0);
    let marks = all.rsplit("/IwReg CS").next().unwrap().to_string();
    let mut pts = Vec::new();
    let mut nums: Vec<f32> = Vec::new();
    for tok in marks.split_whitespace() {
        match tok {
            "m" | "l" => pts.push((nums[nums.len() - 2], nums[nums.len() - 1])),
            "c" => {
                for i in 0..3 {
                    pts.push((nums[nums.len() - 6 + 2 * i], nums[nums.len() - 5 + 2 * i]));
                }
            }
            "re" => {
                let n = nums.len();
                let (x, y, w, h) = (nums[n - 4], nums[n - 3], nums[n - 2], nums[n - 1]);
                pts.extend([(x, y), (x + w, y + h)]);
            }
            _ => {}
        }
        match tok.parse::<f32>() {
            Ok(v) => nums.push(v),
            Err(_) => nums.clear(),
        }
    }
    pts
}

#[test]
fn printer_marks_fall_outside_the_trim_and_the_bleed() {
    // 200% ink inside the trim; the marks in the slug are 400% (registration)
    let (eng, doc) = open("marks", &build("", &[("", "0.5 0.5 0.5 0.5 k 100 100 400 600 re f")], &[]));
    let u = EditSource::User;
    set_boxes(&eng, doc, &box_spec(BoxKind::Trim, BoxKind::Media, [-20.0; 4]), u.clone()).unwrap();
    set_boxes(&eng, doc, &box_spec(BoxKind::Bleed, BoxKind::Trim, [9.0; 4]), u.clone()).unwrap();
    let trim = boxes(&eng, doc, 0, BoxKind::Trim).unwrap();
    let bleed = boxes(&eng, doc, 0, BoxKind::Bleed).unwrap();
    let (state, notes) = add_printer_marks(&eng, doc, &MarksSpec { pages: None, marks: MarkSet::default(), weight: Some(1.0), offset: None, length: None }, u).unwrap();
    assert_eq!(state.undo.as_deref(), Some("Add printer's marks"));
    assert!(notes.is_empty(), "{notes:?}");
    // the media box grew around the trim, which and the bleed did not move
    let media = boxes(&eng, doc, 0, BoxKind::Media).unwrap();
    assert!(media[0] < 0.0 && media[1] < 0.0 && media[2] > 612.0 && media[3] > 792.0, "{media:?}");
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Trim), Some(trim));
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Bleed), Some(bleed));
    assert_eq!(state.sizes[0], [media[2] - media[0], media[3] - media[1]]);
    // every mark vertex is outside the bleed (and so outside the trim) and inside the media box
    let pts = mark_points(&eng, doc);
    assert!(pts.len() > 40, "{} vertices", pts.len());
    let inside = |b: [f32; 4], (x, y): (f32, f32)| x > b[0] + 0.01 && x < b[2] - 0.01 && y > b[1] + 0.01 && y < b[3] - 0.01;
    for p in &pts {
        assert!(!inside(trim, *p) && !inside(bleed, *p), "{p:?} is inside the trim or the bleed");
        assert!(p.0 >= media[0] && p.0 <= media[2] && p.1 >= media[1] && p.1 <= media[3], "{p:?} is off the media box");
    }
    // the page's own content is where it was: ink inside the trim still reads 200%, the marks 400%
    let inner = ink_report(&eng, doc, 0, 300.0, 1600, true).unwrap();
    assert!((inner.max_tac - 200.0).abs() < 1.0, "{}", inner.max_tac);
    assert!(!inner.over_limit);
    let whole = ink_report(&eng, doc, 0, 300.0, 1600, false).unwrap();
    assert!(whole.max_tac > 300.0 && whole.over_limit, "{}", whole.max_tac);
    // undo gives the old media box back
    eng.undo(doc).unwrap();
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Media), Some([0.0, 0.0, 612.0, 792.0]));
}

#[test]
fn marks_on_a_page_without_boxes_trim_at_its_edge_and_report_what_they_skip() {
    let (eng, doc) = open("marks-plain", &build("", &[("", "0 0 0 1 k 100 100 50 50 re f")], &[]));
    let (_, notes) = add_printer_marks(&eng, doc, &MarksSpec { pages: None, marks: MarkSet::default(), weight: None, offset: None, length: None }, EditSource::User).unwrap();
    assert!(notes.iter().any(|n| n.contains("no bleed")), "{notes:?}");
    // the page's old edge is the trim now
    assert_eq!(boxes(&eng, doc, 0, BoxKind::Trim), Some([0.0, 0.0, 612.0, 792.0]));
    let pts = mark_points(&eng, doc);
    assert!(pts.iter().all(|&(x, y)| !(x > 0.01 && x < 611.99 && y > 0.01 && y < 791.99)));
    // a page too small for colour bars says so
    let (eng, doc) = open("marks-small", &build("", &[("", "0 g 1 1 5 5 re f")], &[]));
    set_boxes(&eng, doc, &BoxSpec { pages: None, kind: BoxKind::Trim, rect: Some([300.0, 300.0, 340.0, 340.0]), from: None, margin: None }, EditSource::User).unwrap();
    let (_, notes) = add_printer_marks(&eng, doc, &MarksSpec { pages: None, marks: MarkSet { crop: false, bleed: false, registration: false, color_bars: true }, weight: None, offset: None, length: None }, EditSource::User).unwrap();
    assert!(notes.iter().any(|n| n.contains("too small")), "{notes:?}");
    // nothing picked, a bad weight and a bad page are refused
    let none = MarkSet { crop: false, bleed: false, registration: false, color_bars: false };
    assert!(add_printer_marks(&eng, doc, &MarksSpec { pages: None, marks: none, weight: None, offset: None, length: None }, EditSource::User).is_err());
    assert!(add_printer_marks(&eng, doc, &MarksSpec { pages: None, marks: MarkSet::default(), weight: Some(0.0), offset: None, length: None }, EditSource::User).is_err());
    assert!(add_printer_marks(&eng, doc, &MarksSpec { pages: Some(vec![3]), marks: MarkSet::default(), weight: None, offset: None, length: None }, EditSource::User).is_err());
}

// ---- tiles through the engine --------------------------------------------------------------------

fn wait(eng: &Arc<Engine>, pre: &Arc<Prepress>, key: TileKey, view: u32) -> TileOutcome {
    let (tx, rx) = std::sync::mpsc::channel();
    pre.request_tile(eng, key, view, 1, 0, Box::new(move |o| {
        let _ = tx.send(o);
    }));
    rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap()
}

#[test]
fn views_are_registered_once_and_tiles_follow_the_engine_generation() {
    let (eng, doc) = open("route", &build(SPOT, &[("", SPOT_PAGE)], &[]));
    let pre = Arc::new(Prepress::default());
    let all = pre.register(doc, View::default());
    let cyan_only = pre.register(doc, only([true, false, false, false], true));
    assert_eq!(pre.register(doc, View::default()), all);
    assert_ne!(all, cyan_only);
    let k = key(&eng, doc);
    let rgba = |o: TileOutcome| match o {
        TileOutcome::Ready(t) => t.rgba.as_ref().clone(),
        TileOutcome::Stale => panic!("stale"),
        TileOutcome::Failed(e) => panic!("{e}"),
    };
    let a = rgba(wait(&eng, &pre, k, all));
    let b = rgba(wait(&eng, &pre, k, cyan_only));
    assert_ne!(a, b, "the views differ");
    assert_eq!(a, rgba(wait(&eng, &pre, k, all)), "a second request is the cached tile");
    // an unknown view fails, and an edit makes the old generation stale
    assert!(matches!(wait(&eng, &pre, k, 999), TileOutcome::Failed(_)));
    eng.apply(doc, crate::engine::edit::EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    assert!(matches!(wait(&eng, &pre, k, all), TileOutcome::Stale));
    let fresh = TileKey { rev: eng.doc_entry(doc).unwrap().generation(), ..k };
    assert!(matches!(wait(&eng, &pre, fresh, all), TileOutcome::Ready(_)));
    // a closed document's views are gone
    pre.drop_doc(doc);
    assert!(matches!(wait(&eng, &pre, fresh, all), TileOutcome::Failed(_)));
}

#[test]
fn the_tile_route_parses() {
    use super::app::parse_tile_request;
    let uri: tauri::http::Uri = "inkwell://localhost/p/3/7/1/1500/2/4?v=9&g=5&e=11&r=2".parse().unwrap();
    let (key, view, epoch, rank) = parse_tile_request(&uri).unwrap();
    assert_eq!((key.doc, key.page, key.rot, key.scale, key.tx, key.ty, key.rev), (3, 7, 1, 1500, 2, 4, 5));
    assert_eq!((view, epoch, rank), (9, 11, 2));
    let plain: tauri::http::Uri = "inkwell://localhost/t/3/7/1/1500/2/4".parse().unwrap();
    assert!(parse_tile_request(&plain).is_none());
}
