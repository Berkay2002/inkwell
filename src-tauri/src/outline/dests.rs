//! Named destinations: the catalog's `/Dests` dictionary (PDF 1.1) and the `/Names /Dests` name tree (PDF 1.2).
//! Listed with the page each goes to and what uses it, renamed (every reference we can see follows, or the rename is
//! refused and says which one can't), set to a view, deleted, and removed when nothing uses them.
//!
//! A destination is named by its text (`mupdf_text` of the key's bytes, else the bytes spelled out): a name that
//! lives in both sources is one destination here and every change reaches both places. The tree is read by its own
//! bounded walk (not `Dests`, which flattens it) and written by rebuilding its nodes sorted by the key's bytes with
//! `/Limits` on every node but the root, so a tree comes out valid whatever it was. Like the rest of this module, every
//! change is one `Engine::apply_custom` step and the lookup the panels use (`Names`) is never forked.

use super::{Dest, Names, dest_object_zoomed, name_of, number, page_dest, refuse_if_certified, signed, string_of, text_object, where_to};
use crate::compress::mupdf_text;
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::textedit::Matrix;
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

type Result<T> = std::result::Result<T, EngineError>;

/// Entries read from each source; more are left as they are, and the document's destinations are not edited.
pub const MAX_ENTRIES: usize = 20_000;
/// Name tree nodes read, and levels below the root.
pub const MAX_NODES: usize = 4096;
pub const MAX_DEPTH: usize = 12;
/// Objects visited when looking for what uses a name (pages, annotations, actions, fields, bookmarks).
const MAX_WALK: usize = 1_000_000;
const MAX_PAGES: i32 = 100_000;
/// Entries of `/Names` of the catalog whose values are searched for actions (the other name trees).
const MAX_NAME_TREES: usize = 1024;
/// Bytes a name search may compare in all (names times JavaScript) before "Remove unused" refuses; the list shows the
/// "a script mentions it" hint only while it is under a thirty-second of this, so listing stays quick.
const MAX_SCAN: usize = 1 << 30;
/// JavaScript text read in all, and per script: past it, every name counts as possibly used.
const MAX_JS: usize = 128 * 1024;
/// Entries per leaf and children per inner node of a tree this module writes.
const FAN: usize = 32;
const MAX_NAME: usize = 200;

// ---- reading ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    /// The catalog's `/Dests` dictionary: keys are names.
    Old,
    /// The `/Names /Dests` tree: keys are strings.
    Tree,
}

struct Entry {
    source: Source,
    /// The key object as stored (a name, or a string).
    key: PdfObject,
    /// What the person sees: MuPDF's decoding of the key, else its bytes spelled out.
    text: String,
    value: PdfObject,
}

/// Both sources, read once.
struct Table {
    /// The catalog has a `/Dests` (MuPDF reads it alone then, and never the tree).
    has_old: bool,
    old: Option<PdfObject>,
    /// The root node of `/Names /Dests`.
    tree: Option<PdfObject>,
    entries: Vec<Entry>,
    /// Text → indices into `entries`, so a name is found without a scan.
    by_text: HashMap<String, Vec<usize>>,
    /// A cap was hit: some entries were not read.
    cut: bool,
    /// Why the tree cannot be rewritten from what was read (a shape no writer makes).
    shape: Option<String>,
}

/// The key's bytes as text: the way MuPDF's viewer decodes them, else `\xNN` for what has no faithful text.
fn display(bytes: &[u8]) -> String {
    // shortcut: two keys that are different bytes can show the same text (a literal `\x7F` and a 0x7F): they are
    // one destination here; every use is re-checked after a change, so the worst case is a refusal.
    mupdf_text(bytes).unwrap_or_else(|| {
        let mut s = String::new();
        for &b in bytes {
            match b {
                b'\\' => s.push_str("\\\\"),
                0x20..=0x7E => s.push(b as char),
                _ => s.push_str(&format!("\\x{b:02X}")),
            }
        }
        s
    })
}

impl Table {
    fn load(doc: &PdfDocument) -> Result<Table> {
        let cat = doc.catalog()?;
        let mut t = Table { has_old: false, old: None, tree: None, entries: Vec::new(), by_text: HashMap::new(), cut: false, shape: None };
        if let Some(old) = cat.get_dict("Dests")? {
            // MuPDF's `if (dests)`: a /Dests that is not a dictionary is still there, and hides the tree
            t.has_old = true;
            if let Some(d) = old.resolve()?.filter(|d| d.is_dict().unwrap_or(false)) {
                let n = d.dict_len()?;
                t.cut |= n > MAX_ENTRIES;
                for i in 0..n.min(MAX_ENTRIES) as i32 {
                    let (Some(key), Some(value)) = (d.get_dict_key(i)?, d.get_dict_val(i)?) else { continue };
                    let text = display(&key.as_name()?);
                    t.entries.push(Entry { source: Source::Old, key, text, value });
                }
                t.old = Some(d);
            }
        }
        if let Some(root) = cat.get_dict("Names")?.map(|n| n.get_dict("Dests")).transpose()?.flatten() {
            match root.resolve()?.filter(|r| r.is_dict().unwrap_or(false)) {
                Some(root) => {
                    let mut seen = HashSet::new();
                    if root.is_indirect()? {
                        seen.insert(root.as_indirect()?);
                    }
                    let mut nodes = 0;
                    t.walk(&root, 0, &mut seen, &mut nodes)?;
                    t.tree = Some(root);
                }
                None => t.shape = Some("the name tree of destinations is not a dictionary".into()),
            }
        }
        for (i, e) in t.entries.iter().enumerate() {
            t.by_text.entry(e.text.clone()).or_default().push(i);
        }
        Ok(t)
    }

    fn walk(&mut self, node: &PdfObject, depth: usize, seen: &mut HashSet<i32>, nodes: &mut usize) -> Result<()> {
        *nodes += 1;
        if *nodes > MAX_NODES {
            self.cut = true;
            return Ok(());
        }
        let kids = node.get_dict("Kids")?;
        let names = node.get_dict("Names")?;
        let has_kids = kids.is_some();
        if let Some(k) = kids.filter(|k| k.is_array().unwrap_or(false)) {
            if names.is_some() {
                self.shape = Some("a node of the name tree has both /Kids and /Names".into());
            }
            for i in 0..k.len()?.min(MAX_NODES) as i32 {
                let Some(kid) = k.get_array(i)? else { continue };
                // MuPDF skips a child that is not an indirect reference, and so does a rewrite that would drop it
                if !kid.is_indirect()? || !kid.is_dict()? {
                    self.shape = Some("the name tree has a child that is not a node".into());
                    continue;
                }
                if !seen.insert(kid.as_indirect()?) {
                    self.shape = Some("the name tree reaches a node from two places".into());
                    continue;
                }
                if depth + 1 > MAX_DEPTH {
                    self.cut = true;
                    continue;
                }
                self.walk(&kid, depth + 1, seen, nodes)?;
            }
        } else if has_kids {
            self.shape = Some("a node of the name tree has /Kids that is not an array".into());
        }
        let Some(names) = names else { return Ok(()) };
        if !names.is_array()? {
            self.shape = Some("a node of the name tree has /Names that is not an array".into());
            return Ok(());
        }
        let len = names.len()?;
        if len % 2 != 0 {
            self.shape = Some("a node of the name tree has an odd number of /Names".into());
        }
        let have = self.entries.iter().filter(|e| e.source == Source::Tree).count();
        if have + len / 2 > MAX_ENTRIES {
            self.cut = true;
        }
        for i in 0..(len / 2).min(MAX_ENTRIES.saturating_sub(have)) as i32 {
            let (Some(key), Some(value)) = (names.get_array(2 * i)?, names.get_array(2 * i + 1)?) else { continue };
            let key = key.resolve()?.unwrap_or(key);
            if !key.is_string()? {
                self.shape = Some("the name tree has a key that is not a string".into());
                continue;
            }
            let text = display(&key.as_bytes()?);
            self.entries.push(Entry { source: Source::Tree, key, text, value });
        }
        Ok(())
    }

    /// Why the destinations are not changed here, if they are not.
    fn editable(&self) -> Result<()> {
        if self.cut {
            return Err(EngineError::Invalid(format!(
                "this document has more named destinations, or a deeper name tree, than are read here (at most {MAX_ENTRIES} in each place, {MAX_NODES} nodes, {MAX_DEPTH} levels), so Inkwell leaves the named destinations as they are"
            )));
        }
        if let Some(why) = &self.shape {
            return Err(EngineError::Invalid(format!("{why}, so Inkwell leaves the named destinations as they are")));
        }
        Ok(())
    }

    /// The entries named `name`, in both sources.
    fn named(&self, name: &str) -> Vec<usize> {
        self.by_text.get(name).cloned().unwrap_or_default()
    }
}

/// Where an entry's value goes: an array, a `<< /D … >>`, or (a later hop) another name looked up as it is.
fn entry_dest(doc: &PdfDocument, value: &PdfObject, spaces: &mut HashMap<u32, Option<Matrix>>, names: &mut Names) -> Dest {
    let v = value.resolve().ok().flatten();
    if let Some(cur) = v.filter(|c| c.is_name().unwrap_or(false) || c.is_string().unwrap_or(false)) {
        return match names.lookup(doc, &cur, false) {
            None => Dest::Other { what: super::UNCHECKED_DEST.into() },
            Some(None) => Dest::Broken { what: super::broken_reason(&cur) },
            Some(Some(next)) => where_to(doc, &next, spaces, names),
        };
    }
    where_to(doc, value, spaces, names)
}

/// The name an entry's value is, when it is a name or a string (a later hop), or a dictionary whose `/D` is.
fn chained(value: &PdfObject) -> Option<(PdfObject, String)> {
    let v = value.resolve().ok().flatten()?;
    let target = if v.is_dict().unwrap_or(false) { v.get_dict("D").ok().flatten()?.resolve().ok().flatten()? } else { v };
    let bytes = if target.is_name().ok()? {
        target.as_name().ok()?
    } else if target.is_string().ok()? {
        target.as_bytes().ok()?
    } else {
        return None;
    };
    Some((target, display(&bytes)))
}

// ---- what uses a name ----

/// A `/Dest` or a GoTo action's `/D` (or the open action) holding a name or string: where it is, to rewrite it.
struct Ref {
    holder: PdfObject,
    key: &'static str,
    text: String,
    is_name: bool,
    what: String,
}

struct Js {
    /// None: the script could not be read.
    text: Option<String>,
    what: String,
}

#[derive(Default)]
pub(super) struct Uses {
    refs: Vec<Ref>,
    js: Vec<Js>,
    /// Some of the document was not looked at (more objects than `MAX_WALK`, more pages or name trees than are read,
    /// or more JavaScript than `MAX_JS`).
    pub(super) unchecked: bool,
}

#[derive(Clone)]
enum Origin {
    Bookmark(Option<Rc<str>>),
    Page(u32),
    Other(Rc<str>),
}

impl Origin {
    fn what(&self, key: &str) -> String {
        match self {
            _ if key == "OpenAction" => "the document's open action".into(),
            Origin::Bookmark(Some(t)) => format!("bookmark \"{t}\""),
            Origin::Bookmark(None) => "a bookmark".into(),
            Origin::Page(p) if key == "Dest" => format!("a link on page {}", p + 1),
            Origin::Page(p) => format!("an action on page {}", p + 1),
            Origin::Other(l) => l.to_string(),
        }
    }
}

impl Uses {
    fn scan(&mut self, dict: &PdfObject, origin: &Origin, skip: &[&str], todo: &mut Vec<(PdfObject, Origin)>) -> Result<()> {
        let origin = match origin {
            Origin::Bookmark(_) => match dict.get_dict("Title")? {
                Some(t) => Origin::Bookmark(Some(string_of(Some(t)).chars().take(40).collect::<String>().into())),
                None => origin.clone(),
            },
            o => o.clone(),
        };
        let is_goto = name_of(dict.get_dict("S")?) == "GoTo";
        let n = dict.dict_len()?.min(MAX_WALK);
        for i in 0..n as i32 {
            let (Some(k), Some(val)) = (dict.get_dict_key(i)?, dict.get_dict_val(i)?) else { continue };
            let Ok(key) = k.as_name() else { continue };
            let key = String::from_utf8_lossy(&key).into_owned();
            if key == "Parent" || key == "StructTreeRoot" || skip.contains(&key.as_str()) {
                continue;
            }
            match key.as_str() {
                "Dest" | "OpenAction" => {
                    self.note(dict, if key == "Dest" { "Dest" } else { "OpenAction" }, &val, &origin);
                    // a `/Dest << /D … >>` is read through its `/D` as well
                    if let Some(inner) = val.resolve()?.filter(|v| v.is_dict().unwrap_or(false))
                        && let Some(d) = inner.get_dict("D")?
                    {
                        self.note(&inner, "D", &d, &origin);
                    }
                }
                "D" if is_goto => self.note(dict, "D", &val, &origin),
                "JS" => self.script(&val, &origin),
                _ => {}
            }
            if val.is_indirect()? || val.is_dict()? || val.is_array()? {
                todo.push((val, origin.clone()));
            }
        }
        Ok(())
    }

    fn note(&mut self, holder: &PdfObject, key: &'static str, val: &PdfObject, origin: &Origin) {
        let Some(v) = val.resolve().ok().flatten() else { return };
        let (is_name, bytes) = if v.is_name().unwrap_or(false) {
            (true, v.as_name().ok())
        } else if v.is_string().unwrap_or(false) {
            (false, v.as_bytes().ok())
        } else {
            return;
        };
        let Some(bytes) = bytes else { return };
        self.refs.push(Ref { holder: holder.share(), key, text: display(&bytes), is_name, what: origin.what(key) });
    }

    fn script(&mut self, val: &PdfObject, origin: &Origin) {
        let Some(v) = val.resolve().ok().flatten() else { return };
        let what = match origin {
            Origin::Page(p) => format!("JavaScript on page {}", p + 1),
            Origin::Bookmark(_) => "JavaScript in a bookmark".into(),
            Origin::Other(_) => "JavaScript in the document".into(),
        };
        let bytes = if v.is_string().unwrap_or(false) {
            v.as_bytes().ok()
        } else if v.is_stream().unwrap_or(false) {
            v.read_stream_capped(MAX_JS).ok()
        } else {
            return;
        };
        let total: usize = self.js.iter().map(|j| j.text.as_ref().map_or(0, String::len)).sum();
        match bytes {
            Some(b) if total + b.len() <= MAX_JS => {
                let text = if b.starts_with(&[0xFE, 0xFF]) || b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xEF, 0xBB, 0xBF]) {
                    super::decode_text(&b)
                } else {
                    String::from_utf8_lossy(&b).into_owned()
                };
                self.js.push(Js { text: Some(text), what });
            }
            _ => {
                self.unchecked = true;
                self.js.push(Js { text: None, what });
            }
        }
    }

    /// Bytes of script text held.
    fn js_bytes(&self) -> usize {
        self.js.iter().map(|j| j.text.as_ref().map_or(0, String::len)).sum()
    }

    /// Why `name` may be used by something that can't be rewritten (a script that mentions it, one that could not be
    /// read): none when no script is in doubt.
    fn mentioned(&self, name: &str) -> Option<String> {
        self.js.iter().find(|j| j.text.as_ref().is_none_or(|t| t.contains(name))).map(|j| match &j.text {
            Some(_) => format!("{} mentions it", j.what),
            None => format!("{} could not be read", j.what),
        })
    }
}

/// What uses a destination: every `/Dest`, GoTo `/D` and open action reached from the catalog (bookmarks, link
/// annotations, form and page actions, fields), and the JavaScript there is. Bounded; `unchecked` says when it was.
fn find_uses(doc: &PdfDocument) -> Result<Uses> {
    find_uses_capped(doc, MAX_PAGES, MAX_NAME_TREES)
}

/// `find_uses` with the page and name tree caps given (the tests lower them).
pub(super) fn find_uses_capped(doc: &PdfDocument, max_pages: i32, max_name_trees: usize) -> Result<Uses> {
    let mut u = Uses::default();
    let cat = doc.catalog()?;
    let mut todo: Vec<(PdfObject, Origin)> = Vec::new();
    let doc_label: Rc<str> = "an action in the document".into();
    // the catalog itself (open action, document actions); the page tree, outline and destinations are walked below
    u.scan(&cat, &Origin::Other(doc_label.clone()), &["Pages", "Outlines", "Dests", "Names", "AcroForm"], &mut todo)?;
    if let Some(form) = cat.get_dict("AcroForm")? {
        todo.push((form, Origin::Other("an action in a form field".into())));
    }
    if let Some(names) = cat.get_dict("Names")?.and_then(|n| n.resolve().ok().flatten()).filter(|n| n.is_dict().unwrap_or(false)) {
        // what lies past the cap is not looked at, so a name used only there would look unused
        u.unchecked |= names.dict_len()? > max_name_trees;
        for i in 0..names.dict_len()?.min(max_name_trees) as i32 {
            let (Some(k), Some(v)) = (names.get_dict_key(i)?, names.get_dict_val(i)?) else { continue };
            if k.as_name().is_ok_and(|n| n != b"Dests") {
                todo.push((v, Origin::Other(doc_label.clone())));
            }
        }
    }
    if let Some(o) = cat.get_dict("Outlines")? {
        todo.push((o, Origin::Bookmark(None)));
    }
    u.unchecked |= doc.page_count()? > max_pages;
    for i in 0..doc.page_count()?.min(max_pages) {
        todo.push((doc.find_page(i)?, Origin::Page(i as u32)));
    }
    let mut seen: HashSet<i32> = HashSet::new();
    let mut budget = MAX_WALK;
    while let Some((obj, origin)) = todo.pop() {
        if budget == 0 {
            u.unchecked = true;
            break;
        }
        budget -= 1;
        if obj.is_indirect()? && !seen.insert(obj.as_indirect()?) {
            continue;
        }
        let Some(o) = obj.resolve()? else { continue };
        if o.is_array()? {
            for i in 0..o.len()?.min(MAX_WALK) as i32 {
                if let Some(v) = o.get_array(i)? {
                    todo.push((v, origin.clone()));
                }
            }
        } else if o.is_dict()? {
            u.scan(&o, &origin, &[], &mut todo)?;
        }
    }
    Ok(u)
}

/// How much a destination is used, by name.
struct Usage {
    refs: HashMap<String, Vec<String>>,
}

impl Usage {
    fn of(uses: &Uses, table: &Table) -> Usage {
        let mut refs: HashMap<String, Vec<String>> = HashMap::new();
        for r in &uses.refs {
            refs.entry(r.text.clone()).or_default().push(r.what.clone());
        }
        for e in &table.entries {
            if let Some((_, text)) = chained(&e.value) {
                refs.entry(text).or_default().push(format!("the destination \"{}\"", e.text));
            }
        }
        Usage { refs }
    }

    fn count(&self, name: &str) -> usize {
        self.refs.get(name).map_or(0, Vec::len)
    }
}

// ---- the list ----

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DestEntry {
    /// The destination's name as shown; it is also what a change names it by.
    pub name: String,
    pub dest: Dest,
    /// Where the name lives (a file that has both sources shows which).
    pub in_dests: bool,
    pub in_tree: bool,
    /// Only in the name tree while the catalog has `/Dests`: MuPDF reads `/Dests` alone, so it does not find this one.
    pub shadowed: bool,
    /// A source defines the name more than once.
    pub twice: bool,
    /// What uses it: bookmarks, links and actions (not scripts).
    pub refs: u32,
    /// The first few, in words.
    pub used_by: Vec<String>,
    /// Why it may be used by something that is not counted (a script that mentions it).
    pub maybe: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DestList {
    pub rev: u32,
    pub items: Vec<DestEntry>,
    /// The catalog has `/Dests` and the tree has entries too.
    pub both: bool,
    /// More was there than is read: the list is partial and nothing is edited.
    pub truncated: bool,
    /// Why they can't be changed (a shape this does not rewrite), if they can't.
    pub frozen: Option<String>,
    /// Certified: why every change is refused.
    pub locked: Option<String>,
    /// Signed: a change is saved after the signature.
    pub signed: bool,
    /// What uses a name could not be fully found, so "remove unused" refuses.
    pub unchecked: bool,
}

/// Every name, sorted by name, with its page and what uses it.
pub fn read(doc: &PdfDocument, rev: u32) -> Result<DestList> {
    let table = Table::load(doc)?;
    let uses = find_uses(doc)?;
    let usage = Usage::of(&uses, &table);
    let locked = match refuse_if_certified(doc, "Changing its named destinations") {
        Ok(()) => None,
        Err(EngineError::Invalid(why)) => Some(why),
        Err(e) => return Err(e),
    };
    let mut names = Names::default();
    let mut spaces = HashMap::new();
    let mut order: Vec<&str> = table.by_text.keys().map(String::as_str).collect();
    order.sort_by_cached_key(|n| (n.to_lowercase(), n.to_string()));
    // the script hint costs names times script bytes: only while that is small
    let hints = table.by_text.len().saturating_mul(uses.js_bytes()) <= MAX_SCAN / 32;
    let mut items = Vec::with_capacity(order.len());
    for name in order {
        let at = table.named(name);
        // the entry MuPDF reads: /Dests when the catalog has one, else the tree
        let src = at.iter().copied().find(|&i| table.entries[i].source == Source::Old).unwrap_or(at[0]);
        let (in_dests, in_tree) = (
            at.iter().any(|&i| table.entries[i].source == Source::Old),
            at.iter().any(|&i| table.entries[i].source == Source::Tree),
        );
        let used = usage.refs.get(name);
        items.push(DestEntry {
            name: name.to_string(),
            dest: entry_dest(doc, &table.entries[src].value, &mut spaces, &mut names),
            in_dests,
            in_tree,
            shadowed: table.has_old && !in_dests,
            twice: [Source::Old, Source::Tree].iter().any(|s| at.iter().filter(|&&i| table.entries[i].source == *s).count() > 1),
            refs: used.map_or(0, Vec::len) as u32,
            used_by: used.map(|u| u.iter().take(3).cloned().collect()).unwrap_or_default(),
            maybe: if hints { uses.mentioned(name) } else { None },
        });
    }
    let tree_entries = table.entries.iter().any(|e| e.source == Source::Tree);
    Ok(DestList {
        rev,
        items,
        both: table.has_old && tree_entries,
        truncated: table.cut,
        frozen: table.editable().err().and_then(|e| match e {
            EngineError::Invalid(m) => Some(m),
            _ => None,
        }),
        locked,
        signed: signed(doc),
        unchecked: uses.unchecked,
    })
}

pub fn list(eng: &Engine, doc: DocId) -> Result<DestList> {
    let entry = eng.doc_entry(doc)?;
    eng.read_custom(doc, |d| read(d, entry.generation()))
}

// ---- writing ----

/// One change to the named destinations. A destination is named by its text, as the list shows it.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum DestOp {
    /// Another name; every bookmark, link and GoTo action that used the old one follows.
    Rename { name: String, to: String },
    /// Go to `page` (0-based) at the point `at` (page space) at `zoom` (1 is 100 %; none keeps the reader's).
    SetView { name: String, page: u32, at: Option<[f32; 2]>, zoom: Option<f32> },
    Delete { name: String },
    /// A new destination.
    Add { name: String, page: u32, at: Option<[f32; 2]>, zoom: Option<f32> },
    /// Takes out every destination nothing uses; the ones kept in doubt are reported.
    RemoveUnused,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DestEdited {
    pub state: EditState,
    /// Destinations changed (renamed, set, added, deleted or removed).
    pub count: u32,
    /// "Remove unused": the names kept in doubt, each with why (at most 20).
    pub kept: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Done {
    count: u32,
    kept: Vec<String>,
}

fn label(op: &DestOp) -> String {
    match op {
        DestOp::Rename { name, to } => format!("Rename destination \"{}\" to \"{}\"", super::short(name), super::short(to.trim())),
        DestOp::SetView { name, .. } => format!("Set destination \"{}\"", super::short(name)),
        DestOp::Delete { name } => format!("Delete destination \"{}\"", super::short(name)),
        DestOp::Add { name, .. } => format!("Add destination \"{}\"", super::short(name)),
        DestOp::RemoveUnused => "Remove unused destinations".into(),
    }
}

fn invalid(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// A name as typed: trimmed, not empty, no control characters, at most `MAX_NAME` characters.
fn clean_name(raw: &str) -> Result<String> {
    let n = raw.trim();
    if n.is_empty() {
        return Err(invalid("a destination needs a name"));
    }
    if n.chars().count() > MAX_NAME || n.chars().any(char::is_control) {
        return Err(invalid(format!("a destination's name is at most {MAX_NAME} characters, with no control characters")));
    }
    Ok(n.to_string())
}

/// `/Dests` holds a name, and MuPDF looks it up by the ASCII text of a link's own name or string (`viewer_key`): any
/// other text becomes a key the viewer never forms, so such a name only goes into the tree.
fn old_name_ok(name: &str) -> Result<()> {
    if name.is_ascii() {
        Ok(())
    } else {
        Err(invalid("this document keeps its destinations in the old /Dests dictionary, which MuPDF reads by plain ASCII names only; use an ASCII name"))
    }
}

/// The tree rewritten from `pairs` (key object, value): sorted by the key's bytes (what readers search by), leaves of
/// at most `FAN` entries and `FAN` children per node above, `/Limits` on every node but the root. The root is the
/// object the catalog already points at.
fn rebuild(doc: &mut PdfDocument, mut root: PdfObject, pairs: Vec<(PdfObject, PdfObject)>) -> Result<()> {
    let mut keyed: Vec<(Vec<u8>, PdfObject, PdfObject)> = Vec::with_capacity(pairs.len());
    for (k, v) in pairs {
        keyed.push((k.as_bytes()?, k, v));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    fn limits(doc: &PdfDocument, lo: &PdfObject, hi: &PdfObject) -> Result<PdfObject> {
        let mut l = doc.new_array()?;
        l.array_push(lo.share())?;
        l.array_push(hi.share())?;
        Ok(l)
    }
    root.dict_delete("Limits")?;
    if keyed.len() <= FAN {
        let mut names = doc.new_array()?;
        for (_, k, v) in &keyed {
            names.array_push(k.share())?;
            names.array_push(v.share())?;
        }
        root.dict_delete("Kids")?;
        root.dict_put("Names", names)?;
        return Ok(());
    }
    // (node, first key, last key) of the level being built
    let mut level: Vec<(PdfObject, PdfObject, PdfObject)> = Vec::new();
    for chunk in keyed.chunks(FAN) {
        let mut names = doc.new_array()?;
        for (_, k, v) in chunk {
            names.array_push(k.share())?;
            names.array_push(v.share())?;
        }
        let (lo, hi) = (chunk[0].1.share(), chunk[chunk.len() - 1].1.share());
        let mut node = doc.new_dict()?;
        node.dict_put("Limits", limits(doc, &lo, &hi)?)?;
        node.dict_put("Names", names)?;
        level.push((doc.add_object(&node)?, lo, hi));
    }
    while level.len() > FAN {
        let mut up = Vec::new();
        for group in level.chunks(FAN) {
            let mut kids = doc.new_array()?;
            for (n, _, _) in group {
                kids.array_push(n.share())?;
            }
            let (lo, hi) = (group[0].1.share(), group[group.len() - 1].2.share());
            let mut node = doc.new_dict()?;
            node.dict_put("Limits", limits(doc, &lo, &hi)?)?;
            node.dict_put("Kids", kids)?;
            up.push((doc.add_object(&node)?, lo, hi));
        }
        level = up;
    }
    let mut kids = doc.new_array()?;
    for (n, _, _) in &level {
        kids.array_push(n.share())?;
    }
    root.dict_delete("Names")?;
    root.dict_put("Kids", kids)?;
    Ok(())
}

/// The tree's root, made (with `/Names` in the catalog) when the document has none.
fn tree_root(doc: &mut PdfDocument, table: &Table) -> Result<PdfObject> {
    if let Some(r) = &table.tree {
        return Ok(r.share());
    }
    let root = doc.new_dict()?;
    let root = doc.add_object(&root)?;
    let mut cat = doc.catalog()?;
    let mut names = match cat.get_dict("Names")?.and_then(|n| n.resolve().ok().flatten()).filter(|n| n.is_dict().unwrap_or(false)) {
        Some(n) => n,
        None => {
            let n = doc.new_dict()?;
            let n = doc.add_object(&n)?;
            cat.dict_put("Names", n.share())?;
            n
        }
    };
    names.dict_put("Dests", root.share())?;
    Ok(root)
}

fn tree_pairs(table: &Table) -> Vec<(PdfObject, PdfObject)> {
    table.entries.iter().filter(|e| e.source == Source::Tree).map(|e| (e.key.share(), e.value.share())).collect()
}

/// Entry index → its place in `tree_pairs` (none for an entry of `/Dests`).
fn pair_slots(table: &Table) -> Vec<Option<usize>> {
    let mut n = 0;
    table
        .entries
        .iter()
        .map(|e| {
            (e.source == Source::Tree).then(|| {
                n += 1;
                n - 1
            })
        })
        .collect()
}

/// Every destination now, by name, as the list would judge it: what a rename has to leave as it was.
fn dest_snapshot(doc: &PdfDocument) -> Result<HashMap<String, Vec<String>>> {
    let table = Table::load(doc)?;
    let (mut names, mut spaces) = (Names::default(), HashMap::new());
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for e in &table.entries {
        out.entry(e.text.clone()).or_default().push(format!("{:?}", entry_dest(doc, &e.value, &mut spaces, &mut names)));
    }
    out.values_mut().for_each(|v| v.sort());
    Ok(out)
}

/// Where each reference goes now; `Dest::Other` is not judged and so is not held to anything.
fn ref_snapshot(doc: &PdfDocument, refs: &[Ref]) -> Vec<Dest> {
    let (mut names, mut spaces) = (Names::default(), HashMap::new());
    refs.iter()
        .map(|r| match r.holder.get_dict(r.key) {
            Ok(Some(v)) => where_to(doc, &v, &mut spaces, &mut names),
            _ => Dest::None,
        })
        .collect()
}

fn same_zoom(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).abs() < 1e-4,
        _ => false,
    }
}

/// The value already is `[page /XYZ x y zoom]` for these arguments.
fn is_view(doc: &PdfDocument, value: &PdfObject, page: u32, at: Option<[f32; 2]>, zoom: Option<f32>) -> bool {
    let mut v = value.resolve().ok().flatten();
    if v.as_ref().is_some_and(|d| d.is_dict().unwrap_or(false)) {
        v = v.and_then(|d| d.get_dict("D").ok().flatten()).and_then(|d| d.resolve().ok().flatten());
    }
    let Some(arr) = v.filter(|v| v.is_array().unwrap_or(false)) else { return false };
    let Some((p, a)) = page_dest(doc, &mut HashMap::new(), &arr) else { return false };
    name_of(arr.get_array(1).ok().flatten()) == "XYZ"
        && p == page
        && super::same_point(a, at)
        && same_zoom(number(&arr.get_array(4).ok().flatten()), zoom.filter(|z| z.is_finite() && *z > 0.0).map(|z| z.min(64.0)))
}

/// Runs `op`; false when it would leave the document as it is (nothing was touched then).
pub(crate) fn run(doc: &mut PdfDocument, op: &DestOp, out: &mut Done) -> Result<bool> {
    refuse_if_certified(doc, "Changing its named destinations")?;
    let table = Table::load(doc)?;
    table.editable()?;
    match op {
        DestOp::Rename { name, to } => rename(doc, &table, name, to, out),
        DestOp::SetView { name, page, at, zoom } => {
            let at_entries = table.named(name);
            if at_entries.is_empty() {
                return Err(EngineError::Stale);
            }
            refuse_twice(&table, &at_entries, name)?;
            if at_entries.iter().all(|&i| is_view(doc, &table.entries[i].value, *page, *at, *zoom)) {
                return Ok(false);
            }
            let mut pairs = tree_pairs(&table);
            let slots = pair_slots(&table);
            let mut tree_changed = false;
            let mut old = table.old.as_ref().map(PdfObject::share);
            for &i in &at_entries {
                let e = &table.entries[i];
                let mut new = dest_object_zoomed(doc, *page, *at, *zoom)?;
                // a `<< /D … >>` keeps its dictionary: edited where it stands when it is this entry's own, else (an
                // object other names may share) as a copy that this name alone gets
                if let Some(mut dict) = e.value.resolve()?.filter(|v| v.is_dict().unwrap_or(false)) {
                    if !e.value.is_indirect()? {
                        dict.dict_put("D", new)?;
                        continue;
                    }
                    let mut copy = doc.new_dict()?;
                    for k in 0..dict.dict_len()? as i32 {
                        if let (Some(key), Some(val)) = (dict.get_dict_key(k)?, dict.get_dict_val(k)?) {
                            copy.dict_put(key, val)?;
                        }
                    }
                    copy.dict_put("D", new)?;
                    new = copy;
                }
                match e.source {
                    Source::Old => old.as_mut().ok_or(EngineError::Stale)?.dict_put(e.key.share(), new)?,
                    Source::Tree => {
                        pairs[slots[i].ok_or(EngineError::Stale)?].1 = new;
                        tree_changed = true;
                    }
                }
            }
            if tree_changed {
                let root = tree_root(doc, &table)?;
                rebuild(doc, root, pairs)?;
            }
            out.count = 1;
            Ok(true)
        }
        DestOp::Delete { name } => {
            let at_entries = table.named(name);
            if at_entries.is_empty() {
                return Err(EngineError::Stale);
            }
            remove(doc, &table, &at_entries)?;
            out.count = 1;
            Ok(true)
        }
        DestOp::Add { name, page, at, zoom } => {
            let name = clean_name(name)?;
            if table.entries.iter().any(|e| e.text == name) {
                return Err(invalid(format!("a destination named \"{name}\" already exists")));
            }
            let value = dest_object_zoomed(doc, *page, *at, *zoom)?;
            // A file whose only destinations are in /Dests keeps them there (MuPDF reads nothing else); otherwise a
            // new name goes into the name tree, which a file that has both sources does not show MuPDF (`shadowed`).
            if table.has_old && table.tree.is_none() {
                old_name_ok(&name)?;
                let mut old = table.old.as_ref().ok_or_else(|| invalid("the /Dests of this document is not a dictionary"))?.share();
                old.dict_put(PdfObject::new_name(&name)?, value)?;
            } else {
                let mut pairs = tree_pairs(&table);
                pairs.push((text_object(doc, &name)?, value));
                let root = tree_root(doc, &table)?;
                rebuild(doc, root, pairs)?;
            }
            out.count = 1;
            Ok(true)
        }
        DestOp::RemoveUnused => {
            let uses = find_uses(doc)?;
            if uses.unchecked {
                return Err(invalid(
                    "this document is too large, or has too much JavaScript, to check what uses its destinations, so none are removed",
                ));
            }
            let usage = Usage::of(&uses, &table);
            if table.by_text.len().saturating_mul(uses.js_bytes()) > MAX_SCAN {
                return Err(invalid("this document has too many destinations and too much JavaScript to check which ones a script uses, so none are removed"));
            }
            let mut names: Vec<&str> = table.by_text.keys().map(String::as_str).collect();
            names.sort_unstable();
            let mut gone = Vec::new();
            for name in names {
                let at_entries = table.named(name);
                let why = if usage.count(name) > 0 {
                    continue;
                } else if let Some(m) = uses.mentioned(name) {
                    Some(m)
                } else if at_entries.iter().any(|&i| mupdf_text(&key_bytes(&table.entries[i])).is_none()) {
                    Some("its name can't be read as text, so a reference may spell it another way".to_string())
                } else {
                    None
                };
                match why {
                    Some(w) if out.kept.len() < 20 => out.kept.push(format!("\"{name}\": {w}")),
                    Some(_) => {}
                    None => gone.extend(at_entries),
                }
            }
            if gone.is_empty() {
                return Ok(false);
            }
            let count = gone.iter().map(|&i| table.entries[i].text.as_str()).collect::<HashSet<_>>().len() as u32;
            remove(doc, &table, &gone)?;
            out.count = count;
            Ok(true)
        }
    }
}

fn key_bytes(e: &Entry) -> Vec<u8> {
    match e.source {
        Source::Old => e.key.as_name().unwrap_or_default(),
        Source::Tree => e.key.as_bytes().unwrap_or_default(),
    }
}

/// Takes the entries `gone` (indices into the table) out of their sources.
fn remove(doc: &mut PdfDocument, table: &Table, gone: &[usize]) -> Result<()> {
    let drop: HashSet<usize> = gone.iter().copied().collect();
    if let Some(old) = &table.old {
        let mut old = old.share();
        for &i in gone.iter().filter(|&&i| table.entries[i].source == Source::Old) {
            old.dict_delete(table.entries[i].key.share())?;
        }
    }
    if gone.iter().any(|&i| table.entries[i].source == Source::Tree) {
        let pairs = table
            .entries
            .iter()
            .enumerate()
            .filter(|(i, e)| e.source == Source::Tree && !drop.contains(i))
            .map(|(_, e)| (e.key.share(), e.value.share()))
            .collect();
        let root = tree_root(doc, table)?;
        rebuild(doc, root, pairs)?;
    }
    Ok(())
}

/// A name defined twice in one source (a tree that already had duplicate keys) is changed by neither a rename nor a
/// new view: which of the two a reader finds is not ours to say.
fn refuse_twice(table: &Table, at_entries: &[usize], name: &str) -> Result<()> {
    for s in [Source::Old, Source::Tree] {
        if at_entries.iter().filter(|&&i| table.entries[i].source == s).count() > 1 {
            return Err(invalid(format!("\"{name}\" is defined twice in this document's destinations: delete one of them first")));
        }
    }
    Ok(())
}

fn rename(doc: &mut PdfDocument, table: &Table, name: &str, to: &str, out: &mut Done) -> Result<bool> {
    let to = clean_name(to)?;
    if to == name {
        return Ok(false);
    }
    let at_entries = table.named(name);
    if at_entries.is_empty() {
        return Err(EngineError::Stale);
    }
    if table.entries.iter().any(|e| e.text == to) {
        return Err(invalid(format!("a destination named \"{to}\" already exists")));
    }
    refuse_twice(table, &at_entries, name)?;
    if at_entries.iter().any(|&i| table.entries[i].source == Source::Old) {
        old_name_ok(&to)?;
    }
    let uses = find_uses(doc)?;
    if uses.unchecked {
        return Err(invalid("this document is too large, or has too much JavaScript, to check what uses the destination, so it is not renamed"));
    }
    if let Some(why) = uses.mentioned(name) {
        return Err(invalid(format!("\"{name}\" is not renamed: {why}, and Inkwell can't rewrite JavaScript")));
    }
    let before = (ref_snapshot(doc, &uses.refs), dest_snapshot(doc)?);

    // the references that spell the old name
    for r in uses.refs.iter().filter(|r| r.text == name) {
        let new = if r.is_name { PdfObject::new_name(&to)? } else { text_object(doc, &to)? };
        r.holder.share().dict_put(r.key, new)?;
    }
    // the entries: the key, and the values that are the old name (a later hop)
    let mut pairs = tree_pairs(table);
    let slots = pair_slots(table);
    let mut tree_changed = false;
    let mut old = table.old.as_ref().map(PdfObject::share);
    for (i, e) in table.entries.iter().enumerate() {
        let renamed = at_entries.contains(&i);
        let hop = chained(&e.value).filter(|(_, text)| text == name);
        if !renamed && hop.is_none() {
            continue;
        }
        if let Some((target, _)) = hop {
            let new = if target.is_name()? { PdfObject::new_name(&to)? } else { text_object(doc, &to)? };
            match e.value.resolve()?.filter(|v| v.is_dict().unwrap_or(false)) {
                Some(mut dict) => dict.dict_put("D", new)?,
                None => match e.source {
                    Source::Old => old.as_mut().ok_or(EngineError::Stale)?.dict_put(e.key.share(), new)?,
                    Source::Tree => {
                        pairs[slots[i].ok_or(EngineError::Stale)?].1 = new;
                        tree_changed = true;
                    }
                },
            }
        }
        if renamed {
            match e.source {
                Source::Old => {
                    let old = old.as_mut().ok_or(EngineError::Stale)?;
                    // the value now (a later hop may have just been rewritten)
                    let value = old.get_dict(e.key.share())?.ok_or(EngineError::Stale)?;
                    old.dict_put(PdfObject::new_name(&to)?, value)?;
                    old.dict_delete(e.key.share())?;
                }
                Source::Tree => {
                    pairs[slots[i].ok_or(EngineError::Stale)?].0 = text_object(doc, &to)?;
                    tree_changed = true;
                }
            }
        }
    }
    if tree_changed {
        let root = tree_root(doc, table)?;
        rebuild(doc, root, pairs)?;
    }

    // Everything that went somewhere goes there still, and the destinations are the ones they were.
    let after = ref_snapshot(doc, &uses.refs);
    for ((r, was), now) in uses.refs.iter().zip(&before.0).zip(&after) {
        if !matches!(was, Dest::Other { .. }) && was != now {
            return Err(invalid(format!(
                "renaming \"{name}\" would change where {} goes, and Inkwell can't rewrite it, so the destination is left as it is",
                r.what
            )));
        }
    }
    let mut was = before.1;
    if let Some(v) = was.remove(name) {
        was.insert(to.clone(), v);
    }
    if was != dest_snapshot(doc)? {
        return Err(invalid(format!("renaming \"{name}\" would change another destination, so it is left as it is")));
    }
    out.count = 1;
    Ok(true)
}

/// Applies `op`, listed at generation `rev`: one undo step, or none when it changes nothing.
/// `Stale` when the document moved on.
pub fn apply(eng: &Engine, doc: DocId, rev: u32, op: &DestOp, source: EditSource) -> Result<DestEdited> {
    let entry = eng.doc_entry(doc)?;
    let mut done = Done::default();
    // A removal makes the next Save a full rewrite, which drops the nodes and values left behind: not in a signed
    // document, whose save stays incremental (the signed bytes stay where they are).
    let removes = matches!(op, DestOp::Rename { .. } | DestOp::Delete { .. } | DestOp::RemoveUnused) && !eng.read_custom(doc, |d| Ok(signed(d)))?;
    let state = eng.apply_custom_unless_noop(doc, label(op), source, removes, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        Ok(run(d, op, &mut done)?.then(Vec::new))
    })?;
    Ok(DestEdited { state, count: done.count, kept: done.kept })
}
