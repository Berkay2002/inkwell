//! Where a page's display space lies in its content stream's space.
//!
//! Marks are laid out in display space (x right, y up, origin at the bottom left of the page as
//! shown, `/Rotate` and crop box applied). A `cm` of [`View::matrix`] at the start of the mark's
//! stream maps that space onto the page's user space, whatever the page's box and rotation.

use crate::engine::Result;
use mupdf::pdf::PdfPage;

/// A PDF matrix `[a b c d e f]` (row vectors: `x' = a·x + c·y + e`).
pub(super) type M = [f64; 6];

/// `a` first, then `b`.
pub(super) fn mul(a: M, b: M) -> M {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

pub(super) fn invert(m: M) -> Option<M> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    Some([
        m[3] / det,
        -m[1] / det,
        -m[2] / det,
        m[0] / det,
        (m[2] * m[5] - m[3] * m[4]) / det,
        (m[1] * m[4] - m[0] * m[5]) / det,
    ])
}

/// A page as the person sees it.
#[derive(Clone, Copy, Debug)]
pub(super) struct View {
    pub w: f64,
    pub h: f64,
    /// Display space to user space.
    pub matrix: M,
}

impl View {
    pub(super) fn of(page: &PdfPage) -> Result<View> {
        let b = page.bounds()?;
        let (w, h) = (f64::from(b.width()), f64::from(b.height()));
        let c = page.object().page_ctm()?;
        let user_to_page: M = [c.a, c.b, c.c, c.d, c.e, c.f].map(f64::from);
        let back = invert(user_to_page).ok_or_else(|| crate::engine::EngineError::Invalid("this page has no usable geometry".into()))?;
        // Display (y up) to MuPDF's page space (y down, origin at the top left), then to user space.
        let flip: M = [1.0, 0.0, 0.0, -1.0, f64::from(b.x0), f64::from(b.y1)];
        Ok(View { w, h, matrix: mul(flip, back) })
    }

    /// User space to display space: what a Form XObject made from this page needs as its `/Matrix`.
    pub(super) fn to_display(&self) -> M {
        invert(self.matrix).unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    }
}

/// A number as short as a content stream needs.
pub(super) fn n(v: f64) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    let s = format!("{:.3}", if v.abs() < 5e-4 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.into() }
}
