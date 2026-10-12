//! Redaction: find, mark, apply, sanitize, verify. Standalone on a `PdfDocument`: no UI, history or
//! save logic. The edit lane calls these against the authoritative document.
//!
//! Flow: `find` -> `mark_hits`/`add_mark`/`mark_page` (real /Redact annotations, reviewable) ->
//! `apply` (removes glyphs, image pixels and covered line art under the marks, deletes annotations
//! and optionally form fields under them, paints the overlay, sanitizes, then verifies a full
//! rewrite of the file). After `apply` the document must only ever be written with
//! [`write_options`] (full rewrite, garbage collected): an incremental save keeps the old bytes.
//!
//! Stricter than Acrobat (study 01): catalog/page /AF associated files go with the attachments;
//! annotations of any type under a mark go (MuPDF only drops links and free text); anything
//! quoting redacted text goes even with sanitize off (comments, fields, docinfo, XMP, bookmarks);
//! resources a fully covered pattern/form leaves unused are pruned; no empty /AcroForm is added;
//! and a full rewrite is re-read and searched before `apply` succeeds. Known limit: text inside a
//! partly covered tiling pattern or soft mask is not cut by MuPDF, so `apply` refuses (Leaked).
//!
//! Limits that stay (verify cannot close them):
//! - A corrupt stream MuPDF decodes partially passes as the part it decoded: the tail past the
//!   corruption is neither searched nor removed. A stream that fails outright counts as a leak.
//! - Needles under 5 chars are searched in page text and object strings (3+) only, not in streams,
//!   names or raw bytes (8+ there): in binary data they would match by chance. A short code
//!   quoted in a content stream outside the marks is not caught.
//! - Names (5+ chars) are searched too, so redacting a word that is part of a standard PDF name
//!   (e.g. "Resources") fails unless the word still shows elsewhere on a page.
//! - Text inside a text attachment that is itself encoded (base64 in a mail file, hex, quoted-
//!   printable) is not decoded. Sanitize with embedded files on removes attachments outright.

pub mod app;
mod patterns;
mod sanitize;
#[cfg(test)]
mod tests;

pub use patterns::Pattern;
pub use sanitize::{Sanitize, SanitizeReport, sanitize, sanitize_pages};

use mupdf::pdf::{
    AnnotationArea, AnnotationQuadPoints, PdfAnnotationType, PdfDocument, PdfObject, PdfPage, PdfRedactImageMethod,
    PdfRedactLineArtMethod, PdfRedactOptions, PdfRedactTextMethod, PdfWriteOptions,
};
use mupdf::color::AnnotationColor;
use mupdf::text_page::{TextBlockType, TextPageFlags};
use mupdf::{Buffer, Document, Page, Quad, Rect};
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::collections::HashSet;
use std::fmt::Write as _;

#[derive(Debug, thiserror::Error)]
pub enum RedactError {
    #[error(transparent)]
    Pdf(#[from] mupdf::Error),
    #[error("invalid search: {0}")]
    Query(#[from] regex::Error),
    /// The document still contains redacted content. It has been partly modified: discard it.
    #[error("redaction left recoverable content in {} place(s)", .0.len())]
    Leaked(Vec<Leak>),
    /// The written copy is encrypted and there is no password to check it with: nothing is
    /// known about what it contains, so it is not used.
    #[error("the redacted copy is encrypted and there is no password to check it with")]
    NeedsPassword,
    /// A search that was asked to stop.
    #[error("cancelled")]
    Cancelled,
    /// An option that cannot be done the way it was asked for; nothing was changed by it.
    #[error("{0}")]
    Unsupported(&'static str),
}

type Result<T> = std::result::Result<T, RedactError>;

// ---------------------------------------------------------------------------------------------
// Text index: the page's characters as one searchable string, each char mapped to its quad.

struct IChar {
    byte: usize,
    c: char,
    quad: Quad,
    line: u32,
    /// A line-end hyphen dropped by dehyphenation; `byte` is where the next char starts.
    hidden: bool,
}

#[derive(Default)]
struct PageText {
    text: String,
    chars: Vec<IChar>,
}

impl PageText {
    fn push(&mut self, c: char, quad: Quad, line: u32) {
        self.chars.push(IChar { byte: self.text.len(), c, quad, line, hidden: false });
        self.text.push(c);
    }

    fn selected(&self, r: &std::ops::Range<usize>) -> impl Iterator<Item = &IChar> {
        self.chars
            .iter()
            .filter(move |c| if c.hidden { r.start < c.byte && c.byte < r.end } else { r.contains(&c.byte) })
    }

    /// One quad per line run of the chars in `r`.
    fn quads(&self, r: &std::ops::Range<usize>) -> Vec<Quad> {
        let mut out: Vec<(u32, Quad)> = Vec::new();
        for c in self.selected(r) {
            match out.last_mut() {
                Some((line, q)) if *line == c.line => {
                    q.ur = c.quad.ur;
                    q.lr = c.quad.lr;
                }
                _ => out.push((c.line, c.quad.clone())),
            }
        }
        out.into_iter().map(|(_, q)| q).collect()
    }
}

fn norm(c: char) -> char {
    match c {
        '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{00AD}' => '-',
        c if c.is_whitespace() => ' ',
        c => c,
    }
}

/// Lines in a block are joined with a space, blocks with '\n'. A line-end hyphen between a letter
/// and a lowercase letter is dropped from the text (its quad still goes with the match); any other
/// line-end hyphen joins the lines without a space, so "123-45-\n6789" still reads "123-45-6789".
fn index_page(page: &Page) -> Result<PageText> {
    let tp = page.to_text_page(TextPageFlags::empty())?;
    let mut pt = PageText::default();
    let mut line_no = 0u32;
    for block in tp.blocks() {
        if block.r#type() != TextBlockType::Text {
            continue;
        }
        let mut pending: Option<(Quad, bool, u32)> = None;
        let mut first = true;
        for line in block.lines() {
            let mut cs: Vec<(char, Quad)> = line.chars().filter_map(|c| Some((norm(c.char()?), c.quad()))).collect();
            while cs.last().is_some_and(|c| c.0 == ' ') {
                cs.pop();
            }
            if cs.is_empty() {
                continue;
            }
            line_no += 1;
            if first {
                if !pt.text.is_empty() {
                    pt.text.push('\n');
                }
            } else if let Some((q, letter, l)) = pending.take() {
                if letter && cs[0].0.is_lowercase() {
                    pt.chars.push(IChar { byte: pt.text.len(), c: '-', quad: q, line: l, hidden: true });
                } else {
                    pt.push('-', q, l);
                }
            } else {
                pt.text.push(' ');
            }
            first = false;
            if let Some((_, q)) = cs.pop_if(|c| c.0 == '-') {
                pending = Some((q, cs.last().is_some_and(|c| c.0.is_alphabetic()), line_no));
            }
            for (c, q) in cs {
                pt.push(c, q, line_no);
            }
        }
        if let Some((q, _, l)) = pending {
            pt.push('-', q, l);
        }
    }
    Ok(pt)
}

// ---------------------------------------------------------------------------------------------
// Find

#[derive(Clone, Debug)]
pub enum Query {
    Text { needle: String, case_sensitive: bool, whole_word: bool },
    Regex { pattern: String, case_sensitive: bool },
    Pattern(Pattern),
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub page: i32,
    /// The matched text (normalized: dashes folded, line breaks as spaces, soft hyphens dropped).
    pub text: String,
    /// One quad per line the match covers, in page (fitz) space.
    pub quads: Vec<Quad>,
    /// Up to ~60 chars of the same block before and after the match, cut at a word boundary. Display only.
    pub before: String,
    pub after: String,
}

/// The text around `r` in `text` (within its block: '\n' ends the context), at most `CTX` chars each way.
fn context(text: &str, r: &std::ops::Range<usize>) -> (String, String) {
    const CTX: usize = 60;
    let head = &text[..r.start];
    let head = head.rsplit('\n').next().unwrap_or("");
    let start = head.char_indices().rev().nth(CTX - 1).map_or(0, |(i, _)| i);
    let mut before = &head[start..];
    if start > 0 && !head[..start].ends_with(' ') {
        before = before.split_once(' ').map_or("", |x| x.1);
    }
    let tail = &text[r.end..];
    let tail = tail.split('\n').next().unwrap_or("");
    let end = tail.char_indices().nth(CTX).map_or(tail.len(), |(i, _)| i);
    let mut after = &tail[..end];
    if end < tail.len() && !tail[end..].starts_with(' ') {
        after = after.rsplit_once(' ').map_or("", |x| x.0);
    }
    (before.trim_start().to_owned(), after.trim_end().to_owned())
}

/// `needle` as a regex: words matched literally, any whitespace (incl. line breaks) between them.
fn literal_regex(needle: &str, case_sensitive: bool, whole_word: bool) -> Result<Option<Regex>> {
    let needle: String = needle.chars().map(norm).collect();
    let words: Vec<String> = needle.split_whitespace().map(regex::escape).collect();
    if words.is_empty() {
        return Ok(None);
    }
    let word_char = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let trimmed = needle.trim();
    let mut pat = words.join(r"\s+");
    if whole_word && word_char(trimmed.chars().next()) {
        pat.insert_str(0, r"\b");
    }
    if whole_word && word_char(trimmed.chars().last()) {
        pat.push_str(r"\b");
    }
    Ok(Some(RegexBuilder::new(&pat).case_insensitive(!case_sensitive).build()?))
}

/// A second view of the page text with joins a reader makes but extraction does not: spaces
/// inside letter-spaced runs ("K E R N", 3+ single chars) and a hyphen between two letters (a
/// WinAnsi soft hyphen extracts as '-') or before a line break and a letter. Dropped chars become
/// hidden, so their quads still go with a match. Also returns where the drops are, so `find` keeps only matches that need one.
fn joined(pt: &PageText) -> Option<(PageText, Vec<usize>)> {
    let t: Vec<(usize, char)> = pt.text.char_indices().collect();
    let sep = |i: usize| t.get(i).is_none_or(|c| c.1 == ' ' || c.1 == '\n');
    let single = |i: usize| !sep(i) && (i == 0 || sep(i - 1)) && sep(i + 1);
    let mut drop = vec![false; t.len()];
    let mut i = 0;
    while i < t.len() {
        if !single(i) {
            i += 1;
            continue;
        }
        let mut j = i;
        while t.get(j + 1).is_some_and(|c| c.1 == ' ') && single(j + 2) {
            j += 2;
        }
        if j >= i + 4 {
            (i + 1..j).step_by(2).for_each(|s| drop[s] = true);
        }
        i = j + 1;
    }
    for i in 1..t.len().saturating_sub(1) {
        if t[i].1 == '-' && t[i - 1].1.is_alphabetic() && t[i + 1].1.is_alphabetic() {
            drop[i] = true;
        }
        // A hyphen at the end of a block's last line, the word going on in the next block.
        if t[i].1 == '-' && t[i - 1].1.is_alphabetic() && t[i + 1].1 == '\n' && t.get(i + 2).is_some_and(|c| c.1.is_alphabetic()) {
            drop[i] = true;
            drop[i + 1] = true;
        }
    }
    if !drop.contains(&true) {
        return None;
    }
    let mut text = String::with_capacity(pt.text.len());
    let mut map = vec![0; pt.text.len() + 1];
    let mut dropped = HashSet::new();
    for (k, &(b, c)) in t.iter().enumerate() {
        map[b] = text.len();
        if drop[k] {
            dropped.insert(b);
        } else {
            text.push(c);
        }
    }
    map[pt.text.len()] = text.len();
    let chars = pt
        .chars
        .iter()
        .map(|c| IChar { byte: map[c.byte], c: c.c, quad: c.quad.clone(), line: c.line, hidden: c.hidden || dropped.contains(&c.byte) })
        .collect();
    let at = dropped.iter().map(|&b| map[b]).collect();
    Some((PageText { text, chars }, at))
}

/// The compiled-size limit of a user's regex (the NFA and the lazy DFA each): a pattern that needs
/// more is refused rather than left to eat memory.
pub const REGEX_SIZE_LIMIT: usize = 1 << 20;

pub fn find(doc: &Document, query: &Query) -> Result<Vec<Hit>> {
    find_capped(doc, query, None, usize::MAX, None)
}

/// [`find`] over the pages in `only` (zero-based; every page when `None`), stopping once `max` hits are
/// collected, so a pattern that matches everything on a big document does not build millions of them, and with
/// `Cancelled` when `cancel` is set between pages.
pub fn find_capped(
    doc: &Document,
    query: &Query,
    only: Option<&[u32]>,
    max: usize,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<Hit>> {
    let re = match query {
        Query::Text { needle, case_sensitive, whole_word } => literal_regex(needle, *case_sensitive, *whole_word)?,
        Query::Regex { pattern, case_sensitive } => {
            Some(RegexBuilder::new(pattern).case_insensitive(!case_sensitive).size_limit(REGEX_SIZE_LIMIT).dfa_size_limit(REGEX_SIZE_LIMIT).build()?)
        }
        Query::Pattern(_) => None,
    };
    if re.is_none() && !matches!(query, Query::Pattern(_)) {
        return Ok(Vec::new());
    }
    let mut hits = Vec::new();
    for p in 0..doc.page_count()? {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            return Err(RedactError::Cancelled);
        }
        if only.is_some_and(|o| !o.contains(&(p as u32))) {
            continue;
        }
        let pt = index_page(&doc.load_page(p)?)?;
        let alt = joined(&pt);
        let views = std::iter::once((&pt, None)).chain(alt.as_ref().map(|(v, at)| (v, Some(at))));
        for (pt, joins) in views {
            let ranges = match (query, &re) {
                (Query::Pattern(pat), _) => pat.find(&pt.text),
                (_, Some(re)) => re.find_iter(&pt.text).filter(|m| !m.is_empty()).map(|m| m.range()).collect(),
                _ => Vec::new(),
            };
            for r in ranges {
                // The joined view only adds matches that need a join; the rest the plain view has.
                if joins.is_some_and(|at| !at.iter().any(|&b| r.start < b && b < r.end)) {
                    continue;
                }
                let quads = pt.quads(&r);
                if !quads.is_empty() {
                    let (before, after) = context(&pt.text, &r);
                    hits.push(Hit { page: p, text: pt.text[r].to_owned(), quads, before, after });
                    if hits.len() >= max {
                        return Ok(hits);
                    }
                }
            }
        }
    }
    Ok(hits)
}

// ---------------------------------------------------------------------------------------------
// Mark

#[derive(Clone, Debug, PartialEq)]
pub struct MarkStyle {
    /// Overlay fill painted where content was removed (RGB 0..1).
    pub fill: [f32; 3],
    /// Text drawn on the overlay (e.g. a redaction code), auto-sized to each box.
    pub overlay_text: Option<String>,
}

impl Default for MarkStyle {
    fn default() -> Self {
        Self { fill: [0.0, 0.0, 0.0], overlay_text: None }
    }
}

#[derive(Clone, Debug)]
pub struct Mark {
    pub page: i32,
    /// Object number of the /Redact annotation: the handle for `remove_mark`.
    pub xref: i32,
    pub quads: Vec<Quad>,
    pub style: MarkStyle,
    /// The marked text (/Contents) for text marks; None for area and page marks.
    pub label: Option<String>,
}

/// Adds one /Redact annotation covering `quads` (page space). Returns its object number.
pub fn add_mark(doc: &mut PdfDocument, page: i32, quads: &[Quad], style: &MarkStyle, label: Option<&str>) -> Result<i32> {
    let mut p = doc.load_pdf_page(page)?;
    let area = AnnotationArea::QuadPoints(AnnotationQuadPoints::new(quads.iter().cloned()));
    let mut a = p.add_redact_annotation(area)?;
    let [red, green, blue] = style.fill;
    // MuPDF refuses /IC on Redact (the spec allows it), so write it directly.
    a.object().dict_put("IC", doc.new_object_from_str(&format!("[{red} {green} {blue}]"))?)?;
    // Outline in the app's ink (#1A1C20), not MuPDF's default red: the UI draws marks without red.
    a.set_color(AnnotationColor::Rgb { red: 0.102, green: 0.110, blue: 0.125 })?;
    if let Some(t) = &style.overlay_text {
        a.object().dict_put("OverlayText", PdfObject::new_string(t)?)?;
    }
    if let Some(l) = label {
        a.set_contents(l)?;
    }
    a.update()?;
    // MuPDF strokes the mark's outline with a fixed red in the appearance, whatever /C says: make it ink.
    if let Some(mut n) = a.object().get_dict("AP")?.and_then(|ap| ap.get_dict("N").ok().flatten()) {
        if let Ok(ap) = String::from_utf8(n.read_stream()?) {
            n.write_stream_string(&ap.replace("1 0 0 RG", "0.102 0.11 0.125 RG"))?;
        }
    }
    Ok(a.xref()?)
}

pub fn add_area_mark(doc: &mut PdfDocument, page: i32, area: Rect, style: &MarkStyle) -> Result<i32> {
    add_mark(doc, page, &[area.quad()], style, None)
}

/// "Redact pages": one mark over the whole page and far beyond it, because content outside the
/// crop box (or the media box) is not shown but is still in the file. UIs should clip it to the page.
pub fn mark_page(doc: &mut PdfDocument, page: i32, style: &MarkStyle) -> Result<i32> {
    let b = doc.load_page(page)?.bounds()?;
    // shortcut: content placed more than 1e6 pt off the page survives; use a bbox device if that shows up.
    let m = 1.0e6;
    add_area_mark(doc, page, Rect { x0: b.x0 - m, y0: b.y0 - m, x1: b.x1 + m, y1: b.y1 + m }, style)
}

/// One mark per hit.
pub fn mark_hits(doc: &mut PdfDocument, hits: &[Hit], style: &MarkStyle) -> Result<Vec<i32>> {
    hits.iter().map(|h| add_mark(doc, h.page, &h.quads, style, Some(&h.text))).collect()
}

fn redact_annots(page: &PdfPage) -> Vec<mupdf::pdf::PdfAnnotation> {
    page.annotations().filter(|a| a.r#type().is_ok_and(|t| t == PdfAnnotationType::Redact)).collect()
}

/// The pages (zero-based) that carry a pending mark.
pub fn pages_with_marks(doc: &PdfDocument) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for p in 0..doc.page_count()? {
        if !redact_annots(&doc.load_pdf_page(p)?).is_empty() {
            out.push(p as u32);
        }
    }
    Ok(out)
}

pub fn list_marks(doc: &PdfDocument) -> Result<Vec<Mark>> {
    let mut out = Vec::new();
    for p in 0..doc.page_count()? {
        let page = doc.load_pdf_page(p)?;
        for a in redact_annots(&page) {
            let mut quads = a.quad_points()?;
            if quads.is_empty() {
                quads.push(a.rect()?.quad());
            }
            let label = a.contents()?.filter(|c| !c.is_empty()).map(str::to_owned);
            out.push(Mark { page: p, xref: a.xref()?, quads, style: style_of(&a.object())?, label });
        }
    }
    Ok(out)
}

fn style_of(a: &PdfObject) -> Result<MarkStyle> {
    let fill = match a.get_dict("IC")?.map(|c| nums(&c)).transpose()?.as_deref() {
        Some(&[g]) => [g; 3],
        Some(&[r, g, b]) => [r, g, b],
        Some(&[c, m, y, k]) => [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)],
        _ => [0.0; 3],
    };
    let overlay_text = a.get_dict("OverlayText")?.map(|o| o.as_string()).transpose()?;
    Ok(MarkStyle { fill, overlay_text })
}

pub fn remove_mark(doc: &mut PdfDocument, page: i32, xref: i32) -> Result<bool> {
    let mut p = doc.load_pdf_page(page)?;
    for a in redact_annots(&p) {
        if a.xref()? == xref {
            p.delete_annotation(a)?;
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------------------------
// Targets: what the pending marks cover. Collected before apply so verify knows what must be gone.

#[derive(Clone, Debug)]
pub struct MarkGeom {
    /// Mark boxes in page (fitz) space, for text checks.
    pub rects: Vec<Rect>,
    /// The same boxes in PDF user space, for annotation tests and the overlay.
    pub pdf_rects: Vec<Rect>,
    pub style: MarkStyle,
}

#[derive(Clone, Debug)]
pub struct PageMarks {
    pub page: i32,
    pub marks: Vec<MarkGeom>,
}

impl PageMarks {
    fn covers(&self, q: &Quad) -> bool {
        let r = Rect::from(q.clone());
        let (x, y) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
        self.marks.iter().flat_map(|m| &m.rects).any(|m| m.x0 <= x && x <= m.x1 && m.y0 <= y && y <= m.y1)
    }

    fn touches_pdf(&self, r: &Rect) -> bool {
        self.marks.iter().flat_map(|m| &m.pdf_rects).any(|m| m.x0 < r.x1 && r.x0 < m.x1 && m.y0 < r.y1 && r.y0 < m.y1)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Targets {
    pub pages: Vec<PageMarks>,
    /// Text that the marks cover (and text of annotations/fields under them). Must be gone after apply.
    pub needles: Vec<String>,
}

fn nums(o: &PdfObject) -> Result<Vec<f32>> {
    let mut v = Vec::new();
    for i in 0..o.len()? {
        v.push(o.get_array(i as i32)?.map_or(Ok(0.0), |x| x.as_float())?);
    }
    Ok(v)
}

fn bbox(xs: impl Iterator<Item = (f32, f32)>) -> Rect {
    let mut r = Rect { x0: f32::MAX, y0: f32::MAX, x1: f32::MIN, y1: f32::MIN };
    for (x, y) in xs {
        r.x0 = r.x0.min(x);
        r.y0 = r.y0.min(y);
        r.x1 = r.x1.max(x);
        r.y1 = r.y1.max(y);
    }
    r
}

/// An annotation's /Rect in PDF space.
fn annot_rect(a: &PdfObject) -> Result<Option<Rect>> {
    let Some(r) = a.get_dict("Rect")? else { return Ok(None) };
    let v = nums(&r)?;
    Ok((v.len() == 4).then(|| bbox([(v[0], v[1]), (v[2], v[3])].into_iter())))
}

fn mark_pdf_rects(a: &PdfObject) -> Result<Vec<Rect>> {
    if let Some(qp) = a.get_dict("QuadPoints")? {
        let v = nums(&qp)?;
        if v.len() >= 8 {
            return Ok(v.chunks_exact(8).map(|q| bbox(q.chunks_exact(2).map(|p| (p[0], p[1])))).collect());
        }
    }
    Ok(annot_rect(a)?.into_iter().collect())
}

fn push_needle(needles: &mut Vec<String>, s: &str) {
    let s: String = s.chars().map(norm).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    let mut add = |t: &str| {
        if !t.is_empty() && !needles.iter().any(|n| n == t) {
            needles.push(t.to_owned());
        }
    };
    add(&s);
    // Letter-spaced text ("K E R N") is quoted elsewhere without the spaces.
    let toks: Vec<&str> = s.split(' ').collect();
    if toks.len() >= 3 && toks.iter().all(|t| t.chars().count() == 1) {
        add(&toks.concat());
    }
    // Long single words too, so a name or code quoted elsewhere is caught on its own.
    for w in s.split(' ') {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric());
        if w.chars().count() >= 6 {
            add(w);
        }
    }
}

fn subtype(a: &PdfObject) -> Result<Vec<u8>> {
    Ok(a.get_dict("Subtype")?.map(|s| s.as_name()).transpose()?.unwrap_or_default())
}

fn annots_of(page: &PdfObject) -> Result<Vec<PdfObject>> {
    let Some(arr) = page.get_dict("Annots")? else { return Ok(Vec::new()) };
    let mut v = Vec::new();
    for i in 0..arr.len()? {
        if let Some(a) = arr.get_array(i as i32)?
            && a.is_dict()?
        {
            v.push(a);
        }
    }
    Ok(v)
}

/// Snapshot of every pending mark and the text under it. `fields` = also treat values of form
/// fields under marks as redacted (they get removed by apply).
pub fn collect_targets(doc: &PdfDocument, fields: bool) -> Result<Targets> {
    let mut t = Targets::default();
    for p in 0..doc.page_count()? {
        let page = doc.load_pdf_page(p)?;
        let mut marks = Vec::new();
        for a in redact_annots(&page) {
            let mut rects: Vec<Rect> = a.quad_points()?.into_iter().map(Rect::from).collect();
            if rects.is_empty() {
                rects.push(a.rect()?);
            }
            marks.push(MarkGeom { rects, pdf_rects: mark_pdf_rects(&a.object())?, style: style_of(&a.object())? });
        }
        if marks.is_empty() {
            continue;
        }
        let pm = PageMarks { page: p, marks };

        // Runs of covered chars per line.
        let pt = index_page(&page)?;
        let mut run = String::new();
        let mut last_line = None;
        for c in pt.chars.iter().filter(|c| !c.hidden) {
            if !pm.covers(&c.quad) || last_line.is_some_and(|l| l != c.line) {
                push_needle(&mut t.needles, &run);
                run.clear();
            }
            if pm.covers(&c.quad) {
                run.push(c.c);
                last_line = Some(c.line);
            } else {
                last_line = None;
            }
        }
        push_needle(&mut t.needles, &run);

        for a in annots_of(&page.object())? {
            let st = subtype(&a)?;
            if st == b"Redact" || st == b"Popup" || (st == b"Widget" && !fields) {
                continue;
            }
            if annot_rect(&a)?.is_some_and(|r| pm.touches_pdf(&r)) {
                for key in ["Contents", "V"] {
                    if let Some(s) = a.get_dict_inheritable(key)?
                        && s.is_string()?
                    {
                        push_needle(&mut t.needles, &text_of(&s)?);
                    }
                }
            }
        }
        t.pages.push(pm);
    }
    Ok(t)
}

// ---------------------------------------------------------------------------------------------
// Apply

#[derive(Clone, Debug)]
pub struct ApplyOptions {
    pub images: PdfRedactImageMethod,
    pub line_art: PdfRedactLineArtMethod,
    pub sanitize: Sanitize,
    /// Pages (zero-based) the page-level sanitize switches are limited to; `None` = every page.
    /// The marks themselves are always every pending mark of the document.
    pub pages: Option<Vec<u32>>,
    /// The document's user password, if it has one: the copy is written encrypted as the
    /// original was, and verify needs it to read the copy back.
    pub password: Option<String>,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            images: PdfRedactImageMethod::Pixels,
            line_art: PdfRedactLineArtMethod::RemoveIfCovered,
            sanitize: Sanitize::default(),
            pages: None,
            password: None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub pages: Vec<i32>,
    pub marks: usize,
    pub sanitize: SanitizeReport,
    /// Non-critical findings only (e.g. the same text still appears where it was not marked).
    pub verify: VerifyReport,
    /// The full rewrite that `verify` checked. Save exactly these bytes: any other write of `doc`
    /// (an incremental save above all, which keeps the original bytes) is unverified.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

/// The only write options allowed after a redaction: full rewrite, unreachable objects dropped.
pub fn write_options() -> PdfWriteOptions {
    let mut o = PdfWriteOptions::default();
    o.set_incremental(false).set_garbage_level(3).set_compress(true).set_compress_fonts(true).set_compress_images(true);
    o
}

pub fn save_to_vec(doc: &PdfDocument) -> Result<Vec<u8>> {
    let mut v = Vec::new();
    doc.write_to_with_options(&mut v, write_options())?;
    Ok(v)
}

/// Applies every pending mark. On `Err(Leaked)` the document is partly modified and must be
/// discarded (roll back to the last snapshot).
pub fn apply(doc: &mut PdfDocument, opts: &ApplyOptions) -> Result<ApplyReport> {
    let fields = opts.sanitize.form_fields_under_marks;
    let targets = collect_targets(doc, fields)?;
    let mut rep = SanitizeReport::default();
    let redact = PdfRedactOptions {
        black_boxes: false,
        image_method: opts.images,
        line_art: opts.line_art,
        text: PdfRedactTextMethod::Remove,
    };
    for pm in &targets.pages {
        remove_annots_under(doc, pm, fields, &mut rep)?;
        doc.load_pdf_page(pm.page)?.redact_with_options(redact)?;
        prune_resources(doc, pm.page)?;
        // A thumbnail is a picture of the unredacted page; piece info can hold the editing app's
        // copy of it (Illustrator keeps the whole artwork there). Neither shows up as text.
        let mut obj = doc.find_page(pm.page)?;
        rep.metadata += sanitize::del(&mut obj, "Thumb")? + sanitize::del(&mut obj, "PieceInfo")?;
    }

    let mut early = VerifyReport::default();
    check_page_text(doc, &targets, true, &mut early, &mut HashSet::new())?;
    for pm in &targets.pages {
        if let Some(res) = doc.find_page(pm.page)?.get_dict_inheritable("Resources")?
            && uncut_image(&res, 0)?
        {
            let place = format!("page {}: image in a soft mask or tiling pattern (MuPDF does not cut those)", pm.page + 1);
            early.leaks.push(Leak { needle: String::new(), place, critical: true });
        }
    }
    if !early.is_clean() {
        return Err(RedactError::Leaked(early.leaks));
    }
    for pm in &targets.pages {
        draw_overlays(doc, pm)?;
    }

    let mut sanitize = sanitize::sanitize_pages(doc, &opts.sanitize, &targets.needles, opts.pages.as_deref())?;
    sanitize.annotations += rep.annotations;
    sanitize.form_fields += rep.form_fields;
    sanitize.metadata += rep.metadata;

    let bytes = save_to_vec(doc)?;
    let verify = verify_at(&bytes, &targets, 0, &std::cell::Cell::new(SCAN_BUDGET), opts.password.as_deref())?;
    if !verify.is_clean() {
        return Err(RedactError::Leaked(verify.leaks));
    }
    Ok(ApplyReport {
        pages: targets.pages.iter().map(|p| p.page).collect(),
        marks: targets.pages.iter().map(|p| p.marks.len()).sum(),
        sanitize,
        verify,
        bytes,
    })
}

/// Deletes annotations whose box touches a mark (MuPDF only handles links and free text), plus
/// their popups. Widgets too when `fields`, together with their form field.
fn remove_annots_under(doc: &mut PdfDocument, pm: &PageMarks, fields: bool, rep: &mut SanitizeReport) -> Result<()> {
    let mut page = doc.find_page(pm.page)?;
    let mut widgets = Vec::new();
    let n = sanitize::drop_annots(&mut page, |a, st| {
        if st == b"Redact" || st == b"Link" || (st == b"Widget" && !fields) {
            return Ok(false);
        }
        let hit = annot_rect(a)?.is_some_and(|r| pm.touches_pdf(&r));
        if hit && st == b"Widget" {
            widgets.push(a.clone());
        }
        Ok(hit)
    })?;
    rep.form_fields += widgets.len() as u32;
    rep.annotations += n - widgets.len() as u32;
    for w in widgets {
        sanitize::remove_field(doc, &w)?;
    }
    Ok(())
}

/// Drops page resources the redacted content no longer names: a fully covered pattern or form
/// is no longer drawn but would still carry its text into the file. Works on a copy of the
/// resources because they may be shared with other pages.
pub(super) fn prune_resources(doc: &mut PdfDocument, page: i32) -> Result<()> {
    let mut obj = doc.find_page(page)?;
    let Some(res) = obj.get_dict_inheritable("Resources")? else { return Ok(()) };
    let mut data = Vec::new();
    if let Some(c) = obj.get_dict("Contents")? {
        if c.is_array()? {
            for i in 0..c.len()? as i32 {
                if let Some(s) = c.get_array(i)? {
                    data.extend(s.read_stream()?);
                    data.push(b'\n');
                }
            }
        } else {
            data = c.read_stream()?;
        }
    }
    let used = sanitize::names_in(&data);
    let mut res = res.copy_dict()?;
    for cat in ["XObject", "Pattern", "Shading", "Font", "ExtGState", "Properties", "ColorSpace"] {
        let Some(sub) = res.get_dict(cat)? else { continue };
        let mut sub = sub.copy_dict()?;
        for i in (0..sub.dict_len()? as i32).rev() {
            if let Some(k) = sub.get_dict_key(i)?
                && !used.contains(&k.as_name()?)
            {
                sub.dict_delete(k)?;
            }
        }
        res.dict_put(cat, sub)?;
    }
    obj.dict_put("Resources", doc.add_object(&res)?)?;
    Ok(())
}

/// Whether `res` (after redaction and pruning) still draws an image through a soft-mask group or
/// a tiling pattern, directly or in a form XObject. MuPDF cuts neither, so the pixels under a
/// mark would survive while the page text check sees nothing.
// shortcut: also refuses when that mask or pattern lies away from the marks; compare its bbox with the marks if this blocks real files.
fn uncut_image(res: &PdfObject, depth: u32) -> Result<bool> {
    let vals = |cat: &str| -> Result<Vec<PdfObject>> {
        let Some(d) = res.get_dict(cat)? else { return Ok(Vec::new()) };
        (0..d.dict_len()? as i32).filter_map(|i| d.get_dict_val(i).transpose()).map(|r| Ok(r?)).collect()
    };
    let has_image = |o: &PdfObject| -> Result<bool> {
        let Some(r) = o.get_dict("Resources")? else { return Ok(false) };
        images_in(&r, 0)
    };
    if depth > 8 {
        return Ok(true); // not walked = not known to be safe
    }
    for p in vals("Pattern")? {
        if has_image(&p)? {
            return Ok(true);
        }
    }
    for gs in vals("ExtGState")? {
        if let Some(sm) = gs.get_dict("SMask")?
            && sm.is_dict()?
            && let Some(g) = sm.get_dict("G")?
            && has_image(&g)?
        {
            return Ok(true);
        }
    }
    for x in vals("XObject")? {
        if !sanitize::is_image(&x)?
            && let Some(r) = x.get_dict("Resources")?
            && uncut_image(&r, depth + 1)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `res` draws any image XObject, directly or through nested form XObjects.
fn images_in(res: &PdfObject, depth: u32) -> Result<bool> {
    let Some(xo) = res.get_dict("XObject")? else { return Ok(false) };
    for i in 0..xo.dict_len()? as i32 {
        let Some(x) = xo.get_dict_val(i)? else { continue };
        if sanitize::is_image(&x)? {
            return Ok(true);
        }
        if let Some(r) = x.get_dict("Resources")?
            && (depth >= 8 || images_in(&r, depth + 1)?)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn draw_overlays(doc: &mut PdfDocument, pm: &PageMarks) -> Result<()> {
    let mut s = String::new();
    let mut font = false;
    for m in &pm.marks {
        let [r, g, b] = m.style.fill;
        for q in &m.pdf_rects {
            let (w, h) = (q.x1 - q.x0, q.y1 - q.y0);
            writeln!(s, "q {r:.3} {g:.3} {b:.3} rg {:.2} {:.2} {w:.2} {h:.2} re f Q", q.x0, q.y0).unwrap();
            let Some(text) = m.style.overlay_text.as_deref().filter(|t| !t.is_empty()) else { continue };
            font = true;
            // ponytail: Helvetica average advance (0.55 em) for fitting and centering; measure glyph widths if codes get misaligned.
            let n = text.chars().count() as f32;
            let size = (h * 0.8).min(w / (0.55 * n)).max(2.0);
            let (x, y) = (q.x0 + (w - 0.55 * size * n) / 2.0, q.y0 + (h - 0.7 * size) / 2.0);
            let ink = if 0.299 * r + 0.587 * g + 0.114 * b < 0.5 { 1.0 } else { 0.0 };
            let hex: String = text
                .chars()
                .map(|c| if (' '..='~').contains(&c) || ('\u{a0}'..='\u{ff}').contains(&c) { c as u32 } else { '?' as u32 })
                .map(|c| format!("{c:02X}"))
                .collect();
            writeln!(s, "q BT {ink} g /InkwellRedact {size:.2} Tf {x:.2} {y:.2} Td <{hex}> Tj ET Q").unwrap();
        }
    }
    let page = doc.load_pdf_page(pm.page)?;
    if font {
        let mut res = page.resources()?;
        let mut fonts = match res.get_dict("Font")? {
            Some(f) => f,
            None => {
                let f = doc.new_dict()?;
                res.dict_put("Font", f)?;
                res.get_dict("Font")?.unwrap()
            }
        };
        let f = doc.new_object_from_str("<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>")?;
        fonts.dict_put("InkwellRedact", doc.add_object(&f)?)?;
    }
    let stm = doc.add_stream(&Buffer::from_bytes(s.as_bytes())?, None, false)?;
    let mut obj = page.object();
    match obj.get_dict("Contents")? {
        Some(mut c) if c.is_array()? => c.array_push(stm)?,
        Some(c) => {
            let mut arr = doc.new_array()?;
            arr.array_push(c)?;
            arr.array_push(stm)?;
            obj.dict_put("Contents", arr)?;
        }
        None => obj.dict_put("Contents", stm)?,
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Verify

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leak {
    pub needle: String,
    pub place: String,
    /// False when the text still shows on a page where it was never marked: then copies in other
    /// places are expected too. Critical = recoverable redacted content.
    pub critical: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub leaks: Vec<Leak>,
}

impl VerifyReport {
    pub fn is_clean(&self) -> bool {
        !self.leaks.iter().any(|l| l.critical)
    }
}

fn needle_regex(n: &str) -> String {
    format!("(?i){}", n.split_whitespace().map(regex::escape).collect::<Vec<_>>().join(r"\s+"))
}

/// Page text: any char left under a mark (`geometric`, in-memory only: the overlay text sits there
/// afterwards) and every needle occurrence, critical when under a mark. Fills `visible` with
/// needles that still show elsewhere.
fn check_page_text(
    doc: &Document,
    t: &Targets,
    geometric: bool,
    rep: &mut VerifyReport,
    visible: &mut HashSet<String>,
) -> Result<()> {
    let res: Vec<Regex> = t.needles.iter().map(|n| Regex::new(&needle_regex(n))).collect::<std::result::Result<_, _>>()?;
    for p in 0..doc.page_count()? {
        let pt = index_page(&doc.load_page(p)?)?;
        let pm = t.pages.iter().find(|m| m.page == p);
        if geometric && let Some(pm) = pm {
            let under: String = pt.chars.iter().filter(|c| !c.hidden && pm.covers(&c.quad)).map(|c| c.c).collect();
            if !under.trim().is_empty() {
                rep.leaks.push(Leak { needle: under, place: format!("page {}: text under a mark", p + 1), critical: true });
            }
        }
        for (n, re) in t.needles.iter().zip(&res) {
            for m in re.find_iter(&pt.text) {
                let marked = pm.is_some_and(|pm| pt.quads(&m.range()).iter().any(|q| pm.covers(q)));
                if !marked {
                    visible.insert(n.clone());
                }
                rep.leaks.push(Leak {
                    needle: n.clone(),
                    place: format!("page {}: {}", p + 1, if marked { "text under a mark" } else { "unmarked text" }),
                    critical: marked,
                });
            }
        }
    }
    Ok(())
}

/// Reopens a written copy and searches it everywhere: page text, every object's strings, every
/// decoded non-image stream, and the raw file bytes.
pub fn verify_bytes(bytes: &[u8], t: &Targets) -> Result<VerifyReport> {
    verify_at(bytes, t, 0, &std::cell::Cell::new(SCAN_BUDGET), None)
}

/// Bytes verify unpacks from attachments in total (all gzip, zip and nested PDF streams together):
/// past it an attachment counts as unread, so a pile of small bombs cannot exhaust memory.
const SCAN_BUDGET: u64 = 512 << 20;

/// `verify_bytes` at attachment nesting `depth` (a PDF attached to a PDF...). `left` is what is
/// left of `SCAN_BUDGET`. An encrypted copy is read with `password`; without the right one it is
/// `NeedsPassword`, never clean.
fn verify_at(bytes: &[u8], t: &Targets, depth: u32, left: &std::cell::Cell<u64>, password: Option<&str>) -> Result<VerifyReport> {
    let mut rep = VerifyReport::default();
    if t.needles.is_empty() {
        return Ok(rep);
    }
    let mut doc = PdfDocument::from_bytes(bytes)?;
    if doc.needs_password()? && !password.is_some_and(|pw| doc.authenticate(pw).unwrap_or(false)) {
        return Err(RedactError::NeedsPassword);
    }
    let mut visible = HashSet::new();
    check_page_text(&doc, t, false, &mut rep, &mut visible)?;

    // Short needles only in decoded text: in binary data they would match by chance.
    let set = |min: usize| -> Result<(regex::bytes::RegexSet, Vec<&String>)> {
        let ns: Vec<&String> = t.needles.iter().filter(|n| n.chars().count() >= min).collect();
        Ok((regex::bytes::RegexSet::new(ns.iter().map(|n| needle_regex(n)))?, ns))
    };
    let (strings, sn) = set(3)?;
    let (streams, tn) = set(5)?;
    let (raw, rn) = set(8)?;
    // (needle, place, critical even if the needle is still visible elsewhere on a page)
    let mut hits: Vec<(&String, String, bool)> = Vec::new();

    // Attachments by reference from a file specification's /EF, not only by their optional /Type.
    let mut attached = HashSet::new();
    for i in 1..doc.xref_len()? as i32 {
        if let Ok(Some(o)) = doc.xref_object(i)
            && o.is_dict()?
            && let Some(ef) = o.get_dict("EF")?
            && ef.is_dict()?
        {
            for k in 0..ef.dict_len()? as i32 {
                if let Some(v) = ef.get_dict_val(k)?
                    && v.is_indirect()?
                {
                    attached.insert(v.as_indirect()?);
                }
            }
        }
    }

    for i in 1..doc.xref_len()? as i32 {
        // Anything verify cannot read counts as a leak: it was not checked.
        let unread = |what: &str| Leak { needle: String::new(), place: format!("object {i}: {what}"), critical: true };
        let obj = match doc.xref_object(i) {
            Ok(Some(o)) => o,
            Ok(None) => continue,
            Err(_) => {
                rep.leaks.push(unread("unreadable"));
                continue;
            }
        };
        let (mut found, mut found_names) = (HashSet::new(), HashSet::new());
        scan_strings(&obj, 0, &mut |s, name| {
            // Names (e.g. /Dests keys, resource names) only for needles of 5+ chars: shorter ones
            // match standard names like /Font or /Page by chance.
            if name {
                found_names.extend(streams.matches(s.as_bytes()).iter());
            } else {
                found.extend(strings.matches(s.as_bytes()).iter());
            }
        })?;
        for k in found {
            hits.push((sn[k], format!("object {i}: string"), true));
        }
        for k in found_names {
            hits.push((tn[k], format!("object {i}: name"), false));
        }
        // `is_stream` needs the reference: a resolved dict has lost its object number.
        let r = doc.new_indirect(i, 0)?;
        if r.is_stream()? && !sanitize::is_image(&r)? {
            let Ok(data) = doc.xref_stream(i) else {
                rep.leaks.push(unread("stream that does not decode"));
                continue;
            };
            // Only page content can explain a hit by an unmarked occurrence; attachments and XMP cannot.
            let ty = r.get_dict("Type")?.map(|t| t.as_name()).transpose()?;
            if depth > 0 {
                let n = data.len() as u64;
                if n > left.get() {
                    rep.leaks.push(unread("over the attachment scan budget"));
                    continue;
                }
                left.set(left.get() - n);
            }
            if ty.as_deref() == Some(b"EmbeddedFile") || attached.contains(&i) {
                scan_attachment(&data, t, (&streams, &tn), (depth, left), &format!("object {i}: attachment"), &mut rep.leaks)?;
                continue;
            }
            let strict = ty.as_deref() == Some(b"Metadata");
            for k in streams.matches(&data).iter() {
                hits.push((tn[k], format!("object {i}: stream"), strict));
            }
        }
    }
    for k in raw.matches(bytes).iter() {
        hits.push((rn[k], "raw file bytes".into(), false));
    }
    for (n, place, strict) in hits {
        rep.leaks.push(Leak { needle: n.clone(), place, critical: strict || !visible.contains(n) });
    }
    Ok(rep)
}

/// Searches an attachment's content, every hit critical. Unpacks what it can (gzip, zip, an
/// attached PDF, UTF-16 text); anything it cannot read counts as a leak, because it was not checked.
fn scan_attachment(
    data: &[u8],
    t: &Targets,
    (set, ns): (&regex::bytes::RegexSet, &[&String]),
    (depth, left): (u32, &std::cell::Cell<u64>),
    place: &str,
    leaks: &mut Vec<Leak>,
) -> Result<()> {
    use std::io::Read;
    let unread = |why: &str| Leak { needle: String::new(), place: format!("{place}: {why}"), critical: true };
    let inflate = |r: &mut dyn Read| -> Option<Vec<u8>> {
        let mut out = Vec::new();
        r.take(left.get() + 1).read_to_end(&mut out).ok()?;
        left.set(left.get().checked_sub(out.len() as u64)?);
        Some(out)
    };
    if depth > 4 {
        leaks.push(unread("nested too deep to scan"));
    } else if data.starts_with(b"%PDF") {
        match PdfDocument::from_bytes(data) {
            Err(_) => leaks.push(unread("attached PDF does not open")),
            // Without the password its strings and streams stay ciphertext and nothing matches.
            Ok(d) if d.needs_password()? => leaks.push(unread("attached PDF needs a password")),
            Ok(_) => {
                let sub = Targets { pages: Vec::new(), needles: t.needles.clone() };
                for l in verify_at(data, &sub, depth + 1, left, None)?.leaks {
                    leaks.push(Leak { needle: l.needle, place: format!("{place}: attached PDF: {}", l.place), critical: true });
                }
            }
        }
    } else if data.starts_with(&[0x1f, 0x8b]) {
        // Every member: a plain GzDecoder stops after the first, `gunzip` does not.
        match inflate(&mut flate2::read::MultiGzDecoder::new(data)) {
            Some(d) => scan_attachment(&d, t, (set, ns), (depth + 1, left), &format!("{place}: gzip"), leaks)?,
            None => leaks.push(unread("gzip data that does not decode or is over the scan budget")),
        }
    } else if data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06") {
        // The raw bytes too: stored entries, comments, data outside the central directory.
        for k in set.matches(data).iter() {
            leaks.push(Leak { needle: ns[k].clone(), place: format!("{place}: zip bytes"), critical: true });
        }
        match zip_entries(data, &inflate) {
            Some(entries) => {
                for (name, d) in entries {
                    scan_attachment(&d, t, (set, ns), (depth + 1, left), &format!("{place}: zip entry {name}"), leaks)?;
                }
            }
            None => leaks.push(unread("zip archive verify cannot read (encrypted, zip64, unknown method or over the scan budget)")),
        }
    } else {
        let text: std::borrow::Cow<[u8]> = match data {
            [0xff, 0xfe, rest @ ..] | [0xfe, 0xff, rest @ ..] => {
                let le = data[0] == 0xff;
                let units = rest.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
                String::from_utf16_lossy(&units.collect::<Vec<_>>()).into_bytes().into()
            }
            // Text: valid UTF-8 without control bytes other than whitespace. Single-byte encodings
            // (EBCDIC, Latin-1 accents) fail closed: their letters are not where the regex looks.
            _ if std::str::from_utf8(data).is_ok() && !data.iter().any(|&b| b < 0x09 || (0x0e..0x20).contains(&b)) => data.into(),
            _ => {
                leaks.push(unread("binary attachment verify cannot read (sanitize embedded files to remove it)"));
                return Ok(());
            }
        };
        for k in set.matches(&text).iter() {
            leaks.push(Leak { needle: ns[k].clone(), place: place.to_string(), critical: true });
        }
    }
    Ok(())
}

/// Entries of a zip archive (stored or deflated), from its central directory, plus local entries
/// the directory does not list (readers that walk local headers extract them). None when it uses
/// anything else (encryption, zip64, other methods), the directory and a local header disagree on
/// sizes, or it is malformed: a size that hides part of the data must not pass.
fn zip_entries(z: &[u8], inflate: &dyn Fn(&mut dyn std::io::Read) -> Option<Vec<u8>>) -> Option<Vec<(String, Vec<u8>)>> {
    let u16_at = |p: usize| Some(u16::from_le_bytes(z.get(p..p + 2)?.try_into().ok()?) as usize);
    let u32_at = |p: usize| Some(u32::from_le_bytes(z.get(p..p + 4)?.try_into().ok()?) as usize);
    let eocd = (0..z.len().saturating_sub(21)).rev().take(65_557).find(|&p| z[p..].starts_with(b"PK\x05\x06"))?;
    let (count, mut p) = (u16_at(eocd + 10)?, u32_at(eocd + 16)?);
    if count == 0xffff || p == 0xffff_ffff {
        return None;
    }
    // Local entry at `lho` with `size` bytes of `method` data: (data start, decoded data).
    let entry = |lho: usize, method: usize, size: usize, usize_: usize| -> Option<(usize, Vec<u8>)> {
        let start = lho + 30 + u16_at(lho + 26)? + u16_at(lho + 28)?;
        let raw = z.get(start..start.checked_add(size)?)?;
        let data = match method {
            0 => raw.to_vec(),
            8 => {
                // All of the entry's bytes must be deflate data: bytes past the end of the stream
                // could hold another local entry that the unlisted walk skips as inside this one.
                let mut d = flate2::read::DeflateDecoder::new(raw);
                let data = inflate(&mut d)?;
                (d.total_in() == size as u64).then_some(data)?
            }
            _ => return None,
        };
        (data.len() == usize_).then_some((start + size, data))
    };
    let (mut out, mut listed) = (Vec::new(), Vec::new());
    for _ in 0..count {
        if !z.get(p..)?.starts_with(b"PK\x01\x02") || u16_at(p + 8)? & 1 != 0 {
            return None;
        }
        let (flags, method, size, usize_, lho) = (u16_at(p + 8)?, u16_at(p + 10)?, u32_at(p + 20)?, u32_at(p + 24)?, u32_at(p + 42)?);
        let (nlen, xlen, clen) = (u16_at(p + 28)?, u16_at(p + 30)?, u16_at(p + 32)?);
        let name = String::from_utf8_lossy(z.get(p + 46..p + 46 + nlen)?).into_owned();
        if !z.get(lho..)?.starts_with(b"PK\x03\x04") || u16_at(lho + 8)? != method || (method == 0 && size != usize_) {
            return None;
        }
        // Without a data descriptor (flag 3) the local header carries the sizes too: they must agree.
        if flags & 8 == 0 && (u32_at(lho + 18)? != size || u32_at(lho + 22)? != usize_) {
            return None;
        }
        let (end, data) = entry(lho, method, size, usize_)?;
        listed.push(lho..end);
        out.push((name, data));
        p += 46 + nlen + xlen + clen;
    }
    // Local headers outside the listed entries' data.
    // Sorted for a binary search: a scan per header is entries x headers. With overlapping entries
    // a header may count as unlisted wrongly; it then gets parsed (or fails closed), never skipped.
    listed.sort_by_key(|r| r.start);
    let inside = |q: usize| listed.partition_point(|r| r.start <= q).checked_sub(1).is_some_and(|i| listed[i].contains(&q));
    let unlisted = (0..z.len()).filter(|&q| z[q..].starts_with(b"PK\x03\x04") && !inside(q));
    for lho in unlisted.collect::<Vec<_>>() {
        let (flags, method) = (u16_at(lho + 6)?, u16_at(lho + 8)?);
        if flags & 1 != 0 {
            return None;
        }
        let data = match method {
            // Sizes in a data descriptor after the data are unknown here: deflate ends by itself,
            // stored data does not.
            0 if flags & 8 == 0 => entry(lho, 0, u32_at(lho + 18)?, u32_at(lho + 22)?)?.1,
            8 => {
                let start = lho + 30 + u16_at(lho + 26)? + u16_at(lho + 28)?;
                inflate(&mut flate2::read::DeflateDecoder::new(z.get(start..)?))?
            }
            _ => return None,
        };
        out.push(("(not in the directory)".into(), data));
    }
    Some(out)
}

/// A PDF string as text. MuPDF writes a NUL as C0 80, which `as_string` rejects, so binary strings
/// (checksums, signature /Contents) fall back to their bytes as Latin-1.
// shortcut: a UTF-16 string holding a NUL char is then read byte-wise; decode UTF-16 here if that shows up.
pub(super) fn text_of(o: &PdfObject) -> Result<String> {
    match o.as_string() {
        Err(mupdf::Error::InvalidUtf8) => Ok(o.as_bytes()?.iter().map(|&b| b as char).collect()),
        r => Ok(r?),
    }
}

/// Calls `f(text, is_name)` for every string and every name (values and dict keys) in `o`,
/// not following references.
fn scan_strings(o: &PdfObject, depth: u32, f: &mut dyn FnMut(&str, bool)) -> Result<()> {
    if depth > 0 && o.is_indirect()? {
        return Ok(()); // scanned on its own
    }
    if depth > 32 {
        let place = "an object nested deeper than 32 levels (not scanned)".to_string();
        return Err(RedactError::Leaked(vec![Leak { needle: String::new(), place, critical: true }]));
    }
    let name = |n: &PdfObject| -> Result<String> { Ok(String::from_utf8_lossy(&n.as_name()?).into_owned()) };
    if o.is_string()? {
        f(&text_of(o)?, false);
    } else if o.is_name()? {
        f(&name(o)?, true);
    } else if o.is_array()? {
        for i in 0..o.len()? {
            if let Some(x) = o.get_array(i as i32)? {
                scan_strings(&x, depth + 1, f)?;
            }
        }
    } else if o.is_dict()? {
        for i in 0..o.dict_len()? {
            if let Some(k) = o.get_dict_key(i as i32)? {
                f(&name(&k)?, true);
            }
            if let Some(x) = o.get_dict_val(i as i32)? {
                scan_strings(&x, depth + 1, f)?;
            }
        }
    }
    Ok(())
}
