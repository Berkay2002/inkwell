//! Batch actions (Acrobat's Action Wizard): a saved sequence of steps run over many files.
//!
//! An [`Action`] is a name, ordered steps, an output rule and what to do when a file fails. The steps
//! reuse the engines that already exist (compress, OCR, marks, redact, protect, forms, convert) through
//! the `StepImpl` trait in `steps.rs`; this module adds no PDF logic of its own. [`run`] works through
//! the files one by one on private working copies and never writes over a source: see run.rs.
//!
//! - action.rs: the Action, its steps and their validation.
//! - steps.rs: one adapter per step, the environment and the trait.
//! - run.rs: the runner, its events and report.
//! - store.rs: Actions on disk (versioned JSON), import/export, the four built-ins.
//! - app.rs: the Tauri commands. Docs: docs/batch.md.

pub mod action;
pub mod app;
pub mod run;
pub mod steps;
pub mod store;
#[cfg(test)]
mod tests;

pub use action::{Action, Issue, OnError, OutputRule, Step};
pub use run::{BatchError, Event, FileReport, Report, Status, run};
pub use steps::Env;
pub use store::{Store, StoreError};
