//! The tag tree as something to edit: nodes with their role, /Alt, /ActualText, /Lang and table
//! header attributes; moving a node; and the reading-order operations built on that (reorder, change
//! role, mark as artifact, merge, split). Every operation here runs inside one edit-core step.
//!
//! Marked content is tied to the tree in two places: the element's /K (the MCID, or an MCR) and
//! the ParentTree, a number tree from the page's /StructParents key (and each annotation's
//! /StructParent) back to the element. An operation that changes which element holds some content
//! changes both, so reading order, the checker and a screen reader keep agreeing. [`problems`] is
//! the validator for that agreement, used by the checker and by the tests after every edit.

use super::content;
use super::locate::mcid_of;
use super::tree::{Tree, is_standard, name, page_index, valid_mcid, xref_of};
use super::{Problem, Result, annots, bad, page_content, text_obj, valid_lang};
use mupdf::Buffer;
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The parent of a top-level element in the maps below: the StructTreeRoot.
const ROOT: i32 = -1;

// ---- kids of a node ------------------------------------------------------------------------------

#[derive(Clone)]
enum Kid {
    /// A bare MCID, on the element's page.
    Mcid(i32),
    Mcr(PdfObject),
    Objr(PdfObject),
    Elem(PdfObject),
}

impl Kid {
    fn obj(&self) -> Result<PdfObject> {
        Ok(match self {
            Kid::Mcid(m) => PdfObject::new_int(*m)?,
            Kid::Mcr(o) | Kid::Objr(o) | Kid::Elem(o) => o.clone(),
        })
    }

    fn xref(&self) -> i32 {
        match self {
            Kid::Elem(o) => xref_of(o).unwrap_or(0),
            _ => 0,
        }
    }
}

fn kids_of(node: &PdfObject) -> Result<Vec<Kid>> {
    let Some(k) = node.get_dict("K")? else { return Ok(Vec::new()) };
    let items: Vec<PdfObject> = if k.is_array()? { k.array_iter()?.collect::<std::result::Result<_, _>>()? } else { vec![k] };
    let mut out = Vec::with_capacity(items.len());
    for x in items {
        if x.is_int()? {
            out.push(Kid::Mcid(x.as_int()?));
        } else if x.is_dict()? {
            out.push(match name(&x, "Type")?.as_deref() {
                Some("MCR") => Kid::Mcr(x),
                Some("OBJR") => Kid::Objr(x),
                _ => Kid::Elem(x),
            });
        }
    }
    Ok(out)
}

fn set_kids(doc: &PdfDocument, node: &mut PdfObject, kids: &[Kid]) -> Result<()> {
    if kids.is_empty() {
        node.dict_delete("K")?;
        return Ok(());
    }
    let mut arr = doc.new_array()?;
    for k in kids {
        arr.array_push(k.obj()?)?;
    }
    node.dict_put("K", arr)?;
    Ok(())
}

fn struct_root(doc: &PdfDocument) -> Result<PdfObject> {
    doc.catalog()?.get_dict("StructTreeRoot")?.filter(|r| r.is_dict().unwrap_or(false)).ok_or_else(|| bad("the document has no tags"))
}

// ---- the ParentTree ------------------------------------------------------------------------------

/// The ParentTree as maps from key to element object numbers: an array entry per MCID for a page,
/// a single element for an annotation. Written back as one flat /Nums array.
#[derive(Default, Debug, Clone, PartialEq)]
pub struct ParentTree {
    pub arrays: BTreeMap<i32, Vec<Option<i32>>>,
    pub singles: BTreeMap<i32, i32>,
    /// Something in the file was left out while reading (a key that isn't an integer, a value that
    /// is neither an array nor a reference, an entry that isn't a reference, a key twice, a tree
    /// too deep). A tree read like this is never written back.
    pub skipped: bool,
}

/// Entries (MCIDs over all page keys) a rewritten ParentTree may hold.
const MAX_ENTRIES: usize = 4_000_000;

fn read_nums(node: &PdfObject, out: &mut Vec<(i32, PdfObject)>, depth: usize, seen: &mut HashSet<i32>, skipped: &mut bool) -> Result<()> {
    if depth > 32 || !node.is_dict()? {
        *skipped = true;
        return Ok(());
    }
    if node.is_indirect()? && !seen.insert(node.as_indirect()?) {
        *skipped = true;
        return Ok(());
    }
    if let Some(n) = node.get_dict("Nums")?.filter(|n| n.is_array().unwrap_or(false)) {
        let items: Vec<PdfObject> = n.array_iter()?.collect::<std::result::Result<_, _>>()?;
        if !items.len().is_multiple_of(2) {
            *skipped = true;
        }
        for pair in items.chunks(2) {
            match pair {
                [k, v] if k.is_int()? => out.push((k.as_int()?, v.clone())),
                _ => *skipped = true,
            }
        }
    }
    if let Some(k) = node.get_dict("Kids")?.filter(|k| k.is_array().unwrap_or(false)) {
        for kid in k.array_iter()? {
            read_nums(&kid?, out, depth + 1, seen, skipped)?;
        }
    }
    Ok(())
}

impl ParentTree {
    pub fn read(root: &PdfObject) -> Result<ParentTree> {
        let mut pt = ParentTree::default();
        let Some(tree) = root.get_dict("ParentTree")? else { return Ok(pt) };
        let mut entries = Vec::new();
        read_nums(&tree, &mut entries, 0, &mut HashSet::new(), &mut pt.skipped)?;
        for (k, v) in entries {
            if pt.arrays.contains_key(&k) || pt.singles.contains_key(&k) {
                pt.skipped = true;
            }
            if v.is_array()? {
                // Indexed, not iterated: a null entry (a gap in the MCIDs) has no object to hand back.
                let mut arr = Vec::new();
                for at in 0..v.len()? as i32 {
                    arr.push(match v.get_array(at)? {
                        Some(x) if x.is_indirect()? => Some(x.as_indirect()?),
                        Some(x) if x.is_null()? => None,
                        None => None,
                        Some(_) => {
                            pt.skipped = true;
                            None
                        }
                    });
                }
                pt.arrays.insert(k, arr);
            } else if v.is_indirect()? {
                pt.singles.insert(k, v.as_indirect()?);
            } else {
                pt.skipped = true;
            }
        }
        Ok(pt)
    }

    #[cfg(test)]
    pub(super) fn set_mcid_for_test(&mut self, key: i32, mcid: i32, xref: Option<i32>) -> Result<()> {
        self.set_mcid(key, mcid, xref)
    }

    /// [`ParentTree::read`] for an edit: a tree with entries that can't be kept is refused before anything changes.
    fn read_for_edit(root: &PdfObject) -> Result<ParentTree> {
        let pt = ParentTree::read(root)?;
        if pt.skipped {
            return Err(bad("the ParentTree has entries that can't be read, so it is not edited; check the document for damage"));
        }
        Ok(pt)
    }

    /// Entries over all page keys.
    fn entries(&self) -> usize {
        self.arrays.values().map(Vec::len).sum()
    }

    /// Writes the tree as one flat number tree, raising /ParentTreeNextKey past every key. Refuses a
    /// tree that was not read completely, or one with more entries than [`MAX_ENTRIES`].
    pub fn write(&self, doc: &PdfDocument, root: &mut PdfObject) -> Result<()> {
        if self.skipped {
            return Err(bad("the ParentTree has entries that can't be kept, so it is not rewritten"));
        }
        if self.entries() > MAX_ENTRIES {
            return Err(bad("the ParentTree would be too large to write"));
        }
        let mut keys: Vec<i32> = self.arrays.keys().chain(self.singles.keys()).copied().collect();
        keys.sort_unstable();
        keys.dedup();
        let mut nums = doc.new_array()?;
        for &k in &keys {
            nums.array_push(PdfObject::new_int(k)?)?;
            if let Some(a) = self.arrays.get(&k) {
                let mut arr = doc.new_array()?;
                for e in a {
                    arr.array_push(match e {
                        Some(x) => doc.new_indirect(*x, 0)?,
                        None => PdfObject::new_null(),
                    })?;
                }
                nums.array_push(arr)?;
            } else if let Some(x) = self.singles.get(&k) {
                nums.array_push(doc.new_indirect(*x, 0)?)?;
            }
        }
        let mut tree = doc.new_dict()?;
        tree.dict_put("Nums", nums)?;
        root.dict_put("ParentTree", tree)?;
        let have = root.get_dict("ParentTreeNextKey")?.and_then(|v| v.as_int().ok()).unwrap_or(0);
        let next = have.max(keys.last().map_or(0, |k| k.saturating_add(1)));
        root.dict_put("ParentTreeNextKey", PdfObject::new_int(next)?)?;
        Ok(())
    }

    /// Every entry that names `from` names `to` instead.
    fn repoint(&mut self, from: i32, to: i32) -> bool {
        let mut changed = false;
        for a in self.arrays.values_mut() {
            for e in a.iter_mut().filter(|e| **e == Some(from)) {
                *e = Some(to);
                changed = true;
            }
        }
        for e in self.singles.values_mut().filter(|e| **e == from) {
            *e = to;
            changed = true;
        }
        changed
    }

    /// Entry `mcid` of page key `key` names `xref` (or nothing). An MCID outside `0..=MAX_MCID` is refused.
    fn set_mcid(&mut self, key: i32, mcid: i32, xref: Option<i32>) -> Result<()> {
        if !valid_mcid(mcid) {
            return Err(bad(format!("{mcid} is not a usable marked-content id")));
        }
        let a = self.arrays.entry(key).or_default();
        let at = mcid as usize;
        if a.len() <= at {
            if xref.is_none() {
                return Ok(());
            }
            a.resize(at + 1, None);
        }
        a[at] = xref;
        Ok(())
    }
}

fn page_key(page: &PdfObject) -> Result<Option<i32>> {
    Ok(page.get_dict("StructParents")?.and_then(|v| v.as_int().ok()))
}

// ---- consistency ---------------------------------------------------------------------------------

/// Where the tree and the ParentTree disagree, and structure elements whose /P is not their parent
/// in the tree. Empty means a screen reader, the checker and the page agree.
pub fn problems(doc: &PdfDocument) -> Result<Vec<Problem>> {
    let index = page_index(doc)?;
    let t = Tree::read(doc, &index)?;
    let mut out = Vec::new();
    if !t.exists {
        return Ok(out);
    }
    let root = struct_root(doc)?;
    let root_x = xref_of(&root)?;
    let pt = ParentTree::read(&root)?;
    let n = doc.page_count()?.max(0) as u32;
    let mut key_of: HashMap<u32, i32> = HashMap::new();
    let mut page_of: HashMap<i32, u32> = HashMap::new();
    let mut pages = Vec::with_capacity(n as usize);
    for p in 0..n {
        let page = doc.find_page(p as i32)?;
        if let Some(k) = page_key(&page)? {
            key_of.insert(p, k);
            page_of.insert(k, p);
        }
        pages.push(page);
    }
    let by_xref: HashMap<i32, usize> = t.elems.iter().enumerate().filter(|(_, e)| e.xref != 0).map(|(i, e)| (e.xref, i)).collect();
    let mut p = |page: Option<u32>, xref: i32, message: String| out.push(Problem { page, xref: (xref != 0).then_some(xref), message });
    if t.incomplete {
        p(None, root_x, "The tag tree is deeper or larger than can be read, has an element reachable twice, a marked-content id out of range or content without a page, so it can't be repaired or edited".into());
    }
    if pt.skipped {
        p(None, root_x, "The ParentTree has entries that can't be read (not integer keys, not references, repeated keys), so it can't be repaired".into());
    }
    for e in &t.elems {
        if e.xref == 0 {
            continue;
        }
        for &(pg, m) in &e.refs {
            let Some(key) = key_of.get(&pg) else {
                p(Some(pg), e.xref, format!("Page {} has content in the tag tree but no /StructParents key", pg + 1));
                continue;
            };
            match pt.arrays.get(key) {
                None => p(Some(pg), e.xref, format!("The ParentTree has no entry for page {} (key {key})", pg + 1)),
                Some(a) if a.get(m as usize).copied().flatten() != Some(e.xref) => {
                    p(Some(pg), e.xref, format!("The ParentTree does not map MCID {m} of page {} to its {} element", pg + 1, e.ty))
                }
                _ => {}
            }
        }
    }
    for (&ax, &owner) in &t.annot_owner {
        let o = &t.elems[owner];
        if o.xref == 0 {
            continue;
        }
        let annot = doc.new_indirect(ax, 0)?;
        let at = o.page;
        match annot.get_dict("StructParent")?.and_then(|v| v.as_int().ok()) {
            None => p(at, o.xref, format!("Annotation {ax} is in the tag tree but has no /StructParent")),
            Some(k) if pt.singles.get(&k) != Some(&o.xref) => p(at, o.xref, format!("The ParentTree does not map annotation {ax} (key {k}) to its {} element", o.ty)),
            _ => {}
        }
    }
    // Content drawn by form XObjects has its own key, on the stream.
    for e in &t.elems {
        for &(stm, m) in &e.stm {
            let key = doc.new_indirect(stm, 0)?.get_dict("StructParents")?.and_then(|v| v.as_int().ok());
            let held = key.and_then(|k| pt.arrays.get(&k)).and_then(|a| a.get(m as usize).copied().flatten());
            if e.xref != 0 && held != Some(e.xref) {
                p(e.page, e.xref, format!("The ParentTree does not map MCID {m} of form object {stm} to its {} element", e.ty));
            }
        }
    }
    for (key, a) in &pt.arrays {
        let Some(&pg) = page_of.get(key) else {
            // Not a page's key: the entries still have to name elements that exist.
            for x in a.iter().flatten().filter(|x| !by_xref.contains_key(x)) {
                p(None, *x, format!("The ParentTree (key {key}) maps content to object {x}, which is not in the tag tree"));
            }
            continue;
        };
        for (m, e) in a.iter().enumerate() {
            let Some(x) = e else { continue };
            match by_xref.get(x) {
                None => p(Some(pg), *x, format!("The ParentTree maps MCID {m} of page {} to object {x}, which is not in the tag tree", pg + 1)),
                Some(&i) if !t.elems[i].refs.contains(&(pg, m as i32)) => {
                    p(Some(pg), *x, format!("The ParentTree maps MCID {m} of page {} to a {} element that does not hold it", pg + 1, t.elems[i].ty))
                }
                _ => {}
            }
        }
    }
    for e in &t.elems {
        if e.xref == 0 {
            continue;
        }
        let obj = doc.new_indirect(e.xref, 0)?;
        let parent = obj.get_dict("P")?.filter(|x| x.is_indirect().unwrap_or(false)).and_then(|x| x.as_indirect().ok());
        let want = match e.parent {
            Some(pi) => t.elems[pi].xref,
            None => root_x,
        };
        if want != 0 && parent != Some(want) {
            p(e.page, e.xref, format!("{} element {} has a /P that is not its parent in the tree", e.ty, e.xref));
        }
    }
    if let Some(max) = pt.arrays.keys().chain(pt.singles.keys()).max()
        && root.get_dict("ParentTreeNextKey")?.and_then(|v| v.as_int().ok()).is_none_or(|nk| nk <= *max)
    {
        p(None, root_x, format!("/ParentTreeNextKey is not above the highest key ({max})"));
    }
    Ok(out)
}

// ---- reading and editing context -----------------------------------------------------------------

struct Ctx {
    t: Tree,
    by_xref: HashMap<i32, usize>,
    index: HashMap<i32, u32>,
}

impl Ctx {
    fn load(doc: &PdfDocument) -> Result<Ctx> {
        let index = page_index(doc)?;
        let t = Tree::read(doc, &index)?;
        if !t.exists {
            return Err(bad("the document has no tags"));
        }
        if t.incomplete {
            return Err(bad("the tag tree is too deep or large, or damaged (a repeated element, a marked-content id out of range, content without a page), so it can't be edited"));
        }
        let by_xref = t.elems.iter().enumerate().filter(|(_, e)| e.xref != 0).map(|(i, e)| (e.xref, i)).collect();
        Ok(Ctx { t, by_xref, index })
    }

    fn find(&self, xref: i32) -> Result<usize> {
        self.by_xref.get(&xref).copied().ok_or_else(|| bad(format!("object {xref} is not a structure element")))
    }

    fn subtree(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut todo = vec![i];
        while let Some(e) = todo.pop() {
            out.push(e);
            todo.extend(self.t.elems[e].children.iter().copied());
        }
        out
    }

    fn parent_xref(&self, i: usize) -> i32 {
        self.t.elems[i].parent.map_or(ROOT, |p| self.t.elems[p].xref)
    }

    fn contains(&self, anc: usize, mut i: usize) -> bool {
        loop {
            if i == anc {
                return true;
            }
            match self.t.elems[i].parent {
                Some(p) => i = p,
                None => return false,
            }
        }
    }
}

fn obj_of(doc: &PdfDocument, parent: i32) -> Result<PdfObject> {
    match parent {
        ROOT => struct_root(doc),
        0 => Err(bad("the parent element is written directly inside its own parent, so it can't be edited")),
        x => Ok(doc.new_indirect(x, 0)?),
    }
}

/// Gives every element in the subtree that relies on an inherited page its own /Pg, so moving it
/// under a parent on another page doesn't move its content to that page.
fn pin_pages(doc: &PdfDocument, c: &Ctx, i: usize) -> Result<()> {
    for e in c.subtree(i) {
        let el = &c.t.elems[e];
        let (Some(pg), true) = (el.page, el.xref != 0) else { continue };
        let mut o = doc.new_indirect(el.xref, 0)?;
        if o.get_dict("Pg")?.is_none() {
            o.dict_put("Pg", doc.find_page(pg as i32)?)?;
        }
    }
    Ok(())
}

/// Takes the child with this object number out of `parent`'s kids.
fn detach(doc: &PdfDocument, parent: i32, child: i32) -> Result<()> {
    let mut p = obj_of(doc, parent)?;
    let kids = kids_of(&p)?;
    let before = kids.len();
    let kept: Vec<Kid> = kids.into_iter().filter(|k| !(matches!(k, Kid::Elem(_)) && k.xref() == child)).collect();
    if kept.len() == before {
        return Err(bad(format!("element {child} is not among the kids of its parent")));
    }
    set_kids(doc, &mut p, &kept)
}

fn attach(doc: &PdfDocument, parent: i32, child: i32, at: Option<usize>) -> Result<()> {
    let mut p = obj_of(doc, parent)?;
    let mut kids = kids_of(&p)?;
    let at = at.map_or(kids.len(), |a| a.min(kids.len()));
    kids.insert(at, Kid::Elem(doc.new_indirect(child, 0)?));
    set_kids(doc, &mut p, &kids)?;
    let mut c = doc.new_indirect(child, 0)?;
    c.dict_put("P", p)?;
    Ok(())
}

// ---- the tag tree as nodes -----------------------------------------------------------------------

/// One node of the tag tree, for the Tags panel.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TagNode {
    pub xref: i32,
    pub role: String,
    pub raw_role: String,
    pub page: Option<u32>,
    pub alt: Option<String>,
    pub actual_text: Option<String>,
    pub lang: Option<String>,
    pub id: Option<String>,
    /// A table header's scope: Row, Column or Both.
    pub scope: Option<String>,
    /// The object numbers of the header cells (TH) this cell lists in /Headers.
    pub headers: Vec<i32>,
    pub mcids: Vec<i32>,
    pub annots: Vec<i32>,
    pub editable: bool,
    pub children: Vec<TagNode>,
}

/// The tag tree, nested. With `page`, only the branches that hold content (or an annotation) on that
/// page. Empty on an untagged document.
pub fn tags(doc: &PdfDocument, page: Option<u32>) -> Result<Vec<TagNode>> {
    let index = page_index(doc)?;
    let t = Tree::read(doc, &index)?;
    if !t.exists {
        return Ok(Vec::new());
    }
    let ids: HashMap<&str, i32> = t.elems.iter().filter_map(|e| e.id.as_deref().map(|i| (i, e.xref))).collect();
    let on_page = |i: usize| -> bool {
        let Some(p) = page else { return true };
        let e = &t.elems[i];
        e.refs.iter().any(|(pg, _)| *pg == p)
            || e.annots.iter().any(|a| doc.find_page(p as i32).is_ok_and(|po| annots(&po).is_ok_and(|v| v.iter().any(|(x, _)| x == a))))
    };
    // Branches with something on the page, found bottom-up.
    let mut keep = vec![page.is_none(); t.elems.len()];
    if page.is_some() {
        for &i in t.preorder().iter().rev() {
            keep[i] = on_page(i) || t.elems[i].children.iter().any(|&c| keep[c]);
        }
    }
    fn build(t: &Tree, keep: &[bool], ids: &HashMap<&str, i32>, i: usize) -> TagNode {
        let e = &t.elems[i];
        TagNode {
            xref: e.xref,
            role: e.ty.clone(),
            raw_role: e.raw.clone(),
            page: e.page,
            alt: e.alt_text.clone(),
            actual_text: e.actual_text.clone(),
            lang: e.lang.clone(),
            id: e.id.clone(),
            scope: e.attrs.get("Scope").map(|s| s.trim_start_matches('/').to_owned()),
            headers: e.headers.iter().filter_map(|h| ids.get(h.as_str()).copied()).collect(),
            mcids: e.refs.iter().map(|(_, m)| *m).collect(),
            annots: e.annots.clone(),
            editable: e.xref != 0,
            children: e.children.iter().filter(|&&c| keep[c]).map(|&c| build(t, keep, ids, c)).collect(),
        }
    }
    Ok(t.roots.iter().filter(|&&r| keep[r]).map(|&r| build(&t, &keep, &ids, r)).collect())
}

// ---- change a node -------------------------------------------------------------------------------

fn check_role(c: &Ctx, role: &str) -> Result<()> {
    let ok_shape = !role.is_empty() && role.len() <= 64 && role.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    if !ok_shape || !(is_standard(role) || c.t.role_map.contains_key(role)) {
        return Err(bad(format!("\"{role}\" is not a structure type (P, H1-H6, Figure, Table, L, LI, ... or a type in the RoleMap)")));
    }
    Ok(())
}

/// Changes the role of an element (P, H1-H6, Figure, Table, L, LI, ...). `Artifact` takes the content
/// out of the tree (see [`artifact`]). The content stream's own tag name is left as it is.
pub fn set_role(doc: &mut PdfDocument, xref: i32, role: &str) -> Result<Vec<u32>> {
    let role = role.trim();
    let c = Ctx::load(doc)?;
    let i = c.find(xref)?;
    if role == "Artifact" {
        return artifact(doc, xref);
    }
    check_role(&c, role)?;
    if c.t.elems[i].raw == role {
        return Err(bad(format!("the element is already {role}")));
    }
    let mut o = doc.new_indirect(xref, 0)?;
    o.dict_put("S", PdfObject::new_name(role)?)?;
    Ok(c.t.elems[i].page.into_iter().collect())
}

/// What to change on a node. A field left out stays; an empty text removes it.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TagPatch {
    pub role: Option<String>,
    pub alt: Option<String>,
    pub actual_text: Option<String>,
    /// A BCP 47 language tag for the node's content.
    pub lang: Option<String>,
    /// Row, Column or Both (table header cells).
    pub scope: Option<String>,
    /// Object numbers of the header cells (TH) this cell is read with; they get an /ID if they have none.
    pub headers: Option<Vec<i32>>,
}

impl TagPatch {
    pub fn label(&self) -> String {
        match (&self.role, &self.alt, &self.actual_text, &self.lang, &self.scope, &self.headers) {
            (Some(r), None, None, None, None, None) => format!("Change tag to {}", r.trim()),
            (None, Some(_), None, None, None, None) => "Set tag alternate text".into(),
            (None, None, Some(_), None, None, None) => "Set tag actual text".into(),
            (None, None, None, Some(_), None, None) => "Set tag language".into(),
            (None, None, None, None, Some(_), None) | (None, None, None, None, None, Some(_)) => "Set table header attributes".into(),
            _ => "Edit tag".into(),
        }
    }
}

/// The table attribute dictionary of a node (`/A` with `/O /Table`), made when there is none.
fn table_attrs(doc: &PdfDocument, node: &mut PdfObject) -> Result<PdfObject> {
    let is_table = |d: &PdfObject| -> bool { name(d, "O").ok().flatten().as_deref() == Some("Table") };
    let new = |doc: &PdfDocument| -> Result<PdfObject> {
        let mut d = doc.new_dict()?;
        d.dict_put("O", PdfObject::new_name("Table")?)?;
        Ok(d)
    };
    match node.get_dict("A")? {
        None => {
            node.dict_put("A", new(doc)?)?;
            Ok(node.get_dict("A")?.ok_or_else(|| bad("attribute dictionary missing"))?)
        }
        Some(a) if a.is_array()? => {
            for x in a.array_iter()? {
                let x = x?;
                if x.is_dict()? && is_table(&x) {
                    return Ok(x);
                }
            }
            let mut a = a;
            a.array_push(new(doc)?)?;
            let n = a.len()?;
            Ok(a.get_array(n as i32 - 1)?.ok_or_else(|| bad("attribute dictionary missing"))?)
        }
        Some(a) if a.is_dict()? => {
            if is_table(&a) {
                return Ok(a);
            }
            let mut arr = doc.new_array()?;
            arr.array_push(a)?;
            arr.array_push(new(doc)?)?;
            node.dict_put("A", arr)?;
            let arr = node.get_dict("A")?.ok_or_else(|| bad("attribute dictionary missing"))?;
            Ok(arr.get_array(1)?.ok_or_else(|| bad("attribute dictionary missing"))?)
        }
        Some(_) => {
            node.dict_put("A", new(doc)?)?;
            Ok(node.get_dict("A")?.ok_or_else(|| bad("attribute dictionary missing"))?)
        }
    }
}

/// Applies a patch to one node.
pub fn edit_tag(doc: &mut PdfDocument, xref: i32, patch: &TagPatch) -> Result<Vec<u32>> {
    let c = Ctx::load(doc)?;
    let i = c.find(xref)?;
    let e = &c.t.elems[i];
    let mut o = doc.new_indirect(xref, 0)?;
    let mut changed = false;
    let mut touched: Vec<u32> = Vec::new();
    if let Some(role) = patch.role.as_deref() {
        if role.trim() == "Artifact" {
            return Err(bad("use mark as artifact to take content out of the tree"));
        }
        let role = role.trim();
        check_role(&c, role)?;
        if e.raw != role {
            o.dict_put("S", PdfObject::new_name(role)?)?;
            changed = true;
        }
    }
    let text_key = |key: &str, val: &Option<String>, cur: &Option<String>, doc: &PdfDocument, o: &mut PdfObject| -> Result<bool> {
        let Some(v) = val else { return Ok(false) };
        let v = v.trim();
        if v.is_empty() {
            if cur.is_some() || o.get_dict(key)?.is_some() {
                o.dict_delete(key)?;
                return Ok(true);
            }
            return Ok(false);
        }
        if cur.as_deref() == Some(v) {
            return Ok(false);
        }
        o.dict_put(key, text_obj(doc, v)?)?;
        Ok(true)
    };
    changed |= text_key("Alt", &patch.alt, &e.alt_text, doc, &mut o)?;
    changed |= text_key("ActualText", &patch.actual_text, &e.actual_text, doc, &mut o)?;
    if let Some(l) = patch.lang.as_deref() {
        let l = l.trim();
        if l.is_empty() {
            if e.lang.is_some() {
                o.dict_delete("Lang")?;
                changed = true;
            }
        } else {
            if !valid_lang(l) {
                return Err(bad(format!("\"{l}\" is not a language tag (for example en-US)")));
            }
            if e.lang.as_deref() != Some(l) {
                o.dict_put("Lang", PdfObject::new_string(l)?)?;
                changed = true;
            }
        }
    }
    if let Some(s) = patch.scope.as_deref() {
        let s = s.trim();
        let cur = e.attrs.get("Scope").map(|v| v.trim_start_matches('/'));
        if s.is_empty() {
            if cur.is_some() {
                let mut a = table_attrs(doc, &mut o)?;
                a.dict_delete("Scope")?;
                changed = true;
            }
        } else {
            if !matches!(s, "Row" | "Column" | "Both") {
                return Err(bad("scope is Row, Column or Both"));
            }
            if e.ty != "TH" {
                return Err(bad("only a header cell (TH) has a scope"));
            }
            if cur != Some(s) {
                let mut a = table_attrs(doc, &mut o)?;
                a.dict_put("Scope", PdfObject::new_name(s)?)?;
                changed = true;
            }
        }
    }
    if let Some(hs) = &patch.headers {
        if !matches!(e.ty.as_str(), "TD" | "TH") {
            return Err(bad("only a table cell (TD or TH) lists its headers"));
        }
        let mut used: HashSet<String> = c.t.elems.iter().filter_map(|x| x.id.clone()).collect();
        let mut ids = Vec::new();
        for &h in hs {
            let hi = c.find(h)?;
            if c.t.elems[hi].ty != "TH" {
                return Err(bad(format!("object {h} is a {} element, not a header cell (TH)", c.t.elems[hi].ty)));
            }
            let mut th = doc.new_indirect(h, 0)?;
            let id = match c.t.elems[hi].id.clone() {
                Some(id) => id,
                None => {
                    let mut id = format!("th{h}");
                    while used.contains(&id) {
                        id.push('x');
                    }
                    used.insert(id.clone());
                    th.dict_put("ID", PdfObject::new_string(&id)?)?;
                    changed = true;
                    id
                }
            };
            ids.push(id);
        }
        if hs.is_empty() {
            if !e.headers.is_empty() {
                let mut a = table_attrs(doc, &mut o)?;
                a.dict_delete("Headers")?;
                changed = true;
            }
        } else if ids != e.headers {
            let mut arr = doc.new_array()?;
            for id in &ids {
                arr.array_push(PdfObject::new_string(id)?)?;
            }
            let mut a = table_attrs(doc, &mut o)?;
            a.dict_put("Headers", arr)?;
            changed = true;
        }
    }
    if !changed {
        return Err(bad("nothing to change"));
    }
    touched.extend(e.page);
    Ok(touched)
}

/// Moves a node to be the `index`-th kid of `parent` (0: the top of the tree). The index counts the
/// parent's kids after the node has been taken out of its old place.
pub fn move_node(doc: &mut PdfDocument, xref: i32, parent: i32, index: usize) -> Result<Vec<u32>> {
    let c = Ctx::load(doc)?;
    let i = c.find(xref)?;
    let target = if parent == 0 { ROOT } else { c.t.elems[c.find(parent)?].xref };
    if parent != 0 {
        let pj = c.find(parent)?;
        if c.contains(i, pj) {
            return Err(bad("an element can't be moved into itself"));
        }
    }
    pin_pages(doc, &c, i)?;
    detach(doc, c.parent_xref(i), xref)?;
    attach(doc, target, xref, Some(index))?;
    Ok(c.t.elems[i].page.into_iter().collect())
}

// ---- reorder the regions of a page ---------------------------------------------------------------

/// The elements with content on `page`, in tree order (addressable ones only).
fn page_regions(c: &Ctx, page: u32) -> Vec<i32> {
    c.t.preorder()
        .into_iter()
        .filter(|&i| c.t.elems[i].xref != 0 && (c.t.elems[i].refs.iter().any(|(p, _)| *p == page) || !c.t.elems[i].annots.is_empty() && c.t.elems[i].page == Some(page)))
        .map(|i| c.t.elems[i].xref)
        .collect()
}

/// Indices of a longest strictly increasing run of `s`.
fn longest_increasing(s: &[usize]) -> Vec<usize> {
    let n = s.len();
    let (mut best, mut prev) = (vec![1usize; n], vec![usize::MAX; n]);
    for i in 0..n {
        for j in 0..i {
            if s[j] < s[i] && best[j] + 1 > best[i] {
                best[i] = best[j] + 1;
                prev[i] = j;
            }
        }
    }
    let Some(mut at) = (0..n).max_by_key(|&i| (best[i], std::cmp::Reverse(i))) else { return Vec::new() };
    let mut out = vec![at];
    while prev[at] != usize::MAX {
        at = prev[at];
        out.push(at);
    }
    out.reverse();
    out
}

/// Puts the regions of a page in a new reading order. `order` lists every region of the page (the
/// object numbers [`super::order::reading_order`] gives) in the order to read them. The regions that
/// already read in the right order relative to each other stay where they are; each of the others is
/// moved, as a whole element with its kids, to sit beside the one that should come before it, keeping
/// its place in the tree otherwise.
pub fn reorder(doc: &mut PdfDocument, page: u32, order: &[i32]) -> Result<Vec<u32>> {
    let c = Ctx::load(doc)?;
    if page as i32 >= doc.page_count()? {
        return Err(bad(format!("page {} is outside the document", page + 1)));
    }
    let cur = page_regions(&c, page);
    let want: HashSet<i32> = order.iter().copied().collect();
    if want.len() != order.len() || want != cur.iter().copied().collect::<HashSet<_>>() {
        return Err(bad("the order must list each region of the page exactly once"));
    }
    if cur == order {
        return Err(bad("the regions are already in that order"));
    }
    let target_pos: HashMap<i32, usize> = order.iter().enumerate().map(|(i, x)| (*x, i)).collect();
    let seq: Vec<usize> = cur.iter().map(|x| target_pos[x]).collect();
    let stay: HashSet<i32> = longest_increasing(&seq).into_iter().map(|i| cur[i]).collect();
    // Elements that move can hold content on other pages; their order there must not change.
    let mut spans: HashSet<u32> = HashSet::new();
    for &x in order {
        for e in c.subtree(c.find(x)?) {
            spans.extend(c.t.elems[e].refs.iter().map(|(p, _)| *p));
        }
    }
    spans.remove(&page);
    let before: Vec<(u32, Vec<i32>)> = spans.iter().map(|&p| (p, page_regions(&c, p))).collect();
    let mut parent_of: HashMap<i32, i32> = (0..c.t.elems.len()).filter(|&i| c.t.elems[i].xref != 0).map(|i| (c.t.elems[i].xref, c.parent_xref(i))).collect();
    let first_stay = order.iter().copied().find(|x| stay.contains(x)).ok_or_else(|| bad("nothing to anchor the new order to"))?;
    for (k, &x) in order.iter().enumerate() {
        if stay.contains(&x) {
            continue;
        }
        let (anchor, after) = if k == 0 { (first_stay, false) } else { (order[k - 1], true) };
        place(doc, &c, &mut parent_of, x, anchor, after)?;
    }
    // Read it back: the tree must now read in the order asked for.
    let c2 = Ctx::load(doc)?;
    if page_regions(&c2, page) != order {
        return Err(bad("these regions can't be put in that order by moving whole elements (one holds another)"));
    }
    for (p, was) in before {
        if page_regions(&c2, p) != was {
            return Err(bad(format!("a region also holds content on page {}; moving it would change the reading order there", p + 1)));
        }
    }
    Ok(vec![page])
}

/// The parents a type may sit under, for the types that may sit under only some.
fn needs_parent(ty: &str) -> Option<&'static [&'static str]> {
    Some(match ty {
        "TR" => &["Table", "THead", "TBody", "TFoot"],
        "TH" | "TD" => &["TR"],
        "THead" | "TBody" | "TFoot" => &["Table"],
        "LI" => &["L"],
        "Lbl" | "LBody" => &["LI", "TOCI", "BibEntry", "Note", "FENote"],
        "TOCI" => &["TOC"],
        _ => return None,
    })
}

/// Whether a parent of this type takes a kid of that type (table and list parts take only their own parts).
fn allows_kid(parent: &str, child: &str) -> bool {
    let free = matches!(child, "NonStruct" | "Artifact");
    free || match parent {
        "Table" => matches!(child, "TR" | "THead" | "TBody" | "TFoot" | "Caption"),
        "THead" | "TBody" | "TFoot" => child == "TR",
        "TR" => matches!(child, "TH" | "TD"),
        "L" => matches!(child, "LI" | "L" | "Caption"),
        "LI" => matches!(child, "Lbl" | "LBody"),
        _ => true,
    }
}

/// Whether a `child` element may be moved under a `parent` one without breaking table and list structure.
fn fits(parent: &str, child: &str) -> bool {
    allows_kid(parent, child) && needs_parent(child).is_none_or(|l| l.contains(&parent))
}

/// Moves `x` to be the sibling just before or after `anchor`.
fn place(doc: &PdfDocument, c: &Ctx, parent_of: &mut HashMap<i32, i32>, x: i32, anchor: i32, after: bool) -> Result<()> {
    let p = *parent_of.get(&anchor).ok_or_else(|| bad("a region is not in the tree"))?;
    let old = *parent_of.get(&x).ok_or_else(|| bad("a region is not in the tree"))?;
    // The anchor may not sit inside the element being moved.
    let mut q = anchor;
    while q != ROOT {
        if q == x {
            return Err(bad("these regions can't be put in that order by moving whole elements (one holds another)"));
        }
        q = *parent_of.get(&q).unwrap_or(&ROOT);
    }
    if old != p {
        let child = &c.t.elems[c.find(x)?].ty;
        let parent = if p == ROOT { "" } else { c.t.elems[c.find(p)?].ty.as_str() };
        if !fits(parent, child) {
            return Err(bad(if p == ROOT {
                format!("a {child} can't be moved to the top of the tree")
            } else {
                format!("a {child} can't be moved into a {parent}; it would break the table or list structure")
            }));
        }
    }
    pin_pages(doc, c, c.find(x)?)?;
    detach(doc, old, x)?;
    let kids = kids_of(&obj_of(doc, p)?)?;
    let pos = kids.iter().position(|k| matches!(k, Kid::Elem(_)) && k.xref() == anchor).ok_or_else(|| bad("a region is not among its parent's kids"))?;
    attach(doc, p, x, Some(pos + usize::from(after)))?;
    parent_of.insert(x, p);
    Ok(())
}

// ---- mark as artifact ----------------------------------------------------------------------------

/// Rewrites `/Tag <</MCID n>> BDC` to `/Artifact BMC` for these MCIDs on a page. True if the page changed.
fn artifact_content(doc: &mut PdfDocument, page: u32, mcids: &HashSet<i32>) -> Result<bool> {
    let mut page_obj = doc.find_page(page as i32)?;
    let res = page_obj.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
    let data = page_content(&page_obj)?;
    let mut out = Vec::with_capacity(data.len());
    let (mut at, mut n) = (0, 0);
    for op in content::parse(&data) {
        if op.is(b"BDC")
            && let Some(m) = mcid_of(&op, res.as_ref())?
            && mcids.contains(&m)
            && op.start >= at
        {
            out.extend_from_slice(&data[at..op.start]);
            out.extend_from_slice(b"/Artifact BMC");
            at = op.end;
            n += 1;
        }
    }
    if n == 0 {
        return Ok(false);
    }
    out.extend_from_slice(&data[at..]);
    let stm = doc.add_stream(&Buffer::from_bytes(&out)?, None, false)?;
    page_obj.dict_put("Contents", stm)?;
    Ok(true)
}

/// Marks an element's content (and its kids') as artifact: the marked content in the page becomes
/// `/Artifact BMC`, annotations lose their `/StructParent`, the element leaves the tree, and the
/// ParentTree forgets them. A screen reader no longer reads it.
pub fn artifact(doc: &mut PdfDocument, xref: i32) -> Result<Vec<u32>> {
    let c = Ctx::load(doc)?;
    let i = c.find(xref)?;
    let sub = c.subtree(i);
    let mut per_page: BTreeMap<u32, HashSet<i32>> = BTreeMap::new();
    let mut annot_xrefs = Vec::new();
    for &e in &sub {
        for &(p, m) in &c.t.elems[e].refs {
            per_page.entry(p).or_default().insert(m);
        }
        annot_xrefs.extend(c.t.elems[e].annots.iter().copied());
    }
    if per_page.is_empty() && annot_xrefs.is_empty() {
        return Err(bad("the element holds no content"));
    }
    if sub.iter().any(|&e| !c.t.elems[e].stm.is_empty()) {
        return Err(bad("part of its content is drawn by a form XObject, which can't be marked as artifact from here"));
    }
    let mut root = struct_root(doc)?;
    let mut pt = ParentTree::read_for_edit(&root)?;
    let mut touched = Vec::new();
    for (p, ms) in &per_page {
        if artifact_content(doc, *p, ms)? {
            touched.push(*p);
        }
    }
    detach(doc, c.parent_xref(i), xref)?;
    let mut key_of: HashMap<u32, i32> = HashMap::new();
    for p in per_page.keys() {
        if let Some(k) = page_key(&doc.find_page(*p as i32)?)? {
            key_of.insert(*p, k);
        }
    }
    for (p, ms) in &per_page {
        if let Some(k) = key_of.get(p) {
            for m in ms {
                pt.set_mcid(*k, *m, None)?;
            }
        }
    }
    for ax in &annot_xrefs {
        let mut a = doc.new_indirect(*ax, 0)?;
        if let Some(k) = a.get_dict("StructParent")?.and_then(|v| v.as_int().ok()) {
            pt.singles.remove(&k);
            a.dict_delete("StructParent")?;
        }
    }
    pt.write(doc, &mut root)?;
    Ok(touched)
}

// ---- merge and split -----------------------------------------------------------------------------

/// Every cell whose /Headers names `old` names `new` instead.
fn rewrite_headers(doc: &PdfDocument, c: &Ctx, old: &str, new: &str) -> Result<()> {
    for e in c.t.elems.iter().filter(|e| e.headers.iter().any(|h| h == old)) {
        if e.xref == 0 {
            return Err(bad("a cell that lists this header is written directly inside its parent, so its headers can't be updated"));
        }
        let mut ids: Vec<&str> = Vec::new();
        for h in &e.headers {
            let h = if h == old { new } else { h.as_str() };
            if !ids.contains(&h) {
                ids.push(h);
            }
        }
        let mut arr = doc.new_array()?;
        for id in ids {
            arr.array_push(PdfObject::new_string(id)?)?;
        }
        let mut o = doc.new_indirect(e.xref, 0)?;
        let mut a = table_attrs(doc, &mut o)?;
        a.dict_put("Headers", arr)?;
    }
    Ok(())
}

fn mcr(doc: &PdfDocument, page: &PdfObject, mcid: i32) -> Result<PdfObject> {
    let mut d = doc.new_dict()?;
    d.dict_put("Type", PdfObject::new_name("MCR")?)?;
    d.dict_put("Pg", page.clone())?;
    d.dict_put("MCID", PdfObject::new_int(mcid)?)?;
    Ok(d)
}

/// Merges element `from` into `into`: its content and kids move into `into` (ahead of its own when
/// `from` comes first in the tree, after them otherwise), and `from` leaves the tree. `into` keeps
/// its role and attributes. The ParentTree is pointed at `into`.
pub fn merge(doc: &mut PdfDocument, into: i32, from: i32) -> Result<Vec<u32>> {
    if into == from {
        return Err(bad("choose two different regions"));
    }
    let c = Ctx::load(doc)?;
    let (a, b) = (c.find(into)?, c.find(from)?);
    if c.contains(a, b) || c.contains(b, a) {
        return Err(bad("one region holds the other; they can't be merged"));
    }
    let mut root = struct_root(doc)?;
    let mut pt = ParentTree::read_for_edit(&root)?;
    let b_first = c.t.preorder().iter().position(|&i| i == b) < c.t.preorder().iter().position(|&i| i == a);
    let (ea, eb) = (&c.t.elems[a], &c.t.elems[b]);
    let target_page = ea.page.or(eb.page);
    let mut ao = doc.new_indirect(into, 0)?;
    if ea.page.is_none()
        && let Some(p) = eb.page
        && ao.get_dict("Pg")?.is_none()
    {
        ao.dict_put("Pg", doc.find_page(p as i32)?)?;
    }
    let bo = doc.new_indirect(from, 0)?;
    let mut moved = Vec::new();
    for k in kids_of(&bo)? {
        match k {
            Kid::Mcid(m) => {
                if eb.page == target_page || eb.page.is_none() {
                    moved.push(Kid::Mcid(m));
                } else if let Some(p) = eb.page {
                    moved.push(Kid::Mcr(mcr(doc, &doc.find_page(p as i32)?, m)?));
                }
            }
            Kid::Mcr(mut o) => {
                if o.get_dict("Pg")?.is_none()
                    && eb.page != target_page
                    && let Some(p) = eb.page
                {
                    o.dict_put("Pg", doc.find_page(p as i32)?)?;
                }
                moved.push(Kid::Mcr(o));
            }
            Kid::Objr(mut o) => {
                if o.get_dict("Pg")?.is_none()
                    && eb.page != target_page
                    && let Some(p) = eb.page
                {
                    o.dict_put("Pg", doc.find_page(p as i32)?)?;
                }
                moved.push(Kid::Objr(o));
            }
            Kid::Elem(mut e) => {
                if e.get_dict("Pg")?.is_none() && eb.page != target_page {
                    pin_pages(doc, &c, c.find(xref_of(&e)?).unwrap_or(b))?;
                }
                e.dict_put("P", ao.clone())?;
                moved.push(Kid::Elem(e));
            }
        }
    }
    let mut kids = kids_of(&ao)?;
    if b_first {
        moved.append(&mut kids);
        kids = moved;
    } else {
        kids.append(&mut moved);
    }
    set_kids(doc, &mut ao, &kids)?;
    // Cells that list `from` as a header must list what stands for it now.
    if let Some(bid) = &eb.id {
        match &ea.id {
            None => {
                if let Some(id) = bo.get_dict("ID")? {
                    ao.dict_put("ID", id)?;
                }
            }
            Some(aid) if aid != bid => rewrite_headers(doc, &c, bid, aid)?,
            Some(_) => {}
        }
    }
    detach(doc, c.parent_xref(b), from)?;
    if pt.repoint(from, into) {
        pt.write(doc, &mut root)?;
    }
    let mut touched: Vec<u32> = ea.refs.iter().chain(eb.refs.iter()).map(|(p, _)| *p).collect();
    touched.sort_unstable();
    touched.dedup();
    Ok(touched)
}

/// Splits an element in two at the content item with this MCID: that item and everything after it in
/// the element's kids move into a new element of the same role (or `role`) placed right after it.
/// Returns the pages touched; the new element's object number is in the tree afterwards.
pub fn split(doc: &mut PdfDocument, xref: i32, at_mcid: i32, role: Option<&str>) -> Result<Vec<u32>> {
    let c = Ctx::load(doc)?;
    let i = c.find(xref)?;
    let e = &c.t.elems[i];
    let role = role.map(str::trim).filter(|r| !r.is_empty());
    if let Some(r) = role {
        check_role(&c, r)?;
        if r == "Artifact" {
            return Err(bad("use mark as artifact to take content out of the tree"));
        }
    }
    if !valid_mcid(at_mcid) {
        return Err(bad(format!("{at_mcid} is not a usable marked-content id")));
    }
    let mut root = struct_root(doc)?;
    let mut pt = ParentTree::read_for_edit(&root)?;
    let mut eo = doc.new_indirect(xref, 0)?;
    let kids = kids_of(&eo)?;
    let mcid_at = |k: &Kid| -> Result<Option<i32>> {
        Ok(match k {
            Kid::Mcid(m) => Some(*m),
            Kid::Mcr(o) if o.get_dict("Stm")?.is_none() => o.get_dict("MCID")?.and_then(|v| v.as_int().ok()),
            _ => None,
        })
    };
    let mut at = None;
    for (k, kid) in kids.iter().enumerate() {
        if mcid_at(kid)? == Some(at_mcid) {
            at = Some(k);
            break;
        }
    }
    let at = at.ok_or_else(|| bad(format!("the element does not hold MCID {at_mcid}")))?;
    if at == 0 {
        return Err(bad("nothing would stay in the first part; split after its first item"));
    }
    let page = e.page;
    let mut nd = doc.new_dict()?;
    nd.dict_put("Type", PdfObject::new_name("StructElem")?)?;
    nd.dict_put("S", PdfObject::new_name(role.unwrap_or(&e.raw))?)?;
    nd.dict_put("P", obj_of(doc, c.parent_xref(i))?)?;
    if let Some(p) = page {
        nd.dict_put("Pg", doc.find_page(p as i32)?)?;
    }
    let mut n = doc.add_object(&nd)?;
    let nx = n.as_indirect()?;
    let (keep, moved) = kids.split_at(at);
    // The ParentTree follows the content that moved.
    for kid in moved {
        match kid {
            Kid::Mcid(m) => {
                let p = page.ok_or_else(|| bad("the element's page is unknown"))?;
                let key = page_key(&doc.find_page(p as i32)?)?.ok_or_else(|| bad("the page has no /StructParents; repair the ParentTree first"))?;
                pt.set_mcid(key, *m, Some(nx))?;
            }
            Kid::Mcr(o) => {
                let mcid = o.get_dict("MCID")?.and_then(|v| v.as_int().ok());
                if let (Some(m), Some(stm)) = (mcid, o.get_dict("Stm")?) {
                    // Content drawn by a form XObject is keyed on the stream.
                    let key = stm.get_dict("StructParents")?.and_then(|v| v.as_int().ok()).ok_or_else(|| bad("the form object has no /StructParents; repair the ParentTree first"))?;
                    pt.set_mcid(key, m, Some(nx))?;
                } else if let (Some(m), None) = (mcid, o.get_dict("Stm")?) {
                    let pg = match o.get_dict("Pg")? {
                        Some(p) if p.is_indirect()? => c.index.get(&p.as_indirect()?).copied(),
                        _ => page,
                    };
                    let p = pg.ok_or_else(|| bad("the content's page is unknown"))?;
                    let key = page_key(&doc.find_page(p as i32)?)?.ok_or_else(|| bad("the page has no /StructParents; repair the ParentTree first"))?;
                    pt.set_mcid(key, m, Some(nx))?;
                }
            }
            Kid::Objr(o) => {
                if let Some(k) = o.get_dict("Obj")?.and_then(|a| a.get_dict("StructParent").ok().flatten()).and_then(|v| v.as_int().ok()) {
                    pt.singles.insert(k, nx);
                }
            }
            Kid::Elem(child) => {
                let mut child = child.clone();
                child.dict_put("P", n.clone())?;
            }
        }
    }
    set_kids(doc, &mut eo, keep)?;
    set_kids(doc, &mut n, moved)?;
    let parent = c.parent_xref(i);
    let mut po = obj_of(doc, parent)?;
    let mut siblings = kids_of(&po)?;
    let pos = siblings.iter().position(|k| matches!(k, Kid::Elem(_)) && k.xref() == xref).ok_or_else(|| bad("the element is not among its parent's kids"))?;
    siblings.insert(pos + 1, Kid::Elem(n.clone()));
    set_kids(doc, &mut po, &siblings)?;
    pt.write(doc, &mut root)?;
    Ok(page.into_iter().collect())
}

// ---- repair --------------------------------------------------------------------------------------

/// Rebuilds the ParentTree from the tag tree: the page keys, each MCID's element and each annotation's
/// key. Keys of content outside pages (form XObjects) are kept. Refuses when it is consistent already.
pub fn repair_parent_tree(doc: &mut PdfDocument) -> Result<Vec<u32>> {
    let c = Ctx::load(doc)?;
    let mut root = struct_root(doc)?;
    let mut pt = ParentTree::read_for_edit(&root)?;
    if problems(doc)?.is_empty() {
        return Err(bad("the ParentTree is already consistent with the tags"));
    }
    let n = doc.page_count()?.max(0) as u32;
    let mut next = root.get_dict("ParentTreeNextKey")?.and_then(|v| v.as_int().ok()).unwrap_or(0).max(pt.arrays.keys().chain(pt.singles.keys()).max().map_or(0, |k| k.saturating_add(1)));
    // A key serves one page, one form or one annotation: a key found on two of them is given to the
    // first, and the others get fresh ones.
    let mut claimed: HashSet<i32> = HashSet::new();
    let mut claim = |existing: Option<i32>, next: &mut i32| -> Result<(i32, bool)> {
        if let Some(k) = existing.filter(|k| claimed.insert(*k)) {
            return Ok((k, false));
        }
        let k = *next;
        *next = next.checked_add(1).ok_or_else(|| bad("no free ParentTree key"))?;
        claimed.insert(k);
        Ok((k, true))
    };
    let mut touched = Vec::new();
    // MCIDs are bounded by the tree reader, but the arrays they size are bounded here too.
    let mut budget = MAX_ENTRIES.saturating_sub(pt.entries());
    let rebuild = |pt: &mut ParentTree, key: i32, rows: BTreeMap<i32, i32>, budget: &mut usize| -> Result<bool> {
        let size = rows.keys().max().map_or(0, |m| *m as usize + 1);
        if size > *budget {
            return Err(bad("the ParentTree would be too large to write"));
        }
        *budget -= size;
        let mut arr = vec![None; size];
        for (m, x) in rows {
            arr[m as usize] = Some(x);
        }
        Ok(if pt.arrays.get(&key) != Some(&arr) {
            pt.arrays.insert(key, arr);
            true
        } else {
            false
        })
    };
    for p in 0..n {
        let mut page = doc.find_page(p as i32)?;
        let mut rows: BTreeMap<i32, i32> = BTreeMap::new();
        for e in &c.t.elems {
            for &(pg, m) in &e.refs {
                if pg == p && e.xref != 0 {
                    rows.insert(m, e.xref);
                }
            }
        }
        let existing = page_key(&page)?;
        if existing.is_none() && rows.is_empty() {
            continue;
        }
        let (key, new) = claim(existing, &mut next)?;
        if new {
            page.dict_put("StructParents", PdfObject::new_int(key)?)?;
        }
        if rebuild(&mut pt, key, rows, &mut budget)? {
            touched.push(p);
        }
    }
    // Content drawn by form XObjects is keyed on the stream.
    let mut by_stream: BTreeMap<i32, (PdfObject, BTreeMap<i32, i32>)> = BTreeMap::new();
    for e in c.t.elems.iter().filter(|e| e.xref != 0) {
        for &(stm, m) in &e.stm {
            let obj = doc.new_indirect(stm, 0)?;
            by_stream.entry(stm).or_insert_with(|| (obj, BTreeMap::new())).1.insert(m, e.xref);
        }
    }
    for (_, (mut obj, rows)) in by_stream {
        let (key, new) = claim(obj.get_dict("StructParents")?.and_then(|v| v.as_int().ok()), &mut next)?;
        if new {
            obj.dict_put("StructParents", PdfObject::new_int(key)?)?;
        }
        rebuild(&mut pt, key, rows, &mut budget)?;
    }
    for (&ax, &owner) in &c.t.annot_owner {
        let ox = c.t.elems[owner].xref;
        if ox == 0 {
            continue;
        }
        let mut a = doc.new_indirect(ax, 0)?;
        let (key, new) = claim(a.get_dict("StructParent")?.and_then(|v| v.as_int().ok()), &mut next)?;
        if new {
            a.dict_put("StructParent", PdfObject::new_int(key)?)?;
        }
        pt.singles.insert(key, ox);
    }
    // Each element's /P is its parent in the tree.
    let root_x = xref_of(&root)?;
    for (i, e) in c.t.elems.iter().enumerate() {
        let want = match c.t.elems[i].parent {
            Some(pi) => c.t.elems[pi].xref,
            None => root_x,
        };
        if e.xref == 0 || want == 0 {
            continue;
        }
        let mut o = doc.new_indirect(e.xref, 0)?;
        let have = o.get_dict("P")?.filter(|x| x.is_indirect().unwrap_or(false)).and_then(|x| x.as_indirect().ok());
        if have != Some(want) {
            o.dict_put("P", doc.new_indirect(want, 0)?)?;
            touched.extend(e.page);
        }
    }
    touched.sort_unstable();
    touched.dedup();
    // Entries for elements that are not in the tree any more.
    let live: HashSet<i32> = c.by_xref.keys().copied().collect();
    for a in pt.arrays.values_mut() {
        for e in a.iter_mut().filter(|e| e.is_some_and(|x| !live.contains(&x))) {
            *e = None;
        }
    }
    pt.singles.retain(|_, x| live.contains(x));
    pt.write(doc, &mut root)?;
    Ok(touched)
}
