use mupdf_sys::*;

use std::ffi::{CStr, CString};
use std::ptr;

use crate::{context, Colorspace, Error};

/// What the renderer does with one separation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeparationBehavior {
    /// Folded into the process colours through its alternate.
    Composite,
    /// Painted into a plane of its own.
    Spot,
    /// Not painted.
    Disabled,
}


#[derive(Debug)]
pub struct Separations {
    pub(crate) inner: *mut fz_separations,
}

impl Separations {
    pub(crate) unsafe fn from_raw(ptr: *mut fz_separations) -> Self {
        Self { inner: ptr }
    }

    pub fn len(&self) -> usize {
        unsafe { fz_count_separations(context(), self.inner) as usize }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn active_count(&self) -> usize {
        unsafe { fz_count_active_separations(context(), self.inner) as usize }
    }

    /// An empty set, to which [`Separations::add`] adds spots by name.
    pub fn new() -> Result<Self, Error> {
        unsafe { ffi_try!(mupdf_new_separations(context())) }.map(|inner| Self { inner })
    }

    /// Adds a spot with display equivalents (`0xRRGGBBAA`, `0xCCMMYYKK`).
    pub fn add(&mut self, name: &str, rgba: u32, cmyk: u32) -> Result<(), Error> {
        let name = CString::new(name)?;
        unsafe { ffi_try!(mupdf_add_separation(context(), self.inner, name.as_ptr(), rgba, cmyk)) }
    }

    pub fn name(&self, idx: usize) -> Option<String> {
        let p = unsafe { fz_separation_name(context(), self.inner, idx as i32) };
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }

    pub fn behavior(&self, idx: usize) -> Result<SeparationBehavior, Error> {
        let b = unsafe { ffi_try!(mupdf_separation_behavior(context(), self.inner, idx as i32)) }?;
        Ok(match b {
            1 => SeparationBehavior::Spot,
            2 => SeparationBehavior::Disabled,
            _ => SeparationBehavior::Composite,
        })
    }

    pub fn set_behavior(&mut self, idx: usize, b: SeparationBehavior) -> Result<(), Error> {
        let b = match b {
            SeparationBehavior::Composite => 0,
            SeparationBehavior::Spot => 1,
            SeparationBehavior::Disabled => 2,
        };
        unsafe { ffi_try!(mupdf_separation_set_behavior(context(), self.inner, idx as i32, b)) }
    }

    /// The separation's colour in `cs` (its alternate, converted).
    pub fn equivalent(&self, idx: usize, cs: &Colorspace) -> Result<Vec<f32>, Error> {
        let mut out = vec![0f32; cs.n() as usize];
        unsafe { ffi_try!(mupdf_separation_equivalent(context(), self.inner, idx as i32, cs.inner, out.as_mut_ptr())) }?;
        Ok(out)
    }

    pub(crate) fn as_ptr(&self) -> *mut fz_separations {
        if self.inner.is_null() { ptr::null_mut() } else { self.inner }
    }
}

impl Drop for Separations {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            unsafe { fz_drop_separations(context(), self.inner) }
        }
    }
}
