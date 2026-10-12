//! "Searchable image, cleaned up": straighten a tilted scan and remove specks, then the usual
//! invisible text layer on a picture that is now upright (so the layer is no longer skewed).
//!
//! Only pages whose picture is one image that fills the page are touched, and only when something
//! changes: a page the engine found straight and clean keeps its original bytes. The new picture is
//! written as a new image object in the family of the old one (JPEG stays JPEG, a 1-bit scan becomes
//! CCITT G4 again, everything else Flate), and only this page is pointed at it: an image another page
//! shares stays exactly as it was.

use std::io::Write;

use mupdf::pdf::{PdfDocument, PdfObject, PdfPage};
use mupdf::Buffer;
use serde::Serialize;

use std::sync::atomic::{AtomicBool, Ordering};

use super::raster::{self, Raster};
use super::{OcrError, PageResult, PageStatus, Progress};
use crate::imageedit;
use crate::textedit::content::Matrix;

/// A tilt smaller than this is not worth a resample (the picture would only get softer).
const MIN_DESKEW: f32 = 0.25;
/// A tilt larger than this is more likely a wrong reading than a crooked scan.
const MAX_DESKEW: f32 = 15.0;
/// A JPEG whose original quality cannot be read is written at this one.
const JPEG_QUALITY: u8 = 90;
/// A re-encoded JPEG keeps its original's quality within these bounds: not softer than a scan that
/// was already soft (the artefacts would pile up), not larger than a picture this good needs.
const JPEG_QUALITY_RANGE: (u8, u8) = (60, 95);
/// The picture must cover this much of the page to count as "the scan".
const MIN_COVER: f64 = 0.6;
/// Notes kept in the report.
const MAX_NOTES: usize = 5;

fn mupdf_err(e: mupdf::Error) -> OcrError {
    OcrError::Pdf(e.to_string())
}

/// What cleaning did, summed over the pages.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CleanReport {
    /// Pages whose picture was replaced.
    pub pages_cleaned: u32,
    /// Of those, pages that were straightened.
    pub pages_deskewed: u32,
    pub specks_removed: u32,
    /// Pages that needed nothing.
    pub pages_clean: u32,
    /// Pages left as they were because they could not be cleaned (several pictures, a redaction mark, …).
    pub pages_left: u32,
    /// Why, for the first few (`"Page 3: it has more than one picture"`).
    pub notes: Vec<String>,
}

impl CleanReport {
    fn left(&mut self, page: u32, why: &str) {
        self.pages_left += 1;
        if self.notes.len() < MAX_NOTES {
            self.notes.push(format!("Page {}: {why}", page + 1));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Family {
    /// DCTDecode: re-encoded as JPEG, at about the quality the original had.
    Jpeg(u8),
    /// One bit per pixel, whatever the filter: re-encoded as CCITT G4.
    Bilevel,
    /// Anything else with 8-bit gray or RGB samples: Flate.
    Raw,
}

enum Outcome {
    Cleaned { deskewed: bool, specks: u32 },
    Clean,
    Left(String),
}

/// Cleans the picture of every recognised page that qualifies. A deskewed page gets `skew_deg =
/// None`, so its text layer follows the straightened picture. `cancel` is looked at before each page
/// (`Err(Cancelled)`: the caller drops the document), `progress` sees each page done.
pub(crate) fn clean_pages(
    doc: &mut PdfDocument,
    results: &mut [PageResult],
    progress: &(dyn Fn(Progress) + Sync),
    cancel: &AtomicBool,
) -> Result<CleanReport, OcrError> {
    let mut report = CleanReport::default();
    let total = results.iter().filter(|r| r.status == PageStatus::Recognized).count() as u32;
    for (n, r) in results.iter_mut().filter(|r| r.status == PageStatus::Recognized).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(OcrError::Cancelled);
        }
        let done = clean_page(doc, r);
        progress(Progress { done: n as u32 + 1, total, page: r.page, writing: true });
        match done {
            Ok(Outcome::Cleaned { deskewed, specks }) => {
                report.pages_cleaned += 1;
                report.pages_deskewed += u32::from(deskewed);
                report.specks_removed += specks;
            }
            Ok(Outcome::Clean) => report.pages_clean += 1,
            Ok(Outcome::Left(why)) => report.left(r.page, &why),
            // One page that cannot be rewritten does not fail the run: it keeps its picture.
            Err(e) => report.left(r.page, &e.to_string()),
        }
    }
    Ok(report)
}

/// Whether the page carries a redaction mark: its pixels are the redaction engine's business.
pub(crate) fn has_redaction(doc: &PdfDocument, page: u32) -> bool {
    let Ok(p) = doc.find_page(page as i32) else { return false };
    let Ok(Some(annots)) = p.get_dict("Annots") else { return false };
    let n = annots.len().unwrap_or(0) as i32;
    (0..n).any(|i| {
        annots
            .get_array(i)
            .ok()
            .flatten()
            .and_then(|a| a.get_dict("Subtype").ok().flatten())
            .and_then(|s| s.as_name().ok())
            .is_some_and(|s| s == b"Redact")
    })
}

fn filter_names(obj: &PdfObject) -> Vec<String> {
    let Some(f) = obj.get_dict("Filter").ok().flatten() else { return Vec::new() };
    let name = |x: &PdfObject| x.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned());
    if f.is_array().unwrap_or(false) {
        (0..f.len().unwrap_or(0) as i32).filter_map(|i| f.get_array(i).ok().flatten()).filter_map(|x| name(&x)).collect()
    } else {
        name(&f).into_iter().collect()
    }
}

/// DeviceGray, DeviceRGB, CalGray/CalRGB and ICCBased: the spaces whose samples are brightness or RGB.
fn plain_space(obj: &PdfObject) -> bool {
    let Some(cs) = obj.get_dict("ColorSpace").ok().flatten() else { return false };
    let head = if cs.is_array().unwrap_or(false) { cs.get_array(0).ok().flatten().and_then(|n| n.as_name().ok()) } else { cs.as_name().ok() };
    matches!(head.as_deref(), Some(b"DeviceGray" | b"DeviceRGB" | b"CalGray" | b"CalRGB" | b"ICCBased"))
}

/// A `/Decode` that is absent or the identity; anything else (an inverted gray) is left alone.
/// Returns `Some(invert)` for a 1-bit image with `[1 0]`.
fn decode_inverted(obj: &PdfObject) -> Option<bool> {
    let Some(d) = obj.get_dict("Decode").ok().flatten().filter(|d| d.is_array().unwrap_or(false)) else { return Some(false) };
    let vals: Vec<f32> = (0..d.len().unwrap_or(0) as i32).filter_map(|i| d.get_array(i).ok().flatten()).filter_map(|v| v.as_float().ok().or_else(|| v.as_int().ok().map(|i| i as f32))).collect();
    let identity = vals.chunks(2).all(|p| p.len() == 2 && p[0] == 0.0 && p[1] == 1.0);
    let inverted = vals.chunks(2).all(|p| p.len() == 2 && p[0] == 1.0 && p[1] == 0.0);
    if identity {
        Some(false)
    } else if inverted && vals.len() == 2 {
        Some(true)
    } else {
        None
    }
}

/// Decodes the image into a raster and tells which family it re-encodes in.
pub(crate) fn decode(obj: &PdfObject) -> Result<(Raster, Family), String> {
    if obj.get_dict("ImageMask").ok().flatten().and_then(|v| v.as_bool().ok()) == Some(true) {
        return Err("its picture is a stencil".into());
    }
    if !plain_space(obj) {
        return Err("its picture uses a colour space that is not gray or RGB".into());
    }
    let filters = filter_names(obj);
    if filters.iter().any(|f| f == "JPXDecode") {
        return Err("its picture is JPEG 2000".into());
    }
    let s = imageedit::decoded_samples(obj).map_err(|e| e.to_string())?;
    let (w, h) = (s.w as usize, s.h as usize);
    let invert = decode_inverted(obj).ok_or_else(|| "its picture has an unusual decode table".to_string())?;
    match (s.bpc, s.n) {
        (1, 1) => {
            let mut px = Vec::with_capacity(w * h);
            for y in 0..h {
                let row = &s.data[y * s.row..(y + 1) * s.row];
                for x in 0..w {
                    let bit = (row[x / 8] >> (7 - x % 8)) & 1;
                    px.push(if (bit == 1) != invert { 255 } else { 0 });
                }
            }
            Ok((Raster { w, h, n: 1, px }, Family::Bilevel))
        }
        (8, 1 | 3) if !invert => {
            let n = s.n as usize;
            let mut px = Vec::with_capacity(w * h * n);
            for y in 0..h {
                px.extend_from_slice(&s.data[y * s.row..y * s.row + w * n]);
            }
            let family = if filters.last().is_some_and(|f| f == "DCTDecode") {
                // Only a plain DCT stream can be read for its tables; behind another filter it is not JPEG yet.
                let raw = if filters.len() == 1 { obj.read_raw_stream().ok() } else { None };
                let q = raw.as_deref().and_then(jpeg_quality).unwrap_or(JPEG_QUALITY);
                Family::Jpeg(q.clamp(JPEG_QUALITY_RANGE.0, JPEG_QUALITY_RANGE.1))
            } else {
                Family::Raw
            };
            Ok((Raster { w, h, n, px }, family))
        }
        _ => Err(format!("its picture has {} bit(s) and {} component(s) per pixel", s.bpc, s.n)),
    }
}

/// The IJG quality (1..=100) the first quantisation table of a JPEG stands for, from the size of its
/// entries against the standard luminance table. Rough for tables an encoder made its own way, which
/// is enough to write the copy at a similar quality. `None` when no table is found before the scan.
pub(crate) fn jpeg_quality(data: &[u8]) -> Option<u8> {
    const STD: [u32; 64] = [
        16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68,
        109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
    ];
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if len < 2 || i + 2 + len > data.len() {
            return None;
        }
        if marker == 0xDB {
            let seg = &data[i + 4..i + 2 + len];
            let wide = seg.first()? >> 4 != 0;
            let step = if wide { 2 } else { 1 };
            let table = seg.get(1..1 + 64 * step)?;
            let sum: u32 = if wide { table.chunks(2).map(|p| u32::from(u16::from_be_bytes([p[0], p[1]]))).sum() } else { table.iter().map(|&v| u32::from(v)).sum() };
            let scale = sum * 100 / STD.iter().sum::<u32>();
            let q = if scale <= 100 { (200 - scale.min(200)).div_ceil(2) } else { 5000 / scale };
            return Some(q.clamp(1, 100) as u8);
        }
        i += 2 + len;
    }
    None
}

fn flate(data: &[u8]) -> Vec<u8> {
    use flate2::{Compression, write::ZlibEncoder};
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(data).expect("write to Vec");
    z.finish().expect("finish Vec")
}

/// Writes `ras` as a new image XObject shaped like `orig` (its other keys are kept) in `family`.
pub(crate) fn encode(doc: &mut PdfDocument, orig: &PdfObject, family: Family, ras: &Raster) -> Result<PdfObject, OcrError> {
    let mut dict = orig.copy_dict().map_err(mupdf_err)?;
    for k in ["Filter", "DecodeParms", "Length", "Decode", "SMask", "Mask"] {
        dict.dict_delete(k).map_err(mupdf_err)?;
    }
    let int = |v: usize| PdfObject::new_int(v as i32).map_err(mupdf_err);
    let name = |v: &str| PdfObject::new_name(v).map_err(mupdf_err);
    dict.dict_put("Width", int(ras.w)?).map_err(mupdf_err)?;
    dict.dict_put("Height", int(ras.h)?).map_err(mupdf_err)?;
    let data = match family {
        Family::Jpeg(quality) => {
            use image::{ExtendedColorType, ImageEncoder, codecs::jpeg::JpegEncoder};
            let mut out = Vec::new();
            let color = if ras.n == 1 { ExtendedColorType::L8 } else { ExtendedColorType::Rgb8 };
            JpegEncoder::new_with_quality(&mut out, quality)
                .write_image(&ras.px, ras.w as u32, ras.h as u32, color)
                .map_err(|e| OcrError::Pdf(e.to_string()))?;
            dict.dict_put("Filter", name("DCTDecode")?).map_err(mupdf_err)?;
            dict.dict_put("BitsPerComponent", int(8)?).map_err(mupdf_err)?;
            out
        }
        Family::Bilevel => {
            let (packed, stride) = raster::pack_bilevel(&ras.luma(), ras.w, ras.h, 128);
            let g4 = Buffer::ccitt_g4(&packed, ras.w as u32, ras.h as u32, stride).map_err(mupdf_err)?;
            dict.dict_put("Filter", name("CCITTFaxDecode")?).map_err(mupdf_err)?;
            let parms = doc.new_object_from_str(&format!("<< /K -1 /Columns {} /Rows {} >>", ras.w, ras.h)).map_err(mupdf_err)?;
            dict.dict_put("DecodeParms", parms).map_err(mupdf_err)?;
            dict.dict_put("ColorSpace", name("DeviceGray")?).map_err(mupdf_err)?;
            dict.dict_put("BitsPerComponent", int(1)?).map_err(mupdf_err)?;
            g4
        }
        Family::Raw => {
            dict.dict_put("Filter", name("FlateDecode")?).map_err(mupdf_err)?;
            dict.dict_put("BitsPerComponent", int(8)?).map_err(mupdf_err)?;
            flate(&ras.px)
        }
    };
    let buf = Buffer::from_bytes(&data).map_err(mupdf_err)?;
    doc.add_stream(&buf, Some(&dict), true).map_err(mupdf_err)
}

/// Points the page's own copy of its resources at `new` under `name` (the original may be shared
/// with other pages or inherited).
pub(crate) fn rebind(doc: &mut PdfDocument, page: &mut PdfPage, name: &[u8], new: PdfObject) -> Result<(), OcrError> {
    let mut page_obj = page.object();
    let res = page_obj.get_dict_inheritable("Resources").map_err(mupdf_err)?;
    let mut res = match res {
        Some(r) if r.is_dict().map_err(mupdf_err)? => r.copy_dict().map_err(mupdf_err)?,
        _ => doc.new_dict().map_err(mupdf_err)?,
    };
    let mut xo = match res.get_dict("XObject").map_err(mupdf_err)? {
        Some(x) if x.is_dict().map_err(mupdf_err)? => x.copy_dict().map_err(mupdf_err)?,
        _ => doc.new_dict().map_err(mupdf_err)?,
    };
    xo.dict_put(String::from_utf8_lossy(name).as_ref(), new).map_err(mupdf_err)?;
    res.dict_put("XObject", xo).map_err(mupdf_err)?;
    page_obj.dict_put("Resources", res).map_err(mupdf_err)
}

/// The page's picture, if it has exactly one drawn the plain way and it fills the page: its
/// placement (pixel space to displayed page), the image object and its XObject name.
pub(crate) struct Scan {
    pub name: Vec<u8>,
    pub obj: PdfObject,
    /// Pixel coordinates (x right, y down) to the displayed page.
    pub page_from_pixel: Matrix,
    /// Pixels per inch along the picture's width.
    pub dpi: f32,
}

/// Some scanners and OCR tools write every page with an `/SMask` that is fully opaque. Dropping it
/// changes nothing on screen, so such a picture still counts as plain. Anything else (a real
/// transparency, a stencil `/Mask`, a matte, an unusual decode) does not.
fn opaque_soft_mask(img: &PdfObject) -> bool {
    let has = |o: &PdfObject, k: &str| o.get_dict(k).ok().flatten().is_some();
    if has(img, "Mask") {
        return false;
    }
    let Some(m) = img.get_dict("SMask").ok().flatten() else { return false };
    if !m.is_stream().unwrap_or(false) || has(&m, "Matte") || has(&m, "Decode") || has(&m, "SMask") || has(&m, "Mask") {
        return false;
    }
    if m.get_dict("ImageMask").ok().flatten().and_then(|v| v.as_bool().ok()) == Some(true) {
        return false;
    }
    match imageedit::decoded_samples(&m) {
        Ok(s) if s.bpc == 8 && s.n == 1 && s.w > 0 && s.h > 0 => {
            (0..s.h as usize).all(|y| s.data.get(y * s.row..y * s.row + s.w as usize).is_some_and(|r| r.iter().all(|&v| v == 255)))
        }
        _ => false,
    }
}

pub(crate) fn find_scan(doc: &PdfDocument, page: &PdfPage, index: u32, page_size: [f32; 2]) -> Result<Scan, String> {
    if has_redaction(doc, index) {
        return Err("it has a redaction mark".into());
    }
    let placed = imageedit::placed_images(doc, index).map_err(|e| e.to_string())?;
    let [one] = placed.as_slice() else {
        return Err(if placed.is_empty() { "it has no picture".into() } else { "it has more than one picture".into() });
    };
    if one.inline || one.in_text || one.clipped {
        return Err("its picture is inline, clipped or part of text".into());
    }
    let obj = one.obj.clone().ok_or_else(|| "its picture is inline".to_string())?;
    if one.masked && !opaque_soft_mask(&obj) {
        return Err("its picture has a mask".into());
    }
    let pf = page.ctm().map_err(|e| e.to_string())?;
    let page_from_user = Matrix([pf.a, pf.b, pf.c, pf.d, pf.e, pf.f].map(f64::from));
    let unit_to_page = Matrix(one.ctm).then(&page_from_user);
    let [x0, y0, x1, y1] = unit_to_page.bbox([0.0, 0.0, 1.0, 1.0]);
    let [pw, ph] = page_size.map(f64::from);
    let (ix, iy) = ((x1.min(pw) - x0.max(0.0)).max(0.0), (y1.min(ph) - y0.max(0.0)).max(0.0));
    if pw <= 0.0 || ph <= 0.0 || ix * iy < MIN_COVER * pw * ph {
        return Err("its picture does not fill the page".into());
    }
    let (w, h) = (one.width, one.height);
    if w == 0 || h == 0 {
        return Err("its picture has no pixels".into());
    }
    let pixel_to_unit = Matrix([1.0 / f64::from(w), 0.0, 0.0, -1.0 / f64::from(h), 0.0, 1.0]);
    let page_from_pixel = pixel_to_unit.then(&unit_to_page);
    let (ux, uy) = unit_to_page.apply_vec(1.0, 0.0);
    let width_pt = ux.hypot(uy);
    if width_pt <= 0.0 {
        return Err("its picture has no size".into());
    }
    Ok(Scan { name: one.name.clone(), obj, page_from_pixel, dpi: (f64::from(w) / (width_pt / 72.0)) as f32 })
}

/// Rotation about `c` by `deg` (clockwise on screen, y down): where the straight page's points land
/// on the tilted one, as `PageResult::to_page` has it.
pub(crate) fn turn(deg: f64, c: (f64, f64)) -> Matrix {
    let (s, co) = deg.to_radians().sin_cos();
    Matrix([co, s, -s, co, c.0 - c.0 * co + c.1 * s, c.1 - c.0 * s - c.1 * co])
}

fn clean_page(doc: &mut PdfDocument, r: &mut PageResult) -> Result<Outcome, OcrError> {
    let mut page = doc.load_pdf_page(r.page as i32).map_err(mupdf_err)?;
    let scan = match find_scan(doc, &page, r.page, r.page_size) {
        Ok(s) => s,
        Err(why) => return Ok(Outcome::Left(why)),
    };
    let (mut ras, family) = match decode(&scan.obj) {
        Ok(x) => x,
        Err(why) => return Ok(Outcome::Left(why)),
    };
    let l = ras.luma();
    let thr = raster::otsu(&l);
    let paper = raster::paper(&ras, &l, thr);

    // Straighten, when the engine saw a tilt and the picture's own lines agree. A page that already had
    // text (read anyway because "skip pages with text" was off) keeps its picture's geometry: that
    // text is placed on the tilted picture, and straightening it would put it beside its words.
    let had_text = super::page_has_text(doc, r.page).unwrap_or(true);
    let mut deskewed = false;
    if let Some(deg) = r.skew_deg.filter(|d| !had_text && (MIN_DESKEW..=MAX_DESKEW).contains(&d.abs()) && r.rotation == 0) {
        let better = raster::row_sharpness(&ras, deg) > raster::row_sharpness(&ras, 0.0) * 1.02;
        if better {
            let centre = (f64::from(r.page_size[0]) / 2.0, f64::from(r.page_size[1]) / 2.0);
            let inv = scan.page_from_pixel.invert().ok_or_else(|| OcrError::Pdf("the picture has a singular transform".into()))?;
            // destination pixel -> its page point -> the page point it came from (tilted) -> source pixel
            let src_from_dst = scan.page_from_pixel.then(&turn(f64::from(deg), centre)).then(&inv);
            ras = raster::warp(&ras, &src_from_dst, &paper);
            deskewed = true;
        }
    }
    let specks = raster::despeckle(&mut ras, scan.dpi);
    if !deskewed && specks == 0 {
        return Ok(Outcome::Clean);
    }

    let new = encode(doc, &scan.obj, family, &ras)?;
    rebind(doc, &mut page, &scan.name, new)?;
    if deskewed {
        r.skew_deg = None;
    }
    Ok(Outcome::Cleaned { deskewed, specks })
}
