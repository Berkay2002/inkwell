// Adapted from pdfcraft (MIT OR Apache-2.0), crates/fonts/src/pdf.rs @ b8ddf71
//! What text editing needs from a PDF font: how to split a string into character codes, each
//! code's advance, the glyph height, what each code means (Unicode), and back: which text the font
//! can show. Widths come from the font dictionary (`/Widths`, `/W`, `/DW`, `/MissingWidth`), and
//! for a standard 14 font without widths from MuPDF's built-in metric-compatible faces. Meanings
//! come from `/ToUnicode`, else the encoding (`/Encoding` base and `/Differences` glyph names).
//!
//! pdfcraft reads the dictionaries through its own COS layer; here they are MuPDF objects, and a
//! subset font's glyph coverage is also checked against its embedded program (loaded by MuPDF).

use super::encodings;
use mupdf::Font;
use mupdf::pdf::PdfObject;
use std::collections::{HashMap, HashSet};

/// How the bytes of a string map to codes.
#[derive(Clone, Debug, PartialEq)]
enum Codes {
    One,
    Two,
    /// Codespace ranges `(length, low, high)` from an embedded CMap.
    Ranges(Vec<(usize, Vec<u8>, Vec<u8>)>),
}

/// Why new text can't be written in a font at all (shown text may still be read).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// Glyphs are content-stream procedures (Type 3).
    Type3,
    /// A vertical writing mode (`-V` CMap).
    Vertical,
    /// A predefined CMap other than Identity (`90ms-RKSJ-H`, `UniGB-UCS2-H`, …): its code lengths
    /// and code → CID mapping are not read, so its strings can't be split or written.
    Encoding,
}

pub struct Metrics {
    codes: Codes,
    first: u32,
    widths: Vec<f64>,
    cid_widths: HashMap<u32, f64>,
    cid_ranges: Vec<(u32, u32, f64)>,
    /// Code → CID for embedded non-identity CMaps.
    cid_map: Vec<(u32, u32, u32)>,
    default: f64,
    /// A standard 14 font without `/Widths`: MuPDF's face of it.
    std14: Option<Font>,
    /// The embedded program of a subset simple font, to check which glyphs it has.
    program: Option<Font>,
    /// Glyph units → text space (0.001, or `/FontMatrix[0]` for Type 3).
    pub scale: f64,
    pub ascent: f64,
    pub descent: f64,
    pub composite: bool,
    /// Code → Unicode text.
    unicode: HashMap<u32, String>,
    /// Some code has no meaning (no ToUnicode entry and nothing in the encoding).
    pub has_to_unicode: bool,
    pub base_font: String,
    pub subset: bool,
    /// The font program is in the file.
    pub embedded: bool,
    pub bold: bool,
    pub italic: bool,
    pub limit: Option<Limit>,
    code_len: usize,
}

/// Most ToUnicode mappings kept (a hostile CMap can't take minutes or gigabytes).
const MAX_CMAP_ENTRIES: usize = 1 << 20;

fn num(o: &Option<PdfObject>) -> Option<f64> {
    let o = o.as_ref()?;
    if o.is_number().ok()? { o.as_float().ok().map(f64::from) } else { None }
}

fn get(o: &PdfObject, key: &str) -> Option<PdfObject> {
    o.get_dict(key).ok().flatten()
}

fn name_of(o: &Option<PdfObject>) -> Option<Vec<u8>> {
    let o = o.as_ref()?;
    if o.is_name().ok()? { o.as_name().ok() } else { None }
}

fn nums(o: &Option<PdfObject>) -> Vec<f64> {
    let Some(a) = o.as_ref().filter(|a| a.is_array().unwrap_or(false)) else {
        return Vec::new();
    };
    let n = a.len().unwrap_or(0);
    (0..n as i32).map(|i| num(&a.get_array(i).ok().flatten()).unwrap_or(0.0)).collect()
}

fn be(b: &[u8]) -> u32 {
    b.iter().fold(0u32, |a, x| (a << 8) | u32::from(*x))
}

fn style_from_name(name: &str) -> (bool, bool) {
    let name = name.to_ascii_lowercase();
    let name = name.split_once('+').map_or(name.as_str(), |(_, n)| n);
    (
        ["bold", "black", "heavy", "semibold", "demi"].iter().any(|s| name.contains(s)),
        ["italic", "oblique", "slanted"].iter().any(|s| name.contains(s)),
    )
}

/// A standard 14 font name for a `/BaseFont` without widths.
fn std14_name(base: &str, bold: bool, italic: bool) -> &'static str {
    let b = base.to_ascii_lowercase();
    if b.contains("symbol") {
        return "Symbol";
    }
    if b.contains("dingbat") {
        return "ZapfDingbats";
    }
    let family = if b.contains("courier") || b.contains("mono") {
        0
    } else if b.contains("times") || (b.contains("serif") && !b.contains("sans")) || b.contains("roman") {
        1
    } else {
        2
    };
    STD14[family][bold as usize + 2 * italic as usize]
}

/// [family][regular, bold, italic, bold italic] for Courier, Times and Helvetica.
pub const STD14: [[&str; 4]; 3] = [
    ["Courier", "Courier-Bold", "Courier-Oblique", "Courier-BoldOblique"],
    ["Times-Roman", "Times-Bold", "Times-Italic", "Times-BoldItalic"],
    ["Helvetica", "Helvetica-Bold", "Helvetica-Oblique", "Helvetica-BoldOblique"],
];

/// See [`Metrics::summary`].
pub struct Summary {
    pub base_font: String,
    pub bold: bool,
    pub italic: bool,
    pub embedded: bool,
    pub subset: bool,
    pub usable: bool,
}

impl Metrics {
    /// Metrics for text without a usable font (Helvetica-like).
    pub fn fallback() -> Self {
        Metrics {
            codes: Codes::One,
            first: 0,
            widths: Vec::new(),
            cid_widths: HashMap::new(),
            cid_ranges: Vec::new(),
            cid_map: Vec::new(),
            default: 500.0,
            std14: Font::new("Helvetica").ok(),
            program: None,
            scale: 0.001,
            ascent: 0.9,
            descent: -0.25,
            composite: false,
            unicode: (32..127u32).map(|c| (c, char::from_u32(c).unwrap_or(' ').to_string())).collect(),
            has_to_unicode: true,
            base_font: "Helvetica".into(),
            subset: false,
            embedded: false,
            bold: false,
            italic: false,
            limit: None,
            code_len: 1,
        }
    }

    /// What a font list shows of a font, from its dicts alone: no font program, CMap or
    /// ToUnicode stream is read. `usable` is what [`Metrics::from_dict`] would allow for writing
    /// (a CMap stream counts as horizontal unless it says `/WMode 1`).
    pub fn summary(font: &PdfObject) -> Summary {
        let subtype = name_of(&get(font, "Subtype")).unwrap_or_else(|| b"Type1".to_vec());
        let composite = subtype == b"Type0";
        let descendant = if composite { get(font, "DescendantFonts").and_then(|a| a.get_array(0).ok().flatten()) } else { None };
        let descriptor = get(descendant.as_ref().unwrap_or(font), "FontDescriptor");
        let base = name_of(&get(font, "BaseFont"))
            .or_else(|| descriptor.as_ref().and_then(|d| name_of(&get(d, "FontName"))))
            .map_or_else(|| String::from_utf8_lossy(&subtype).into_owned(), |n| String::from_utf8_lossy(&n).into_owned());
        let (mut bold, mut italic) = style_from_name(&base);
        if let Some(d) = &descriptor {
            let flags = num(&get(d, "Flags")).unwrap_or(0.0) as i64;
            italic |= flags & 64 != 0 || num(&get(d, "ItalicAngle")).unwrap_or(0.0).abs() > 0.1;
            bold |= flags & 262_144 != 0 || num(&get(d, "FontWeight")).unwrap_or(0.0) >= 600.0;
        }
        let writable = if subtype == b"Type3" {
            false
        } else if composite {
            match get(font, "Encoding") {
                Some(e) if e.is_stream().unwrap_or(false) => num(&get(&e, "WMode")) != Some(1.0),
                e => name_of(&e).as_deref() == Some(b"Identity-H"),
            }
        } else {
            true
        };
        let to_unicode = get(font, "ToUnicode").is_some_and(|t| t.is_stream().unwrap_or(false));
        Summary {
            embedded: descriptor.as_ref().is_some_and(|d| ["FontFile", "FontFile2", "FontFile3"].iter().any(|k| get(d, k).is_some())),
            subset: base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase()),
            usable: writable && (!composite || to_unicode),
            base_font: base,
            bold,
            italic,
        }
    }

    pub fn from_dict(font: &PdfObject) -> Self {
        let mut m = Metrics::fallback();
        m.std14 = None;
        let subtype = name_of(&get(font, "Subtype")).unwrap_or_else(|| b"Type1".to_vec());
        let mut base = name_of(&get(font, "BaseFont")).map(|n| String::from_utf8_lossy(&n).into_owned()).unwrap_or_default();
        let descriptor;
        if subtype == b"Type0" {
            m.composite = true;
            m.default = 1000.0;
            m.codes = Codes::Two;
            match get(font, "Encoding") {
                Some(e) if e.is_stream().unwrap_or(false) => {
                    if let Ok(data) = e.read_stream() {
                        let (spaces, cids, vertical) = parse_cmap(&data);
                        if !spaces.is_empty() {
                            m.codes = Codes::Ranges(spaces);
                        }
                        m.cid_map = cids;
                        if vertical || num(&get(&e, "WMode")) == Some(1.0) {
                            m.limit = Some(Limit::Vertical);
                        }
                    }
                }
                e => match name_of(&e).as_deref() {
                    Some(b"Identity-H") => {}
                    Some(b"Identity-V") => m.limit = Some(Limit::Vertical),
                    Some(n) if n.ends_with(b"-V") => m.limit = Some(Limit::Vertical),
                    _ => m.limit = Some(Limit::Encoding),
                },
            }
            let desc = get(font, "DescendantFonts").and_then(|a| a.get_array(0).ok().flatten());
            if let Some(desc) = &desc {
                if let Some(dw) = num(&get(desc, "DW")) {
                    m.default = dw;
                }
                read_w(&mut m, get(desc, "W"));
            }
            descriptor = desc.as_ref().and_then(|d| get(d, "FontDescriptor"));
        } else {
            m.first = num(&get(font, "FirstChar")).unwrap_or(0.0).max(0.0) as u32;
            m.widths = nums(&get(font, "Widths"));
            descriptor = get(font, "FontDescriptor");
            m.default = descriptor.as_ref().and_then(|d| num(&get(d, "MissingWidth"))).unwrap_or(0.0);
            if subtype == b"Type3" {
                m.limit = Some(Limit::Type3);
                let fm = nums(&get(font, "FontMatrix"));
                if fm.len() == 6 {
                    m.scale = fm[0];
                    let bbox = nums(&get(font, "FontBBox"));
                    if bbox.len() == 4 && fm[3] != 0.0 {
                        m.ascent = (bbox[3] * fm[3]).max(0.5);
                        m.descent = (bbox[1] * fm[3]).min(-0.1);
                    }
                }
                if base.is_empty() {
                    base = descriptor
                        .as_ref()
                        .and_then(|d| name_of(&get(d, "FontName")))
                        .map(|n| String::from_utf8_lossy(&n).into_owned())
                        .unwrap_or_else(|| "Type3".into());
                }
            }
        }
        let (bold, italic) = style_from_name(&base);
        m.bold |= bold;
        m.italic |= italic;
        if let Some(d) = &descriptor {
            let flags = num(&get(d, "Flags")).unwrap_or(0.0) as i64;
            let angle = num(&get(d, "ItalicAngle")).unwrap_or(0.0);
            let weight = num(&get(d, "FontWeight")).unwrap_or(0.0);
            m.italic |= flags & 64 != 0 || angle.abs() > 0.1;
            m.bold |= flags & 262_144 != 0 || weight >= 600.0;
            if subtype != b"Type3" {
                let a = num(&get(d, "Ascent")).unwrap_or(0.0) / 1000.0;
                m.ascent = if a > 0.3 { a.min(1.5) } else { 0.9 };
                let de = num(&get(d, "Descent")).unwrap_or(0.0) / 1000.0;
                m.descent = if de < -0.05 { de.max(-0.6) } else { -0.25 };
            }
        }
        if subtype != b"Type3" && !m.composite && m.widths.is_empty() {
            m.std14 = Font::new(std14_name(&base, m.bold, m.italic)).ok();
        }
        m.embedded = descriptor.as_ref().is_some_and(|d| ["FontFile", "FontFile2", "FontFile3"].iter().any(|k| get(d, k).is_some()));
        m.subset = base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase());
        if m.subset && !m.composite && subtype != b"Type3" {
            m.program = descriptor.as_ref().and_then(load_program);
        }
        m.base_font = base;
        m.code_len = match &m.codes {
            Codes::One => 1,
            Codes::Two => 2,
            Codes::Ranges(r) => r.last().map_or(2, |x| x.0),
        };
        let (unicode, has) = unicode_map(font, m.composite);
        m.unicode = unicode;
        m.has_to_unicode = has;
        m
    }

    /// Split a string into `(code, byte length)`.
    pub fn codes(&self, s: &[u8]) -> Vec<(u32, usize)> {
        let mut out = Vec::with_capacity(s.len());
        let mut i = 0;
        while i < s.len() {
            let len = match &self.codes {
                Codes::One => 1,
                Codes::Two => 2,
                Codes::Ranges(r) => r
                    .iter()
                    .find(|(len, lo, hi)| {
                        s.get(i..i + len).is_some_and(|b| b.iter().zip(lo.iter().zip(hi)).all(|(x, (l, h))| x >= l && x <= h))
                    })
                    .map_or(r.first().map_or(1, |x| x.0), |x| x.0),
            };
            let len = len.min(s.len() - i).max(1);
            out.push((be(&s[i..i + len]), len));
            i += len;
        }
        out
    }

    fn cid(&self, code: u32) -> u32 {
        if self.cid_map.is_empty() {
            return code;
        }
        self.cid_map.iter().find(|(lo, hi, _)| code >= *lo && code <= *hi).map_or(0, |(lo, _, c)| c.saturating_add(code - lo))
    }

    /// The advance of `code` in text space per unit of font size.
    pub fn width(&self, code: u32) -> f64 {
        if self.composite {
            let cid = self.cid(code);
            let w =
                self.cid_widths.get(&cid).copied().or_else(|| self.cid_ranges.iter().find(|(a, b, _)| cid >= *a && cid <= *b).map(|r| r.2));
            return w.unwrap_or(self.default) * self.scale;
        }
        if let Some(w) = code.checked_sub(self.first).and_then(|i| self.widths.get(i as usize)) {
            return w * self.scale;
        }
        if let Some(f) = &self.std14 {
            let c = self.unicode.get(&code).and_then(|t| t.chars().next()).unwrap_or('n');
            if let Ok(g) = f.encode_character(c as i32) {
                if let Ok(a) = f.advance_glyph(g) {
                    return f64::from(a);
                }
            }
            return 0.5;
        }
        self.default * self.scale
    }

    /// Word spacing applies to the single-byte code 32 (§9.3.3).
    pub fn is_space(&self, code: u32, len: usize) -> bool {
        code == 32 && len == 1
    }

    /// The Unicode text of one code, if known.
    pub fn text_of(&self, code: u32) -> Option<&str> {
        self.unicode.get(&code).map(String::as_str)
    }

    /// Whether the font has a glyph for `code` showing `c` (subset fonts lack the glyphs they
    /// don't use).
    fn has_glyph(&self, code: u32, c: char) -> bool {
        if self.composite {
            let cid = self.cid(code);
            return self.cid_widths.contains_key(&cid) || self.cid_ranges.iter().any(|(a, b, _)| cid >= *a && cid <= *b) || !self.subset;
        }
        let width_ok = match code.checked_sub(self.first).and_then(|i| self.widths.get(i as usize)) {
            Some(w) => *w > 0.0 || code == 32,
            None => self.widths.is_empty() && !self.subset,
        };
        if !width_ok {
            return false;
        }
        match &self.program {
            // A subset keeps only the glyphs the document used: ask the program itself.
            Some(p) if c != ' ' => p.encode_character(c as i32).is_ok_and(|g| g > 0),
            _ => true,
        }
    }

    /// The bytes that show `text` in this font, or `None` if some character has no code or no
    /// glyph in it (the caller then substitutes another font).
    pub fn encode(&self, text: &str) -> Option<Vec<u8>> {
        if self.limit.is_some() {
            return None;
        }
        let wanted: HashSet<char> = text.chars().collect();
        let mut reverse: HashMap<char, u32> = HashMap::new();
        for (&code, t) in &self.unicode {
            let mut it = t.chars();
            let (Some(c), None) = (it.next(), it.next()) else {
                continue;
            };
            if wanted.contains(&c) && self.has_glyph(code, c) {
                reverse.entry(c).and_modify(|r| *r = (*r).min(code)).or_insert(code);
            }
        }
        let mut out = Vec::with_capacity(text.len() * self.code_len);
        for ch in text.chars() {
            let code = *reverse.get(&ch)?;
            let len = self.len_of(code)?;
            out.extend_from_slice(&code.to_be_bytes()[4 - len..]);
        }
        Some(out)
    }

    /// How many bytes `code` takes: its codespace range's length (CMaps may mix one- and
    /// two-byte codes). None when no range holds it.
    fn len_of(&self, code: u32) -> Option<usize> {
        match &self.codes {
            Codes::One => (code < 0x100).then_some(1),
            Codes::Two => (code < 0x1_0000).then_some(2),
            Codes::Ranges(r) => r.iter().find_map(|(len, lo, hi)| {
                let len = (*len).clamp(1, 4);
                if len < 4 && code >= 1u32 << (8 * len) {
                    return None;
                }
                let bytes = &code.to_be_bytes()[4 - len..];
                bytes.iter().zip(lo.iter().zip(hi)).all(|(x, (l, h))| x >= l && x <= h).then_some(len)
            }),
        }
    }
}

fn read_w(m: &mut Metrics, w: Option<PdfObject>) {
    let Some(w) = w.filter(|w| w.is_array().unwrap_or(false)) else {
        return;
    };
    let n = w.len().unwrap_or(0) as i32;
    let mut i = 0;
    while i < n {
        let Some(c0) = num(&w.get_array(i).ok().flatten()).map(|v| v as u32) else {
            break;
        };
        let next = w.get_array(i + 1).ok().flatten();
        match &next {
            Some(a) if a.is_array().unwrap_or(false) => {
                for (k, v) in nums(&next).into_iter().enumerate() {
                    m.cid_widths.insert(c0 + k as u32, v);
                }
                i += 2;
            }
            Some(_) => {
                if let (Some(c1), Some(v)) = (num(&next), num(&w.get_array(i + 2).ok().flatten())) {
                    m.cid_ranges.push((c0, c1 as u32, v));
                }
                i += 3;
            }
            None => break,
        }
    }
}

/// The embedded font program of a descriptor, loaded by MuPDF.
fn load_program(d: &PdfObject) -> Option<Font> {
    let f = ["FontFile", "FontFile2", "FontFile3"].iter().find_map(|k| get(d, k))?;
    let data = f.read_stream().ok()?;
    Font::from_bytes("embedded", &data).ok()
}

/// A token of a CMap stream (PostScript syntax, ISO 32000-2 §9.10.3).
#[derive(Debug, PartialEq)]
enum Tok<'a> {
    Hex(Option<Vec<u8>>),
    Open,
    Close,
    Word(&'a [u8]),
}

struct CLexer<'a> {
    data: &'a [u8],
    pos: usize,
}

fn is_space(b: u8) -> bool {
    matches!(b, 0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

fn is_delimiter(b: u8) -> bool {
    matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

impl<'a> Iterator for CLexer<'a> {
    type Item = Tok<'a>;

    fn next(&mut self) -> Option<Tok<'a>> {
        let at = |i: usize| self.data.get(i).copied();
        loop {
            let b = at(self.pos)?;
            if is_space(b) {
                self.pos += 1;
            } else if b == b'%' {
                while at(self.pos).is_some_and(|c| c != b'\n' && c != b'\r') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        let start = self.pos;
        let b = at(start)?;
        self.pos += 1;
        match b {
            b'[' => Some(Tok::Open),
            b']' => Some(Tok::Close),
            b'<' | b'>' if self.data.get(self.pos) == Some(&b) => {
                self.pos += 1;
                Some(Tok::Word(&self.data[start..self.pos]))
            }
            b'<' => {
                let mut digits = Vec::new();
                let mut valid = true;
                loop {
                    match self.data.get(self.pos).copied() {
                        None => return Some(Tok::Hex(None)),
                        Some(b'>') => break,
                        Some(c) if is_space(c) => {}
                        Some(c) => match (c as char).to_digit(16) {
                            Some(d) => digits.push(d as u8),
                            None => valid = false,
                        },
                    }
                    self.pos += 1;
                }
                self.pos += 1;
                if !valid || digits.is_empty() || digits.len() % 2 != 0 {
                    return Some(Tok::Hex(None));
                }
                Some(Tok::Hex(Some(digits.chunks(2).map(|p| p[0] << 4 | p[1]).collect())))
            }
            b'(' => {
                let mut depth = 1usize;
                while depth > 0 {
                    match self.data.get(self.pos).copied() {
                        None => break,
                        Some(b'\\') => self.pos += 1,
                        Some(b'(') => depth += 1,
                        Some(b')') => depth -= 1,
                        _ => {}
                    }
                    self.pos += 1;
                }
                Some(Tok::Word(&self.data[start..self.pos.min(self.data.len())]))
            }
            _ => {
                if b == b'/' || !is_delimiter(b) {
                    while self.data.get(self.pos).is_some_and(|&c| !is_space(c) && !is_delimiter(c)) {
                        self.pos += 1;
                    }
                }
                Some(Tok::Word(&self.data[start..self.pos]))
            }
        }
    }
}

/// The next entry of a `begin… end…` section: `N` tokens, or `None` at its `end` operator.
fn entry<'a, const N: usize>(toks: &mut CLexer<'a>, end: &[u8]) -> Option<[Tok<'a>; N]> {
    let mut out = Vec::with_capacity(N);
    for _ in 0..N {
        match toks.next()? {
            Tok::Word(w) if w == end => return None,
            t => out.push(t),
        }
    }
    out.try_into().ok()
}

fn number(t: &Tok) -> Option<u32> {
    match t {
        Tok::Word(w) => std::str::from_utf8(w).ok()?.parse().ok(),
        _ => None,
    }
}

type Spaces = Vec<(usize, Vec<u8>, Vec<u8>)>;

/// Codespace ranges, CID mappings and the writing mode of an embedded CMap.
fn parse_cmap(data: &[u8]) -> (Spaces, Vec<(u32, u32, u32)>, bool) {
    let (mut spaces, mut cids) = (Vec::new(), Vec::new());
    let mut vertical = false;
    let mut toks = CLexer { data, pos: 0 };
    let mut prev_wmode = false;
    while let Some(t) = toks.next() {
        match t {
            Tok::Word(b"/WMode") => {
                prev_wmode = true;
                continue;
            }
            Tok::Word(b"1") if prev_wmode => vertical = true,
            Tok::Word(b"begincodespacerange") => {
                while let Some([lo, hi]) = entry(&mut toks, b"endcodespacerange") {
                    if let (Tok::Hex(Some(lo)), Tok::Hex(Some(hi))) = (lo, hi) {
                        if lo.len() == hi.len() && spaces.len() < 1 << 16 {
                            spaces.push((lo.len(), lo, hi));
                        }
                    }
                }
            }
            Tok::Word(b"begincidrange") => {
                while let Some([lo, hi, c]) = entry(&mut toks, b"endcidrange") {
                    if let (Tok::Hex(Some(lo)), Tok::Hex(Some(hi)), Some(c)) = (&lo, &hi, number(&c)) {
                        if cids.len() < 1 << 16 {
                            cids.push((be(lo), be(hi), c));
                        }
                    }
                }
            }
            Tok::Word(b"begincidchar") => {
                while let Some([code, c]) = entry(&mut toks, b"endcidchar") {
                    if let (Tok::Hex(Some(code)), Some(c)) = (&code, number(&c)) {
                        if cids.len() < 1 << 16 {
                            cids.push((be(code), be(code), c));
                        }
                    }
                }
            }
            _ => {}
        }
        prev_wmode = false;
    }
    spaces.sort_by_key(|s| s.0);
    (spaces, cids, vertical)
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
    String::from_utf16_lossy(&units)
}

/// `bfchar`/`bfrange` entries of a ToUnicode CMap (later entries win).
pub(crate) fn parse_to_unicode(data: &[u8], out: &mut HashMap<u32, String>) {
    let mut budget = MAX_CMAP_ENTRIES;
    let mut toks = CLexer { data, pos: 0 };
    'scan: while let Some(t) = toks.next() {
        match t {
            Tok::Word(b"beginbfchar") => {
                while let Some([c, u]) = entry(&mut toks, b"endbfchar") {
                    if let (Tok::Hex(Some(c)), Tok::Hex(Some(u))) = (c, u) {
                        if budget == 0 {
                            break 'scan;
                        }
                        budget -= 1;
                        out.insert(be(&c), utf16(&u));
                    }
                }
            }
            Tok::Word(b"beginbfrange") => {
                while let Some([lo, hi, dst]) = entry(&mut toks, b"endbfrange") {
                    let (Tok::Hex(Some(lo)), Tok::Hex(Some(hi))) = (lo, hi) else {
                        break;
                    };
                    let (lo, hi) = (be(&lo), be(&hi));
                    if hi < lo || hi - lo >= 65536 {
                        continue;
                    }
                    match dst {
                        Tok::Open => {
                            let mut code = lo;
                            for t in toks.by_ref() {
                                match t {
                                    Tok::Close => break,
                                    Tok::Hex(Some(u)) if code <= hi && budget > 0 => {
                                        budget -= 1;
                                        out.insert(code, utf16(&u));
                                        code += 1;
                                    }
                                    _ => code += 1,
                                }
                            }
                        }
                        Tok::Hex(Some(u)) => {
                            let mut units: Vec<u16> = u.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
                            let Some(last) = units.last().copied() else {
                                continue;
                            };
                            for k in 0..=(hi - lo) {
                                if budget == 0 {
                                    break 'scan;
                                }
                                budget -= 1;
                                *units.last_mut().expect("not empty") = last.wrapping_add(k as u16);
                                out.insert(lo + k, String::from_utf16_lossy(&units));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// A glyph name's Unicode (Annex D names, `uniXXXX`, `uXXXX[XX]`).
pub fn glyph_unicode(name: &str) -> Option<char> {
    if let Ok(i) = encodings::NAMES.binary_search_by(|(n, _)| (*n).cmp(name)) {
        return char::from_u32(encodings::NAMES[i].1);
    }
    let hex =
        name.strip_prefix("uni").filter(|h| h.len() == 4).or_else(|| name.strip_prefix('u').filter(|h| (4..=6).contains(&h.len())))?;
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

/// Code → Unicode for a font (its ToUnicode CMap over its encoding), and whether it has a
/// meaning for text at all (a ToUnicode CMap, or a simple font's encoding).
fn unicode_map(font: &PdfObject, composite: bool) -> (HashMap<u32, String>, bool) {
    let mut out = HashMap::new();
    let mut known = false;
    if !composite {
        known = true;
        let enc = get(font, "Encoding");
        let base_name = match &enc {
            Some(e) if e.is_name().unwrap_or(false) => e.as_name().ok(),
            Some(e) if e.is_dict().unwrap_or(false) => name_of(&get(e, "BaseEncoding")),
            _ => None,
        };
        let table: &[u32; 256] = match base_name.as_deref() {
            Some(b"WinAnsiEncoding") => &encodings::WIN_ANSI,
            Some(b"MacRomanEncoding") => &encodings::MAC_ROMAN,
            _ => &encodings::STANDARD,
        };
        for (code, u) in table.iter().enumerate() {
            if let Some(c) = char::from_u32(*u).filter(|_| *u != 0) {
                out.insert(code as u32, c.to_string());
            }
        }
        if let Some(diffs) = enc.as_ref().filter(|e| e.is_dict().unwrap_or(false)).and_then(|e| get(e, "Differences")) {
            let mut code = 0u32;
            for i in 0..diffs.len().unwrap_or(0) as i32 {
                let Some(item) = diffs.get_array(i).ok().flatten() else {
                    continue;
                };
                if item.is_number().unwrap_or(false) {
                    code = item.as_int().unwrap_or(0).max(0) as u32;
                } else if let Ok(n) = item.as_name() {
                    match glyph_unicode(&String::from_utf8_lossy(&n)) {
                        Some(c) => out.insert(code, c.to_string()),
                        None => out.remove(&code),
                    };
                    code = code.saturating_add(1);
                }
            }
        }
    }
    if let Some(t) = get(font, "ToUnicode").filter(|t| t.is_stream().unwrap_or(false)) {
        if let Ok(data) = t.read_stream() {
            parse_to_unicode(&data, &mut out);
            known = true;
        }
    }
    (out, known)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_to_unicode_ranges_and_chars() {
        let mut m = HashMap::new();
        let cmap = b"2 beginbfchar <0003> <0020> <0024> <0041> endbfchar\n1 beginbfrange <0044> <0046> <0061> endbfrange\n\
                     1 beginbfrange <0050> <0051> [<00E9> <00F6>] endbfrange";
        parse_to_unicode(cmap, &mut m);
        assert_eq!(m.get(&3).map(String::as_str), Some(" "));
        assert_eq!(m.get(&0x24).map(String::as_str), Some("A"));
        assert_eq!(m.get(&0x46).map(String::as_str), Some("c"));
        assert_eq!(m.get(&0x51).map(String::as_str), Some("ö"));
    }

    #[test]
    fn mixed_length_codes_keep_their_length() {
        let mut doc = mupdf::pdf::PdfDocument::new();
        let cmap = b"/WMode 0 def 2 begincodespacerange <00> <80> <8140> <9FFC> endcodespacerange";
        let enc = doc.add_stream(&mupdf::Buffer::from_bytes(cmap).unwrap(), None, false).unwrap();
        let tu = b"2 beginbfchar <41> <0041> <8140> <3042> endbfchar";
        let tu = doc.add_stream(&mupdf::Buffer::from_bytes(tu).unwrap(), None, false).unwrap();
        let mut font = doc
            .new_object_from_str("<< /Type /Font /Subtype /Type0 /BaseFont /Mixed /DescendantFonts [<< /Subtype /CIDFontType0 >>] >>")
            .unwrap();
        font.dict_put("Encoding", enc).unwrap();
        font.dict_put("ToUnicode", tu).unwrap();
        let m = Metrics::from_dict(&font);
        assert_eq!(m.limit, None);
        assert_eq!(m.encode("Aあ"), Some(vec![0x41, 0x81, 0x40]));
        assert_eq!(m.codes(&[0x41, 0x81, 0x40]), vec![(0x41, 1), (0x8140, 2)]);
    }

    #[test]
    fn predefined_cmaps_other_than_identity_are_not_written() {
        let doc = mupdf::pdf::PdfDocument::new();
        let font = |enc: &str| {
            let src =
                format!("<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /{enc} /DescendantFonts [<< /Subtype /CIDFontType0 >>] >>");
            Metrics::from_dict(&doc.new_object_from_str(&src).unwrap())
        };
        assert_eq!(font("Identity-H").limit, None);
        assert_eq!(font("90ms-RKSJ-H").limit, Some(Limit::Encoding));
        assert_eq!(font("UniJIS-UCS2-V").limit, Some(Limit::Vertical));
        assert_eq!(font("90ms-RKSJ-H").encode("a"), None);
    }

    #[test]
    fn names_glyphs() {
        assert_eq!(glyph_unicode("A"), Some('A'));
        assert_eq!(glyph_unicode("uni00E9"), Some('é'));
        assert_eq!(glyph_unicode("quoteright"), Some('\u{2019}'));
    }
}
