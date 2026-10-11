//! Printer's marks: crop marks, bleed marks, registration marks and colour bars, drawn outside the
//! trim box on an enlarged media box. The trim and bleed boxes decide where: the marks start beyond
//! the bleed, so none of them prints in the bleed or on the page. The marks are in the registration
//! colour (a Separation named All: every plate, spots included), the colour bars in process colours.

use super::boxes::{BoxKind, Box4, effective, stated, union, write};
use super::{Result, bad};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine};
use crate::textedit::content as cs;
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::Buffer;
use serde::Deserialize;
use std::fmt::Write as _;

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MarkSet {
    pub crop: bool,
    pub bleed: bool,
    pub registration: bool,
    pub color_bars: bool,
}

impl Default for MarkSet {
    fn default() -> Self {
        MarkSet { crop: true, bleed: true, registration: true, color_bars: true }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MarksSpec {
    /// 0-based; every page when absent.
    pub pages: Option<Vec<u32>>,
    #[serde(default)]
    pub marks: MarkSet,
    /// Line weight in points. Default 0.25.
    pub weight: Option<f32>,
    /// Distance from the trim edge to where the crop marks start, in points. Never less than the
    /// bleed plus 2. Default: the bleed plus 6.
    pub offset: Option<f32>,
    /// Crop mark length in points. Default 18.
    pub length: Option<f32>,
}

const REG: &str = "IwReg";

fn num(v: f32) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn line(s: &mut String, x0: f32, y0: f32, x1: f32, y1: f32) {
    let _ = writeln!(s, "{} {} m {} {} l S", num(x0), num(y0), num(x1), num(y1));
}

/// How the marks lay out around one page.
struct Layout {
    trim: Box4,
    bleed: Box4,
    offset: f32,
    length: f32,
}

impl Layout {
    /// How far past the trim edge the marks reach.
    fn slug(&self) -> f32 {
        self.offset + self.length.max(14.0) + 4.0
    }

    fn content(&self, set: MarkSet, weight: f32, notes: &mut Vec<String>) -> String {
        let [tx0, ty0, tx1, ty1] = self.trim;
        let [bx0, by0, bx1, by1] = self.bleed;
        let (o, l) = (self.offset, self.length);
        let mut s = String::new();
        let _ = writeln!(s, "q\n/{REG} CS 1 SCN {} w 0 J 0 j", num(weight));
        if set.crop {
            for (x, xs) in [(tx0, -1.0), (tx1, 1.0)] {
                for (y, ys) in [(ty0, -1.0), (ty1, 1.0)] {
                    // horizontal, beside the corner, and vertical, above or below it
                    line(&mut s, x + xs * o, y, x + xs * (o + l), y);
                    line(&mut s, x, y + ys * o, x, y + ys * (o + l));
                }
            }
        }
        if set.bleed {
            let bl = l.min(6.0);
            if (bx0 < tx0 - 0.01) || (by0 < ty0 - 0.01) || (bx1 > tx1 + 0.01) || (by1 > ty1 + 0.01) {
                for (x, xs) in [(bx0, -1.0), (bx1, 1.0)] {
                    for (y, ys) in [(by0, -1.0), (by1, 1.0)] {
                        line(&mut s, x + xs * 2.0, y, x + xs * (2.0 + bl), y);
                        line(&mut s, x, y + ys * 2.0, x, y + ys * (2.0 + bl));
                    }
                }
            } else {
                notes.push("The page has no bleed (the bleed box is the trim box), so no bleed marks were drawn".into());
            }
        }
        if set.registration {
            let r = 4.5;
            let d = o + 9.0;
            let (cx, cy) = ((tx0 + tx1) / 2.0, (ty0 + ty1) / 2.0);
            for (x, y) in [(cx, ty1 + d), (cx, ty0 - d), (tx0 - d, cy), (tx1 + d, cy)] {
                // a circle (four Béziers) with a cross through it
                let k = 0.5523 * r;
                let _ = writeln!(
                    s,
                    "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c S",
                    num(x + r), num(y),
                    num(x + r), num(y + k), num(x + k), num(y + r), num(x), num(y + r),
                    num(x - k), num(y + r), num(x - r), num(y + k), num(x - r), num(y),
                    num(x - r), num(y - k), num(x - k), num(y - r), num(x), num(y - r),
                    num(x + k), num(y - r), num(x + r), num(y - k), num(x + r), num(y)
                );
                line(&mut s, x - r - 2.0, y, x + r + 2.0, y);
                line(&mut s, x, y - r - 2.0, x, y + r + 2.0);
            }
        }
        s.push_str("Q\n");
        if set.color_bars {
            // along the bottom, left of the middle registration mark, between the corner crop marks
            let room = (tx1 - tx0) / 2.0 - (o + l) - 14.0;
            const PATCHES: [[f32; 4]; 14] = [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
                [1.0, 1.0, 0.0, 0.0],
                [0.0, 1.0, 1.0, 0.0],
                [1.0, 0.0, 1.0, 0.0],
                [0.5, 0.0, 0.0, 0.0],
                [0.0, 0.5, 0.0, 0.0],
                [0.0, 0.0, 0.5, 0.0],
                [0.0, 0.0, 0.0, 0.5],
                [0.0, 0.0, 0.0, 0.25],
                [0.0, 0.0, 0.0, 0.1],
                [0.0, 0.0, 0.0, 0.0],
            ];
            let size = (room / PATCHES.len() as f32).min(9.0);
            if size >= 3.0 {
                let y = ty0 - o - 9.0;
                let x0 = tx0 + o + l + 4.0;
                let _ = writeln!(s, "q");
                for (i, p) in PATCHES.iter().enumerate() {
                    let _ = writeln!(s, "{} {} {} {} k {} {} {} {} re f", num(p[0]), num(p[1]), num(p[2]), num(p[3]), num(x0 + i as f32 * size), num(y), num(size), num(9.0));
                }
                let _ = writeln!(s, "Q");
            } else {
                notes.push("The page is too small for colour bars".into());
            }
        }
        s
    }
}

fn q_balance(data: &[u8]) -> (usize, usize) {
    let (mut depth, mut low) = (0i64, 0i64);
    for op in cs::parse(data) {
        match op.op.as_slice() {
            b"q" => depth += 1,
            b"Q" => {
                depth -= 1;
                low = low.min(depth);
            }
            _ => {}
        }
    }
    ((-low).min(256) as usize, (depth - low).min(256) as usize)
}

fn contents(page: &PdfObject) -> Result<Vec<PdfObject>> {
    let mut out = Vec::new();
    if let Some(c) = page.get_dict("Contents")? {
        if c.is_array()? {
            for i in 0..c.len()? {
                if let Some(o) = c.get_array(i as i32)? {
                    out.push(o);
                }
            }
        } else if !c.is_null()? {
            out.push(c);
        }
    }
    Ok(out)
}

fn registration_space(doc: &mut PdfDocument) -> Result<PdfObject> {
    let f = doc.new_object_from_str("<</FunctionType 2/Domain[0 1]/C0[0 0 0 0]/C1[1 1 1 1]/N 1>>")?;
    let mut a = doc.new_array()?;
    a.array_push(PdfObject::new_name("Separation")?)?;
    a.array_push(PdfObject::new_name("All")?)?;
    a.array_push(PdfObject::new_name("DeviceCMYK")?)?;
    a.array_push(f)?;
    Ok(a)
}

fn mark_page(doc: &mut PdfDocument, index: u32, spec: &MarksSpec, notes: &mut Vec<String>) -> Result<()> {
    let mut page = doc.find_page(index as i32)?;
    let media = effective(&page, BoxKind::Media)?;
    let crop = effective(&page, BoxKind::Crop)?;
    let trim = effective(&page, BoxKind::Trim)?;
    let bleed = union(effective(&page, BoxKind::Bleed)?, trim);
    let bleed_out = [trim[0] - bleed[0], trim[1] - bleed[1], bleed[2] - trim[2], bleed[3] - trim[3]].into_iter().fold(0.0f32, f32::max);
    let weight = spec.weight.unwrap_or(0.25);
    if !(weight.is_finite() && weight > 0.0 && weight <= 10.0) {
        return Err(bad("The mark weight must be between 0 and 10 points"));
    }
    let length = spec.length.unwrap_or(18.0);
    if !(length.is_finite() && (3.0..=144.0).contains(&length)) {
        return Err(bad("The crop mark length must be between 3 and 144 points"));
    }
    if let Some(o) = spec.offset {
        if !(o.is_finite() && (0.0..=144.0).contains(&o)) {
            return Err(bad("The offset must be between 0 and 144 points"));
        }
    }
    let offset = spec.offset.unwrap_or(bleed_out + 6.0).max(bleed_out + 2.0);
    let lay = Layout { trim, bleed, offset, length };
    let slug = lay.slug();

    // The boxes must survive the enlarged media box: a box the page leaves to its default (the crop box,
    // which the media box then defaults) is written now, as it is before the change.
    for (kind, value) in [(BoxKind::Trim, trim), (BoxKind::Bleed, effective(&page, BoxKind::Bleed)?), (BoxKind::Art, effective(&page, BoxKind::Art)?)] {
        if stated(&page, kind)?.is_none() {
            write(doc, &mut page, kind, value)?;
        }
    }
    let had_crop = stated(&page, BoxKind::Crop)?.is_some();
    let new_media = union(media, [trim[0] - slug, trim[1] - slug, trim[2] + slug, trim[3] + slug]);
    write(doc, &mut page, BoxKind::Media, new_media)?;
    if had_crop {
        // a crop box inside the new media box would hide the marks
        write(doc, &mut page, BoxKind::Crop, union(crop, new_media))?;
    }

    // resources: the registration colour space, in a dictionary of the page's own
    let mut res = match page.get_dict_inheritable("Resources")? {
        Some(r) if r.is_dict()? => r.copy_dict()?,
        _ => doc.new_dict()?,
    };
    let mut csd = match res.get_dict("ColorSpace")? {
        Some(c) if c.is_dict()? => c.copy_dict()?,
        _ => doc.new_dict()?,
    };
    let reg = registration_space(doc)?;
    csd.dict_put(REG, reg)?;
    res.dict_put("ColorSpace", csd)?;
    page.dict_put("Resources", res)?;

    // contents: the page's own, balanced, then the marks
    let list = contents(&page)?;
    let mut data = Vec::new();
    for o in &list {
        if o.is_stream()? {
            data.extend(o.read_stream()?);
            data.push(b'\n');
        }
    }
    let (opens, closes) = q_balance(&data);
    let marks = lay.content(spec.marks, weight, notes);
    let add = |doc: &mut PdfDocument, text: &str| -> Result<PdfObject> { Ok(doc.add_stream(&Buffer::from_bytes(text.as_bytes())?, None, false)?) };
    let mut out = doc.new_array_with_capacity(list.len() as i32 + 3)?;
    out.array_push(add(doc, &format!("{}q\n", "q\n".repeat(opens)))?)?;
    for o in list {
        out.array_push(o)?;
    }
    out.array_push(add(doc, &format!("{}Q\n", "Q\n".repeat(closes)))?)?;
    out.array_push(add(doc, &marks)?)?;
    page.dict_put("Contents", out)?;
    Ok(())
}

/// Adds printer's marks to pages, as one undo step. Returns the new state and notes for the person
/// (a page too small for colour bars, a page without bleed).
pub fn add_printer_marks(eng: &Engine, doc: DocId, spec: &MarksSpec, source: EditSource) -> Result<(EditState, Vec<String>)> {
    if !(spec.marks.crop || spec.marks.bleed || spec.marks.registration || spec.marks.color_bars) {
        return Err(bad("Pick at least one kind of mark"));
    }
    let mut notes = Vec::new();
    let state = eng.apply_custom(doc, "Add printer's marks".to_string(), source, false, |d| {
        let count = d.page_count()?.max(0) as u32;
        let pages: Vec<u32> = match &spec.pages {
            Some(p) => {
                if let Some(n) = p.iter().find(|&&n| n >= count) {
                    return Err(crate::engine::EngineError::BadPage(*n));
                }
                let mut p = p.clone();
                p.sort_unstable();
                p.dedup();
                p
            }
            None => (0..count).collect(),
        };
        for &p in &pages {
            mark_page(d, p, spec, &mut notes)?;
        }
        Ok(pages)
    })?;
    notes.sort();
    notes.dedup();
    Ok((state, notes))
}
