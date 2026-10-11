//! Version history store: immutable, encrypted, deduplicated versions of PDF files.
//! Design, measurements and integration points: docs/history.md.

mod crypto;
mod identity;
mod retention;
mod store;
pub mod types;

pub use retention::plan_retention;
pub use store::{CDC, SessionGuard, Store, chunks, default_root};
pub use types::*;

// DPAPI and NTFS: the store tests run on Windows only.
#[cfg(all(test, windows))]
mod tests;
