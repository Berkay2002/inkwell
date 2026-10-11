//! Adding, updating and removing marks on a document, and Bates numbering across files.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/edit/src/lib.rs @ b8ddf71 (`add_header_footer`,
//! `add_watermark`, `add_background`, `place_tagged`, `remove_marks`, `marks_present`), rebuilt on
//! MuPDF's object API.

use super::geometry::{M, View, n};
use super::text::{Measure, face, first_undrawable, literal, undrawable_message};
use super::{AddRequest, Background, Bates, Date, Mark, MarkKind, Picture, Rgb, Style, Watermark, bates_fits, bates_text, expand};
use crate::engine::edit::{open_source, rewrite_opts, write_doc, write_new};
use crate::engine::{EngineError, Result};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Buffer, Document, Image};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

fn invalid(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

fn name(s: &str) -> Result<PdfObject> {
    Ok(PdfObject::new_name(s)?)
}

fn real(v: f64) -> Result<PdfObject> {
    Ok(PdfObject::new_real(v as f32)?)
}

// ---- reading and writing a page's content list -----------------------------------------------------

/// The page's `/Contents` as a list of streams (a single stream is a list of one).
fn contents_list(page: &PdfObject) -> Result<Vec<PdfObject>> {
    let mut out = Vec::new();
    if let Some(c) = page.get_dict("Contents")? {
        if c.is_null()? {
            return Ok(out);
        }
        if c.is_array()? {
            for i in 0..c.len()? {
                if let Some(item) = c.get_array(i as i32)? {
                    out.push(item);
                }
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn set_contents(doc: &PdfDocument, page: &mut PdfObject, list: Vec<PdfObject>) -> Result<()> {
    let mut a = doc.new_array_with_capacity(list.len() as i32 + 1)?;
    for o in list {
        a.array_push(o)?;
    }
    page.dict_put("Contents", a)?;
    Ok(())
}

/// The `/PCMark` tag in a content stream's dictionary: a mark kind, or `Wrap` for the q/Q wrapper.
fn tag_of(o: &PdfObject) -> Option<String> {
    let v = o.get_dict("PCMark").ok().flatten()?;
    v.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned())
}

fn is_wrap(o: &PdfObject) -> bool {
    tag_of(o).as_deref() == Some("Wrap")
}

fn add_stream(doc: &mut PdfDocument, bytes: &[u8], tag: &str, spec: Option<&str>) -> Result<PdfObject> {
    let mut dict = doc.new_dict()?;
    dict.dict_put("PCMark", name(tag)?)?;
    if let Some(s) = spec {
        dict.dict_put("IWSpec", PdfObject::new_string(s)?)?;
    }
    let buf = Buffer::from_bytes(bytes)?;
    Ok(doc.add_stream(&buf, Some(&dict), false)?)
}

/// Puts a mark's content on page `index`: prepended (behind the page), or appended after wrapping
/// the original content in q/Q.
fn place(doc: &mut PdfDocument, index: u32, tag: &str, content: &[u8], behind: bool, spec: Option<&str>) -> Result<()> {
    let mut page = doc.find_page(index as i32)?;
    let mut list = contents_list(&page)?;
    let mark = add_stream(doc, content, tag, spec)?;
    if behind {
        list.insert(0, mark);
    } else {
        let wrapped = list.iter().any(is_wrap);
        let original = list.iter().any(|o| tag_of(o).is_none());
        if !wrapped && original {
            let open = add_stream(doc, b"q %Inkwell\n", "Wrap", None)?;
            let close = add_stream(doc, b"Q %Inkwell\n", "Wrap", None)?;
            // Marks behind the page stay outside the wrapper.
            let split = list.iter().position(|o| tag_of(o).is_none()).unwrap_or(list.len());
            list.insert(split, open);
            list.push(close);
        }
        list.push(mark);
    }
    set_contents(doc, &mut page, list)
}

// ---- resources --------------------------------------------------------------------------------------

/// `res[key]` as a dictionary, created when missing.
fn sub(res: &mut PdfObject, doc: &PdfDocument, key: &str) -> Result<PdfObject> {
    if let Some(d) = res.get_dict(key)? {
        if d.is_dict()? {
            return Ok(d);
        }
    }
    res.dict_put(key, doc.new_dict()?)?;
    res.get_dict(key)?.ok_or_else(|| invalid("the page's resources could not be changed"))
}

/// What one run of marks needs on every page, made once per document.
#[derive(Default)]
struct Shared {
    fonts: HashMap<String, PdfObject>,
}

impl Shared {
    /// Registers the face of `style` in page `index`'s resources; returns its resource name.
    fn font(&mut self, doc: &mut PdfDocument, index: u32, style: &Style) -> Result<String> {
        let (base, res_name) = face(style);
        if !self.fonts.contains_key(&res_name) {
            let mut d = doc.new_dict()?;
            d.dict_put("Type", name("Font")?)?;
            d.dict_put("Subtype", name("Type1")?)?;
            d.dict_put("BaseFont", name(base)?)?;
            d.dict_put("Encoding", name("WinAnsiEncoding")?)?;
            let o = doc.add_object(&d)?;
            self.fonts.insert(res_name.clone(), o);
        }
        let mut res = doc.load_pdf_page(index as i32)?.resources()?;
        let mut fonts = sub(&mut res, doc, "Font")?;
        fonts.dict_put(res_name.as_str(), self.fonts[&res_name].clone())?;
        Ok(res_name)
    }
}

/// `/IWGS<percent> gs` for `opacity`, registered in page `index`'s resources. None at full opacity.
fn opacity_state(doc: &mut PdfDocument, index: u32, opacity: f64) -> Result<Option<String>> {
    let o = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 };
    if o >= 0.999 {
        return Ok(None);
    }
    let key = format!("IWGS{}", (o * 100.0).round() as i64);
    let mut gs = doc.new_dict()?;
    gs.dict_put("Type", name("ExtGState")?)?;
    gs.dict_put("CA", real(o)?)?;
    gs.dict_put("ca", real(o)?)?;
    let mut res = doc.load_pdf_page(index as i32)?.resources()?;
    let mut states = sub(&mut res, doc, "ExtGState")?;
    states.dict_put(key.as_str(), gs)?;
    Ok(Some(key))
}

fn rg(c: Rgb) -> String {
    let c = c.map(|x| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 });
    format!("{} {} {} rg", n(c[0]), n(c[1]), n(c[2]))
}

/// An artifact section: `q`, the marked-content begin, the view `cm`, the body, and the ends.
fn section(kind: MarkKind, subtype: &str, attached: Option<&str>, view: M, oc: Option<&str>, body: &str) -> String {
    let mut s = format!("q\n/Artifact <</Type /Pagination /Subtype /{subtype}");
    if let Some(a) = attached {
        s.push_str(&format!(" /Attached [/{a}]"));
    }
    s.push_str(&format!(" /PCMark /{}>> BDC\n", kind.tag()));
    s.push_str(&format!("{} {} {} {} {} {} cm\n", n(view[0]), n(view[1]), n(view[2]), n(view[3]), n(view[4]), n(view[5])));
    if let Some(oc) = oc {
        s.push_str(&format!("/OC /{oc} BDC\n"));
    }
    s.push_str(body);
    if oc.is_some() {
        s.push_str("EMC\n");
    }
    s.push_str("EMC\nQ\n");
    s
}

// ---- optional content for "show on screen / in print" ----------------------------------------------

/// An optional content group named "Watermark" that is on for the screen and/or for print, with
/// the document's default configuration pointing at it the way Acrobat's own are.
fn watermark_group(doc: &mut PdfDocument, screen: bool, print: bool) -> Result<PdfObject> {
    let state = |on: bool| if on { "ON" } else { "OFF" };
    let mut usage = doc.new_dict()?;
    let mut view = doc.new_dict()?;
    view.dict_put("ViewState", name(state(screen))?)?;
    usage.dict_put("View", view)?;
    let mut prt = doc.new_dict()?;
    prt.dict_put("PrintState", name(state(print))?)?;
    prt.dict_put("Subtype", name("Watermark")?)?;
    usage.dict_put("Print", prt)?;
    let mut g = doc.new_dict()?;
    g.dict_put("Type", name("OCG")?)?;
    g.dict_put("Name", PdfObject::new_string("Watermark")?)?;
    g.dict_put("PCMark", name("Watermark")?)?;
    g.dict_put("Usage", usage)?;
    let group = doc.add_object(&g)?;

    let mut catalog = doc.catalog()?;
    let mut props = match catalog.get_dict("OCProperties")? {
        Some(p) if p.is_dict()? => p,
        _ => {
            catalog.dict_put("OCProperties", doc.new_dict()?)?;
            catalog.get_dict("OCProperties")?.ok_or_else(|| invalid("the document's layers could not be changed"))?
        }
    };
    let mut groups = match props.get_dict("OCGs")? {
        Some(a) if a.is_array()? => a,
        _ => {
            props.dict_put("OCGs", doc.new_array()?)?;
            props.get_dict("OCGs")?.ok_or_else(|| invalid("the document's layers could not be changed"))?
        }
    };
    groups.array_push(group.clone())?;
    let mut d = sub(&mut props, doc, "D")?;
    // A configuration that starts with everything off needs the group listed as on.
    let base_off = d.get_dict("BaseState")?.is_some_and(|b| b.as_name().is_ok_and(|n| n == b"OFF"));
    if base_off {
        let mut on = match d.get_dict("ON")? {
            Some(a) if a.is_array()? => a,
            _ => {
                d.dict_put("ON", doc.new_array()?)?;
                d.get_dict("ON")?.ok_or_else(|| invalid("the document's layers could not be changed"))?
            }
        };
        on.array_push(group.clone())?;
    }
    let mut auto = match d.get_dict("AS")? {
        Some(a) if a.is_array()? => a,
        _ => {
            d.dict_put("AS", doc.new_array()?)?;
            d.get_dict("AS")?.ok_or_else(|| invalid("the document's layers could not be changed"))?
        }
    };
    for event in ["View", "Print"] {
        let mut found = None;
        for i in 0..auto.len()? {
            if let Some(e) = auto.get_array(i as i32)? {
                if e.get_dict("Event")?.is_some_and(|v| v.as_name().is_ok_and(|n| n == event.as_bytes())) {
                    found = Some(e);
                    break;
                }
            }
        }
        let mut entry = match found {
            Some(e) => e,
            None => {
                let mut e = doc.new_dict()?;
                e.dict_put("Event", name(event)?)?;
                let mut cat = doc.new_array()?;
                cat.array_push(name(event)?)?;
                e.dict_put("Category", cat)?;
                e.dict_put("OCGs", doc.new_array()?)?;
                auto.array_push(e)?;
                auto.get_array(auto.len()? as i32 - 1)?.ok_or_else(|| invalid("the document's layers could not be changed"))?
            }
        };
        let mut list = match entry.get_dict("OCGs")? {
            Some(a) if a.is_array()? => a,
            _ => {
                entry.dict_put("OCGs", doc.new_array()?)?;
                entry.get_dict("OCGs")?.ok_or_else(|| invalid("the document's layers could not be changed"))?
            }
        };
        list.array_push(group.clone())?;
    }
    Ok(group)
}

/// Takes the layers marks made out of the document once no page uses them.
fn drop_unused_groups(doc: &mut PdfDocument) -> Result<()> {
    let count = doc.page_count()?.max(0);
    let mut used = BTreeSet::new();
    for i in 0..count {
        let page = doc.find_page(i)?;
        if let Some(res) = page.get_dict_inheritable("Resources")? {
            if let Some(props) = res.get_dict("Properties")? {
                for k in 0..props.dict_len()? {
                    if let Some(v) = props.get_dict_val(k as i32)? {
                        if v.is_indirect()? {
                            used.insert(v.as_indirect()?);
                        }
                    }
                }
            }
        }
    }
    let catalog = doc.catalog()?;
    let Some(props) = catalog.get_dict("OCProperties")? else { return Ok(()) };
    let ours = |o: &PdfObject| -> bool {
        o.is_indirect().unwrap_or(false)
            && o.get_dict("PCMark").ok().flatten().is_some()
            && !used.contains(&o.as_indirect().unwrap_or(-1))
    };
    // Rebuild each list of groups without ours that no page refers to.
    let filter = |doc: &PdfDocument, arr: &PdfObject| -> Result<PdfObject> {
        let mut out = doc.new_array()?;
        for i in 0..arr.len()? {
            if let Some(o) = arr.get_array(i as i32)? {
                if !ours(&o) {
                    out.array_push(o)?;
                }
            }
        }
        Ok(out)
    };
    let mut props = props;
    if let Some(a) = props.get_dict("OCGs")? {
        if a.is_array()? {
            let f = filter(doc, &a)?;
            props.dict_put("OCGs", f)?;
        }
    }
    if let Some(mut d) = props.get_dict("D")? {
        for key in ["ON", "OFF"] {
            if let Some(a) = d.get_dict(key)? {
                if a.is_array()? {
                    let f = filter(doc, &a)?;
                    d.dict_put(key, f)?;
                }
            }
        }
        if let Some(auto) = d.get_dict("AS")? {
            if auto.is_array()? {
                for i in 0..auto.len()? {
                    if let Some(mut e) = auto.get_array(i as i32)? {
                        if let Some(a) = e.get_dict("OCGs")? {
                            if a.is_array()? {
                                let f = filter(doc, &a)?;
                                e.dict_put("OCGs", f)?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// ---- pictures ---------------------------------------------------------------------------------------

/// A picture in the document: an image XObject (drawn into the unit square) or a form.
struct Loaded {
    obj: PdfObject,
    /// The size to fit to the page; only its proportions matter.
    size: (f64, f64),
    image: bool,
}

const MAX_PICTURE_BYTES: u64 = 200 << 20;

fn load_picture(doc: &mut PdfDocument, p: &Picture) -> Result<Loaded> {
    let meta = std::fs::metadata(&p.file).map_err(|e| EngineError::Io(e.to_string()))?;
    if meta.len() > MAX_PICTURE_BYTES {
        return Err(invalid("that picture file is too large"));
    }
    let bytes = std::fs::read(&p.file).map_err(|e| EngineError::Io(e.to_string()))?;
    if bytes.starts_with(b"%PDF") {
        let mut d = Document::from_bytes(&bytes, "application/pdf")?;
        if d.needs_password()? {
            match p.password.as_deref() {
                None => return Err(EngineError::NeedsPassword),
                Some(pw) if !d.authenticate(pw)? => return Err(EngineError::BadPassword),
                _ => {}
            }
        }
        let src = PdfDocument::try_from(d)?;
        return page_form(doc, &src, p.page);
    }
    let img = Image::from_bytes(&bytes).map_err(|_| invalid("that file is not a picture or PDF this app can read"))?;
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    if w < 1.0 || h < 1.0 {
        return Err(invalid("that picture is empty"));
    }
    let obj = doc.add_image(&img)?;
    Ok(Loaded { obj, size: (w, h), image: true })
}

/// Page `index` of `src` as a form XObject of `doc`: its content, with its resources grafted, drawn
/// in the page's display space (so `/Rotate` and the crop box are honoured).
fn page_form(doc: &mut PdfDocument, src: &PdfDocument, index: u32) -> Result<Loaded> {
    let count = src.page_count()?.max(0) as u32;
    if index >= count {
        return Err(EngineError::BadPage(index));
    }
    let page = src.load_pdf_page(index as i32)?;
    let view = View::of(&page)?;
    let obj = src.find_page(index as i32)?;
    let mut bytes = Vec::new();
    for s in contents_list(&obj)? {
        if s.is_stream()? {
            bytes.extend(s.read_stream()?);
            bytes.push(b'\n');
        }
    }
    let mut map = doc.new_graft_map()?;
    let mut dict = doc.new_dict()?;
    dict.dict_put("Type", name("XObject")?)?;
    dict.dict_put("Subtype", name("Form")?)?;
    let crop = page.crop_box()?;
    let mut bbox = doc.new_array()?;
    for v in [crop.x0, crop.y0, crop.x1, crop.y1] {
        bbox.array_push(real(f64::from(v))?)?;
    }
    dict.dict_put("BBox", bbox)?;
    let to_display = view.to_display();
    let mut matrix = doc.new_array()?;
    for v in to_display {
        matrix.array_push(real(v)?)?;
    }
    dict.dict_put("Matrix", matrix)?;
    if let Some(res) = obj.get_dict_inheritable("Resources")? {
        dict.dict_put("Resources", map.graft_object(&res)?)?;
    }
    let buf = Buffer::from_bytes(&bytes)?;
    let form = doc.add_stream(&buf, Some(&dict), false)?;
    Ok(Loaded { obj: form, size: (view.w, view.h), image: false })
}

/// Draws the picture fitted to the page at `scale`, centred plus `offset`, turned by `rotation`.
fn picture_ops(src: &Loaded, page: (f64, f64), scale: f64, rotation: f64, offset: [f64; 2], res_name: &str) -> String {
    let (w, h) = page;
    let (sw, sh) = (src.size.0.max(0.01), src.size.1.max(0.01));
    let k = (w / sw).min(h / sh) * if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (dw, dh) = (sw * k, sh * k);
    let (s, c) = rotation.to_radians().sin_cos();
    let (cx, cy) = (w / 2.0 + offset[0], h / 2.0 + offset[1]);
    let place = if src.image {
        format!("{} 0 0 {} {} {} cm", n(dw), n(dh), n(-dw / 2.0), n(-dh / 2.0))
    } else {
        format!("{} 0 0 {} {} {} cm", n(k), n(k), n(-dw / 2.0), n(-dh / 2.0))
    };
    format!("{} {} {} {} {} {} cm\n{place}\n/{res_name} Do\n", n(c), n(s), n(-s), n(c), n(cx), n(cy))
}

fn add_picture_resource(doc: &PdfDocument, index: u32, src: &Loaded) -> Result<String> {
    let num = src.obj.as_indirect()?;
    let key = format!("IWPic{num}");
    let mut res = doc.load_pdf_page(index as i32)?.resources()?;
    let mut xo = sub(&mut res, doc, "XObject")?;
    xo.dict_put(key.as_str(), src.obj.clone())?;
    Ok(key)
}

// ---- adding -----------------------------------------------------------------------------------------

/// ASCII-only JSON, so the spec survives as a PDF string without an encoding question.
fn ascii_json(mark: &Mark) -> Option<String> {
    let s = serde_json::to_string(mark).ok()?;
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut b = [0u16; 2];
            for u in c.encode_utf16(&mut b) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    // Bound the entry: a spec this large is a mistake, and the marks still work without it.
    (out.len() <= 16 << 10).then_some(out)
}

fn check_pages(pages: &[u32], count: u32) -> Result<Vec<u32>> {
    let set: BTreeSet<u32> = pages.iter().copied().collect();
    if set.is_empty() {
        return Err(invalid("no pages selected"));
    }
    if let Some(&p) = set.iter().find(|&&p| p >= count) {
        return Err(EngineError::BadPage(p));
    }
    Ok(set.into_iter().collect())
}

fn style_ok(s: &Style, allow_fit: bool) -> Result<()> {
    let size_ok = s.size.is_finite() && s.size <= 600.0 && (s.size > 0.0 || (allow_fit && s.size == 0.0));
    if !size_ok || !s.color.iter().all(|c| c.is_finite()) {
        return Err(invalid("the font size is not valid"));
    }
    Ok(())
}

fn margins_ok(m: &[f64; 4]) -> Result<()> {
    if m.iter().all(|v| v.is_finite() && *v >= 0.0 && *v <= 5000.0) {
        Ok(())
    } else {
        Err(invalid("the margins are not valid"))
    }
}

fn drawable<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<()> {
    match first_undrawable(lines) {
        Some(c) => Err(invalid(undrawable_message(c))),
        None => Ok(()),
    }
}

/// The text drawing of one group of three slots (headers, or footers) as stream bytes: literal
/// strings hold WinAnsi bytes above 0x7F, which a `String` cannot carry.
fn slots_bytes(m: &mut Measure, view: &View, header: bool, texts: &[String], style: &Style, margins: &[f64; 4], font: &str) -> Option<Vec<u8>> {
    let size = style.size;
    let [top, bottom, left, right] = *margins;
    let line_h = size * 1.2;
    let mut out: Vec<u8> = format!("BT\n/{font} {} Tf\n{}\n", n(size), rg(style.color)).into_bytes();
    let mut underlines = String::new();
    let mut any = false;
    for (slot, text) in texts.iter().enumerate() {
        if text.trim().is_empty() {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (li, line) in lines.iter().enumerate() {
            let tw = m.width(style, line);
            let x = match slot {
                0 => left,
                1 => (view.w - tw) / 2.0,
                _ => view.w - right - tw,
            };
            let y = if header {
                view.h - top - size * 0.8 - li as f64 * line_h
            } else {
                bottom + size * 0.2 + (lines.len() - 1 - li) as f64 * line_h
            };
            out.extend(format!("1 0 0 1 {} {} Tm ", n(x), n(y)).bytes());
            out.extend(literal(line));
            out.extend_from_slice(b" Tj\n");
            any = true;
            if style.underline {
                underlines.push_str(&format!("{} {} {} {} re f\n", n(x), n(y - size * 0.15), n(tw), n((size * 0.06).max(0.4))));
            }
        }
    }
    if !any {
        return None;
    }
    out.extend_from_slice(b"ET\n");
    if !underlines.is_empty() {
        out.extend(format!("{}\n{underlines}", rg(style.color)).bytes());
    }
    Some(out)
}

/// Header and footer (and Bates) text for page `index`, as the content of one mark stream.
fn text_marks(
    m: &mut Measure,
    view: &View,
    kind: MarkKind,
    texts: &[String; 6],
    style: &Style,
    margins: &[f64; 4],
    font: &str,
) -> Option<Vec<u8>> {
    let mut content = Vec::new();
    if let Some(b) = slots_bytes(m, view, true, &texts[..3], style, margins, font) {
        content.extend(section_bytes_oc(kind, "Header", Some("Top"), view.matrix, None, &b));
    }
    if let Some(b) = slots_bytes(m, view, false, &texts[3..], style, margins, font) {
        content.extend(section_bytes_oc(kind, "Footer", Some("Bottom"), view.matrix, None, &b));
    }
    (!content.is_empty()).then_some(content)
}

/// Adds `req.mark` to `req.pages`. Returns the pages touched.
pub fn add(doc: &mut PdfDocument, req: &AddRequest) -> Result<Vec<u32>> {
    let count = doc.page_count()?.max(0) as u32;
    let pages = check_pages(&req.pages, count)?;
    validate(&req.mark, pages.len() as u64)?;
    if req.replace {
        remove(doc, req.mark.kind(), &pages)?;
    }
    let spec = ascii_json(&req.mark);
    let spec = spec.as_deref();
    let mut shared = Shared::default();
    let mut m = Measure::default();
    match &req.mark {
        Mark::HeaderFooter(hf) => {
            for (k, &i) in pages.iter().enumerate() {
                let view = View::of(&doc.load_pdf_page(i as i32)?)?;
                let number = hf.start_number + k as u32;
                let texts: [String; 6] = std::array::from_fn(|s| expand(&hf.text[s], number, count as usize, req.date));
                let font = shared.font(doc, i, &hf.style)?;
                if let Some(content) = text_marks(&mut m, &view, MarkKind::HeaderFooter, &texts, &hf.style, &hf.margins, &font) {
                    place(doc, i, MarkKind::HeaderFooter.tag(), &content, false, spec)?;
                }
            }
        }
        Mark::Bates(b) => {
            for (k, &i) in pages.iter().enumerate() {
                let view = View::of(&doc.load_pdf_page(i as i32)?)?;
                let mut texts: [String; 6] = Default::default();
                texts[b.slot as usize] = bates_text(b, b.start + k as u64);
                let font = shared.font(doc, i, &b.style)?;
                if let Some(content) = text_marks(&mut m, &view, MarkKind::Bates, &texts, &b.style, &b.margins, &font) {
                    place(doc, i, MarkKind::Bates.tag(), &content, false, spec)?;
                }
            }
        }
        Mark::Watermark(w) => add_watermark(doc, &pages, w, spec, &mut shared, &mut m)?,
        Mark::Background(bg) => add_background(doc, &pages, bg, spec)?,
    }
    Ok(pages)
}

fn validate(mark: &Mark, page_count: u64) -> Result<()> {
    match mark {
        Mark::HeaderFooter(hf) => {
            if hf.text.iter().all(|t| t.trim().is_empty()) {
                return Err(invalid("Type the header or footer text first."));
            }
            hf.text.iter().try_for_each(|t| drawable(t.lines()))?;
            style_ok(&hf.style, false)?;
            margins_ok(&hf.margins)?;
            let last = u64::from(hf.start_number) + page_count.saturating_sub(1);
            if last > u64::from(u32::MAX) {
                return Err(invalid("The start number leaves no room for the page numbers."));
            }
        }
        Mark::Bates(b) => {
            if b.slot > 5 {
                return Err(invalid("Choose where the Bates number goes."));
            }
            if !(3..=15).contains(&b.digits) {
                return Err(invalid("A Bates number has 3 to 15 digits."));
            }
            drawable([b.prefix.as_str(), b.suffix.as_str()])?;
            style_ok(&b.style, false)?;
            margins_ok(&b.margins)?;
            if !bates_fits(b.digits, b.start, page_count) {
                return Err(invalid(format!("{} pages starting at {} need more than {} digits.", page_count, b.start, b.digits)));
            }
        }
        Mark::Watermark(w) => {
            if w.picture.is_none() {
                if w.text.trim().is_empty() {
                    return Err(invalid("Type the watermark text first."));
                }
                drawable(w.text.lines())?;
            }
            style_ok(&w.style, true)?;
            if !(w.opacity.is_finite() && w.rotation.is_finite() && w.scale.is_finite() && w.offset.iter().all(|v| v.is_finite())) {
                return Err(invalid("The watermark settings are not valid."));
            }
            if !w.show_on_screen && !w.show_in_print {
                return Err(invalid("A watermark must show on screen, in print, or both."));
            }
        }
        Mark::Background(bg) => {
            if !(bg.opacity.is_finite() && bg.scale.is_finite() && bg.color.iter().all(|c| c.is_finite())) {
                return Err(invalid("The background settings are not valid."));
            }
        }
    }
    Ok(())
}

fn add_watermark(doc: &mut PdfDocument, pages: &[u32], w: &Watermark, spec: Option<&str>, shared: &mut Shared, m: &mut Measure) -> Result<()> {
    let loaded = match &w.picture {
        Some(p) => Some(load_picture(doc, p)?),
        None => None,
    };
    let group = if w.show_on_screen && w.show_in_print { None } else { Some(watermark_group(doc, w.show_on_screen, w.show_in_print)?) };
    let text = w.text.trim();
    let lines: Vec<&str> = text.lines().collect();
    for &i in pages {
        let view = View::of(&doc.load_pdf_page(i as i32)?)?;
        let gs = opacity_state(doc, i, w.opacity)?;
        let mut body = String::new();
        if let Some(gs) = &gs {
            body.push_str(&format!("/{gs} gs\n"));
        }
        let mut bytes: Vec<u8>;
        if let Some(src) = &loaded {
            let key = add_picture_resource(doc, i, src)?;
            body.push_str(&picture_ops(src, (view.w, view.h), w.scale, w.rotation, w.offset, &key));
            bytes = body.into_bytes();
        } else {
            let font = shared.font(doc, i, &w.style)?;
            let unit = Style { size: 1.0, ..w.style.clone() };
            let widest = lines.iter().map(|l| m.width(&unit, l)).fold(0.0, f64::max).max(0.01);
            let diag = (view.w * view.w + view.h * view.h).sqrt();
            let size = if w.style.size > 0.0 { w.style.size } else { (diag * w.scale.max(0.05) / widest).clamp(6.0, 400.0) };
            let st = Style { size, ..w.style.clone() };
            let (s, c) = w.rotation.to_radians().sin_cos();
            let (cx, cy) = (view.w / 2.0 + w.offset[0], view.h / 2.0 + w.offset[1]);
            body.push_str(&format!(
                "{} {} {} {} {} {} cm\nBT\n/{font} {} Tf\n{}\n",
                n(c),
                n(s),
                n(-s),
                n(c),
                n(cx),
                n(cy),
                n(size),
                rg(w.style.color)
            ));
            bytes = body.into_bytes();
            let line_h = size * 1.15;
            let total = line_h * (lines.len() as f64 - 1.0);
            for (li, line) in lines.iter().enumerate() {
                let tw = m.width(&st, line);
                bytes.extend(format!("1 0 0 1 {} {} Tm ", n(-tw / 2.0), n(total / 2.0 - li as f64 * line_h - size * 0.35)).bytes());
                bytes.extend(literal(line));
                bytes.extend_from_slice(b" Tj\n");
            }
            bytes.extend_from_slice(b"ET\n");
        }
        let oc = match &group {
            Some(g) => {
                let mut res = doc.load_pdf_page(i as i32)?.resources()?;
                let mut props = sub(&mut res, doc, "Properties")?;
                props.dict_put("IWOC", g.clone())?;
                Some("IWOC")
            }
            None => None,
        };
        let content = section_bytes_oc(MarkKind::Watermark, "Watermark", None, view.matrix, oc, &bytes);
        place(doc, i, MarkKind::Watermark.tag(), &content, w.behind, spec)?;
    }
    Ok(())
}

/// An artifact section around `body` bytes; `attached` names the page edge a header or footer hangs from.
fn section_bytes_oc(kind: MarkKind, subtype: &str, attached: Option<&str>, view: M, oc: Option<&str>, body: &[u8]) -> Vec<u8> {
    let head = section(kind, subtype, attached, view, oc, "\u{0}");
    let (before, after) = head.split_once('\u{0}').unwrap_or((&head, ""));
    let mut out = before.as_bytes().to_vec();
    out.extend_from_slice(body);
    out.extend_from_slice(after.as_bytes());
    out
}

fn add_background(doc: &mut PdfDocument, pages: &[u32], bg: &Background, spec: Option<&str>) -> Result<()> {
    let loaded = match &bg.picture {
        Some(p) => Some(load_picture(doc, p)?),
        None => None,
    };
    for &i in pages {
        let view = View::of(&doc.load_pdf_page(i as i32)?)?;
        let gs = opacity_state(doc, i, bg.opacity)?;
        let mut body = String::new();
        if let Some(gs) = &gs {
            body.push_str(&format!("/{gs} gs\n"));
        }
        match &loaded {
            Some(src) => {
                let key = add_picture_resource(doc, i, src)?;
                body.push_str(&picture_ops(src, (view.w, view.h), bg.scale, 0.0, [0.0; 2], &key));
            }
            None => body.push_str(&format!("{}\n0 0 {} {} re f\n", rg(bg.color), n(view.w), n(view.h))),
        }
        let content = section_bytes_oc(MarkKind::Background, "Background", None, view.matrix, None, body.as_bytes());
        place(doc, i, MarkKind::Background.tag(), &content, true, spec)?;
    }
    Ok(())
}

// ---- removing and reading ---------------------------------------------------------------------------

/// Removes the marks of `kind` from `pages` and returns how many streams went. When no mark is
/// left on top of a page, the q/Q wrapper goes too, restoring the original content list.
pub fn remove(doc: &mut PdfDocument, kind: MarkKind, pages: &[u32]) -> Result<usize> {
    let count = doc.page_count()?.max(0) as u32;
    let pages = check_pages(pages, count)?;
    let mut removed = 0;
    for &i in &pages {
        let mut page = doc.find_page(i as i32)?;
        let list = contents_list(&page)?;
        let before = list.len();
        let mut kept: Vec<PdfObject> = list.into_iter().filter(|o| tag_of(o).as_deref() != Some(kind.tag())).collect();
        removed += before - kept.len();
        let close = kept.iter().rposition(is_wrap);
        let on_top = close.is_some_and(|c| kept[c + 1..].iter().any(|o| tag_of(o).is_some_and(|t| t != "Wrap")));
        if !on_top {
            kept.retain(|o| !is_wrap(o));
        }
        if kept.len() != before {
            set_contents(doc, &mut page, kept)?;
            if kind == MarkKind::Watermark {
                // The page's name for the layer goes with its watermark.
                if let Some(res) = page.get_dict_inheritable("Resources")? {
                    if let Some(mut props) = res.get_dict("Properties")? {
                        if props.get_dict("IWOC")?.is_some() {
                            props.dict_delete("IWOC")?;
                        }
                    }
                }
            }
        }
    }
    if removed > 0 && kind == MarkKind::Watermark {
        drop_unused_groups(doc)?;
    }
    Ok(removed)
}

/// One kind of mark a document has.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KindInfo {
    pub kind: MarkKind,
    /// 0-based pages carrying it.
    pub pages: Vec<u32>,
    /// The settings of the first one, when it was made by this app.
    pub mark: Option<serde_json::Value>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub kinds: Vec<KindInfo>,
}

/// Which marks the document has, on which pages, and the settings they were made with.
pub fn info(doc: &PdfDocument) -> Result<Info> {
    let count = doc.page_count()?.max(0);
    let mut found: Vec<KindInfo> = Vec::new();
    for i in 0..count {
        let page = doc.find_page(i)?;
        for s in contents_list(&page)? {
            let Some(kind) = tag_of(&s).and_then(|t| MarkKind::from_tag(&t)) else { continue };
            let pos = match found.iter().position(|k| k.kind == kind) {
                Some(p) => p,
                None => {
                    found.push(KindInfo { kind, pages: vec![], mark: None });
                    found.len() - 1
                }
            };
            let k = &mut found[pos];
            if k.pages.last() != Some(&(i as u32)) {
                k.pages.push(i as u32);
            }
            if k.mark.is_none() {
                if let Some(v) = s.get_dict("IWSpec")?.and_then(|v| v.as_string().ok()) {
                    k.mark = serde_json::from_str(&v).ok();
                }
            }
        }
    }
    Ok(Info { kinds: found })
}

// ---- Bates numbering across files -------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BatesFile {
    pub input: String,
    pub output: String,
    pub pages: u32,
    /// The first and last number written.
    pub first: u64,
    pub last: u64,
}

/// The name of the numbered copy of `input` in `dir`: `<name> - Bates.pdf`, then `(2)`… so an
/// earlier run, or a file of the same name, is never overwritten.
fn output_name(dir: &Path, input: &Path) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Document".into());
    (1..)
        .map(|k| if k == 1 { dir.join(format!("{stem} - Bates.pdf")) } else { dir.join(format!("{stem} - Bates ({k}).pdf")) })
        .find(|p| !p.exists())
        .expect("a free name")
}

/// Numbers the files in order, continuing the count from one to the next, and writes each as a
/// copy in `dir`. Nothing is written when any file cannot be opened or the numbers run out.
pub fn bates_files(inputs: &[String], bates: &Bates, date: Date, dir: &Path) -> Result<Vec<BatesFile>> {
    if inputs.is_empty() {
        return Err(invalid("Add at least one file."));
    }
    if !dir.is_dir() {
        return Err(invalid(format!("{} is not a folder", dir.display())));
    }
    let _ = mupdf::Context::get();
    let mut docs = Vec::new();
    let mut total = 0u64;
    for p in inputs {
        let d = open_source(p, None).map_err(|e| match e {
            EngineError::NeedsPassword | EngineError::BadPassword => {
                invalid(format!("{} is password protected. Open it, remove its password and try again.", Path::new(p).display()))
            }
            e => e,
        })?;
        if !crate::protect::Rights::from_p(d.permissions().bits() as i32).may_extract() {
            return Err(invalid(format!(
                "{} doesn't allow changes. Open it with its permissions password and remove its security first.",
                Path::new(p).file_name().unwrap_or_default().to_string_lossy()
            )));
        }
        total += d.page_count()?.max(0) as u64;
        docs.push(d);
    }
    if !bates_fits(bates.digits, bates.start, total) {
        return Err(invalid(format!("{total} pages starting at {} need more than {} digits.", bates.start, bates.digits)));
    }
    let mut next = bates.start;
    let mut done: Vec<BatesFile> = Vec::new();
    for (p, mut d) in inputs.iter().zip(docs) {
        let pages = d.page_count()?.max(0) as u32;
        let b = Bates { start: next, ..bates.clone() };
        let req = AddRequest { mark: Mark::Bates(b), pages: (0..pages).collect(), replace: true, date };
        add(&mut d, &req)?;
        let bytes = write_doc(&d, rewrite_opts(), 1 << 20)?;
        let out = output_name(dir, Path::new(p));
        let r = write_new(&out, &bytes, pages);
        if let Err(e) = r {
            let written: Vec<_> = done.iter().map(|f| f.output.clone()).collect();
            return Err(if written.is_empty() {
                e
            } else {
                EngineError::Io(format!("{} could not be written ({e}); already written: {}", out.display(), written.join(", ")))
            });
        }
        done.push(BatesFile {
            input: p.clone(),
            output: out.to_string_lossy().into_owned(),
            pages,
            first: next,
            last: next + u64::from(pages).saturating_sub(1),
        });
        next += u64::from(pages);
    }
    Ok(done)
}
