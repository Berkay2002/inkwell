//! Page boxes: setting TrimBox, BleedBox, ArtBox and CropBox, and reading the effective box of a kind.
//! Boxes are in default user space, `[x0, y0, x1, y1]` with y up, as the file stores them.

use super::{Result, bad};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine};
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};

pub(crate) type Box4 = [f32; 4];

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BoxKind {
    Media,
    Crop,
    Trim,
    Bleed,
    Art,
}

impl BoxKind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            BoxKind::Media => "MediaBox",
            BoxKind::Crop => "CropBox",
            BoxKind::Trim => "TrimBox",
            BoxKind::Bleed => "BleedBox",
            BoxKind::Art => "ArtBox",
        }
    }

    fn label(self) -> &'static str {
        match self {
            BoxKind::Media => "media box",
            BoxKind::Crop => "crop box",
            BoxKind::Trim => "trim box",
            BoxKind::Bleed => "bleed box",
            BoxKind::Art => "art box",
        }
    }
}

pub(crate) fn norm(b: Box4) -> Box4 {
    [b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])]
}

fn read(o: &PdfObject) -> Result<Option<Box4>> {
    if !o.is_array()? || o.len()? < 4 {
        return Ok(None);
    }
    let mut b = [0f32; 4];
    for (i, v) in b.iter_mut().enumerate() {
        match o.get_array(i as i32)?.and_then(|n| n.as_float().ok()) {
            Some(f) if f.is_finite() => *v = f,
            _ => return Ok(None),
        }
    }
    Ok(Some(norm(b)))
}

/// The box as the page states it (MediaBox and CropBox are inherited down the page tree).
pub(crate) fn stated(page: &PdfObject, kind: BoxKind) -> Result<Option<Box4>> {
    let o = if matches!(kind, BoxKind::Media | BoxKind::Crop) { page.get_dict_inheritable(kind.key())? } else { page.get_dict(kind.key())? };
    match o {
        Some(o) => read(&o),
        None => Ok(None),
    }
}

/// The box that applies: the stated one, else the default the PDF specification gives (the crop box,
/// which itself defaults to the media box).
pub(crate) fn effective(page: &PdfObject, kind: BoxKind) -> Result<Box4> {
    let media = stated(page, BoxKind::Media)?.unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let crop = stated(page, BoxKind::Crop)?.unwrap_or(media);
    Ok(match kind {
        BoxKind::Media => media,
        BoxKind::Crop => crop,
        _ => stated(page, kind)?.unwrap_or(crop),
    })
}

/// Which boxes the page states itself; the others are the defaults of `effective`.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatedBoxes {
    pub crop: bool,
    pub trim: bool,
    pub bleed: bool,
    pub art: bool,
}

/// The boxes of one page, as the UI shows them: user space, `[x0, y0, x1, y1]`, y up.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PageBoxes {
    /// `/Rotate`, 0, 90, 180 or 270.
    pub rotate: i32,
    pub media: Box4,
    pub crop: Box4,
    pub trim: Box4,
    pub bleed: Box4,
    pub art: Box4,
    pub stated: StatedBoxes,
}

/// The page's /Rotate (inherited) as 0, 90, 180 or 270, step for step as MuPDF reads it (pdf-page.c: a real
/// rounds half up to an int, a negative turns into 360 - n % 360, then it snaps to the nearest quarter turn
/// with halves rounding up), so the sheet and the renderer agree on which side is the left.
pub(crate) fn rotate_of(page: &PdfObject) -> Result<i32> {
    let v = page.get_dict_inheritable("Rotate")?.and_then(|r| r.as_float().ok()).unwrap_or(0.0);
    Ok(quarter_turn(v))
}

/// A /Rotate value as 0, 90, 180 or 270 (see [`rotate_of`]). `as i32` saturates; the rest is i64, so no value
/// overflows (negating i32::MIN did).
pub(super) fn quarter_turn(v: f32) -> i32 {
    let v = if v.is_finite() { v } else { 0.0 };
    let r = i64::from((v + 0.5).floor() as i32);
    let r = r.rem_euclid(360);
    (90 * ((r + 45) / 90) % 360) as i32
}

/// Reads the boxes of page `page` (0-based) of the document as it is now.
pub fn page_boxes(eng: &Engine, doc: DocId, page: u32) -> Result<PageBoxes> {
    eng.read_custom(doc, |pdf| {
        if page as i32 >= pdf.page_count()? {
            return Err(crate::engine::EngineError::BadPage(page));
        }
        let p = pdf.find_page(page as i32)?;
        let rotate = rotate_of(&p)?;
        let has = |k| stated(&p, k).map(|b| b.is_some());
        Ok(PageBoxes {
            rotate,
            media: effective(&p, BoxKind::Media)?,
            crop: effective(&p, BoxKind::Crop)?,
            trim: effective(&p, BoxKind::Trim)?,
            bleed: effective(&p, BoxKind::Bleed)?,
            art: effective(&p, BoxKind::Art)?,
            stated: StatedBoxes { crop: has(BoxKind::Crop)?, trim: has(BoxKind::Trim)?, bleed: has(BoxKind::Bleed)?, art: has(BoxKind::Art)? },
        })
    })
}

pub(crate) fn write(doc: &PdfDocument, page: &mut PdfObject, kind: BoxKind, b: Box4) -> Result<()> {
    let mut a = doc.new_array_with_capacity(4)?;
    for v in b {
        a.array_push(PdfObject::new_real(v)?)?;
    }
    page.dict_put(kind.key(), a)?;
    Ok(())
}

pub(crate) fn contains(outer: Box4, inner: Box4) -> bool {
    const TOL: f32 = 0.01;
    inner[0] >= outer[0] - TOL && inner[1] >= outer[1] - TOL && inner[2] <= outer[2] + TOL && inner[3] <= outer[3] + TOL
}

pub(crate) fn union(a: Box4, b: Box4) -> Box4 {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

/// Which box to set on which pages, and to what: an absolute `rect`, or the box `from` moved outward by
/// `margin` (`[left, bottom, right, top]` in points; negative moves inward). A bleed box is typically
/// the trim box with a margin of 9 points (3 mm is 8.5).
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BoxSpec {
    /// 0-based; every page when absent.
    pub pages: Option<Vec<u32>>,
    pub kind: BoxKind,
    pub rect: Option<[f32; 4]>,
    pub from: Option<BoxKind>,
    pub margin: Option<[f32; 4]>,
    /// `margin` is `[left, bottom, right, top]` of the page as it is shown, outward positive: each page maps
    /// it through its own /Rotate. Otherwise it is in the page's own space (`[x0, y0, x1, y1]`).
    #[serde(default)]
    pub shown: bool,
}

/// `m` (left, bottom, right, top of the page as shown) as `[x0, y0, x1, y1]` margins of a page turned by `rotate`.
fn user_margins(m: [f32; 4], rotate: i32) -> [f32; 4] {
    // the coordinate each shown side is: left, bottom, right, top
    let at = match rotate {
        90 => [1, 2, 3, 0],
        180 => [2, 3, 0, 1],
        270 => [3, 0, 1, 2],
        _ => [0, 1, 2, 3],
    };
    let mut out = [0.0; 4];
    for (side, &i) in at.iter().enumerate() {
        out[i] = m[side];
    }
    out
}

/// Sets a trim, bleed, art or crop box on pages, as one undo step. Refuses a result where the trim box
/// is not inside the bleed box or the bleed box is not inside the media box.
pub fn set_boxes(eng: &Engine, doc: DocId, spec: &BoxSpec, source: EditSource) -> Result<EditState> {
    if spec.kind == BoxKind::Media {
        return Err(bad("The media box is the paper size; it is changed by the page size tools"));
    }
    if spec.rect.is_none() && (spec.from.is_none() || spec.margin.is_none()) {
        return Err(bad("Give the box as a rectangle, or as another box and a margin"));
    }
    let label = format!("Set {}", spec.kind.label());
    eng.apply_custom(doc, label, source, false, |d| {
        super::refuse_certified(d, "Changing its page boxes")?;
        let count = d.page_count()?.max(0) as u32;
        let pages: Vec<u32> = match &spec.pages {
            Some(p) => {
                if let Some(n) = p.iter().find(|&&n| n >= count) {
                    return Err(crate::engine::EngineError::BadPage(*n));
                }
                p.clone()
            }
            None => (0..count).collect(),
        };
        for &p in &pages {
            let mut page = d.find_page(p as i32)?;
            let b = match (spec.rect, spec.from, spec.margin) {
                (Some(r), _, _) => norm(r),
                (None, Some(from), Some(m)) => {
                    let f = effective(&page, from)?;
                    let m = if spec.shown { user_margins(m, rotate_of(&page)?) } else { m };
                    norm([f[0] - m[0], f[1] - m[1], f[2] + m[2], f[3] + m[3]])
                }
                _ => unreachable!("checked above"),
            };
            if !b.iter().all(|v| v.is_finite()) || b[2] - b[0] < 1.0 || b[3] - b[1] < 1.0 {
                return Err(bad(format!("Page {}: the {} would be empty", p + 1, spec.kind.label())));
            }
            let media = effective(&page, BoxKind::Media)?;
            let (trim, bleed) = match spec.kind {
                BoxKind::Trim => (b, effective(&page, BoxKind::Bleed)?),
                BoxKind::Bleed => (effective(&page, BoxKind::Trim)?, b),
                _ => (effective(&page, BoxKind::Trim)?, effective(&page, BoxKind::Bleed)?),
            };
            match spec.kind {
                BoxKind::Bleed if !contains(media, b) => return Err(bad(format!("Page {}: the bleed box goes beyond the media box", p + 1))),
                BoxKind::Trim | BoxKind::Bleed if !contains(bleed, trim) => {
                    return Err(bad(format!("Page {}: the trim box must lie inside the bleed box", p + 1)));
                }
                BoxKind::Trim if !contains(media, b) => return Err(bad(format!("Page {}: the trim box goes beyond the media box", p + 1))),
                BoxKind::Crop | BoxKind::Art if !contains(media, b) => {
                    return Err(bad(format!("Page {}: the {} goes beyond the media box", p + 1, spec.kind.label())));
                }
                _ => {}
            }
            write(d, &mut page, spec.kind, b)?;
        }
        Ok(pages)
    })
}
