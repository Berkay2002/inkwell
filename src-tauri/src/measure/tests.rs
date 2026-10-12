//! Measuring against hand-computed numbers: each kind, a rotated page, `/UserUnit`, an Acrobat-style `/Measure`
//! (read, rewritten, read again), viewports, snapping, and the undo/save/reopen path.

use super::calc::{PageGeom, polygon_centroid, trim_num};
use super::dict::MeasureDict;
use super::numfmt::{NumberFormat, Style, chain_text};
use super::*;
use crate::engine::edit::EditOp;
use std::path::PathBuf;
use std::sync::Arc;

// ---- fixtures ------------------------------------------------------------------------------------------------

fn dir(test: &str) -> PathBuf {
    let d = std::env::temp_dir().join("inkwell-tests").join(format!("measure-{test}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A one-page PDF. Objects 1 catalog, 2 pages, 3 page, 4 content; `extra` are further `(number, body)` objects (5..).
struct Spec {
    rotate: i32,
    user_unit: Option<f32>,
    media: [f32; 4],
    content: &'static str,
    /// Extra entries of the page dictionary.
    page: String,
    extra: Vec<(usize, String)>,
    /// Entries of a second, empty page (object 9), when the test needs one.
    second: Option<String>,
    /// Entries of the page dictionary beside /MediaBox: a /CropBox for instance.
    boxes: String,
    /// Extra entries of the catalog: /Perms, /AcroForm.
    cat: String,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            rotate: 0,
            user_unit: None,
            media: [0.0, 0.0, 612.0, 792.0],
            content: "",
            page: String::new(),
            extra: Vec::new(),
            second: None,
            boxes: String::new(),
            cat: String::new(),
        }
    }
}

fn build(s: &Spec) -> Vec<u8> {
    let uu = s.user_unit.map_or(String::new(), |u| format!("/UserUnit {u} "));
    let mut objs: Vec<(usize, Vec<u8>)> = vec![
        (1, format!("<< /Type /Catalog /Pages 2 0 R {} >>", s.cat).into_bytes()),
        (
            2,
            if s.second.is_some() {
                b"<< /Type /Pages /Kids [3 0 R 9 0 R] /Count 2 >>".to_vec()
            } else {
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()
            },
        ),
        (
            3,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [{} {} {} {}] /Rotate {} {uu}/Contents 4 0 R /Resources << >> {} {} >>",
                s.media[0], s.media[1], s.media[2], s.media[3], s.rotate, s.boxes, s.page
            )
            .into_bytes(),
        ),
        (4, format!("<< /Length {} >>\nstream\n{}\nendstream", s.content.len(), s.content).into_bytes()),
    ];
    if let Some(extras) = &s.second {
        objs.push((9, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {extras} >>").into_bytes()));
    }
    objs.extend(s.extra.iter().map(|(n, b)| (*n, b.clone().into_bytes())));
    objs.sort_by_key(|o| o.0);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0usize; objs.last().unwrap().0 + 1];
    for (n, body) in &objs {
        offsets[*n] = out.len();
        out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for o in &offsets[1..] {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len()).as_bytes());
    out
}

fn open(test: &str, spec: &Spec) -> (Arc<Engine>, DocId, String) {
    let p = dir(test).join("a.pdf");
    std::fs::write(&p, build(spec)).unwrap();
    let path = p.to_string_lossy().into_owned();
    let eng = Engine::new(2, 64 << 20);
    let id = eng.open(&path).unwrap().doc;
    (eng, id, path)
}

const FT10: Scale = Scale { paper: 1.0, paper_unit: Unit::In, real: 10.0, real_unit: Unit::Ft };

fn add(eng: &Engine, id: DocId, kind: Kind, pts: &[[f32; 2]], scale: Option<&Scale>) -> (i32, Quantity) {
    let (_, xref, q) = add_one(eng, id, 0, kind, pts, scale, "Alex Johnson", EditSource::User).unwrap();
    (xref, q)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-4 * b.abs().max(1.0)
}

/// Reads an entry of the annotation `xref` on page 1 from the authoritative document.
fn with_annot<R>(eng: &Engine, id: DocId, xref: i32, f: impl FnOnce(&mupdf::pdf::PdfObject) -> R) -> R {
    eng.read_custom(id, |d| {
        let p = d.load_pdf_page(0)?;
        let a = p.annotations().find(|a| a.xref().is_ok_and(|x| x == xref)).expect("annotation");
        Ok(f(&a.object()))
    })
    .unwrap()
}

fn nums(o: &mupdf::pdf::PdfObject, key: &str) -> Vec<f64> {
    let a = o.get_dict(key).unwrap().expect(key);
    (0..a.len().unwrap()).map(|i| f64::from(a.get_array(i as i32).unwrap().unwrap().as_float().unwrap())).collect()
}

// ---- pure pieces ---------------------------------------------------------------------------------------------

#[test]
fn units_scales_and_formats() {
    // 72 pt of paper in every unit.
    let want = [72.0, 1.0, 25.4, 2.54, 0.0254, 1.0 / 12.0];
    for (u, w) in Unit::ALL.iter().zip(want) {
        assert!(close(72.0 / u.pt_per_unit(), w), "{u:?}");
        assert_eq!(Unit::parse(u.label()), Some(*u));
    }
    assert_eq!(Unit::parse(" Feet "), Some(Unit::Ft));
    assert_eq!(Unit::parse("km"), None);

    assert_eq!(FT10.label(), "1 in = 10 ft");
    assert!(close(FT10.real_per_pt(), 10.0 / 72.0));
    assert_eq!(Scale::parse("1 in = 10 ft"), Some(FT10));
    assert_eq!(Scale::parse("0.25in=1ft"), Some(Scale { paper: 0.25, paper_unit: Unit::In, real: 1.0, real_unit: Unit::Ft }));
    assert_eq!(Scale::parse("1:100"), None);
    assert_eq!(Scale::parse("0 in = 1 ft"), None);
    assert!(Scale { real: -1.0, ..FT10 }.validate().is_err());
    assert_eq!(trim_num(2.5), "2.5");
    assert_eq!(trim_num(10.0), "10");

    let f = NumberFormat::new("ft", 1.0);
    assert_eq!(f.text(1234.5), "1,234.50 ft");
    assert_eq!(NumberFormat { fixed: false, ..f.clone() }.text(1234.5), "1,234.5 ft");
    assert_eq!(NumberFormat { d: 10.0, ..f.clone() }.number(2.26), "2.3");
    assert_eq!(NumberFormat { style: Style::Round, ..f.clone() }.number(2.5), "3");
    assert_eq!(NumberFormat { style: Style::Truncate, ..f.clone() }.number(2.9), "2");
    let frac = NumberFormat { style: Style::Fraction, d: 8.0, fixed: false, ..f.clone() };
    assert_eq!(frac.number(3.5), "3 1/2");
    assert_eq!(frac.number(0.26), "1/4");
    assert_eq!(frac.number(2.97), "3");
    assert_eq!(NumberFormat { thousands: ".".into(), decimal: ",".into(), ..f.clone() }.number(1234.5), "1.234,50");
    // The label is PS + unit + SS; /O says whether it comes before the number or after it.
    assert_eq!(NumberFormat { prefix: true, ..f.clone() }.text(2.0), " ft2.00");
    let fenced = NumberFormat { ps: "[".into(), ss: "]".into(), ..f.clone() };
    assert_eq!(fenced.text(2.0), "2.00[ft]");
    assert_eq!(NumberFormat { prefix: true, ..fenced }.text(2.0), "[ft]2.00");
    let inches = NumberFormat { c: 12.0, ..NumberFormat::new("in", 1.0) };
    assert_eq!(chain_text(3.375, &[f.clone(), inches]), "3 ft 4.50 in");
}

#[test]
fn geometry_helpers() {
    assert!(close(calc::polygon_area(&[[0.0, 0.0], [144.0, 0.0], [0.0, 72.0]]), 5184.0));
    assert!(close(calc::path_length(&[[0.0, 0.0], [72.0, 0.0], [72.0, 72.0]]), 144.0));
    let c = polygon_centroid(&[[0.0, 0.0], [6.0, 0.0], [6.0, 3.0], [0.0, 3.0]]);
    assert!(close(c[0], 3.0) && close(c[1], 1.5));
    // Rotate 90 clockwise on a 612 x 792 page, as MuPDF reports it: display (X, Y) = (y, x).
    let g = PageGeom::new([0.0, 1.0, 1.0, 0.0, 0.0, 0.0]).unwrap();
    assert_eq!(g.user_unit, 1.0);
    assert!(close(PageGeom::new([0.0, -2.0, 2.0, 0.0, 5.0, 5.0]).unwrap().user_unit, 2.0));
    assert_eq!(g.to_page([50.0, 100.0]), [100.0, 50.0]);
    let back = g.to_user(g.to_page([12.5, 99.0]));
    assert!(close(back[0], 12.5) && close(back[1], 99.0));
    assert!(PageGeom::new([0.0; 6]).is_none());
    assert!(PageGeom::new([1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0]).is_none());
}

// ---- each kind, against hand-computed numbers --------------------------------------------------------------

#[test]
fn each_kind_matches_the_numbers_worked_out_by_hand() {
    let (eng, id, _) = open("kinds", &Spec::default());
    // 3-4-5 of 36 pt: 180 pt = 2.5 in, 1 in = 10 ft.
    let (_, q) = add(&eng, id, Kind::Distance, &[[100.0, 100.0], [208.0, 244.0]], Some(&FT10));
    assert!(close(q.value, 25.0), "{q:?}");
    assert_eq!((q.unit.as_str(), q.text.as_str()), ("ft", "25.00 ft"));
    // 72 + 72 + 72 pt = 3 in = 30 ft (a polyline: the path, not the chord).
    let (_, q) = add(&eng, id, Kind::Perimeter, &[[0.0, 0.0], [72.0, 0.0], [72.0, 72.0], [144.0, 72.0]], Some(&FT10));
    assert_eq!(q.text, "30.00 ft");
    // 144 x 72 pt = 2 in x 1 in = 2 sq in = 200 sq ft.
    let (_, q) = add(&eng, id, Kind::Area, &[[0.0, 0.0], [144.0, 0.0], [144.0, 72.0], [0.0, 72.0]], Some(&FT10));
    assert!(close(q.value, 200.0), "{q:?}");
    assert_eq!((q.unit.as_str(), q.text.as_str()), ("sq ft", "200.00 sq ft"));
    // The right triangle: 144 x 72 / 2 = 5184 pt^2 = 1 sq in = 100 sq ft, whatever the vertex order.
    let tri = [[0.0, 0.0], [144.0, 0.0], [0.0, 72.0]];
    assert_eq!(add(&eng, id, Kind::Area, &tri, Some(&FT10)).1.text, "100.00 sq ft");
    assert_eq!(value_of(&eng, id, 0, Kind::Area, &[tri[2], tri[0], tri[1]], Some(&FT10)).unwrap().text, "100.00 sq ft");
    // Other units: 72 pt = 1 in = 2.54 cm; 1 cm = 1 m, so 2.54 m.
    let cm_m = Scale { paper: 1.0, paper_unit: Unit::Cm, real: 1.0, real_unit: Unit::M };
    let q = value_of(&eng, id, 0, Kind::Distance, &[[0.0, 0.0], [72.0, 0.0]], Some(&cm_m)).unwrap();
    assert!(close(q.value, 2.54) && q.unit == "m", "{q:?}");
    // No scale anywhere: 1 in = 1 in.
    assert_eq!(value_of(&eng, id, 0, Kind::Distance, &[[0.0, 0.0], [144.0, 0.0]], None).unwrap().text, "2.00 in");
    // The panel list shows them with the quantity read back from each /Measure.
    let all = list_all(&eng, id).unwrap();
    let kinds: Vec<Kind> = all.iter().map(|m| m.kind).collect();
    assert_eq!(kinds, [Kind::Distance, Kind::Perimeter, Kind::Area, Kind::Area]);
    assert_eq!(all[0].quantity.text, "25.00 ft");
    assert_eq!(all[0].contents, "25.00 ft");
    assert_eq!(all[0].scale, Some(FT10));
    assert_eq!(all[0].author, "Alex Johnson");
    assert_eq!(all[2].points.len(), 4);
    assert_eq!(all[2].quantity.text, "200.00 sq ft");
}

#[test]
fn bad_input_is_refused_and_leaves_no_step() {
    let (eng, id, _) = open("bad", &Spec::default());
    let steps = eng.history(id).unwrap().len();
    let a = [10.0, 10.0];
    let b = [50.0, 10.0];
    let c = [50.0, 50.0];
    let bads: Vec<(Kind, Vec<[f32; 2]>, Option<Scale>)> = vec![
        (Kind::Distance, vec![a], None),
        (Kind::Distance, vec![a, b, c], None),
        (Kind::Perimeter, vec![a], None),
        (Kind::Area, vec![a, b], None),
        (Kind::Distance, vec![a, a], None),
        (Kind::Distance, vec![a, [f32::NAN, 1.0]], None),
        (Kind::Distance, vec![a, b], Some(Scale { real: 0.0, ..FT10 })),
        (Kind::Distance, vec![a, b], Some(Scale { paper: f64::NAN, ..FT10 })),
    ];
    for (kind, pts, scale) in bads {
        assert!(add_one(&eng, id, 0, kind, &pts, scale.as_ref(), "", EditSource::User).is_err(), "{kind:?} {pts:?}");
    }
    assert!(add_one(&eng, id, 5, Kind::Distance, &[a, b], None, "", EditSource::User).is_err());
    assert!(scale_set(&eng, id, 0, &Scale { paper: -1.0, ..FT10 }, EditSource::User).is_err());
    assert_eq!(eng.history(id).unwrap().len(), steps);
    assert!(list_all(&eng, id).unwrap().is_empty());
}

// ---- rotation and UserUnit -------------------------------------------------------------------------------

#[test]
fn rotated_page_measures_the_same_and_stores_user_space() {
    // Same 3-4-5 segment, in display space of a page turned 90 degrees clockwise.
    let (eng, id, _) = open("rotated", &Spec { rotate: 90, ..Default::default() });
    let (xref, q) = add(&eng, id, Kind::Distance, &[[100.0, 50.0], [208.0, 194.0]], Some(&FT10));
    assert_eq!(q.text, "25.00 ft");
    // Display (X, Y) = user (y, x): the file holds (50, 100) and (194, 208).
    let l = with_annot(&eng, id, xref, |o| nums(o, "L"));
    for (got, want) in l.iter().zip([50.0, 100.0, 194.0, 208.0]) {
        assert!(close(*got, want), "{l:?}");
    }
    // Read back, the points are display points again, and so is the value.
    let m = &list_all(&eng, id).unwrap()[0];
    for (got, want) in m.points.iter().flatten().zip([100.0, 50.0, 208.0, 194.0]) {
        assert!(close(f64::from(*got), want), "{:?}", m.points);
    }
    assert_eq!(m.quantity.text, "25.00 ft");
    // An area on the rotated page: 2 sq in again.
    let (_, q) = add(&eng, id, Kind::Area, &[[0.0, 0.0], [72.0, 0.0], [72.0, 144.0], [0.0, 144.0]], Some(&FT10));
    assert_eq!(q.text, "200.00 sq ft");
}

#[test]
fn user_unit_page_measures_in_real_points() {
    // 306 x 396 user units of 2 pt each: the page is 612 x 792 pt, the viewer's space as on any letter page.
    let spec = Spec { user_unit: Some(2.0), media: [0.0, 0.0, 306.0, 396.0], ..Default::default() };
    let (eng, id, _) = open("userunit", &spec);
    let (xref, q) = add(&eng, id, Kind::Distance, &[[0.0, 0.0], [108.0, 144.0]], Some(&FT10));
    assert_eq!(q.text, "25.00 ft");
    // In the file: user space is half the display size, and /C is per user unit (2 pt): 10 ft / 72 x 2.
    let l = with_annot(&eng, id, xref, |o| nums(o, "L"));
    assert!(close((l[2] - l[0]).abs(), 54.0) && close((l[3] - l[1]).abs(), 72.0), "{l:?}");
    let c = with_annot(&eng, id, xref, |o| {
        let x = o.get_dict("Measure").unwrap().unwrap().get_dict("X").unwrap().unwrap();
        f64::from(x.get_array(0).unwrap().unwrap().get_dict("C").unwrap().unwrap().as_float().unwrap())
    });
    assert!(close(c, 10.0 / 72.0 * 2.0), "C = {c}");
    // The scale reads back as the one that was set, and an area is right too: 2 sq in.
    assert_eq!(list_all(&eng, id).unwrap()[0].scale, Some(FT10));
    assert_eq!(add(&eng, id, Kind::Area, &[[0.0, 0.0], [144.0, 0.0], [144.0, 72.0], [0.0, 72.0]], Some(&FT10)).1.text, "200.00 sq ft");
    // A page scale set on this page applies to a measurement with no scale of its own.
    scale_set(&eng, id, 0, &FT10, EditSource::User).unwrap();
    assert_eq!(add(&eng, id, Kind::Distance, &[[0.0, 0.0], [108.0, 144.0]], None).1.text, "25.00 ft");
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, FT10);
}

// ---- Acrobat-style /Measure ---------------------------------------------------------------------------------

/// A page with a whole-page viewport (1 in = 10 ft), a detail viewport (1 in = 1 ft) over the lower left, and a
/// line annotation with its own /Measure, written the way Acrobat does (X carries the scale, D and A have C 1).
fn acrobat() -> Spec {
    let fmt = |u: &str, c: &str| format!("<< /Type /NumberFormat /U ({u}) /C {c} /D 100 /F /D /FD true /RT (,) /RD (.) /PS ( ) /SS () >>");
    let measure = |r: &str, c: &str| {
        format!(
            "<< /Type /Measure /Subtype /RL /R ({r}) /X [{}] /D [{}] /A [{}] /O [0 0] /CYX 1 >>",
            fmt("ft", c),
            fmt("ft", "1"),
            fmt("sq ft", "1")
        )
    };
    Spec {
        page: "/VP [5 0 R 6 0 R] /Annots [7 0 R 8 0 R]".into(),
        extra: vec![
            (5, format!("<< /Type /Viewport /BBox [0 0 612 792] /Name (Whole) /Measure {} >>", measure("1 in = 10 ft", "0.1388889"))),
            (6, format!("<< /Type /Viewport /BBox [0 0 300 300] /Name (Detail) /Measure {} >>", measure("1 in = 1 ft", "0.01388889"))),
            // User space (0,0)-(108,144): 180 pt.
            (
                7,
                format!(
                    "<< /Type /Annot /Subtype /Line /Rect [0 0 108 144] /L [0 0 108 144] /IT /LineDimension /C [1 0 0] \
                     /Contents (25.00 ft) /F 4 /Measure {} >>",
                    measure("1 in = 10 ft", "0.1388889")
                ),
            ),
            // 144 x 72 pt triangle-ish polygon: (400,400) (544,400) (400,472), area 5184 pt^2 = 1 sq in at 1 in = 10 ft.
            (
                8,
                format!(
                    "<< /Type /Annot /Subtype /Polygon /Rect [400 400 544 472] /Vertices [400 400 544 400 400 472] \
                     /IT /PolygonDimension /C [1 0 0] /F 4 /Measure {} >>",
                    measure("1 in = 10 ft", "0.1388889")
                ),
            ),
        ],
        ..Default::default()
    }
}

#[test]
fn an_acrobat_style_measure_is_read_and_survives_a_rewrite() {
    let (eng, id, _) = open("acrobat", &acrobat());
    let all = list_all(&eng, id).unwrap();
    assert_eq!(all.len(), 2);
    // Page space is y down: user (0,0) is display (0, 792).
    assert_eq!(all[0].kind, Kind::Distance);
    assert_eq!(all[0].points, vec![[0.0, 792.0], [108.0, 648.0]]);
    assert_eq!(all[0].quantity.text, "25.00 ft");
    assert_eq!(all[0].scale, Some(FT10));
    assert_eq!((all[1].kind, all[1].quantity.text.as_str()), (Kind::Area, "100.00 sq ft"));

    // Read the annotation's measure, write it as a new object, read that: the same dictionary.
    let mut seen = None;
    eng.apply_custom(id, "Rewrite measure".into(), EditSource::User, false, |d| {
        let p = d.load_pdf_page(0)?;
        let a = p.annotations().next().unwrap();
        let m = MeasureDict::read(&a.object().get_dict("Measure")?.unwrap())?.unwrap();
        let again = MeasureDict::read(&m.write(d)?)?.unwrap();
        seen = Some((m, again));
        Ok(vec![0])
    })
    .unwrap();
    let (m, again) = seen.unwrap();
    assert_eq!(m, again);
    assert_eq!(m.r, "1 in = 10 ft");
    assert!(close(m.x[0].c, 0.1388889) && m.x[0].fixed && m.x[0].unit == "ft");
    assert_eq!(m.cyx, Some(1.0));
    assert!(m.quantity(Kind::Distance, &[[0.0, 0.0], [108.0, 144.0]]).text == "25.00 ft");

    // The page's scale is its widest viewport; a point inside the detail viewport gets the detail's scale.
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, FT10);
    // Display (10, 782) is user (10, 10), inside the detail box.
    let detail = value_of(&eng, id, 0, Kind::Distance, &[[10.0, 782.0], [118.0, 638.0]], None).unwrap();
    assert_eq!(detail.text, "2.50 ft");
    let outside = value_of(&eng, id, 0, Kind::Distance, &[[400.0, 100.0], [508.0, 244.0]], None).unwrap();
    assert_eq!(outside.text, "25.00 ft");

    // Setting the page scale replaces the whole-page viewport's measure and keeps the detail viewport (2 entries).
    let one_to_five = Scale { real: 5.0, ..FT10 };
    scale_set(&eng, id, 0, &one_to_five, EditSource::User).unwrap();
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, one_to_five);
    let n = eng.read_custom(id, |d| Ok(d.load_pdf_page(0)?.object().get_dict("VP")?.unwrap().len()?)).unwrap();
    assert_eq!(n, 2);
    assert_eq!(value_of(&eng, id, 0, Kind::Distance, &[[400.0, 100.0], [508.0, 244.0]], None).unwrap().text, "12.50 ft");
}

#[test]
fn a_measure_with_a_unit_we_have_no_name_for_still_reads() {
    let fmt = |u: &str, c: &str| format!("<< /Type /NumberFormat /U ({u}) /C {c} /D 10 /F /D >>");
    let spec = Spec {
        page: "/Annots [5 0 R]".into(),
        extra: vec![(
            5,
            format!(
                "<< /Type /Annot /Subtype /Line /Rect [0 0 100 100] /L [0 0 72 0] /IT /LineDimension /F 4 /Measure \
                 << /Type /Measure /Subtype /RL /R (1:100) /X [{}] >> >>",
                fmt("km", "0.002")
            ),
        )],
        ..Default::default()
    };
    let (eng, id, _) = open("km", &spec);
    let m = &list_all(&eng, id).unwrap()[0];
    // 72 user units x 0.002, one decimal ("D 10"), no D array: the X format is used, trailing zeros trimmed.
    assert_eq!(m.quantity.text, "0.1 km");
    assert_eq!(m.scale, None);
}

// ---- snapping ----------------------------------------------------------------------------------------------

const PATHS: &str = "0 0 0 RG 100 100 m 300 100 l 300 200 l S 50 50 80 40 re f";

#[test]
fn snapping_to_a_known_path() {
    let (eng, id, _) = open("snap", &Spec { content: PATHS, ..Default::default() });
    // Page space is y down: user y 100 is display 692.
    let snap = |x: f32, y: f32, r: f32| snap_at(&eng, &SnapCache::default(), id, 0, x, y, r).unwrap();

    let h = snap(303.0, 695.0, 10.0).expect("end point");
    assert_eq!((h.kind, h.x, h.y), (SnapKind::Endpoint, 300.0, 692.0));
    assert!((h.distance - 18f32.sqrt()).abs() < 1e-3);

    let h = snap(202.0, 690.0, 6.0).expect("midpoint of the first segment");
    assert_eq!((h.kind, h.x, h.y), (SnapKind::Midpoint, 200.0, 692.0));

    let h = snap(150.0, 688.0, 5.0).expect("on the line");
    assert_eq!((h.kind, h.x, h.y), (SnapKind::Line, 150.0, 692.0));
    assert!((h.distance - 4.0).abs() < 1e-3);

    // Midpoint of the vertical segment (300,100)-(300,200): display (300, 642).
    let h = snap(298.0, 640.0, 6.0).unwrap();
    assert_eq!((h.kind, h.x, h.y), (SnapKind::Midpoint, 300.0, 642.0));

    // A corner of the rectangle (50,50)-(130,90): user (130,90) is display (130, 702).
    let h = snap(131.0, 703.0, 4.0).unwrap();
    assert_eq!((h.kind, h.x, h.y), (SnapKind::Endpoint, 130.0, 702.0));

    // Nothing within the radius, or a radius too small.
    assert_eq!(snap(150.0, 600.0, 10.0), None);
    assert_eq!(snap(150.0, 688.0, 3.0), None);

    // An end point beats a nearer line point: 300,692 is 3 away, the line y=692 passes 1 away from (298, 693).
    let h = snap(298.0, 693.0, 5.0).unwrap();
    assert_eq!(h.kind, SnapKind::Endpoint);
}

#[test]
fn snapping_follows_a_rotated_page_and_ignores_annotations() {
    let (eng, id, _) = open("snap-rot", &Spec { content: PATHS, rotate: 90, ..Default::default() });
    // User (300, 100) is display (100, 300) on a page turned 90 degrees clockwise.
    let h = snap_at(&eng, &SnapCache::default(), id, 0, 102.0, 302.0, 5.0).unwrap().unwrap();
    assert_eq!((h.kind, h.x, h.y), (SnapKind::Endpoint, 100.0, 300.0));
    // A measurement elsewhere is not a path of the page: no snapping to it.
    add(&eng, id, Kind::Distance, &[[500.0, 500.0], [560.0, 500.0]], Some(&FT10));
    assert_eq!(snap_at(&eng, &SnapCache::default(), id, 0, 502.0, 502.0, 8.0).unwrap(), None);
}

#[test]
fn curves_snap_by_end_points_midpoint_and_line() {
    // A half circle-ish curve from (100,100) to (200,100) bulging up by 3/4 of 100 (control points at y 200).
    let content: &'static str = "0 0 0 RG 100 100 m 100 200 200 200 200 100 c S";
    let (eng, id, _) = open("snap-curve", &Spec { content, ..Default::default() });
    let h = snap_at(&eng, &SnapCache::default(), id, 0, 101.0, 693.0, 4.0).unwrap().unwrap();
    assert_eq!((h.kind, h.x, h.y), (SnapKind::Endpoint, 100.0, 692.0));
    // t = 0.5: x 150, y 100 + 0.75 * 100 = 175 -> display 617.
    let h = snap_at(&eng, &SnapCache::default(), id, 0, 150.0, 619.0, 4.0).unwrap().unwrap();
    assert_eq!(h.kind, SnapKind::Midpoint);
    assert!((h.x - 150.0).abs() < 1e-3 && (h.y - 617.0).abs() < 1e-3);
    // Off the middle, on the curve: a line hit within a point of the true curve.
    let h = snap_at(&eng, &SnapCache::default(), id, 0, 118.0, 650.0, 12.0).unwrap().unwrap();
    assert_eq!(h.kind, SnapKind::Line);
}

#[test]
fn a_crossing_snaps_and_a_kind_that_is_off_falls_back() {
    // Two strokes crossing at user (200, 200) = display (200, 592), away from their middles and ends.
    let content: &'static str = "0 0 0 RG 100 100 m 400 400 l S 100 300 m 250 150 l S";
    let (eng, id, _) = open("snap-cross", &Spec { content, ..Default::default() });
    let ask = |x: f32, y: f32, r: f32, on: SnapKinds| snap_at_kinds(&eng, &SnapCache::default(), id, 0, x, y, r, &on).unwrap();
    let all = SnapKinds::default();

    let h = ask(203.0, 590.0, 6.0, all).expect("the crossing");
    assert_eq!(h.kind, SnapKind::Intersection);
    assert!((h.x - 200.0).abs() < 1e-3 && (h.y - 592.0).abs() < 1e-3);
    // The same pointer with crossings off is a point on a line; with every kind off, nothing.
    let h = ask(203.0, 590.0, 6.0, SnapKinds { intersection: false, ..all }).expect("line");
    assert_eq!(h.kind, SnapKind::Line);
    assert_eq!(ask(203.0, 590.0, 6.0, SnapKinds { endpoint: false, midpoint: false, intersection: false, line: false }), None);
    // Two lines that do not meet inside their lengths cross nowhere.
    let (eng2, id2, _) = open("snap-nocross", &Spec { content: "0 0 0 RG 100 100 m 150 150 l S 100 300 m 250 150 l S", ..Default::default() });
    let h = snap_at_kinds(&eng2, &SnapCache::default(), id2, 0, 200.0, 592.0, 6.0, &all).unwrap().expect("line");
    assert_eq!(h.kind, SnapKind::Line);

    // An end point and a midpoint that are off give way to the line under the pointer, not to nothing.
    let (eng, id, _) = open("snap-off", &Spec { content: PATHS, ..Default::default() });
    let ask = |x: f32, y: f32, on: SnapKinds| snap_at_kinds(&eng, &SnapCache::default(), id, 0, x, y, 6.0, &on).unwrap();
    let h = ask(303.0, 692.0, SnapKinds { endpoint: false, ..all }).expect("line near the corner");
    assert_eq!(h.kind, SnapKind::Line);
    let h = ask(201.0, 692.0, SnapKinds { midpoint: false, ..all }).expect("line near the midpoint");
    assert_eq!((h.kind, h.x, h.y), (SnapKind::Line, 201.0, 692.0));
    // Ends off at a corner that is also near a midpoint gives the midpoint.
    let h = ask(299.0, 645.0, SnapKinds { endpoint: false, ..all }).unwrap();
    assert_eq!(h.kind, SnapKind::Midpoint);
}

// ---- the edit path -------------------------------------------------------------------------------------------

#[test]
fn a_measurement_is_one_undo_step_kept_by_save_and_drawn() {
    let spec = Spec { content: "", ..Default::default() };
    let (eng, id, path) = open("steps", &spec);
    let base = eng.history(id).unwrap().len();
    let (xref, q) = add(&eng, id, Kind::Distance, &[[100.0, 100.0], [300.0, 100.0]], Some(&FT10));
    assert_eq!(q.text, "27.78 ft");
    let h = eng.history(id).unwrap();
    assert_eq!(h.len(), base + 1);
    assert_eq!(h.last().unwrap().label, "Add distance measurement on p. 1");
    assert_eq!(h.last().unwrap().pages, vec![0]);

    // The annotation: /IT, /Subj, the label, a print flag and an appearance with the label in it.
    with_annot(&eng, id, xref, |o| {
        let name = |k: &str| String::from_utf8(o.get_dict(k).unwrap().unwrap().as_name().unwrap()).unwrap();
        assert_eq!(name("IT"), "LineDimension");
        assert_eq!(name("Subtype"), "Line");
        assert_eq!(o.get_dict("Subj").unwrap().unwrap().as_string().unwrap(), "Distance Measurement");
        assert_eq!(o.get_dict("Contents").unwrap().unwrap().as_string().unwrap(), "27.78 ft");
        let n = o.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
        let s = String::from_utf8(n.read_stream().unwrap()).unwrap();
        assert!(s.contains("(27.78 ft) Tj"), "{s}");
        let bbox = nums(&n, "BBox");
        let rect = nums(o, "Rect");
        assert_eq!(bbox.len(), 4);
        for (a, b) in bbox.iter().zip(&rect) {
            assert!(close(*a, *b));
        }
        // The label and ticks make the box bigger than the line's own box.
        assert!(bbox[0] < 100.0 && bbox[2] > 300.0 && bbox[3] - bbox[1] > 10.0);
    });

    // It is drawn: red pixels on the line and above it (the label), none below.
    let (red_line, red_label, red_below) = eng
        .read_custom(id, |d| {
            let p = d.load_pdf_page(0)?;
            let pix = p.to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_rgb(), false, true)?;
            let (w, n) = (pix.width() as usize, pix.n() as usize);
            let s = pix.samples();
            let red = |x: usize, y: usize| {
                let i = (y * w + x) * n;
                let (r, g, b) = (i32::from(s[i]), i32::from(s[i + 1]), i32::from(s[i + 2]));
                r > 150 && r > g + 40 && r > b + 40
            };
            let count = |ys: std::ops::Range<usize>| ys.flat_map(|y| (100..300).map(move |x| (x, y))).filter(|&(x, y)| red(x, y)).count();
            // The points are in page space: the line runs along display y 100, the label sits above it.
            Ok((count(99..102), count(84..98), count(106..125)))
        })
        .unwrap();
    assert!(red_line > 150, "line pixels {red_line}");
    assert!(red_label > 10, "label pixels {red_label}");
    assert_eq!(red_below, 0);

    // Undo and redo are the one step.
    eng.undo(id).unwrap();
    assert!(list_all(&eng, id).unwrap().is_empty());
    eng.redo(id).unwrap();
    assert_eq!(list_all(&eng, id).unwrap().len(), 1);

    // Saved and reopened with a fresh MuPDF: still there, with its value and scale.
    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let m = list_all(&eng2, id2).unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].quantity.text, "27.78 ft");
    assert_eq!(m[0].scale, Some(FT10));
    assert_eq!(m[0].contents, "27.78 ft");
}

#[test]
fn page_scale_is_an_undo_step_kept_by_save_and_used_by_new_measurements() {
    let (eng, id, path) = open("scale", &Spec::default());
    let s = scale_get(&eng, id, 0).unwrap();
    assert!(!s.defined);
    assert_eq!(s.label, "1 in = 1 in");
    let st = scale_set(&eng, id, 0, &FT10, EditSource::User).unwrap();
    assert!(st.dirty && st.undo.is_some());
    let s = scale_get(&eng, id, 0).unwrap();
    assert!(s.defined);
    assert_eq!((s.scale, s.label.as_str()), (FT10, "1 in = 10 ft"));
    assert_eq!(eng.history(id).unwrap().last().unwrap().label, "Set measurement scale on p. 1 to 1 in = 10 ft");

    // Setting it again replaces the viewport's measure rather than adding one.
    let ft5 = Scale { paper: 0.5, ..FT10 };
    scale_set(&eng, id, 0, &ft5, EditSource::User).unwrap();
    let n = eng.read_custom(id, |d| Ok(d.load_pdf_page(0)?.object().get_dict("VP")?.unwrap().len()?)).unwrap();
    assert_eq!(n, 1);
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, ft5);

    // 0.5 in = 10 ft: 180 pt = 2.5 in = 50 ft.
    assert_eq!(add(&eng, id, Kind::Distance, &[[0.0, 0.0], [108.0, 144.0]], None).1.text, "50.00 ft");
    eng.undo(id).unwrap();
    eng.undo(id).unwrap();
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, FT10);
    eng.undo(id).unwrap();
    assert!(!scale_get(&eng, id, 0).unwrap().defined);
    eng.redo(id).unwrap();
    eng.save(id, None).unwrap();

    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let s = scale_get(&eng2, id2, 0).unwrap();
    assert_eq!((s.defined, s.scale), (true, FT10));
    assert_eq!(value_of(&eng2, id2, 0, Kind::Distance, &[[0.0, 0.0], [108.0, 144.0]], None).unwrap().text, "25.00 ft");
}

#[test]
fn measuring_does_not_disturb_other_edits() {
    let (eng, id, _) = open("mixed", &Spec::default());
    add(&eng, id, Kind::Distance, &[[10.0, 10.0], [50.0, 10.0]], Some(&FT10));
    eng.apply(id, EditOp::InsertBlank { at: 1, like: 0 }).unwrap();
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    // The page was turned after the measurement: its points read back in the new display space, value unchanged.
    let m = &list_all(&eng, id).unwrap()[0];
    assert_eq!(m.quantity.text, "5.56 ft");
    assert_eq!(m.page, 0);
}

#[test]
fn polygon_and_polyline_appearances_draw() {
    let (eng, id, _) = open("draw", &Spec::default());
    add(&eng, id, Kind::Area, &[[100.0, 100.0], [300.0, 100.0], [300.0, 200.0], [100.0, 200.0]], Some(&FT10));
    add(&eng, id, Kind::Perimeter, &[[100.0, 300.0], [200.0, 300.0], [200.0, 380.0]], Some(&FT10));
    let px = eng
        .read_custom(id, |d| {
            let p = d.load_pdf_page(0)?;
            let pix = p.to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_rgb(), false, true)?;
            let (w, n) = (pix.width() as usize, pix.n() as usize);
            let at = |x: usize, y: usize| {
                let i = (y * w + x) * n;
                [i32::from(pix.samples()[i]), i32::from(pix.samples()[i + 1]), i32::from(pix.samples()[i + 2])]
            };
            // Inside the polygon, away from the label: a pale red wash; on its edge: strong red; the polyline's second leg.
            Ok((at(120, 120), at(200, 100), at(200, 340), at(400, 400)))
        })
        .unwrap();
    let (wash, edge, leg, blank) = px;
    assert!(wash[0] > wash[1] + 10 && wash[1] > 150, "wash {wash:?}");
    assert!(edge[0] > edge[1] + 60, "edge {edge:?}");
    assert!(leg[0] > leg[1] + 60, "leg {leg:?}");
    assert_eq!(blank, [255, 255, 255]);
}

// ---- review round: boxes, rotations, limits, malformed input, viewports --------------------------------------

/// Where a page-space point lands in user space, worked out by hand for each `/Rotate` (`b` = x0 y0 x1 y1 of the box,
/// `uu` = /UserUnit). Page space is the displayed page in points, y down.
fn user_of(rotate: i32, b: [f64; 4], uu: f64, d: [f64; 2]) -> [f64; 2] {
    let (x, y) = (d[0] / uu, d[1] / uu);
    match rotate {
        0 => [b[0] + x, b[3] - y],
        90 => [b[0] + y, b[1] + x],
        180 => [b[2] - x, b[1] + y],
        _ => [b[2] - y, b[3] - x],
    }
}

#[test]
fn every_rotation_box_origin_and_user_unit_store_user_space_and_measure_alike() {
    // A media box away from the origin, with and without a crop box inside it, all four rotations, and /UserUnit 1 or 2.
    let media = [100.0f32, 50.0, 712.0, 842.0];
    for (crop, b) in [("", [100.0, 50.0, 712.0, 842.0]), ("/CropBox [120 70 700 800]", [120.0, 70.0, 700.0, 800.0])] {
        for rotate in [0, 90, 180, 270] {
            for uu in [1.0f32, 2.0] {
                let spec = Spec { rotate, media, user_unit: (uu != 1.0).then_some(uu), boxes: crop.into(), ..Default::default() };
                let (eng, id, _) = open("rot-all", &spec);
                let pts = [[100.0f32, 50.0], [208.0, 194.0]];
                let (xref, q) = add(&eng, id, Kind::Distance, &pts, Some(&FT10));
                // 180 pt of paper whatever the page does: 2.5 in = 25 ft.
                assert_eq!(q.text, "25.00 ft", "rotate {rotate} crop {crop:?} uu {uu}");
                let l = with_annot(&eng, id, xref, |o| nums(o, "L"));
                let want = [user_of(rotate, b, f64::from(uu), [100.0, 50.0]), user_of(rotate, b, f64::from(uu), [208.0, 194.0])];
                for (got, want) in l.iter().zip(want.iter().flatten()) {
                    assert!((got - want).abs() < 1e-2, "rotate {rotate} crop {crop:?} uu {uu}: {l:?} vs {want:?}");
                }
                let c = with_annot(&eng, id, xref, |o| {
                    let x = o.get_dict("Measure").unwrap().unwrap().get_dict("X").unwrap().unwrap();
                    f64::from(x.get_array(0).unwrap().unwrap().get_dict("C").unwrap().unwrap().as_float().unwrap())
                });
                assert!(close(c, 10.0 / 72.0 * f64::from(uu)), "C {c} uu {uu}");
                // Read back through list: display points and the same value; an area as well.
                let m = &list_all(&eng, id).unwrap()[0];
                for (got, want) in m.points.iter().flatten().zip(pts.iter().flatten()) {
                    assert!((got - want).abs() < 1e-2, "{:?}", m.points);
                }
                assert_eq!((m.quantity.text.as_str(), m.scale), ("25.00 ft", Some(FT10)));
                let area = [[0.0, 0.0], [144.0, 0.0], [144.0, 72.0], [0.0, 72.0]];
                assert_eq!(add(&eng, id, Kind::Area, &area, Some(&FT10)).1.text, "200.00 sq ft");
            }
        }
    }
}

#[test]
fn a_new_page_scale_goes_under_existing_detail_viewports() {
    // Only a detail viewport (1 in = 1 ft over the lower left); then the page gets a scale of its own.
    let measure = "<< /Type /Measure /Subtype /RL /R (1 in = 1 ft) /X [<< /Type /NumberFormat /U (ft) /C 0.01388889 /D 100 >>] >>";
    let spec = Spec {
        page: "/VP [5 0 R]".into(),
        extra: vec![(5, format!("<< /Type /Viewport /BBox [0 0 300 300] /Measure {measure} >>"))],
        ..Default::default()
    };
    let (eng, id, path) = open("vp-order", &spec);
    scale_set(&eng, id, 0, &FT10, EditSource::User).unwrap();
    let vps = |eng: &Engine, id| eng.read_custom(id, |d| Ok(d.load_pdf_page(0)?.object().get_dict("VP")?.unwrap().len()?)).unwrap();
    assert_eq!(vps(&eng, id), 2);
    // Display (10, 782) is user (10, 10): the detail (no /FD, so no trailing zero) still wins there; elsewhere the new page scale applies.
    let inside = [[10.0, 782.0], [118.0, 638.0]];
    let outside = [[400.0, 100.0], [508.0, 244.0]];
    assert_eq!(value_of(&eng, id, 0, Kind::Distance, &inside, None).unwrap().text, "2.5 ft");
    assert_eq!(value_of(&eng, id, 0, Kind::Distance, &outside, None).unwrap().text, "25.00 ft");
    // Again: the whole-page viewport is replaced, not added to.
    scale_set(&eng, id, 0, &Scale { real: 5.0, ..FT10 }, EditSource::User).unwrap();
    assert_eq!(vps(&eng, id), 2);
    assert_eq!(value_of(&eng, id, 0, Kind::Distance, &outside, None).unwrap().text, "12.50 ft");
    assert_eq!(value_of(&eng, id, 0, Kind::Distance, &inside, None).unwrap().text, "2.5 ft");
    // Saved, reopened: same order.
    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert_eq!(value_of(&eng2, id2, 0, Kind::Distance, &inside, None).unwrap().text, "2.5 ft");
    assert_eq!(value_of(&eng2, id2, 0, Kind::Distance, &outside, None).unwrap().text, "12.50 ft");
}

#[test]
fn a_viewport_array_shared_between_pages_is_not_edited_in_place() {
    let measure = "<< /Type /Measure /Subtype /RL /R (1 in = 1 ft) /X [<< /Type /NumberFormat /U (ft) /C 0.01388889 /D 100 >>] >>";
    let spec = Spec {
        page: "/VP 10 0 R".into(),
        second: Some("/VP 10 0 R".into()),
        extra: vec![
            (10, "[11 0 R]".into()),
            (11, format!("<< /Type /Viewport /BBox [0 0 612 792] /Measure {measure} >>")),
        ],
        ..Default::default()
    };
    let (eng, id, _) = open("vp-shared", &spec);
    let one_to_one = Scale { real: 1.0, ..FT10 };
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, one_to_one);
    scale_set(&eng, id, 0, &FT10, EditSource::User).unwrap();
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, FT10);
    // The other page still has the shared viewport as it was.
    assert_eq!(scale_get(&eng, id, 1).unwrap().scale, one_to_one);
    eng.undo(id).unwrap();
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, one_to_one);
}

#[test]
fn a_page_with_no_usable_transform_does_not_break_the_list() {
    let spec = Spec {
        page: "/Annots [5 0 R]".into(),
        // A second page with /UserUnit 0: MuPDF's transform for it cannot be inverted.
        second: Some("/UserUnit 0 /Annots [6 0 R]".into()),
        extra: vec![
            (
                5,
                "<< /Type /Annot /Subtype /Line /Rect [0 0 100 100] /L [0 0 72 0] /F 4 /Measure \
                 << /Type /Measure /Subtype /RL /R (1 in = 1 ft) /X [<< /Type /NumberFormat /U (ft) /C 0.01388889 /D 100 /FD true >>] >> >>"
                    .into(),
            ),
            (
                6,
                "<< /Type /Annot /Subtype /Line /Rect [0 0 100 100] /L [0 0 72 0] /F 4 /Measure \
                 << /Type /Measure /Subtype /RL /X [<< /Type /NumberFormat /U (ft) /C 0.01388889 /D 100 >>] >> >>"
                    .into(),
            ),
        ],
        ..Default::default()
    };
    let (eng, id, _) = open("zero-uu", &spec);
    let all = list_all(&eng, id).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].page, all[0].quantity.text.as_str()), (0, "1.00 ft"));
    assert!(scale_get(&eng, id, 1).is_err());
    assert!(add_one(&eng, id, 1, Kind::Distance, &[[0.0, 0.0], [10.0, 0.0]], None, "", EditSource::User).is_err());
}

#[test]
fn malformed_measures_are_left_out_and_never_panic() {
    let ok = "<< /Type /Measure /Subtype /RL /X [<< /Type /NumberFormat /U (ft) /C 0.01388889 /D 100 /FD true >>] >>";
    let line = |n: usize, measure: &str| {
        (n, format!("<< /Type /Annot /Subtype /Line /Rect [0 0 100 100] /L [0 0 72 0] /F 4 /Measure {measure} >>"))
    };
    let spec = Spec {
        page: "/Annots [5 0 R 6 0 R 7 0 R 8 0 R 9 0 R 10 0 R 11 0 R]".into(),
        extra: vec![
            line(5, ok),
            line(6, "5"),                                                                       // not a dictionary
            line(7, "<< /Type /Measure /Subtype /RL /X [<< /U (ft) /C 0 >>] >>"),               // C = 0
            line(8, "<< /Type /Measure /Subtype /RL >>"),                                       // no /X
            line(9, "<< /Type /Measure /Subtype /RL /X [5 (x) null] >>"),                       // elements not dictionaries
            // A huge precision: read, clamped, and the text is still made.
            line(10, "<< /Type /Measure /Subtype /RL /X [<< /U (ft) /C 0.01388889 /D 99999999999 /FD true >>] >>"),
            line(11, "<< /Type /Measure /Subtype /Other /X [<< /U (ft) /C 1 >>] >>"),           // not a rectilinear measure
        ],
        ..Default::default()
    };
    let (eng, id, _) = open("malformed", &spec);
    let all = list_all(&eng, id).unwrap();
    let xrefs: Vec<&str> = all.iter().map(|m| m.quantity.unit.as_str()).collect();
    assert_eq!(all.len(), 2, "{xrefs:?}");
    assert_eq!(all[0].quantity.text, "1.00 ft");
    assert!(all[1].quantity.text.starts_with("1.0000000"), "{}", all[1].quantity.text);
    // A page-level viewport with junk is skipped, and the page scale falls back.
    assert!(!scale_get(&eng, id, 0).unwrap().defined);
}

#[test]
fn point_counts_and_coordinates_are_capped() {
    let (eng, id, _) = open("caps", &Spec::default());
    let steps = eng.history(id).unwrap().len();
    let many: Vec<[f32; 2]> = (0..=write::MAX_POINTS).map(|i| [i as f32 * 0.01, (i % 7) as f32]).collect();
    assert!(add_one(&eng, id, 0, Kind::Perimeter, &many, Some(&FT10), "", EditSource::User).is_err());
    assert!(value_of(&eng, id, 0, Kind::Area, &many, Some(&FT10)).is_err());
    let far = [[0.0, 0.0], [2e7, 0.0]];
    assert!(add_one(&eng, id, 0, Kind::Distance, &far, Some(&FT10), "", EditSource::User).is_err());
    assert!(value_of(&eng, id, 0, Kind::Distance, &far, None).is_err());
    assert_eq!(eng.history(id).unwrap().len(), steps);
    // The limits themselves are fine.
    let edge = [[0.0, 0.0], [9.9e6, 0.0]];
    assert!(value_of(&eng, id, 0, Kind::Distance, &edge, None).is_ok());
    let max: Vec<[f32; 2]> = many[..write::MAX_POINTS].to_vec();
    assert!(add_one(&eng, id, 0, Kind::Perimeter, &max, Some(&FT10), "", EditSource::User).is_ok());
}

#[test]
fn the_y_axis_and_cyx_scale_a_measure_that_has_them() {
    // X: 1 user unit = 1 ft; Y: 1 user unit = 2 (some other unit); CYX 2 turns Y units into X units: y counts 4.
    let spec = Spec {
        page: "/Annots [5 0 R]".into(),
        extra: vec![(
            5,
            "<< /Type /Annot /Subtype /Line /Rect [0 0 100 100] /L [0 0 3 1] /F 4 /Measure << /Type /Measure /Subtype /RL \
             /X [<< /U (ft) /C 1 /D 100 /FD true >>] /Y [<< /U (m) /C 2 /D 100 >>] /CYX 2 >> >>"
                .into(),
        )],
        ..Default::default()
    };
    let (eng, id, _) = open("cyx", &spec);
    // sqrt(3^2 + (1 x 2 x 2)^2) = 5.
    assert_eq!(list_all(&eng, id).unwrap()[0].quantity.text, "5.00 ft");
}

#[test]
fn number_format_edges() {
    let f = NumberFormat::new("ft", 1.0);
    let frac = NumberFormat { style: Style::Fraction, d: 8.0, fixed: false, ..f.clone() };
    assert_eq!(frac.number(-3.5), "-3 1/2");
    assert_eq!(frac.number(-0.25), "-1/4");
    assert_eq!(NumberFormat { d: 1e300, ..f.clone() }.number(1.5).len(), 11);
    // A suffix that is a single space does not leave a blank at the end of the label.
    assert_eq!(chain_text(2.0, &[NumberFormat { ss: " ".into(), ..f }]), "2.00 ft");
}

#[test]
fn snapping_is_cached_per_revision_and_follows_edits() {
    let (eng, id, _) = open("snap-cache", &Spec { content: PATHS, ..Default::default() });
    let cache = SnapCache::default();
    let ask = |x, y| snap_at(&eng, &cache, id, 0, x, y, 5.0).unwrap();
    assert_eq!(ask(302.0, 694.0).unwrap().kind, SnapKind::Endpoint);
    let first = cache.index(&eng, id, 0).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &cache.index(&eng, id, 0).unwrap()));
    // A step that changes the page space (turning the page) is a new revision: a new index, new coordinates.
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    assert_eq!(ask(302.0, 694.0), None);
    let h = ask(102.0, 302.0).unwrap();
    assert_eq!((h.x, h.y), (100.0, 300.0));
    assert!(!std::sync::Arc::ptr_eq(&first, &cache.index(&eng, id, 0).unwrap()));
    // Undo is a revision too: the old coordinates are back.
    eng.undo(id).unwrap();
    assert_eq!(ask(302.0, 694.0).unwrap().kind, SnapKind::Endpoint);
}

#[test]
fn a_hostile_page_is_cut_off_at_the_segment_limit() {
    // 5000 paths of 30 lines each: 150k segments, over the 100k limit.
    let mut content = String::new();
    for i in 0..5000 {
        content.push_str(&format!("{} 10 m ", i % 600));
        for k in 0..30 {
            content.push_str(&format!("{} {} l ", (i + k) % 600, 10 + k * 20));
        }
        content.push_str("S\n");
    }
    let content: &'static str = Box::leak(content.into_boxed_str());
    let (eng, id, _) = open("hostile", &Spec { content, ..Default::default() });
    let started = std::time::Instant::now();
    let idx = eng.read_custom(id, |d| SnapIndex::build(d, 0)).unwrap();
    assert!(idx.truncated);
    assert!(started.elapsed() < snap::TIME_LIMIT * 3, "{:?}", started.elapsed());
    // What was read still snaps: the first path starts at (0, 10) -> display (0, 782).
    assert_eq!(idx.snap(0.5, 781.5, 3.0).unwrap().kind, SnapKind::Endpoint);
}

// ---- Codex round: one read for concurrent requests, a watchdog, enclosing viewports --------------------------

/// A page of many paths, so a read takes long enough for requests to overlap.
fn busy_content() -> &'static str {
    let mut content = String::new();
    for i in 0..3000 {
        content.push_str(&format!("{} 10 m ", i % 600));
        for k in 0..20 {
            content.push_str(&format!("{} {} l ", (i + k) % 600, 10 + k * 30));
        }
        content.push_str("S\n");
    }
    Box::leak(content.into_boxed_str())
}

#[test]
fn concurrent_first_requests_share_one_read() {
    let (eng, id, _) = open("snap-coalesce", &Spec { content: busy_content(), ..Default::default() });
    let cache = SnapCache::default();
    let got: Vec<std::sync::Arc<SnapIndex>> = std::thread::scope(|s| {
        let tasks: Vec<_> = (0..8).map(|_| s.spawn(|| cache.index(&eng, id, 0).unwrap())).collect();
        tasks.into_iter().map(|t| t.join().unwrap()).collect()
    });
    assert_eq!(cache.builds.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(got.iter().all(|i| std::sync::Arc::ptr_eq(i, &got[0])));
    // A later revision is a read of its own.
    eng.apply(id, EditOp::Rotate { pages: vec![0], quarter_turns: 1 }).unwrap();
    cache.index(&eng, id, 0).unwrap();
    assert_eq!(cache.builds.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn the_watchdog_stops_a_read_that_overruns_its_time() {
    let (eng, id, _) = open("snap-watchdog", &Spec { content: busy_content(), ..Default::default() });
    // No time at all: the watchdog fires at once, the run is aborted, and the index says it is cut short.
    let cut = eng.read_custom(id, |d| SnapIndex::build_within(d, 0, std::time::Duration::ZERO)).unwrap();
    assert!(cut.truncated);
    // With the real limit the same page is read whole (3000 paths of 20 segments).
    let whole = eng.read_custom(id, |d| SnapIndex::build(d, 0)).unwrap();
    assert!(!whole.truncated);
    assert!(whole.snap(0.5, 781.5, 3.0).is_some());
}

#[test]
fn a_viewport_that_encloses_the_crop_box_is_the_page_scale() {
    // The crop box is smaller than the media box; the existing page viewport is the media box (1 in = 1 ft).
    let measure = "<< /Type /Measure /Subtype /RL /R (1 in = 1 ft) /X [<< /Type /NumberFormat /U (ft) /C 0.01388889 /D 100 >>] >>";
    let spec = Spec {
        boxes: "/CropBox [50 50 500 700]".into(),
        page: "/VP [5 0 R]".into(),
        extra: vec![(5, format!("<< /Type /Viewport /BBox [0 0 612 792] /Measure {measure} >>"))],
        ..Default::default()
    };
    let (eng, id, _) = open("vp-enclosing", &spec);
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, Scale { real: 1.0, ..FT10 });
    scale_set(&eng, id, 0, &FT10, EditSource::User).unwrap();
    // The enclosing viewport was replaced, not shadowed: one viewport, the new scale in force everywhere.
    let n = eng.read_custom(id, |d| Ok(d.load_pdf_page(0)?.object().get_dict("VP")?.unwrap().len()?)).unwrap();
    assert_eq!(n, 1);
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, FT10);
    assert_eq!(value_of(&eng, id, 0, Kind::Distance, &[[10.0, 10.0], [118.0, 154.0]], None).unwrap().text, "25.00 ft");
    // Undoing restores the file's own viewport.
    eng.undo(id).unwrap();
    assert_eq!(scale_get(&eng, id, 0).unwrap().scale, Scale { real: 1.0, ..FT10 });
}

// ---- the UI's calls: change a scale, export ------------------------------------------------------------------

#[test]
fn rescale_replaces_the_measurement_in_one_undo_step() {
    let (eng, id, path) = open("rescale", &Spec::default());
    let (xref, q) = add(&eng, id, Kind::Distance, &[[100.0, 100.0], [300.0, 100.0]], Some(&FT10));
    assert_eq!(q.text, "27.78 ft");
    let base = eng.history(id).unwrap().len();

    // 1 in = 1 m: the same 200 pt line is now 2.78 m.
    let m = Scale { paper: 1.0, paper_unit: Unit::In, real: 1.0, real_unit: Unit::M };
    let (st, new, q2) = rescale(&eng, id, 0, xref, &m, EditSource::User).unwrap();
    assert!(st.dirty);
    assert_eq!(q2.text, "2.78 m");
    let h = eng.history(id).unwrap();
    assert_eq!(h.len(), base + 1, "one step");
    assert!(h.last().unwrap().label.starts_with("Change the scale of a measurement on p. 1"));
    let all = list_all(&eng, id).unwrap();
    assert_eq!(all.len(), 1, "replaced, not added");
    assert_eq!(all[0].xref, new);
    assert_eq!(all[0].points, vec![[100.0, 100.0], [300.0, 100.0]]);
    assert_eq!(all[0].author, "Alex Johnson");
    assert_eq!(all[0].scale, Some(m));

    // Undo brings the first one back whole; a stale xref and a bad scale are refused and leave no step.
    eng.undo(id).unwrap();
    let back = list_all(&eng, id).unwrap();
    assert_eq!((back.len(), back[0].quantity.text.as_str()), (1, "27.78 ft"));
    let n = eng.history(id).unwrap().len();
    assert!(rescale(&eng, id, 0, 99_999, &m, EditSource::User).is_err());
    let zero = Scale { paper: 0.0, ..m };
    assert!(rescale(&eng, id, 0, back[0].xref, &zero, EditSource::User).is_err());
    assert_eq!(eng.history(id).unwrap().len(), n);

    // Saved and reopened it is the new one.
    rescale(&eng, id, 0, back[0].xref, &m, EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let m2 = list_all(&eng2, id2).unwrap();
    assert_eq!((m2.len(), m2[0].quantity.text.as_str()), (1, "2.78 m"));
}

#[test]
fn csv_has_value_and_unit_apart_and_is_safe_in_a_spreadsheet() {
    let (eng, id, _) = open("csv", &Spec::default());
    add(&eng, id, Kind::Distance, &[[0.0, 0.0], [720.0, 0.0]], Some(&FT10));
    add(&eng, id, Kind::Area, &[[0.0, 0.0], [72.0, 0.0], [72.0, 72.0], [0.0, 72.0]], Some(&FT10));
    let mut items = list_all(&eng, id).unwrap();
    items[1].author = "=HYPERLINK(\"x\"),\"Bo\"".into();
    let csv = csv_of(&items);
    let lines: Vec<&str> = csv.trim_end().split("\r\n").collect();
    assert_eq!(lines[0], "\u{feff}Page,Type,Value,Unit,Label,Scale,Author");
    assert_eq!(lines[1], "1,distance,100,ft,100.00 ft,1 in = 10 ft,Alex Johnson");
    // 1 sq in at 10 ft per inch is 100 sq ft; the formula-looking author is quoted and defused.
    assert_eq!(lines[2], "1,area,100,sq ft,100.00 sq ft,1 in = 10 ft,\"'=HYPERLINK(\"\"x\"\"),\"\"Bo\"\"\"");
    assert_eq!(csv_of(&[]), "\u{feff}Page,Type,Value,Unit,Label,Scale,Author\r\n");
}

#[test]
fn an_export_replaces_a_file_the_user_confirmed_over() {
    // The save dialog lets the user pick an existing file; the write goes through a temporary file and a rename that
    // replaces the destination (MoveFileExW with MOVEFILE_REPLACE_EXISTING on Windows), never a failure.
    let to = dir("overwrite").join("m.csv");
    std::fs::write(&to, "old,contents that are longer than the new ones\r\n").unwrap();
    crate::organize::app::write_plain(&to, csv_of(&[]).as_bytes()).unwrap();
    assert_eq!(std::fs::read_to_string(&to).unwrap(), csv_of(&[]));
    let left: Vec<_> = std::fs::read_dir(to.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left.len(), 1, "no temporary file is left behind: {left:?}");
}

// ---- number formats and a file's own /D and /A -----------------------------------------------------------------

fn fmt(unit: &str) -> NumberFormat {
    NumberFormat::new(unit, 1.0)
}

#[test]
fn a_chain_of_units_reads_whole_units_then_the_remainder() {
    // miles, feet (5280 to the mile), inches (12 to the foot, in eighths, reduced): 1 mi 2 ft 3 1/2 in.
    let chain = [
        fmt("mi"),
        NumberFormat::new("ft", 5280.0),
        NumberFormat { style: Style::Fraction, d: 8.0, fixed: false, ..NumberFormat::new("in", 12.0) },
    ];
    assert_eq!(chain_text(1.0 + (2.0 + 3.5 / 12.0) / 5280.0, &chain), "1 mi 2 ft 3 1/2 in");
    // The whole units of the first element stay whole; nothing is left over to show.
    assert_eq!(chain_text(3.0, &chain), "3 mi 0 ft 0 in");
}

#[test]
fn a_remainder_that_rounds_up_to_a_whole_unit_carries() {
    let ft_in = [fmt("ft"), NumberFormat::new("in", 12.0)];
    assert_eq!(chain_text(2.999_999, &ft_in), "3 ft 0.00 in");
    let eighths = [fmt("ft"), NumberFormat { style: Style::Fraction, d: 8.0, fixed: false, ..NumberFormat::new("in", 12.0) }];
    assert_eq!(chain_text(2.999, &eighths), "3 ft 0 in");
    // Cascades through every element: 1.9999999 h is 2 h, not 1 h 59 min 60.00 s.
    let clock = [fmt("h"), NumberFormat::new("min", 60.0), NumberFormat::new("s", 60.0)];
    assert_eq!(chain_text(1.999_999_99, &clock), "2 h 0 min 0.00 s");
    // A value that needs no carry is untouched.
    assert_eq!(chain_text(2.5, &ft_in), "2 ft 6.00 in");
}

#[test]
fn separators_styles_and_the_label_position_follow_the_format() {
    let eu = NumberFormat { thousands: ".".into(), decimal: ",".into(), ..fmt("m") };
    assert_eq!(eu.text(1_234_567.891), "1.234.567,89 m");
    // /FD false drops trailing zeros, /F /R rounds, /F /T truncates.
    assert_eq!(NumberFormat { fixed: false, ..fmt("m") }.number(2.5), "2.5");
    assert_eq!(NumberFormat { fixed: false, ..fmt("m") }.number(2.0), "2");
    assert_eq!(NumberFormat { style: Style::Round, ..fmt("m") }.number(2.5), "3");
    assert_eq!(NumberFormat { style: Style::Truncate, ..fmt("m") }.number(2.9), "2");
    // /D 10 is one decimal, /D 1000 three.
    assert_eq!(NumberFormat { d: 10.0, ..fmt("m") }.number(1.26), "1.3");
    assert_eq!(NumberFormat { d: 1000.0, ..fmt("m") }.number(1.2345), "1.235");
    // After the number the label is /PS, /U, /SS.
    assert_eq!(NumberFormat { ps: "_".into(), ss: "!".into(), ..fmt("ft") }.text(3.0), "3.00_ft!");
}

/// A page with one line whose /Measure has `x`, `d` and `a` as given (number format arrays, as PDF text).
fn own_formats(x: &str, d: &str, a: &str) -> Spec {
    let measure = format!("<< /Type /Measure /Subtype /RL /R (1 in = 10 ft) /X [{x}] /D [{d}] /A [{a}] >>");
    Spec {
        page: "/Annots [5 0 R]".into(),
        extra: vec![(
            5,
            format!("<< /Type /Annot /Subtype /Line /Rect [0 0 200 200] /L [0 0 108 0] /IT /LineDimension /F 4 /Measure {measure} >>"),
        )],
        ..Default::default()
    }
}

const XFT: &str = "<< /Type /NumberFormat /U (ft) /C 0.0138888889 /D 100 /FD true >>"; // 72 user units to the foot

#[test]
fn a_files_own_distance_chain_values_and_labels_the_measurement() {
    // /D: feet then inches (12 to the foot); 108 units = 1.5 ft = 1 ft 6.00 in.
    let d = "<< /Type /NumberFormat /U (ft) /C 1 /D 1 /F /D >> << /Type /NumberFormat /U (in) /C 12 /D 100 /F /D /FD true >>";
    let a = "<< /Type /NumberFormat /U (sq ft) /C 1 /D 100 /F /D /FD true >> << /Type /NumberFormat /U (sq in) /C 144 /D 100 /F /D /FD true >>";
    let (eng, id, _) = open("own-chain", &own_formats(XFT, d, a));
    let m = &list_all(&eng, id).unwrap()[0];
    assert_eq!(m.quantity.text, "1 ft 6.00 in");
    assert_eq!(m.quantity.note, None);
    assert!(!m.plain, "the UI cannot show this label by itself");
    // The live label asks the engine for the points under the annotation's own /Measure; 1 ft 0 in at 72 units.
    // y = 792 is the top edge of the page in page space.
    let q = value_in(&eng, id, 0, Kind::Distance, &[[0.0, 792.0], [72.0, 792.0]], m.xref).unwrap();
    assert_eq!(q.text, "1 ft 0.00 in");
    // An area reads through /A: a 72 x 72 unit square is 1 sq ft; half a unit more makes 1 sq ft 72.00 sq in.
    let sq = [[0.0, 792.0], [72.0, 792.0], [72.0, 720.0], [0.0, 720.0]];
    assert_eq!(value_in(&eng, id, 0, Kind::Area, &sq, m.xref).unwrap().text, "1 sq ft 0.00 sq in");
    // Moving a point writes that label and keeps the file's formats.
    let (_, q) = move_points(&eng, id, 0, m.xref, &[[0.0, 792.0], [144.0, 792.0]], EditSource::User).unwrap();
    assert_eq!(q.text, "2 ft 0.00 in");
    with_annot(&eng, id, m.xref, |o| {
        assert_eq!(o.get_dict("Contents").unwrap().unwrap().as_string().unwrap(), "2 ft 0.00 in");
        let measure = o.get_dict("Measure").unwrap().unwrap();
        assert_eq!(measure.get_dict("D").unwrap().unwrap().len().unwrap(), 2, "the file's own /D is still there");
    });
}

#[test]
fn a_malformed_number_format_falls_back_with_a_note_and_stays_listed() {
    let ok_d = "<< /Type /NumberFormat /U (ft) /C 1 /D 100 /FD true >>";
    let nine = vec![ok_d; 9].join(" ");
    for (case, d) in [
        ("zero /C", "<< /Type /NumberFormat /U (in) /C 0 >>".to_string()),
        ("negative /C", "<< /Type /NumberFormat /U (in) /C -3 >>".to_string()),
        ("no /U", "<< /Type /NumberFormat /C 1 >>".to_string()),
        ("a bad link in a chain", format!("{ok_d} 5 << /Type /NumberFormat /U (in) /C 12 >>")),
        ("a chain of nine", nine.clone()),
    ] {
        let (eng, id, _) = open("malformed-d", &own_formats(XFT, &d, ok_d));
        let all = list_all(&eng, id).unwrap();
        assert_eq!(all.len(), 1, "{case}: still listed");
        // 108 units at 72 to the foot, in the /X format: what a measure without /D reads.
        assert_eq!(all[0].quantity.text, "1.50 ft", "{case}");
        let note = all[0].quantity.note.as_deref().unwrap_or_default();
        assert!(note.contains("/D") && note.contains("malformed"), "{case}: {note:?}");
        assert!(!all[0].plain, "{case}: the engine words the note");
    }
    // The cap is 8: a chain of eight is fine.
    let eight = vec![ok_d; 8].join(" ");
    let (eng, id, _) = open("chain-8", &own_formats(XFT, &eight, ok_d));
    assert_eq!(list_all(&eng, id).unwrap()[0].quantity.note, None);
}

#[test]
fn an_endless_x_chain_is_left_out_not_walked() {
    let x = vec![XFT; 9].join(" ");
    let (eng, id, _) = open("x-chain", &own_formats(&x, XFT, XFT));
    assert!(list_all(&eng, id).unwrap().is_empty());
}

#[test]
fn a_number_format_array_that_refers_to_itself_falls_back() {
    // /D is an indirect array whose only element is itself (20), or an object that is its own value (21).
    for (case, d, objs) in [
        ("array holding itself", "20 0 R", vec![(20, "[20 0 R]".to_string())]),
        ("reference to itself", "21 0 R", vec![(21, "21 0 R".to_string())]),
    ] {
        let measure = format!("<< /Type /Measure /Subtype /RL /R (1 in = 10 ft) /X [{XFT}] /D {d} /A {d} >>");
        let mut extra = vec![(
            5,
            format!("<< /Type /Annot /Subtype /Line /Rect [0 0 200 200] /L [0 0 108 0] /IT /LineDimension /F 4 /Measure {measure} >>"),
        )];
        extra.extend(objs);
        let spec = Spec { page: "/Annots [5 0 R]".into(), extra, ..Default::default() };
        let (eng, id, _) = open("self-ref", &spec);
        let all = list_all(&eng, id).unwrap();
        assert_eq!(all.len(), 1, "{case}: still listed");
        assert_eq!(all[0].quantity.text, "1.50 ft", "{case}");
        assert!(all[0].quantity.note.is_some(), "{case}: says it was left out");
        let q = value_in(&eng, id, 0, Kind::Distance, &[[0.0, 792.0], [72.0, 792.0]], all[0].xref).unwrap();
        assert_eq!(q.text, "1.00 ft", "{case}");
    }
}

#[test]
fn an_area_falling_back_to_a_chained_x_uses_its_first_unit_only() {
    // No /A: /X is feet then inches (C 12); an area of 72 x 72 units is 1 sq ft, never "1 sq ft 0.00 sq in" with a
    // linear 12 between the units.
    let x = format!("{XFT} << /Type /NumberFormat /U (in) /C 12 /D 100 /FD true >>");
    let measure = format!("<< /Type /Measure /Subtype /RL /R (1 in = 10 ft) /X [{x}] >>");
    let spec = Spec {
        page: "/Annots [5 0 R]".into(),
        extra: vec![(
            5,
            format!(
                "<< /Type /Annot /Subtype /Polygon /Rect [0 0 72 72] /Vertices [0 0 72 0 72 72 0 72] /IT /PolygonDimension /F 4 /Measure {measure} >>"
            ),
        )],
        ..Default::default()
    };
    let (eng, id, _) = open("area-chain-x", &spec);
    let all = list_all(&eng, id).unwrap();
    assert_eq!(all[0].quantity.text, "1.00 sq ft");
    assert_eq!(all[0].quantity.unit, "sq ft");
}

/// A line from another app: blue, 3 pt wide, open arrows at both ends and leaders of 20 pt (+5 beyond, 2 pt gap); its
/// form is shared with a second line and claims to be ours.
fn foreign_line() -> Spec {
    let d = "<< /Type /NumberFormat /U (ft) /C 1 /D 100 /FD true >>";
    let measure = format!("<< /Type /Measure /Subtype /RL /R (1 in = 10 ft) /X [{XFT}] /D [{d}] >>");
    Spec {
        page: "/Annots [5 0 R 8 0 R]".into(),
        extra: vec![
            (
                5,
                format!(
                    "<< /Type /Annot /Subtype /Line /Rect [0 0 108 10] /L [0 0 108 0] /IT /LineDimension /F 4 /C [0 0 1] /BS << /W 3 >> \
                     /LE [/OpenArrow /OpenArrow] /LL 20 /LLE 5 /LLO 2 /Contents (1.50 ft) /AP << /N 7 0 R >> /Measure {measure} >>"
                ),
            ),
            (7, "<< /Type /XObject /Subtype /Form /BBox [0 0 108 10] /InkwellMeasure true /Length 3 >>\nstream\nq Q\nendstream".into()),
            // A second annotation drawn with the same form.
            (
                8,
                format!(
                    "<< /Type /Annot /Subtype /Line /Rect [0 100 108 110] /L [0 100 108 100] /F 4 /AP << /N 7 0 R >> /Measure {measure} >>"
                ),
            ),
        ],
        ..Default::default()
    }
}

#[test]
fn moving_a_foreign_measure_keeps_its_look_and_does_not_clip_it() {
    let (eng, id, _) = open("foreign-look", &foreign_line());
    let xref = list_all(&eng, id).unwrap().iter().find(|m| m.contents == "1.50 ft").unwrap().xref;
    let steps = history_len(&eng, id);
    // 144 units: 2 ft.
    let (_, q) = move_points(&eng, id, 0, xref, &[[0.0, 792.0], [144.0, 792.0]], EditSource::User).unwrap();
    assert_eq!(q.text, "2.00 ft");
    assert_eq!(history_len(&eng, id), steps + 1);
    with_annot(&eng, id, xref, |o| {
        // Its own entries are still there.
        assert_eq!(nums(o, "C"), vec![0.0, 0.0, 1.0]);
        assert_eq!(o.get_dict("BS").unwrap().unwrap().get_dict("W").unwrap().unwrap().as_float().unwrap(), 3.0);
        assert_eq!(o.get_dict("LE").unwrap().unwrap().len().unwrap(), 2);
        let ap = o.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
        let drawing = String::from_utf8(ap.read_stream().unwrap()).unwrap();
        // It is drawn in that look: blue, 3 pt, with the arrows (a closed path of three points is not there).
        assert!(drawing.contains("0.000 0.000 1.000 RG"), "{drawing}");
        assert!(drawing.contains(" 3 w"), "{drawing}");
        assert!(!drawing.contains("0.800 0.150 0.120"), "{drawing}");
        // Page space (y down, the line at 792): the dimension line is drawn 20 below the points, with the leaders from
        // 2 below each point to 5 past it. All of it is inside /Rect with the stroke's half width.
        assert!(drawing.contains("0.0000 812.0000 m\n144.0000 812.0000 l\nS"), "{drawing}");
        assert!(drawing.contains("0.0000 794.0000 m 0.0000 817.0000 l S"), "{drawing}");
        assert!(drawing.contains("144.0000 794.0000 m 144.0000 817.0000 l S"), "{drawing}");
        let r = nums(o, "Rect");
        assert!(r[0] <= -1.5 && r[2] >= 144.0 + 1.5, "{r:?}");
        assert!(r[1] <= -25.0 - 1.5, "{r:?}");
        let bbox = nums(&ap, "BBox");
        assert_eq!(bbox, r);
    });
    // The form they shared (which called itself ours) is another annotation's: untouched, and still its form.
    let other = list_all(&eng, id).unwrap().iter().find(|m| m.xref != xref).unwrap().xref;
    with_annot(&eng, id, other, |o| {
        let ap = o.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
        assert_eq!(ap.as_indirect().unwrap(), 7);
        assert_eq!(ap.read_stream().unwrap(), b"q Q");
        assert_eq!(nums(&ap, "BBox"), vec![0.0, 0.0, 108.0, 10.0]);
        assert!(ap.get_dict("Resources").unwrap().is_none(), "nothing was added to the shared form");
    });
    with_annot(&eng, id, xref, |o| {
        let ap = o.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
        assert_ne!(ap.as_indirect().unwrap(), 7, "its own new form");
    });
}

/// A polygon from another app. `annot` is extra entries of the annotation (`/IC`, `/CA`), `gs` the
/// graphics states its form carries (`/Resources` of the form).
fn foreign_area(annot: &str, gs: &str) -> Spec {
    let d = "<< /Type /NumberFormat /U (sq ft) /C 1 /D 100 /FD true >>";
    let measure = format!("<< /Type /Measure /Subtype /RL /R (1 in = 10 ft) /X [{XFT}] /D [{XFT}] /A [{d}] >>");
    let res = if gs.is_empty() { String::new() } else { format!("/Resources << /ExtGState << {gs} >> >>") };
    Spec {
        page: "/Annots [5 0 R]".into(),
        extra: vec![
            (
                5,
                format!(
                    "<< /Type /Annot /Subtype /Polygon /Rect [100 600 172 672] /Vertices [100 600 172 600 172 672 100 672] \
                     /IT /PolygonDimension /F 4 /C [0 0 1] {annot} /AP << /N 7 0 R >> /Measure {measure} >>"
                ),
            ),
            (7, format!("<< /Type /XObject /Subtype /Form /BBox [100 600 172 672] {res} /Length 3 >>\nstream\nq Q\nendstream")),
        ],
        ..Default::default()
    }
}

/// The `/ca` of the first graphics state of the annotation's own appearance, and the pixel inside the area after a move.
fn fill_after_move(test: &str, spec: &Spec) -> (f64, [i32; 3]) {
    let (eng, id, _) = open(test, spec);
    let xref = list_all(&eng, id).unwrap()[0].xref;
    // The same 72 x 72 square, moved up to the top of the page (page space, y down).
    let square = [[100.0, 100.0], [172.0, 100.0], [172.0, 172.0], [100.0, 172.0]];
    move_points(&eng, id, 0, xref, &square, EditSource::User).unwrap();
    // A second move reads the form the first one wrote: the opacity is carried, not reset.
    let again = [[100.0, 100.0], [172.0, 100.0], [172.0, 172.0], [100.0, 180.0]];
    move_points(&eng, id, 0, xref, &again, EditSource::User).unwrap();
    let ca = with_annot(&eng, id, xref, |o| {
        let ap = o.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
        let gs = ap.get_dict("Resources").unwrap().unwrap().get_dict("ExtGState").unwrap().unwrap().get_dict("GS1").unwrap().unwrap();
        f64::from(gs.get_dict("ca").unwrap().unwrap().as_float().unwrap())
    });
    let px = eng
        .read_custom(id, |d| {
            let p = d.load_pdf_page(0)?;
            let pix = p.to_pixmap(&mupdf::Matrix::IDENTITY, &mupdf::Colorspace::device_rgb(), false, true)?;
            let (w, n) = (pix.width() as usize, pix.n() as usize);
            let i = (130 * w + 120) * n;
            Ok([i32::from(pix.samples()[i]), i32::from(pix.samples()[i + 1]), i32::from(pix.samples()[i + 2])])
        })
        .unwrap();
    (ca, px)
}

#[test]
fn a_moved_foreign_area_keeps_the_fill_opacity_the_file_asked_for() {
    const RED: &str = "/IC [1 0 0]";
    // An /IC and no opacity anywhere: opaque, as it was drawn before the move.
    let (ca, px) = fill_after_move("fill-opaque", &foreign_area(RED, ""));
    assert_eq!(ca, 1.0);
    assert!(px[0] > 200 && px[1] < 30 && px[2] < 30, "solid red inside: {px:?}");
    // /CA on the annotation.
    let (ca, px) = fill_after_move("fill-ca", &foreign_area("/IC [1 0 0] /CA 0.5", ""));
    assert_eq!(ca, 0.5);
    assert!(px[0] > 200 && (100..160).contains(&px[1]), "half red inside: {px:?}");
    // /ca of its appearance's graphics state, when the annotation says nothing.
    let (ca, _) = fill_after_move("fill-gs", &foreign_area(RED, "/GS0 << /Type /ExtGState /ca 0.25 /CA 1 >>"));
    assert_eq!(ca, 0.25);
    // The annotation's /CA wins over the form's.
    let (ca, _) = fill_after_move("fill-both", &foreign_area("/IC [1 0 0] /CA 0.5", "/GS0 << /ca 0.25 >>"));
    assert_eq!(ca, 0.5);
    // No /IC: the file draws no fill, so an opacity it names is no reason to paint one; the light wash stays.
    for (test, annot, gs) in [("wash-ca", "/CA 1", ""), ("wash-gs", "", "/GS0 << /ca 1 >>")] {
        let (ca, px) = fill_after_move(test, &foreign_area(annot, gs));
        assert!((ca - 0.12).abs() < 1e-6, "{test}: {ca}");
        assert!(px[0] > 200 && px[1] > 150, "{test}: pale inside: {px:?}");
    }
}

#[test]
fn an_area_of_ours_keeps_its_light_wash_when_moved() {
    let (eng, id, _) = open("own-wash", &Spec::default());
    let (xref, _) = add(&eng, id, Kind::Area, &[[100.0, 100.0], [172.0, 100.0], [172.0, 172.0], [100.0, 172.0]], Some(&FT10));
    move_points(&eng, id, 0, xref, &[[100.0, 100.0], [172.0, 100.0], [172.0, 172.0], [100.0, 200.0]], EditSource::User).unwrap();
    let ca = with_annot(&eng, id, xref, |o| {
        let ap = o.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
        let gs = ap.get_dict("Resources").unwrap().unwrap().get_dict("ExtGState").unwrap().unwrap().get_dict("GS1").unwrap().unwrap();
        f64::from(gs.get_dict("ca").unwrap().unwrap().as_float().unwrap())
    });
    assert!((ca - 0.12).abs() < 1e-6, "{ca}");
}

#[test]
fn plain_says_whether_the_ui_can_label_it_alone() {
    // Nothing on the page, and a scale of ours, are plain; so is an annotation we made.
    let (eng, id, _) = open("plain", &Spec::default());
    assert!(scale_get(&eng, id, 0).unwrap().plain);
    scale_set(&eng, id, 0, &FT10, EditSource::User).unwrap();
    assert!(scale_get(&eng, id, 0).unwrap().plain);
    add(&eng, id, Kind::Distance, &[[10.0, 10.0], [80.0, 10.0]], None);
    assert!(list_all(&eng, id).unwrap()[0].plain);
    // A page with a detail viewport over a page scale is valued by the engine.
    let (eng, id, _) = open("plain-vp", &acrobat());
    assert!(!scale_get(&eng, id, 0).unwrap().plain);
}

// ---- dragging a point -------------------------------------------------------------------------------------------

fn history_len(eng: &Engine, id: DocId) -> usize {
    eng.history(id).unwrap().len()
}

#[test]
fn moving_a_point_rewrites_the_annotation_in_place_in_one_undo_step() {
    let (eng, id, path) = open("move", &Spec::default());
    let square = [[100.0, 100.0], [172.0, 100.0], [172.0, 172.0], [100.0, 172.0]];
    let cases: [(Kind, Vec<[f32; 2]>, Vec<[f32; 2]>, &str, &str); 3] = [
        // 200 pt = 27.78 ft; then 144 pt = 20.00 ft.
        (Kind::Distance, vec![[100.0, 100.0], [300.0, 100.0]], vec![[100.0, 100.0], [100.0, 244.0]], "27.78 ft", "20.00 ft"),
        // 72 + 72 = 144 pt = 20.00 ft; then 72 + 144 = 216 pt = 30.00 ft.
        (Kind::Perimeter, vec![[100.0, 100.0], [172.0, 100.0], [172.0, 172.0]], vec![[100.0, 100.0], [172.0, 100.0], [172.0, 244.0]], "20.00 ft", "30.00 ft"),
        // 1 sq in = 100 sq ft; then the last corner moved down 72 pt: shoelace 7776 pt^2 = 1.5 sq in = 150 sq ft.
        (Kind::Area, square.to_vec(), vec![square[0], square[1], square[2], [100.0, 244.0]], "100.00 sq ft", "150.00 sq ft"),
    ];
    for (kind, before, after, v0, v1) in cases {
        let (xref, q) = add(&eng, id, kind, &before, Some(&FT10));
        assert_eq!(q.text, v0);
        let user_before = with_annot(&eng, id, xref, |o| {
            let key = if kind == Kind::Distance { "L" } else { "Vertices" };
            (nums(o, "Rect"), nums(o, key))
        });
        let steps = history_len(&eng, id);

        let (st, q) = move_points(&eng, id, 0, xref, &after, EditSource::User).unwrap();
        assert!(st.dirty);
        assert_eq!(q.text, v1, "{kind:?}");
        assert_eq!(history_len(&eng, id), steps + 1, "{kind:?}: one undo step");
        assert!(eng.history(id).unwrap().last().unwrap().label.starts_with("Move a point of a measurement on p. 1"));

        // The same annotation: xref, author and subject stay; the points, label and rect are the new ones.
        let all = list_all(&eng, id).unwrap();
        let m = all.iter().find(|m| m.xref == xref).expect("same xref");
        assert_eq!((m.points.clone(), m.contents.as_str(), m.quantity.text.as_str()), (after.clone(), v1, v1), "{kind:?}");
        assert_eq!((m.author.as_str(), m.scale), ("Alex Johnson", Some(FT10)), "{kind:?}");
        with_annot(&eng, id, xref, |o| {
            // User space: y is 792 - y on this page.
            let key = if kind == Kind::Distance { "L" } else { "Vertices" };
            let user = nums(o, key);
            let want: Vec<f64> = after.iter().flat_map(|p| [f64::from(p[0]), 792.0 - f64::from(p[1])]).collect();
            assert!(user.iter().zip(&want).all(|(a, b)| (a - b).abs() < 1e-3), "{kind:?}: {user:?} vs {want:?}");
            let rect = nums(o, "Rect");
            for xy in want.chunks(2) {
                assert!(xy[0] >= rect[0] - 1e-3 && xy[0] <= rect[2] + 1e-3 && xy[1] >= rect[1] - 1e-3 && xy[1] <= rect[3] + 1e-3, "{kind:?}: rect {rect:?} holds {xy:?}");
            }
            assert_eq!(o.get_dict("Subj").unwrap().unwrap().as_string().unwrap(), kind.subject());
            assert_eq!(o.get_dict("IT").unwrap().unwrap().as_name().unwrap(), kind.intent().as_bytes());
            assert!(o.get_dict("AP").unwrap().is_some());
        });

        // Undo brings the old geometry back whole; redo the new one.
        eng.undo(id).unwrap();
        let back = list_all(&eng, id).unwrap();
        let m = back.iter().find(|m| m.xref == xref).unwrap();
        assert_eq!((m.points.clone(), m.quantity.text.as_str()), (before.clone(), v0), "{kind:?}: undone");
        with_annot(&eng, id, xref, |o| {
            let key = if kind == Kind::Distance { "L" } else { "Vertices" };
            assert_eq!((nums(o, "Rect"), nums(o, key)), (user_before.0.clone(), user_before.1.clone()), "{kind:?}: undone");
        });
        eng.redo(id).unwrap();
        assert_eq!(list_all(&eng, id).unwrap().iter().find(|m| m.xref == xref).unwrap().quantity.text, v1, "{kind:?}: redone");
    }

    // Saved and read again, the three are where they were moved to.
    eng.save(id, None).unwrap();
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    let texts: Vec<String> = list_all(&eng2, id2).unwrap().into_iter().map(|m| m.quantity.text).collect();
    assert_eq!(texts, ["20.00 ft", "30.00 ft", "150.00 sq ft"]);
}

#[test]
fn a_moved_measurement_keeps_the_scale_it_was_made_with_and_draws() {
    // The page says 1 in = 10 ft, the measurement was made at 1 in = 1 m: moving it does not re-scale it.
    let (eng, id, _) = open("move-scale", &Spec::default());
    scale_set(&eng, id, 0, &FT10, EditSource::User).unwrap();
    let m1 = Scale { paper: 1.0, paper_unit: Unit::In, real: 1.0, real_unit: Unit::M };
    let (xref, _) = add(&eng, id, Kind::Distance, &[[100.0, 100.0], [172.0, 100.0]], Some(&m1));
    let (_, q) = move_points(&eng, id, 0, xref, &[[100.0, 100.0], [244.0, 100.0]], EditSource::User).unwrap();
    assert_eq!(q.text, "2.00 m");
    // The appearance is the new one: the label is in the stream, and the page draws.
    let ap = with_annot(&eng, id, xref, |o| {
        let n = o.get_dict("AP").unwrap().unwrap().get_dict("N").unwrap().unwrap();
        String::from_utf8_lossy(&n.read_stream().unwrap()).into_owned()
    });
    assert!(ap.contains("(2.00 m)"), "{ap}");
}

#[test]
fn an_acrobat_measurement_keeps_its_own_measure_when_a_point_moves() {
    let (eng, id, _) = open("move-acrobat", &acrobat());
    let line = list_all(&eng, id).unwrap().into_iter().find(|m| m.kind == Kind::Distance).unwrap();
    // User (0,0)-(0,72) is 72 units at 0.1388889 ft: 10.00 ft.
    let (_, q) = move_points(&eng, id, 0, line.xref, &[[0.0, 792.0], [0.0, 720.0]], EditSource::User).unwrap();
    assert_eq!(q.text, "10.00 ft");
    with_annot(&eng, id, line.xref, |o| {
        let m = o.get_dict("Measure").unwrap().unwrap();
        assert_eq!(m.get_dict("R").unwrap().unwrap().as_string().unwrap(), "1 in = 10 ft");
        assert!(m.get_dict("A").unwrap().is_some() && m.get_dict("D").unwrap().is_some());
    });
    assert_eq!(list_all(&eng, id).unwrap().iter().find(|m| m.xref == line.xref).unwrap().scale.map(|s| s.label()).as_deref(), Some("1 in = 10 ft"));
}

#[test]
fn a_move_that_cannot_be_made_is_refused_and_leaves_no_step() {
    let (eng, id, _) = open("move-bad", &Spec::default());
    let (xref, _) = add(&eng, id, Kind::Distance, &[[100.0, 100.0], [200.0, 100.0]], Some(&FT10));
    let steps = history_len(&eng, id);
    let good = [[100.0, 100.0], [150.0, 100.0]];
    let bad: [(&str, i32, Vec<[f32; 2]>); 5] = [
        ("a gone xref", 99_999, good.to_vec()),
        ("one point", xref, vec![[1.0, 1.0]]),
        ("three points on a distance", xref, vec![[1.0, 1.0], [2.0, 2.0], [3.0, 3.0]]),
        ("not a number", xref, vec![[1.0, f32::NAN], [2.0, 2.0]]),
        ("too far away", xref, vec![[1.0, 1.0], [2e7, 1.0]]),
    ];
    for (what, x, pts) in bad {
        assert!(move_points(&eng, id, 0, x, &pts, EditSource::User).is_err(), "{what}");
        assert!(value_in(&eng, id, 0, Kind::Distance, &pts, x).is_err(), "{what}: the live value");
    }
    assert!(move_points(&eng, id, 1, xref, &good, EditSource::User).is_err(), "no such page");
    assert_eq!(history_len(&eng, id), steps);
    assert_eq!(list_all(&eng, id).unwrap()[0].points, vec![[100.0, 100.0], [200.0, 100.0]]);
}

#[test]
fn moving_a_point_in_a_signed_document_is_saved_after_the_signature() {
    // /P 3 certification allows annotations; the signature field has a value, so the file counts as signed.
    let sig = "<< /FT /Sig /T (Signature1) /V << /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> >> >>";
    let line = "<< /Type /Annot /Subtype /Line /Rect [0 0 200 200] /L [0 0 72 0] /IT /LineDimension /F 4 /Measure \
                << /Type /Measure /Subtype /RL /R (1 in = 10 ft) /X [<< /Type /NumberFormat /U (ft) /C 0.1388889 /D 100 /FD true >>] >> >>";
    let spec = Spec {
        page: "/Annots [6 0 R]".into(),
        cat: "/AcroForm << /SigFlags 3 /Fields [5 0 R] >> /Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P 3 >> >> ] >> >>".into(),
        extra: vec![(5, sig.into()), (6, line.into())],
        ..Default::default()
    };
    let (eng, id, path) = open("move-signed", &spec);
    assert!(eng.read_custom(id, |d| Ok(crate::outline::signed(d))).unwrap());
    let original = std::fs::read(&path).unwrap();
    let xref = list_all(&eng, id).unwrap()[0].xref;
    move_points(&eng, id, 0, xref, &[[0.0, 792.0], [144.0, 792.0]], EditSource::User).unwrap();
    eng.save(id, None).unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.len() > original.len() && saved.starts_with(&original), "an update after the signed bytes, not a rewrite");
    let eng2 = Engine::new(2, 64 << 20);
    let id2 = eng2.open(&path).unwrap().doc;
    assert_eq!(list_all(&eng2, id2).unwrap()[0].quantity.text, "20.00 ft");
}

// ---- certified documents -----------------------------------------------------------------------------------

/// `acrobat()` (a page with measurements on it) certified with DocMDP `/P` `p`.
fn certified(p: u8) -> Spec {
    let mut s = acrobat();
    s.cat = format!("/Perms << /DocMDP << /Type /Sig /Reference [ << /TransformMethod /DocMDP /TransformParams << /P {p} >> >> ] >> >>");
    s
}

/// Every write of measure.rs on a document that must not change: refused, nothing listed, no undo step.
fn refused(eng: &Engine, id: DocId, what: &str) {
    let steps = eng.history(id).unwrap().len();
    let before = list_all(eng, id).unwrap().len();
    let (a, b) = ([10.0, 10.0], [50.0, 10.0]);
    let scale = Scale { paper: 1.0, paper_unit: Unit::In, real: 2.0, real_unit: Unit::M };
    let xref = list_all(eng, id).unwrap()[0].xref;
    let rs: [(&str, Result<()>); 4] = [
        ("add", add_one(eng, id, 0, Kind::Distance, &[a, b], None, "", EditSource::User).map(|_| ())),
        ("scale", scale_set(eng, id, 0, &scale, EditSource::User).map(|_| ())),
        ("rescale", rescale(eng, id, 0, xref, &scale, EditSource::User).map(|_| ())),
        ("move", move_points(eng, id, 0, xref, &[a, b], EditSource::User).map(|_| ())),
    ];
    for (op, r) in rs {
        match r {
            Err(EngineError::Invalid(m)) => assert!(m.contains("certified"), "{what} {op}: {m}"),
            other => panic!("{what} {op}: expected a refusal, got {other:?}"),
        }
    }
    assert_eq!(eng.history(id).unwrap().len(), steps, "{what}: no undo step");
    assert_eq!(list_all(eng, id).unwrap().len(), before, "{what}: nothing added or replaced");
    assert!(lock_of(eng, id).unwrap().is_some_and(|m| m.contains("certified")), "{what}: the UI is told why");
}

#[test]
fn a_certified_document_that_forbids_annotations_is_not_measured_on() {
    for p in [1, 2] {
        let (eng, id, _) = open(&format!("certified-{p}"), &certified(p));
        refused(&eng, id, &format!("P {p}"));
    }
}

#[test]
fn a_certification_that_allows_annotations_allows_measuring() {
    let (eng, id, _) = open("certified-3", &certified(3));
    assert_eq!(lock_of(&eng, id).unwrap(), None);
    let steps = eng.history(id).unwrap().len();
    let all = list_all(&eng, id).unwrap();
    add(&eng, id, Kind::Distance, &[[10.0, 10.0], [50.0, 10.0]], Some(&FT10));
    rescale(&eng, id, 0, all[0].xref, &FT10, EditSource::User).unwrap();
    // The re-measured annotation has a new xref.
    let again = list_all(&eng, id).unwrap();
    move_points(&eng, id, 0, again.last().unwrap().xref, &[[10.0, 10.0], [60.0, 10.0]], EditSource::User).unwrap();
    assert_eq!(eng.history(id).unwrap().len(), steps + 3);
    // A page scale is a /VP entry of the page dictionary: not an annotation, so no /P allows it.
    let e = scale_set(&eng, id, 0, &FT10, EditSource::User).unwrap_err().to_string();
    assert!(e.contains("certified"), "{e}");
    assert_eq!(eng.history(id).unwrap().len(), steps + 3);
}

#[test]
fn a_certification_that_cannot_be_read_is_refused() {
    // More fields than the check will walk: it fails closed, as a certified document.
    let mut s = acrobat();
    s.cat = format!("/AcroForm << /Fields [{}] >>", "0 ".repeat(20_001));
    let (eng, id, _) = open("certified-unreadable", &s);
    refused(&eng, id, "unreadable");
}

#[test]
fn an_uncertified_document_is_not_locked() {
    let (eng, id, _) = open("uncertified", &acrobat());
    assert_eq!(lock_of(&eng, id).unwrap(), None);
}
