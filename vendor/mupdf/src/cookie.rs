use mupdf_sys::*;

use crate::{context, Error};

/// Provide two-way communication between application and library.
/// Intended for multi-threaded applications where one thread is rendering pages and
/// another thread wants to read progress feedback or abort a job that takes a long time to finish.
/// The communication is unsynchronized without locking.
#[derive(Debug)]
pub struct Cookie {
    pub(crate) inner: *mut fz_cookie,
}

impl Cookie {
    pub fn new() -> Result<Self, Error> {
        unsafe { ffi_try!(mupdf_new_cookie(context())) }.map(|inner| Self { inner })
    }

    /// Abort rendering
    pub fn abort(&mut self) {
        unsafe {
            (*self.inner).abort = 1;
        }
    }

    /// [`Cookie::abort`] through a shared reference, for a device callback that runs while the cookie is lent to
    /// the page run (the flag is one plain write the C side also reads unsynchronized).
    pub fn abort_shared(&self) {
        unsafe {
            (*self.inner).abort = 1;
        }
    }

    /// Communicates rendering progress back to the application.
    /// Increments as a page is being rendered.
    pub fn progress(&self) -> i32 {
        unsafe { (*self.inner).progress }
    }

    /// Communicates the known upper bound of rendering back to the application
    pub fn max_progress(&self) -> usize {
        unsafe { (*self.inner).progress_max }
    }

    /// count of errors during current rendering
    pub fn errors(&self) -> i32 {
        unsafe { (*self.inner).errors }
    }

    /// Initially should be set to 0.
    /// Will be set to non-zero if a TRYLATER error is thrown during rendering
    pub fn incomplete(&self) -> bool {
        unsafe { (*self.inner).incomplete > 0 }
    }

    pub fn set_incomplete(&mut self, value: bool) {
        let val = if value { 1 } else { 0 };
        unsafe {
            (*self.inner).incomplete = val;
        }
    }
}

/// A way to abort a page run from another thread: the flag is one plain write the C side reads without locking, which is
/// what the cookie is for ("another thread wants to abort a job"). Borrowing the cookie keeps it alive while the handle is.
#[derive(Debug)]
pub struct AbortHandle<'a> {
    inner: *mut fz_cookie,
    _cookie: std::marker::PhantomData<&'a Cookie>,
}

// SAFETY: see the type's doc; the only use is writing the abort flag, and the borrow keeps the cookie allocated.
unsafe impl Send for AbortHandle<'_> {}
unsafe impl Sync for AbortHandle<'_> {}

impl AbortHandle<'_> {
    pub fn abort(&self) {
        unsafe {
            (*self.inner).abort = 1;
        }
    }
}

impl Cookie {
    pub fn abort_handle(&self) -> AbortHandle<'_> {
        AbortHandle { inner: self.inner, _cookie: std::marker::PhantomData }
    }
}

impl Drop for Cookie {
    fn drop(&mut self) {
        if !self.inner.is_null() {
            unsafe { fz_free(context(), self.inner.cast()) }
        }
    }
}
