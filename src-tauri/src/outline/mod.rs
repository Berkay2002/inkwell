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
use std::collections::{HashMap, HashSet, VecDeque};

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

/// The document's named destinations, loaded once for as many lookups as a read needs (MuPDF builds
/// the whole table each time it is asked).
#[derive(Default)]
pub(crate) struct Names {
    tree: Option<Option<PdfObject>>,
}

impl Names {
    fn tree(&mut self, doc: &PdfDocument) -> Option<&PdfObject> {
        self.tree
            .get_or_insert_with(|| {
                let t = doc.load_name_tree(PdfObject::new_name("Dests").ok()?).ok()?;
                t.is_dict().ok()?.then_some(t)
            })
            .as_ref()
    }
}

/// The explicit destination array behind `target` (itself, a name or string looked up in the
/// document's names, or a `<< /D … >>` dictionary), when it can be found.
pub(crate) fn dest_array(doc: &PdfDocument, names: &mut Names, target: &PdfObject) -> Option<PdfObject> {
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
            names.tree(doc)?.get_dict(cur.as_string().ok()?.as_str()).ok().flatten()
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

fn dest_of(doc: &PdfDocument, item: &PdfObject, spaces: &mut HashMap<u32, Option<Matrix>>, names: &mut Names) -> Dest {
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
    match dest_array(doc, names, &t).and_then(|arr| page_dest(doc, spaces, &arr)) {
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

fn is_open(item: &PdfObject) -> bool {
    number(&item.get_dict("Count").ok().flatten()).is_some_and(|c| c > 0.0)
}

/// The outline as it really is, walked once from the root: each item is met once (a link to an item
/// already met, from a shared chain or a cycle, ends that chain), at most [`MAX_ITEMS`] items and
/// [`MAX_DEPTH`] levels. Everything that reads or changes the outline goes through this and never
/// through an item's own `/Parent`, which a damaged file can get wrong, and the work is bounded by
/// the items, not by how the file links them.
struct Shape {
    refs: HashMap<i32, PdfObject>,
    /// Item → the item it is under (none: the root).
    parent: HashMap<i32, Option<i32>>,
    /// Item (none: the root) → the items directly under it, in order.
    kids: HashMap<Option<i32>, Vec<i32>>,
    /// Items were left out: more than [`MAX_ITEMS`], or deeper than [`MAX_DEPTH`].
    truncated: bool,
    /// A chain ran into an item that another chain had already taken (a shared item): the file reaches
    /// that item from two places, and relinking either parent would cut the other's list.
    tangled: bool,
}

fn shape(root: &PdfObject) -> Result<Shape> {
    let mut s = Shape { refs: HashMap::new(), parent: HashMap::new(), kids: HashMap::new(), truncated: false, tangled: false };
    let mut seen: HashSet<i32> = HashSet::new();
    if root.is_indirect()? {
        seen.insert(root.as_indirect()?);
    }
    // First in, first out: an item shared by two parents belongs to the one that comes first.
    let mut todo: VecDeque<(Option<i32>, PdfObject, usize)> = VecDeque::from([(None, root.clone(), 0)]);
    while let Some((key, obj, depth)) = todo.pop_front() {
        let mut list = Vec::new();
        let mut chain: HashSet<i32> = HashSet::new();
        let mut cur = obj.get_dict("First")?;
        while let Some(item) = cur {
            // A link to nothing, or to something that is not an item (a deleted object), ends the chain.
            if !item.is_indirect()? || !item.resolve().ok().flatten().is_some_and(|d| d.is_dict().unwrap_or(false)) {
                break;
            }
            let id = item.as_indirect()?;
            if seen.contains(&id) {
                // An item met before ends the chain. One met in this very chain (a cycle back) loses
                // nothing, and relinking mends it; one another chain took is shared.
                if !chain.contains(&id) {
                    s.tangled = true;
                }
                break;
            }
            if depth >= MAX_DEPTH || s.refs.len() >= MAX_ITEMS {
                s.truncated = true;
                break;
            }
            seen.insert(id);
            chain.insert(id);
            cur = item.get_dict("Next")?;
            s.refs.insert(id, item.clone());
            s.parent.insert(id, key);
            list.push(id);
            todo.push_back((Some(id), item, depth + 1));
        }
        s.kids.insert(key, list);
    }
    Ok(s)
}

impl Shape {
    fn kid_ids(&self, key: Option<i32>) -> &[i32] {
        self.kids.get(&key).map_or(&[], Vec::as_slice)
    }

    fn obj(&self, id: i32) -> Result<PdfObject> {
        self.refs.get(&id).cloned().ok_or(EngineError::Stale)
    }

    fn objs(&self, key: Option<i32>) -> Vec<PdfObject> {
        self.kid_ids(key).iter().filter_map(|id| self.refs.get(id).cloned()).collect()
    }

    /// `x` is `anc` or lies under it.
    fn under(&self, x: Option<i32>, anc: i32) -> bool {
        let mut cur = x;
        for _ in 0..=MAX_DEPTH + 1 {
            match cur {
                None => return false,
                Some(c) if c == anc => return true,
                Some(c) => cur = self.parent.get(&c).copied().flatten(),
            }
        }
        false
    }

    /// An outline that was cut short, or that shares items between parents, is not changed: the part left out would be given wrong counts.
    fn editable(&self) -> Result<()> {
        if self.truncated {
            return Err(EngineError::Invalid("this outline is too large, or nested too deep, to be changed here".into()));
        }
        if self.tangled {
            return Err(EngineError::Invalid(
                "this outline reaches some bookmarks from more than one place, so Inkwell leaves it as it is".into(),
            ));
        }
        Ok(())
    }
}

/// The outline root and its walked shape, for a change.
fn walked(doc: &PdfDocument) -> Result<(PdfObject, Shape)> {
    let root = root(doc)?.ok_or(EngineError::Stale)?;
    let sh = shape(&root)?;
    sh.editable()?;
    Ok((root, sh))
}

fn color_of(item: &PdfObject) -> Option<[f32; 3]> {
    let c = item.get_dict("C").ok().flatten()?;
    let v: Vec<f32> = (0..3).filter_map(|i| number(&c.get_array(i).ok().flatten())).collect();
    (v.len() == 3).then(|| [v[0].clamp(0.0, 1.0), v[1].clamp(0.0, 1.0), v[2].clamp(0.0, 1.0)])
}

struct Reader<'a> {
    doc: &'a PdfDocument,
    sh: &'a Shape,
    spaces: HashMap<u32, Option<Matrix>>,
    names: Names,
}

impl Reader<'_> {
    fn level(&mut self, key: Option<i32>) -> Result<Vec<Bookmark>> {
        let sh = self.sh;
        let mut out = Vec::new();
        for &id in sh.kid_ids(key) {
            let item = &sh.refs[&id];
            let flags = number(&item.get_dict("F")?).map_or(0, |f| f as i32);
            let children = self.level(Some(id))?;
            out.push(Bookmark {
                id,
                title: string_of(item.get_dict("Title")?),
                dest: dest_of(self.doc, item, &mut self.spaces, &mut self.names),
                open: is_open(item) && !children.is_empty(),
                bold: flags & 2 != 0,
                italic: flags & 1 != 0,
                color: color_of(item),
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
    let sh = shape(&root)?;
    let mut r = Reader { doc, sh: &sh, spaces: HashMap::new(), names: Names::default() };
    let items = r.level(None)?;
    Ok(Bookmarks { rev, count: count_items(&items), items, truncated: sh.truncated || sh.tangled })
}

pub fn list(eng: &Engine, doc: DocId) -> Result<Bookmarks> {
    let entry = eng.doc_entry(doc)?;
    eng.read_custom(doc, |d| read(d, entry.generation()))
}

/// What a walk of the form could not finish. A caller that asks whether the document is certified refuses on it
/// (`signed` says yes): what was skipped may be the signature that was looked for.
fn unchecked() -> EngineError {
    EngineError::Invalid(
        "This document's form is too large or too damaged to check for a certification, so Inkwell treats it as certified and leaves it as it is."
            .into(),
    )
}

/// The entries of an array object (nothing for anything else). An array longer than `max`, or an entry that cannot be
/// read, is an error and not a shorter list: whatever is dropped may be what the caller is looking for.
fn array_items(a: Option<PdfObject>, max: usize) -> Result<Vec<PdfObject>> {
    let Some(a) = a.filter(|a| a.is_array().unwrap_or(false)) else { return Ok(Vec::new()) };
    let n = a.len()?;
    if n > max {
        return Err(unchecked());
    }
    let mut items = Vec::with_capacity(n);
    for i in 0..n as i32 {
        items.extend(a.get_array(i)?);
    }
    Ok(items)
}

/// The `/V` of every signature field that has one (its own, or the nearest of its ancestors in the walk), found in
/// the AcroForm's field tree. Bounded (each field once per state, a cap on fields and on what is waiting). A node
/// that is not a dictionary is skipped; a walk that hits a cap or cannot read a node is an error ([`unchecked`]), never
/// a shorter answer.
fn signature_values(form: &PdfObject) -> Result<Vec<PdfObject>> {
    const CAP: usize = 20_000;
    let mut found = Vec::new();
    // (field, is a signature by inheritance, the /V inherited from above and who it is: its object number, or its
    // holder's when it is direct)
    let mut todo: Vec<(PdfObject, bool, Option<(PdfObject, i32)>)> =
        array_items(form.get_dict("Fields")?, CAP)?.into_iter().map(|f| (f, false, None)).collect();
    // Once per (field, inherited type, inherited value): the same field reached from two parents is read in each state.
    let mut seen: HashSet<(i32, bool, i32)> = HashSet::new();
    let mut budget = CAP;
    let number_of = |o: &PdfObject| o.is_indirect().unwrap_or(false).then(|| o.as_indirect().ok()).flatten();
    while let Some((field, inherited, above)) = todo.pop() {
        if budget == 0 {
            return Err(unchecked());
        }
        budget -= 1;
        let num = number_of(&field);
        if let Some(n) = num {
            if !seen.insert((n, inherited, above.as_ref().map_or(i32::MIN, |a| a.1))) {
                continue;
            }
        }
        if !field.resolve().ok().flatten().is_some_and(|d| d.is_dict().unwrap_or(false)) {
            continue;
        }
        let kind = name_of(field.get_dict("FT")?);
        let is_sig = if kind.is_empty() { inherited } else { kind == "Sig" };
        let value = field.get_dict("V")?.map(|v| {
            // A direct value in a direct field has no number: its DocMDP stands for it, so two of them that certify
            // alike are one state and a revisit of the same one is not a new state (negative, clear of -1 and MIN).
            let id = number_of(&v).or(num).unwrap_or_else(|| -10 - reference_p(&v).unwrap_or(0));
            (v, id)
        });
        let value = value.or(above);
        if is_sig {
            found.extend(value.as_ref().map(|v| v.0.clone()));
        }
        let room = CAP.saturating_sub(todo.len());
        todo.extend(array_items(field.get_dict("Kids")?, room)?.into_iter().map(|k| (k, is_sig, value.clone())));
    }
    Ok(found)
}

/// The `/P` of the `/DocMDP` transform in a signature's `/Reference` array: 2 when it names none (the default), 1 when
/// it names one that is not 1, 2 or 3 (a damaged certification is the strictest, not the loosest); `None` when the
/// signature has no such transform. Several: the strictest, whatever the order. A `/Reference` that is one dictionary
/// is read as the one entry it would be, and one with more than 16 entries as `/P 1`.
fn reference_p(sig: &PdfObject) -> Option<i32> {
    let refs = sig.get_dict("Reference").ok().flatten();
    let list = match refs {
        Some(r) if r.is_dict().unwrap_or(false) => vec![r],
        r => match array_items(r, 16) {
            Ok(list) => list,
            Err(_) => return Some(1),
        },
    };
    list.iter()
        .filter(|r| name_of(r.get_dict("TransformMethod").ok().flatten()) == "DocMDP")
        .map(|r| {
            let p = r.get_dict("TransformParams").ok().flatten().and_then(|t| t.get_dict("P").ok().flatten()).filter(|p| !p.is_null().unwrap_or(false));
            if p.is_none() { 2 } else { number(&p).filter(|v| matches!(*v, 1.0 | 2.0 | 3.0)).map_or(1, |v| v as i32) }
        })
        .min()
}

/// The document is certified (a `/DocMDP` signature): any change to it, even to its bookmarks, would
/// void the certification. Its `/P`: 1 no changes, 2 forms and signing, 3 also annotations.
/// The catalog's `/Perms /DocMDP` is where a certification is recorded; a signature field whose `/V` has a `/DocMDP`
/// `/Reference` certifies too. The strictest (lowest) `/P` of all of them counts, so a file that says 3 in one place and
/// 1 in another is 1. Only fields of `/AcroForm /Fields` are read: a signature widget that no field tree lists is not a
/// field to a reader either.
pub(crate) fn certified(doc: &PdfDocument) -> Result<Option<i32>> {
    let cat = doc.catalog()?;
    let from_catalog = match cat.get_dict("Perms")?.map(|p| p.get_dict("DocMDP")).transpose()?.flatten() {
        Some(sig) if !sig.is_null().unwrap_or(true) => Some(reference_p(&sig).unwrap_or(2)),
        _ => None,
    };
    let from_fields = match cat.get_dict("AcroForm")? {
        Some(form) => signature_values(&form)?.iter().filter_map(reference_p).min(),
        None => None,
    };
    Ok(from_catalog.into_iter().chain(from_fields).min())
}

/// The document is signed: `/AcroForm /SigFlags` bit 1, a certification (`/Perms`), or a signature
/// field that has a value. Saving it in full would void the signatures (they cover the bytes of the file
/// as it was), so changes to it stay incremental: nothing that was written is rewritten.
pub(crate) fn signed(doc: &PdfDocument) -> bool {
    signed_inner(doc).unwrap_or(true)
}

fn signed_inner(doc: &PdfDocument) -> Result<bool> {
    let cat = doc.catalog()?;
    if cat.get_dict("Perms")?.is_some_and(|p| p.get_dict("DocMDP").ok().flatten().is_some() || p.get_dict("UR3").ok().flatten().is_some()) {
        return Ok(true);
    }
    let Some(form) = cat.get_dict("AcroForm")? else { return Ok(false) };
    if number(&form.get_dict("SigFlags")?).is_some_and(|f| f as i32 & 1 != 0) {
        return Ok(true);
    }
    Ok(!signature_values(&form)?.is_empty())
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

/// Sets `/Count` on the root and on every item under it, from the outline as it is now (an item's
/// sign says open or closed, its size the items shown below it when it is open).
fn recount(doc: &PdfDocument) -> Result<()> {
    let Some(mut root) = root(doc)? else { return Ok(()) };
    let sh = shape(&root)?;
    sh.editable()?;
    count_below(&sh, None, &mut root, true)?;
    Ok(())
}

fn count_below(sh: &Shape, key: Option<i32>, item: &mut PdfObject, is_root: bool) -> Result<i32> {
    let mut visible = 0;
    for &id in sh.kid_ids(key) {
        let mut k = sh.refs[&id].clone();
        let open = is_open(&k);
        let below = count_below(sh, Some(id), &mut k, false)?;
        visible += 1 + if open { below } else { 0 };
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

/// The item `id` and everything under it, deleted; returns how many.
fn delete_tree(doc: &mut PdfDocument, sh: &Shape, id: i32) -> Result<u32> {
    let mut n = 0;
    let mut stack = vec![id];
    while let Some(cur) = stack.pop() {
        stack.extend_from_slice(sh.kid_ids(Some(cur)));
        doc.delete_object(cur)?;
        n += 1;
    }
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

/// Puts a new item under `parent`, whose children now are `ks`, at `index` (none: last); `ks` is
/// updated. Returns the new item.
fn insert_new(
    doc: &mut PdfDocument,
    parent: &mut PdfObject,
    ks: &mut Vec<PdfObject>,
    index: Option<usize>,
    title: &str,
    dest: Option<PdfObject>,
) -> Result<PdfObject> {
    let mut dict = doc.new_dict()?;
    dict.dict_put("Title", text_object(doc, title)?)?;
    dict.dict_put("Parent", parent.clone())?;
    if let Some(d) = dest {
        dict.dict_put("Dest", d)?;
    }
    let new = doc.add_object(&dict)?;
    let at = index.map_or(ks.len(), |i| i.min(ks.len()));
    ks.insert(at, new.clone());
    relink(parent, ks)?;
    Ok(new)
}

fn same_point(a: Option<[f32; 2]>, b: Option<[f32; 2]>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a[0] - b[0]).abs() < 0.01 && (a[1] - b[1]).abs() < 0.01,
        _ => false,
    }
}

/// Runs `op` on `doc`; returns the id and count for [`BookmarkEdited`], or `None` when the change
/// would leave the document as it is (nothing was touched then).
pub(crate) fn run(doc: &mut PdfDocument, op: &BookmarkOp) -> Result<Option<(Option<i32>, u32)>> {
    if certified(doc)?.is_some() {
        return Err(EngineError::Invalid(
            "This document is certified. Changing its bookmarks would void the certification, so Inkwell leaves them as they are.".into(),
        ));
    }
    let mut made = None;
    let mut count = 0;
    match op {
        BookmarkOp::Add { parent, index, title, page, at } => {
            let title = clean_title(title);
            if title.is_empty() {
                return Err(EngineError::Invalid("a bookmark needs a name".into()));
            }
            let dest = dest_object(doc, *page, *at)?;
            let root = root_or_new(doc)?;
            let sh = shape(&root)?;
            sh.editable()?;
            let mut p = match parent {
                Some(id) => sh.obj(*id)?,
                None => root.clone(),
            };
            let mut ks = sh.objs(*parent);
            let new = insert_new(doc, &mut p, &mut ks, index.map(|i| i as usize), &title, Some(dest))?;
            if parent.is_some() {
                // A group with a new member is shown open, so the member is seen.
                p.dict_put("Count", PdfObject::new_int(1)?)?;
            }
            made = Some(new.as_indirect()?);
            count = 1;
            recount(doc)?;
        }
        BookmarkOp::Rename { id, title } => {
            let title = clean_title(title);
            if title.is_empty() {
                return Err(EngineError::Invalid("a bookmark needs a name".into()));
            }
            let (_, sh) = walked(doc)?;
            let mut it = sh.obj(*id)?;
            let wanted: String = title.chars().filter(|c| !c.is_control() || *c == '\t').take(MAX_TITLE).collect();
            if string_of(it.get_dict("Title")?) == wanted {
                return Ok(None);
            }
            it.dict_put("Title", text_object(doc, &title)?)?;
        }
        BookmarkOp::Delete { ids } => {
            let (root, sh) = walked(doc)?;
            let mut want = ids.clone();
            want.sort_unstable();
            want.dedup();
            if want.iter().any(|id| !sh.refs.contains_key(id)) {
                return Err(EngineError::Stale);
            }
            if want.is_empty() {
                return Ok(None);
            }
            // Nested picks go with their ancestor: take the topmost ones only, by the real tree.
            let tops: Vec<i32> = want.iter().copied().filter(|id| !want.iter().any(|a| a != id && sh.under(sh.parent[id], *a))).collect();
            let drop: HashSet<i32> = tops.iter().copied().collect();
            let mut parents: Vec<Option<i32>> = Vec::new();
            for id in &tops {
                let p = sh.parent[id];
                if !parents.contains(&p) {
                    parents.push(p);
                }
            }
            for p in parents {
                let mut obj = match p {
                    Some(id) => sh.obj(id)?,
                    None => root.clone(),
                };
                let rest: Vec<PdfObject> = sh.objs(p).into_iter().filter(|k| !drop.contains(&k.as_indirect().unwrap_or(-1))).collect();
                relink(&mut obj, &rest)?;
            }
            for id in &tops {
                count += delete_tree(doc, &sh, *id)?;
            }
            recount(doc)?;
        }
        BookmarkOp::Move { id, parent, index } => {
            let (root, sh) = walked(doc)?;
            let it = sh.obj(*id)?;
            if let Some(p) = parent {
                if !sh.refs.contains_key(p) {
                    return Err(EngineError::Stale);
                }
                if p == id || sh.under(Some(*p), *id) {
                    return Err(EngineError::Invalid("a bookmark can't go inside itself".into()));
                }
            }
            let from = sh.parent[id];
            let rest: Vec<PdfObject> = sh.objs(from).into_iter().filter(|k| k.as_indirect().ok() != Some(*id)).collect();
            let old_at = sh.kid_ids(from).iter().position(|k| k == id).unwrap_or(0);
            if from == *parent && old_at == (*index as usize).min(rest.len()) {
                return Ok(None);
            }
            let obj_of = |key: Option<i32>| -> Result<PdfObject> {
                match key {
                    Some(k) => sh.obj(k),
                    None => Ok(root.clone()),
                }
            };
            let mut from_obj = obj_of(from)?;
            relink(&mut from_obj, &rest)?;
            let mut to = obj_of(*parent)?;
            let mut ks = if from == *parent { rest } else { sh.objs(*parent) };
            let at = (*index as usize).min(ks.len());
            ks.insert(at, it);
            relink(&mut to, &ks)?;
            if parent.is_some() {
                to.dict_put("Count", PdfObject::new_int(1)?)?;
            }
            recount(doc)?;
        }
        BookmarkOp::SetDest { id, page, at } => {
            let (_, sh) = walked(doc)?;
            let mut it = sh.obj(*id)?;
            let dest = dest_object(doc, *page, *at)?;
            let now = dest_of(doc, &it, &mut HashMap::new(), &mut Names::default());
            if let Dest::Page { page: p, at: a } = now {
                if p == *page && same_point(a, *at) && it.get_dict("A")?.is_none() {
                    return Ok(None);
                }
            }
            it.dict_delete("A")?;
            it.dict_put("Dest", dest)?;
        }
        BookmarkOp::SetOpen { id, open } => {
            let (_, sh) = walked(doc)?;
            let mut it = sh.obj(*id)?;
            if sh.kid_ids(Some(*id)).is_empty() {
                return Err(EngineError::Invalid("this bookmark has nothing under it".into()));
            }
            if is_open(&it) == *open {
                return Ok(None);
            }
            it.dict_put("Count", PdfObject::new_int(if *open { 1 } else { -1 })?)?;
            recount(doc)?;
        }
        BookmarkOp::SetStyle { id, bold, italic, color, reset_color } => {
            let (_, sh) = walked(doc)?;
            let mut it = sh.obj(*id)?;
            let cur = number(&it.get_dict("F")?).map_or(0, |f| f as i32);
            let mut flags = cur;
            if let Some(b) = bold {
                flags = if *b { flags | 2 } else { flags & !2 };
            }
            if let Some(i) = italic {
                flags = if *i { flags | 1 } else { flags & !1 };
            }
            let have = color_of(&it);
            let same_color = if reset_color == &Some(true) {
                it.get_dict("C")?.is_none()
            } else if let Some(c) = color {
                have.is_some_and(|h| (0..3).all(|i| (h[i] - c[i].clamp(0.0, 1.0)).abs() < 1e-4))
            } else {
                true
            };
            if flags == cur && same_color {
                return Ok(None);
            }
            if flags != cur {
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
            let mut root = root_or_new(doc)?;
            let sh = shape(&root)?;
            sh.editable()?;
            let mut top: Vec<PdfObject> = Vec::new();
            if *replace {
                for &id in sh.kid_ids(None) {
                    delete_tree(doc, &sh, id)?;
                }
                relink(&mut root, &[])?;
            } else {
                top = sh.objs(None);
            }
            // The children of the items made here, by item; and, for nesting, level → the item last made.
            let mut lists: HashMap<i32, Vec<PdfObject>> = HashMap::new();
            let mut stack: Vec<(u8, PdfObject)> = Vec::new();
            for row in &found.rows {
                while stack.last().is_some_and(|(l, _)| *l >= row.level) {
                    stack.pop();
                }
                let mut parent = stack.last().map_or_else(|| root.clone(), |(_, p)| p.clone());
                let dest = dest_object(doc, row.page, Some(row.at))?;
                let list = match stack.last() {
                    Some((_, p)) => lists.entry(p.as_indirect()?).or_default(),
                    None => &mut top,
                };
                let new = insert_new(doc, &mut parent, list, None, &row.title, Some(dest))?;
                if made.is_none() {
                    made = Some(new.as_indirect()?);
                }
                if stack.last().is_some() {
                    parent.dict_put("Count", PdfObject::new_int(1)?)?;
                }
                stack.push((row.level, new));
                count += 1;
            }
            recount(doc)?;
        }
    }
    Ok(Some((made, count)))
}

/// Applies `op`, listed at generation `rev`: one undo step, or none when it changes nothing.
/// `Stale` when the document moved on.
pub fn apply(eng: &Engine, doc: DocId, rev: u32, op: &BookmarkOp, source: EditSource) -> Result<BookmarkEdited> {
    let entry = eng.doc_entry(doc)?;
    let mut out = (None, 0);
    // A removal makes the next Save a full rewrite, which would void a signature: a signed document's
    // deleted items stay in the file as unused objects, and its save stays incremental.
    let removes = matches!(op, BookmarkOp::Delete { .. } | BookmarkOp::FromHeadings { replace: true })
        && !eng.read_custom(doc, |d| Ok(signed(d)))?;
    let state = eng.apply_custom_unless_noop(doc, label(op), source, removes, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        match run(d, op)? {
            Some(r) => {
                out = r;
                Ok(Some(Vec::new()))
            }
            None => Ok(None),
        }
    })?;
    Ok(BookmarkEdited { state, id: out.0, count: out.1 })
}

/// What [`BookmarkOp::FromHeadings`] would make, changing nothing.
pub fn preview_headings(eng: &Engine, doc: DocId) -> Result<HeadingPreview> {
    eng.read_custom(doc, |d| headings::find(d).map(|h| h.preview()))
}
