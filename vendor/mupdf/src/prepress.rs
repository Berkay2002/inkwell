//! Print production support on top of MuPDF: pixmaps that carry separations, ICC colour spaces
//! and the colour-conversion filter (`vendor/mupdf-sys/wrapper/prepress.c`).

use std::ops::Deref;

use mupdf_sys::*;

use crate::pdf::PdfDocument;
use crate::{context, Colorspace, Error, IRect, Pixmap, Separations};

/// The colour model of an ICC profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IccKind {
    Gray,
    Rgb,
    Cmyk,
}

/// An ICC profile loaded as a colour space; dropped with its last use.
#[derive(Debug)]
pub struct IccColorspace(Colorspace);

impl IccColorspace {
    pub fn new(kind: IccKind, profile: &[u8]) -> Result<Self, Error> {
        let kind = match kind {
            IccKind::Gray => 1,
            IccKind::Rgb => 2,
            IccKind::Cmyk => 4,
        };
        let inner = unsafe { ffi_try!(mupdf_new_icc_colorspace(context(), kind, profile.as_ptr(), profile.len())) }?;
        if inner.is_null() {
            return Err(Error::UnexpectedNullPtr);
        }
        Ok(Self(unsafe { Colorspace::from_raw(inner) }))
    }
}

impl Deref for IccColorspace {
    type Target = Colorspace;
    fn deref(&self) -> &Colorspace {
        &self.0
    }
}

impl Drop for IccColorspace {
    fn drop(&mut self) {
        unsafe { mupdf_drop_colorspace(context(), self.0.inner) }
    }
}

impl Pixmap {
    /// An empty pixmap that also carries `seps`: every separation set to `Spot` gets a plane after
    /// the colour space's own, and overprint is simulated while painting into it. The image area
    /// is not initialised.
    pub fn new_with_separations(cs: &Colorspace, rect: IRect, seps: &Separations, alpha: bool) -> Result<Self, Error> {
        unsafe {
            ffi_try!(mupdf_new_pixmap_with_seps(
                context(),
                cs.inner,
                rect.x0,
                rect.y0,
                rect.x1 - rect.x0,
                rect.y1 - rect.y0,
                seps.as_ptr(),
                alpha
            ))
        }
        .map(|inner| unsafe { Pixmap::from_raw(inner) })
    }

    /// How many spot planes the pixmap carries.
    pub fn spot_planes(&self) -> usize {
        unsafe { mupdf_pixmap_seps_count(context(), self.inner) as usize }
    }
}

/// Which colours [`PdfDocument::convert_colors`] converts to DeviceCMYK.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConvertColors {
    pub gray: bool,
    pub rgb: bool,
    /// Separation and DeviceN colours (through their alternate).
    pub spot: bool,
    /// Gray becomes K alone (K = 1 - gray).
    pub gray_to_k: bool,
    /// Pure black (gray 0, RGB 0 0 0) becomes K = 1 and nothing else.
    pub preserve_black: bool,
}

impl PdfDocument {
    /// Rewrites the page's content (annotations, forms and patterns included) so the picked kinds
    /// of colour are DeviceCMYK. The values are computed through `profile` (a CMYK ICC profile) or,
    /// without one, MuPDF's own conversion. Images of those kinds are converted too.
    pub fn convert_colors(&mut self, page: i32, what: &ConvertColors, profile: Option<&IccColorspace>) -> Result<(), Error> {
        let opts = mupdf_convert_colors_opts {
            outcs: profile.map_or(std::ptr::null_mut(), |p| p.0.inner),
            gray: what.gray as i32,
            rgb: what.rgb as i32,
            spot: what.spot as i32,
            gray_to_k: what.gray_to_k as i32,
            preserve_black: what.preserve_black as i32,
        };
        unsafe { ffi_try!(mupdf_pdf_convert_colors(context(), self.as_raw(), page, &opts)) }
    }
}
