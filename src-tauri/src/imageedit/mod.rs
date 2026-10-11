// Adapted from pdfcraft (MIT OR Apache-2.0), crates/edit/src/images.rs @ b8ddf71
//! Edit images in place (Acrobat "Edit text & images" for images): list the images a page draws
//! (image XObjects and inline images), and move, resize, rotate, flip, crop, replace or delete
//! one, or add a new one.
//!
//! As in pdfcraft, an image is changed where it is drawn: its `Do` (or inline `BI … EI`) is
//! wrapped in `q … cm … Q` with the extra transform, swapped for another XObject, clipped, or
//! removed; every other operator of the page's content is copied byte for byte. pdfcraft does
//! this on its own COS layer; here the page, its resources and the new image streams are MuPDF
//! objects, and new images are made by MuPDF (`pdf_add_image`: JPEG data is kept as it is, a PNG's
//! alpha becomes an `/SMask`).
//!
//! Every change is one `Engine::apply_removing` step: one undo step, kept by Save, which then
//! writes the file in full without unreferenced objects. A page stops naming an image it no
//! longer draws (its `/XObject` entry is dropped from page-local resources), so a removed or
//! replaced image that nothing else uses is not in the saved file. Coordinates in and out are the
//! viewer's page space (points, origin top left, `/Rotate` applied), like `text_blocks`.
//! See docs/image-edit.md.

pub mod app;

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::textedit::content::{Matrix, Obj, Op, Pieces, inline_dict, num};
use crate::textedit::page_space;
use crate::textedit::text::{page_streams, splice, write_streams};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Buffer, Image};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;

type Result<T> = std::result::Result<T, EngineError>;

fn invalid(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

fn get(o: &PdfObject, key: &str) -> Option<PdfObject> {
    o.get_dict(key).ok().flatten()
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImageKind {
    /// An image XObject drawn with `Do`.
    XObject,
    /// An inline image (`BI … ID … EI`).
    Inline,
}

/// One image drawn on a page.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageBlock {
    /// Pass to [`edit`] with the `rev` of the list it came from.
    pub index: u32,
    pub kind: ImageKind,
    /// What shows, in page space: the image's box cut by a rectangular clip around it.
    pub rect: [f32; 4],
    /// The whole image's box in page space (larger than `rect` when it is cropped by a clip).
    pub full_rect: [f32; 4],
    /// The placement: the image's unit square → user space (the CTM at the `Do`).
    pub matrix: [f32; 6],
    /// Pixel size.
    pub width: u32,
    pub height: u32,
    /// `DeviceRGB`, `ICCBased`, `Indexed`, … (`ImageMask` for a stencil mask).
    pub color_space: String,
    pub bits_per_component: u32,
    /// The stream's filters, outermost first (`DCTDecode`, `FlateDecode`, …).
    pub filter: Vec<String>,
    /// Has an `/SMask` or `/Mask`.
    pub masked: bool,
    /// The resource name and object number of an XObject.
    pub name: Option<String>,
    pub xref: Option<i32>,
    pub editable: bool,
    /// Why it can't be changed.
    pub reason: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageBlocks {
    /// The generation the list belongs to.
    pub rev: u32,
    pub images: Vec<ImageBlock>,
}

/// A change to one image (or a new one). Page space; `file` is a path (the command swaps an open
/// token from `pick_source` for it).
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ImageChange {
    Move {
        dx: f64,
        dy: f64,
    },
    /// Fit the image's box to `rect`; with `keep_aspect` the box keeps its proportions, as large
    /// as fits, from `rect`'s top left corner.
    Resize {
        rect: [f64; 4],
        #[serde(default)]
        keep_aspect: bool,
    },
    /// Quarter turns clockwise (as seen) about the centre: 1, 2 or 3 (negative turns back).
    Rotate {
        quarter_turns: i32,
    },
    /// Mirror left-right (`horizontal`) or upside down, about the centre.
    Flip {
        horizontal: bool,
    },
    Delete,
    /// Draw this image file instead, fitted into the old box (keeping its own proportions).
    Replace {
        file: String,
    },
    /// Show only `rect`: a clip, or with `pixels` a new image of just those pixels.
    Crop {
        rect: [f64; 4],
        #[serde(default)]
        pixels: bool,
    },
    /// A new image in `rect` (no index); with `keep_aspect` fitted inside it.
    Add {
        file: String,
        rect: [f64; 4],
        #[serde(default = "yes")]
        keep_aspect: bool,
    },
}

fn yes() -> bool {
    true
}

impl ImageChange {
    fn label(&self, page: u32) -> String {
        let what = match self {
            ImageChange::Move { .. } => "Move image",
            ImageChange::Resize { .. } => "Resize image",
            ImageChange::Rotate { .. } => "Rotate image",
            ImageChange::Flip { .. } => "Flip image",
            ImageChange::Delete => "Delete image",
            ImageChange::Replace { .. } => "Replace image",
            ImageChange::Crop { .. } => "Crop image",
            ImageChange::Add { .. } => "Add image",
        };
        format!("{what} on p. {}", page + 1)
    }

    /// The image file this change reads, if any.
    pub fn file_mut(&mut self) -> Option<&mut String> {
        match self {
            ImageChange::Replace { file } | ImageChange::Add { file, .. } => Some(file),
            _ => None,
        }
    }
}

// ---- reading -----------------------------------------------------------------------------------

/// An image as found in the content.
#[derive(Clone, Debug)]
struct Found {
    /// Operator index (in the page's streams parsed joined).
    op: usize,
    kind: ImageKind,
    /// The CTM at the `Do`.
    ctm: Matrix,
    /// The clip box in user space, if a rectangle clips it.
    clip: Option<[f64; 4]>,
    in_text: bool,
    name: Option<Vec<u8>>,
    obj: Option<PdfObject>,
    width: u32,
    height: u32,
    color_space: String,
    bpc: u32,
    filter: Vec<String>,
    masked: bool,
}

struct PageImages {
    page: PdfObject,
    streams: Vec<(PdfObject, Vec<u8>)>,
    pieces: Pieces,
    ops: Vec<Op>,
    /// The CTM in effect before each operator.
    ctms: Vec<Matrix>,
    found: Vec<Found>,
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn intersect(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

fn name_str(o: &Obj) -> String {
    match o {
        Obj::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Obj::Array(a) => a.first().map(name_str).unwrap_or_default(),
        _ => String::new(),
    }
}

/// Inline colour space abbreviations (ISO 32000-2 Table 92).
fn inline_space(o: &Obj) -> String {
    match name_str(o).as_str() {
        "G" => "DeviceGray".into(),
        "RGB" => "DeviceRGB".into(),
        "CMYK" => "DeviceCMYK".into(),
        "I" => "Indexed".into(),
        other => other.into(),
    }
}

fn inline_filter(o: &Obj) -> Vec<String> {
    let one = |s: String| match s.as_str() {
        "AHx" => "ASCIIHexDecode".into(),
        "A85" => "ASCII85Decode".into(),
        "LZW" => "LZWDecode".into(),
        "Fl" => "FlateDecode".into(),
        "RL" => "RunLengthDecode".into(),
        "CCF" => "CCITTFaxDecode".into(),
        "DCT" => "DCTDecode".into(),
        _ => s,
    };
    match o {
        Obj::Array(a) => a.iter().map(|x| one(name_str(x))).collect(),
        Obj::Name(_) => vec![one(name_str(o))],
        _ => Vec::new(),
    }
}

fn pdf_names(o: Option<PdfObject>) -> Vec<String> {
    let Some(o) = o else { return Vec::new() };
    let name = |x: &PdfObject| x.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned());
    if o.is_array().unwrap_or(false) {
        (0..o.len().unwrap_or(0) as i32).filter_map(|i| o.get_array(i).ok().flatten()).filter_map(|x| name(&x)).collect()
    } else {
        name(&o).into_iter().collect()
    }
}

fn int_of(o: Option<PdfObject>) -> u32 {
    o.and_then(|v| v.as_int().ok()).unwrap_or(0).max(0) as u32
}

fn space_of(img: &PdfObject) -> String {
    if get(img, "ImageMask").and_then(|v| v.as_bool().ok()) == Some(true) {
        return "ImageMask".into();
    }
    match get(img, "ColorSpace") {
        Some(cs) if cs.is_array().unwrap_or(false) => cs
            .get_array(0)
            .ok()
            .flatten()
            .and_then(|n| n.as_name().ok())
            .map(|n| String::from_utf8_lossy(&n).into_owned())
            .unwrap_or_default(),
        Some(cs) => cs.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned()).unwrap_or_default(),
        None => String::new(),
    }
}

fn read_images(doc: &PdfDocument, page: u32) -> Result<PageImages> {
    let count = doc.page_count()?.max(0) as u32;
    if page >= count {
        return Err(EngineError::BadPage(page));
    }
    let p = doc.find_page(page as i32)?;
    let res = p.get_dict_inheritable("Resources")?;
    let xobjects = res.as_ref().and_then(|r| get(r, "XObject"));
    let streams = page_streams(&p)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse();
    let mut found = Vec::new();
    // Graphics state: CTM and clip box; the streams are one stream (§7.8.2).
    let mut ctm = Matrix::IDENTITY;
    let mut clip: Option<[f64; 4]> = None;
    let mut stack = Vec::new();
    // The current path's rectangles (their union, in user space), and whether it has other parts.
    let mut path: Option<[f64; 4]> = None;
    let mut path_other = false;
    let mut pending_clip = false;
    let mut in_text = false;
    let mut ctms = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        ctms.push(ctm);
        match op.op.as_slice() {
            b"q" => stack.push((ctm, clip)),
            b"Q" => (ctm, clip) = stack.pop().unwrap_or((ctm, clip)),
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    ctm = Matrix(m).then(&ctm);
                }
            }
            b"BT" => in_text = true,
            b"ET" => in_text = false,
            b"re" => match op.nums::<4>() {
                // Only rectangles are tracked as clips (several: their union); anything else in
                // the path clears the guess.
                Some([x, y, w, h]) => {
                    let r = ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                    path = Some(path.map_or(r, |p| union(p, r)));
                }
                None => path_other = true,
            },
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" => path_other = true,
            b"W" | b"W*" => pending_clip = true,
            b"n" | b"f" | b"F" | b"f*" | b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                if pending_clip && !path_other {
                    if let Some(r) = path {
                        clip = Some(clip.map_or(r, |c| intersect(c, r)));
                    }
                }
                pending_clip = false;
                path = None;
                path_other = false;
            }
            b"Do" => {
                let Some(name) = op.name(0) else { continue };
                let Some(x) = xobjects.as_ref().and_then(|x| get(x, &String::from_utf8_lossy(name))) else { continue };
                if get(&x, "Subtype").and_then(|s| s.as_name().ok()).as_deref() != Some(b"Image") {
                    continue;
                }
                found.push(Found {
                    op: i,
                    kind: ImageKind::XObject,
                    ctm,
                    clip,
                    in_text,
                    name: Some(name.to_vec()),
                    width: int_of(get(&x, "Width")),
                    height: int_of(get(&x, "Height")),
                    color_space: space_of(&x),
                    bpc: int_of(get(&x, "BitsPerComponent")).max(u32::from(space_of(&x) == "ImageMask")),
                    filter: pdf_names(get(&x, "Filter")),
                    masked: get(&x, "SMask").is_some() || get(&x, "Mask").is_some(),
                    obj: Some(x),
                });
            }
            b"BI" => {
                let d = inline_dict(&pieces.data()[op.span.clone()]);
                let val = |k: &[u8]| d.iter().find(|(key, _)| key == k).map(|(_, v)| v);
                let n = |k: &[u8]| val(k).and_then(Obj::as_f64).unwrap_or(0.0).max(0.0) as u32;
                let mask = matches!(val(b"ImageMask"), Some(Obj::Bool(true)));
                found.push(Found {
                    op: i,
                    kind: ImageKind::Inline,
                    ctm,
                    clip,
                    in_text,
                    name: None,
                    obj: None,
                    width: n(b"Width"),
                    height: n(b"Height"),
                    color_space: if mask { "ImageMask".into() } else { val(b"ColorSpace").map(inline_space).unwrap_or_default() },
                    bpc: if mask { 1 } else { n(b"BitsPerComponent") },
                    filter: val(b"Filter").map(inline_filter).unwrap_or_default(),
                    masked: false,
                });
            }
            _ => {}
        }
    }
    Ok(PageImages { page: p, streams, pieces, ops, ctms, found })
}

const UNIT: [f64; 4] = [0.0, 0.0, 1.0, 1.0];

fn rect32(m: &Matrix, r: [f64; 4]) -> [f32; 4] {
    m.bbox(r).map(|v| v as f32)
}

/// The images `page` (0-based) draws, in drawing order.
pub fn blocks(eng: &Engine, doc: DocId, page: u32) -> Result<ImageBlocks> {
    let entry = eng.doc_entry(doc)?;
    eng.read_custom(doc, |d| {
        let rev = entry.generation();
        let pi = read_images(d, page)?;
        let m = page_space(d, page)?;
        let images = pi
            .found
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let full = f.ctm.bbox(UNIT);
                let shown = f.clip.map_or(full, |c| intersect(full, c));
                let reason = (f.in_text).then(|| "it is drawn inside a text object".to_string());
                ImageBlock {
                    index: i as u32,
                    kind: f.kind,
                    rect: rect32(&m, shown),
                    full_rect: rect32(&m, full),
                    matrix: f.ctm.0.map(|v| v as f32),
                    width: f.width,
                    height: f.height,
                    color_space: f.color_space.clone(),
                    bits_per_component: f.bpc,
                    filter: f.filter.clone(),
                    masked: f.masked,
                    name: f.name.as_ref().map(|n| String::from_utf8_lossy(n).into_owned()),
                    xref: f.obj.as_ref().and_then(|o| o.as_indirect().ok()).filter(|n| *n > 0),
                    editable: reason.is_none(),
                    reason,
                }
            })
            .collect();
        Ok(ImageBlocks { rev, images })
    })
}

// ---- changing ----------------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageEdited {
    pub state: EditState,
}

/// The page-local `/XObject` dict: the page's resources and their `/XObject` dict are copied onto
/// the page first (shallowly) when they are inherited, shared or indirect, so removing a name
/// never touches another page.
fn local_xobjects(doc: &mut PdfDocument, page: &PdfObject) -> Result<PdfObject> {
    let mut page = page.clone();
    let res = match page.get_dict_inheritable("Resources")? {
        Some(r) if r.is_dict()? => r.copy_dict()?,
        _ => doc.new_dict()?,
    };
    page.dict_put("Resources", res)?;
    let mut res = get(&page, "Resources").ok_or_else(|| invalid("the page has no resources"))?;
    let xo = match get(&res, "XObject") {
        Some(x) if x.is_dict()? => x.copy_dict()?,
        _ => doc.new_dict()?,
    };
    res.dict_put("XObject", xo)?;
    get(&res, "XObject").ok_or_else(|| invalid("the page has no XObjects"))
}

/// Adds image object `img` to the page's XObjects under a new name.
fn add_resource(doc: &mut PdfDocument, page: &PdfObject, img: &PdfObject) -> Result<Vec<u8>> {
    let mut xo = local_xobjects(doc, page)?;
    let name = (0..).map(|i| format!("IwImg{i}")).find(|n| get(&xo, n).is_none()).expect("a free name");
    xo.dict_put(name.as_str(), img.clone())?;
    Ok(name.into_bytes())
}

/// Drops the page's XObject entries for `names` the page's content no longer draws, so an image
/// nothing else uses goes with the next full save.
fn forget_unused(doc: &mut PdfDocument, page: u32, names: &HashSet<Vec<u8>>) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let p = doc.find_page(page as i32)?;
    let streams = page_streams(&p)?;
    let ops = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>()).parse();
    let used: HashSet<&[u8]> = ops.iter().filter(|o| o.is("Do")).filter_map(|o| o.name(0)).collect();
    let gone: Vec<&Vec<u8>> = names.iter().filter(|n| !used.contains(n.as_slice())).collect();
    if gone.is_empty() {
        return Ok(());
    }
    // A form XObject without its own resources draws with the page's (PDF 1.1): keep names then.
    let res = p.get_dict_inheritable("Resources")?;
    let xobjects = res.as_ref().and_then(|r| get(r, "XObject"));
    if let Some(x) = &xobjects {
        for i in 0..x.dict_len()? as i32 {
            if let Some(v) = x.get_dict_val(i)? {
                let form = get(&v, "Subtype").and_then(|s| s.as_name().ok()).as_deref() == Some(b"Form");
                if form && get(&v, "Resources").is_none() {
                    return Ok(());
                }
            }
        }
    }
    let mut xo = local_xobjects(doc, &p)?;
    for n in gone {
        xo.dict_delete(String::from_utf8_lossy(n).as_ref())?;
    }
    Ok(())
}

fn flate(data: &[u8]) -> Result<Vec<u8>> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(data).map_err(|e| EngineError::Io(e.to_string()))?;
    z.finish().map_err(|e| EngineError::Io(e.to_string()))
}

/// The largest image file read for inserting.
const MAX_FILE: u64 = 256 << 20;

/// A new image XObject from an image file (JPEG kept as it is; alpha becomes an /SMask).
fn load_image(doc: &mut PdfDocument, path: &str) -> Result<(PdfObject, u32, u32)> {
    let size = std::fs::metadata(path).map_err(|e| EngineError::Io(e.to_string()))?.len();
    if size > MAX_FILE {
        return Err(invalid("the image file is too large"));
    }
    let bytes = std::fs::read(path).map_err(|e| EngineError::Io(e.to_string()))?;
    // MuPDF reads the header here; the pixels are decoded only when the image is added, and only
    // after the size check below.
    let img = Image::from_bytes(&bytes).map_err(|_| invalid("that file is not an image Inkwell can read"))?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Err(invalid("the image is empty"));
    }
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(invalid("the image has too many pixels (more than 100 megapixels)"));
    }
    Ok((doc.add_image(&img)?, w, h))
}

/// The matrix that maps the unit square onto `rect` (user space, upright).
fn unit_to(rect: [f64; 4]) -> Matrix {
    Matrix([rect[2] - rect[0], 0.0, 0.0, rect[3] - rect[1], rect[0], rect[1]])
}

/// The page-space transform mapping box `from` onto box `to`.
fn rect_to_rect(from: [f64; 4], to: [f64; 4]) -> Matrix {
    let (fw, fh) = ((from[2] - from[0]).max(1e-6), (from[3] - from[1]).max(1e-6));
    let (sx, sy) = ((to[2] - to[0]) / fw, (to[3] - to[1]) / fh);
    Matrix([sx, 0.0, 0.0, sy, to[0] - from[0] * sx, to[1] - from[1] * sy])
}

/// A page-space rect, ordered.
fn ordered(r: [f64; 4]) -> Result<[f64; 4]> {
    if !r.iter().all(|v| v.is_finite()) {
        return Err(invalid("the rectangle must be finite"));
    }
    let r = [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])];
    if r[2] - r[0] < 0.01 || r[3] - r[1] < 0.01 {
        return Err(invalid("the rectangle is empty"));
    }
    Ok(r)
}

/// Keeps `w × h` proportions inside `rect`, from its top left corner (page space).
fn fit_top_left(rect: [f64; 4], w: f64, h: f64) -> [f64; 4] {
    let s = ((rect[2] - rect[0]) / w).min((rect[3] - rect[1]) / h);
    [rect[0], rect[1], rect[0] + w * s, rect[1] + h * s]
}

/// The unit-square sub-rectangle an image of aspect `aspect` (w / h) takes when fitted, centred,
/// into a placement whose sides are `sx` and `sy` long.
fn fit_in_unit(aspect: f64, sx: f64, sy: f64) -> [f64; 4] {
    let boxed = sx / sy.max(1e-9);
    if aspect > boxed {
        let k = boxed / aspect;
        [0.0, (1.0 - k) / 2.0, 1.0, (1.0 + k) / 2.0]
    } else {
        let k = aspect / boxed;
        [(1.0 - k) / 2.0, 0.0, (1.0 + k) / 2.0, 1.0]
    }
}

/// Applies `change` to image `index` of `page` (or adds one), listed at generation `rev`: one
/// undo step. `Stale` when the document has changed since that list.
pub fn edit(
    eng: &Engine,
    doc: DocId,
    rev: u32,
    page: u32,
    index: Option<u32>,
    change: &ImageChange,
    source: EditSource,
) -> Result<ImageEdited> {
    let entry = eng.doc_entry(doc)?;
    let state = eng.apply_removing(doc, change.label(page), source, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        apply(d, page, index, change)?;
        Ok(vec![page])
    })?;
    Ok(ImageEdited { state })
}

fn apply(d: &mut PdfDocument, page: u32, index: Option<u32>, change: &ImageChange) -> Result<()> {
    let to_page = page_space(d, page)?;
    let to_user = to_page.invert().ok_or_else(|| invalid("the page has no area"))?;
    let pi = read_images(d, page)?;
    if let ImageChange::Add { file, rect, keep_aspect } = change {
        if index.is_some() {
            return Err(invalid("a new image takes no index"));
        }
        return add(d, &pi, &to_user, file, *rect, *keep_aspect);
    }
    let index = index.ok_or_else(|| invalid("which image?"))? as usize;
    let f = pi.found.get(index).cloned().ok_or_else(|| invalid(format!("page {} has no image {}", page + 1, index + 1)))?;
    if f.in_text && !matches!(change, ImageChange::Delete) {
        return Err(invalid("this image is drawn inside a text object; it can only be deleted"));
    }
    // The image's operator exactly as written (an inline image keeps its parameters and data).
    let original = Op::verbatim(&pi.pieces.data()[pi.ops[f.op].span.clone()]);
    // Page-space transform of the image's box, for move, resize, rotate and flip.
    let full = to_page.bbox(f.ctm.bbox(UNIT));
    let (cx, cy) = ((full[0] + full[2]) / 2.0, (full[1] + full[3]) / 2.0);
    let about_centre = |m: [f64; 4]| Matrix([m[0], m[1], m[2], m[3], 0.0, 0.0]).then(&Matrix::translate(cx, cy));
    let page_t = match change {
        ImageChange::Move { dx, dy } => Some(Matrix::translate(*dx, *dy)),
        ImageChange::Resize { rect, keep_aspect } => {
            let r = ordered(*rect)?;
            let r = if *keep_aspect { fit_top_left(r, full[2] - full[0], full[3] - full[1]) } else { r };
            Some(rect_to_rect(full, r))
        }
        ImageChange::Rotate { quarter_turns } => {
            // Clockwise as seen: page space has y pointing down.
            let m = match quarter_turns.rem_euclid(4) {
                0 => return Err(invalid("a rotation of 0 changes nothing")),
                1 => [0.0, 1.0, -1.0, 0.0],
                2 => [-1.0, 0.0, 0.0, -1.0],
                _ => [0.0, -1.0, 1.0, 0.0],
            };
            Some(Matrix::translate(-cx, -cy).then(&about_centre(m)))
        }
        ImageChange::Flip { horizontal } => {
            let m = if *horizontal { [-1.0, 0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, -1.0] };
            Some(Matrix::translate(-cx, -cy).then(&about_centre(m)))
        }
        _ => None,
    };
    let mut gone = HashSet::new();
    if let Some(t) = page_t {
        // The image and the wrappers around it (its clip crop, earlier transforms) move as one:
        // the new `cm` goes right after the outermost wrapper's `q`, or around the operator.
        let (start, end) = unit_of(&pi.ops, f.op);
        let c = pi.ctms[start];
        // New placement = C · T (T in user space); as a `cm` there: C · T · C⁻¹.
        let t_user = to_page.then(&t).then(&to_user);
        let inv = c.invert().ok_or_else(|| invalid("the image has no area"))?;
        let m = c.then(&t_user).then(&inv);
        if !m.0.iter().all(|v| v.is_finite() && v.abs() < 1e9) {
            return Err(invalid("the image can't be moved that far"));
        }
        let cm = Op::new("cm", m.0.iter().map(|v| num(*v)).collect());
        let new = if start == end {
            let mut wrapped = Some(vec![Op::new("q", vec![]), cm, original, Op::new("Q", vec![])]);
            splice(&pi.pieces, &pi.ops, |i| if i == f.op { (wrapped.take().unwrap_or_default(), false) } else { (Vec::new(), true) })
        } else {
            let mut cm = Some(vec![cm]);
            splice(&pi.pieces, &pi.ops, |i| if i == start + 1 { (cm.take().unwrap_or_default(), true) } else { (Vec::new(), true) })
        };
        return write_streams(d, &pi.page, &pi.streams, new);
    }
    let replacement: Vec<Op> = {
        match change {
            ImageChange::Delete => {
                gone.extend(f.name.clone());
                Vec::new()
            }
            ImageChange::Replace { file } => {
                let (img, w, h) = load_image(d, file)?;
                let name = add_resource(d, &pi.page, &img)?;
                gone.extend(f.name.clone());
                // Fitted into the old placement, keeping the new image's proportions.
                let c = f.ctm.0;
                let (sx, sy) = ((c[0] * c[0] + c[1] * c[1]).sqrt(), (c[2] * c[2] + c[3] * c[3]).sqrt());
                let r = fit_in_unit(f64::from(w) / f64::from(h), sx, sy);
                vec![
                    Op::new("q", vec![]),
                    Op::new("cm", unit_to(r).0.iter().map(|v| num(*v)).collect()),
                    Op::new("Do", vec![Obj::Name(name)]),
                    Op::new("Q", vec![]),
                ]
            }
            ImageChange::Crop { rect, pixels } => {
                // The crop in the image's unit square (the space its Do paints in).
                let r = ordered(*rect)?;
                let inv = f.ctm.invert().ok_or_else(|| invalid("the image has no area"))?;
                let u = to_user.then(&inv).bbox(r);
                let u = intersect(u, UNIT);
                if u[2] - u[0] < 1e-6 || u[3] - u[1] < 1e-6 {
                    return Err(invalid("the crop doesn't overlap the image"));
                }
                if *pixels {
                    let (obj, sub) = pixel_crop(d, &f, u)?;
                    let name = add_resource(d, &pi.page, &obj)?;
                    gone.extend(f.name.clone());
                    vec![
                        Op::new("q", vec![]),
                        Op::new("cm", unit_to(sub).0.iter().map(|v| num(*v)).collect()),
                        Op::new("Do", vec![Obj::Name(name)]),
                        Op::new("Q", vec![]),
                    ]
                } else {
                    vec![
                        Op::new("q", vec![]),
                        Op::new("re", vec![num(u[0]), num(u[1]), num(u[2] - u[0]), num(u[3] - u[1])]),
                        Op::new("W", vec![]),
                        Op::new("n", vec![]),
                        original,
                        Op::new("Q", vec![]),
                    ]
                }
            }
            _ => unreachable!("transforms are handled above"),
        }
    };
    let mut replacement = Some(replacement);
    let new = splice(&pi.pieces, &pi.ops, |i| if i == f.op { (replacement.take().unwrap_or_default(), false) } else { (Vec::new(), true) });
    write_streams(d, &pi.page, &pi.streams, new)?;
    forget_unused(d, page, &gone)
}

/// The operators that draw image operator `op` as a unit: the operator itself, and around it each
/// `q … Q` that holds nothing else but `cm`s and a rectangular clip (`re W n`): the wrappers a
/// transform or a clip crop leave. Returns the first and last operator index (the same for a bare
/// operator); a transform goes right after the first one's `q`, so the clip moves with the image.
fn unit_of(ops: &[Op], op: usize) -> (usize, usize) {
    let (mut lo, mut hi) = (op, op);
    loop {
        if !ops.get(hi + 1).is_some_and(|o| o.is("Q")) {
            break;
        }
        let mut k = lo;
        while k > 0 && matches!(ops[k - 1].op.as_slice(), b"cm" | b"re" | b"W" | b"W*" | b"n") {
            k -= 1;
        }
        if k == 0 || !ops[k - 1].is("q") {
            break;
        }
        (lo, hi) = (k - 1, hi + 1);
    }
    (lo, hi)
}

/// Draws a new image in page-space `rect`, after the page's content (in a clean graphics state).
fn add(d: &mut PdfDocument, pi: &PageImages, to_user: &Matrix, file: &str, rect: [f64; 4], keep_aspect: bool) -> Result<()> {
    let r = ordered(rect)?;
    let (img, w, h) = load_image(d, file)?;
    // Fitted in page space, then taken to user space: the image shows upright on the page.
    let r = if keep_aspect {
        let s = ((r[2] - r[0]) / f64::from(w)).min((r[3] - r[1]) / f64::from(h));
        let (fw, fh) = (f64::from(w) * s, f64::from(h) * s);
        let (x, y) = ((r[0] + r[2] - fw) / 2.0, (r[1] + r[3] - fh) / 2.0);
        [x, y, x + fw, y + fh]
    } else {
        r
    };
    // The unit square's corners in page space: (0,0) is the image's bottom left, so it maps to
    // the rect's bottom left (y down), and (1,1) to its top right.
    let place_page = Matrix([r[2] - r[0], 0.0, 0.0, -(r[3] - r[1]), r[0], r[3]]);
    let m = place_page.then(to_user);
    let name = add_resource(d, &pi.page, &img)?;
    // Whatever state the page's content leaves (a cm, a clip, a colour, q without Q or Q
    // without q) must not reach the new image: the content goes inside q … Q, with as many
    // extra q before it as it closes more than it opens, and as many Q after it as are left open.
    let (mut depth, mut lowest) = (0i64, 0i64);
    for op in &pi.ops {
        match op.op.as_slice() {
            b"q" => depth += 1,
            b"Q" => {
                depth -= 1;
                lowest = lowest.min(depth);
            }
            _ => {}
        }
    }
    let opened = 1 + (-lowest) as usize;
    let left_open = (opened as i64 + depth).max(0) as usize;
    let ops = [
        Op::new("q", vec![]),
        Op::new("cm", m.0.iter().map(|v| num(*v)).collect()),
        Op::new("Do", vec![Obj::Name(name)]),
        Op::new("Q", vec![]),
    ];
    let mut streams = pi.streams.clone();
    let mut new: Vec<Vec<u8>> = streams.iter().map(|(_, b)| b.clone()).collect();
    // New pieces of their own before and after, so the old streams stay as they are.
    streams.insert(0, (PdfObject::new_null(), Vec::new()));
    new.insert(0, b"q\n".repeat(opened));
    let mut tail = b"Q\n".repeat(left_open);
    tail.extend_from_slice(&crate::textedit::content::serialize_ops(&ops));
    streams.push((PdfObject::new_null(), Vec::new()));
    new.push(tail);
    write_streams(d, &pi.page, &streams, new)
}

/// Components per pixel of an image's colour space.
fn components(img: &PdfObject) -> Option<u32> {
    if get(img, "ImageMask").and_then(|v| v.as_bool().ok()) == Some(true) {
        return Some(1);
    }
    let cs = get(img, "ColorSpace")?;
    let family = |n: &[u8]| -> Option<u32> {
        Some(match n {
            b"DeviceGray" | b"CalGray" | b"Indexed" | b"Separation" | b"G" | b"I" => 1,
            b"DeviceRGB" | b"CalRGB" | b"Lab" | b"RGB" => 3,
            b"DeviceCMYK" | b"CMYK" => 4,
            _ => return None,
        })
    };
    if cs.is_array().ok()? {
        let head = cs.get_array(0).ok()??.as_name().ok()?;
        match head.as_slice() {
            b"ICCBased" => {
                cs.get_array(1).ok()?.and_then(|s| get(&s, "N")).and_then(|n| n.as_int().ok()).and_then(|n| u32::try_from(n).ok())
            }
            b"DeviceN" => cs.get_array(1).ok()?.and_then(|a| a.len().ok()).and_then(|n| u32::try_from(n).ok()),
            other => family(other),
        }
        // A real colour space has at most 32 components (DeviceN's limit).
        .filter(|n| (1..=32).contains(n))
    } else {
        family(&cs.as_name().ok()?)
    }
}

/// The most pixels an image may have to be decoded here (a decompression bomb guard).
const MAX_PIXELS: u64 = 100_000_000;

/// Bytes of decoded samples an image of `w × h` pixels, `n` components and `bpc` bits takes, and
/// its row length; None when it is too large to decode.
fn sample_size(w: u32, h: u32, n: u32, bpc: u32) -> Option<(usize, usize)> {
    if u64::from(w).checked_mul(u64::from(h))? > MAX_PIXELS {
        return None;
    }
    let row = u64::from(w).checked_mul(u64::from(n))?.checked_mul(u64::from(bpc))?.div_ceil(8);
    let total = row.checked_mul(u64::from(h))?;
    Some((usize::try_from(total).ok()?, usize::try_from(row).ok()?))
}

/// The decoded samples of `img`, at most `limit` bytes of them. Plain Flate is inflated here with
/// that limit; other filters go through MuPDF, after a check that the raw data is not an extreme
/// expansion of what was declared.
fn samples(img: &PdfObject, limit: usize) -> Result<Vec<u8>> {
    let too_big = || invalid("the image's data is larger than its declared size");
    let filters = pdf_names(get(img, "Filter"));
    let parms = get(img, "DecodeParms").is_some_and(|p| !p.is_null().unwrap_or(true));
    if filters.iter().map(String::as_str).eq(["FlateDecode"]) && !parms {
        use std::io::Read;
        let raw = img.read_raw_stream()?;
        let mut out = Vec::with_capacity(limit.min(raw.len().saturating_mul(4)));
        flate2::read::ZlibDecoder::new(&raw[..])
            .take(limit as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|_| invalid("the image's data is damaged"))?;
        if out.len() > limit {
            return Err(too_big());
        }
        return Ok(out);
    }
    if filters.is_empty() {
        let raw = img.read_raw_stream()?;
        return if raw.len() > limit.saturating_add(4096) { Err(too_big()) } else { Ok(raw) };
    }
    let data = img.read_stream()?;
    if data.len() > limit.saturating_add(4096) {
        return Err(too_big());
    }
    Ok(data)
}

/// Copies the pixels of `img` inside unit-square box `u` into a new image XObject (its SMask or
/// stencil Mask cropped the same way). Returns it and the unit-square box it covers exactly
/// (rounded out to whole pixels).
fn pixel_crop(d: &mut PdfDocument, f: &Found, u: [f64; 4]) -> Result<(PdfObject, [f64; 4])> {
    let obj = f.obj.as_ref().ok_or_else(|| invalid("an inline image can only be cropped with a clip"))?;
    let (w, h) = (f.width, f.height);
    if w == 0 || h == 0 {
        return Err(invalid("the image has no pixels"));
    }
    let x0 = ((u[0] * f64::from(w)).floor() as u32).min(w - 1);
    let x1 = ((u[2] * f64::from(w)).ceil() as u32).clamp(x0 + 1, w);
    let y0 = (((1.0 - u[3]) * f64::from(h)).floor() as u32).min(h - 1);
    let y1 = (((1.0 - u[1]) * f64::from(h)).ceil() as u32).clamp(y0 + 1, h);
    let sub = [
        f64::from(x0) / f64::from(w),
        1.0 - f64::from(y1) / f64::from(h),
        f64::from(x1) / f64::from(w),
        1.0 - f64::from(y0) / f64::from(h),
    ];
    let made = crop_stream(d, obj, sub, true)?;
    Ok((made, sub))
}

/// A cropped copy of image stream `img` covering unit-square box `sub` of it. With `masks`, its
/// `/SMask` or stencil `/Mask` is cropped too (one level only: a mask's own masks are dropped, so a
/// mask that refers back to its image can't recurse).
fn crop_stream(d: &mut PdfDocument, img: &PdfObject, sub: [f64; 4], masks: bool) -> Result<PdfObject> {
    if pdf_names(get(img, "Filter")).iter().any(|f| f == "JPXDecode") {
        return Err(invalid("a JPEG 2000 image can only be cropped with a clip"));
    }
    let (w, h) = (int_of(get(img, "Width")), int_of(get(img, "Height")));
    let bpc = if get(img, "ImageMask").and_then(|v| v.as_bool().ok()) == Some(true) { 1 } else { int_of(get(img, "BitsPerComponent")) };
    let n = components(img).ok_or_else(|| invalid("the image's colour space is not one Inkwell can crop"))?;
    if w == 0 || h == 0 || !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return Err(invalid("the image's pixel format can't be cropped"));
    }
    let (size, row) = sample_size(w, h, n, bpc).ok_or_else(|| invalid("the image is too large to crop by pixels"))?;
    let data = samples(img, size)?;
    if data.len() < size {
        return Err(invalid("the image's data is shorter than its size"));
    }
    let x0 = ((sub[0] * f64::from(w)).round() as u32).min(w - 1);
    let x1 = ((sub[2] * f64::from(w)).round() as u32).clamp(x0 + 1, w);
    let y0 = (((1.0 - sub[3]) * f64::from(h)).round() as u32).min(h - 1);
    let y1 = (((1.0 - sub[1]) * f64::from(h)).round() as u32).clamp(y0 + 1, h);
    let (nw, nh) = (x1 - x0, y1 - y0);
    let px = u64::from(n) * u64::from(bpc);
    let new_row = (u64::from(nw) * px).div_ceil(8) as usize;
    let mut out = vec![0u8; new_row * nh as usize];
    for (ry, y) in (y0..y1).enumerate() {
        let src = &data[y as usize * row..(y as usize + 1) * row];
        let dst = &mut out[ry * new_row..(ry + 1) * new_row];
        if px % 8 == 0 {
            let b = (px / 8) as usize;
            dst[..nw as usize * b].copy_from_slice(&src[x0 as usize * b..x1 as usize * b]);
        } else {
            // Bit by bit for packed samples.
            let start = u64::from(x0) * px;
            for k in 0..u64::from(nw) * px {
                let s = start + k;
                let bit = (src[(s / 8) as usize] >> (7 - s % 8)) & 1;
                dst[(k / 8) as usize] |= bit << (7 - k % 8);
            }
        }
    }
    let mut dict = img.copy_dict()?;
    for k in ["Filter", "DecodeParms", "Length", "SMask", "Mask"] {
        dict.dict_delete(k)?;
    }
    dict.dict_put("Width", PdfObject::new_int(nw as i32)?)?;
    dict.dict_put("Height", PdfObject::new_int(nh as i32)?)?;
    dict.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
    // The masks are cropped to the same part of the picture (they may have their own size).
    for key in ["SMask", "Mask"] {
        match get(img, key) {
            Some(m) if m.is_stream()? => {
                if masks {
                    let cropped = crop_stream(d, &m, sub, false)?;
                    dict.dict_put(key, cropped)?;
                }
            }
            Some(m) => dict.dict_put(key, m)?,
            None => {}
        }
    }
    Ok(d.add_stream(&Buffer::from_bytes(&flate(&out)?)?, Some(&dict), true)?)
}

#[cfg(test)]
mod tests;
