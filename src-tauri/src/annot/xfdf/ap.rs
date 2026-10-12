//! Appearance streams (`/AP`) that travel with a comment: read from a file or from a stamp of the document,
//! kept as plain Rust data, and written into a document or into a file.
//!
//! An appearance is hostile input. It is never copied as it is (no `graft_object`): it is read through a
//! whitelist into a [`Package`] of dictionaries and streams that holds only what draws, and only what the
//! content stream names. Everything else (actions, `/AA`, `/OC`, JavaScript, metadata, patterns, shadings, type 3
//! and composite fonts, inline images) is not carried; an appearance that needs one of those is refused as a
//! whole and the comment gets Inkwell's own drawing. Every stream is decoded under a size cap while it is read
//! (a compression bomb stops at the cap), and every name the content stream uses must resolve in the form's
//! own `/Resources`, never in the page's.
//!
//! The file shape is Inkwell's own reading of "base64 of the appearance" in ISO 19444-1: an FDF file that holds one
//! annotation with an `/AP`. It was not checked against a file Acrobat wrote.

use crate::engine::EngineError;
use crate::exchange::{Budget, pdf_name};
use crate::forms::get;
use mupdf::Buffer;
use mupdf::pdf::{PdfDocument, PdfObject};
use std::collections::HashMap;
use std::fmt::Write as _;

/// Why a part of a comment (its appearance, its rich text, its file) is not carried. The text is what the import
/// report shows, so it reads after "N comments:".
pub(crate) type Why = &'static str;

pub(super) const NOT_STREAM: Why = "appearance that is not one stream";
pub(super) const UNSUPPORTED: Why = "appearance with a pattern, shading, inline image, or a composite or Type 3 font";
pub(super) const UNRESOLVED: Why = "appearance that uses a resource it does not define";
pub(super) const TOO_BIG: Why = "appearance over the size limit, or that unpacks to more than it";
pub(super) const DAMAGED: Why = "appearance that is damaged or not understood";
pub(super) const NO_ROOM: Why = "appearance past the limit for appearances and files in one file";

/// Stream bytes, decoded, in one appearance.
pub(crate) const MAX_PACKAGE: usize = 4 << 20;
/// Dictionaries and streams in one appearance (a font is a few, an image two).
const MAX_OBJECTS: usize = 64;
/// Forms inside forms.
const MAX_DEPTH: usize = 4;
/// Pixels in one image of an appearance.
const MAX_PIXELS: u64 = 16_000_000;
/// Numbers in a font's `/Widths` or `/Differences`.
const MAX_LIST: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Val {
    Int(i32),
    Real(f32),
    Bool(bool),
    Name(String),
    Arr(Vec<Val>),
    Dict(Vec<(String, Val)>),
    /// Another object of the package.
    Ref(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Obj {
    pub dict: Vec<(String, Val)>,
    /// Stream bytes: decoded, except an image's, which its `/Filter` says how they are stored.
    pub data: Option<Vec<u8>>,
}

/// Objects, children before the objects that refer to them, and which one is the root (the appearance's `/N`, or
/// an attachment's file specification).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Package {
    pub objs: Vec<Obj>,
    pub root: usize,
}

// ---- reading ---------------------------------------------------------------------------------

struct Reader<'a> {
    objs: Vec<Obj>,
    /// Object number to index: a font or image many things share is read once.
    seen: HashMap<i32, usize>,
    budget: &'a Budget,
    total: usize,
}

fn number_of(v: &PdfObject) -> Option<i32> {
    if v.is_indirect().unwrap_or(false) { v.as_indirect().ok() } else { None }
}

fn name_of(v: Option<PdfObject>) -> Option<String> {
    let v = v?;
    let v = v.resolve().ok().flatten().unwrap_or(v);
    if !v.is_name().unwrap_or(false) {
        return None;
    }
    String::from_utf8(v.as_name().ok()?).ok()
}

fn int_of(v: Option<PdfObject>) -> Option<i32> {
    let v = v?;
    let v = v.resolve().ok().flatten().unwrap_or(v);
    if v.is_number().unwrap_or(false) { v.as_int().ok().or_else(|| v.as_float().ok().map(|f| f as i32)) } else { None }
}

fn real_of(v: Option<PdfObject>) -> Option<f32> {
    let v = v?;
    let v = v.resolve().ok().flatten().unwrap_or(v);
    if v.is_number().unwrap_or(false) { v.as_float().ok().filter(|f| f.is_finite()) } else { None }
}

fn bool_of(v: Option<PdfObject>) -> Option<bool> {
    let v = v?;
    let v = v.resolve().ok().flatten().unwrap_or(v);
    if v.is_bool().unwrap_or(false) { v.as_bool().ok() } else { None }
}

fn dict_of(v: Option<PdfObject>) -> Option<PdfObject> {
    let v = v?;
    let v = v.resolve().ok().flatten().unwrap_or(v);
    v.is_dict().unwrap_or(false).then_some(v)
}

/// Numbers of an array: exactly `n` when `n` is given, else at most `max`.
fn numbers(v: Option<PdfObject>, n: Option<usize>, max: usize) -> Option<Vec<f32>> {
    let v = v?;
    let v = v.resolve().ok().flatten().unwrap_or(v);
    if !v.is_array().unwrap_or(false) {
        return None;
    }
    let len = v.len().ok()?;
    if len > max || n.is_some_and(|n| n != len) {
        return None;
    }
    (0..len as i32).map(|i| real_of(v.get_array(i).ok().flatten())).collect()
}

fn reals(v: &[f32]) -> Val {
    Val::Arr(v.iter().map(|x| Val::Real(*x)).collect())
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<(), Why> {
        if self.total + n > MAX_PACKAGE {
            return Err(TOO_BIG);
        }
        if !self.budget.take_blob(n) {
            return Err(NO_ROOM);
        }
        self.total += n;
        Ok(())
    }

    fn room(&self) -> usize {
        MAX_PACKAGE.saturating_sub(self.total).min(self.budget.blobs_left()).min(self.budget.work_left())
    }

    fn push(&mut self, o: Obj, num: Option<i32>) -> Result<usize, Why> {
        if self.objs.len() >= MAX_OBJECTS {
            return Err(TOO_BIG);
        }
        self.objs.push(o);
        let i = self.objs.len() - 1;
        if let Some(n) = num {
            self.seen.insert(n, i);
        }
        Ok(i)
    }

    /// A stream's decoded bytes, stopped at what is left of the caps.
    fn decoded(&mut self, s: &PdfObject) -> Result<Vec<u8>, Why> {
        let room = self.room();
        if room == 0 {
            return Err(NO_ROOM);
        }
        let r = s.read_stream_capped(room);
        self.budget.spend_work(r.as_ref().map_or(room, Vec::len));
        let b = r.map_err(|e| if e.is_stream_cap() { TOO_BIG } else { DAMAGED })?;
        self.take(b.len())?;
        Ok(b)
    }

    /// A form XObject: the appearance's `/N`, or one it draws.
    fn form(&mut self, o: &PdfObject, depth: usize) -> Result<usize, Why> {
        if depth > MAX_DEPTH {
            return Err(UNSUPPORTED);
        }
        let num = number_of(o);
        if let Some(i) = num.and_then(|n| self.seen.get(&n)) {
            return Ok(*i);
        }
        if !o.is_stream().unwrap_or(false) {
            return Err(NOT_STREAM);
        }
        if name_of(get(o, "Subtype")).is_some_and(|s| s != "Form") {
            return Err(UNSUPPORTED);
        }
        let bbox = numbers(get(o, "BBox"), Some(4), 4).ok_or(DAMAGED)?;
        if !(bbox[2] > bbox[0] || bbox[2] < bbox[0]) || !(bbox[3] > bbox[1] || bbox[3] < bbox[1]) {
            return Err(DAMAGED);
        }
        let mut dict = vec![("Type".to_string(), Val::Name("XObject".into())), ("Subtype".to_string(), Val::Name("Form".into())), ("BBox".to_string(), reals(&bbox))];
        if let Some(m) = numbers(get(o, "Matrix"), Some(6), 6) {
            dict.push(("Matrix".into(), reals(&m)));
        }
        if let Some(g) = dict_of(get(o, "Group")).filter(|g| name_of(get(g, "S")).is_some_and(|s| s == "Transparency")) {
            let mut gd = vec![("S".to_string(), Val::Name("Transparency".into()))];
            for k in ["I", "K"] {
                if let Some(b) = bool_of(get(&g, k)) {
                    gd.push((k.into(), Val::Bool(b)));
                }
            }
            dict.push(("Group".into(), Val::Dict(gd)));
        }
        let data = self.decoded(o)?;
        let used = scan(&data)?;
        let res = dict_of(get(o, "Resources"));
        let lookup = |cat: &str, name: &[u8]| -> Result<PdfObject, Why> {
            let name = std::str::from_utf8(name).map_err(|_| UNRESOLVED)?;
            res.as_ref().and_then(|r| get(r, cat)).and_then(|d| get(&d, name)).ok_or(UNRESOLVED)
        };
        let mut resources: Vec<(String, Val)> = Vec::new();
        for (cat, names) in [("Font", &used.fonts), ("XObject", &used.xobjects), ("ExtGState", &used.gs)] {
            let mut entries: Vec<(String, Val)> = Vec::new();
            for name in names {
                let v = lookup(cat, name)?;
                let idx = match cat {
                    "Font" => self.font(&v)?,
                    "XObject" => {
                        let sub = name_of(get(&v, "Subtype"));
                        match sub.as_deref() {
                            Some("Form") => self.form(&v, depth + 1)?,
                            Some("Image") => self.image(&v, true)?,
                            _ => return Err(UNSUPPORTED),
                        }
                    }
                    _ => self.ext_gstate(&v)?,
                };
                entries.push((String::from_utf8_lossy(name).into_owned(), Val::Ref(idx)));
            }
            if !entries.is_empty() {
                resources.push((cat.into(), Val::Dict(entries)));
            }
        }
        dict.push(("Resources".into(), Val::Dict(resources)));
        self.push(Obj { dict, data: Some(data) }, num)
    }

    fn ext_gstate(&mut self, g: &PdfObject) -> Result<usize, Why> {
        let num = number_of(g);
        if let Some(i) = num.and_then(|n| self.seen.get(&n)) {
            return Ok(*i);
        }
        if dict_of(Some(g.clone())).is_none() {
            return Err(DAMAGED);
        }
        let mut d = vec![("Type".to_string(), Val::Name("ExtGState".into()))];
        for k in ["CA", "ca"] {
            if let Some(v) = real_of(get(g, k)) {
                d.push((k.into(), Val::Real(v.clamp(0.0, 1.0))));
            }
        }
        if let Some(b) = bool_of(get(g, "AIS")) {
            d.push(("AIS".into(), Val::Bool(b)));
        }
        // A blend mode is a name, or a list of them (the first one it knows is used).
        let bm = name_of(get(g, "BM")).or_else(|| {
            let a = get(g, "BM")?;
            name_of(a.get_array(0).ok().flatten())
        });
        if let Some(b) = bm.filter(|b| b.chars().all(|c| c.is_ascii_alphanumeric())) {
            d.push(("BM".into(), Val::Name(b)));
        }
        // A soft mask is another group of drawing: only "none" is taken.
        if name_of(get(g, "SMask")).is_some_and(|s| s == "None") {
            d.push(("SMask".into(), Val::Name("None".into())));
        }
        self.push(Obj { dict: d, data: None }, num)
    }

    fn font(&mut self, f: &PdfObject) -> Result<usize, Why> {
        let num = number_of(f);
        if let Some(i) = num.and_then(|n| self.seen.get(&n)) {
            return Ok(*i);
        }
        let sub = name_of(get(f, "Subtype")).ok_or(DAMAGED)?;
        if sub != "Type1" && sub != "TrueType" {
            return Err(UNSUPPORTED);
        }
        let mut d = vec![("Type".to_string(), Val::Name("Font".into())), ("Subtype".to_string(), Val::Name(sub))];
        if let Some(b) = name_of(get(f, "BaseFont")) {
            d.push(("BaseFont".into(), Val::Name(b)));
        }
        for k in ["FirstChar", "LastChar"] {
            if let Some(v) = int_of(get(f, k)) {
                d.push((k.into(), Val::Int(v.clamp(0, 255))));
            }
        }
        if let Some(w) = numbers(get(f, "Widths"), None, MAX_LIST) {
            d.push(("Widths".into(), reals(&w)));
        }
        if let Some(e) = get(f, "Encoding") {
            if let Some(n) = name_of(Some(e.clone())) {
                d.push(("Encoding".into(), Val::Name(n)));
            } else if let Some(ed) = dict_of(Some(e)) {
                let mut ev = vec![("Type".to_string(), Val::Name("Encoding".into()))];
                if let Some(b) = name_of(get(&ed, "BaseEncoding")) {
                    ev.push(("BaseEncoding".into(), Val::Name(b)));
                }
                if let Some(diff) = get(&ed, "Differences").and_then(|a| a.resolve().ok().flatten()).filter(|a| a.is_array().unwrap_or(false)) {
                    let len = diff.len().unwrap_or(0).min(MAX_LIST);
                    let mut items = Vec::new();
                    for i in 0..len as i32 {
                        let item = diff.get_array(i).ok().flatten();
                        if let Some(n) = name_of(item.clone()) {
                            items.push(Val::Name(n));
                        } else if let Some(v) = int_of(item) {
                            items.push(Val::Int(v));
                        }
                    }
                    ev.push(("Differences".into(), Val::Arr(items)));
                }
                d.push(("Encoding".into(), Val::Dict(ev)));
            }
        }
        if let Some(fd) = dict_of(get(f, "FontDescriptor")) {
            let idx = self.descriptor(&fd)?;
            d.push(("FontDescriptor".into(), Val::Ref(idx)));
        }
        self.push(Obj { dict: d, data: None }, num)
    }

    fn descriptor(&mut self, fd: &PdfObject) -> Result<usize, Why> {
        let num = number_of(fd);
        if let Some(i) = num.and_then(|n| self.seen.get(&n)) {
            return Ok(*i);
        }
        let mut d = vec![("Type".to_string(), Val::Name("FontDescriptor".into()))];
        if let Some(n) = name_of(get(fd, "FontName")) {
            d.push(("FontName".into(), Val::Name(n)));
        }
        if let Some(v) = int_of(get(fd, "Flags")) {
            d.push(("Flags".into(), Val::Int(v)));
        }
        if let Some(b) = numbers(get(fd, "FontBBox"), Some(4), 4) {
            d.push(("FontBBox".into(), reals(&b)));
        }
        for k in ["ItalicAngle", "Ascent", "Descent", "CapHeight", "XHeight", "StemV", "StemH", "AvgWidth", "MaxWidth", "MissingWidth", "Leading"] {
            if let Some(v) = real_of(get(fd, k)) {
                d.push((k.into(), Val::Real(v)));
            }
        }
        for k in ["FontFile", "FontFile2", "FontFile3"] {
            let Some(s) = get(fd, k).filter(|s| s.is_stream().unwrap_or(false)) else { continue };
            let mut sd = Vec::new();
            for l in ["Length1", "Length2", "Length3"] {
                if let Some(v) = int_of(get(&s, l)) {
                    sd.push((l.to_string(), Val::Int(v.max(0))));
                }
            }
            if k == "FontFile3" {
                match name_of(get(&s, "Subtype")) {
                    Some(n) if matches!(n.as_str(), "Type1C" | "CIDFontType0C" | "OpenType") => sd.push(("Subtype".into(), Val::Name(n))),
                    _ => return Err(UNSUPPORTED),
                }
            }
            let data = self.decoded(&s)?;
            let i = self.push(Obj { dict: sd, data: Some(data) }, number_of(&s))?;
            d.push((k.into(), Val::Ref(i)));
        }
        self.push(Obj { dict: d, data: None }, num)
    }

    /// An image. `mask_ok`: it may have a soft mask (the mask itself may not).
    fn image(&mut self, im: &PdfObject, mask_ok: bool) -> Result<usize, Why> {
        let num = number_of(im);
        if let Some(i) = num.and_then(|n| self.seen.get(&n)) {
            return Ok(*i);
        }
        if !im.is_stream().unwrap_or(false) {
            return Err(NOT_STREAM);
        }
        let dim = |k: &str| int_of(get(im, k)).filter(|v| *v > 0).map(|v| v as u64);
        let (w, h) = (dim("Width").ok_or(DAMAGED)?, dim("Height").ok_or(DAMAGED)?);
        if w * h > MAX_PIXELS {
            return Err(TOO_BIG);
        }
        let stencil = bool_of(get(im, "ImageMask")).unwrap_or(false);
        let bpc = if stencil { 1 } else { int_of(get(im, "BitsPerComponent")).ok_or(DAMAGED)? };
        if !matches!(bpc, 1 | 2 | 4 | 8) || (stencil && bpc != 1) {
            return Err(UNSUPPORTED);
        }
        let mut d = vec![
            ("Type".to_string(), Val::Name("XObject".into())),
            ("Subtype".to_string(), Val::Name("Image".into())),
            ("Width".to_string(), Val::Int(w as i32)),
            ("Height".to_string(), Val::Int(h as i32)),
            ("BitsPerComponent".to_string(), Val::Int(bpc)),
        ];
        let mut comps = 1u64;
        if stencil {
            d.push(("ImageMask".into(), Val::Bool(true)));
        } else {
            let cs = get(im, "ColorSpace").ok_or(DAMAGED)?;
            let cs = cs.resolve().ok().flatten().unwrap_or(cs);
            let device = |n: &str| match n {
                "DeviceGray" | "G" => Some(("DeviceGray", 1)),
                "DeviceRGB" | "RGB" => Some(("DeviceRGB", 3)),
                "DeviceCMYK" | "CMYK" => Some(("DeviceCMYK", 4)),
                _ => None,
            };
            let picked = if let Some(n) = name_of(Some(cs.clone())) {
                device(&n)
            } else if cs.is_array().unwrap_or(false) && name_of(cs.get_array(0).ok().flatten()).is_some_and(|n| n == "ICCBased") {
                // The profile is not carried: the device space of its component count stands in for it.
                let n = cs.get_array(1).ok().flatten().and_then(|p| int_of(get(&p, "N")));
                match n {
                    Some(1) => Some(("DeviceGray", 1)),
                    Some(3) => Some(("DeviceRGB", 3)),
                    Some(4) => Some(("DeviceCMYK", 4)),
                    _ => None,
                }
            } else {
                None
            };
            let (name, n) = picked.ok_or(UNSUPPORTED)?;
            comps = n;
            d.push(("ColorSpace".into(), Val::Name(name.into())));
        }
        if let Some(dec) = numbers(get(im, "Decode"), None, 8) {
            d.push(("Decode".into(), reals(&dec)));
        }
        if let Some(b) = bool_of(get(im, "Interpolate")) {
            d.push(("Interpolate".into(), Val::Bool(b)));
        }
        if let Some(m) = get(im, "SMask").filter(|m| !m.is_null().unwrap_or(true)) {
            if !mask_ok {
                return Err(UNSUPPORTED);
            }
            let i = self.image(&m, false)?;
            if !self.objs[i].dict.iter().any(|(k, v)| k == "ColorSpace" && *v == Val::Name("DeviceGray".into())) {
                return Err(UNSUPPORTED);
            }
            d.push(("SMask".into(), Val::Ref(i)));
        }
        // The filters, as the image is stored: Flate and DCT only.
        let filters: Vec<String> = match get(im, "Filter") {
            None => Vec::new(),
            Some(f) => match name_of(Some(f.clone())) {
                Some(n) => vec![n],
                None => {
                    let a = f.resolve().ok().flatten().unwrap_or(f);
                    if !a.is_array().unwrap_or(false) || a.len().unwrap_or(9) > 2 {
                        return Err(UNSUPPORTED);
                    }
                    (0..a.len().unwrap_or(0) as i32).map(|i| name_of(a.get_array(i).ok().flatten()).ok_or(UNSUPPORTED)).collect::<Result<_, _>>()?
                }
            },
        };
        let known = |f: &String| matches!(f.as_str(), "FlateDecode" | "Fl" | "DCTDecode" | "DCT");
        if !filters.iter().all(known) {
            return Err(UNSUPPORTED);
        }
        let flate_only = filters.iter().all(|f| matches!(f.as_str(), "FlateDecode" | "Fl"));
        let expected = (w * comps * bpc as u64).div_ceil(8) * h;
        // The stored bytes: refused by their declared length before they are read, and again by what was read.
        if int_of(get(im, "Length")).is_some_and(|v| v as usize > self.room()) {
            return Err(TOO_BIG);
        }
        if flate_only && !filters.is_empty() {
            // What it inflates to must be what the picture needs: a bomb is stopped here, not at render time.
            let slack = (expected as usize + 65_536).min(self.budget.work_left());
            if slack == 0 {
                return Err(NO_ROOM);
            }
            let r = im.read_stream_capped(slack);
            self.budget.spend_work(r.as_ref().map_or(slack, Vec::len));
            r.map_err(|e| if e.is_stream_cap() { TOO_BIG } else { DAMAGED })?;
        }
        let data = im.read_raw_stream().map_err(|_| DAMAGED)?;
        if data.len() > self.room() {
            return Err(TOO_BIG);
        }
        self.take(data.len())?;
        if !filters.is_empty() {
            let names = filters.iter().map(|f| Val::Name(f.clone())).collect::<Vec<_>>();
            d.push(("Filter".into(), if names.len() == 1 { names.into_iter().next().unwrap_or(Val::Bool(false)) } else { Val::Arr(names) }));
            if let Some(p) = dict_of(get(im, "DecodeParms")) {
                let mut pd = Vec::new();
                for k in ["Predictor", "Colors", "Columns", "BitsPerComponent", "ColorTransform"] {
                    if let Some(v) = int_of(get(&p, k)) {
                        pd.push((k.to_string(), Val::Int(v)));
                    }
                }
                if !pd.is_empty() {
                    d.push(("DecodeParms".into(), Val::Dict(pd)));
                }
            }
        }
        self.push(Obj { dict: d, data: Some(data) }, num)
    }
}

/// The appearance of the annotation or FDF comment `o` (its `/AP /N`), or why it is not carried. `None` when it
/// has none. Charged to `b`.
pub(super) fn read(o: &PdfObject, b: &Budget) -> Option<Result<Package, Why>> {
    let ap = dict_of(get(o, "AP"))?;
    let n = get(&ap, "N")?;
    let mut r = Reader { objs: Vec::new(), seen: HashMap::new(), budget: b, total: 0 };
    Some(r.form(&n, 0).map(|root| Package { objs: r.objs, root }))
}

// ---- the content stream: which resources it names ----------------------------------------------

#[derive(Default)]
struct Used {
    fonts: Vec<Vec<u8>>,
    xobjects: Vec<Vec<u8>>,
    gs: Vec<Vec<u8>>,
}

impl Used {
    fn add(v: &mut Vec<Vec<u8>>, n: &[u8]) -> Result<(), Why> {
        if !v.iter().any(|x| x == n) {
            if v.len() >= MAX_OBJECTS {
                return Err(TOO_BIG);
            }
            v.push(n.to_vec());
        }
        Ok(())
    }
}

enum Tok {
    Name(Vec<u8>),
    Other,
}

fn is_space(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// The resource names a content stream uses, or why it cannot be taken: patterns, shadings, named colour
/// spaces, property lists and inline images need resources this reader does not carry.
fn scan(d: &[u8]) -> Result<Used, Why> {
    let mut used = Used::default();
    let mut ops: Vec<Tok> = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let c = d[i];
        match c {
            c if is_space(c) => i += 1,
            b'%' => {
                while i < d.len() && !matches!(d[i], 10 | 13) {
                    i += 1;
                }
            }
            b'(' => {
                let mut depth = 0usize;
                while i < d.len() {
                    match d[i] {
                        b'\\' => i += 1,
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
                ops.push(Tok::Other);
            }
            b'<' => {
                if d.get(i + 1) == Some(&b'<') {
                    i += 2;
                } else {
                    while i < d.len() && d[i] != b'>' {
                        i += 1;
                    }
                    i += 1;
                }
                ops.push(Tok::Other);
            }
            b'>' | b'[' | b']' | b'{' | b'}' | b')' => i += 1,
            b'/' => {
                i += 1;
                let mut n = Vec::new();
                while i < d.len() && !is_space(d[i]) && !is_delim(d[i]) {
                    if d[i] == b'#' && i + 2 < d.len() + 0 {
                        let hex = std::str::from_utf8(&d[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                        if let Some(b) = hex {
                            n.push(b);
                            i += 3;
                            continue;
                        }
                    }
                    n.push(d[i]);
                    i += 1;
                }
                ops.push(Tok::Name(n));
            }
            _ => {
                let start = i;
                while i < d.len() && !is_space(d[i]) && !is_delim(d[i]) {
                    i += 1;
                }
                let tok = &d[start..i];
                let numeric = tok.iter().all(|c| matches!(c, b'0'..=b'9' | b'+' | b'-' | b'.'));
                if numeric {
                    ops.push(Tok::Other);
                } else {
                    operator(tok, &ops, &mut used)?;
                    ops.clear();
                }
            }
        }
        if ops.len() > 4096 {
            ops.drain(..2048);
        }
    }
    Ok(used)
}

fn operator(op: &[u8], ops: &[Tok], used: &mut Used) -> Result<(), Why> {
    let last = || match ops.last() {
        Some(Tok::Name(n)) => Some(n.as_slice()),
        _ => None,
    };
    match op {
        b"Tf" => {
            let n = ops.len().checked_sub(2).and_then(|i| match &ops[i] {
                Tok::Name(n) => Some(n.as_slice()),
                Tok::Other => None,
            });
            Used::add(&mut used.fonts, n.ok_or(DAMAGED)?)
        }
        b"Do" => Used::add(&mut used.xobjects, last().ok_or(DAMAGED)?),
        b"gs" => Used::add(&mut used.gs, last().ok_or(DAMAGED)?),
        b"cs" | b"CS" => match last() {
            Some(b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK") => Ok(()),
            _ => Err(UNSUPPORTED),
        },
        b"scn" | b"SCN" => {
            if last().is_some() {
                Err(UNSUPPORTED)
            } else {
                Ok(())
            }
        }
        b"sh" | b"BI" | b"ID" => Err(UNSUPPORTED),
        b"BDC" | b"DP" => {
            if matches!(ops.get(1), Some(Tok::Name(_))) {
                Err(UNSUPPORTED)
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

// ---- writing: into a document ---------------------------------------------------------------

/// Writes the package's objects into `doc` (children first) and returns the root. Nothing is registered anywhere:
/// the caller puts the root where it belongs.
pub(super) fn install(doc: &mut PdfDocument, p: &Package) -> Result<PdfObject, EngineError> {
    let mut made: Vec<PdfObject> = Vec::with_capacity(p.objs.len());
    for o in &p.objs {
        let dict = dict_to_pdf(doc, &o.dict, &made)?;
        let r = match &o.data {
            Some(d) => {
                let compressed = o.dict.iter().any(|(k, _)| k == "Filter");
                doc.add_stream(&Buffer::from_bytes(d)?, Some(&dict), compressed)?
            }
            None => doc.add_object(&dict)?,
        };
        made.push(r);
    }
    made.get(p.root).cloned().ok_or_else(|| crate::exchange::bad("the appearance is empty"))
}

fn dict_to_pdf(doc: &PdfDocument, d: &[(String, Val)], made: &[PdfObject]) -> Result<PdfObject, EngineError> {
    let mut out = doc.new_dict()?;
    for (k, v) in d {
        out.dict_put(k.as_str(), val_to_pdf(doc, v, made)?)?;
    }
    Ok(out)
}

fn val_to_pdf(doc: &PdfDocument, v: &Val, made: &[PdfObject]) -> Result<PdfObject, EngineError> {
    Ok(match v {
        Val::Int(i) => PdfObject::new_int(*i)?,
        Val::Real(r) => PdfObject::new_real(*r)?,
        Val::Bool(b) => PdfObject::new_bool(*b),
        Val::Name(n) => PdfObject::new_name(n)?,
        Val::Arr(a) => {
            let mut arr = doc.new_array()?;
            for x in a {
                arr.array_push(val_to_pdf(doc, x, made)?)?;
            }
            arr
        }
        Val::Dict(d) => dict_to_pdf(doc, d, made)?,
        Val::Ref(i) => made.get(*i).cloned().ok_or_else(|| crate::exchange::bad("the appearance refers to an object it does not have"))?,
    })
}

// ---- writing: into a file --------------------------------------------------------------------

fn val_text(v: &Val, first: usize, out: &mut String) {
    match v {
        Val::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Val::Real(r) => out.push_str(&super::n(*r)),
        Val::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Val::Name(n) => out.push_str(&pdf_name(n)),
        Val::Arr(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                val_text(x, first, out);
            }
            out.push(']');
        }
        Val::Dict(d) => dict_text(d, first, out),
        Val::Ref(i) => {
            let _ = write!(out, "{} 0 R", first + i);
        }
    }
}

fn dict_text(d: &[(String, Val)], first: usize, out: &mut String) {
    out.push_str("<<");
    for (k, v) in d {
        let _ = write!(out, " {} ", pdf_name(k));
        val_text(v, first, out);
    }
    out.push_str(" >>");
}

/// The package as PDF objects numbered from `first`: object `i` of the package is object `first + i`.
pub(super) fn write_objects(p: &Package, first: usize) -> Vec<Vec<u8>> {
    p.objs
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let mut dict = o.dict.clone();
            if let Some(d) = &o.data {
                dict.push(("Length".into(), Val::Int(d.len() as i32)));
            }
            let mut s = format!("{} 0 obj\n", first + i);
            dict_text(&dict, first, &mut s);
            let mut out = s.into_bytes();
            if let Some(d) = &o.data {
                out.extend_from_slice(b"\nstream\n");
                out.extend_from_slice(d);
                out.extend_from_slice(b"\nendstream");
            }
            out.extend_from_slice(b"\nendobj\n");
            out
        })
        .collect()
}

/// A small FDF file holding one annotation whose `/AP /N` is the package: what `<appearance>` carries, as base64.
pub(super) fn to_fdf(p: &Package) -> Vec<u8> {
    let mut objs = vec![b"1 0 obj\n<< /FDF << /Annots [2 0 R] >> >>\nendobj\n".to_vec()];
    objs.push(format!("2 0 obj\n<< /Type /Annot /AP << /N {} 0 R >> >>\nendobj\n", 3 + p.root).into_bytes());
    objs.extend(write_objects(p, 3));
    super::assemble_fdf(&objs)
}

/// The appearance of an `<appearance>` element's text (`over`: it was longer than the reader's limit for it).
pub(super) fn from_base64(text: &str, over: bool, b: &Budget) -> Result<Package, Why> {
    use base64::Engine as _;
    if over || text.len() / 4 * 3 > MAX_PACKAGE + (64 << 10) {
        return Err(TOO_BIG);
    }
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(t).map_err(|_| DAMAGED)?;
    from_fdf(&bytes, b)
}

/// The appearance in [`to_fdf`]'s bytes.
pub(super) fn from_fdf(bytes: &[u8], b: &Budget) -> Result<Package, Why> {
    let r = crate::exchange::with_fdf(bytes, |root| {
        let annots = get(root, "Annots").and_then(|a| a.resolve().ok().flatten()).filter(|a| a.is_array().unwrap_or(false));
        let first = annots.and_then(|a| a.get_array(0).ok().flatten());
        Ok(first.and_then(|a| read(&a, b)))
    });
    match r {
        Ok(Some(r)) => r,
        _ => Err(DAMAGED),
    }
}
