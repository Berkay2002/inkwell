//! Save as PDF: the print plan written as a new vector PDF, one page per sheet side.
//!
//! The same sheets the preview and the printer draw (`layout`), the same page choice and content
//! mode (`render::Source`), only the target is a file. Each source page used becomes one Form
//! XObject, placed by its matrix and clipped to its cell or tile, so a poster's tiles and an N-up
//! sheet's cells share one copy of the page. Idea after pdfcraft's writer (crates/print/src/spool.rs
//! @ b8ddf71, "one form per source page used"); the object work is MuPDF's.
//!
//! The open document is never touched: pages come from the private copy `Source` parses, and the
//! output is a new file.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use mupdf::pdf::{PdfDocument, PdfGraftMap, PdfObject, PdfPage};
use mupdf::Buffer;
use serde::Serialize;

use super::layout::{self, Label, Sheet, LABEL_PT};
use super::render::{RenderError, Source};
use super::{plan, PrintError, Progress, Settings};
use crate::engine::edit::{rewrite_opts, write_atomic, write_doc, Readback};
use crate::engine::{DocId, Engine, EngineError};

/// What was written.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    /// Source pages chosen.
    pub pages: u32,
    /// Pages of the new file.
    pub sheets: u32,
}

type M = [f64; 6];

/// `a` first, then `b` (PDF row-vector matrices).
fn mul(a: M, b: M) -> M {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn n(v: f64) -> String {
    let s = format!("{:.3}", if v.is_finite() && v.abs() >= 5e-4 { v } else { 0.0 });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.into() }
}

/// A PDF literal string for text made of ASCII (labels are digits and fixed words).
fn literal(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars().filter(char::is_ascii) {
        if matches!(c, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push(')');
    out
}

fn name(s: &str) -> Result<PdfObject, mupdf::Error> {
    PdfObject::new_name(s)
}

/// The label text of a sheet as content operators (the page is y up, `h` high).
fn label_ops(labels: &[&Label], h: f64) -> String {
    let mut s = String::new();
    for l in labels {
        s += &format!("BT /F1 {} Tf 0 g {} {} Td {} Tj ET\n", n(LABEL_PT), n(l.x), n(h - l.y), literal(&l.text));
    }
    s
}

/// A page of `w` x `h` points appended to `doc`: `content` over `xobjects` (name, object) and the
/// `/F1` font when `font` is given.
fn add_page(doc: &mut PdfDocument, (w, h): (f64, f64), content: &str, xobjects: &[(String, PdfObject)], font: Option<&PdfObject>) -> Result<(), mupdf::Error> {
    let mut res = doc.new_dict()?;
    if !xobjects.is_empty() {
        let mut x = doc.new_dict()?;
        for (k, v) in xobjects {
            x.dict_put(k.as_str(), v.clone())?;
        }
        res.dict_put("XObject", x)?;
    }
    if let Some(f) = font {
        let mut fonts = doc.new_dict()?;
        fonts.dict_put("F1", f.clone())?;
        res.dict_put("Font", fonts)?;
    }
    let contents = doc.add_stream(&Buffer::from_bytes(content.as_bytes())?, None, false)?;
    let mut media = doc.new_array()?;
    for v in [0.0, 0.0, w, h] {
        media.array_push(PdfObject::new_real(v as f32)?)?;
    }
    let mut page = doc.new_dict()?;
    page.dict_put("Type", name("Page")?)?;
    page.dict_put("MediaBox", media)?;
    page.dict_put("Resources", res)?;
    page.dict_put("Contents", contents)?;
    let page = doc.add_object(&page)?;
    let at = doc.page_count()?;
    doc.insert_page(at, &page)
}

fn helvetica(doc: &mut PdfDocument) -> Result<PdfObject, mupdf::Error> {
    let mut f = doc.new_dict()?;
    f.dict_put("Type", name("Font")?)?;
    f.dict_put("Subtype", name("Type1")?)?;
    f.dict_put("BaseFont", name("Helvetica")?)?;
    f.dict_put("Encoding", name("WinAnsiEncoding")?)?;
    doc.add_object(&f)
}

/// A one-page document holding just `labels` of a `w` x `h` sheet, and its page: how the raster
/// renderer draws text (MuPDF has no show-string call in this binding), from the operators the
/// PDF writer writes.
pub(super) fn labels_page(w: f64, h: f64, labels: &[&Label]) -> Result<(PdfDocument, PdfPage), RenderError> {
    let mut doc = PdfDocument::new();
    let font = helvetica(&mut doc)?;
    add_page(&mut doc, (w, h), &label_ops(labels, h), &[], Some(&font))?;
    let page = doc.load_pdf_page(0)?;
    Ok((doc, page))
}

/// The operators of one sheet side (y up): its pages, borders, cut marks and labels.
fn sheet_ops(sheet: &Sheet, bounds: &HashMap<u32, [f64; 4]>) -> String {
    let h = sheet.h;
    let mut s = String::new();
    for p in &sheet.placed {
        let Some(b) = bounds.get(&p.page) else { continue };
        let c = p.clip;
        if c[2] <= c[0] || c[3] <= c[1] {
            continue;
        }
        // the form (display space, y up) -> MuPDF page space (y down) -> sheet (y down) -> output (y up)
        let m = p.matrix((b[0], b[1]), b[2] - b[0]);
        let full = mul(mul([1.0, 0.0, 0.0, -1.0, b[0], b[3]], m), [1.0, 0.0, 0.0, -1.0, 0.0, h]);
        s += &format!("q {} {} {} {} re W n\n", n(c[0]), n(h - c[3]), n(c[2] - c[0]), n(c[3] - c[1]));
        s += &format!("{} cm /Fm{} Do Q\n", full.map(n).join(" "), p.page);
    }
    if !sheet.borders.is_empty() || !sheet.lines.is_empty() {
        s += "q 0.5 w 0 G\n";
        for r in &sheet.borders {
            s += &format!("{} {} {} {} re S\n", n(r[0]), n(h - r[3]), n(r[2] - r[0]), n(r[3] - r[1]));
        }
        for l in &sheet.lines {
            s += &format!("{} {} m {} {} l S\n", n(l[0]), n(h - l[1]), n(l[2]), n(h - l[3]));
        }
        s += "Q\n";
    }
    s += &label_ops(&sheet.labels.iter().collect::<Vec<_>>(), h);
    s
}

/// Writes the plan for `doc` under `s` to `out` as a PDF. Copies and collation do not apply; two-sided
/// padding does, so a booklet printed double-sided from the file comes out right.
pub fn save(eng: &Engine, doc: DocId, s: &Settings, out: &Path, cancel: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<Saved, PrintError> {
    let check = || if cancel.load(Ordering::Relaxed) { Err(PrintError::Cancelled) } else { Ok(()) };
    check()?;
    // the open document's own file must never be the target
    eng.check_not_open(&out.to_string_lossy(), None)?;
    let p = plan(eng, doc, s)?;
    let (mut src, rev) = Source::open(eng, doc, s.content)?;
    if rev != p.rev {
        return Err(PrintError::Engine(EngineError::Stale));
    }
    let mut used: Vec<u32> = p.sheets.iter().flat_map(|f| f.placed.iter().map(|x| x.page)).collect();
    used.sort_unstable();
    used.dedup();
    let src_doc = src.export_doc(&used)?;

    let mut file = PdfDocument::new();
    let mut map: PdfGraftMap = file.new_graft_map().map_err(RenderError::from)?;
    let mut forms: HashMap<u32, PdfObject> = HashMap::new();
    let mut bounds: HashMap<u32, [f64; 4]> = HashMap::new();
    for &page in &used {
        check()?;
        let (form, _, _) = crate::marks::display_form(&mut file, &mut map, src_doc, page)?;
        let b = src_doc.load_pdf_page(page as i32).map_err(RenderError::from)?.bounds().map_err(RenderError::from)?;
        bounds.insert(page, [f64::from(b.x0), f64::from(b.y0), f64::from(b.x1), f64::from(b.y1)]);
        forms.insert(page, form);
    }
    let font = if p.sheets.iter().any(|f| !f.labels.is_empty()) { Some(helvetica(&mut file).map_err(RenderError::from)?) } else { None };

    let order = layout::sequence(p.sheets.len(), 1, true, s.two_sided());
    let total = order.len() as u32;
    progress(Progress { done: 0, total });
    let mut size = p.sheets.first().map_or((595.0, 842.0), |f| (f.w, f.h));
    for (done, item) in order.iter().enumerate() {
        check()?;
        match item {
            Some(k) => {
                let sheet = &p.sheets[*k];
                size = (sheet.w, sheet.h);
                let xobjects: Vec<(String, PdfObject)> =
                    sheet.placed.iter().filter_map(|x| forms.get(&x.page).map(|f| (format!("Fm{}", x.page), f.clone()))).collect();
                add_page(&mut file, size, &sheet_ops(sheet, &bounds), &xobjects, font.as_ref()).map_err(RenderError::from)?;
            }
            // the blank back of an odd last sheet, as large as the sheet before it
            None => add_page(&mut file, size, "", &[], None).map_err(RenderError::from)?,
        }
        progress(Progress { done: done as u32 + 1, total });
    }
    check()?;
    let bytes = write_doc(&file, rewrite_opts(), 1 << 20)?;
    write_atomic(out, &bytes, total, Readback::default())?;
    Ok(Saved { pages: p.selected.len() as u32, sheets: total })
}
