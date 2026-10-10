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

mod patterns;
mod sanitize;
#[cfg(test)]
mod tests;

pub use patterns::Pattern;
pub use sanitize::{Sanitize, SanitizeReport, sanitize};

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

pub fn find(doc: &Document, query: &Query) -> Result<Vec<Hit>> {
    let re = match query {
        Query::Text { needle, case_sensitive, whole_word } => literal_regex(needle, *case_sensitive, *whole_word)?,
        Query::Regex { pattern, case_sensitive } => {
            Some(RegexBuilder::new(pattern).case_insensitive(!case_sensitive).build()?)
        }
        Query::Pattern(_) => None,
    };
    if re.is_none() && !matches!(query, Query::Pattern(_)) {
        return Ok(Vec::new());
    }
    let mut hits = Vec::new();
    for p in 0..doc.page_count()? {
        let pt = index_page(&doc.load_page(p)?)?;
        let ranges = match (query, &re) {
            (Query::Pattern(pat), _) => pat.find(&pt.text),
            (_, Some(re)) => re.find_iter(&pt.text).filter(|m| !m.is_empty()).map(|m| m.range()).collect(),
            _ => Vec::new(),
        };
        for r in ranges {
            let quads = pt.quads(&r);
            if !quads.is_empty() {
                hits.push(Hit { page: p, text: pt.text[r].to_owned(), quads });
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
}

/// Adds one /Redact annotation covering `quads` (page space). Returns its object number.
pub fn add_mark(doc: &mut PdfDocument, page: i32, quads: &[Quad], style: &MarkStyle) -> Result<i32> {
    let mut p = doc.load_pdf_page(page)?;
    let area = AnnotationArea::QuadPoints(AnnotationQuadPoints::new(quads.iter().cloned()));
    let mut a = p.add_redact_annotation(area)?;
    let [red, green, blue] = style.fill;
    // MuPDF refuses /IC on Redact (the spec allows it), so write it directly.
    a.object().dict_put("IC", doc.new_object_from_str(&format!("[{red} {green} {blue}]"))?)?;
    a.set_color(AnnotationColor::Rgb { red: 1.0, green: 0.0, blue: 0.0 })?;
    if let Some(t) = &style.overlay_text {
        a.object().dict_put("OverlayText", PdfObject::new_string(t)?)?;
    }
    a.update()?;
    Ok(a.xref()?)
}

pub fn add_area_mark(doc: &mut PdfDocument, page: i32, area: Rect, style: &MarkStyle) -> Result<i32> {
    add_mark(doc, page, &[area.quad()], style)
}

/// "Redact pages": one mark over the whole page.
pub fn mark_page(doc: &mut PdfDocument, page: i32, style: &MarkStyle) -> Result<i32> {
    let bounds = doc.load_page(page)?.bounds()?;
    add_area_mark(doc, page, bounds, style)
}

/// One mark per hit.
pub fn mark_hits(doc: &mut PdfDocument, hits: &[Hit], style: &MarkStyle) -> Result<Vec<i32>> {
    hits.iter().map(|h| add_mark(doc, h.page, &h.quads, style)).collect()
}

fn redact_annots(page: &PdfPage) -> Vec<mupdf::pdf::PdfAnnotation> {
    page.annotations().filter(|a| a.r#type().is_ok_and(|t| t == PdfAnnotationType::Redact)).collect()
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
            out.push(Mark { page: p, xref: a.xref()?, quads, style: style_of(&a.object())? });
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
                        push_needle(&mut t.needles, &s.as_string()?);
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
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            images: PdfRedactImageMethod::Pixels,
            line_art: PdfRedactLineArtMethod::RemoveIfCovered,
            sanitize: Sanitize::default(),
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
    }

    let mut early = VerifyReport::default();
    check_page_text(doc, &targets, true, &mut early, &mut HashSet::new())?;
    if !early.is_clean() {
        return Err(RedactError::Leaked(early.leaks));
    }
    for pm in &targets.pages {
        draw_overlays(doc, pm)?;
    }

    let mut sanitize = sanitize::sanitize(doc, &opts.sanitize, &targets.needles)?;
    sanitize.annotations += rep.annotations;
    sanitize.form_fields += rep.form_fields;

    let verify = verify_bytes(&save_to_vec(doc)?, &targets)?;
    if !verify.is_clean() {
        return Err(RedactError::Leaked(verify.leaks));
    }
    Ok(ApplyReport {
        pages: targets.pages.iter().map(|p| p.page).collect(),
        marks: targets.pages.iter().map(|p| p.marks.len()).sum(),
        sanitize,
        verify,
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
    let mut rep = VerifyReport::default();
    if t.needles.is_empty() {
        return Ok(rep);
    }
    let doc = PdfDocument::from_bytes(bytes)?;
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

    for i in 1..doc.xref_len()? as i32 {
        let Ok(Some(obj)) = doc.xref_object(i) else { continue };
        let mut found = HashSet::new();
        scan_strings(&obj, 0, &mut |s| found.extend(strings.matches(s.as_bytes()).iter()))?;
        for k in found {
            hits.push((sn[k], format!("object {i}: string"), true));
        }
        // `is_stream` needs the reference: a resolved dict has lost its object number.
        let r = doc.new_indirect(i, 0)?;
        if r.is_stream()? && !sanitize::is_image(&r)?
            && let Ok(data) = doc.xref_stream(i)
        {
            // Only page content can explain a hit by an unmarked occurrence; attachments and XMP cannot.
            let ty = r.get_dict("Type")?.map(|t| t.as_name()).transpose()?;
            let strict = matches!(ty.as_deref(), Some(b"EmbeddedFile" | b"Metadata"));
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

fn scan_strings(o: &PdfObject, depth: u32, f: &mut dyn FnMut(&str)) -> Result<()> {
    if depth > 32 || (depth > 0 && o.is_indirect()?) {
        return Ok(());
    }
    if o.is_string()? {
        f(&o.as_string()?);
    } else if o.is_array()? {
        for i in 0..o.len()? {
            if let Some(x) = o.get_array(i as i32)? {
                scan_strings(&x, depth + 1, f)?;
            }
        }
    } else if o.is_dict()? {
        for i in 0..o.dict_len()? {
            if let Some(x) = o.get_dict_val(i as i32)? {
                scan_strings(&x, depth + 1, f)?;
            }
        }
    }
    Ok(())
}
