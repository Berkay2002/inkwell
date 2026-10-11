//! A comparison report as a PDF: the summary (counts by kind), every change grouped by page, and for
//! the changed pages the two renders side by side with the changed regions outlined.
//!
//! Built from a [`Comparison`] and the two files' bytes. Text is set in MuPDF's built-in Helvetica (every
//! Unicode character it has, as a CID font, the way `convert::create` writes text pages); page images are
//! the same cached renders the compare view uses.

use super::types::*;
use super::visual::{BytesId, render_page};
use mupdf::pdf::page::{InsertImageOptions, PageImageSource};
use mupdf::pdf::{PdfDocument, PdfObject, PdfWriteOptions};
use mupdf::size::Size;
use mupdf::{Font, Image, Rect};
use std::sync::atomic::{AtomicBool, Ordering};

const PAGE_W: f32 = 595.0;
const PAGE_H: f32 = 842.0;
const MARGIN: f32 = 48.0;
/// Changed pages shown as pictures; the list above them covers every change.
pub const MAX_PICTURE_PAGES: usize = 60;
/// Longest text shown for one side of a text change.
const CLIP_CHARS: usize = 600;

type Rgb = (f32, f32, f32);
const INK: Rgb = (0.10, 0.11, 0.125);
const DIM: Rgb = (0.42, 0.40, 0.37);
const RULE: Rgb = (0.89, 0.87, 0.83);
const ADD: Rgb = (0.12, 0.50, 0.25);
const DEL: Rgb = (0.75, 0.18, 0.15);
const CHG: Rgb = (0.70, 0.45, 0.05);

fn tone_of(c: &Change) -> (Rgb, &'static str) {
    use ChangeKind::*;
    match c.kind {
        PageAdded | AnnotationAdded | OcrLayerAdded => (ADD, "+"),
        PageRemoved | AnnotationRemoved | RedactionApplied => (DEL, "-"),
        TextChanged => {
            let runs = match &c.detail {
                Detail::Text { runs } => runs.as_slice(),
                _ => &[],
            };
            let ins = runs.iter().any(|r| r.op == RunOp::Insert);
            let del = runs.iter().any(|r| r.op == RunOp::Delete);
            match (ins, del) {
                (true, false) => (ADD, "+"),
                (false, true) => (DEL, "-"),
                _ => (CHG, "~"),
            }
        }
        _ => (CHG, "~"),
    }
}

/// The groups the summary counts in (the same as the filter in the compare view).
pub fn category(k: ChangeKind) -> &'static str {
    use ChangeKind::*;
    match k {
        PageAdded | PageRemoved | PageReplaced | PageMoved | PageRotated | PageBoxChanged => "Pages",
        TextChanged | OcrLayerAdded | RedactionApplied => "Text",
        FormatChanged => "Formatting",
        ImageReplaced | PageContentChanged => "Images and graphics",
        AnnotationAdded | AnnotationRemoved | AnnotationChanged => "Annotations",
        FormFieldChanged => "Form fields",
        MetadataChanged | BookmarksChanged | AttachmentsChanged => "Document",
    }
}

const CATEGORY_ORDER: &[&str] = &["Text", "Formatting", "Images and graphics", "Pages", "Annotations", "Form fields", "Document"];

/// Counts per category, in the order the report and the view list them (empty ones left out).
pub fn counts(changes: &[Change]) -> Vec<(&'static str, u32)> {
    CATEGORY_ORDER
        .iter()
        .map(|&c| (c, changes.iter().filter(|ch| category(ch.kind) == c).count() as u32))
        .filter(|(_, n)| *n > 0)
        .collect()
}

pub struct ReportInput<'a> {
    pub a: &'a [u8],
    pub b: &'a [u8],
    /// File names or version titles, as the view shows them.
    pub a_name: &'a str,
    pub b_name: &'a str,
    /// Already formatted ("11 Oct 2026, 14:32").
    pub date: &'a str,
}

// ---- drawing ----------------------------------------------------------------------------------------

/// Helvetica as an embedded CID font, as in `convert::create`.
struct Face {
    font: Font,
}

impl Face {
    fn gid(&self, c: char) -> u16 {
        let g = self.font.encode_character(c as i32).unwrap_or(0);
        if g > 0 { g as u16 } else { self.font.encode_character('?' as i32).unwrap_or(0).max(0) as u16 }
    }

    fn width(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.font.advance_glyph(self.gid(c) as i32).unwrap_or(0.5)).sum::<f32>() * size
    }

    fn hex(&self, s: &str) -> String {
        s.chars().filter(|c| !c.is_control()).map(|c| format!("{:04x}", self.gid(c))).collect()
    }

    /// Greedy line breaking within `width`; words longer than a line are broken by character.
    fn wrap(&self, text: &str, size: f32, width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split(' ').filter(|w| !w.is_empty()) {
                let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
                if self.width(&candidate, size) <= width {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                for ch in word.chars() {
                    if !line.is_empty() && self.width(&format!("{line}{ch}"), size) > width {
                        lines.push(std::mem::take(&mut line));
                    }
                    line.push(ch);
                }
            }
            lines.push(line);
        }
        lines
    }
}

struct Sheet {
    ops: String,
    images: Vec<(Rect, Vec<u8>)>,
}

struct Writer {
    pdf: PdfDocument,
    reg: Face,
    bold: Face,
    f1: PdfObject,
    f2: PdfObject,
    sheet: Option<Sheet>,
    /// Pen position: the top of the next thing, PDF coordinates (up).
    y: f32,
    pages: u32,
}

impl Writer {
    fn new() -> Result<Writer> {
        let mut pdf = PdfDocument::new();
        let (reg, bold) = (Face { font: Font::new("Helvetica")? }, Face { font: Font::new("Helvetica-Bold")? });
        let f1 = pdf.add_font(&reg.font)?;
        let f2 = pdf.add_font(&bold.font)?;
        Ok(Writer { pdf, reg, bold, f1, f2, sheet: None, y: 0.0, pages: 0 })
    }

    fn rgb(c: Rgb) -> String {
        format!("{:.3} {:.3} {:.3}", c.0, c.1, c.2)
    }

    fn page(&mut self) -> Result<()> {
        self.flush()?;
        self.sheet = Some(Sheet { ops: String::new(), images: Vec::new() });
        self.y = PAGE_H - MARGIN;
        Ok(())
    }

    /// Adds the page being drawn to the document.
    fn flush(&mut self) -> Result<()> {
        let Some(sheet) = self.sheet.take() else { return Ok(()) };
        let mut page = self.pdf.new_page(Size::new(PAGE_W, PAGE_H))?;
        let mut fonts = self.pdf.new_dict()?;
        fonts.dict_put("F1", self.f1.clone())?;
        fonts.dict_put("F2", self.f2.clone())?;
        let mut res = self.pdf.new_dict()?;
        res.dict_put("Font", fonts)?;
        page.object().dict_put("Resources", res)?;
        for (rect, png) in &sheet.images {
            let img = Image::from_bytes(png)?;
            page.insert_image(&mut self.pdf, *rect, PageImageSource::Image(&img), InsertImageOptions::default())?;
        }
        page.insert_contents(&mut self.pdf, sheet.ops.as_bytes(), true)?;
        self.pages += 1;
        Ok(())
    }

    fn ops(&mut self) -> &mut String {
        &mut self.sheet.as_mut().expect("a page is open").ops
    }

    /// Starts a new page when `h` points do not fit under the pen.
    fn room(&mut self, h: f32) -> Result<()> {
        if self.sheet.is_none() || self.y - h < MARGIN {
            self.page()?;
        }
        Ok(())
    }

    fn text(&mut self, x: f32, baseline: f32, size: f32, bold: bool, colour: Rgb, s: &str) {
        let (font, name) = if bold { (&self.bold, "F2") } else { (&self.reg, "F1") };
        let hex = font.hex(s);
        if hex.is_empty() {
            return;
        }
        let op = format!("BT /{name} {size} Tf {} rg {x:.2} {baseline:.2} Td <{hex}> Tj ET\n", Self::rgb(colour));
        self.ops().push_str(&op);
    }

    fn fill(&mut self, r: [f32; 4], colour: Rgb) {
        let op = format!("{} rg {:.2} {:.2} {:.2} {:.2} re f\n", Self::rgb(colour), r[0], r[1], r[2] - r[0], r[3] - r[1]);
        self.ops().push_str(&op);
    }

    fn outline(&mut self, r: [f32; 4], colour: Rgb, width: f32) {
        let op = format!("{} RG {width} w {:.2} {:.2} {:.2} {:.2} re S\n", Self::rgb(colour), r[0], r[1], r[2] - r[0], r[3] - r[1]);
        self.ops().push_str(&op);
    }

    /// Wrapped paragraph; returns nothing, moves the pen down.
    fn para(&mut self, x: f32, width: f32, size: f32, bold: bool, colour: Rgb, s: &str) -> Result<()> {
        let lines = if bold { self.bold.wrap(s, size, width) } else { self.reg.wrap(s, size, width) };
        let lead = size * 1.35;
        for l in lines {
            self.room(lead)?;
            self.y -= lead;
            self.text(x, self.y + size * 0.28, size, bold, colour, &l);
        }
        Ok(())
    }
}

fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > CLIP_CHARS { format!("{}...", s.chars().take(CLIP_CHARS).collect::<String>()) } else { s.to_string() }
}

fn page_of(c: &Change) -> Option<u32> {
    c.b_page.or(c.a_page)
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Builds the report. `cancel` is polled per page written; `on` gets (written, total) as `Stage::Pages`.
pub fn build(input: &ReportInput, cmp: &Comparison, cancel: &AtomicBool, on: &mut dyn FnMut(Progress)) -> Result<Vec<u8>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_inner(input, cmp, cancel, on))).unwrap_or(Err(DiffError::Panic))
}

fn build_inner(input: &ReportInput, cmp: &Comparison, cancel: &AtomicBool, on: &mut dyn FnMut(Progress)) -> Result<Vec<u8>> {
    let _ = mupdf::Context::get();
    let mut w = Writer::new()?;
    let width = PAGE_W - 2.0 * MARGIN;
    let check = |w: &mut Writer, on: &mut dyn FnMut(Progress), total: u32| -> Result<()> {
        if cancel.load(Ordering::Relaxed) {
            return Err(DiffError::Cancelled);
        }
        on(Progress { stage: Stage::Pages, done: w.pages, total });
        Ok(())
    };

    // ---- summary
    w.page()?;
    w.y -= 22.0;
    w.text(MARGIN, w.y, 22.0, true, INK, "Comparison report");
    w.y -= 10.0;
    w.fill([MARGIN, w.y - 1.0, MARGIN + width, w.y], RULE);
    w.y -= 20.0;
    for (label, name, pages) in [("Before", input.a_name, cmp.pages.a_pages), ("After", input.b_name, cmp.pages.b_pages)] {
        w.text(MARGIN, w.y, 10.0, true, DIM, label);
        let line = format!("{name}  ({})", plural(pages, "page", "pages"));
        w.text(MARGIN + 52.0, w.y, 11.0, false, INK, &line);
        w.y -= 17.0;
    }
    w.text(MARGIN, w.y, 10.0, true, DIM, "Compared");
    w.text(MARGIN + 52.0, w.y, 11.0, false, INK, input.date);
    w.y -= 30.0;

    let total = cmp.changes.len() as u32;
    let headline = if total == 0 {
        if cmp.identical { "No differences: the two files have the same pages, text, annotations and form fields.".to_string() } else { "No differences found.".to_string() }
    } else {
        format!("{} found", plural(total, "difference", "differences"))
    };
    w.para(MARGIN, width, 15.0, true, INK, &headline)?;
    w.y -= 8.0;
    for (name, n) in counts(&cmp.changes) {
        w.room(18.0)?;
        w.fill([MARGIN, w.y - 12.0, MARGIN + 8.0, w.y - 4.0], DIM);
        w.text(MARGIN + 16.0, w.y - 12.0, 11.5, false, INK, name);
        let n = n.to_string();
        let nw = w.reg.width(&n, 11.5);
        w.text(MARGIN + 170.0 - nw, w.y - 12.0, 11.5, true, INK, &n);
        w.y -= 18.0;
    }

    // ---- the list
    let mut groups: Vec<(Option<u32>, Vec<&Change>)> = Vec::new();
    for c in &cmp.changes {
        let p = page_of(c);
        match groups.iter_mut().find(|g| g.0 == p) {
            Some(g) => g.1.push(c),
            None => groups.push((p, vec![c])),
        }
    }
    groups.sort_by_key(|g| g.0.unwrap_or(u32::MAX));
    if !groups.is_empty() {
        w.y -= 22.0;
        w.room(60.0)?;
        w.text(MARGIN, w.y, 14.0, true, INK, "All changes");
        w.y -= 8.0;
    }
    for (p, items) in &groups {
        w.room(48.0)?;
        w.y -= 20.0;
        let title = match p {
            Some(p) => format!("Page {}", p + 1),
            None => "Document".to_string(),
        };
        w.text(MARGIN, w.y, 11.0, true, DIM, &title);
        w.fill([MARGIN, w.y - 5.0, MARGIN + width, w.y - 4.5], RULE);
        w.y -= 6.0;
        for c in items {
            w.room(30.0)?;
            let (colour, glyph) = tone_of(c);
            w.y -= 3.0;
            let top = w.y;
            w.fill([MARGIN, top - 11.0, MARGIN + 3.0, top], colour);
            w.text(MARGIN + 10.0, top - 10.0, 10.5, true, colour, glyph);
            w.para(MARGIN + 26.0, width - 26.0, 10.5, false, INK, &c.label)?;
            if let Detail::Text { runs } = &c.detail {
                let join = |op| clip(&runs.iter().filter(|r| r.op == op).map(|r| r.text.as_str()).collect::<Vec<_>>().join(" ... "));
                let (del, ins) = (join(RunOp::Delete), join(RunOp::Insert));
                if !del.is_empty() {
                    w.para(MARGIN + 26.0, width - 26.0, 9.5, false, DEL, &format!("- {del}"))?;
                }
                if !ins.is_empty() {
                    w.para(MARGIN + 26.0, width - 26.0, 9.5, false, ADD, &format!("+ {ins}"))?;
                }
            }
            if let Detail::Format { props, .. } = &c.detail {
                let s = props.iter().map(|p| format!("{}: {} to {}", p.name, p.old, p.new)).collect::<Vec<_>>().join("; ");
                w.para(MARGIN + 26.0, width - 26.0, 9.5, false, DIM, &s)?;
            }
            if let Detail::Metadata { fields } = &c.detail {
                for f in fields {
                    let s = format!("{}: {} to {}", f.field, f.old.as_deref().unwrap_or("(none)"), f.new.as_deref().unwrap_or("(none)"));
                    w.para(MARGIN + 26.0, width - 26.0, 9.5, false, DIM, &clip(&s))?;
                }
            }
            if let Detail::Bookmarks { items } = &c.detail {
                for i in items.iter().take(40) {
                    let op = format!("{:?}", i.op).to_lowercase();
                    w.para(MARGIN + 26.0, width - 26.0, 9.5, false, DIM, &format!("{op}: {}", clip(&i.title)))?;
                }
            }
            w.y -= 2.0;
        }
        check(&mut w, on, 0)?;
    }

    // ---- the pages, side by side
    let mut pairs: Vec<(Option<u32>, Option<u32>)> = Vec::new();
    for c in &cmp.changes {
        if page_of(c).is_none() {
            continue;
        }
        let key = (c.a_page, c.b_page);
        if !pairs.contains(&key) {
            pairs.push(key);
        }
    }
    pairs.sort_by_key(|&(a, b)| b.or(a).unwrap_or(u32::MAX));
    let shown = pairs.len().min(MAX_PICTURE_PAGES);
    let (ia, ib) = (BytesId::of(input.a), BytesId::of(input.b));
    let col = (width - 16.0) / 2.0;
    for (n, &(pa, pb)) in pairs.iter().take(shown).enumerate() {
        let total = (w.pages + (shown - n) as u32).max(1);
        check(&mut w, on, total)?;
        w.page()?;
        w.y -= 16.0;
        let head = match (pa, pb) {
            (Some(a), Some(b)) if a == b => format!("Page {}", b + 1),
            (Some(a), Some(b)) => format!("Page {} before, page {} after", a + 1, b + 1),
            (None, Some(b)) => format!("Page {} (added)", b + 1),
            (Some(a), None) => format!("Page {} before (removed)", a + 1),
            (None, None) => String::new(),
        };
        w.text(MARGIN, w.y, 13.0, true, INK, &head);
        w.y -= 12.0;
        let top = w.y;
        let mut bottom = top;
        for (side, (page, bytes, id, x)) in [(Side::A, (pa, input.a, ia, MARGIN)), (Side::B, (pb, input.b, ib, MARGIN + col + 16.0))] {
            let caption = if side == Side::A { "Before" } else { "After" };
            w.text(x, top - 11.0, 9.5, true, DIM, caption);
            let Some(page) = page else { continue };
            let r = render_page(id, bytes, page, 1.0)?;
            let (pw, ph) = (r.width as f32 / r.scale, r.height as f32 / r.scale);
            let k = (col / pw).min((PAGE_H - 2.0 * MARGIN - 70.0) / ph);
            let (dw, dh) = (pw * k, ph * k);
            let y1 = top - 18.0;
            let rect = Rect::new(x, y1 - dh, x + dw, y1);
            w.sheet.as_mut().expect("a page is open").images.push((rect, r.to_png()?));
            w.outline([x, y1 - dh, x + dw, y1], RULE, 0.75);
            for c in cmp.changes.iter().filter(|c| if side == Side::A { c.a_page == Some(page) } else { c.b_page == Some(page) }) {
                let r = if side == Side::A { c.rect_a } else { c.rect_b };
                let (colour, _) = tone_of(c);
                if let Some(r) = r {
                    let pad = 1.5;
                    w.outline([x + r[0] * k - pad, y1 - r[3] * k - pad, x + r[2] * k + pad, y1 - r[1] * k + pad], colour, 1.0);
                }
            }
            bottom = bottom.min(y1 - dh);
        }
        w.y = bottom;
    }
    if pairs.len() > shown {
        w.room(30.0)?;
        w.y -= 24.0;
        let s = format!("Pictures are shown for the first {shown} of {} changed pages; the list above covers every change.", pairs.len());
        w.para(MARGIN, width, 10.0, false, DIM, &s)?;
    }

    w.flush()?;
    on(Progress { stage: Stage::Pages, done: w.pages, total: w.pages });
    let mut out = Vec::new();
    let mut opts = PdfWriteOptions::default();
    opts.set_compress(true).set_compress_images(true).set_compress_fonts(true).set_garbage_level(2);
    w.pdf.write_to_with_options(&mut out, opts)?;
    Ok(out)
}
