//! Autotag (best effort): a basic structure tree for an untagged document, from MuPDF's
//! structured text. Headings come from font size (sizes clearly above the body text, largest
//! first, as H1-H6), blocks become paragraphs, blocks whose lines start with a bullet or a number
//! become lists (L > LI > LBody), regular grids of short cells become tables (Table > TR > TH/TD,
//! see `tables`), runs of entries with a page number at the end become a table of contents
//! (TOC > TOCI > Reference, see `toc`), images become figures (without alternate text: the checker
//! then asks for it), a short "Figure 2" or "Table 1" line next to a figure or table becomes its
//! Caption (see `captions`), link and widget annotations become Link and Form elements, and every
//! other painting (rules, fills, shadings) is marked as an artifact.
//!
//! Content is tied to the tags by rewriting each page's content stream: every run of text that
//! falls on one structured-text line is wrapped in `/Tag <</MCID n>> BDC ... EMC`, every image in
//! a Figure's marked content, every path in `/Artifact BMC ... EMC`. Nothing is moved or redrawn,
//! so the page looks and extracts exactly as before. Content that is already marked is left as
//! it is. The result is labelled best effort: headings, lists and order are guesses to review.

use super::content::{self, Op, TextPos};
use super::captions;
use super::tables::{self, Table};
use super::toc::{self, Toc};
use super::tree::{name, xref_of};
use super::{Progress, Result, annots, bad, page_content};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::text_page::{TextBlockType, TextCharFlags, TextPageFlags};
use mupdf::{Buffer, Matrix, Rect};
use std::collections::{HashMap, HashSet};
use serde::Serialize;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

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
    baseline: f32,
    bold: bool,
}

/// What a line belongs to when it is not ordinary text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Own {
    Table(usize),
    Toc(usize),
}

struct PageText {
    lines: Vec<Line>,
    blocks: usize,
    tables: Vec<Table>,
    tocs: Vec<Toc>,
    /// The table or table of contents each line is part of.
    own: Vec<Option<Own>>,
    /// The page's runs and the length of the content they were computed from, when table
    /// detection computed them: the tag pass takes them instead of walking the content again.
    runs: Option<(usize, Vec<Run>)>,
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
            let mut baseline = None;
            let (mut glyphs, mut bold) = (0, 0);
            for ch in l.chars() {
                size = size.max(ch.size());
                baseline.get_or_insert(ch.origin().y);
                if let Some(c) = ch.char() {
                    text.push(c);
                    if !c.is_whitespace() {
                        glyphs += 1;
                        bold += ch.flags().contains(TextCharFlags::BOLD) as usize;
                    }
                }
            }
            if text.trim().is_empty() {
                continue;
            }
            any = true;
            let bbox = l.bounds();
            lines.push(Line { bbox, block: blocks, text, size, baseline: baseline.unwrap_or(bbox.y1), bold: glyphs > 0 && bold == glyphs });
        }
        if any {
            blocks += 1;
        }
    }
    let (tables, tocs, runs) = find_groups(doc, p, &lines)?;
    let mut own = vec![None; lines.len()];
    for (i, t) in tables.iter().enumerate() {
        t.rows.iter().flatten().for_each(|&l| own[l] = Some(Own::Table(i)));
    }
    for (i, t) in tocs.iter().enumerate() {
        t.rows.iter().flatten().for_each(|&l| own[l] = Some(Own::Toc(i)));
    }
    Ok(PageText { lines, blocks, tables, tocs, own, runs })
}

/// For each group of lines, whether the page draws nothing but those lines and artifacts between
/// the group's first and last run: only then are tag order and content order the same.
fn undisturbed(rs: &[Run], span: &[Option<(usize, usize)>], groups: &[Vec<usize>]) -> Vec<bool> {
    let mut owner: Vec<Option<usize>> = vec![None; span.len()];
    for (g, ls) in groups.iter().enumerate() {
        ls.iter().for_each(|&l| owner[l] = Some(g));
    }
    groups
        .iter()
        .enumerate()
        .map(|(g, ls)| {
            let (lo, hi) = ls.iter().filter_map(|&l| span[l]).fold((usize::MAX, 0), |(lo, hi), (a, b)| (lo.min(a), hi.max(b)));
            lo > hi
                || rs[lo..=hi].iter().all(|r| match r.target {
                    Target::Artifact => true,
                    Target::Line(l) => owner[l] == Some(g),
                    _ => false,
                })
        })
        .collect()
}

/// Whether the runs `cap` are drawn right before (`above`) or after the runs `target`, with only artifacts between.
fn adjacent(runs: &[Run], cap: &[usize], target: &[usize], above: bool) -> bool {
    let (Some(&c0), Some(&c1), Some(&t0), Some(&t1)) = (cap.iter().min(), cap.iter().max(), target.iter().min(), target.iter().max()) else {
        return false;
    };
    // All of the caption's runs on one side of all of the target's.
    if if above { c1 >= t0 } else { t1 >= c0 } {
        return false;
    }
    let mine: HashSet<usize> = cap.iter().chain(target).copied().collect();
    (c0.min(t0)..=c1.max(t1)).all(|k| mine.contains(&k) || matches!(runs[k].target, Target::Artifact))
}

/// The tables and the tables of contents on page `p`. Only lines that get a marked-content run can
/// be part of them (text that is already marked, or inside a form, is left alone), and form fields
/// keep the text near them. A page with too many lines is not looked at (walking its content is the
/// costly part). Also returns the page's runs, which the tag pass needs again.
type Groups = (Vec<Table>, Vec<Toc>, Option<(usize, Vec<Run>)>);

fn find_groups(doc: &PdfDocument, p: i32, lines: &[Line]) -> Result<Groups> {
    if lines.len() < toc::MIN_ENTRIES || lines.len() > tables::MAX_ITEMS {
        return Ok((Vec::new(), Vec::new(), None));
    }
    let page = doc.find_page(p)?;
    let ctm = page.page_ctm()?;
    let res = page.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
    let data = page_content(&page)?;
    let ops = content::parse(&data);
    let rs = in_order(runs(&ops, res.as_ref(), ctm.clone(), lines)?, data.len());
    let mut span: Vec<Option<(usize, usize)>> = vec![None; lines.len()];
    for (k, r) in rs.iter().enumerate() {
        if let Target::Line(l) = r.target {
            span[l] = Some((span[l].map_or(k, |s| s.0), k));
        }
    }
    let items: Vec<tables::Item> = (0..lines.len())
        .filter_map(|i| {
            // Upright text only: a rotated line is taller than it is wide.
            let l = &lines[i];
            let upright = l.bbox.x1 - l.bbox.x0 >= l.bbox.y1 - l.bbox.y0 || l.text.chars().count() <= 3;
            span[i].filter(|_| upright).map(|runs| tables::Item { id: i, bbox: l.bbox, baseline: l.baseline, size: l.size, bold: l.bold, text: &l.text, runs })
        })
        .collect();
    let mut widgets = Vec::new();
    for (_, a) in annots(&page)? {
        if name(&a, "Subtype")?.as_deref() == Some("Widget")
            && let Some(r) = a.get_dict("Rect")?.filter(|r| r.is_array().unwrap_or(false) && r.len().unwrap_or(0) == 4)
        {
            let v: Vec<f32> = (0..4).map(|i| r.get_array(i).ok().flatten().and_then(|n| n.as_float().ok()).unwrap_or(0.0)).collect();
            widgets.push(bbox_of(&ctm, Rect { x0: v[0], y0: v[1], x1: v[2], y1: v[3] }));
        }
    }
    let mut found = tables::detect(&items, &tables::rules(&ops, &ctm), &widgets);
    // Between a table's first and last cell the page may draw only its cells and artifacts: text
    // drawn in the middle (a running footer, say) would be read in another order than drawn.
    let cells = |rows: &[Vec<usize>]| rows.iter().flatten().copied().collect::<Vec<usize>>();
    let mut keep = undisturbed(&rs, &span, &found.iter().map(|t| cells(&t.rows)).collect::<Vec<_>>()).into_iter();
    found.retain(|_| keep.next().unwrap_or(false));
    // What the tables did not take may be a table of contents (a list of titles and page numbers
    // that a table would not claim: no rule, no header).
    let mut taken = vec![false; lines.len()];
    found.iter().flat_map(|t| t.rows.iter().flatten()).for_each(|&l| taken[l] = true);
    let loose: Vec<tables::Item> = items.iter().filter(|it| !taken[it.id]).copied().collect();
    let mut tocs = toc::detect(&loose, &widgets, doc.page_count()?.max(0) as usize);
    let mut keep = undisturbed(&rs, &span, &tocs.iter().map(|t| cells(&t.rows)).collect::<Vec<_>>()).into_iter();
    tocs.retain(|_| keep.next().unwrap_or(false));
    Ok((found, tocs, Some((data.len(), rs))))
}

/// A block's lines in order, cut where they change between a table's cells (or a table of
/// contents' lines) and plain text: `(owner, line indices)`. A block with none is one piece.
fn pieces(t: &PageText, b: usize) -> Vec<(Option<Own>, Vec<usize>)> {
    let mut out: Vec<(Option<Own>, Vec<usize>)> = Vec::new();
    for i in (0..t.lines.len()).filter(|&i| t.lines[i].block == b) {
        match out.last_mut() {
            Some((o, idx)) if *o == t.own[i] => idx.push(i),
            _ => out.push((t.own[i], vec![i])),
        }
    }
    out
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

pub(super) fn bbox_of(m: &Matrix, r: Rect) -> Rect {
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

pub(super) const UNIT: Rect = Rect { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 };

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
                    close(&mut open, &mut out);
                    out.push(Run { start, end: op.end, target: Target::Artifact });
                }
            }
            b"n" => path_start = None,
            // A non-text run closes the open text run first, so runs stay in content order even
            // when an image or shading sits inside a text object (`BT Tj /Im0 Do Tj ET`).
            b"sh" if depth == 0 => {
                close(&mut open, &mut out);
                out.push(Run { start: op.start, end: op.end, target: Target::Artifact });
            }
            b"BI" if depth == 0 => {
                close(&mut open, &mut out);
                out.push(Run { start: op.start, end: op.end, target: Target::Figure(bbox_of(&ctm, UNIT)) });
            }
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
                close(&mut open, &mut out);
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
    /// Kids written before the element's own content (a caption above its figure or table).
    before: Vec<usize>,
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
        self.els.push(El { ty: ty.into(), before: Vec::new(), kids: Vec::new(), mcids: Vec::new(), objr: None });
        self.els.len() - 1
    }

    fn has_content(&self, i: usize) -> bool {
        let e = &self.els[i];
        !e.mcids.is_empty() || e.objr.is_some() || e.before.iter().chain(&e.kids).any(|&k| self.has_content(k))
    }
}

/// The box that holds all of `boxes`.
fn union(boxes: impl Iterator<Item = Rect>) -> Rect {
    boxes.fold(Rect { x0: f32::MAX, y0: f32::MAX, x1: f32::MIN, y1: f32::MIN }, |a, b| Rect { x0: a.x0.min(b.x0), y0: a.y0.min(b.y0), x1: a.x1.max(b.x1), y1: a.y1.max(b.y1) })
}

/// The page's link annotations that go to a page of the document (a destination that exists), with
/// their boxes in page space. Annotations with an inline dictionary can't be referenced and are left out.
fn toc_links(doc: &PdfDocument, p: i32, page: &PdfObject) -> Result<Vec<(Rect, i32, PdfObject)>> {
    let read = crate::links::read_page(doc, p as u32, 0)?;
    let mut out = Vec::new();
    for (x, a) in annots(page)? {
        let hit = read.links.iter().find(|l| l.id == x && x > 0 && matches!(l.target, crate::links::Target::Page { .. }));
        if let Some(l) = hit {
            out.push((Rect { x0: l.rect[0], y0: l.rect[1], x1: l.rect[2], y1: l.rect[3] }, x, a));
        }
    }
    Ok(out)
}

/// Takes the first link whose box is over the entry `row`: its middle is on the entry's lines and it
/// shares at least half the narrower width with it.
fn take_link(links: &mut Vec<(Rect, i32, PdfObject)>, row: Rect) -> Option<(i32, PdfObject)> {
    let at = links.iter().position(|(r, ..)| {
        let mid = (r.y0 + r.y1) / 2.0;
        let shared = (r.x1.min(row.x1) - r.x0.max(row.x0)).max(0.0);
        mid >= row.y0 - 2.0 && mid <= row.y1 + 2.0 && shared > 0.0 && shared >= 0.5 * (r.x1 - r.x0).min(row.x1 - row.x0)
    })?;
    let (_, x, a) = links.remove(at);
    Some((x, a))
}

/// Inserts `el` (top at `y0`) before the first block that starts below it.
fn place(top: &mut Vec<(usize, f32)>, el: usize, y0: f32) {
    let at = top.iter().position(|(_, y)| *y > y0).unwrap_or(top.len());
    top.insert(at, (el, y0));
}

/// What Autotag would add to an untagged document, counted from the file without running it
/// (the layout analysis itself is the slow part): enough to say what a run does before it runs.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// The document has no tag tree, so Autotag can run.
    pub untagged: bool,
    pub pages: u32,
    /// Images that become Figure elements without alternate text.
    pub figures: u32,
    pub links: u32,
    pub fields: u32,
}

pub fn preview(doc: &PdfDocument) -> Result<Preview> {
    let untagged = doc.catalog()?.get_dict("StructTreeRoot")?.is_none();
    let n = doc.page_count()?.max(0);
    let (mut figures, mut links, mut fields) = (0, 0, 0);
    for p in 0..n {
        let page = doc.find_page(p)?;
        let res = page.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
        let mut depth = 0usize;
        for op in content::parse(&page_content(&page)?) {
            match op.name.as_slice() {
                b"BMC" | b"BDC" => depth += 1,
                b"EMC" => depth = depth.saturating_sub(1),
                b"BI" if depth == 0 => figures += 1,
                b"Do" if depth == 0 => {
                    let x = match (&res, op.operands.first().and_then(|o| o.name())) {
                        (Some(r), Some(n)) => match r.get_dict("XObject")? {
                            Some(d) => d.get_dict(String::from_utf8_lossy(n).into_owned())?,
                            None => None,
                        },
                        _ => None,
                    };
                    if let Some(x) = x
                        && name(&x, "Subtype")?.as_deref() == Some("Image")
                    {
                        figures += 1;
                    }
                }
                _ => {}
            }
        }
        for (_, a) in annots(&page)? {
            match name(&a, "Subtype")?.as_deref() {
                Some("Link") => links += 1,
                Some("Widget") => fields += 1,
                _ => {}
            }
        }
    }
    Ok(Preview { untagged, pages: n as u32, figures, links, fields })
}

fn stop() -> super::EngineError {
    bad("cancelled")
}

/// Tags the document. Returns the pages whose content was marked. `progress` gets a `read` event
/// per page, then a `tag` event per page; `cancel` is polled before each page and ends the run
/// with "cancelled" (the caller's edit step then changes nothing).
pub fn run(doc: &mut PdfDocument, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Vec<u32>> {
    let mut cat = doc.catalog()?;
    if cat.get_dict("StructTreeRoot")?.is_some() {
        return Err(bad("the document is already tagged; autotag only tags untagged documents"));
    }
    let n = doc.page_count()?;
    let mut texts = Vec::with_capacity(n as usize);
    // Body size: the size most characters are set in.
    let mut chars_at: HashMap<i32, usize> = HashMap::new();
    for p in 0..n {
        if cancel.load(Ordering::Relaxed) {
            return Err(stop());
        }
        let t = page_text(doc, p)?;
        progress(Progress { done: p as u32 + 1, pages: n as u32, stage: "read" });
        for l in &t.lines {
            *chars_at.entry(half(l.size)).or_default() += l.text.chars().count();
        }
        texts.push(t);
    }
    let body = chars_at.iter().max_by_key(|(s, c)| (**c, -**s)).map_or(24, |(s, _)| *s);
    let heading_size = |t: &PageText, idx: &[usize]| -> Option<i32> {
        let size = half(idx.iter().map(|&i| t.lines[i].size).fold(0.0, f32::max));
        let chars: usize = idx.iter().map(|&i| t.lines[i].text.chars().count()).sum();
        (size as f32 >= body as f32 * HEADING_RATIO && idx.len() <= HEADING_MAX_LINES && chars <= HEADING_MAX_CHARS).then_some(size)
    };
    let mut heading_sizes: Vec<i32> = Vec::new();
    for t in &texts {
        for b in 0..t.blocks {
            for (own, idx) in pieces(t, b) {
                if own.is_none()
                    && let Some(s) = heading_size(t, &idx)
                    && !heading_sizes.contains(&s)
                {
                    heading_sizes.push(s);
                }
            }
        }
    }
    heading_sizes.sort_unstable_by(|a, b| b.cmp(a));
    let level = |s: i32| heading_sizes.iter().position(|h| *h == s).map_or(6, |i| (i + 1).min(6));

    let mut built: Vec<(i32, Page)> = Vec::new();
    let mut touched = Vec::new();
    // A page object listed twice in /Kids is tagged once: its content is already marked, so the
    // second entry would apply offsets from the unmarked stream to the marked one.
    let mut done: HashSet<i32> = HashSet::new();
    for p in 0..n {
        if cancel.load(Ordering::Relaxed) {
            return Err(stop());
        }
        let cached = texts[p as usize].runs.take();
        let t = &texts[p as usize];
        let mut page_obj = doc.find_page(p)?;
        let x = xref_of(&page_obj)?;
        if x != 0 && !done.insert(x) {
            progress(Progress { done: p as u32 + 1, pages: n as u32, stage: "tag" });
            continue;
        }
        let page_ctm = page_obj.page_ctm()?;
        let res = page_obj.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
        let data = page_content(&page_obj)?;
        let ops = content::parse(&data);
        let runs = match cached {
            Some((len, r)) if len == data.len() => r,
            _ => in_order(runs(&ops, res.as_ref(), page_ctm.clone(), &t.lines)?, data.len()),
        };

        // Blocks to elements. A table or a table of contents takes its lines out of their blocks and
        // comes in once, where its first line is.
        let mut pg = Page { els: Vec::new(), top: Vec::new(), owner: Vec::new() };
        let mut line_owner = vec![usize::MAX; t.lines.len()];
        let mut top: Vec<(usize, f32)> = Vec::new();
        let mut seen = vec![false; t.tables.len()];
        let mut seen_toc = vec![false; t.tocs.len()];
        // Paragraphs that may turn out to be captions, and the boxes of the tables they may belong to.
        let mut paragraphs: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut table_boxes: Vec<(usize, Rect, Vec<usize>)> = Vec::new();
        let mut links = if t.tocs.is_empty() { Vec::new() } else { toc_links(doc, p, &page_obj)? };
        let mut claimed: HashSet<i32> = HashSet::new();
        for b in 0..t.blocks {
            for (own, idx) in pieces(t, b) {
                let y0 = idx.iter().map(|&i| t.lines[i].bbox.y0).fold(f32::MAX, f32::min);
                match own {
                    Some(Own::Table(ti)) => {
                        if !std::mem::replace(&mut seen[ti], true) {
                            let tb = &t.tables[ti];
                            let e = pg.add("Table");
                            for (r, row) in tb.rows.iter().enumerate() {
                                let tr = pg.add("TR");
                                pg.els[e].kids.push(tr);
                                for &l in row {
                                    let c = pg.add(if r == 0 && tb.header { "TH" } else { "TD" });
                                    pg.els[tr].kids.push(c);
                                    line_owner[l] = c;
                                }
                            }
                            let cells = || tb.rows.iter().flatten().map(|&l| t.lines[l].bbox);
                            let top_y = cells().map(|b| b.y0).fold(f32::MAX, f32::min);
                            table_boxes.push((e, union(cells()), tb.rows.iter().flatten().copied().collect()));
                            top.push((e, top_y));
                        }
                    }
                    Some(Own::Toc(ti)) => {
                        if !std::mem::replace(&mut seen_toc[ti], true) {
                            let e = pg.add("TOC");
                            let mut top_y = f32::MAX;
                            for row in &t.tocs[ti].rows {
                                let item = pg.add("TOCI");
                                pg.els[e].kids.push(item);
                                let b = union(row.iter().map(|&l| t.lines[l].bbox));
                                top_y = top_y.min(b.y0);
                                // TOCI takes Reference (and Lbl, P, NonStruct, TOC), not marked content or Link
                                // directly (ISO 32000-1 Table 333). The entry's text is in a Reference; a link
                                // annotation already over the entry, with a target that exists, makes it a Link
                                // inside that Reference, holding the text and the annotation.
                                let reference = pg.add("Reference");
                                pg.els[item].kids.push(reference);
                                let mut owner = reference;
                                if let Some((x, a)) = take_link(&mut links, b) {
                                    claimed.insert(x);
                                    owner = pg.add("Link");
                                    pg.els[owner].objr = Some(a);
                                    pg.els[reference].kids.push(owner);
                                }
                                row.iter().for_each(|&l| line_owner[l] = owner);
                            }
                            top.push((e, top_y));
                        }
                    }
                    None => {
                        if let Some(s) = heading_size(t, &idx) {
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
                            paragraphs.push((e, idx));
                        }
                    }
                }
            }
        }

        // Figures get their elements now, so a caption can go inside; they take their place in the
        // reading order below, with the run.
        let figure_of: Vec<Option<usize>> = runs.iter().map(|r| matches!(r.target, Target::Figure(_)).then(|| pg.add("Figure"))).collect();
        // The runs (in draw order) of every line, to know what a figure or a table is drawn next to.
        let mut line_runs: Vec<Vec<usize>> = vec![Vec::new(); t.lines.len()];
        for (k, r) in runs.iter().enumerate() {
            if let Target::Line(l) = r.target {
                line_runs[l].push(k);
            }
        }
        // (element, box, the runs it is drawn with)
        let mut targets: Vec<(usize, captions::Target, Vec<usize>)> = table_boxes
            .into_iter()
            .map(|(e, bbox, ls)| (e, captions::Target { kind: captions::Kind::Table, bbox }, ls.iter().flat_map(|&l| line_runs[l].iter().copied()).collect()))
            .collect();
        for (k, (r, e)) in runs.iter().zip(&figure_of).enumerate() {
            if let (Target::Figure(bbox), Some(e)) = (r.target, e) {
                targets.push((*e, captions::Target { kind: captions::Kind::Figure, bbox }, vec![k]));
            }
        }
        let boxes: Vec<captions::Target> = targets.iter().map(|(_, t0, _)| captions::Target { kind: t0.kind, bbox: t0.bbox }).collect();
        let cands: Vec<(usize, captions::Kind)> = paragraphs
            .iter()
            .enumerate()
            .filter_map(|(n, (_, idx))| {
                let chars: usize = idx.iter().map(|&i| t.lines[i].text.chars().count()).sum();
                let kind = captions::label(&t.lines[idx[0]].text)?;
                (idx.len() <= captions::MAX_LINES && chars <= captions::MAX_CHARS).then_some((n, kind))
            })
            .collect();
        // Bounded work: a page with very many lines, targets or label paragraphs keeps its paragraphs.
        let cheap = t.lines.len() <= tables::MAX_ITEMS && cands.len() * boxes.len() <= captions::MAX_WORK;
        let line_boxes: Vec<Rect> = if cheap && !cands.is_empty() && !boxes.is_empty() { t.lines.iter().map(|l| l.bbox).collect() } else { Vec::new() };
        let mut filled: HashSet<(usize, bool)> = HashSet::new();
        for (n, kind) in cands.into_iter().filter(|_| !line_boxes.is_empty()) {
            let (e, idx) = &paragraphs[n];
            let cap = union(idx.iter().map(|&i| t.lines[i].bbox));
            let Some((k, above)) = captions::place(kind, cap, &boxes, &line_boxes, idx) else { continue };
            let host = targets[k].0;
            // The caption must be drawn right before (above) or right after (below) its figure or table, with
            // nothing but artifacts between: the Caption is a kid of it, so tag order has to be draw order.
            let own: Vec<usize> = idx.iter().flat_map(|&l| line_runs[l].iter().copied()).collect();
            if !adjacent(&runs, &own, &targets[k].2, above) || !filled.insert((host, above)) {
                continue;
            }
            pg.els[*e].ty = "Caption".into();
            top.retain(|(x, _)| x != e);
            if above { pg.els[host].before.push(*e) } else { pg.els[host].kids.push(*e) }
        }

        // Runs to marked content.
        let mut loose = None;
        let mut marks: Vec<(usize, usize, String)> = Vec::new();
        for (k, r) in runs.iter().enumerate() {
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
                    let e = figure_of[k].expect("a figure run has its element");
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
            if a.get_dict("StructParent")?.is_some() || claimed.contains(&xref_of(&a)?) {
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
                // `in_order` already dropped overlapping runs, so this never fires; if it did, the
                // MCID would already be assigned, so it is a bug, but slicing backwards would panic.
                let ok = *s >= at && *e >= *s && *e <= data.len();
                debug_assert!(ok, "marks out of order: {s}..{e} after {at}");
                if !ok {
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
        progress(Progress { done: p as u32 + 1, pages: n as u32, stage: "tag" });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(stop());
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
    if el.ty == "TH" {
        d.dict_put("A", doc.new_object_from_str("<</O/Table/Scope/Column>>")?)?;
    }
    let r = doc.add_object(&d)?;
    refs[e] = Some(r.clone());
    let mut k = doc.new_array()?;
    for &c in &el.before {
        if pg.has_content(c) {
            let cr = write_el(doc, pg, c, &r, page, refs, objrs)?;
            k.array_push(cr)?;
        }
    }
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
