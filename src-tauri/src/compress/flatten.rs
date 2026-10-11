//! Flatten transparency (Acrobat's Optimized PDF > Transparency).
//!
//! A page that uses transparency (alpha, soft masks, blend modes, images with a soft mask) is drawn
//! once at the chosen resolution and replaced by that picture. Nothing else is touched: the pages
//! without transparency, the annotations and the form fields stay as they are. What it costs is the
//! text and the vectors on the pages it flattens (they become part of the picture), so the report
//! lists the page numbers and the visual check compares the result with the original.
//!
//! Left alone, with a note: rotated pages, pages with a `/UserUnit`, and pages that have no usable
//! page box.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Buffer, Colorspace, Image, Matrix};
use serde::{Deserialize, Serialize};

use super::Error;

/// Resolution bounds: 36 ppi is unreadable, 600 is more than any screen or office printer needs.
pub const PPI: std::ops::RangeInclusive<u32> = 36..=600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Transparency {
    /// Flatten the pages that use transparency.
    pub flatten: bool,
    /// Resolution of the picture that replaces such a page.
    pub ppi: u32,
}

impl Default for Transparency {
    fn default() -> Self {
        Self { flatten: false, ppi: 150 }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlattenReport {
    /// 1-based numbers of the pages that were replaced by a picture.
    pub pages: Vec<u32>,
    /// Pages that use transparency and were left alone, with the reason.
    pub skipped: Vec<(u32, String)>,
}

const DEPTH: usize = 8;

fn num(o: &PdfObject) -> Option<f32> {
    o.is_number().ok()?.then(|| o.as_float().ok()).flatten()
}

/// Does this ExtGState dictionary draw with anything but plain opaque paint?
fn gstate_transparent(g: &PdfObject) -> bool {
    for k in ["CA", "ca"] {
        if g.get_dict(k).ok().flatten().and_then(|v| num(&v)).is_some_and(|v| v < 1.0) {
            return true;
        }
    }
    if let Ok(Some(m)) = g.get_dict("SMask") {
        let none = m.is_name().unwrap_or(false) && m.as_name().is_ok_and(|n| n == b"None");
        if !none {
            return true;
        }
    }
    if let Ok(Some(b)) = g.get_dict("BM") {
        let first = if b.is_array().unwrap_or(false) { b.get_array(0).ok().flatten() } else { Some(b) };
        let normal = first.is_some_and(|f| f.as_name().is_ok_and(|n| n == b"Normal" || n == b"Compatible"));
        if !normal {
            return true;
        }
    }
    false
}

/// Calls `f` on each value of the dictionary `res[key]` (each indirect object once); true when `f` did.
fn each(res: &PdfObject, seen: &mut HashSet<i32>, key: &str, f: &mut dyn FnMut(&PdfObject) -> bool) -> bool {
    let Ok(Some(d)) = res.get_dict(key) else { return false };
    if !d.is_dict().unwrap_or(false) {
        return false;
    }
    for i in 0..d.dict_len().unwrap_or(0) as i32 {
        let Ok(Some(v)) = d.get_dict_val(i) else { continue };
        if let Ok(n) = v.as_indirect()
            && n > 0
            && !seen.insert(n)
        {
            continue;
        }
        if f(&v) {
            return true;
        }
    }
    false
}

fn resources_transparent(res: &PdfObject, seen: &mut HashSet<i32>, depth: usize) -> bool {
    if depth > DEPTH || !res.is_dict().unwrap_or(false) {
        return false;
    }
    if each(res, seen, "ExtGState", &mut |g| gstate_transparent(g)) {
        return true;
    }
    let mut nested: Vec<PdfObject> = Vec::new();
    let found = each(res, seen, "XObject", &mut |x| {
        let sub = x.get_dict("Subtype").ok().flatten().and_then(|s| s.as_name().ok());
        match sub.as_deref() {
            Some(b"Image") => {
                x.get_dict("SMask").ok().flatten().is_some()
                    || x.get_dict("SMaskInData").ok().flatten().and_then(|v| v.as_int().ok()).unwrap_or(0) > 0
            }
            Some(b"Form") => {
                if let Ok(Some(r)) = x.get_dict("Resources") {
                    nested.push(r);
                }
                false
            }
            _ => false,
        }
    });
    if found {
        return true;
    }
    each(res, seen, "Pattern", &mut |p| {
        if let Ok(Some(r)) = p.get_dict("Resources") {
            nested.push(r);
        }
        false
    });
    nested.iter().any(|r| resources_transparent(r, seen, depth + 1))
}

/// Zero-based indexes of the pages that use transparency.
pub fn pages_using_transparency(doc: &PdfDocument) -> Result<Vec<usize>, Error> {
    let mut out = Vec::new();
    for i in 0..doc.page_count()? {
        let page = doc.find_page(i)?;
        let Ok(Some(res)) = page.get_dict_inheritable("Resources") else { continue };
        if resources_transparent(&res, &mut HashSet::new(), 0) {
            out.push(i as usize);
        }
    }
    Ok(out)
}

fn rect_of(page: &PdfObject, key: &str) -> Option<[f32; 4]> {
    let a = page.get_dict_inheritable(key).ok()??;
    if !a.is_array().ok()? || a.len().ok()? != 4 {
        return None;
    }
    let mut v = [0f32; 4];
    for (i, slot) in v.iter_mut().enumerate() {
        *slot = num(&a.get_array(i as i32).ok()??)?;
    }
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

/// Replaces each page in `pages` (zero-based) by a picture of it. Pages that cannot be flattened are
/// listed in `skipped`.
pub fn flatten(
    doc: &mut PdfDocument,
    pages: &[usize],
    ppi: u32,
    cancel: &AtomicBool,
    tick: &mut dyn FnMut(usize, usize),
) -> Result<FlattenReport, Error> {
    let mut report = FlattenReport::default();
    let scale = ppi as f32 / 72.0;
    for (k, &i) in pages.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        tick(k, pages.len());
        let mut page = doc.find_page(i as i32)?;
        let n = i as u32 + 1;
        let rotate = page.get_dict_inheritable("Rotate")?.and_then(|r| r.as_int().ok()).unwrap_or(0);
        if rotate.rem_euclid(360) != 0 {
            report.skipped.push((n, "The page is rotated".into()));
            continue;
        }
        if page.get_dict("UserUnit")?.is_some() {
            report.skipped.push((n, "The page has its own unit".into()));
            continue;
        }
        let media = rect_of(&page, "MediaBox");
        let crop = rect_of(&page, "CropBox").or(media);
        let Some(b) = crop.filter(|b| b[2] - b[0] > 1.0 && b[3] - b[1] > 1.0) else {
            report.skipped.push((n, "The page has no page box".into()));
            continue;
        };
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        let px = (w * scale).ceil().max(1.0) as f64 * (h * scale).ceil().max(1.0) as f64;
        if px > 60_000_000.0 {
            report.skipped.push((n, "The page is too large to draw at that resolution".into()));
            continue;
        }

        // Contents only: annotations and fields stay live on top of the picture.
        let pix = {
            let p = doc.load_pdf_page(i as i32)?;
            p.to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), false, false)?
        };
        let img = Image::from_pixmap(&pix)?;
        let image = doc.add_image(&img)?;

        let content = format!("q {w} 0 0 {h} {} {} cm /InkwellFlat Do Q\n", b[0], b[1]);
        let stream = doc.add_stream(&Buffer::from_bytes(content.as_bytes())?, None, false)?;
        let mut xobjects = doc.new_dict()?;
        xobjects.dict_put("InkwellFlat", image)?;
        let mut res = doc.new_dict()?;
        res.dict_put("XObject", xobjects)?;
        page.dict_put("Contents", stream)?;
        page.dict_put("Resources", res)?;
        // The page group described the transparency of what was there.
        page.dict_delete("Group")?;
        report.pages.push(n);
    }
    Ok(report)
}
