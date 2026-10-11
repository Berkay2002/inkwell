//! What each piece of marked content on a page holds: its text (decoded to Unicode as far as the
//! font allows) and where it is painted. This is what the reading-order list shows next to every
//! region: a preview of the text and a box to outline.
//!
//! Text widths come from the font's /Widths (simple fonts) or /W and /DW (CID fonts), so a box is
//! as wide as the text really is; a font without widths (a standard 14 font written without them)
//! gets an average. Characters come from the font's ToUnicode CMap, else the code itself for a
//! simple font; a CID font with neither contributes no text.

use super::autotag::{UNIT, bbox_of};
use super::content::{self, Op, Operand, TextPos};
use super::locate::{Box4, mcid_of};
use super::tree::{Tree, name};
use super::{Result, dict, page_content};
use mupdf::pdf::PdfObject;
use mupdf::{Matrix, Rect};
use serde::Serialize;
use std::collections::HashMap;
use std::rc::Rc;

/// Longest text preview, in characters.
const PREVIEW: usize = 120;
const MAX_FORM_DEPTH: usize = 6;
/// Operators read per page before the scan gives up (a guard against absurd content).
const MAX_OPS: usize = 4_000_000;

/// One MCID's content on a page.
#[derive(Clone, Debug, Default)]
pub struct Marked {
    pub bbox: Option<Rect>,
    pub text: String,
    /// Where the last text piece ended (page space), to decide on a space before the next.
    end: Option<(f32, f32)>,
}

impl Marked {
    pub fn bounds(&self) -> Option<Box4> {
        let r = self.bbox?;
        let b = [r.x0, r.y0, r.x1, r.y1];
        (b.iter().all(|v| v.is_finite()) && r.x1 >= r.x0 && r.y1 >= r.y0 && r.x1 - r.x0 < 1.0e5 && r.y1 - r.y0 < 1.0e5).then_some(b)
    }

    /// The text collapsed to single spaces and cut to a preview.
    pub fn preview(&self) -> String {
        let t = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.chars().count() > PREVIEW {
            let mut s: String = t.chars().take(PREVIEW).collect();
            s.push('…');
            s
        } else {
            t
        }
    }
}

fn union(a: Option<Rect>, b: Rect) -> Rect {
    match a {
        None => b,
        Some(a) => Rect { x0: a.x0.min(b.x0), y0: a.y0.min(b.y0), x1: a.x1.max(b.x1), y1: a.y1.max(b.y1) },
    }
}

fn num(o: &PdfObject) -> f32 {
    o.as_float().unwrap_or(0.0)
}

fn rect_of(o: &PdfObject, key: &str) -> Result<Option<Rect>> {
    let Some(a) = o.get_dict(key)?.filter(|a| a.is_array().unwrap_or(false) && a.len().unwrap_or(0) == 4) else { return Ok(None) };
    let v: Vec<f32> = (0..4).map(|i| a.get_array(i).ok().flatten().map_or(0.0, |n| num(&n))).collect();
    Ok(Some(Rect { x0: v[0].min(v[2]), y0: v[1].min(v[3]), x1: v[0].max(v[2]), y1: v[1].max(v[3]) }))
}

fn matrix_of(o: &PdfObject) -> Result<Matrix> {
    Ok(match o.get_dict("Matrix")? {
        Some(a) if a.is_array()? && a.len()? == 6 => {
            let v: Vec<f32> = (0..6).map(|i| a.get_array(i).ok().flatten().map_or(0.0, |n| num(&n))).collect();
            Matrix::new(v[0], v[1], v[2], v[3], v[4], v[5])
        }
        _ => Matrix::IDENTITY,
    })
}

// ---- fonts ---------------------------------------------------------------------------------------

enum Widths {
    Simple { first: i32, w: Vec<f32>, missing: f32 },
    Cid { dw: f32, w: HashMap<u32, f32> },
}

struct FontInfo {
    two_byte: bool,
    widths: Widths,
    to_unicode: Option<HashMap<u32, String>>,
    ascent: f32,
    descent: f32,
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks(2).map(|c| if c.len() == 2 { u16::from_be_bytes([c[0], c[1]]) } else { u16::from(c[0]) }).collect();
    String::from_utf16_lossy(&units)
}

fn code_of(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, x| a << 8 | u32::from(*x))
}

/// Entries a ToUnicode map keeps (the 16-bit code space is 65,536) and bytes of it that are read.
const MAX_CMAP_ENTRIES: usize = 70_000;
const MAX_CMAP_BYTES: usize = 4 << 20;

/// A ToUnicode CMap: `bfchar` and `bfrange` entries, keyed by source code. At most
/// [`MAX_CMAP_ENTRIES`] entries are read, counted as entries written (so ranges that overlap and
/// overwrite each other still run out), however many ranges the file holds.
fn parse_to_unicode(data: &[u8]) -> HashMap<u32, String> {
    let mut m = HashMap::new();
    let mut left = MAX_CMAP_ENTRIES;
    'ops: for op in content::parse(data) {
        match op.name.as_slice() {
            b"endbfchar" => {
                for pair in op.operands.chunks(2) {
                    if let [Operand::Str(src), Operand::Str(dst)] = pair {
                        if left == 0 {
                            break 'ops;
                        }
                        left -= 1;
                        m.insert(code_of(src), utf16(dst));
                    }
                }
            }
            b"endbfrange" => {
                for t in op.operands.chunks(3) {
                    let [Operand::Str(lo), Operand::Str(hi), dst] = t else { continue };
                    let (lo, hi) = (code_of(lo), code_of(hi));
                    if hi < lo || hi - lo > 0xFFFF {
                        continue;
                    }
                    match dst {
                        Operand::Str(d) => {
                            let base = utf16(d);
                            let head: String = base.chars().take(base.chars().count().saturating_sub(1)).collect();
                            let last = base.chars().last();
                            for (k, c) in (lo..=hi).enumerate() {
                                if left == 0 {
                                    break 'ops;
                                }
                                left -= 1;
                                let mut s = head.clone();
                                if let Some(l) = last.and_then(|l| char::from_u32((l as u32).saturating_add(k as u32))) {
                                    s.push(l);
                                }
                                m.insert(c, s);
                            }
                        }
                        Operand::Array(items) => {
                            for (k, d) in items.iter().enumerate() {
                                if let Operand::Str(d) = d {
                                    if left == 0 {
                                        break 'ops;
                                    }
                                    left -= 1;
                                    m.insert(lo.saturating_add(k as u32), utf16(d));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    m
}

fn load_font(f: &PdfObject) -> Result<FontInfo> {
    let subtype = name(f, "Subtype")?;
    let type0 = subtype.as_deref() == Some("Type0");
    let desc_font = if type0 {
        f.get_dict("DescendantFonts")?.filter(|d| d.is_array().unwrap_or(false)).and_then(|d| d.get_array(0).ok().flatten())
    } else {
        None
    };
    let source = desc_font.as_ref().unwrap_or(f);
    let (mut ascent, mut descent) = (800.0, -200.0);
    let mut missing = 550.0;
    if let Some(fd) = dict(source, "FontDescriptor")? {
        if let Some(a) = fd.get_dict("Ascent")?.map(|a| num(&a)).filter(|a| (200.0..=1500.0).contains(a)) {
            ascent = a;
        }
        if let Some(d) = fd.get_dict("Descent")?.map(|d| num(&d)).filter(|d| (-800.0..=0.0).contains(d)) {
            descent = d;
        }
        if let Some(w) = fd.get_dict("MissingWidth")?.map(|w| num(&w)).filter(|w| *w > 0.0) {
            missing = w;
        }
    }
    let widths = if type0 {
        let mut w = HashMap::new();
        if let Some(arr) = source.get_dict("W")?.filter(|a| a.is_array().unwrap_or(false)) {
            let items: Vec<PdfObject> = arr.array_iter()?.collect::<std::result::Result<_, _>>()?;
            let mut i = 0;
            while i < items.len() && w.len() < 70_000 {
                if i + 1 < items.len() && items[i + 1].is_array()? {
                    let first = num(&items[i]) as u32;
                    for (k, x) in items[i + 1].array_iter()?.enumerate() {
                        w.insert(first.saturating_add(k as u32), num(&x?));
                    }
                    i += 2;
                } else if i + 2 < items.len() {
                    let (a, b) = (num(&items[i]) as u32, num(&items[i + 1]) as u32);
                    let v = num(&items[i + 2]);
                    for c in a..=b.min(a.saturating_add(0xFFFF)) {
                        w.insert(c, v);
                    }
                    i += 3;
                } else {
                    break;
                }
            }
        }
        Widths::Cid { dw: source.get_dict("DW")?.map_or(1000.0, |d| num(&d)), w }
    } else {
        let first = f.get_dict("FirstChar")?.and_then(|v| v.as_int().ok()).unwrap_or(0);
        let w: Vec<f32> = match f.get_dict("Widths")?.filter(|a| a.is_array().unwrap_or(false)) {
            Some(a) => a.array_iter()?.map(|x| x.map(|x| num(&x))).collect::<std::result::Result<_, _>>()?,
            None => Vec::new(),
        };
        Widths::Simple { first, w, missing }
    };
    let to_unicode = f.get_dict("ToUnicode")?.filter(|s| s.is_stream().unwrap_or(false)).map(|s| parse_to_unicode(&s.read_stream_capped(MAX_CMAP_BYTES).unwrap_or_default()));
    Ok(FontInfo { two_byte: type0, widths, to_unicode, ascent, descent })
}

impl FontInfo {
    fn width(&self, code: u32) -> f32 {
        match &self.widths {
            Widths::Simple { first, w, missing } => {
                let i = code as i64 - i64::from(*first);
                if i >= 0 && (i as usize) < w.len() { w[i as usize] } else { *missing }
            }
            Widths::Cid { dw, w } => w.get(&code).copied().unwrap_or(*dw),
        }
    }

    fn unicode(&self, code: u32, out: &mut String) {
        if let Some(m) = &self.to_unicode
            && let Some(s) = m.get(&code)
        {
            out.push_str(s);
            return;
        }
        if !self.two_byte && code >= 32 && code != 127 {
            out.extend(char::from_u32(code));
        }
    }

    /// `(unicode text, advance in thousandths of text space, codes)` for one string.
    fn show(&self, s: &[u8], tc: f32, tw: f32, size: f32) -> (String, f32) {
        let mut text = String::new();
        let mut adv = 0.0;
        let step = if self.two_byte { 2 } else { 1 };
        for chunk in s.chunks(step) {
            let code = code_of(chunk);
            adv += self.width(code) / 1000.0 * size + tc + if step == 1 && code == 32 { tw } else { 0.0 };
            self.unicode(code, &mut text);
        }
        (text, adv)
    }
}

// ---- the scan ------------------------------------------------------------------------------------

struct Scan<'a> {
    out: HashMap<i32, Marked>,
    fonts: HashMap<i32, Rc<FontInfo>>,
    ops_left: usize,
    _page: &'a PdfObject,
}

/// What `q` saves and `Q` restores: the CTM and the text state parameters (font, size, spacing,
/// scaling, rise are part of the graphics state).
struct Saved {
    ctm: Matrix,
    font: Option<Rc<FontInfo>>,
    size: f32,
    tc: f32,
    tw: f32,
    th: f32,
    rise: f32,
}

struct State {
    ctm: Matrix,
    stack: Vec<Saved>,
    text: TextPos,
    font: Option<Rc<FontInfo>>,
    size: f32,
    tc: f32,
    tw: f32,
    th: f32,
    rise: f32,
    marks: Vec<Option<i32>>,
    path: Option<Rect>,
}

impl Scan<'_> {
    fn add(&mut self, mcid: Option<i32>, r: Rect) {
        if let Some(m) = mcid {
            let e = self.out.entry(m).or_default();
            e.bbox = Some(union(e.bbox, r));
        }
    }

    fn font(&mut self, res: Option<&PdfObject>, n: &[u8]) -> Result<Option<Rc<FontInfo>>> {
        let Some(f) = res.and_then(|r| r.get_dict("Font").ok().flatten()).and_then(|d| d.get_dict(String::from_utf8_lossy(n).into_owned()).ok().flatten()) else {
            return Ok(None);
        };
        let x = super::tree::xref_of(&f)?;
        if x != 0
            && let Some(i) = self.fonts.get(&x)
        {
            return Ok(Some(i.clone()));
        }
        let info = Rc::new(load_font(&f)?);
        if x != 0 {
            self.fonts.insert(x, info.clone());
        }
        Ok(Some(info))
    }

    /// Runs one content stream. `outer` is the MCID the stream is drawn under when it is a form
    /// (`top` false): marked content inside a form belongs to the form, so its text and paint count
    /// for the MCID of the page content that draws it.
    fn run(&mut self, ops: &[Op], res: Option<&PdfObject>, st: &mut State, top: bool, outer: Option<i32>, depth: usize) -> Result<()> {
        let page_ctm = st.ctm.clone();
        for op in ops {
            if self.ops_left == 0 {
                break;
            }
            self.ops_left -= 1;
            let cur = if top { st.marks.iter().rev().find_map(|m| *m) } else { outer };
            match op.name.as_slice() {
                b"q" => st.stack.push(Saved { ctm: st.ctm.clone(), font: st.font.clone(), size: st.size, tc: st.tc, tw: st.tw, th: st.th, rise: st.rise }),
                b"Q" => match st.stack.pop() {
                    Some(v) => {
                        st.ctm = v.ctm;
                        (st.font, st.size, st.tc, st.tw, st.th, st.rise) = (v.font, v.size, v.tc, v.tw, v.th, v.rise);
                    }
                    None => st.ctm = page_ctm.clone(),
                },
                b"cm" => st.ctm = content::matrix(op) * &st.ctm,
                b"BMC" if top => st.marks.push(None),
                b"BDC" if top => st.marks.push(mcid_of(op, res)?),
                b"EMC" if top => {
                    st.marks.pop();
                }
                b"BT" => st.text.apply(op),
                b"Tf" => {
                    st.size = op.n(1);
                    st.font = match op.operands.first().and_then(Operand::name) {
                        Some(n) => self.font(res, n)?,
                        None => None,
                    };
                }
                b"Tc" => st.tc = op.n(0),
                b"Tw" => st.tw = op.n(0),
                b"Tz" => st.th = op.n(0) / 100.0,
                b"Ts" => st.rise = op.n(0),
                b"Td" | b"TD" | b"Tm" | b"T*" | b"TL" => st.text.apply(op),
                _ if content::shows(op) => {
                    st.text.apply(op);
                    if op.is(b"\"") {
                        st.tw = op.n(0);
                        st.tc = op.n(1);
                    }
                    self.show(op, st, cur);
                }
                b"m" | b"l" | b"c" | b"v" | b"y" | b"re" => {
                    let pts: Vec<(f32, f32)> = match op.name.as_slice() {
                        b"re" => vec![(op.n(0), op.n(1)), (op.n(0) + op.n(2), op.n(1) + op.n(3))],
                        b"m" | b"l" => vec![(op.n(0), op.n(1))],
                        b"c" => vec![(op.n(0), op.n(1)), (op.n(2), op.n(3)), (op.n(4), op.n(5))],
                        _ => vec![(op.n(0), op.n(1)), (op.n(2), op.n(3))],
                    };
                    for (x, y) in pts {
                        let (x, y) = st.ctm.transform_xy(x, y);
                        st.path = Some(union(st.path, Rect { x0: x, y0: y, x1: x, y1: y }));
                    }
                }
                b"n" => st.path = None,
                _ if content::paints_path(op) => {
                    if let Some(r) = st.path.take() {
                        self.add(cur, r);
                    }
                }
                b"BI" => self.add(cur, bbox_of(&st.ctm, UNIT)),
                b"Do" => {
                    let Some(x) = op.operands.first().and_then(Operand::name).and_then(|n| {
                        res.and_then(|r| r.get_dict("XObject").ok().flatten()).and_then(|d| d.get_dict(String::from_utf8_lossy(n).into_owned()).ok().flatten())
                    }) else {
                        continue;
                    };
                    match name(&x, "Subtype")?.as_deref() {
                        Some("Image") => self.add(cur, bbox_of(&st.ctm, UNIT)),
                        Some("Form") if depth < MAX_FORM_DEPTH => {
                            let m = matrix_of(&x)?;
                            let formctm = m * &st.ctm;
                            let inner = dict(&x, "Resources")?;
                            let body = x.read_stream().unwrap_or_default();
                            let ops = content::parse(&body);
                            let mut sub = State {
                                ctm: formctm.clone(),
                                stack: Vec::new(),
                                text: TextPos::default(),
                                font: None,
                                size: 0.0,
                                tc: 0.0,
                                tw: 0.0,
                                th: 1.0,
                                rise: 0.0,
                                marks: Vec::new(),
                                path: None,
                            };
                            let before = cur.and_then(|c| self.out.get(&c)).and_then(|m| m.bbox);
                            self.run(&ops, inner.as_ref().or(res), &mut sub, false, cur, depth + 1)?;
                            // A form that paints nothing we can see still takes its box.
                            if cur.is_some() && cur.and_then(|c| self.out.get(&c)).and_then(|m| m.bbox) == before {
                                let bb = rect_of(&x, "BBox")?.unwrap_or(UNIT);
                                self.add(cur, bbox_of(&formctm, bb));
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn show(&mut self, op: &Op, st: &mut State, mcid: Option<i32>) {
        let Some(font) = st.font.clone() else { return };
        // Pieces in order: strings and TJ adjustments.
        let mut text = String::new();
        let mut adv = 0.0f32;
        let strings: Vec<&Operand> = match op.name.as_slice() {
            b"\"" => op.operands.get(2).into_iter().collect(),
            _ => op.operands.first().into_iter().collect(),
        };
        for o in strings {
            match o {
                Operand::Str(s) => {
                    let (t, a) = font.show(s, st.tc, st.tw, st.size);
                    text.push_str(&t);
                    adv += a * st.th;
                }
                Operand::Array(items) => {
                    for it in items {
                        match it {
                            Operand::Str(s) => {
                                let (t, a) = font.show(s, st.tc, st.tw, st.size);
                                text.push_str(&t);
                                adv += a * st.th;
                            }
                            Operand::Num(n) => adv -= n / 1000.0 * st.size * st.th,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let start = &st.text.tm * &st.ctm;
        let (x0, y0) = start.transform_xy(0.0, 0.0);
        let lo = st.rise + font.descent / 1000.0 * st.size;
        let hi = st.rise + font.ascent / 1000.0 * st.size;
        let (x1, y1) = start.transform_xy(adv, 0.0);
        let r = bbox_of(&start, Rect { x0: 0.0, y0: lo.min(hi), x1: adv.max(0.0), y1: lo.max(hi) });
        // Move the text matrix past what was shown.
        st.text.tm = Matrix::new_translate(adv, 0.0) * &st.text.tm;
        if let Some(m) = mcid {
            let e = self.out.entry(m).or_default();
            e.bbox = Some(union(e.bbox, r));
            if !text.is_empty() {
                let scale = start.a.hypot(start.b).max(0.01);
                let gap = e.end.is_some_and(|(ex, ey)| (y0 - ey).abs() > 1.0 || x0 - ex > 0.2 * st.size * scale);
                if gap && !e.text.is_empty() && !e.text.ends_with(' ') && !text.starts_with(' ') {
                    e.text.push(' ');
                }
                e.text.push_str(&text);
            }
            e.end = Some((x1, y1));
        }
    }
}

/// The content of the marked-content sequences of one page, by MCID (page-level content; text and
/// paint drawn through form XObjects count for the MCID that draws the form).
pub fn scan_page(page: &PdfObject) -> Result<HashMap<i32, Marked>> {
    let res = page.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
    let ops = content::parse(&page_content(page)?);
    let mut scan = Scan { out: HashMap::new(), fonts: HashMap::new(), ops_left: MAX_OPS, _page: page };
    let mut st = State {
        ctm: page.page_ctm()?,
        stack: Vec::new(),
        text: TextPos::default(),
        font: None,
        size: 0.0,
        tc: 0.0,
        tw: 0.0,
        th: 1.0,
        rise: 0.0,
        marks: Vec::new(),
        path: None,
    };
    scan.run(&ops, res.as_ref(), &mut st, true, None, 0)?;
    Ok(scan.out)
}

// ---- the reading order of a page -----------------------------------------------------------------

/// One region of a page's reading order: a structure element that holds marked content on the page.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    /// The element's object number: what the editing commands take. 0 for an element written
    /// directly inside its parent, which can't be addressed (and so not edited).
    pub xref: i32,
    /// The standard type after the role map (P, H1, Figure, ...).
    pub role: String,
    /// The type as written.
    pub raw_role: String,
    /// How deep the element sits in the tree.
    pub depth: u32,
    /// The parent element's object number (0 at the top or when direct).
    pub parent: i32,
    /// Its marked content on this page, in tree order.
    pub mcids: Vec<i32>,
    /// Annotations it holds (links, fields).
    pub annots: Vec<i32>,
    /// Where it is on the page, `[x0, y0, x1, y1]` in points from the top left, when it paints.
    pub bbox: Option<Box4>,
    /// The start of its text.
    pub text: String,
    pub alt: bool,
    pub editable: bool,
}

/// Marked content on the page that no element points at: nothing reads it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Orphan {
    pub mcid: i32,
    pub bbox: Option<Box4>,
    pub text: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReadingOrder {
    pub page: u32,
    /// A tag tree exists.
    pub tagged: bool,
    /// In reading order (the order of the tree).
    pub regions: Vec<Region>,
    pub orphans: Vec<Orphan>,
}

/// The regions of page `page`, in the order a screen reader reads them: the elements of the tree, in
/// tree order, that hold marked content on the page (an element that also has child elements is
/// listed at the place of its first content).
pub fn reading_order(doc: &mupdf::pdf::PdfDocument, page: u32) -> Result<ReadingOrder> {
    let n = doc.page_count()?.max(0) as u32;
    if page >= n {
        return Err(super::bad(format!("page {} is outside the document ({n} pages)", page + 1)));
    }
    let index = super::tree::page_index(doc)?;
    let t = Tree::read(doc, &index)?;
    let page_obj = doc.find_page(page as i32)?;
    let marked = scan_page(&page_obj)?;
    let mut regions = Vec::new();
    let mut claimed = std::collections::HashSet::new();
    let depth_of = |i: usize| t.ancestors(i).count() as u32;
    for i in t.preorder() {
        let e = &t.elems[i];
        let mcids: Vec<i32> = e.refs.iter().filter(|(p, _)| *p == page).map(|(_, m)| *m).collect();
        let annots: Vec<i32> = e.annots.iter().copied().filter(|x| annot_on_page(&page_obj, *x)).collect();
        if mcids.is_empty() && annots.is_empty() {
            continue;
        }
        let mut bbox: Option<Rect> = None;
        let mut text = String::new();
        for m in &mcids {
            claimed.insert(*m);
            if let Some(info) = marked.get(m) {
                if let Some(r) = info.bounds() {
                    bbox = Some(union(bbox, Rect { x0: r[0], y0: r[1], x1: r[2], y1: r[3] }));
                }
                let p = info.preview();
                if !p.is_empty() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&p);
                }
            }
        }
        for x in &annots {
            if let Some(b) = annot_rect(&page_obj, *x) {
                bbox = Some(union(bbox, Rect { x0: b[0], y0: b[1], x1: b[2], y1: b[3] }));
            }
        }
        let b = bbox.map(|r| [r.x0, r.y0, r.x1, r.y1]).filter(|b| b.iter().all(|v| v.is_finite()));
        if text.chars().count() > PREVIEW {
            text = text.chars().take(PREVIEW).collect::<String>() + "…";
        }
        regions.push(Region {
            xref: e.xref,
            role: e.ty.clone(),
            raw_role: e.raw.clone(),
            depth: depth_of(i),
            parent: e.parent.map_or(0, |p| t.elems[p].xref),
            mcids,
            annots,
            bbox: b,
            text,
            alt: e.alt,
            editable: e.xref != 0,
        });
    }
    let mut orphans: Vec<Orphan> = marked
        .iter()
        .filter(|(m, _)| !claimed.contains(*m))
        .map(|(m, info)| Orphan { mcid: *m, bbox: info.bounds(), text: info.preview() })
        .collect();
    orphans.sort_by_key(|o| o.mcid);
    Ok(ReadingOrder { page, tagged: t.exists, regions, orphans })
}

fn annot_on_page(page: &PdfObject, xref: i32) -> bool {
    super::annots(page).is_ok_and(|a| a.iter().any(|(x, _)| *x == xref))
}

fn annot_rect(page: &PdfObject, xref: i32) -> Option<Box4> {
    let (_, a) = super::annots(page).ok()?.into_iter().find(|(x, _)| *x == xref)?;
    super::locate::annot_box(page, &a).ok().flatten()
}
