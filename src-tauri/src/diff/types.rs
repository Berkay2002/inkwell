//! Public types of the compare module. Serialised with camelCase for the UI.
//!
//! Conventions: page numbers in `Change`/`PageMap` are 0-based indices (labels are 1-based for people).
//! Rectangles are `[x0, y0, x1, y1]` in *page space*: points at scale 1, origin top-left, page rotation
//! applied (the same space MuPDF reports text boxes in, and the space `VisualDiff::boxes` use).

use serde::Serialize;

pub type Rect4 = [f32; 4];

/// Which input a problem refers to.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    A,
    B,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Side::A => "first",
            Side::B => "second",
        })
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Serialize)]
#[serde(tag = "code", rename_all = "kebab-case")]
pub enum DiffError {
    #[error("{message}")]
    Mupdf { message: String },
    #[error("the {side} file needs a password")]
    NeedsPassword { side: Side },
    #[error("wrong password for the {side} file")]
    BadPassword { side: Side },
    #[error("the {side} file is not a PDF")]
    NotPdf { side: Side },
    #[error("page {page} is out of range")]
    BadPage { page: u32 },
    #[error("the {side} file has {count} {}, so page {page} is out of range", if *.count == 1 { "page" } else { "pages" })]
    PageOutOfRange { side: Side, page: u32, count: u32 },
    #[error("no pages of the {side} file were chosen")]
    NoPages { side: Side },
    #[error("that page took too long to draw")]
    Slow,
    #[error("comparison failed unexpectedly")]
    Panic,
    #[error("cancelled")]
    Cancelled,
}

impl From<mupdf::Error> for DiffError {
    fn from(e: mupdf::Error) -> Self {
        DiffError::Mupdf { message: e.to_string() }
    }
}

pub type Result<T> = std::result::Result<T, DiffError>;

#[derive(Clone, Debug, Default)]
pub struct CompareOptions {
    pub password_a: Option<String>,
    pub password_b: Option<String>,
    /// Report Producer / ModDate / XMP modify-date style fields too (they change on every save).
    pub include_volatile_metadata: bool,
    /// The pages of A and of B to compare, 0-based in the file's own numbering (`None` = every page). Page pairing,
    /// text, formatting, image and visual diffs and the report then cover these pages only, under their own numbers.
    pub pages_a: Option<Vec<u32>>,
    pub pages_b: Option<Vec<u32>>,
}

/// Where a comparison is, for a progress bar: `done` of `total` units of the current stage.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: Stage,
    pub done: u32,
    pub total: u32,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// Reading the first file's pages.
    First,
    /// Reading the second file's pages.
    Second,
    /// Matching pages and comparing each pair.
    Pages,
    /// Metadata, bookmarks, attachments.
    Document,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub pages: PageMap,
    pub changes: Vec<Change>,
    /// No semantic change at all (byte layout may differ).
    pub identical: bool,
}

/// How the pages of the two files correspond. Every page number is the file's own (0-based), so with a page range
/// on a side the numbers are the picked pages' real ones (old p. 5), never positions in the range.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PageMap {
    /// How many pages of each file were compared (all of them without a range).
    pub a_pages: u32,
    pub b_pages: u32,
    /// The pages compared on each side, ascending.
    pub a_ids: Vec<u32>,
    pub b_ids: Vec<u32>,
    /// For each of `b_ids`, the matched page of A (`None` = added).
    pub b_to_a: Vec<Option<u32>>,
    /// Compared pages of A with no counterpart in B.
    pub removed_a: Vec<u32>,
}

/// Declaration order is the order changes are listed in within one page.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    PageAdded,
    PageRemoved,
    /// An unmatched page of A and one of B at the same position.
    PageReplaced,
    PageMoved,
    PageRotated,
    PageBoxChanged,
    TextChanged,
    /// The same words in a different font, size or colour.
    FormatChanged,
    ImageReplaced,
    OcrLayerAdded,
    RedactionApplied,
    /// Painted content differs but none of the more specific kinds explains it (vector art, colours).
    PageContentChanged,
    AnnotationAdded,
    AnnotationRemoved,
    AnnotationChanged,
    FormFieldChanged,
    MetadataChanged,
    BookmarksChanged,
    AttachmentsChanged,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub kind: ChangeKind,
    pub a_page: Option<u32>,
    pub b_page: Option<u32>,
    pub rect_a: Option<Rect4>,
    pub rect_b: Option<Rect4>,
    /// One human line, e.g. "Text changed on p. 2: 'blank' -> 'A. Novak'".
    pub label: String,
    pub detail: Detail,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Detail {
    None,
    #[serde(rename_all = "camelCase")]
    PageMoved { from: u32, to: u32 },
    #[serde(rename_all = "camelCase")]
    PageRotated { from: i32, to: i32 },
    #[serde(rename_all = "camelCase")]
    PageBox { name: String, old: Option<Rect4>, new: Option<Rect4> },
    #[serde(rename_all = "camelCase")]
    Annotation { subtype: String, old: Option<AnnotInfo>, new: Option<AnnotInfo>, changed: Vec<String> },
    #[serde(rename_all = "camelCase")]
    FormField { name: String, old: Option<String>, new: Option<String> },
    #[serde(rename_all = "camelCase")]
    Text { runs: Vec<TextRun> },
    #[serde(rename_all = "camelCase")]
    Image { old: Option<ImageInfo>, new: Option<ImageInfo> },
    #[serde(rename_all = "camelCase")]
    Format { text: String, props: Vec<FormatProp> },
    #[serde(rename_all = "camelCase")]
    Ocr { words: u32 },
    #[serde(rename_all = "camelCase")]
    Redaction { via: RedactionVia, rects: Vec<Rect4>, words_removed: u32 },
    #[serde(rename_all = "camelCase")]
    Metadata { fields: Vec<MetadataField> },
    #[serde(rename_all = "camelCase")]
    Bookmarks { items: Vec<BookmarkChange> },
    #[serde(rename_all = "camelCase")]
    Attachments { items: Vec<AttachmentChange> },
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotInfo {
    pub subtype: String,
    pub name: Option<String>,
    pub contents: String,
    pub author: Option<String>,
    /// The /C array (1, 3 or 4 components, 0..1).
    pub colour: Option<Vec<f32>>,
    pub rect: Rect4,
}

/// One property of a formatting change: `font`, `size` (points) or `colour` (#rrggbb).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FormatProp {
    pub name: String,
    pub old: String,
    pub new: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunOp {
    Equal,
    Insert,
    Delete,
}

/// A run of consecutive words with the same op. `boxes_a`/`boxes_b` are the word boxes on each side
/// (Equal runs have both, Delete only A, Insert only B), one per word.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextRun {
    pub op: RunOp,
    pub text: String,
    pub boxes_a: Vec<Rect4>,
    pub boxes_b: Vec<Rect4>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// First 8 bytes of the BLAKE3 of the image, hex.
    pub hash: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RedactionVia {
    /// Redact annotations disappeared and the content under them changed.
    Annotation,
    /// Text vanished and an opaque filled box now covers where it was.
    OpaqueBox,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MetadataField {
    pub source: MetadataSource,
    pub field: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MetadataSource {
    Info,
    Xmp,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BookmarkOp {
    Added,
    Removed,
    Renamed,
    /// Same title, points at a different page.
    Retargeted,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkChange {
    pub op: BookmarkOp,
    pub depth: u32,
    pub title: String,
    pub old_title: Option<String>,
    /// 0-based page in A / B terms.
    pub old_page: Option<u32>,
    pub new_page: Option<u32>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AttachmentOp {
    Added,
    Removed,
    Changed,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentChange {
    pub op: AttachmentOp,
    pub name: String,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}
