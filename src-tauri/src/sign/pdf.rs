//! Digital signatures in the PDF: signing (PAdES B-B, `ETSI.CAdES.detached`) as an incremental
//! update appended to a file's bytes, and listing and validating the signatures a file holds.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/sign/src/pdf.rs @ b8ddf71: the signature
//! dictionary, fixed-width `/ByteRange` and `/Contents` placeholders patched after the write,
//! DocMDP certification and the validation checks. pdfcraft's own object model is replaced by
//! MuPDF: the update is written by MuPDF's incremental save of a throwaway document opened from
//! the bytes, so the bytes before it are kept exactly and earlier signatures stay valid.

use super::crypto::cms::{self, SignedData};
use super::crypto::{Certificate, DigestAlg, DigitalId, SignError};
use mupdf::pdf::{PdfDocument, PdfObject, PdfWriteOptions};
use mupdf::{Buffer, Rect};
use serde::Serialize;
use std::io::Write;

type Result<T> = std::result::Result<T, SignError>;

fn pdf_err(e: mupdf::Error) -> SignError {
    SignError::Pdf(e.to_string())
}

fn refuse(m: impl Into<String>) -> SignError {
    SignError::Pdf(m.into())
}

/// The signing time, as `/M` (`D:20261011143200+02'00'`) and as the appearance shows it
/// (`2026.10.11 14:32:00 +02'00'`, Acrobat's form).
#[derive(Clone, Debug)]
pub struct SignDate {
    pub pdf: String,
    pub display: String,
}

impl SignDate {
    /// `unix` seconds, shown at `offset_min` minutes east of UTC (the user's zone, from the UI).
    pub fn new(unix: i64, offset_min: i32) -> SignDate {
        let offset_min = offset_min.clamp(-14 * 60, 14 * 60);
        let local = unix + offset_min as i64 * 60;
        let (y, mo, d) = civil(local.div_euclid(86_400));
        let s = local.rem_euclid(86_400);
        let (h, mi, se) = (s / 3600, s / 60 % 60, s % 60);
        let sign = if offset_min < 0 { '-' } else { '+' };
        let (oh, om) = (offset_min.abs() / 60, offset_min.abs() % 60);
        SignDate {
            pdf: format!("D:{y:04}{mo:02}{d:02}{h:02}{mi:02}{se:02}{sign}{oh:02}'{om:02}'"),
            display: format!("{y:04}.{mo:02}.{d:02} {h:02}:{mi:02}:{se:02} {sign}{oh:02}'{om:02}'"),
        }
    }

    pub fn now(offset_min: i32) -> SignDate {
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        SignDate::new(unix, offset_min)
    }
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// What to sign and how it looks.
#[derive(Clone, Debug, Default)]
pub struct SignRequest {
    /// Sign this existing, empty signature field (its full name).
    pub field: Option<String>,
    /// Or create a field on this page (0-based) at this rect, in page space (points, y down, as
    /// the viewer shows the page). `None`: an invisible signature.
    pub page: u32,
    pub rect: Option<[f32; 4]>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    /// Certify with these DocMDP permissions: 1 no changes, 2 form filling and signing, 3 also comments.
    pub certify: Option<u8>,
    /// A picture for the left of the appearance (a handwritten signature), PNG or JPEG.
    pub image: Option<Vec<u8>>,
    pub date: Option<SignDate>,
}

/// Fixed-width `/ByteRange` placeholder values, patched after the write.
const BR_MARK: &str = "0 1111111111 2222222222 3333333333";

/// Appends a signature to `base` (a complete PDF) and returns the signed file: `base` followed by
/// one incremental update. `password` opens an encrypted file (refused for now).
pub fn sign(base: &[u8], id: &DigitalId, req: &SignRequest) -> Result<Vec<u8>> {
    let mut doc = PdfDocument::from_bytes(base).map_err(pdf_err)?;
    let trailer = doc.trailer().map_err(pdf_err)?;
    if trailer.get_dict("Encrypt").map_err(pdf_err)?.is_some() {
        // MuPDF would encrypt the placeholder with the rest of the update.
        return Err(SignError::Unsupported("signing an encrypted PDF (remove its password first)".into()));
    }
    let fields = sig_fields(&doc)?;
    let signed_before = fields.iter().any(|f| f.value.is_some());
    // DocMDP (ISO 32000-1 12.8.2.2): a certification that allows no changes ends the document.
    if let Some(p) = docmdp_level(&doc)? {
        if p == 1 {
            return Err(refuse("the document is certified with no changes allowed; it can't be signed again"));
        }
    }
    if req.certify.is_some() && signed_before {
        return Err(refuse("only the first signature can certify a document; this one is already signed"));
    }
    let date = req.date.clone().unwrap_or_else(|| SignDate::now(0));
    let alg = id.key.preferred_digest();
    let reserve = 8192 + id.cert.raw.len() + id.chain.iter().map(|c| c.raw.len()).sum::<usize>();
    let name = id.cert.display_name();

    // The signature dictionary.
    let mut v = format!(
        "<</Type/Sig/Filter/Adobe.PPKLite/SubFilter/ETSI.CAdES.detached/ByteRange[{BR_MARK}]/Contents<{}>/M({})/Name{}",
        "0".repeat(reserve * 2),
        date.pdf,
        text_string(&name)
    );
    for (key, val) in [("Reason", &req.reason), ("Location", &req.location), ("ContactInfo", &req.contact)] {
        if let Some(s) = val.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            v += &format!("/{key}{}", text_string(s));
        }
    }
    v += "/Prop_Build<</App<</Name/Inkwell>>>>";
    if let Some(p) = req.certify {
        v += &format!("/Reference[<</Type/SigRef/TransformMethod/DocMDP/TransformParams<</Type/TransformParams/P {}/V/1.2>>>>]", p.clamp(1, 3));
    }
    v += ">>";
    let sig = doc.add_object(&doc.new_object_from_str(&v).map_err(pdf_err)?).map_err(pdf_err)?;
    let mut catalog = doc.catalog().map_err(pdf_err)?;
    if req.certify.is_some() {
        let mut perms = doc.new_dict().map_err(pdf_err)?;
        perms.dict_put("DocMDP", sig.clone()).map_err(pdf_err)?;
        catalog.dict_put("Perms", perms).map_err(pdf_err)?;
    }

    // The field: an existing empty one, or a new widget on the page.
    let (mut widget, bbox, rotate) = match req.field.as_deref() {
        Some(n) => {
            let f = fields.into_iter().find(|f| f.name == n).ok_or_else(|| refuse(format!("there is no signature field {n:?}")))?;
            if f.value.is_some() {
                return Err(refuse(format!("the field {n:?} is already signed")));
            }
            let mut field = f.field.clone();
            field.dict_put("V", sig.clone()).map_err(pdf_err)?;
            let rotate = page_rotation(&doc.find_page(f.page as i32).map_err(pdf_err)?);
            (f.widget, f.size, rotate)
        }
        None => {
            let count = doc.page_count().map_err(pdf_err)?;
            if req.page as i32 >= count {
                return Err(refuse(format!("page {} does not exist", req.page + 1)));
            }
            let mut page = doc.find_page(req.page as i32).map_err(pdf_err)?;
            let rotate = page_rotation(&page);
            let taken: Vec<String> = sig_fields(&doc)?.into_iter().map(|f| f.name).collect();
            let fname = (1..).map(|i| format!("Signature{i}")).find(|n| !taken.contains(n)).unwrap_or_default();
            let (rect, size) = match req.rect {
                Some(r) => {
                    let ctm = page.page_ctm().map_err(pdf_err)?;
                    let inv = ctm.invert().ok_or_else(|| refuse("the page has no usable transform"))?;
                    let pr = Rect { x0: r[0].min(r[2]), y0: r[1].min(r[3]), x1: r[0].max(r[2]), y1: r[1].max(r[3]) };
                    if pr.width() < 4.0 || pr.height() < 4.0 {
                        return Err(refuse("the signature area is too small"));
                    }
                    (pr.transform(&inv), [pr.width(), pr.height()])
                }
                None => (Rect { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 }, [0.0, 0.0]),
            };
            let w = doc
                .new_object_from_str(&format!(
                    "<</Type/Annot/Subtype/Widget/FT/Sig/T{}/F 132/Rect[{} {} {} {}]>>",
                    text_string(&fname),
                    num(rect.x0),
                    num(rect.y0),
                    num(rect.x1),
                    num(rect.y1)
                ))
                .map_err(pdf_err)?;
            let mut w = doc.add_object(&w).map_err(pdf_err)?;
            w.dict_put("V", sig.clone()).map_err(pdf_err)?;
            w.dict_put("P", page.clone()).map_err(pdf_err)?;
            match page.get_dict("Annots").map_err(pdf_err)? {
                Some(mut a) if a.is_array().map_err(pdf_err)? => a.array_push(w.clone()).map_err(pdf_err)?,
                _ => {
                    let mut a = doc.new_array().map_err(pdf_err)?;
                    a.array_push(w.clone()).map_err(pdf_err)?;
                    page.dict_put("Annots", a).map_err(pdf_err)?;
                }
            }
            let mut af = match catalog.get_dict("AcroForm").map_err(pdf_err)? {
                Some(af) if af.is_dict().map_err(pdf_err)? => af,
                _ => {
                    let af = doc.new_dict().map_err(pdf_err)?;
                    catalog.dict_put("AcroForm", af).map_err(pdf_err)?;
                    catalog.get_dict("AcroForm").map_err(pdf_err)?.ok_or_else(|| refuse("AcroForm"))?
                }
            };
            match af.get_dict("Fields").map_err(pdf_err)? {
                Some(mut f) if f.is_array().map_err(pdf_err)? => f.array_push(w.clone()).map_err(pdf_err)?,
                _ => {
                    let mut f = doc.new_array().map_err(pdf_err)?;
                    f.array_push(w.clone()).map_err(pdf_err)?;
                    af.dict_put("Fields", f).map_err(pdf_err)?;
                }
            }
            (w, size, rotate)
        }
    };
    // SignaturesExist | AppendOnly.
    if let Some(mut af) = catalog.get_dict("AcroForm").map_err(pdf_err)? {
        af.dict_put("SigFlags", PdfObject::new_int(3).map_err(pdf_err)?).map_err(pdf_err)?;
    }

    // The appearance.
    if bbox[0] > 0.0 && bbox[1] > 0.0 {
        let lines = detail_lines(&name, &date, req);
        let ap = appearance(&mut doc, bbox, rotate, &name, &lines, req.image.as_deref())?;
        let mut apd = doc.new_dict().map_err(pdf_err)?;
        apd.dict_put("N", ap).map_err(pdf_err)?;
        widget.dict_put("AP", apd).map_err(pdf_err)?;
    }

    // Write the update with the placeholders, then patch them.
    let mut opts = PdfWriteOptions::default();
    opts.set_incremental(true);
    let mut out = Vec::with_capacity(base.len() + reserve * 2 + (64 << 10));
    doc.write_to_with_options(&mut out, opts).map_err(pdf_err)?;
    drop(doc);
    if out.len() <= base.len() || out[..base.len()] != *base {
        return Err(refuse("the document could not be updated incrementally"));
    }
    let br_at = find_once(&out[base.len()..], BR_MARK.as_bytes()).map(|i| i + base.len()).ok_or_else(|| refuse("the byte range placeholder was not written"))?;
    let hole = format!("<{}>", "0".repeat(reserve * 2));
    let start = find_once(&out[base.len()..], hole.as_bytes()).map(|i| i + base.len()).ok_or_else(|| refuse("the signature placeholder was not written"))?;
    let end = start + hole.len();
    let mut br = format!("0 {start} {end} {}", out.len() - end).into_bytes();
    if br.len() > BR_MARK.len() {
        return Err(refuse("the document is too large to sign"));
    }
    br.resize(BR_MARK.len(), b' ');
    out[br_at..br_at + BR_MARK.len()].copy_from_slice(&br);
    let digest = alg.digest(&[&out[..start], &out[end..]]);
    let cms = cms::sign_detached(&id.key, &id.cert, &id.chain, alg, &digest)?;
    if cms.len() > reserve {
        return Err(refuse("the signature does not fit its placeholder"));
    }
    let hex: Vec<u8> = cms.iter().flat_map(|b| format!("{b:02X}").into_bytes()).collect();
    out[start + 1..start + 1 + hex.len()].copy_from_slice(&hex);
    Ok(out)
}

fn find_once(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let first = hay.windows(needle.len()).position(|w| w == needle)?;
    hay[first + 1..].windows(needle.len()).all(|w| w != needle).then_some(first)
}

/// A number as PDF writes it: no exponent, at most 3 decimals.
fn num(x: f32) -> String {
    let s = format!("{x:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// A PDF text string: PDFDocEncoding when plain ASCII, else UTF-16BE with a BOM. Always hex, so
/// nothing in it needs escaping.
fn text_string(s: &str) -> String {
    let bytes: Vec<u8> = if s.chars().all(|c| (' '..='~').contains(&c)) {
        s.bytes().collect()
    } else {
        [0xFE, 0xFF].into_iter().chain(s.encode_utf16().flat_map(u16::to_be_bytes)).collect()
    };
    let mut out = String::with_capacity(bytes.len() * 2 + 2);
    out.push('<');
    for b in bytes {
        out += &format!("{b:02X}");
    }
    out.push('>');
    out
}

fn page_rotation(page: &PdfObject) -> i32 {
    let r = page.get_dict_inheritable("Rotate").ok().flatten().and_then(|r| r.as_int().ok()).unwrap_or(0);
    r.rem_euclid(360) / 90 * 90
}

fn detail_lines(name: &str, date: &SignDate, req: &SignRequest) -> Vec<String> {
    let mut lines = vec![format!("Digitally signed by {name}"), format!("Date: {}", date.display)];
    if let Some(r) = req.reason.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        lines.push(format!("Reason: {r}"));
    }
    if let Some(l) = req.location.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        lines.push(format!("Location: {l}"));
    }
    lines
}

/// Text as Helvetica's WinAnsiEncoding bytes in a PDF literal string; characters it doesn't
/// have become '?'.
fn winansi(s: &str) -> (String, Vec<char>) {
    let mut out = String::from("(");
    let mut chars = Vec::new();
    for c in s.chars() {
        let c = if (' '..='~').contains(&c) || ('\u{A0}'..='\u{FF}').contains(&c) { c } else { '?' };
        chars.push(c);
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x80 => out.push(c),
            c => out += &format!("\\{:03o}", c as u32),
        }
    }
    out.push(')');
    (out, chars)
}

/// Helvetica advance widths, in text space units per point of font size.
struct Metrics(Option<mupdf::Font>);

impl Metrics {
    fn new() -> Metrics {
        Metrics(mupdf::Font::new("Helvetica").ok())
    }

    fn width(&self, chars: &[char]) -> f32 {
        let Some(f) = &self.0 else { return chars.len() as f32 * 0.55 };
        chars.iter().map(|&c| f.encode_character(c as i32).and_then(|g| f.advance_glyph(g)).unwrap_or(0.55)).sum()
    }
}

/// The signature's normal appearance: the picture (or the signer's name, large) on the left
/// and the details on the right; stacked when the box is narrow. `size` is the box as the page
/// shows it; `rotate` the page's `/Rotate`, undone by the form's `/Matrix` so it reads upright.
fn appearance(doc: &mut PdfDocument, size: [f32; 2], rotate: i32, name: &str, lines: &[String], image: Option<&[u8]>) -> Result<PdfObject> {
    let [w, h] = size;
    let m = Metrics::new();
    let pad = (h * 0.06).clamp(1.0, 6.0);
    let wide = w >= h * 2.2;
    let (left, right) = if wide {
        (Rect { x0: pad, y0: pad, x1: w * 0.45, y1: h - pad }, Rect { x0: w * 0.47, y0: pad, x1: w - pad, y1: h - pad })
    } else {
        (Rect { x0: pad, y0: h * 0.45, x1: w - pad, y1: h - pad }, Rect { x0: pad, y0: pad, x1: w - pad, y1: h * 0.43 })
    };
    let mut content = String::new();
    let resources = String::from("/Font<</Helv<</Type/Font/Subtype/Type1/BaseFont/Helvetica/Encoding/WinAnsiEncoding>>>>");
    let mut img_obj = None;
    if let Some(bytes) = image {
        let (xobj, iw, ih) = add_image(doc, bytes)?;
        let scale = (left.width() / iw).min(left.height() / ih);
        let (dw, dh) = (iw * scale, ih * scale);
        let x = left.x0 + (left.width() - dw) / 2.0;
        let y = left.y0 + (left.height() - dh) / 2.0;
        content += &format!("q {} 0 0 {} {} {} cm /Img Do Q\n", num(dw), num(dh), num(x), num(y));
        img_obj = Some(xobj);
    } else {
        let (s, chars) = winansi(name);
        let tw = m.width(&chars).max(0.1);
        let fs = (left.width() / tw).min(left.height() * 0.6).max(1.0);
        let y = left.y0 + (left.height() - fs * 0.72) / 2.0;
        content += &format!("BT /Helv {} Tf 0.12 0.16 0.27 rg {} {} Td {s} Tj ET\n", num(fs), num(left.x0), num(y));
    }
    let encoded: Vec<(String, Vec<char>)> = lines.iter().map(|l| winansi(l)).collect();
    let widest = encoded.iter().map(|(_, c)| m.width(c)).fold(0.1f32, f32::max);
    let fs = (right.width() / widest).min(right.height() / (lines.len() as f32 * 1.2)).max(1.0);
    let lead = fs * 1.2;
    let top = right.y0 + (right.height() + lines.len() as f32 * lead) / 2.0 - fs;
    content += &format!("BT /Helv {} Tf 0.1 0.11 0.13 rg {} TL {} {} Td\n", num(fs), num(lead), num(right.x0), num(top));
    for (i, (s, _)) in encoded.iter().enumerate() {
        content += &if i == 0 { format!("{s} Tj\n") } else { format!("T* {s} Tj\n") };
    }
    content += "ET\n";
    let mut dict = format!("<</Type/XObject/Subtype/Form/BBox[0 0 {} {}]", num(w), num(h));
    if rotate != 0 {
        // Map the upright box onto the rotated page so the text reads upright there.
        let (bw, bh) = (w, h);
        let mtx = match rotate {
            90 => format!("[0 1 -1 0 {} 0]", num(bh)),
            180 => format!("[-1 0 0 -1 {} {}]", num(bw), num(bh)),
            _ => format!("[0 -1 1 0 0 {}]", num(bw)),
        };
        dict += &format!("/Matrix{mtx}");
    }
    dict += "/Resources<<";
    dict += &resources;
    dict += ">>>>";
    let form = doc.new_object_from_str(&dict).map_err(pdf_err)?;
    if let Some(xobj) = img_obj {
        let mut res = form.get_dict("Resources").map_err(pdf_err)?.ok_or_else(|| refuse("Resources"))?;
        let mut xo = doc.new_dict().map_err(pdf_err)?;
        xo.dict_put("Img", xobj).map_err(pdf_err)?;
        res.dict_put("XObject", xo).map_err(pdf_err)?;
    }
    let buf = Buffer::from_bytes(content.as_bytes()).map_err(pdf_err)?;
    doc.add_stream(&buf, Some(&form), false).map_err(pdf_err)
}

/// An image XObject (with an `/SMask` for transparency) from PNG or JPEG bytes; returns it and
/// its pixel size.
fn add_image(doc: &mut PdfDocument, bytes: &[u8]) -> Result<(PdfObject, f32, f32)> {
    const MAX: u32 = 2400;
    let img = image::load_from_memory(bytes).map_err(|e| SignError::Malformed(format!("signature picture: {e}")))?;
    let img = if img.width() > MAX || img.height() > MAX { img.thumbnail(MAX, MAX) } else { img };
    let rgba = img.to_rgba8();
    let (iw, ih) = rgba.dimensions();
    let mut rgb = Vec::with_capacity((iw * ih * 3) as usize);
    let mut alpha = Vec::with_capacity((iw * ih) as usize);
    for p in rgba.pixels() {
        rgb.extend_from_slice(&p.0[..3]);
        alpha.push(p.0[3]);
    }
    let zip = |data: &[u8]| -> Result<Buffer> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(data).map_err(|e| SignError::Crypto(e.to_string()))?;
        let out = z.finish().map_err(|e| SignError::Crypto(e.to_string()))?;
        Buffer::from_bytes(&out).map_err(pdf_err)
    };
    let head = |cs: &str| format!("<</Type/XObject/Subtype/Image/Width {iw}/Height {ih}/ColorSpace/{cs}/BitsPerComponent 8/Filter/FlateDecode>>");
    let mut obj = doc.new_object_from_str(&head("DeviceRGB")).map_err(pdf_err)?;
    if alpha.iter().any(|&a| a != 255) {
        let mask = doc.new_object_from_str(&head("DeviceGray")).map_err(pdf_err)?;
        let mask = doc.add_stream(&zip(&alpha)?, Some(&mask), true).map_err(pdf_err)?;
        obj.dict_put("SMask", mask).map_err(pdf_err)?;
    }
    let obj = doc.add_stream(&zip(&rgb)?, Some(&obj), true).map_err(pdf_err)?;
    Ok((obj, iw as f32, ih as f32))
}

// ---- reading ------------------------------------------------------------------------------------

/// A signature field's widget on a page.
struct SigField {
    name: String,
    page: u32,
    widget: PdfObject,
    /// The dictionary holding `/T` and `/V` (the widget itself, or its parent field).
    field: PdfObject,
    /// The signature dictionary, when signed.
    value: Option<PdfObject>,
    /// The widget's rect in page space (points, y down).
    rect: [f32; 4],
    /// Its size as the page shows it.
    size: [f32; 2],
}

fn sig_fields(doc: &PdfDocument) -> Result<Vec<SigField>> {
    let mut out = Vec::new();
    let n = doc.page_count().map_err(pdf_err)?;
    for p in 0..n {
        let page = doc.find_page(p).map_err(pdf_err)?;
        let Some(annots) = page.get_dict("Annots").map_err(pdf_err)? else { continue };
        if !annots.is_array().map_err(pdf_err)? {
            continue;
        }
        let ctm = page.page_ctm().map_err(pdf_err)?;
        for a in annots.array_iter().map_err(pdf_err)? {
            let a = a.map_err(pdf_err)?;
            if !a.is_dict().unwrap_or(false) || a.get_dict("Subtype").ok().flatten().and_then(|s| s.as_name().ok()).as_deref() != Some(b"Widget") {
                continue;
            }
            let ft = a.get_dict_inheritable("FT").ok().flatten().and_then(|s| s.as_name().ok());
            if ft.as_deref() != Some(b"Sig") {
                continue;
            }
            let field = if a.get_dict("T").ok().flatten().is_some() { a.clone() } else { a.get_dict("Parent").ok().flatten().unwrap_or_else(|| a.clone()) };
            let name = full_name(&field);
            let value = field.get_dict("V").ok().flatten().filter(|v| v.is_dict().unwrap_or(false));
            let r = rect_of(&a).transform(&ctm);
            out.push(SigField { name, page: p as u32, widget: a, field, value, rect: [r.x0, r.y0, r.x1, r.y1], size: [r.width(), r.height()] });
        }
    }
    Ok(out)
}

fn full_name(field: &PdfObject) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(field.clone());
    for _ in 0..32 {
        let Some(f) = cur else { break };
        if let Some(t) = f.get_dict("T").ok().flatten().and_then(|t| t.as_string().ok()) {
            parts.push(t);
        }
        cur = f.get_dict("Parent").ok().flatten();
    }
    parts.reverse();
    parts.join(".")
}

fn rect_of(a: &PdfObject) -> Rect {
    let v: Vec<f32> = a
        .get_dict("Rect")
        .ok()
        .flatten()
        .and_then(|r| r.array_iter().ok().map(|it| it.filter_map(|x| x.ok()?.as_float().ok()).collect()))
        .unwrap_or_default();
    match v.as_slice() {
        [a, b, c, d] => Rect { x0: a.min(*c), y0: b.min(*d), x1: a.max(*c), y1: b.max(*d) },
        _ => Rect { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 },
    }
}

/// The certification signature's DocMDP permissions (1-3), if the document is certified.
fn docmdp_level(doc: &PdfDocument) -> Result<Option<u8>> {
    let catalog = doc.catalog().map_err(pdf_err)?;
    let Some(sig) = catalog.get_dict("Perms").map_err(pdf_err)?.and_then(|p| p.get_dict("DocMDP").ok().flatten()) else { return Ok(None) };
    Ok(Some(docmdp_of(&sig).unwrap_or(2)))
}

fn docmdp_of(sig: &PdfObject) -> Option<u8> {
    let refs = sig.get_dict("Reference").ok()??;
    for r in refs.array_iter().ok()? {
        let r = r.ok()?;
        if r.get_dict("TransformMethod").ok()??.as_name().ok()? == b"DocMDP" {
            let p = r.get_dict("TransformParams").ok().flatten().and_then(|t| t.get_dict("P").ok().flatten()).and_then(|p| p.as_int().ok()).unwrap_or(2);
            return Some(p.clamp(1, 3) as u8);
        }
    }
    None
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Intact, and the signer chains to a root Windows trusts.
    Valid,
    /// Intact, but the signer isn't trusted (self-signed, unknown issuer, expired).
    UnknownSigner,
    /// The signature doesn't match the bytes, or can't be read.
    Invalid,
    /// An empty signature field.
    Unsigned,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Changes {
    /// The signature covers the whole file.
    None,
    /// Only further signatures were added after it.
    SignaturesOnly,
    /// Something else was changed after it (form fill, comments, or other edits).
    Changed,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInfo {
    pub field: String,
    pub page: u32,
    /// Page space (points, y down).
    pub rect: [f32; 4],
    pub status: Status,
    /// One sentence for the status ("The signature is intact. The signer is not trusted: ...").
    pub summary: String,
    pub signer: String,
    pub issuer: String,
    /// `/M` as stored (`D:...`).
    pub time: String,
    pub reason: String,
    pub location: String,
    pub sub_filter: String,
    pub digest: String,
    /// DocMDP permissions when this is a certification signature.
    pub certify: Option<u8>,
    /// Byte range and digest check out.
    pub intact: bool,
    pub changes: Changes,
    pub trusted: bool,
    pub trust_note: String,
    pub cert_not_after: String,
    pub cert_fingerprint: String,
}

/// Lists every signature field with its validation, in page order. `trust` judges the signer's
/// certificate and the others the CMS carries (Windows chain engine; see `crypto::trust`).
/// Judges a signer's certificate given the others its signature carries: (trusted, why not).
pub type Trust = dyn Fn(&Certificate, &[Certificate]) -> (bool, String);

pub fn list(doc: &PdfDocument, bytes: &[u8], trust: &Trust) -> Result<Vec<SignatureInfo>> {
    let fields = sig_fields(doc)?;
    let mut infos: Vec<SignatureInfo> = fields.iter().map(|f| info(f, bytes, trust)).collect();
    // Where each intact signature's revision ends. A later revision counts as "only a
    // signature" when it ends at one of these AND adds nothing but what a signature adds.
    let intact_ends: Vec<usize> =
        fields.iter().zip(&infos).filter(|(_, i)| i.intact).filter_map(|(f, _)| f.value.as_ref().and_then(out_covered)).collect();
    let mut verdicts: std::collections::HashMap<(usize, usize), bool> = std::collections::HashMap::new();
    for (f, i) in fields.iter().zip(infos.iter_mut()) {
        let Some(covered) = f.value.as_ref().and_then(out_covered).filter(|_| i.intact) else { continue };
        if bytes[covered.min(bytes.len())..].iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let mut prev = covered;
        let mut only_sigs = true;
        for e in revision_ends(bytes, covered) {
            let ok = intact_ends.iter().any(|s| s.abs_diff(e) <= 2)
                && *verdicts.entry((prev, e)).or_insert_with(|| signature_only_revision(&bytes[..prev], &bytes[..e]).unwrap_or(false));
            if !ok {
                only_sigs = false;
                break;
            }
            prev = e;
        }
        // A certification that allows no changes is broken by any later revision, signatures too.
        i.changes = if only_sigs && i.certify != Some(1) { Changes::SignaturesOnly } else { Changes::Changed };
    }
    for i in &mut infos {
        summarize(i);
    }
    Ok(infos)
}

/// `after` is `before` with only references to `widgets` appended.
fn gains_only(before: &[String], after: &[String], widgets: &std::collections::HashSet<i32>) -> bool {
    after.len() >= before.len()
        && after[..before.len()] == *before
        && after[before.len()..].iter().all(|r| widgets.iter().any(|w| *r == format!("{w} 0 R")))
}

fn printed_elems(o: Option<PdfObject>) -> Option<Vec<String>> {
    let o = o?;
    if !o.is_array().ok()? {
        return None;
    }
    Some(o.array_iter().ok()?.filter_map(|x| x.ok()).map(|x| x.to_string()).collect())
}

/// Whether the revision from `prev` to `next` (both complete files) only adds signatures: every
/// object it adds or changes is a newly signed signature widget, its field and signature
/// dictionary, its appearance and the XObjects that names, the AcroForm's `/Fields` and
/// `/SigFlags`, and `/Annots` arrays gaining only those widgets. Anything else (a new
/// annotation, changed page content, another catalog entry) means the document was changed.
fn signature_only_revision(prev: &[u8], next: &[u8]) -> Result<bool> {
    use std::collections::HashSet;
    let a = PdfDocument::from_bytes(prev).map_err(pdf_err)?;
    let b = PdfDocument::from_bytes(next).map_err(pdf_err)?;
    let ind = |o: &PdfObject| if o.is_indirect().unwrap_or(false) { o.as_indirect().ok() } else { None };
    let old: HashSet<String> = sig_fields(&a)?.into_iter().filter(|f| f.value.is_some()).map(|f| f.name).collect();
    // New objects allowed, and existing ones allowed to change.
    let mut parts: HashSet<i32> = HashSet::new();
    let mut widgets: HashSet<i32> = HashSet::new();
    let mut pages: HashSet<i32> = HashSet::new();
    for f in sig_fields(&b)?.into_iter().filter(|f| f.value.is_some() && !old.contains(&f.name)) {
        widgets.extend(ind(&f.widget));
        widgets.extend(ind(&f.field));
        parts.extend(f.value.as_ref().and_then(ind));
        if let Ok(p) = b.find_page(f.page as i32) {
            pages.extend(ind(&p));
        }
        // The normal appearance and the XObjects (and soft masks) its resources name.
        if let Some(ap) = f.widget.get_dict("AP").ok().flatten().and_then(|d| d.get_dict("N").ok().flatten()) {
            parts.extend(ind(&ap));
            if let Some(xo) = ap.get_dict("Resources").ok().flatten().and_then(|r| r.get_dict("XObject").ok().flatten()) {
                for i in 0..xo.dict_len().unwrap_or(0) as i32 {
                    if let Some(x) = xo.get_dict_val(i).ok().flatten() {
                        parts.extend(ind(&x));
                        parts.extend(x.get_dict("SMask").ok().flatten().as_ref().and_then(ind));
                    }
                }
            }
        }
    }
    if widgets.is_empty() {
        return Ok(false);
    }
    let catalog_num = b.trailer().map_err(pdf_err)?.get_dict("Root").map_err(pdf_err)?.as_ref().and_then(ind);
    let acroform_num = b.catalog().map_err(pdf_err)?.get_dict("AcroForm").map_err(pdf_err)?.as_ref().and_then(ind);
    let get = |d: &PdfDocument, n: i32| d.xref_object(n).ok().flatten();
    let printed = |d: &PdfDocument, n: i32| -> Option<(String, Vec<u8>)> {
        let o = get(d, n)?;
        let stream = if o.is_stream().unwrap_or(false) { o.read_raw_stream().unwrap_or_default() } else { Vec::new() };
        Some((o.to_string(), stream))
    };
    // A dict printed without some keys, to compare the rest.
    let without = |d: &PdfDocument, n: i32, keys: &[&str]| -> Option<String> {
        let mut o = get(d, n)?.copy_dict().ok()?;
        for k in keys {
            o.dict_delete(*k).ok()?;
        }
        Some(o.to_string())
    };
    let (count_a, count_b) = (a.xref_len().map_err(pdf_err)? as i32, b.xref_len().map_err(pdf_err)? as i32);
    if count_b < count_a {
        return Ok(false);
    }
    for n in 1..count_b {
        let before = if n < count_a { printed(&a, n) } else { None };
        if before == printed(&b, n) {
            continue;
        }
        let ok = if before.is_none() {
            parts.contains(&n) || widgets.contains(&n)
        } else if widgets.contains(&n) {
            // An existing empty field being signed.
            true
        } else if let (Some(x), Some(y)) = (printed_elems(get(&a, n)), printed_elems(get(&b, n))) {
            // An indirect /Annots or /Fields array.
            gains_only(&x, &y, &widgets)
        } else if pages.contains(&n) {
            let annots = |d: &PdfDocument| get(d, n).and_then(|p| p.get_dict("Annots").ok().flatten());
            let inline_ok = match (annots(&a), annots(&b)) {
                (Some(x), Some(y)) if ind(&y).is_some() => ind(&x) == ind(&y),
                (x, Some(y)) => printed_elems(Some(y)).is_some_and(|after| gains_only(&printed_elems(x).unwrap_or_default(), &after, &widgets)),
                _ => false,
            };
            inline_ok && without(&a, n, &["Annots"]) == without(&b, n, &["Annots"])
        } else if Some(n) == acroform_num {
            without(&a, n, &["Fields", "SigFlags"]) == without(&b, n, &["Fields", "SigFlags"])
        } else if Some(n) == catalog_num {
            // Only an inline AcroForm may change in the catalog.
            acroform_num.is_none() && without(&a, n, &["AcroForm"]) == without(&b, n, &["AcroForm"])
        } else {
            false
        };
        if !ok {
            return Ok(false);
        }
    }
    Ok(true)
}

fn byte_range(v: &PdfObject) -> Result<[usize; 4]> {
    let br = v.get_dict("ByteRange").map_err(pdf_err)?.ok_or_else(|| SignError::Malformed("no /ByteRange".into()))?;
    let n: Vec<i64> = br.array_iter().map_err(pdf_err)?.filter_map(|x| x.ok()?.as_int().ok().map(i64::from)).collect();
    match n.as_slice() {
        [a, b, c, d] if *a >= 0 && *b >= 0 && *c >= 0 && *d >= 0 => Ok([*a as usize, *b as usize, *c as usize, *d as usize]),
        _ => Err(SignError::Malformed("the /ByteRange is not four non-negative numbers".into())),
    }
}

fn info(f: &SigField, bytes: &[u8], trust: &Trust) -> SignatureInfo {
    let get = |key: &str| -> String { f.value.as_ref().and_then(|v| v.get_dict(key).ok().flatten()).and_then(|s| s.as_string().ok()).unwrap_or_default() };
    let mut out = SignatureInfo {
        field: f.name.clone(),
        page: f.page,
        rect: f.rect,
        status: Status::Unsigned,
        summary: "Not signed yet.".into(),
        signer: String::new(),
        issuer: String::new(),
        time: get("M"),
        reason: get("Reason"),
        location: get("Location"),
        sub_filter: f
            .value
            .as_ref()
            .and_then(|v| v.get_dict("SubFilter").ok().flatten())
            .and_then(|s| s.as_name().ok())
            .map(|n| String::from_utf8_lossy(&n).into_owned())
            .unwrap_or_default(),
        digest: String::new(),
        certify: f.value.as_ref().and_then(docmdp_of),
        intact: false,
        changes: Changes::None,
        trusted: false,
        trust_note: String::new(),
        cert_not_after: String::new(),
        cert_fingerprint: String::new(),
    };
    let Some(v) = &f.value else { return out };
    out.signer = get("Name");
    match check(v, bytes, &mut out, trust) {
        Ok(()) => {}
        Err(e) => {
            out.status = Status::Invalid;
            out.intact = false;
            out.summary = format!("The signature is not valid: {e}.");
            return out;
        }
    }
    out
}

fn out_covered(v: &PdfObject) -> Option<usize> {
    byte_range(v).ok().map(|b| b[2] + b[3])
}

/// The ends (just past `%%EOF`) of the revisions after `from`.
fn revision_ends(bytes: &[u8], from: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut i = from;
    while let Some(p) = bytes[i.min(bytes.len())..].windows(5).position(|w| w == b"%%EOF") {
        let mut e = i + p + 5;
        while e < bytes.len() && (bytes[e] == b'\r' || bytes[e] == b'\n') {
            e += 1;
        }
        ends.push(e);
        i = e;
    }
    ends
}

/// The integrity, signer and trust checks; fills `out`. `Err`: the signature is invalid.
fn check(v: &PdfObject, bytes: &[u8], out: &mut SignatureInfo, trust: &Trust) -> Result<()> {
    let sf = out.sub_filter.as_str();
    if !matches!(sf, "adbe.pkcs7.detached" | "ETSI.CAdES.detached") {
        return Err(SignError::Unsupported(format!("signatures of type {sf} aren't checked yet")));
    }
    let [a, b, c, d] = byte_range(v)?;
    if a != 0 || b >= c || c.checked_add(d).is_none_or(|e| e > bytes.len()) {
        return Err(SignError::Malformed("the byte range doesn't fit the file".into()));
    }
    // The gap must be exactly the hex string that is /Contents, so nothing unsigned hides in it.
    if bytes[b] != b'<' || bytes[c - 1] != b'>' || !bytes[b + 1..c - 1].iter().all(u8::is_ascii_hexdigit) {
        return Err(SignError::Malformed("the unsigned gap is not just the signature value".into()));
    }
    let contents = v.get_dict("Contents").map_err(pdf_err)?.and_then(|s| s.as_bytes().ok()).ok_or_else(|| SignError::Malformed("no /Contents".into()))?;
    let gap = hex_decode(&bytes[b + 1..c - 1]);
    if gap.len() < contents.len() || gap[..contents.len()] != *contents {
        return Err(SignError::Malformed("the signature value in the file differs from /Contents".into()));
    }
    let sd = SignedData::parse(&contents)?;
    let cert = sd.signer_certificate().ok_or_else(|| SignError::Malformed("the signer's certificate is missing".into()))?.clone();
    let alg: DigestAlg = sd.signer.digest;
    out.digest = alg.name().to_string();
    out.signer = cert.display_name();
    out.issuer = cert.issuer.display_name();
    out.cert_not_after = format!("{:04}-{:02}-{:02}", cert.not_after.year, cert.not_after.month, cert.not_after.day);
    out.cert_fingerprint = cert.fingerprint();
    let digest = alg.digest(&[&bytes[..b], &bytes[c..c + d]]);
    // Detached: the signed attributes commit to the document through messageDigest.
    if sd.content.is_some() {
        return Err(SignError::Unsupported("signatures that embed their content aren't checked yet".into()));
    }
    if sd.signer.signed_attrs.is_some() && sd.signer.message_digest.as_deref() != Some(&digest[..]) {
        return Err(SignError::Crypto("the document was changed after it was signed, or the signature is damaged".into()));
    }
    if !sd.verify_signature(&cert, &digest)? {
        return Err(SignError::Crypto("the document was changed after it was signed, or the signature is damaged".into()));
    }
    out.intact = true;
    let others: Vec<Certificate> = sd.certificates.iter().filter(|x| x.raw != cert.raw).cloned().collect();
    let (trusted, note) = trust(&cert, &others);
    out.trusted = trusted;
    out.trust_note = note;
    out.status = if trusted { Status::Valid } else { Status::UnknownSigner };
    Ok(())
}

fn hex_decode(h: &[u8]) -> Vec<u8> {
    let v = |c: u8| match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    };
    h.chunks(2).map(|p| (v(p[0]) << 4) | p.get(1).map_or(0, |&x| v(x))).collect()
}

/// The summary sentence for a signature's state.
fn summarize(s: &mut SignatureInfo) {
    let changed = match s.changes {
        Changes::None => "",
        Changes::SignaturesOnly => " Only signatures were added after it.",
        Changes::Changed => " The document was changed after it was signed.",
    };
    match s.status {
        Status::Valid => s.summary = format!("The signature is valid, signed by {}.{changed}", s.signer),
        Status::UnknownSigner => s.summary = format!("The signature is intact, but the signer is not trusted: {}.{changed}", s.trust_note),
        _ => {}
    }
}
