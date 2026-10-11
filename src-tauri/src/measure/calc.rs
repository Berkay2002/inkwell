//! Units, scales and geometry. Pure: no PDF objects here.
//!
//! Points the UI and the commands pass are in *page space* (points, y down, as the viewer shows the page, rotation
//! applied; the same space as `annot`). [`PageGeom`] maps them to PDF user space, where a measurement is made.

use serde::{Deserialize, Serialize};

/// A unit of length. Paper units (`Scale::paper_unit`) and real units (`Scale::real_unit`) use the same set.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Pt,
    In,
    Mm,
    Cm,
    M,
    Ft,
}

impl Unit {
    pub const ALL: [Unit; 6] = [Unit::Pt, Unit::In, Unit::Mm, Unit::Cm, Unit::M, Unit::Ft];

    /// How many PDF points (1/72 in) one of this unit is.
    pub fn pt_per_unit(self) -> f64 {
        match self {
            Unit::Pt => 1.0,
            Unit::In => 72.0,
            Unit::Mm => 72.0 / 25.4,
            Unit::Cm => 72.0 / 2.54,
            Unit::M => 72.0 / 0.0254,
            Unit::Ft => 864.0,
        }
    }

    /// The label written in a number format (`/U`).
    pub fn label(self) -> &'static str {
        match self {
            Unit::Pt => "pt",
            Unit::In => "in",
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::M => "m",
            Unit::Ft => "ft",
        }
    }

    /// The label of an area in this unit (`/U` of an `/A` number format).
    pub fn area_label(self) -> &'static str {
        match self {
            Unit::Pt => "sq pt",
            Unit::In => "sq in",
            Unit::Mm => "sq mm",
            Unit::Cm => "sq cm",
            Unit::M => "sq m",
            Unit::Ft => "sq ft",
        }
    }

    /// Reads a label the way Acrobat and other writers spell it ("in", "inch", "ft", "feet", "mm", "metre", ...).
    pub fn parse(s: &str) -> Option<Unit> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "pt" | "pts" | "point" | "points" => Unit::Pt,
            "in" | "inch" | "inches" | "\"" => Unit::In,
            "mm" | "millimeter" | "millimeters" | "millimetre" | "millimetres" => Unit::Mm,
            "cm" | "centimeter" | "centimeters" | "centimetre" | "centimetres" => Unit::Cm,
            "m" | "meter" | "meters" | "metre" | "metres" => Unit::M,
            "ft" | "foot" | "feet" | "'" => Unit::Ft,
            _ => return None,
        })
    }
}

/// A drawing scale such as "1 in = 10 ft": `paper` of `paper_unit` on the page stand for `real` of `real_unit`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scale {
    pub paper: f64,
    pub paper_unit: Unit,
    pub real: f64,
    pub real_unit: Unit,
}

impl Default for Scale {
    /// What a page without a scale measures in (Acrobat's default too): 1 in = 1 in.
    fn default() -> Self {
        Scale::unscaled(Unit::In)
    }
}

impl Scale {
    pub fn unscaled(unit: Unit) -> Self {
        Scale { paper: 1.0, paper_unit: unit, real: 1.0, real_unit: unit }
    }

    pub fn validate(&self) -> Result<(), String> {
        for (what, v) in [("paper", self.paper), ("real", self.real)] {
            if !v.is_finite() || v <= 0.0 {
                return Err(format!("the {what} side of a scale must be a number above zero"));
            }
        }
        Ok(())
    }

    /// Units of `real_unit` for one point on the page.
    pub fn real_per_pt(&self) -> f64 {
        self.real / (self.paper * self.paper_unit.pt_per_unit())
    }

    /// The text of the `/R` entry: "1 in = 10 ft".
    pub fn label(&self) -> String {
        format!("{} {} = {} {}", trim_num(self.paper), self.paper_unit.label(), trim_num(self.real), self.real_unit.label())
    }

    /// Reads "1 in = 10 ft" (also "1in=10ft", "0.25 in = 1 ft"); `None` for anything else, such as "1:100".
    pub fn parse(s: &str) -> Option<Scale> {
        let (l, r) = s.split_once('=')?;
        let side = |t: &str| -> Option<(f64, Unit)> {
            let t = t.trim();
            let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).filter(|&i| i > 0)?;
            let (n, u) = t.split_at(split);
            Some((n.parse().ok()?, Unit::parse(u)?))
        };
        let ((paper, paper_unit), (real, real_unit)) = (side(l)?, side(r)?);
        let sc = Scale { paper, paper_unit, real, real_unit };
        sc.validate().ok()?;
        Some(sc)
    }
}

/// `10`, `0.25`, `2.5` (up to six decimals, no trailing zeros).
pub fn trim_num(v: f64) -> String {
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// What a measurement is. The kind decides the annotation: a line, a polyline or a polygon.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Two points: the straight distance. A `/Line` with `/IT /LineDimension`.
    Distance,
    /// Two or more points: the length of the path through them. A `/PolyLine` with `/IT /PolyLineDimension`.
    Perimeter,
    /// Three or more points: the area of the polygon they make. A `/Polygon` with `/IT /PolygonDimension`.
    Area,
}

impl Kind {
    pub fn min_points(self) -> usize {
        match self {
            Kind::Distance | Kind::Perimeter => 2,
            Kind::Area => 3,
        }
    }

    /// The `/IT` name.
    pub fn intent(self) -> &'static str {
        match self {
            Kind::Distance => "LineDimension",
            Kind::Perimeter => "PolyLineDimension",
            Kind::Area => "PolygonDimension",
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Kind::Distance => "distance",
            Kind::Perimeter => "perimeter",
            Kind::Area => "area",
        }
    }

    /// `/Subj` text, as Acrobat names its tools.
    pub fn subject(self) -> &'static str {
        match self {
            Kind::Distance => "Distance Measurement",
            Kind::Perimeter => "Perimeter Measurement",
            Kind::Area => "Area Measurement",
        }
    }
}

/// A measured value and how it reads.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Quantity {
    /// In the first unit of the number format array (`unit`).
    pub value: f64,
    /// "ft", "sq ft", ...
    pub unit: String,
    /// The label as drawn and stored in `/Contents`: "25.00 ft".
    pub text: String,
}

pub type P2 = [f64; 2];

pub fn dist(a: P2, b: P2) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Length of the path through the points (open).
pub fn path_length(p: &[P2]) -> f64 {
    p.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Area of the polygon (shoelace formula; the polygon closes itself).
pub fn polygon_area(p: &[P2]) -> f64 {
    let n = p.len();
    let twice: f64 = (0..n).map(|i| p[i][0] * p[(i + 1) % n][1] - p[(i + 1) % n][0] * p[i][1]).sum();
    twice.abs() / 2.0
}

/// Centre of mass of the polygon; the middle of its box when it has no area.
pub fn polygon_centroid(p: &[P2]) -> P2 {
    let n = p.len();
    let (mut a2, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (q, r) = (p[i], p[(i + 1) % n]);
        let cross = q[0] * r[1] - r[0] * q[1];
        a2 += cross;
        cx += (q[0] + r[0]) * cross;
        cy += (q[1] + r[1]) * cross;
    }
    if a2.abs() < 1e-9 {
        let (x0, y0, x1, y1) = bounds(p);
        return [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    }
    [cx / (3.0 * a2), cy / (3.0 * a2)]
}

pub fn bounds(p: &[P2]) -> (f64, f64, f64, f64) {
    let f = |g: fn(f64, f64) -> f64, i: usize, s: f64| p.iter().map(|q| q[i]).fold(s, g);
    (f(f64::min, 0, f64::INFINITY), f(f64::min, 1, f64::INFINITY), f(f64::max, 0, f64::NEG_INFINITY), f(f64::max, 1, f64::NEG_INFINITY))
}

/// The page's two spaces: PDF user space, where `/Vertices`, `/L` and `/Measure` live, and page space, where the UI
/// and MuPDF's annotation calls work. `m` maps user space to page space (it holds the rotation, the crop box origin
/// and the `/UserUnit` scale, MuPDF's `pdf_page_transform`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeom {
    m: [f64; 6],
    inv: [f64; 6],
    /// Points (1/72 in) per user space unit: `/UserUnit`, taken from the transform itself (the square root of its
    /// determinant) so it is whatever MuPDF used.
    pub user_unit: f64,
}

fn apply(m: &[f64; 6], p: P2) -> P2 {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

impl PageGeom {
    /// `None` for a transform that cannot be inverted (`/UserUnit 0`, a degenerate box) or is not finite.
    pub fn new(m: [f64; 6]) -> Option<PageGeom> {
        let det = m[0] * m[3] - m[1] * m[2];
        let user_unit = det.abs().sqrt();
        if m.iter().any(|v| !v.is_finite()) || !det.is_finite() || det.abs() < 1e-12 || !user_unit.is_finite() {
            return None;
        }
        let inv_det = 1.0 / det;
        let inv = [
            m[3] * inv_det,
            -m[1] * inv_det,
            -m[2] * inv_det,
            m[0] * inv_det,
            (m[2] * m[5] - m[3] * m[4]) * inv_det,
            (m[1] * m[4] - m[0] * m[5]) * inv_det,
        ];
        Some(PageGeom { m, inv, user_unit })
    }

    pub fn to_user(&self, p: P2) -> P2 {
        apply(&self.inv, p)
    }

    pub fn to_page(&self, p: P2) -> P2 {
        apply(&self.m, p)
    }

    /// The matrix that takes page space to user space (for an appearance stream drawn in page space).
    pub fn page_to_user(&self) -> [f64; 6] {
        self.inv
    }
}
