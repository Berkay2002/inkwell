//! "Editable text": the recognised lines become real text objects in a bundled font and the pixels
//! under them are replaced by the paper, so the page can be changed with the Edit text tool.
//!
//! Per page:
//!
//! 1. The picture must be the one image that fills the page (like the cleaned-up style); a page that
//!    does not qualify keeps its picture and gets the invisible layer instead (reported).
//! 2. Every word is judged. A word is turned into text only when the engine was sure of it, the
//!    bundled fonts have all its letters and the paper around it is one flat colour (a word printed
//!    on a photograph is left in the picture). The rest stays in the picture and is written as
//!    invisible text, so it is still searchable; the report lists those words.
//! 3. Lines get a baseline, a size and a colour measured from the pixels (the engines report boxes,
//!    not fonts). Text is one `TJ` per run of confident words on a line: a bundled Noto face with
//!    `Tz` fitted to the line and a gap before every word that puts it where the scan had it.
//! 4. The boxes of the converted words are painted with the colour of their surroundings and the
//!    page's own copy of the picture is replaced (an image another page shares stays as it was).
//!
//! The fonts are subset once per document, after every page has been planned.

use std::sync::atomic::{AtomicBool, Ordering};

use mupdf::Font;
use mupdf::pdf::PdfDocument;
use serde::{Deserialize, Serialize};

use super::clean;
use super::raster::{self, Raster};
use super::{OcrError, PageResult, PageStatus, Progress, layer};
use crate::textedit::bundled::{self, Subset};
use crate::textedit::content::Matrix;

/// Tesseract confidence (0..100) below which a word is not turned into text.
const MIN_CONF: f32 = 60.0;
/// Height of the tallest letters of a line as a share of its font size (cap height and ascenders of
/// the bundled faces).
const ASC: f32 = 0.74;
/// Side bearing of the glyphs at the ends of a word, in em: the box wraps ink, the pen starts left of it.
const TRIM: f32 = 0.04;
const MAX_UNSURE: usize = 200;
const MAX_NOTES: usize = 5;
/// A ring pixel within this many levels of the ring's median counts as flat paper.
const FLAT_TOL: i32 = 28;
const FLAT_SHARE: f32 = 0.75;
const TZ_MIN: f32 = 60.0;
const TZ_MAX: f32 = 160.0;

/// Which bundled face family to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontPref {
    /// Monospaced when the page's words all advance alike, Sans otherwise.
    #[default]
    Auto,
    Sans,
    Mono,
}

/// A word that stayed in the picture.
#[derive(Clone, Debug, Serialize)]
pub struct UnsureWord {
    /// Zero-based page.
    pub page: u32,
    pub text: String,
    /// `[x0, y0, x1, y1]` on the displayed page, in points.
    pub bbox: [f32; 4],
    /// `low-confidence`, `unusual`, `no-font` or `picture`.
    pub why: &'static str,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct EditableReport {
    /// Pages whose text is now real text and whose picture lost it.
    pub pages_converted: u32,
    /// Pages left as a picture with an invisible layer (several pictures, a redaction mark, …).
    pub pages_kept: u32,
    pub words_converted: u32,
    /// Words left in the picture (listed in `unsure`, up to a limit).
    pub words_kept: u32,
    pub unsure: Vec<UnsureWord>,
    /// Why, for the first few kept pages.
    pub notes: Vec<String>,
}

/// What [`apply`] did, for the caller's report.
pub(crate) struct Applied {
    pub pages_written: u32,
    pub words: u32,
    pub report: EditableReport,
}

// ---- judging words -----------------------------------------------------------------------------

const LETTERS_WITH_ASCENDERS: &str = "bdfhijklt";

fn has_ascender(text: &str) -> bool {
    text.chars().any(|c| c.is_uppercase() || c.is_ascii_digit() || LETTERS_WITH_ASCENDERS.contains(c))
}

/// Windows OCR gives no confidence. Marks that are not punctuation, or a digit between two lower
/// case letters (a `1` for an `l`), are what its misreads look like.
fn suspect(text: &str) -> bool {
    const PUNCT: &str = ".,;:!?'\"’‘“”()[]-–—/%$€£&@#*+=°§•…«»";
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return true;
    }
    if chars.iter().any(|c| !(c.is_alphanumeric() || PUNCT.contains(*c))) {
        return true;
    }
    chars.windows(3).any(|w| w[1].is_ascii_digit() && w[0].is_lowercase() && w[2].is_lowercase())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Real,
    Keep(&'static str),
}

// ---- the page's pixels --------------------------------------------------------------------------

/// The text frame of one page mapped onto its picture.
struct Frame<'a> {
    page: &'a PageResult,
    px_from_page: Matrix,
    w: usize,
    h: usize,
    /// Points per picture pixel.
    step: f32,
}

impl Frame<'_> {
    fn pixel(&self, tx: f32, ty: f32) -> (f64, f64) {
        let (px, py) = self.page.to_page(tx, ty);
        self.px_from_page.apply(f64::from(px), f64::from(py))
    }

    /// The pixels a text-frame box covers (the box of its four corners), clipped to the picture.
    fn rect(&self, b: [f32; 4]) -> Option<[usize; 4]> {
        let c = [self.pixel(b[0], b[1]), self.pixel(b[2], b[1]), self.pixel(b[2], b[3]), self.pixel(b[0], b[3])];
        let min = |f: fn(&(f64, f64)) -> f64| c.iter().map(f).fold(f64::MAX, f64::min);
        let max = |f: fn(&(f64, f64)) -> f64| c.iter().map(f).fold(f64::MIN, f64::max);
        let x0 = min(|p| p.0).floor().max(0.0);
        let y0 = min(|p| p.1).floor().max(0.0);
        let x1 = max(|p| p.0).ceil().min(self.w as f64);
        let y1 = max(|p| p.1).ceil().min(self.h as f64);
        (x1 > x0 && y1 > y0).then(|| [x0 as usize, y0 as usize, x1 as usize, y1 as usize])
    }

    fn index(&self, tx: f32, ty: f32) -> Option<usize> {
        let (x, y) = self.pixel(tx, ty);
        (x >= 0.0 && y >= 0.0 && x < self.w as f64 && y < self.h as f64).then(|| y as usize * self.w + x as usize)
    }
}

/// The baseline and the top of the ink of a line (text frame `y`), from the rows of ink in its box.
/// The baseline is the row where the ink thins out the most (letters end, descenders are sparse);
/// the top is the first row with a few ink columns and a small share of the densest row (a line with
/// only a handful of ascenders has little ink up there).
fn profile(f: &Frame, ink: &[bool], b: [f32; 4]) -> Option<(f32, f32)> {
    let rows = ((b[3] - b[1]) / f.step).ceil() as usize;
    if rows < 5 {
        return None;
    }
    let mut d = vec![0i32; rows + 1];
    for (r, slot) in d.iter_mut().take(rows).enumerate() {
        let ty = b[1] + (r as f32 + 0.5) * f.step;
        let mut tx = b[0];
        while tx < b[2] {
            if f.index(tx, ty).is_some_and(|i| ink[i]) {
                *slot += 1;
            }
            tx += f.step * 2.0;
        }
    }
    let m = *d.iter().max()?;
    if m < 3 {
        return None;
    }
    let top = d.iter().position(|&v| v >= 2 && v as f32 >= 0.04 * m as f32)?;
    let from = (rows / 4).max(top + 1);
    let mut best = (0i32, None);
    for r in from..rows {
        let drop = d[r] - d[r + 1];
        if drop > best.0 {
            best = (drop, Some(r));
        }
    }
    let r = best.1?;
    let baseline = b[1] + (r as f32 + 1.0) * f.step;
    let top = b[1] + top as f32 * f.step;
    (baseline > top + 3.0 * f.step).then_some((baseline, top))
}

/// The one colour the paper around `rect` has, or `None` when it is not flat (a photograph, a
/// gradient, a neighbour's ink).
fn flat_ring(ras: &Raster, rect: [usize; 4], width: usize) -> Option<Vec<u8>> {
    let [x0, y0, x1, y1] = rect;
    let (ex0, ey0) = (x0.saturating_sub(width), y0.saturating_sub(width));
    let (ex1, ey1) = ((x1 + width).min(ras.w), (y1 + width).min(ras.h));
    let n = ras.n;
    let mut ring: Vec<usize> = Vec::new();
    for y in ey0..ey1 {
        for x in ex0..ex1 {
            if x >= x0 && x < x1 && y >= y0 && y < y1 {
                continue;
            }
            ring.push((y * ras.w + x) * n);
        }
    }
    if ring.len() < 8 {
        return None;
    }
    let med: Vec<u8> = (0..n)
        .map(|c| {
            let mut v: Vec<u8> = ring.iter().map(|&i| ras.px[i + c]).collect();
            let mid = v.len() / 2;
            *v.select_nth_unstable(mid).1
        })
        .collect();
    let close = ring
        .iter()
        .filter(|&&i| (0..n).all(|c| (i32::from(ras.px[i + c]) - i32::from(med[c])).abs() <= FLAT_TOL))
        .count();
    (close as f32 >= FLAT_SHARE * ring.len() as f32).then_some(med)
}

fn fill(ras: &mut Raster, rect: [usize; 4], colour: &[u8]) {
    let [x0, y0, x1, y1] = rect;
    for y in y0..y1 {
        for x in x0..x1 {
            let o = (y * ras.w + x) * ras.n;
            ras.px[o..o + ras.n].copy_from_slice(&colour[..ras.n]);
        }
    }
}

/// The colour of the ink inside the boxes: the mean of the most contrasting quarter of its pixels.
fn ink_colour(ras: &Raster, luma: &[u8], ink: &[bool], rects: &[[usize; 4]], dark_on_light: bool) -> [f32; 3] {
    let mut px: Vec<usize> = Vec::new();
    for &[x0, y0, x1, y1] in rects {
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * ras.w + x;
                if ink[i] {
                    px.push(i);
                }
            }
        }
    }
    if px.is_empty() {
        return [0.0; 3];
    }
    px.sort_unstable_by_key(|&i| if dark_on_light { i32::from(luma[i]) } else { -i32::from(luma[i]) });
    px.truncate((px.len() / 4).max(1));
    let mut sum = [0f32; 3];
    for &i in &px {
        for c in 0..3 {
            sum[c] += f32::from(ras.px[i * ras.n + c.min(ras.n - 1)]) / 255.0;
        }
    }
    sum.map(|s| (s / px.len() as f32 * 100.0).round() / 100.0)
}

// ---- fonts --------------------------------------------------------------------------------------

/// The bundled faces loaded so far and the glyphs used from each.
struct Kit {
    fonts: Vec<Option<Font>>,
    subsets: Vec<Subset>,
}

/// A word set in one face.
struct Set {
    face: usize,
    glyphs: Vec<(u16, char)>,
    /// Sum of the advances, thousandths of an em.
    adv: f64,
}

impl Kit {
    fn new() -> Self {
        let n = bundled::FACES.len();
        Self { fonts: (0..n).map(|_| None).collect(), subsets: (0..n).map(|_| Subset::default()).collect() }
    }

    fn font(&mut self, i: usize) -> Option<&Font> {
        if self.fonts[i].is_none() {
            self.fonts[i] = bundled::load(i).ok();
        }
        self.fonts[i].as_ref()
    }

    /// `text` in face `i`, or `None` when the face lacks a letter.
    fn set(&mut self, i: usize, text: &str) -> Option<Set> {
        let mut glyphs = Vec::new();
        let mut adv = 0.0;
        for c in text.chars().filter(|c| !c.is_control() && *c != ' ') {
            let font = self.font(i)?;
            let g = bundled::glyph(font, c)?;
            adv += bundled::advance(font, g);
            glyphs.push((g, c));
        }
        if glyphs.is_empty() || glyphs.iter().any(|(g, c)| !self.subsets[i].fits(*g, &c.to_string())) {
            return None;
        }
        Some(Set { face: i, glyphs, adv })
    }

    /// The face for a word: the page's primary face, the CJK one for ideographs.
    fn set_for(&mut self, primary: usize, text: &str) -> Option<Set> {
        if let Some(s) = self.set(primary, text) {
            return Some(s);
        }
        if text.chars().any(|c| c as u32 >= 0x2E80) { self.set(bundled::cjk(), text) } else { None }
    }

    /// Records the glyphs of a word that is going to be written and returns its codes.
    fn use_set(&mut self, s: &Set) -> Vec<u8> {
        let mut out = Vec::with_capacity(s.glyphs.len() * 2);
        for (g, c) in &s.glyphs {
            self.subsets[s.face].use_glyph(*g, &c.to_string());
            out.extend(self.subsets[s.face].code(*g));
        }
        out
    }

    fn space(&mut self, face: usize) -> Option<(Vec<u8>, f64)> {
        let font = self.font(face)?;
        let g = bundled::glyph(font, ' ')?;
        let adv = bundled::advance(font, g);
        self.subsets[face].use_glyph(g, " ");
        Some((self.subsets[face].code(g), adv))
    }
}

/// Whether the words of a page advance alike (a typewriter or a terminal listing): the width per
/// letter of words of four or more letters varies by under 7 %.
fn looks_monospaced(page: &PageResult) -> bool {
    let per: Vec<f32> = page
        .lines
        .iter()
        .flatten()
        .filter(|w| w.text.chars().count() >= 4 && w.text.chars().all(char::is_alphanumeric))
        .map(|w| (w.bbox[2] - w.bbox[0]) / w.text.chars().count() as f32)
        .collect();
    if per.len() < 10 {
        return false;
    }
    let mean = per.iter().sum::<f32>() / per.len() as f32;
    let var = per.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / per.len() as f32;
    mean > 0.0 && var.sqrt() / mean < 0.07
}

// ---- the plan -----------------------------------------------------------------------------------

/// One piece of a `TJ` array.
enum Piece {
    Codes(Vec<u8>),
    /// Thousandths of an em; positive moves the pen back.
    Gap(f32),
}

/// A run of words in one face on one line.
struct Run {
    face: usize,
    size: f32,
    colour: [f32; 3],
    /// Horizontal scaling, percent.
    tz: f32,
    /// Where the pen starts, text frame.
    x: f32,
    baseline: f32,
    pieces: Vec<Piece>,
}

struct PagePlan {
    page: u32,
    runs: Vec<Run>,
    /// The words that stay in the picture, as a result of their own (for the invisible layer).
    hidden: PageResult,
}

fn median(v: &mut [f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    Some(v[v.len() / 2])
}

fn page_box(r: &PageResult, b: [f32; 4]) -> [f32; 4] {
    let c = [r.to_page(b[0], b[1]), r.to_page(b[2], b[1]), r.to_page(b[2], b[3]), r.to_page(b[0], b[3])];
    let min = |f: fn(&(f32, f32)) -> f32| c.iter().map(f).fold(f32::MAX, f32::min);
    let max = |f: fn(&(f32, f32)) -> f32| c.iter().map(f).fold(f32::MIN, f32::max);
    [min(|p| p.0), min(|p| p.1), max(|p| p.0), max(|p| p.1)]
}

struct Converted {
    plan: PagePlan,
    words: u32,
    kept: Vec<(String, [f32; 4], &'static str)>,
}

/// Plans one page and rewrites its picture. `Err` is the reason the page keeps its picture; nothing
/// has been changed then.
fn convert_page(doc: &mut PdfDocument, r: &PageResult, kit: &mut Kit, pref: FontPref) -> Result<Converted, String> {
    let mut page = doc.load_pdf_page(r.page as i32).map_err(|e| e.to_string())?;
    let scan = clean::find_scan(doc, &page, r.page, r.page_size)?;
    let (mut ras, family) = clean::decode(&scan.obj)?;
    let px_from_page = scan.page_from_pixel.invert().ok_or_else(|| "its picture has a singular transform".to_string())?;
    let luma = ras.luma();
    let thr = raster::otsu(&luma);
    let paper = raster::paper(&ras, &luma, thr);
    let paper_luma = if paper.len() == 1 { paper[0] } else { ((299 * u32::from(paper[0]) + 587 * u32::from(paper[1]) + 114 * u32::from(paper[2])) / 1000) as u8 };
    let dark_on_light = paper_luma >= thr;
    let ink: Vec<bool> = luma.iter().map(|&l| if dark_on_light { l < thr } else { l >= thr }).collect();
    let frame = Frame { page: r, px_from_page, w: ras.w, h: ras.h, step: (72.0 / scan.dpi.max(36.0)).min(2.0) };
    let px_per_pt = scan.dpi.max(36.0) / 72.0;

    let mono = match pref {
        FontPref::Mono => true,
        FontPref::Sans => false,
        FontPref::Auto => looks_monospaced(r),
    };
    let primary = bundled::latin(mono, false, false);

    // Lines: boxes, baselines and sizes.
    struct LineInfo {
        baseline: f32,
        size: f32,
        own_size: bool,
    }
    let mut infos: Vec<LineInfo> = Vec::new();
    let mut sizes: Vec<f32> = Vec::new();
    let mut fallback_sizes: Vec<f32> = Vec::new();
    for line in &r.lines {
        if line.is_empty() {
            infos.push(LineInfo { baseline: 0.0, size: 0.0, own_size: false });
            continue;
        }
        let x0 = line.iter().map(|w| w.bbox[0]).fold(f32::MAX, f32::min);
        let y0 = line.iter().map(|w| w.bbox[1]).fold(f32::MAX, f32::min);
        let x1 = line.iter().map(|w| w.bbox[2]).fold(f32::MIN, f32::max);
        let y1 = line.iter().map(|w| w.bbox[3]).fold(f32::MIN, f32::max);
        let height = (y1 - y0).max(1.0);
        let b = [x0, y0 - 2.0 * frame.step, x1, y1 + 2.0 * frame.step];
        let asc = line.iter().any(|w| has_ascender(&w.text));
        let (baseline, size, own) = match profile(&frame, &ink, b) {
            Some((base, top)) => {
                let s = (base - top) / ASC;
                if asc {
                    sizes.push(s);
                } else {
                    fallback_sizes.push(s);
                }
                (base, s, asc)
            }
            None => {
                // No ink to measure: a box holds ascenders and descenders, about one em.
                let s = height / 1.1;
                fallback_sizes.push(s);
                (y1 - 0.2 * s, s, false)
            }
        };
        infos.push(LineInfo { baseline, size, own_size: own });
    }
    let page_size = median(&mut sizes).or_else(|| median(&mut fallback_sizes)).unwrap_or(10.0).max(2.0);
    for i in &mut infos {
        if !i.own_size || i.size < page_size * 0.5 || i.size > page_size * 6.0 {
            i.size = page_size;
        }
    }

    // Judge the words and decide what to paint.
    let mut verdicts: Vec<Vec<Verdict>> = Vec::new();
    let mut sets: Vec<Vec<Option<Set>>> = Vec::new();
    let mut paints: Vec<([usize; 4], Vec<u8>)> = Vec::new();
    for (li, line) in r.lines.iter().enumerate() {
        let size = infos[li].size;
        let ring = (px_per_pt * size * 0.06).round().clamp(2.0, 6.0) as usize;
        let mut vs = Vec::with_capacity(line.len());
        let mut ss = Vec::with_capacity(line.len());
        for w in line {
            let sure = match w.conf {
                Some(c) => c >= MIN_CONF && !suspect(&w.text),
                None => !suspect(&w.text),
            };
            if !sure {
                vs.push(Verdict::Keep(if w.conf.is_some_and(|c| c < MIN_CONF) { "low-confidence" } else { "unusual" }));
                ss.push(None);
                continue;
            }
            let Some(set) = kit.set_for(primary, &w.text) else {
                vs.push(Verdict::Keep("no-font"));
                ss.push(None);
                continue;
            };
            // Paint a little beyond the box: its edge is the ink's edge, the soft halo lies outside.
            let (px, py) = (size * 0.08, size * 0.06);
            let pad = [w.bbox[0] - px, w.bbox[1] - py, w.bbox[2] + px, w.bbox[3] + py];
            let flat = frame.rect(pad).and_then(|rect| flat_ring(&ras, rect, ring).map(|c| (rect, c)));
            match flat {
                Some((rect, colour)) => {
                    paints.push((rect, colour));
                    vs.push(Verdict::Real);
                    ss.push(Some(set));
                }
                None => {
                    vs.push(Verdict::Keep("picture"));
                    ss.push(None);
                }
            }
        }
        verdicts.push(vs);
        sets.push(ss);
    }
    if !verdicts.iter().flatten().any(|v| *v == Verdict::Real) {
        return Err("no word could be turned into text".into());
    }

    // Colours come from the untouched picture.
    let colours: Vec<[f32; 3]> = r
        .lines
        .iter()
        .enumerate()
        .map(|(li, line)| {
            let rects: Vec<[usize; 4]> = line
                .iter()
                .enumerate()
                .filter(|(wi, _)| verdicts[li][*wi] == Verdict::Real)
                .filter_map(|(_, w)| frame.rect(w.bbox))
                .collect();
            ink_colour(&ras, &luma, &ink, &rects, dark_on_light)
        })
        .collect();

    // Runs.
    let mut runs: Vec<Run> = Vec::new();
    let mut words = 0u32;
    let mut kept: Vec<(String, [f32; 4], &'static str)> = Vec::new();
    let mut hidden_lines: Vec<Vec<super::Word>> = Vec::new();
    for (li, line) in r.lines.iter().enumerate() {
        let info = &infos[li];
        let size = info.size;
        let hidden: Vec<super::Word> = line
            .iter()
            .enumerate()
            .filter(|(wi, _)| verdicts[li][*wi] != Verdict::Real)
            .map(|(_, w)| w.clone())
            .collect();
        for (wi, w) in line.iter().enumerate() {
            if let Verdict::Keep(why) = verdicts[li][wi] {
                kept.push((w.text.clone(), page_box(r, w.bbox), why));
            }
        }
        if !hidden.is_empty() {
            hidden_lines.push(hidden);
        }
        // One horizontal scale for the line: no word may grow past its own box (words would run
        // together; every word is anchored at its own start, so narrower text only widens the gaps).
        // The tightest word sets it, unless a single odd box would squeeze the whole line.
        let mut ratios: Vec<f32> = Vec::new();
        let mut short: Vec<f32> = Vec::new();
        for (wi, w) in line.iter().enumerate() {
            if let Some(s) = &sets[li][wi] {
                let natural = (s.adv as f32 / 1000.0 - 2.0 * TRIM) * size;
                if natural > 0.0 {
                    let r = (w.bbox[2] - w.bbox[0]) / natural;
                    if s.glyphs.len() >= 3 { ratios.push(r) } else { short.push(r) }
                }
            }
        }
        if ratios.is_empty() {
            ratios = short;
        }
        let Some(median) = median(&mut ratios.clone()) else { continue };
        let tightest = ratios.iter().copied().fold(f32::MAX, f32::min);
        let tz = (100.0 * tightest.max(0.85 * median)).clamp(TZ_MIN, TZ_MAX);
        let unit = size * tz / 100.0 / 1000.0; // points per thousandth of an em
        let mut open: Option<(Run, f32)> = None; // (run, pen x)
        let last_real = (0..line.len()).rev().find(|&wi| sets[li][wi].is_some());
        for (wi, w) in line.iter().enumerate() {
            let Some(set) = &sets[li][wi] else {
                if let Some((run, _)) = open.take() {
                    runs.push(run);
                }
                continue;
            };
            words += 1;
            if open.as_ref().is_some_and(|(run, _)| run.face != set.face) {
                runs.push(open.take().expect("checked").0);
            }
            let want = w.bbox[0] - TRIM * size * tz / 100.0;
            let (run, pen) = open.get_or_insert_with(|| {
                (Run { face: set.face, size, colour: colours[li], tz, x: want, baseline: info.baseline, pieces: Vec::new() }, want)
            });
            let shift = want - *pen;
            if shift.abs() > 0.05 {
                run.pieces.push(Piece::Gap(-shift / unit));
                *pen = want;
            }
            let codes = kit.use_set(set);
            run.pieces.push(Piece::Codes(codes));
            *pen += set.adv as f32 * unit;
            // A real space after the word keeps extractors from running words together.
            let next_in_run = (wi + 1..line.len()).next().is_some_and(|n| sets[li][n].as_ref().is_some_and(|s| s.face == set.face));
            if Some(wi) != last_real && next_in_run {
                if let Some((code, adv)) = kit.space(set.face) {
                    run.pieces.push(Piece::Codes(code));
                    *pen += adv as f32 * unit;
                }
            }
        }
        if let Some((run, _)) = open.take() {
            runs.push(run);
        }
    }

    // Everything is decided: paint, re-encode, point the page at the new picture.
    for (rect, colour) in &paints {
        fill(&mut ras, *rect, colour);
    }
    let new = clean::encode(doc, &scan.obj, family, &ras).map_err(|e| e.to_string())?;
    clean::rebind(doc, &mut page, &scan.name, new).map_err(|e| e.to_string())?;

    let mut hidden = r.clone();
    hidden.lines = hidden_lines;
    Ok(Converted { plan: PagePlan { page: r.page, runs, hidden }, words, kept })
}

fn num(out: &mut String, v: f32) {
    use std::fmt::Write as _;
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    let _ = write!(out, "{}", if s.is_empty() || s == "-" || s == "-0" { "0" } else { s });
}

/// The content stream of a page's text: one `q … Q` with a `Tm`, `Tz` and `TJ` per run.
fn text_stream(plan: &PagePlan, user_from_page: &mupdf::Matrix) -> Vec<u8> {
    let m = user_from_page;
    let r = &plan.hidden;
    let (sin, cos) = r.baseline_deg().to_radians().sin_cos();
    let (ux, uy) = (m.a * cos + m.c * sin, m.b * cos + m.d * sin);
    let (vx, vy) = (m.a * sin - m.c * cos, m.b * sin - m.d * cos);
    let mut s = String::from("q\nBT\n0 Tr 0 Tc 0 Tw\n");
    let mut colour = [-1.0f32; 3];
    for run in &plan.runs {
        if run.colour != colour {
            for c in run.colour {
                num(&mut s, c);
                s.push(' ');
            }
            s.push_str("rg\n");
            colour = run.colour;
        }
        s.push_str(&format!("/{} ", face_res(run.face)));
        num(&mut s, run.size);
        s.push_str(" Tf\n");
        let (x, y) = r.to_page(run.x, run.baseline);
        let (ex, ey) = (m.a * x + m.c * y + m.e, m.b * x + m.d * y + m.f);
        for v in [ux, uy, vx, vy, ex, ey] {
            num(&mut s, v);
            s.push(' ');
        }
        s.push_str("Tm ");
        num(&mut s, run.tz);
        s.push_str(" Tz [");
        for p in &run.pieces {
            match p {
                Piece::Codes(c) => {
                    s.push('<');
                    for b in c {
                        s.push_str(&format!("{b:02X}"));
                    }
                    s.push('>');
                }
                Piece::Gap(g) => {
                    s.push(' ');
                    num(&mut s, *g);
                    s.push(' ');
                }
            }
        }
        s.push_str("] TJ\n");
    }
    s.push_str("ET\nQ\n");
    s.into_bytes()
}

/// Resource name of face `i` in a page's `/Font` dictionary.
fn face_res(i: usize) -> String {
    format!("InkEdF{i}")
}

/// Converts the recognised pages of `results` (see the module docs). Pages that do not qualify
/// get the invisible layer instead and are counted in the report.
/// `cancel` is looked at before each page's work (`Err(Cancelled)`: the caller drops the document)
/// and `progress` sees each page converted or written. The writing pass counts as the second half.
pub(crate) fn apply(
    doc: &mut PdfDocument,
    results: &[PageResult],
    pref: FontPref,
    progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
) -> Result<Applied, OcrError> {
    let todo: Vec<&PageResult> = results.iter().filter(|r| r.status == PageStatus::Recognized).collect();
    let total = todo.len() as u32;
    let stop = || if cancel.load(Ordering::Relaxed) { Err(OcrError::Cancelled) } else { Ok(()) };
    let mut report = EditableReport::default();
    let mut out = Applied { pages_written: 0, words: 0, report: EditableReport::default() };
    if todo.is_empty() {
        return Ok(out);
    }
    let mut kit = Kit::new();
    let mut plans: Vec<PagePlan> = Vec::new();
    let mut fallback: Vec<&PageResult> = Vec::new();
    for (n, r) in todo.into_iter().enumerate() {
        stop()?;
        let converted = convert_page(doc, r, &mut kit, pref);
        progress(Progress { done: n as u32 + 1, total, page: r.page, writing: true });
        match converted {
            Ok(c) => {
                report.pages_converted += 1;
                report.words_converted += c.words;
                report.words_kept += c.kept.len() as u32;
                for (text, bbox, why) in c.kept {
                    if report.unsure.len() < MAX_UNSURE {
                        report.unsure.push(UnsureWord { page: r.page, text, bbox, why });
                    }
                }
                plans.push(c.plan);
            }
            Err(why) => {
                report.pages_kept += 1;
                if report.notes.len() < MAX_NOTES {
                    report.notes.push(format!("Page {}: {why}; it keeps its picture and gets the invisible text layer", r.page + 1));
                }
                fallback.push(r);
            }
        }
    }

    // Fonts: once per document, with every glyph the pages use.
    let mut fonts: Vec<(usize, mupdf::pdf::PdfObject)> = Vec::new();
    for i in 0..bundled::FACES.len() {
        if kit.subsets[i].is_empty() {
            continue;
        }
        let Some(font) = kit.fonts[i].as_ref() else { continue };
        let obj = bundled::embed(doc, i, font, &kit.subsets[i]).map_err(|e| OcrError::Pdf(e.to_string()))?;
        fonts.push((i, obj));
    }
    let hidden_words = plans.iter().map(|p| &p.hidden).chain(fallback.iter().copied());
    let codes = layer::Codes::build(hidden_words.flat_map(|r| r.lines.iter().flatten()).map(|w| w.text.as_str()));
    let any_hidden = plans.iter().any(|p| !p.hidden.lines.is_empty()) || !fallback.is_empty();
    let ghost = if any_hidden { Some(layer::install_font(doc, &codes)?) } else { None };

    for plan in &plans {
        stop()?;
        let mut page = doc.load_pdf_page(plan.page as i32)?;
        let user_from_page = page.ctm()?.invert().ok_or_else(|| OcrError::Pdf(format!("page {} has a singular transform", plan.page + 1)))?;
        let mut res: Vec<(String, mupdf::pdf::PdfObject)> = fonts.iter().map(|(i, o)| (face_res(*i), o.clone())).collect();
        let text = text_stream(plan, &user_from_page);
        let mut streams: Vec<Vec<u8>> = Vec::new();
        if let Some(g) = &ghost {
            if !plan.hidden.lines.is_empty() {
                let name = layer::font_name_for(&page);
                streams.push(layer::layer_stream(&plan.hidden, &user_from_page, &codes, &name));
                res.push((name, g.clone()));
            }
        }
        streams.push(text);
        layer::add_streams(doc, &mut page, &res, &streams.iter().map(Vec::as_slice).collect::<Vec<_>>())?;
        out.pages_written += 1;
    }
    for r in fallback {
        stop()?;
        let Some(g) = &ghost else { break };
        let mut page = doc.load_pdf_page(r.page as i32)?;
        let user_from_page = page.ctm()?.invert().ok_or_else(|| OcrError::Pdf(format!("page {} has a singular transform", r.page + 1)))?;
        let name = layer::font_name_for(&page);
        let stream = layer::layer_stream(r, &user_from_page, &codes, &name);
        layer::add_layer(doc, &mut page, g, &name, &stream)?;
        out.pages_written += 1;
        out.words += r.lines.iter().map(|l| l.len() as u32).sum::<u32>();
    }
    out.words += report.words_converted + report.words_kept;
    out.report = report;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr::Word;

    fn page_of(words: &[(&str, f32, f32)]) -> PageResult {
        PageResult {
            page: 0,
            status: PageStatus::Recognized,
            lines: vec![words.iter().map(|(t, x0, x1)| Word { text: t.to_string(), bbox: [*x0, 0.0, *x1, 10.0], conf: None }).collect()],
            skew_deg: None,
            rotation: 0,
            page_size: [200.0, 100.0],
            ms: 0,
        }
    }

    #[test]
    fn misreads_look_suspect_and_ordinary_words_do_not() {
        for ok in ["Invoice", "$1,234.56", "COVID-19", "B2B", "(see", "e-mail", "Åke", "naïve", "50%"] {
            assert!(!suspect(ok), "{ok}");
        }
        for bad in ["|", "he1lo", "a~b", "wor_d", "{x}", "\\", ""] {
            assert!(suspect(bad), "{bad}");
        }
    }

    #[test]
    fn typed_pages_are_told_from_proportional_ones_by_the_width_per_letter() {
        // Ten 6 pt wide letters per word, whatever the word.
        let mono: Vec<(String, f32, f32)> = (0..12).map(|i| ("abcdefgh"[..4 + i % 4].to_string(), 0.0, 6.0 * (4 + i % 4) as f32)).collect();
        let m: Vec<(&str, f32, f32)> = mono.iter().map(|(t, a, b)| (t.as_str(), *a, *b)).collect();
        assert!(looks_monospaced(&page_of(&m)));
        // "illl" and "mmmm" at their natural widths.
        let prop: Vec<(&str, f32, f32)> = (0..12).map(|i| if i % 2 == 0 { ("illl", 0.0, 8.0) } else { ("mmmm", 0.0, 40.0) }).collect();
        assert!(!looks_monospaced(&page_of(&prop)));
        assert!(!looks_monospaced(&page_of(&[("abcd", 0.0, 24.0)])), "too few words to say");
    }

    #[test]
    fn a_flat_ring_is_paper_and_a_noisy_one_is_not() {
        let mut r = Raster::filled(60, 40, 1, &[250]);
        assert_eq!(flat_ring(&r, [20, 10, 40, 30], 3), Some(vec![250]));
        for y in 0..40 {
            for x in 0..60 {
                r.px[y * 60 + x] = (40 + (x * 37 + y * 91) % 180) as u8;
            }
        }
        assert_eq!(flat_ring(&r, [20, 10, 40, 30], 3), None);
    }

    #[test]
    fn the_baseline_is_where_the_letters_end_not_where_a_descender_ends() {
        // A 100x40 page at 72 dpi: letters from y=10 to y=24 (a baseline at 24), a descender to y=30.
        let mut r = Raster::filled(100, 40, 1, &[255]);
        for x in 10..90 {
            for y in 10..24 {
                if (x / 4) % 2 == 0 {
                    r.px[y * 100 + x] = 0;
                }
            }
        }
        for x in 30..33 {
            for y in 24..30 {
                r.px[y * 100 + x] = 0;
            }
        }
        let page = page_of(&[("abc", 10.0, 90.0)]);
        let f = Frame { page: &page, px_from_page: Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]), w: 100, h: 40, step: 1.0 };
        let ink: Vec<bool> = r.px.iter().map(|&v| v < 128).collect();
        let (baseline, top) = profile(&f, &ink, [10.0, 6.0, 90.0, 34.0]).expect("a profile");
        assert!((baseline - 24.0).abs() <= 1.0, "baseline {baseline}");
        assert!((top - 10.0).abs() <= 1.0, "top {top}");
    }
}
