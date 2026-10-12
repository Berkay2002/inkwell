//! Export a PDF (pages to images, text, HTML, Word, RTF) and Create a PDF (from images, text, the clipboard,
//! or blank): the logic behind the Tauri commands in `lib.rs`.
//!
//! Plain functions over `&Engine` (export) or nothing at all (create), a progress callback and a cancel
//! flag, so the tests run them without a Tauri runtime. Every job writes into a hidden staging folder next to
//! the destination and moves the finished files into place at the end: a cancelled or failed job leaves
//! nothing behind.
//!
//! Gaps against Acrobat, by design of the engine underneath: no ".doc"/".xlsx"/".pptx" or PostScript
//! output, and the Word and RTF exports are a flowing reconstruction (paragraphs, headings, simple tables,
//! images), not a layout-exact one.

mod create;
mod docx;
mod export;
mod flow;
mod rtf;
#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::Serialize;

pub use create::{create, discard_scratch, write_scratch_bytes, CreateOutput, CreateRequest, CreateSource, ImageInput, PageSize, TextInput};
pub use export::{export, export_document, export_new, ExportKind, ExportOutput, ExportRequest, ImageFormat, Target};

// ---------------------------------------------------------------- errors, progress

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Invalid(String),
    #[error("password required")]
    NeedsPassword,
    #[error("wrong password")]
    BadPassword,
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Mupdf(#[from] mupdf::Error),
    #[error("{0}")]
    Engine(#[from] crate::engine::EngineError),
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // The UI matches on these codes, like the other tools'.
        s.serialize_str(&match self {
            Error::Cancelled => "cancelled".to_string(),
            Error::NeedsPassword => "needs-password".to_string(),
            Error::BadPassword => "bad-password".to_string(),
            Error::Invalid(_) => "invalid-options".to_string(),
            other => other.to_string(),
        })
    }
}

impl From<Error> for crate::tools::ToolError {
    fn from(e: Error) -> Self {
        // `invalid-options` hides the reason the UI should show: keep it for those.
        let msg = match &e {
            Error::Invalid(m) => m.clone(),
            Error::Engine(crate::engine::EngineError::Stale) => "stale".to_string(),
            other => serde_json::to_value(other).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| other.to_string()),
        };
        crate::tools::ToolError(msg)
    }
}

/// One step of a job: `done` of `total` units (pages or images) finished.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    /// 0..=1 over the whole job.
    pub fraction: f32,
}

pub(crate) struct Run<'a> {
    pub cancel: &'a AtomicBool,
    pub progress: &'a mut dyn FnMut(Progress),
    pub total: u32,
}

impl Run<'_> {
    pub fn check(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Relaxed) { Err(Error::Cancelled) } else { Ok(()) }
    }

    /// `done` units are finished; the last tenth of the bar is the move into place.
    pub fn step(&mut self, done: u32) {
        let total = self.total.max(1);
        (self.progress)(Progress { done, total: self.total, fraction: 0.9 * done.min(total) as f32 / total as f32 });
    }
}

// ---------------------------------------------------------------- staging and publishing

/// A hidden folder next to the destination that holds a job's files until the job is done. Removed
/// (with whatever is left in it) when dropped.
pub(crate) struct Staging {
    dir: PathBuf,
}

impl Staging {
    pub fn new(dest_dir: &Path) -> Result<Self, Error> {
        use std::hash::{BuildHasher, Hasher};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        loop {
            let nonce = std::collections::hash_map::RandomState::new().build_hasher().finish();
            let dir = dest_dir.join(format!(".inkwell-convert-{}-{}-{nonce:08x}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self { dir }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Moves `(staged name, final path)` into place. Existing files at the final paths are replaced
    /// (the save dialog confirmed that, or the names were made unique first).
    pub fn publish(&self, files: &[(String, PathBuf)]) -> Result<(), Error> {
        for (staged, to) in files {
            if let Err(e) = fs::rename(self.path(staged), to) {
                // Already-moved files of a failed move are taken back out.
                for (_, done) in files.iter().take_while(|(s, _)| s != staged) {
                    let _ = fs::remove_file(done);
                }
                return Err(e.into());
            }
        }
        Ok(())
    }
}

impl Staging {
    /// [`Staging::publish`] that never replaces a file: a hard link (atomic) or, where the file system has no links,
    /// an exclusive create and a copy. A name that is taken meanwhile is an error (`output-exists`) and the files
    /// already moved are taken back out.
    pub fn publish_new(&self, files: &[(String, PathBuf)]) -> Result<(), Error> {
        for (staged, to) in files {
            if let Err(e) = link_new(&self.path(staged), to) {
                for (_, done) in files.iter().take_while(|(s, _)| s != staged) {
                    let _ = fs::remove_file(done);
                }
                return Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
                    std::io::Error::new(std::io::ErrorKind::AlreadyExists, "output-exists").into()
                } else {
                    e.into()
                });
            }
        }
        Ok(())
    }
}

/// `to` is made from `from` and never replaces a file that exists.
fn link_new(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::io::ErrorKind::AlreadyExists;
    match fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == AlreadyExists => Err(e),
        Err(_) => {
            let mut dst = fs::OpenOptions::new().write(true).create_new(true).open(to)?;
            let copied = fs::File::open(from).and_then(|mut src| std::io::copy(&mut src, &mut dst)).and_then(|_| dst.sync_all());
            if copied.is_err() {
                drop(dst);
                let _ = fs::remove_file(to);
            }
            copied
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// `stem` with characters Windows does not allow in a file name replaced by `_`.
pub(crate) fn safe_stem(stem: &str) -> String {
    let s: String = stem.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "document".into() } else { s }
}

/// The first `stem`, `stem (2)`, `stem (3)` ... for which none of `suffixes` exists in `dir`.
pub(crate) fn unique_stem(dir: &Path, stem: &str, suffixes: &[String]) -> String {
    let taken = |s: &str| suffixes.iter().any(|x| dir.join(format!("{s}{x}")).exists());
    if !taken(stem) {
        return stem.to_string();
    }
    (2..)
        .map(|n| format!("{stem} ({n})"))
        .find(|s| !taken(s))
        .unwrap_or_else(|| stem.to_string())
}

/// `1-3, 5` (one-based) to zero-based indices; an empty spec means every page.
pub(crate) fn pages_from(spec: Option<&str>, count: u32) -> Result<Vec<u32>, Error> {
    match spec.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok((0..count).collect()),
        Some(s) => {
            let v = crate::ocr::parse_page_range(s, count).map_err(|_| Error::Invalid(format!("bad page range: {s}")))?;
            if v.is_empty() { Err(Error::Invalid("no pages selected".into())) } else { Ok(v) }
        }
    }
}
