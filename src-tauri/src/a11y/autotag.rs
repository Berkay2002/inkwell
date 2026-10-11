//! Autotag (best effort): a basic structure tree for an untagged document, from MuPDF's
//! structured text. Headings come from font size (sizes clearly above the body text, largest
//! first, as H1-H6), blocks become paragraphs, blocks whose lines start with a bullet or a number
//! become lists (L > LI > LBody), images become figures (without alternate text: the checker
//! then asks for it), link and widget annotations become Link and Form elements, and every
//! other painting (rules, fills, shadings) is marked as an artifact.
//!
//! Content is tied to the tags by rewriting each page's content stream: every run of text that
//! falls on one structured-text line is wrapped in `/Tag <</MCID n>> BDC ... EMC`, every image in
//! a Figure's marked content, every path in `/Artifact BMC ... EMC`. Nothing is moved or redrawn,
//! so the page looks and extracts exactly as before. Content that is already marked is left as
//! it is. The result is labelled best effort: headings, lists and order are guesses to review.

use super::content::{self, Op, TextPos};
use super::tree::name;
use super::{Result, annots, bad, page_content};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::text_page::{TextBlockType, TextPageFlags};
use mupdf::{Buffer, Matrix, Rect};
use std::collections::HashMap;
use std::sync::LazyLock;

/// A heading is at least this much larger than the body text.
const HEADING_RATIO: f32 = 1.15;
/// Longer blocks are never headings.
const HEADING_MAX_CHARS: usize = 200;
const HEADING_MAX_LINES: usize = 3;

static LIST_MARK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\s*(?:[•◦▪‣●■·○\u{2013}\u{2014}*-]\s|[•◦▪‣●■○]|\(?[0-9]{1,3}[.)]\s|\(?[a-zA-Z][.)]\s|\(?[ivxIVX]{1,5}[.)]\s)")
        .expect("valid regex")
});

struct Line {
    bbox: Rect,
    block: usize,
    text: String,
    size: f32,
}

struct PageText {
    lines: Vec<Line>,
    blocks: usize,
}

fn page_text(doc: &PdfDocument, p: i32) -> Result<PageText> {
    let page = doc.load_pdf_page(p)?;
    let tp = page.to_text_page(TextPageFlags::empty())?;
    let mut lines = Vec::new();
    let mut blocks = 0;
    for b in tp.blocks() {
        if b.r#type() != TextBlockType::Text {
            continue;
        }
        let mut any = false;
        for l in b.lines() {
            let mut text = String::new();
            let mut size = 0f32;
            for ch in l.chars() {
                size = size.max(ch.size());
                if let Some(c) = ch.char() {
                    text.push(c);
                }
            }
            if text.trim().is_empty() {
                continue;
            }
            any = true;
            lines.push(Line { bbox: l.bounds(), block: blocks, text, size });
        }
        if any {
            blocks += 1;
        }
    }
    Ok(PageText { lines, blocks })
}

/// Font sizes in half points.
fn half(s: f32) -> i32 {
    (s * 2.0).round() as i32
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Line(usize),
    /// Text that matches no structured-text line.
    Loose,
    Figure(Rect),
    /// A form XObject with text in it.
    FormText(Rect),
    Artifact,
}

#[derive(Debug)]
struct Run {
    start: usize,
    end: usize,
    target: Target,
}

fn bbox_of(m: &Matrix, r: Rect) -> Rect {
    let pts = [m.transform_xy(r.x0, r.y0), m.transform_xy(r.x1, r.y0), m.transform_xy(r.x0, r.y1), m.transform_xy(r.x1, r.y1)];
    let mut b = Rect { x0: f32::MAX, y0: f32::MAX, x1: f32::MIN, y1: f32::MIN };
    for (x, y) in pts {
        b.x0 = b.x0.min(x);
        b.y0 = b.y0.min(y);
        b.x1 = b.x1.max(x);
        b.y1 = b.y1.max(y);
    }
    b
}

const UNIT: Rect = Rect { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 };

/// The line a point (page space) falls on: inside a line's box (1 pt slack), else the nearest
/// one within 12 pt.
fn line_at(lines: &[Line], x: f32, y: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, l) in lines.iter().enumerate() {
        let b = l.bbox;
        let dx = (b.x0 - 1.0 - x).max(x - b.x1 - 1.0).max(0.0);
        let dy = (b.y0 - 1.0 - y).max(y - b.y1 - 1.0).max(0.0);
        let d = dx.hypot(dy);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.filter(|(d, _)| *d <= 12.0).map(|(_, i)| i)
}

/// Walks a page's content and returns the runs to mark, in content order.
fn runs(ops: &[Op], res: Option<&PdfObject>, page_ctm: Matrix, lines: &[Line]) -> Result<Vec<Run>> {
    let mut out = Vec::new();
    let mut ctm = page_ctm.clone();
    let mut stack: Vec<Matrix> = Vec::new();
    let mut text = TextPos::default();
    let mut depth = 0usize;
    let mut open: Option<Run> = None;
    let mut path_start: Option<usize> = None;
    let close = |open: &mut Option<Run>, out: &mut Vec<Run>| {
        if let Some(r) = open.take() {
            out.push(r);
        }
    };
    let xobject = |n: &[u8]| -> Result<Option<PdfObject>> {
        let Some(r) = res else { return Ok(None) };
        let Some(x) = r.get_dict("XObject")? else { return Ok(None) };
        Ok(x.get_dict(String::from_utf8_lossy(n).into_owned())?)
    };
    for op in ops {
        match op.name.as_slice() {
            b"BMC" | b"BDC" => {
                close(&mut open, &mut out);
                depth += 1;
            }
            b"EMC" => {
                close(&mut open, &mut out);
                depth = depth.saturating_sub(1);
            }
            b"q" => stack.push(ctm.clone()),
            b"Q" => ctm = stack.pop().unwrap_or_else(|| page_ctm.clone()),
            b"cm" => ctm = content::matrix(op) * &ctm,
            b"BT" => text.apply(op),
            b"ET" => close(&mut open, &mut out),
            _ if content::shows(op) => {
                text.apply(op);
                if depth > 0 {
                    continue;
                }
                let (x, y) = (&text.tm * &ctm).transform_xy(0.0, 0.0);
                let target = line_at(lines, x, y).map_or(Target::Loose, Target::Line);
                match &mut open {
                    Some(r) if r.target == target => r.end = op.end,
                    _ => {
                        close(&mut open, &mut out);
                        open = Some(Run { start: op.start, end: op.end, target });
                    }
                }
            }
            b"Td" | b"TD" | b"Tm" | b"T*" | b"TL" => text.apply(op),
            _ if content::builds_path(op) => {
                path_start.get_or_insert(op.start);
            }
            _ if content::paints_path(op) => {
                let start = path_start.take().unwrap_or(op.start);
                if depth == 0 {
                    out.push(Run { start, end: op.end, target: Target::Artifact });
                }
            }
            b"n" => path_start = None,
            b"sh" if depth == 0 => out.push(Run { start: op.start, end: op.end, target: Target::Artifact }),
            b"BI" if depth == 0 => out.push(Run { start: op.start, end: op.end, target: Target::Figure(bbox_of(&ctm, UNIT)) }),
            b"Do" if depth == 0 => {
                let Some(x) = op.operands.first().and_then(|o| o.name()).map(xobject).transpose()?.flatten() else { continue };
                let target = match name(&x, "Subtype")?.as_deref() {
                    Some("Image") => Target::Figure(bbox_of(&ctm, UNIT)),
                    Some("Form") => {
                        let body = x.read_stream().unwrap_or_default();
                        let has_text = content::parse(&body).iter().any(content::shows);
                        if has_text {
                            let m = match x.get_dict("Matrix")? {
                                Some(a) if a.is_array()? && a.len()? == 6 => {
                                    let v: Vec<f32> = (0..6).map(|i| a.get_array(i).ok().flatten().and_then(|n| n.as_float().ok()).unwrap_or(0.0)).collect();
                                    Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])
                                }
                                _ => Matrix::IDENTITY,
                            };
                            let bb = match x.get_dict("BBox")? {
                                Some(a) if a.is_array()? && a.len()? == 4 => {
                                    let v: Vec<f32> = (0..4).map(|i| a.get_array(i).ok().flatten().and_then(|n| n.as_float().ok()).unwrap_or(0.0)).collect();
                                    Rect { x0: v[0], y0: v[1], x1: v[2], y1: v[3] }
                                }
                                _ => UNIT,
                            };
                            Target::FormText(bbox_of(&(m * &ctm), bb))
                        } else {
                            Target::Artifact
                        }
                    }
                    _ => continue,
                };
                out.push(Run { start: op.start, end: op.end, target });
            }
            _ => {}
        }
    }
    close(&mut open, &mut out);
    Ok(out)
}

/// The runs that can be marked: in content order, each starting at or after the end of the one
/// before, within the stream. Invalid content (a path left open across a text object, say) can
/// make a run start before an earlier one ends; such a run is left unmarked.
fn in_order(runs: Vec<Run>, len: usize) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::with_capacity(runs.len());
    for r in runs {
        let after = out.last().map_or(0, |l| l.end);
        if r.start >= after && r.end >= r.start && r.end <= len {
            out.push(r);
        }
    }
    out
}

/// A structure element being built.
struct El {
    ty: String,
    kids: Vec<usize>,
    mcids: Vec<i32>,
    /// The annotation it owns through an OBJR.
    objr: Option<PdfObject>,
}

struct Page {
    els: Vec<El>,
    /// Top-level elements in reading order.
    top: Vec<usize>,
    /// The element that owns each MCID, in MCID order.
    owner: Vec<usize>,
}

impl Page {
    fn add(&mut self, ty: &str) -> usize {
        self.els.push(El { ty: ty.into(), kids: Vec::new(), mcids: Vec::new(), objr: None });
        self.els.len() - 1
    }

    fn has_content(&self, i: usize) -> bool {
        let e = &self.els[i];
        !e.mcids.is_empty() || e.objr.is_some() || e.kids.iter().any(|&k| self.has_content(k))
    }
}

/// Inserts `el` (top at `y0`) before the first block that starts below it.
fn place(top: &mut Vec<(usize, f32)>, el: usize, y0: f32) {
    let at = top.iter().position(|(_, y)| *y > y0).unwrap_or(top.len());
    top.insert(at, (el, y0));
}

/// Tags the document. Returns the pages whose content was marked.
pub fn run(doc: &mut PdfDocument) -> Result<Vec<u32>> {
    let mut cat = doc.catalog()?;
    if cat.get_dict("StructTreeRoot")?.is_some() {
        return Err(bad("the document is already tagged; autotag only tags untagged documents"));
    }
    let n = doc.page_count()?;
    let mut texts = Vec::with_capacity(n as usize);
    // Body size: the size most characters are set in.
    let mut chars_at: HashMap<i32, usize> = HashMap::new();
    for p in 0..n {
        let t = page_text(doc, p)?;
        for l in &t.lines {
            *chars_at.entry(half(l.size)).or_default() += l.text.chars().count();
        }
        texts.push(t);
    }
    let body = chars_at.iter().max_by_key(|(s, c)| (**c, -**s)).map_or(24, |(s, _)| *s);
    let is_heading_block = |t: &PageText, b: usize| -> Option<i32> {
        let ls: Vec<&Line> = t.lines.iter().filter(|l| l.block == b).collect();
        let size = half(ls.iter().map(|l| l.size).fold(0.0, f32::max));
        let chars: usize = ls.iter().map(|l| l.text.chars().count()).sum();
        (size as f32 >= body as f32 * HEADING_RATIO && ls.len() <= HEADING_MAX_LINES && chars <= HEADING_MAX_CHARS).then_some(size)
    };
    let mut heading_sizes: Vec<i32> = Vec::new();
    for t in &texts {
        for b in 0..t.blocks {
            if let Some(s) = is_heading_block(t, b)
                && !heading_sizes.contains(&s)
            {
                heading_sizes.push(s);
            }
        }
    }
    heading_sizes.sort_unstable_by(|a, b| b.cmp(a));
    let level = |s: i32| heading_sizes.iter().position(|h| *h == s).map_or(6, |i| (i + 1).min(6));

    let mut built: Vec<(i32, Page)> = Vec::new();
    let mut touched = Vec::new();
    for p in 0..n {
        let t = &texts[p as usize];
        let mut page_obj = doc.find_page(p)?;
        let page_ctm = page_obj.page_ctm()?;
        let res = page_obj.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
        let data = page_content(&page_obj)?;
        let ops = content::parse(&data);
        let runs = in_order(runs(&ops, res.as_ref(), page_ctm.clone(), &t.lines)?, data.len());

        // Blocks to elements.
        let mut pg = Page { els: Vec::new(), top: Vec::new(), owner: Vec::new() };
        let mut line_owner = vec![usize::MAX; t.lines.len()];
        let mut top: Vec<(usize, f32)> = Vec::new();
        for b in 0..t.blocks {
            let idx: Vec<usize> = (0..t.lines.len()).filter(|&i| t.lines[i].block == b).collect();
            let y0 = idx.iter().map(|&i| t.lines[i].bbox.y0).fold(f32::MAX, f32::min);
            if let Some(s) = is_heading_block(t, b) {
                let e = pg.add(&format!("H{}", level(s)));
                idx.iter().for_each(|&i| line_owner[i] = e);
                top.push((e, y0));
            } else if LIST_MARK.is_match(&t.lines[idx[0]].text) {
                let l = pg.add("L");
                let mut body = usize::MAX;
                for &i in &idx {
                    if body == usize::MAX || LIST_MARK.is_match(&t.lines[i].text) {
                        let li = pg.add("LI");
                        body = pg.add("LBody");
                        pg.els[li].kids.push(body);
                        pg.els[l].kids.push(li);
                    }
                    line_owner[i] = body;
                }
                top.push((l, y0));
            } else {
                let e = pg.add("P");
                idx.iter().for_each(|&i| line_owner[i] = e);
                top.push((e, y0));
            }
        }

        // Runs to marked content.
        let mut loose = None;
        let mut marks: Vec<(usize, usize, String)> = Vec::new();
        for r in &runs {
            let el = match r.target {
                Target::Artifact => {
                    marks.push((r.start, r.end, "/Artifact BMC\n".into()));
                    continue;
                }
                Target::Line(l) => line_owner[l],
                Target::Loose => *loose.get_or_insert_with(|| {
                    let e = pg.add("P");
                    top.push((e, f32::MAX));
                    e
                }),
                Target::Figure(b) => {
                    let e = pg.add("Figure");
                    place(&mut top, e, b.y0);
                    e
                }
                Target::FormText(b) => {
                    let e = pg.add("P");
                    place(&mut top, e, b.y0);
                    e
                }
            };
            let mcid = pg.owner.len() as i32;
            pg.owner.push(el);
            pg.els[el].mcids.push(mcid);
            marks.push((r.start, r.end, format!("/{} <</MCID {mcid}>> BDC\n", pg.els[el].ty)));
        }

        // Links and fields.
        for (_, a) in annots(&page_obj)? {
            let ty = match name(&a, "Subtype")?.as_deref() {
                Some("Link") => "Link",
                Some("Widget") => "Form",
                _ => continue,
            };
            if a.get_dict("StructParent")?.is_some() {
                continue;
            }
            let rect = match a.get_dict("Rect")? {
                Some(r) if r.is_array()? && r.len()? == 4 => {
                    let v: Vec<f32> = (0..4).map(|i| r.get_array(i).ok().flatten().and_then(|n| n.as_float().ok()).unwrap_or(0.0)).collect();
                    bbox_of(&page_ctm, Rect { x0: v[0], y0: v[1], x1: v[2], y1: v[3] })
                }
                _ => Rect { x0: 0.0, y0: f32::MAX, x1: 0.0, y1: f32::MAX },
            };
            let e = pg.add(ty);
            pg.els[e].objr = Some(a);
            place(&mut top, e, rect.y0);
        }
        pg.top = top.into_iter().map(|(e, _)| e).filter(|&e| pg.has_content(e)).collect();

        if !marks.is_empty() {
            let mut out = Vec::with_capacity(data.len() + marks.len() * 24);
            let mut at = 0;
            for (s, e, prefix) in &marks {
                // `in_order` already dropped overlapping runs; never slice backwards.
                if *s < at || *e < *s || *e > data.len() {
                    continue;
                }
                out.extend_from_slice(&data[at..*s]);
                out.extend_from_slice(prefix.as_bytes());
                out.extend_from_slice(&data[*s..*e]);
                out.extend_from_slice(b"\nEMC\n");
                at = *e;
            }
            out.extend_from_slice(&data[at..]);
            let stm = doc.add_stream(&Buffer::from_bytes(&out)?, None, false)?;
            page_obj.dict_put("Contents", stm)?;
            touched.push(p as u32);
        }
        built.push((p, pg));
    }

    // The tree.
    let mut root = doc.add_object(&doc.new_object_from_str("<</Type/StructTreeRoot>>")?)?;
    let mut top_el = doc.add_object(&doc.new_object_from_str("<</Type/StructElem/S/Document>>")?)?;
    top_el.dict_put("P", root.clone())?;
    let mut top_k = doc.new_array()?;
    let mut nums = doc.new_array()?;
    let mut next_key = n;
    let mut objrs: Vec<(PdfObject, PdfObject)> = Vec::new();
    for (p, pg) in &built {
        let mut page_obj = doc.find_page(*p)?;
        let mut refs: Vec<Option<PdfObject>> = vec![None; pg.els.len()];
        for &e in &pg.top {
            let r = write_el(doc, pg, e, &top_el, &page_obj, &mut refs, &mut objrs)?;
            top_k.array_push(r)?;
        }
        if !pg.owner.is_empty() {
            let mut arr = doc.new_array()?;
            for &e in &pg.owner {
                arr.array_push(refs[e].clone().unwrap_or_else(PdfObject::new_null))?;
            }
            nums.array_push(PdfObject::new_int(*p)?)?;
            nums.array_push(arr)?;
            page_obj.dict_put("StructParents", PdfObject::new_int(*p)?)?;
        }
    }
    // Annotation keys follow the page keys, so the number tree stays sorted.
    for (mut a, r) in objrs {
        a.dict_put("StructParent", PdfObject::new_int(next_key)?)?;
        nums.array_push(PdfObject::new_int(next_key)?)?;
        nums.array_push(r)?;
        next_key += 1;
    }
    top_el.dict_put("K", top_k)?;
    root.dict_put("K", top_el)?;
    let mut tree = doc.new_dict()?;
    tree.dict_put("Nums", nums)?;
    root.dict_put("ParentTree", tree)?;
    root.dict_put("ParentTreeNextKey", PdfObject::new_int(next_key)?)?;
    cat.dict_put("StructTreeRoot", root)?;
    cat.dict_put("MarkInfo", doc.new_object_from_str("<</Marked true>>")?)?;
    Ok(touched)
}

/// Writes element `e` and its children; records `(annotation, element)` for each OBJR.
fn write_el(
    doc: &mut PdfDocument,
    pg: &Page,
    e: usize,
    parent: &PdfObject,
    page: &PdfObject,
    refs: &mut [Option<PdfObject>],
    objrs: &mut Vec<(PdfObject, PdfObject)>,
) -> Result<PdfObject> {
    let el = &pg.els[e];
    let mut d = doc.new_dict()?;
    d.dict_put("Type", PdfObject::new_name("StructElem")?)?;
    d.dict_put("S", PdfObject::new_name(&el.ty)?)?;
    d.dict_put("P", parent.clone())?;
    d.dict_put("Pg", page.clone())?;
    let r = doc.add_object(&d)?;
    refs[e] = Some(r.clone());
    let mut k = doc.new_array()?;
    for &m in &el.mcids {
        k.array_push(PdfObject::new_int(m)?)?;
    }
    if let Some(a) = &el.objr {
        let mut o = doc.new_dict()?;
        o.dict_put("Type", PdfObject::new_name("OBJR")?)?;
        o.dict_put("Obj", a.clone())?;
        o.dict_put("Pg", page.clone())?;
        k.array_push(o)?;
        objrs.push((a.clone(), r.clone()));
    }
    for &c in &el.kids {
        if pg.has_content(c) {
            let cr = write_el(doc, pg, c, &r, page, refs, objrs)?;
            k.array_push(cr)?;
        }
    }
    let mut r2 = r.clone();
    r2.dict_put("K", k)?;
    Ok(r)
}
