//! Fill & Sign (phase 3b): text, date, check/cross/dot/circle/line marks, signature and initials
//! images, placed on a page as ordinary annotations (so the comments engine still moves, resizes
//! and deletes them, and other viewers draw their appearance streams), and tagged with a private
//! key `/InkwellFS /<kind>` so they can be listed and flattened apart from comments.
//!
//! Every change is one `Engine::apply_custom` step: one undo step, kept by Save, recorded in history.
//! Geometry is in page space (points, y down). Move, resize and delete are `annot_edit` and
//! `annot_delete` (same `(page, xref)` names); only creation and flattening live here.
//! Saved signatures are in [`store`]; the webview draws typed and drawn ones and hands over PNGs.

use crate::annot::{self, AnnotDto, NewAnnot, Style, rect_of};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use mupdf::pdf::{PdfAnnotation, PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};

pub mod app;
pub mod store;
#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, EngineError>;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// The private annotation key that marks a Fill & Sign item; its value is the item kind.
pub const MARK: &str = "InkwellFS";
/// #1F2A44.
pub const INK: [f32; 3] = [31.0 / 255.0, 42.0 / 255.0, 68.0 / 255.0];
const DEFAULT_SIZE: f32 = 12.0;

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Mark {
    Check,
    Cross,
    Dot,
    /// A rounded rectangle outline, as Acrobat draws it.
    Circle,
    Line,
}

/// What to place. Points are `[x, y]`, rects `[x0, y0, x1, y1]`.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Spec {
    /// `at`: the top-left corner. `width` defaults to an estimate for Helvetica. Also the date.
    Text { at: [f32; 2], text: String, font_size: Option<f32>, width: Option<f32> },
    /// `at`: the centre. `size`: the side of the box in points (a line is three times as long, flat).
    Mark { mark: Mark, at: [f32; 2], size: Option<f32> },
    /// A signature or initials image in `rect`: a saved one by `saved_id`, or a PNG as base64.
    Image { kind: store::Kind, rect: [f32; 4], saved_id: Option<String>, png_base64: Option<String> },
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NewItem {
    pub page: u32,
    #[serde(flatten)]
    pub spec: Spec,
    /// Defaults to #1F2A44.
    pub color: Option<[f32; 3]>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemDto {
    /// text, check, cross, dot, circle, line, signature or initials.
    pub item: String,
    #[serde(flatten)]
    pub annot: AnnotDto,
}

fn kind_name(spec: &Spec) -> &'static str {
    match spec {
        Spec::Text { .. } => "text",
        Spec::Mark { mark: Mark::Check, .. } => "check",
        Spec::Mark { mark: Mark::Cross, .. } => "cross",
        Spec::Mark { mark: Mark::Dot, .. } => "dot",
        Spec::Mark { mark: Mark::Circle, .. } => "circle",
        Spec::Mark { mark: Mark::Line, .. } => "line",
        Spec::Image { kind: store::Kind::Signature, .. } => "signature",
        Spec::Image { .. } => "initials",
    }
}

fn marker(o: &PdfObject) -> Option<String> {
    let name = o.get_dict(MARK).ok().flatten()?.as_name().ok()?;
    Some(String::from_utf8_lossy(&name).into_owned())
}

// ---- reading -----------------------------------------------------------------------------------

/// Every Fill & Sign item of the document, in page order.
pub fn list(doc: &PdfDocument) -> Result<Vec<ItemDto>> {
    let mut out = Vec::new();
    for p in 0..doc.page_count()? as u32 {
        let page = doc.load_pdf_page(p as i32)?;
        for a in page.annotations() {
            if let Some(item) = marker(&a.object()) {
                out.push(ItemDto { item, annot: annot::dto(p, &a)? });
            }
        }
    }
    Ok(out)
}

// ---- writing -----------------------------------------------------------------------------------

/// Strokes of a mark inside the box `[x0, y0, x1, y1]`, from unit coordinates (y down).
fn strokes(b: [f32; 4], unit: &[&[[f32; 2]]]) -> Vec<Vec<[f32; 2]>> {
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    unit.iter().map(|s| s.iter().map(|p| [b[0] + p[0] * w, b[1] + p[1] * h]).collect()).collect()
}

/// A closed rounded rectangle outline in unit coordinates, corner radius `r`.
fn rounded(r: f32) -> Vec<[f32; 2]> {
    let mut pts = Vec::new();
    // corner centres, going clockwise from the top-left
    for (cx, cy, start) in [(r, r, 180.0f32), (1.0 - r, r, 270.0), (1.0 - r, 1.0 - r, 0.0), (r, 1.0 - r, 90.0)] {
        for i in 0..=6 {
            let a = (start + 15.0 * i as f32).to_radians();
            pts.push([cx + r * a.cos(), cy + r * a.sin()]);
        }
    }
    pts.push(pts[0]);
    pts
}

fn tag(a: &PdfAnnotation, kind: &str) -> Result<()> {
    a.object().dict_put(MARK, PdfObject::new_name(kind)?)?;
    Ok(())
}

/// Creates the item and returns its xref. `png` is the image of a signature or initials.
pub fn add(doc: &mut PdfDocument, it: &NewItem, png: Option<&[u8]>) -> Result<i32> {
    // Fill & Sign items are annotations: a certification allows them only at P=3.
    if crate::outline::certified(doc).map_or(true, |c| c.is_some_and(|p| p < 3)) {
        return Err(bad("this document is certified and its certification doesn't allow adding to it"));
    }
    let kind = kind_name(&it.spec);
    let color = it.color.unwrap_or(INK);
    let xref = match &it.spec {
        Spec::Text { at, text, font_size, width } => {
            if text.trim().is_empty() {
                return Err(bad("the text is empty"));
            }
            let size = font_size.unwrap_or(DEFAULT_SIZE).clamp(4.0, 200.0);
            // Helvetica averages about half an em per character; a little slack avoids wrapping
            let w = width.unwrap_or(text.chars().count() as f32 * size * 0.55 + 4.0);
            let what = NewAnnot::FreeText { rect: [at[0], at[1], at[0] + w, at[1] + size * 1.4], text: text.clone(), font_size: Some(size) };
            annot::add(doc, it.page, &what, &Style { color: Some(color), ..Default::default() }, "")?
        }
        Spec::Mark { mark, at, size } => {
            let s = size.unwrap_or(DEFAULT_SIZE).clamp(2.0, 400.0);
            let (w, h) = if *mark == Mark::Line { (s * 3.0, 0.0) } else { (s, s) };
            let b = [at[0] - w / 2.0, at[1] - h / 2.0, at[0] + w / 2.0, at[1] + h / 2.0];
            let style = Style { color: Some(color), width: Some((s / 8.0).max(0.75)), ..Default::default() };
            let (what, style) = match mark {
                Mark::Check => (NewAnnot::Ink { strokes: strokes(b, &[&[[0.1, 0.55], [0.38, 0.85], [0.92, 0.12]]]) }, style),
                Mark::Cross => (NewAnnot::Ink { strokes: strokes(b, &[&[[0.12, 0.12], [0.88, 0.88]], &[[0.88, 0.12], [0.12, 0.88]]]) }, style),
                Mark::Circle => (NewAnnot::Ink { strokes: vec![strokes(b, &[&rounded(0.3)]).remove(0)] }, style),
                Mark::Dot => (
                    NewAnnot::Shape { shape: annot::Kind::Ellipse, points: vec![[b[0], b[1]], [b[2], b[3]]] },
                    Style { fill: Some(color), width: Some(0.5), ..style },
                ),
                Mark::Line => (NewAnnot::Shape { shape: annot::Kind::Line, points: vec![[b[0], at[1]], [b[2], at[1]]] }, style),
            };
            annot::add(doc, it.page, &what, &style, "")?
        }
        Spec::Image { rect, .. } => {
            let png = png.ok_or_else(|| bad("a signature needs an image"))?;
            let mut page = doc.load_pdf_page(it.page as i32)?;
            let mut a = page.add_stamp_annotation(rect_of(*rect), if kind == "signature" { "Signature" } else { "Initials" })?;
            a.set_stamp_image(png)?;
            annot::meta(&mut a, "", true)?;
            a.update()?;
            a.xref()?
        }
    };
    let (_page, a) = annot::find(doc, it.page, xref)?;
    tag(&a, kind)?;
    Ok(xref)
}

/// Bakes the Fill & Sign items of `pages` (all pages when `None`) into the page content, and returns the
/// pages that had some. Everything else (comments, links, form widgets) stays an annotation:
/// `pdf_bake_document` bakes every annotation of every page, so it is run while /Annots holds only the
/// items to bake, and the rest is put back afterwards.
pub fn flatten(doc: &mut PdfDocument, pages: Option<&[u32]>) -> Result<Vec<u32>> {
    // Flattening changes the page content, which no certification allows.
    if crate::outline::certified(doc).map_or(true, |c| c.is_some()) {
        return Err(bad("this document is certified, and flattening would change its pages"));
    }
    let mut held = Vec::new();
    let mut hit = Vec::new();
    for p in 0..doc.page_count()? as u32 {
        let page = doc.find_page(p as i32)?;
        let Some(mut annots) = page.get_dict("Annots")? else { continue };
        let want = pages.is_none_or(|ps| ps.contains(&p));
        let (mut keep, mut i, mut baking) = (Vec::new(), 0, false);
        while i < annots.len()? {
            let a = annots.get_array(i as i32)?.ok_or_else(|| bad("a page has a broken annotation list"))?;
            if want && marker(&a).is_some() {
                baking = true;
                i += 1;
            } else {
                annots.array_delete(i as i32)?;
                keep.push(a);
            }
        }
        if baking {
            hit.push(p);
        }
        held.push((annots, keep));
    }
    if hit.is_empty() {
        return Err(bad("there is nothing to flatten"));
    }
    doc.bake(true, false)?;
    for (mut annots, keep) in held {
        for a in keep {
            annots.array_push(a)?;
        }
    }
    Ok(hit)
}

// ---- engine wiring: one undo step each --------------------------------------------------------

/// `png`: the image for a signature or initials (the caller resolved it from a saved one or the request).
pub fn add_one(eng: &Engine, doc: DocId, it: &NewItem, png: Option<&[u8]>, source: EditSource) -> Result<(EditState, i32)> {
    let mut xref = 0;
    let state = eng.apply_custom(doc, format!("Add {} on p. {}", kind_name(&it.spec), it.page + 1), source, false, |d| {
        xref = add(d, it, png)?;
        Ok(vec![it.page])
    })?;
    Ok((state, xref))
}

pub fn flatten_one(eng: &Engine, doc: DocId, pages: Option<&[u32]>, source: EditSource) -> Result<EditState> {
    eng.apply_custom(doc, "Flatten Fill & Sign items".to_owned(), source, false, |d| flatten(d, pages))
}

pub fn list_all(eng: &Engine, doc: DocId) -> Result<Vec<ItemDto>> {
    eng.read_custom(doc, |d| list(d))
}
