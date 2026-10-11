//! Page marks: header & footer, watermark, background and Bates numbering (Acrobat's Edit PDF ▸
//! Header & Footer, Watermark, Background and Bates numbering).
//!
//! **How marks are stored.** Each mark is one content stream appended to (or, for a mark behind the
//! page, prepended to) the page's `/Contents`, drawn in display space (after `/Rotate`) and wrapped
//! in a marked-content artifact (ISO 32000 §14.8.2.2), so readers and screen readers skip it:
//! `/Artifact <</Type /Pagination /Subtype /Header /Attached [/Top] /PCMark /HeaderFooter>> BDC … EMC`.
//! The `/PCMark` entry, also written into the stream dictionary, names the kind of mark: that is how
//! Update and Remove find marks again without decompressing page content. A stream's `/IWSpec`
//! holds the settings it was made with (ASCII JSON), which the Update sheet starts from. Where
//! something is drawn on top, the page's original content is wrapped in `q … Q` (two tiny streams)
//! so its graphics state cannot leak into the mark.
//!
//! The artifact structure and the `/PCMark` tags follow pdfcraft (below), so files made by either
//! app are read, updated and removed by the other.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/edit/src/lib.rs @ b8ddf71: the mark kinds,
//! defaults, token expansion and layout. Text is measured with MuPDF's real Helvetica, Times and
//! Courier metrics instead of pdfcraft's class-based approximation.

use serde::{Deserialize, Serialize};

pub mod app;
mod apply;
mod geometry;
mod text;
#[cfg(test)]
mod tests;

pub use apply::{Info, KindInfo, add, bates_files, info, remove};
pub(crate) use apply::display_form;
pub(crate) use geometry::n;

/// The kinds of marks; each has its own Update and Remove, as in Acrobat's menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkKind {
    HeaderFooter,
    Watermark,
    Background,
    Bates,
}

impl MarkKind {
    /// The `/PCMark` tag in the content stream's dictionary (pdfcraft's names).
    pub(crate) fn tag(self) -> &'static str {
        match self {
            MarkKind::HeaderFooter => "HeaderFooter",
            MarkKind::Watermark => "Watermark",
            MarkKind::Background => "Background",
            MarkKind::Bates => "Bates",
        }
    }

    pub(crate) fn from_tag(t: &str) -> Option<MarkKind> {
        [MarkKind::HeaderFooter, MarkKind::Watermark, MarkKind::Background, MarkKind::Bates].into_iter().find(|k| k.tag() == t)
    }

    /// What the step is called in the undo menu and the version history.
    pub fn noun(self) -> &'static str {
        match self {
            MarkKind::HeaderFooter => "header and footer",
            MarkKind::Watermark => "watermark",
            MarkKind::Background => "background",
            MarkKind::Bates => "Bates numbering",
        }
    }
}

pub type Rgb = [f64; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Family {
    #[default]
    Helvetica,
    Times,
    Courier,
}

/// Text appearance. The faces are the standard 14, written without embedding (WinAnsiEncoding).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Style {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
    /// Points. Watermarks: 0 fits the text to the page (see [`Watermark::scale`]).
    pub size: f64,
    pub color: Rgb,
    pub underline: bool,
}

impl Default for Style {
    /// Acrobat's Header & Footer dialog: 12 pt black; the pdfcraft default of 8 pt is too small to
    /// read at a glance, so the dialog's own 12 is used.
    fn default() -> Self {
        Style { family: Family::Helvetica, bold: false, italic: false, size: 12.0, color: [0.0; 3], underline: false }
    }
}

/// Header and footer settings (Acrobat's Add Header and Footer dialog).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HeaderFooter {
    /// Top left, top centre, top right, then bottom left, bottom centre, bottom right. Tokens:
    /// `<<1>>` page number, `<<n>>` page count, `<<1 of n>>`, `<<1/n>>`, `<<Page 1>>`,
    /// `<<Page 1 of n>>`, and dates `<<m/d/yyyy>>`, `<<mm/dd/yyyy>>`, `<<d/m/yyyy>>`,
    /// `<<yyyy-mm-dd>>`, `<<m/d/yy>>`, `<<mmmm d, yyyy>>` …
    pub text: [String; 6],
    pub style: Style,
    /// Points from the page edge to the text: top, bottom, left, right.
    pub margins: [f64; 4],
    /// The number `<<1>>` shows on the first page of the range.
    pub start_number: u32,
}

impl Default for HeaderFooter {
    /// Acrobat's margins: 0.5 in top and bottom, 1 in left and right.
    fn default() -> Self {
        HeaderFooter { text: Default::default(), style: Style::default(), margins: [36.0, 36.0, 72.0, 72.0], start_number: 1 }
    }
}

/// Bates numbering: prefix, a number of fixed width, suffix, in one of the six header/footer places.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Bates {
    pub prefix: String,
    pub suffix: String,
    /// 3 to 15, as in Acrobat.
    pub digits: u32,
    pub start: u64,
    /// 0 to 5, in the order of [`HeaderFooter::text`]: the place the number is drawn.
    pub slot: u8,
    pub style: Style,
    pub margins: [f64; 4],
}

impl Default for Bates {
    fn default() -> Self {
        Bates { prefix: String::new(), suffix: String::new(), digits: 6, start: 1, slot: 5, style: Style::default(), margins: [36.0, 36.0, 72.0, 72.0] }
    }
}

/// A picture for a watermark or background (Acrobat: Source ▸ File): an image, or a page of a PDF.
/// From the UI, `file` is an open token (paths.rs); the command swaps it for the path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    pub file: String,
    /// 0-based; for a PDF.
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Watermark {
    pub text: String,
    pub picture: Option<Picture>,
    pub style: Style,
    /// Relative to the page: 1 fits the page. Text with `style.size == 0` is as wide as `scale` x
    /// the page diagonal; pictures fit the page at this factor.
    pub scale: f64,
    pub opacity: f64,
    /// Degrees counter-clockwise (Acrobat's 45 runs from bottom left to top right).
    pub rotation: f64,
    /// Behind the page content rather than on top of it.
    pub behind: bool,
    /// Where the centre is, from the page centre in points (right, up).
    pub offset: [f64; 2],
    /// Acrobat's appearance options.
    pub show_on_screen: bool,
    pub show_in_print: bool,
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark {
            text: String::new(),
            picture: None,
            style: Style { size: 0.0, color: [0.6, 0.6, 0.6], bold: true, ..Style::default() },
            scale: 0.5,
            opacity: 0.5,
            rotation: 45.0,
            behind: false,
            offset: [0.0; 2],
            show_on_screen: true,
            show_in_print: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Background {
    pub color: Rgb,
    pub opacity: f64,
    pub picture: Option<Picture>,
    pub scale: f64,
}

impl Default for Background {
    fn default() -> Self {
        Background { color: [1.0, 1.0, 0.85], opacity: 1.0, picture: None, scale: 1.0 }
    }
}

/// One mark to add. `Update` is the same with `replace`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Mark {
    HeaderFooter(HeaderFooter),
    Watermark(Watermark),
    Background(Background),
    Bates(Bates),
}

impl Mark {
    pub fn kind(&self) -> MarkKind {
        match self {
            Mark::HeaderFooter(_) => MarkKind::HeaderFooter,
            Mark::Watermark(_) => MarkKind::Watermark,
            Mark::Background(_) => MarkKind::Background,
            Mark::Bates(_) => MarkKind::Bates,
        }
    }
}

/// Values tokens need from outside the document: today's date, taken by the UI so the live preview
/// and the written page agree.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Date {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

/// What is being asked for: the mark and the pages (0-based) it goes on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRequest {
    pub mark: Mark,
    pub pages: Vec<u32>,
    /// Update: marks of this kind already on these pages are removed first.
    #[serde(default)]
    pub replace: bool,
    #[serde(default)]
    pub date: Date,
}

const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// Expands the `<<…>>` tokens of `template` for page number `page` (counting from the start number)
/// of `count` pages.
pub fn expand(template: &str, page: u32, count: usize, date: Date) -> String {
    let Date { year: y, month: m, day: d } = date;
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("<<") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find(">>") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        let yy = y.rem_euclid(100);
        let value = match token {
            "1" => page.to_string(),
            "n" => count.to_string(),
            "1 of n" => format!("{page} of {count}"),
            "1/n" => format!("{page}/{count}"),
            "Page 1" => format!("Page {page}"),
            "Page 1 of n" => format!("Page {page} of {count}"),
            "m/d" => format!("{m}/{d}"),
            "m/d/yy" => format!("{m}/{d}/{yy:02}"),
            "m/d/yyyy" => format!("{m}/{d}/{y}"),
            "mm/dd/yy" => format!("{m:02}/{d:02}/{yy:02}"),
            "mm/dd/yyyy" => format!("{m:02}/{d:02}/{y}"),
            "d/m/yy" => format!("{d}/{m}/{yy:02}"),
            "d/m/yyyy" => format!("{d}/{m}/{y}"),
            "dd/mm/yyyy" => format!("{d:02}/{m:02}/{y}"),
            "yyyy-mm-dd" => format!("{y}-{m:02}-{d:02}"),
            "mmmm d, yyyy" => format!("{} {d}, {y}", MONTHS[(m.clamp(1, 12) - 1) as usize]),
            _ => format!("<<{token}>>"),
        };
        out.push_str(&value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// A Bates number: `prefix`, `number` padded with zeros to `digits`, `suffix`.
pub fn bates_text(b: &Bates, number: u64) -> String {
    let digits = b.digits.clamp(3, 15) as usize;
    format!("{}{:0digits$}{}", b.prefix, number, b.suffix)
}

/// The largest start number whose last of `count` numbers still fits `digits` digits.
pub fn bates_fits(digits: u32, start: u64, count: u64) -> bool {
    let digits = digits.clamp(3, 15);
    let max = 10u64.pow(digits) - 1;
    count == 0 || start.checked_add(count - 1).is_some_and(|last| last <= max)
}
