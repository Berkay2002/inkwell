//! Shared types of the history store (serde for the UI).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

macro_rules! hex_id {
    ($(#[$m:meta])* $name:ident, $n:expr) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub [u8; $n]);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                for b in self.0 {
                    write!(f, "{b:02x}")?;
                }
                Ok(())
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }
        impl FromStr for $name {
            type Err = HistoryError;
            fn from_str(s: &str) -> std::result::Result<Self, HistoryError> {
                let b = s.as_bytes();
                if b.len() != $n * 2 {
                    return Err(HistoryError::Corrupt(format!("bad id {s:?}")));
                }
                let mut out = [0u8; $n];
                for (i, o) in out.iter_mut().enumerate() {
                    let hx = std::str::from_utf8(&b[i * 2..i * 2 + 2]).ok();
                    *o = hx.and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or_else(|| HistoryError::Corrupt(format!("bad id {s:?}")))?;
                }
                Ok(Self(out))
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

hex_id!(
    /// A document's history (128-bit random).
    DocId, 16
);
hex_id!(
    /// A version: 48-bit ms time prefix + 80 random bits, so ids sort by time.
    VersionId, 16
);
hex_id!(
    /// Keyed BLAKE3 of a chunk's plaintext (key derived from the doc key).
    ChunkId, 32
);
hex_id!(
    /// Keyed BLAKE3 of some bytes (content, path, /ID).
    Hash, 32
);

impl VersionId {
    pub fn millis(&self) -> u64 {
        let mut b = [0u8; 8];
        b[2..].copy_from_slice(&self.0[..6]);
        u64::from_be_bytes(b)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Author {
    User,
    Assistant { turn_id: String },
    External,
    Autosave,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Kind {
    Save,
    Checkpoint,
    Autosave,
    External,
    Restore { from: VersionId },
    Import,
}

/// What the caller says about a version it commits.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NewVersion {
    pub author: Author,
    pub kind: Kind,
    pub label: String,
    #[serde(default)]
    pub summary: Vec<String>,
    /// User-named versions are kept forever.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub pages_touched: Vec<u32>,
}

/// Immutable, encrypted at rest. `name` is the name given at commit; later renames live in the
/// doc's `names` record (manifests are never rewritten).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Manifest {
    pub id: VersionId,
    pub parents: Vec<VersionId>,
    pub created: u64,
    pub author: Author,
    pub kind: Kind,
    pub label: String,
    pub summary: Vec<String>,
    pub name: Option<String>,
    pub pages_touched: Vec<u32>,
    pub size: u64,
    pub content_hash: Hash,
    pub chunks: Vec<(ChunkId, u32)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub id: VersionId,
    pub parents: Vec<VersionId>,
    pub created: u64,
    pub author: Author,
    pub kind: Kind,
    pub label: String,
    pub summary: Vec<String>,
    /// Current name (commit-time name or a later rename).
    pub name: Option<String>,
    pub pages_touched: Vec<u32>,
    pub size: u64,
    /// Some chunk files are missing: listed so the user sees it, but `read` fails.
    pub broken: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Resolution {
    Known { doc: DocId },
    /// Ask "same document?"; answer with `Store::confirm_same`, or `attach` for a new history.
    Ambiguous { candidates: Vec<DocId> },
    New,
    /// The user chose "Never keep history for this file".
    Excluded,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    /// Keep every version this many days.
    pub all_days: u32,
    /// Then one per day for this many months (30 days each); then one per month.
    pub daily_months: u32,
    /// 0 = no cap.
    pub per_doc_cap_bytes: u64,
    pub total_cap_bytes: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self { all_days: 30, daily_months: 6, per_doc_cap_bytes: 2 << 30, total_cap_bytes: 20 << 30 }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DocUsage {
    pub doc: Option<DocId>,
    pub bytes: u64,
    pub versions: u64,
    pub chunks: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub per_doc: Vec<DocUsage>,
    pub total: u64,
    pub chunks: u64,
    pub versions: u64,
    /// Docs that could not be opened (e.g. their key no longer unwraps): not counted above.
    pub skipped: Vec<DocId>,
}

/// Result of `Store::prune` / `Store::gc`.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Maintenance {
    pub pruned: usize,
    pub freed_bytes: u64,
    /// Docs that could not be opened (e.g. their key no longer unwraps): left untouched.
    pub skipped: Vec<DocId>,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsckReport {
    pub temp_files_removed: u32,
    pub wals_kept: u32,
    pub wals_rolled_back: u32,
    /// Manifests that fail to decrypt (moved aside to `*.bad`, never deleted).
    pub bad_manifests: u32,
    /// Versions listed as `broken` (chunks missing).
    pub broken_versions: u32,
    /// Docs that could not be opened (e.g. their key no longer unwraps): left untouched.
    pub skipped: Vec<DocId>,
}

/// Target of "Never keep history for this file".
#[derive(Clone, Debug)]
pub enum Target {
    Doc(DocId),
    Path(std::path::PathBuf),
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the history store is in use by another Inkwell window")]
    Locked,
    #[error("the history store moved to {0}")]
    Moved(String),
    #[error("history data failed its integrity check ({0})")]
    Integrity(String),
    #[error("history data is corrupt: {0}")]
    Corrupt(String),
    #[error("Windows could not protect or unprotect a history key")]
    Dpapi,
    #[error("no such document or version")]
    NotFound,
    #[error("history is turned off for this file")]
    NeverKeep,
    #[error("the file already exists")]
    Exists,
    /// Several histories may belong to the file: ask which one (or none) first.
    #[error("more than one history may belong to this file")]
    Ambiguous(Vec<DocId>),
    #[error("{0}")]
    Invalid(String),
}

impl From<serde_json::Error> for HistoryError {
    fn from(e: serde_json::Error) -> Self {
        HistoryError::Corrupt(e.to_string())
    }
}

impl Serialize for HistoryError {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

pub type Result<T> = std::result::Result<T, HistoryError>;
