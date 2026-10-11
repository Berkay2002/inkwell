//! Bookmarks (the outline): list, add, rename, delete, move and nest, set the destination, open and
//! closed, bold / italic / colour, and bookmarks made from the document's headings.
//!
//! Every change is one `Engine::apply_custom` step on the authoritative document: one undo step,
//! kept by Save, recorded in history. An item is named by its object number, which a snapshot
//! (undo, redo) keeps. Items are changed in place (title, `/Dest`, `/C`, `/F`, `/Count`) and
//! relinked (`/First /Last /Prev /Next /Parent`), so the actions this editor does not make (a link
//! to another file, a named destination, JavaScript) are kept as they were. Coordinates in and out
//! are the viewer's page space (points, origin top left, `/Rotate` applied). See docs/bookmarks-links.md.

pub mod app;
mod headings;
#[cfg(test)]
pub(crate) mod tests;

pub use headings::{HeadingPreview, HeadingRow};

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::textedit::{Matrix, page_space};
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

type Result<T> = std::result::Result<T, EngineError>;

/// More items than this are not read (a damaged or hostile outline); deeper nesting is cut off.
const MAX_ITEMS: usize = 50_000;
const MAX_DEPTH: usize = 48;
/// A title is cut at this many characters.
pub const MAX_TITLE: usize = 500;

/// Where a bookmark goes.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Dest {
    /// Nowhere (an item with no destination, as a heading of a group).
    None,
    /// A page (0-based) and, when the destination names one, the point at the top left of the view
    /// in page space.
    Page { page: u32, at: Option<[f32; 2]> },
    Uri { uri: String },
    /// Another kind of action (a file, JavaScript, a named action), kept as it is.
    Other { what: String },
    /// A destination that can't be followed: a named destination that is not in the document, or a
    /// page that is. `what` says which, as a short phrase.
    Broken { what: String },
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    /// The item's object number.
    pub id: i32,
    pub title: String,
    pub dest: Dest,
    /// Shown expanded (`/Count` positive).
    pub open: bool,
    pub bold: bool,
    pub italic: bool,
    /// RGB 0..1 when the item has its own colour.
    pub color: Option<[f32; 3]>,
    pub children: Vec<Bookmark>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bookmarks {
    /// The generation the tree belongs to; pass it back with a change.
    pub rev: u32,
    pub items: Vec<Bookmark>,
    /// Items in all.
    pub count: u32,
    /// The outline had more items than are read here (the rest is left as it is, and can't be edited).
    pub truncated: bool,
}

/// One change to the outline.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum BookmarkOp {
    /// A new bookmark under `parent` (none: at the top level) at position `index` among its
    /// siblings (none: last). `at`: the point shown at the top left, page space.
    #[serde(rename_all = "camelCase")]
    Add { parent: Option<i32>, index: Option<u32>, title: String, page: u32, at: Option<[f32; 2]> },
    Rename { id: i32, title: String },
    /// The items and everything under them.
    Delete { ids: Vec<i32> },
    /// Under `parent` (none: top level) at `index` among the siblings left after it is taken out.
    Move { id: i32, parent: Option<i32>, index: u32 },
    /// Where the bookmark goes: `page` (0-based) at the point `at` (page space), or the top of it.
    SetDest { id: i32, page: u32, at: Option<[f32; 2]> },
    SetOpen { id: i32, open: bool },
    #[serde(rename_all = "camelCase")]
    SetStyle { id: i32, bold: Option<bool>, italic: Option<bool>, color: Option<[f32; 3]>, reset_color: Option<bool> },
    /// Bookmarks from the document's headings (see [`headings`]). `replace` drops the bookmarks
    /// there are first; otherwise the new ones come after them.
    FromHeadings { replace: bool },
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkEdited {
    pub state: EditState,
    /// The new bookmark (Add), or the first of the bookmarks made (FromHeadings).
    pub id: Option<i32>,
    /// Bookmarks added (Add, FromHeadings) or removed (Delete).
    pub count: u32,
}

// ---- strings ----

const PDFDOC_80: [char; 32] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}', '\u{2039}', '\u{203A}',
    '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}', '\u{201D}', '\u{2018}', '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}',
    '\u{FB02}', '\u{0141}', '\u{0152}', '\u{0160}', '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}',
    '\u{017E}', '\u{FFFD}',
];
const PDFDOC_18: [char; 8] = ['\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}', '\u{02DC}'];

/// A PDF text string (UTF-16 with a byte order mark, UTF-8 with one, or PDFDocEncoding) as text.
pub fn decode_text(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]));
        return char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect();
    }
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        let units = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]));
        return char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect();
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    b.iter()
        .map(|&c| match c {
            0x18..=0x1F => PDFDOC_18[(c - 0x18) as usize],
            0x80..=0x9F => PDFDOC_80[(c - 0x80) as usize],
            0xA0 => '\u{20AC}',
            c => char::from(c),
        })
        .collect()
}

/// `text` as a PDF string object: a plain string when it is ASCII, else UTF-16BE with a byte order mark.
pub fn text_object(doc: &PdfDocument, text: &str) -> Result<PdfObject> {
    let clean: String = text.chars().filter(|c| !c.is_control() || *c == '\t').take(MAX_TITLE).collect();
    if clean.chars().all(|c| c.is_ascii() && c != '\0') {
        return Ok(PdfObject::new_string(&clean)?);
    }
    let mut hex = String::from("<FEFF");
    for u in clean.encode_utf16() {
        hex.push_str(&format!("{u:04X}"));
    }
    hex.push('>');
    Ok(doc.new_object_from_str(&hex)?)
}

fn string_of(o: Option<PdfObject>) -> String {
    let Some(o) = o else { return String::new() };
    let o = o.resolve().ok().flatten().unwrap_or(o);
    if o.is_string().unwrap_or(false) {
        decode_text(&o.as_bytes().unwrap_or_default())
    } else if o.is_name().unwrap_or(false) {
        String::from_utf8_lossy(&o.as_name().unwrap_or_default()).into_owned()
    } else {
        String::new()
    }
}

fn name_of(o: Option<PdfObject>) -> String {
    o.and_then(|s| s.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned()).unwrap_or_default()
}

fn number(o: &Option<PdfObject>) -> Option<f32> {
    let o = o.as_ref()?;
    let o = o.resolve().ok().flatten().unwrap_or_else(|| o.clone());
    o.is_number().unwrap_or(false).then(|| o.as_float().ok()).flatten()
}

// ---- destinations ----

/// The explicit destination array behind `target` (itself, a name or string looked up in the
/// document's names, or a `<< /D … >>` dictionary), when it can be found.
pub(crate) fn dest_array(doc: &PdfDocument, target: &PdfObject) -> Option<PdfObject> {
    let mut t = target.resolve().ok().flatten();
    for _ in 0..4 {
        let cur = t?;
        if cur.is_array().ok()? {
            return Some(cur);
        }
        t = if cur.is_dict().ok()? {
            cur.get_dict("D").ok().flatten()
        } else if cur.is_name().ok()? {
            let name = String::from_utf8_lossy(&cur.as_name().ok()?).into_owned();
            let dests = doc.catalog().ok()?.get_dict("Dests").ok().flatten();
            dests.and_then(|d| d.get_dict(name.as_str()).ok().flatten())
        } else if cur.is_string().ok()? {
            // MuPDF flattens /Root /Names /Dests into a dict keyed by name.
            let tree = doc.load_name_tree(PdfObject::new_name("Dests").ok()?).ok()?;
            if tree.is_dict().ok()? { tree.get_dict(cur.as_string().ok()?.as_str()).ok().flatten() } else { None }
        } else {
            None
        };
        t = t.and_then(|v| v.resolve().ok().flatten());
    }
    None
}

/// A destination array's page (0-based) and the point it shows at the top left, in page space.
pub(crate) fn page_dest(doc: &PdfDocument, spaces: &mut HashMap<u32, Option<Matrix>>, arr: &PdfObject) -> Option<(u32, Option<[f32; 2]>)> {
    let page_ref = arr.get_array(0).ok().flatten()?;
    let n = doc.lookup_page_number(&page_ref).ok()?;
    if n < 0 {
        return None;
    }
    let page = n as u32;
    let get = |i: i32| arr.get_array(i).ok().flatten();
    let kind = name_of(get(1));
    let (left, top) = match kind.as_str() {
        "XYZ" => (number(&get(2)), number(&get(3))),
        "FitH" | "FitBH" => (None, number(&get(2))),
        "FitR" => (number(&get(2)), number(&get(5))),
        "FitV" | "FitBV" => (number(&get(2)), None),
        _ => (None, None),
    };
    let m = spaces.entry(page).or_insert_with(|| page_space(doc, page).ok()).clone();
    let at = match (m, top) {
        (Some(m), Some(top)) => {
            let (x, y) = m.apply(f64::from(left.unwrap_or(0.0)), f64::from(top));
            Some([x as f32, y as f32])
        }
        _ => None,
    };
    Some((page, at))
}

fn dest_of(doc: &PdfDocument, item: &PdfObject, spaces: &mut HashMap<u32, Option<Matrix>>) -> Dest {
    let mut target = item.get_dict("Dest").ok().flatten();
    if target.is_none() {
        if let Some(a) = item.get_dict("A").ok().flatten() {
            match name_of(a.get_dict("S").ok().flatten()).as_str() {
                "GoTo" => target = a.get_dict("D").ok().flatten(),
                "URI" => return Dest::Uri { uri: string_of(a.get_dict("URI").ok().flatten()) },
                other => return Dest::Other { what: other.to_string() },
            }
        }
    }
    let Some(t) = target else { return Dest::None };
    match dest_array(doc, &t).and_then(|arr| page_dest(doc, spaces, &arr)) {
        Some((page, at)) => Dest::Page { page, at },
        None => Dest::Broken { what: broken_reason(&t) },
    }
}

/// Why a destination can't be followed, as a short phrase.
pub(crate) fn broken_reason(t: &PdfObject) -> String {
    let t = t.resolve().ok().flatten().unwrap_or_else(|| t.clone());
    if t.is_name().unwrap_or(false) || t.is_string().unwrap_or(false) {
        let n: String = string_of(Some(t)).chars().take(60).collect();
        format!("the named destination \"{n}\" is not in the document")
    } else if t.is_array().unwrap_or(false) {
        "its page is not in the document".into()
    } else {
        "the destination is not valid".into()
    }
}

/// `/Dest [page /XYZ x y null]`: `at` is page space; none shows the top left of the page.
pub(crate) fn dest_object(doc: &PdfDocument, page: u32, at: Option<[f32; 2]>) -> Result<PdfObject> {
    let n = doc.page_count()?;
    if page as i32 >= n {
        return Err(EngineError::BadPage(page));
    }
    let page_obj = doc.find_page(page as i32)?;
    let mut arr = doc.new_array()?;
    arr.array_push(page_obj)?;
    arr.array_push(PdfObject::new_name("XYZ")?)?;
    match at {
        Some([x, y]) => {
            let back = page_space(doc, page)?.invert().ok_or_else(|| EngineError::Invalid("the page has no area".into()))?;
            let (ux, uy) = back.apply(f64::from(x), f64::from(y));
            arr.array_push(PdfObject::new_real(ux as f32)?)?;
            arr.array_push(PdfObject::new_real(uy as f32)?)?;
        }
        None => {
            arr.array_push(PdfObject::new_null())?;
            arr.array_push(PdfObject::new_null())?;
        }
    }
    arr.array_push(PdfObject::new_null())?;
    Ok(arr)
}

// ---- reading ----

fn root(doc: &PdfDocument) -> Result<Option<PdfObject>> {
    Ok(doc.catalog()?.get_dict("Outlines")?.filter(|o| o.resolve().ok().flatten().is_some_and(|d| d.is_dict().unwrap_or(false))))
}

/// The items directly under `parent`, in order (a cycle or a runaway chain is cut).
fn kids(parent: &PdfObject) -> Result<Vec<PdfObject>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = parent.get_dict("First")?;
    while let Some(item) = cur {
        if !item.is_indirect()? {
            break;
        }
        let num = item.as_indirect()?;
        if !seen.insert(num) || out.len() >= MAX_ITEMS {
            break;
        }
        cur = item.get_dict("Next")?;
        out.push(item);
    }
    Ok(out)
}

fn is_open(item: &PdfObject) -> bool {
    number(&item.get_dict("Count").ok().flatten()).is_some_and(|c| c > 0.0)
}

struct Reader<'a> {
    doc: &'a PdfDocument,
    spaces: HashMap<u32, Option<Matrix>>,
    seen: HashSet<i32>,
    truncated: bool,
}

impl Reader<'_> {
    fn level(&mut self, parent: &PdfObject, depth: usize) -> Result<Vec<Bookmark>> {
        let mut out = Vec::new();
        for item in kids(parent)? {
            let id = item.as_indirect()?;
            if self.seen.len() >= MAX_ITEMS || depth >= MAX_DEPTH {
                self.truncated = true;
                break;
            }
            if !self.seen.insert(id) {
                continue;
            }
            let color = item.get_dict("C")?.and_then(|c| {
                let v: Vec<f32> = (0..3).filter_map(|i| number(&c.get_array(i).ok().flatten())).collect();
                (v.len() == 3).then(|| [v[0].clamp(0.0, 1.0), v[1].clamp(0.0, 1.0), v[2].clamp(0.0, 1.0)])
            });
            let flags = number(&item.get_dict("F")?).map_or(0, |f| f as i32);
            let children = self.level(&item, depth + 1)?;
            out.push(Bookmark {
                id,
                title: string_of(item.get_dict("Title")?),
                dest: dest_of(self.doc, &item, &mut self.spaces),
                open: is_open(&item) && !children.is_empty(),
                bold: flags & 2 != 0,
                italic: flags & 1 != 0,
                color,
                children,
            });
        }
        Ok(out)
    }
}

fn count_items(items: &[Bookmark]) -> u32 {
    items.iter().map(|b| 1 + count_items(&b.children)).sum()
}

/// The outline as the document is now.
pub fn read(doc: &PdfDocument, rev: u32) -> Result<Bookmarks> {
    let Some(root) = root(doc)? else {
        return Ok(Bookmarks { rev, items: Vec::new(), count: 0, truncated: false });
    };
    let mut r = Reader { doc, spaces: HashMap::new(), seen: HashSet::new(), truncated: false };
    let items = r.level(&root, 0)?;
    Ok(Bookmarks { rev, count: count_items(&items), items, truncated: r.truncated })
}

pub fn list(eng: &Engine, doc: DocId) -> Result<Bookmarks> {
    let entry = eng.doc_entry(doc)?;
    eng.read_custom(doc, |d| read(d, entry.generation()))
}

// ---- writing ----

/// Makes `kids` the children of `parent` in that order.
fn relink(parent: &mut PdfObject, kids: &[PdfObject]) -> Result<()> {
    let n = kids.len();
    for (i, k) in kids.iter().enumerate() {
        let mut k = k.clone();
        k.dict_put("Parent", parent.clone())?;
        if i > 0 {
            k.dict_put("Prev", kids[i - 1].clone())?;
        } else {
            k.dict_delete("Prev")?;
        }
        if i + 1 < n {
            k.dict_put("Next", kids[i + 1].clone())?;
        } else {
            k.dict_delete("Next")?;
        }
    }
    if let (Some(first), Some(last)) = (kids.first(), kids.last()) {
        parent.dict_put("First", first.clone())?;
        parent.dict_put("Last", last.clone())?;
    } else {
        parent.dict_delete("First")?;
        parent.dict_delete("Last")?;
    }
    Ok(())
}

/// Sets `/Count` on `item` and everything under it (its sign says open or closed, its size the
/// items shown below it when it is open); returns that size.
fn recount(item: &mut PdfObject, is_root: bool, depth: usize) -> Result<i32> {
    let mut visible = 0;
    if depth < MAX_DEPTH {
        for mut k in kids(item)? {
            let open = is_open(&k);
            let below = recount(&mut k, false, depth + 1)?;
            visible += 1 + if open { below } else { 0 };
        }
    }
    if visible == 0 {
        item.dict_delete("Count")?;
    } else if is_root || is_open(item) {
        item.dict_put("Count", PdfObject::new_int(visible)?)?;
    } else {
        item.dict_put("Count", PdfObject::new_int(-visible)?)?;
    }
    Ok(visible)
}

/// `/Outlines`, made when the document has none.
fn root_or_new(doc: &mut PdfDocument) -> Result<PdfObject> {
    if let Some(r) = root(doc)? {
        return Ok(r);
    }
    let dict = doc.new_object_from_str("<< /Type /Outlines >>")?;
    let r = doc.add_object(&dict)?;
    doc.catalog()?.dict_put("Outlines", r.clone())?;
    Ok(r)
}

fn item(doc: &PdfDocument, id: i32) -> Result<PdfObject> {
    let r = doc.new_indirect(id, 0)?;
    match r.resolve() {
        Ok(Some(d)) if d.is_dict()? && d.get_dict("Parent")?.is_some() && d.get_dict("Title")?.is_some() => Ok(r),
        _ => Err(EngineError::Stale),
    }
}

fn parent_of(doc: &PdfDocument, id: Option<i32>) -> Result<PdfObject> {
    match id {
        Some(p) => item(doc, p),
        None => root(doc)?.ok_or(EngineError::Stale),
    }
}

/// `x` is `anc` or lies under it.
fn under(x: &PdfObject, anc: i32) -> bool {
    let mut cur = Some(x.clone());
    for _ in 0..=MAX_DEPTH * 4 {
        let Some(c) = cur else { return false };
        if c.is_indirect().unwrap_or(false) && c.as_indirect().unwrap_or(-1) == anc {
            return true;
        }
        cur = c.get_dict("Parent").ok().flatten();
    }
    false
}

fn delete_tree(doc: &mut PdfDocument, it: &PdfObject, depth: usize) -> Result<u32> {
    let mut n = 1;
    if depth < MAX_DEPTH {
        for k in kids(it)? {
            n += delete_tree(doc, &k, depth + 1)?;
        }
    }
    doc.delete_object(it.as_indirect()?)?;
    Ok(n)
}

fn label(op: &BookmarkOp) -> String {
    match op {
        BookmarkOp::Add { title, .. } => format!("Add bookmark \"{}\"", short(&clean_title(title))),
        BookmarkOp::Rename { title, .. } => format!("Rename bookmark to \"{}\"", short(&clean_title(title))),
        BookmarkOp::Delete { ids } if ids.len() == 1 => "Delete bookmark".into(),
        BookmarkOp::Delete { ids } => format!("Delete {} bookmarks", ids.len()),
        BookmarkOp::Move { .. } => "Move bookmark".into(),
        BookmarkOp::SetDest { .. } => "Set bookmark destination".into(),
        BookmarkOp::SetOpen { open: true, .. } => "Expand bookmark".into(),
        BookmarkOp::SetOpen { open: false, .. } => "Collapse bookmark".into(),
        BookmarkOp::SetStyle { .. } => "Format bookmark".into(),
        BookmarkOp::FromHeadings { .. } => "Bookmarks from headings".into(),
    }
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(40).collect();
    if s.chars().count() > 40 { format!("{t}…") } else { t }
}

fn clean_title(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Puts `new` under `parent` at `index` (none: last). Returns the new item.
fn insert_new(doc: &mut PdfDocument, parent: &mut PdfObject, index: Option<usize>, title: &str, dest: Option<PdfObject>) -> Result<PdfObject> {
    let mut dict = doc.new_dict()?;
    dict.dict_put("Title", text_object(doc, title)?)?;
    dict.dict_put("Parent", parent.clone())?;
    if let Some(d) = dest {
        dict.dict_put("Dest", d)?;
    }
    let new = doc.add_object(&dict)?;
    let mut ks = kids(parent)?;
    let at = index.map_or(ks.len(), |i| i.min(ks.len()));
    ks.insert(at, new.clone());
    relink(parent, &ks)?;
    Ok(new)
}

/// Runs `op` on `doc`; returns the id and count for [`BookmarkEdited`].
pub(crate) fn run(doc: &mut PdfDocument, op: &BookmarkOp) -> Result<(Option<i32>, u32)> {
    let mut made = None;
    let mut count = 0;
    match op {
        BookmarkOp::Add { parent, index, title, page, at } => {
            let title = clean_title(title);
            if title.is_empty() {
                return Err(EngineError::Invalid("a bookmark needs a name".into()));
            }
            let dest = dest_object(doc, *page, *at)?;
            let mut root = root_or_new(doc)?;
            let mut p = match parent {
                Some(id) => item(doc, *id)?,
                None => root.clone(),
            };
            let new = insert_new(doc, &mut p, index.map(|i| i as usize), &title, Some(dest))?;
            if parent.is_some() {
                // A group with a new member is shown open, so the member is seen.
                p.dict_put("Count", PdfObject::new_int(1)?)?;
            }
            made = Some(new.as_indirect()?);
            count = 1;
            recount(&mut root, true, 0)?;
        }
        BookmarkOp::Rename { id, title } => {
            let title = clean_title(title);
            if title.is_empty() {
                return Err(EngineError::Invalid("a bookmark needs a name".into()));
            }
            let mut it = item(doc, *id)?;
            it.dict_put("Title", text_object(doc, &title)?)?;
        }
        BookmarkOp::Delete { ids } => {
            let mut root = root(doc)?.ok_or(EngineError::Stale)?;
            // Nested picks go with their ancestor: take the topmost ones only.
            let items: Vec<PdfObject> = ids.iter().map(|id| item(doc, *id)).collect::<Result<_>>()?;
            // Decided before anything is deleted: a deleted item has no `/Parent` left to ask.
            let mut nested = Vec::with_capacity(items.len());
            for it in &items {
                let mut gone = false;
                if let Some(p) = it.get_dict("Parent")? {
                    gone = ids.iter().any(|a| under(&p, *a));
                }
                nested.push(gone);
            }
            for (it, gone) in items.iter().zip(nested) {
                if gone {
                    continue;
                }
                let mut parent = it.get_dict("Parent")?.ok_or(EngineError::Stale)?;
                let num = it.as_indirect()?;
                let rest: Vec<PdfObject> = kids(&parent)?.into_iter().filter(|k| k.as_indirect().ok() != Some(num)).collect();
                relink(&mut parent, &rest)?;
                count += delete_tree(doc, it, 0)?;
            }
            recount(&mut root, true, 0)?;
        }
        BookmarkOp::Move { id, parent, index } => {
            let mut root = root(doc)?.ok_or(EngineError::Stale)?;
            let it = item(doc, *id)?;
            let mut to = parent_of(doc, *parent)?;
            if let Some(p) = parent {
                if under(&to, *id) || p == id {
                    return Err(EngineError::Invalid("a bookmark can't go inside itself".into()));
                }
            }
            let mut from = it.get_dict("Parent")?.ok_or(EngineError::Stale)?;
            let num = *id;
            let rest: Vec<PdfObject> = kids(&from)?.into_iter().filter(|k| k.as_indirect().ok() != Some(num)).collect();
            relink(&mut from, &rest)?;
            let mut ks = kids(&to)?;
            let at = (*index as usize).min(ks.len());
            ks.insert(at, it);
            relink(&mut to, &ks)?;
            if parent.is_some() {
                to.dict_put("Count", PdfObject::new_int(1)?)?;
            }
            recount(&mut root, true, 0)?;
        }
        BookmarkOp::SetDest { id, page, at } => {
            let mut it = item(doc, *id)?;
            let dest = dest_object(doc, *page, *at)?;
            it.dict_delete("A")?;
            it.dict_put("Dest", dest)?;
        }
        BookmarkOp::SetOpen { id, open } => {
            let mut it = item(doc, *id)?;
            let mut root = root(doc)?.ok_or(EngineError::Stale)?;
            if kids(&it)?.is_empty() {
                return Err(EngineError::Invalid("this bookmark has nothing under it".into()));
            }
            it.dict_put("Count", PdfObject::new_int(if *open { 1 } else { -1 })?)?;
            recount(&mut root, true, 0)?;
        }
        BookmarkOp::SetStyle { id, bold, italic, color, reset_color } => {
            let mut it = item(doc, *id)?;
            let cur = number(&it.get_dict("F")?).map_or(0, |f| f as i32);
            let mut flags = cur;
            if let Some(b) = bold {
                flags = if *b { flags | 2 } else { flags & !2 };
            }
            if let Some(i) = italic {
                flags = if *i { flags | 1 } else { flags & !1 };
            }
            if flags != cur || number(&it.get_dict("F")?).is_none() {
                if flags == 0 {
                    it.dict_delete("F")?;
                } else {
                    it.dict_put("F", PdfObject::new_int(flags)?)?;
                }
            }
            if reset_color == &Some(true) {
                it.dict_delete("C")?;
            } else if let Some(c) = color {
                let mut arr = doc.new_array()?;
                for v in c {
                    arr.array_push(PdfObject::new_real(v.clamp(0.0, 1.0))?)?;
                }
                it.dict_put("C", arr)?;
            }
        }
        BookmarkOp::FromHeadings { replace } => {
            let found = headings::find(doc)?;
            if found.rows.is_empty() {
                return Err(EngineError::Invalid("no headings were found".into()));
            }
            if *replace {
                if let Some(r) = root(doc)? {
                    for k in kids(&r)? {
                        delete_tree(doc, &k, 0)?;
                    }
                    let mut r = r;
                    relink(&mut r, &[])?;
                }
            }
            let mut root = root_or_new(doc)?;
            // Level → the item last made at it, for nesting.
            let mut stack: Vec<(u8, PdfObject)> = Vec::new();
            for row in &found.rows {
                while stack.last().is_some_and(|(l, _)| *l >= row.level) {
                    stack.pop();
                }
                let mut parent = stack.last().map_or_else(|| root.clone(), |(_, p)| p.clone());
                let dest = dest_object(doc, row.page, Some(row.at))?;
                let new = insert_new(doc, &mut parent, None, &row.title, Some(dest))?;
                if made.is_none() {
                    made = Some(new.as_indirect()?);
                }
                if stack.last().is_some() {
                    parent.dict_put("Count", PdfObject::new_int(1)?)?;
                }
                stack.push((row.level, new));
                count += 1;
            }
            recount(&mut root, true, 0)?;
        }
    }
    Ok((made, count))
}

/// Applies `op`, listed at generation `rev`: one undo step. `Stale` when the document moved on.
pub fn apply(eng: &Engine, doc: DocId, rev: u32, op: &BookmarkOp, source: EditSource) -> Result<BookmarkEdited> {
    let entry = eng.doc_entry(doc)?;
    let mut out = (None, 0);
    let state = eng.apply_custom(doc, label(op), source, false, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        out = run(d, op)?;
        Ok(Vec::new())
    })?;
    Ok(BookmarkEdited { state, id: out.0, count: out.1 })
}

/// What [`BookmarkOp::FromHeadings`] would make, changing nothing.
pub fn preview_headings(eng: &Engine, doc: DocId) -> Result<HeadingPreview> {
    eng.read_custom(doc, |d| headings::find(d).map(|h| h.preview()))
}
