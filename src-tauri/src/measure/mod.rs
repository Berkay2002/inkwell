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
        crate::outline::refuse_if_certified(d, "Setting a measurement scale")?;
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
        crate::outline::refuse_unless_annotating_allowed(d, "Adding a measurement")?;
        made = Some(write::add(d, page, kind, points, scale, author)?);
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

/// The snap for one point, from the page's cached index.
pub fn snap_at(eng: &Engine, cache: &SnapCache, doc: DocId, page: u32, x: f32, y: f32, radius: f32) -> Result<Option<SnapHit>> {
    Ok(cache.index(eng, doc, page)?.snap(x, y, radius))
}
