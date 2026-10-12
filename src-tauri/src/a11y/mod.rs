// Adapted from pdfcraft (MIT OR Apache-2.0), crates/a11y/src/{lib,content}.rs @ b8ddf71
//! Accessibility (Acrobat's "Accessibility checker" and "Autotag"): a check that reads the
//! document and reports, per rule, Passed / Failed / Needs manual check / Skipped with the page,
//! object and a fix hint; safe automatic fixes, each one edit-core step (undoable, kept by Save);
//! and a best-effort autotag that builds a basic structure tree from MuPDF's structured text.
//!
//! The rules, which are automatic and which manual, and the gaps are in docs/a11y.md.

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, DocSource, Engine, EngineError};
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

pub mod app;
pub mod autotag;
pub mod content;
pub mod locate;
pub mod matterhorn;
pub mod order;
pub mod report;
pub mod tables;
pub mod tags;
pub mod tree;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_structure;
#[cfg(test)]
mod tests_matterhorn;
#[cfg(test)]
mod tests_hostile;
#[cfg(test)]
mod tests_tables;

use locate::Locator;
use tree::{Tree, name, text, xref_of};

pub type Result<T> = std::result::Result<T, EngineError>;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// Where the tag tree and the ParentTree disagree (see `tags::problems`).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub page: Option<u32>,
    pub xref: Option<i32>,
    pub message: String,
}

/// The rules, in report order. Serialised as their stable ids (`tagged-pdf`, `title`, ...).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Rule {
    TaggedPdf,
    Title,
    DisplayDocTitle,
    PrimaryLanguage,
    FigureAltText,
    ReadingOrder,
    TableHeaders,
    HeadingNesting,
    ListStructure,
    TaggedLinks,
    FieldTooltips,
    ColorContrast,
    FontsUnicode,
    Bookmarks,
    TabOrder,
    ContentMarking,
    RoleMap,
    LanguageValues,
    Metadata,
    AccessibilityPermission,
    TableStructure,
    AnnotationsTagged,
    FontsEmbedded,
    Xfa,
    OptionalContent,
    ParentTree,
    TableScope,
}

impl Rule {
    pub const ALL: [Rule; 27] = [
        Rule::TaggedPdf,
        Rule::Title,
        Rule::DisplayDocTitle,
        Rule::PrimaryLanguage,
        Rule::FigureAltText,
        Rule::ReadingOrder,
        Rule::TableHeaders,
        Rule::HeadingNesting,
        Rule::ListStructure,
        Rule::TaggedLinks,
        Rule::FieldTooltips,
        Rule::ColorContrast,
        Rule::FontsUnicode,
        Rule::Bookmarks,
        Rule::TabOrder,
        Rule::ContentMarking,
        Rule::RoleMap,
        Rule::LanguageValues,
        Rule::Metadata,
        Rule::AccessibilityPermission,
        Rule::TableStructure,
        Rule::AnnotationsTagged,
        Rule::FontsEmbedded,
        Rule::Xfa,
        Rule::OptionalContent,
        Rule::ParentTree,
        Rule::TableScope,
    ];

    /// `(id, name, fix hint, automatic fix)`; the fix is a [`Fix`] tag (`"title"`, ...).
    fn info(self) -> (&'static str, &'static str, &'static str, Option<&'static str>) {
        match self {
            Rule::TaggedPdf => (
                "tagged-pdf",
                "Tagged PDF",
                "Tags give the document a structure assistive technology can follow. Re-export it tagged from the source application, or run Autotag (best effort) and review the result.",
                Some("autotag"),
            ),
            Rule::Title => ("title", "Title", "Give the document a meaningful title in its properties.", Some("title")),
            Rule::DisplayDocTitle => (
                "display-doc-title",
                "Title in title bar",
                "Viewers should show the document title, not the file name (Initial view: Show document title).",
                Some("displayDocTitle"),
            ),
            Rule::PrimaryLanguage => (
                "primary-language",
                "Primary language",
                "Screen readers choose their voice from the document language. Set it (for example en-US).",
                Some("language"),
            ),
            Rule::FigureAltText => (
                "figure-alt-text",
                "Figures alternate text",
                "Describe each figure in alternate text, or retag purely decorative images as artifacts.",
                Some("figureAlt"),
            ),
            Rule::ReadingOrder => (
                "reading-order",
                "Reading order",
                "Assistive technology reads in tag order. Where it differs from the order the page is drawn in, check that the tags read naturally and reorder them if not; content outside the tags is not read at all.",
                None,
            ),
            Rule::TableHeaders => ("table-headers", "Table headers", "Mark the header cells of each table as TH.", None),
            Rule::HeadingNesting => (
                "heading-nesting",
                "Heading nesting",
                "Heading levels should not skip (an H1 followed by an H3). Retag the headings.",
                None,
            ),
            Rule::ListStructure => (
                "list-structure",
                "List structure",
                "Lists are L > LI > (Lbl, LBody). Fix the list's tags.",
                None,
            ),
            Rule::TaggedLinks => (
                "tagged-links",
                "Tagged links",
                "Each link annotation must be in the tag tree inside a Link element to be announced.",
                None,
            ),
            Rule::FieldTooltips => (
                "field-tooltips",
                "Form field tooltips",
                "Each form field needs a tooltip (TU) that says what to enter.",
                Some("fieldTooltips"),
            ),
            Rule::ColorContrast => (
                "color-contrast",
                "Color contrast",
                "Check by eye or with a contrast tool that text contrasts enough with its background (WCAG 4.5:1 for body text).",
                None,
            ),
            Rule::FontsUnicode => (
                "fonts-unicode",
                "Character encoding",
                "Fonts must map their characters to Unicode (ToUnicode) so text can be read aloud and copied. Recreate the PDF with the fonts embedded properly, or run OCR.",
                None,
            ),
            Rule::Bookmarks => ("bookmarks", "Bookmarks", "Documents over 20 pages need bookmarks for their main sections.", None),
            Rule::TabOrder => (
                "tab-order",
                "Tab order",
                "Pages with links, comments or fields should tab in structure order (Tabs /S).",
                Some("tabOrder"),
            ),
            Rule::ContentMarking => (
                "content-marking",
                "Content tagged or artifact",
                "Every piece of text, image and graphic must be in the tag tree or marked as an artifact, and artifacts and tags must not be nested in each other. Tag the content, or mark it as artifact in the reading order.",
                None,
            ),
            Rule::RoleMap => (
                "role-map",
                "Role map",
                "Custom tag types must map to a standard type, in a map that doesn't loop, and standard types may not be remapped. Fix the role map or retag the elements.",
                None,
            ),
            Rule::LanguageValues => ("language-values", "Language values", "Every /Lang, on the document and on tags, must be a valid language tag such as en-US.", None),
            Rule::Metadata => (
                "metadata",
                "XMP metadata",
                "A PDF/UA file carries XMP metadata with a title (dc:title) and the PDF/UA identification (pdfuaid:part 1).",
                Some("metadata"),
            ),
            Rule::AccessibilityPermission => (
                "accessibility-permission",
                "Accessibility permission",
                "Encryption must allow content extraction for accessibility (permission bit 10). Save the document without that restriction.",
                None,
            ),
            Rule::TableStructure => (
                "table-structure",
                "Table structure",
                "Tables are Table > TR > TH/TD (with optional THead, TBody and TFoot); rows must line up, and /Headers must name header cells that exist.",
                None,
            ),
            Rule::AnnotationsTagged => (
                "annotations-tagged",
                "Annotations in the tag tree",
                "Form fields belong in a Form element and other annotations in an Annot element, each with a description (Contents, or alternate text on the tag).",
                None,
            ),
            Rule::FontsEmbedded => ("fonts-embedded", "Embedded fonts", "Every font must be embedded in the file. Re-create the PDF with its fonts embedded.", None),
            Rule::Xfa => ("xfa", "XFA forms", "Forms described by XFA can't be tagged. Convert the form to an AcroForm.", None),
            Rule::OptionalContent => (
                "optional-content",
                "Optional content",
                "Each optional content configuration needs a /Name and must not use /AS (automatic state).",
                None,
            ),
            Rule::ParentTree => (
                "parent-tree",
                "Tag tree links",
                "Marked content and annotations map back to their tags through the ParentTree, and each tag's /P is its parent. Repair the links.",
                Some("parentTree"),
            ),
            Rule::TableScope => (
                "table-scope",
                "Table header scope",
                "Give each header cell (TH) a Scope (Row, Column or Both), or have the data cells name their headers with /Headers.",
                None,
            ),
        }
    }

    pub fn id(self) -> &'static str {
        self.info().0
    }

    /// Acrobat's group for the rule (see `report::GROUPS`).
    pub fn group(self) -> &'static str {
        match self {
            Rule::TaggedPdf | Rule::Title | Rule::DisplayDocTitle | Rule::PrimaryLanguage | Rule::ReadingOrder | Rule::Bookmarks | Rule::ColorContrast => "Document",
            Rule::TaggedLinks | Rule::TabOrder | Rule::FontsUnicode | Rule::ContentMarking | Rule::AnnotationsTagged | Rule::FontsEmbedded | Rule::OptionalContent => "Page content",
            Rule::RoleMap | Rule::LanguageValues | Rule::Metadata | Rule::AccessibilityPermission | Rule::ParentTree => "Document",
            Rule::FieldTooltips | Rule::Xfa => "Forms",
            Rule::FigureAltText => "Alternate text",
            Rule::TableHeaders | Rule::TableStructure | Rule::TableScope => "Tables",
            Rule::ListStructure => "Lists",
            Rule::HeadingNesting => "Headings",
        }
    }

    pub fn name(self) -> &'static str {
        self.info().1
    }

    pub fn hint(self) -> &'static str {
        self.info().2
    }

    pub fn fix(self) -> Option<&'static str> {
        self.info().3
    }

    /// Only a person can judge it.
    pub fn manual(self) -> bool {
        self == Rule::ColorContrast
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Passed,
    Failed,
    /// Needs manual check.
    Manual,
    /// Does not apply (for example a structure rule on an untagged document).
    Skipped,
}

/// One problem.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// 0-based page.
    pub page: Option<u32>,
    /// What it is about, e.g. `Figure`, `Link annotation`, `Font Helvetica`, `Field "name"`.
    pub object: Option<String>,
    /// Object number, when the object is an indirect PDF object (`figureAlt` takes it).
    pub xref: Option<i32>,
    /// Where it is on its page, `[x0, y0, x1, y1]` in points from the top left, when it has an extent
    /// (annotations, figures and other painted content; not text).
    pub rect: Option<locate::Box4>,
    pub message: String,
}

impl Finding {
    fn doc(message: impl Into<String>) -> Finding {
        Finding { message: message.into(), ..Finding::default() }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuleResult {
    pub rule: Rule,
    pub name: &'static str,
    /// Acrobat's group: Document, Page content, Forms, Alternate text, Tables, Lists or Headings.
    pub group: &'static str,
    pub status: Status,
    pub hint: &'static str,
    /// The [`Fix`] that addresses it automatically, if any.
    pub fix: Option<&'static str>,
    pub findings: Vec<Finding>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub pages: u32,
    pub tagged: bool,
    /// A `/DocMDP` certification: no accessibility edit is made to the document (see [`edit_doc`]).
    pub certified: bool,
    /// Signed: edits are saved after the signed bytes, which stay as they were.
    pub signed: bool,
    pub results: Vec<RuleResult>,
}

impl Report {
    pub fn result(&self, rule: Rule) -> &RuleResult {
        self.results.iter().find(|r| r.rule == rule).expect("every rule is reported")
    }

    pub fn status(&self, rule: Rule) -> Status {
        self.result(rule).status
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Pages scanned so far.
    pub done: u32,
    pub pages: u32,
    /// What is running: `check`, or for Autotag `read` (the text of every page, to find the body size) then `tag`.
    pub stage: &'static str,
}

/// Findings beyond this many per rule are summarised ("... and n more").
const MAX_FINDINGS: usize = 200;

// ---- reading helpers -----------------------------------------------------------------------------

fn is_true(o: Option<PdfObject>) -> Result<bool> {
    Ok(match o {
        Some(v) if v.is_bool()? => v.as_bool()?,
        _ => false,
    })
}

fn dict(o: &PdfObject, key: &str) -> Result<Option<PdfObject>> {
    Ok(o.get_dict(key)?.filter(|v| v.is_dict().unwrap_or(false)))
}

/// The page's annotations (Popups left out) with their object numbers.
pub(crate) fn annots(page: &PdfObject) -> Result<Vec<(i32, PdfObject)>> {
    let mut out = Vec::new();
    let Some(a) = page.get_dict("Annots")?.filter(|a| a.is_array().unwrap_or(false)) else { return Ok(out) };
    for x in a.array_iter()? {
        let x = x?;
        if x.is_dict()? && name(&x, "Subtype")?.as_deref() != Some("Popup") {
            out.push((xref_of(&x)?, x));
        }
    }
    Ok(out)
}

/// The decoded content of a page (its streams joined).
pub(crate) fn page_content(page: &PdfObject) -> Result<Vec<u8>> {
    let Some(c) = page.get_dict("Contents")? else { return Ok(Vec::new()) };
    if c.is_array()? {
        let mut out = Vec::new();
        for s in c.array_iter()? {
            let s = s?;
            if s.is_stream()? {
                out.extend(s.read_stream()?);
                out.push(b'\n');
            }
        }
        Ok(out)
    } else if c.is_stream()? {
        Ok(c.read_stream()?)
    } else {
        Ok(Vec::new())
    }
}

/// The form field a widget belongs to: the widget itself when it has a name, else its parent.
fn field_of(widget: &PdfObject) -> Result<Option<PdfObject>> {
    if widget.get_dict("T")?.is_some() {
        return Ok(Some(widget.clone()));
    }
    Ok(widget.get_dict("Parent")?.filter(|p| p.is_dict().unwrap_or(false)))
}

fn full_name(field: &PdfObject) -> Result<String> {
    let mut parts = Vec::new();
    let mut node = Some(field.clone());
    for _ in 0..64 {
        let Some(n) = node else { break };
        if let Some(t) = text(&n, "T")? {
            parts.push(t);
        }
        node = n.get_dict("Parent")?.filter(|p| p.is_dict().unwrap_or(false));
    }
    parts.reverse();
    Ok(parts.join("."))
}

/// What one page's content holds.
#[derive(Default)]
struct PageScan {
    /// MCIDs of the page's own content in drawing order (first occurrence).
    mcids: Vec<i32>,
    /// Fonts used (here or in form XObjects): object number (0: direct) and the font.
    fonts: Vec<(i32, PdfObject)>,
    /// How its content is marked: what is neither tagged nor artifact, and nesting.
    marking: matterhorn::Marking,
    /// Forms already checked for their own marking.
    marked_forms: HashSet<i32>,
}

const MAX_FORM_DEPTH: usize = 12;

fn res_lookup(res: Option<&PdfObject>, cat: &str, key: &[u8]) -> Result<Option<PdfObject>> {
    let Some(r) = res else { return Ok(None) };
    let Some(d) = dict(r, cat)? else { return Ok(None) };
    Ok(d.get_dict(String::from_utf8_lossy(key).into_owned())?)
}

#[allow(clippy::too_many_arguments)]
fn scan_stream(data: &[u8], res: Option<&PdfObject>, top: bool, depth: usize, seen: &mut HashSet<i32>, marks: &mut Vec<matterhorn::Mark>, track: bool, out: &mut PageScan) -> Result<()> {
    let mut mcid_seen: HashSet<i32> = out.mcids.iter().copied().collect();
    for op in content::parse(data) {
        if track {
            matterhorn::track(&op, res, marks, &mut out.marking)?;
        }
        match op.name.as_slice() {
            b"BDC" if top => {
                let mcid = match op.operands.get(1) {
                    Some(d @ content::Operand::Dict(_)) => d.get(b"MCID").map(|m| m.num() as i32),
                    Some(content::Operand::Name(n)) => match res_lookup(res, "Properties", n)? {
                        Some(p) if p.is_dict()? => p.get_dict("MCID")?.map(|m| m.as_int()).transpose()?,
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(m) = mcid.filter(|m| mcid_seen.insert(*m)) {
                    out.mcids.push(m);
                }
            }
            b"Tf" => {
                if let Some(f) = op.operands.first().and_then(|o| o.name()).map(|n| res_lookup(res, "Font", n)).transpose()?.flatten() {
                    let x = xref_of(&f)?;
                    if x == 0 || !out.fonts.iter().any(|(n, _)| *n == x) {
                        out.fonts.push((x, f));
                    }
                }
            }
            b"Do" if depth < MAX_FORM_DEPTH => {
                let Some(x) = op.operands.first().and_then(|o| o.name()).map(|n| res_lookup(res, "XObject", n)).transpose()?.flatten() else { continue };
                if !x.is_stream()? || name(&x, "Subtype")?.as_deref() != Some("Form") {
                    continue;
                }
                let n = xref_of(&x)?;
                // A form drawn inside tagged content or an artifact is covered by that; one drawn outside
                // is checked for its own marking (once per form).
                let covered = marks.iter().any(|k| matches!(k, matterhorn::Mark::Tagged | matterhorn::Mark::Artifact));
                let mark_it = track && !covered && (n == 0 || out.marked_forms.insert(n));
                let scan_it = n == 0 || seen.insert(n);
                if !mark_it && !scan_it {
                    continue;
                }
                let inner = dict(&x, "Resources")?;
                let data = x.read_stream().unwrap_or_default();
                scan_stream(&data, inner.as_ref().or(res), false, depth + 1, seen, &mut Vec::new(), mark_it, out)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn scan_page(page: &PdfObject) -> Result<PageScan> {
    let mut out = PageScan::default();
    let res = page.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
    let data = page_content(page)?;
    scan_stream(&data, res.as_ref(), true, 0, &mut HashSet::new(), &mut Vec::new(), true, &mut out)?;
    Ok(out)
}

const STANDARD_ENCODINGS: &[&str] = &["StandardEncoding", "WinAnsiEncoding", "MacRomanEncoding", "MacExpertEncoding", "PDFDocEncoding"];

/// A glyph name that says nothing about its character: `g12`, `glyph7`, `cid0042`, `index3`,
/// `G5`, a bare number. (`uniXXXX`, `uXXXX` and Adobe Glyph List names do map.)
fn opaque_glyph(n: &str) -> bool {
    let digits = n.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    let prefix = &n[..n.len() - digits.len()];
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && matches!(prefix.to_ascii_lowercase().as_str(), "" | "g" | "gid" | "glyph" | "cid" | "index" | "c")
}

/// Whether text in this font can be mapped to Unicode: a ToUnicode CMap, a standard encoding,
/// or a predefined CJK collection.
pub(crate) fn maps_to_unicode(f: &PdfObject) -> Result<bool> {
    if f.get_dict("ToUnicode")?.is_some() {
        return Ok(true);
    }
    let enc = f.get_dict("Encoding")?;
    Ok(match name(f, "Subtype")?.as_deref() {
        Some("Type0") => {
            let identity = match &enc {
                Some(e) if e.is_name()? => e.as_name()?.starts_with(b"Identity"),
                _ => false,
            };
            let ordering = f
                .get_dict("DescendantFonts")?
                .filter(|d| d.is_array().unwrap_or(false))
                .and_then(|d| d.get_array(0).ok().flatten())
                .and_then(|d| d.get_dict("CIDSystemInfo").ok().flatten())
                .and_then(|c| c.get_dict("Ordering").ok().flatten())
                .and_then(|o| o.as_string().ok());
            let known = matches!(ordering.as_deref(), Some("Japan1" | "GB1" | "CNS1" | "Korea1" | "KR"));
            !identity && known
        }
        Some("Type3") => enc.is_some(),
        _ => match &enc {
            Some(e) if e.is_name()? => STANDARD_ENCODINGS.contains(&String::from_utf8_lossy(&e.as_name()?).as_ref()),
            Some(e) if e.is_dict()? => {
                let base_ok = name(e, "BaseEncoding")?.is_none_or(|b| STANDARD_ENCODINGS.contains(&b.as_str()));
                let mut names_ok = true;
                if let Some(d) = e.get_dict("Differences")?.filter(|d| d.is_array().unwrap_or(false)) {
                    for x in d.array_iter()? {
                        let x = x?;
                        if x.is_name()? && opaque_glyph(&String::from_utf8_lossy(&x.as_name()?)) {
                            names_ok = false;
                            break;
                        }
                    }
                }
                base_ok && names_ok
            }
            _ => {
                // The font's built-in encoding: standard unless the font is symbolic.
                let flags = f
                    .get_dict("FontDescriptor")?
                    .and_then(|d| d.get_dict("Flags").ok().flatten())
                    .and_then(|v| v.as_int().ok())
                    .unwrap_or(32);
                let base = name(f, "BaseFont")?.unwrap_or_default();
                flags & 4 == 0 || matches!(base.as_str(), "Symbol" | "ZapfDingbats")
            }
        },
    })
}

// ---- the check -----------------------------------------------------------------------------------

/// Runs every rule. `progress` is called per page; `cancel` is polled between pages.
pub fn check(doc: &PdfDocument, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Report> {
    let n = doc.page_count()?.max(0) as u32;
    let index = tree::page_index(doc)?;
    let tree = Tree::read(doc, &index)?;
    let cancelled = || cancel.load(Ordering::Relaxed);
    if cancelled() {
        return Err(bad("cancelled"));
    }
    let cat = doc.catalog()?;

    let mut scans = Vec::with_capacity(n as usize);
    let mut pages = Vec::with_capacity(n as usize);
    for p in 0..n {
        if cancelled() {
            return Err(bad("cancelled"));
        }
        let obj = doc.find_page(p as i32)?;
        scans.push(scan_page(&obj)?);
        pages.push(obj);
        progress(Progress { done: p + 1, pages: n, stage: "check" });
    }

    let loc = Locator::new(&pages, &tree);
    let mut results = Vec::with_capacity(Rule::ALL.len());
    for rule in Rule::ALL {
        if cancelled() {
            return Err(bad("cancelled"));
        }
        let (status, mut findings) = if rule.manual() {
            (Status::Manual, Vec::new())
        } else {
            match run_rule(rule, doc, &cat, &tree, &pages, &scans, &loc)? {
                None => (Status::Skipped, Vec::new()),
                Some(f) => (if f.is_empty() { Status::Passed } else { Status::Failed }, f),
            }
        };
        if findings.len() > MAX_FINDINGS {
            let more = findings.len() - MAX_FINDINGS;
            findings.truncate(MAX_FINDINGS);
            findings.push(Finding::doc(format!("... and {more} more")));
        }
        results.push(RuleResult { rule, name: rule.name(), group: rule.group(), status, hint: rule.hint(), fix: rule.fix(), findings });
    }
    Ok(Report { pages: n, tagged: tree.exists, certified: crate::outline::certified(doc).map_or(true, |c| c.is_some()), signed: crate::outline::signed(doc), results })
}

fn elem_finding(t: &Tree, loc: &Locator, i: usize, what: &str) -> Finding {
    let e = &t.elems[i];
    let at = e.page.map(|p| format!(" on page {}", p + 1)).unwrap_or_default();
    Finding {
        page: e.page,
        object: Some(e.ty.clone()),
        xref: (e.xref != 0).then_some(e.xref),
        rect: loc.element(i),
        message: format!("{} element{at}: {what}", e.ty),
    }
}

/// `None`: the rule does not apply (Skipped).
fn run_rule(rule: Rule, doc: &PdfDocument, cat: &PdfObject, t: &Tree, pages: &[PdfObject], scans: &[PageScan], loc: &Locator) -> Result<Option<Vec<Finding>>> {
    let mut out = Vec::new();
    let structure = matches!(
        rule,
        Rule::FigureAltText
            | Rule::ReadingOrder
            | Rule::TableHeaders
            | Rule::HeadingNesting
            | Rule::ListStructure
            | Rule::ContentMarking
            | Rule::RoleMap
            | Rule::TableStructure
            | Rule::TableScope
            | Rule::AnnotationsTagged
            | Rule::ParentTree
    );
    if structure && !t.exists {
        return Ok(None);
    }
    let order = t.preorder();
    match rule {
        Rule::TaggedPdf => {
            if !t.exists {
                out.push(Finding::doc("The document has no tags (structure tree)"));
            }
            if !is_true(dict(cat, "MarkInfo")?.map(|m| m.get_dict("Marked")).transpose()?.flatten())? {
                out.push(Finding::doc("The document isn't marked as tagged (MarkInfo /Marked true)"));
            }
            if is_true(dict(cat, "MarkInfo")?.map(|m| m.get_dict("Suspects")).transpose()?.flatten())? {
                out.push(Finding::doc("The tags are marked as suspect (MarkInfo /Suspects true)"));
            }
        }
        Rule::Title => {
            let info = doc.trailer()?.get_dict("Info")?.filter(|i| i.is_dict().unwrap_or(false));
            if info.map(|i| text(&i, "Title")).transpose()?.flatten().is_none() {
                out.push(Finding::doc("The document has no title"));
            }
        }
        Rule::DisplayDocTitle => {
            if !is_true(dict(cat, "ViewerPreferences")?.map(|v| v.get_dict("DisplayDocTitle")).transpose()?.flatten())? {
                out.push(Finding::doc("Viewers show the file name, not the document title (DisplayDocTitle is not set)"));
            }
        }
        Rule::PrimaryLanguage => {
            if text(cat, "Lang")?.is_none() {
                out.push(Finding::doc("No document language is set"));
            }
        }
        Rule::FigureAltText => {
            for &i in &order {
                if t.is(i, "Figure") && !t.elems[i].alt && !t.ancestors(i).any(|a| t.elems[a].alt) {
                    out.push(elem_finding(t, loc, i, "no alternate text"));
                }
            }
        }
        Rule::ReadingOrder => {
            let mut tagged: HashMap<u32, Vec<i32>> = HashMap::new();
            let mut seen = HashSet::new();
            for &(p, m) in &t.order {
                if seen.insert((p, m)) {
                    tagged.entry(p).or_default().push(m);
                }
            }
            for (p, scan) in scans.iter().enumerate() {
                let p = p as u32;
                let tags = tagged.remove(&p).unwrap_or_default();
                let in_tags: HashSet<i32> = tags.iter().copied().collect();
                let drawn: HashSet<i32> = scan.mcids.iter().copied().collect();
                let orphans = scan.mcids.iter().filter(|m| !in_tags.contains(m)).count();
                if orphans > 0 {
                    out.push(Finding {
                        page: Some(p),
                        object: Some("Marked content".into()),
                        xref: None,
                        rect: None,
                        message: format!("Page {}: {orphans} marked-content item{} not in the tag tree (never read)", p + 1, if orphans == 1 { " is" } else { "s are" }),
                    });
                }
                let a: Vec<i32> = tags.iter().copied().filter(|m| drawn.contains(m)).collect();
                let b: Vec<i32> = scan.mcids.iter().copied().filter(|m| in_tags.contains(m)).collect();
                if let Some(k) = (0..a.len()).find(|&k| a[k] != b[k]) {
                    out.push(Finding {
                        page: Some(p),
                        object: Some("Marked content".into()),
                        xref: None,
                        rect: None,
                        message: format!(
                            "Page {}: the tags read in a different order than the page is drawn (tag order reaches MCID {} where content order has MCID {})",
                            p + 1,
                            a[k],
                            b[k]
                        ),
                    });
                }
            }
        }
        Rule::TableHeaders => {
            for &i in &order {
                if !t.is(i, "Table") {
                    continue;
                }
                let has_th = t.table_rows(i).iter().any(|&r| t.elems[r].children.iter().any(|&c| t.is(c, "TH")));
                if !has_th {
                    out.push(elem_finding(t, loc, i, "no header cells (TH)"));
                }
            }
        }
        Rule::HeadingNesting => {
            let mut prev = 0u8;
            for &i in &order {
                let level = match t.elems[i].ty.as_bytes() {
                    [b'H', d @ b'1'..=b'6'] => d - b'0',
                    _ => continue,
                };
                if level > prev + 1 {
                    let what = if prev == 0 { format!("the first heading is an H{level}") } else { format!("H{level} follows an H{prev}") };
                    out.push(elem_finding(t, loc, i, &format!("{what}; heading levels should not skip")));
                }
                prev = level;
            }
            // H and H1-H6 together.
            if let Some(&h) = order.iter().find(|&&i| t.is(i, "H"))
                && order.iter().any(|&i| matches!(t.elems[i].ty.as_bytes(), [b'H', b'1'..=b'6']))
            {
                out.push(elem_finding(t, loc, h, "an unnumbered H is used together with numbered headings (H1-H6)"));
            }
        }
        Rule::ListStructure => {
            for &i in &order {
                match t.elems[i].ty.as_str() {
                    "LI" if !t.parent_is(i, &["L"]) => out.push(elem_finding(t, loc, i, "not in an L (list)")),
                    "Lbl" | "LBody" if !t.parent_is(i, &["LI", "TOCI", "BibEntry", "Note", "FENote"]) => {
                        out.push(elem_finding(t, loc, i, "not in an LI (list item)"))
                    }
                    "L" => {
                        let stray = t.elems[i].children.iter().filter(|&&c| !matches!(t.elems[c].ty.as_str(), "LI" | "L" | "Caption" | "NonStruct" | "Artifact")).count();
                        if stray > 0 {
                            out.push(elem_finding(t, loc, i, &format!("{stray} child element{} that are not LI", if stray == 1 { "" } else { "s" })));
                        }
                        if t.elems[i].children.is_empty() {
                            out.push(elem_finding(t, loc, i, "an empty list"));
                        }
                    }
                    _ => {}
                }
            }
        }
        Rule::TaggedLinks => {
            for (p, page) in pages.iter().enumerate() {
                for (x, a) in annots(page)? {
                    if name(&a, "Subtype")?.as_deref() != Some("Link") {
                        continue;
                    }
                    let owner = t.annot_owner.get(&x).copied();
                    let why = if !t.exists {
                        Some("the document isn't tagged".to_string())
                    } else if owner.is_none() {
                        Some("not in the tag tree".to_string())
                    } else if a.get_dict("StructParent")?.is_none() {
                        Some("no /StructParent entry".to_string())
                    } else {
                        owner.filter(|o| !t.is(*o, "Link")).map(|o| format!("tagged as {}, not Link", t.elems[o].ty))
                    };
                    if let Some(why) = why {
                        out.push(Finding {
                            page: Some(p as u32),
                            object: Some("Link annotation".into()),
                            xref: (x != 0).then_some(x),
                            rect: locate::annot_box(page, &a)?,
                            message: format!("Link on page {}: {why}", p + 1),
                        });
                    }
                }
            }
        }
        Rule::FieldTooltips => {
            let mut seen = HashSet::new();
            for (p, page) in pages.iter().enumerate() {
                for (_, a) in annots(page)? {
                    if name(&a, "Subtype")?.as_deref() != Some("Widget") {
                        continue;
                    }
                    let Some(field) = field_of(&a)? else { continue };
                    let x = xref_of(&field)?;
                    if x != 0 && !seen.insert(x) {
                        continue;
                    }
                    if text(&field, "TU")?.is_none() {
                        let n = full_name(&field)?;
                        out.push(Finding {
                            page: Some(p as u32),
                            object: Some(format!("Field \"{n}\"")),
                            xref: (x != 0).then_some(x),
                            rect: locate::annot_box(page, &a)?,
                            message: format!("Field \"{n}\" on page {} has no tooltip (TU)", p + 1),
                        });
                    }
                }
            }
        }
        Rule::FontsUnicode => {
            let mut seen = HashSet::new();
            for (p, scan) in scans.iter().enumerate() {
                for (x, f) in &scan.fonts {
                    if (*x != 0 && !seen.insert(*x)) || maps_to_unicode(f)? {
                        continue;
                    }
                    let base = name(f, "BaseFont")?.unwrap_or_else(|| "(unnamed)".into());
                    out.push(Finding {
                        page: Some(p as u32),
                        object: Some(format!("Font {base}")),
                        xref: (*x != 0).then_some(*x),
                        rect: None,
                        message: format!("Font {base} on page {} doesn't map its characters to Unicode", p + 1),
                    });
                }
            }
        }
        Rule::Bookmarks => {
            let has = dict(cat, "Outlines")?.map(|o| o.get_dict("First")).transpose()?.flatten().is_some();
            if pages.len() > 20 && !has {
                out.push(Finding::doc(format!("{} pages and no bookmarks", pages.len())));
            }
        }
        Rule::TabOrder => {
            for (p, page) in pages.iter().enumerate() {
                if !annots(page)?.is_empty() && name(page, "Tabs")?.as_deref() != Some("S") {
                    out.push(Finding {
                        page: Some(p as u32),
                        object: Some("Page".into()),
                        xref: xref_of(page).ok().filter(|x| *x != 0),
                        rect: None,
                        message: format!("Page {} has links, comments or fields but doesn't tab in structure order", p + 1),
                    });
                }
            }
        }
        Rule::ContentMarking => {
            for (p, scan) in scans.iter().enumerate() {
                let m = &scan.marking;
                let at = |what: &str, message: String| Finding {
                    page: Some(p as u32),
                    object: Some(what.into()),
                    xref: xref_of(&pages[p]).ok().filter(|x| *x != 0),
                    rect: None,
                    message,
                };
                if m.untagged() > 0 {
                    out.push(at("Page content", format!("Page {}: {} neither tagged nor marked as artifact", p + 1, m.describe())));
                }
                if m.artifact_in_tagged > 0 {
                    out.push(at("Artifact", format!("Page {}: {} artifact{} inside tagged content", p + 1, m.artifact_in_tagged, if m.artifact_in_tagged == 1 { " is" } else { "s are" })));
                }
                if m.tagged_in_artifact > 0 {
                    out.push(at("Tagged content", format!("Page {}: {} tagged item{} inside an artifact", p + 1, m.tagged_in_artifact, if m.tagged_in_artifact == 1 { " is" } else { "s are" })));
                }
            }
        }
        Rule::RoleMap => out.extend(matterhorn::role_map(t)),
        Rule::LanguageValues => {
            if let Some(l) = text(cat, "Lang")?
                && !valid_lang(&l)
            {
                out.push(Finding::doc(format!("The document language \"{l}\" is not a valid language tag")));
            }
            for &i in &order {
                if let Some(l) = &t.elems[i].lang
                    && !valid_lang(l)
                {
                    out.push(elem_finding(t, loc, i, &format!("the language \"{l}\" is not a valid language tag")));
                }
            }
        }
        Rule::Metadata => out.extend(matterhorn::metadata(cat)?),
        Rule::AccessibilityPermission => out.extend(matterhorn::permissions(doc)?),
        Rule::TableStructure => out.extend(matterhorn::table_structure(t, loc)),
        Rule::TableScope => out.extend(matterhorn::table_scope(t, loc)),
        Rule::AnnotationsTagged => {
            for (p, page) in pages.iter().enumerate() {
                for (x, a) in annots(page)? {
                    let sub = name(&a, "Subtype")?.unwrap_or_default();
                    if matches!(sub.as_str(), "PrinterMark" | "TrapNet") {
                        continue;
                    }
                    // Hidden and NoView annotations are not presented.
                    if a.get_dict("F")?.and_then(|v| v.as_int().ok()).unwrap_or(0) & (2 | 32) != 0 {
                        continue;
                    }
                    let owner = t.annot_owner.get(&x).copied();
                    let rect = locate::annot_box(page, &a)?;
                    let described = text(&a, "Contents")?.is_some();
                    let mut finding = |why: String| {
                        out.push(Finding {
                            page: Some(p as u32),
                            object: Some(format!("{sub} annotation")),
                            xref: (x != 0).then_some(x),
                            rect,
                            message: format!("{sub} annotation on page {}: {why}", p + 1),
                        })
                    };
                    match (sub.as_str(), owner) {
                        // Links that are not in the tree at all are the tagged-links rule's.
                        ("Link", Some(o)) if t.is(o, "Link") && !t.elems[o].alt && !described => finding("no description (Contents or alternate text)".into()),
                        ("Link", _) => {}
                        ("Widget", None) => finding("not in the tag tree (needs a Form element)".into()),
                        ("Widget", Some(o)) if !t.is(o, "Form") => finding(format!("tagged as {}, not Form", t.elems[o].ty)),
                        ("Widget", _) => {}
                        (_, None) => finding("not in the tag tree (needs an Annot element)".into()),
                        (_, Some(o)) if !t.is(o, "Annot") => finding(format!("tagged as {}, not Annot", t.elems[o].ty)),
                        (_, Some(o)) if !t.elems[o].alt && !described => finding("no description (Contents or alternate text)".into()),
                        _ => {}
                    }
                }
            }
        }
        Rule::FontsEmbedded => {
            let mut seen = HashSet::new();
            for (p, scan) in scans.iter().enumerate() {
                for (x, f) in &scan.fonts {
                    if (*x != 0 && !seen.insert(*x)) || matterhorn::font_embedded(f)? {
                        continue;
                    }
                    let base = name(f, "BaseFont")?.unwrap_or_else(|| "(unnamed)".into());
                    out.push(Finding {
                        page: Some(p as u32),
                        object: Some(format!("Font {base}")),
                        xref: (*x != 0).then_some(*x),
                        rect: None,
                        message: format!("Font {base} on page {} is not embedded", p + 1),
                    });
                }
            }
        }
        Rule::Xfa => out.extend(matterhorn::xfa(cat)?),
        Rule::OptionalContent => out.extend(matterhorn::optional_content(cat)?),
        Rule::ParentTree => {
            for pr in tags::problems(doc)? {
                out.push(Finding { page: pr.page, object: Some("Tag tree".into()), xref: pr.xref, rect: None, message: pr.message });
            }
        }
        Rule::ColorContrast => {}
    }
    Ok(Some(out))
}

// ---- fixes ---------------------------------------------------------------------------------------

/// A safe automatic fix. Each is one undo step. Serialised as `{"fix": "language", "lang": "en-US"}`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "fix", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Fix {
    /// Sets the document title; without one, the file name (without extension).
    Title { title: Option<String> },
    /// Sets the document language (a BCP 47 tag such as `en-US`).
    Language { lang: String },
    /// Viewers show the title in the title bar (ViewerPreferences /DisplayDocTitle).
    DisplayDocTitle,
    /// Every page with annotations tabs in structure order (/Tabs /S).
    TabOrder,
    /// Every field without a tooltip gets one made from its name ("first_name" -> "First name").
    FieldTooltips,
    /// Alternate text for the figure element with this object number (from the report).
    FigureAlt { xref: i32, alt: String },
    /// Best-effort tags for an untagged document (see `autotag`).
    Autotag,
    /// Adds the PDF/UA identification and the title to the XMP metadata (made if the file has none).
    Metadata,
    /// Rebuilds the ParentTree (page keys, MCID entries, annotation keys) from the tag tree.
    ParentTree,
    /// Several of the fixes above as one undo step ("Fix all safe issues"). Each that has nothing
    /// to do is skipped; the step is refused only when none applied.
    Many { fixes: Vec<Fix> },
}

impl Fix {
    pub fn label(&self) -> String {
        match self {
            Fix::Title { .. } => "Set document title".into(),
            Fix::Language { lang } => format!("Set document language to {}", lang.trim()),
            Fix::DisplayDocTitle => "Show document title in title bar".into(),
            Fix::TabOrder => "Set tab order to structure order".into(),
            Fix::FieldTooltips => "Add form field tooltips".into(),
            Fix::FigureAlt { .. } => "Set figure alternate text".into(),
            Fix::Autotag => "Autotag (best effort)".into(),
            Fix::ParentTree => "Repair tag tree links".into(),
            Fix::Metadata => "Add PDF/UA metadata".into(),
            Fix::Many { fixes } => format!("Fix {} accessibility issue{}", fixes.len(), if fixes.len() == 1 { "" } else { "s" }),
        }
    }
}

/// A PDF text string: literal when it is plain ASCII, else UTF-16BE with a byte order mark.
pub(crate) fn text_obj(doc: &PdfDocument, s: &str) -> Result<PdfObject> {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    if s.is_ascii() {
        return Ok(PdfObject::new_string(&s)?);
    }
    let hex: String = s.encode_utf16().map(|u| format!("{u:04X}")).collect();
    Ok(doc.new_object_from_str(&format!("<FEFF{hex}>"))?)
}

/// "first_name" -> "First name", "dateOfBirth" -> "Date of birth".
pub fn tooltip_from_name(n: &str) -> String {
    let mut words = String::new();
    let mut prev_lower = false;
    for c in n.chars() {
        if matches!(c, '_' | '-' | '.' | ' ') {
            words.push(' ');
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            words.push(' ');
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        words.extend(c.to_lowercase());
    }
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut cs = words.chars();
    cs.next().map(|f| f.to_uppercase().chain(cs).collect()).unwrap_or_default()
}

fn valid_lang(l: &str) -> bool {
    let mut parts = l.split('-');
    let first = parts.next().unwrap_or("");
    (2..=8).contains(&first.len())
        && first.chars().all(|c| c.is_ascii_alphabetic())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Applies `fix` to `doc`; returns the pages it touched. `file_stem` names an untitled document.
/// Fails, changing nothing that matters, when there is nothing to fix.
pub fn apply_fix(doc: &mut PdfDocument, fix: &Fix, file_stem: &str) -> Result<Vec<u32>> {
    apply_fix_with(doc, fix, file_stem, &mut |_| {}, &AtomicBool::new(false))
}

/// [`apply_fix`] with progress and cancel, which only Autotag uses (a cancelled step changes nothing).
pub fn apply_fix_with(doc: &mut PdfDocument, fix: &Fix, file_stem: &str, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Vec<u32>> {
    let mut cat = doc.catalog()?;
    match fix {
        Fix::Title { title } => {
            let t = title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(file_stem).trim().to_owned();
            if t.is_empty() {
                return Err(bad("give the document title"));
            }
            let mut info = match doc.trailer()?.get_dict("Info")?.filter(|i| i.is_dict().unwrap_or(false)) {
                Some(i) => i,
                None => {
                    // Adding the object starts the incremental section; only then is the trailer
                    // the one the update writes.
                    let i = doc.add_object(&doc.new_dict()?)?;
                    doc.trailer()?.dict_put("Info", i.clone())?;
                    i
                }
            };
            info.dict_put("Title", text_obj(doc, &t)?)?;
            Ok(Vec::new())
        }
        Fix::Language { lang } => {
            let l = lang.trim();
            if !valid_lang(l) {
                return Err(bad(format!("\"{l}\" is not a language tag (for example en-US)")));
            }
            cat.dict_put("Lang", PdfObject::new_string(l)?)?;
            Ok(Vec::new())
        }
        Fix::DisplayDocTitle => {
            match dict(&cat, "ViewerPreferences")? {
                Some(mut v) => v.dict_put("DisplayDocTitle", PdfObject::new_bool(true))?,
                None => cat.dict_put("ViewerPreferences", doc.new_object_from_str("<</DisplayDocTitle true>>")?)?,
            }
            Ok(Vec::new())
        }
        Fix::TabOrder => {
            let mut touched = Vec::new();
            for p in 0..doc.page_count()? {
                let mut page = doc.find_page(p)?;
                if !annots(&page)?.is_empty() && name(&page, "Tabs")?.as_deref() != Some("S") {
                    page.dict_put("Tabs", PdfObject::new_name("S")?)?;
                    touched.push(p as u32);
                }
            }
            if touched.is_empty() {
                return Err(bad("every page already tabs in structure order"));
            }
            Ok(touched)
        }
        Fix::FieldTooltips => {
            let mut touched = Vec::new();
            let mut seen = HashSet::new();
            for p in 0..doc.page_count()? {
                let page = doc.find_page(p)?;
                for (_, a) in annots(&page)? {
                    if name(&a, "Subtype")?.as_deref() != Some("Widget") {
                        continue;
                    }
                    let Some(mut field) = field_of(&a)? else { continue };
                    let x = xref_of(&field)?;
                    if (x != 0 && !seen.insert(x)) || text(&field, "TU")?.is_some() {
                        continue;
                    }
                    // The same name the checker reports: a terminal field may take it from an ancestor.
                    let tip = tooltip_from_name(&full_name(&field)?);
                    if tip.is_empty() {
                        continue;
                    }
                    field.dict_put("TU", text_obj(doc, &tip)?)?;
                    if touched.last() != Some(&(p as u32)) {
                        touched.push(p as u32);
                    }
                }
            }
            if touched.is_empty() {
                return Err(bad("no field is missing a tooltip"));
            }
            Ok(touched)
        }
        Fix::FigureAlt { xref, alt } => {
            let alt = alt.trim();
            if alt.is_empty() {
                return Err(bad("give the alternate text"));
            }
            let index = tree::page_index(doc)?;
            let t = Tree::read(doc, &index)?;
            let Some(e) = t.elems.iter().find(|e| e.xref == *xref && *xref != 0) else {
                return Err(bad(format!("object {xref} is not a structure element")));
            };
            if e.ty != "Figure" {
                return Err(bad(format!("object {xref} is a {} element, not a Figure", e.ty)));
            }
            let page = e.page;
            let mut obj = doc.new_indirect(*xref, 0)?;
            obj.dict_put("Alt", text_obj(doc, alt)?)?;
            Ok(page.into_iter().collect())
        }
        Fix::Autotag => autotag::run(doc, progress, cancel),
        Fix::ParentTree => tags::repair_parent_tree(doc),
        Fix::Metadata => {
            let info = doc.trailer()?.get_dict("Info")?.filter(|i| i.is_dict().unwrap_or(false));
            let title = info.map(|i| text(&i, "Title")).transpose()?.flatten().unwrap_or_else(|| file_stem.trim().to_owned());
            if title.is_empty() {
                return Err(bad("give the document a title first"));
            }
            let current = matterhorn::xmp_strict(&cat)?;
            let new = match &current {
                None => matterhorn::new_xmp(&title),
                Some(x) => matterhorn::amend_xmp(x, &title)?,
            };
            if current.as_deref() == Some(new.as_str()) {
                return Err(bad("the metadata already identifies the file as PDF/UA and has a title"));
            }
            let mut stm = doc.add_stream(&mupdf::Buffer::from_bytes(new.as_bytes())?, None, false)?;
            stm.dict_put("Type", PdfObject::new_name("Metadata")?)?;
            stm.dict_put("Subtype", PdfObject::new_name("XML")?)?;
            cat.dict_put("Metadata", stm)?;
            Ok(Vec::new())
        }
        Fix::Many { fixes } => {
            let mut touched: Vec<u32> = Vec::new();
            let mut applied = 0;
            for f in fixes {
                if matches!(f, Fix::Many { .. } | Fix::Autotag) {
                    return Err(bad("a group of fixes holds only the simple fixes"));
                }
                match apply_fix_with(doc, f, file_stem, progress, cancel) {
                    Ok(t) => {
                        applied += 1;
                        touched.extend(t);
                    }
                    // Nothing to do for this one (already set, invalid input): the others still apply.
                    Err(EngineError::Invalid(_)) => {}
                    Err(e) => return Err(e),
                }
            }
            if applied == 0 {
                return Err(bad("nothing to fix"));
            }
            touched.sort_unstable();
            touched.dedup();
            Ok(touched)
        }
    }
}

// ---- engine entry points -------------------------------------------------------------------------

/// Checks the document as it is now (unsaved edits included). Reads its own copy of the current
/// generation (the published snapshot, or the file before any edit), so edits are not held up
/// while it runs. `cancel` is polled between pages.
pub fn check_doc(eng: &Engine, doc: DocId, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Report> {
    let d = open_copy(eng, doc)?;
    check(&d, progress, cancel)
}

/// What Autotag would add to the document as it is now (counted, not run).
pub fn preview_doc(eng: &Engine, doc: DocId) -> Result<autotag::Preview> {
    autotag::preview(&open_copy(eng, doc)?)
}

/// The report as a file: HTML or a PDF. It checks the document again, so the file matches what is open now.
pub fn report_bytes(eng: &Engine, doc: DocId, pdf: bool, date: &str, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Vec<u8>> {
    let report = check_doc(eng, doc, progress, cancel)?;
    let path = eng.doc_entry(doc)?.path();
    let name = std::path::Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if pdf { report::pdf(&name, date, &report) } else { Ok(report::html(&name, date, &report).into_bytes()) }
}

/// The document as it is now, in a copy of its own (the published snapshot, or the file before any edit).
pub(crate) fn open_copy(eng: &Engine, doc: DocId) -> Result<PdfDocument> {
    let (_, source) = eng.doc_source(doc)?;
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
    Ok(d)
}

/// Every accessibility write goes through here: a certified document is refused (any change to it, the tags
/// included, voids the certification; as outline/mod.rs does), and the step is never one that rewrites the file,
/// so a signed document's save stays incremental and its signed bytes stay as they were.
fn edit_doc(eng: &Engine, doc: DocId, label: String, source: EditSource, f: impl FnOnce(&mut PdfDocument) -> Result<Vec<u32>>) -> Result<EditState> {
    eng.apply_custom(doc, label, source, false, |d| {
        if crate::outline::certified(d)?.is_some() {
            return Err(bad(
                "This document is certified. Changing its tags or reading order would void the certification, so Inkwell leaves them as they are.",
            ));
        }
        f(d)
    })
}

/// Whether the document is certified or signed, as it is now: what the edit controls are gated on, known
/// without the check having run.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Protection {
    pub certified: bool,
    pub signed: bool,
}

pub fn protection_doc(eng: &Engine, doc: DocId) -> Result<Protection> {
    eng.read_custom(doc, |d| Ok(Protection { certified: crate::outline::certified(d).map_or(true, |c| c.is_some()), signed: crate::outline::signed(d) }))
}

/// Applies one fix as one undo step.
pub fn fix_doc(eng: &Engine, doc: DocId, fix: &Fix, source: EditSource) -> Result<EditState> {
    fix_doc_with(eng, doc, fix, source, &mut |_| {}, &AtomicBool::new(false))
}

/// [`fix_doc`] with progress and cancel (Autotag reports per page; a cancelled run leaves the document as it was).
pub fn fix_doc_with(eng: &Engine, doc: DocId, fix: &Fix, source: EditSource, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<EditState> {
    let path = eng.doc_entry(doc)?.path();
    let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    edit_doc(eng, doc, fix.label(), source, |d| apply_fix_with(d, fix, &stem, progress, cancel))
}

// ---- reading order and tags ----------------------------------------------------------------------

/// The regions of a page in reading order, with a text preview and a box each (see `order`). Reads its own
/// copy of the current generation, like the check.
pub fn reading_order_doc(eng: &Engine, doc: DocId, page: u32) -> Result<order::ReadingOrder> {
    order::reading_order(&open_copy(eng, doc)?, page)
}

/// The tag tree, nested; with `page`, only the branches that hold content on that page.
pub fn tags_doc(eng: &Engine, doc: DocId, page: Option<u32>) -> Result<Vec<tags::TagNode>> {
    tags::tags(&open_copy(eng, doc)?, page)
}

/// Where the ParentTree and the tags disagree (empty: they agree).
pub fn tag_problems_doc(eng: &Engine, doc: DocId) -> Result<Vec<Problem>> {
    tags::problems(&open_copy(eng, doc)?)
}

/// Puts the regions of a page in a new reading order, as one undo step.
pub fn reorder_doc(eng: &Engine, doc: DocId, page: u32, order: &[i32], source: EditSource) -> Result<EditState> {
    edit_doc(eng, doc, "Change reading order".into(), source, |d| tags::reorder(d, page, order))
}

/// Changes an element's role (`Artifact` marks its content as artifact), as one undo step.
pub fn set_role_doc(eng: &Engine, doc: DocId, xref: i32, role: &str, source: EditSource) -> Result<EditState> {
    let label = if role.trim() == "Artifact" { "Mark as artifact".to_string() } else { format!("Change tag to {}", role.trim()) };
    edit_doc(eng, doc, label, source, |d| tags::set_role(d, xref, role))
}

/// Takes an element's content out of the reading order and marks it as artifact, as one undo step.
pub fn artifact_doc(eng: &Engine, doc: DocId, xref: i32, source: EditSource) -> Result<EditState> {
    edit_doc(eng, doc, "Mark as artifact".into(), source, |d| tags::artifact(d, xref))
}

/// Merges one region into another, as one undo step.
pub fn merge_doc(eng: &Engine, doc: DocId, into: i32, from: i32, source: EditSource) -> Result<EditState> {
    edit_doc(eng, doc, "Merge regions".into(), source, |d| tags::merge(d, into, from))
}

/// Most regions one merge takes, `into` included.
const MAX_MERGE: usize = 100;

/// Merges several regions into `into` (in the order given), as one undo step: a refusal part way leaves the
/// document as it was.
pub fn merge_many_doc(eng: &Engine, doc: DocId, into: i32, from: &[i32], source: EditSource) -> Result<EditState> {
    // Each merge reads the tree again (the one before changed it), so the count is bounded.
    if from.is_empty() || from.len() >= MAX_MERGE {
        return Err(bad(format!("choose between two and {MAX_MERGE} regions to merge")));
    }
    edit_doc(eng, doc, "Merge regions".into(), source, |d| {
        let mut pages = Vec::new();
        for &f in from {
            pages.extend(tags::merge(d, into, f)?);
        }
        pages.sort_unstable();
        pages.dedup();
        Ok(pages)
    })
}

/// Splits a region before the content item with this MCID, as one undo step.
pub fn split_doc(eng: &Engine, doc: DocId, xref: i32, at_mcid: i32, role: Option<&str>, source: EditSource) -> Result<EditState> {
    edit_doc(eng, doc, "Split region".into(), source, |d| tags::split(d, xref, at_mcid, role))
}

/// Edits one node's role, alternate text, actual text, language, table scope or headers, as one undo step.
pub fn edit_tag_doc(eng: &Engine, doc: DocId, xref: i32, patch: &tags::TagPatch, source: EditSource) -> Result<EditState> {
    edit_doc(eng, doc, patch.label(), source, |d| tags::edit_tag(d, xref, patch))
}

/// Moves a node under `parent` (0: the top of the tree), before or after its kid `anchor` (last when
/// there is none), as one undo step.
pub fn move_tag_doc(eng: &Engine, doc: DocId, xref: i32, parent: i32, anchor: Option<i32>, after: bool, source: EditSource) -> Result<EditState> {
    edit_doc(eng, doc, "Move tag".into(), source, |d| tags::move_node(d, xref, parent, anchor, after))
}
