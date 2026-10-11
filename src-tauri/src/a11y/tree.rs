// Adapted from pdfcraft (MIT OR Apache-2.0), crates/a11y/src/structure.rs @ b8ddf71
//! The structure tree (tags) as the checker reads it: element types through the role map,
//! alternate text, attributes, marked-content references in tree order, and annotation
//! references (ISO 32000-2 §14.7-14.8).

use super::Result;
use mupdf::pdf::{PdfDocument, PdfObject};
use std::collections::{HashMap, HashSet};

/// Standard structure types (PDF 1.7 and 2.0); anything else goes through the role map.
const STANDARD: &[&str] = &[
    "Document", "DocumentFragment", "Part", "Art", "Sect", "Div", "BlockQuote", "Caption", "TOC", "TOCI", "Index",
    "NonStruct", "Private", "Aside", "Title", "FENote", "P", "H", "H1", "H2", "H3", "H4", "H5", "H6", "L", "LI", "Lbl",
    "LBody", "Table", "TR", "TH", "TD", "THead", "TBody", "TFoot", "Span", "Quote", "Note", "Reference", "BibEntry",
    "Code", "Link", "Annot", "Ruby", "RB", "RT", "RP", "Warichu", "WT", "WP", "Figure", "Formula", "Form", "Sub", "Em",
    "Strong", "Artifact",
];

/// Elements beyond this are not read (a guard against absurd trees).
const MAX_ELEMENTS: usize = 500_000;
const MAX_DEPTH: usize = 256;

#[derive(Clone, Debug, Default)]
pub struct Elem {
    /// The standard type after the role map (or the type as written if it maps to nothing).
    pub ty: String,
    /// Object number (0 for a direct element).
    pub xref: i32,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Has non-empty /Alt or /ActualText.
    pub alt: bool,
    /// Marked content or an object reference directly under it.
    pub content: bool,
    /// Object numbers of the annotations it references (OBJR).
    pub annots: Vec<i32>,
    /// 0-based page (its own /Pg or inherited).
    pub page: Option<u32>,
    /// `/Summary`, `/Scope`, `/ColSpan`... from its attribute objects, as written.
    pub attrs: HashMap<String, String>,
}

#[derive(Debug, Default)]
pub struct Tree {
    /// A /StructTreeRoot exists.
    pub exists: bool,
    pub elems: Vec<Elem>,
    /// Top-level elements in order.
    pub roots: Vec<usize>,
    /// Marked-content references `(page, mcid)` in tree (reading) order. References into form
    /// XObjects (`/Stm`) are left out.
    pub order: Vec<(u32, i32)>,
    /// Object numbers of every annotation an OBJR points at, with the referencing element.
    pub annot_owner: HashMap<i32, usize>,
}

pub fn xref_of(o: &PdfObject) -> Result<i32> {
    Ok(if o.is_indirect()? { o.as_indirect()? } else { 0 })
}

pub fn name(o: &PdfObject, key: &str) -> Result<Option<String>> {
    Ok(match o.get_dict(key)? {
        Some(v) if v.is_name()? => Some(String::from_utf8_lossy(&v.as_name()?).into_owned()),
        _ => None,
    })
}

/// A text string value, trimmed; `None` when absent or blank.
pub fn text(o: &PdfObject, key: &str) -> Result<Option<String>> {
    Ok(match o.get_dict(key)? {
        Some(v) if v.is_string()? => Some(v.as_string().unwrap_or_default().trim().to_owned()).filter(|s| !s.is_empty()),
        _ => None,
    })
}

/// Page object number → 0-based index.
pub fn page_index(doc: &PdfDocument) -> Result<HashMap<i32, u32>> {
    let mut m = HashMap::new();
    for p in 0..doc.page_count()? {
        m.insert(xref_of(&doc.find_page(p)?)?, p as u32);
    }
    Ok(m)
}

impl Tree {
    pub fn read(doc: &PdfDocument, pages: &HashMap<i32, u32>) -> Result<Tree> {
        let mut t = Tree::default();
        let cat = doc.catalog()?;
        let Some(root) = cat.get_dict("StructTreeRoot")? else { return Ok(t) };
        if !root.is_dict()? {
            return Ok(t);
        }
        t.exists = true;
        let mut role_map = HashMap::new();
        if let Some(rm) = root.get_dict("RoleMap")?.filter(|r| r.is_dict().unwrap_or(false)) {
            for kv in rm.dict_iter()? {
                let (k, v) = kv?;
                if v.is_name()? {
                    role_map.insert(String::from_utf8_lossy(&k.as_name()?).into_owned(), String::from_utf8_lossy(&v.as_name()?).into_owned());
                }
            }
        }
        let mut r = Reader { pages, role_map, seen: HashSet::new() };
        if let Some(k) = root.get_dict("K")? {
            let mut roots = Vec::new();
            r.kids(&mut t, &k, None, None, &mut roots, 0)?;
            t.roots = roots;
        }
        Ok(t)
    }

    /// Elements in document (pre-)order.
    pub fn preorder(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.elems.len());
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            out.push(i);
            stack.extend(self.elems[i].children.iter().rev().copied());
        }
        out
    }

    pub fn ancestors(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(self.elems[i].parent, move |p| self.elems[*p].parent)
    }

    pub fn is(&self, i: usize, ty: &str) -> bool {
        self.elems[i].ty == ty
    }

    pub fn parent_is(&self, i: usize, types: &[&str]) -> bool {
        self.elems[i].parent.is_some_and(|p| types.contains(&self.elems[p].ty.as_str()))
    }

    /// The rows of a table (through THead/TBody/TFoot, not into nested tables).
    pub fn table_rows(&self, table: usize) -> Vec<usize> {
        let mut rows = Vec::new();
        for &k in &self.elems[table].children {
            match self.elems[k].ty.as_str() {
                "TR" => rows.push(k),
                "THead" | "TBody" | "TFoot" => rows.extend(self.elems[k].children.iter().copied().filter(|r| self.is(*r, "TR"))),
                _ => {}
            }
        }
        rows
    }
}

struct Reader<'a> {
    pages: &'a HashMap<i32, u32>,
    role_map: HashMap<String, String>,
    seen: HashSet<i32>,
}

impl Reader<'_> {
    fn standard(&self, ty: &str) -> String {
        let mut t = ty.to_owned();
        for _ in 0..12 {
            if STANDARD.contains(&t.as_str()) {
                return t;
            }
            match self.role_map.get(&t) {
                Some(n) if *n != t => t = n.clone(),
                _ => break,
            }
        }
        t
    }

    fn page_of(&self, d: &PdfObject) -> Result<Option<u32>> {
        Ok(match d.get_dict("Pg")? {
            Some(p) if p.is_indirect()? => self.pages.get(&p.as_indirect()?).copied(),
            _ => None,
        })
    }

    fn kids(&mut self, t: &mut Tree, k: &PdfObject, parent: Option<usize>, page: Option<u32>, out: &mut Vec<usize>, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH || t.elems.len() >= MAX_ELEMENTS {
            return Ok(());
        }
        if k.is_indirect()? && !self.seen.insert(k.as_indirect()?) {
            return Ok(());
        }
        if k.is_array()? {
            for x in k.array_iter()? {
                self.kids(t, &x?, parent, page, out, depth + 1)?;
            }
        } else if k.is_int()? {
            if let Some(p) = parent {
                t.elems[p].content = true;
                if let Some(pg) = page {
                    t.order.push((pg, k.as_int()?));
                }
            }
        } else if k.is_dict()? {
            self.dict(t, k, parent, page, out, depth)?;
        }
        Ok(())
    }

    fn dict(&mut self, t: &mut Tree, d: &PdfObject, parent: Option<usize>, page: Option<u32>, out: &mut Vec<usize>, depth: usize) -> Result<()> {
        match name(d, "Type")?.as_deref() {
            Some("MCR") => {
                if let Some(p) = parent {
                    t.elems[p].content = true;
                    let pg = self.page_of(d)?.or(page);
                    if let (Some(pg), Some(m), None) = (pg, d.get_dict("MCID")?, d.get_dict("Stm")?) {
                        t.order.push((pg, m.as_int()?));
                    }
                }
                return Ok(());
            }
            Some("OBJR") => {
                if let Some(p) = parent {
                    t.elems[p].content = true;
                    if let Some(o) = d.get_dict("Obj")?.filter(|o| o.is_indirect().unwrap_or(false))
                        && o.get_dict("Subtype")?.is_some()
                        && o.get_dict("Rect")?.is_some()
                    {
                        let n = o.as_indirect()?;
                        t.elems[p].annots.push(n);
                        t.annot_owner.insert(n, p);
                    }
                }
                return Ok(());
            }
            _ => {}
        }
        let Some(s) = name(d, "S")? else { return Ok(()) };
        let page = self.page_of(d)?.or(page);
        let mut attrs = HashMap::new();
        if let Some(a) = d.get_dict("A")? {
            let list: Vec<PdfObject> = if a.is_array()? { a.array_iter()?.collect::<std::result::Result<_, _>>()? } else { vec![a] };
            for x in list.into_iter().filter(|x| x.is_dict().unwrap_or(false)) {
                for kv in x.dict_iter()? {
                    let (k, v) = kv?;
                    let val = if v.is_string()? { v.as_string().unwrap_or_default() } else { v.to_string() };
                    attrs.insert(String::from_utf8_lossy(&k.as_name()?).into_owned(), val);
                }
            }
        }
        let i = t.elems.len();
        let alt = text(d, "Alt")?.is_some() || text(d, "ActualText")?.is_some();
        t.elems.push(Elem { ty: self.standard(&s), xref: xref_of(d)?, parent, alt, page, attrs, ..Elem::default() });
        if let Some(p) = parent {
            t.elems[p].children.push(i);
        }
        out.push(i);
        if let Some(k) = d.get_dict("K")? {
            let mut ignored = Vec::new();
            self.kids(t, &k, Some(i), page, &mut ignored, depth + 1)?;
        }
        Ok(())
    }
}
