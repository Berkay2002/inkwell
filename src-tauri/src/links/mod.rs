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
use crate::outline::{dest_array, page_dest, text_object};
use crate::textedit::{Matrix, page_space};
use mupdf::pdf::{PdfDocument, PdfObject};
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
        let lower = query.to_ascii_lowercase();
        if lower.split('&').any(|p| p.starts_with("attach") || p.starts_with("insert")) {
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
    for ch in tail.chars() {
        if ch.is_ascii() {
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
    let color = a.get_dict("C").ok().flatten().and_then(|c| nums(&c, 3)).map_or([0.0, 0.0, 0.0], |v| [v[0], v[1], v[2]]);
    Border { visible: width > 0.0, width: width.max(0.0), color }
}

fn target_of(doc: &PdfDocument, a: &PdfObject, spaces: &mut HashMap<u32, Option<Matrix>>) -> Target {
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
    match dest_array(doc, &d).and_then(|arr| page_dest(doc, spaces, &arr)) {
        Some((page, at)) => Target::Page { page, at },
        None => Target::Other { what: "Dest".into() },
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
    let m = page_space(doc, page)?;
    let mut spaces = HashMap::new();
    let mut links = Vec::new();
    for a in page_annots(doc, page)? {
        let Some(r) = a.get_dict("Rect")?.and_then(|r| nums(&r, 4)) else { continue };
        let b = m.bbox([r[0], r[1], r[2], r[3]].map(f64::from));
        let (x0, x1) = (b[0].min(b[2]) as f32, b[0].max(b[2]) as f32);
        let (y0, y1) = (b[1].min(b[3]) as f32, b[1].max(b[3]) as f32);
        links.push(Link { id: a.as_indirect()?, rect: [x0, y0, x1, y1], target: target_of(doc, &a, &mut spaces), border: border_of(&a) });
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

/// A file as a link has it: relative to the document's folder when it is in it or below it, the
/// bare file name otherwise (Inkwell never puts a folder path of this machine into a document).
fn file_name_for(doc_path: &str, file: &str) -> String {
    let f = std::path::Path::new(file);
    let dir = std::path::Path::new(doc_path).parent();
    if let Some(rel) = dir.and_then(|d| f.strip_prefix(d).ok()) {
        let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if !parts.is_empty() {
            return parts.join("/");
        }
    }
    f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Puts `target` on `a` (and takes the other kind of target off).
fn put_target(doc: &mut PdfDocument, doc_path: &str, a: &mut PdfObject, target: &NewTarget) -> Result<()> {
    match target {
        NewTarget::Page { page, at } => {
            let n = doc.page_count()?;
            if *page as i32 >= n {
                return Err(EngineError::BadPage(*page));
            }
            let dest = crate::outline::dest_object(doc, *page, *at)?;
            a.dict_delete("A")?;
            a.dict_put("Dest", dest)?;
        }
        NewTarget::Uri { uri } => {
            let uri = gate_url(uri)?;
            let mut act = doc.new_dict()?;
            act.dict_put("S", PdfObject::new_name("URI")?)?;
            act.dict_put("URI", PdfObject::new_string(&uri)?)?;
            a.dict_delete("Dest")?;
            a.dict_put("A", act)?;
        }
        NewTarget::File { file, page } => {
            let name = file_name_for(doc_path, file);
            if name.is_empty() {
                return Err(EngineError::Invalid("that file has no name".into()));
            }
            let is_pdf = file.to_ascii_lowercase().ends_with(".pdf");
            let mut spec = doc.new_dict()?;
            spec.dict_put("Type", PdfObject::new_name("Filespec")?)?;
            spec.dict_put("F", text_object(doc, &name)?)?;
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
        }
    }
    Ok(())
}

/// A new link annotation on `page`; returns its reference.
fn new_link(doc: &mut PdfDocument, doc_path: &str, page: u32, rect: [f32; 4], target: &NewTarget, border: &BorderSpec) -> Result<PdfObject> {
    let r = user_rect(doc, page, rect)?;
    let mut a = doc.new_dict()?;
    a.dict_put("Type", PdfObject::new_name("Annot")?)?;
    a.dict_put("Subtype", PdfObject::new_name("Link")?)?;
    a.dict_put("Rect", real_array(doc, &r)?)?;
    a.dict_put("F", PdfObject::new_int(4)?)?;
    put_border(doc, &mut a, border)?;
    put_target(doc, doc_path, &mut a, target)?;
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
    Ok(r)
}

fn link_obj(doc: &PdfDocument, page: u32, id: i32) -> Result<PdfObject> {
    page_annots(doc, page)?
        .into_iter()
        .find(|a| a.as_indirect().ok() == Some(id))
        .ok_or(EngineError::Stale)
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

fn run(doc: &mut PdfDocument, doc_path: &str, op: &LinkOp) -> Result<(Option<i32>, u32, Vec<u32>)> {
    let guard = |doc: &PdfDocument, page: u32| -> Result<()> {
        if page as i32 >= doc.page_count()? { Err(EngineError::BadPage(page)) } else { Ok(()) }
    };
    match op {
        LinkOp::Add { page, rect, target, border } => {
            guard(doc, *page)?;
            let b = border.clone().unwrap_or(DEFAULT_BORDER);
            let r = new_link(doc, doc_path, *page, *rect, target, &b)?;
            Ok((Some(r.as_indirect()?), 1, vec![*page]))
        }
        LinkOp::SetTarget { page, id, target } => {
            guard(doc, *page)?;
            let mut a = link_obj(doc, *page, *id)?;
            put_target(doc, doc_path, &mut a, target)?;
            Ok((None, 0, vec![*page]))
        }
        LinkOp::SetRect { page, id, rect } => {
            guard(doc, *page)?;
            let mut a = link_obj(doc, *page, *id)?;
            let r = user_rect(doc, *page, *rect)?;
            a.dict_put("Rect", real_array(doc, &r)?)?;
            Ok((None, 0, vec![*page]))
        }
        LinkOp::SetBorder { page, id, border } => {
            guard(doc, *page)?;
            let mut a = link_obj(doc, *page, *id)?;
            put_border(doc, &mut a, border)?;
            Ok((None, 0, vec![*page]))
        }
        LinkOp::Delete { page, ids } => {
            guard(doc, *page)?;
            let mut n = 0;
            for id in ids {
                link_obj(doc, *page, *id)?;
                remove_link(doc, *page, *id)?;
                n += 1;
            }
            Ok((None, n, vec![*page]))
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
            Ok((None, n, pages))
        }
    }
}

/// Applies `op`, listed at generation `rev`: one undo step. `Stale` when the document moved on.
pub fn apply(eng: &Engine, doc: DocId, rev: u32, op: &LinkOp, source: EditSource) -> Result<LinkEdited> {
    let entry = eng.doc_entry(doc)?;
    let path = eng.edit_state(doc)?.path;
    let mut out = (None, 0);
    let state = eng.apply_custom(doc, label(op), source, false, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        let (id, count, pages) = run(d, &path, op)?;
        out = (id, count);
        Ok(pages)
    })?;
    Ok(LinkEdited { state, id: out.0, count: out.1 })
}

/// What [`LinkOp::FromUrls`] would make, changing nothing.
pub fn preview_urls(eng: &Engine, doc: DocId) -> Result<UrlPreview> {
    eng.read_custom(doc, |d| urls::find(d).map(|h| urls::preview(&h)))
}
