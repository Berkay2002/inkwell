//! Custom stamps (Acrobat's Stamps > Custom Stamps): a library of pictures in the app data directory,
//! and placing one on a page as a `/Stamp` annotation.
//!
//! The library is a small index (`stamps.json`: id, name, category, pixel size) and one PNG per stamp
//! (`<id>.png`, plus `<id>.t.png` for the manager's thumbnail). Every PNG is made here from the decoded
//! pixels of the picked picture (or the rendered page of a picked PDF), never the source bytes, at most
//! [`MAX_SIDE`] pixels on its longest side. The index and the PNGs are read as untrusted: a corrupt index is
//! an empty library with a warning, an id is only ever used when it is in the 16-hex-digit form made here
//! and listed in the index, and every write is a temp file renamed over the old one.
//!
//! Placing one draws the picture in a form XObject with exactly one image under `/Resources /XObject`.
//! MuPDF treats such a stamp as one whose appearance it can rebuild (`pdf_annot_is_standard_stamp`), so a
//! move or resize re-fits the same picture into the new `/Rect`; [`fit_aspect`] keeps its proportions.
//! The assistant has no tool for this (a stamp is often a signature picture, see docs/comments.md).

use super::{Result, bad};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Buffer, Colorspace, Image, Matrix, Pixmap, Rect};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};

/// Stamps the library holds.
pub const MAX_STAMPS: usize = 200;
/// The largest source file read.
const MAX_SOURCE: u64 = 20 << 20;
/// The most pixels a source picture may have before it is decoded (a decompression bomb guard).
const MAX_SOURCE_PIXELS: u64 = 50_000_000;
/// Longest side of a stored picture in pixels: larger ones are scaled down.
pub const MAX_SIDE: u32 = 4096;
pub const MAX_NAME: usize = 128;
pub const MAX_CATEGORY: usize = 64;
/// The default category of a stamp made without one.
pub const DEFAULT_CATEGORY: &str = "Custom";
/// Width a placed stamp gets, in points.
pub const DEFAULT_WIDTH: f32 = 150.0;
/// Resolution a PDF page is rendered at (less where the page would pass [`MAX_SIDE`]).
const PDF_DPI: f32 = 300.0;
const THUMB_SIDE: u32 = 96;
const INDEX: &str = "stamps.json";
/// The index is a few kilobytes; one that is larger than this is not ours.
const MAX_INDEX: u64 = 1 << 20;
/// A stored PNG is at most 4096 x 4096 x 4 bytes uncompressed; a file larger than this is not ours.
const MAX_PNG: u64 = 64 << 20;

/// One writer at a time: a create, rename or delete reads the index and writes it back.
static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    pub id: String,
    pub name: String,
    pub category: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub stamps: Vec<Stamp>,
    /// Set when the index was damaged or had entries that could not be used: shown once as a toast.
    pub warning: Option<String>,
}

#[derive(Deserialize)]
struct IndexFile {
    #[serde(default)]
    stamps: Vec<Stamp>,
}

fn io(e: impl std::fmt::Display) -> EngineError {
    EngineError::Io(e.to_string())
}

fn valid_id(id: &str) -> bool {
    id.len() == 16 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn new_id() -> Result<String> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|e| bad(format!("no random source: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn png_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.png"))
}

fn thumb_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.t.png"))
}

/// Control characters out, ends trimmed, cut to `max` characters (for what the index says).
fn tidy(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(max).collect()
}

/// A name or category typed by the user: tidied, and refused when it is longer than `max` characters.
fn field(what: &str, s: &str, max: usize) -> Result<String> {
    let t = tidy(s, usize::MAX);
    if t.chars().count() > max {
        return Err(bad(format!("{what} is at most {max} characters")));
    }
    Ok(t)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e)
    })
}

// ---- the index -------------------------------------------------------------------------------

/// Reads the index; never fails. A damaged file is set aside as `stamps.json.bad` and reads as empty.
fn load(dir: &Path) -> (Vec<Stamp>, Option<String>) {
    let path = dir.join(INDEX);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), None),
        Err(_) => return (Vec::new(), Some("The custom stamps could not be read.".into())),
    };
    let mut raw = Vec::new();
    let parsed = file
        .take(MAX_INDEX + 1)
        .read_to_end(&mut raw)
        .ok()
        .filter(|_| raw.len() as u64 <= MAX_INDEX)
        .and_then(|_| serde_json::from_slice::<IndexFile>(&raw).ok());
    let Some(index) = parsed else {
        let _ = std::fs::rename(&path, dir.join(format!("{INDEX}.bad")));
        return (Vec::new(), Some("The custom stamps list was damaged, so it starts empty.".into()));
    };
    let total = index.stamps.len();
    let mut seen = std::collections::HashSet::new();
    let stamps: Vec<Stamp> = index
        .stamps
        .into_iter()
        .filter(|s| valid_id(&s.id) && (1..=MAX_SIDE).contains(&s.width) && (1..=MAX_SIDE).contains(&s.height))
        .filter(|s| seen.insert(s.id.clone()))
        .filter(|s| png_path(dir, &s.id).is_file())
        .take(MAX_STAMPS)
        .map(|s| {
            let name = tidy(&s.name, MAX_NAME);
            let category = tidy(&s.category, MAX_CATEGORY);
            Stamp {
                name: if name.is_empty() { "Stamp".into() } else { name },
                category: if category.is_empty() { DEFAULT_CATEGORY.into() } else { category },
                ..s
            }
        })
        .collect();
    let warning = (stamps.len() < total).then(|| "Some custom stamps could not be used and were left out.".to_string());
    (stamps, warning)
}

fn store(dir: &Path, stamps: &[Stamp]) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(io)?;
    let json = serde_json::json!({ "stamps": stamps });
    write_atomic(&dir.join(INDEX), serde_json::to_vec_pretty(&json).map_err(io)?.as_slice())
}

pub fn list(dir: &Path) -> Library {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (stamps, warning) = load(dir);
    Library { stamps, warning }
}

// ---- making a stamp's picture -----------------------------------------------------------------

fn unpremultiply(c: u8, a: u8) -> u8 {
    if a == 255 || a == 0 { c } else { ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8 }
}

/// RGBA pixels (not premultiplied) of a MuPDF pixmap: grey, RGB or CMYK, with or without alpha.
fn from_pixmap(p: &Pixmap) -> Result<image::RgbaImage> {
    let (w, h, n, stride) = (p.width() as usize, p.height() as usize, p.n() as usize, p.stride() as usize);
    let alpha = p.alpha();
    let cs = n - usize::from(alpha);
    if !matches!(cs, 1 | 3 | 4) || w == 0 || h == 0 {
        return Err(bad("that picture's colours are not supported"));
    }
    let s = p.samples();
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = s.get(y * stride..y * stride + w * n).ok_or_else(|| bad("the picture's data is damaged"))?;
        for px in row.chunks_exact(n) {
            let a = if alpha { px[n - 1] } else { 255 };
            let c = |i: usize| unpremultiply(px[i], a);
            let rgb = match cs {
                1 => [c(0); 3],
                3 => [c(0), c(1), c(2)],
                _ => {
                    // CMYK the way MuPDF's own naive conversion goes.
                    let k = 255 - u32::from(c(3));
                    [0, 1, 2].map(|i| ((255 - u32::from(c(i))) * k / 255) as u8)
                }
            };
            out.extend([rgb[0], rgb[1], rgb[2], a]);
        }
    }
    image::RgbaImage::from_raw(w as u32, h as u32, out).ok_or_else(|| bad("the picture's data is damaged"))
}

/// PNG and JPEG through the `image` crate (it applies a JPEG's EXIF orientation); everything else MuPDF reads
/// (TIFF, BMP, GIF, JPEG 2000, ...) through MuPDF.
fn decode_picture(bytes: &[u8]) -> Result<image::RgbaImage> {
    let sniff = bytes.starts_with(b"\x89PNG") || bytes.starts_with(b"\xff\xd8\xff");
    let too_big = || bad("the picture has too many pixels (more than 50 megapixels)");
    if sniff {
        use image::ImageDecoder;
        let bad_picture = |_| bad("that file is not a picture Inkwell can read");
        let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(io)?;
        let mut dec = reader.into_decoder().map_err(bad_picture)?;
        let (w, h) = dec.dimensions();
        if u64::from(w) * u64::from(h) > MAX_SOURCE_PIXELS {
            return Err(too_big());
        }
        let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut img = image::DynamicImage::from_decoder(dec).map_err(bad_picture)?;
        img.apply_orientation(orientation);
        return Ok(img.to_rgba8());
    }
    let img = Image::from_bytes(bytes).map_err(|_| bad("that file is not a picture Inkwell can read"))?;
    if img.width() == 0 || img.height() == 0 {
        return Err(bad("the picture is empty"));
    }
    if u64::from(img.width()) * u64::from(img.height()) > MAX_SOURCE_PIXELS {
        return Err(too_big());
    }
    from_pixmap(&img.to_pixmap()?)
}

/// Page `page` (0-based, the first when None) of a PDF as pixels at [`PDF_DPI`], transparent where the page
/// paints nothing, cut to the part that has something on it.
// shortcut: a page with a white background shape keeps it (only an unpainted page is transparent); cropping to
// the painted part is not the page's own box. Upgrade: a "knock out white" switch if people ask.
fn render_pdf(bytes: &[u8], page: Option<u32>) -> Result<image::RgbaImage> {
    let doc = mupdf::Document::from_bytes(bytes, "application/pdf").map_err(|_| bad("that PDF could not be opened"))?;
    if doc.needs_password()? {
        return Err(bad("that PDF is password protected"));
    }
    let count = doc.page_count()?;
    let p = page.unwrap_or(0) as i32;
    if count < 1 || p >= count {
        return Err(bad(format!("that PDF has {count} page{}", if count == 1 { "" } else { "s" })));
    }
    let page = doc.load_page(p)?;
    let b = page.bounds()?;
    let longest = (b.x1 - b.x0).max(b.y1 - b.y0).max(1.0);
    let scale = (PDF_DPI / 72.0).min(MAX_SIDE as f32 / longest);
    let pix = page.to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), true, true)?;
    if u64::from(pix.width()) * u64::from(pix.height()) > MAX_SOURCE_PIXELS {
        return Err(bad("that page is too large"));
    }
    let img = from_pixmap(&pix)?;
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, px) in img.enumerate_pixels() {
        if px[3] != 0 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    if x1 <= x0 {
        return Err(bad("that page has nothing on it to make a stamp from"));
    }
    Ok(image::imageops::crop_imm(&img, x0, y0, x1 - x0, y1 - y0).to_image())
}

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>> {
    use image::{ExtendedColorType, ImageEncoder};
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(img.as_raw(), img.width(), img.height(), ExtendedColorType::Rgba8).map_err(io)?;
    Ok(out)
}

fn fit_side(img: image::RgbaImage, side: u32) -> image::RgbaImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= side {
        return img;
    }
    let k = f64::from(side) / f64::from(w.max(h));
    let (nw, nh) = (((f64::from(w) * k).round() as u32).max(1), ((f64::from(h) * k).round() as u32).max(1));
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::CatmullRom)
}

/// What to make a stamp from: a picture file, or a page of a PDF.
pub struct Source<'a> {
    pub path: &'a str,
    /// 0-based page of a PDF; the first when None.
    pub page: Option<u32>,
}

pub fn create(dir: &Path, src: &Source, name: &str, category: &str) -> Result<Stamp> {
    let name = field("A stamp's name", name, MAX_NAME)?;
    if name.is_empty() {
        return Err(bad("Give the stamp a name"));
    }
    let category = field("A category", category, MAX_CATEGORY)?;
    let category = if category.is_empty() { DEFAULT_CATEGORY.to_string() } else { category };

    let size = std::fs::metadata(src.path).map_err(io)?.len();
    if size > MAX_SOURCE {
        return Err(bad("that file is larger than 20 MB"));
    }
    let bytes = std::fs::read(src.path).map_err(io)?;
    let img = if bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-") { render_pdf(&bytes, src.page)? } else { decode_picture(&bytes)? };
    let img = fit_side(img, MAX_SIDE);
    let (png, thumb) = (encode_png(&img)?, encode_png(&fit_side(img.clone(), THUMB_SIDE))?);

    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut stamps, _) = load(dir);
    if stamps.len() >= MAX_STAMPS {
        return Err(bad(format!("The library holds {MAX_STAMPS} stamps. Delete one to add another.")));
    }
    std::fs::create_dir_all(dir).map_err(io)?;
    let id = new_id()?;
    let stamp = Stamp { id: id.clone(), name, category, width: img.width(), height: img.height() };
    write_atomic(&png_path(dir, &id), &png)?;
    let done = write_atomic(&thumb_path(dir, &id), &thumb).and_then(|()| {
        stamps.push(stamp.clone());
        store(dir, &stamps)
    });
    if let Err(e) = done {
        let _ = std::fs::remove_file(png_path(dir, &id));
        let _ = std::fs::remove_file(thumb_path(dir, &id));
        return Err(e);
    }
    Ok(stamp)
}

/// Renames a stamp and/or moves it to another category; absent fields stay.
pub fn update(dir: &Path, id: &str, name: Option<&str>, category: Option<&str>) -> Result<Stamp> {
    let name = name.map(|n| field("A stamp's name", n, MAX_NAME)).transpose()?;
    if name.as_deref() == Some("") {
        return Err(bad("Give the stamp a name"));
    }
    let category = category.map(|c| field("A category", c, MAX_CATEGORY)).transpose()?;
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut stamps, _) = load(dir);
    let s = stamps.iter_mut().find(|s| s.id == id).ok_or_else(|| bad("that stamp is not in the library"))?;
    if let Some(n) = name {
        s.name = n;
    }
    if let Some(c) = category {
        s.category = if c.is_empty() { DEFAULT_CATEGORY.into() } else { c };
    }
    let out = s.clone();
    store(dir, &stamps)?;
    Ok(out)
}

pub fn delete(dir: &Path, id: &str) -> Result<()> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut stamps, _) = load(dir);
    let before = stamps.len();
    stamps.retain(|s| s.id != id);
    if stamps.len() == before {
        return Err(bad("that stamp is not in the library"));
    }
    store(dir, &stamps)?;
    let _ = std::fs::remove_file(png_path(dir, id));
    let _ = std::fs::remove_file(thumb_path(dir, id));
    Ok(())
}

fn listed(dir: &Path, id: &str) -> Result<Stamp> {
    if !valid_id(id) {
        return Err(bad("that stamp is not in the library"));
    }
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(dir).0.into_iter().find(|s| s.id == id).ok_or_else(|| bad("that stamp is not in the library"))
}

fn read_capped(path: &Path) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    std::fs::File::open(path).map_err(io)?.take(MAX_PNG + 1).read_to_end(&mut raw).map_err(io)?;
    if raw.len() as u64 > MAX_PNG {
        return Err(bad("that stamp's picture is damaged"));
    }
    Ok(raw)
}

/// The manager's thumbnail of a stamp (PNG bytes).
pub fn thumbnail(dir: &Path, id: &str) -> Result<Vec<u8>> {
    listed(dir, id)?;
    read_capped(&thumb_path(dir, id))
}

/// A stamp and its picture, checked against the index (the file is as untrusted as the index).
pub fn picture(dir: &Path, id: &str) -> Result<(Stamp, Vec<u8>)> {
    let s = listed(dir, id)?;
    let png = read_capped(&png_path(dir, id))?;
    let dims = image::ImageReader::new(Cursor::new(&png)).with_guessed_format().map_err(io)?.into_dimensions();
    if dims.ok() != Some((s.width, s.height)) {
        return Err(bad("that stamp's picture is damaged"));
    }
    Ok((s, png))
}

// ---- placing ---------------------------------------------------------------------------------

/// The box of a stamp of aspect `w / h` centred on `at` (page space, y down): [`DEFAULT_WIDTH`] wide, no larger
/// than the page, moved inside it.
pub fn place_rect(at: [f32; 2], aspect: f32, page: Rect) -> [f32; 4] {
    let (pw, ph) = ((page.x1 - page.x0).max(1.0), (page.y1 - page.y0).max(1.0));
    let aspect = if aspect.is_finite() && aspect > 1e-3 { aspect } else { 1.0 };
    let (mut w, mut h) = (DEFAULT_WIDTH.min(pw), 0.0);
    h = (w / aspect).max(1.0);
    if h > ph {
        h = ph;
        w = (h * aspect).max(1.0);
    }
    let x0 = (at[0] - w / 2.0).clamp(page.x0, page.x1 - w);
    let y0 = (at[1] - h / 2.0).clamp(page.y0, page.y1 - h);
    [x0, y0, x0 + w, y0 + h]
}

/// Draws the library picture `png` of stamp `s` on `page` centred on `at`; returns the annotation's xref.
/// A certified document refuses (unless its `/P` allows annotations); a signed one is added to incrementally
/// by the caller's step, as for every comment.
pub fn place(doc: &mut PdfDocument, page: u32, s: &Stamp, png: &[u8], at: [f32; 2], author: &str) -> Result<i32> {
    crate::outline::refuse_unless_annotating_allowed(doc, "Placing a stamp")?;
    let mut p = doc.load_pdf_page(page as i32)?;
    let rect = place_rect(at, s.width as f32 / s.height as f32, p.bounds()?);
    let img = Image::from_bytes(png).map_err(|_| bad("that stamp's picture is damaged"))?;
    let image = doc.add_image(&img)?;

    let mut xobjects = doc.new_dict()?;
    xobjects.dict_put("I", image)?;
    let mut resources = doc.new_dict()?;
    resources.dict_put("XObject", xobjects)?;
    let mut form = doc.new_dict()?;
    form.dict_put("Type", PdfObject::new_name("XObject")?)?;
    form.dict_put("Subtype", PdfObject::new_name("Form")?)?;
    let mut bbox = doc.new_array()?;
    for v in [0, 0, 1, 1] {
        bbox.array_push(PdfObject::new_int(v)?)?;
    }
    form.dict_put("BBox", bbox)?;
    form.dict_put("Resources", resources)?;
    let ap_n = doc.add_stream(&Buffer::from_bytes(b"/I Do\n")?, Some(&form), false)?;
    let mut ap = doc.new_dict()?;
    ap.dict_put("N", ap_n)?;

    let mut a = p.add_stamp_annotation(super::rect_of(rect), &s.name)?;
    a.object().dict_put("AP", ap)?;
    super::meta(&mut a, author, true)?;
    a.update()?;
    Ok(a.xref()?)
}

/// [`place`] as one undo step.
pub fn place_one(eng: &Engine, doc: DocId, page: u32, s: &Stamp, png: &[u8], at: [f32; 2], author: &str, source: EditSource) -> Result<(EditState, i32)> {
    let mut xref = 0;
    let state = eng.apply_custom(doc, format!("Add stamp on p. {}", page + 1), source, false, |d| {
        xref = place(d, page, s, png, at, author)?;
        Ok(vec![page])
    })?;
    Ok((state, xref))
}

/// Whether the open document takes annotations, and whether it is signed (a stamp is then saved as an update
/// after the signature).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    /// Why a stamp cannot be placed (a certification that allows no annotations), or None.
    pub reason: Option<String>,
    pub signed: bool,
}

pub fn gate(eng: &Engine, doc: DocId) -> Result<Gate> {
    eng.read_custom(doc, |d| {
        let reason = match crate::outline::refuse_unless_annotating_allowed(d, "Placing a stamp") {
            Ok(()) => None,
            Err(EngineError::Invalid(m)) => Some(m),
            Err(e) => return Err(e),
        };
        Ok(Gate { reason, signed: crate::outline::signed(d) })
    })
}

/// `to` (the box a move or resize asked for) cut down to the aspect `aspect` (width / height) of a picture:
/// the largest box of that proportion inside `to`, kept on the side or sides of `from` that did not move
/// (a handle drag keeps the opposite corner), else centred.
pub fn fit_aspect(from: Rect, to: Rect, aspect: f32) -> Rect {
    let (tw, th) = (to.x1 - to.x0, to.y1 - to.y0);
    if !(aspect.is_finite() && aspect > 1e-3) || tw <= 1e-3 || th <= 1e-3 {
        return to;
    }
    let (w, h) = if tw / th > aspect { (th * aspect, th) } else { (tw, tw / aspect) };
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    let place = |lo: f32, hi: f32, from_lo: f32, from_hi: f32, size: f32| {
        if near(lo, from_lo) {
            lo
        } else if near(hi, from_hi) {
            hi - size
        } else {
            (lo + hi) / 2.0 - size / 2.0
        }
    };
    let x0 = place(to.x0, to.x1, from.x0, from.x1, w);
    let y0 = place(to.y0, to.y1, from.y0, from.y1, h);
    Rect { x0, y0, x1: x0 + w, y1: y0 + h }
}

/// The pixel size of the one image a stamp's appearance draws, when it is a picture stamp.
pub fn picture_size(annot: &PdfObject) -> Option<(u32, u32)> {
    let xobjects = annot.get_dict("AP").ok()??.get_dict("N").ok()??.get_dict("Resources").ok()??.get_dict("XObject").ok()??;
    if !xobjects.is_dict().ok()? || xobjects.dict_len().ok()? != 1 {
        return None;
    }
    let img = xobjects.get_dict_val(0).ok()??;
    if !img.is_stream().ok()? || img.get_dict("Subtype").ok()??.as_name().ok()? != b"Image" {
        return None;
    }
    let dim = |k: &str| img.get_dict(k).ok()?.and_then(|v| v.as_int().ok()).and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
    Some((dim("Width")?, dim("Height")?))
}

#[cfg(test)]
pub(crate) mod tests;
