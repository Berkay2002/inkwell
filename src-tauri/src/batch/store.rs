//! Actions on disk: one JSON file per Action in a folder of the app data directory, plus the built-ins
//! that live in code. The same envelope is the import/export file.
//!
//! ```json
//! { "format": "inkwell-batch-action", "version": 1, "action": { "name": "...", "steps": [ ... ] } }
//! ```
//!
//! `version` only goes up. A file with a newer one is refused (it may hold steps this build cannot run);
//! an older one is brought forward by [`migrate`] before it is read. Passwords of a Protect step are
//! never written (see `ProtectStep`).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::action::{Action, MAX_NAME, OnError, OutputRule, Step};
use crate::compress::Preset;
use crate::marks::{Style, Watermark};
use crate::redact::{Pattern, Sanitize};
use crate::tools::BackendChoice;

pub const FORMAT: &str = "inkwell-batch-action";
/// The schema this build writes and reads without migrating.
pub const VERSION: u32 = 1;
/// An imported file bigger than this is not an Action.
pub const MAX_FILE: u64 = 1 << 20;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StoreError {
    #[error("{0}")]
    Invalid(String),
    #[error("This action was made by a newer Inkwell (format {0}). Update Inkwell to use it.")]
    TooNew(u32),
    #[error("There is no such action.")]
    NotFound,
    #[error("Built-in actions cannot be changed. Duplicate it first.")]
    BuiltIn,
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e.to_string())
    }
}

type Result<T> = std::result::Result<T, StoreError>;

// ---------------------------------------------------------------- the file format

/// The envelope for `action`, as the bytes of a file.
pub fn to_file(action: &Action) -> Result<Vec<u8>> {
    let mut a = action.clone();
    a.built_in = false;
    let v = json!({ "format": FORMAT, "version": VERSION, "action": a });
    serde_json::to_vec_pretty(&v).map_err(|e| StoreError::Io(e.to_string()))
}

/// Reads an envelope. The Action keeps whatever steps do not parse as [`Step::Invalid`] (validation
/// names them); it is not otherwise checked.
pub fn from_file(bytes: &[u8]) -> Result<Action> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(StoreError::Invalid("That file is too big to be an action.".into()));
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|_| StoreError::Invalid("That file is not an Inkwell action.".into()))?;
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(StoreError::Invalid("That file is not an Inkwell action.".into()));
    }
    let version = v.get("version").and_then(Value::as_u64).filter(|&n| n >= 1 && n <= u64::from(u32::MAX));
    let Some(version) = version.map(|n| n as u32) else {
        return Err(StoreError::Invalid("That action file has no format version.".into()));
    };
    if version > VERSION {
        return Err(StoreError::TooNew(version));
    }
    let v = migrate(v, version);
    let action = v.get("action").cloned().ok_or_else(|| StoreError::Invalid("That action file holds no action.".into()))?;
    let mut a: Action =
        serde_json::from_value(action).map_err(|e| StoreError::Invalid(format!("That action could not be read: {e}")))?;
    a.built_in = false;
    Ok(a)
}

/// Brings an envelope written as version `from` up to [`VERSION`]. Version 1 is the first, so there is
/// nothing to do yet; the next schema change adds a step for the old version here (and a test with a
/// file of the old shape) instead of breaking the files people have saved.
fn migrate(v: Value, _from: u32) -> Value {
    v
}

// ---------------------------------------------------------------- ids

pub fn id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0] != b'-' && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn new_id() -> String {
    let mut b = [0u8; 6];
    getrandom::fill(&mut b).expect("system random source");
    format!("a-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

// ---------------------------------------------------------------- the store

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn file(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// The built-ins first, then the saved ones by name. A file that cannot be read is left out (it is
    /// not deleted: a newer Inkwell may understand it).
    pub fn list(&self) -> Vec<Action> {
        let mut saved: Vec<Action> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let id = e.path().file_stem()?.to_string_lossy().into_owned();
                let mut a = self.read(&e.path()).ok()?;
                a.id = id;
                Some(a)
            })
            .filter(|a| id_ok(&a.id) && !builtins().iter().any(|b| b.id == a.id))
            .collect();
        saved.sort_by_key(|a| a.name.to_lowercase());
        let mut all = builtins();
        all.extend(saved);
        all
    }

    fn read(&self, path: &Path) -> Result<Action> {
        if fs::metadata(path)?.len() > MAX_FILE {
            return Err(StoreError::Invalid("too big".into()));
        }
        from_file(&fs::read(path)?)
    }

    pub fn get(&self, id: &str) -> Result<Action> {
        if let Some(b) = builtins().into_iter().find(|b| b.id == id) {
            return Ok(b);
        }
        if !id_ok(id) {
            return Err(StoreError::NotFound);
        }
        let path = self.file(id);
        if !path.is_file() {
            return Err(StoreError::NotFound);
        }
        let mut a = self.read(&path)?;
        a.id = id.to_string();
        Ok(a)
    }

    /// Saves `action` (a new id when it has none) and returns it as stored. Needs a name and steps that
    /// parse; the rest is for `validate`, so a draft can be kept.
    pub fn save(&self, mut action: Action) -> Result<Action> {
        let name = action.name.trim().to_string();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(StoreError::Invalid(format!("Give the action a name of 1 to {MAX_NAME} characters.")));
        }
        action.name = name;
        if let Some(Step::Invalid { why, .. }) = action.steps.iter().find(|s| matches!(s, Step::Invalid { .. })) {
            return Err(StoreError::Invalid(why.clone()));
        }
        if action.id.is_empty() {
            action.id = new_id();
        }
        if builtins().iter().any(|b| b.id == action.id) {
            return Err(StoreError::BuiltIn);
        }
        if !id_ok(&action.id) {
            return Err(StoreError::Invalid("That action id is not valid.".into()));
        }
        action.built_in = false;
        let bytes = to_file(&action)?;
        fs::create_dir_all(&self.dir)?;
        write_file_atomic(&self.file(&action.id), &bytes)?;
        Ok(action)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        if builtins().iter().any(|b| b.id == id) {
            return Err(StoreError::BuiltIn);
        }
        if !id_ok(id) {
            return Err(StoreError::NotFound);
        }
        match fs::remove_file(self.file(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StoreError::NotFound),
            Err(e) => Err(e.into()),
        }
    }

    /// Reads an action file and stores it under a new id (an import never replaces an action).
    pub fn import(&self, bytes: &[u8]) -> Result<Action> {
        let mut a = from_file(bytes)?;
        a.id = String::new();
        a.name = a.name.trim().to_string();
        if self.list().iter().any(|x| x.name == a.name) {
            const TAG: &str = " (imported)";
            let room = MAX_NAME - TAG.chars().count();
            a.name = format!("{}{TAG}", a.name.chars().take(room).collect::<String>().trim_end());
        }
        self.save(a)
    }
}

/// Writes `bytes` to `path` through a temp file with a name nobody has (made with `create_new`), then a
/// rename: a half-written file never shows, and no other file is ever the temp file.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::{ErrorKind::AlreadyExists, Write};
    let (tmp, mut file) = loop {
        let mut r = [0u8; 8];
        getrandom::fill(&mut r).expect("system random source");
        let tmp = path.with_file_name(format!(".inkwell-action-{}.tmp", r.iter().map(|b| format!("{b:02x}")).collect::<String>()));
        match fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    written.and_then(|()| fs::rename(&tmp, path)).inspect_err(|_| drop(fs::remove_file(&tmp)))
}

// ---------------------------------------------------------------- built-in actions

fn builtin(id: &str, name: &str, description: &str, steps: Vec<Step>, suffix: &str) -> Action {
    Action {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        steps,
        output: OutputRule::NextToSource { suffix: suffix.into() },
        on_error: OnError::Skip,
        built_in: true,
    }
}

/// The four Actions that come with the app.
pub fn builtins() -> Vec<Action> {
    use super::action::*;
    vec![
        builtin(
            "builtin-prepare-for-email",
            "Prepare for email",
            "Removes metadata and scripts, then makes the file smaller.",
            vec![
                Step::Sanitize(Sanitize { metadata: true, scripts_actions: true, ..Sanitize::NONE }),
                Step::Compress(CompressStep { preset: Preset::Balanced }),
            ],
            " (email)",
        ),
        builtin(
            "builtin-make-searchable",
            "Make searchable",
            "Recognizes the text of scanned pages so they can be searched and copied.",
            vec![Step::Ocr(OcrStep { languages: Vec::new(), force: false, backend: BackendChoice::default() })],
            " (searchable)",
        ),
        builtin(
            "builtin-publish-with-watermark",
            "Publish with watermark",
            "Flattens form fields, stamps a diagonal CONFIDENTIAL watermark and compresses.",
            vec![
                Step::FlattenForms(FlattenFormsStep {}),
                Step::Watermark(Watermark {
                    text: "CONFIDENTIAL".into(),
                    style: Style { size: 0.0, color: [0.75, 0.1, 0.1], bold: true, ..Style::default() },
                    opacity: 0.25,
                    ..Watermark::default()
                }),
                Step::Compress(CompressStep { preset: Preset::HighQuality }),
            ],
            " (published)",
        ),
        builtin(
            "builtin-redact-emails-and-phones",
            "Redact emails and phone numbers",
            "Finds email addresses and phone numbers, removes them for good and checks the result.",
            vec![Step::Redact(RedactStep {
                queries: vec![RedactQuery::Pattern { pattern: Pattern::Email }, RedactQuery::Pattern { pattern: Pattern::Phone }],
                overlay_text: None,
                // Everything that could quote the text, but not the person's own comments and bookmarks.
                sanitize: Sanitize {
                    metadata: true,
                    hidden_layers: true,
                    scripts_actions: true,
                    embedded_files: true,
                    form_fields_under_marks: true,
                    ..Sanitize::NONE
                },
            })],
            " (redacted)",
        ),
    ]
}
