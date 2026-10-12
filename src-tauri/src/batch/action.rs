//! The Action: what is stored, exported, imported, validated and run.
//!
//! An Action is a name, a description, ordered steps, an output rule and what to do when a file
//! fails. Its JSON is the wire format, the stored format (inside the versioned envelope of
//! `store.rs`) and the exported file format, so every field has a default and a name that stays.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use super::pages;
use super::steps::{Env, Problem, StepImpl};
use crate::compress::Preset;
use crate::convert::ImageFormat;
use crate::marks::{Bates, HeaderFooter, Watermark};
use crate::protect::{Changes, Cipher, Printing, ProtectSpec};
use crate::redact::{Pattern, Sanitize};
use crate::tools::BackendChoice;

/// Where the finished PDF goes. A result never replaces a file: when the name is taken, ` (2)`,
/// ` (3)`... is added.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OutputRule {
    /// `<name><suffix>.pdf` in the folder of the source.
    NextToSource { suffix: String },
    /// `<name><suffix>.pdf` in a folder the person picks when the run starts (a `pick_folder` token
    /// handed to `batch_run`; a token is good for one run, so the Action cannot hold it).
    Folder { suffix: String },
}

impl Default for OutputRule {
    fn default() -> Self {
        OutputRule::NextToSource { suffix: " (batch)".into() }
    }
}

impl OutputRule {
    pub fn suffix(&self) -> &str {
        match self {
            OutputRule::NextToSource { suffix } | OutputRule::Folder { suffix } => suffix,
        }
    }

    pub fn needs_folder(&self) -> bool {
        matches!(self, OutputRule::Folder { .. })
    }
}

/// What a file that fails does to the rest of the run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnError {
    /// Report it and go on with the next file.
    #[default]
    Skip,
    /// Report it; the files after it are not touched.
    Stop,
}

// ---------------------------------------------------------------- steps

/// Compress with one of the presets of the Compress tool.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompressStep {
    pub preset: Preset,
}

/// OCR the pages that have no text (the existing searchable-image path).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OcrStep {
    /// BCP-47 tags in priority order; empty = the engine's default.
    pub languages: Vec<String>,
    /// Read pages that already have text and replace an earlier layer of ours.
    pub force: bool,
    pub backend: BackendChoice,
}

/// Bates numbers, counted on from file to file (the default) or restarted in every file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BatesStep {
    pub bates: Bates,
    pub continue_across_files: bool,
}

impl Default for BatesStep {
    fn default() -> Self {
        Self { bates: Bates::default(), continue_across_files: true }
    }
}

/// A password: zeroed when dropped, never printed, never serialised.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Secret {
    fn from(s: &str) -> Self {
        Secret(s.to_string())
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Secret(s)
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("…")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Secret)
    }
}

/// Protect with passwords. **The passwords are never written**: a stored or exported Action holds
/// the rights and the cipher, and the person types the passwords when the run starts.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProtectStep {
    #[serde(skip_serializing)]
    pub open_password: Option<Secret>,
    #[serde(skip_serializing)]
    pub permissions_password: Option<Secret>,
    pub printing: Printing,
    pub changes: Changes,
    pub copy: bool,
    pub accessibility: bool,
    pub cipher: Cipher,
}

impl Default for ProtectStep {
    fn default() -> Self {
        Self {
            open_password: None,
            permissions_password: None,
            printing: Printing::default(),
            changes: Changes::default(),
            copy: true,
            accessibility: true,
            cipher: Cipher::default(),
        }
    }
}

impl ProtectStep {
    /// The engine's spec, normalised the way the Protect sheet's is.
    pub fn spec(&self) -> ProtectSpec {
        let pw = |s: &Option<Secret>| s.as_ref().map(|s| s.as_str().to_string()).filter(|p| !p.is_empty());
        let mut spec = ProtectSpec {
            open_password: pw(&self.open_password),
            permissions_password: pw(&self.permissions_password),
            printing: self.printing,
            changes: self.changes,
            copy: self.copy,
            accessibility: self.accessibility,
            cipher: self.cipher,
        };
        spec.normalize();
        spec
    }
}

/// One thing to find and redact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RedactQuery {
    /// A built-in shape: personnummer, SSN, IBAN, card number, email, phone number.
    Pattern { pattern: Pattern },
    Text {
        text: String,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
    },
    Regex {
        regex: String,
        #[serde(default)]
        case_sensitive: bool,
    },
}

/// Find, mark, apply and verify, in every file. A file with nothing to find is copied unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RedactStep {
    pub queries: Vec<RedactQuery>,
    /// Text drawn on each black box (a redaction code).
    pub overlay_text: Option<String>,
    pub sanitize: Sanitize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlattenFormsStep {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportImagesStep {
    pub format: ImageFormat,
    pub dpi: u32,
    pub gray: bool,
    /// 1..=100, JPEG only.
    pub quality: u8,
}

impl Default for ExportImagesStep {
    fn default() -> Self {
        Self { format: ImageFormat::Png, dpi: 150, gray: false, quality: 90 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportTextStep {
    /// Keep the columns and indentation of the page.
    pub layout: bool,
}

/// Document properties. `None` leaves an entry alone, an empty text removes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PropertiesStep {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
}

/// One step. The set is closed: a step that is not here (removing security, for one) is not an
/// Action step, and a file that names one parses into [`Step::Invalid`] so that validation can say so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Step {
    Compress(CompressStep),
    Ocr(OcrStep),
    HeaderFooter(HeaderFooter),
    Watermark(Watermark),
    Bates(BatesStep),
    Protect(ProtectStep),
    Sanitize(Sanitize),
    Redact(RedactStep),
    FlattenForms(FlattenFormsStep),
    ExportImages(ExportImagesStep),
    ExportText(ExportTextStep),
    SetProperties(PropertiesStep),
    /// A step this version does not know, or whose settings do not parse. Never produced by
    /// serialising a valid Action; `why` is for the person.
    #[serde(skip_deserializing)]
    Invalid { name: String, why: String },
}

impl Step {
    /// The implementation behind the step: what validation, the signature check and the runner ask.
    pub(super) fn imp(&self) -> &dyn StepImpl {
        match self {
            Step::Compress(s) => s,
            Step::Ocr(s) => s,
            Step::HeaderFooter(s) => s,
            Step::Watermark(s) => s,
            Step::Bates(s) => s,
            Step::Protect(s) => s,
            Step::Sanitize(s) => s,
            Step::Redact(s) => s,
            Step::FlattenForms(s) => s,
            Step::ExportImages(s) => s,
            Step::ExportText(s) => s,
            Step::SetProperties(s) => s,
            Step::Invalid { .. } => &super::steps::InvalidStep,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Step::Invalid { name, .. } => format!("Unknown step \"{name}\""),
            s => s.imp().label(),
        }
    }

    /// The step rewrites the file, so a signed file would no longer validate.
    pub fn breaks_signature(&self) -> bool {
        self.imp().breaks_signature()
    }

    /// The step works page by page, so it can be given a page range. The others (compress, protect, set
    /// properties, sanitize, the exports) act on the file as a whole.
    pub fn takes_pages(&self) -> bool {
        matches!(self, Step::HeaderFooter(_) | Step::Watermark(_) | Step::Bates(_) | Step::Redact(_) | Step::Ocr(_) | Step::FlattenForms(_))
    }
}

/// A step as the Action holds it: the step, the pages it works on, and whether the run stops before it to
/// ask. In JSON the two extras sit beside the step's own fields (`{"kind":"watermark","pages":"1-3","prompt":true,...}`)
/// and are left out when they are at their defaults, so an action that uses neither is the file it always was.
#[derive(Clone, Debug, PartialEq)]
pub struct StepEntry {
    pub step: Step,
    /// Page range text (`1-3, 5, 8-`, one-based); empty = every page. Only for steps that [`Step::takes_pages`].
    pub pages: String,
    /// "Prompt user": a run shows the step's options for each file and waits for the answer.
    pub prompt: bool,
}

impl From<Step> for StepEntry {
    fn from(step: Step) -> Self {
        Self { step, pages: String::new(), prompt: false }
    }
}

impl std::ops::Deref for StepEntry {
    type Target = Step;
    fn deref(&self) -> &Step {
        &self.step
    }
}

impl PartialEq<Step> for StepEntry {
    fn eq(&self, other: &Step) -> bool {
        self.pages.is_empty() && !self.prompt && self.step == *other
    }
}

impl StepEntry {
    /// The step's label, with its page range when it has one.
    pub fn label(&self) -> String {
        match self.pages.trim() {
            "" => self.step.label(),
            p => format!("{} (pages {p})", self.step.label()),
        }
    }

    pub fn has_range(&self) -> bool {
        !self.pages.trim().is_empty()
    }
}

impl Serialize for StepEntry {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut v = serde_json::to_value(&self.step).map_err(serde::ser::Error::custom)?;
        if let Some(o) = v.as_object_mut() {
            if !self.pages.is_empty() {
                o.insert("pages".into(), self.pages.clone().into());
            }
            if self.prompt {
                o.insert("prompt".into(), true.into());
            }
        }
        v.serialize(s)
    }
}

impl<'de> Deserialize<'de> for StepEntry {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde_json::Value::deserialize(d).map(parse_entry)
    }
}

/// Reads one step, keeping what it cannot read as [`Step::Invalid`] instead of failing the whole file.
pub(super) fn parse_entry(mut v: serde_json::Value) -> StepEntry {
    let (mut pages, mut prompt, mut bad) = (String::new(), false, None);
    if let Some(o) = v.as_object_mut() {
        match o.remove("pages") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(s)) => pages = s,
            Some(_) => bad = Some("The page range of a step has to be text."),
        }
        match o.remove("prompt") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Bool(b)) => prompt = b,
            Some(_) => bad = Some("The \"ask first\" setting of a step has to be true or false."),
        }
    }
    let mut step = parse_step(v);
    if let (Some(why), false) = (bad, matches!(step, Step::Invalid { .. })) {
        let name = serde_json::to_value(&step).ok().and_then(|v| v.get("kind").and_then(|k| k.as_str().map(String::from))).unwrap_or_default();
        step = Step::Invalid { name, why: why.into() };
    }
    StepEntry { step, pages, prompt }
}

pub(super) fn parse_step(v: serde_json::Value) -> Step {
    let name = v.get("kind").and_then(|k| k.as_str()).unwrap_or("").to_string();
    match serde_json::from_value::<Step>(v) {
        Ok(s) => s,
        Err(e) => {
            let known = KINDS.contains(&name.as_str());
            let why = if is_remove_security(&name) {
                "Removing security is not allowed in a batch action.".to_string()
            } else if known {
                format!("The settings of {name} are not valid: {e}")
            } else if name.is_empty() {
                "A step needs a kind.".to_string()
            } else {
                format!("\"{name}\" is not a step this version of Inkwell knows.")
            };
            Step::Invalid { name, why }
        }
    }
}

const KINDS: [&str; 12] = [
    "compress", "ocr", "headerFooter", "watermark", "bates", "protect", "sanitize", "redact", "flattenForms", "exportImages",
    "exportText", "setProperties",
];

fn is_remove_security(name: &str) -> bool {
    let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    matches!(n.as_str(), "removesecurity" | "unprotect" | "decrypt" | "removepassword" | "removepasswords")
}

// ---------------------------------------------------------------- the action

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Action {
    /// `[a-z0-9-]`, up to 64 characters; the file name of the stored Action. Empty = pick one when saved.
    pub id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<StepEntry>,
    pub output: OutputRule,
    pub on_error: OnError,
    /// A built-in Action: read-only; Duplicate it to change it. Never read from a file.
    #[serde(skip_deserializing)]
    pub built_in: bool,
}

/// Why an Action cannot run (or be saved). `step` is the index of the step, none for the Action itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub step: Option<usize>,
    /// `name`, `no-steps`, `output`, `order`, `unknown-step`, `forbidden-step`, `invalid-step`, `unavailable`.
    pub code: &'static str,
    pub message: String,
}

pub const MAX_NAME: usize = 80;

/// A file name part is safe to put after a source's name: no separators, no controls, not long.
fn suffix_problem(s: &str) -> Option<&'static str> {
    if s.chars().count() > 40 {
        Some("The ending of the file name is too long (40 characters at most).")
    } else if s.chars().any(|c| c.is_control() || r#"<>:"/\|?*"#.contains(c)) {
        Some("The ending of the file name cannot hold < > : \" / \\ | ? * or control characters.")
    } else {
        None
    }
}

impl Action {
    /// Everything that keeps this Action from running: empty when it can. Steps that are
    /// unavailable on this machine (OCR without an engine) are reported here too, so a run never
    /// fails on them halfway.
    pub fn validate(&self, env: &Env) -> Vec<Issue> {
        let mut out = Vec::new();
        let mut issue = |step: Option<usize>, code: &'static str, message: String| out.push(Issue { step, code, message });
        let name = self.name.trim();
        if name.is_empty() {
            issue(None, "name", "Give the action a name.".into());
        } else if name.chars().count() > MAX_NAME {
            issue(None, "name", format!("The name is too long ({MAX_NAME} characters at most)."));
        }
        if self.steps.is_empty() {
            issue(None, "no-steps", "Add at least one step.".into());
        }
        let suffix = self.output.suffix();
        if let Some(p) = suffix_problem(suffix) {
            issue(None, "output", p.into());
        } else if suffix.trim().is_empty() && !self.output.needs_folder() {
            issue(
                None,
                "output",
                "Give the copies an ending (for example \" (email)\"): a result never replaces its source.".into(),
            );
        }
        for (i, entry) in self.steps.iter().enumerate() {
            match &entry.step {
                Step::Invalid { name, why } => {
                    let code = if is_remove_security(name) { "forbidden-step" } else { "unknown-step" };
                    issue(Some(i), code, why.clone());
                }
                s => {
                    match s.imp().validate(env) {
                        Ok(()) => {}
                        Err(Problem::Invalid(m)) => issue(Some(i), "invalid-step", m),
                        Err(Problem::Unavailable(m)) => issue(Some(i), "unavailable", m),
                    }
                    if entry.has_range() {
                        if !s.takes_pages() {
                            issue(Some(i), "invalid-step", "This step works on the whole file: clear its page range.".into());
                        } else if let Err(m) = pages::check(&entry.pages) {
                            issue(Some(i), "invalid-step", m);
                        }
                    }
                }
            }
        }
        // Protecting writes an encrypted file: whatever comes after could not read it.
        if let Some(p) = self.steps.iter().position(|s| matches!(s.step, Step::Protect(_)))
            && p + 1 != self.steps.len()
        {
            issue(Some(p), "order", "Protect with a password has to be the last step.".into());
        }
        let protects = self.steps.iter().filter(|s| matches!(s.step, Step::Protect(_))).count();
        if protects > 1 {
            issue(None, "order", "Use Protect with a password once.".into());
        }
        // One Bates numbering per file: a second would number over the first.
        let mut seen = HashSet::new();
        for (i, s) in self.steps.iter().enumerate() {
            let dup = match &s.step {
                Step::HeaderFooter(_) => "header and footer",
                Step::Watermark(_) => "watermark",
                Step::Bates(_) => "Bates numbering",
                _ => continue,
            };
            if !seen.insert(dup) {
                issue(Some(i), "order", format!("A second {dup} step would replace the first one."));
            }
        }
        out
    }

    /// True when the Action produces a PDF (it has a step other than the exports).
    pub fn writes_pdf(&self) -> bool {
        self.steps.iter().any(|s| !matches!(s.step, Step::ExportImages(_) | Step::ExportText(_)))
    }

    /// The first step that would invalidate a signature, if any.
    pub fn first_signature_breaker(&self) -> Option<&Step> {
        self.steps.iter().map(|s| &s.step).find(|s| s.breaks_signature())
    }
}
