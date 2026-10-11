//! Where findings are on the page, so the UI's "Show" can highlight them: an annotation's rectangle
//! and the bounds of an element's marked content. Boxes are `[x0, y0, x1, y1]` in page points,
//! origin top left, y down (the viewer's page space).
//!
//! Marked content is located by what it paints: images, inline images, form XObjects (their
//! `/BBox`) and filled or stroked paths. Text has no extent here (the lexer drops strings), so a
//! text-only element gets no box and "Show" goes to its page.

use super::autotag::{UNIT, bbox_of};
use super::content::{self, Op, Operand};
use super::tree::{Tree, name};
use super::{Result, page_content};
use mupdf::pdf::PdfObject;
use mupdf::{Matrix, Rect};
use std::cell::RefCell;
use std::collections::HashMap;

pub type Box4 = [f32; 4];

fn to_box(r: Rect) -> Option<Box4> {
    let b = [r.x0, r.y0, r.x1, r.y1];
    (b.iter().all(|v| v.is_finite()) && r.x1 > r.x0 && r.y1 > r.y0 && r.x1 - r.x0 < 1.0e5 && r.y1 - r.y0 < 1.0e5).then_some(b)
}

fn union(a: Option<Rect>, b: Rect) -> Rect {
    match a {
        None => b,
        Some(a) => Rect { x0: a.x0.min(b.x0), y0: a.y0.min(b.y0), x1: a.x1.max(b.x1), y1: a.y1.max(b.y1) },
    }
}

fn rect_of(o: &PdfObject, key: &str) -> Result<Option<Rect>> {
    let Some(a) = o.get_dict(key)?.filter(|a| a.is_array().unwrap_or(false) && a.len().unwrap_or(0) == 4) else { return Ok(None) };
    let v: Vec<f32> = (0..4).map(|i| a.get_array(i).ok().flatten().and_then(|n| n.as_float().ok()).unwrap_or(0.0)).collect();
    Ok(Some(Rect { x0: v[0].min(v[2]), y0: v[1].min(v[3]), x1: v[0].max(v[2]), y1: v[1].max(v[3]) }))
}

/// An annotation's `/Rect` in page space.
pub fn annot_box(page: &PdfObject, annot: &PdfObject) -> Result<Option<Box4>> {
    let Some(r) = rect_of(annot, "Rect")? else { return Ok(None) };
    Ok(to_box(bbox_of(&page.page_ctm()?, r)))
}

fn mcid_of(op: &Op, res: Option<&PdfObject>) -> Result<Option<i32>> {
    Ok(match op.operands.get(1) {
        Some(d @ Operand::Dict(_)) => d.get(b"MCID").map(|m| m.num() as i32),
        Some(Operand::Name(n)) => match res {
            Some(r) => match r.get_dict("Properties")?.filter(|p| p.is_dict().unwrap_or(false)) {
                Some(p) => match p.get_dict(String::from_utf8_lossy(n).into_owned())? {
                    Some(d) if d.is_dict()? => d.get_dict("MCID")?.map(|m| m.as_int()).transpose()?,
                    _ => None,
                },
                None => None,
            },
            None => None,
        },
        _ => None,
    })
}

/// The bounds of the painted marked content of one page, by MCID (top-level content only).
pub fn mcid_bounds(page: &PdfObject) -> Result<HashMap<i32, Box4>> {
    let page_ctm = page.page_ctm()?;
    let res = page.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
    let ops = content::parse(&page_content(page)?);
    let xobject = |n: &[u8]| -> Result<Option<PdfObject>> {
        let Some(r) = &res else { return Ok(None) };
        let Some(x) = r.get_dict("XObject")? else { return Ok(None) };
        Ok(x.get_dict(String::from_utf8_lossy(n).into_owned())?)
    };
    let mut out: HashMap<i32, Rect> = HashMap::new();
    let mut ctm = page_ctm.clone();
    let mut stack: Vec<Matrix> = Vec::new();
    let mut marks: Vec<Option<i32>> = Vec::new();
    let mut path: Option<Rect> = None;
    for op in &ops {
        let cur = marks.iter().rev().find_map(|m| *m);
        let mut add = |r: Rect| {
            if let Some(m) = cur {
                let u = union(out.get(&m).copied(), r);
                out.insert(m, u);
            }
        };
        match op.name.as_slice() {
            b"q" => stack.push(ctm.clone()),
            b"Q" => ctm = stack.pop().unwrap_or_else(|| page_ctm.clone()),
            b"cm" => ctm = content::matrix(op) * &ctm,
            b"BMC" => marks.push(None),
            b"BDC" => marks.push(mcid_of(op, res.as_ref())?),
            b"EMC" => {
                marks.pop();
            }
            b"m" | b"l" | b"c" | b"v" | b"y" | b"re" => {
                let pts: Vec<(f32, f32)> = match op.name.as_slice() {
                    b"re" => vec![(op.n(0), op.n(1)), (op.n(0) + op.n(2), op.n(1) + op.n(3))],
                    b"m" | b"l" => vec![(op.n(0), op.n(1))],
                    b"c" => vec![(op.n(0), op.n(1)), (op.n(2), op.n(3)), (op.n(4), op.n(5))],
                    _ => vec![(op.n(0), op.n(1)), (op.n(2), op.n(3))],
                };
                for (x, y) in pts {
                    let (x, y) = ctm.transform_xy(x, y);
                    path = Some(union(path, Rect { x0: x, y0: y, x1: x, y1: y }));
                }
            }
            b"n" => path = None,
            _ if content::paints_path(op) => {
                if let Some(r) = path.take() {
                    add(r);
                }
            }
            b"BI" => add(bbox_of(&ctm, UNIT)),
            b"Do" => {
                let Some(x) = op.operands.first().and_then(|o| o.name()).map(xobject).transpose()?.flatten() else { continue };
                match name(&x, "Subtype")?.as_deref() {
                    Some("Image") => add(bbox_of(&ctm, UNIT)),
                    Some("Form") => {
                        let m = match x.get_dict("Matrix")? {
                            Some(a) if a.is_array()? && a.len()? == 6 => {
                                let v: Vec<f32> = (0..6).map(|i| a.get_array(i).ok().flatten().and_then(|n| n.as_float().ok()).unwrap_or(0.0)).collect();
                                Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])
                            }
                            _ => Matrix::IDENTITY,
                        };
                        let bb = rect_of(&x, "BBox")?.unwrap_or(UNIT);
                        add(bbox_of(&(m * &ctm), bb));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(out.into_iter().filter_map(|(m, r)| to_box(r).map(|b| (m, b))).collect())
}

/// Finds boxes for tree elements, parsing each page's content at most once.
pub struct Locator<'a> {
    pages: &'a [PdfObject],
    tree: &'a Tree,
    cache: RefCell<HashMap<u32, HashMap<i32, Box4>>>,
}

impl<'a> Locator<'a> {
    pub fn new(pages: &'a [PdfObject], tree: &'a Tree) -> Self {
        Locator { pages, tree, cache: RefCell::new(HashMap::new()) }
    }

    /// The union of the bounds of element `i`'s own marked content and its descendants'.
    pub fn element(&self, i: usize) -> Option<Box4> {
        let mut acc: Option<Rect> = None;
        let mut todo = vec![i];
        let mut seen = 0;
        while let Some(e) = todo.pop() {
            seen += 1;
            if seen > 256 {
                break;
            }
            let el = &self.tree.elems[e];
            if let (Some(p), false) = (el.page, el.mcids.is_empty()) {
                let mut cache = self.cache.borrow_mut();
                if !cache.contains_key(&p) {
                    let b = self.pages.get(p as usize).and_then(|pg| mcid_bounds(pg).ok()).unwrap_or_default();
                    cache.insert(p, b);
                }
                for m in &el.mcids {
                    if let Some(b) = cache[&p].get(m) {
                        acc = Some(union(acc, Rect { x0: b[0], y0: b[1], x1: b[2], y1: b[3] }));
                    }
                }
            }
            todo.extend(el.children.iter().copied());
        }
        acc.and_then(to_box)
    }
}
