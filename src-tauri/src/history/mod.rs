//! Version history store: immutable, encrypted, deduplicated versions of PDF files.
//! Design, measurements and integration points: docs/history.md.

pub(crate) mod crypto;
mod identity;
mod retention;
mod store;
pub mod types;

pub use retention::plan_retention;
pub use store::{CDC, SessionGuard, Store, chunks, default_root};
pub(crate) use store::write_atomic;
pub use types::*;

#[cfg(test)]
mod tests;
