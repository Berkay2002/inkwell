//! `/Measure` dictionaries (ISO 32000-1 section 12.9, subtype `/RL`) and the page `/VP` viewports that carry them.
//!
//! What a measure means here, in one place (Acrobat writes the same shape, from memory of its files; the PC should
//! open one of ours in Acrobat once to confirm):
//! - `/X` carries the scale: its first element's `/C` is the real distance (in `/U`) for one user space unit. `/Y` is
//!   absent when both axes agree; when present, its `/C` scales the y axis.
//! - `/D` (distance) and `/A` (area) say how the result reads. Their first element's `/C` multiplies the result of the
//!   scaled geometry again (Acrobat writes 1). Without `/D` or `/A` the `/X` format is used ("sq " + unit for area).
//! - A later element's `/C` is how many of it make one of the previous one (`/C 12` after "ft" is inches).
//! - `/R` is the text of the ratio, "1 in = 10 ft". `/O` and `/CYX` are kept when read but not used.
//! - `/UserUnit` is folded into `/X`'s `/C`: user space units are `user_unit` points, so a page with `/UserUnit 2`
//!   writes twice the `/C` of one without, and reading divides it out again.

use super::calc::{Kind, P2, Quantity, Scale, Unit, path_length, polygon_area};
use super::numfmt::{NumberFormat, Style, chain_text};
use crate::engine::EngineError;
use mupdf::pdf::{PdfDocument, PdfObject};

type Result<T> = std::result::Result<T, EngineError>;

#[derive(Clone, Debug, PartialEq)]
pub struct MeasureDict {
    pub r: String,
    pub x: Vec<NumberFormat>,
    pub y: Vec<NumberFormat>,
    pub d: Vec<NumberFormat>,
    pub a: Vec<NumberFormat>,
    pub o: Option<[f64; 2]>,
    pub cyx: Option<f64>,
}

impl MeasureDict {
    /// The measure for `scale` on a page whose user space unit is `user_unit` points.
    pub fn from_scale(scale: &Scale, user_unit: f64) -> MeasureDict {
        let real_per_user = scale.real_per_pt() * user_unit;
        MeasureDict {
            r: scale.label(),
            x: vec![NumberFormat::new(scale.real_unit.label(), real_per_user)],
            y: Vec::new(),
            d: vec![NumberFormat::new(scale.real_unit.label(), 1.0)],
            a: vec![NumberFormat::new(scale.real_unit.area_label(), 1.0)],
            o: Some([0.0, 0.0]),
            cyx: None,
        }
    }

    /// The scale as the user would state it: `/R` when it parses and agrees with `/C`, else derived from `/C`
    /// ("1 in = n unit"). `None` when the distance unit is not one of ours (the measure still reads values).
    pub fn scale(&self, user_unit: f64) -> Option<Scale> {
        let first = self.x.first()?;
        let dist = self.d.first();
        let unit = Unit::parse(&dist.unwrap_or(first).unit)?;
        let real_per_pt = first.c * dist.map_or(1.0, |d| d.c) / user_unit;
        if let Some(s) = Scale::parse(&self.r)
            && s.real_unit == unit
            && ((s.real_per_pt() - real_per_pt) / real_per_pt).abs() < 1e-4
        {
            return Some(s);
        }
        Some(Scale { paper: 1.0, paper_unit: Unit::In, real: real_per_pt * 72.0, real_unit: unit })
    }

    /// The value of a measurement whose points are in user space.
    pub fn quantity(&self, kind: Kind, user_pts: &[P2]) -> Quantity {
        let cx = self.x.first().map_or(1.0, |f| f.c);
        // `/Y` replaces `/X` on the y axis; `/CYX` converts its units to the x axis units.
        let cy = self.y.first().map_or(cx, |f| f.c * self.cyx.unwrap_or(1.0));
        let scaled: Vec<P2> = user_pts.iter().map(|p| [p[0] * cx, p[1] * cy]).collect();
        let raw = if kind == Kind::Area { polygon_area(&scaled) } else { path_length(&scaled) };
        let explicit = if kind == Kind::Area { &self.a } else { &self.d };
        let (formats, value) = match explicit.first() {
            Some(first) => (explicit.clone(), raw * first.c),
            None => {
                let mut f = self.x.clone();
                if let Some(first) = f.first_mut() {
                    first.c = 1.0;
                    if kind == Kind::Area {
                        first.unit = format!("sq {}", first.unit);
                    }
                }
                (f, raw)
            }
        };
        Quantity { value, unit: formats.first().map(|f| f.unit.clone()).unwrap_or_default(), text: chain_text(value, &formats) }
    }

    /// Adds the dictionary to the document and returns the reference to put in `/Measure`.
    pub fn write(&self, doc: &mut PdfDocument) -> Result<PdfObject> {
        let mut m = doc.new_dict()?;
        m.dict_put("Type", PdfObject::new_name("Measure")?)?;
        m.dict_put("Subtype", PdfObject::new_name("RL")?)?;
        m.dict_put("R", PdfObject::new_string(&self.r)?)?;
        for (key, list) in [("X", &self.x), ("Y", &self.y), ("D", &self.d), ("A", &self.a)] {
            if list.is_empty() {
                continue;
            }
            let mut arr = doc.new_array()?;
            for f in list {
                arr.array_push(format_obj(doc, f)?)?;
            }
            m.dict_put(key, arr)?;
        }
        if let Some(o) = self.o {
            let mut arr = doc.new_array()?;
            for v in o {
                arr.array_push(PdfObject::new_real(v as f32)?)?;
            }
            m.dict_put("O", arr)?;
        }
        if let Some(c) = self.cyx {
            m.dict_put("CYX", PdfObject::new_real(c as f32)?)?;
        }
        Ok(doc.add_object(&m)?)
    }

    /// Reads a `/Measure` (a direct or indirect dictionary). `None` when it is not a rectilinear measure with a usable
    /// `/X`.
    pub fn read(o: &PdfObject) -> Result<Option<MeasureDict>> {
        if !o.is_dict()? {
            return Ok(None);
        }
        if let Some(st) = o.get_dict("Subtype")?
            && st.is_name()?
            && st.as_name()? != b"RL"
        {
            return Ok(None);
        }
        let list = |key: &str| -> Result<Vec<NumberFormat>> {
            let Some(arr) = o.get_dict(key)? else { return Ok(Vec::new()) };
            if !arr.is_array()? {
                return Ok(Vec::new());
            }
            let mut out = Vec::new();
            for i in 0..arr.len()? {
                if let Some(f) = arr.get_array(i as i32)?
                    && let Some(f) = read_format(&f)?
                {
                    out.push(f);
                }
            }
            Ok(out)
        };
        let x = list("X")?;
        if x.is_empty() {
            return Ok(None);
        }
        let r = text(o, "R")?.unwrap_or_default();
        let origin = match o.get_dict("O")? {
            Some(a) if a.is_array()? && a.len()? >= 2 => match (num_at(&a, 0)?, num_at(&a, 1)?) {
                (Some(p), Some(q)) => Some([p, q]),
                _ => None,
            },
            _ => None,
        };
        Ok(Some(MeasureDict { r, x, y: list("Y")?, d: list("D")?, a: list("A")?, o: origin, cyx: num(o, "CYX")? }))
    }
}

fn num_at(arr: &PdfObject, i: i32) -> Result<Option<f64>> {
    match arr.get_array(i)? {
        Some(v) if v.is_number()? => Ok(Some(f64::from(v.as_float()?))),
        _ => Ok(None),
    }
}

fn num(o: &PdfObject, key: &str) -> Result<Option<f64>> {
    match o.get_dict(key)? {
        Some(v) if v.is_number()? => Ok(Some(f64::from(v.as_float()?))),
        _ => Ok(None),
    }
}

fn text(o: &PdfObject, key: &str) -> Result<Option<String>> {
    match o.get_dict(key)? {
        Some(v) if v.is_string()? => Ok(Some(v.as_string()?)),
        _ => Ok(None),
    }
}

fn flag(o: &PdfObject, key: &str) -> Result<Option<bool>> {
    match o.get_dict(key)? {
        Some(v) if v.is_bool()? => Ok(Some(v.as_bool()?)),
        _ => Ok(None),
    }
}

fn read_format(o: &PdfObject) -> Result<Option<NumberFormat>> {
    if !o.is_dict()? {
        return Ok(None);
    }
    let Some(c) = num(o, "C")?.filter(|c| c.is_finite() && *c > 0.0) else { return Ok(None) };
    let mut f = NumberFormat::new(&text(o, "U")?.unwrap_or_default(), c);
    f.fixed = flag(o, "FD")?.unwrap_or(false);
    if let Some(n) = o.get_dict("F")?
        && n.is_name()?
    {
        f.style = Style::from_name(&String::from_utf8_lossy(&n.as_name()?));
    }
    if let Some(d) = num(o, "D")?.filter(|d| d.is_finite()) {
        f.d = d.clamp(1.0, 1e9);
    }
    // The spec's defaults: "," and "." separators, a single space on each side of the label (a trailing one is not shown).
    f.thousands = text(o, "RT")?.unwrap_or_else(|| ",".into());
    f.decimal = text(o, "RD")?.unwrap_or_else(|| ".".into());
    f.ps = text(o, "PS")?.unwrap_or_else(|| " ".into());
    f.ss = text(o, "SS")?.unwrap_or_else(|| " ".into());
    if let Some(n) = o.get_dict("O")?
        && n.is_name()?
    {
        f.prefix = n.as_name()? == b"P";
    }
    Ok(Some(f))
}

fn format_obj(doc: &PdfDocument, f: &NumberFormat) -> Result<PdfObject> {
    let mut d = doc.new_dict()?;
    d.dict_put("Type", PdfObject::new_name("NumberFormat")?)?;
    d.dict_put("U", PdfObject::new_string(&f.unit)?)?;
    d.dict_put("C", PdfObject::new_real(f.c as f32)?)?;
    d.dict_put("F", PdfObject::new_name(f.style.name())?)?;
    d.dict_put("D", PdfObject::new_int(f.d.round().clamp(1.0, 1e9) as i32)?)?;
    d.dict_put("FD", PdfObject::new_bool(f.fixed))?;
    d.dict_put("RT", PdfObject::new_string(&f.thousands)?)?;
    d.dict_put("RD", PdfObject::new_string(&f.decimal)?)?;
    d.dict_put("PS", PdfObject::new_string(&f.ps)?)?;
    d.dict_put("SS", PdfObject::new_string(&f.ss)?)?;
    if f.prefix {
        d.dict_put("O", PdfObject::new_name("P")?)?;
    }
    Ok(d)
}

// ---- page viewports -------------------------------------------------------------------------------------------

fn box_of(arr: &PdfObject) -> Result<Option<[f64; 4]>> {
    if !arr.is_array()? || arr.len()? < 4 {
        return Ok(None);
    }
    let mut v = [0.0; 4];
    for (i, c) in v.iter_mut().enumerate() {
        *c = num_at(arr, i as i32)?.unwrap_or(0.0);
    }
    Ok(Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])]))
}

/// `[x0 y0 x1 y1]` of a page's crop box (else media box) in user space.
pub fn page_box(page: &PdfObject) -> Result<[f64; 4]> {
    for key in ["CropBox", "MediaBox"] {
        if let Some(b) = page.get_dict_inheritable(key)?
            && let Some(b) = box_of(&b)?
        {
            return Ok(b);
        }
    }
    Ok([0.0, 0.0, 612.0, 792.0])
}

struct Viewport {
    bbox: [f64; 4],
    measure: MeasureDict,
}

fn viewports(page: &PdfObject) -> Result<Vec<Viewport>> {
    let Some(vp) = page.get_dict("VP")? else { return Ok(Vec::new()) };
    if !vp.is_array()? {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for i in 0..vp.len()? {
        let Some(v) = vp.get_array(i as i32)? else { continue };
        if !v.is_dict()? {
            continue;
        }
        let (Some(b), Some(m)) = (v.get_dict("BBox")?, v.get_dict("Measure")?) else { continue };
        if let (Some(bbox), Some(measure)) = (box_of(&b)?, MeasureDict::read(&m)?) {
            out.push(Viewport { bbox, measure });
        }
    }
    Ok(out)
}

/// Whether a viewport box covers the whole page (it contains the crop box; a box of the media box or larger counts).
fn page_wide(bbox: [f64; 4], page: [f64; 4]) -> bool {
    bbox[0] <= page[0] + 0.5 && bbox[1] <= page[1] + 0.5 && bbox[2] >= page[2] - 0.5 && bbox[3] >= page[3] - 0.5
}

/// The measure in force on the page: at the user space point `at` the last viewport that contains it (the later one is
/// on top); without a point the page's scale: the last viewport that covers the whole page, else the widest one. `None`
/// when the page has no usable viewport there.
pub fn viewport_measure(page: &PdfObject, at: Option<P2>) -> Result<Option<MeasureDict>> {
    let all = viewports(page)?;
    let area = |v: &Viewport| (v.bbox[2] - v.bbox[0]) * (v.bbox[3] - v.bbox[1]);
    let hit = match at {
        Some(p) => all.into_iter().rev().find(|v| p[0] >= v.bbox[0] && p[0] <= v.bbox[2] && p[1] >= v.bbox[1] && p[1] <= v.bbox[3]),
        None => {
            let b = page_box(page)?;
            match all.iter().rposition(|v| page_wide(v.bbox, b)) {
                Some(i) => all.into_iter().nth(i),
                None => all.into_iter().max_by(|a, b| area(a).total_cmp(&area(b))),
            }
        }
    };
    Ok(hit.map(|v| v.measure))
}

/// Gives the whole page `m` as its scale: the last viewport that covers the whole page (see [`page_wide`]) gets the new
/// measure, or a new one is added *below* the others (index 0: later viewports are on top, so a detail drawn at another
/// scale stays on top of it). `/VP` is always written as a fresh array and a changed viewport as a fresh dictionary: the
/// old ones may be shared with other pages and are never edited in place.
pub fn set_page_measure(doc: &mut PdfDocument, page: &mut PdfObject, m: &MeasureDict) -> Result<()> {
    let bbox = page_box(page)?;
    let measure = m.write(doc)?;
    let mut items: Vec<(PdfObject, bool)> = Vec::new();
    if let Some(vp) = page.get_dict("VP")?
        && vp.is_array()?
    {
        for i in 0..vp.len()? {
            let Some(v) = vp.get_array(i as i32)? else { continue };
            let wide = v.is_dict()?
                && match v.get_dict("BBox")?.map(|b| box_of(&b)).transpose()?.flatten() {
                    Some(c) => page_wide(c, bbox),
                    None => false,
                };
            items.push((v, wide));
        }
    }
    let target = items.iter().rposition(|(_, wide)| *wide);
    let mut measure = Some(measure);
    let mut kept = Vec::new();
    for (i, (v, _)) in items.into_iter().enumerate() {
        match measure.take_if(|_| target == Some(i)) {
            Some(measure) => {
                let mut copy = doc.new_dict()?;
                for k in 0..v.dict_len()? {
                    if let (Some(key), Some(val)) = (v.get_dict_key(k as i32)?, v.get_dict_val(k as i32)?)
                        && key.as_name()? != b"Measure"
                    {
                        copy.dict_put(key, val)?;
                    }
                }
                copy.dict_put("Measure", measure)?;
                kept.push(doc.add_object(&copy)?);
            }
            None => kept.push(v),
        }
    }
    let mut arr = doc.new_array()?;
    if let Some(measure) = measure {
        let mut v = doc.new_dict()?;
        v.dict_put("Type", PdfObject::new_name("Viewport")?)?;
        let mut b = doc.new_array()?;
        for c in bbox {
            b.array_push(PdfObject::new_real(c as f32)?)?;
        }
        v.dict_put("BBox", b)?;
        v.dict_put("Name", PdfObject::new_string("Inkwell scale")?)?;
        v.dict_put("Measure", measure)?;
        arr.array_push(doc.add_object(&v)?)?;
    }
    for v in kept {
        arr.array_push(v)?;
    }
    page.dict_put("VP", arr)?;
    Ok(())
}
