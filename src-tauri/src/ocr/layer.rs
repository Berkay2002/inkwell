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

/// Resource name of the font in a page's `/Font` dictionary.
pub(crate) const FONT_RES: &str = "InkOcrF";
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

/// Adds the shared font. `max_code` is the largest UTF-16 code unit the layers use: the
/// CIDToGIDMap only needs to reach it, which keeps it a few hundred bytes for Latin text.
pub(crate) fn install_font(doc: &mut PdfDocument, max_code: u32) -> Result<PdfObject, OcrError> {
    let max_code = max_code.clamp(0x7f, 0xffff);
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

    const CMAP: &str = "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
        /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
        /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
        1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
        1 beginbfrange\n<0000> <FFFF> <0000>\nendbfrange\n\
        endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";
    let cmap_buf = Buffer::from_bytes(CMAP.as_bytes()).map_err(mupdf_err)?;
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
pub(crate) fn layer_stream(page: &PageResult, user_from_page: &Matrix) -> (Vec<u8>, u32) {
    let m = user_from_page;
    // Baseline direction on the displayed page and its "up" (y is down there).
    let (sin, cos) = page.skew_deg.unwrap_or(0.0).to_radians().sin_cos();
    let (ux, uy) = (m.a * cos + m.c * sin, m.b * cos + m.d * sin);
    let (vx, vy) = (m.a * sin - m.c * cos, m.b * sin - m.d * cos);
    let mut max_code = 0u32;
    let mut s = String::with_capacity(page.lines.len() * 200);
    s.push_str(&format!("/{TAG} BMC\nq\nBT\n3 Tr\n"));
    let mut last_fs = -1.0f32;
    for line in &page.lines {
        let top = line.iter().map(|w| w.bbox[1]).fold(f32::MAX, f32::min);
        let bottom = line.iter().map(|w| w.bbox[3]).fold(f32::MIN, f32::max);
        let fs = (bottom - top).max(1.0);
        for w in line {
            let units: Vec<u16> = w.text.chars().filter(|c| !c.is_control()).collect::<String>().encode_utf16().collect();
            if units.is_empty() {
                continue;
            }
            max_code = units.iter().fold(max_code, |a, &u| a.max(u as u32));
            let width = (w.bbox[2] - w.bbox[0]).max(0.5);
            // Every glyph advances 0.5 em, so Tz stretches the run to the box width.
            let tz = 200.0 * width / (fs * units.len() as f32);
            let (x, y) = page.to_page(w.bbox[0], bottom);
            let ex = m.a * x + m.c * y + m.e;
            let ey = m.b * x + m.d * y + m.f;
            if fs != last_fs {
                s.push_str(&format!("/{FONT_RES} "));
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
    (s.into_bytes(), max_code)
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
        page_obj.dict_put("Contents", keep).map_err(mupdf_err)?;
    }
    Ok(dropped)
}

/// Appends `stream` to the page and registers the font in the page's resources.
pub(crate) fn add_layer(doc: &mut PdfDocument, page: &mut PdfPage, font: &PdfObject, stream: &[u8]) -> Result<(), OcrError> {
    let mut page_obj = page.object();

    // Own resources dictionary: the original may be inherited or shared with other pages.
    let res = page_obj.get_dict_inheritable("Resources").map_err(mupdf_err)?;
    let mut res = match res {
        Some(r) if r.is_dict().map_err(mupdf_err)? => r.copy_dict().map_err(mupdf_err)?,
        _ => doc.new_dict().map_err(mupdf_err)?,
    };
    let mut fonts = match res.get_dict("Font").map_err(mupdf_err)? {
        Some(f) if f.is_dict().map_err(mupdf_err)? => f.copy_dict().map_err(mupdf_err)?,
        _ => doc.new_dict().map_err(mupdf_err)?,
    };
    fonts.dict_put(FONT_RES, font.clone()).map_err(mupdf_err)?;
    res.dict_put("Font", fonts).map_err(mupdf_err)?;
    page_obj.dict_put("Resources", res).map_err(mupdf_err)?;

    page.wrap_contents(doc).map_err(mupdf_err)?;
    let dict = doc.new_object_from_str("<< /Filter /FlateDecode >>").map_err(mupdf_err)?;
    let z = Buffer::from_bytes(&deflate(stream)).map_err(mupdf_err)?;
    let new_stream = doc.add_stream(&z, Some(&dict), true).map_err(mupdf_err)?;

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
    arr.array_push(new_stream).map_err(mupdf_err)?;
    page_obj.dict_put("Contents", arr).map_err(mupdf_err)
}
