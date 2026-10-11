// Adapted from pdfcraft (MIT OR Apache-2.0), crates/preflight/src/lib.rs @ b8ddf71
//! Preflight (Acrobat's "PDF Standards" tool): checks a document against PDF/A-1b, -2b, -3b, -2u,
//! PDF/UA-1 and PDF/X-4, reports per rule with the page and object and a fix where it is safe,
//! predicts what a conversion would do, and converts: to a new file, or as one undo step in place.
//!
//! The rules decide from the object graph; this is not veraPDF. What is checked and what is not
//! is in docs/pdfa.md. PDF/UA reuses the accessibility checker (`crate::a11y`).

use crate::a11y::locate::Box4;
use crate::a11y::{self, Progress};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, DocSource, Engine, EngineError};
use mupdf::pdf::PdfDocument;
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;

pub mod app;
mod check;
mod fix;
pub mod fonts;
pub mod icc;
pub mod report;
#[cfg(test)]
mod tests;
pub mod xmp;

pub type Result<T> = std::result::Result<T, EngineError>;

pub(crate) fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

// ---- profiles ------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Profile {
    #[serde(rename = "pdfa-1b")]
    Pdfa1b,
    #[serde(rename = "pdfa-2b")]
    Pdfa2b,
    #[serde(rename = "pdfa-3b")]
    Pdfa3b,
    #[serde(rename = "pdfa-2u")]
    Pdfa2u,
    #[serde(rename = "pdfua-1")]
    Pdfua1,
    #[serde(rename = "pdfx-4")]
    Pdfx4,
}

/// The rule families: PDF/A-1 differs from PDF/A-2 and -3 in many rules, which share theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    A1,
    A23,
    Ua,
    X,
}

impl Profile {
    pub const ALL: [Profile; 6] = [Profile::Pdfa1b, Profile::Pdfa2b, Profile::Pdfa3b, Profile::Pdfa2u, Profile::Pdfua1, Profile::Pdfx4];

    pub fn label(self) -> &'static str {
        match self {
            Profile::Pdfa1b => "PDF/A-1b",
            Profile::Pdfa2b => "PDF/A-2b",
            Profile::Pdfa3b => "PDF/A-3b",
            Profile::Pdfa2u => "PDF/A-2u",
            Profile::Pdfua1 => "PDF/UA-1",
            Profile::Pdfx4 => "PDF/X-4",
        }
    }

    pub fn family(self) -> Family {
        match self {
            Profile::Pdfa1b => Family::A1,
            Profile::Pdfa2b | Profile::Pdfa3b | Profile::Pdfa2u => Family::A23,
            Profile::Pdfua1 => Family::Ua,
            Profile::Pdfx4 => Family::X,
        }
    }

    /// `(part, conformance)` for the PDF/A profiles.
    pub fn pdfa(self) -> Option<(u8, char)> {
        match self {
            Profile::Pdfa1b => Some((1, 'B')),
            Profile::Pdfa2b => Some((2, 'B')),
            Profile::Pdfa3b => Some((3, 'B')),
            Profile::Pdfa2u => Some((2, 'U')),
            _ => None,
        }
    }

    pub fn is_pdfa(self) -> bool {
        self.pdfa().is_some()
    }
}

// ---- rules ---------------------------------------------------------------------------------------

/// The rules, in report order. Serialised as their ids in the report (`rule`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    FileHeader,
    TrailerId,
    Encryption,
    Lzw,
    ExternalStreams,
    ObjectStreams,
    OptionalContent,
    PageBoxes,
    XmpPresent,
    XmpId,
    XmpSync,
    XmpUnfiltered,
    InfoDates,
    Trapped,
    OutputIntent,
    OutputIntentProfile,
    DeviceColour,
    RgbContent,
    FontsEmbedded,
    FontsCidToGid,
    FontsUnicode,
    ImageInterpolate,
    ImageAlternates,
    ImageJpx,
    PsXObject,
    Transparency,
    TransferFunctions,
    ActionsForbidden,
    JavaScript,
    ActionsAdditional,
    AnnotationTypes,
    AnnotationFlags,
    AnnotationAppearance,
    NeedAppearances,
    Xfa,
    EmbeddedFiles,
}

/// What the profile families do with a rule.
struct Def {
    id: &'static str,
    name: &'static str,
    group: &'static str,
    hint: &'static str,
    fix: Option<FixGroup>,
    /// Clauses (ISO 19005-1, ISO 19005-2/-3, ISO 15930-7), empty where the rule is not in that family.
    a1: &'static str,
    a23: &'static str,
    x: &'static str,
    ua: &'static str,
}

impl Rule {
    pub const ALL: [Rule; 36] = [
        Rule::FileHeader,
        Rule::TrailerId,
        Rule::Encryption,
        Rule::Lzw,
        Rule::ExternalStreams,
        Rule::ObjectStreams,
        Rule::OptionalContent,
        Rule::PageBoxes,
        Rule::XmpPresent,
        Rule::XmpId,
        Rule::XmpSync,
        Rule::XmpUnfiltered,
        Rule::InfoDates,
        Rule::Trapped,
        Rule::OutputIntent,
        Rule::OutputIntentProfile,
        Rule::DeviceColour,
        Rule::RgbContent,
        Rule::FontsEmbedded,
        Rule::FontsCidToGid,
        Rule::FontsUnicode,
        Rule::ImageInterpolate,
        Rule::ImageAlternates,
        Rule::ImageJpx,
        Rule::PsXObject,
        Rule::Transparency,
        Rule::TransferFunctions,
        Rule::ActionsForbidden,
        Rule::JavaScript,
        Rule::ActionsAdditional,
        Rule::AnnotationTypes,
        Rule::AnnotationFlags,
        Rule::AnnotationAppearance,
        Rule::NeedAppearances,
        Rule::Xfa,
        Rule::EmbeddedFiles,
    ];

    fn def(self) -> Def {
        use FixGroup as G;
        let d = |id, name, group, hint, fix, a1, a23, x, ua| Def { id, name, group, hint, fix, a1, a23, x, ua };
        match self {
            Rule::FileHeader => d(
                "file-header",
                "File header",
                "File",
                "The file must start with %PDF-1.n and a comment of four or more bytes above 127 on the second line. Saving as a new file rewrites the header.",
                Some(G::Structure),
                "6.1.2",
                "6.1.2",
                "",
                "",
            ),
            Rule::TrailerId => d("trailer-id", "File identifier", "File", "The trailer needs an /ID of two strings.", Some(G::Structure), "6.1.3", "6.1.3", "6.1.? (ID)", ""),
            Rule::Encryption => d(
                "encryption",
                "No encryption",
                "File",
                "The standard forbids encryption. Save as a new file to write the document without it.",
                Some(G::Structure),
                "6.1.3",
                "6.1.4",
                "",
                "",
            ),
            Rule::Lzw => d("lzw", "No LZW compression", "File", "LZW streams are re-encoded without compression and compressed again when the file is written.", Some(G::Structure), "6.1.10", "6.1.10", "", ""),
            Rule::ExternalStreams => d("external-streams", "No external streams", "File", "A stream refers to a file outside the document (F key). Nothing can fix it automatically.", None, "6.1.7", "6.1.7", "", ""),
            Rule::ObjectStreams => d(
                "object-streams",
                "No object or xref streams",
                "File",
                "PDF/A-1 is built on PDF 1.4, which has neither. Save as a new file to write the classic structure.",
                Some(G::Structure),
                "6.1.4",
                "",
                "",
                "",
            ),
            Rule::OptionalContent => d(
                "optional-content",
                "Layers (optional content)",
                "File",
                "PDF/A-1 has no layers; PDF/A-2 and -3 allow them without automatic state (/AS).",
                Some(G::Structure),
                "6.1.13",
                "6.9",
                "",
                "",
            ),
            Rule::PageBoxes => d("page-boxes", "Trim and art boxes", "File", "Every page needs a TrimBox or ArtBox inside its MediaBox. The fix uses the page's crop box.", Some(G::Structure), "", "", "6.? (boxes)", ""),
            Rule::XmpPresent => d("xmp-present", "XMP metadata", "Metadata", "The document needs an XMP metadata stream in its catalog.", Some(G::Metadata), "6.7.2", "6.6.2.1", "XMP", "XMP"),
            Rule::XmpId => d(
                "xmp-id",
                "Standard identification",
                "Metadata",
                "The XMP metadata must say which part and level the file conforms to.",
                Some(G::Metadata),
                "6.7.11",
                "6.6.4",
                "pdfxid",
                "pdfuaid",
            ),
            Rule::XmpSync => d(
                "xmp-sync",
                "Metadata matches document information",
                "Metadata",
                "Title, author, subject, keywords, producer, creator and dates in the XMP must equal the document information.",
                Some(G::Metadata),
                "6.7.3",
                "6.6.2.3",
                "XMP",
                "dc:title",
            ),
            Rule::XmpUnfiltered => d("xmp-unfiltered", "XMP stream not compressed", "Metadata", "The metadata stream must not use a filter, so that other tools can read it.", Some(G::Metadata), "6.7.2", "6.6.2.1", "", ""),
            Rule::InfoDates => d("info-dates", "Creation and modification dates", "Metadata", "The document information needs CreationDate and ModDate.", Some(G::Metadata), "", "", "Info dates", ""),
            Rule::Trapped => d("trapped", "Trapping status", "Metadata", "The document information needs Trapped (/True or /False).", Some(G::Metadata), "", "", "Trapped", ""),
            Rule::OutputIntent => d(
                "output-intent",
                "Output intent",
                "Colour",
                "Device colour needs an output intent with an ICC profile. An sRGB one is added when only RGB and gray are used; CMYK needs the print condition you choose.",
                Some(G::Colour),
                "6.2.2",
                "6.2.3",
                "GTS_PDFX",
                "",
            ),
            Rule::OutputIntentProfile => d(
                "output-intent-profile",
                "Output intent profile",
                "Colour",
                "The output intent's destination profile must be a valid ICC profile (and the same one in every output intent).",
                None,
                "6.2.2",
                "6.2.3",
                "GTS_PDFX",
                "",
            ),
            Rule::DeviceColour => d(
                "device-colour",
                "Device colour matches the output intent",
                "Colour",
                "DeviceRGB needs an RGB output intent and DeviceCMYK a CMYK one (or a Default colour space). Nothing converts colour automatically.",
                None,
                "6.2.3.3",
                "6.2.4.3",
                "",
                "",
            ),
            Rule::RgbContent => d("rgb-content", "RGB content", "Colour", "RGB colour is used. PDF/X-4 allows it with an output intent; a press may want it converted.", None, "", "", "info", ""),
            Rule::FontsEmbedded => d(
                "fonts-embedded",
                "Fonts embedded",
                "Fonts",
                "Every font must be embedded. The fix embeds the built-in base-14 clones and installed Windows fonts; a font that is not available stays unfixed and blocks the conversion.",
                Some(G::Fonts),
                "6.3.4",
                "6.2.11.4.1",
                "fonts",
                "7.21.4.1",
            ),
            Rule::FontsCidToGid => d(
                "fonts-cidtogid",
                "CIDToGIDMap present",
                "Fonts",
                "An embedded CIDFontType2 needs a CIDToGIDMap. The fix adds /Identity, which is what its absence already means.",
                Some(G::Fonts),
                "6.3.? (CID)",
                "6.2.11.3.2",
                "",
                "",
            ),
            Rule::FontsUnicode => d(
                "fonts-unicode",
                "Text maps to Unicode",
                "Fonts",
                "PDF/A-2u needs every font to map its codes to Unicode (ToUnicode, or a standard encoding). Nothing repairs it automatically.",
                None,
                "",
                "6.2.11.7",
                "",
                "",
            ),
            Rule::ImageInterpolate => d("image-interpolate", "No image interpolation", "Images", "Image Interpolate is turned off.", Some(G::Images), "6.2.4", "6.2.8", "", ""),
            Rule::ImageAlternates => d("image-alternates", "No image alternates or OPI", "Images", "Alternates and OPI entries are removed from images.", Some(G::Images), "6.2.4", "6.2.8", "OPI", ""),
            Rule::ImageJpx => d("image-jpx", "No JPEG 2000 images", "Images", "PDF/A-1 does not allow JPXDecode. Nothing re-encodes images automatically.", None, "6.2.4", "", "", ""),
            Rule::PsXObject => d("ps-xobject", "No PostScript XObjects", "Images", "PostScript XObjects are not allowed and cannot be converted automatically.", None, "6.2.7", "6.2.7", "", ""),
            Rule::Transparency => d(
                "transparency",
                "No transparency",
                "Appearance",
                "PDF/A-1 does not allow transparency. The fix makes everything opaque, which changes how pages look where it was used.",
                Some(G::Appearance),
                "6.4",
                "",
                "",
                "",
            ),
            Rule::TransferFunctions => d(
                "transfer-functions",
                "No transfer functions",
                "Appearance",
                "Transfer functions (TR) are not allowed; they are removed, which changes how pages look where they were not the identity.",
                Some(G::Appearance),
                "6.2.9",
                "6.2.5",
                "",
                "",
            ),
            Rule::ActionsForbidden => d(
                "actions-forbidden",
                "Forbidden actions",
                "Actions and annotations",
                "Launch, Sound, Movie, ResetForm, ImportData, Hide, SetOCGState, Rendition, Trans, GoTo3DView, JavaScript and unsupported Named actions are removed.",
                Some(G::Actions),
                "6.6.1",
                "6.5.1",
                "",
                "",
            ),
            Rule::JavaScript => d("javascript", "Document JavaScript", "Actions and annotations", "The document-level JavaScript name tree is removed.", Some(G::Actions), "6.6.1", "6.5.1", "", ""),
            Rule::ActionsAdditional => d(
                "actions-additional",
                "Additional actions",
                "Actions and annotations",
                "Additional-actions (AA) entries on the catalog, pages, annotations and fields are removed.",
                Some(G::Actions),
                "6.6.2",
                "6.5.2",
                "",
                "",
            ),
            Rule::AnnotationTypes => d(
                "annotation-types",
                "Annotation types",
                "Actions and annotations",
                "Sound, movie, screen, 3D and rich media annotations (and file attachments in PDF/A-1) are removed.",
                Some(G::Annotations),
                "6.5.2",
                "6.3.1",
                "",
                "",
            ),
            Rule::AnnotationFlags => d(
                "annotation-flags",
                "Annotation flags",
                "Actions and annotations",
                "Annotations must print and must not be hidden. The fix sets the Print flag and clears Hidden, Invisible and NoView.",
                Some(G::Annotations),
                "6.5.3",
                "6.3.2",
                "",
                "",
            ),
            Rule::AnnotationAppearance => d(
                "annotation-appearance",
                "Annotation appearances",
                "Actions and annotations",
                "Every annotation except links and popups needs an appearance stream. Form fields get theirs from the Forms fix.",
                Some(G::Forms),
                "6.5.3",
                "6.3.3",
                "",
                "",
            ),
            Rule::NeedAppearances => d(
                "need-appearances",
                "Form appearances drawn",
                "Forms",
                "NeedAppearances asks the viewer to draw fields. The fix draws them into the file and clears it.",
                Some(G::Forms),
                "6.9",
                "6.4.1",
                "",
                "",
            ),
            Rule::Xfa => d("xfa", "No XFA forms", "Forms", "XFA form data is removed. A dynamic form may have nothing left to show but its AcroForm fallback.", Some(G::Forms), "6.9", "6.4.2", "", ""),
            Rule::EmbeddedFiles => d(
                "embedded-files",
                "Embedded files",
                "Embedded files",
                "PDF/A-1 allows none; PDF/A-2 only PDF/A files; PDF/A-3 any file with an AFRelationship. Removing them loses the attached data.",
                Some(G::Files),
                "6.1.11",
                "6.8",
                "",
                "",
            ),
        }
    }

    pub fn id(self) -> &'static str {
        self.def().id
    }

    /// Whether the rule belongs to the profile.
    pub fn applies(self, p: Profile) -> bool {
        let d = self.def();
        match p.family() {
            Family::A1 => !d.a1.is_empty(),
            Family::A23 => {
                if self == Rule::FontsUnicode {
                    return p == Profile::Pdfa2u;
                }
                !d.a23.is_empty()
            }
            Family::X => !d.x.is_empty(),
            Family::Ua => !d.ua.is_empty(),
        }
    }

    pub fn clause(self, p: Profile) -> &'static str {
        let d = self.def();
        match p.family() {
            Family::A1 => d.a1,
            Family::A23 => d.a23,
            Family::X => d.x,
            Family::Ua => d.ua,
        }
    }
}

/// What a conversion can be told to do, in the order it does it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum FixGroup {
    /// File structure: header, identifier, LZW, object streams, encryption, page boxes.
    Structure,
    Actions,
    Annotations,
    Forms,
    Images,
    Files,
    Colour,
    Fonts,
    /// What changes how pages look: transparency, transfer functions.
    Appearance,
    Metadata,
    /// PDF/UA: the accessibility checker's safe fixes.
    Accessibility,
}

impl FixGroup {
    pub fn label(self) -> &'static str {
        match self {
            FixGroup::Fonts => "Fonts",
            FixGroup::Colour => "Colour and output intent",
            FixGroup::Metadata => "Metadata",
            FixGroup::Actions => "Actions and JavaScript",
            FixGroup::Annotations => "Annotations",
            FixGroup::Forms => "Forms",
            FixGroup::Images => "Images",
            FixGroup::Files => "Embedded files",
            FixGroup::Structure => "File structure",
            FixGroup::Appearance => "Appearance",
            FixGroup::Accessibility => "Accessibility",
        }
    }

    /// Groups that change how pages look are left for the person to switch on.
    pub fn default_on(self) -> bool {
        self != FixGroup::Appearance
    }
}

// ---- the report ----------------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Passed,
    Failed,
    /// Needs a manual check (PDF/UA).
    Manual,
    /// Listed for information; it does not fail the profile (PDF/X-4 RGB content).
    Info,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// 0-based; `None` for the document as a whole.
    pub page: Option<u32>,
    /// What it is about: `Font Arial`, `Link annotation`, `Image`...
    pub object: Option<String>,
    pub xref: Option<i32>,
    pub rect: Option<Box4>,
    pub message: String,
    /// A fix group repairs it (when the conversion is allowed to).
    pub fixable: bool,
    /// Fixable only when the result is written as a new file.
    pub new_file: bool,
    /// Why it cannot be fixed (when it cannot): "needs fonts that aren't installed".
    pub note: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuleResult {
    pub rule: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub clause: &'static str,
    pub status: Status,
    pub hint: &'static str,
    /// The conversion group that repairs the rule.
    pub fix: Option<FixGroup>,
    /// An accessibility-checker fix ("title", "language", ...) for PDF/UA rules.
    pub a11y_fix: Option<&'static str>,
    /// Total findings (`findings` is capped).
    pub count: u32,
    pub findings: Vec<Finding>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanGroup {
    pub id: FixGroup,
    pub label: &'static str,
    /// What it does, in a sentence.
    pub effect: String,
    /// Issues it settles (findings), counting those that need a new file.
    pub issues: u32,
    /// Of those, the ones only a new file can settle.
    pub new_file_issues: u32,
    pub default_on: bool,
    /// Why the group cannot run at all.
    pub disabled: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Remaining {
    pub count: u32,
    /// "need fonts that aren't installed"
    pub text: String,
}

/// What a conversion would do, counted from the report.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// Findings of the failed rules.
    pub total: u32,
    pub groups: Vec<PlanGroup>,
    /// Findings no group settles, by reason.
    pub remaining: Vec<Remaining>,
    /// A language is needed for the accessibility group to set one.
    pub needs_lang: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub profile: Profile,
    pub pages: u32,
    /// What the file's XMP claims (`PDF/A-2b`, `PDF/UA-1`, `PDF/X-4`), if anything.
    pub claimed: Option<String>,
    /// No rule failed (manual checks may remain).
    pub conforms: bool,
    pub results: Vec<RuleResult>,
    pub plan: Plan,
}

impl Report {
    pub fn result(&self, rule: &str) -> Option<&RuleResult> {
        self.results.iter().find(|r| r.rule == rule)
    }

    pub fn status(&self, rule: &str) -> Option<Status> {
        self.result(rule).map(|r| r.status)
    }

    pub fn failed(&self) -> usize {
        self.results.iter().filter(|r| r.status == Status::Failed).count()
    }
}

/// What a file claims, read from its XMP alone (no check): the badge in Document properties.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    /// `PDF/A-2b`, `PDF/UA-1`, `PDF/X-4`.
    pub label: String,
    /// The profile "Verify" runs.
    pub profile: Option<Profile>,
}

// ---- entry points --------------------------------------------------------------------------------

/// Runs the profile's rules on `doc`. `head` is the first bytes of the file (the header rule reads them).
pub fn check(doc: &PdfDocument, head: &[u8], profile: Profile, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Report> {
    check::run(doc, head, profile, progress, cancel)
}

/// The claim in the XMP of `doc`.
pub fn claim(doc: &PdfDocument) -> Result<Option<Claim>> {
    check::claim(doc)
}

/// What a file's own claim says, as a profile this module can verify.
pub fn claimed_profile(label: &str) -> Option<Profile> {
    Profile::ALL.iter().copied().find(|p| p.label().eq_ignore_ascii_case(label))
}

/// The document as it is now (unsaved edits included), in a copy of its own, and the file's first bytes.
fn open_copy(eng: &Engine, doc: DocId) -> Result<(PdfDocument, Vec<u8>)> {
    let (_, source) = eng.doc_source(doc)?;
    let head: Vec<u8> = match &source {
        DocSource::Path(p) => {
            use std::io::Read as _;
            let mut buf = vec![0u8; 512];
            let n = std::fs::File::open(p.as_str()).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
            buf.truncate(n);
            buf
        }
        DocSource::Bytes(b, n) => b[..(*n).min(512)].to_vec(),
    };
    let mut d = match &source {
        DocSource::Path(p) => PdfDocument::open(p.as_str())?,
        DocSource::Bytes(b, n) => PdfDocument::from_bytes(&b[..*n])?,
    };
    if d.needs_password()? {
        match eng.password(doc)? {
            Some(p) if d.authenticate(&p)? => {}
            Some(_) => return Err(EngineError::BadPassword),
            None => return Err(EngineError::NeedsPassword),
        }
    }
    Ok((d, head))
}

pub fn check_doc(eng: &Engine, doc: DocId, profile: Profile, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Report> {
    let (d, head) = open_copy(eng, doc)?;
    check(&d, &head, profile, progress, cancel)
}

pub fn claim_doc(eng: &Engine, doc: DocId) -> Result<Option<Claim>> {
    let (d, _) = open_copy(eng, doc)?;
    claim(&d)
}

/// What to convert and how.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Convert {
    pub groups: Vec<FixGroup>,
    /// The document language the accessibility group sets when the document has none (`en-US`).
    pub lang: Option<String>,
}

/// Converts in place as one undo step. A document that is encrypted is refused: PDF/A has no
/// encryption, and an in-place change would keep it.
pub fn convert_in_place(eng: &Engine, doc: DocId, profile: Profile, c: &Convert, source: EditSource, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<EditState> {
    let path = eng.doc_entry(doc)?.path();
    let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let label = format!("Convert to {}", profile.label());
    // `apply_removing`: removed JavaScript, annotations and files are not left in the saved bytes.
    eng.apply_removing(doc, label, source, |d| fix::apply(d, profile, c, &stem, false, progress, cancel).map(|a| a.pages))
}

/// What a conversion wrote.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Converted {
    pub path: String,
    /// The new file checked against the same profile.
    pub report: Report,
    /// What the conversion did, as sentences.
    pub fixed: Vec<String>,
}

/// Converts into a new file at `dest` (never an open document; the caller checked). The open
/// document is not touched; the new file is read back and checked.
#[allow(clippy::too_many_arguments)]
pub fn convert_to_file(
    eng: &Engine,
    doc: DocId,
    profile: Profile,
    c: &Convert,
    dest: &str,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<Converted> {
    // The new file replaces what is at `dest`: never a document that is open.
    eng.check_not_open(dest, None)?;
    let path = eng.doc_entry(doc)?.path();
    let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (mut d, _) = open_copy(eng, doc)?;
    let applied = fix::apply(&mut d, profile, c, &stem, true, progress, cancel)?;
    let bytes = fix::write_new(&d, profile)?;
    crate::history_ipc::write_replace(std::path::Path::new(dest), &bytes).map_err(|e| EngineError::Io(e.to_string()))?;
    let reread = PdfDocument::from_bytes(&bytes)?;
    let head = bytes[..bytes.len().min(512)].to_vec();
    let report = check(&reread, &head, profile, &mut |_| {}, cancel)?;
    Ok(Converted { path: dest.to_string(), report, fixed: applied.log })
}

/// The report as a file: HTML or a PDF. It checks again, so the file matches what is open now.
pub fn report_bytes(eng: &Engine, doc: DocId, profile: Profile, pdf: bool, date: &str, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Vec<u8>> {
    let report = check_doc(eng, doc, profile, progress, cancel)?;
    let path = eng.doc_entry(doc)?.path();
    let name = std::path::Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if pdf { report::pdf(&name, date, &report) } else { Ok(report::html(&name, date, &report).into_bytes()) }
}

#[allow(unused_imports)]
pub(crate) use a11y::Progress as CheckProgress;
