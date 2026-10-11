// Adapted from pdfcraft (MIT OR Apache-2.0), crates/edit/src/text.rs @ b8ddf71
//! Edit text in place: the text blocks already on a page, and rewriting a block with new text,
//! size, colour, alignment, position or width.
//!
//! A *line* is a run of text-showing operators inside one `BT … ET` on one baseline; a *block*
//! is consecutive lines with the same font, size and colour, aligned and regularly spaced (a
//! paragraph). Rewriting a block removes its show operators and inserts, where it started, one new
//! text object in its own graphics state (`q … Q`) with the text reflowed to the block's width.
//! Every other operator is copied byte for byte; content streams whose bytes changed become new
//! stream objects (a stream may be shared with another page).
//!
//! pdfcraft does this on its own COS document; here the page, its resources and its streams are
//! MuPDF objects, and fallback fonts are MuPDF's built-in standard faces (embedded through
//! `pdf_add_font` when the text needs more than WinAnsi).

use super::content::{Matrix, Obj, Op, Pieces, num, serialize_ops};
use super::font::{Limit, Metrics, STD14};
use crate::engine::EngineError;
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Buffer, Font};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::rc::Rc;

type Result<T> = std::result::Result<T, EngineError>;

fn invalid(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// Horizontal alignment of a block's lines.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// Why a block (or an area of the page) can't be edited as text.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Issue {
    /// Set in a Type 3 font: its glyphs are drawings; new text can't be written in it.
    Type3,
    /// The font says nothing about which characters its codes are (no ToUnicode, no encoding):
    /// the text can't be read, so it can't be edited.
    NoUnicode,
    /// Vertical writing.
    Vertical,
    /// A predefined CJK encoding (CMap) that isn't read.
    Encoding,
    /// The text is used as a clipping path.
    Clip,
    /// Invisible text (render mode 3), usually an OCR layer under a scan.
    Invisible,
    /// On an optional content layer that is hidden.
    Hidden,
    /// The text is drawn by a form XObject (shared content, e.g. a stamp or a template).
    InForm,
    /// The page shows glyph-like shapes but no text: text converted to outlines.
    Outlines,
}

/// Text and graphics state captured for rewriting.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TextState {
    font: Option<(Vec<u8>, f64)>,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    render_mode: i64,
    fill: Vec<Op>,
}

impl TextState {
    fn of(ts: &Ts) -> Self {
        TextState {
            font: ts.font.as_ref().map(|(n, _)| (n.clone(), ts.size)),
            char_spacing: ts.char_spacing,
            word_spacing: ts.word_spacing,
            scale: ts.scale,
            leading: ts.leading,
            rise: ts.rise,
            render_mode: ts.render_mode,
            fill: ts.fill.clone(),
        }
    }

    /// Operators that set this state (inside a text object).
    fn ops(&self) -> Vec<Op> {
        let mut v = Vec::new();
        if let Some((f, size)) = &self.font {
            v.push(Op::new("Tf", vec![Obj::Name(f.clone()), num(*size)]));
        }
        v.push(Op::new("Tc", vec![num(self.char_spacing)]));
        v.push(Op::new("Tw", vec![num(self.word_spacing)]));
        v.push(Op::new("Tz", vec![num(self.scale * 100.0)]));
        v.push(Op::new("TL", vec![num(self.leading)]));
        v.push(Op::new("Ts", vec![num(self.rise)]));
        v.push(Op::new("Tr", vec![num(self.render_mode as f64)]));
        v.extend(self.fill.iter().cloned());
        v
    }
}

#[derive(Clone, Debug)]
struct Origin {
    /// Text matrix where the line starts, and the text line matrix there.
    tm: Matrix,
    tlm: Matrix,
    /// Text space → user space (the y axis scale of `tm × ctm`).
    k: f64,
    ctm: Matrix,
    state: TextState,
    bt_op: usize,
    bt_state: TextState,
}

/// One line of existing text.
#[derive(Clone, Debug)]
pub struct Line {
    pub text: String,
    /// Box in user space.
    pub rect: [f64; 4],
    font: Vec<u8>,
    pub base_font: String,
    /// Size in user space.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub color: [f64; 3],
    pub decodable: bool,
    limit: Option<Limit>,
    render_mode: i64,
    /// Inside optional content that is hidden.
    hidden: bool,
    ops: Vec<usize>,
    origin: Origin,
    /// Baseline start and end in user space.
    start: (f64, f64),
    end: (f64, f64),
}

impl Line {
    fn issue(&self) -> Option<Issue> {
        match self.limit {
            Some(Limit::Type3) => return Some(Issue::Type3),
            Some(Limit::Vertical) => return Some(Issue::Vertical),
            Some(Limit::Encoding) => return Some(Issue::Encoding),
            None => {}
        }
        if self.hidden {
            return Some(Issue::Hidden);
        }
        if self.render_mode >= 4 {
            return Some(Issue::Clip);
        }
        if self.render_mode == 3 {
            return Some(Issue::Invisible);
        }
        if !self.decodable {
            return Some(Issue::NoUnicode);
        }
        None
    }
}

/// A paragraph: consecutive lines of one font, size and colour, aligned and regularly spaced.
#[derive(Clone, Debug)]
pub struct Block {
    pub text: String,
    /// Box in user space.
    pub rect: [f64; 4],
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub color: [f64; 3],
    pub align: Align,
    pub issue: Option<Issue>,
    /// Indexes into the page's lines.
    pub lines: Vec<usize>,
}

/// What the page holds that can't be edited, outside the blocks.
#[derive(Clone, Debug)]
pub struct Area {
    pub rect: [f64; 4],
    pub issue: Issue,
}

#[derive(Clone)]
struct Ts {
    ctm: Matrix,
    fill: Vec<Op>,
    font: Option<(Vec<u8>, Rc<Metrics>)>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    render_mode: i64,
}

impl Ts {
    fn new() -> Self {
        Ts {
            ctm: Matrix::IDENTITY,
            fill: Vec::new(),
            font: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            render_mode: 0,
        }
    }
}

fn fill_color(fill: &[Op]) -> [f64; 3] {
    let Some(op) = fill.iter().rev().find(|op| matches!(op.op.as_slice(), b"g" | b"rg" | b"k")) else {
        return [0.0, 0.0, 0.0];
    };
    let v = |i: usize| op.num(i).unwrap_or(0.0).clamp(0.0, 1.0);
    match op.op.as_slice() {
        b"g" => [v(0); 3],
        b"rg" => [v(0), v(1), v(2)],
        _ => [(1.0 - v(0)) * (1.0 - v(3)), (1.0 - v(1)) * (1.0 - v(3)), (1.0 - v(2)) * (1.0 - v(3))],
    }
}

/// One text-showing operator.
struct Shown {
    op: usize,
    tm: Matrix,
    tlm: Matrix,
    state: Ts,
    bt_op: usize,
    bt_state: Ts,
    text: String,
    rect: [f64; 4],
    start: (f64, f64),
    end: (f64, f64),
    bt: usize,
    font: Vec<u8>,
    size: f64,
    decodable: bool,
    /// How far it moves the text matrix (text space, after horizontal scaling), and the font
    /// size × horizontal scale it was shown with.
    advance: f64,
    em: f64,
    hidden: bool,
}

/// How a show operator moves the text matrix, to keep that move when the operator is removed.
#[derive(Clone, Copy, Debug)]
struct Advance {
    x: f64,
    em: f64,
}

/// The page as read: its content split into the streams' operators, and what it shows.
pub(crate) struct PageContent {
    page: PdfObject,
    streams: Vec<(PdfObject, Vec<u8>)>,
    pieces: Pieces,
    ops: Vec<Op>,
    fonts: Option<PdfObject>,
    /// Every show operator's move, by operator index.
    advances: HashMap<usize, Advance>,
    pub lines: Vec<Line>,
    pub areas: Vec<Area>,
    /// The page's crop box in user space.
    pub crop: [f64; 4],
}

fn get(o: &PdfObject, key: &str) -> Option<PdfObject> {
    o.get_dict(key).ok().flatten()
}

fn nums(o: Option<PdfObject>) -> Vec<f64> {
    let Some(a) = o.filter(|a| a.is_array().unwrap_or(false)) else {
        return Vec::new();
    };
    (0..a.len().unwrap_or(0) as i32).map(|i| a.get_array(i).ok().flatten().and_then(|x| x.as_float().ok()).map_or(0.0, f64::from)).collect()
}

fn page_streams(page: &PdfObject) -> Result<Vec<(PdfObject, Vec<u8>)>> {
    let Some(c) = get(page, "Contents") else {
        return Ok(Vec::new());
    };
    let list: Vec<PdfObject> =
        if c.is_array()? { (0..c.len()? as i32).filter_map(|i| c.get_array(i).ok().flatten()).collect() } else { vec![c] };
    Ok(list
        .into_iter()
        .map(|o| {
            let data = if o.is_stream().unwrap_or(false) { o.read_stream().unwrap_or_default() } else { Vec::new() };
            (o, data)
        })
        .collect())
}

/// Reads page `page` (0-based) of `doc`.
pub(crate) fn read_page(doc: &PdfDocument, page: u32) -> Result<PageContent> {
    let count = doc.page_count()?.max(0) as u32;
    if page >= count {
        return Err(EngineError::BadPage(page));
    }
    let p = doc.find_page(page as i32)?;
    let res = p.get_dict_inheritable("Resources")?;
    let fonts = res.as_ref().and_then(|r| get(r, "Font"));
    let xobjects = res.as_ref().and_then(|r| get(r, "XObject"));
    let streams = page_streams(&p)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse();
    let mut cache = HashMap::new();
    let mut extra = Extra::default();
    let layers = Layers::of(doc, res.as_ref());
    let shown = interpret(&ops, fonts.as_ref(), xobjects.as_ref(), &layers, &mut cache, &mut extra);
    let advances = shown.iter().map(|s| (s.op, Advance { x: s.advance, em: s.em })).collect();
    let mut lines: Vec<Line> = Vec::new();
    let mut last = None;
    for s in shown {
        add_shown(&mut lines, &mut last, s, &cache);
    }
    lines.retain(|l| !l.text.trim().is_empty() || !l.decodable);
    let crop = {
        let b = nums(p.get_dict_inheritable("CropBox")?);
        let b = if b.len() == 4 { b } else { nums(p.get_dict_inheritable("MediaBox")?) };
        if b.len() == 4 { [b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])] } else { [0.0, 0.0, 612.0, 792.0] }
    };
    let mut areas: Vec<Area> = extra.forms.into_iter().map(|rect| Area { rect, issue: Issue::InForm }).collect();
    // Many small filled curves and no text: the text was converted to outlines.
    if lines.is_empty() && extra.glyphlike >= 100 {
        areas.push(Area { rect: extra.glyph_box, issue: Issue::Outlines });
    }
    Ok(PageContent { page: p, streams, pieces, ops, fonts, advances, lines, areas, crop })
}

/// What the interpreter notices besides text.
#[derive(Default)]
struct Extra {
    /// Boxes of form XObjects that show text.
    forms: Vec<[f64; 4]>,
    /// Filled paths the size of glyphs, with curves, and their union.
    glyphlike: usize,
    glyph_box: [f64; 4],
}

/// Which optional content groups are hidden in the default configuration (/OCProperties /D).
pub(crate) struct Layers {
    properties: Option<PdfObject>,
    /// Object numbers listed in /OFF, or with /BaseState /OFF the ones listed in /ON.
    listed: HashSet<i32>,
    base_off: bool,
}

fn ref_numbers(a: Option<PdfObject>) -> HashSet<i32> {
    let Some(a) = a.filter(|a| a.is_array().unwrap_or(false)) else { return HashSet::new() };
    (0..a.len().unwrap_or(0) as i32)
        .filter_map(|i| a.get_array(i).ok().flatten())
        .filter(|o| o.is_indirect().unwrap_or(false))
        .filter_map(|o| o.as_indirect().ok())
        .collect()
}

impl Layers {
    fn of(doc: &PdfDocument, res: Option<&PdfObject>) -> Layers {
        let d = doc.catalog().ok().and_then(|c| get(&c, "OCProperties")).and_then(|p| get(&p, "D"));
        let base_off = d.as_ref().and_then(|d| get(d, "BaseState")).and_then(|b| b.as_name().ok()).as_deref() == Some(b"OFF");
        let listed = ref_numbers(d.as_ref().and_then(|d| get(d, if base_off { "ON" } else { "OFF" })));
        Layers { properties: res.and_then(|r| get(r, "Properties")), listed, base_off }
    }

    fn off(&self, ocg: &PdfObject) -> bool {
        let Some(n) = ocg.is_indirect().ok().filter(|i| *i).and_then(|_| ocg.as_indirect().ok()) else { return false };
        self.listed.contains(&n) != self.base_off
    }

    /// Whether the `/OC` property `operand` of a `BDC` names hidden content: an OCG that is
    /// off, or a membership dictionary whose policy (/P, AnyOn by default) leaves it off.
    fn hidden(&self, operand: Option<&Obj>) -> bool {
        let Some(Obj::Name(n)) = operand else { return false };
        let Some(r) = self.properties.as_ref().and_then(|p| get(p, &String::from_utf8_lossy(n))) else { return false };
        let kind = get(&r, "Type").and_then(|t| t.as_name().ok());
        if kind.as_deref() != Some(b"OCMD") {
            return self.off(&r);
        }
        let groups: Vec<PdfObject> = match get(&r, "OCGs") {
            Some(g) if g.is_array().unwrap_or(false) => {
                (0..g.len().unwrap_or(0) as i32).filter_map(|i| g.get_array(i).ok().flatten()).collect()
            }
            Some(g) => vec![g],
            None => return false,
        };
        if groups.is_empty() {
            return false;
        }
        let off: Vec<bool> = groups.iter().map(|g| self.off(g)).collect();
        match get(&r, "P").and_then(|p| p.as_name().ok()).as_deref() {
            Some(b"AllOn") => off.iter().any(|o| *o),
            Some(b"AnyOff") => !off.iter().any(|o| *o),
            Some(b"AllOff") => !off.iter().all(|o| *o),
            _ => off.iter().all(|o| *o),
        }
    }
}

fn metrics_for(fonts: Option<&PdfObject>, name: &[u8], cache: &mut HashMap<Vec<u8>, Rc<Metrics>>) -> Rc<Metrics> {
    cache
        .entry(name.to_vec())
        .or_insert_with(|| {
            let key = String::from_utf8_lossy(name).into_owned();
            Rc::new(
                fonts
                    .and_then(|f| get(f, &key))
                    .filter(|d| d.is_dict().unwrap_or(false))
                    .map(|d| Metrics::from_dict(&d))
                    .unwrap_or_else(Metrics::fallback),
            )
        })
        .clone()
}

/// Whether a form XObject's content shows text (one level deep).
fn form_shows_text(form: &PdfObject) -> bool {
    let Ok(data) = form.read_stream() else {
        return false;
    };
    super::content::parse(&data).iter().any(|o| matches!(o.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\""))
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

const EMPTY: [f64; 4] = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];

fn interpret(
    ops: &[Op],
    fonts: Option<&PdfObject>,
    xobjects: Option<&PdfObject>,
    layers: &Layers,
    cache: &mut HashMap<Vec<u8>, Rc<Metrics>>,
    extra: &mut Extra,
) -> Vec<Shown> {
    let mut out = Vec::new();
    // Marked content: whether each open sequence is hidden optional content.
    let mut marked: Vec<bool> = Vec::new();
    let mut ts = Ts::new();
    let mut at_bt = (0usize, ts.clone());
    let mut stack: Vec<Ts> = Vec::new();
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let mut bt = 0usize;
    // The current path: its box in user space and whether it has curves.
    let mut path = (EMPTY, false);
    extra.glyph_box = EMPTY;
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"BMC" => marked.push(false),
            b"BDC" => marked.push(op.name(0) == Some(b"OC") && layers.hidden(op.operands.get(1))),
            b"EMC" => {
                marked.pop();
            }
            b"q" => stack.push(ts.clone()),
            b"Q" => {
                if let Some(s) = stack.pop() {
                    ts = s;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    ts.ctm = Matrix(m).then(&ts.ctm);
                }
            }
            b"m" | b"l" => {
                if let Some([x, y]) = op.nums::<2>() {
                    let (x, y) = ts.ctm.apply(x, y);
                    path.0 = union(path.0, [x, y, x, y]);
                }
            }
            b"c" | b"v" | b"y" => {
                for pair in op.operands.chunks(2) {
                    if let (Some(x), Some(y)) = (pair[0].as_f64(), pair.get(1).and_then(Obj::as_f64)) {
                        let (x, y) = ts.ctm.apply(x, y);
                        path.0 = union(path.0, [x, y, x, y]);
                    }
                }
                path.1 = true;
            }
            b"re" => {
                if let Some([x, y, w, h]) = op.nums::<4>() {
                    path.0 = union(path.0, ts.ctm.bbox([x, y, x + w, y + h]));
                }
            }
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"n" => {
                let [x0, y0, x1, y1] = path.0;
                let filled = !matches!(op.op.as_slice(), b"S" | b"s" | b"n");
                if filled && path.1 && x1 >= x0 && (2.0..60.0).contains(&(y1 - y0)) && x1 - x0 < 200.0 {
                    extra.glyphlike += 1;
                    extra.glyph_box = union(extra.glyph_box, path.0);
                }
                path = (EMPTY, false);
            }
            b"BT" => {
                tm = Matrix::IDENTITY;
                tlm = Matrix::IDENTITY;
                bt += 1;
                at_bt = (i, ts.clone());
            }
            b"g" | b"rg" | b"k" | b"cs" => ts.fill = vec![op.clone()],
            b"sc" | b"scn" => {
                ts.fill.retain(|o| o.is("cs"));
                ts.fill.push(op.clone());
            }
            b"Tf" => {
                ts.size = op.num(1).unwrap_or(ts.size);
                if let Some(name) = op.name(0) {
                    let m = metrics_for(fonts, name, cache);
                    ts.font = Some((name.to_vec(), m));
                }
            }
            b"Tc" => ts.char_spacing = op.num(0).unwrap_or(0.0),
            b"Tw" => ts.word_spacing = op.num(0).unwrap_or(0.0),
            b"Tz" => ts.scale = op.num(0).unwrap_or(100.0) / 100.0,
            b"TL" => ts.leading = op.num(0).unwrap_or(0.0),
            b"Ts" => ts.rise = op.num(0).unwrap_or(0.0),
            b"Tr" => ts.render_mode = op.num(0).map_or(0, |v| v as i64),
            b"Do" => {
                let form = op.name(0).and_then(|n| xobjects.and_then(|x| get(x, &String::from_utf8_lossy(n))));
                if let Some(f) = form.filter(|f| get(f, "Subtype").and_then(|s| s.as_name().ok()).as_deref() == Some(b"Form")) {
                    if form_shows_text(&f) {
                        let b = nums(get(&f, "BBox"));
                        let m = nums(get(&f, "Matrix"));
                        let m = if m.len() == 6 { Matrix([m[0], m[1], m[2], m[3], m[4], m[5]]) } else { Matrix::IDENTITY };
                        if b.len() == 4 {
                            extra.forms.push(m.then(&ts.ctm).bbox([b[0], b[1], b[2], b[3]]));
                        }
                    }
                }
            }
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.is("TD") {
                        ts.leading = -y;
                    }
                    tlm = Matrix::translate(x, y).then(&tlm);
                    tm = tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    tlm = Matrix(m);
                    tm = tlm;
                }
            }
            b"T*" => {
                tlm = Matrix::translate(0.0, -ts.leading).then(&tlm);
                tm = tlm;
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                if matches!(op.op.as_slice(), b"'" | b"\"") {
                    if op.is("\"") {
                        ts.word_spacing = op.num(0).unwrap_or(ts.word_spacing);
                        ts.char_spacing = op.num(1).unwrap_or(ts.char_spacing);
                    }
                    tlm = Matrix::translate(0.0, -ts.leading).then(&tlm);
                    tm = tlm;
                }
                let Some((name, m)) = ts.font.clone() else {
                    continue;
                };
                let pieces: Vec<Obj> = match op.op.as_slice() {
                    b"TJ" => match op.operands.first() {
                        Some(Obj::Array(a)) => a.clone(),
                        _ => Vec::new(),
                    },
                    _ => op.operands.last().cloned().into_iter().collect(),
                };
                let trm0 = tm.then(&ts.ctm);
                let start = trm0.apply(0.0, ts.rise);
                let mut text = String::new();
                let mut decodable = m.has_to_unicode;
                let mut x_text = 0.0;
                for p in &pieces {
                    match p {
                        Obj::Str(s) => {
                            for (code, len) in m.codes(s) {
                                match m.text_of(code) {
                                    Some(t) => text.push_str(t),
                                    None => decodable = false,
                                }
                                let w =
                                    m.width(code) * ts.size + ts.char_spacing + if m.is_space(code, len) { ts.word_spacing } else { 0.0 };
                                x_text += w * ts.scale;
                            }
                        }
                        other => {
                            if let Some(n) = other.as_f64() {
                                let dx = -n / 1000.0 * ts.size * ts.scale;
                                // A large gap inside TJ reads as a space.
                                if dx > ts.size * 0.2 && !text.ends_with(' ') {
                                    text.push(' ');
                                }
                                x_text += dx;
                            }
                        }
                    }
                }
                let begin = tm;
                tm = Matrix::translate(x_text, 0.0).then(&tm);
                let end = tm.then(&ts.ctm).apply(0.0, ts.rise);
                if !pieces.iter().any(|p| matches!(p, Obj::Str(s) if !s.is_empty())) {
                    continue;
                }
                let corners = [
                    trm0.apply(0.0, ts.rise + m.descent * ts.size),
                    trm0.apply(x_text, ts.rise + m.descent * ts.size),
                    trm0.apply(0.0, ts.rise + m.ascent * ts.size),
                    trm0.apply(x_text, ts.rise + m.ascent * ts.size),
                ];
                let rect = corners.iter().fold(EMPTY, |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]);
                let size_user = (trm0.0[2].powi(2) + trm0.0[3].powi(2)).sqrt() * ts.size;
                out.push(Shown {
                    op: i,
                    tm: begin,
                    tlm,
                    state: ts.clone(),
                    bt_op: at_bt.0,
                    bt_state: at_bt.1.clone(),
                    text,
                    rect,
                    start,
                    end,
                    bt,
                    font: name,
                    size: if size_user > 0.0 { size_user } else { ts.size },
                    decodable,
                    advance: x_text,
                    em: ts.size * ts.scale,
                    hidden: marked.iter().any(|h| *h),
                });
            }
            _ => {}
        }
    }
    out
}

/// Adds `s` to the line it continues, or starts a new one. `last` is the previous run's
/// (`bt`, end point, size).
fn add_shown(lines: &mut Vec<Line>, last: &mut Option<(usize, (f64, f64), f64)>, s: Shown, cache: &HashMap<Vec<u8>, Rc<Metrics>>) {
    let color = fill_color(&s.state.fill);
    let m = cache.get(&s.font).cloned().unwrap_or_else(|| Rc::new(Metrics::fallback()));
    // Distance from the previous run's end, along and across the line (in its text direction).
    let along = |l: &Line, end: (f64, f64)| -> Option<(f64, f64)> {
        let inv = l.origin.tm.then(&l.origin.ctm).invert()?;
        let (ex, ey) = inv.apply(end.0, end.1);
        let (sx, sy) = inv.apply(s.start.0, s.start.1);
        Some(((sx - ex) * l.origin.k, (sy - ey) * l.origin.k))
    };
    let joins = last.is_some_and(|(bt, end, size)| {
        let Some(l) = lines.last() else { return false };
        bt == s.bt
            && l.font == s.font
            && (s.size - l.size).abs() < 0.01
            && l.color == color
            && l.render_mode == s.state.render_mode
            && l.hidden == s.hidden
            && along(l, end).is_some_and(|(dx, dy)| dy.abs() < size * 0.3 && dx > -size && dx < size * 3.0)
    });
    if joins {
        let (_, end, size) = last.expect("checked");
        let l = lines.last_mut().expect("checked");
        let gap = along(l, end).map_or(0.0, |d| d.0);
        if gap > size * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ') {
            l.text.push(' ');
        }
        l.text.push_str(&s.text);
        l.rect = union(l.rect, s.rect);
        l.ops.push(s.op);
        l.decodable &= s.decodable;
        l.end = s.end;
    } else {
        let trm = s.tm.then(&s.state.ctm);
        lines.push(Line {
            text: s.text.clone(),
            rect: s.rect,
            font: s.font.clone(),
            base_font: m.base_font.clone(),
            size: s.size,
            bold: m.bold,
            italic: m.italic,
            color,
            decodable: s.decodable,
            limit: m.limit,
            render_mode: s.state.render_mode,
            hidden: s.hidden,
            ops: vec![s.op],
            origin: Origin {
                tm: s.tm,
                tlm: s.tlm,
                k: (trm.0[2].powi(2) + trm.0[3].powi(2)).sqrt(),
                ctm: s.state.ctm,
                state: TextState::of(&s.state),
                bt_op: s.bt_op,
                bt_state: TextState::of(&s.bt_state),
            },
            start: s.start,
            end: s.end,
        });
    }
    *last = Some((s.bt, s.end, s.size.max(1.0)));
}

/// A line's start and end along the text direction of `first` (text space x of `first`).
fn span_in(first: &Line, l: &Line) -> (f64, f64, f64) {
    let Some(inv) = first.origin.tm.then(&first.origin.ctm).invert() else {
        return (0.0, 0.0, 0.0);
    };
    let (x0, y0) = inv.apply(l.start.0, l.start.1);
    let (x1, _) = inv.apply(l.end.0, l.end.1);
    (x0, x1, y0)
}

fn paragraph_separator(c: char) -> bool {
    c.is_whitespace() && c != '\u{3000}'
}

/// Groups lines into paragraphs.
pub fn group_blocks(lines: &[Line]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut gap: Option<f64> = None;
    for (i, l) in lines.iter().enumerate() {
        let joins = blocks.last().is_some_and(|b| {
            let Some(&p) = b.lines.last() else {
                return false;
            };
            let prev = &lines[p];
            let first = &lines[b.lines[0]];
            let (px0, px1, py) = span_in(first, prev);
            let (lx0, lx1, ly) = span_in(first, l);
            // Line gap in user space, along the first line's up direction.
            let g = (py - ly) * first.origin.k;
            let tol = l.size.max(1.0) / first.origin.k.max(1e-6);
            let aligned = (px0 - lx0).abs() < tol || ((px0 + px1) / 2.0 - (lx0 + lx1) / 2.0).abs() < tol || (px1 - lx1).abs() < tol;
            prev.font == l.font
                && prev.color == l.color
                && prev.render_mode == l.render_mode
                && prev.hidden == l.hidden
                && (prev.size - l.size).abs() < 0.01
                && aligned
                && g > l.size * 0.8
                && g < l.size * 2.5
                && gap.is_none_or(|first| (g - first).abs() < first * 0.2)
        });
        if joins {
            let b = blocks.last_mut().expect("checked");
            let first = &lines[b.lines[0]];
            let prev = &lines[*b.lines.last().expect("not empty")];
            gap.get_or_insert((span_in(first, prev).2 - span_in(first, l).2) * first.origin.k);
            if b.text.ends_with('-') {
                b.text.pop();
            } else {
                b.text.push(' ');
            }
            b.text.push_str(l.text.trim_matches(paragraph_separator));
            b.rect = union(b.rect, l.rect);
            b.lines.push(i);
        } else {
            gap = None;
            blocks.push(Block {
                text: l.text.trim_matches(paragraph_separator).to_string(),
                rect: l.rect,
                base_font: l.base_font.clone(),
                size: l.size,
                bold: l.bold,
                italic: l.italic,
                color: l.color,
                align: Align::Left,
                issue: l.issue(),
                lines: vec![i],
            });
        }
    }
    for b in &mut blocks {
        b.align = detect_align(lines, &b.lines);
        if b.issue.is_none() {
            b.issue = b.lines.iter().find_map(|&i| lines[i].issue());
        }
    }
    blocks
}

fn detect_align(lines: &[Line], members: &[usize]) -> Align {
    if members.len() < 2 {
        return Align::Left;
    }
    let first = &lines[members[0]];
    let spans: Vec<(f64, f64)> = members.iter().map(|&i| span_in(first, &lines[i])).map(|(a, b, _)| (a, b)).collect();
    let tol = first.size.max(1.0) * 0.25 / first.origin.k.max(1e-6);
    let same = |f: &dyn Fn(&(f64, f64)) -> f64, s: &[(f64, f64)]| s.iter().all(|x| (f(x) - f(&s[0])).abs() < tol);
    let left = same(&|x| x.0, &spans);
    let right = same(&|x| x.1, &spans);
    if left && right {
        return Align::Justify;
    }
    if left {
        // Every line but the last fills the width: justified.
        let body = &spans[..spans.len() - 1];
        return if spans.len() >= 3 && same(&|x| x.1, body) { Align::Justify } else { Align::Left };
    }
    if right {
        return Align::Right;
    }
    if same(&|x| (x.0 + x.1) / 2.0, &spans) {
        return Align::Center;
    }
    Align::Left
}

/// Greedy word wrapping to `width` with `advance` giving a string's width; `\n` breaks a line.
/// Each line comes with whether it ends a paragraph.
fn wrap(text: &str, width: f64, advance: impl Fn(&str) -> f64) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(paragraph_separator).filter(|w| !w.is_empty()) {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && advance(&candidate) > width {
                out.push((std::mem::take(&mut line), false));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        out.push((line, true));
    }
    out
}

/// A change to one block. Unset fields keep the block's own.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TextChange {
    /// New text; `\n` starts a new line. The rest is reflowed to the block's width.
    pub text: Option<String>,
    /// Font size in points.
    pub size: Option<f64>,
    /// Fill colour, RGB 0..1.
    pub color: Option<[f64; 3]>,
    pub align: Option<Align>,
    /// Move by `[dx, dy]` in user space.
    pub offset: Option<[f64; 2]>,
    /// Reflow to this width in user space.
    pub width: Option<f64>,
}

/// The font new text was set in when the block's own could not show it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Fallback {
    /// The font used instead (`Helvetica-Bold`, …).
    pub font: String,
    /// Embedded into the file (the text needs more than WinAnsi), or a standard font every
    /// viewer has.
    pub embedded: bool,
    /// Why: the original font and the first character it could not show.
    pub reason: String,
}

/// How new text is written.
enum Face {
    /// The block's own font.
    Own { name: Vec<u8>, m: Rc<Metrics>, space: bool },
    /// A standard 14 font in WinAnsiEncoding (not embedded).
    Std { name: Vec<u8>, font: Font },
    /// A standard face embedded as Type0/Identity-H (codes are glyph ids).
    Embedded { name: Vec<u8>, font: Font },
}

/// WinAnsiEncoding code of `c`.
fn win_ansi_byte(c: char) -> Option<u8> {
    match c {
        '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => u8::try_from(u32::from(c)).ok(),
        _ => {
            let high = &super::encodings::WIN_ANSI[0x80..0xa0];
            let i = high.iter().position(|&u| u != 0 && u == u32::from(c))?;
            u8::try_from(0x80 + i).ok()
        }
    }
}

impl Face {
    fn name(&self) -> &[u8] {
        match self {
            Face::Own { name, .. } | Face::Std { name, .. } | Face::Embedded { name, .. } => name,
        }
    }

    /// Whether a space is shown with a space code (else words are placed apart with TJ gaps).
    fn has_space(&self) -> bool {
        match self {
            Face::Own { space, .. } => *space,
            _ => true,
        }
    }

    fn encode(&self, s: &str) -> Option<Vec<u8>> {
        match self {
            Face::Own { m, .. } => m.encode(s),
            Face::Std { .. } => s.chars().map(win_ansi_byte).collect(),
            Face::Embedded { font, .. } => {
                let mut out = Vec::new();
                for c in s.chars() {
                    let g = font.encode_character(c as i32).ok().filter(|g| *g > 0 || c == ' ')?;
                    out.extend_from_slice(&(g as u16).to_be_bytes());
                }
                Some(out)
            }
        }
    }

    /// Width of `s` (no spaces when `!has_space`) in text space before horizontal scaling.
    fn advance(&self, s: &str, size: f64, cs: f64, ws: f64) -> f64 {
        match self {
            Face::Own { m, .. } => m
                .encode(s)
                .map(|b| m.codes(&b).iter().map(|(c, l)| m.width(*c) * size + cs + if m.is_space(*c, *l) { ws } else { 0.0 }).sum())
                .unwrap_or(0.0),
            Face::Std { font, .. } | Face::Embedded { font, .. } => s
                .chars()
                .map(|c| {
                    let w = font.encode_character(c as i32).ok().and_then(|g| font.advance_glyph(g).ok()).map_or(0.5, f64::from);
                    w * size + cs + if c == ' ' { ws } else { 0.0 }
                })
                .sum(),
        }
    }
}

/// A font resource name not yet in `fonts`.
fn free_name(fonts: Option<&PdfObject>, base: &str) -> String {
    (0..).map(|i| format!("{base}{i}")).find(|n| fonts.is_none_or(|f| get(f, n).is_none())).expect("a free name")
}

/// The page's /Resources /Font dict, created if missing (shared resources are extended in place:
/// a new key is unused by every other page).
fn font_dict(doc: &mut PdfDocument, page: &PdfObject) -> Result<PdfObject> {
    let mut res = match page.get_dict_inheritable("Resources")? {
        Some(r) if r.is_dict()? => r,
        _ => {
            let r = doc.new_dict()?;
            page.clone().dict_put("Resources", r)?;
            get(page, "Resources").ok_or_else(|| invalid("the page has no resources"))?
        }
    };
    match get(&res, "Font") {
        Some(f) if f.is_dict()? => Ok(f),
        _ => {
            res.dict_put("Font", doc.new_dict()?)?;
            get(&res, "Font").ok_or_else(|| invalid("the page has no fonts"))
        }
    }
}

/// The standard 14 family closest to `base`: 0 Courier, 1 Times, 2 Helvetica.
fn family_of(base: &str) -> usize {
    let n = base.to_ascii_lowercase();
    if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console", "cmtt"].iter().any(|s| n.contains(s)) {
        0
    } else if !n.contains("sans")
        && ["times", "serif", "roman", "mincho", "cambria", "georgia", "palatino", "garamond", "cmr", "minion", "book"]
            .iter()
            .any(|s| n.contains(s))
    {
        1
    } else {
        2
    }
}

/// Chooses the face for `text`: the block's own font when it has every glyph, else a standard
/// font, else an embedded standard face.
fn choose_face(
    doc: &mut PdfDocument,
    pc: &PageContent,
    first: &Line,
    text: &str,
    cache_font: Rc<Metrics>,
) -> Result<(Face, Option<Fallback>)> {
    let words: String = text.chars().filter(|c| *c != ' ' && *c != '\n').collect();
    if cache_font.limit.is_none() && first.decodable && cache_font.encode(&words).is_some() {
        let space = cache_font.encode(" ").is_some();
        return Ok((Face::Own { name: first.font.clone(), m: cache_font, space }, None));
    }
    let missing = text.chars().find(|c| *c != '\n' && *c != ' ' && cache_font.encode(&c.to_string()).is_none());
    let why = match (cache_font.limit, missing) {
        (Some(Limit::Type3), _) => format!("{} is a Type 3 font", first.base_font),
        (_, Some(c)) if cache_font.subset => format!("the embedded subset of {} has no glyph for '{c}'", first.base_font),
        (_, Some(c)) => format!("{} has no glyph for '{c}'", first.base_font),
        _ => format!("{} can't show the new text", first.base_font),
    };
    let std = STD14[family_of(&first.base_font)][first.bold as usize + 2 * first.italic as usize];
    let font = Font::new(std)?;
    let fonts = font_dict(doc, &pc.page)?;
    let name = free_name(Some(&fonts), "IwEd");
    let mut fonts = fonts;
    if text.chars().filter(|c| *c != '\n').all(|c| win_ansi_byte(c).is_some()) {
        let mut f = doc.new_dict()?;
        f.dict_put("Type", PdfObject::new_name("Font")?)?;
        f.dict_put("Subtype", PdfObject::new_name("Type1")?)?;
        f.dict_put("BaseFont", PdfObject::new_name(std)?)?;
        f.dict_put("Encoding", PdfObject::new_name("WinAnsiEncoding")?)?;
        let r = doc.add_object(&f)?;
        fonts.dict_put(name.as_str(), r)?;
        let fb = Fallback { font: std.into(), embedded: false, reason: why };
        return Ok((Face::Std { name: name.into_bytes(), font }, Some(fb)));
    }
    if let Some(c) = text.chars().find(|c| *c != '\n' && *c != ' ' && font.encode_character(*c as i32).map_or(true, |g| g <= 0)) {
        return Err(invalid(format!("no available font can show '{c}' (U+{:04X})", c as u32)));
    }
    let r = doc.add_font(&font)?;
    fonts.dict_put(name.as_str(), r)?;
    let fb = Fallback { font: std.into(), embedded: true, reason: why };
    Ok((Face::Embedded { name: name.into_bytes(), font }, Some(fb)))
}

/// The operators of every line drawn in the same area as one of `rects` (fake bold, a second
/// copy): a surviving copy would show the old text under the new.
fn coincident_ops(lines: &[Line], rects: &[[f64; 4]]) -> HashSet<usize> {
    let area = |r: [f64; 4]| ((r[2] - r[0]) * (r[3] - r[1])).max(0.0);
    let mut drop = HashSet::new();
    for l in lines {
        let covered = rects.iter().any(|m| {
            let ix = (m[2].min(l.rect[2]) - m[0].max(l.rect[0])).max(0.0);
            let iy = (m[3].min(l.rect[3]) - m[1].max(l.rect[1])).max(0.0);
            let small = area(*m).min(area(l.rect));
            small > 0.0 && ix * iy / small > 0.5
        });
        if covered && l.render_mode != 3 {
            drop.extend(l.ops.iter().copied());
        }
    }
    drop
}

/// Rewrites block `index` of page `page` with `change`. Returns the fallback font used, if any.
pub fn rewrite_block(doc: &mut PdfDocument, page: u32, index: usize, change: &TextChange) -> Result<Option<Fallback>> {
    let pc = read_page(doc, page)?;
    let blocks = group_blocks(&pc.lines);
    let b = blocks.get(index).cloned().ok_or_else(|| invalid(format!("page {} has no text block {}", page + 1, index + 1)))?;
    if let Some(issue) = b.issue {
        return Err(invalid(format!("this text can't be edited: {}", issue_text(issue))));
    }
    let members: Vec<&Line> = b.lines.iter().map(|&i| &pc.lines[i]).collect();
    let first = members[0];
    let o = &first.origin;
    let (_, old_size) = o.state.font.clone().ok_or_else(|| invalid("the text has no font"))?;
    let k = o.k.max(1e-6);
    let size = match change.size {
        Some(pt) if pt.is_finite() && pt > 0.0 => (pt / k).max(0.1),
        Some(_) => return Err(invalid("the font size must be a positive number")),
        None => old_size,
    };
    // The text: new, or the block's own (single spaces between words, hard breaks kept).
    let raw = change.text.clone().unwrap_or_else(|| b.text.clone());
    let text = raw
        .replace("\r\n", "\n")
        .split('\n')
        .map(|p| p.split(paragraph_separator).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    let mut cache = HashMap::new();
    let metrics = metrics_for(pc.fonts.as_ref(), &first.font, &mut cache);
    let (face, fallback) = choose_face(doc, &pc, first, &text, metrics)?;
    let st = &o.state;
    let advance = |s: &str| -> f64 {
        let t = if face.has_space() {
            face.advance(s, size, st.char_spacing, st.word_spacing)
        } else {
            let spaces = s.matches(' ').count() as f64;
            face.advance(&s.replace(' ', ""), size, st.char_spacing, 0.0) + spaces * size * 0.25
        };
        t * st.scale * k
    };
    // The block's extent along its first line, in that line's text space.
    let spans: Vec<(f64, f64, f64)> = members.iter().map(|l| span_in(first, l)).collect();
    let mut left = spans.iter().map(|s| s.0).fold(f64::MAX, f64::min);
    let right = spans.iter().map(|s| s.1).fold(f64::MIN, f64::max);
    let align = change.align.unwrap_or(b.align);
    let own = ((right - left) * k).max(size * k);
    let upright = {
        let t = o.tm.then(&o.ctm).0;
        t[1].abs() < 1e-6 && t[2].abs() < 1e-6 && t[0] > 0.0
    };
    let mut width = match change.width {
        Some(w) if w.is_finite() && w > 0.0 => w.clamp(size * k, 14_400.0),
        Some(_) => return Err(invalid("the width must be a positive number")),
        // A single left-aligned line grows to the right, up to the page's margin.
        None if members.len() == 1 && align == Align::Left && upright => own.max(pc.crop[2] - 36.0 - b.rect[0]),
        None => own,
    };
    let mut wrapped = wrap(&text, width + 0.5, advance);
    if change.width.is_none() && members.len() == 1 && align == Align::Left && upright {
        width = wrapped.iter().map(|l| advance(&l.0)).fold(own, f64::max).min(width);
    }
    // A word longer than the width widens the box, keeping the aligned edge.
    let widest = wrapped.iter().map(|l| advance(&l.0)).fold(0.0, f64::max);
    if widest > width {
        let extra = widest - width;
        match align {
            Align::Center => left -= extra / 2.0 / k,
            Align::Right => left -= extra / k,
            _ => {}
        }
        width = widest;
        wrapped = wrap(&text, width + 0.5, advance);
    }
    let lead = if members.len() > 1 {
        (spans[0].2 - spans[1].2) * size / old_size
    } else if st.leading > 0.0 {
        st.leading * size / old_size
    } else {
        size * 1.2
    };
    let mut block_ops = vec![Op::new("q", vec![])];
    if let Some([dx, dy]) = change.offset {
        if !(dx.is_finite() && dy.is_finite()) || dx.abs() > 1e5 || dy.abs() > 1e5 {
            return Err(invalid("the text can't be moved that far"));
        }
        let c = o.ctm.0;
        let back = Matrix([c[0], c[1], c[2], c[3], 0.0, 0.0])
            .invert()
            .ok_or_else(|| invalid("the text is drawn in a space it can't be moved in"))?;
        let (ax, ay) = back.apply(dx, dy);
        block_ops.push(Op::new("cm", vec![num(1.0), num(0.0), num(0.0), num(1.0), num(ax), num(ay)]));
    }
    // Acrobat's marker for edited text.
    block_ops.push(Op::new("MP", vec![Obj::name("TouchUp_TextEdit")]));
    block_ops.push(Op::new("BT", vec![]));
    let mut state = st.clone();
    state.word_spacing = 0.0;
    state.font = Some((face.name().to_vec(), size));
    if let Some([r, g, bl]) = change.color {
        if ![r, g, bl].iter().all(|v| (0.0..=1.0).contains(v)) {
            return Err(invalid("colour components must be between 0 and 1"));
        }
        state.fill = vec![Op::new("rg", vec![num(r), num(g), num(bl)])];
    }
    block_ops.extend(state.ops());
    let origin = Matrix::translate(left, 0.0).then(&o.tm);
    block_ops.push(Op::new("Tm", origin.0.iter().map(|v| num(*v)).collect()));
    let mut x = 0.0;
    let mut tw_set = 0.0;
    for (i, (line, last_of_para)) in wrapped.iter().enumerate() {
        let adv = advance(line);
        let free = (width - adv).max(0.0) / k;
        let dx = match align {
            Align::Center => free / 2.0,
            Align::Right => free,
            _ => 0.0,
        };
        if i > 0 || dx != 0.0 {
            block_ops.push(Op::new("Td", vec![num(dx - x), num(if i > 0 { -lead } else { 0.0 })]));
        }
        x = dx;
        let spaces = line.matches(' ').count();
        // Justified lines (all but a paragraph's last) spread the free width over their spaces.
        let extra = if align == Align::Justify && !*last_of_para && spaces > 0 { free / spaces as f64 } else { 0.0 };
        if line.is_empty() {
            continue;
        }
        let single_byte_space = matches!(&face, Face::Own { m, .. } if !m.composite) || matches!(face, Face::Std { .. });
        if face.has_space() && (extra == 0.0 || single_byte_space) {
            // Word spacing (code 32) justifies; it is in unscaled text space.
            let tw = extra / st.scale;
            if tw != tw_set {
                block_ops.push(Op::new("Tw", vec![num(tw)]));
                tw_set = tw;
            }
            let bytes = face.encode(line).ok_or_else(|| invalid(format!("\"{line}\" can't be shown")))?;
            block_ops.push(Op::new("Tj", vec![Obj::Str(bytes)]));
        } else {
            // Words placed apart: the font has no space glyph (TeX fonts), or justified
            // two-byte text.
            let mut arr = Vec::new();
            for (w, word) in line.split(' ').enumerate() {
                if w > 0 {
                    // The gap after horizontal scaling: a quarter em for a missing space glyph,
                    // plus the justification.
                    if face.has_space() {
                        arr.push(Obj::Str(face.encode(" ").unwrap_or_default()));
                    }
                    let gap = if face.has_space() { 0.0 } else { size * 0.25 * st.scale } + extra;
                    if gap != 0.0 {
                        arr.push(num(-gap * 1000.0 / (size * st.scale)));
                    }
                }
                if !word.is_empty() {
                    arr.push(Obj::Str(face.encode(word).ok_or_else(|| invalid(format!("\"{word}\" can't be shown")))?));
                }
            }
            block_ops.push(Op::new("TJ", vec![Obj::Array(arr)]));
        }
    }
    block_ops.push(Op::new("ET", vec![]));
    block_ops.push(Op::new("Q", vec![]));
    // The block's operators go, and anything drawn in the same area.
    let member_rects: Vec<[f64; 4]> = members.iter().map(|l| l.rect).collect();
    let mut drop = coincident_ops(&pc.lines, &member_rects);
    drop.extend(members.iter().flat_map(|l| l.ops.iter().copied()));
    // Text shown earlier in the same text object stays first: the block goes between two halves
    // of the text object, keeping its place in reading order.
    let start = first.ops.first().copied().unwrap_or(o.bt_op);
    let split = pc
        .ops
        .get(o.bt_op..start)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .any(|(i, op)| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") && !drop.contains(&(o.bt_op + i)));
    let after = if split { &o.state } else { &o.bt_state };
    if split {
        block_ops.insert(0, Op::new("ET", vec![]));
        // Text state operators belong inside the reopened text object.
        block_ops.push(Op::new("BT", vec![]));
        block_ops.extend(after.ops().into_iter().filter(|op| !op.is("Tf") || after.font.is_some()));
        // Back where the removed operator started. A ' or " moves to the next line itself (its
        // stand-in below does): start from the line before it.
        if matches!(pc.ops.get(start).map(|op| op.op.as_slice()), Some(b"'" | b"\"")) {
            let before = Matrix::translate(0.0, o.state.leading).then(&o.tlm);
            block_ops.push(Op::new("Tm", before.0.iter().map(|v| num(*v)).collect()));
        } else {
            block_ops.push(Op::new("Tm", o.tlm.0.iter().map(|v| num(*v)).collect()));
            // The text already shown on this line moved the text matrix past the line start.
            let em = o.state.font.as_ref().map_or(0.0, |f| f.1) * o.state.scale;
            let dx = o.tlm.invert().map_or(0.0, |inv| inv.apply(o.tm.0[4], o.tm.0[5]).0);
            if dx.abs() > 1e-9 && em.abs() > 1e-9 {
                block_ops.push(Op::new("TJ", vec![Obj::Array(vec![num(-dx * 1000.0 / em)])]));
            }
        }
    }
    let at = if split { start } else { o.bt_op };
    let new_data = splice(&pc.pieces, &pc.ops, |i| {
        let mut insert = if i == at { std::mem::take(&mut block_ops) } else { Vec::new() };
        let keep = !drop.contains(&i);
        if !keep {
            insert.extend(stand_in(&pc, i));
        }
        (insert, keep)
    });
    write_contents(doc, &pc, new_data)?;
    Ok(fallback)
}

/// What replaces removed show operator `i`: the same moves without painting. A ' or " still
/// goes to the next line (" still sets its word and character spacing), and the text matrix
/// still advances by the text's width (a numeric-only TJ), so text after it in the same text
/// object stays where it was.
fn stand_in(pc: &PageContent, i: usize) -> Vec<Op> {
    let Some(op) = pc.ops.get(i) else { return Vec::new() };
    let mut v = Vec::new();
    match op.op.as_slice() {
        b"'" => v.push(Op::new("T*", vec![])),
        b"\"" => {
            v.push(Op::new("Tw", vec![op.operands.first().cloned().unwrap_or(Obj::Int(0))]));
            v.push(Op::new("Tc", vec![op.operands.get(1).cloned().unwrap_or(Obj::Int(0))]));
            v.push(Op::new("T*", vec![]));
        }
        _ => {}
    }
    if let Some(a) = pc.advances.get(&i) {
        if a.x.abs() > 1e-9 && a.em.abs() > 1e-9 {
            v.push(Op::new("TJ", vec![Obj::Array(vec![num(-a.x * 1000.0 / a.em)])]));
        }
    }
    v
}

/// Rewrites the page's content: `edit(i)` gives the operators to insert before operator `i`
/// and whether to keep it. Everything else is copied byte for byte.
fn splice(pieces: &Pieces, ops: &[Op], mut edit: impl FnMut(usize) -> (Vec<Op>, bool)) -> Vec<Vec<u8>> {
    let data = pieces.data();
    let mut edits = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let (insert, keep) = edit(i);
        if insert.is_empty() && keep {
            continue;
        }
        let mut bytes = Vec::new();
        if !insert.is_empty() {
            if op.span.start.checked_sub(1).and_then(|p| data.get(p)).is_some_and(|b| !b.is_ascii_whitespace()) {
                bytes.push(b'\n');
            }
            bytes.extend_from_slice(&serialize_ops(&insert));
        }
        let end = if keep { op.span.start } else { op.span.end };
        edits.push((op.span.start..end, bytes));
    }
    pieces.splice(edits)
}

/// Puts the rewritten pieces back: changed ones become new (Flate) streams, the others stay.
fn write_contents(doc: &mut PdfDocument, pc: &PageContent, new: Vec<Vec<u8>>) -> Result<()> {
    let mut list = Vec::with_capacity(new.len());
    for ((obj, old), data) in pc.streams.iter().zip(new) {
        if *old == data {
            list.push(obj.clone());
            continue;
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&data).map_err(|e| EngineError::Io(e.to_string()))?;
        let packed = z.finish().map_err(|e| EngineError::Io(e.to_string()))?;
        let mut dict = doc.new_dict()?;
        dict.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
        list.push(doc.add_stream(&Buffer::from_bytes(&packed)?, Some(&dict), true)?);
    }
    let mut page = pc.page.clone();
    if list.len() == 1 {
        page.dict_put("Contents", list.pop().expect("one"))?;
    } else {
        let mut arr = doc.new_array()?;
        for o in list {
            arr.array_push(o)?;
        }
        page.dict_put("Contents", arr)?;
    }
    Ok(())
}

/// A sentence for the UI.
pub fn issue_text(i: Issue) -> &'static str {
    match i {
        Issue::Type3 => "it is set in a Type 3 font, whose letters are drawings",
        Issue::NoUnicode => "its font doesn't say which characters it shows (no ToUnicode map)",
        Issue::Vertical => "it is vertical text",
        Issue::Encoding => "its font uses a predefined CJK encoding that can't be rewritten",
        Issue::Clip => "it is used as a clipping path",
        Issue::Invisible => "it is invisible text (an OCR layer)",
        Issue::Hidden => "it is on a hidden layer",
        Issue::InForm => "it is drawn by a shared form object (a stamp or template)",
        Issue::Outlines => "the text was converted to shapes (outlines)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_keeps_hard_breaks() {
        let w = |s: &str| s.chars().count() as f64;
        let lines = |v: Vec<(String, bool)>| v.into_iter().map(|l| l.0).collect::<Vec<_>>();
        assert_eq!(lines(wrap("aa bb cc", 5.0, w)), ["aa bb", "cc"]);
        assert_eq!(wrap("aa\nbb cc", 10.0, w), [("aa".to_string(), true), ("bb cc".to_string(), true)]);
    }
}
