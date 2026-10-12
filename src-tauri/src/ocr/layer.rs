//! The invisible text layer: a content stream (`3 Tr`) drawn after the page, plus one shared
//! glyphless Type0 font per document with an identity ToUnicode map.
//!
//! The page's own streams, image XObjects, MediaBox and CropBox are never rewritten: the layer is
//! one more stream appended to `/Contents` (the old contents are wrapped in `q`/`Q` so a CTM left
//! behind by the scanner's stream cannot scale the layer).

use std::io::Write;

use mupdf::pdf::{PdfDocument, PdfObject, PdfPage};
use mupdf::{Buffer, Matrix};

use super::glyphless::GLYPHLESS_TTF;
use super::{OcrError, PageResult};

/// Preferred resource name of the font in a page's `/Font` dictionary; [`font_name_for`] picks an
/// unused variant when the page already has something under it.
const FONT_RES: &str = "InkOcrF";
/// Marked-content tag that opens every layer stream; used to find and replace our own layers.
const TAG: &str = "InkOcr";
const BASE_FONT: &str = "InkOcrGlyphless";

fn mupdf_err(e: mupdf::Error) -> OcrError {
    OcrError::Pdf(e.to_string())
}

fn deflate(data: &[u8]) -> Vec<u8> {
    use flate2::{Compression, write::ZlibEncoder};
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(data).expect("write to Vec");
    z.finish().expect("finish Vec")
}

/// Characters outside the BMP (emoji, mathematical letters) cannot be one code of a 2-byte
/// Identity-H string, and a surrogate pair would extract as two lone surrogates. Each distinct one
/// gets a code in the Private Use Area instead and the ToUnicode map turns it back into the pair.
pub(crate) struct Codes {
    /// `(code, character)` in assignment order.
    astral: Vec<(u16, char)>,
    max_unit: u32,
}

impl Codes {
    pub(crate) fn build<'a>(texts: impl Iterator<Item = &'a str>) -> Self {
        let mut astral: Vec<char> = Vec::new();
        let mut used_pua = std::collections::BTreeSet::new();
        let mut max_unit = 0x7f;
        for t in texts {
            for c in t.chars() {
                if c as u32 > 0xFFFF {
                    if !astral.contains(&c) {
                        astral.push(c);
                    }
                } else {
                    max_unit = max_unit.max(c as u32);
                    if (0xE000..=0xF8FF).contains(&(c as u32)) {
                        used_pua.insert(c as u32);
                    }
                }
            }
        }
        // Codes already used by real private-use characters are skipped; the PUA block holds 6400.
        let mut free = (0xE000u32..=0xF8FF).filter(|c| !used_pua.contains(c));
        let astral: Vec<(u16, char)> = astral.into_iter().map_while(|c| free.next().map(|code| (code as u16, c))).collect();
        if let Some(&(code, _)) = astral.last() {
            max_unit = max_unit.max(code as u32);
        }
        Self { astral, max_unit }
    }

    /// UTF-16 code units for the layer: BMP characters as themselves, astral ones as their PUA code.
    /// An astral character beyond the 6400 codes is replaced by U+FFFD.
    pub(crate) fn encode(&self, text: &str) -> Vec<u16> {
        let mut out = Vec::with_capacity(text.len());
        for c in text.chars().filter(|c| !c.is_control()) {
            if (c as u32) <= 0xFFFF {
                out.push(c as u16);
            } else {
                out.push(self.astral.iter().find(|(_, a)| *a == c).map_or(0xFFFD, |(code, _)| *code));
            }
        }
        out
    }

    fn unit_codes(&self) -> impl Iterator<Item = u16> + '_ {
        self.astral.iter().map(|(code, _)| *code)
    }
}

/// ToUnicode CMap: identity everywhere except the assigned codes, which map to their surrogate pair.
fn to_unicode_cmap(codes: &Codes) -> String {
    let mut assigned: Vec<u16> = codes.unit_codes().collect();
    assigned.sort_unstable();
    // Identity ranges covering 0000-FFFF without the assigned codes.
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut start = 0u32;
    for a in assigned.iter().map(|&a| a as u32) {
        if a > start {
            ranges.push((start, a - 1));
        }
        start = a + 1;
    }
    if start <= 0xFFFF {
        ranges.push((start, 0xFFFF));
    }
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for chunk in ranges.chunks(100) {
        cmap.push_str(&format!("{} beginbfrange\n", chunk.len()));
        for (a, b) in chunk {
            cmap.push_str(&format!("<{a:04X}> <{b:04X}> <{a:04X}>\n"));
        }
        cmap.push_str("endbfrange\n");
    }
    for chunk in codes.astral.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (code, c) in chunk {
            let mut u = [0u16; 2];
            let u = c.encode_utf16(&mut u);
            cmap.push_str(&format!("<{code:04X}> <{:04X}{:04X}>\n", u[0], u[1]));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    cmap
}

/// Adds the shared font. The CIDToGIDMap only needs to reach the largest code the layers use,
/// which keeps it a few hundred bytes for Latin text.
pub(crate) fn install_font(doc: &mut PdfDocument, codes: &Codes) -> Result<PdfObject, OcrError> {
    let max_code = codes.max_unit.clamp(0x7f, 0xffff);
    let mut map = Vec::with_capacity((max_code as usize + 1) * 2);
    for _ in 0..=max_code {
        map.extend_from_slice(&[0, 1]); // every CID -> glyph 1 (the empty one)
    }
    let map = deflate(&map);

    let ttf_dict = doc.new_object_from_str(&format!("<< /Length1 {} >>", GLYPHLESS_TTF.len())).map_err(mupdf_err)?;
    let ttf_buf = Buffer::from_bytes(&GLYPHLESS_TTF).map_err(mupdf_err)?;
    let ttf = doc.add_stream(&ttf_buf, Some(&ttf_dict), false).map_err(mupdf_err)?;

    let map_dict = doc.new_object_from_str("<< /Filter /FlateDecode >>").map_err(mupdf_err)?;
    let map_buf = Buffer::from_bytes(&map).map_err(mupdf_err)?;
    let map = doc.add_stream(&map_buf, Some(&map_dict), true).map_err(mupdf_err)?;

    let cmap = to_unicode_cmap(codes);
    let cmap_buf = Buffer::from_bytes(cmap.as_bytes()).map_err(mupdf_err)?;
    let tounicode = doc.add_stream(&cmap_buf, None, false).map_err(mupdf_err)?;

    let r = |o: &PdfObject| o.as_indirect().map_err(mupdf_err);
    let desc = doc
        .new_object_from_str(&format!(
            "<< /Type /FontDescriptor /FontName /{BASE_FONT} /Flags 5 /FontBBox [0 0 500 1000] \
             /ItalicAngle 0 /Ascent 1000 /Descent -1 /CapHeight 1000 /StemV 80 /FontFile2 {} 0 R >>",
            r(&ttf)?
        ))
        .map_err(mupdf_err)?;
    let desc = doc.add_object(&desc).map_err(mupdf_err)?;
    let cid = doc
        .new_object_from_str(&format!(
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{BASE_FONT} \
             /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
             /FontDescriptor {} 0 R /CIDToGIDMap {} 0 R /DW 500 >>",
            r(&desc)?,
            r(&map)?
        ))
        .map_err(mupdf_err)?;
    let cid = doc.add_object(&cid).map_err(mupdf_err)?;
    let t0 = doc
        .new_object_from_str(&format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /{BASE_FONT} /Encoding /Identity-H \
             /DescendantFonts [{} 0 R] /ToUnicode {} 0 R >>",
            r(&cid)?,
            r(&tounicode)?
        ))
        .map_err(mupdf_err)?;
    doc.add_object(&t0).map_err(mupdf_err)
}

fn num(out: &mut String, v: f32) {
    use std::fmt::Write as _;
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    let _ = write!(out, "{}", if s.is_empty() || s == "-" || s == "-0" { "0" } else { s });
}

/// Builds the layer's content stream for one page. Word boxes are in the page's text frame (see
/// [`super::Word`]); `user_from_page` maps displayed-page points to default user space (the
/// inverse of the page transform), which also covers `/Rotate` and a CropBox offset. A skewed
/// scan gets its words laid along the skewed baseline.
/// Returns the stream and the largest UTF-16 code unit used.
pub(crate) fn layer_stream(page: &PageResult, user_from_page: &Matrix, codes: &Codes, font_res: &str) -> Vec<u8> {
    let m = user_from_page;
    // Baseline direction on the displayed page and its "up" (y is down there).
    let (sin, cos) = page.baseline_deg().to_radians().sin_cos();
    let (ux, uy) = (m.a * cos + m.c * sin, m.b * cos + m.d * sin);
    let (vx, vy) = (m.a * sin - m.c * cos, m.b * sin - m.d * cos);
    let mut s = String::with_capacity(page.lines.len() * 200);
    s.push_str(&format!("/{TAG} BMC\nq\nBT\n3 Tr\n"));
    let mut last_fs = -1.0f32;
    for line in &page.lines {
        let top = line.iter().map(|w| w.bbox[1]).fold(f32::MAX, f32::min);
        let bottom = line.iter().map(|w| w.bbox[3]).fold(f32::MIN, f32::max);
        let fs = (bottom - top).max(1.0);
        for w in line {
            let units = codes.encode(&visual_order(&w.text));
            if units.is_empty() {
                continue;
            }
            let width = (w.bbox[2] - w.bbox[0]).max(0.5);
            // Every glyph advances 0.5 em, so Tz stretches the run to the box width.
            let tz = 200.0 * width / (fs * units.len() as f32);
            let (x, y) = page.to_page(w.bbox[0], bottom);
            let ex = m.a * x + m.c * y + m.e;
            let ey = m.b * x + m.d * y + m.f;
            if fs != last_fs {
                s.push_str(&format!("/{font_res} "));
                num(&mut s, fs);
                s.push_str(" Tf\n");
                last_fs = fs;
            }
            for v in [ux, uy, vx, vy, ex, ey] {
                num(&mut s, v);
                s.push(' ');
            }
            s.push_str("Tm ");
            num(&mut s, tz);
            s.push_str(" Tz <");
            for u in &units {
                s.push_str(&format!("{u:04X}"));
            }
            s.push_str("0020> Tj\n"); // the real space after the word
        }
    }
    s.push_str("ET\nQ\nEMC\n");
    s.into_bytes()
}

/// A PDF string always advances left to right, and extractors read a right-to-left run (Arabic,
/// Hebrew) back from the right, as producers store it: in visual order. A word that is mostly RTL is
/// therefore written reversed, with the runs of digits and Latin letters inside it kept readable.
fn visual_order(text: &str) -> String {
    if !super::order::mostly_rtl(std::iter::once(text)) {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().rev().collect();
    let embedded = |c: char| c.is_ascii_alphanumeric();
    let mut i = 0;
    while i < chars.len() {
        if embedded(chars[i]) {
            let start = i;
            while i < chars.len() && embedded(chars[i]) {
                i += 1;
            }
            chars[start..i].reverse();
        } else {
            i += 1;
        }
    }
    chars.into_iter().collect()
}

/// Drops the content streams that earlier OCR runs of ours appended. Returns how many.
pub(crate) fn strip_layers(doc: &mut PdfDocument, page: &mut PdfPage) -> Result<usize, OcrError> {
    let mut page_obj = page.object();
    let Some(contents) = page_obj.get_dict("Contents").map_err(mupdf_err)? else { return Ok(0) };
    if !contents.is_array().map_err(mupdf_err)? {
        return Ok(0);
    }
    let n = contents.len().map_err(mupdf_err)?;
    let mut keep = doc.new_array().map_err(mupdf_err)?;
    let mut dropped = 0;
    let probe = format!("/{TAG} BMC");
    for i in 0..n {
        let Some(item) = contents.get_array(i as i32).map_err(mupdf_err)? else { continue };
        // Stream reads need the indirect reference itself (a resolved dict has lost its number).
        let ours = item.read_stream().ok().is_some_and(|b| b.starts_with(probe.as_bytes()));
        if ours {
            dropped += 1;
        } else {
            keep.array_push(item).map_err(mupdf_err)?;
        }
    }
    if dropped > 0 {
        // `add_layer` brackets the contents in a `q` stream and a `Q` stream each time; take the
        // pair that went with the layer out so reruns do not stack wrappers.
        let len = keep.len().map_err(mupdf_err)?;
        let bare = |i: usize, op: &[u8]| {
            keep.get_array(i as i32)
                .ok()
                .flatten()
                .and_then(|o| o.read_stream().ok())
                .is_some_and(|b| b.trim_ascii() == op)
        };
        if len >= 3 && bare(0, b"q") && bare(len as usize - 1, b"Q") {
            let mut inner = doc.new_array().map_err(mupdf_err)?;
            for i in 1..len - 1 {
                if let Some(item) = keep.get_array(i as i32).map_err(mupdf_err)? {
                    inner.array_push(item).map_err(mupdf_err)?;
                }
            }
            keep = inner;
        }
        page_obj.dict_put("Contents", keep).map_err(mupdf_err)?;
    }
    Ok(dropped)
}

/// Resource name for our font on this page: [`FONT_RES`] unless the page's own fonts already use
/// it, then `InkOcrF1`, `InkOcrF2`... A name that holds a font of ours from an earlier run is reused
/// (and overwritten) so reruns do not pile up entries.
pub(crate) fn font_name_for(page: &PdfPage) -> String {
    let fonts = page
        .object()
        .get_dict_inheritable("Resources")
        .ok()
        .flatten()
        .and_then(|r| r.get_dict("Font").ok().flatten());
    let Some(fonts) = fonts else { return FONT_RES.to_string() };
    for n in 0.. {
        let name = if n == 0 { FONT_RES.to_string() } else { format!("{FONT_RES}{n}") };
        match fonts.get_dict(name.as_str()).ok().flatten() {
            None => return name,
            Some(f) => {
                let ours = f.get_dict("BaseFont").ok().flatten().and_then(|b| b.as_name().ok()).is_some_and(|b| b == BASE_FONT.as_bytes());
                if ours {
                    return name;
                }
            }
        }
    }
    unreachable!()
}

/// Appends `stream` to the page and registers the font in the page's resources under `font_res`.
pub(crate) fn add_layer(doc: &mut PdfDocument, page: &mut PdfPage, font: &PdfObject, font_res: &str, stream: &[u8]) -> Result<(), OcrError> {
    add_streams(doc, page, &[(font_res.to_string(), font.clone())], &[stream])
}

/// Appends `streams` to the page, in order, and registers `fonts` (resource name, font object) in
/// the page's own copy of its resources.
pub(crate) fn add_streams(doc: &mut PdfDocument, page: &mut PdfPage, fonts: &[(String, PdfObject)], streams: &[&[u8]]) -> Result<(), OcrError> {
    let mut page_obj = page.object();

    // Own resources dictionary: the original may be inherited or shared with other pages.
    let res = page_obj.get_dict_inheritable("Resources").map_err(mupdf_err)?;
    let mut res = match res {
        Some(r) if r.is_dict().map_err(mupdf_err)? => r.copy_dict().map_err(mupdf_err)?,
        _ => doc.new_dict().map_err(mupdf_err)?,
    };
    let mut font_dict = match res.get_dict("Font").map_err(mupdf_err)? {
        Some(f) if f.is_dict().map_err(mupdf_err)? => f.copy_dict().map_err(mupdf_err)?,
        _ => doc.new_dict().map_err(mupdf_err)?,
    };
    for (name, font) in fonts {
        font_dict.dict_put(name.as_str(), font.clone()).map_err(mupdf_err)?;
    }
    res.dict_put("Font", font_dict).map_err(mupdf_err)?;
    page_obj.dict_put("Resources", res).map_err(mupdf_err)?;

    page.wrap_contents(doc).map_err(mupdf_err)?;
    let dict = doc.new_object_from_str("<< /Filter /FlateDecode >>").map_err(mupdf_err)?;

    let old = page_obj.get_dict("Contents").map_err(mupdf_err)?;
    let mut arr = doc.new_array().map_err(mupdf_err)?;
    match old {
        Some(o) if o.is_array().map_err(mupdf_err)? => {
            for i in 0..o.len().map_err(mupdf_err)? {
                if let Some(item) = o.get_array(i as i32).map_err(mupdf_err)? {
                    arr.array_push(item).map_err(mupdf_err)?;
                }
            }
        }
        Some(o) if !o.is_null().map_err(mupdf_err)? => arr.array_push(o).map_err(mupdf_err)?,
        _ => {}
    }
    for stream in streams {
        let z = Buffer::from_bytes(&deflate(stream)).map_err(mupdf_err)?;
        let new_stream = doc.add_stream(&z, Some(&dict), true).map_err(mupdf_err)?;
        arr.array_push(new_stream).map_err(mupdf_err)?;
    }
    page_obj.dict_put("Contents", arr).map_err(mupdf_err)
}
