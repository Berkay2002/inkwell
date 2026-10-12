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
use std::collections::{HashMap, HashSet};
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
    /// The turn of the image's bottom edge on the page, in degrees clockwise as seen (-180, 180]:
    /// 0 for an upright image, 90 for one turned a quarter to the right.
    pub angle: f32,
    /// The image's corners in page space (bottom left, bottom right, top right, top left of the
    /// picture itself): the box is this quadrilateral's bounding box, which a turned image fills
    /// only partly.
    pub quad: [f32; 8],
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
    /// The resource names of the form XObjects it is drawn through, outermost first (empty for a picture
    /// the page draws itself). `name` and `xref` are then the picture's entry in the innermost form.
    pub in_form: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageBlocks {
    /// The generation the list belongs to.
    pub rev: u32,
    pub images: Vec<ImageBlock>,
    /// Why the images can't be changed (a certified document): the controls say so.
    pub locked: Option<String>,
    pub signed: bool,
    /// Why the images can't be copied out (the owner's permissions withhold it).
    pub copy_locked: Option<String>,
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
    /// Turn by `degrees` clockwise (as seen) about the centre, any angle; the image keeps its shape.
    Angle {
        degrees: f64,
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
            ImageChange::Rotate { .. } | ImageChange::Angle { .. } => "Rotate image",
            ImageChange::Flip { .. } => "Flip image",
            ImageChange::Delete => "Delete image",
            ImageChange::Replace { .. } => "Replace image",
            ImageChange::Crop { .. } => "Crop image",
            ImageChange::Add { .. } => "Add image",
        };
        format!("{what} on p. {}", page + 1)
    }

    /// The label of this change made to `n` images at once.
    fn label_many(&self, n: usize, page: u32) -> String {
        if n == 1 {
            return self.label(page);
        }
        let what = self.label(page);
        let verb = what.split(" image").next().unwrap_or("Change");
        format!("{verb} {n} images on p. {}", page + 1)
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
    /// The scope (the page, or one drawing of a form) whose content holds the operator.
    scope: usize,
    /// The box in user space the enclosing forms' `/BBox`es leave, if there are any.
    frame: Option<[f64; 4]>,
    name: Option<Vec<u8>>,
    obj: Option<PdfObject>,
    width: u32,
    height: u32,
    color_space: String,
    bpc: u32,
    filter: Vec<String>,
    masked: bool,
}

/// A content stream as read: its pieces, operators and the CTM in effect before each operator.
struct Content {
    streams: Vec<(PdfObject, Vec<u8>)>,
    pieces: Pieces,
    ops: Vec<Op>,
    /// The full CTM before each operator: for a form it starts from the form's `/Matrix` and the CTM at its
    /// `Do`, so the placement of a picture inside a form is its place on the page's user space.
    ctms: Vec<Matrix>,
}

impl Content {
    fn empty() -> Self {
        Content { streams: Vec::new(), pieces: Pieces::join::<&[u8]>(&[]), ops: Vec::new(), ctms: Vec::new() }
    }
}

/// One drawing of a content stream: the page's, or a form XObject's (a form drawn twice is two scopes, each
/// with its own CTM). Scope 0 is the page.
struct Scope {
    /// The page, or the form XObject as its parent's resources name it.
    holder: PdfObject,
    content: Content,
    /// The scope and operator index of the `Do` that draws it, and the resource name it uses there.
    via: Option<(usize, usize, Vec<u8>)>,
    /// The form's object number (0 for the page).
    num: i32,
    /// It has `/Resources` of its own (a page that inherits them, or a form without, has not).
    own_res: bool,
    /// Object numbers of its `/Resources` and `/XObject` dictionaries when they are indirect, else 0.
    res_num: i32,
    xo_num: i32,
    /// Why nothing drawn in it can be changed.
    reason: Option<String>,
    /// The resource names of the forms from the page down to this one.
    names: Vec<String>,
}

struct PageImages {
    page: PdfObject,
    scopes: Vec<Scope>,
    found: Vec<Found>,
}

/// How deep forms are entered, how many form drawings and how many bytes of form content one page reads
/// (the content is untrusted: a form can draw a hundred forms, each drawing a hundred).
const MAX_FORM_DEPTH: usize = 4;
const MAX_FORM_DRAWS: usize = 64;
const MAX_FORM_BYTES: usize = 16 << 20;

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

/// The graphics state a content stream starts from.
#[derive(Clone, Copy)]
struct Start {
    ctm: Matrix,
    clip: Option<[f64; 4]>,
    frame: Option<[f64; 4]>,
    in_text: bool,
}

struct Reader {
    scopes: Vec<Scope>,
    found: Vec<Found>,
    /// Enter form XObjects.
    forms: bool,
    draws: usize,
    bytes: usize,
    /// Object numbers of the forms being read, outermost first (a form that draws itself stops there).
    chain: Vec<i32>,
}

fn nums_of(o: Option<PdfObject>) -> Vec<f64> {
    let Some(a) = o.filter(|a| a.is_array().unwrap_or(false)) else { return Vec::new() };
    (0..a.len().unwrap_or(0) as i32).map(|i| a.get_array(i).ok().flatten().and_then(|x| x.as_float().ok()).map_or(0.0, f64::from)).collect()
}

impl Reader {
    /// Reads `streams` as the content of `holder`: records its images, and (with `forms`) enters the forms it
    /// draws. `xobjects` is the `/XObject` dict its names resolve in. Returns the scope's index.
    fn run(&mut self, mut sc: Scope, streams: Vec<(PdfObject, Vec<u8>)>, xobjects: Option<PdfObject>, start: Start, depth: usize) -> usize {
        let at = self.scopes.len();
        // Reserved, so the pictures found below can name their scope; filled in at the end.
        self.scopes.push(Scope {
            holder: PdfObject::new_null(),
            content: Content::empty(),
            via: None,
            num: 0,
            own_res: false,
            res_num: 0,
            xo_num: 0,
            reason: None,
            names: Vec::new(),
        });
        let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
        let ops = pieces.parse();
        // Graphics state: CTM and clip box; the streams are one stream (§7.8.2).
        let (mut ctm, mut clip, mut in_text) = (start.ctm, start.clip, start.in_text);
        let mut stack = Vec::new();
        // The current path's rectangles (their union, in user space), and whether it has other parts.
        let mut path: Option<[f64; 4]> = None;
        let mut path_other = false;
        let mut pending_clip = false;
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
                // Straight edges count by their box (a turned crop is a parallelogram); curves clear it.
                b"m" | b"l" => match op.nums::<2>() {
                    Some([x, y]) => {
                        let (x, y) = ctm.apply(x, y);
                        path = Some(path.map_or([x, y, x, y], |p| union(p, [x, y, x, y])));
                    }
                    None => path_other = true,
                },
                b"h" => {}
                b"c" | b"v" | b"y" => path_other = true,
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
                    match get(&x, "Subtype").and_then(|s| s.as_name().ok()).as_deref() {
                        Some(b"Image") => self.found.push(Found {
                            op: i,
                            kind: ImageKind::XObject,
                            ctm,
                            clip,
                            in_text,
                            scope: at,
                            frame: start.frame,
                            name: Some(name.to_vec()),
                            width: int_of(get(&x, "Width")),
                            height: int_of(get(&x, "Height")),
                            color_space: space_of(&x),
                            bpc: int_of(get(&x, "BitsPerComponent")).max(u32::from(space_of(&x) == "ImageMask")),
                            filter: pdf_names(get(&x, "Filter")),
                            masked: get(&x, "SMask").is_some() || get(&x, "Mask").is_some(),
                            obj: Some(x),
                        }),
                        Some(b"Form") if self.forms => {
                            let here = Start { ctm, clip, frame: start.frame, in_text };
                            self.form(&x, (at, i, name), &sc, xobjects.as_ref(), here, depth);
                        }
                        _ => {}
                    }
                }
                b"BI" => {
                    let d = inline_dict(&pieces.data()[op.span.clone()]);
                    let val = |k: &[u8]| d.iter().find(|(key, _)| key == k).map(|(_, v)| v);
                    let n = |k: &[u8]| val(k).and_then(Obj::as_f64).unwrap_or(0.0).max(0.0) as u32;
                    let mask = matches!(val(b"ImageMask"), Some(Obj::Bool(true)));
                    self.found.push(Found {
                        op: i,
                        kind: ImageKind::Inline,
                        ctm,
                        clip,
                        in_text,
                        scope: at,
                        frame: start.frame,
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
        sc.content = Content { streams, pieces, ops, ctms };
        self.scopes[at] = sc;
        at
    }

    /// Enters the form `x`, drawn by operator `via.1` of scope `via.0` under resource name `via.2`, at graphics state `st`.
    fn form(&mut self, x: &PdfObject, via: (usize, usize, &[u8]), parent: &Scope, parent_xo: Option<&PdfObject>, st: Start, depth: usize) {
        let num = x.as_indirect().unwrap_or(0).max(0);
        // A form that draws itself (directly or through others) is entered once.
        if depth >= MAX_FORM_DEPTH || self.draws >= MAX_FORM_DRAWS || (num > 0 && self.chain.contains(&num)) {
            return;
        }
        if !x.is_stream().unwrap_or(false) {
            return;
        }
        let Ok(data) = x.read_stream_capped(MAX_FORM_BYTES.saturating_sub(self.bytes)) else { return };
        self.bytes += data.len();
        self.draws += 1;
        let m = nums_of(get(x, "Matrix"));
        let m = if m.len() == 6 { Matrix([m[0], m[1], m[2], m[3], m[4], m[5]]) } else { Matrix::IDENTITY };
        let e = m.then(&st.ctm);
        let b = nums_of(get(x, "BBox"));
        let (mut clip, mut frame) = (st.clip, st.frame);
        if let [x0, y0, x1, y1] = b[..] {
            // The form's `/BBox` clips what it draws: in user space, its box.
            let r = e.bbox([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]);
            clip = Some(clip.map_or(r, |c| intersect(c, r)));
            frame = Some(frame.map_or(r, |f| intersect(f, r)));
        }
        let res = get(x, "Resources").filter(|r| r.is_dict().unwrap_or(false));
        let own_res = res.is_some();
        // A form without resources of its own draws with its parent's (PDF 1.1): readable, not changeable.
        let xo = match &res {
            Some(r) => get(r, "XObject"),
            None => parent_xo.cloned(),
        };
        let reason = parent
            .reason
            .clone()
            .or_else(|| (!own_res).then(|| "it is inside a form that has no resources of its own (PDF 1.1)".to_string()));
        let mut names = parent.names.clone();
        names.push(String::from_utf8_lossy(via.2).into_owned());
        let sc = Scope {
            holder: x.clone(),
            content: Content::empty(),
            via: Some((via.0, via.1, via.2.to_vec())),
            num,
            own_res,
            res_num: get(x, "Resources").and_then(|r| r.as_indirect().ok()).unwrap_or(0).max(0),
            xo_num: res.as_ref().and_then(|r| get(r, "XObject")).and_then(|o| o.as_indirect().ok()).unwrap_or(0).max(0),
            reason,
            names,
        };
        self.chain.push(num);
        // The form's stream is its one content piece; the object stays the same one when it is rewritten.
        self.run(sc, vec![(x.clone(), data)], xo, Start { ctm: e, clip, frame, in_text: st.in_text }, depth + 1);
        self.chain.pop();
    }
}

fn read_images(doc: &PdfDocument, page: u32) -> Result<PageImages> {
    read_images_in(doc, page, true)
}

/// The images of `page`; `forms` also enters the form XObjects it draws (the editor does, the OCR clean-up
/// does not).
fn read_images_in(doc: &PdfDocument, page: u32, forms: bool) -> Result<PageImages> {
    let count = doc.page_count()?.max(0) as u32;
    if page >= count {
        return Err(EngineError::BadPage(page));
    }
    let p = doc.find_page(page as i32)?;
    let res = p.get_dict_inheritable("Resources")?;
    let xobjects = res.as_ref().and_then(|r| get(r, "XObject"));
    let own = get(&p, "Resources");
    let sc = Scope {
        holder: p.clone(),
        content: Content::empty(),
        via: None,
        num: 0,
        own_res: own.is_some(),
        res_num: own.and_then(|r| r.as_indirect().ok()).unwrap_or(0).max(0),
        xo_num: xobjects.as_ref().and_then(|x| x.as_indirect().ok()).unwrap_or(0).max(0),
        reason: None,
        names: Vec::new(),
    };
    let mut r = Reader { scopes: Vec::new(), found: Vec::new(), forms, draws: 0, bytes: 0, chain: Vec::new() };
    let start = Start { ctm: Matrix::IDENTITY, clip: None, frame: None, in_text: false };
    r.run(sc, page_streams(&p)?, xobjects, start, 0);
    Ok(PageImages { page: p, scopes: r.scopes, found: r.found })
}

const UNIT: [f64; 4] = [0.0, 0.0, 1.0, 1.0];

/// The turn of a picture's bottom edge on the page, degrees clockwise as seen, for its placement in
/// page space (y down). A picture shown mirrored (the page's y flip makes an upright one's
/// determinant negative) is measured from its left-right mirror image, so turning it to 0 keeps it
/// mirrored left-right rather than top-bottom.
fn angle_of(placed: &Matrix) -> f32 {
    let [a, b, c, d, ..] = placed.0;
    let (x, y) = if a * d - b * c > 0.0 { (-a, -b) } else { (a, b) };
    y.atan2(x).to_degrees() as f32
}

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
                let sc = &pi.scopes[f.scope];
                let reason = sc.reason.clone().or_else(|| f.in_text.then(|| "it is drawn inside a text object".to_string()));
                let placed = f.ctm.then(&m);
                let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| placed.apply(u, v));
                let mut quad = [0.0f32; 8];
                for (k, (x, y)) in corners.iter().enumerate() {
                    quad[2 * k] = *x as f32;
                    quad[2 * k + 1] = *y as f32;
                }
                ImageBlock {
                    index: i as u32,
                    kind: f.kind,
                    rect: rect32(&m, shown),
                    full_rect: rect32(&m, full),
                    matrix: f.ctm.0.map(|v| v as f32),
                    angle: angle_of(&placed),
                    quad,
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
                    in_form: sc.names.clone(),
                }
            })
            .collect();
        let locked = match crate::outline::refuse_if_certified(d, "Changing its images") {
            Err(EngineError::Invalid(m)) => Some(m),
            _ => None,
        };
        let copy_locked = (!crate::protect::Rights::of(d).may_extract())
            .then(|| "The owner of this document does not allow copying its pictures.".to_string());
        Ok(ImageBlocks { rev, images, locked, signed: crate::outline::signed(d), copy_locked })
    })
}

// ---- changing ----------------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageEdited {
    pub state: EditState,
    /// Something to tell the user about a change that was made: a picture moved out of its form's frame.
    pub warning: Option<String>,
}

/// The holder's own `/XObject` dict (the holder is a page or a form XObject): its resources and their `/XObject`
/// dict are copied onto it first (shallowly) when they are inherited, shared or indirect, so removing or adding
/// a name never touches another page or form.
fn local_xobjects(doc: &mut PdfDocument, page: &PdfObject) -> Result<PdfObject> {
    let mut page = page.clone();
    // A copy of shared or inherited dictionaries makes every form in it have one more user.
    let local = |o: &PdfObject| o.is_dict().unwrap_or(false) && !o.is_indirect().unwrap_or(true);
    let resources = get(&page, "Resources").filter(local);
    if !resources.as_ref().is_some_and(|r| get(r, "XObject").is_some_and(|x| local(&x))) {
        references_added();
    }
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

/// Adds XObject `obj` to the holder's resources under a new name made of `prefix` and a number, one that is no
/// key of its XObjects and not in `avoid` (names some content draws, so a dangling `Do` of it doesn't start
/// drawing the new object).
fn add_named(doc: &mut PdfDocument, holder: &PdfObject, prefix: &str, obj: &PdfObject, avoid: &HashSet<&[u8]>) -> Result<Vec<u8>> {
    let mut xo = local_xobjects(doc, holder)?;
    let name = (0..)
        .map(|i| format!("{prefix}{i}"))
        .find(|n| get(&xo, n).is_none() && !avoid.contains(n.as_bytes()))
        .expect("a free name");
    xo.dict_put(name.as_str(), obj.clone())?;
    Ok(name.into_bytes())
}

/// Adds image object `img` to the holder's XObjects under a new name.
fn add_resource(doc: &mut PdfDocument, holder: &PdfObject, img: &PdfObject) -> Result<Vec<u8>> {
    add_named(doc, holder, "IwImg", img, &HashSet::new())
}

/// The content streams of a page, or the one stream of a form XObject, as they are now.
fn holder_streams(holder: &PdfObject) -> Result<Vec<(PdfObject, Vec<u8>)>> {
    if holder.is_stream()? {
        Ok(vec![(holder.clone(), holder.read_stream()?)])
    } else {
        page_streams(holder)
    }
}

/// Drops the holder's XObject entries for `names` its content no longer draws, so an image nothing else uses goes
/// with the next full save.
fn forget_unused(doc: &mut PdfDocument, p: &PdfObject, names: &HashSet<Vec<u8>>) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let streams = holder_streams(p)?;
    let ops = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>()).parse();
    let used: HashSet<&[u8]> = ops.iter().filter(|o| o.is("Do")).filter_map(|o| o.name(0)).collect();
    let gone: Vec<&Vec<u8>> = names.iter().filter(|n| !used.contains(n.as_slice())).collect();
    if gone.is_empty() {
        return Ok(());
    }
    if shares_resources(p)? {
        return Ok(());
    }
    let mut xo = local_xobjects(doc, p)?;
    for n in gone {
        xo.dict_delete(String::from_utf8_lossy(n).as_ref())?;
    }
    Ok(())
}

/// The holder's resources are also those of a form XObject or a Type3 font that has none of its own (PDF 1.1): its
/// content can draw names the holder's own content doesn't.
fn shares_resources(holder: &PdfObject) -> Result<bool> {
    let Some(res) = holder.get_dict_inheritable("Resources")? else { return Ok(false) };
    for (dict, subtype) in [("XObject", &b"Form"[..]), ("Font", &b"Type3"[..])] {
        let Some(x) = get(&res, dict) else { continue };
        for i in 0..x.dict_len()? as i32 {
            if let Some(v) = x.get_dict_val(i)? {
                let kind = get(&v, "Subtype").and_then(|s| s.as_name().ok());
                if kind.as_deref() == Some(subtype) && get(&v, "Resources").is_none() {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
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
    let mut warning = None;
    let state = eng.apply_removing(doc, change.label(page), source, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        crate::outline::refuse_if_certified(d, "Changing its images")?;
        warning = apply(d, page, index, change, &mut Users::new())?;
        Ok(vec![page])
    })?;
    Ok(ImageEdited { state, warning })
}

/// One image of a batch: its `index` in the list and what is done to it.
#[derive(Deserialize, Clone, Debug)]
pub struct ImageItem {
    pub index: u32,
    pub change: ImageChange,
}

/// The most images one batch changes (each change parses the page's content again). The UI's picks stop at the
/// same number: keep `MAX_PICKS` in src/lib/imageEdit.ts equal to this.
const MAX_BATCH: usize = 100;

/// Applies one change to each of several images of `page` listed at `rev`: one undo step. Moves,
/// turns, flips or deletes only, and all of one kind of "delete or not"; a failing item changes
/// nothing (the step is not taken).
pub fn edit_many(eng: &Engine, doc: DocId, rev: u32, page: u32, items: &[ImageItem], source: EditSource) -> Result<ImageEdited> {
    let (first, rest) = items.split_first().ok_or_else(|| invalid("which images?"))?;
    if items.len() > MAX_BATCH {
        return Err(invalid("too many images at once"));
    }
    let deleting = matches!(first.change, ImageChange::Delete);
    let mut seen = HashSet::new();
    for (i, c) in std::iter::once(first).chain(rest).map(|it| (it.index, &it.change)) {
        if !seen.insert(i) {
            return Err(invalid("an image is named twice"));
        }
        let ok = matches!(c, ImageChange::Move { .. } | ImageChange::Rotate { .. } | ImageChange::Angle { .. } | ImageChange::Flip { .. } | ImageChange::Delete);
        if !ok || matches!(c, ImageChange::Delete) != deleting {
            return Err(invalid("these changes can't be made to several images at once"));
        }
    }
    let mut order: Vec<&ImageItem> = items.iter().collect();
    // Deleting renumbers the images after the one removed: from the last one back.
    if deleting {
        order.sort_by_key(|it| std::cmp::Reverse(it.index));
    }
    let entry = eng.doc_entry(doc)?;
    let mut warning = None;
    let state = eng.apply_removing(doc, first.change.label_many(items.len(), page), source, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        crate::outline::refuse_if_certified(d, "Changing its images")?;
        let mut users = Users::new();
        for it in order {
            let w = apply(d, page, Some(it.index), &it.change, &mut users)?;
            warning = warning.take().or(w);
        }
        Ok(vec![page])
    })?;
    Ok(ImageEdited { state, warning })
}

/// `m`'s linear part keeps the axes of the page: an upright or quarter-turned image.
fn axis_aligned(m: &Matrix) -> bool {
    let [a, b, c, d, ..] = m.0;
    let eps = 1e-6 * (a.abs() + b.abs() + c.abs() + d.abs());
    (b.abs() <= eps && c.abs() <= eps) || (a.abs() <= eps && d.abs() <= eps)
}

/// A `cm` with its numbers kept to eight decimals. The change is written in the image's own unit square,
/// where a turn's entries are small ratios: at four decimals a typed 30 degrees was stored as 30.0007.
fn fine_cm(m: &Matrix) -> Op {
    let n = |v: f64| {
        let s = format!("{v:.8}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s.is_empty() || s == "-" || s == "-0" { "0".to_string() } else { s.to_string() }
    };
    Op::verbatim(format!("{} cm", m.0.map(n).join(" ")).as_bytes())
}

/// The warning for a picture moved out of the frame (`/BBox`) of the form it is drawn in: how much of its box
/// shows before and after, in user space.
fn frame_warning(old: [f64; 4], new: [f64; 4], frame: [f64; 4]) -> Option<String> {
    let shown = |b: [f64; 4]| {
        let area = (b[2] - b[0]) * (b[3] - b[1]);
        let i = intersect(b, frame);
        if area <= 1e-9 { 1.0 } else { (i[2] - i[0]).max(0.0) * (i[3] - i[1]).max(0.0) / area }
    };
    let (was, now) = (shown(old), shown(new));
    if now >= was - 1e-3 {
        None
    } else if now < 1e-3 {
        Some("The picture now lies outside the frame (/BBox) of the form it is drawn in, so it is hidden. Undo brings it back.".into())
    } else {
        Some("Part of the picture now lies outside the frame (/BBox) of the form it is drawn in and is cut off.".into())
    }
}

/// Puts rewritten content back where it came from: a page's `/Contents`, or a form XObject's own stream
/// (the object stays the one the other drawings of it point at).
fn write_content(d: &mut PdfDocument, holder: &PdfObject, sc: &Scope, new: Vec<Vec<u8>>) -> Result<()> {
    if sc.via.is_none() {
        return write_streams(d, holder, &sc.content.streams, new);
    }
    write_form(d, holder, &new.into_iter().next().unwrap_or_default())
}

fn write_form(_d: &mut PdfDocument, form: &PdfObject, data: &[u8]) -> Result<()> {
    let mut f = form.clone();
    f.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
    f.dict_delete("DecodeParms")?;
    f.write_raw_stream_buffer(&Buffer::from_bytes(&flate(data)?)?)?;
    Ok(())
}

/// The most objects the user count of a form looks at.
const MAX_WALK: usize = 2_000_000;

/// Bumped whenever a change gives forms or resource dictionaries more users than they had (a copy of a resources
/// dictionary, a copy of a form): the counts taken before it are then too low. Per thread: one edit runs on one.
fn references_added() {
    REFERENCES.with(|r| r.set(r.get().wrapping_add(1)));
}

thread_local! {
    static REFERENCES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// Walks of the whole file made on this thread.
    static WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The user counts of one edit (a batch, a move to another page): the file is walked once, and again only when a
/// copy has added users since. A file too big to walk stays unwalked: its forms count as shared, except those this
/// edit made itself.
struct Users {
    seen: u32,
    counts: Option<HashMap<i32, u32>>,
    walked: bool,
    too_big: bool,
    /// The forms this edit made: each is drawn once, by the `Do` that was pointed at it.
    made: HashSet<i32>,
}

impl Users {
    fn new() -> Users {
        Users { seen: 0, counts: None, walked: false, too_big: false, made: HashSet::new() }
    }

    /// The slot counts of every form and resource dictionary `pi`'s scopes name; None when the file is too big.
    fn of(&mut self, d: &PdfDocument, pi: &PageImages) -> Option<HashMap<i32, u32>> {
        let now = REFERENCES.with(|r| r.get());
        if !self.too_big && (!self.walked || self.seen != now) {
            let mut targets: HashSet<i32> = pi.scopes.iter().flat_map(|s| [s.num, s.res_num, s.xo_num]).collect();
            targets.retain(|n| *n > 0);
            self.counts = incoming(d, &targets);
            self.too_big = self.counts.is_none();
            (self.walked, self.seen) = (true, now);
        }
        self.counts.clone()
    }
}

/// How many slots (dictionary values and array elements) of the objects reachable from the trailer refer to each
/// of `targets`. None when the file is too large to walk: the caller then takes everything as shared.
fn incoming(d: &PdfDocument, targets: &HashSet<i32>) -> Option<HashMap<i32, u32>> {
    #[cfg(test)]
    WALKS.with(|w| w.set(w.get() + 1));
    let mut counts = HashMap::new();
    let mut seen = HashSet::new();
    let mut stack = vec![d.trailer().ok()?];
    let mut work = 0usize;
    while let Some(o) = stack.pop() {
        work += 1;
        if work > MAX_WALK {
            return None;
        }
        if o.is_indirect().ok()? {
            let n = o.as_indirect().ok()?;
            if targets.contains(&n) {
                *counts.entry(n).or_insert(0u32) += 1;
            }
            if !seen.insert(n) {
                continue;
            }
        }
        // A stream is its dictionary here: its data is not looked at.
        if o.is_dict().ok()? {
            for i in 0..o.dict_len().ok()? as i32 {
                stack.extend(o.get_dict_val(i).ok()?);
            }
        } else if o.is_array().ok()? {
            for i in 0..o.len().ok()? as i32 {
                stack.extend(o.get_array(i).ok()?);
            }
        }
    }
    Some(counts)
}

/// The form XObject a change inside scope `at` is written to. The form itself when this drawing is its only user
/// (nothing else in the document can reach it: one slot names it, one `Do` draws it, and the dictionaries on the
/// way down are not shared); otherwise copies, so no other drawing changes:
/// from the outermost form on the way that is shared, each form is copied (its stream and its own resources), the
/// `Do` that draws it in the parent is pointed at the copy under a fresh name, and the copy's own `Do` of the next
/// form is pointed at that one's copy. Whatever is not sure is taken as shared: a copy costs a few bytes, a change
/// to another page is a wrong document.
fn private_form(d: &mut PdfDocument, pi: &PageImages, at: usize, users: &mut Users) -> Result<PdfObject> {
    let mut chain = Vec::new();
    let mut s = at;
    while let Some((p, _, _)) = pi.scopes[s].via {
        chain.push(s);
        s = p;
    }
    chain.reverse();
    let counts = users.of(d, pi);
    let used_once = |n: i32| n == 0 || counts.as_ref().is_some_and(|c| c.get(&n).copied().unwrap_or(0) <= 1);
    // Names a form or a Type3 font without resources draws are not among the parent's own `Do`s.
    let mut open_parent = HashSet::new();
    for &c in &chain {
        let Some((p, _, _)) = pi.scopes[c].via else { continue };
        if shares_resources(&pi.scopes[p].holder)? {
            open_parent.insert(p);
        }
    }
    let first_shared = chain.iter().position(|&c| {
        let sc = &pi.scopes[c];
        let Some((p, _, name)) = &sc.via else { return true };
        let parent = &pi.scopes[*p];
        let draws = parent.content.ops.iter().filter(|o| o.is("Do") && o.name(0) == Some(name.as_slice())).count();
        let named = match &counts {
            Some(c) => c.get(&sc.num).copied() == Some(1),
            None => users.made.contains(&sc.num),
        };
        !(sc.num > 0 && named && draws == 1 && parent.own_res && !open_parent.contains(p) && used_once(parent.res_num) && used_once(parent.xo_num))
    });
    let Some(j) = first_shared else { return Ok(pi.scopes[at].holder.clone()) };
    // The parent of the first copy is not shared: the page, or a form this way alone reaches.
    let first_parent = pi.scopes[pi.scopes[chain[j]].via.as_ref().map_or(0, |v| v.0)].holder.clone();
    let drawn: HashSet<&[u8]> = pi.scopes.iter().flat_map(|s| s.content.ops.iter()).filter(|o| o.is("Do")).filter_map(|o| o.name(0)).collect();
    let mut parent_holder = first_parent;
    let mut clone = pi.scopes[at].holder.clone();
    for &c in &chain[j..] {
        let sc = &pi.scopes[c];
        let (p, op, via_name) = sc.via.clone().expect("a form is drawn by an operator");
        let parent = &pi.scopes[p];
        // The copy: the form's dictionary and decoded stream, with resources of its own.
        let mut dict = sc.holder.copy_dict()?;
        for k in ["Filter", "DecodeParms", "Length", "DL"] {
            dict.dict_delete(k)?;
        }
        dict.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
        let data = sc.content.streams.first().map(|(_, b)| b.as_slice()).unwrap_or_default();
        clone = d.add_stream(&Buffer::from_bytes(&flate(data)?)?, Some(&dict), true)?;
        local_xobjects(d, &clone)?;
        users.made.insert(clone.as_indirect()?);
        // The copy's resources name the forms the original's do, and the parent names the copy.
        references_added();
        let name = add_named(d, &parent_holder, "IwForm", &clone, &drawn)?;
        // Only the one `Do` is pointed at the copy; the parent's other draws of the form stay on the original.
        let mut once = Some(vec![Op::new("Do", vec![Obj::Name(name)])]);
        let new = splice(&parent.content.pieces, &parent.content.ops, |i| if i == op { (once.take().unwrap_or_default(), false) } else { (Vec::new(), true) });
        write_content(d, &parent_holder, parent, new)?;
        // The parent (a copy of its own, or the page) no longer draws the shared form under that name.
        forget_unused(d, &parent_holder, &HashSet::from([via_name]))?;
        parent_holder = clone.clone();
    }
    Ok(clone)
}

/// Applies `change` to image `index` of `page` (or adds one). The result is a warning for the user, if the change
/// calls for one.
fn apply(d: &mut PdfDocument, page: u32, index: Option<u32>, change: &ImageChange, users: &mut Users) -> Result<Option<String>> {
    let to_page = page_space(d, page)?;
    let to_user = to_page.invert().ok_or_else(|| invalid("the page has no area"))?;
    let pi = read_images(d, page)?;
    if let ImageChange::Add { file, rect, keep_aspect } = change {
        if index.is_some() {
            return Err(invalid("a new image takes no index"));
        }
        add(d, &pi, &to_user, file, *rect, *keep_aspect)?;
        return Ok(None);
    }
    let index = index.ok_or_else(|| invalid("which image?"))? as usize;
    let f = pi.found.get(index).cloned().ok_or_else(|| invalid(format!("page {} has no image {}", page + 1, index + 1)))?;
    if f.in_text && !matches!(change, ImageChange::Delete) {
        return Err(invalid("this image is drawn inside a text object; it can only be deleted"));
    }
    let sc = &pi.scopes[f.scope];
    if let Some(why) = &sc.reason {
        return Err(invalid(format!("this image can't be changed: {why}")));
    }
    let c = &sc.content;
    // The image's operator exactly as written (an inline image keeps its parameters and data).
    let original = Op::verbatim(&c.pieces.data()[c.ops[f.op].span.clone()]);
    // Page-space transform of the image's box, for move, resize, rotate and flip.
    let full = to_page.bbox(f.ctm.bbox(UNIT));
    let (cx, cy) = ((full[0] + full[2]) / 2.0, (full[1] + full[3]) / 2.0);
    let about_centre = |m: [f64; 4]| Matrix([m[0], m[1], m[2], m[3], 0.0, 0.0]).then(&Matrix::translate(cx, cy));
    let page_t = match change {
        ImageChange::Move { dx, dy } => Some(Matrix::translate(*dx, *dy)),
        ImageChange::Resize { rect, keep_aspect } => {
            let r = ordered(*rect)?;
            // A box stretched unevenly would shear an image turned by other than quarter turns.
            if !*keep_aspect && !axis_aligned(&f.ctm.then(&to_page)) {
                return Err(invalid("a turned image can only be resized keeping its proportions"));
            }
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
        ImageChange::Angle { degrees } => {
            if !degrees.is_finite() {
                return Err(invalid("the angle must be a number"));
            }
            let a = degrees.rem_euclid(360.0);
            if a < 1e-6 || 360.0 - a < 1e-6 {
                return Err(invalid("a rotation of 0 changes nothing"));
            }
            let (sin, cos) = a.to_radians().sin_cos();
            Some(Matrix::translate(-cx, -cy).then(&about_centre([cos, sin, -sin, cos])))
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
        let (start, end) = unit_of(&c.ops, f.op);
        let ctm = c.ctms[start];
        // New placement = C · T (T in user space); as a `cm` there: C · T · C⁻¹.
        let t_user = to_page.then(&t).then(&to_user);
        let inv = ctm.invert().ok_or_else(|| invalid("the image has no area"))?;
        let m = ctm.then(&t_user).then(&inv);
        if !m.0.iter().all(|v| v.is_finite() && v.abs() < 1e9) {
            return Err(invalid("the image can't be moved that far"));
        }
        let warning = f.frame.and_then(|frame| frame_warning(f.ctm.bbox(UNIT), f.ctm.then(&t_user).bbox(UNIT), frame));
        let cm = fine_cm(&m);
        let new = if start == end {
            let mut wrapped = Some(vec![Op::new("q", vec![]), cm, original, Op::new("Q", vec![])]);
            splice(&c.pieces, &c.ops, |i| if i == f.op { (wrapped.take().unwrap_or_default(), false) } else { (Vec::new(), true) })
        } else {
            let mut cm = Some(vec![cm]);
            splice(&c.pieces, &c.ops, |i| if i == start + 1 { (cm.take().unwrap_or_default(), true) } else { (Vec::new(), true) })
        };
        let holder = if f.scope == 0 { pi.page.clone() } else { private_form(d, &pi, f.scope, users)? };
        write_content(d, &holder, sc, new)?;
        return Ok(warning);
    }
    // Where a name is added or dropped: the page, or the form the picture is drawn in (its own copy when it is shared).
    let holder = if f.scope == 0 { pi.page.clone() } else { private_form(d, &pi, f.scope, users)? };
    let replacement: Vec<Op> = {
        match change {
            ImageChange::Delete => {
                gone.extend(f.name.clone());
                Vec::new()
            }
            ImageChange::Replace { file } => {
                let (img, w, h) = load_image(d, file)?;
                let name = add_resource(d, &holder, &img)?;
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
                    let name = add_resource(d, &holder, &obj)?;
                    gone.extend(f.name.clone());
                    vec![
                        Op::new("q", vec![]),
                        Op::new("cm", unit_to(sub).0.iter().map(|v| num(*v)).collect()),
                        Op::new("Do", vec![Obj::Name(name)]),
                        Op::new("Q", vec![]),
                    ]
                } else {
                    // The clip is the page-space rectangle itself: in the image's unit square it is
                    // a rectangle when the image is upright or turned by quarter turns, and a
                    // parallelogram (four straight edges) when it is turned or sheared otherwise.
                    let to_unit = to_user.then(&inv);
                    let corners = [(r[0], r[1]), (r[2], r[1]), (r[2], r[3]), (r[0], r[3])].map(|(x, y)| to_unit.apply(x, y));
                    let upright = (0..4).all(|i| {
                        let (a, b) = (corners[i], corners[(i + 1) % 4]);
                        (a.0 - b.0).abs() < 1e-9 || (a.1 - b.1).abs() < 1e-9
                    });
                    let mut ops = vec![Op::new("q", vec![])];
                    if upright {
                        ops.push(Op::new("re", vec![num(u[0]), num(u[1]), num(u[2] - u[0]), num(u[3] - u[1])]));
                    } else {
                        for (i, (x, y)) in corners.iter().enumerate() {
                            ops.push(Op::new(if i == 0 { "m" } else { "l" }, vec![num(*x), num(*y)]));
                        }
                        ops.push(Op::new("h", vec![]));
                    }
                    ops.extend([Op::new("W", vec![]), Op::new("n", vec![]), original, Op::new("Q", vec![])]);
                    ops
                }
            }
            _ => unreachable!("transforms are handled above"),
        }
    };
    let mut replacement = Some(replacement);
    let new = splice(&c.pieces, &c.ops, |i| if i == f.op { (replacement.take().unwrap_or_default(), false) } else { (Vec::new(), true) });
    write_content(d, &holder, sc, new)?;
    forget_unused(d, &holder, &gone)?;
    Ok(None)
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
        while k > 0 && matches!(ops[k - 1].op.as_slice(), b"cm" | b"re" | b"m" | b"l" | b"h" | b"W" | b"W*" | b"n") {
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
    append_draws(d, pi, draw_ops(Op::new("Do", vec![Obj::Name(name)]), m, None))
}

/// `q [re W n] cm draw Q`: `draw` placed by `place` (unit square → user space), clipped to a
/// user-space rectangle.
fn draw_ops(draw: Op, place: Matrix, clip: Option<[f64; 4]>) -> Vec<Op> {
    let mut ops = vec![Op::new("q", vec![])];
    if let Some(c) = clip {
        ops.push(Op::new("re", vec![num(c[0]), num(c[1]), num(c[2] - c[0]), num(c[3] - c[1])]));
        ops.extend([Op::new("W", vec![]), Op::new("n", vec![])]);
    }
    ops.push(Op::new("cm", place.0.iter().map(|v| num(*v)).collect()));
    ops.extend([draw, Op::new("Q", vec![])]);
    ops
}

/// Appends `ops` after the page's content, in a clean graphics state. Whatever state the content
/// leaves (a cm, a clip, a colour, q without Q or Q without q) must not reach them: the content
/// goes inside q … Q, with as many extra q before it as it closes more than it opens, and as many
/// Q after it as are left open.
fn append_draws(d: &mut PdfDocument, pi: &PageImages, ops: Vec<Op>) -> Result<()> {
    let c = &pi.scopes[0].content;
    let (mut depth, mut lowest) = (0i64, 0i64);
    for op in &c.ops {
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
    let mut streams = c.streams.clone();
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

// ---- copy, paste, move to another page ---------------------------------------------------------

/// One copied picture: where it sat, in page space (as seen).
#[derive(Clone, Debug)]
struct ClipImage {
    /// The image's unit square → page space.
    place: Matrix,
    /// The rectangular clip around it.
    clip: Option<[f64; 4]>,
}

/// Copied pictures: a small PDF holding their image objects (with SMask, decode parameters and
/// colour space; a paste grafts them into the target, so nothing is shared by reference) and where
/// each of them sat.
#[derive(Clone, Debug, Default)]
pub struct Clip {
    bytes: Vec<u8>,
    images: Vec<ClipImage>,
    /// What shows of each, page space.
    rects: Vec<[f64; 4]>,
}

/// The pictures on the clipboard: how many, and the boxes they covered on their page.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClipInfo {
    pub count: u32,
    pub rect: [f32; 4],
    pub rects: Vec<[f32; 4]>,
}

impl Clip {
    fn rect(&self) -> [f64; 4] {
        self.rects.iter().copied().reduce(union).unwrap_or([0.0; 4])
    }

    pub fn info(&self) -> ClipInfo {
        ClipInfo { count: self.images.len() as u32, rect: self.rect().map(|v| v as f32), rects: self.rects.iter().map(|r| r.map(|v| v as f32)).collect() }
    }
}

/// What the app holds between a copy and a paste (any open document may paste it).
#[derive(Default)]
pub struct ImageClipboard(pub parking_lot::Mutex<Option<std::sync::Arc<Clip>>>);

/// The most pictures one copy takes.
const MAX_CLIP_IMAGES: usize = 100;

/// An inline image (`BI … EI` bytes) whose colour space is a device one, a stencil, or an Indexed one over
/// a device base: nothing in it names the resources of the page it is on.
fn self_contained(bytes: &[u8]) -> bool {
    let device = |o: &Obj| matches!(o, Obj::Name(n) if matches!(n.as_slice(), b"G" | b"RGB" | b"CMYK" | b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK"));
    let d = inline_dict(bytes);
    match d.iter().find(|(k, _)| k == b"ColorSpace").map(|(_, v)| v) {
        None => true,
        Some(Obj::Array(a)) => matches!(a.first(), Some(Obj::Name(n)) if matches!(n.as_slice(), b"I" | b"Indexed")) && a.get(1).is_some_and(device),
        Some(v) => device(v),
    }
}

/// The page's size in page space.
fn page_size(d: &PdfDocument, page: u32) -> Result<(f64, f64)> {
    let b = d.load_page(page as i32)?.bounds()?;
    Ok((f64::from(b.x1 - b.x0), f64::from(b.y1 - b.y0)))
}

/// The top left corner of a `w × h` group dropped at `at`, kept on a `pw × ph` page.
fn keep_on_page(at: [f64; 2], size: (f64, f64), page: (f64, f64)) -> Result<(f64, f64)> {
    if !at.iter().all(|v| v.is_finite()) {
        return Err(invalid("where to?"));
    }
    Ok((at[0].clamp(0.0, (page.0 - size.0).max(0.0)), at[1].clamp(0.0, (page.1 - size.1).max(0.0))))
}

fn unique(indices: &[u32]) -> Result<Vec<u32>> {
    let mut seen = HashSet::new();
    let v: Vec<u32> = indices.iter().copied().filter(|i| seen.insert(*i)).collect();
    if v.is_empty() {
        return Err(invalid("which images?"));
    }
    if v.len() > MAX_CLIP_IMAGES {
        return Err(invalid("too many images at once"));
    }
    Ok(v)
}

/// The bytes of decoded samples `img` (declared) takes, with those of its soft mask and stencil mask, or why
/// it is too large to copy.
fn declared_bytes(img: &PdfObject, f: &Found) -> Result<u64> {
    let (n, bpc) = (components(img).unwrap_or(4), f.bpc.clamp(1, 16));
    let (bytes, _) = sample_size(f.width, f.height, n, bpc)
        .ok_or_else(|| invalid("the image is too large to copy (more than 100 megapixels or 512 MB of samples)"))?;
    let mut total = bytes as u64;
    for key in ["SMask", "Mask"] {
        let Some(m) = get(img, key).filter(|m| m.is_stream().unwrap_or(false)) else { continue };
        let bpc = if key == "Mask" { 1 } else { int_of(get(&m, "BitsPerComponent")).clamp(1, 16) };
        let (b, _) = sample_size(int_of(get(&m, "Width")), int_of(get(&m, "Height")), 1, bpc)
            .ok_or_else(|| invalid("the image's mask is too large to copy"))?;
        total += b as u64;
    }
    Ok(total)
}

/// Keys of an image, its masks, its colour space (ICC profile, Indexed lookup, tint functions) and the filter
/// parameters that a copy keeps. Anything else, in any of those dictionaries, is left behind: an arbitrary key
/// could point at the page, the catalog or any other part of the document.
const COPY_KEYS: &[&str] = &[
    "Type", "Subtype", "Width", "Height", "ColorSpace", "BitsPerComponent", "Filter", "DecodeParms", "Decode", "SMask", "Mask",
    "ImageMask", "Interpolate", "Intent", "SMaskInData", "Matte", "Name", "Predictor", "Colors", "Columns", "Rows", "EarlyChange",
    "K", "EndOfLine", "EncodedByteAlign", "EndOfBlock", "BlackIs1", "DamagedRowsBeforeError", "JBIG2Globals", "ColorTransform", "N",
    "Alternate", "Range", "WhitePoint", "BlackPoint", "Gamma", "Matrix", "FunctionType", "Domain", "C0", "C1", "Size", "Order",
    "Encode", "Bounds", "Functions", "BitsPerSample", "Colorants", "Process", "Components", "MixingHints",
];

/// How deep and how wide a copied image's description may go (an image's own graph is a few objects).
const COPY_DEPTH: u32 = 8;
/// Objects one copy may visit in all (a hundred pictures, each with a mask, a colour space and a few parameters).
const COPY_NODES: u32 = 4096;

/// Copies image XObjects, with the objects they need, into `out` as new objects: dictionaries are rebuilt from
/// [`COPY_KEYS`] only and streams are copied as raw bytes with their own filters. One copier serves a whole
/// copy, so its caps cover every picture, and a stream several pictures share (a soft mask, a profile) is read
/// and copied once, and shared in the copy.
struct Copier<'a> {
    out: &'a mut PdfDocument,
    nodes: u32,
    bytes: u64,
    /// The copy of each source stream already made (by object number, and whether it was met as a mask).
    seen: std::collections::HashMap<(i32, bool), PdfObject>,
}

impl Copier<'_> {
    fn value(&mut self, v: &PdfObject, depth: u32, in_mask: bool) -> Result<PdfObject> {
        self.nodes += 1;
        if depth > COPY_DEPTH || self.nodes > COPY_NODES {
            return Err(invalid("the image's description is too deeply nested to copy"));
        }
        if v.is_stream()? {
            return self.stream(v, depth, in_mask);
        }
        if v.is_dict()? {
            let mut d = self.out.new_dict()?;
            self.keys(v, &mut d, depth, in_mask)?;
            return Ok(d);
        }
        if v.is_array()? {
            let mut a = self.out.new_array()?;
            for i in 0..v.len()? as i32 {
                let e = match v.get_array(i)? {
                    Some(e) => self.value(&e, depth + 1, in_mask)?,
                    None => PdfObject::new_null(),
                };
                a.array_push(e)?;
            }
            return Ok(a);
        }
        // A number, name, string or boolean: nothing to follow.
        Ok(self.out.graft_object(v)?)
    }

    fn keys(&mut self, src: &PdfObject, dst: &mut PdfObject, depth: u32, in_mask: bool) -> Result<()> {
        for i in 0..src.dict_len()? as i32 {
            let (Some(k), Some(v)) = (src.get_dict_key(i)?, src.get_dict_val(i)?) else { continue };
            let name = String::from_utf8_lossy(&k.as_name()?).into_owned();
            let mask = matches!(name.as_str(), "SMask" | "Mask");
            // A mask is a plain image with no mask of its own (which also ends one that refers back to its image).
            if !COPY_KEYS.contains(&name.as_str()) || v.is_null()? || (in_mask && mask) {
                continue;
            }
            let c = self.value(&v, depth + 1, in_mask || mask)?;
            dst.dict_put(name.as_str(), c)?;
        }
        Ok(())
    }

    fn stream(&mut self, src: &PdfObject, depth: u32, in_mask: bool) -> Result<PdfObject> {
        let num = src.as_indirect().unwrap_or(0);
        if num > 0 {
            if let Some(done) = self.seen.get(&(num, in_mask)) {
                return Ok(done.clone());
            }
        }
        let too_big = || invalid("these images are too large to copy together");
        // The stream's own /Length says what is coming before any of it is read (it is only a hint).
        let hint = get(src, "Length").and_then(|n| n.as_int().ok()).unwrap_or(0).max(0) as u64;
        if self.bytes.saturating_add(hint) > MAX_SAMPLE_BYTES {
            return Err(too_big());
        }
        let raw = src.read_raw_stream()?;
        self.bytes += raw.len() as u64;
        if self.bytes > MAX_SAMPLE_BYTES {
            return Err(too_big());
        }
        let mut d = self.out.new_dict()?;
        self.keys(src, &mut d, depth, in_mask)?;
        let copy = self.out.add_stream(&Buffer::from_bytes(&raw)?, Some(&d), true)?;
        if num > 0 {
            self.seen.insert((num, in_mask), copy.clone());
        }
        Ok(copy)
    }
}

/// Copies the images `indices` of `page` (listed at `rev`) as a [`Clip`]. Pictures drawn inline
/// can be moved to another page but not copied; the owner's permissions must allow copying.
pub fn copy(eng: &Engine, doc: DocId, rev: u32, page: u32, indices: &[u32]) -> Result<Clip> {
    let entry = eng.doc_entry(doc)?;
    let indices = unique(indices)?;
    eng.read_custom(doc, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        if !crate::protect::Rights::of(d).may_extract() {
            return Err(invalid("The owner of this document does not allow copying its pictures."));
        }
        let pi = read_images(d, page)?;
        let to_page = page_space(d, page)?;
        let mut out = PdfDocument::new();
        let mut xo = out.new_dict()?;
        let mut copier = Copier { out: &mut out, nodes: 0, bytes: 0, seen: Default::default() };
        let (mut images, mut rects, mut total) = (Vec::new(), Vec::new(), 0u64);
        for (n, i) in indices.iter().enumerate() {
            let f = pi.found.get(*i as usize).ok_or_else(|| invalid(format!("page {} has no image {}", page + 1, i + 1)))?;
            if f.in_text {
                return Err(invalid("this image is drawn inside a text object; it can't be copied"));
            }
            let Some(obj) = &f.obj else {
                return Err(invalid("an inline picture can be moved to another page but not copied"));
            };
            total += declared_bytes(obj, f)?;
            if total > MAX_SAMPLE_BYTES {
                return Err(invalid("these images are too large to copy together"));
            }
            // Only what an image is made of comes along: the page's layers, structure entry, alternates
            // and OPI proxies stay behind, and so does any other key that points elsewhere in the file.
            let g = copier.stream(obj, 0, false)?;
            xo.dict_put(format!("I{n}").as_str(), g)?;
            let full = f.ctm.bbox(UNIT);
            rects.push(to_page.bbox(f.clip.map_or(full, |c| intersect(full, c))));
            images.push(ClipImage { place: f.ctm.then(&to_page), clip: f.clip.map(|c| to_page.bbox(c)) });
        }
        // The copies hang off a page's resources, so the file is an ordinary PDF.
        let pg = out.new_page_at(0, mupdf::Size { width: 612.0, height: 792.0 })?;
        let mut res = out.new_dict()?;
        res.dict_put("XObject", xo)?;
        pg.object().dict_put("Resources", res)?;
        drop(pg);
        let mut w = mupdf::pdf::PdfWriteOptions::default();
        w.set_garbage_level(1);
        let mut bytes = Vec::new();
        out.write_to_with_options(&mut bytes, w)?;
        Ok(Clip { bytes, images, rects })
    })
}

/// Pastes `clip` on `page` of a document (listed at `rev`), its top left corner at page-space
/// `at` (kept on the page): one undo step. The picture objects are copied into this document.
pub fn paste(eng: &Engine, doc: DocId, rev: u32, page: u32, at: [f64; 2], clip: &Clip, source: EditSource) -> Result<ImageEdited> {
    let n = clip.images.len();
    let entry = eng.doc_entry(doc)?;
    let label = if n == 1 { format!("Paste image on p. {}", page + 1) } else { format!("Paste {n} images on p. {}", page + 1) };
    let state = eng.apply_removing(doc, label, source, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        crate::outline::refuse_if_certified(d, "Pasting images")?;
        if n == 0 {
            return Err(invalid("nothing is copied"));
        }
        let to_page = page_space(d, page)?;
        let to_user = to_page.invert().ok_or_else(|| invalid("the page has no area"))?;
        let group = clip.rect();
        let (x, y) = keep_on_page(at, (group[2] - group[0], group[3] - group[1]), page_size(d, page)?)?;
        let shift = Matrix::translate(x - group[0], y - group[1]);
        let into_user = shift.then(&to_user);
        let pi = read_images(d, page)?;
        let src = PdfDocument::from_bytes(&clip.bytes).map_err(|_| invalid("the copied images can't be read"))?;
        let xo = get(&src.find_page(0)?, "Resources")
            .and_then(|r| get(&r, "XObject"))
            .ok_or_else(|| invalid("the copied images can't be read"))?;
        let mut map = d.new_graft_map()?;
        let mut ops = Vec::new();
        for (i, im) in clip.images.iter().enumerate() {
            let obj = get(&xo, &format!("I{i}")).ok_or_else(|| invalid("the copied images can't be read"))?;
            let name = add_resource(d, &pi.page, &map.graft_object(&obj)?)?;
            let place = im.place.then(&into_user);
            ops.extend(draw_ops(Op::new("Do", vec![Obj::Name(name)]), place, im.clip.map(|c| into_user.bbox(c))));
        }
        append_draws(d, &pi, ops)?;
        Ok(vec![page])
    })?;
    Ok(ImageEdited { state, warning: None })
}

/// Moves the images `indices` of page `from` (listed at `rev`) to `target` of the same document: its
/// page and the group's top left corner at page-space `at` there (kept on the page): one undo step.
/// The picture goes where it was dropped, whatever the two pages' size or rotation.
pub fn move_to_page(eng: &Engine, doc: DocId, rev: u32, from: u32, indices: &[u32], target: (u32, [f64; 2]), source: EditSource) -> Result<ImageEdited> {
    let (to, at) = target;
    if from == to {
        return Err(invalid("the images are on that page already"));
    }
    let indices = unique(indices)?;
    let entry = eng.doc_entry(doc)?;
    let label = if indices.len() == 1 { format!("Move image to p. {}", to + 1) } else { format!("Move {} images to p. {}", indices.len(), to + 1) };
    let state = eng.apply_removing(doc, label, source, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        crate::outline::refuse_if_certified(d, "Moving images")?;
        let (from_page, to_page) = (page_space(d, from)?, page_space(d, to)?);
        let to_user = to_page.invert().ok_or_else(|| invalid("the page has no area"))?;
        let pf = read_images(d, from)?;
        let pt = read_images(d, to)?;
        let mut picked = Vec::new();
        for i in &indices {
            let f = pf.found.get(*i as usize).ok_or_else(|| invalid(format!("page {} has no image {}", from + 1, i + 1)))?;
            if f.in_text {
                return Err(invalid("this image is drawn inside a text object; it can only be deleted"));
            }
            if let Some(why) = &pf.scopes[f.scope].reason {
                return Err(invalid(format!("this image can't be changed: {why}")));
            }
            // A page's own colour space can't come along in the picture's operator.
            let c = &pf.scopes[f.scope].content;
            if f.kind == ImageKind::Inline && !self_contained(&c.pieces.data()[c.ops[f.op].span.clone()]) {
                return Err(invalid("this inline picture uses a colour space of its page, so it can't be moved to another page"));
            }
            picked.push((*i, f));
        }
        let group = picked
            .iter()
            .map(|(_, f)| {
                let full = f.ctm.bbox(UNIT);
                from_page.bbox(f.clip.map_or(full, |c| intersect(full, c)))
            })
            .reduce(union)
            .unwrap_or([0.0; 4]);
        let (x, y) = keep_on_page(at, (group[2] - group[0], group[3] - group[1]), page_size(d, to)?)?;
        let into_user = from_page.then(&Matrix::translate(x - group[0], y - group[1])).then(&to_user);
        // The pictures the page itself draws are cut out of its content below; those drawn in a form are deleted
        // one by one (a shared form is copied first), the last of the list first since deleting renumbers.
        let (mut draws, mut gone, mut remove, mut in_forms) = (Vec::new(), HashSet::new(), HashSet::new(), Vec::new());
        for (i, f) in picked {
            let c = &pf.scopes[f.scope].content;
            let draw = match (&f.obj, &f.name) {
                // The same image object moves: the new page names it, the old one forgets it.
                (Some(obj), Some(name)) => {
                    if f.scope == 0 {
                        gone.insert(name.clone());
                    }
                    Op::new("Do", vec![Obj::Name(add_resource(d, &pt.page, obj)?)])
                }
                _ => Op::verbatim(&c.pieces.data()[c.ops[f.op].span.clone()]),
            };
            let place = f.ctm.then(&into_user);
            draws.extend(draw_ops(draw, place, f.clip.map(|c| into_user.bbox(c))));
            if f.scope == 0 {
                remove.insert(f.op);
            } else {
                in_forms.push(i);
            }
        }
        in_forms.sort_unstable_by_key(|i| std::cmp::Reverse(*i));
        let mut users = Users::new();
        for i in in_forms {
            apply(d, from, Some(i), &ImageChange::Delete, &mut users)?;
        }
        if !remove.is_empty() {
            // Read again: copying a shared form pointed one of the page's `Do`s elsewhere (the operators stay).
            let pf = read_images_in(d, from, false)?;
            let c = &pf.scopes[0].content;
            let new = splice(&c.pieces, &c.ops, |i| (Vec::new(), !remove.contains(&i)));
            write_streams(d, &pf.page, &c.streams, new)?;
        }
        append_draws(d, &pt, draws)?;
        forget_unused(d, &pf.page, &gone)?;
        Ok(vec![from, to])
    })?;
    Ok(ImageEdited { state, warning: None })
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

/// The most bytes of decoded samples a pixel crop reads (32 components at 16 bits would otherwise
/// let a 100-megapixel image ask for 6.4 GB): 512 MB, a 100-megapixel RGBA image and some.
const MAX_SAMPLE_BYTES: u64 = 512 << 20;

/// Bytes of decoded samples an image of `w × h` pixels, `n` components and `bpc` bits takes, and
/// its row length; None when it is too large to decode.
fn sample_size(w: u32, h: u32, n: u32, bpc: u32) -> Option<(usize, usize)> {
    if u64::from(w).checked_mul(u64::from(h))? > MAX_PIXELS {
        return None;
    }
    let row = u64::from(w).checked_mul(u64::from(n))?.checked_mul(u64::from(bpc))?.div_ceil(8);
    let total = row.checked_mul(u64::from(h))?;
    if total > MAX_SAMPLE_BYTES {
        return None;
    }
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

// ---- for other modules (OCR clean-up) ----------------------------------------------------------

/// An image a page draws, as the OCR clean-up needs it.
#[derive(Clone)]
pub(crate) struct Placed {
    pub inline: bool,
    pub in_text: bool,
    /// Key in the page's `/XObject` dictionary (empty for an inline image).
    pub name: Vec<u8>,
    pub obj: Option<PdfObject>,
    /// The unit square to default user space.
    pub ctm: [f64; 6],
    pub clipped: bool,
    pub masked: bool,
    pub width: u32,
    pub height: u32,
}

/// The images `page` (0-based) draws, in drawing order.
pub(crate) fn placed_images(doc: &PdfDocument, page: u32) -> Result<Vec<Placed>> {
    Ok(read_images_in(doc, page, false)?
        .found
        .into_iter()
        .map(|f| Placed {
            inline: f.kind == ImageKind::Inline,
            in_text: f.in_text,
            name: f.name.unwrap_or_default(),
            obj: f.obj,
            ctm: f.ctm.0,
            clipped: f.clip.is_some(),
            masked: f.masked,
            width: f.width,
            height: f.height,
        })
        .collect())
}

/// The decoded samples of an image XObject, with the size guards of pixel crops.
pub(crate) struct Decoded {
    pub data: Vec<u8>,
    pub w: u32,
    pub h: u32,
    /// Components per pixel.
    pub n: u32,
    pub bpc: u32,
    /// Bytes per row.
    pub row: usize,
}

pub(crate) fn decoded_samples(img: &PdfObject) -> Result<Decoded> {
    let (w, h) = (int_of(get(img, "Width")), int_of(get(img, "Height")));
    let bpc = if get(img, "ImageMask").and_then(|v| v.as_bool().ok()) == Some(true) { 1 } else { int_of(get(img, "BitsPerComponent")) };
    let n = components(img).ok_or_else(|| invalid("the image's colour space is not one Inkwell can read"))?;
    if w == 0 || h == 0 || !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return Err(invalid("the image's pixel format can't be read"));
    }
    let (size, row) = sample_size(w, h, n, bpc).ok_or_else(|| invalid("the image is too large to read by pixels"))?;
    let data = samples(img, size)?;
    if data.len() < size {
        return Err(invalid("the image's data is shorter than its size"));
    }
    Ok(Decoded { data, w, h, n, bpc, row })
}

#[cfg(test)]
mod tests;
