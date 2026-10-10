//! Compare two PDFs (two versions or two different files): a typed semantic change list and visual
//! page diffs. Works on bytes only; nothing here touches the user's files or the engine.
//!
//! * [`compare`]: pages matched by content fingerprint + text similarity (not index), then per page
//!   rotation/boxes, annotations, form fields, text (word-level), images, OCR layer, redactions;
//!   document level metadata, bookmarks, attachments.
//! * [`render_page`] / [`visual_diff`]: cached renders and a pixel diff with changed regions as boxes.
//!
//! MuPDF contexts are per thread: call everything from one worker thread at a time (`spawn_blocking`).

mod content;
mod pages;
mod semantic;
mod snap;
pub mod types;
pub mod visual;

#[cfg(test)]
mod tests;

pub use types::*;
pub use visual::{BytesId, Raster, VisualDiff, diff_rasters, diff_rasters_hinted, render_page, render_page_pw, visual_diff, visual_diff_hinted};

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Semantic comparison of two PDFs given as bytes. Panics inside MuPDF are turned into an error.
pub fn compare(a: &[u8], b: &[u8], opts: &CompareOptions) -> Result<Comparison> {
    catch_unwind(AssertUnwindSafe(|| semantic::compare(a, b, opts))).unwrap_or(Err(DiffError::Panic))
}
