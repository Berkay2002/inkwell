//! The fallback faces compiled into Inkwell (Noto Sans, Noto Sans Mono, Noto Sans CJK; see
//! third_party/fonts/README.md), and embedding them into a PDF: only the glyphs used, as a
//! Type0/Identity-H font with a ToUnicode map, made at runtime with the `subsetter` crate.
//!
//! An embedded subset's codes are its new glyph ids (two bytes each), so the text is written after
//! the subset is made: layout keeps the original glyph ids and asks [`Subset::code`] at the end.

use crate::engine::EngineError;
use mupdf::Font;
use mupdf::pdf::{PdfDocument, PdfObject};
use std::collections::BTreeMap;
use std::fmt::Write as _;

type Result<T> = std::result::Result<T, EngineError>;

/// A bundled face.
#[derive(Debug)]
pub struct Face {
    /// Stable id for the font picker (`bundled:<id>`).
    pub id: &'static str,
    pub family: &'static str,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    /// Han, kana and Hangul.
    pub cjk: bool,
    /// PostScript name (the embedded /BaseFont after the subset tag).
    pub ps_name: &'static str,
    data: &'static [u8],
    /// CFF outlines (embedded as /FontFile3 /CIDFontType0C) rather than TrueType.
    cff: bool,
}

macro_rules! font {
    ($file:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fonts/", $file))
    };
}

pub static FACES: [Face; 7] = [
    Face {
        id: "NotoSans-Regular",
        family: "Noto Sans",
        bold: false,
        italic: false,
        mono: false,
        cjk: false,
        ps_name: "NotoSans-Regular",
        data: font!("NotoSans-Regular.ttf"),
        cff: false,
    },
    Face {
        id: "NotoSans-Bold",
        family: "Noto Sans",
        bold: true,
        italic: false,
        mono: false,
        cjk: false,
        ps_name: "NotoSans-Bold",
        data: font!("NotoSans-Bold.ttf"),
        cff: false,
    },
    Face {
        id: "NotoSans-Italic",
        family: "Noto Sans",
        bold: false,
        italic: true,
        mono: false,
        cjk: false,
        ps_name: "NotoSans-Italic",
        data: font!("NotoSans-Italic.ttf"),
        cff: false,
    },
    Face {
        id: "NotoSans-BoldItalic",
        family: "Noto Sans",
        bold: true,
        italic: true,
        mono: false,
        cjk: false,
        ps_name: "NotoSans-BoldItalic",
        data: font!("NotoSans-BoldItalic.ttf"),
        cff: false,
    },
    Face {
        id: "NotoSansMono-Regular",
        family: "Noto Sans Mono",
        bold: false,
        italic: false,
        mono: true,
        cjk: false,
        ps_name: "NotoSansMono-Regular",
        data: font!("NotoSansMono-Regular.ttf"),
        cff: false,
    },
    Face {
        id: "NotoSansMono-Bold",
        family: "Noto Sans Mono",
        bold: true,
        italic: false,
        mono: true,
        cjk: false,
        ps_name: "NotoSansMono-Bold",
        data: font!("NotoSansMono-Bold.ttf"),
        cff: false,
    },
    Face {
        id: "NotoSansCJK-Regular",
        family: "Noto Sans CJK",
        bold: false,
        italic: false,
        mono: false,
        cjk: true,
        ps_name: "NotoSansCJKsc-Regular",
        data: font!("NotoSansCJK-Regular.otf"),
        cff: true,
    },
];

/// MuPDF's copy of face `i` (for its cmap and advances). Load it once per rewrite: the CJK face
/// takes a moment to parse.
pub fn load(i: usize) -> Result<Font> {
    Font::from_bytes(FACES[i].ps_name, FACES[i].data).map_err(|e| EngineError::Mupdf(e.to_string()))
}

/// The face index for an id (`NotoSans-Bold`).
pub fn by_id(id: &str) -> Option<usize> {
    FACES.iter().position(|f| f.id == id)
}

/// The bundled face closest to the style: mono or not, bold, italic (the nearest style present).
pub fn latin(mono: bool, bold: bool, italic: bool) -> usize {
    let exact = FACES.iter().position(|f| !f.cjk && f.mono == mono && f.bold == bold && f.italic == italic);
    let near = FACES.iter().position(|f| !f.cjk && f.mono == mono && f.bold == bold);
    exact.or(near).unwrap_or(0)
}

/// The CJK face.
pub fn cjk() -> usize {
    FACES.iter().position(|f| f.cjk).unwrap_or(0)
}

/// The glyph id a loaded face has for `c`, if any.
pub fn glyph(f: &Font, c: char) -> Option<u16> {
    f.encode_character(c as i32).ok().filter(|g| *g > 0).and_then(|g| u16::try_from(g).ok())
}

/// The advance of glyph `g` of a loaded face, in thousandths of an em.
pub fn advance(f: &Font, g: u16) -> f64 {
    f.advance_glyph(i32::from(g)).map_or(500.0, |a| f64::from(a) * 1000.0)
}

/// The glyphs one rewrite uses from one face, in the order they were first used.
#[derive(Default)]
pub struct Subset {
    remapper: subsetter::GlyphRemapper,
    /// Original glyph id → the text it shows (for ToUnicode).
    text: BTreeMap<u16, String>,
}

impl Subset {
    pub fn use_glyph(&mut self, g: u16, text: &str) {
        self.remapper.remap(g);
        self.text.entry(g).or_insert_with(|| text.to_string());
    }

    /// The two-byte code glyph `g` has in the embedded font (its new glyph id).
    pub fn code(&self, g: u16) -> Vec<u8> {
        self.remapper.get(g).unwrap_or(0).to_be_bytes().to_vec()
    }

    /// Whether no glyph was used.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The new glyph ids in use.
    fn glyphs(&self) -> Vec<(u16, u16, &str)> {
        self.text.iter().filter_map(|(old, t)| Some((*old, self.remapper.get(*old)?, t.as_str()))).collect()
    }
}

/// A table of an OpenType font.
fn table<'a>(font: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let n = u16::from_be_bytes(font.get(4..6)?.try_into().ok()?) as usize;
    for i in 0..n {
        let rec = font.get(12 + 16 * i..28 + 16 * i)?;
        if &rec[..4] == tag {
            let off = u32::from_be_bytes(rec[8..12].try_into().ok()?) as usize;
            let len = u32::from_be_bytes(rec[12..16].try_into().ok()?) as usize;
            return font.get(off..off.checked_add(len)?);
        }
    }
    None
}

/// The font's box from its `head` table, in thousandths of an em.
fn bbox(font: &[u8]) -> [f64; 4] {
    let Some(head) = table(font, b"head") else { return [-200.0, -300.0, 1200.0, 1000.0] };
    let i16_at = |o: usize| head.get(o..o + 2).map_or(0, |b| i16::from_be_bytes([b[0], b[1]]));
    let upm = head.get(18..20).map_or(1000.0, |b| f64::from(u16::from_be_bytes([b[0], b[1]]))).max(1.0);
    [36, 38, 40, 42].map(|o| f64::from(i16_at(o)) * 1000.0 / upm)
}

/// A six-letter subset tag (`ABCDEF+`) derived from the glyphs.
fn tag(glyphs: &[(u16, u16, &str)]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (g, _, _) in glyphs {
        h = (h ^ u64::from(*g)).wrapping_mul(0x0100_0000_01b3);
    }
    (0..6).map(|i| char::from(b'A' + ((h >> (i * 5)) % 26) as u8)).collect()
}

fn utf16_hex(s: &str) -> String {
    s.encode_utf16().map(|u| format!("{u:04X}")).collect()
}

/// Embeds face `i` with the glyphs of `subset`: returns the Type0 font object, ready for a
/// page's /Font resources.
pub fn embed(doc: &mut PdfDocument, i: usize, font: &Font, subset: &Subset) -> Result<PdfObject> {
    let face = &FACES[i];
    let glyphs = subset.glyphs();
    let data = subsetter::subset(face.data, 0, &subset.remapper)
        .map_err(|e| EngineError::Invalid(format!("the font {} could not be subset ({e})", face.ps_name)))?;
    let name = format!("{}+{}", tag(&glyphs), face.ps_name);
    let (ascent, descent) = (f64::from(font.ascender()) * 1000.0, f64::from(font.descender()) * 1000.0);
    let bb = bbox(face.data);

    // The font program: TrueType as is (FontFile2), CFF as its bare table (FontFile3/CIDFontType0C).
    let (key, program) = if face.cff {
        (
            "FontFile3",
            table(&data, b"CFF ").ok_or_else(|| EngineError::Invalid(format!("the subset of {} has no CFF table", face.ps_name)))?.to_vec(),
        )
    } else {
        ("FontFile2", data)
    };
    let mut file_dict = doc.new_dict()?;
    if face.cff {
        file_dict.dict_put("Subtype", PdfObject::new_name("CIDFontType0C")?)?;
    }
    file_dict.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
    let file = doc.add_stream(&mupdf::Buffer::from_bytes(&super::text::flate(&program)?)?, Some(&file_dict), true)?;

    let mut desc = doc.new_object_from_str(&format!(
        "<< /Type /FontDescriptor /FontName /{name} /Flags {} /FontBBox [{} {} {} {}] /ItalicAngle {} /Ascent {} /Descent {} /CapHeight {} /StemV {} >>",
        4 | if face.italic { 64 } else { 0 } | if face.mono { 1 } else { 0 },
        bb[0].round(),
        bb[1].round(),
        bb[2].round(),
        bb[3].round(),
        if face.italic { -12 } else { 0 },
        ascent.round(),
        descent.round(),
        (ascent * 0.7).round(),
        if face.bold { 140 } else { 80 },
    ))?;
    desc.dict_put(key, file)?;
    let desc = doc.add_object(&desc)?;

    // Widths by new glyph id (= CID).
    let mut w = String::from("[");
    for (old, new, _) in &glyphs {
        let _ = write!(w, " {new} [{}]", advance(font, *old).round());
    }
    w.push(']');
    let cid = doc.new_object_from_str(&format!(
        "<< /Type /Font /Subtype /{} /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /DW 1000 /W {w} >>",
        if face.cff { "CIDFontType0" } else { "CIDFontType2" }
    ))?;
    let mut cid = cid;
    cid.dict_put("FontDescriptor", desc)?;
    if !face.cff {
        cid.dict_put("CIDToGIDMap", PdfObject::new_name("Identity")?)?;
    }
    let cid = doc.add_object(&cid)?;

    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    for chunk in glyphs.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (_, new, t) in chunk {
            let _ = writeln!(cmap, "<{new:04X}> <{}>", utf16_hex(t));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let mut cmap_dict = doc.new_dict()?;
    cmap_dict.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
    let tounicode = doc.add_stream(&mupdf::Buffer::from_bytes(&super::text::flate(cmap.as_bytes())?)?, Some(&cmap_dict), true)?;

    let mut type0 = doc.new_object_from_str(&format!("<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H >>"))?;
    let mut kids = doc.new_array()?;
    kids.array_push(cid)?;
    type0.dict_put("DescendantFonts", kids)?;
    type0.dict_put("ToUnicode", tounicode)?;
    Ok(doc.add_object(&type0)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_load_and_cover_their_scripts() {
        for (i, f) in FACES.iter().enumerate() {
            let font = load(i).unwrap();
            let probe = if f.cjk { ['你', 'あ', '한'] } else { ['A', 'Ω', 'Ж'] };
            for c in probe {
                assert!(glyph(&font, c).is_some(), "{} lacks {c}", f.id);
            }
        }
        assert_eq!(FACES[latin(false, true, true)].id, "NotoSans-BoldItalic");
        assert_eq!(FACES[latin(true, false, true)].id, "NotoSansMono-Regular");
    }

    #[test]
    fn subsets_are_small() {
        let mut doc = PdfDocument::new();
        for (i, text) in [(by_id("NotoSans-Regular").unwrap(), "Ωμέγα Привет"), (cjk(), "你好世界한국어")] {
            let font = load(i).unwrap();
            let mut s = Subset::default();
            for c in text.chars() {
                if let Some(g) = glyph(&font, c) {
                    s.use_glyph(g, &c.to_string());
                }
            }
            let data = subsetter::subset(FACES[i].data, 0, &s.remapper).unwrap();
            assert!(data.len() < 16 << 10, "{}: {} bytes", FACES[i].id, data.len());
            embed(&mut doc, i, &font, &s).unwrap();
        }
    }
}
