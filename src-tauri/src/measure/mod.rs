//! Measuring tools (engine): distance, perimeter and area with a drawing scale and units, `/Measure` dictionaries
//! that Acrobat reads and writes, measurement annotations, and snapping to a page's vector paths. Docs:
//! `docs/measure.md`.
//!
//! Points are in page space (points, y down, as the viewer shows the page, rotation applied); `/UserUnit` and the
//! page rotation are folded in by [`calc::PageGeom`]. Creating a measurement and setting a page scale are
//! `Engine::apply_custom` steps: one undo step each, saved by Save, recorded in history.

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use serde::Serialize;

pub mod app;
pub mod calc;
pub mod dict;
pub mod numfmt;
pub mod snap;
#[cfg(test)]
mod tests;
pub mod write;

pub use calc::{Kind, Quantity, Scale, Unit};
pub use snap::{SnapCache, SnapHit, SnapIndex, SnapKind};
pub use write::MeasureDto;

type Result<T> = std::result::Result<T, EngineError>;

/// A page's scale and whether the file defines one (a page without gets 1 in = 1 in).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PageScale {
    pub scale: Scale,
    pub defined: bool,
    /// "1 in = 10 ft"
    pub label: String,
}

pub fn scale_get(eng: &Engine, doc: DocId, page: u32) -> Result<PageScale> {
    eng.read_custom(doc, |d| {
        let (scale, defined) = write::page_scale(d, page)?;
        Ok(PageScale { label: scale.label(), scale, defined })
    })
}

pub fn scale_set(eng: &Engine, doc: DocId, page: u32, scale: &Scale, source: EditSource) -> Result<EditState> {
    scale.validate().map_err(EngineError::Invalid)?;
    eng.apply_custom(doc, format!("Set measurement scale on p. {} to {}", page + 1, scale.label()), source, false, |d| {
        write::set_page_scale(d, page, scale)?;
        Ok(vec![page])
    })
}

/// Adds the measurement as one undo step and returns the new state, the annotation's xref and its value. With no
/// `scale`, the page's own scale at the first point applies.
#[allow(clippy::too_many_arguments)]
pub fn add_one(
    eng: &Engine,
    doc: DocId,
    page: u32,
    kind: Kind,
    points: &[[f32; 2]],
    scale: Option<&Scale>,
    author: &str,
    source: EditSource,
) -> Result<(EditState, i32, Quantity)> {
    let mut made = None;
    let state = eng.apply_custom(doc, format!("Add {} measurement on p. {}", kind.noun(), page + 1), source, false, |d| {
        made = Some(write::add(d, page, kind, points, scale, author)?);
        Ok(vec![page])
    })?;
    let (xref, quantity) = made.ok_or(EngineError::Panic)?;
    Ok((state, xref, quantity))
}

/// Gives the measurement `xref` another scale and values it again: the annotation is replaced by one with the same
/// kind, points and author (one undo step; the xref changes).
pub fn rescale(eng: &Engine, doc: DocId, page: u32, xref: i32, scale: &Scale, source: EditSource) -> Result<(EditState, i32, Quantity)> {
    scale.validate().map_err(EngineError::Invalid)?;
    let mut made = None;
    let state = eng.apply_custom(doc, format!("Change the scale of a measurement on p. {} to {}", page + 1, scale.label()), source, false, |d| {
        let old = write::list(d)?
            .into_iter()
            .find(|m| m.page == page && m.xref == xref)
            .ok_or_else(|| EngineError::Invalid("that measurement is no longer in the file".into()))?;
        crate::annot::delete(d, page, &[xref])?;
        made = Some(write::add(d, page, old.kind, &old.points, Some(scale), &old.author)?);
        Ok(vec![page])
    })?;
    let (xref, quantity) = made.ok_or(EngineError::Panic)?;
    Ok((state, xref, quantity))
}

/// The value the points would measure, without making anything.
pub fn value_of(eng: &Engine, doc: DocId, page: u32, kind: Kind, points: &[[f32; 2]], scale: Option<&Scale>) -> Result<Quantity> {
    eng.read_custom(doc, |d| write::value(d, page, kind, points, scale))
}

pub fn list_all(eng: &Engine, doc: DocId) -> Result<Vec<MeasureDto>> {
    eng.read_custom(doc, write::list)
}

/// The measurements as CSV (UTF-8 with a byte order mark and CRLF lines, as spreadsheets expect): page, type, the
/// number and its unit apart, the label, the scale and the author. Text that a spreadsheet would run as a formula
/// gets a leading apostrophe.
pub fn csv_of(items: &[MeasureDto]) -> String {
    fn cell(s: &str) -> String {
        let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_owned() };
        if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
    }
    let mut out = String::from("\u{feff}Page,Type,Value,Unit,Label,Scale,Author\r\n");
    for m in items {
        let scale = m.scale.as_ref().map(Scale::label).unwrap_or_default();
        let row = [
            (m.page + 1).to_string(),
            m.kind.noun().to_owned(),
            calc::trim_num((m.quantity.value * 1e4).round() / 1e4), // f32 page points leave a rounding tail
            cell(&m.quantity.unit),
            cell(&m.quantity.text),
            cell(&scale),
            cell(&m.author),
        ];
        out.push_str(&row.join(","));
        out.push_str("\r\n");
    }
    out
}

/// The snap for one point, from the page's cached index.
pub fn snap_at(eng: &Engine, cache: &SnapCache, doc: DocId, page: u32, x: f32, y: f32, radius: f32) -> Result<Option<SnapHit>> {
    Ok(cache.index(eng, doc, page)?.snap(x, y, radius))
}
