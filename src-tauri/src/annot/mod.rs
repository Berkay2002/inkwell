//! Comments and annotations (phase 3a): markup, notes, text boxes, ink, shapes, stamps, replies and
//! review state, written with MuPDF (which also builds the appearance streams other viewers show).
//!
//! Every change is one `Engine::apply_custom` step: one undo step, kept by Save, recorded in history.
//! Geometry is in page space (points, y down, as the viewer shows the page); MuPDF applies the page
//! transform. An annotation is named by `(page, xref)`.
//!
//! Replies are `/Text` annotations with `/IRT` (the parent) and `/RT /R`; a review state is such a
//! reply with `/StateModel /Review` and `/State` (the latest one wins; "Completed" is resolved).
//! Replies and state replies are hidden on the page (they are only listed in the panel).
//! The review-state convention follows pdfcraft's `crates/annot` (ideas only, no code taken).
//! XFDF and FDF export and import of comments is `xfdf.rs` (see docs/comments.md).

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use mupdf::color::AnnotationColor;
use mupdf::pdf::{AnnotationFlags, LineEndingStyle, PdfAnnotation, PdfAnnotationType, PdfDocument, PdfObject};
use mupdf::{Point, Quad, Rect};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub mod app;
pub mod stamps;
#[cfg(test)]
mod tests;
pub mod xfdf;

type Result<T> = std::result::Result<T, EngineError>;

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Note,
    FreeText,
    Ink,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Polygon,
    Stamp,
    Other,
}

/// What to create. Points are `[x, y]`, rects `[x0, y0, x1, y1]`, quads `[ulx, uly, urx, ury, llx, lly, lrx, lry]`.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum NewAnnot {
    /// `markup`: Highlight, Underline, StrikeOut or Squiggly.
    Markup { markup: Kind, quads: Vec<[f32; 8]> },
    Note { at: [f32; 2], text: String },
    FreeText { rect: [f32; 4], text: String, font_size: Option<f32> },
    Ink { strokes: Vec<Vec<[f32; 2]>> },
    /// `shape`: Rectangle and Ellipse take two corners, Line and Arrow two ends, Polygon three or more.
    Shape { shape: Kind, points: Vec<[f32; 2]> },
    /// `name`: Approved, Draft, Confidential, ... or any text for a custom stamp (the date stamp).
    Stamp { name: String, rect: [f32; 4] },
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Style {
    pub color: Option<[f32; 3]>,
    pub fill: Option<[f32; 3]>,
    pub opacity: Option<f32>,
    pub width: Option<f32>,
}

/// Changes to an existing annotation; absent fields stay.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Patch {
    #[serde(flatten)]
    pub style: Style,
    /// Remove the fill colour.
    #[serde(default)]
    pub no_fill: bool,
    pub contents: Option<String>,
    /// Move and resize: the geometry is mapped from the old rect onto this one.
    pub rect: Option<[f32; 4]>,
    pub font_size: Option<f32>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnotDto {
    pub page: u32,
    pub xref: i32,
    pub kind: Kind,
    pub author: String,
    pub contents: String,
    /// PDF date strings as stored (`D:20261011143200Z`); the UI formats them.
    pub created: String,
    pub modified: String,
    pub color: Option<[f32; 3]>,
    pub fill: Option<[f32; 3]>,
    pub opacity: f32,
    pub width: f32,
    pub rect: [f32; 4],
    pub quads: Vec<[f32; 8]>,
    pub points: Vec<[f32; 2]>,
    pub strokes: Vec<Vec<[f32; 2]>>,
    pub icon: Option<String>,
    /// A stamp that draws a picture (a custom stamp): its resize keeps the picture's proportions.
    pub picture: bool,
    pub font_size: Option<f32>,
    /// The annotation this one replies to.
    pub parent: Option<i32>,
    /// The review state of this annotation ("Completed", ...), if one was set.
    pub state: Option<String>,
    pub resolved: bool,
}

// ---- small helpers -----------------------------------------------------------------------------

pub fn rect_of(r: [f32; 4]) -> Rect {
    Rect { x0: r[0].min(r[2]), y0: r[1].min(r[3]), x1: r[0].max(r[2]), y1: r[1].max(r[3]) }
}

fn quad_of(d: &[f32; 8]) -> Quad {
    let p = |i: usize| Point { x: d[i], y: d[i + 1] };
    Quad { ul: p(0), ur: p(2), ll: p(4), lr: p(6) }
}

fn quad_dto(q: &Quad) -> [f32; 8] {
    [q.ul.x, q.ul.y, q.ur.x, q.ur.y, q.ll.x, q.ll.y, q.lr.x, q.lr.y]
}

fn pt(p: [f32; 2]) -> Point {
    Point { x: p[0], y: p[1] }
}

fn rgb(c: [f32; 3]) -> AnnotationColor {
    AnnotationColor::Rgb { red: c[0], green: c[1], blue: c[2] }
}

fn rgb_of(c: Option<AnnotationColor>) -> Option<[f32; 3]> {
    match c? {
        AnnotationColor::Gray(g) => Some([g, g, g]),
        AnnotationColor::Rgb { red, green, blue } => Some([red, green, blue]),
        AnnotationColor::Cmyk { cyan, magenta, yellow, key } => {
            let k = 1.0 - key;
            Some([(1.0 - cyan) * k, (1.0 - magenta) * k, (1.0 - yellow) * k])
        }
    }
}

/// `D:YYYYMMDDHHmmSSZ` for the current time.
pub fn pdf_now() -> String {
    pdf_date(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64))
}

/// `D:YYYYMMDDHHmmSSZ` for `secs` since the Unix epoch (civil date from days: Hinnant's algorithm).
pub fn pdf_date(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn put_str(a: &PdfAnnotation, key: &str, v: &str) -> Result<()> {
    a.object().dict_put(key, PdfObject::new_string(v)?)?;
    Ok(())
}

fn get_str(o: &PdfObject, key: &str) -> String {
    o.get_dict(key).ok().flatten().and_then(|v| v.as_string().ok()).unwrap_or_default()
}

fn default_color(kind: Kind) -> [f32; 3] {
    match kind {
        Kind::Highlight => [0.965, 0.847, 0.376],
        Kind::Underline => [0.18, 0.60, 0.34],
        Kind::StrikeOut => [0.80, 0.20, 0.20],
        Kind::Squiggly => [0.18, 0.60, 0.34],
        Kind::Note => [0.878, 0.663, 0.231],
        Kind::FreeText => [0.10, 0.11, 0.125],
        _ => [0.18, 0.294, 0.561],
    }
}

pub(crate) fn find(doc: &PdfDocument, page: u32, xref: i32) -> Result<(mupdf::pdf::PdfPage, PdfAnnotation)> {
    let p = doc.load_pdf_page(page as i32)?;
    let a = p
        .annotations()
        .find(|a| a.xref().is_ok_and(|x| x == xref))
        .ok_or_else(|| bad(format!("no annotation {xref} on page {}", page + 1)))?;
    Ok((p, a))
}

fn parent_of(o: &PdfObject) -> Option<i32> {
    let v = o.get_dict("IRT").ok().flatten()?;
    v.is_indirect().ok()?.then(|| v.as_indirect().ok()).flatten()
}

fn kind_of(a: &PdfAnnotation) -> Result<Kind> {
    Ok(match a.r#type()? {
        PdfAnnotationType::Highlight => Kind::Highlight,
        PdfAnnotationType::Underline => Kind::Underline,
        PdfAnnotationType::StrikeOut => Kind::StrikeOut,
        PdfAnnotationType::Squiggly => Kind::Squiggly,
        PdfAnnotationType::Text => Kind::Note,
        PdfAnnotationType::FreeText => Kind::FreeText,
        PdfAnnotationType::Ink => Kind::Ink,
        PdfAnnotationType::Square => Kind::Rectangle,
        PdfAnnotationType::Circle => Kind::Ellipse,
        PdfAnnotationType::Polygon => Kind::Polygon,
        PdfAnnotationType::Stamp => Kind::Stamp,
        PdfAnnotationType::Line => {
            let (s, e) = a.line_ending_styles()?;
            if s == LineEndingStyle::None && e == LineEndingStyle::None { Kind::Line } else { Kind::Arrow }
        }
        _ => Kind::Other,
    })
}

/// What is shown in the Comments panel: everything but links, widgets, popups and redaction marks.
fn is_comment(t: PdfAnnotationType) -> bool {
    !matches!(t, PdfAnnotationType::Link | PdfAnnotationType::Widget | PdfAnnotationType::Popup | PdfAnnotationType::Redact)
}

// ---- reading -----------------------------------------------------------------------------------

pub(crate) fn dto(page: u32, a: &PdfAnnotation) -> Result<AnnotDto> {
    let o = a.object();
    let kind = kind_of(a)?;
    let r = a.rect()?;
    let quads = if a.has_quads()? { a.quads()?.iter().map(quad_dto).collect() } else { Vec::new() };
    let points = match kind {
        Kind::Line | Kind::Arrow => {
            let (s, e) = a.line()?;
            vec![[s.x, s.y], [e.x, e.y]]
        }
        Kind::Polygon => a.vertices()?.iter().map(|p| [p.x, p.y]).collect(),
        _ => Vec::new(),
    };
    let strokes = if kind == Kind::Ink {
        a.ink_list()?.iter().map(|s| s.iter().map(|p| [p.x, p.y]).collect()).collect()
    } else {
        Vec::new()
    };
    let state = get_str(&o, "State");
    Ok(AnnotDto {
        page,
        xref: a.xref()?,
        kind,
        author: get_str(&o, "T"),
        contents: get_str(&o, "Contents"),
        created: get_str(&o, "CreationDate"),
        modified: get_str(&o, "M"),
        // A free text's /C is its background; its colour is the text colour in the default appearance.
        color: rgb_of(if kind == Kind::FreeText {
            a.default_appearance().ok().flatten().and_then(|d| d.color)
        } else {
            a.color().ok().flatten()
        }),
        fill: rgb_of(a.interior_color().ok().flatten()),
        opacity: a.opacity().unwrap_or(1.0),
        width: a.border_width().unwrap_or(1.0),
        rect: [r.x0, r.y0, r.x1, r.y1],
        quads,
        points,
        strokes,
        icon: a.icon_name().ok().flatten().map(str::to_owned),
        picture: kind == Kind::Stamp && stamps::picture_size(&o).is_some(),
        font_size: a.default_appearance().ok().flatten().map(|d| d.size),
        parent: parent_of(&o),
        resolved: false,
        state: (!state.is_empty()).then_some(state),
    })
}

/// Annotations loaded for one document by [`list`]: past this the rest of the pages are left out
/// and the list says so (`incomplete`).
const MAX_LOADED: usize = 50_000;
/// Entries [`AnnotWalk`] charges in one document: an indirect object an earlier array listed too, and
/// every direct entry. The first sight of an indirect object is not counted.
const MAX_SCANNED: usize = 2_000_000;

/// The comments of a document, and whether some were left out (past [`MAX_LOADED`] annotations or
/// [`MAX_SCANNED`] repeated array entries).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AnnotList {
    pub items: Vec<AnnotDto>,
    pub incomplete: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Which {
    /// Everything MuPDF puts in `page.annotations()`: not links, popups or widgets.
    Comments,
    /// What `page.widgets()` returns.
    Widgets,
}

/// The annotations a page brings that no earlier page had.
pub(crate) struct Fresh {
    /// Their object numbers.
    pub nums: HashSet<i32>,
    /// The page has annotation dictionaries written inline (object number 0), which are never shared.
    pub direct: bool,
    /// Annotations loading the page makes (every dictionary in `/Annots` but links and popups).
    pub loaded: usize,
}

/// Which pages have annotations that still need listing, read from the page dictionaries alone.
/// Loading a page makes MuPDF create every annotation in its `/Annots`, so a document whose pages
/// share one array (or list the same objects) costs pages x annotations if every page is loaded.
/// An object is assigned to the first page that has it, or to the page its `/P` names when that
/// page has it too. Pages that bring nothing new are not loaded.
///
/// Known limit: a dictionary that two pages list through different arrays, with no `/P` naming the
/// later one, belongs to the first page only. The later page still draws it, but it is not listed
/// or selectable there (and a widget is not fillable there).
#[derive(Default)]
pub(crate) struct AnnotWalk {
    arrays: HashSet<i32>,
    seen: HashSet<i32>,
    /// Page objects walked: a page listed several times in `/Kids` is one object, one walk.
    pages: HashSet<i32>,
    /// What each indirect entry met so far is, so that meeting it again costs no dictionary read.
    class: HashMap<i32, Class>,
    /// What an owner page's array lists, read once per array (by object number when it is indirect).
    listed: HashMap<ListKey, HashSet<i32>>,
    scanned: usize,
    /// The scan cap was hit: pages from then on are left out.
    pub incomplete: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    NotDict,
    /// Loading the page makes no annotation of it.
    Link,
    Widget,
    Other,
}

impl Class {
    fn of(e: &PdfObject) -> Result<Class> {
        if !e.is_dict()? {
            return Ok(Class::NotDict);
        }
        Ok(match e.get_dict("Subtype")?.and_then(|s| s.as_name().ok()).as_deref() {
            Some(b"Link" | b"Popup") => Class::Link,
            Some(b"Widget") => Class::Widget,
            _ => Class::Other,
        })
    }
}

#[derive(Hash, PartialEq, Eq)]
enum ListKey {
    /// An indirect `/Annots` array, by object number.
    Array(i32),
    /// A direct one, by the page that has it.
    Page(i32),
}

impl AnnotWalk {
    /// Counts one repeated entry; true once the budget is gone.
    fn repeat(&mut self) -> bool {
        self.scanned += 1;
        if self.scanned > MAX_SCANNED {
            self.incomplete = true;
        }
        self.incomplete
    }

    /// `None` when loading page `p` would add nothing (no annotations, or all assigned already).
    pub fn page(&mut self, doc: &PdfDocument, p: u32, which: Which) -> Result<Option<Fresh>> {
        if self.incomplete {
            return Ok(None);
        }
        let page = doc.find_page(p as i32)?;
        if page.is_indirect()? && !self.pages.insert(page.as_indirect()?) {
            return Ok(None);
        }
        let Some(annots) = page.get_dict("Annots")? else { return Ok(None) };
        if !annots.is_array()? {
            return Ok(None);
        }
        // One array shared by pages (an indirect /Annots): its objects went to the first of them.
        if annots.is_indirect()? && !self.arrays.insert(annots.as_indirect()?) {
            return Ok(None);
        }
        let mut fresh = Fresh { nums: HashSet::new(), direct: false, loaded: 0 };
        for i in 0..annots.len()? {
            let Some(e) = annots.get_array(i as i32)? else { continue };
            let num = if e.is_indirect()? { Some(e.as_indirect()?) } else { None };
            let cached = num.and_then(|n| self.class.get(&n).copied());
            // Only the first sight of an indirect object is free; a direct entry is never shared, but
            // a page object listed twice (or many inline dictionaries) would still cost per page.
            if (num.is_none() || cached.is_some()) && self.repeat() {
                return Ok(None);
            }
            let class = match cached {
                Some(c) => c,
                None => {
                    let c = Class::of(&e)?;
                    if let Some(n) = num {
                        self.class.insert(n, c);
                    }
                    c
                }
            };
            if matches!(class, Class::Other | Class::Widget) {
                fresh.loaded += 1;
            }
            let wanted = match which {
                Which::Widgets => class == Class::Widget,
                Which::Comments => class == Class::Other,
            };
            if !wanted {
                continue;
            }
            let Some(num) = num else {
                fresh.direct = true;
                continue;
            };
            let later = self.seen.contains(&num) || self.belongs_later(doc, &e, p, num)?;
            // Out of budget mid-check the answer above is a guess: stop here instead of listing from it.
            if self.incomplete {
                return Ok(None);
            }
            if later {
                continue;
            }
            self.seen.insert(num);
            fresh.nums.insert(num);
        }
        Ok((fresh.direct || !fresh.nums.is_empty()).then_some(fresh))
    }

    /// The annotation names a later page in its `/P` and that page lists it too.
    fn belongs_later(&mut self, doc: &PdfDocument, e: &PdfObject, p: u32, num: i32) -> Result<bool> {
        let Some(owner) = e.get_dict("P")?.filter(|o| o.is_dict().unwrap_or(false)) else { return Ok(false) };
        let q = doc.lookup_page_number(&owner).unwrap_or(-1);
        if q <= p as i32 || q >= doc.page_count()? {
            return Ok(false);
        }
        let Some(list) = doc.find_page(q)?.get_dict("Annots")?.filter(|a| a.is_array().unwrap_or(false)) else { return Ok(false) };
        let key = if list.is_indirect()? { ListKey::Array(list.as_indirect()?) } else { ListKey::Page(q) };
        // That page's array is the one being walked or was walked: it will not be visited again, so
        // the annotation stays with the first page that has it.
        if matches!(key, ListKey::Array(a) if self.arrays.contains(&a)) {
            return Ok(false);
        }
        if let Some(nums) = self.listed.get(&key) {
            return Ok(nums.contains(&num));
        }
        let mut nums = HashSet::new();
        for i in 0..list.len()? {
            // Each entry read is charged: many annotations naming many other pages, each with a long array, would
            // otherwise cost n x m reads. Out of budget the walk is incomplete and this one stays where it is.
            if self.repeat() {
                return Ok(false);
            }
            if let Some(x) = list.get_array(i as i32)?.filter(|x| x.is_indirect().unwrap_or(false)) {
                nums.insert(x.as_indirect()?);
            }
        }
        let found = nums.contains(&num);
        self.listed.insert(key, nums);
        Ok(found)
    }
}

/// Every comment of the document in page order. A review-state reply is folded into its parent's
/// `state`/`resolved` (the latest wins) and not listed on its own. An annotation object that
/// several pages list (a shared `/Annots`) is listed once, on one page; see [`AnnotWalk`] for
/// which page and for the case that is left to the first.
pub fn list(doc: &PdfDocument) -> Result<AnnotList> {
    let (all, incomplete) = each(doc, |p, a| dto(p, a))?;
    let mut items: Vec<AnnotDto> = Vec::new();
    let mut states: Vec<(i32, String)> = Vec::new();
    for d in all {
        match (&d.state, d.parent) {
            (Some(s), Some(parent)) => states.push((parent, s.clone())),
            _ => items.push(d),
        }
    }
    for (parent, s) in states {
        if let Some(d) = items.iter_mut().find(|d| d.xref == parent) {
            d.resolved = s == "Completed";
            d.state = Some(s);
        }
    }
    Ok(AnnotList { items, incomplete })
}

/// `f` for every comment annotation (everything [`is_comment`]) in page order, once each; and whether
/// some pages were left out (see [`AnnotWalk`] for which page an object belongs to and the caps).
pub(crate) fn each<T>(doc: &PdfDocument, mut f: impl FnMut(u32, &PdfAnnotation) -> Result<T>) -> Result<(Vec<T>, bool)> {
    let mut all = Vec::new();
    let mut walk = AnnotWalk::default();
    let mut loaded = 0usize;
    let mut left_out = false;
    for p in 0..doc.page_count()?.max(0) as u32 {
        let Some(fresh) = walk.page(doc, p, Which::Comments)? else { continue };
        if loaded + fresh.loaded > MAX_LOADED {
            left_out = true;
            continue;
        }
        loaded += fresh.loaded;
        let mut pending = fresh.nums;
        let page = doc.load_pdf_page(p as i32)?;
        for a in page.annotations() {
            let x = a.xref()?;
            let ours = if x == 0 { fresh.direct } else { pending.remove(&x) };
            // Fill & Sign items are annotations too; fillsign lists them, not as comments.
            if ours && a.r#type().is_ok_and(is_comment) && a.object().get_dict(crate::fillsign::MARK).ok().flatten().is_none() {
                all.push(f(p, &a)?);
            }
        }
    }
    Ok((all, left_out || walk.incomplete))
}

// ---- writing -----------------------------------------------------------------------------------

/// The OS login name (Windows `USERNAME`, else `USER`), trimmed; empty when neither is set.
pub fn os_user(var: impl Fn(&str) -> Option<String>) -> String {
    ["USERNAME", "USER"].iter().filter_map(|k| var(k)).map(|v| v.trim().to_owned()).find(|v| !v.is_empty()).unwrap_or_default()
}

pub(crate) fn meta(a: &mut PdfAnnotation, author: &str, created: bool) -> Result<()> {
    if !author.is_empty() {
        a.set_author(author)?;
    }
    let now = pdf_now();
    put_str(a, "M", &now)?;
    if created {
        put_str(a, "CreationDate", &now)?;
        // The name XFDF and FDF files match a comment by (an import updates the comment with the same one).
        put_str(a, "NM", &new_name())?;
    }
    Ok(())
}

/// A random 128-bit name in UUID text form, for `/NM`.
fn new_name() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("system random source");
    b[6] = b[6] & 0x0F | 0x40;
    b[8] = b[8] & 0x3F | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

fn style(a: &mut PdfAnnotation, kind: Kind, s: &Style) -> Result<()> {
    if kind != Kind::FreeText {
        a.set_color(rgb(s.color.unwrap_or_else(|| default_color(kind))))?;
    }
    // MuPDF throws for a fill or a border on a type that has none (a note, a markup, a stamp), which would
    // leave the annotation half made: an import can carry both for any type.
    let closed = matches!(kind, Kind::Rectangle | Kind::Ellipse | Kind::Line | Kind::Arrow | Kind::Polygon);
    if let Some(f) = s.fill.filter(|_| closed) {
        a.set_interior_color(rgb(f))?;
    }
    if let Some(o) = s.opacity {
        a.set_opacity(o.clamp(0.0, 1.0))?;
    }
    if let Some(w) = s.width.filter(|_| closed || matches!(kind, Kind::Ink | Kind::FreeText)) {
        a.set_border_width(w.max(0.0))?;
    }
    Ok(())
}

fn hide_reply(a: &mut PdfAnnotation, parent: i32, doc: &PdfDocument, state: Option<(&str, &str)>) -> Result<()> {
    let mut o2 = a.object();
    o2.dict_put("IRT", doc.new_indirect(parent, 0)?)?;
    o2.dict_put("RT", PdfObject::new_name("R")?)?;
    if let Some((model, st)) = state {
        o2.dict_put("StateModel", PdfObject::new_string(model)?)?;
        o2.dict_put("State", PdfObject::new_string(st)?)?;
    }
    a.set_flags(AnnotationFlags::IS_HIDDEN | AnnotationFlags::IS_PRINT)?;
    Ok(())
}

/// Creates the annotation on `page` and returns its xref.
pub fn add(doc: &mut PdfDocument, page: u32, what: &NewAnnot, st: &Style, author: &str) -> Result<i32> {
    crate::outline::refuse_unless_annotating_allowed(doc, "Adding a comment")?;
    let mut p = doc.load_pdf_page(page as i32)?;
    let (mut a, kind) = match what {
        NewAnnot::Markup { markup: kind, quads } => {
            if quads.is_empty() {
                return Err(bad("a text markup needs at least one quad"));
            }
            let q: Vec<Quad> = quads.iter().map(quad_of).collect();
            let a = match kind {
                Kind::Highlight => p.add_highlight_annotation(q)?,
                Kind::Underline => p.add_underline_annotation(q)?,
                Kind::StrikeOut => p.add_strikeout_annotation(q)?,
                Kind::Squiggly => p.add_squiggly_annotation(q)?,
                k => return Err(bad(format!("{k:?} is not a text markup"))),
            };
            (a, *kind)
        }
        NewAnnot::Note { at, text } => {
            let r = Rect { x0: at[0], y0: at[1], x1: at[0] + 22.0, y1: at[1] + 22.0 };
            (p.add_text_annotation(r, text)?, Kind::Note)
        }
        NewAnnot::FreeText { rect, text, font_size } => {
            let mut a = p.add_free_text_annotation(rect_of(*rect), text)?;
            let color = st.color.unwrap_or_else(|| default_color(Kind::FreeText));
            a.set_default_appearance("Helv", font_size.unwrap_or(12.0), Some(rgb(color)))?;
            a.set_border_width(st.width.unwrap_or(0.0))?;
            (a, Kind::FreeText)
        }
        NewAnnot::Ink { strokes } => {
            let s = strokes.iter().map(|s| s.iter().map(|p| pt(*p)).collect::<Vec<_>>());
            (p.add_ink_annotation(s)?, Kind::Ink)
        }
        NewAnnot::Shape { shape, points } => {
            let pts: Vec<Point> = points.iter().map(|p| pt(*p)).collect();
            let corners = |n: usize| if pts.len() == n { Ok(()) } else { Err(bad(format!("{shape:?} needs {n} points"))) };
            let a = match shape {
                Kind::Rectangle | Kind::Ellipse => {
                    corners(2)?;
                    let r = Rect { x0: pts[0].x.min(pts[1].x), y0: pts[0].y.min(pts[1].y), x1: pts[0].x.max(pts[1].x), y1: pts[0].y.max(pts[1].y) };
                    if *shape == Kind::Rectangle { p.add_rect_annotation(r)? } else { p.add_circle_annotation(r)? }
                }
                Kind::Line | Kind::Arrow => {
                    corners(2)?;
                    let mut a = p.add_line_annotation(pts[0], pts[1])?;
                    if *shape == Kind::Arrow {
                        a.set_line_ending_styles(LineEndingStyle::None, LineEndingStyle::OpenArrow)?;
                    }
                    a
                }
                Kind::Polygon => {
                    if pts.len() < 3 {
                        return Err(bad("a polygon needs at least three points"));
                    }
                    p.add_polygon_annotation(pts)?
                }
                k => return Err(bad(format!("{k:?} is not a shape"))),
            };
            (a, *shape)
        }
        NewAnnot::Stamp { name, rect } => (p.add_stamp_annotation(rect_of(*rect), name)?, Kind::Stamp),
    };
    if kind != Kind::Stamp {
        let mut s = st.clone();
        if kind == Kind::FreeText {
            s.width = None;
        }
        style(&mut a, kind, &s)?;
    }
    meta(&mut a, author, true)?;
    a.update()?;
    Ok(a.xref()?)
}

/// Maps `p` from the box `from` onto the box `to` (a move when the sizes match).
fn map_point(p: Point, from: Rect, to: Rect) -> Point {
    let sx = if from.x1 - from.x0 > 1e-3 { (to.x1 - to.x0) / (from.x1 - from.x0) } else { 1.0 };
    let sy = if from.y1 - from.y0 > 1e-3 { (to.y1 - to.y0) / (from.y1 - from.y0) } else { 1.0 };
    Point { x: to.x0 + (p.x - from.x0) * sx, y: to.y0 + (p.y - from.y0) * sy }
}

/// `turns` is the page's `/Rotate` in quarter turns: a picture stamp's pixels are stored turned back by it.
fn reshape(a: &mut PdfAnnotation, kind: Kind, to: Rect, turns: u32) -> Result<()> {
    let from = a.rect()?;
    let m = |p: Point| map_point(p, from, to);
    match kind {
        Kind::Highlight | Kind::Underline | Kind::StrikeOut | Kind::Squiggly => {
            let q: Vec<Quad> = a
                .quads()?
                .iter()
                .map(|q| Quad { ul: m(q.ul), ur: m(q.ur), ll: m(q.ll), lr: m(q.lr) })
                .collect();
            a.set_quad_points(q)?;
        }
        Kind::Line | Kind::Arrow => {
            let (s, e) = a.line()?;
            a.set_line(m(s), m(e))?;
        }
        Kind::Polygon => {
            let v: Vec<Point> = a.vertices()?.into_iter().map(m).collect();
            a.set_vertices(v)?;
        }
        Kind::Ink => {
            let strokes: Vec<Vec<Point>> = a.ink_list()?.into_iter().map(|s| s.into_iter().map(m).collect()).collect();
            a.set_ink_list(strokes.iter().map(|s| s.iter().copied()))?;
        }
        // A picture stamp keeps the picture's proportions whatever box was asked for.
        Kind::Stamp => {
            let fit = stamps::picture_size(&a.object()).map_or(to, |(w, h)| {
                let (w, h) = if turns % 2 == 1 { (h, w) } else { (w, h) };
                stamps::fit_aspect(from, to, w as f32 / h as f32)
            });
            a.set_rect(fit)?;
        }
        // Rect-based kinds; the others derive their /Rect from the geometry above.
        _ => a.set_rect(to)?,
    }
    Ok(())
}

pub fn edit(doc: &mut PdfDocument, page: u32, xref: i32, patch: &Patch, author: &str) -> Result<()> {
    crate::outline::refuse_unless_annotating_allowed(doc, "Changing a comment")?;
    let (p, mut a) = find(doc, page, xref)?;
    let kind = kind_of(&a)?;
    let s = &patch.style;
    if let Some(c) = s.color {
        // A free text's colour is its text colour (its /C would fill the box).
        if kind != Kind::FreeText {
            a.set_color(rgb(c))?;
        } else {
            let size = a.default_appearance()?.map_or(12.0, |d| d.size);
            a.set_default_appearance("Helv", size, Some(rgb(c)))?;
        }
    }
    if let Some(f) = s.fill {
        a.set_interior_color(rgb(f))?;
    }
    if patch.no_fill {
        a.object().dict_delete("IC")?;
    }
    if let Some(o) = s.opacity {
        a.set_opacity(o.clamp(0.0, 1.0))?;
    }
    if let Some(w) = s.width {
        a.set_border_width(w.max(0.0))?;
    }
    if let Some(t) = &patch.contents {
        a.set_contents(t)?;
    }
    if let Some(sz) = patch.font_size {
        let c = a.default_appearance()?.and_then(|d| d.color).unwrap_or(rgb(default_color(Kind::FreeText)));
        a.set_default_appearance("Helv", sz, Some(c))?;
    }
    if let Some(r) = patch.rect {
        reshape(&mut a, kind, rect_of(r), stamps::quarter_turns(&p)?)?;
    }
    meta(&mut a, author, false)?;
    a.update()?;
    Ok(())
}

/// Deletes the annotations and everything that replies to them.
pub fn delete(doc: &mut PdfDocument, page: u32, xrefs: &[i32]) -> Result<()> {
    crate::outline::refuse_unless_annotating_allowed(doc, "Deleting a comment")?;
    let mut p = doc.load_pdf_page(page as i32)?;
    let mut doomed: Vec<i32> = xrefs.to_vec();
    loop {
        let more: Vec<i32> = p
            .annotations()
            .filter_map(|a| {
                let x = a.xref().ok()?;
                (!doomed.contains(&x) && parent_of(&a.object()).is_some_and(|q| doomed.contains(&q))).then_some(x)
            })
            .collect();
        if more.is_empty() {
            break;
        }
        doomed.extend(more);
    }
    let found: Vec<PdfAnnotation> = p.annotations().filter(|a| a.xref().is_ok_and(|x| doomed.contains(&x))).collect();
    if found.len() < xrefs.len() {
        return Err(bad(format!("no such annotation on page {}", page + 1)));
    }
    for a in found {
        p.delete_annotation(a)?;
    }
    Ok(())
}

/// A reply (a hidden `/Text` with `/IRT`) to `parent`; returns its xref.
pub fn reply(doc: &mut PdfDocument, page: u32, parent: i32, text: &str, author: &str) -> Result<i32> {
    crate::outline::refuse_unless_annotating_allowed(doc, "Replying to a comment")?;
    let (mut p, pa) = find(doc, page, parent)?;
    let r = pa.rect()?;
    let mut a = p.add_text_annotation(Rect { x0: r.x0, y0: r.y0, x1: r.x0 + 22.0, y1: r.y0 + 22.0 }, text)?;
    hide_reply(&mut a, parent, doc, None)?;
    meta(&mut a, author, true)?;
    Ok(a.xref()?)
}

/// Sets the review state of `parent` ("Completed" = resolved; "None" reopens) as a new state reply.
pub fn set_state(doc: &mut PdfDocument, page: u32, parent: i32, state: &str, author: &str) -> Result<i32> {
    crate::outline::refuse_unless_annotating_allowed(doc, "Changing a comment's status")?;
    if !["None", "Accepted", "Rejected", "Cancelled", "Completed"].contains(&state) {
        return Err(bad(format!("unknown review state {state}")));
    }
    let (mut p, pa) = find(doc, page, parent)?;
    let r = pa.rect()?;
    let mut a = p.add_text_annotation(Rect { x0: r.x0, y0: r.y0, x1: r.x0 + 22.0, y1: r.y0 + 22.0 }, "")?;
    hide_reply(&mut a, parent, doc, Some(("Review", state)))?;
    meta(&mut a, author, true)?;
    Ok(a.xref()?)
}

// ---- engine wiring: one undo step each --------------------------------------------------------

fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn noun(what: &NewAnnot) -> &'static str {
    match what {
        NewAnnot::Markup { markup: Kind::Highlight, .. } => "highlight",
        NewAnnot::Markup { markup: Kind::Underline, .. } => "underline",
        NewAnnot::Markup { markup: Kind::StrikeOut, .. } => "strikethrough",
        NewAnnot::Markup { .. } => "squiggly underline",
        NewAnnot::Note { .. } => "note",
        NewAnnot::FreeText { .. } => "text box",
        NewAnnot::Ink { .. } => "drawing",
        NewAnnot::Shape { shape: Kind::Rectangle, .. } => "rectangle",
        NewAnnot::Shape { shape: Kind::Ellipse, .. } => "ellipse",
        NewAnnot::Shape { shape: Kind::Line, .. } => "line",
        NewAnnot::Shape { shape: Kind::Arrow, .. } => "arrow",
        NewAnnot::Shape { .. } => "polygon",
        NewAnnot::Stamp { .. } => "stamp",
    }
}

pub fn add_one(eng: &Engine, doc: DocId, page: u32, what: &NewAnnot, st: &Style, author: &str, source: EditSource) -> Result<(EditState, i32)> {
    let mut xref = 0;
    let state = eng.apply_custom(doc, format!("Add {} on p. {}", noun(what), page + 1), source, false, |d| {
        xref = add(d, page, what, st, author)?;
        Ok(vec![page])
    })?;
    Ok((state, xref))
}

pub fn edit_one(eng: &Engine, doc: DocId, page: u32, xref: i32, patch: &Patch, author: &str, source: EditSource) -> Result<EditState> {
    let what = if patch.rect.is_some() { "Move or resize annotation" } else if patch.contents.is_some() { "Edit comment" } else { "Change annotation" };
    eng.apply_custom(doc, format!("{what} on p. {}", page + 1), source, false, |d| {
        edit(d, page, xref, patch, author)?;
        Ok(vec![page])
    })
}

/// `items`: `(page, xref)`; one undo step.
pub fn delete_many(eng: &Engine, doc: DocId, items: &[(u32, i32)], source: EditSource) -> Result<EditState> {
    let items = items.to_vec();
    let mut pages: Vec<u32> = items.iter().map(|i| i.0).collect();
    pages.sort_unstable();
    pages.dedup();
    eng.apply_custom(doc, format!("Delete {} comment{}", items.len(), s(items.len())), source, false, |d| {
        for &pg in &pages {
            let xs: Vec<i32> = items.iter().filter(|i| i.0 == pg).map(|i| i.1).collect();
            delete(d, pg, &xs)?;
        }
        Ok(pages.clone())
    })
}

pub fn reply_one(eng: &Engine, doc: DocId, page: u32, parent: i32, text: &str, author: &str, source: EditSource) -> Result<(EditState, i32)> {
    let mut xref = 0;
    let state = eng.apply_custom(doc, format!("Reply on p. {}", page + 1), source, false, |d| {
        xref = reply(d, page, parent, text, author)?;
        Ok(vec![page])
    })?;
    Ok((state, xref))
}

pub fn state_one(eng: &Engine, doc: DocId, page: u32, parent: i32, state: &str, author: &str, source: EditSource) -> Result<EditState> {
    let label = if state == "Completed" { "Resolve comment" } else if state == "None" { "Reopen comment" } else { "Set comment status" };
    eng.apply_custom(doc, format!("{label} on p. {}", page + 1), source, false, |d| {
        set_state(d, page, parent, state, author)?;
        Ok(vec![page])
    })
}

pub fn list_all(eng: &Engine, doc: DocId) -> Result<Vec<AnnotDto>> {
    list_status(eng, doc).map(|l| l.items)
}

/// [`list_all`] with whether some annotations were left out.
pub fn list_status(eng: &Engine, doc: DocId) -> Result<AnnotList> {
    eng.read_custom(doc, |d| list(d))
}
