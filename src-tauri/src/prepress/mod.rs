//! Print production (Acrobat's "Print production" tools): which inks a page uses, a page shown
//! with any set of them switched on, ink coverage and total area coverage (TAC), what an object
//! at a point is made of, colour conversion to CMYK, and printer's marks with trim and bleed boxes.
//!
//! Built on MuPDF's separations: a page is painted into a CMYK pixmap that carries one plane per
//! spot colour, and overprint is simulated while it is painted. Every view of the page (plates,
//! coverage) is composed from those planes. The PDF/X checks stay in `crate::preflight`; this
//! module calls them where it needs them. See docs/prepress.md.

use crate::engine::EngineError;

pub mod app;
mod boxes;
mod convert;
mod ink;
mod inspect;
mod marks;
mod render;
mod seps;
#[cfg(test)]
mod tests;

pub use boxes::{BoxKind, BoxSpec, set_boxes};
pub use convert::{ConvertReport, ConvertSpec, IntentSource, convert_colors};
pub use ink::{InkReport, PlaneInk, doc_ink, ink_report};
pub use inspect::{Inspected, ObjectKind, inspect_at};
pub use marks::{MarkSet, MarksSpec, add_printer_marks};
pub use render::{Mode, PROCESS, View, render_view_tile};
pub use seps::{PageSeps, Spot, page_separations};

pub type Result<T> = std::result::Result<T, EngineError>;

pub(crate) fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// The total area coverage above which a page is flagged, in percent.
pub const DEFAULT_TAC_LIMIT: f32 = 300.0;
