// Adapted from pdfcraft (MIT OR Apache-2.0), crates/edit/src/text.rs @ b8ddf71
//! Edit text in place: the text blocks already on a page, and rewriting a block with new text,
//! runs of other fonts, sizes and colours, alignment, position or width.
//!
//! A *line* is a run of text-showing operators inside one `BT … ET` on one baseline, made of
//! *runs* of one font, size and colour; a *block* is consecutive lines of similar size, aligned
//! and regularly spaced (a paragraph). Rewriting a block removes its show operators and inserts,
//! where it started, one new text object in its own graphics state (`q … Q`) with the text
//! reflowed to the block's width (see layout.rs). Every other operator is copied byte for byte;
//! content streams whose bytes changed become new stream objects (a stream may be shared with
//! another page).
//!
//! pdfcraft does this on its own COS document; here the page, its resources and its streams are
//! MuPDF objects, and fallback fonts are the standard 14 and the bundled Noto faces (bundled.rs).

use super::content::{Matrix, Obj, Op, Pieces, num, serialize_ops};
use super::font::{Limit, Metrics};
use super::layout::{Deco, Face, Measure, Placement, RunSpec, Setter, emit, family_of, forget_unused_fonts, restyled_face, wrap};
use crate::engine::EngineError;
use mupdf::Buffer;
use mupdf::pdf::{PdfDocument, PdfObject};
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

/// Superscript or subscript, written with `Ts` (text rise) and a smaller size.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Rise {
    Super,
    Sub,
}

/// The run's size over its base size, and the rise over the base size, for super and subscript
/// (the usual word-processor values; docs/text-edit.md).
const RISE_SCALE: f64 = 0.65;
const RISE_UP: f64 = 0.33;
const RISE_DOWN: f64 = 0.15;
/// Decoration (underline, strikeout) thickness and the centre line's offset from the baseline, over
/// the run's size.
pub(crate) const DECO_THICK: f64 = 0.05;
pub(crate) const UNDERLINE_AT: f64 = -0.12;
pub(crate) const STRIKE_AT: f64 = 0.28;

/// `null` in JSON is a value (clear), a missing key is `None` (keep).
pub(crate) fn clearable<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Option<Option<Rise>>, D::Error> {
    Option::<Rise>::deserialize(d).map(Some)
}

/// Whether a `Ts` of `rise` on text of `size` is a super or subscript, as opposed to a position
/// (some producers move text with `Ts`): up to 0.6 of the size.
fn is_script(rise: f64, size: f64) -> bool {
    rise.abs() <= size.abs() * 0.6
}

/// The rise a `Ts` value means: its sign (none for a value that is not a finite, sane number).
fn rise_of(ts: f64) -> Option<Rise> {
    if !ts.is_finite() || ts.abs() > 1e5 {
        None
    } else if ts > 1e-6 {
        Some(Rise::Super)
    } else if ts < -1e-6 {
        Some(Rise::Sub)
    } else {
        None
    }
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

/// One glyph as shown: its code, what it means, its advance (thousandths of the text-space
/// unit at size 1, word spacing included for a space), and the TJ adjustment before it.
#[derive(Clone, Debug)]
pub(crate) struct Glyph {
    /// Empty for a space read from a gap between operators (no code shows it).
    pub bytes: Vec<u8>,
    pub text: String,
    pub w: f64,
    pub kern: f64,
    /// A single-byte code 32 (word spacing applies), or a gap read as a space.
    pub space: bool,
}

impl Glyph {
    pub(crate) fn gap() -> Self {
        Glyph { bytes: Vec::new(), text: " ".into(), w: 250.0, kern: 0.0, space: true }
    }
}

/// Consecutive glyphs of one font, size and colour on a line.
#[derive(Clone, Debug)]
pub struct Run {
    /// Font resource name.
    pub(crate) font: Vec<u8>,
    pub base_font: String,
    /// Size in user space, and the `Tf` size (text space).
    pub size: f64,
    pub(crate) tsize: f64,
    pub color: [f64; 3],
    pub(crate) fill: Vec<Op>,
    pub bold: bool,
    pub italic: bool,
    pub text: String,
    pub(crate) glyphs: Vec<Glyph>,
    /// `Ts` (text space), kept exactly so an edit that leaves it alone does not move the run.
    pub(crate) rise: f64,
    /// Our own tagged decoration lies under or through it.
    pub underline: bool,
    pub strike: bool,
}

impl Run {
    pub(crate) fn same_style(&self, o: &Run) -> bool {
        self.font == o.font
            && (self.size - o.size).abs() < 0.01
            && self.color == o.color
            && (self.rise - o.rise).abs() < 1e-6
            && self.underline == o.underline
            && self.strike == o.strike
    }

    /// Super or subscript; a rise past most of the size is a position and reads as none (it is kept
    /// as written, not scaled with the size).
    pub fn rise_kind(&self) -> Option<Rise> {
        rise_of(self.rise).filter(|_| is_script(self.rise, self.tsize))
    }
}

/// The run block-level attributes (font, size, colour) are read from and that typed text takes
/// the style of: the first one that is not raised (a footnote mark starts many paragraphs).
pub(crate) fn body_run(runs: &[Run]) -> usize {
    runs.iter().position(|r| r.rise_kind().is_none()).unwrap_or(0)
}

/// One line of existing text.
#[derive(Clone, Debug)]
pub struct Line {
    pub text: String,
    /// Box in user space.
    pub rect: [f64; 4],
    pub base_font: String,
    /// Size in user space (the largest of its runs).
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
    pub runs: Vec<Run>,
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
    /// The block's text as runs of one style, in reading order: a line break is a space (or
    /// nothing after a hyphen, which goes), consecutive parts of one style are one run.
    pub runs: Vec<Run>,
    /// Where in the runs' joined text a line-end hyphen was dropped (the character offset the next line
    /// starts at): a rewrite of the block does not put it back.
    pub joins: Vec<usize>,
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
    glyphs: Vec<Glyph>,
    underline: bool,
    strike: bool,
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
    /// Our own decorations (`/Span <</IWDeco …>> BDC … EMC`): the operators they span and the show
    /// operators they lie under or through.
    decos: Vec<(usize, usize, Vec<usize>)>,
}

impl PageContent {
    /// A font resource as the font picker names it: `doc:<object>` for an indirect font,
    /// `res:<name>` for one written into the resources directly.
    pub(crate) fn font_id(&self, res: &[u8]) -> String {
        let name = String::from_utf8_lossy(res).into_owned();
        match self.fonts.as_ref().and_then(|f| get(f, &name)) {
            Some(v) if v.is_indirect().unwrap_or(false) => v.as_indirect().map_or(format!("res:{name}"), |n| format!("doc:{n}")),
            _ => format!("res:{name}"),
        }
    }
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

pub(crate) fn page_streams(page: &PdfObject) -> Result<Vec<(PdfObject, Vec<u8>)>> {
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
    let mut shown = interpret(&ops, fonts.as_ref(), xobjects.as_ref(), &layers, &mut cache, &mut extra);
    let advances = shown.iter().map(|s| (s.op, Advance { x: s.advance, em: s.em })).collect();
    // A run is underlined or struck out where one of our own decorations lies under or through it.
    let mut decos = Vec::new();
    let invs: Vec<Option<Matrix>> = shown.iter().map(|s| s.tm.then(&s.state.ctm).invert()).collect();
    for d in &extra.decos {
        let mut under = Vec::new();
        for (s, inv) in shown.iter_mut().zip(&invs) {
            if inv.as_ref().is_some_and(|inv| d.lies_on(s, inv)) {
                under.push(s.op);
                if d.strike {
                    s.strike = true;
                } else {
                    s.underline = true;
                }
            }
        }
        decos.push((d.first, d.last, under));
    }
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
    Ok(PageContent { page: p, streams, pieces, ops, fonts, advances, lines, areas, crop, decos })
}

/// What the interpreter notices besides text.
#[derive(Default)]
struct Extra {
    /// Boxes of form XObjects that show text.
    forms: Vec<[f64; 4]>,
    /// Filled paths the size of glyphs, with curves, and their union.
    glyphlike: usize,
    glyph_box: [f64; 4],
    /// Our own decorations, in stream order.
    decos: Vec<DecoMark>,
}

/// How many decorations of ours a page is read for.
const MAX_DECOS: usize = 10_000;

/// One decoration of ours as read: its operators, and the middle of its left and right edge in
/// user space.
struct DecoMark {
    first: usize,
    last: usize,
    strike: bool,
    a: (f64, f64),
    b: (f64, f64),
}

impl DecoMark {
    /// Whether it lies where `s`'s decoration would be drawn: across its middle, at the offset
    /// the engine draws it at (from the baseline and the rise), in the shown text's own space.
    fn lies_on(&self, s: &Shown, inv: &Matrix) -> bool {
        let (xa, ya) = inv.apply(self.a.0, self.a.1);
        let (xb, yb) = inv.apply(self.b.0, self.b.1);
        let size = s.state.size.abs();
        let want = s.state.rise + size * if self.strike { STRIKE_AT } else { UNDERLINE_AT };
        let (lo, hi) = (xa.min(xb) - size * 0.05, xa.max(xb) + size * 0.05);
        size > 0.0 && (ya - want).abs() <= size * 0.04 && (yb - ya).abs() <= size * 0.04 && (lo..=hi).contains(&(s.advance / 2.0))
    }
}

/// The `EMC` that closes the decoration `ops[at]` opens, when the group is exactly what [`deco_ops`]
/// writes, in that order: `q`, `cm`, the fill (`g`, `rg`, `k`, or `cs` with `sc`/`scn`), `re`, `f`,
/// `Q`. A tagged group of anything else (a lone `Q`, a bare `cm`, text) is not ours to read, skip
/// or remove. The shape fixes where the `EMC` is, so nothing is searched for.
fn deco_group(ops: &[Op], at: usize) -> Option<usize> {
    deco_kind(ops.get(at)?)?;
    if !(ops.get(at + 1)?.is("q") && ops.get(at + 2)?.is("cm")) {
        return None;
    }
    let fills = ops[at + 3..].iter().take(8).take_while(|o| matches!(o.op.as_slice(), b"g" | b"rg" | b"k" | b"cs" | b"sc" | b"scn")).count();
    if fills == 0 {
        return None;
    }
    let tail = ops.get(at + 3 + fills..at + 7 + fills)?;
    (tail[0].is("re") && tail[1].is("f") && tail[2].is("Q") && tail[3].is("EMC")).then_some(at + 6 + fills)
}

/// Whether a `BDC` opens one of our decorations (`/Span <</IWDeco /U|/S>>`): `Some(true)` for a strikeout.
fn deco_kind(op: &Op) -> Option<bool> {
    if !op.is("BDC") || op.name(0) != Some(b"Span".as_slice()) {
        return None;
    }
    match op.operands.get(1) {
        Some(Obj::Dict(d)) => d.iter().find(|(k, _)| k == b"IWDeco").map(|(_, v)| v.as_name() == Some(b"S".as_slice())),
        _ => None,
    }
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
    // The decoration being read: where its `BDC` is (marked-content depth, operator), its kind and
    // its rectangle's edge midpoints.
    let mut deco: Option<(usize, usize, bool, Option<((f64, f64), (f64, f64))>)> = None;
    // The current path: its box in user space and whether it has curves.
    let mut path = (EMPTY, false);
    extra.glyph_box = EMPTY;
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"BMC" => marked.push(false),
            b"BDC" => {
                marked.push(op.name(0) == Some(b"OC") && layers.hidden(op.operands.get(1)));
                if let Some(strike) = deco_kind(op).filter(|_| deco_group(ops, i).is_some()) {
                    deco = Some((marked.len(), i, strike, None));
                }
            }
            b"EMC" => {
                if let Some((_, first, strike, Some((a, b)))) = deco.filter(|d| d.0 == marked.len()) {
                    // A page with more is read as far as this and no further (the rest are left as they are).
                    if extra.decos.len() < MAX_DECOS {
                        extra.decos.push(DecoMark { first, last: i, strike, a, b });
                    }
                }
                if deco.is_some_and(|d| d.0 == marked.len()) {
                    deco = None;
                }
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
                    if let Some(d) = deco.as_mut() {
                        d.3 = Some((ts.ctm.apply(x, y + h / 2.0), ts.ctm.apply(x + w, y + h / 2.0)));
                    }
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
                // The baseline and the box ignore a script's rise: a raised run stays on its line. A
                // rise past 0.6 of the size is a position (some producers move text with `Ts`): it
                // stays in the start and the box, and reads as no rise.
                let lift = if rise_of(ts.rise).is_some() && !is_script(ts.rise, ts.size) { ts.rise } else { 0.0 };
                let start = trm0.apply(0.0, lift);
                let mut text = String::new();
                let mut decodable = m.has_to_unicode;
                let mut x_text = 0.0;
                let mut glyphs = Vec::new();
                let mut kern = 0.0;
                for p in &pieces {
                    match p {
                        Obj::Str(s) => {
                            let mut at = 0;
                            for (code, len) in m.codes(s) {
                                let t = m.text_of(code);
                                match t {
                                    Some(t) => text.push_str(t),
                                    None => decodable = false,
                                }
                                let space = m.is_space(code, len);
                                let w = m.width(code) * ts.size + ts.char_spacing + if space { ts.word_spacing } else { 0.0 };
                                x_text += w * ts.scale;
                                // The glyph's own advance: a rewrite sets Tw 0 and spreads
                                // justified lines itself.
                                glyphs.push(Glyph {
                                    bytes: s[at..(at + len).min(s.len())].to_vec(),
                                    text: t.unwrap_or_default().to_string(),
                                    w: m.width(code) * 1000.0,
                                    kern: std::mem::take(&mut kern),
                                    space: space || t == Some(" "),
                                });
                                at += len;
                            }
                        }
                        other => {
                            if let Some(n) = other.as_f64() {
                                let dx = -n / 1000.0 * ts.size * ts.scale;
                                // A large gap inside TJ reads as a space.
                                if dx > ts.size * 0.2 && !text.ends_with(' ') {
                                    text.push(' ');
                                    glyphs.push(Glyph { kern: std::mem::take(&mut kern), ..Glyph::gap() });
                                    // The gap shows as the space; what is left of it is kerning.
                                    kern = n + 250.0;
                                } else {
                                    kern += n;
                                }
                                x_text += dx;
                            }
                        }
                    }
                }
                let begin = tm;
                tm = Matrix::translate(x_text, 0.0).then(&tm);
                let end = tm.then(&ts.ctm).apply(0.0, lift);
                if !pieces.iter().any(|p| matches!(p, Obj::Str(s) if !s.is_empty())) {
                    continue;
                }
                let corners = [
                    trm0.apply(0.0, lift + m.descent * ts.size),
                    trm0.apply(x_text, lift + m.descent * ts.size),
                    trm0.apply(0.0, lift + m.ascent * ts.size),
                    trm0.apply(x_text, lift + m.ascent * ts.size),
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
                    glyphs,
                    underline: false,
                    strike: false,
                });
            }
            _ => {}
        }
    }
    out
}

/// Adds `s` to the line it continues, or starts a new one. `last` is the previous run's
/// (`bt`, end point, size). A line holds runs of any font, size and colour; a new run starts where
/// the style changes.
fn add_shown(lines: &mut Vec<Line>, last: &mut Option<(usize, (f64, f64), f64)>, s: Shown, cache: &HashMap<Vec<u8>, Rc<Metrics>>) {
    let color = fill_color(&s.state.fill);
    let m = cache.get(&s.font).cloned().unwrap_or_else(|| Rc::new(Metrics::fallback()));
    let run = Run {
        font: s.font.clone(),
        base_font: m.base_font.clone(),
        size: s.size,
        tsize: s.state.size,
        color,
        fill: s.state.fill.clone(),
        bold: m.bold,
        italic: m.italic,
        text: s.text.clone(),
        glyphs: s.glyphs.clone(),
        rise: s.state.rise,
        underline: s.underline,
        strike: s.strike,
    };
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
            && l.render_mode == s.state.render_mode
            && l.hidden == s.hidden
            && along(l, end).is_some_and(|(dx, dy)| dy.abs() < size * 0.3 && dx > -size && dx < size * 3.0)
    });
    if joins {
        let (_, end, size) = last.expect("checked");
        let l = lines.last_mut().expect("checked");
        let gap = along(l, end).map_or(0.0, |d| d.0);
        let mut run = run;
        if gap > size * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ') {
            l.text.push(' ');
            run.text.insert(0, ' ');
            run.glyphs.insert(0, Glyph::gap());
        }
        l.text.push_str(&s.text);
        l.rect = union(l.rect, s.rect);
        l.ops.push(s.op);
        l.decodable &= s.decodable;
        l.end = s.end;
        l.size = l.size.max(s.size);
        l.limit = l.limit.or(m.limit);
        match l.runs.last_mut() {
            Some(r) if r.same_style(&run) => {
                r.text.push_str(&run.text);
                r.glyphs.extend(run.glyphs);
            }
            _ => l.runs.push(run),
        }
    } else {
        let trm = s.tm.then(&s.state.ctm);
        lines.push(Line {
            text: s.text.clone(),
            rect: s.rect,
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
            runs: vec![run],
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
            // One paragraph may mix fonts and colours, but not very different sizes (a heading
            // above its body text is a block of its own).
            let ratio = prev.size.max(l.size) / prev.size.min(l.size).max(0.01);
            prev.render_mode == l.render_mode
                && prev.hidden == l.hidden
                && ratio < 1.3
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
                runs: Vec::new(),
                joins: Vec::new(),
            });
        }
    }
    for b in &mut blocks {
        (b.runs, b.joins) = block_runs(lines, &b.lines);
        if let Some(r) = b.runs.get(body_run(&b.runs)) {
            (b.base_font, b.size, b.bold, b.italic, b.color) = (r.base_font.clone(), r.size, r.bold, r.italic, r.color);
        }
        b.align = detect_align(lines, &b.lines);
        if b.issue.is_none() {
            b.issue = b.lines.iter().find_map(|&i| lines[i].issue());
        }
    }
    blocks
}

/// The runs of a block's lines in reading order, joined across line breaks (see [`Block::runs`]), and where
/// line-end hyphens were dropped (see [`Block::joins`]).
fn block_runs(lines: &[Line], members: &[usize]) -> (Vec<Run>, Vec<usize>) {
    let mut out: Vec<Run> = Vec::new();
    let mut joins = Vec::new();
    for (n, &li) in members.iter().enumerate() {
        let line = &lines[li];
        for (k, run) in line.runs.iter().enumerate() {
            let mut run = run.clone();
            if k == 0 {
                // Leading spaces of a line go; a line after the first starts after a break.
                while run.glyphs.first().is_some_and(|g| g.text.chars().all(paragraph_separator) && !g.text.is_empty()) {
                    run.glyphs.remove(0);
                }
                run.text = run.text.trim_start_matches(paragraph_separator).to_string();
                if n > 0 {
                    if let Some(prev) = out.last_mut() {
                        let hyphen = prev.text.trim_end_matches(paragraph_separator).ends_with('-');
                        while prev.glyphs.last().is_some_and(|g| g.text.chars().all(paragraph_separator) && !g.text.is_empty()) {
                            prev.glyphs.pop();
                        }
                        prev.text = prev.text.trim_end_matches(paragraph_separator).to_string();
                        if hyphen {
                            prev.text.pop();
                            if prev.glyphs.last().is_some_and(|g| g.text == "-") {
                                prev.glyphs.pop();
                            }
                            joins.push(out.iter().map(|r| r.text.chars().count()).sum());
                        } else if prev.same_style(&run) {
                            prev.text.push(' ');
                            prev.glyphs.push(Glyph::gap());
                        } else {
                            run.text.insert(0, ' ');
                            run.glyphs.insert(0, Glyph::gap());
                        }
                    }
                }
            }
            match out.last_mut() {
                Some(prev) if prev.same_style(&run) => {
                    prev.text.push_str(&run.text);
                    prev.glyphs.extend(run.glyphs);
                }
                _ => out.push(run),
            }
        }
    }
    if let Some(last) = out.last_mut() {
        while last.glyphs.last().is_some_and(|g| g.text.chars().all(paragraph_separator) && !g.text.is_empty()) {
            last.glyphs.pop();
        }
        last.text = last.text.trim_end_matches(paragraph_separator).to_string();
    }
    out.retain(|r| !r.text.is_empty());
    (out, joins)
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
    /// Distance between lines as a multiple of the font size.
    pub line_spacing: Option<f64>,
    /// Bold and italic for every run: a font of the same family and style on the page, else the
    /// standard face of that style (reported as a fallback).
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    /// For every run without its own: underline, strikeout, superscript or subscript (`Some(None)`
    /// clears the rise).
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    #[serde(default, deserialize_with = "clearable", skip_serializing_if = "Option::is_none")]
    pub rise: Option<Option<Rise>>,
    /// The block's new content as runs of their own style, in order (replaces `text`).
    pub runs: Option<Vec<RunChange>>,
}

/// One run of a block's new content. Unset fields come from run `source` of the block (or its
/// first run). A run with a `source`, no `text` (or its own text again) and no font or style
/// change keeps its glyphs exactly, kerning included.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunChange {
    pub text: Option<String>,
    /// Index into the block's runs (`TextBlockDto::runs`).
    pub source: Option<usize>,
    /// A font picker id (`doc:<object>`, `res:<name>`, `std:<name>`, `bundled:<id>`).
    pub font: Option<String>,
    /// Points.
    pub size: Option<f64>,
    pub color: Option<[f64; 3]>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    #[serde(default, deserialize_with = "clearable", skip_serializing_if = "Option::is_none")]
    pub rise: Option<Option<Rise>>,
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

/// Text with single spaces between words; hard breaks (`\n`) kept.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .split('\n')
        .map(|p| p.split(paragraph_separator).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text with each stretch of spaces as one space, hard breaks (`\n`) kept, the ends left as they are.
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.replace("\r\n", "\n").chars() {
        if c != '\n' && paragraph_separator(c) {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn fill_ops(color: Option<[f64; 3]>, own: &[Op]) -> Result<Vec<Op>> {
    match color {
        Some([r, g, b]) => {
            if ![r, g, b].iter().all(|v| (0.0..=1.0).contains(v)) {
                return Err(invalid("colour components must be between 0 and 1"));
            }
            Ok(vec![Op::new("rg", vec![num(r), num(g), num(b)])])
        }
        // A run that never set a colour is black (the runs before it in the new text object
        // may have set another).
        None if own.is_empty() => Ok(vec![Op::new("g", vec![num(0.0)])]),
        None => Ok(own.to_vec()),
    }
}

/// Rewrites block `index` of page `page` with `change`. Returns the fallback fonts used.
pub fn rewrite_block(doc: &mut PdfDocument, page: u32, index: usize, change: &TextChange) -> Result<Vec<Fallback>> {
    rewrite_block_expecting(doc, page, index, change, None)
}

/// [`rewrite_block`] that refuses when the block at `index` no longer reads `expect` (a batch of
/// edits on one page, planned from one reading: Find & Replace).
pub fn rewrite_block_expecting(doc: &mut PdfDocument, page: u32, index: usize, change: &TextChange, expect: Option<&str>) -> Result<Vec<Fallback>> {
    let pc = read_page(doc, page)?;
    let blocks = group_blocks(&pc.lines);
    let b = blocks.get(index).cloned().ok_or_else(|| invalid(format!("page {} has no text block {}", page + 1, index + 1)))?;
    if expect.is_some_and(|t| t != b.text) {
        return Err(invalid("the text changed while it was being replaced"));
    }
    if let Some(issue) = b.issue {
        return Err(invalid(format!("this text can't be edited: {}", issue_text(issue))));
    }
    let members: Vec<&Line> = b.lines.iter().map(|&i| &pc.lines[i]).collect();
    let first = members[0];
    let o = &first.origin;
    o.state.font.as_ref().ok_or_else(|| invalid("the text has no font"))?;
    let k = o.k.max(1e-6);
    // The run the block's own size, font and colour are, and typed text takes the style of.
    let body = body_run(&b.runs);
    let first_run = b.runs.get(body).ok_or_else(|| invalid("the block has no text"))?;
    let factor = match change.size {
        Some(pt) if pt.is_finite() && pt > 0.0 => pt / first_run.size.max(1e-6),
        Some(_) => return Err(invalid("the font size must be a positive number")),
        None => 1.0,
    };
    let specs: Vec<RunChange> = match (&change.runs, &change.text) {
        (Some(r), _) => r.clone(),
        (None, Some(t)) => vec![RunChange { text: Some(t.clone()), source: Some(body), ..Default::default() }],
        (None, None) => (0..b.runs.len()).map(|i| RunChange { source: Some(i), ..Default::default() }).collect(),
    };
    if specs.is_empty() {
        return Err(invalid("there is no text to set"));
    }
    let st = &o.state;
    let mut setter = Setter::new(doc, pc.page.clone());
    let mut items = Vec::new();
    // The size lines are spaced by: the first run that is not raised, and what it was.
    let mut reference: Option<(f64, f64)> = None;
    let mut first_size = None;
    for rc in &specs {
        let src = match rc.source {
            Some(i) => Some(b.runs.get(i).ok_or_else(|| invalid(format!("the block has no run {}", i + 1)))?),
            None => None,
        };
        let like = src.unwrap_or(first_run);
        let bold = rc.bold.or(change.bold).unwrap_or(like.bold);
        let italic = rc.italic.or(change.italic).unwrap_or(like.italic);
        let (primary, own) = match &rc.font {
            Some(id) => {
                let f = setter.face_by_id(id)?;
                (f, matches!(&setter.faces[f], Face::Doc { res, .. } if *res == like.font))
            }
            None => restyled_face(&mut setter, like, bold, italic)?,
        };
        // The run's text-space size per point (its own text matrix and CTM).
        let per_pt = if like.size > 1e-9 { like.tsize / like.size } else { 1.0 / k };
        let was = like.rise_kind();
        let now = rc.rise.or(change.rise).unwrap_or(was);
        let prior = like.tsize * factor;
        // Super and subscript are set at 0.65 of the base size; our own are put back to it.
        let ours = was.is_some()
            && like.tsize > 0.0
            && ((like.rise / like.tsize).abs() - if was == Some(Rise::Super) { RISE_UP } else { RISE_DOWN } / RISE_SCALE).abs() < 0.05;
        let size = match rc.size {
            Some(pt) if pt.is_finite() && pt > 0.0 => pt * per_pt,
            Some(_) => return Err(invalid("the font size must be a positive number")),
            None if was.is_none() && now.is_some() => prior * RISE_SCALE,
            None if was.is_some() && now.is_none() && ours => prior / RISE_SCALE,
            None => prior,
        };
        let rise = match now {
            // A position written with `Ts` stays as it was unless the rise was cleared on purpose.
            None if was.is_none() && rc.rise.or(change.rise).is_none() && like.rise.is_finite() && like.rise.abs() <= 1e5 => like.rise,
            None => 0.0,
            // Unchanged: its own `Ts`, scaled with the size.
            Some(_) if now == was && like.tsize > 0.0 => like.rise * size / like.tsize,
            Some(Rise::Super) => RISE_UP * size / RISE_SCALE,
            Some(Rise::Sub) => -RISE_DOWN * size / RISE_SCALE,
        };
        // Never a number the file can't hold ("inf" is not a PDF number).
        let rise = if rise.is_finite() && rise.abs() <= 1e5 { rise } else { 0.0 };
        first_size.get_or_insert(size);
        // A whole-block text loses outer spaces; a run keeps its own (they join it to the next).
        let norm = if change.runs.is_some() { collapse } else { normalize };
        let text = rc.text.as_deref().map(norm).filter(|t| src.is_none_or(|s| *t != s.text));
        if text.is_none() && src.is_none() {
            continue;
        }
        let spec = RunSpec {
            source: src,
            text,
            primary,
            own_font: own && src.is_some(),
            size,
            fill: fill_ops(rc.color.or(change.color), &like.fill)?,
            bold,
            italic,
            family: family_of(&like.base_font),
            rise,
            underline: rc.underline.or(change.underline).unwrap_or(like.underline),
            strike: rc.strike.or(change.strike).unwrap_or(like.strike),
        };
        if now.is_none() {
            reference.get_or_insert((size, like.tsize));
        }
        items.extend(setter.items(&spec)?);
    }
    let (size, old_tf) = match (reference, first_size) {
        (Some(r), _) => r,
        (None, Some(s)) => (s, first_run.tsize),
        (None, None) => (first_run.tsize, first_run.tsize),
    };
    let m = Measure { styles: &setter.styles, cs: st.char_spacing, scale: st.scale };
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
    let grows = change.width.is_none() && members.len() == 1 && align == Align::Left && upright;
    // Widths in user space (as on the page); the layout works in text space (÷ k).
    let mut width = match change.width {
        Some(w) if w.is_finite() && w > 0.0 => w.clamp(size * k, 14_400.0),
        Some(_) => return Err(invalid("the width must be a positive number")),
        // A single left-aligned line grows to the right, up to the page's margin.
        None if grows => own.max(pc.crop[2] - 36.0 - b.rect[0]),
        None => own,
    };
    let mut lines = wrap(items.clone(), (width + 0.5) / k, &m);
    if grows {
        width = lines.iter().map(|l| m.line(&l.items) * k).fold(own, f64::max).min(width);
    }
    // A word longer than the width widens the box, keeping the aligned edge.
    let widest = lines.iter().map(|l| m.line(&l.items) * k).fold(0.0, f64::max);
    if widest > width {
        let extra = widest - width;
        match align {
            Align::Center => left -= extra / 2.0 / k,
            Align::Right => left -= extra / k,
            _ => {}
        }
        width = widest;
        lines = wrap(items, (width + 0.5) / k, &m);
    }
    let lead = if let Some(ls) = change.line_spacing {
        if !(ls.is_finite() && (0.5..=5.0).contains(&ls)) {
            return Err(invalid("the line spacing must be between 0.5 and 5 times the font size"));
        }
        ls * size
    } else if members.len() > 1 {
        (spans[0].2 - spans[1].2) * size / old_tf
    } else if st.leading > 0.0 {
        st.leading * size / old_tf
    } else {
        size * 1.2
    };
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
    // An earlier edit's own text object whose text all goes, goes whole: its Tf, colour and
    // moves too, so the fonts it named are no longer set anywhere.
    let (mut whole, wrappers) = edited_objects(&pc.ops, &drop);
    // A decoration goes with the text it lies on (drawn again below where the text is now).
    for (first, last, under) in &pc.decos {
        if under.iter().any(|s| drop.contains(s)) {
            whole.extend(*first..=*last);
        }
    }
    // The block was an earlier edit's, inside its own `q cm … Q`: it is set there again, and a
    // move changes that `cm` instead of adding another (nesting stays the same however often
    // it moves).
    let wrapper = if split { None } else { wrappers.iter().find(|w| w.bt == o.bt_op) };
    let mut block_ops = if wrapper.is_some() { Vec::new() } else { vec![Op::new("q", vec![])] };
    let mut new_cm = None;
    if let Some([dx, dy]) = change.offset {
        if !(dx.is_finite() && dy.is_finite()) || dx.abs() > 1e5 || dy.abs() > 1e5 {
            return Err(invalid("the text can't be moved that far"));
        }
        let c = o.ctm.0;
        let back = Matrix([c[0], c[1], c[2], c[3], 0.0, 0.0])
            .invert()
            .ok_or_else(|| invalid("the text is drawn in a space it can't be moved in"))?;
        let (ax, ay) = back.apply(dx, dy);
        let shift = Matrix([1.0, 0.0, 0.0, 1.0, ax, ay]);
        match wrapper {
            Some(w) => {
                let old = pc.ops[w.cm].nums::<6>().map_or(Matrix::IDENTITY, Matrix);
                new_cm = Some((w.cm, Op::new("cm", shift.then(&old).0.iter().map(|v| num(*v)).collect())));
            }
            None => block_ops.push(Op::new("cm", shift.0.iter().map(|v| num(*v)).collect())),
        }
    }
    // Acrobat's marker for edited text.
    block_ops.push(Op::new("MP", vec![Obj::name("TouchUp_TextEdit")]));
    block_ops.push(Op::new("BT", vec![]));
    let mut state = st.clone();
    state.word_spacing = 0.0;
    // Each run sets its own rise.
    state.rise = 0.0;
    state.font = None;
    state.fill = Vec::new();
    block_ops.extend(state.ops());
    let origin = Matrix::translate(left, 0.0).then(&o.tm);
    block_ops.push(Op::new("Tm", origin.0.iter().map(|v| num(*v)).collect()));
    let tm_scale = (origin.0[0] * origin.0[3] - origin.0[1] * origin.0[2]).abs().sqrt();
    let placement = Placement { align, width: width / k, lead, first_size: size, stroke: 0.03 * tm_scale, render_mode: st.render_mode };
    let (shown, decos) = emit(&setter, &lines, &m, &placement);
    block_ops.extend(shown);
    setter.finish()?;
    let fallbacks = std::mem::take(&mut setter.fallbacks);
    std::mem::drop(setter);
    block_ops.push(Op::new("ET", vec![]));
    block_ops.extend(deco_ops(&origin, &decos));
    if wrapper.is_none() {
        block_ops.push(Op::new("Q", vec![]));
    }
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
        if let Some((_, cm)) = new_cm.as_ref().filter(|(j, _)| *j == i) {
            return (vec![cm.clone()], false);
        }
        let keep = !drop.contains(&i) && !whole.contains(&i);
        if !keep && !whole.contains(&i) {
            insert.extend(stand_in(&pc, i));
        }
        (insert, keep)
    });
    write_contents(doc, &pc, new_data)?;
    // The edit is written: a stale font entry left behind is harmless (it goes with the next full save), so
    // it is not an error the caller could mistake for a refused edit.
    if let Err(e) = forget_unused_fonts(doc, page) {
        eprintln!("textedit: could not tidy the fonts of page {}: {e}", page + 1);
    }
    Ok(fallbacks)
}

/// The operators of the edited text objects (`q [cm] /TouchUp_TextEdit MP BT … ET Q`, as
/// [`rewrite_block`] and [`add_text`] write them) whose every show operator is in `drop`: the
/// marker and the text object. A wrapper with a `cm` stays (`q cm Q`): the new block is set
/// inside it, in the space the old one was drawn in (a move, an added box's placement); a bare
/// `q … Q` goes too. Also returns the kept wrappers.
fn edited_objects(ops: &[Op], drop: &HashSet<usize>) -> (HashSet<usize>, Vec<Wrapper>) {
    let mut out = HashSet::new();
    let mut kept = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        if !(op.is("MP") && op.name(0) == Some(b"TouchUp_TextEdit".as_slice()) && ops.get(i + 1).is_some_and(|o| o.is("BT"))) {
            continue;
        }
        let q = match i.checked_sub(1).map(|j| &ops[j]) {
            Some(o) if o.is("q") => i - 1,
            Some(o) if o.is("cm") && i >= 2 && ops[i - 2].is("q") => i - 2,
            _ => continue,
        };
        let Some(et) = ops[i + 2..].iter().position(|o| o.is("ET") || o.is("BT")).map(|p| i + 2 + p) else { continue };
        // The object's own decorations (`BDC … EMC`) stand between its `ET` and its `Q`.
        let mut close = et + 1;
        while let Some(end) = deco_group(ops, close) {
            close = end + 1;
        }
        if !ops[et].is("ET") || !ops.get(close).is_some_and(|o| o.is("Q")) {
            continue;
        }
        let shows = (i + 2..et).filter(|&j| matches!(ops[j].op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\""));
        if shows.clone().count() > 0 && shows.into_iter().all(|j| drop.contains(&j)) {
            if q == i - 1 {
                out.extend(q..=close);
            } else {
                out.extend(i..close);
                kept.push(Wrapper { cm: i - 1, bt: i + 1 });
            }
        }
    }
    (out, kept)
}

/// The operators that draw `decos` (from [`emit`]) in the space of the text matrix `origin`: a
/// filled rectangle each, in the run's colour, tagged `/Span <</IWDeco /U|/S>>` so they are read
/// back and replaced with the text they belong to.
fn deco_ops(origin: &Matrix, decos: &[Deco]) -> Vec<Op> {
    let mut ops = Vec::new();
    for d in decos {
        let tag = Obj::Dict(vec![(b"IWDeco".to_vec(), Obj::name(if d.strike { "S" } else { "U" }))]);
        ops.push(Op::new("BDC", vec![Obj::name("Span"), tag]));
        ops.push(Op::new("q", vec![]));
        ops.push(Op::new("cm", origin.0.iter().map(|v| num(*v)).collect()));
        ops.extend(d.fill.iter().cloned());
        ops.push(Op::new("re", vec![num(d.x), num(d.y), num(d.w), num(d.h)]));
        ops.push(Op::new("f", vec![]));
        ops.push(Op::new("Q", vec![]));
        ops.push(Op::new("EMC", vec![]));
    }
    ops
}

/// A kept `q cm … Q` around an earlier edit's text object: its `cm` and `BT` operators.
struct Wrapper {
    cm: usize,
    bt: usize,
}

/// New text for [`add_text`].
pub struct NewText<'a> {
    pub text: &'a str,
    /// Font size in points.
    pub size: f64,
    /// RGB 0..1.
    pub color: [f64; 3],
    /// Top left of the box, in page space (the page as shown, y down).
    pub at: [f64; 2],
    /// Wrap to this width (page space); none keeps the lines as typed.
    pub width: Option<f64>,
    /// Page space to user space.
    pub back: Matrix,
}

/// Puts `new` on the page as a text object of its own, in Helvetica (embedded when the text
/// needs more than WinAnsi). The page's own content is wrapped in `q`/`Q` so the new text starts
/// from a clean graphics state, and is otherwise untouched.
pub fn add_text(doc: &mut PdfDocument, page: u32, new: &NewText) -> Result<()> {
    let count = doc.page_count()?.max(0) as u32;
    if page >= count {
        return Err(EngineError::BadPage(page));
    }
    let [r, g, b] = new.color;
    if ![r, g, b].iter().all(|v| (0.0..=1.0).contains(v)) {
        return Err(invalid("colour components must be between 0 and 1"));
    }
    if !(new.size.is_finite() && (1.0..=1000.0).contains(&new.size)) {
        return Err(invalid("the font size must be between 1 and 1000 points"));
    }
    if !new.at.iter().all(|v| v.is_finite() && v.abs() < 1e6) {
        return Err(invalid("the text can't be placed there"));
    }
    let text = new
        .text
        .replace("\r\n", "\n")
        .split('\n')
        .map(|p| p.split(paragraph_separator).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        return Err(invalid("there is no text to add"));
    }
    let p = doc.find_page(page as i32)?;
    let size = new.size;
    // Helvetica, with the usual fallbacks (standard faces, bundled Noto for anything else).
    let mut setter = Setter::new(doc, p.clone());
    let primary = setter.std_face("Helvetica")?;
    let spec = RunSpec {
        source: None,
        text: Some(text),
        primary,
        own_font: false,
        size,
        fill: vec![Op::new("rg", vec![num(r), num(g), num(b)])],
        bold: false,
        italic: false,
        family: 2,
        rise: 0.0,
        underline: false,
        strike: false,
    };
    let items = setter.items(&spec)?;
    let m = Measure { styles: &setter.styles, cs: 0.0, scale: 1.0 };
    let width = match new.width {
        Some(w) if w.is_finite() && w > 0.0 => w.clamp(size, 14_400.0) + 0.5,
        Some(_) => return Err(invalid("the width must be a positive number")),
        None => f64::MAX,
    };
    let lines = wrap(items, width, &m);
    // Text space: x along the line, y up as the page is shown. The first baseline sits under the
    // top of the box by about the font's ascent.
    let flip = Matrix([1.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
    let lin = Matrix([new.back.0[0], new.back.0[1], new.back.0[2], new.back.0[3], 0.0, 0.0]);
    let upright = flip.then(&lin);
    let (ux, uy) = new.back.apply(new.at[0], new.at[1] + size * 0.94);
    let mut ops = vec![
        Op::new("q", vec![]),
        Op::new("cm", vec![num(upright.0[0]), num(upright.0[1]), num(upright.0[2]), num(upright.0[3]), num(ux), num(uy)]),
        Op::new("MP", vec![Obj::name("TouchUp_TextEdit")]),
        Op::new("BT", vec![]),
    ];
    let placement = Placement { align: Align::Left, width: 0.0, lead: size * 1.2, first_size: size, stroke: 0.03, render_mode: 0 };
    ops.extend(emit(&setter, &lines, &m, &placement).0);
    setter.finish()?;
    drop(setter);
    ops.push(Op::new("ET", vec![]));
    ops.push(Op::new("Q", vec![]));

    let old = page_streams(&p)?;
    // The page's own content is wrapped once: a page that already starts with our `q` and has our
    // `Q` after its content is in a clean state at its end, so the new text only goes last.
    let wrapped = old.first().is_some_and(|(_, d)| d == WRAP_OPEN) && old.iter().skip(1).any(|(_, d)| d == WRAP_CLOSE);
    let mut list = Vec::with_capacity(old.len() + 3);
    if !wrapped {
        list.push(flate_stream(doc, WRAP_OPEN)?);
    }
    list.extend(old.into_iter().map(|(obj, _)| obj));
    if !wrapped {
        list.push(flate_stream(doc, WRAP_CLOSE)?);
    }
    list.push(flate_stream(doc, &serialize_ops(&ops))?);
    let mut arr = doc.new_array()?;
    for o in list {
        arr.array_push(o)?;
    }
    let mut page_obj = p;
    page_obj.dict_put("Contents", arr)?;
    Ok(())
}

/// The streams `add_text` puts before and after a page's own content.
const WRAP_OPEN: &[u8] = b"q\n";
const WRAP_CLOSE: &[u8] = b"\nQ\n";

/// Flate (zlib) bytes of `data`.
pub(crate) fn flate(data: &[u8]) -> Result<Vec<u8>> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(data).map_err(|e| EngineError::Io(e.to_string()))?;
    z.finish().map_err(|e| EngineError::Io(e.to_string()))
}

fn flate_stream(doc: &mut PdfDocument, data: &[u8]) -> Result<PdfObject> {
    let packed = flate(data)?;
    let mut dict = doc.new_dict()?;
    dict.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
    Ok(doc.add_stream(&Buffer::from_bytes(&packed)?, Some(&dict), true)?)
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
pub(crate) fn splice(pieces: &Pieces, ops: &[Op], mut edit: impl FnMut(usize) -> (Vec<Op>, bool)) -> Vec<Vec<u8>> {
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
    write_streams(doc, &pc.page, &pc.streams, new)
}

/// Puts rewritten pieces back on `page`: changed ones become new (Flate) streams, the others
/// stay. `streams` are the page's content streams as read.
pub(crate) fn write_streams(doc: &mut PdfDocument, page: &PdfObject, streams: &[(PdfObject, Vec<u8>)], new: Vec<Vec<u8>>) -> Result<()> {
    let mut list = Vec::with_capacity(new.len());
    for ((obj, old), data) in streams.iter().zip(new) {
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
    let mut page = page.clone();
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

    fn group(src: &str) -> Option<usize> {
        deco_group(&super::super::content::parse(src.as_bytes()), 0)
    }

    #[test]
    fn a_decoration_group_is_exactly_what_deco_ops_writes() {
        let tag = "/Span <</IWDeco /U>> BDC";
        // Ours: with each kind of fill, and with the group closed at its own EMC.
        assert_eq!(group(&format!("{tag} q 1 0 0 1 5 6 cm 0 g 0 -1 30 0.6 re f Q EMC")), Some(7));
        assert_eq!(group(&format!("{tag} q 1 0 0 1 5 6 cm 1 0 0 rg 0 -1 30 0.6 re f Q EMC")), Some(7));
        assert_eq!(group(&format!("{tag} q 1 0 0 1 5 6 cm /DeviceN cs 0.1 0.2 scn 0 -1 30 0.6 re f Q EMC")), Some(8));
        // Not ours: a lone Q, a bare cm (it would leak into what follows), a lost fill, a wrong order, text inside.
        for bad in [
            "Q 0 -1 30 0.6 re f EMC",
            "1 0 0 1 100 0 cm 0 g 0 -1 30 0.6 re f EMC",
            "q 1 0 0 1 5 6 cm 0 -1 30 0.6 re f Q EMC",
            "q 0 g 1 0 0 1 5 6 cm 0 -1 30 0.6 re f Q EMC",
            "q 1 0 0 1 5 6 cm 0 g 0 -1 30 0.6 re Q f EMC",
            "q 1 0 0 1 5 6 cm 0 g 0 -1 30 0.6 re f Q Q EMC",
            "q 1 0 0 1 5 6 cm 0 g 0 -1 30 0.6 re f Q",
            "q 1 0 0 1 5 6 cm 0 g 0 -1 30 0.6 re f BT ET Q EMC",
        ] {
            assert_eq!(group(&format!("{tag} {bad}")), None, "{bad}");
        }
        assert_eq!(group("/Span <</Other /U>> BDC q 1 0 0 1 5 6 cm 0 g 0 -1 30 0.6 re f Q EMC"), None);
    }

    #[test]
    fn normalizes_spaces_and_keeps_breaks() {
        assert_eq!(normalize("  a \t b\r\nc  "), "a b\nc");
        assert_eq!(collapse(" a \t b\r\nc  "), " a b\nc ");
    }
}
