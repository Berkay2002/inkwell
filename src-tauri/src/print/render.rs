//! Draws a sheet with MuPDF. The preview and the printer both call [`render`], so what the dialog
//! shows is what the printer gets: same pages, same content mode, same matrices; only the
//! resolution differs.

use std::collections::{HashMap, HashSet};

use mupdf::pdf::{AnnotationFlags, PdfDocument, Permission};
use mupdf::{Colorspace, Device, DisplayList, IRect, Matrix, Pixmap, Rect};
use serde::{Deserialize, Serialize};

use super::layout::{Label, Sheet, LABEL_PT};
use crate::engine::{DocSource, Engine, DocId};

/// Acrobat's Comments & Forms choices (Document and Stamps is left out: stamps are markups here).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Content {
    /// Page content and form fields.
    Document,
    /// Page content, form fields and every printable comment.
    #[default]
    DocumentAndMarkups,
    /// Form fields only.
    FormFieldsOnly,
}

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("password required")]
    NeedsPassword,
    #[error("wrong password")]
    BadPassword,
    #[error("this document does not allow printing")]
    NotPermitted,
    #[error("this document does not allow copying. Open it with its permissions password first.")]
    NoCopy,
    #[error("{0}")]
    Mupdf(#[from] mupdf::Error),
    #[error("{0}")]
    Engine(#[from] crate::engine::EngineError),
}

struct Page {
    list: DisplayList,
    /// MuPDF's bounds of the page: the origin and size of its display space.
    bounds: Rect,
}

/// The document as it prints: a private parsed copy (never the viewer's replicas or the edit lane's
/// document), with the display lists of the pages used so far.
pub struct Source {
    doc: PdfDocument,
    content: Content,
    pages: HashMap<u32, Page>,
}

impl Source {
    /// Opens what document `id` shows now (unsaved edits included). Returns the edit generation it
    /// belongs to.
    pub fn open(eng: &Engine, id: DocId, content: Content) -> Result<(Source, u32), RenderError> {
        let (rev, src) = eng.doc_source(id)?;
        let mut doc = match &src {
            DocSource::Path(p) => PdfDocument::open(p.as_str())?,
            DocSource::Bytes(b, n) => PdfDocument::from_bytes(&b[..*n])?,
        };
        if doc.needs_password()? {
            match eng.password(id)? {
                Some(p) if doc.authenticate(&p)? => {}
                Some(_) => return Err(RenderError::BadPassword),
                None => return Err(RenderError::NeedsPassword),
            }
        }
        if !doc.permissions().contains(Permission::PRINT) {
            return Err(RenderError::NotPermitted);
        }
        Ok((Source { doc, content, pages: HashMap::new() }, rev))
    }

    fn page(&mut self, index: u32) -> Result<&Page, RenderError> {
        if !self.pages.contains_key(&index) {
            let page = self.doc.load_pdf_page(index as i32)?;
            let bounds = page.bounds()?;
            if self.content == Content::DocumentAndMarkups {
                hide_unprintable(&page)?;
            }
            let mut list = DisplayList::new(bounds)?;
            {
                let dev = Device::from_display_list(&mut list)?;
                let id = Matrix::IDENTITY;
                if self.content != Content::FormFieldsOnly {
                    page.run_contents(&dev, &id)?;
                }
                if self.content == Content::DocumentAndMarkups {
                    page.run_annotations(&dev, &id)?;
                }
                page.run_widgets(&dev, &id)?;
            }
            self.pages.insert(index, Page { list, bounds });
        }
        Ok(&self.pages[&index])
    }

    /// The private copy with what each page prints as drawn into its own content, for the PDF
    /// writer: unprintable markups hidden, then annotations (markups mode) and form fields baked in
    /// by MuPDF, which synthesises missing appearances and keeps the graphics state balanced. Forms
    /// only drops the page content first.
    pub(super) fn export_doc(&mut self, pages: &[u32]) -> Result<&PdfDocument, RenderError> {
        // Vector output makes text and images copyable, which a printer never gave anyone.
        if !crate::protect::Rights::of(&self.doc).may_extract() {
            return Err(RenderError::NoCopy);
        }
        self.isolate(pages)?;
        for &index in pages {
            let page = self.doc.load_pdf_page(index as i32)?;
            match self.content {
                Content::DocumentAndMarkups => hide_unprintable(&page)?,
                Content::FormFieldsOnly => {
                    let empty = self.doc.add_stream(&mupdf::Buffer::from_bytes(b"")?, None, false)?;
                    page.object().dict_put("Contents", empty)?;
                }
                Content::Document => {}
            }
        }
        self.doc.bake(self.content == Content::DocumentAndMarkups, true)?;
        Ok(&self.doc)
    }

    /// Makes baking touch only the pages being exported. MuPDF's bake writes each annotation's
    /// appearance into the page's Resources/XObject, which pages often share, so a used page would
    /// carry (and the writer graft) the comments of every other page. Unused pages lose their
    /// /Annots and used pages get a Resources dict, and an XObject dict, of their own.
    fn isolate(&mut self, pages: &[u32]) -> Result<(), RenderError> {
        let keep: HashSet<u32> = pages.iter().copied().collect();
        let count = self.doc.page_count()?.max(0) as u32;
        for index in 0..count {
            let mut page = self.doc.find_page(index as i32)?;
            if !keep.contains(&index) {
                page.dict_delete("Annots")?;
                continue;
            }
            let Some(res) = page.get_dict_inheritable("Resources")? else { continue };
            let mut own = self.doc.new_dict()?;
            for k in 0..res.dict_len()? {
                let (Some(key), Some(val)) = (res.get_dict_key(k as i32)?, res.get_dict_val(k as i32)?) else { continue };
                own.dict_put(key, val)?;
            }
            let mut xobjects = self.doc.new_dict()?;
            if let Some(shared) = res.get_dict("XObject")? {
                for k in 0..shared.dict_len()? {
                    let (Some(key), Some(val)) = (shared.get_dict_key(k as i32)?, shared.get_dict_val(k as i32)?) else { continue };
                    xobjects.dict_put(key, val)?;
                }
            }
            own.dict_put("XObject", xobjects)?;
            page.dict_put("Resources", own)?;
        }
        Ok(())
    }

    /// Forgets the display lists of every page not in `keep` (a long job holds one sheet's pages).
    pub fn retain(&mut self, keep: &[u32]) {
        self.pages.retain(|p, _| keep.contains(p));
    }
}

/// Annotations print only with their Print flag; MuPDF's view-time drawing would show all of them.
/// The copy is private, so hiding them here changes nothing the user saves.
fn hide_unprintable(page: &mupdf::pdf::PdfPage) -> Result<(), RenderError> {
    for mut a in page.annotations() {
        let f = a.flags()?;
        if !f.contains(AnnotationFlags::IS_PRINT) && !f.contains(AnnotationFlags::IS_HIDDEN) {
            a.set_flags(f | AnnotationFlags::IS_HIDDEN)?;
        }
    }
    Ok(())
}

/// The pixel rectangle `[x0, y0, x1, y1]` (points) covers at `scale` pixels per point.
fn px(r: [f64; 4], scale: f64) -> IRect {
    IRect { x0: (r[0] * scale).floor() as i32, y0: (r[1] * scale).floor() as i32, x1: (r[2] * scale).ceil() as i32, y1: (r[3] * scale).ceil() as i32 }
}

fn meet(a: IRect, b: IRect) -> IRect {
    IRect { x0: a.x0.max(b.x0), y0: a.y0.max(b.y0), x1: a.x1.min(b.x1), y1: a.y1.min(b.y1) }
}

/// The sheet's size in pixels at `scale` pixels per point.
pub fn sheet_px(sheet: &Sheet, scale: f64) -> (i32, i32) {
    ((sheet.w * scale).ceil() as i32, (sheet.h * scale).ceil() as i32)
}

/// A cut mark (an axis-aligned hairline) as pixels: at least one device pixel thick.
fn line_px(l: [f64; 4], scale: f64) -> IRect {
    let w = ((0.5 * scale).round() as i32).max(1);
    let r = px(l, scale);
    if l[0] == l[2] {
        IRect { x1: r.x0 + w, ..r }
    } else {
        IRect { y1: r.y0 + w, ..r }
    }
}

/// Where a label's text can land, with room for the font's width and descenders.
fn label_px(l: &Label, scale: f64) -> IRect {
    let w = l.text.len() as f64 * LABEL_PT * 0.6;
    px([l.x, l.y - LABEL_PT, l.x + w, l.y + LABEL_PT * 0.3], scale)
}

/// Does anything of `sheet` land in `band` (pixel rows `y0..y1`)? An empty band stays paper-white.
/// Cut marks and labels sit in the margin, outside every clip, so they count on their own.
pub fn band_has_ink(sheet: &Sheet, scale: f64, y0: i32, y1: i32) -> bool {
    sheet.placed.iter().any(|p| {
        let c = px(p.clip, scale);
        c.y0 < y1 && c.y1 > y0
    }) || sheet.borders.iter().any(|b| {
        let c = px(*b, scale);
        c.y0 - 1 < y1 && c.y1 + 1 > y0
    }) || sheet.lines.iter().any(|l| {
        let c = line_px(*l, scale);
        c.y0 < y1 && c.y1 > y0
    }) || sheet.labels.iter().any(|l| {
        let c = label_px(l, scale);
        c.y0 < y1 && c.y1 > y0
    })
}

/// One band of `sheet` (device pixels, origin at the sheet's top left, `scale` pixels per point) as a
/// white-backed RGB pixmap, or gray when `gray`.
pub fn render(src: &mut Source, sheet: &Sheet, scale: f64, band: IRect, gray: bool) -> Result<Pixmap, RenderError> {
    let cs = if gray { Colorspace::device_gray() } else { Colorspace::device_rgb() };
    let mut pix = Pixmap::new_with_rect(&cs, band, false)?;
    pix.clear_with(0xff)?;
    for p in &sheet.placed {
        let clip = meet(px(p.clip, scale), band);
        if clip.x0 >= clip.x1 || clip.y0 >= clip.y1 {
            continue;
        }
        let page = src.page(p.page)?;
        let m = p.matrix((page.bounds.x0 as f64, page.bounds.y0 as f64), page.bounds.width() as f64);
        let k = scale;
        let ctm = Matrix { a: (m[0] * k) as f32, b: (m[1] * k) as f32, c: (m[2] * k) as f32, d: (m[3] * k) as f32, e: (m[4] * k) as f32, f: (m[5] * k) as f32 };
        let dev = Device::from_pixmap_with_clip(&pix, clip)?;
        // MuPDF takes the cull rectangle in device space (after the ctm).
        let area = Rect::new(clip.x0 as f32, clip.y0 as f32, clip.x1 as f32, clip.y1 as f32);
        page.list.run(&dev, &ctm, area)?;
    }
    // Borders: a hairline of at least one device pixel.
    let line = |gray: bool| if gray { mupdf::pixmap::Pixel::gray(0) } else { mupdf::pixmap::Pixel::rgb(0, 0, 0) };
    for b in &sheet.borders {
        let w = ((0.5 * scale).round() as i32).max(1);
        let r = px(*b, scale);
        for side in [
            IRect { x0: r.x0, y0: r.y0, x1: r.x1, y1: r.y0 + w },
            IRect { x0: r.x0, y0: r.y1 - w, x1: r.x1, y1: r.y1 },
            IRect { x0: r.x0, y0: r.y0, x1: r.x0 + w, y1: r.y1 },
            IRect { x0: r.x1 - w, y0: r.y0, x1: r.x1, y1: r.y1 },
        ] {
            let c = meet(side, band);
            if c.x0 < c.x1 && c.y0 < c.y1 {
                pix.set_rect(c, line(gray))?;
            }
        }
    }
    for l in &sheet.lines {
        let c = meet(line_px(*l, scale), band);
        if c.x0 < c.x1 && c.y0 < c.y1 {
            pix.set_rect(c, line(gray))?;
        }
    }
    // Labels are text: drawn from the same page content the PDF writer emits.
    let in_band: Vec<&Label> = sheet.labels.iter().filter(|l| {
        let c = label_px(l, scale);
        c.y0 < band.y1 && c.y1 > band.y0 && c.x0 < band.x1 && c.x1 > band.x0
    }).collect();
    if !in_band.is_empty() {
        let (doc, page) = super::pdf::labels_page(sheet.w, sheet.h, &in_band)?;
        let ctm = Matrix { a: scale as f32, b: 0.0, c: 0.0, d: scale as f32, e: 0.0, f: 0.0 };
        let dev = Device::from_pixmap_with_clip(&pix, band)?;
        page.run_contents(&dev, &ctm)?;
        drop(doc);
    }
    Ok(pix)
}
