//! Comments to and from XFDF (ISO 19444-1) and FDF (ISO 32000-2, 12.7.8): Acrobat's "Export / Import
//! data file" for comments. Both files are hostile input (see `crate::exchange` for the caps).
//!
//! Export reads each comment's own dictionary, so geometry is in PDF user space (y up), which is what
//! these files use; the list DTO is in page space. Import maps the file's points through the page
//! transform, creates the comment with the same code the tools use (which draws our own appearance:
//! no `/AP` or `<appearance>` of the file is used), then puts the file's name, author, dates and
//! subject back. A comment is matched by its `name` (`/NM`): the one that exists is updated in place,
//! the others are added, and a reply keeps its parent.
//!
//! Not carried: appearance streams, rich text, pop-up windows (a comment's pop-up is written, not read),
//! attachments, sound, carets and redaction marks (skipped and counted), callout lines of free text.

use super::{Kind, NewAnnot, Style, add, bad, each, find, meta, parent_of, put_str, rgb};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::exchange::{self, Format, MAX_DEPTH, MAX_ITEMS, MAX_NODES, MAX_STR, esc, pdf_name, pdf_text};
use crate::forms::{decode_text, get, parse_da, text};
use mupdf::pdf::{AnnotationFlags, PdfAnnotationType, PdfDocument, PdfObject};
use mupdf::{Matrix, Point, Quad};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

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
}

const SUBS: [(Sub, &str, &str); 13] = [
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

fn string(o: &PdfObject, k: &str) -> String {
    let Some(v) = get(o, k) else { return String::new() };
    let s = if v.is_string().unwrap_or(false) {
        v.as_string().ok().or_else(|| v.as_bytes().ok().map(|b| decode_text(&b)))
    } else {
        text(&v)
    };
    s.filter(|s| s.len() <= MAX_STR).unwrap_or_default()
}

fn floats(o: &PdfObject, cap: usize) -> Vec<f32> {
    if !o.is_array().unwrap_or(false) {
        return Vec::new();
    }
    let n = o.len().unwrap_or(0).min(cap);
    (0..n as i32).filter_map(|i| o.get_array(i).ok().flatten().and_then(|v| v.as_float().ok())).filter(|v| v.is_finite()).collect()
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

/// Reads one comment dictionary. `/AP`, `/AA`, `/A`, `/F` file specifications and everything that is
/// not listed here are never looked at.
fn read(o: &PdfObject, sub: Sub) -> Comment {
    let mut c = Comment::new(sub);
    c.name = string(o, "NM");
    c.contents = string(o, "Contents");
    c.author = string(o, "T");
    c.subject = string(o, "Subj");
    c.created = string(o, "CreationDate");
    c.modified = string(o, "M");
    c.da = string(o, "DA");
    c.state = string(o, "State");
    c.state_model = string(o, "StateModel");
    c.icon = get(o, "Name").as_ref().and_then(text).unwrap_or_default();
    c.reply_type = get(o, "RT").as_ref().and_then(text).unwrap_or_default();
    c.flags = get(o, "F").and_then(|f| f.as_int().ok());
    let r = get(o, "Rect").map(|a| floats(&a, 4)).unwrap_or_default();
    if r.len() == 4 {
        c.rect = Some([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
    }
    let q = get(o, "QuadPoints").map(|a| floats(&a, MAX_QUADS * 8)).unwrap_or_default();
    c.quads = q[..q.len() / 8 * 8].to_vec();
    let l = get(o, "L").map(|a| floats(&a, 4)).unwrap_or_default();
    if l.len() == 4 {
        c.line = Some([l[0], l[1], l[2], l[3]]);
    }
    if let Some(le) = get(o, "LE").filter(|a| a.is_array().unwrap_or(false)) {
        let name = |i: i32| le.get_array(i).ok().flatten().as_ref().and_then(text).unwrap_or_default();
        (c.head, c.tail) = (name(0), name(1));
    }
    c.vertices = pairs(&get(o, "Vertices").map(|a| floats(&a, MAX_NUMS)).unwrap_or_default());
    if let Some(ink) = get(o, "InkList").filter(|a| a.is_array().unwrap_or(false)) {
        for i in 0..ink.len().unwrap_or(0).min(MAX_STROKES) as i32 {
            if let Some(s) = ink.get_array(i).ok().flatten() {
                c.strokes.push(pairs(&floats(&s, MAX_NUMS)));
            }
        }
    }
    c.color = get(o, "C").and_then(|a| rgb_from(&floats(&a, 4)));
    c.fill = get(o, "IC").and_then(|a| rgb_from(&floats(&a, 4)));
    c.opacity = get(o, "CA").and_then(|v| v.as_float().ok()).filter(|v| v.is_finite());
    c.width = get(o, "BS")
        .and_then(|b| get(&b, "W"))
        .and_then(|v| v.as_float().ok())
        .or_else(|| get(o, "Border").and_then(|b| floats(&b, 3).get(2).copied()))
        .filter(|v| v.is_finite());
    if let Some(p) = get(o, "Popup").filter(|p| p.is_dict().unwrap_or(false)) {
        let r = get(&p, "Rect").map(|a| floats(&a, 4)).unwrap_or_default();
        if r.len() == 4 {
            let open = get(&p, "Open").and_then(|v| v.as_bool().ok()).unwrap_or(false);
            c.popup = Some(([r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])], open));
        }
    }
    c
}

/// Every comment of the document that has a type files can carry, in page order, and how many were
/// left out for having another type (carets, attachments, sound, ...). Unnamed comments get a stable
/// name for this export only; nothing is written to the document.
fn collect(doc: &PdfDocument) -> Result<(Vec<Comment>, usize)> {
    let mut left = 0usize;
    let (all, _) = each(doc, |page, a| {
        let Some(sub) = Sub::of_type(a.r#type()?) else {
            left += 1;
            return Ok(None);
        };
        let o = a.object();
        let mut c = read(&o, sub);
        c.page = i64::from(page);
        c.xref = a.xref()?;
        c.parent_xref = parent_of(&o);
        Ok(Some(c))
    })?;
    let mut items: Vec<Comment> = all.into_iter().flatten().collect();
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
    Ok((items, left))
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
        let mut put = |k: &str, v: &str| {
            if !v.is_empty() {
                let _ = write!(at, " {k}=\"{}\"", esc(v));
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
        put("inreplyto", &c.parent);
        put("replyType", &c.reply_type);
        put("statemodel", &c.state_model);
        put("state", &c.state);
        let _ = write!(o, "<{tag}{at}>");
        if !c.contents.is_empty() {
            let _ = write!(o, "<contents>{}</contents>", esc(&c.contents));
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
        let _ = writeln!(o, "</{tag}>");
    }
    o.push_str("</annots>\n</xfdf>\n");
    o
}

fn fdf(items: &[Comment], file: &str) -> Vec<u8> {
    // Object 1 is the root; comment i is object i + 2, so a reply's /IRT points inside this file.
    let numbers: HashMap<&str, usize> = items.iter().enumerate().map(|(i, c)| (c.name.as_str(), i + 2)).collect();
    let mut objs: Vec<String> = Vec::with_capacity(items.len() + 1);
    let refs: Vec<String> = (0..items.len()).map(|i| format!("{} 0 R", i + 2)).collect();
    objs.push(format!("<< /FDF << /Annots [{}] /F {} >> >>", refs.join(" "), pdf_text(file)));
    for c in items {
        let mut d = format!("<< /Type /Annot /Subtype /{} /Page {}", c.sub.pdf(), c.page);
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
        d.push_str(" >>");
        objs.push(d);
    }
    let mut out = b"%FDF-1.2\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
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

/// What an export wrote.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub count: usize,
    /// Comments of a type the files cannot carry (carets, attachments, sound).
    pub left_out: usize,
}

/// The file bytes for every comment of the open document. Read-only: allowed on a signed or certified one.
pub fn export(eng: &Engine, doc: DocId, format: Format, file_name: &str) -> Result<(Vec<u8>, Exported)> {
    let (items, left_out) = eng.read_custom(doc, collect)?;
    let bytes = match format {
        Format::Fdf => fdf(&items, file_name),
        _ => xfdf(&items, file_name).into_bytes(),
    };
    Ok((bytes, Exported { count: items.len(), left_out }))
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
        let Some(annots) = get(root, "Annots").filter(|a| a.is_array().unwrap_or(false)) else { return Ok(out) };
        let count = annots.len().unwrap_or(0).min(MAX_NODES);
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
                number_of.insert(n, string(&e, "NM"));
            }
            entries.push(e);
        }
        for e in entries {
            let sub = get(&e, "Subtype").as_ref().and_then(text).and_then(|t| Sub::of_pdf(&t));
            let Some(sub) = sub else {
                out.unknown += 1;
                continue;
            };
            if out.items.len() >= MAX_ITEMS {
                out.over += 1;
                continue;
            }
            let mut c = read(&e, sub);
            c.page = get(&e, "Page").and_then(|p| p.as_int().ok()).map_or(-1, i64::from);
            // The parent is named by its /NM: through the object the /IRT points at (this file's own
            // object number first), or the dictionary written in place.
            if let Some(p) = e.get_dict("IRT").ok().flatten() {
                c.parent = if p.is_indirect().unwrap_or(false) {
                    p.as_indirect().ok().and_then(|n| number_of.get(&n).cloned()).unwrap_or_default()
                } else {
                    string(&p, "NM")
                };
            }
            out.items.push(c);
        }
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
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub reason: String,
    pub count: usize,
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
const FAILED: &str = "could not be created";

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
        a.set_flags(AnnotationFlags::from_bits_truncate(f))?;
    }
    if c.sub == Sub::Text && !c.icon.is_empty() {
        a.set_icon_name(&c.icon)?;
    }
    a.update()?;
    Ok(())
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
    if let Some(v) = c.fill {
        a.set_interior_color(rgb(v))?;
    }
    if let Some(v) = c.opacity {
        a.set_opacity(v.clamp(0.0, 1.0))?;
    }
    if let Some(v) = c.width.filter(|_| c.sub != Sub::FreeText) {
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

/// Applies the comments of a file to `doc`. The caller has checked that the document is not certified
/// and runs this inside one edit step. Returns the summary and the pages touched.
pub(crate) fn import(doc: &mut PdfDocument, parsed: &Parsed) -> Result<(Summary, Vec<u32>)> {
    let pages = doc.page_count()?.max(0) as i64;
    let mut sum = Summary::default();
    let mut why: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut touched: Vec<u32> = Vec::new();
    for (r, k) in [(UNKNOWN, parsed.unknown), (LIMIT, parsed.over)] {
        if k > 0 {
            *why.entry(r).or_default() += k;
        }
    }
    // name -> (page, xref, type) of every comment the document has.
    let mut known: HashMap<String, (u32, i32, Option<Sub>)> = HashMap::new();
    each(doc, |p, a| {
        let o = a.object();
        let nm = string(&o, "NM");
        if !nm.is_empty() {
            known.entry(nm).or_insert((p, a.xref()?, Sub::of_type(a.r#type()?)));
        }
        Ok(())
    })?;
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
                }
                Err(_) => *why.entry(FAILED).or_default() += 1,
            }
            continue;
        }
        let made = make(c, &m).and_then(|(what, st)| {
            let r = match &what {
                Made::Tool(w) => add(doc, page, w, &st, &c.author),
                Made::PolyLine(points) => new_polyline(doc, page, points, &st, &c.author),
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
                touched.push(page);
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
            } else if let Some(&(kp, kx, _)) = known.get(&c.name).filter(|_| !c.name.is_empty()) {
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
    sum.skipped = why.into_iter().map(|(reason, count)| Skipped { reason: reason.to_string(), count }).collect();
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
        if crate::outline::certified(d)?.is_some() {
            return Err(bad("This document is certified. Importing comments would void the certification, so Inkwell leaves it as it is."));
        }
        let (s, pages) = import(d, parsed)?;
        let changed = s.added + s.updated > 0;
        summary = s;
        Ok(changed.then_some(pages))
    })?;
    Ok(Imported { summary, state, signed })
}

#[cfg(test)]
mod tests;
