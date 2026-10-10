//! Inkwell patch: image rewriting with progress/cancel, font subsetting, object streams.
//!
//! Backed by `vendor/mupdf-sys/wrapper/compress.c` (a copy of MuPDF's `pdf_rewrite_images` that reports
//! progress per page, can be cancelled and keeps going when one image fails).

use std::ffi::{c_int, c_void, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

use mupdf_sys::*;

use super::{PdfDocument, PdfWriteOptions};
use crate::{context, Error};

/// How a class of images is re-encoded once it has (maybe) been resampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recompress {
    /// Leave the image alone.
    Never,
    /// Same family as the source (JPEG stays JPEG, JPX stays JPX, CCITT stays CCITT, the rest Flate).
    Same,
    /// Like `Same`, but only for images that were resampled; the others are not touched.
    SameIfScaled,
    Flate,
    Jpeg,
    Jpx,
    /// CCITT G4 (bitonal images only).
    Fax,
}

impl Recompress {
    fn raw(self) -> c_int {
        match self {
            Self::Never => 0,
            Self::Same => 1,
            Self::Flate => 2,
            Self::Jpeg => 3,
            Self::Jpx => 4,
            Self::Fax => 5,
            Self::SameIfScaled => 6,
        }
    }
}

/// Options for one image class (colour / gray, lossy / lossless, or bitonal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageClassOptions {
    /// Downsample images shown above this many ppi (0 = never downsample).
    pub threshold_ppi: u32,
    /// ... to this many ppi.
    pub target_ppi: u32,
    /// Bicubic resampling; `false` is box averaging (halves in powers of two).
    pub bicubic: bool,
    pub method: Recompress,
    /// 1..=100 for JPEG / JPX, 0 = MuPDF default.
    pub quality: u32,
}

impl ImageClassOptions {
    pub const OFF: Self = Self {
        threshold_ppi: 0,
        target_ppi: 0,
        bicubic: true,
        method: Recompress::Never,
        quality: 0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageRewriteOptions {
    pub color_lossy: ImageClassOptions,
    pub color_lossless: ImageClassOptions,
    pub gray_lossy: ImageClassOptions,
    pub gray_lossless: ImageClassOptions,
    pub bitonal: ImageClassOptions,
    /// Keep a recompressed image even when it is not smaller.
    pub always: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageRewriteStats {
    /// Image uses visited (an image shown on three pages counts three times).
    pub seen: u32,
    pub resampled: u32,
    pub replaced: u32,
    /// Rewritten, but not smaller, so the original was kept.
    pub kept: u32,
    /// Could not be decoded / encoded; kept as is.
    pub failed: u32,
}

/// `pass` is 0 while scanning for image sizes and 1 while rewriting; return `true` to cancel.
pub type RewriteProgress<'a> = &'a mut dyn FnMut(u32, usize, usize) -> bool;

unsafe extern "C" fn progress_trampoline(
    opaque: *mut c_void,
    pass: c_int,
    page: c_int,
    pages: c_int,
) -> c_int {
    // SAFETY: `opaque` is the `&mut RewriteProgress` that `rewrite_images` keeps alive for the call.
    let cb = unsafe { &mut *(opaque as *mut RewriteProgress<'_>) };
    match catch_unwind(AssertUnwindSafe(|| {
        cb(pass.max(0) as u32, page.max(0) as usize, pages.max(0) as usize)
    })) {
        Ok(false) => 0,
        // A panicking callback cancels the job rather than unwinding through C.
        Ok(true) | Err(_) => 1,
    }
}

impl PdfWriteOptions {
    pub fn use_objstms(&self) -> bool {
        self.inner.do_use_objstms != 0
    }

    /// Pack small objects and the xref into compressed streams (PDF 1.5).
    pub fn set_use_objstms(&mut self, value: bool) -> &mut Self {
        self.inner.do_use_objstms = value as c_int;
        self
    }
}

struct ClassStrings {
    lossy: CString,
    lossless: CString,
}

impl PdfDocument {
    /// Downsamples and recompresses images in place; see [`ImageRewriteOptions`].
    pub fn rewrite_images(
        &mut self,
        opts: &ImageRewriteOptions,
        progress: RewriteProgress<'_>,
    ) -> Result<ImageRewriteStats, Error> {
        let q = |c: &ImageClassOptions| CString::new(c.quality.min(100).to_string()).unwrap();
        let color = ClassStrings { lossy: q(&opts.color_lossy), lossless: q(&opts.color_lossless) };
        let gray = ClassStrings { lossy: q(&opts.gray_lossy), lossless: q(&opts.gray_lossless) };
        let bitonal = q(&opts.bitonal);
        let method = |c: &ImageClassOptions| c.bicubic as c_int;

        let mut raw: pdf_image_rewriter_options = unsafe { std::mem::zeroed() };
        raw.color_lossy_image_subsample_method = method(&opts.color_lossy);
        raw.color_lossy_image_subsample_threshold = opts.color_lossy.threshold_ppi as c_int;
        raw.color_lossy_image_subsample_to = opts.color_lossy.target_ppi as c_int;
        raw.color_lossy_image_recompress_method = opts.color_lossy.method.raw();
        raw.color_lossy_image_recompress_quality = color.lossy.as_ptr() as *mut _;
        raw.color_lossless_image_subsample_method = method(&opts.color_lossless);
        raw.color_lossless_image_subsample_threshold = opts.color_lossless.threshold_ppi as c_int;
        raw.color_lossless_image_subsample_to = opts.color_lossless.target_ppi as c_int;
        raw.color_lossless_image_recompress_method = opts.color_lossless.method.raw();
        raw.color_lossless_image_recompress_quality = color.lossless.as_ptr() as *mut _;
        raw.gray_lossy_image_subsample_method = method(&opts.gray_lossy);
        raw.gray_lossy_image_subsample_threshold = opts.gray_lossy.threshold_ppi as c_int;
        raw.gray_lossy_image_subsample_to = opts.gray_lossy.target_ppi as c_int;
        raw.gray_lossy_image_recompress_method = opts.gray_lossy.method.raw();
        raw.gray_lossy_image_recompress_quality = gray.lossy.as_ptr() as *mut _;
        raw.gray_lossless_image_subsample_method = method(&opts.gray_lossless);
        raw.gray_lossless_image_subsample_threshold = opts.gray_lossless.threshold_ppi as c_int;
        raw.gray_lossless_image_subsample_to = opts.gray_lossless.target_ppi as c_int;
        raw.gray_lossless_image_recompress_method = opts.gray_lossless.method.raw();
        raw.gray_lossless_image_recompress_quality = gray.lossless.as_ptr() as *mut _;
        raw.bitonal_image_subsample_method = method(&opts.bitonal);
        raw.bitonal_image_subsample_threshold = opts.bitonal.threshold_ppi as c_int;
        raw.bitonal_image_subsample_to = opts.bitonal.target_ppi as c_int;
        raw.bitonal_image_recompress_method = opts.bitonal.method.raw();
        raw.bitonal_image_recompress_quality = bitonal.as_ptr() as *mut _;
        raw.recompress_when = opts.always as c_int;

        let mut stats = mupdf_rewrite_stats {
            images_seen: 0,
            images_resampled: 0,
            images_replaced: 0,
            images_kept: 0,
            images_failed: 0,
        };
        let mut progress = progress;
        let opaque = ptr::addr_of_mut!(progress) as *mut c_void;
        unsafe {
            ffi_try!(mupdf_pdf_rewrite_images_progress(
                context(),
                self.inner,
                &mut raw,
                &mut stats,
                Some(progress_trampoline),
                opaque
            ))?;
        }
        Ok(ImageRewriteStats {
            seen: stats.images_seen as u32,
            resampled: stats.images_resampled as u32,
            replaced: stats.images_replaced as u32,
            kept: stats.images_kept as u32,
            failed: stats.images_failed as u32,
        })
    }

    /// Subsets every embedded TrueType/CID font to the glyphs the pages use.
    pub fn subset_fonts(&mut self) -> Result<(), Error> {
        unsafe { ffi_try!(mupdf_pdf_subset_all_fonts(context(), self.inner)) }
    }
}
