//! Comments to and from XFDF (ISO 19444-1) and FDF (ISO 32000-2, 12.7.8): Acrobat's "Export / Import
//! data file" for comments. Both files are hostile input (see `crate::exchange` for the caps).
//!
//! Export reads each comment's own dictionary, so geometry is in PDF user space (y up), which is what
//! these files use; the list DTO is in page space. Import maps the file's points through the page
//! transform, creates the comment with the same code the tools use (which draws our own appearance),
//! then puts the file's name, author, dates and subject back. A comment is matched by its `name`
//! (`/NM`): the one that exists is updated in place, the others are added, and a reply keeps its parent.
//!
//! What only raw writes can set comes last (`finish`): the file's appearance (`<appearance>`, FDF `/AP`)
//! becomes the comment's `/AP` when `ap` could read it, and the rich text (`<contents-richtext>`, `/RC`)
//! its `/RC`; the plain `/Contents` is always kept. A part that cannot be carried is left out, the comment
//! gets Inkwell's own drawing, and the report counts it. Carets and file attachments are comments of their
//! own (`attach` holds the cap and the never-open rules for the file).
//!
//! Not carried: pop-up windows (a comment's pop-up is written, not read), sound, redaction marks and
//! measurements (skipped and counted), callout lines of free text. Links of an attachment (`/F` that is not an
//! embedded file) are never followed.

use super::{Kind, NewAnnot, Style, add, bad, each, find, meta, parent_of, put_str, rgb};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::exchange::{self, Budget, Format, Unpack, MAX_DEPTH, MAX_EXPORT_COPIED, MAX_ITEMS, MAX_NODES, MAX_STR, esc, pdf_name, pdf_text};
use crate::forms::{decode_text, get, parse_da, text};
use ap::{Package, Why};
use attach::AttachFile;
use mupdf::pdf::{AnnotationFlags, PdfAnnotationType, PdfDocument, PdfObject};
use mupdf::{Matrix, Point, Quad};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

mod ap;
mod attach;
mod rich;
mod stream;

type Result<T> = std::result::Result<T, EngineError>;

/// The comment types that travel, with their XFDF element and PDF `/Subtype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sub {
    Text,
    FreeText,
    Line,
    Square,
    Circle,
    Polygon,
    PolyLine,
    Highlight,
    Underline,
    Squiggly,
    StrikeOut,
    Stamp,
    Ink,
    Caret,
    FileAttachment,
}

const SUBS: [(Sub, &str, &str); 15] = [
    (Sub::Text, "text", "Text"),
    (Sub::FreeText, "freetext", "FreeText"),
    (Sub::Line, "line", "Line"),
    (Sub::Square, "square", "Square"),
    (Sub::Circle, "circle", "Circle"),
    (Sub::Polygon, "polygon", "Polygon"),
    (Sub::PolyLine, "polyline", "PolyLine"),
    (Sub::Highlight, "highlight", "Highlight"),
    (Sub::Underline, "underline", "Underline"),
    (Sub::Squiggly, "squiggly", "Squiggly"),
    (Sub::StrikeOut, "strikeout", "StrikeOut"),
    (Sub::Stamp, "stamp", "Stamp"),
    (Sub::Ink, "ink", "Ink"),
    (Sub::Caret, "caret", "Caret"),
    (Sub::FileAttachment, "fileattachment", "FileAttachment"),
];

impl Sub {
    fn tag(self) -> &'static str {
        SUBS.iter().find(|s| s.0 == self).map_or("text", |s| s.1)
    }

    fn pdf(self) -> &'static str {
        SUBS.iter().find(|s| s.0 == self).map_or("Text", |s| s.2)
    }

    fn of_tag(t: &str) -> Option<Sub> {
        SUBS.iter().find(|s| s.1 == t).map(|s| s.0)
    }

    fn of_pdf(t: &str) -> Option<Sub> {
        SUBS.iter().find(|s| s.2 == t).map(|s| s.0)
    }

    fn of_type(t: PdfAnnotationType) -> Option<Sub> {
        Some(match t {
            PdfAnnotationType::Text => Sub::Text,
            PdfAnnotationType::FreeText => Sub::FreeText,
            PdfAnnotationType::Line => Sub::Line,
            PdfAnnotationType::Square => Sub::Square,
            PdfAnnotationType::Circle => Sub::Circle,
            PdfAnnotationType::Polygon => Sub::Polygon,
            PdfAnnotationType::PolyLine => Sub::PolyLine,
            PdfAnnotationType::Highlight => Sub::Highlight,
            PdfAnnotationType::Underline => Sub::Underline,
            PdfAnnotationType::Squiggly => Sub::Squiggly,
            PdfAnnotationType::StrikeOut => Sub::StrikeOut,
            PdfAnnotationType::Stamp => Sub::Stamp,
            PdfAnnotationType::Ink => Sub::Ink,
            PdfAnnotationType::Caret => Sub::Caret,
            PdfAnnotationType::FileAttachment => Sub::FileAttachment,
            _ => return None,
        })
    }

    fn markup(self) -> Option<Kind> {
        Some(match self {
            Sub::Highlight => Kind::Highlight,
            Sub::Underline => Kind::Underline,
            Sub::StrikeOut => Kind::StrikeOut,
            Sub::Squiggly => Kind::Squiggly,
            _ => return None,
        })
    }
}

const FLAGS: [(i32, &str); 10] = [
    (1, "invisible"),
    (2, "hidden"),
    (4, "print"),
    (8, "nozoom"),
    (16, "norotate"),
    (32, "noview"),
    (64, "readonly"),
    (128, "locked"),
    (256, "togglenoview"),
    (512, "lockedcontents"),
];

/// One comment as the files carry it. Geometry is in PDF user space.
#[derive(Clone, Debug)]
pub(crate) struct Comment {
    pub sub: Sub,
    /// 0-based; a file may name any number.
    pub page: i64,
    /// Export only: the annotation's object number, and its parent's.
    pub xref: i32,
    pub parent_xref: Option<i32>,
    pub name: String,
    pub rect: Option<[f32; 4]>,
    /// Quadrilaterals, 8 numbers each.
    pub quads: Vec<f32>,
    pub line: Option<[f32; 4]>,
    pub head: String,
    pub tail: String,
    /// `x y` pairs.
    pub vertices: Vec<f32>,
    pub strokes: Vec<Vec<f32>>,
    pub color: Option<[f32; 3]>,
    pub fill: Option<[f32; 3]>,
    pub opacity: Option<f32>,
    pub width: Option<f32>,
    pub contents: String,
    pub author: String,
    pub subject: String,
    pub created: String,
    pub modified: String,
    pub icon: String,
    pub da: String,
    pub flags: Option<i32>,
    /// The name of the comment this replies to.
    pub parent: String,
    pub reply_type: String,
    pub state_model: String,
    pub state: String,
    pub popup: Option<([f32; 4], bool)>,
    /// A measurement (`/Measure`, or an intent that ends in `Dimension`): its scale data is not carried.
    pub measured: bool,
    /// Rich text (`/RC`) as sanitized `<body>` XHTML; `contents` is its plain text.
    pub rich: Option<String>,
    /// A caret's `/RD` (XFDF `fringe`) and `/Sy`.
    pub fringe: Option<[f32; 4]>,
    pub symbol: String,
    /// The appearance (`/AP /N`) as plain data. Read from a file always; from a document only for a stamp.
    pub ap: Option<Package>,
    /// A FileAttachment's file.
    pub file: Option<AttachFile>,
    /// Parts of the comment that were read but cannot be carried (each with why): the comment itself is kept.
    pub notes: Vec<Why>,
}

impl Comment {
    fn new(sub: Sub) -> Comment {
        Comment {
            sub,
            page: 0,
            xref: 0,
            parent_xref: None,
            name: String::new(),
            rect: None,
            quads: Vec::new(),
            line: None,
            head: String::new(),
            tail: String::new(),
            vertices: Vec::new(),
            strokes: Vec::new(),
            color: None,
            fill: None,
            opacity: None,
            width: None,
            contents: String::new(),
            author: String::new(),
            subject: String::new(),
            created: String::new(),
            modified: String::new(),
            icon: String::new(),
            da: String::new(),
            flags: None,
            parent: String::new(),
            reply_type: String::new(),
            state_model: String::new(),
            state: String::new(),
            popup: None,
            measured: false,
            rich: None,
            fringe: None,
            symbol: String::new(),
            ap: None,
            file: None,
            notes: Vec::new(),
        }
    }

    fn is_state(&self) -> bool {
        !self.parent.is_empty() && !self.state.is_empty()
    }
}

// ---- reading a comment's dictionary (the document's own, or an FDF's) -----------------------------

const MAX_NUMS: usize = 65_536;
const MAX_QUADS: usize = 4_096;
const MAX_STROKES: usize = 1_024;

/// A text entry. Charged to `b`; once the budget is gone nothing more is copied.
fn string(o: &PdfObject, k: &str, b: &Budget) -> String {
    if b.gone() {
        return String::new();
    }
    let Some(v) = get(o, k) else { return String::new() };
    let object = indirect_number(&v);
    // A shared text over the limit is dropped by every comment that uses it: not copied, not charged again.
    if object.and_then(|n| b.string_len_of(n)).is_some_and(|n| n > MAX_STR) {
        b.note_too_long();
        return String::new();
    }
    let s = match object.and_then(|n| b.string_of(n)) {
        Some(s) => Some(s),
        None => {
            let s = if v.is_string().unwrap_or(false) {
                v.as_string().ok().or_else(|| v.as_bytes().ok().map(|b| decode_text(&b)))
            } else {
                text(&v)
            };
            if let (Some(n), Some(s)) = (object, &s) {
                // Over the limit it is dropped below, so it is neither kept nor charged (only its length is).
                if s.len() > MAX_STR {
                    b.keep_long(n, s.len());
                } else {
                    b.keep_string(n, s);
                }
            }
            s
        }
    };
    let n = s.as_ref().map_or(0, String::len);
    // Over the limit: dropped before it is charged, so it does not use up the export budget.
    if n > MAX_STR {
        b.note_too_long();
        return String::new();
    }
    if !b.take(n) {
        return String::new();
    }
    s.unwrap_or_default()
}

/// A name entry, as text. Only a PDF name counts, never a string: a string can be large and shared by
/// every comment, a name is not read at all otherwise. Charged to `b` like [`string`].
fn name_in(v: Option<PdfObject>, b: &Budget) -> String {
    let Some(n) = v.filter(|v| v.is_name().unwrap_or(false)).and_then(|v| v.as_name().ok()) else { return String::new() };
    if b.gone() || !b.take(n.len()) {
        return String::new();
    }
    String::from_utf8_lossy(&n).into_owned()
}

/// The object number of a reference (`None` for a direct object).
fn indirect_number(v: &PdfObject) -> Option<i32> {
    if v.is_indirect().unwrap_or(false) { v.as_indirect().ok() } else { None }
}

/// Numbers of an array, at most `cap`; each is charged to `b` (8 bytes: it is held twice before it is used).
fn floats(o: &PdfObject, cap: usize, b: &Budget) -> Vec<f32> {
    if !o.is_array().unwrap_or(false) {
        return Vec::new();
    }
    let object = indirect_number(o);
    let n = o.len().unwrap_or(0).min(cap);
    // Each use is a copy of its own, so each is charged; what a reference was read as is kept to read it once.
    if !b.take(24 + n * 8) {
        return Vec::new();
    }
    if let Some(v) = object.and_then(|x| b.numbers_of(x, cap)) {
        return v;
    }
    let v: Vec<f32> = (0..n as i32).filter_map(|i| o.get_array(i).ok().flatten().and_then(|v| v.as_float().ok())).filter(|v| v.is_finite()).collect();
    if let Some(x) = object {
        b.keep_numbers(x, cap, &v);
    }
    v
}

fn rgb_from(a: &[f32]) -> Option<[f32; 3]> {
    let c = |v: f32| v.clamp(0.0, 1.0);
    match a.len() {
        1 => Some([c(a[0]); 3]),
        3 => Some([c(a[0]), c(a[1]), c(a[2])]),
        4 => {
            let k = 1.0 - c(a[3]);
            Some([(1.0 - c(a[0])) * k, (1.0 - c(a[1])) * k, (1.0 - c(a[2])) * k])
        }
        _ => None,
    }
}

fn pairs(v: &[f32]) -> Vec<f32> {
    v[..v.len() / 2 * 2].to_vec()
}

/// A measurement annotation (the Measure tool's, or Acrobat's): `/Measure` data, or a dimension intent.
fn is_measurement(o: &PdfObject) -> bool {
    get(o, "Measure").is_some() || get(o, "IT").filter(|t| t.is_name().unwrap_or(false)).and_then(|t| t.as_name().ok()).is_some_and(|t| t.ends_with(b"Dimension"))
}

/// The rich text of `/RC` (a string or a stream), sanitized.
fn rich_of(o: &PdfObject, b: &Budget) -> Option<std::result::Result<rich::Rich, Why>> {
    let v = get(o, "RC")?;
    if v.is_stream().unwrap_or(false) {
        // Through the same decoder as every stream (its filters, its allowance, its cache).
        let bytes = match stream::unpack(&v, rich::MAX_RICH * 4, b) {
            Ok(x) => x,
            Err(Unpack::Over | Unpack::NoWork) => return Some(Err(rich::TOO_BIG)),
            Err(_) => return Some(Err(rich::DAMAGED)),
        };
        if !b.take(bytes.len()) {
            return None;
        }
        return Some(rich::from_bytes(&bytes));
    }
    let s = string(o, "RC", b);
    (!s.is_empty()).then(|| rich::from_bytes(s.as_bytes()))
}

/// Reads one comment dictionary. `/AA`, `/A`, `/F` and everything that is not listed here are never looked
/// at. `/AP` is read (into plain data, see `ap`) only when `want_ap`; `/FS` only for a FileAttachment, and
/// only the embedded file, never a link.
fn read(o: &PdfObject, sub: Sub, b: &Budget, want_ap: bool) -> Comment {
    let mut c = Comment::new(sub);
    c.name = string(o, "NM", b);
    c.contents = string(o, "Contents", b);
    c.author = string(o, "T", b);
    c.subject = string(o, "Subj", b);
    c.created = string(o, "CreationDate", b);
    c.modified = string(o, "M", b);
    c.da = string(o, "DA", b);
    c.state = string(o, "State", b);
    c.state_model = string(o, "StateModel", b);
    c.icon = name_in(get(o, "Name"), b);
    c.reply_type = name_in(get(o, "RT"), b);
    c.flags = get(o, "F").and_then(|f| f.as_int().ok());
    c.measured = is_measurement(o);
    let r = get(o, "Rect").map(|a| floats(&a, 4, b)).unwrap_or_default();
    if r.len() == 4 {
        c.rect = Some([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
    }
    let q = get(o, "QuadPoints").map(|a| floats(&a, MAX_QUADS * 8, b)).unwrap_or_default();
    c.quads = q[..q.len() / 8 * 8].to_vec();
    let l = get(o, "L").map(|a| floats(&a, 4, b)).unwrap_or_default();
    if l.len() == 4 {
        c.line = Some([l[0], l[1], l[2], l[3]]);
    }
    if let Some(le) = get(o, "LE").filter(|a| a.is_array().unwrap_or(false)) {
        let name = |i: i32| name_in(le.get_array(i).ok().flatten(), b);
        (c.head, c.tail) = (name(0), name(1));
    }
    c.vertices = pairs(&get(o, "Vertices").map(|a| floats(&a, MAX_NUMS, b)).unwrap_or_default());
    if let Some(ink) = get(o, "InkList").filter(|a| a.is_array().unwrap_or(false)) {
        for i in 0..ink.len().unwrap_or(0).min(MAX_STROKES) as i32 {
            if let Some(s) = ink.get_array(i).ok().flatten() {
                c.strokes.push(pairs(&floats(&s, MAX_NUMS, b)));
            }
        }
    }
    c.color = get(o, "C").and_then(|a| rgb_from(&floats(&a, 4, b)));
    c.fill = get(o, "IC").and_then(|a| rgb_from(&floats(&a, 4, b)));
    c.opacity = get(o, "CA").and_then(|v| v.as_float().ok()).filter(|v| v.is_finite());
    c.width = get(o, "BS")
        .and_then(|b| get(&b, "W"))
        .and_then(|v| v.as_float().ok())
        .or_else(|| get(o, "Border").and_then(|bd| floats(&bd, 3, b).get(2).copied()))
        .filter(|v| v.is_finite());
    if let Some(p) = get(o, "Popup").filter(|p| p.is_dict().unwrap_or(false)) {
        let r = get(&p, "Rect").map(|a| floats(&a, 4, b)).unwrap_or_default();
        if r.len() == 4 {
            let open = get(&p, "Open").and_then(|v| v.as_bool().ok()).unwrap_or(false);
            c.popup = Some(([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])], open));
        }
    }
    if sub == Sub::Caret {
        let rd = get(o, "RD").map(|a| floats(&a, 4, b)).unwrap_or_default();
        if rd.len() == 4 {
            c.fringe = Some([rd[0], rd[1], rd[2], rd[3]]);
        }
        c.symbol = name_in(get(o, "Sy"), b);
    }
    match rich_of(o, b) {
        Some(Ok(r)) => {
            if c.contents.is_empty() {
                c.contents = r.text;
            }
            c.rich = Some(r.xml);
        }
        Some(Err(w)) => c.notes.push(w),
        None => {}
    }
    if want_ap {
        match ap::read(o, b) {
            Some(Ok(p)) => c.ap = Some(p),
            Some(Err(w)) => c.notes.push(w),
            None => {}
        }
    }
    if sub == Sub::FileAttachment {
        match attach::read(o, b) {
            Some(Ok(f)) => c.file = Some(f),
            Some(Err(w)) => c.notes.push(w),
            None => c.notes.push(attach::EXTERNAL),
        }
    }
    c
}

/// What an export reads from a document.
struct Collected {
    /// Every comment of the document that has a type files can carry, in page order.
    items: Vec<Comment>,
    /// Comments left out: another type (sound, ...), a measurement or an attachment that cannot be written, and
    /// their replies.
    left: usize,
    /// Text entries left empty for being over 1 MB (the importer refuses a file with a longer value, so writing
    /// one would make the file unreadable).
    long_text: usize,
    /// Parts of kept comments, and attachments of left-out ones, that cannot be written, with why.
    notes: BTreeMap<Why, usize>,
}

/// Unnamed comments get a stable name for this export only; nothing is written to the document.
fn collect(doc: &PdfDocument) -> Result<Collected> {
    collect_within(doc, MAX_EXPORT_COPIED)
}

/// [`collect`] with the copy bound given (a test uses a small one: a shape that fills the real bound is slow to build).
fn collect_within(doc: &PdfDocument, limit: usize) -> Result<Collected> {
    let budget = Budget::with_limit(limit);
    let mut left = 0usize;
    let mut notes: BTreeMap<Why, usize> = BTreeMap::new();
    // Measurements are left out (their /Measure scale has no place in these files), and so are their replies.
    // The same for an attachment whose file cannot be written (over the cap): a comment without its file is not
    // the comment.
    let mut measured: HashSet<i32> = HashSet::new();
    let (all, _) = each(doc, |page, a| {
        let Some(sub) = Sub::of_type(a.r#type()?) else {
            left += 1;
            return Ok(None);
        };
        let o = a.object();
        if is_measurement(&o) {
            measured.insert(a.xref()?);
            left += 1;
            return Ok(None);
        }
        // Only a stamp's appearance is written: it is what makes a picture (the other types are drawn from their data).
        let mut c = read(&o, sub, &budget, sub == Sub::Stamp);
        if c.ap.as_ref().is_some_and(|p| ap::to_xml(p).is_none()) {
            c.ap = None;
            c.notes.push(ap::TOO_BIG);
        }
        if sub == Sub::FileAttachment && c.file.is_none() {
            for w in c.notes {
                *notes.entry(w).or_default() += 1;
            }
            measured.insert(a.xref()?);
            left += 1;
            return Ok(None);
        }
        c.page = i64::from(page);
        c.xref = a.xref()?;
        c.parent_xref = parent_of(&o);
        Ok(Some(c))
    })?;
    if budget.gone() {
        return Err(bad("The comments of this document hold more than 256 MB, or refer to the same text or numbers so many times, that Inkwell cannot export them to a file."));
    }
    let mut items: Vec<Comment> = all.into_iter().flatten().collect();
    let kept = items.len();
    items.retain(|c| !c.parent_xref.is_some_and(|p| measured.contains(&p)));
    left += kept - items.len();
    for c in &mut items {
        // A reply is plain text: the files give it no rich text, and import makes none.
        if c.parent_xref.is_some() {
            c.rich = None;
        }
        for w in &c.notes {
            *notes.entry(w).or_default() += 1;
        }
    }
    let names: HashMap<i32, String> = items
        .iter_mut()
        .map(|c| {
            if c.name.is_empty() {
                c.name = format!("inkwell-{}-{}", c.page, c.xref);
            }
            (c.xref, c.name.clone())
        })
        .collect();
    for c in &mut items {
        if let Some(p) = c.parent_xref {
            c.parent = names.get(&p).cloned().unwrap_or_default();
        }
    }
    Ok(Collected { items, left, long_text: budget.too_long(), notes })
}

// ---- export --------------------------------------------------------------------------------------

fn n(v: f32) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.into() }
}

fn join(v: &[f32], sep: &str) -> String {
    v.iter().map(|x| n(*x)).collect::<Vec<_>>().join(sep)
}

fn hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", b(c[0]), b(c[1]), b(c[2]))
}

fn flag_names(f: i32) -> String {
    FLAGS.iter().filter(|(b, _)| f & b != 0).map(|(_, s)| *s).collect::<Vec<_>>().join(",")
}

fn xfdf(items: &[Comment], file: &str) -> String {
    let mut o = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n");
    let _ = writeln!(o, "<f href=\"{}\"/>", esc(file));
    o.push_str("<annots>\n");
    for c in items {
        let tag = c.sub.tag();
        let mut at = String::new();
        // A line break or tab in an attribute is a reference: the reader would read the raw one as a space.
        let mut put = |k: &str, v: &str| {
            if !v.is_empty() {
                let _ = write!(at, " {k}=\"{}\"", esc(v).replace('\n', "&#10;").replace('\t', "&#9;"));
            }
        };
        put("page", &c.page.to_string());
        if let Some(r) = c.rect {
            put("rect", &join(&r, ","));
        }
        put("name", &c.name);
        put("title", &c.author);
        put("subject", &c.subject);
        put("date", &c.modified);
        put("creationdate", &c.created);
        put("flags", &c.flags.map(flag_names).unwrap_or_default());
        put("color", &c.color.map(hex).unwrap_or_default());
        put("interior-color", &c.fill.map(hex).unwrap_or_default());
        put("opacity", &c.opacity.map(n).unwrap_or_default());
        put("width", &c.width.map(n).unwrap_or_default());
        put("coords", &join(&c.quads, ","));
        if let Some(l) = c.line {
            put("start", &join(&l[..2], ","));
            put("end", &join(&l[2..], ","));
        }
        put("head", &c.head);
        put("tail", &c.tail);
        put("icon", &c.icon);
        if let Some(r) = c.fringe {
            put("fringe", &join(&r, ","));
        }
        put("symbol", &c.symbol);
        if let Some(f) = &c.file {
            put("file", &f.name);
        }
        put("inreplyto", &c.parent);
        put("replyType", &c.reply_type);
        put("statemodel", &c.state_model);
        put("state", &c.state);
        let _ = write!(o, "<{tag}{at}>");
        if !c.contents.is_empty() {
            let _ = write!(o, "<contents>{}</contents>", esc(&c.contents));
        }
        if let Some(r) = &c.rich {
            let _ = write!(o, "<contents-richtext>{r}</contents-richtext>");
        }
        if !c.da.is_empty() {
            let _ = write!(o, "<defaultappearance>{}</defaultappearance>", esc(&c.da));
        }
        if !c.strokes.is_empty() {
            o.push_str("<inklist>");
            for s in &c.strokes {
                let p: Vec<String> = s.chunks_exact(2).map(|p| format!("{},{}", n(p[0]), n(p[1]))).collect();
                let _ = write!(o, "<gesture>{}</gesture>", p.join(";"));
            }
            o.push_str("</inklist>");
        }
        if !c.vertices.is_empty() {
            let p: Vec<String> = c.vertices.chunks_exact(2).map(|p| format!("{},{}", n(p[0]), n(p[1]))).collect();
            let _ = write!(o, "<vertices>{}</vertices>", p.join(";"));
        }
        if let Some((r, open)) = c.popup {
            let _ = write!(o, "<popup page=\"{}\" rect=\"{}\" open=\"{}\"/>", c.page, join(&r, ","), if open { "yes" } else { "no" });
        }
        if let Some(x) = c.ap.as_ref().and_then(ap::to_xml) {
            use base64::Engine as _;
            let _ = write!(o, "<appearance>{}</appearance>", base64::engine::general_purpose::STANDARD.encode(x));
        }
        if let Some(f) = &c.file {
            o.push_str(&attach::to_data(f));
        }
        let _ = writeln!(o, "</{tag}>");
    }
    o.push_str("</annots>\n</xfdf>\n");
    o
}

/// An FDF file from whole objects, numbered 1.. in order (object 1 is the root).
fn assemble_fdf(objs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%FDF-1.2\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for o in objs {
        offsets.push(out.len());
        out.extend_from_slice(o);
    }
    let xref = out.len();
    let mut t = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for o in offsets {
        let _ = writeln!(t, "{o:010} 00000 n ");
    }
    let _ = write!(t, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1);
    out.extend_from_slice(t.as_bytes());
    out
}

fn fdf(items: &[Comment], file: &str) -> Result<Vec<u8>> {
    // Object 1 is the root; comment i is object i + 2, so a reply's /IRT points inside this file. The objects of
    // appearances and files follow the comments.
    let numbers: HashMap<&str, usize> = items.iter().enumerate().map(|(i, c)| (c.name.as_str(), i + 2)).collect();
    let mut next = items.len() + 2;
    let mut bodies: Vec<Vec<u8>> = Vec::with_capacity(items.len());
    let mut extra: Vec<Vec<u8>> = Vec::new();
    let refs: Vec<String> = (0..items.len()).map(|i| format!("{} 0 R", i + 2)).collect();
    let root = format!("1 0 obj\n<< /FDF << /Annots [{}] /F {} >> >>\nendobj\n", refs.join(" "), pdf_text(file));
    for (i, c) in items.iter().enumerate() {
        let mut d = format!("{} 0 obj\n<< /Type /Annot /Subtype /{} /Page {}", i + 2, c.sub.pdf(), c.page);
        let mut put = |k: &str, v: String| {
            let _ = write!(d, " /{k} {v}");
        };
        let nums = |v: &[f32]| format!("[{}]", join(v, " "));
        if let Some(r) = c.rect {
            put("Rect", nums(&r));
        }
        for (k, v) in [("NM", &c.name), ("T", &c.author), ("Subj", &c.subject), ("Contents", &c.contents), ("M", &c.modified), ("CreationDate", &c.created), ("DA", &c.da)] {
            if !v.is_empty() {
                put(k, pdf_text(v));
            }
        }
        if let Some(r) = &c.rich {
            put("RC", pdf_text(r));
        }
        if let Some(f) = c.flags {
            put("F", f.to_string());
        }
        if let Some(v) = c.color {
            put("C", nums(&v));
        }
        if let Some(v) = c.fill {
            put("IC", nums(&v));
        }
        if let Some(v) = c.opacity {
            put("CA", n(v));
        }
        if let Some(v) = c.width {
            put("BS", format!("<< /W {} >>", n(v)));
        }
        if !c.quads.is_empty() {
            put("QuadPoints", nums(&c.quads));
        }
        if let Some(l) = c.line {
            put("L", nums(&l));
        }
        if !c.head.is_empty() || !c.tail.is_empty() {
            let le = |s: &str| pdf_name(if s.is_empty() { "None" } else { s });
            put("LE", format!("[{} {}]", le(&c.head), le(&c.tail)));
        }
        if !c.vertices.is_empty() {
            put("Vertices", nums(&c.vertices));
        }
        if !c.strokes.is_empty() {
            put("InkList", format!("[{}]", c.strokes.iter().map(|s| nums(s)).collect::<Vec<_>>().join(" ")));
        }
        if !c.icon.is_empty() {
            put("Name", pdf_name(&c.icon));
        }
        if let Some(r) = c.fringe {
            put("RD", nums(&r));
        }
        if !c.symbol.is_empty() {
            put("Sy", pdf_name(&c.symbol));
        }
        if let Some(p) = numbers.get(c.parent.as_str()).filter(|_| !c.parent.is_empty()) {
            put("IRT", format!("{p} 0 R"));
        }
        if !c.reply_type.is_empty() {
            put("RT", pdf_name(&c.reply_type));
        }
        if !c.state_model.is_empty() {
            put("StateModel", pdf_text(&c.state_model));
        }
        if !c.state.is_empty() {
            put("State", pdf_text(&c.state));
        }
        if let Some(p) = &c.ap {
            put("AP", format!("<< /N {} 0 R >>", next + p.root));
            extra.extend(ap::write_objects(p, next));
            next += p.objs.len();
        }
        if let Some(f) = &c.file {
            put("FS", format!("{} 0 R", next + 1));
            extra.extend(attach::write_objects(f, next)?);
            next += 2;
        }
        d.push_str(" >>\nendobj\n");
        bodies.push(d.into_bytes());
    }
    let mut objs = vec![root.into_bytes()];
    objs.extend(bodies);
    objs.extend(extra);
    Ok(assemble_fdf(&objs))
}

/// What an export wrote.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub count: usize,
    /// Comments left out: types the files cannot carry (sound), measurements and attachments that are over their
    /// limit, and their replies.
    pub left_out: usize,
    /// Text entries left empty for being over 1 MB, which a file Inkwell reads cannot hold.
    pub long_text: usize,
    /// Appearances (of picture stamps) and attached files written.
    pub appearances: usize,
    pub files: usize,
    /// Parts that could not be written (an appearance with a pattern or over the limit, rich text that is too big,
    /// an attached file over its limit), with how many each.
    pub notes: Vec<Skipped>,
}

/// The file bytes for every comment of the open document. Read-only: allowed on a signed or certified one.
pub fn export(eng: &Engine, doc: DocId, format: Format, file_name: &str) -> Result<(Vec<u8>, Exported)> {
    let Collected { items, left, long_text, notes } = eng.read_custom(doc, collect)?;
    let bytes = match format {
        Format::Fdf => fdf(&items, file_name)?,
        _ => xfdf(&items, file_name).into_bytes(),
    };
    let report = Exported {
        count: items.len(),
        left_out: left,
        long_text,
        appearances: items.iter().filter(|c| c.ap.is_some()).count(),
        files: items.iter().filter(|c| c.file.is_some()).count(),
        notes: skipped(notes),
    };
    Ok((bytes, report))
}

// ---- parsing the files ---------------------------------------------------------------------------

/// What a file holds: the comments it can carry, and how many it could not take.
#[derive(Debug, Default)]
pub struct Parsed {
    pub(crate) items: Vec<Comment>,
    /// Elements or objects of another type (and the ones past the per-file limit).
    pub unknown: usize,
    pub over: usize,
}

fn csv(s: &str) -> Vec<f32> {
    s.split([',', ';', ' ', '\t', '\n', '\r']).filter(|t| !t.is_empty()).filter_map(|t| t.parse::<f32>().ok()).filter(|v| v.is_finite()).collect()
}

fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok().map(|v| f32::from(v) / 255.0);
    Some([p(0)?, p(2)?, p(4)?])
}

fn parse_flags(s: &str) -> Option<i32> {
    let mut f = 0;
    for t in s.split([',', ' ']).filter(|t| !t.is_empty()) {
        f |= FLAGS.iter().find(|(_, n)| n.eq_ignore_ascii_case(t)).map(|(b, _)| *b)?;
    }
    Some(f)
}

fn points(s: &str) -> Vec<f32> {
    pairs(&csv(s))
}

pub fn parse_xfdf(bytes: &[u8]) -> Result<Parsed> {
    let root = exchange::parse_xml(bytes)?;
    if root.name != "xfdf" {
        return Err(bad("This is not an XFDF file."));
    }
    let mut out = Parsed::default();
    let budget = Budget::new();
    let Some(annots) = root.child("annots") else { return Ok(out) };
    for e in &annots.kids {
        if e.name == "popup" {
            continue;
        }
        let Some(sub) = Sub::of_tag(&e.name) else {
            out.unknown += 1;
            continue;
        };
        if out.items.len() >= MAX_ITEMS {
            out.over += 1;
            continue;
        }
        let a = |k: &str| e.attr(k).unwrap_or("").to_string();
        let mut c = Comment::new(sub);
        c.page = e.attr("page").and_then(|p| p.trim().parse().ok()).unwrap_or(-1);
        let r = csv(e.attr("rect").unwrap_or(""));
        if r.len() == 4 {
            c.rect = Some([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
        }
        c.name = a("name");
        c.measured = e.child("measure").is_some() || a("intent").to_ascii_lowercase().ends_with("dimension");
        c.author = a("title");
        c.subject = a("subject");
        c.modified = a("date");
        c.created = a("creationdate");
        c.flags = e.attr("flags").and_then(parse_flags);
        c.color = e.attr("color").and_then(parse_hex);
        c.fill = e.attr("interior-color").and_then(parse_hex);
        c.opacity = e.attr("opacity").and_then(|v| v.trim().parse().ok()).filter(|v: &f32| v.is_finite());
        c.width = e.attr("width").and_then(|v| v.trim().parse().ok()).filter(|v: &f32| v.is_finite());
        let q = csv(e.attr("coords").unwrap_or(""));
        c.quads = q[..q.len().min(MAX_QUADS * 8) / 8 * 8].to_vec();
        let (s, t) = (csv(e.attr("start").unwrap_or("")), csv(e.attr("end").unwrap_or("")));
        if s.len() == 2 && t.len() == 2 {
            c.line = Some([s[0], s[1], t[0], t[1]]);
        }
        c.head = a("head");
        c.tail = a("tail");
        c.icon = a("icon");
        c.parent = a("inreplyto");
        c.reply_type = a("replyType");
        c.state_model = a("statemodel");
        c.state = a("state");
        c.contents = e.child("contents").map(|k| k.text.clone()).unwrap_or_default();
        c.da = e.child("defaultappearance").map(|k| k.text.clone()).unwrap_or_default();
        let fringe = csv(e.attr("fringe").unwrap_or(""));
        if sub == Sub::Caret && fringe.len() == 4 {
            c.fringe = Some([fringe[0], fringe[1], fringe[2], fringe[3]]);
        }
        if sub == Sub::Caret {
            c.symbol = a("symbol");
        }
        if let Some(r) = e.child("contents-richtext") {
            // The element holds the `<body>` (any namespace prefix is dropped by the reader).
            match r.child("body").ok_or(rich::DAMAGED).and_then(rich::sanitize) {
                Ok(r) => {
                    if c.contents.is_empty() {
                        c.contents = r.text;
                    }
                    c.rich = Some(r.xml);
                }
                Err(w) => c.notes.push(w),
            }
        }
        if let Some(p) = e.child("appearance") {
            match ap::from_base64(&p.text, p.over, &budget) {
                Ok(p) => c.ap = Some(p),
                Err(w) => c.notes.push(w),
            }
        }
        if sub == Sub::FileAttachment {
            match e.child("data") {
                Some(d) => match attach::from_data(d, &a("file"), &budget) {
                    Ok(f) => c.file = Some(f),
                    Err(w) => c.notes.push(w),
                },
                None => c.notes.push(attach::EXTERNAL),
            }
        }
        if let Some(v) = e.child("vertices") {
            c.vertices = points(&v.text).into_iter().take(MAX_NUMS).collect();
        }
        if let Some(l) = e.child("inklist") {
            c.strokes = l.children("gesture").take(MAX_STROKES).map(|g| points(&g.text).into_iter().take(MAX_NUMS).collect()).collect();
        }
        out.items.push(c);
    }
    Ok(out)
}

pub fn parse_fdf(bytes: &[u8]) -> Result<Parsed> {
    exchange::with_fdf(bytes, |root| {
        let mut out = Parsed::default();
        let budget = Budget::new();
        let Some(annots) = get(root, "Annots").filter(|a| a.is_array().unwrap_or(false)) else { return Ok(out) };
        let count = annots.len().unwrap_or(0);
        if count > MAX_NODES {
            return Err(bad("This FDF file has more than 100 000 comments, so Inkwell does not read it."));
        }
        // Object numbers of the comments, so an /IRT can name its parent.
        let mut number_of: HashMap<i32, String> = HashMap::new();
        let mut seen: HashSet<i32> = HashSet::new();
        let mut entries: Vec<PdfObject> = Vec::new();
        for i in 0..count as i32 {
            let Some(e) = annots.get_array(i).ok().flatten() else { continue };
            let num = if e.is_indirect().unwrap_or(false) { e.as_indirect().ok() } else { None };
            if num.is_some_and(|n| !seen.insert(n)) || !e.is_dict().unwrap_or(false) {
                continue;
            }
            if let Some(n) = num {
                number_of.insert(n, string(&e, "NM", &budget));
            }
            entries.push(e);
        }
        for e in entries {
            budget.check()?;
            let sub = get(&e, "Subtype").filter(|t| t.is_name().unwrap_or(false)).and_then(|t| t.as_name().ok()).and_then(|t| Sub::of_pdf(&String::from_utf8_lossy(&t)));
            let Some(sub) = sub else {
                out.unknown += 1;
                continue;
            };
            if out.items.len() >= MAX_ITEMS {
                out.over += 1;
                continue;
            }
            let mut c = read(&e, sub, &budget, true);
            c.page = get(&e, "Page").and_then(|p| p.as_int().ok()).map_or(-1, i64::from);
            // The parent is named by its /NM: through the object the /IRT points at (this file's own
            // object number first), or the dictionary written in place.
            if let Some(p) = e.get_dict("IRT").ok().flatten() {
                c.parent = if p.is_indirect().unwrap_or(false) {
                    p.as_indirect().ok().and_then(|n| number_of.get(&n).cloned()).unwrap_or_default()
                } else {
                    string(&p, "NM", &budget)
                };
            }
            out.items.push(c);
        }
        budget.check()?;
        Ok(out)
    })
}

/// A file's comments, by what it is (see [`exchange::sniff`]).
pub fn parse(bytes: &[u8]) -> Result<Parsed> {
    match exchange::sniff(bytes) {
        Some(Format::Xfdf) => parse_xfdf(bytes),
        Some(Format::Fdf) => parse_fdf(bytes),
        _ => Err(bad("This is not an XFDF or FDF file.")),
    }
}

// ---- import --------------------------------------------------------------------------------------

/// What an import did.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub added: usize,
    pub updated: usize,
    /// Why comments were skipped, with how many each.
    pub skipped: Vec<Skipped>,
    /// Comments that were made without a part of the file (its appearance, rich text or file), with how many each:
    /// the comment has Inkwell's own drawing, or plain text only.
    pub notes: Vec<Skipped>,
    /// Comments that now have the appearance the file gave, and attached files that were added.
    pub appearances: usize,
    pub files: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub reason: String,
    pub count: usize,
}

fn skipped(m: BTreeMap<&'static str, usize>) -> Vec<Skipped> {
    m.into_iter().map(|(reason, count)| Skipped { reason: reason.to_string(), count }).collect()
}

/// The result of the import command: the summary, the document's edit state and whether it is signed
/// (the change is then saved as an update after the signature, which will show it).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub summary: Summary,
    pub state: EditState,
    pub signed: bool,
}

const ABSENT: &str = "the page is not in this document";
const UNKNOWN: &str = "type of comment these files cannot carry here";
const LIMIT: &str = "past the limit of 10 000 comments in one file";
const DIFFERS: &str = "a comment of another type has this name";
const NO_PLACE: &str = "no position";
const NO_PARENT: &str = "reply to a comment that is not in the file or the document";
const NOT_NOTE: &str = "reply that is not a note";
const BAD_STATE: &str = "unknown review state";
const STATE_KNOWN: &str = "a review state with this name is already in the document";
const FAILED: &str = "could not be created";
const MEASURED: &str = "a measurement, whose scale these files cannot carry";
const MEASURED_HERE: &str = "a measurement of the document has this name (the Measure tool changes it)";

fn tp(m: &Matrix, x: f32, y: f32) -> [f32; 2] {
    [m.a * x + m.c * y + m.e, m.b * x + m.d * y + m.f]
}

fn tpairs(m: &Matrix, v: &[f32]) -> Vec<[f32; 2]> {
    v.chunks_exact(2).map(|p| tp(m, p[0], p[1])).collect()
}

/// The page-space box of a PDF-space rectangle.
fn trect(m: &Matrix, r: [f32; 4]) -> [f32; 4] {
    let c = [tp(m, r[0], r[1]), tp(m, r[2], r[1]), tp(m, r[0], r[3]), tp(m, r[2], r[3])];
    let xs = c.map(|p| p[0]);
    let ys = c.map(|p| p[1]);
    [xs.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max), ys.iter().copied().fold(f32::MIN, f32::max)]
}

/// The box of what a comment draws, for a file that gave no `rect`.
fn rect_of_geometry(c: &Comment) -> Option<[f32; 4]> {
    let all: Vec<f32> = c.quads.iter().chain(c.vertices.iter()).chain(c.strokes.iter().flatten()).chain(c.line.iter().flatten()).copied().collect();
    let pts: Vec<(f32, f32)> = all.chunks_exact(2).map(|p| (p[0], p[1])).collect();
    if pts.is_empty() {
        return None;
    }
    let f = |g: fn(f32, f32) -> f32, i: usize, init: f32| pts.iter().fold(init, |a, p| g(a, if i == 0 { p.0 } else { p.1 }));
    Some([f(f32::min, 0, f32::MAX), f(f32::min, 1, f32::MAX), f(f32::max, 0, f32::MIN), f(f32::max, 1, f32::MIN)])
}

fn arrow(c: &Comment) -> bool {
    let on = |s: &str| !s.is_empty() && !s.eq_ignore_ascii_case("none");
    on(&c.head) || on(&c.tail)
}

/// What import creates: one of the tools' annotations, or a polyline (which the tools do not have).
enum Made {
    Tool(NewAnnot),
    PolyLine(Vec<[f32; 2]>),
    /// A caret or a file attachment (the tools have neither), by its page-space rectangle.
    Caret([f32; 4]),
    File([f32; 4]),
}

/// What to create for `c`, in page space, or why it cannot be.
fn make(c: &Comment, m: &Matrix) -> std::result::Result<(Made, Style), &'static str> {
    let rect = c.rect.or_else(|| rect_of_geometry(c)).ok_or(NO_PLACE)?;
    let page_rect = trect(m, rect);
    let mut st = Style { color: c.color, fill: c.fill, opacity: c.opacity, width: c.width };
    let what = match c.sub {
        s if s.markup().is_some() => {
            let q: Vec<f32> = if c.quads.is_empty() {
                vec![rect[0], rect[3], rect[2], rect[3], rect[0], rect[1], rect[2], rect[1]]
            } else {
                c.quads.clone()
            };
            let quads = q
                .chunks_exact(8)
                .map(|g| {
                    let p = tpairs(m, g);
                    [p[0][0], p[0][1], p[1][0], p[1][1], p[2][0], p[2][1], p[3][0], p[3][1]]
                })
                .collect();
            Made::Tool(NewAnnot::Markup { markup: s.markup().unwrap_or(Kind::Highlight), quads })
        }
        Sub::Text => Made::Tool(NewAnnot::Note { at: [page_rect[0], page_rect[1]], text: c.contents.clone() }),
        Sub::FreeText => {
            let mut size = 12.0;
            if !c.da.is_empty() {
                let (_, s, color) = parse_da(&c.da);
                size = if s > 0.0 { s } else { 12.0 };
                st.color = Some(color);
            }
            st.width = None;
            Made::Tool(NewAnnot::FreeText { rect: page_rect, text: c.contents.clone(), font_size: Some(size) })
        }
        Sub::Ink => {
            let strokes: Vec<Vec<[f32; 2]>> = c.strokes.iter().map(|s| tpairs(m, s)).filter(|s| !s.is_empty()).collect();
            if strokes.is_empty() {
                return Err(NO_PLACE);
            }
            Made::Tool(NewAnnot::Ink { strokes })
        }
        Sub::Square | Sub::Circle => Made::Tool(NewAnnot::Shape {
            shape: if c.sub == Sub::Square { Kind::Rectangle } else { Kind::Ellipse },
            points: vec![[page_rect[0], page_rect[1]], [page_rect[2], page_rect[3]]],
        }),
        Sub::Line => {
            let l = c.line.ok_or(NO_PLACE)?;
            Made::Tool(NewAnnot::Shape { shape: if arrow(c) { Kind::Arrow } else { Kind::Line }, points: vec![tp(m, l[0], l[1]), tp(m, l[2], l[3])] })
        }
        Sub::Polygon => {
            if c.vertices.len() < 6 {
                return Err(NO_PLACE);
            }
            Made::Tool(NewAnnot::Shape { shape: Kind::Polygon, points: tpairs(m, &c.vertices) })
        }
        Sub::PolyLine => {
            if c.vertices.len() < 4 {
                return Err(NO_PLACE);
            }
            Made::PolyLine(tpairs(m, &c.vertices))
        }
        Sub::Stamp => Made::Tool(NewAnnot::Stamp { name: if c.icon.is_empty() { "Draft".into() } else { c.icon.clone() }, rect: page_rect }),
        Sub::Caret => Made::Caret(page_rect),
        Sub::FileAttachment => {
            // Without its file it is not the comment: skipped, with the reason the file was refused.
            if c.file.is_none() {
                return Err(c.notes.iter().copied().find(|w| attach::REFUSED.contains(w)).unwrap_or(attach::EXTERNAL));
            }
            Made::File(page_rect)
        }
        _ => return Err(UNKNOWN),
    };
    Ok((what, st))
}

/// Puts the file's own name, author, dates, subject and flags on a comment this import made or updated.
fn stamp(doc: &mut PdfDocument, page: u32, xref: i32, c: &Comment) -> Result<()> {
    let (_p, mut a) = find(doc, page, xref)?;
    if !c.name.is_empty() {
        put_str(&a, "NM", &c.name)?;
    }
    if !c.author.is_empty() {
        a.set_author(&c.author)?;
    }
    if !c.contents.is_empty() {
        a.set_contents(&c.contents)?;
    }
    for (k, v) in [("Subj", &c.subject), ("CreationDate", &c.created), ("M", &c.modified)] {
        if !v.is_empty() {
            put_str(&a, k, v)?;
        }
    }
    if let Some(f) = c.flags {
        let mut f = AnnotationFlags::from_bits_truncate(f);
        // A reply or review state stays hidden on the page, whatever flags the file gives it (also one matched by
        // name that the file lists without its `inreplyto`). What the document's own comment is decides, not what
        // the file says: an entry naming an existing top-level note does not hide it.
        if a.object().get_dict("IRT")?.is_some() {
            f |= AnnotationFlags::IS_HIDDEN;
        }
        a.set_flags(f)?;
    }
    if (c.sub == Sub::Text && !c.icon.is_empty()) || (c.sub == Sub::FileAttachment && ICONS.contains(&c.icon.as_str())) {
        a.set_icon_name(&c.icon)?;
    }
    a.update()?;
    Ok(())
}

/// The icons of a file attachment (PDF 32000-1, 12.5.6.15).
const ICONS: [&str; 4] = ["Graph", "PushPin", "Paperclip", "Tag"];

fn real_array(doc: &PdfDocument, v: &[f32]) -> Result<PdfObject> {
    let mut a = doc.new_array()?;
    for x in v {
        a.array_push(PdfObject::new_real(*x)?)?;
    }
    Ok(a)
}

/// What only raw writes can set: the appearance the file gave (with the rectangle it was drawn for), rich text,
/// and a caret's fringe and symbol. It comes after every setter: `set_contents` drops `/RC`, and a raw `/AP` is
/// what MuPDF's own drawing must not replace afterwards (nothing flags it as stale; an edit by Inkwell's own
/// code regenerates it). Returns whether the appearance was installed.
fn finish(doc: &mut PdfDocument, page: u32, xref: i32, c: &Comment) -> Result<bool> {
    let ap = c.ap.as_ref().zip(c.rect);
    let symbol = ["P", "None"].contains(&c.symbol.as_str());
    if c.rich.is_none() && c.fringe.is_none() && !symbol && ap.is_none() {
        return Ok(false);
    }
    let (_p, mut a) = find(doc, page, xref)?;
    let mut o = a.object();
    if let Some(r) = &c.rich {
        put_str(&a, "RC", r)?;
    }
    if c.sub == Sub::Caret {
        if let Some(rd) = c.fringe {
            o.dict_put("RD", real_array(doc, &rd)?)?;
        }
        if symbol {
            o.dict_put("Sy", PdfObject::new_name(&c.symbol)?)?;
        }
    }
    let Some((p, r)) = ap else { return Ok(false) };
    let rect = real_array(doc, &r)?;
    let mut d = doc.new_dict()?;
    // The appearance's objects are written last, after everything that can fail for this comment: a signed
    // document's incremental save writes every new object, referenced or not.
    d.dict_put("N", ap::install(doc, p)?)?;
    o.dict_put("AP", d)?;
    o.dict_put("Rect", rect)?;
    // So that the page shows the new appearance now, not after the document is reopened.
    a.update()?;
    Ok(true)
}

fn new_caret(doc: &mut PdfDocument, page: u32, r: [f32; 4], st: &Style, author: &str) -> Result<i32> {
    let mut p = doc.load_pdf_page(page as i32)?;
    let mut a = p.add_caret_annotation(super::rect_of(r))?;
    a.set_color(rgb(st.color.unwrap_or([0.18, 0.294, 0.561])))?;
    if let Some(o) = st.opacity {
        a.set_opacity(o.clamp(0.0, 1.0))?;
    }
    meta(&mut a, author, true)?;
    a.update()?;
    Ok(a.xref()?)
}

fn new_file(doc: &mut PdfDocument, page: u32, r: [f32; 4], st: &Style, c: &Comment) -> Result<i32> {
    let file = c.file.as_ref().ok_or_else(|| bad("this attachment has no file"))?;
    let mut p = doc.load_pdf_page(page as i32)?;
    let mut a = p.create_annotation(PdfAnnotationType::FileAttachment)?;
    let icon = if ICONS.contains(&c.icon.as_str()) { c.icon.as_str() } else { "PushPin" };
    // The file is written after everything else that can fail, so a failure leaves no unreferenced stream behind.
    let made = (|| -> Result<()> {
        a.set_rect(super::rect_of(r))?;
        a.set_icon_name(icon)?;
        a.set_color(rgb(st.color.unwrap_or([0.18, 0.294, 0.561])))?;
        if let Some(o) = st.opacity {
            a.set_opacity(o.clamp(0.0, 1.0))?;
        }
        meta(&mut a, &c.author, true)?;
        a.update()?;
        // Last, for the same reason as the appearance's objects in `finish`.
        let fs = attach::install(doc, file)?;
        a.object().dict_put("FS", fs)?;
        Ok(())
    })();
    if let Err(e) = made {
        let _ = p.delete_annotation(a);
        return Err(e);
    }
    Ok(a.xref()?)
}

fn new_polyline(doc: &mut PdfDocument, page: u32, pts: &[[f32; 2]], st: &Style, author: &str) -> Result<i32> {
    let mut p = doc.load_pdf_page(page as i32)?;
    let mut a = p.add_polyline_annotation(pts.iter().map(|q| Point { x: q[0], y: q[1] }))?;
    a.set_color(rgb(st.color.unwrap_or([0.18, 0.294, 0.561])))?;
    if let Some(o) = st.opacity {
        a.set_opacity(o.clamp(0.0, 1.0))?;
    }
    if let Some(w) = st.width {
        a.set_border_width(w.max(0.0))?;
    }
    meta(&mut a, author, true)?;
    a.update()?;
    Ok(a.xref()?)
}

/// Changes the existing comment `xref` to what `c` says (same type; checked by the caller).
fn update(doc: &mut PdfDocument, page: u32, xref: i32, c: &Comment, m: &Matrix) -> Result<()> {
    let (_p, mut a) = find(doc, page, xref)?;
    if c.sub == Sub::FreeText {
        if !c.da.is_empty() {
            let (_, size, color) = parse_da(&c.da);
            a.set_default_appearance("Helv", if size > 0.0 { size } else { 12.0 }, Some(rgb(color)))?;
        }
    } else if let Some(v) = c.color {
        a.set_color(rgb(v))?;
    }
    // Only the types that have a fill or a border take one (MuPDF throws otherwise, after the colour changed).
    let shape = matches!(c.sub, Sub::Square | Sub::Circle | Sub::Line | Sub::PolyLine | Sub::Polygon);
    if let Some(v) = c.fill.filter(|_| shape) {
        a.set_interior_color(rgb(v))?;
    }
    if let Some(v) = c.opacity {
        a.set_opacity(v.clamp(0.0, 1.0))?;
    }
    if let Some(v) = c.width.filter(|_| shape || c.sub == Sub::Ink) {
        a.set_border_width(v.max(0.0))?;
    }
    if !c.contents.is_empty() || c.sub == Sub::Text {
        a.set_contents(&c.contents)?;
    }
    let quad = |g: &[f32]| {
        let p = tpairs(m, g);
        Quad {
            ul: Point { x: p[0][0], y: p[0][1] },
            ur: Point { x: p[1][0], y: p[1][1] },
            ll: Point { x: p[2][0], y: p[2][1] },
            lr: Point { x: p[3][0], y: p[3][1] },
        }
    };
    let pt = |q: [f32; 2]| Point { x: q[0], y: q[1] };
    match c.sub {
        s if s.markup().is_some() => {
            if !c.quads.is_empty() {
                a.set_quad_points(c.quads.chunks_exact(8).map(quad).collect::<Vec<_>>())?;
            }
        }
        Sub::Line => {
            if let Some(l) = c.line {
                a.set_line(pt(tp(m, l[0], l[1])), pt(tp(m, l[2], l[3])))?;
            }
        }
        Sub::Polygon | Sub::PolyLine => {
            if c.vertices.len() >= 4 {
                a.set_vertices(tpairs(m, &c.vertices).into_iter().map(pt))?;
            }
        }
        Sub::Ink => {
            let strokes: Vec<Vec<Point>> = c.strokes.iter().map(|s| tpairs(m, s).into_iter().map(pt).collect()).filter(|s: &Vec<Point>| !s.is_empty()).collect();
            if !strokes.is_empty() {
                a.set_ink_list(strokes.iter().map(|s| s.iter().copied()))?;
            }
        }
        _ => {
            if let Some(r) = c.rect {
                let r = trect(m, r);
                a.set_rect(super::rect_of(r))?;
            }
        }
    }
    a.update()?;
    drop(a);
    stamp(doc, page, xref, c)
}

/// [`finish`] for a comment this import made or updated, and the report of what of it could not be carried.
fn raw(doc: &mut PdfDocument, page: u32, xref: i32, c: &Comment, sum: &mut Summary, notes: &mut BTreeMap<&'static str, usize>) {
    for w in &c.notes {
        *notes.entry(w).or_default() += 1;
    }
    if c.ap.is_some() && c.rect.is_none() {
        *notes.entry(ap::DAMAGED).or_default() += 1;
    }
    match finish(doc, page, xref, c) {
        Ok(true) => sum.appearances += 1,
        Ok(false) => {}
        Err(_) => *notes.entry(ap::DAMAGED).or_default() += 1,
    }
}

/// Applies the comments of a file to `doc`. The caller has checked that the document is not certified
/// and runs this inside one edit step. Returns the summary and the pages touched.
pub(crate) fn import(doc: &mut PdfDocument, parsed: &Parsed) -> Result<(Summary, Vec<u32>)> {
    let pages = doc.page_count()?.max(0) as i64;
    let mut sum = Summary::default();
    let mut why: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut notes: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut touched: Vec<u32> = Vec::new();
    for (r, k) in [(UNKNOWN, parsed.unknown), (LIMIT, parsed.over)] {
        if k > 0 {
            *why.entry(r).or_default() += k;
        }
    }
    // name -> (page, xref, type) of every comment the document has.
    let mut known: HashMap<String, (u32, i32, Option<Sub>)> = HashMap::new();
    // Names of the document's measurements: an update would move them away from the /Measure they carry.
    let mut measured_here: HashSet<String> = HashSet::new();
    let budget = Budget::new();
    each(doc, |p, a| {
        let o = a.object();
        let nm = string(&o, "NM", &budget);
        if !nm.is_empty() {
            if is_measurement(&o) {
                measured_here.insert(nm.clone());
            }
            known.entry(nm).or_insert((p, a.xref()?, Sub::of_type(a.r#type()?)));
        }
        Ok(())
    })?;
    budget.check()?;
    let mut ctms: HashMap<u32, Matrix> = HashMap::new();
    let mut ctm = |doc: &PdfDocument, p: u32| -> Result<Matrix> {
        if let Some(m) = ctms.get(&p) {
            return Ok(m.clone());
        }
        let m = doc.load_pdf_page(p as i32)?.ctm()?;
        ctms.insert(p, m.clone());
        Ok(m)
    };

    let mut pending: Vec<&Comment> = Vec::new();
    for c in &parsed.items {
        if c.measured {
            // Made without its /Measure it would be a plain line or polygon claiming to be one; not made at all.
            *why.entry(MEASURED).or_default() += 1;
            continue;
        }
        if measured_here.contains(&c.name) {
            *why.entry(MEASURED_HERE).or_default() += 1;
            continue;
        }
        if !c.parent.is_empty() {
            pending.push(c);
            continue;
        }
        if c.page < 0 || c.page >= pages {
            *why.entry(ABSENT).or_default() += 1;
            continue;
        }
        let page = c.page as u32;
        let m = ctm(doc, page)?;
        if let Some(&(kp, kx, ksub)) = known.get(&c.name).filter(|_| !c.name.is_empty()) {
            if ksub != Some(c.sub) {
                *why.entry(DIFFERS).or_default() += 1;
                continue;
            }
            // A comment lives on one page: the file's page is only used to place a new one.
            let m = ctm(doc, kp)?;
            match update(doc, kp, kx, c, &m) {
                Ok(()) => {
                    sum.updated += 1;
                    touched.push(kp);
                    raw(doc, kp, kx, c, &mut sum, &mut notes);
                    if c.file.is_some() {
                        *notes.entry(attach::KEPT).or_default() += 1;
                    }
                }
                Err(_) => *why.entry(FAILED).or_default() += 1,
            }
            continue;
        }
        let made = make(c, &m).and_then(|(what, st)| {
            let r = match &what {
                Made::Tool(w) => add(doc, page, w, &st, &c.author),
                Made::PolyLine(points) => new_polyline(doc, page, points, &st, &c.author),
                Made::Caret(r) => new_caret(doc, page, *r, &st, &c.author),
                Made::File(r) => new_file(doc, page, *r, &st, c),
            };
            r.map_err(|_| FAILED)
        });
        match made {
            Ok(xref) => {
                if stamp(doc, page, xref, c).is_err() {
                    *why.entry(FAILED).or_default() += 1;
                    continue;
                }
                if !c.name.is_empty() {
                    known.insert(c.name.clone(), (page, xref, Some(c.sub)));
                }
                sum.added += 1;
                sum.files += usize::from(c.sub == Sub::FileAttachment);
                touched.push(page);
                raw(doc, page, xref, c, &mut sum, &mut notes);
            }
            Err(r) => *why.entry(r).or_default() += 1,
        }
    }
    // Replies and review states, parents first: a reply to a reply waits for its parent.
    for _ in 0..MAX_DEPTH {
        let before = pending.len();
        let mut next = Vec::new();
        for c in std::mem::take(&mut pending) {
            let Some(&(page, parent, _)) = known.get(&c.parent) else {
                next.push(c);
                continue;
            };
            if c.is_state() {
                if known.contains_key(&c.name) && !c.name.is_empty() {
                    *why.entry(STATE_KNOWN).or_default() += 1;
                    continue;
                }
                let r = super::set_state(doc, page, parent, &c.state, &c.author);
                match r {
                    Ok(x) => {
                        let _ = stamp(doc, page, x, c);
                        if !c.name.is_empty() {
                            known.insert(c.name.clone(), (page, x, Some(Sub::Text)));
                        }
                        sum.added += 1;
                        touched.push(page);
                    }
                    Err(_) => *why.entry(BAD_STATE).or_default() += 1,
                }
            } else if c.sub != Sub::Text {
                *why.entry(NOT_NOTE).or_default() += 1;
            } else if let Some(&(kp, kx, ksub)) = known.get(&c.name).filter(|_| !c.name.is_empty()) {
                // A reply is a note: the name of a comment of another type is not its own.
                if ksub != Some(Sub::Text) {
                    *why.entry(DIFFERS).or_default() += 1;
                    continue;
                }
                let m = ctm(doc, kp)?;
                match update(doc, kp, kx, c, &m) {
                    Ok(()) => {
                        sum.updated += 1;
                        touched.push(kp);
                    }
                    Err(_) => *why.entry(FAILED).or_default() += 1,
                }
            } else {
                match super::reply(doc, page, parent, &c.contents, &c.author).and_then(|x| stamp(doc, page, x, c).map(|()| x)) {
                    Ok(x) => {
                        if !c.name.is_empty() {
                            known.insert(c.name.clone(), (page, x, Some(Sub::Text)));
                        }
                        sum.added += 1;
                        touched.push(page);
                    }
                    Err(_) => *why.entry(FAILED).or_default() += 1,
                }
            }
        }
        let progress = next.len() < before;
        pending = next;
        if pending.is_empty() || !progress {
            break;
        }
    }
    if !pending.is_empty() {
        *why.entry(NO_PARENT).or_default() += pending.len();
    }
    sum.skipped = skipped(why);
    sum.notes = skipped(notes);
    touched.sort_unstable();
    touched.dedup();
    Ok((sum, touched))
}

/// Imports parsed comments as one undo step. A certified document is refused before anything changes; a
/// signed one is changed incrementally (nothing already written is rewritten) and the result says so.
pub fn import_one(eng: &Engine, doc: DocId, parsed: &Parsed, source: EditSource) -> Result<Imported> {
    let signed = eng.read_custom(doc, |d| Ok(crate::outline::signed(d)))?;
    let mut summary = Summary::default();
    let state = eng.apply_custom_unless_noop(doc, "Import comments".into(), source, false, |d| {
        crate::outline::refuse_unless_annotating_allowed(d, "Importing comments")?;
        let (s, pages) = import(d, parsed)?;
        let changed = s.added + s.updated > 0;
        summary = s;
        Ok(changed.then_some(pages))
    })?;
    Ok(Imported { summary, state, signed })
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_full;
