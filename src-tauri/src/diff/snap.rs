//! A snapshot of one PDF for comparing: per-page fingerprints, boxes, annotations, form fields,
//! images, plus lazily extracted text. Everything is read through the object model so it does not
//! depend on how the file is laid out (xref streams, compression, object order, incremental updates).

use super::content::{self, ResCat};
use super::types::*;
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Matrix, Rect, TextPageFlags};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub type Hash = [u8; 32];

/// Keys that point back up or sideways in the object graph (or carry volatile bookkeeping).
const SKIP_KEYS: &[&str] = &[
    "Parent", "P", "Prev", "Next", "First", "Last", "StructParent", "StructParents", "Metadata", "PieceInfo",
    "LastModified",
];

/// Stream filters that decode losslessly: the decoded bytes are hashed instead of the encoded ones,
/// so re-compressing a stream does not look like a change.
const LOSSLESS: &[&[u8]] = &[
    b"FlateDecode", b"Fl", b"LZWDecode", b"LZW", b"ASCII85Decode", b"A85", b"ASCIIHexDecode", b"AHx",
    b"RunLengthDecode", b"RL",
];

const MAX_DEPTH: u32 = 28;
const MAX_FORM_DEPTH: u32 = 6;
/// Form expansions per page: forms reused under many names at every nesting level would otherwise
/// expand exponentially (a crafted file could hang the compare). Past it, forms hash structurally.
const MAX_FORMS_PER_PAGE: u32 = 512;

#[derive(Clone, Debug)]
pub struct ImageRef {
    pub name: String,
    pub hash: Hash,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct WidgetInfo {
    pub name: String,
    pub value: Option<String>,
    pub rect: Rect4,
}

/// How a word is set: font family (subset prefix and weight/slant suffix kept as the file names it),
/// size rounded to half a point, fill colour as 0xRRGGBB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Style {
    pub font: String,
    /// Twice the size in points, so halves compare exactly.
    pub size2: i32,
    pub rgb: u32,
}

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub rect: Rect4,
    pub style: Style,
}

/// "ABCDEF+Helvetica-Bold" -> "Helvetica-Bold".
fn font_label(name: &str) -> String {
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest.to_string(),
        _ => name.to_string(),
    }
}

pub struct PageSnap {
    /// Everything the page paints: its content streams plus the resources they actually use.
    pub fp: Hash,
    pub images: Vec<ImageRef>,
    /// Invisible text (render mode 3): an OCR layer.
    pub ocr: bool,
    pub rotation: i32,
    pub boxes: Vec<(&'static str, Option<Rect4>)>,
    pub annots: Vec<AnnotInfo>,
    pub widgets: Vec<WidgetInfo>,
}

pub struct Snap {
    pub doc: PdfDocument,
    pub pages: Vec<PageSnap>,
    memo: RefCell<HashMap<i32, Hash>>,
    active: RefCell<HashSet<i32>>,
    forms_left: Cell<u32>,
    words: RefCell<HashMap<usize, Rc<Vec<Word>>>>,
}

pub fn gd(o: &PdfObject, key: &str) -> Option<PdfObject> {
    let v = o.get_dict(key).ok().flatten()?;
    if v.is_null().unwrap_or(false) { None } else { Some(v) }
}

fn gdi(o: &PdfObject, key: &str) -> Option<PdfObject> {
    let v = o.get_dict_inheritable(key).ok().flatten()?;
    if v.is_null().unwrap_or(false) { None } else { Some(v) }
}

/// PDF text string (PDFDocEncoding ~ Latin-1, or UTF-16BE with BOM, or UTF-8 with BOM).
pub fn text_string(o: &PdfObject) -> Option<String> {
    let b = o.as_bytes().ok()?;
    Some(decode_text(&b))
}

fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else if b.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(&b[3..]).into_owned()
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

fn name_of(o: &PdfObject) -> Option<String> {
    if !o.is_name().unwrap_or(false) {
        return None;
    }
    o.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned())
}

fn nums(o: &PdfObject, n: usize) -> Option<Vec<f32>> {
    if !o.is_array().ok()? || o.len().ok()? < n {
        return None;
    }
    (0..n).map(|i| o.get_array(i as i32).ok().flatten()?.as_float().ok()).collect()
}

fn norm_rect(v: &[f32]) -> Rect4 {
    let r = |x: f32| (x * 100.0).round() / 100.0;
    [r(v[0].min(v[2])), r(v[1].min(v[3])), r(v[0].max(v[2])), r(v[1].max(v[3]))]
}

pub fn rect4(r: Rect) -> Rect4 {
    [r.x0, r.y0, r.x1, r.y1]
}

fn to_page_space(v: &[f32], ctm: &Matrix) -> Rect4 {
    let r = Rect::new(v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])).transform(ctm);
    norm_rect(&[r.x0, r.y0, r.x1, r.y1])
}

impl Snap {
    /// `step(done, total)` is called after each page; it returns false to stop (the open is cancelled).
    pub fn open(bytes: &[u8], side: Side, password: Option<&str>, step: &mut dyn FnMut(u32, u32) -> bool) -> Result<Snap> {
        let doc = open_doc(bytes, side, password)?;
        let n = doc.page_count()?.max(0) as usize;
        let mut snap = Snap {
            doc,
            pages: Vec::with_capacity(n),
            memo: RefCell::default(),
            active: RefCell::default(),
            forms_left: Cell::new(MAX_FORMS_PER_PAGE),
            words: RefCell::default(),
        };
        for i in 0..n {
            let p = snap.read_page(i)?;
            snap.pages.push(p);
            if !step(i as u32 + 1, n as u32) {
                return Err(DiffError::Cancelled);
            }
        }
        Ok(snap)
    }

    fn read_page(&self, i: usize) -> Result<PageSnap> {
        let page = self.doc.find_page(i as i32)?;
        self.forms_left.set(MAX_FORMS_PER_PAGE);
        let res = gdi(&page, "Resources");
        let mut images = Vec::new();
        let mut ocr = false;
        let content = self.page_content(&page);
        let fp = self.paint(&content, res.as_ref(), 0, "", &mut images, &mut ocr);
        let rotation = gdi(&page, "Rotate").and_then(|r| r.as_int().ok()).map(|r| r.rem_euclid(360)).unwrap_or(0);
        let boxes = ["MediaBox", "CropBox", "TrimBox", "BleedBox", "ArtBox"]
            .into_iter()
            .map(|k| (k, gdi(&page, k).and_then(|o| nums(&o, 4)).map(|v| norm_rect(&v))))
            .collect();
        let ctm = page.page_ctm().unwrap_or_else(|_| Matrix::new_scale(1.0, 1.0));
        let (annots, widgets) = self.read_annots(&page, &ctm);
        Ok(PageSnap { fp, images, ocr, rotation, boxes, annots, widgets })
    }

    fn page_content(&self, page: &PdfObject) -> Vec<u8> {
        let mut out = Vec::new();
        match gd(page, "Contents") {
            Some(c) if c.is_array().unwrap_or(false) => {
                for i in 0..c.len().unwrap_or(0) {
                    if let Some(s) = c.get_array(i as i32).ok().flatten() {
                        out.extend(stream_bytes(&s, true));
                        out.push(b'\n');
                    }
                }
            }
            Some(c) if c.is_stream().unwrap_or(false) => out = stream_bytes(&c, true),
            _ => {}
        }
        out
    }

    /// Hash of a content stream and the resources it uses (forms recursively).
    fn paint(
        &self,
        content: &[u8],
        res: Option<&PdfObject>,
        depth: u32,
        prefix: &str,
        images: &mut Vec<ImageRef>,
        ocr: &mut bool,
    ) -> Hash {
        let scan = content::scan(content);
        *ocr |= scan.invisible_text;
        let mut h = blake3::Hasher::new();
        h.update(content);
        for (cat, name) in &scan.used {
            let Some(entry) = res.and_then(|r| gd(r, cat.key())).and_then(|d| {
                let key = String::from_utf8_lossy(name).into_owned();
                gd(&d, &key)
            }) else {
                continue;
            };
            h.update(&[*cat as u8]);
            h.update(name);
            // Not resolved: a stream is only a stream through its indirect reference.
            let subtype = gd(&entry, "Subtype").and_then(|s| name_of(&s));
            if *cat == ResCat::XObject && subtype.as_deref() == Some("Form") && depth < MAX_FORM_DEPTH && self.take_form() {
                let s = &entry;
                let form_res = gd(s, "Resources");
                let form_res = form_res.as_ref().or(res);
                let fname = format!("{prefix}{}/", String::from_utf8_lossy(name));
                let c = stream_bytes(s, true);
                h.update(&self.paint(&c, form_res, depth + 1, &fname, images, ocr));
            } else if *cat == ResCat::XObject && subtype.as_deref() == Some("Image") {
                let ih = self.obj_hash(&entry, 0);
                let s = &entry;
                let dim = |k| gd(s, k).and_then(|v| v.as_int().ok()).unwrap_or(0).max(0) as u32;
                images.push(ImageRef {
                    name: format!("{prefix}{}", String::from_utf8_lossy(name)),
                    hash: ih,
                    width: dim("Width"),
                    height: dim("Height"),
                });
                h.update(&ih);
            } else {
                h.update(&self.obj_hash(&entry, 0));
            }
        }
        *h.finalize().as_bytes()
    }

    fn take_form(&self) -> bool {
        let n = self.forms_left.get();
        self.forms_left.set(n.saturating_sub(1));
        n > 0
    }

    /// Structural hash: independent of object numbers, dictionary key order, number formatting and
    /// (for lossless filters) of how a stream is compressed.
    fn obj_hash(&self, o: &PdfObject, depth: u32) -> Hash {
        if depth > MAX_DEPTH {
            return [1; 32];
        }
        if o.is_indirect().unwrap_or(false) {
            let x = o.as_indirect().unwrap_or(0);
            if let Some(h) = self.memo.borrow().get(&x) {
                return *h;
            }
            if !self.active.borrow_mut().insert(x) {
                return [2; 32];
            }
            // hashed through the indirect reference: streams are only reachable that way
            let h = self.direct_hash(o, depth + 1);
            self.active.borrow_mut().remove(&x);
            self.memo.borrow_mut().insert(x, h);
            return h;
        }
        self.direct_hash(o, depth + 1)
    }

    fn direct_hash(&self, o: &PdfObject, depth: u32) -> Hash {
        let mut h = blake3::Hasher::new();
        let stream = o.is_stream().unwrap_or(false);
        if stream || o.is_dict().unwrap_or(false) {
            let lossless = stream && is_lossless(o);
            h.update(if stream { b"S" } else { b"D" });
            let mut ents: Vec<(String, Hash)> = Vec::new();
            if let Ok(it) = o.dict_iter() {
                for kv in it.flatten() {
                    let k = String::from_utf8_lossy(&kv.0.as_name().unwrap_or_default()).into_owned();
                    if SKIP_KEYS.contains(&k.as_str()) {
                        continue;
                    }
                    if stream && (k == "Length" || k == "DL" || (lossless && (k == "Filter" || k == "DecodeParms" || k == "DP"))) {
                        continue;
                    }
                    ents.push((k, self.obj_hash(&kv.1, depth)));
                }
            }
            ents.sort();
            for (k, v) in ents {
                h.update(k.as_bytes());
                h.update(&[0]);
                h.update(&v);
            }
            if stream {
                h.update(&stream_bytes(o, lossless));
            }
        } else if o.is_array().unwrap_or(false) {
            h.update(b"[");
            for i in 0..o.len().unwrap_or(0) {
                match o.get_array(i as i32).ok().flatten() {
                    Some(e) => h.update(&self.obj_hash(&e, depth)),
                    None => h.update(b"n"),
                };
            }
        } else if o.is_name().unwrap_or(false) {
            h.update(b"/");
            h.update(&o.as_name().unwrap_or_default());
        } else if o.is_string().unwrap_or(false) {
            h.update(b"s");
            h.update(&o.as_bytes().unwrap_or_default());
        } else if o.is_number().unwrap_or(false) {
            h.update(format!("#{:.3}", o.as_float().unwrap_or(0.0)).as_bytes());
        } else if o.is_bool().unwrap_or(false) {
            h.update(if o.as_bool().unwrap_or(false) { b"T" } else { b"F" });
        } else {
            h.update(b"n");
        }
        *h.finalize().as_bytes()
    }

    fn read_annots(&self, page: &PdfObject, ctm: &Matrix) -> (Vec<AnnotInfo>, Vec<WidgetInfo>) {
        let (mut annots, mut widgets) = (Vec::new(), Vec::new());
        let Some(arr) = gd(page, "Annots").filter(|a| a.is_array().unwrap_or(false)) else {
            return (annots, widgets);
        };
        for i in 0..arr.len().unwrap_or(0) {
            let Some(a) = arr.get_array(i as i32).ok().flatten() else { continue };
            if !a.is_dict().unwrap_or(false) {
                continue;
            }
            let subtype = gd(&a, "Subtype").and_then(|s| name_of(&s)).unwrap_or_default();
            let Some(rect) = gd(&a, "Rect").and_then(|r| nums(&r, 4)).map(|v| to_page_space(&v, ctm)) else {
                continue;
            };
            match subtype.as_str() {
                "Link" | "Popup" => {}
                "Widget" => {
                    let value = field_attr(&a, "V").and_then(|v| value_string(&v));
                    widgets.push(WidgetInfo { name: field_name(&a), value, rect });
                }
                _ => {
                    let colour = gd(&a, "C").and_then(|c| {
                        let n = c.len().ok()?;
                        nums(&c, n).filter(|v| matches!(v.len(), 1 | 3 | 4))
                    });
                    annots.push(AnnotInfo {
                        subtype,
                        name: gd(&a, "NM").and_then(|s| text_string(&s)),
                        contents: gd(&a, "Contents").and_then(|s| text_string(&s)).unwrap_or_default(),
                        author: gd(&a, "T").and_then(|s| text_string(&s)),
                        colour,
                        rect,
                    });
                }
            }
        }
        (annots, widgets)
    }

    /// Words of a page (reading order, page space). Annotations and form widgets are not included.
    pub fn words(&self, i: usize) -> Rc<Vec<Word>> {
        if let Some(w) = self.words.borrow().get(&i) {
            return w.clone();
        }
        let w: Vec<Word> = (|| -> Result<Vec<Word>> {
            let list = self.doc.load_page(i as i32)?.to_display_list(false)?;
            let tp = list.to_text_page(TextPageFlags::empty())?;
            Ok(page_words(&tp))
        })()
        .unwrap_or_default();
        let w = Rc::new(w);
        self.words.borrow_mut().insert(i, w.clone());
        w
    }

    /// Bounds of image blocks on the page (page space).
    pub fn image_rects(&self, i: usize) -> Vec<Rect4> {
        (|| -> Result<Vec<Rect4>> {
            let list = self.doc.load_page(i as i32)?.to_display_list(false)?;
            let tp = list.to_text_page(TextPageFlags::PRESERVE_IMAGES)?;
            Ok(tp
                .blocks()
                .filter(|b| b.r#type() == mupdf::text_page::TextBlockType::Image)
                .map(|b| rect4(b.bounds()))
                .collect())
        })()
        .unwrap_or_default()
    }

    /// Opaque filled rectangles painted on the page (page space).
    pub fn opaque_fills(&self, i: usize) -> Vec<Rect4> {
        (|| -> Result<Vec<Rect4>> {
            let d = self.doc.load_page(i as i32)?.drawings()?;
            Ok(d.iter()
                .filter(|d| d.fill.is_some() && d.fill_opacity.unwrap_or(1.0) >= 0.99)
                .map(|d| rect4(d.rect))
                .collect())
        })()
        .unwrap_or_default()
    }
}

/// The words of a text page with their boxes and style (the first character's), in reading order.
/// Splits like `TextPage::words`: on whitespace, within a line.
fn page_words(tp: &mupdf::TextPage) -> Vec<Word> {
    use mupdf::text_page::TextBlockType;
    let mut out = Vec::new();
    for block in tp.blocks().filter(|b| b.r#type() == TextBlockType::Text) {
        for line in block.lines() {
            let mut cur = String::new();
            let mut bounds: Option<Rect> = None;
            let mut style: Option<Style> = None;
            let flush = |cur: &mut String, bounds: &mut Option<Rect>, style: &mut Option<Style>, out: &mut Vec<Word>| {
                if let (false, Some(b), Some(s)) = (cur.is_empty(), bounds.take(), style.take()) {
                    out.push(Word { text: std::mem::take(cur), rect: rect4(b), style: s });
                }
                cur.clear();
            };
            for ch in line.chars() {
                let Some(c) = ch.char() else { continue };
                if c.is_whitespace() {
                    flush(&mut cur, &mut bounds, &mut style, &mut out);
                    continue;
                }
                let q = Rect::from(ch.quad());
                bounds = Some(match bounds {
                    Some(b) => b.r#union(&q),
                    None => q,
                });
                if style.is_none() {
                    let font = ch
                        .font()
                        .and_then(|f| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| font_label(f.name()))).ok())
                        .unwrap_or_default();
                    style = Some(Style { font, size2: (ch.size() * 2.0).round() as i32, rgb: ch.argb() & 0x00FF_FFFF });
                }
                cur.push(c);
            }
            flush(&mut cur, &mut bounds, &mut style, &mut out);
        }
    }
    out
}

pub fn open_doc(bytes: &[u8], side: Side, password: Option<&str>) -> Result<PdfDocument> {
    let _ = mupdf::Context::get();
    if !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return Err(DiffError::NotPdf { side });
    }
    let mut doc = PdfDocument::from_bytes(bytes).map_err(|_| DiffError::NotPdf { side })?;
    if doc.needs_password()? {
        match password {
            None => return Err(DiffError::NeedsPassword { side }),
            Some(p) => {
                if !doc.authenticate(p)? {
                    return Err(DiffError::BadPassword { side });
                }
            }
        }
    }
    Ok(doc)
}

fn is_lossless(s: &PdfObject) -> bool {
    let ok = |n: &PdfObject| name_of(n).is_some_and(|n| LOSSLESS.contains(&n.as_bytes()));
    match gd(s, "Filter") {
        None => true,
        Some(f) if f.is_array().unwrap_or(false) => (0..f.len().unwrap_or(0))
            .all(|i| f.get_array(i as i32).ok().flatten().is_some_and(|n| ok(&n))),
        Some(f) => ok(&f),
    }
}

fn stream_bytes(s: &PdfObject, lossless_ok: bool) -> Vec<u8> {
    if lossless_ok && is_lossless(s) {
        if let Ok(b) = s.read_stream() {
            return b;
        }
    }
    s.read_raw_stream().unwrap_or_default()
}

/// Inheritable form field attribute (V, FT, ...), looking up /Parent.
fn field_attr(o: &PdfObject, key: &str) -> Option<PdfObject> {
    let mut cur = o.try_clone().ok()?;
    for _ in 0..16 {
        if let Some(v) = gd(&cur, key) {
            return Some(v);
        }
        cur = gd(&cur, "Parent")?;
    }
    None
}

/// Fully qualified field name: the /T of the field and its ancestors, dot-joined.
fn field_name(o: &PdfObject) -> String {
    let mut parts = Vec::new();
    let mut cur = o.try_clone().ok();
    for _ in 0..16 {
        let Some(c) = cur else { break };
        if let Some(t) = gd(&c, "T").and_then(|t| text_string(&t)) {
            parts.push(t);
        }
        cur = gd(&c, "Parent");
    }
    parts.reverse();
    parts.join(".")
}

fn value_string(v: &PdfObject) -> Option<String> {
    if v.is_string().unwrap_or(false) {
        text_string(v)
    } else if v.is_name().unwrap_or(false) {
        name_of(v)
    } else if v.is_array().unwrap_or(false) {
        let items: Vec<String> =
            (0..v.len().unwrap_or(0)).filter_map(|i| v.get_array(i as i32).ok().flatten().and_then(|e| value_string(&e))).collect();
        Some(items.join(", "))
    } else if v.is_dict().unwrap_or(false) {
        Some("(signature)".into())
    } else {
        None
    }
}
