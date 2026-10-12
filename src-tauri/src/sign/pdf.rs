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
    sign_with(base, id, req, &mut |_, _| {})
}

/// [`sign`], with `tamper` given the document and the signature's widget just before the update
/// is written: the tests forge signed revisions that carry more than a signature this way.
pub(super) fn sign_with(base: &[u8], id: &DigitalId, req: &SignRequest, tamper: &mut dyn FnMut(&mut PdfDocument, &mut PdfObject)) -> Result<Vec<u8>> {
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
    if req.certify.is_some() && (signed_before || crate::outline::signed(&doc)) {
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

    tamper(&mut doc, &mut widget);
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
    // Bounded before decoding: a small file can claim a huge canvas.
    let picture = |e: image::ImageError| SignError::Malformed(format!("signature picture: {e}"));
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| SignError::Malformed(format!("signature picture: {e}")))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(96 << 20);
    reader.limits(limits);
    let img = reader.decode().map_err(picture)?;
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
    /// Hidden or NoView: it draws nothing.
    hidden: bool,
    /// The share of the page's visible area (crop box) the widget's box covers, 0 to 1.
    cover: f32,
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
            let raw = rect_of(&a);
            let r = raw.transform(&ctm);
            let flags = a.get_dict("F").ok().flatten().and_then(|f| f.as_int().ok()).unwrap_or(0);
            let crop = page_box(&page);
            let cover = if area(&crop) > 0.0 { area(&intersect(&raw, &crop)) / area(&crop) } else { 0.0 };
            out.push(SigField {
                name,
                page: p as u32,
                widget: a,
                field,
                value,
                rect: [r.x0, r.y0, r.x1, r.y1],
                size: [r.width(), r.height()],
                // Hidden, or NoView without Print (NoView + Print still prints).
                hidden: flags & 2 != 0 || (flags & 32 != 0 && flags & 4 == 0),
                cover,
            });
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
    rect_in(a.get_dict("Rect").ok().flatten())
}

/// A rectangle array read as MuPDF draws it: its first four numbers (a longer array still draws).
fn rect_in(o: Option<PdfObject>) -> Rect {
    let v: Vec<f32> = o.and_then(|r| r.array_iter().ok().map(|it| it.filter_map(|x| x.ok()?.as_float().ok()).collect())).unwrap_or_default();
    match v.get(..4) {
        Some(&[a, b, c, d]) => Rect { x0: a.min(c), y0: b.min(d), x1: a.max(c), y1: b.max(d) },
        _ => Rect { x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0 },
    }
}

/// The page's visible area: its crop box, else its media box (unrotated page space).
fn page_box(page: &PdfObject) -> Rect {
    let get = |k: &str| page.get_dict_inheritable(k).ok().flatten();
    let media = rect_in(get("MediaBox"));
    match get("CropBox") {
        Some(c) => intersect(&rect_in(Some(c)), &media),
        None => media,
    }
}

fn intersect(a: &Rect, b: &Rect) -> Rect {
    Rect { x0: a.x0.max(b.x0), y0: a.y0.max(b.y0), x1: a.x1.min(b.x1), y1: a.y1.min(b.y1) }
}

fn area(r: &Rect) -> f32 {
    (r.x1 - r.x0).max(0.0) * (r.y1 - r.y0).max(0.0)
}

/// The object number of an indirect reference.
fn ind(o: &PdfObject) -> Option<i32> {
    if o.is_indirect().unwrap_or(false) { o.as_indirect().ok() } else { None }
}

fn name_of(o: &PdfObject, key: &str) -> Option<Vec<u8>> {
    o.get_dict(key).ok().flatten().and_then(|n| n.as_name().ok().map(|n| n.to_vec()))
}

/// Whether MuPDF had to rebuild the file's cross-reference table to read it. Then the objects it
/// sees need not be the ones another reader sees, so no revision of it is judged harmless.
fn repaired(d: &PdfDocument) -> bool {
    !d.can_be_saved_incrementally()
}

/// The certification signature's DocMDP permissions (1-3), if the document is certified.
/// The strictest certification anywhere in the document (the catalog's /Perms or a field's DocMDP
/// reference, see `outline::certified`); one that can't be read counts as allowing no changes.
fn docmdp_level(doc: &PdfDocument) -> Result<Option<u8>> {
    Ok(crate::outline::certified(doc).map_or(Some(1), |c| c.map(|p| p.clamp(1, 3) as u8)))
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
    /// Intact, but the signer isn't trusted (self-signed, unknown issuer, expired, not for signing).
    UnknownSigner,
    /// Intact and trusted, but with a caution in `warnings` (an untrusted signer stays
    /// `UnknownSigner`, its warnings listed all the same).
    Warning,
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

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WarningCode {
    /// The document was changed after signing.
    Changed,
    /// The signature hashes with SHA-1.
    Sha1,
    /// A later signature added a visible box, which can draw over what this one signed.
    LaterBox,
    /// ... and that box covers more than a quarter of its page.
    LaterBoxLarge,
    /// A later update names a second cross-reference table (/XRefStm), which readers may resolve
    /// differently.
    HybridXref,
    /// MuPDF had to repair the file to read it: other readers may show it differently.
    Damaged,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Warning {
    pub code: WarningCode,
    /// One sentence.
    pub text: String,
}

impl Warning {
    fn new(code: WarningCode, text: impl Into<String>) -> Self {
        Warning { code, text: text.into() }
    }
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
    /// Cautions about an intact signature (changes after it, a weak hash, a later visible box).
    pub warnings: Vec<Warning>,
    pub cert_not_after: String,
    pub cert_fingerprint: String,
}

/// Lists every signature field with its validation, in page order. `trust` judges the signer's
/// certificate and the others the CMS carries (Windows chain engine; see `crypto::trust`).
/// Judges a signer's certificate given the others its signature carries: (trusted, why not).
pub type Trust = dyn Fn(&Certificate, &[Certificate]) -> (bool, String);

/// At most this many signatures are checked in a file, and at most this many revisions after one.
const MAX_SIGNATURES: usize = 64;
const MAX_REVISIONS: usize = 64;

pub fn list(doc: &PdfDocument, bytes: &[u8], trust: &Trust) -> Result<Vec<SignatureInfo>> {
    let fields = sig_fields(doc)?;
    // Each signature hashes the file and parses the revision it signs: a bound on the work.
    if fields.iter().filter(|f| f.value.is_some()).count() > MAX_SIGNATURES {
        return Err(refuse(format!("this document has more than {MAX_SIGNATURES} signatures, more than Inkwell checks")));
    }
    let mut infos: Vec<SignatureInfo> = fields.iter().map(|f| info(f, bytes, trust)).collect();
    // An intact signature must belong to its field in the bytes it signs (see `bound`) before it
    // can vouch for anything below. `signed_then[k]`: the widgets already signed in those bytes.
    let mut signed_then: Vec<std::collections::HashSet<i32>> = vec![Default::default(); fields.len()];
    for (k, (f, i)) in fields.iter().zip(infos.iter_mut()).enumerate() {
        if !i.intact {
            continue;
        }
        match bound(f, doc, bytes) {
            Ok((certify, then)) => {
                i.certify = certify;
                signed_then[k] = then;
            }
            Err(why) => {
                i.status = Status::Invalid;
                i.intact = false;
                i.warnings.clear();
                i.summary = format!("The signature is not valid: {why}.");
            }
        }
    }
    let covered = |f: &SigField| f.value.as_ref().and_then(out_covered);
    // Where each intact signature's revision ends. A later revision counts as "only a
    // signature" when it ends at one of these AND adds nothing but what a signature adds.
    let intact_ends: Vec<usize> = fields.iter().zip(&infos).filter(|(_, i)| i.intact).filter_map(|(f, _)| covered(f)).collect();
    // ... and their signature dictionaries: every signature a later revision adds must be one of them.
    let intact_values: std::collections::HashSet<i32> = fields.iter().zip(&infos).filter(|(_, i)| i.intact).filter_map(|(f, _)| f.value.as_ref().and_then(ind)).collect();
    let mut verdicts: std::collections::HashMap<(usize, usize), bool> = std::collections::HashMap::new();
    for (f, i) in fields.iter().zip(infos.iter_mut()) {
        let Some(covered) = covered(f).filter(|_| i.intact) else { continue };
        if bytes[covered.min(bytes.len())..].iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        // A file MuPDF rebuilt may not read the same anywhere else: its updates aren't judged.
        let mut only_sigs = !repaired(doc);
        let ends = revision_ends(bytes, covered);
        if ends.len() > MAX_REVISIONS {
            only_sigs = false;
        }
        // A hybrid file's second cross-reference table (/XRefStm) is read by some readers and not
        // others, so an update that names one may show them different files.
        if hybrid_after(doc, &bytes[covered.min(bytes.len())..]) {
            only_sigs = false;
            i.warnings.push(Warning::new(WarningCode::HybridXref, "A later update adds a second cross-reference table, which other PDF readers may read differently."));
        }
        let mut prev = covered;
        for e in ends {
            if !only_sigs {
                break;
            }
            only_sigs = intact_ends.iter().any(|s| s.abs_diff(e) <= 2)
                && *verdicts.entry((prev, e)).or_insert_with(|| signature_only_revision(&bytes[..prev], &bytes[..e], &intact_values).unwrap_or(false));
            prev = e;
        }
        // Bytes after the last %%EOF are an update too (MuPDF loads one without %%EOF through its
        // startxref), and so is junk after a later signature: none of it was examined.
        if !bytes[prev.min(bytes.len())..].iter().all(u8::is_ascii_whitespace) {
            only_sigs = false;
        }
        i.changes = if only_sigs { Changes::SignaturesOnly } else { Changes::Changed };
    }
    // A signature box that wasn't signed yet in the bytes this one signs draws over its page (it
    // can hide or "correct" what was signed), so this one says so, whether or not that later
    // signature is valid.
    for k in 0..fields.len() {
        if !infos[k].intact {
            continue;
        }
        let mut found = Vec::new();
        for g in &fields {
            let later = ind(&g.widget).is_none_or(|w| !signed_then[k].contains(&w));
            if g.value.is_some() && later && !g.hidden && g.cover > 0.0 {
                let page = g.page + 1;
                found.push(Warning::new(WarningCode::LaterBox, format!("A later signature added a visible box on page {page}.")));
                if g.cover > 0.25 {
                    found.push(Warning::new(WarningCode::LaterBoxLarge, format!("A later signature's box covers more than a quarter of page {page}.")));
                }
            }
        }
        for w in found {
            if !infos[k].warnings.contains(&w) {
                infos[k].warnings.push(w);
            }
        }
    }
    if repaired(doc) {
        for i in infos.iter_mut().filter(|i| i.intact) {
            if i.changes != Changes::None {
                i.changes = Changes::Changed;
            }
            i.warnings.push(Warning::new(WarningCode::Damaged, "The file is damaged and was repaired to read it, so other PDF readers may show it differently."));
        }
    }
    for i in &mut infos {
        finish(i);
    }
    Ok(infos)
}

/// Ties an intact signature to the field showing it, through the bytes it signs: that revision
/// must already hold this widget and field (by object number and name) with this signature
/// dictionary as its value, and the dictionary must read the same there, nested objects
/// included. Otherwise a field added later could show a copy of someone else's signature, or
/// point at it. Returns the DocMDP level when that revision's catalog names this signature as
/// its certification, and the widgets already signed in that revision.
fn bound(f: &SigField, doc: &PdfDocument, bytes: &[u8]) -> std::result::Result<(Option<u8>, std::collections::HashSet<i32>), &'static str> {
    const UNBOUND: &str = "it is not part of the revision it signs";
    let v = f.value.as_ref().ok_or(UNBOUND)?;
    let (Some(vn), Some(wn), Some(fnum), Some(end)) = (ind(v), ind(&f.widget), ind(&f.field), out_covered(v)) else { return Err(UNBOUND) };
    const DAMAGED: &str = "the revision it signs is damaged";
    let then = PdfDocument::from_bytes(&bytes[..end.min(bytes.len())]).ok().filter(|d| !repaired(d)).ok_or(DAMAGED)?;
    let then = &then;
    let fields = sig_fields(then).map_err(|_| UNBOUND)?;
    let same = |g: &SigField| {
        ind(&g.widget) == Some(wn) && ind(&g.field) == Some(fnum) && g.name == f.name && g.value.as_ref().and_then(ind) == Some(vn)
    };
    if !fields.iter().any(same) {
        return Err(UNBOUND);
    }
    let (now, was) = (deep(doc, vn), deep(then, vn));
    if repaired(then) {
        return Err(DAMAGED);
    }
    if now.is_none() || was.is_none() {
        return Err("its signature dictionary is nested too deeply to check");
    }
    if now != was {
        return Err("its signature dictionary was changed after signing");
    }
    let perms = then.catalog().ok().and_then(|c| c.get_dict("Perms").ok().flatten()).and_then(|p| p.get_dict("DocMDP").ok().flatten());
    let certify = if perms.as_ref().and_then(ind) == Some(vn) { Some(docmdp_of(v).unwrap_or(2)) } else { None };
    Ok((certify, fields.iter().filter(|g| g.value.is_some()).filter_map(|g| ind(&g.widget)).collect()))
}

/// Object `n` printed with every reference followed (each object once, streams by their raw
/// bytes' hash), to compare it across revisions. `None` past 10 000 nodes or 64 levels (the app
/// checks on a small stack).
fn deep(d: &PdfDocument, n: i32) -> Option<String> {
    use std::fmt::Write;
    fn walk(o: &PdfObject, seen: &mut std::collections::HashSet<i32>, out: &mut String, budget: &mut u32, depth: u32) -> Option<()> {
        *budget = budget.checked_sub(1)?;
        if depth > 64 {
            return None;
        }
        if let Some(n) = ind(o) {
            let _ = write!(out, "{n}R");
            if seen.insert(n) {
                out.push('=');
                match o.resolve().ok()? {
                    Some(r) if !r.is_indirect().ok()? => walk(&r, seen, out, budget, depth + 1)?,
                    _ => out.push_str("null"),
                }
                // (Only the reference knows its stream: MuPDF finds it through the xref.)
                if o.is_stream().ok()? {
                    let raw = o.read_raw_stream().ok()?;
                    let _ = write!(out, "stream{}", hex(&DigestAlg::Sha256.digest(&[&raw])));
                }
            }
        } else if o.is_dict().ok()? {
            out.push_str("<<");
            for i in 0..o.dict_len().ok()? as i32 {
                out.push_str(&o.get_dict_key(i).ok()??.to_string());
                walk(&o.get_dict_val(i).ok()??, seen, out, budget, depth + 1)?;
            }
            out.push_str(">>");
        } else if o.is_array().ok()? {
            out.push('[');
            for x in o.array_iter().ok()? {
                walk(&x.ok()?, seen, out, budget, depth + 1)?;
            }
            out.push(']');
        } else {
            out.push_str(&o.to_string());
            out.push(' ');
        }
        Some(())
    }
    let mut out = String::new();
    walk(&d.new_indirect(n, 0).ok()?, &mut Default::default(), &mut out, &mut 10_000, 0)?;
    Some(out)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `after` is `before` with only references to `allowed` appended.
fn gains_only(before: &[String], after: &[String], allowed: &std::collections::HashSet<i32>) -> bool {
    after.len() >= before.len()
        && after[..before.len()] == *before
        && after[before.len()..].iter().all(|r| allowed.iter().any(|w| *r == format!("{w} 0 R")))
}

fn printed_elems(o: Option<PdfObject>) -> Option<Vec<String>> {
    let o = o?;
    if !o.is_array().ok()? {
        return None;
    }
    Some(o.array_iter().ok()?.filter_map(|x| x.ok()).map(|x| x.to_string()).collect())
}

/// A dictionary printed without some keys, to compare the rest.
fn without(o: &PdfObject, keys: &[&str]) -> Option<String> {
    let mut o = o.copy_dict().ok()?;
    for k in keys {
        o.dict_delete(*k).ok()?;
    }
    Some(o.to_string())
}

/// Every key of the dictionary `o` is one of `allowed`.
fn keys_within(o: &PdfObject, allowed: &[&str]) -> bool {
    let Ok(n) = o.dict_len() else { return false };
    (0..n as i32).all(|i| {
        o.get_dict_key(i).ok().flatten().and_then(|k| k.as_name().ok().map(|k| k.to_vec())).is_some_and(|k| allowed.iter().any(|a| a.as_bytes() == k))
    })
}

/// Whether the file, or any update in `after` (the bytes after a signature), names a second
/// cross-reference table: every trailer dictionary there is parsed, so an escaped name
/// (`/XRef#53tm`) counts too. A "trailer" inside a stream only errs towards "changed".
fn hybrid_after(doc: &PdfDocument, after: &[u8]) -> bool {
    let named = |t: &PdfObject| t.get_dict("XRefStm").ok().flatten().is_some();
    if doc.trailer().is_ok_and(|t| named(&t)) {
        return true;
    }
    let mut i = 0;
    while let Some(p) = after[i..].windows(7).position(|w| w == b"trailer") {
        let start = i + p + 7;
        let end = after[start..].windows(9).position(|w| w == b"startxref").map_or(after.len(), |q| start + q);
        let text = String::from_utf8_lossy(&after[start..end]);
        match doc.new_object_from_str(text.trim()) {
            Ok(t) if t.is_dict().unwrap_or(false) => {
                if named(&t) {
                    return true;
                }
            }
            // A trailer that doesn't parse: not judged harmless.
            _ => return true,
        }
        i = start;
    }
    false
}

/// One cross-reference section: its entries `(number, type, field 2, field 3)`, types as in an
/// xref stream (0 free, 1 at an offset, 2 in an object stream at an index), and its trailer's
/// /Prev.
pub(super) struct Section {
    pub entries: Vec<(i32, u8, u64, u64)>,
    pub prev: Option<u64>,
}

/// The number after the last `startxref` in `b`.
fn last_startxref(b: &[u8]) -> Option<u64> {
    let at = b.windows(9).rposition(|w| w == b"startxref")? + 9;
    let s = &b[at..];
    let skip = s.iter().take_while(|c| c.is_ascii_whitespace()).count();
    let len = s[skip..].iter().take_while(|c| c.is_ascii_digit()).count();
    std::str::from_utf8(&s[skip..skip + len]).ok()?.parse().ok()
}

/// The section at `off` of `bytes` (`d` is the document read from them, which parses the
/// dictionaries and decodes an xref stream). An xref stream must list itself at `off`, so the
/// object read here is the one MuPDF read there. `None` if it can't be read, or lists a number
/// twice (readers differ on which entry counts).
pub(super) fn xref_section(d: &PdfDocument, bytes: &[u8], off: u64) -> Option<Section> {
    let off = usize::try_from(off).ok().filter(|o| *o < bytes.len())?;
    let num = |s: &[u8]| -> Option<(u64, usize)> {
        let skip = s.iter().take_while(|c| c.is_ascii_whitespace()).count();
        let len = s[skip..].iter().take_while(|c| c.is_ascii_digit()).count();
        Some((std::str::from_utf8(&s[skip..skip + len]).ok()?.parse().ok()?, skip + len))
    };
    let prev_of = |t: &PdfObject| -> Option<Option<u64>> {
        match t.get_dict("Prev").ok()? {
            Some(p) => Some(Some(u64::try_from(p.as_int().ok()?).ok()?)),
            None => Some(None),
        }
    };
    let mut out = Vec::new();
    let prev;
    if bytes[off..].starts_with(b"xref") {
        let mut i = off + 4;
        loop {
            i += bytes.get(i..)?.iter().take_while(|c| c.is_ascii_whitespace()).count();
            if bytes[i..].starts_with(b"trailer") {
                let start = i + 7;
                let end = bytes[start..].windows(9).position(|w| w == b"startxref").map_or(bytes.len(), |q| start + q);
                let t = d.new_object_from_str(String::from_utf8_lossy(&bytes[start..end]).trim()).ok()?;
                if !t.is_dict().ok()? {
                    return None;
                }
                prev = prev_of(&t)?;
                break;
            }
            let (start, l1) = num(&bytes[i..])?;
            let (count, l2) = num(&bytes[i + l1..])?;
            i += l1 + l2;
            for k in 0..count {
                i += bytes.get(i..)?.iter().take_while(|c| c.is_ascii_whitespace()).count();
                let (o, l1) = num(&bytes[i..])?;
                let (g, l2) = num(&bytes[i + l1..])?;
                i += l1 + l2;
                i += bytes[i..].iter().take_while(|c| c.is_ascii_whitespace()).count();
                let kind = match bytes.get(i)? {
                    b'n' => 1,
                    b'f' => 0,
                    _ => return None,
                };
                i += 1;
                out.push((i32::try_from(start + k).ok()?, kind, o, g));
                if out.len() > 10_000_000 {
                    return None;
                }
            }
        }
    } else {
        // An xref stream: its own object, read through MuPDF (filters decoded).
        let (n, _) = num(&bytes[off..])?;
        let n = i32::try_from(n).ok()?;
        let xs = d.new_indirect(n, 0).ok()?;
        let w: Vec<usize> = xs.get_dict("W").ok()??.array_iter().ok()?.map(|x| x.ok()?.as_int().ok().map(|v| v as usize)).collect::<Option<_>>()?;
        let [w1, w2, w3] = w[..] else { return None };
        if w1 > 8 || w2 > 8 || w3 > 8 {
            return None;
        }
        let size = xs.get_dict("Size").ok()??.as_int().ok()? as i64;
        let index: Vec<i64> = match xs.get_dict("Index").ok()? {
            Some(ix) => ix.array_iter().ok()?.map(|x| x.ok()?.as_int().ok().map(i64::from)).collect::<Option<_>>()?,
            None => vec![0, size],
        };
        let data = xs.read_stream().ok()?;
        let field = |b: &[u8]| b.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64);
        let mut at = 0;
        for pair in index.chunks(2) {
            let [start, count] = pair[..] else { return None };
            for k in 0..count {
                let row = data.get(at..at + w1 + w2 + w3)?;
                at += w1 + w2 + w3;
                let kind = if w1 == 0 { 1 } else { field(&row[..w1]) };
                out.push((i32::try_from(start + k).ok()?, u8::try_from(kind).ok()?, field(&row[w1..w1 + w2]), field(&row[w1 + w2..])));
                if out.len() > 10_000_000 {
                    return None;
                }
            }
        }
        if !out.iter().any(|e| e.0 == n && e.1 == 1 && e.2 == off as u64) {
            return None;
        }
        prev = prev_of(&xs)?;
    }
    let mut seen = std::collections::HashSet::new();
    if !out.iter().all(|e| seen.insert(e.0)) {
        return None;
    }
    Some(Section { entries: out, prev })
}

/// The entries of the section the last `startxref` of `next` names, which must start at or after
/// `from`.
pub(super) fn update_entries(b: &PdfDocument, next: &[u8], from: usize) -> Option<Vec<(i32, u8, u64, u64)>> {
    let off = last_startxref(next).filter(|o| *o >= from as u64)?;
    Some(xref_section(b, next, off)?.entries)
}

/// The generation each object number has in the file `bytes` (`d` read from them), from the
/// newest section that lists it along the /Prev chain. `None` if the chain can't be read.
fn generations(d: &PdfDocument, bytes: &[u8]) -> Option<std::collections::HashMap<i32, u64>> {
    let mut gens = std::collections::HashMap::new();
    let mut seen = std::collections::HashSet::new();
    let mut off = last_startxref(bytes);
    while let Some(o) = off {
        if !seen.insert(o) || seen.len() > 4096 {
            return None;
        }
        let sec = xref_section(d, bytes, o)?;
        for &(n, kind, _, f3) in &sec.entries {
            gens.entry(n).or_insert(if kind == 2 { 0 } else { f3 });
        }
        off = sec.prev;
    }
    Some(gens)
}

/// The `n g obj` header at `offset` of `b`.
fn obj_header(b: &[u8], offset: u64) -> Option<(i64, i64)> {
    let s = b.get(usize::try_from(offset).ok()?..)?;
    let head = String::from_utf8_lossy(&s[..s.len().min(48)]);
    let mut t = head.split_ascii_whitespace();
    let (n, g) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    t.next()?.starts_with("obj").then_some((n, g))
}

/// Whether object `n`, listed in object stream `stm` at `index`, is really the object at that
/// index, and the stream lists no number twice. MuPDF finds an object in a stream by its number
/// (the first copy) while other readers take the index: a stream with two copies of one object
/// shows them different objects.
fn in_objstm(b: &PdfDocument, n: i32, stm: u64, index: u64) -> Option<bool> {
    let s = b.new_indirect(i32::try_from(stm).ok()?, 0).ok()?;
    let count = s.get_dict("N").ok()??.as_int().ok()?;
    let first = usize::try_from(s.get_dict("First").ok()??.as_int().ok()?).ok()?;
    let data = s.read_stream().ok()?;
    let head = std::str::from_utf8(data.get(..first)?).ok()?;
    let nums: Vec<i64> = head.split_ascii_whitespace().map(|t| t.parse().ok()).collect::<Option<_>>()?;
    if nums.len() != 2 * usize::try_from(count).ok()? {
        return Some(false);
    }
    let objs: Vec<i64> = nums.iter().step_by(2).copied().collect();
    let unique: std::collections::HashSet<i64> = objs.iter().copied().collect();
    Some(unique.len() == objs.len() && objs.get(usize::try_from(index).ok()?) == Some(&i64::from(n)))
}

/// Adds the object numbers the printed object `s` refers to (`n g R`, whatever the generation:
/// MuPDF ignores it). Digits inside a string may count too, which only errs towards "changed".
fn refs(s: &str, out: &mut std::collections::HashSet<i32>) {
    let spaced = s.replace('/', " /");
    let toks: Vec<&str> = spaced.split(|c: char| c.is_ascii_whitespace() || "[]<>(){}".contains(c)).filter(|t| !t.is_empty()).collect();
    for w in toks.windows(3) {
        if w[2] == "R" && !w[1].is_empty() && w[1].bytes().all(|c| c.is_ascii_digit()) && w[0].bytes().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = w[0].parse::<i32>() {
                out.insert(n);
            }
        }
    }
}

/// What a signature's widget may carry. Not /A or /AA (actions), /OC, /Kids, /Type /Page ...
const WIDGET_KEYS: &[&str] = &["Type", "Subtype", "FT", "T", "TU", "V", "Rect", "AP", "F", "P", "Parent", "MK", "DA", "Q", "Ff"];
/// What a separate signature field (the widget's parent) may carry.
const FIELD_KEYS: &[&str] = &["FT", "T", "TU", "V", "Kids", "Ff"];

/// Whether the revision from `prev` to `next` (both complete files) only adds signatures. It may
/// add: each new signature's dictionary, appearance and the images that names; its widget and
/// field (from a fixed list of keys, no actions); the page's /Annots and the form's /Fields
/// gaining exactly those; /SigFlags; and object or cross-reference streams nothing refers to. It
/// may sign an empty signature field in place (its value, appearance and flags only). Anything
/// else (another catalog, a new annotation, changed page content, form settings) means the
/// document was changed.
pub(super) fn signature_only_revision(prev: &[u8], next: &[u8], intact: &std::collections::HashSet<i32>) -> Result<bool> {
    use std::collections::{HashMap, HashSet};
    let a = PdfDocument::from_bytes(prev).map_err(pdf_err)?;
    let b = PdfDocument::from_bytes(next).map_err(pdf_err)?;
    if repaired(&a) || repaired(&b) {
        return Ok(false);
    }
    // The same catalog, and no new encryption.
    let (ta, tb) = (a.trailer().map_err(pdf_err)?, b.trailer().map_err(pdf_err)?);
    let root = |t: &PdfObject| t.get_dict("Root").ok().flatten().as_ref().and_then(ind);
    let encrypt = |t: &PdfObject| t.get_dict("Encrypt").ok().flatten().map(|e| e.to_string());
    let Some(catalog_num) = root(&ta).filter(|r| root(&tb) == Some(*r)) else { return Ok(false) };
    if encrypt(&ta) != encrypt(&tb) {
        return Ok(false);
    }
    // The rest of the trailer too (/Info, ...): only the links between sections change.
    const SECTION: &[&str] = &["Prev", "Size", "ID", "XRefStm"];
    if without(&ta, SECTION).is_none() || without(&ta, SECTION) != without(&tb, SECTION) {
        return Ok(false);
    }
    let get = |d: &PdfDocument, n: i32| d.xref_object(n).ok().flatten();
    let is_sig = |o: &PdfObject| name_of(o, "FT").as_deref() == Some(b"Sig");

    let before_fields = sig_fields(&a)?;
    let old: HashSet<String> = before_fields.iter().filter(|f| f.value.is_some()).map(|f| f.name.clone()).collect();
    // The empty signature fields that may be signed in place, and what each object may change: a
    // widget that is its own field (value, appearance, flags), or a widget (appearance, flags) and
    // its parent field (value), which must be a field dictionary, not an annotation or a page.
    let mut empty: HashMap<i32, &[&str]> = HashMap::new();
    for f in before_fields.iter().filter(|f| f.value.is_none()) {
        let (Some(w), Some(p)) = (ind(&f.widget), ind(&f.field)) else { continue };
        if w == p {
            empty.insert(w, &["V", "AP", "F"]);
        } else if f.field.get_dict("Subtype").ok().flatten().is_none() && is_sig(&f.field) {
            empty.insert(w, &["AP", "F"]);
            empty.insert(p, &["V"]);
        }
    }

    // What the new signatures bring.
    let mut parts: HashSet<i32> = HashSet::new();
    let mut widgets: HashSet<i32> = HashSet::new();
    let mut fields: HashSet<i32> = HashSet::new();
    let mut top: HashSet<i32> = HashSet::new();
    let mut pages: HashSet<i32> = HashSet::new();
    for f in sig_fields(&b)?.into_iter().filter(|f| f.value.is_some() && !old.contains(&f.name)) {
        let v = f.value.as_ref().expect("filtered");
        let (Some(w), Some(p), Some(vn)) = (ind(&f.widget), ind(&f.field), ind(v)) else { return Ok(false) };
        // Only signatures that verify: an invalid one vouches for nothing it adds.
        if !intact.contains(&vn) {
            return Ok(false);
        }
        // Signed in this revision: it covers exactly up to the revision's end.
        let end = out_covered(v).unwrap_or(usize::MAX);
        if end > next.len() || !next[end..].iter().all(u8::is_ascii_whitespace) {
            return Ok(false);
        }
        widgets.insert(w);
        fields.insert(p);
        parts.insert(vn);
        if f.field.get_dict("Parent").ok().flatten().is_none() {
            top.insert(p);
        }
        let Some(page) = b.find_page(f.page as i32).ok().as_ref().and_then(ind) else { return Ok(false) };
        if f.widget.get_dict("P").ok().flatten().is_some_and(|x| ind(&x) != Some(page)) {
            return Ok(false);
        }
        pages.insert(page);
        // The normal appearance (the only one) and the images (and soft masks) its resources name.
        if let Some(ap) = f.widget.get_dict("AP").ok().flatten() {
            if ind(&ap).is_some() || !keys_within(&ap, &["N"]) {
                return Ok(false);
            }
            if let Some(n) = ap.get_dict("N").ok().flatten() {
                parts.extend(ind(&n));
                if let Some(xo) = n.get_dict("Resources").ok().flatten().and_then(|r| r.get_dict("XObject").ok().flatten()) {
                    for i in 0..xo.dict_len().unwrap_or(0) as i32 {
                        if let Some(x) = xo.get_dict_val(i).ok().flatten() {
                            parts.extend(ind(&x));
                            parts.extend(x.get_dict("SMask").ok().flatten().as_ref().and_then(ind));
                        }
                    }
                }
            }
        }
    }
    if widgets.is_empty() {
        return Ok(false);
    }
    let widget_ok = |o: &PdfObject| {
        let mk_ok = match o.get_dict("MK").ok().flatten() {
            Some(mk) => ind(&mk).is_none() && ["I", "RI", "IX"].iter().all(|k| mk.get_dict(*k).ok().flatten().is_none()),
            None => true,
        };
        keys_within(o, WIDGET_KEYS) && name_of(o, "Subtype").as_deref() == Some(b"Widget") && mk_ok
    };
    let field_ok = |o: &PdfObject| {
        let kids_ok = o.get_dict("Kids").ok().flatten().is_none_or(|k| printed_elems(Some(k)).is_some_and(|k| gains_only(&[], &k, &widgets)));
        keys_within(o, FIELD_KEYS) && o.get_dict("Subtype").ok().flatten().is_none() && is_sig(o) && kids_ok
    };
    // Whether object `n` has a stream. Asked of its reference: a resolved object doesn't know.
    let is_stream = |d: &PdfDocument, n: i32| d.new_indirect(n, 0).is_ok_and(|r| r.is_stream().unwrap_or(true));
    let plain_dict = |d: &PdfDocument, n: i32| get(d, n).is_some_and(|o| o.is_dict().unwrap_or(false)) && !is_stream(d, n);

    // The arrays that may grow: the signed pages' /Annots (each used by that page alone) and
    // the form's /Fields.
    let annots_of = |d: &PdfDocument, page: i32| get(d, page).and_then(|p| p.get_dict("Annots").ok().flatten()).as_ref().and_then(ind);
    let annot_arrays: HashSet<i32> = pages.iter().filter_map(|&p| annots_of(&a, p)).collect();
    let mut users: HashMap<i32, usize> = HashMap::new();
    for i in 0..b.page_count().map_err(pdf_err)? {
        if let Some(n) = b.find_page(i).ok().as_ref().and_then(ind).and_then(|p| annots_of(&b, p)) {
            *users.entry(n).or_default() += 1;
        }
    }
    if annot_arrays.iter().any(|n| users.get(n).copied().unwrap_or(0) > 1) {
        return Ok(false);
    }
    let acroform = |d: &PdfDocument| get(d, catalog_num).and_then(|c| c.get_dict("AcroForm").ok().flatten());
    let (form_a, form_b) = (acroform(&a), acroform(&b));
    let form_num = form_a.as_ref().and_then(ind).filter(|n| form_b.as_ref().and_then(ind) == Some(*n));
    let fields_array = form_a.as_ref().and_then(|f| f.get_dict("Fields").ok().flatten()).as_ref().and_then(ind);
    // The form, before and after: only /Fields (gaining the new top-level fields) and /SigFlags.
    let form_ok = |x: Option<&PdfObject>, y: &PdfObject| {
        let fx = x.and_then(|x| x.get_dict("Fields").ok().flatten());
        let fields_ok = match (fx, y.get_dict("Fields").ok().flatten()) {
            (Some(fx), Some(fy)) if ind(&fy).is_some() => ind(&fx) == ind(&fy),
            (fx, Some(fy)) => printed_elems(Some(fy)).is_some_and(|after| gains_only(&printed_elems(fx).unwrap_or_default(), &after, &top)),
            (None, None) => true,
            _ => false,
        };
        let rest_ok = match x {
            Some(x) => without(x, &["Fields", "SigFlags"]) == without(y, &["Fields", "SigFlags"]),
            None => keys_within(y, &["Fields", "SigFlags"]),
        };
        fields_ok && rest_ok
    };

    let printed = |d: &PdfDocument, n: i32| -> Option<(String, Vec<u8>)> {
        let o = get(d, n)?;
        let stream = if is_stream(d, n) { d.new_indirect(n, 0).ok()?.read_raw_stream().ok()? } else { Vec::new() };
        Some((o.to_string(), stream))
    };
    // The update is one cross-reference section, in its own bytes, whose /Prev is the signed
    // revision's: MuPDF follows the whole /Prev chain, so a second section (or a /Prev into the
    // signed bytes) could list objects no check here would see.
    let (Some(start_a), Some(start_b)) = (last_startxref(prev), last_startxref(next)) else { return Ok(false) };
    if start_b < prev.len() as u64 {
        return Ok(false);
    }
    let Some(section) = xref_section(&b, next, start_b) else { return Ok(false) };
    if section.prev != Some(start_a) {
        return Ok(false);
    }
    // What it lists. An object at an offset must be the object written there, number and
    // generation (MuPDF repairs a wrong number, caught above). An object placed in an object
    // stream must be the one at that index, since MuPDF takes the first copy of a number there and
    // other readers the index. And a number the signed revision used keeps its generation:
    // MuPDF ignores generations, but other readers resolve the old `n g R` to null.
    let Some(gens) = generations(&a, prev) else { return Ok(false) };
    let count_a = a.xref_len().map_err(pdf_err)? as i32;
    for &(n, kind, f2, f3) in &section.entries {
        let ok = match kind {
            0 => true,
            1 => n == 0 || obj_header(next, f2) == Some((i64::from(n), f3 as i64)),
            2 => in_objstm(&b, n, f2, f3) == Some(true),
            _ => false,
        };
        if !ok {
            return Ok(false);
        }
        let used = n > 0 && n < count_a && get(&a, n).is_some_and(|x| x.to_string() != "null");
        if used && kind != 0 && gens.get(&n).copied().unwrap_or(0) != if kind == 2 { 0 } else { f3 } {
            return Ok(false);
        }
    }
    let (count_a, count_b) = (a.xref_len().map_err(pdf_err)? as i32, b.xref_len().map_err(pdf_err)? as i32);
    // Every object takes some bytes: a table far larger than the file is only there to cost time.
    if count_b < count_a || count_b as usize > next.len() / 4 + 1024 {
        return Ok(false);
    }
    let mut structural = Vec::new();
    // Objects new in this revision, and the numbers the signed revision already named (a dangling
    // reference planted before signing, or one to a null, would let a "new" object change what
    // the signed pages show) and the ones this revision names (nothing may use a structural one).
    let mut fresh = Vec::new();
    let (mut named_a, mut named_b) = (HashSet::new(), HashSet::new());
    refs(&ta.to_string(), &mut named_a);
    refs(&tb.to_string(), &mut named_b);
    for n in 1..count_b {
        let was = if n < count_a { printed(&a, n) } else { None };
        if let Some((s, _)) = &was {
            refs(s, &mut named_a);
        }
        // A free entry reads as null: the number was unused.
        let before = was.filter(|(s, _)| s != "null");
        // An entry that doesn't load is never taken as harmless.
        let Some(after) = printed(&b, n) else { return Ok(false) };
        refs(&after.0, &mut named_b);
        let after = Some(after);
        if before == after {
            continue;
        }
        if before.is_none() {
            fresh.push(n);
        }
        let Some(o) = get(&b, n) else { return Ok(false) };
        let ok = if before.is_none() {
            if parts.contains(&n) {
                true
            } else if widgets.contains(&n) {
                plain_dict(&b, n) && widget_ok(&o)
            } else if fields.contains(&n) {
                plain_dict(&b, n) && field_ok(&o)
            } else if is_stream(&b, n)
                && matches!(name_of(&o, "Type").as_deref(), Some(b"ObjStm" | b"XRef"))
                && o.get_dict("Subtype").ok().flatten().is_none()
            {
                // Structure only, as long as nothing uses it (checked below).
                structural.push(n);
                true
            } else {
                false
            }
        } else if widgets.contains(&n) || fields.contains(&n) {
            // An existing empty field being signed: only its value, appearance and flags change.
            match (empty.get(&n), get(&a, n)) {
                (Some(keys), Some(was)) => plain_dict(&a, n) && plain_dict(&b, n) && without(&was, keys) == without(&o, keys),
                _ => false,
            }
        } else if annot_arrays.contains(&n) {
            match (printed_elems(get(&a, n)), printed_elems(Some(o.clone()))) {
                (Some(x), Some(y)) => gains_only(&x, &y, &widgets),
                _ => false,
            }
        } else if Some(n) == fields_array {
            match (printed_elems(get(&a, n)), printed_elems(Some(o.clone()))) {
                (Some(x), Some(y)) => gains_only(&x, &y, &top),
                _ => false,
            }
        } else if pages.contains(&n) {
            let annots = |d: &PdfDocument| get(d, n).and_then(|p| p.get_dict("Annots").ok().flatten());
            let annots_ok = match (annots(&a), annots(&b)) {
                (Some(x), Some(y)) if ind(&y).is_some() => ind(&x) == ind(&y),
                (x, Some(y)) => printed_elems(Some(y)).is_some_and(|after| gains_only(&printed_elems(x).unwrap_or_default(), &after, &widgets)),
                _ => false,
            };
            annots_ok && get(&a, n).is_some_and(|was| without(&was, &["Annots"]) == without(&o, &["Annots"]))
        } else if Some(n) == form_num {
            get(&a, n).is_some_and(|was| form_ok(Some(&was), &o))
        } else if n == catalog_num {
            // Only an inline form may change in the catalog; it may not become (or stop being) a
            // separate object.
            let rest = get(&a, n).is_some_and(|was| without(&was, &["AcroForm"]) == without(&o, &["AcroForm"]));
            let form = match (&form_a, &form_b) {
                (Some(x), Some(y)) if ind(x).is_some() || ind(y).is_some() => form_num.is_some(),
                (Some(x), Some(y)) => form_ok(Some(x), y),
                (None, Some(y)) => ind(y).is_none() && form_ok(None, y),
                _ => false,
            };
            rest && form
        } else {
            false
        };
        if !ok {
            return Ok(false);
        }
    }
    // An object or cross-reference stream is only structure: nothing may draw it or name it.
    if structural.iter().any(|m| named_b.contains(m)) || fresh.iter().any(|m| named_a.contains(m)) {
        return Ok(false);
    }
    // Reading every object may have made MuPDF rebuild the table (an entry pointing at the wrong
    // object): then what was compared is not what is shown.
    if repaired(&a) || repaired(&b) || b.trailer().ok().and_then(|t| t.get_dict("XRefStm").ok().flatten()).is_some() {
        return Ok(false);
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
        warnings: Vec::new(),
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
    let si = &sd.signer;
    if si.signed_attrs.is_none() && sf == "ETSI.CAdES.detached" {
        return Err(SignError::Malformed("a CAdES signature without signed attributes".into()));
    }
    if si.signed_attrs.is_some() {
        if si.content_type.as_deref() != Some(cms::DATA) {
            return Err(SignError::Malformed("the signed content type is not data".into()));
        }
        if si.message_digest.as_deref() != Some(&digest[..]) {
            return Err(SignError::Crypto("the document was changed after it was signed, or the signature is damaged".into()));
        }
        // ESS signing-certificate(-v2) binds the signature to one certificate: it must be this one.
        match &si.signing_cert_hash {
            Some((h, value)) if h.digest(&[&cert.raw]) != *value => {
                return Err(SignError::Malformed("the signature names a different certificate than the one it carries".into()));
            }
            None if sf == "ETSI.CAdES.detached" => return Err(SignError::Malformed("a CAdES signature that doesn't name its certificate".into())),
            _ => {}
        }
    }
    if !sd.verify_signature(&cert, &digest)? {
        return Err(SignError::Crypto("the document was changed after it was signed, or the signature is damaged".into()));
    }
    out.intact = true;
    if [si.digest, si.scheme_digest.unwrap_or(si.digest)].contains(&DigestAlg::Sha1) {
        out.warnings.push(Warning::new(WarningCode::Sha1, "It uses SHA-1, a hash algorithm that is no longer safe."));
    }
    let others: Vec<Certificate> = sd.certificates.iter().filter(|x| x.raw != cert.raw).cloned().collect();
    let (trusted, note) = match cert.signing_problem() {
        // A certificate not meant for signing documents (a TLS server's, say) is never "valid".
        Some(why) => (false, why.trim_end_matches('.').to_string()),
        None => trust(&cert, &others),
    };
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

/// The final status of an intact signature once its later changes are known, and its summary.
/// A certification is broken by any later revision at P=1. At P=2 or 3 form filling (and at 3
/// comments) are allowed, which Inkwell doesn't tell apart from other edits: a change after it is
/// reported as a change, and the certification is not called broken.
fn finish(s: &mut SignatureInfo) {
    if !matches!(s.status, Status::Valid | Status::UnknownSigner) {
        return;
    }
    let broken = s.certify == Some(1) && s.changes != Changes::None;
    if broken {
        s.status = Status::Invalid;
        s.summary = "The signature is not valid: the document was changed after it was certified, which the certification does not allow.".into();
        return;
    }
    if s.changes == Changes::Changed {
        let text = match s.certify {
            Some(2) => "The document was changed after it was certified. The certification allows filling in forms; Inkwell can't tell whether that is all that changed.",
            Some(3) => "The document was changed after it was certified. The certification allows filling in forms and adding comments; Inkwell can't tell whether that is all that changed.",
            _ => "The document was changed after it was signed.",
        };
        s.warnings.insert(0, Warning::new(WarningCode::Changed, text));
    }
    let trust = if s.trusted { format!("signed by {}", s.signer) } else { format!("but the signer is not trusted: {}", s.trust_note) };
    let only_sigs = if s.changes == Changes::SignaturesOnly { " Only signatures were added after it." } else { "" };
    // Trust and warnings are separate: only a trusted signer is ever "valid", with or without a
    // caution, and an untrusted one stays unknown whatever else is true.
    let cautions: Vec<&str> = s.warnings.iter().map(|w| w.text.as_str()).collect();
    s.status = match (s.trusted, cautions.is_empty()) {
        (false, _) => Status::UnknownSigner,
        (true, true) => Status::Valid,
        (true, false) => Status::Warning,
    };
    s.summary = if s.status == Status::Valid {
        format!("The signature is valid, {trust}.{only_sigs}")
    } else if cautions.is_empty() {
        format!("The signature is intact, {trust}.{only_sigs}")
    } else {
        format!("The signature is intact, {trust}. {}", cautions.join(" "))
    };
}

/// Every signature dictionary in `bytes`, valid or not (tests judge one revision by hand).
#[cfg(test)]
pub(super) fn values(bytes: &[u8]) -> std::collections::HashSet<i32> {
    let d = PdfDocument::from_bytes(bytes).unwrap();
    sig_fields(&d).unwrap().iter().filter_map(|f| f.value.as_ref().and_then(ind)).collect()
}
