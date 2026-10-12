//! Measurement annotations: a `/Line` (`/IT /LineDimension`), `/PolyLine` (`/PolyLineDimension`) or `/Polygon`
//! (`/PolygonDimension`) with a `/Measure`, the value in `/Contents` and an appearance stream that draws it.
//!
//! The geometry is made through MuPDF's annotation calls (page space, which MuPDF turns into user space for
//! `/L` and `/Vertices`); the appearance is written here, because MuPDF's own would not draw the label for a
//! polyline or polygon. A later colour change through `annot_edit` makes MuPDF redraw the shape without the label.

use super::calc::{Kind, P2, PageGeom, Quantity, Scale, bounds, dist, polygon_centroid};
use super::dict::{self, MeasureDict};
use crate::annot::pdf_now;
use crate::engine::EngineError;
use mupdf::color::AnnotationColor;
use mupdf::pdf::{AnnotationFlags, PdfAnnotation, PdfAnnotationType, PdfDocument, PdfObject, PdfPage};
use mupdf::{Buffer, Font, Point};
use serde::Serialize;
use std::fmt::Write as _;

type Result<T> = std::result::Result<T, EngineError>;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// Stroke and label colour of a new measurement.
const COLOR: [f32; 3] = [0.80, 0.15, 0.12];
const LABEL_SIZE: f64 = 10.0;
const LINE_WIDTH: f64 = 1.0;
/// How the annotation already looks. A measurement made here has `/C` and a border width only; one from another app
/// may also have line endings (`/LE`), a leader length (`/LL`) and an interior colour (`/IC`), and they are kept.
struct Look {
    color: [f64; 3],
    width: f64,
    /// `/LE` of a line or polyline (first and last point); `None` draws our short ticks across a distance.
    ends: Option<[String; 2]>,
    /// `/LL`: the dimension line is drawn this far from `/L`, with leader lines from its ends back to the points;
    /// perpendicular, positive clockwise on screen.
    leader: f64,
    /// `/LLE`: how far the leaders go on past the dimension line; `/LLO`: the gap left next to the points.
    leader_ext: f64,
    leader_off: f64,
    interior: Option<[f64; 3]>,
    /// The fill opacity the file asks for: `/ca` or `/CA` of the annotation, else the `/ca` (or `/CA`) of its appearance's
    /// graphics states. `None`: not said.
    fill_alpha: Option<f64>,
}

impl Look {
    fn of(o: &PdfObject) -> Look {
        let nums = |key: &str| -> Option<Vec<f64>> {
            let a = o.get_dict(key).ok()??;
            if !a.is_array().ok()? {
                return None;
            }
            (0..a.len().ok()?).map(|i| a.get_array(i as i32).ok()?.filter(|v| v.is_number().unwrap_or(false))?.as_float().ok().map(f64::from)).collect()
        };
        let number = |key: &str| -> Option<f64> {
            let v = o.get_dict(key).ok()??;
            if v.is_number().ok()? { v.as_float().ok().map(f64::from).filter(|v| v.is_finite()) } else { None }
        };
        let rgb = |v: Vec<f64>| -> Option<[f64; 3]> {
            let v: Vec<f64> = v.into_iter().map(|c| c.clamp(0.0, 1.0)).collect();
            match v.len() {
                1 => Some([v[0]; 3]),
                3 => Some([v[0], v[1], v[2]]),
                4 => Some([(1.0 - v[0]) * (1.0 - v[3]), (1.0 - v[1]) * (1.0 - v[3]), (1.0 - v[2]) * (1.0 - v[3])]),
                _ => None,
            }
        };
        let w = o
            .get_dict("BS")
            .ok()
            .flatten()
            .and_then(|b| b.get_dict("W").ok().flatten())
            .filter(|w| w.is_number().unwrap_or(false))
            .and_then(|w| w.as_float().ok().map(f64::from))
            .or_else(|| nums("Border").and_then(|b| b.get(2).copied()))
            .filter(|w| w.is_finite() && *w > 0.0)
            .map_or(LINE_WIDTH, |w| w.min(100.0));
        let ends = o.get_dict("LE").ok().flatten().filter(|a| a.is_array().unwrap_or(false)).and_then(|a| {
            let name = |i: i32| a.get_array(i).ok().flatten().filter(|n| n.is_name().unwrap_or(false)).and_then(|n| n.as_name().ok());
            let text = |n: Vec<u8>| String::from_utf8_lossy(&n).into_owned();
            Some([text(name(0)?), text(name(1)?)])
        });
        // The annotation's own opacity; failing that the first graphics state of its appearance (a form, not a state dict)
        // that has one.
        let alpha = |d: &PdfObject| -> Option<f64> {
            ["ca", "CA"].iter().find_map(|k| {
                let v = d.get_dict(*k).ok()??;
                if v.is_number().ok()? { v.as_float().ok().map(f64::from).filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)) } else { None }
            })
        };
        let from_ap = || -> Option<f64> {
            let n = o.get_dict("AP").ok()??.get_dict("N").ok()??;
            if !n.is_stream().ok()? {
                return None;
            }
            let states = n.get_dict("Resources").ok()??.get_dict("ExtGState").ok()??;
            (0..states.dict_len().ok()?.min(32)).find_map(|i| alpha(&states.get_dict_val(i as i32).ok()??))
        };
        Look {
            fill_alpha: alpha(o).or_else(from_ap),
            color: nums("C").and_then(rgb).unwrap_or(COLOR.map(f64::from)),
            width: w,
            ends,
            leader: number("LL").map_or(0.0, |v| v.clamp(-1000.0, 1000.0)),
            leader_ext: number("LLE").map_or(0.0, |v| v.clamp(0.0, 1000.0)),
            leader_off: number("LLO").map_or(0.0, |v| v.clamp(0.0, 1000.0)),
            interior: nums("IC").and_then(rgb),
        }
    }
}

/// A measurement annotation as the panel lists it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MeasureDto {
    pub page: u32,
    pub xref: i32,
    pub kind: Kind,
    /// Page space, as the viewer shows the page.
    pub points: Vec<[f32; 2]>,
    /// The label as stored.
    pub contents: String,
    pub author: String,
    /// Computed from the points and the annotation's own `/Measure`, so a file from Acrobat shows Acrobat's scale.
    pub quantity: Quantity,
    /// `None` when the measure uses a unit this app has no name for (the quantity is still right).
    pub scale: Option<Scale>,
    /// The UI's own live label (the scale, two decimals) reads exactly as this measure does; when not (a file's own
    /// `/D` or `/A`, a `/Y`), the engine values the points (`measure_value` with the xref).
    pub plain: bool,
}

/// The page's user space <-> page space transform and `/UserUnit`.
pub fn page_geom(page: &PdfPage) -> Result<PageGeom> {
    let m = page.ctm()?;
    PageGeom::new([m.a, m.b, m.c, m.d, m.e, m.f].map(f64::from)).ok_or_else(|| bad("this page has no usable size"))
}

/// Most points a measurement takes, and the farthest a coordinate may be from the origin (page space, points).
pub const MAX_POINTS: usize = 10_000;
pub const MAX_COORD: f32 = 1e7;

fn check_points(kind: Kind, pts: &[[f32; 2]]) -> Result<()> {
    if pts.len() > MAX_POINTS {
        return Err(bad(format!("a measurement takes at most {MAX_POINTS} points")));
    }
    let enough = match kind {
        Kind::Distance => pts.len() == 2,
        _ => pts.len() >= kind.min_points(),
    };
    if !enough {
        return Err(bad(match kind {
            Kind::Distance => "a distance needs exactly two points",
            Kind::Perimeter => "a perimeter needs at least two points",
            Kind::Area => "an area needs at least three points",
        }));
    }
    if pts.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return Err(bad("a point is not a number"));
    }
    if pts.iter().any(|p| p[0].abs() > MAX_COORD || p[1].abs() > MAX_COORD) {
        return Err(bad("a point is too far from the page"));
    }
    if pts.windows(2).all(|w| w[0] == w[1]) {
        return Err(bad("the points are all in the same place"));
    }
    Ok(())
}

/// The measure to use for a measurement starting at `first` (page space): the scale given, else the page's viewport
/// there, else 1 in = 1 in.
fn resolve(page: &PdfPage, geom: &PageGeom, scale: Option<&Scale>, first: [f32; 2]) -> Result<MeasureDict> {
    if let Some(s) = scale {
        s.validate().map_err(bad)?;
        return Ok(MeasureDict::from_scale(s, geom.user_unit));
    }
    let at = geom.to_user([f64::from(first[0]), f64::from(first[1])]);
    Ok(dict::viewport_measure(&page.object(), Some(at))?.unwrap_or_else(|| MeasureDict::from_scale(&Scale::default(), geom.user_unit)))
}

fn user_points(geom: &PageGeom, pts: &[[f32; 2]]) -> Vec<P2> {
    pts.iter().map(|p| geom.to_user([f64::from(p[0]), f64::from(p[1])])).collect()
}

/// The value of a measurement without making it (the readout while the user is still clicking points).
pub fn value(doc: &PdfDocument, page: u32, kind: Kind, pts: &[[f32; 2]], scale: Option<&Scale>) -> Result<Quantity> {
    check_points(kind, pts)?;
    let p = doc.load_pdf_page(page as i32)?;
    let geom = page_geom(&p)?;
    Ok(resolve(&p, &geom, scale, pts[0])?.quantity(kind, &user_points(&geom, pts)))
}

/// The annotation `xref` on the page as a measurement: its kind and its own `/Measure`.
fn find_measure(page: &PdfPage, xref: i32) -> Result<(PdfAnnotation, Kind, MeasureDict)> {
    for a in page.annotations() {
        if a.xref().ok() != Some(xref) {
            continue;
        }
        let kind = match a.r#type() {
            Ok(PdfAnnotationType::Line) => Kind::Distance,
            Ok(PdfAnnotationType::PolyLine) => Kind::Perimeter,
            Ok(PdfAnnotationType::Polygon) => Kind::Area,
            _ => break,
        };
        if let Some(m) = a.object().get_dict("Measure").ok().flatten().and_then(|m| MeasureDict::read(&m).ok().flatten()) {
            return Ok((a, kind, m));
        }
        break;
    }
    Err(bad("that measurement is no longer in the file"))
}

/// The value `pts` would measure under the `/Measure` of the annotation `xref` (a point being dragged), without making
/// anything.
pub fn value_in(doc: &PdfDocument, page: u32, kind: Kind, pts: &[[f32; 2]], xref: i32) -> Result<Quantity> {
    check_points(kind, pts)?;
    let p = doc.load_pdf_page(page as i32)?;
    let geom = page_geom(&p)?;
    let (_, _, measure) = find_measure(&p, xref)?;
    Ok(measure.quantity(kind, &user_points(&geom, pts)))
}

/// Whether the page's scale reads in the UI's own live label (see [`MeasureDict::is_plain`]); true for a page with none.
pub fn page_plain(doc: &PdfDocument, page: u32) -> Result<bool> {
    dict::viewports_plain(&doc.load_pdf_page(page as i32)?.object())
}

/// The scale of the page (its widest viewport) and whether the page has one; 1 in = 1 in when it has none.
pub fn page_scale(doc: &PdfDocument, page: u32) -> Result<(Scale, bool)> {
    let p = doc.load_pdf_page(page as i32)?;
    let uu = page_geom(&p)?.user_unit;
    Ok(match dict::viewport_measure(&p.object(), None)?.and_then(|m| m.scale(uu)) {
        Some(s) => (s, true),
        None => (Scale::default(), false),
    })
}

/// Makes `scale` the scale of the whole page (a viewport over the crop box).
pub fn set_page_scale(doc: &mut PdfDocument, page: u32, scale: &Scale) -> Result<()> {
    scale.validate().map_err(bad)?;
    let p = doc.load_pdf_page(page as i32)?;
    let uu = page_geom(&p)?.user_unit;
    let mut obj = p.object();
    dict::set_page_measure(doc, &mut obj, &MeasureDict::from_scale(scale, uu))
}

/// Creates the measurement on `page` and returns the annotation's xref and value. `pts` are in page space.
pub fn add(doc: &mut PdfDocument, page: u32, kind: Kind, pts: &[[f32; 2]], scale: Option<&Scale>, author: &str) -> Result<(i32, Quantity)> {
    check_points(kind, pts)?;
    let mut p = doc.load_pdf_page(page as i32)?;
    let geom = page_geom(&p)?;
    let measure = resolve(&p, &geom, scale, pts[0])?;
    let q = measure.quantity(kind, &user_points(&geom, pts));

    let points: Vec<Point> = pts.iter().map(|v| Point { x: v[0], y: v[1] }).collect();
    let mut a = match kind {
        Kind::Distance => p.add_line_annotation(points[0], points[1])?,
        Kind::Perimeter => p.add_polyline_annotation(points)?,
        Kind::Area => p.add_polygon_annotation(points)?,
    };
    // No /IC: the light wash of an area lives only in our appearance, so a redraw by another viewer (or by MuPDF) draws
    // the outline rather than a solid fill.
    a.set_color(AnnotationColor::Rgb { red: COLOR[0], green: COLOR[1], blue: COLOR[2] })?;
    a.set_border_width(LINE_WIDTH as f32)?;
    a.set_flags(AnnotationFlags::IS_PRINT)?;
    a.set_contents(&q.text)?;
    if !author.is_empty() {
        a.set_author(author)?;
    }
    let now = pdf_now();
    let mut o = a.object();
    o.dict_put("M", PdfObject::new_string(&now)?)?;
    o.dict_put("CreationDate", PdfObject::new_string(&now)?)?;
    o.dict_put("Subj", PdfObject::new_string(kind.subject())?)?;
    o.dict_put("IT", PdfObject::new_name(kind.intent())?)?;
    let m = measure.write(doc)?;
    o.dict_put("Measure", m)?;
    restyle(doc, &mut a, &geom, kind, pts, &q.text)?;
    Ok((a.xref()?, q))
}

/// Gives the annotation `a` our appearance for `pts` and `label`, and the `/Rect` that holds it. MuPDF builds its own
/// appearance on `update` (it would not draw the label); ours replaces it.
fn restyle(doc: &mut PdfDocument, a: &mut PdfAnnotation, geom: &PageGeom, kind: Kind, pts: &[[f32; 2]], label: &str) -> Result<()> {
    // Read before `update`, which may redraw the annotation's own form.
    let look = Look::of(&a.object());
    let objects = doc.count_objects()? as i32;
    a.update()?;
    // The stream `update` made is replaced below by ours: drop it rather than leave it in the file.
    if let Some(n) = a.object().get_dict("AP")?.and_then(|ap| ap.get_dict("N").ok().flatten())
        && n.is_indirect()?
        && n.as_indirect()? >= objects
    {
        doc.delete_object(n.as_indirect()?)?;
    }
    let ap = appearance(doc, geom, kind, pts, label, &look)?;
    let mut o = a.object();
    o.dict_put("Rect", rect_obj(doc, ap.bbox)?)?;
    let mut ap_dict = doc.new_dict()?;
    ap_dict.dict_put("N", ap.form)?;
    o.dict_put("AP", ap_dict)?;
    Ok(())
}

/// Moves the points of the measurement `xref` (page space), in place: the geometry, `/Rect`, the appearance and the
/// label in `/Contents` follow, and its own `/Measure`, author and replies stay. Returns the new value.
pub fn set_points(doc: &mut PdfDocument, page: u32, xref: i32, pts: &[[f32; 2]]) -> Result<Quantity> {
    let p = doc.load_pdf_page(page as i32)?;
    let geom = page_geom(&p)?;
    let (mut a, kind, measure) = find_measure(&p, xref)?;
    check_points(kind, pts)?;
    let q = measure.quantity(kind, &user_points(&geom, pts));
    let at = |i: usize| Point { x: pts[i][0], y: pts[i][1] };
    if kind == Kind::Distance {
        a.set_line(at(0), at(1))?;
    } else {
        a.set_vertices((0..pts.len()).map(at))?;
    }
    a.set_contents(&q.text)?;
    a.object().dict_put("M", PdfObject::new_string(&pdf_now())?)?;
    restyle(doc, &mut a, &geom, kind, pts, &q.text)?;
    Ok(q)
}

struct Appearance {
    form: PdfObject,
    /// User space.
    bbox: [f64; 4],
}

fn rect_obj(doc: &PdfDocument, b: [f64; 4]) -> Result<PdfObject> {
    let mut arr = doc.new_array()?;
    for v in b {
        arr.array_push(PdfObject::new_real(v as f32)?)?;
    }
    Ok(arr)
}

/// Draws the measurement in page space (y down, so text can be set the way it reads on screen) under a `cm` that maps
/// page space to user space; the form's box is the drawing's box in user space.
fn appearance(
    doc: &mut PdfDocument,
    geom: &PageGeom,
    kind: Kind,
    pts: &[[f32; 2]],
    label: &str,
    look: &Look,
) -> Result<Appearance> {
    let orig: Vec<P2> = pts.iter().map(|p| [f64::from(p[0]), f64::from(p[1])]).collect();
    // `/LL` on a distance: the points are what is measured, the dimension line (and its label and endings) is drawn
    // `LL` away from them, clockwise on screen when positive, and leader lines join the two.
    let l = dist(orig[0], orig[1]).max(1e-9);
    let side = if kind == Kind::Distance && look.leader != 0.0 { [-(orig[1][1] - orig[0][1]) / l, (orig[1][0] - orig[0][0]) / l] } else { [0.0, 0.0] };
    let shift = [side[0] * look.leader, side[1] * look.leader];
    let pg: Vec<P2> = orig.iter().map(|p| [p[0] + shift[0], p[1] + shift[1]]).collect();
    let w = text_width(label, LABEL_SIZE);
    // Where the label sits and which way it reads: along the line, never upside down.
    let along = |a: P2, b: P2| {
        let l = dist(a, b).max(1e-9);
        let d = [(b[0] - a[0]) / l, (b[1] - a[1]) / l];
        if d[0] < -1e-9 || (d[0].abs() <= 1e-9 && d[1] > 0.0) { [-d[0], -d[1]] } else { d }
    };
    let (dir, origin) = match kind {
        Kind::Area => {
            // Centred on the centroid, the baseline a third of the text height below it.
            let c = polygon_centroid(&pg);
            ([1.0, 0.0], [c[0] - w / 2.0, c[1] + LABEL_SIZE * 0.35])
        }
        _ => {
            let (a, b) = pg
                .windows(2)
                .map(|s| (s[0], s[1]))
                .max_by(|x, y| dist(x.0, x.1).total_cmp(&dist(y.0, y.1)))
                .unwrap_or((pg[0], pg[1]));
            let dir = along(a, b);
            let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let up = [dir[1], -dir[0]];
            let lift = look.width / 2.0 + 3.0;
            (dir, [mid[0] - dir[0] * w / 2.0 + up[0] * lift, mid[1] - dir[1] * w / 2.0 + up[1] * lift])
        }
    };
    let up = [dir[1], -dir[0]];
    let text_box = [
        origin,
        [origin[0] + dir[0] * w, origin[1] + dir[1] * w],
        [origin[0] + dir[0] * w + up[0] * LABEL_SIZE, origin[1] + dir[1] * w + up[1] * LABEL_SIZE],
        [origin[0] + up[0] * LABEL_SIZE, origin[1] + up[1] * LABEL_SIZE],
    ];

    let f = |v: f64| format!("{v:.4}");
    let [cr, cg, cb] = look.color;
    let [ir, ig, ib] = look.interior.unwrap_or(look.color);
    let lw = look.width;
    let mut s = String::new();
    let m = geom.page_to_user();
    let _ = writeln!(s, "q\n{} {} {} {} {} {} cm", f(m[0]), f(m[1]), f(m[2]), f(m[3]), f(m[4]), f(m[5]));
    let path = |s: &mut String, close: bool| {
        for (i, p) in pg.iter().enumerate() {
            let _ = writeln!(s, "{} {} {}", f(p[0]), f(p[1]), if i == 0 { "m" } else { "l" });
        }
        if close {
            s.push_str("h\n");
        }
    };
    let mut extent: Vec<P2> = pg.clone();
    extent.extend(orig.iter().copied());
    extent.extend(text_box);
    match kind {
        Kind::Distance => {
            let _ = writeln!(s, "{cr:.3} {cg:.3} {cb:.3} RG {ir:.3} {ig:.3} {ib:.3} rg {lw} w 0 J");
            path(&mut s, false);
            s.push_str("S\n");
            if look.leader != 0.0 {
                // From next to each point (`/LLO` away), across the dimension line and `/LLE` past it.
                let u = if look.leader > 0.0 { side } else { [-side[0], -side[1]] };
                for (e, o) in pg.iter().zip(&orig).take(2).map(|(e, o)| (*e, *o)) {
                    let (x0, y0) = (o[0] + u[0] * look.leader_off, o[1] + u[1] * look.leader_off);
                    let (x1, y1) = (e[0] + u[0] * look.leader_ext, e[1] + u[1] * look.leader_ext);
                    let _ = writeln!(s, "{} {} m {} {} l S", f(x0), f(y0), f(x1), f(y1));
                    extent.extend([[x0, y0], [x1, y1]]);
                }
            }
            if look.ends.is_none() {
                // Short ticks across both ends, as on a dimension line.
                let d = along(pg[0], pg[1]);
                for e in [pg[0], pg[1]] {
                    let (x0, y0, x1, y1) = (e[0] - d[1] * 5.0, e[1] + d[0] * 5.0, e[0] + d[1] * 5.0, e[1] - d[0] * 5.0);
                    let _ = writeln!(s, "{} {} m {} {} l S", f(x0), f(y0), f(x1), f(y1));
                    extent.extend([[x0, y0], [x1, y1]]);
                }
            }
        }
        Kind::Perimeter => {
            let _ = writeln!(s, "{cr:.3} {cg:.3} {cb:.3} RG {ir:.3} {ig:.3} {ib:.3} rg {lw} w 1 J 1 j");
            path(&mut s, false);
            s.push_str("S\n");
        }
        Kind::Area => {
            s.push_str("q /GS1 gs\n");
            let _ = writeln!(s, "{ir:.3} {ig:.3} {ib:.3} rg {cr:.3} {cg:.3} {cb:.3} RG {lw} w 1 j");
            path(&mut s, true);
            s.push_str("B\nQ\n");
        }
    }
    // The file's own line endings, at the first and last point, pointing away from the line.
    if let (Some(ends), true) = (&look.ends, kind != Kind::Area && pg.len() >= 2) {
        let size = (lw * 4.0).max(8.0);
        let n = pg.len();
        let _ = writeln!(s, "{cr:.3} {cg:.3} {cb:.3} RG {ir:.3} {ig:.3} {ib:.3} rg {lw} w 0 J 0 j");
        for (name, at, from) in [(&ends[0], pg[0], pg[1]), (&ends[1], pg[n - 1], pg[n - 2])] {
            line_ending(&mut s, &mut extent, name, at, from, size);
        }
    }
    // The label: a white halo under the coloured text so it reads over drawings.
    let tm = format!("{} {} {} {} {} {} Tm", f(dir[0]), f(dir[1]), f(up[0]), f(up[1]), f(origin[0]), f(origin[1]));
    let lit = literal(label);
    let _ = writeln!(s, "BT /Helv {LABEL_SIZE} Tf 1 Tr 3 w 1 1 1 RG {tm} {lit} Tj ET");
    let _ = writeln!(s, "BT /Helv {LABEL_SIZE} Tf 0 Tr {cr:.3} {cg:.3} {cb:.3} rg {tm} {lit} Tj ET");
    s.push_str("Q\n");

    // Box in user space: the page-space extent, padded for the stroke and the halo, through the transform.
    let (x0, y0, x1, y1) = bounds(&extent);
    // The ending's points are in the extent; a wide stroke's mitred tips need room beyond them.
    let pad = lw / 2.0 + 2.0 + if look.ends.is_some() { lw } else { 0.0 };
    let corners = [[x0 - pad, y0 - pad], [x1 + pad, y0 - pad], [x1 + pad, y1 + pad], [x0 - pad, y1 + pad]].map(|c| geom.to_user(c));
    let (bx0, by0, bx1, by1) = bounds(&corners);
    let bbox = [bx0, by0, bx1, by1];

    // A new form and font on every call, never an edit of the stream the annotation points at: a file may share
    // that form between annotations or with a page, whatever it says about itself.
    let mut font = doc.new_dict()?;
    font.dict_put("Type", PdfObject::new_name("Font")?)?;
    font.dict_put("Subtype", PdfObject::new_name("Type1")?)?;
    font.dict_put("BaseFont", PdfObject::new_name("Helvetica")?)?;
    font.dict_put("Encoding", PdfObject::new_name("WinAnsiEncoding")?)?;
    let font = doc.add_object(&font)?;
    let mut fonts = doc.new_dict()?;
    fonts.dict_put("Helv", font)?;
    let mut gs = doc.new_dict()?;
    gs.dict_put("Type", PdfObject::new_name("ExtGState")?)?;
    // An area of ours is a light wash. One that came with an interior colour is the file's: it keeps the opacity the
    // file asked for, and is opaque when it asked for none. Without `/IC` the file draws no fill, so an opacity it
    // names (`/CA 1` is common) is not a reason to paint one.
    let wash = if look.interior.is_some() { look.fill_alpha.unwrap_or(1.0) } else { 0.12 };
    gs.dict_put("ca", PdfObject::new_real(wash as f32)?)?;
    gs.dict_put("CA", PdfObject::new_real(1.0)?)?;
    let mut states = doc.new_dict()?;
    states.dict_put("GS1", gs)?;
    let mut res = doc.new_dict()?;
    res.dict_put("Font", fonts)?;
    res.dict_put("ExtGState", states)?;
    let buf = Buffer::from_bytes(s.as_bytes())?;
    let mut dict = doc.new_dict()?;
    dict.dict_put("Type", PdfObject::new_name("XObject")?)?;
    dict.dict_put("Subtype", PdfObject::new_name("Form")?)?;
    dict.dict_put("BBox", rect_obj(doc, bbox)?)?;
    dict.dict_put("Resources", res)?;
    let form = doc.add_stream(&buf, Some(&dict), false)?;
    Ok(Appearance { form, bbox })
}

/// Draws the `/LE` style `name` at `at`, the end of a line whose neighbouring point is `from`, `size` long (page
/// space). The points drawn go into `extent`; an unknown name draws nothing.
fn line_ending(s: &mut String, extent: &mut Vec<P2>, name: &str, at: P2, from: P2, size: f64) {
    let l = dist(at, from).max(1e-9);
    let reversed = name.starts_with('R') && name.ends_with("Arrow");
    let name = if reversed { &name[1..] } else { name };
    // `u` points away from the line, `v` across it.
    let sign = if reversed { -1.0 } else { 1.0 };
    let u = [(at[0] - from[0]) / l * sign, (at[1] - from[1]) / l * sign];
    let v = [-u[1], u[0]];
    let pt = |a: f64, b: f64| [at[0] + u[0] * a + v[0] * b, at[1] + u[1] * a + v[1] * b];
    let f = |p: P2| format!("{:.4} {:.4}", p[0], p[1]);
    let h = size / 2.0;
    let (shape, op): (Vec<P2>, &str) = match name {
        "OpenArrow" => (vec![pt(-size, h * 0.8), pt(0.0, 0.0), pt(-size, -h * 0.8)], "S"),
        "ClosedArrow" => (vec![pt(-size, h * 0.8), pt(0.0, 0.0), pt(-size, -h * 0.8)], "b"),
        "Butt" => (vec![pt(0.0, h), pt(0.0, -h)], "S"),
        "Slash" => (vec![pt(h * 0.87, h * 0.5), pt(-h * 0.87, -h * 0.5)], "S"),
        "Square" => (vec![pt(-h, h), pt(h, h), pt(h, -h), pt(-h, -h)], "b"),
        "Diamond" => (vec![pt(h, 0.0), pt(0.0, h), pt(-h, 0.0), pt(0.0, -h)], "b"),
        "Circle" => {
            // Four Bezier arcs, centred on the end.
            let k = 0.5523 * h;
            let c = [pt(h, 0.0), pt(h, k), pt(k, h), pt(0.0, h), pt(-k, h), pt(-h, k), pt(-h, 0.0), pt(-h, -k), pt(-k, -h), pt(0.0, -h), pt(k, -h), pt(h, -k)];
            let _ = write!(s, "{} m ", f(c[0]));
            for i in 0..4 {
                let (a, b, e) = (c[(3 * i + 1) % 12], c[(3 * i + 2) % 12], c[(3 * i + 3) % 12]);
                let _ = write!(s, "{} {} {} c ", f(a), f(b), f(e));
            }
            s.push_str("b\n");
            extent.extend([pt(h, h), pt(-h, -h), pt(h, -h), pt(-h, h)]);
            return;
        }
        _ => return,
    };
    for (i, p) in shape.iter().enumerate() {
        let _ = write!(s, "{} {} ", f(*p), if i == 0 { "m" } else { "l" });
    }
    let _ = writeln!(s, "{op}");
    extent.extend(shape);
}

/// The advance of `text` in Helvetica at `size`, from MuPDF's metrics for the base face.
fn text_width(text: &str, size: f64) -> f64 {
    let font = Font::new("Helvetica").ok();
    let em: f64 = text
        .chars()
        .map(|c| {
            font.as_ref()
                .and_then(|f| f.encode_character(c as i32).ok().filter(|g| *g > 0 || c == ' ').and_then(|g| f.advance_glyph(g).ok()))
                .map_or(0.55, f64::from)
        })
        .sum();
    em * size
}

/// A PDF literal string in WinAnsiEncoding (Latin-1 range; anything else becomes `?`), delimiters escaped.
fn literal(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        let b = match c {
            '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => c,
            _ => '?',
        };
        match b {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(b);
            }
            c if (c as u32) < 0x80 => out.push(c),
            c => {
                let _ = write!(out, "\\{:03o}", c as u32);
            }
        }
    }
    out.push(')');
    out
}

fn text_of(o: &PdfObject, key: &str) -> String {
    o.get_dict(key).ok().flatten().and_then(|v| v.as_string().ok()).unwrap_or_default()
}

fn dto(page: u32, a: &PdfAnnotation, kind: Kind, pts: Vec<[f32; 2]>, geom: &PageGeom, measure: &MeasureDict) -> Result<MeasureDto> {
    let o = a.object();
    let quantity = measure.quantity(kind, &user_points(geom, &pts));
    Ok(MeasureDto {
        page,
        xref: a.xref()?,
        kind,
        points: pts,
        contents: text_of(&o, "Contents"),
        author: text_of(&o, "T"),
        quantity,
        scale: measure.scale(geom.user_unit),
        plain: measure.is_plain(),
    })
}

/// Every measurement annotation (a line, polyline or polygon with a readable `/Measure`) in page order.
pub fn list(doc: &PdfDocument) -> Result<Vec<MeasureDto>> {
    let mut out = Vec::new();
    for pg in 0..doc.page_count()? as u32 {
        let page = doc.load_pdf_page(pg as i32)?;
        // A page whose transform cannot be inverted (/UserUnit 0) has nothing we can value; the rest still list.
        let Ok(geom) = page_geom(&page) else { continue };
        for a in page.annotations() {
            let kind = match a.r#type() {
                Ok(PdfAnnotationType::Line) => Kind::Distance,
                Ok(PdfAnnotationType::PolyLine) => Kind::Perimeter,
                Ok(PdfAnnotationType::Polygon) => Kind::Area,
                _ => continue,
            };
            // One malformed annotation (no readable /Measure, bad geometry) is left out, not the whole list.
            let Some(measure) = a.object().get_dict("Measure").ok().flatten().and_then(|m| MeasureDict::read(&m).ok().flatten()) else {
                continue;
            };
            let pts: Result<Vec<[f32; 2]>> = if kind == Kind::Distance {
                a.line().map(|(s, e)| vec![[s.x, s.y], [e.x, e.y]]).map_err(Into::into)
            } else {
                a.vertices().map(|v| v.iter().map(|p| [p.x, p.y]).collect()).map_err(Into::into)
            };
            if let Ok(pts) = pts
                && pts.len() >= 2
                && let Ok(d) = dto(pg, &a, kind, pts, &geom, &measure)
            {
                out.push(d);
            }
        }
    }
    Ok(out)
}
