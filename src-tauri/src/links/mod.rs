//! Links: the link annotations of a page (list, draw one, change where it goes, its border,
//! delete) and links made from the addresses in the text.
//!
//! Every change is one `Engine::apply_custom` step on the authoritative document: one undo step,
//! kept by Save, recorded in history. A link is named by `(page, object number)`. Coordinates in and
//! out are the viewer's page space (points, origin top left, `/Rotate` applied). A web address
//! passes [`gate_url`] before it is written or opened, and nothing here opens one: the UI asks
//! first (docs/bookmarks-links.md). A link to a file is made from an open token (never a path).

pub mod app;
#[cfg(test)]
mod tests;
mod urls;

pub use urls::UrlPreview;

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::outline::{Names, broken_reason, certified, dest_array, page_dest, text_object};
use crate::textedit::{Matrix, page_space};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::Buffer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

type Result<T> = std::result::Result<T, EngineError>;

/// A web address longer than this is refused.
pub const MAX_URL: usize = 2048;
/// The schemes a link may take people to.
const SCHEMES: [&str; 3] = ["https", "http", "mailto"];
/// Links of one page read at most.
const MAX_LINKS: usize = 5000;
/// The smallest side of a link rectangle, in points.
const MIN_SIDE: f32 = 2.0;

/// Where a link goes.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Target {
    /// A page (0-based), at the point `at` (page space) when the destination names one.
    Page { page: u32, at: Option<[f32; 2]> },
    Uri { uri: String },
    /// A file: its name as the link has it, and the page of a PDF it opens at.
    File { name: String, page: Option<u32> },
    /// Another kind of action, kept as it is (JavaScript, a named action), or nowhere.
    Other { what: String },
    /// A destination that can't be followed: a named destination that is not in the document, or a
    /// page that is. `what` says which, as a short phrase.
    Broken { what: String },
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Border {
    pub visible: bool,
    /// Points.
    pub width: f32,
    /// RGB 0..1.
    pub color: [f32; 3],
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// The annotation's object number.
    pub id: i32,
    /// `[x0, y0, x1, y1]` in page space.
    pub rect: [f32; 4],
    pub target: Target,
    pub border: Border,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PageLinks {
    /// The generation the list belongs to; pass it back with a change.
    pub rev: u32,
    pub links: Vec<Link>,
}

/// Where a new or changed link goes.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NewTarget {
    /// `at`: the point shown at the top left, page space; none shows the page from its top.
    Page { page: u32, at: Option<[f32; 2]> },
    Uri { uri: String },
    /// `file`: an open token (the command swaps it for the path before the engine sees it);
    /// `page`: the page of a PDF it opens at.
    File { file: String, page: Option<u32> },
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BorderSpec {
    pub visible: bool,
    /// Points; 1 when it is not given.
    pub width: Option<f32>,
    /// RGB 0..1; ink blue when it is not given.
    pub color: Option<[f32; 3]>,
}

/// One change to the links.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum LinkOp {
    Add { page: u32, rect: [f32; 4], target: NewTarget, border: Option<BorderSpec> },
    SetTarget { page: u32, id: i32, target: NewTarget },
    SetRect { page: u32, id: i32, rect: [f32; 4] },
    SetBorder { page: u32, id: i32, border: BorderSpec },
    Delete { page: u32, ids: Vec<i32> },
    /// A link on every web address in the text that has none (see [`urls`]).
    FromUrls,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LinkEdited {
    pub state: EditState,
    /// The new link (Add).
    pub id: Option<i32>,
    /// Links added or removed.
    pub count: u32,
    /// For a link to a file (Add, SetTarget): the name the link now has for it, which is found from
    /// the folder of the document: a bare name, or a path with `..` when the file is above or beside it.
    pub name: Option<String>,
}

/// Resolves the file token of a target: `f` turns it into a path.
pub fn resolve_files(op: &mut LinkOp, f: impl Fn(&str) -> Result<String>) -> Result<()> {
    match op {
        LinkOp::Add { target: NewTarget::File { file, .. }, .. } | LinkOp::SetTarget { target: NewTarget::File { file, .. }, .. } => {
            *file = f(file)?;
        }
        _ => {}
    }
    Ok(())
}

// ---- the address gate ----

/// `url` as it may be written into a link and opened: trimmed, at most [`MAX_URL`] long, one of
/// [`SCHEMES`], no hidden or control characters, a host for a web address, and a `mailto:` that
/// does not ask the mail program to attach a file. Characters outside ASCII are percent-encoded.
pub fn gate_url(url: &str) -> Result<String> {
    let bad = |why: &str| EngineError::Invalid(why.to_string());
    let u = url.trim();
    if u.is_empty() {
        return Err(bad("type the address first"));
    }
    if u.len() > MAX_URL {
        return Err(bad("that address is too long"));
    }
    if u.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')) {
        return Err(bad("the address has spaces or hidden characters"));
    }
    let Some((scheme, rest)) = u.split_once(':') else {
        return Err(bad("an address starts with https://, http:// or mailto:"));
    };
    let scheme = scheme.to_ascii_lowercase();
    if !SCHEMES.contains(&scheme.as_str()) {
        return Err(bad("only https://, http:// and mailto: addresses can be linked"));
    }
    let mut out = String::with_capacity(u.len() + 8);
    out.push_str(&scheme);
    out.push(':');
    let tail = if scheme == "mailto" {
        let (to, query) = rest.split_once('?').unwrap_or((rest, ""));
        if to.is_empty() && query.is_empty() {
            return Err(bad("the mail address is empty"));
        }
        // Parameter names are compared decoded (`%61ttach` is `attach`), with `&` or `;` between them.
        let asks_for_a_file = query.split(['&', ';']).any(|p| {
            let name = percent_decode(p.split('=').next().unwrap_or("")).to_ascii_lowercase();
            name.starts_with("attach") || name.starts_with("insert")
        });
        if asks_for_a_file {
            return Err(bad("a mail link can't attach files"));
        }
        rest
    } else {
        let Some(after) = rest.strip_prefix("//") else {
            return Err(bad("a web address starts with https:// or http://"));
        };
        let host = after.split(['/', '?', '#']).next().unwrap_or("");
        let host = host.rsplit('@').next().unwrap_or("");
        let host = host.split(':').next().unwrap_or("");
        if host.is_empty() || !host.chars().any(|c| c.is_alphanumeric()) {
            return Err(bad("the web address has no site name"));
        }
        out.push_str("//");
        after
    };
    // What RFC 3986 does not allow in an address is percent-encoded: everything outside ASCII, the
    // characters that are not part of the syntax (`"<>\^`{|}`), and a `%` that starts no escape.
    let chars: Vec<char> = tail.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        let is_escape = ch == '%' && chars.get(i + 1).is_some_and(char::is_ascii_hexdigit) && chars.get(i + 2).is_some_and(char::is_ascii_hexdigit);
        if ch.is_ascii() && !matches!(ch, '"' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}') && (ch != '%' || is_escape) {
            out.push(ch);
        } else {
            let mut b = [0u8; 4];
            for byte in ch.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    Ok(out)
}

/// `s` with its `%XX` escapes decoded (anything that is not an escape is kept).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() {
            out.push((hex(b[i + 1]).unwrap() * 16 + hex(b[i + 2]).unwrap()) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---- reading ----

fn nums(o: &PdfObject, n: i32) -> Option<Vec<f32>> {
    let v: Vec<f32> = (0..n)
        .filter_map(|i| {
            let e = o.get_array(i).ok().flatten()?;
            let e = e.resolve().ok().flatten().unwrap_or(e);
            e.is_number().ok()?.then(|| e.as_float().ok()).flatten()
        })
        .collect();
    (v.len() == n as usize).then_some(v)
}

fn name_of(o: Option<PdfObject>) -> String {
    o.and_then(|s| s.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned()).unwrap_or_default()
}

fn str_of(o: Option<PdfObject>) -> String {
    let Some(o) = o else { return String::new() };
    let o = o.resolve().ok().flatten().unwrap_or(o);
    if o.is_string().unwrap_or(false) {
        crate::outline::decode_text(&o.as_bytes().unwrap_or_default())
    } else {
        String::new()
    }
}

const INK: [f32; 3] = [0.18, 0.294, 0.561];

fn border_of(a: &PdfObject) -> Border {
    let mut width = 1.0;
    if let Some(bs) = a.get_dict("BS").ok().flatten() {
        if let Some(w) = bs.get_dict("W").ok().flatten() {
            width = w.resolve().ok().flatten().unwrap_or(w).as_float().unwrap_or(1.0);
        }
    } else if let Some(b) = a.get_dict("Border").ok().flatten() {
        width = nums(&b, 3).map_or(1.0, |v| v[2]);
    }
    let c = a.get_dict("C").ok().flatten();
    let color = c.as_ref().and_then(|c| nums(c, 3)).map_or([0.0, 0.0, 0.0], |v| [v[0], v[1], v[2]].map(|x| x.clamp(0.0, 1.0)));
    // Acrobat draws nothing for width 0, an empty `/C []` or the hidden flag.
    let no_colour = c.as_ref().is_some_and(|c| c.is_array().unwrap_or(false) && c.len().unwrap_or(1) == 0);
    let hidden = a.get_dict("F").ok().flatten().is_some_and(|f| f.as_int().unwrap_or(0) & 2 != 0);
    Border { visible: width > 0.0 && !no_colour && !hidden, width: width.max(0.0), color }
}

fn target_of(doc: &PdfDocument, a: &PdfObject, spaces: &mut HashMap<u32, Option<Matrix>>, names: &mut Names) -> Target {
    let mut dest = a.get_dict("Dest").ok().flatten();
    if dest.is_none() {
        if let Some(act) = a.get_dict("A").ok().flatten() {
            match name_of(act.get_dict("S").ok().flatten()).as_str() {
                "GoTo" => dest = act.get_dict("D").ok().flatten(),
                "URI" => return Target::Uri { uri: str_of(act.get_dict("URI").ok().flatten()) },
                s @ ("GoToR" | "Launch") => {
                    let f = act.get_dict("F").ok().flatten();
                    let name = f
                        .as_ref()
                        .map(|f| {
                            if f.is_dict().unwrap_or(false) {
                                let n = str_of(f.get_dict("UF").ok().flatten());
                                if n.is_empty() { str_of(f.get_dict("F").ok().flatten()) } else { n }
                            } else {
                                str_of(Some(f.clone()))
                            }
                        })
                        .unwrap_or_default();
                    let page = (s == "GoToR")
                        .then(|| act.get_dict("D").ok().flatten())
                        .flatten()
                        .and_then(|d| d.get_array(0).ok().flatten())
                        .and_then(|p| p.is_number().ok()?.then(|| p.as_int().ok()).flatten())
                        .filter(|p| *p >= 0)
                        .map(|p| p as u32);
                    return Target::File { name, page };
                }
                other => return Target::Other { what: other.to_string() },
            }
        }
    }
    let Some(d) = dest else { return Target::Other { what: String::new() } };
    match dest_array(doc, names, &d).and_then(|arr| page_dest(doc, spaces, &arr)) {
        Some((page, at)) => Target::Page { page, at },
        None => Target::Broken { what: broken_reason(&d) },
    }
}

fn is_link(a: &PdfObject) -> bool {
    name_of(a.get_dict("Subtype").ok().flatten()) == "Link"
}

/// The link annotations of `page` that are indirect objects (the ones that can be named).
fn page_annots(doc: &PdfDocument, page: u32) -> Result<Vec<PdfObject>> {
    let p = doc.find_page(page as i32)?;
    let Some(annots) = p.get_dict("Annots")? else { return Ok(Vec::new()) };
    let arr = annots.resolve()?.unwrap_or(annots);
    if !arr.is_array()? {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for a in arr.array_iter()? {
        let a = a?;
        if out.len() >= MAX_LINKS {
            break;
        }
        if a.is_indirect()? && a.resolve()?.is_some_and(|d| d.is_dict().unwrap_or(false) && is_link(&d)) {
            out.push(a);
        }
    }
    Ok(out)
}

pub fn read_page(doc: &PdfDocument, page: u32, rev: u32) -> Result<PageLinks> {
    read_page_with(doc, page, rev, &mut Names::default())
}

/// [`read_page`] for a read of many pages: the named destinations are loaded once, in `names`.
pub(crate) fn read_page_with(doc: &PdfDocument, page: u32, rev: u32, names: &mut Names) -> Result<PageLinks> {
    let m = page_space(doc, page)?;
    let mut spaces = HashMap::new();
    let mut links = Vec::new();
    for a in page_annots(doc, page)? {
        let Some(r) = a.get_dict("Rect")?.and_then(|r| nums(&r, 4)) else { continue };
        let b = m.bbox([r[0], r[1], r[2], r[3]].map(f64::from));
        let (x0, x1) = (b[0].min(b[2]) as f32, b[0].max(b[2]) as f32);
        let (y0, y1) = (b[1].min(b[3]) as f32, b[1].max(b[3]) as f32);
        links.push(Link { id: a.as_indirect()?, rect: [x0, y0, x1, y1], target: target_of(doc, &a, &mut spaces, names), border: border_of(&a) });
    }
    Ok(PageLinks { rev, links })
}

pub fn list(eng: &Engine, doc: DocId, page: u32) -> Result<PageLinks> {
    let entry = eng.doc_entry(doc)?;
    eng.read_custom(doc, |d| {
        if page as i32 >= d.page_count()? {
            return Err(EngineError::BadPage(page));
        }
        read_page(d, page, entry.generation())
    })
}

// ---- audit ----

/// Rows of an audit that are listed one by one.
const AUDIT_ROWS: usize = 200;

/// A link whose destination can't be followed.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuditRow {
    /// 0-based.
    pub page: u32,
    pub id: i32,
    /// Why, as a short phrase.
    pub what: String,
}

/// What the links of the whole document are: how many, how many go to the web, and the ones that
/// are broken.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkAudit {
    pub total: u32,
    pub external: u32,
    pub broken: u32,
    /// The first of the broken ones.
    pub rows: Vec<AuditRow>,
}

pub fn audit(eng: &Engine, doc: DocId) -> Result<LinkAudit> {
    eng.read_custom(doc, |d| {
        let mut out = LinkAudit::default();
        let mut names = Names::default();
        for page in 0..d.page_count()?.max(0) as u32 {
            for l in read_page_with(d, page, 0, &mut names)?.links {
                out.total += 1;
                match l.target {
                    Target::Uri { .. } => out.external += 1,
                    Target::Broken { what } => {
                        out.broken += 1;
                        if out.rows.len() < AUDIT_ROWS {
                            out.rows.push(AuditRow { page, id: l.id, what });
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(out)
    })
}

// ---- writing ----

/// A rectangle in page space as `/Rect` in user space.
fn user_rect(doc: &PdfDocument, page: u32, r: [f32; 4]) -> Result<[f32; 4]> {
    let back = page_space(doc, page)?.invert().ok_or_else(|| EngineError::Invalid("the page has no area".into()))?;
    let b = back.bbox(r.map(f64::from));
    let out = [b[0].min(b[2]) as f32, b[1].min(b[3]) as f32, b[0].max(b[2]) as f32, b[1].max(b[3]) as f32];
    if out[2] - out[0] < MIN_SIDE || out[3] - out[1] < MIN_SIDE {
        return Err(EngineError::Invalid("the link is too small".into()));
    }
    Ok(out)
}

fn real_array(doc: &PdfDocument, v: &[f32]) -> Result<PdfObject> {
    let mut arr = doc.new_array()?;
    for x in v {
        arr.array_push(PdfObject::new_real(*x)?)?;
    }
    Ok(arr)
}

/// Gives a link whose border is visible an appearance stream that draws it, so page tiles and other
/// readers show the border too (a link has none of its own to draw otherwise). `clear` takes the stream
/// off when the border is not visible: only where the person just changed the border, never for a
/// link that merely moved.
fn sync_ap(doc: &mut PdfDocument, a: &mut PdfObject, clear: bool) -> Result<()> {
    let b = border_of(a);
    let rect = a.get_dict("Rect")?.and_then(|r| nums(&r, 4));
    let Some(r) = rect.filter(|_| b.visible && b.width > 0.0) else {
        if clear {
            a.dict_delete("AP")?;
        }
        return Ok(());
    };
    let (w, h) = ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
    // The stroke is centred on the line: keep the whole width inside the rectangle.
    let bw = b.width.min(w / 2.0).min(h / 2.0);
    let content = format!(
        "q {:.3} {:.3} {:.3} RG {:.3} w {:.3} {:.3} {:.3} {:.3} re S Q",
        b.color[0],
        b.color[1],
        b.color[2],
        bw,
        bw / 2.0,
        bw / 2.0,
        (w - bw).max(0.0),
        (h - bw).max(0.0)
    );
    let mut form = doc.new_dict()?;
    form.dict_put("Type", PdfObject::new_name("XObject")?)?;
    form.dict_put("Subtype", PdfObject::new_name("Form")?)?;
    form.dict_put("BBox", real_array(doc, &[0.0, 0.0, w, h])?)?;
    let stream = doc.add_stream(&Buffer::from_bytes(content.as_bytes())?, Some(&form), false)?;
    let mut ap = doc.new_dict()?;
    ap.dict_put("N", stream)?;
    a.dict_put("AP", ap)?;
    Ok(())
}

fn put_border(doc: &PdfDocument, a: &mut PdfObject, b: &BorderSpec) -> Result<()> {
    let width = if b.visible { b.width.unwrap_or(1.0).clamp(0.25, 12.0) } else { 0.0 };
    let mut arr = doc.new_array()?;
    for v in [0, 0] {
        arr.array_push(PdfObject::new_int(v)?)?;
    }
    arr.array_push(PdfObject::new_real(width)?)?;
    a.dict_put("Border", arr)?;
    a.dict_delete("BS")?;
    if b.visible {
        let c = b.color.unwrap_or(INK).map(|v| v.clamp(0.0, 1.0));
        a.dict_put("C", real_array(doc, &c)?)?;
    } else {
        a.dict_delete("C")?;
    }
    Ok(())
}

/// The PDF a file link names, as a path on this machine, for opening it in Inkwell once the person
/// said yes (the confirmation shows this path in full). Only a relative name is followed, from the
/// document's folder: no drive or network share; and only a PDF, since nothing else is ever run from
/// a link.
pub fn resolve_file_link(doc_path: &str, name: &str) -> Result<std::path::PathBuf> {
    let bad = |why: String| EngineError::Invalid(why);
    let n = name.trim();
    if n.is_empty() {
        return Err(bad("the link names no file".into()));
    }
    if n.starts_with(['/', '\\']) || n.contains(':') || n.chars().any(char::is_control) {
        return Err(bad(format!("the link names a file outside this document's folder ({n})")));
    }
    let mut path = std::path::Path::new(doc_path).parent().map(|d| d.to_path_buf()).unwrap_or_default();
    for part in n.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            // Up from the folder of the document: allowed (the confirmation shows where it ends up).
            if !path.pop() {
                return Err(bad(format!("the link names a file that can't be found from this document ({n})")));
            }
            continue;
        }
        path.push(part);
    }
    let is_pdf = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !is_pdf {
        return Err(bad(format!("Inkwell opens only PDF files from a link, and this one is {n}")));
    }
    if !path.is_file() {
        return Err(bad(format!("{n} is not next to this document")));
    }
    Ok(path)
}

/// `p` as its volume (`c:`, `//server/share`, or `/`) and its components, `\\?\` prefixes dropped,
/// separators unified, `.` and `..` resolved.
fn split_volume(p: &str) -> Option<(String, Vec<String>)> {
    let mut s = p.replace('\\', "/");
    if let Some(r) = s.strip_prefix("//?/UNC/") {
        s = format!("//{r}");
    } else if let Some(r) = s.strip_prefix("//?/") {
        s = r.to_string();
    }
    let (vol, rest) = if let Some(r) = s.strip_prefix("//") {
        let mut it = r.splitn(3, '/');
        let host = it.next()?;
        let share = it.next()?;
        (format!("//{host}/{share}"), it.next().unwrap_or("").to_string())
    } else if s.len() >= 2 && s.as_bytes()[1] == b':' {
        (s[..2].to_string(), s[2..].to_string())
    } else if let Some(r) = s.strip_prefix('/') {
        ("/".to_string(), r.to_string())
    } else {
        return None;
    };
    let mut comps: Vec<String> = Vec::new();
    for c in rest.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                comps.pop();
            }
            c => comps.push(c.to_string()),
        }
    }
    Some((vol, comps))
}

/// A file as a link has it: its path from the folder of the document, with `/` between the parts and
/// `..` to go up (the file on the same drive as the document, found without regard to case). A file on
/// another drive or share can't be found from the document, so it is refused with that reason: Inkwell
/// never writes a path of this machine into a document.
fn file_link_name(doc_path: &str, file: &str) -> Result<String> {
    let bad = |m: String| EngineError::Invalid(m);
    let (Some((fv, fc)), Some((dv, dc))) = (split_volume(file), split_volume(doc_path)) else {
        return Err(bad("that file has no full path, so a link can't find it from the document".into()));
    };
    let Some(name) = fc.last() else { return Err(bad("that file has no name".into())) };
    if !fv.eq_ignore_ascii_case(&dv) {
        return Err(bad(format!(
            "{name} is on another drive than this document, so a link made here could not find it. Put a copy next to the document and link that."
        )));
    }
    let dir = &dc[..dc.len().saturating_sub(1)];
    let fdir = &fc[..fc.len() - 1];
    let same = |a: &String, b: &String| a.to_lowercase() == b.to_lowercase();
    let common = dir.iter().zip(fdir).take_while(|(a, b)| same(a, b)).count();
    let mut out: Vec<String> = vec!["..".to_string(); dir.len() - common];
    out.extend(fc[common..].iter().cloned());
    Ok(out.join("/"))
}

/// A byte string (`/F` of a file specification is one: PDFDocEncoding, not a text string): ASCII as it
/// is, other characters as their Latin-1 byte when they have one, else `_`.
fn byte_string(doc: &PdfDocument, text: &str) -> Result<PdfObject> {
    if text.chars().all(|c| c.is_ascii() && c != '\0' && !c.is_control()) {
        return Ok(PdfObject::new_string(text)?);
    }
    let mut hex = String::from("<");
    for c in text.chars() {
        let b = if (c as u32) < 0x100 && !c.is_control() { c as u8 } else { b'_' };
        hex.push_str(&format!("{b:02X}"));
    }
    hex.push('>');
    Ok(doc.new_object_from_str(&hex)?)
}

/// The kinds of file a link may open: documents the system shows, none of which runs code when it is
/// opened (not a program, a script, a shortcut, a disk image, or an Office file that can hold macros). Anything
/// not listed is refused: a list of what is dangerous is always one entry short.
const OPENABLE: &[&str] = &[
    "pdf", "txt", "text", "md", "csv", "tsv", "log", "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "docx", "xlsx", "pptx",
    "odt", "ods", "odp",
];

/// The refusal for a file a link must not open, or `None`.
fn runnable(name: &str) -> Option<EngineError> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim_end_matches(['.', ' ']);
    let ext = base.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext {
        Some(e) if OPENABLE.contains(&e.as_str()) => None,
        _ => Some(EngineError::Invalid(format!(
            "A link can't open {base}: a link opens only documents (PDF, text, images, Word, Excel, PowerPoint and OpenDocument files without macros), never a program, a script or a file Inkwell can't tell the kind of."
        ))),
    }
}

/// Puts `target` on `a` (and takes the other kind of target off).
/// Returns the name the link has for a file target.
fn put_target(doc: &mut PdfDocument, doc_path: &str, a: &mut PdfObject, target: &NewTarget) -> Result<Option<String>> {
    match target {
        NewTarget::Page { page, at } => {
            let n = doc.page_count()?;
            if *page as i32 >= n {
                return Err(EngineError::BadPage(*page));
            }
            let dest = crate::outline::dest_object(doc, *page, *at)?;
            a.dict_delete("A")?;
            a.dict_put("Dest", dest)?;
            Ok(None)
        }
        NewTarget::Uri { uri } => {
            let uri = gate_url(uri)?;
            let mut act = doc.new_dict()?;
            act.dict_put("S", PdfObject::new_name("URI")?)?;
            act.dict_put("URI", PdfObject::new_string(&uri)?)?;
            a.dict_delete("Dest")?;
            a.dict_put("A", act)?;
            Ok(None)
        }
        NewTarget::File { file, page } => {
            let name = file_link_name(doc_path, file)?;
            let is_pdf = file.to_ascii_lowercase().ends_with(".pdf");
            if !is_pdf {
                if let Some(refusal) = runnable(&name) {
                    return Err(refusal);
                }
            }
            let mut spec = doc.new_dict()?;
            spec.dict_put("Type", PdfObject::new_name("Filespec")?)?;
            spec.dict_put("F", byte_string(doc, &name)?)?;
            spec.dict_put("UF", text_object(doc, &name)?)?;
            let mut act = doc.new_dict()?;
            if is_pdf {
                act.dict_put("S", PdfObject::new_name("GoToR")?)?;
                let mut d = doc.new_array()?;
                d.array_push(PdfObject::new_int(page.unwrap_or(0) as i32)?)?;
                d.array_push(PdfObject::new_name("Fit")?)?;
                act.dict_put("D", d)?;
                act.dict_put("NewWindow", PdfObject::new_bool(true))?;
            } else {
                act.dict_put("S", PdfObject::new_name("Launch")?)?;
            }
            act.dict_put("F", spec)?;
            a.dict_delete("Dest")?;
            a.dict_put("A", act)?;
            Ok(Some(name))
        }
    }
}

/// A new link annotation on `page`; returns its reference.
fn new_link(
    doc: &mut PdfDocument,
    doc_path: &str,
    page: u32,
    rect: [f32; 4],
    target: &NewTarget,
    border: &BorderSpec,
) -> Result<(PdfObject, Option<String>)> {
    let r = user_rect(doc, page, rect)?;
    let mut a = doc.new_dict()?;
    a.dict_put("Type", PdfObject::new_name("Annot")?)?;
    a.dict_put("Subtype", PdfObject::new_name("Link")?)?;
    a.dict_put("Rect", real_array(doc, &r)?)?;
    a.dict_put("F", PdfObject::new_int(4)?)?;
    put_border(doc, &mut a, border)?;
    sync_ap(doc, &mut a, false)?;
    let name = put_target(doc, doc_path, &mut a, target)?;
    let mut p = doc.find_page(page as i32)?;
    a.dict_put("P", p.clone())?;
    let r = doc.add_object(&a)?;
    match p.get_dict("Annots")? {
        Some(arr) if arr.resolve()?.is_some_and(|x| x.is_array().unwrap_or(false)) => {
            let mut arr = arr;
            arr.array_push(r.clone())?;
        }
        _ => {
            let mut arr = doc.new_array()?;
            arr.array_push(r.clone())?;
            p.dict_put("Annots", arr)?;
        }
    }
    Ok((r, name))
}

fn link_obj(doc: &PdfDocument, page: u32, id: i32) -> Result<PdfObject> {
    page_annots(doc, page)?
        .into_iter()
        .find(|a| a.as_indirect().ok() == Some(id))
        .ok_or(EngineError::Stale)
}

/// A link that was in the structure tree (`/StructParent`) leaves its entry in the parent tree, and the
/// object reference in the element that held it, when it is deleted; a dangling one is what a checker
/// reports as a broken tag.
fn leave_structure(doc: &PdfDocument, a: &PdfObject, link: i32) -> Result<()> {
    let Some(key) = a.get_dict("StructParent")?.and_then(|k| k.resolve().ok().flatten().unwrap_or(k).as_int().ok()) else {
        return Ok(());
    };
    let Some(root) = doc.catalog()?.get_dict("StructTreeRoot")? else { return Ok(()) };
    let Some(tree) = root.get_dict("ParentTree")? else { return Ok(()) };
    let mut todo = vec![tree];
    let mut budget = 4096;
    while let Some(node) = todo.pop() {
        budget -= 1;
        if budget == 0 {
            break;
        }
        if let Some(nums) = node.get_dict("Nums")? {
            let mut nums = nums.resolve()?.unwrap_or(nums);
            if nums.is_array()? {
                let len = nums.len()? as i32;
                let mut i = 0;
                while i + 1 < len {
                    let k = nums.get_array(i)?.and_then(|k| k.resolve().ok().flatten().unwrap_or(k).as_int().ok());
                    if k == Some(key) {
                        if let Some(elem) = nums.get_array(i + 1)? {
                            drop_object_ref(&elem, link)?;
                        }
                        nums.array_delete(i + 1)?;
                        nums.array_delete(i)?;
                        return Ok(());
                    }
                    i += 2;
                }
            }
        }
        if let Some(kids) = node.get_dict("Kids")? {
            let kids = kids.resolve()?.unwrap_or(kids);
            if kids.is_array()? {
                for j in 0..kids.len()?.min(256) as i32 {
                    if let Some(k) = kids.get_array(j)? {
                        todo.push(k);
                    }
                }
            }
        }
    }
    Ok(())
}

/// Takes the object reference (`/Type /OBJR`) to the annotation `link` out of the content of `elem`.
fn drop_object_ref(elem: &PdfObject, link: i32) -> Result<()> {
    let is_ref = |e: &PdfObject| -> bool {
        e.is_dict().unwrap_or(false)
            && e.get_dict("Obj").ok().flatten().is_some_and(|o| o.is_indirect().unwrap_or(false) && o.as_indirect().ok() == Some(link))
    };
    let mut elem = elem.clone();
    let Some(k) = elem.get_dict("K")? else { return Ok(()) };
    let kr = k.resolve()?.unwrap_or_else(|| k.clone());
    if kr.is_array()? {
        let mut arr = kr;
        for i in (0..arr.len()? as i32).rev() {
            if arr.get_array(i)?.is_some_and(|e| is_ref(&e)) {
                arr.array_delete(i)?;
            }
        }
    } else if is_ref(&k) {
        elem.dict_delete("K")?;
    }
    Ok(())
}

fn remove_link(doc: &mut PdfDocument, page: u32, id: i32) -> Result<()> {
    let p = doc.find_page(page as i32)?;
    if let Some(arr) = p.get_dict("Annots")? {
        let arr = arr.resolve()?.unwrap_or(arr);
        let mut arr = arr;
        let len = arr.len()? as i32;
        for i in (0..len).rev() {
            if let Some(e) = arr.get_array(i)? {
                if e.is_indirect()? && e.as_indirect()? == id {
                    arr.array_delete(i)?;
                }
            }
        }
    }
    doc.delete_object(id)?;
    Ok(())
}

const DEFAULT_BORDER: BorderSpec = BorderSpec { visible: false, width: None, color: None };

fn label(op: &LinkOp) -> String {
    match op {
        LinkOp::Add { page, .. } => format!("Add link on p. {}", page + 1),
        LinkOp::SetTarget { page, .. } => format!("Change link target on p. {}", page + 1),
        LinkOp::SetRect { page, .. } => format!("Move link on p. {}", page + 1),
        LinkOp::SetBorder { page, .. } => format!("Change link border on p. {}", page + 1),
        LinkOp::Delete { page, ids } if ids.len() == 1 => format!("Delete link on p. {}", page + 1),
        LinkOp::Delete { page, ids } => format!("Delete {} links on p. {}", ids.len(), page + 1),
        LinkOp::FromUrls => "Create links from web addresses".into(),
    }
}

/// What a change did: the new link, how many were made or removed, the pages touched, the name of a file.
struct Done {
    id: Option<i32>,
    count: u32,
    pages: Vec<u32>,
    name: Option<String>,
}

impl Done {
    fn on(page: u32) -> Done {
        Done { id: None, count: 0, pages: vec![page], name: None }
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

/// Runs `op`; `None` when it would leave the document as it is (nothing was touched then).
fn run(doc: &mut PdfDocument, doc_path: &str, op: &LinkOp) -> Result<Option<Done>> {
    // Links are annotations: a certification that does not allow annotations (`/P` 1 or 2) is voided by them.
    if let Some(p) = certified(doc)? {
        if p <= 2 {
            return Err(EngineError::Invalid(
                "This document is certified, and certification does not allow changes to its links. Inkwell leaves them as they are.".into(),
            ));
        }
    }
    let guard = |doc: &PdfDocument, page: u32| -> Result<()> {
        if page as i32 >= doc.page_count()? { Err(EngineError::BadPage(page)) } else { Ok(()) }
    };
    match op {
        LinkOp::Add { page, rect, target, border } => {
            guard(doc, *page)?;
            let b = border.clone().unwrap_or(DEFAULT_BORDER);
            let (r, name) = new_link(doc, doc_path, *page, *rect, target, &b)?;
            Ok(Some(Done { id: Some(r.as_indirect()?), count: 1, pages: vec![*page], name }))
        }
        LinkOp::SetTarget { page, id, target } => {
            guard(doc, *page)?;
            let mut a = link_obj(doc, *page, *id)?;
            if same_target(doc, doc_path, &a, target)? {
                return Ok(None);
            }
            let name = put_target(doc, doc_path, &mut a, target)?;
            Ok(Some(Done { name, ..Done::on(*page) }))
        }
        LinkOp::SetRect { page, id, rect } => {
            guard(doc, *page)?;
            let mut a = link_obj(doc, *page, *id)?;
            let r = user_rect(doc, *page, *rect)?;
            if a.get_dict("Rect")?.and_then(|c| nums(&c, 4)).is_some_and(|c| (0..4).all(|i| near(c[i], r[i]))) {
                return Ok(None);
            }
            a.dict_put("Rect", real_array(doc, &r)?)?;
            sync_ap(doc, &mut a, false)?;
            Ok(Some(Done::on(*page)))
        }
        LinkOp::SetBorder { page, id, border } => {
            guard(doc, *page)?;
            let mut a = link_obj(doc, *page, *id)?;
            let now = border_of(&a);
            let same = if border.visible {
                let w = border.width.unwrap_or(1.0).clamp(0.25, 12.0);
                let c = border.color.unwrap_or(INK).map(|v| v.clamp(0.0, 1.0));
                now.visible && near(now.width, w) && (0..3).all(|i| (now.color[i] - c[i]).abs() < 0.002)
            } else {
                !now.visible
            };
            if same {
                return Ok(None);
            }
            put_border(doc, &mut a, border)?;
            sync_ap(doc, &mut a, true)?;
            Ok(Some(Done::on(*page)))
        }
        LinkOp::Delete { page, ids } => {
            guard(doc, *page)?;
            let mut ids = ids.clone();
            ids.sort_unstable();
            ids.dedup();
            if ids.is_empty() {
                return Ok(None);
            }
            // All of them are checked before the first goes.
            let found: Vec<PdfObject> = ids.iter().map(|id| link_obj(doc, *page, *id)).collect::<Result<_>>()?;
            for (id, a) in ids.iter().zip(&found) {
                leave_structure(doc, a, *id)?;
                remove_link(doc, *page, *id)?;
            }
            Ok(Some(Done { count: ids.len() as u32, ..Done::on(*page) }))
        }
        LinkOp::FromUrls => {
            let hits = urls::find(doc)?;
            if hits.is_empty() {
                return Err(EngineError::Invalid("no web addresses without a link were found".into()));
            }
            let mut pages = Vec::new();
            let mut n = 0;
            for h in &hits {
                let t = NewTarget::Uri { uri: h.url.clone() };
                for r in &h.rects {
                    new_link(doc, doc_path, h.page, *r, &t, &DEFAULT_BORDER)?;
                    n += 1;
                }
                if !pages.contains(&h.page) {
                    pages.push(h.page);
                }
            }
            Ok(Some(Done { id: None, count: n, pages, name: None }))
        }
    }
}

/// `target` is what the link `a` already goes to.
fn same_target(doc: &PdfDocument, doc_path: &str, a: &PdfObject, target: &NewTarget) -> Result<bool> {
    let now = target_of(doc, a, &mut HashMap::new(), &mut Names::default());
    Ok(match (target, now) {
        (NewTarget::Page { page, at }, Target::Page { page: p, at: q }) => {
            *page == p
                && match (at, q) {
                    (None, None) => true,
                    (Some(x), Some(y)) => near(x[0], y[0]) && near(x[1], y[1]),
                    _ => false,
                }
        }
        (NewTarget::Uri { uri }, Target::Uri { uri: u }) => gate_url(uri).is_ok_and(|g| g == u),
        (NewTarget::File { file, page }, Target::File { name, page: p }) => {
            let wanted = file_link_name(doc_path, file)?;
            wanted == name && (!file.to_ascii_lowercase().ends_with(".pdf") || page.unwrap_or(0) == p.unwrap_or(0))
        }
        _ => false,
    })
}

/// Applies `op`, listed at generation `rev`: one undo step, or none when it changes nothing.
/// `Stale` when the document moved on.
pub fn apply(eng: &Engine, doc: DocId, rev: u32, op: &LinkOp, source: EditSource) -> Result<LinkEdited> {
    let entry = eng.doc_entry(doc)?;
    let path = eng.edit_state(doc)?.path;
    let mut out = (None, 0, None);
    // A removal makes the next Save a full rewrite, which would void a signature (a /P 3 certified file
    // allows deleting links): a signed document's save stays incremental.
    let removes = matches!(op, LinkOp::Delete { .. }) && !eng.read_custom(doc, |d| Ok(crate::outline::signed(d)))?;
    let state = eng.apply_custom_unless_noop(doc, label(op), source, removes, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        Ok(run(d, &path, op)?.map(|done| {
            out = (done.id, done.count, done.name);
            done.pages
        }))
    })?;
    Ok(LinkEdited { state, id: out.0, count: out.1, name: out.2 })
}

/// What [`LinkOp::FromUrls`] would make, changing nothing.
pub fn preview_urls(eng: &Engine, doc: DocId) -> Result<UrlPreview> {
    eng.read_custom(doc, |d| urls::find(d).map(|h| urls::preview(&h)))
}
