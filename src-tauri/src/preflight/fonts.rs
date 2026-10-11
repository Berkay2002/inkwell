//! Font embedding for the conversion: which missing fonts can be embedded and from what, and the
//! embedding itself. Nothing is substituted silently: a font is embedded from the built-in
//! base-14 clone it is named after, or from the installed font file of the same name, or it is
//! reported as blocked with the reason.

use super::Result;
use crate::a11y::tree::{name, xref_of};
use crate::textedit::encodings::{MAC_ROMAN, NAMES, STANDARD, WIN_ANSI};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Buffer, Font};
use std::path::PathBuf;

/// What can be done about a font.
#[derive(Clone, Debug, PartialEq)]
pub enum FontPlan {
    /// Embedded already (or a Type 3 font, which has no font file).
    Embedded,
    /// The built-in clone of one of the standard 14 fonts.
    Builtin { program: &'static str },
    /// The installed TrueType file of the same name.
    Installed { path: PathBuf, ps_name: String },
    /// `reason` says it in the finding; `note` completes "N need ..." in the outcome line.
    Blocked { reason: String, note: &'static str },
}

fn blocked(reason: impl Into<String>, note: &'static str) -> FontPlan {
    FontPlan::Blocked { reason: reason.into(), note }
}

/// The CIDFont of a Type 0 font.
pub fn descendant(f: &PdfObject) -> Result<Option<PdfObject>> {
    let Some(d) = f.get_dict("DescendantFonts")? else { return Ok(None) };
    if d.is_array()? { Ok(d.get_array(0)?.filter(|x| x.is_dict().unwrap_or(false))) } else { Ok(Some(d).filter(|x| x.is_dict().unwrap_or(false))) }
}

fn descriptor(f: &PdfObject) -> Result<Option<PdfObject>> {
    let target = if name(f, "Subtype")?.as_deref() == Some("Type0") { descendant(f)? } else { Some(f.share()) };
    Ok(target.and_then(|t| t.get_dict("FontDescriptor").ok().flatten()).filter(|d| d.is_dict().unwrap_or(false)))
}

fn has_file(d: &PdfObject) -> bool {
    ["FontFile", "FontFile2", "FontFile3"].iter().any(|k| d.get_dict(*k).ok().flatten().is_some())
}

/// `(family, bold, italic)` when `base` is one of the base-14 names (or the common spellings of them).
fn base14(base: &str) -> Option<&'static str> {
    let b = base.replace(' ', "");
    let (fam, style) = match b.find(['-', ',']) {
        Some(i) => (&b[..i], b[i + 1..].to_ascii_lowercase()),
        None => (b.as_str(), String::new()),
    };
    let bold = style.contains("bold");
    let italic = style.contains("italic") || style.contains("oblique");
    Some(match (fam.to_ascii_lowercase().as_str(), bold, italic) {
        ("helvetica", false, false) => "Helvetica",
        ("helvetica", true, false) => "Helvetica-Bold",
        ("helvetica", false, true) => "Helvetica-Oblique",
        ("helvetica", true, true) => "Helvetica-BoldOblique",
        ("times", false, false) => "Times-Roman",
        ("times", true, false) => "Times-Bold",
        ("times", false, true) => "Times-Italic",
        ("times", true, true) => "Times-BoldItalic",
        ("courier", false, false) => "Courier",
        ("courier", true, false) => "Courier-Bold",
        ("courier", false, true) => "Courier-Oblique",
        ("courier", true, true) => "Courier-BoldOblique",
        _ => return None,
    })
}

pub fn classify(f: &PdfObject) -> Result<FontPlan> {
    let sub = name(f, "Subtype")?.unwrap_or_default();
    if sub == "Type3" {
        return Ok(FontPlan::Embedded);
    }
    let desc = descriptor(f)?;
    if desc.as_ref().is_some_and(has_file) {
        return Ok(FontPlan::Embedded);
    }
    let base = name(f, "BaseFont")?.unwrap_or_default();
    if sub == "Type0" {
        return Ok(blocked("a CID font whose font file isn't in the document can't be rebuilt", "the original font files"));
    }
    if base.is_empty() {
        return Ok(blocked("the font has no name to look up", "the original font files"));
    }
    let (subset, plain) = match base.split_once('+') {
        Some((p, r)) if p.len() == 6 && p.chars().all(|c| c.is_ascii_uppercase()) => (true, r.to_string()),
        _ => (false, base.clone()),
    };
    if subset {
        return Ok(blocked(format!("{plain} is a subset whose font file is missing"), "the original font files"));
    }
    if matches!(plain.as_str(), "Symbol" | "ZapfDingbats") {
        return Ok(blocked(format!("{plain} is a symbol font with no clone to embed"), "fonts that aren't installed"));
    }
    if let Some(program) = base14(&plain) {
        if Font::base14_program(program).is_some() {
            return Ok(FontPlan::Builtin { program });
        }
    }
    let Some(path) = crate::engine::fonts::installed_file(&plain) else {
        return Ok(blocked(format!("{plain} isn't installed on this computer"), "fonts that aren't installed"));
    };
    let Ok(data) = std::fs::read(&path) else {
        return Ok(blocked(format!("the file of {plain} can't be read"), "fonts that aren't installed"));
    };
    let Some(info) = TtInfo::parse(&data) else {
        return Ok(blocked(format!("{plain} is not a TrueType font file"), "fonts that aren't installed"));
    };
    if info.fs_type & 0x0002 != 0 {
        return Ok(blocked(format!("{plain} doesn't allow embedding (its licence flags forbid it)"), "fonts that allow embedding"));
    }
    Ok(FontPlan::Installed { path, ps_name: info.ps_name })
}

// ---- TrueType facts ------------------------------------------------------------------------------

struct TtInfo {
    ps_name: String,
    fs_type: u16,
    units: f32,
    bbox: [i16; 4],
    ascent: i16,
    descent: i16,
    cap_height: Option<i16>,
    italic_angle: f32,
    fixed: bool,
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*d.get(o)?, *d.get(o + 1)?, *d.get(o + 2)?, *d.get(o + 3)?]))
}

impl TtInfo {
    fn parse(d: &[u8]) -> Option<TtInfo> {
        let magic = be32(d, 0)?;
        if magic != 0x0001_0000 && magic != 0x7472_7565 {
            return None;
        }
        let n = be16(d, 4)? as usize;
        let table = |tag: &[u8; 4]| -> Option<(usize, usize)> {
            (0..n).find_map(|i| {
                let r = 12 + i * 16;
                (d.get(r..r + 4)? == tag).then(|| Some((be32(d, r + 8)? as usize, be32(d, r + 12)? as usize))).flatten()
            })
        };
        let (head, _) = table(b"head")?;
        let units = be16(d, head + 18)? as f32;
        let bbox = [be16(d, head + 36)? as i16, be16(d, head + 38)? as i16, be16(d, head + 40)? as i16, be16(d, head + 42)? as i16];
        let (hhea, _) = table(b"hhea")?;
        let (ascent, descent) = (be16(d, hhea + 4)? as i16, be16(d, hhea + 6)? as i16);
        let (fs_type, cap_height) = match table(b"OS/2") {
            Some((o, len)) => {
                let version = be16(d, o)?;
                (be16(d, o + 8)?, if version >= 2 && len >= 90 { be16(d, o + 88).map(|v| v as i16) } else { None })
            }
            None => (0, None),
        };
        let (italic_angle, fixed) = match table(b"post") {
            Some((o, _)) => (be32(d, o + 4)? as i32 as f32 / 65536.0, be32(d, o + 12)? != 0),
            None => (0.0, false),
        };
        let ps_name = Self::ps_name(d, table(b"name")?.0)?;
        Some(TtInfo { ps_name, fs_type, units: units.max(1.0), bbox, ascent, descent, cap_height, italic_angle, fixed })
    }

    /// nameID 6 (the PostScript name), Mac Roman or Windows Unicode.
    fn ps_name(d: &[u8], at: usize) -> Option<String> {
        let count = be16(d, at + 2)? as usize;
        let strings = at + be16(d, at + 4)? as usize;
        let mut found: Option<String> = None;
        for i in 0..count {
            let r = at + 6 + i * 12;
            if be16(d, r + 6)? != 6 {
                continue;
            }
            let (platform, len, off) = (be16(d, r)?, be16(d, r + 8)? as usize, be16(d, r + 10)? as usize);
            let raw = d.get(strings + off..strings + off + len)?;
            let s: String = if platform == 3 || platform == 0 {
                raw.chunks(2).filter_map(|c| (c.len() == 2).then(|| u16::from_be_bytes([c[0], c[1]]))).filter_map(|u| char::from_u32(u as u32)).collect()
            } else {
                raw.iter().map(|b| *b as char).collect()
            };
            let s: String = s.chars().filter(|c| c.is_ascii_graphic()).collect();
            if !s.is_empty() {
                found = Some(s);
                if platform == 3 {
                    break;
                }
            }
        }
        found
    }

    fn scale(&self, v: i16) -> i32 {
        (v as f32 * 1000.0 / self.units).round() as i32
    }
}

// ---- standard-14 metrics (Adobe core 14 AFM) -----------------------------------------------------

/// `(name, FontBBox, Ascent, Descent, CapHeight, ItalicAngle, StemV, Flags)`
const BASE14_METRICS: &[(&str, [i32; 4], i32, i32, i32, f32, i32, i32)] = &[
    ("Helvetica", [-166, -225, 1000, 931], 718, -207, 718, 0.0, 88, 32),
    ("Helvetica-Bold", [-170, -228, 1003, 962], 718, -207, 718, 0.0, 140, 32 | 262144),
    ("Helvetica-Oblique", [-170, -225, 1116, 931], 718, -207, 718, -12.0, 88, 32 | 64),
    ("Helvetica-BoldOblique", [-174, -228, 1114, 962], 718, -207, 718, -12.0, 140, 32 | 64 | 262144),
    ("Times-Roman", [-168, -218, 1000, 898], 683, -217, 662, 0.0, 84, 32 | 2),
    ("Times-Bold", [-168, -218, 1000, 935], 683, -217, 676, 0.0, 139, 32 | 2 | 262144),
    ("Times-Italic", [-169, -217, 1010, 883], 683, -217, 653, -15.5, 76, 32 | 2 | 64),
    ("Times-BoldItalic", [-200, -218, 996, 921], 683, -217, 669, -15.0, 121, 32 | 2 | 64 | 262144),
    ("Courier", [-23, -250, 715, 805], 629, -157, 562, 0.0, 51, 32 | 1),
    ("Courier-Bold", [-113, -250, 749, 801], 629, -157, 562, 0.0, 106, 32 | 1 | 262144),
    ("Courier-Oblique", [-27, -250, 849, 805], 629, -157, 562, -12.0, 51, 32 | 1 | 64),
    ("Courier-BoldOblique", [-57, -250, 869, 801], 629, -157, 562, -12.0, 106, 32 | 1 | 64 | 262144),
];

// ---- encodings -----------------------------------------------------------------------------------

fn glyph_unicode(n: &str) -> u32 {
    if let Some((_, u)) = NAMES.iter().find(|(g, _)| *g == n) {
        return *u;
    }
    if let Some(h) = n.strip_prefix("uni").filter(|h| h.len() >= 4) {
        if let Ok(u) = u32::from_str_radix(&h[..4], 16) {
            return u;
        }
    }
    0
}

/// Code → Unicode for a simple font as its Encoding says (Standard when it says nothing).
fn code_unicode(f: &PdfObject) -> Result<[u32; 256]> {
    let enc = f.get_dict("Encoding")?;
    let table_of = |n: &str| match n {
        "WinAnsiEncoding" => &WIN_ANSI,
        "MacRomanEncoding" => &MAC_ROMAN,
        _ => &STANDARD,
    };
    let mut out;
    match &enc {
        Some(e) if e.is_name()? => out = *table_of(&String::from_utf8_lossy(&e.as_name()?)),
        Some(e) if e.is_dict()? => {
            out = *table_of(&name(e, "BaseEncoding")?.unwrap_or_default());
            if let Some(d) = e.get_dict("Differences")?.filter(|d| d.is_array().unwrap_or(false)) {
                let mut code = 0usize;
                for x in d.array_iter()? {
                    let x = x?;
                    if x.is_number()? {
                        code = x.as_int()?.max(0) as usize;
                    } else if x.is_name()? {
                        if code < 256 {
                            out[code] = glyph_unicode(&String::from_utf8_lossy(&x.as_name()?));
                        }
                        code += 1;
                    }
                }
            }
        }
        _ => out = STANDARD,
    }
    Ok(out)
}

/// Widths (1/1000 em) of codes `first..=last` in `program`, by Unicode.
fn widths_for(program: &Font, codes: &[u32; 256]) -> Result<(i32, i32, Vec<i32>)> {
    let mut w = [0i32; 256];
    let (mut first, mut last) = (i32::MAX, -1);
    for c in 0..256usize {
        if codes[c] == 0 {
            continue;
        }
        let g = program.encode_character(codes[c] as i32).unwrap_or(0);
        if g > 0 {
            w[c] = (program.advance_glyph(g).unwrap_or(0.0) * 1000.0).round() as i32;
            first = first.min(c as i32);
            last = last.max(c as i32);
        }
    }
    if last < 0 {
        return Ok((32, 32, vec![0]));
    }
    Ok((first, last, w[first as usize..=last as usize].to_vec()))
}

fn put_widths(doc: &PdfDocument, f: &mut PdfObject, program: &Font) -> Result<()> {
    if f.get_dict("Widths")?.is_some() && f.get_dict("FirstChar")?.is_some() && f.get_dict("LastChar")?.is_some() {
        return Ok(());
    }
    let (first, last, w) = widths_for(program, &code_unicode(f)?)?;
    let mut arr = doc.new_array()?;
    for v in w {
        arr.array_push(PdfObject::new_int(v)?)?;
    }
    f.dict_put("FirstChar", PdfObject::new_int(first)?)?;
    f.dict_put("LastChar", PdfObject::new_int(last)?)?;
    f.dict_put("Widths", arr)?;
    Ok(())
}

// ---- embedding -----------------------------------------------------------------------------------

fn font_descriptor(doc: &mut PdfDocument, f: &mut PdfObject, font_name: &str, spec: &str) -> Result<PdfObject> {
    let mut d = match f.get_dict("FontDescriptor")?.filter(|d| d.is_dict().unwrap_or(false)) {
        Some(d) => d,
        None => {
            let fresh = doc.new_object_from_str(spec)?;
            let r = doc.add_object(&fresh)?;
            f.dict_put("FontDescriptor", r.clone())?;
            r
        }
    };
    d.dict_put("FontName", PdfObject::new_name(font_name)?)?;
    // Whatever the descriptor lacked comes from the spec.
    let spec_obj = doc.new_object_from_str(spec)?;
    for k in ["Type", "Flags", "FontBBox", "ItalicAngle", "Ascent", "Descent", "CapHeight", "StemV"] {
        if d.get_dict(k)?.is_none() {
            if let Some(v) = spec_obj.get_dict(k)? {
                d.dict_put(k, v)?;
            }
        }
    }
    // Stale links to other programs go.
    for k in ["FontFile", "FontFile2", "FontFile3"] {
        d.dict_delete(k)?;
    }
    Ok(d)
}

/// Embeds the font program of `plan` into the font dictionary `f` (a Type1 or TrueType simple font).
/// Returns a sentence for the log.
pub fn embed(doc: &mut PdfDocument, f: &PdfObject, plan: &FontPlan) -> Result<String> {
    let mut f = f.share();
    let shown = name(&f, "BaseFont")?.unwrap_or_default();
    match plan {
        FontPlan::Builtin { program } => {
            let data = Font::base14_program(program).ok_or_else(|| super::bad(format!("no built-in {program}")))?;
            let (_, bbox, asc, desc, cap, ia, stem, flags) = BASE14_METRICS.iter().find(|m| m.0 == *program).copied().unwrap_or(BASE14_METRICS[0]);
            let spec = format!(
                "<</Type/FontDescriptor/FontName/{program}/Flags {flags}/FontBBox[{} {} {} {}]/ItalicAngle {ia}/Ascent {asc}/Descent {desc}/CapHeight {cap}/StemV {stem}>>",
                bbox[0], bbox[1], bbox[2], bbox[3]
            );
            let mut d = font_descriptor(doc, &mut f, program, &spec)?;
            let mut sd = doc.new_dict()?;
            sd.dict_put("Subtype", PdfObject::new_name("Type1C")?)?;
            let file = doc.add_stream(&Buffer::from_bytes(&data)?, Some(&sd), false)?;
            d.dict_put("FontFile3", file)?;
            f.dict_put("BaseFont", PdfObject::new_name(program)?)?;
            f.dict_put("Subtype", PdfObject::new_name("Type1")?)?;
            let clone = Font::new(program)?;
            put_widths(doc, &mut f, &clone)?;
            Ok(format!("Embedded the built-in {program} for {shown}"))
        }
        FontPlan::Installed { path, ps_name } => {
            let data = std::fs::read(path).map_err(|e| super::bad(format!("can't read {}: {e}", path.display())))?;
            let info = TtInfo::parse(&data).ok_or_else(|| super::bad("not a TrueType font"))?;
            let mut flags = 32;
            if info.fixed {
                flags |= 1;
            }
            if info.italic_angle != 0.0 {
                flags |= 64;
            }
            let cap = info.cap_height.map(|c| info.scale(c)).unwrap_or_else(|| info.scale(info.ascent) * 7 / 10);
            let spec = format!(
                "<</Type/FontDescriptor/FontName/{ps_name}/Flags {flags}/FontBBox[{} {} {} {}]/ItalicAngle {}/Ascent {}/Descent {}/CapHeight {cap}/StemV 80>>",
                info.scale(info.bbox[0]),
                info.scale(info.bbox[1]),
                info.scale(info.bbox[2]),
                info.scale(info.bbox[3]),
                info.italic_angle,
                info.scale(info.ascent),
                info.scale(info.descent),
            );
            let mut d = font_descriptor(doc, &mut f, ps_name, &spec)?;
            let mut sd = doc.new_dict()?;
            sd.dict_put("Length1", PdfObject::new_int(data.len() as i32)?)?;
            let file = doc.add_stream(&Buffer::from_bytes(&data)?, Some(&sd), false)?;
            d.dict_put("FontFile2", file)?;
            f.dict_put("BaseFont", PdfObject::new_name(ps_name)?)?;
            f.dict_put("Subtype", PdfObject::new_name("TrueType")?)?;
            // A TrueType font needs an Encoding a reader can map by name: WinAnsi, with the codes
            // StandardEncoding would have drawn differently as Differences.
            if f.get_dict("Encoding")?.is_none() {
                let std_only = standard_differences();
                let enc = doc.new_object_from_str(&format!("<</Type/Encoding/BaseEncoding/WinAnsiEncoding/Differences[{std_only}]>>"))?;
                f.dict_put("Encoding", enc)?;
            }
            let program = Font::from_bytes_with_index(ps_name, 0, &data)?;
            put_widths(doc, &mut f, &program)?;
            Ok(format!("Embedded the installed {ps_name} for {shown}"))
        }
        FontPlan::Embedded | FontPlan::Blocked { .. } => Err(super::bad("nothing to embed")),
    }
}

/// `code /name code /name ...` for the codes where StandardEncoding and WinAnsi differ.
fn standard_differences() -> String {
    let mut out = String::new();
    let name_of = |u: u32| NAMES.iter().find(|(_, v)| *v == u).map(|(n, _)| *n);
    for c in 32..256usize {
        if STANDARD[c] != 0 && STANDARD[c] != WIN_ANSI[c] {
            if let Some(n) = name_of(STANDARD[c]) {
                out.push_str(&format!("{c} /{n} "));
            }
        }
    }
    out
}

/// The CIDToGIDMap fix: `/Identity`, which is what its absence already meant.
pub fn set_identity_cid_to_gid(doc: &mut PdfDocument, f: &PdfObject) -> Result<bool> {
    let _ = doc;
    let Some(mut d) = descendant(f)? else { return Ok(false) };
    let embedded = d.get_dict("FontDescriptor")?.is_some_and(|fd| fd.get_dict("FontFile2").ok().flatten().is_some());
    if name(&d, "Subtype")?.as_deref() == Some("CIDFontType2") && embedded && d.get_dict("CIDToGIDMap")?.is_none() {
        d.dict_put("CIDToGIDMap", PdfObject::new_name("Identity")?)?;
        return Ok(true);
    }
    let _ = xref_of(&d);
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_14_names_in_their_spellings() {
        assert_eq!(base14("Helvetica"), Some("Helvetica"));
        assert_eq!(base14("Helvetica-BoldOblique"), Some("Helvetica-BoldOblique"));
        assert_eq!(base14("Helvetica,Bold"), Some("Helvetica-Bold"));
        assert_eq!(base14("Times-Roman"), Some("Times-Roman"));
        assert_eq!(base14("Times-Italic"), Some("Times-Italic"));
        assert_eq!(base14("Courier"), Some("Courier"));
        assert_eq!(base14("Courier-Bold"), Some("Courier-Bold"));
        assert_eq!(base14("Arial"), None);
        assert_eq!(base14("TimesNewRoman"), None);
    }

    #[test]
    fn standard_differences_name_the_quote_marks() {
        let d = standard_differences();
        assert!(d.contains("39 /quoteright"), "{d}");
        assert!(d.contains("96 /quoteleft"), "{d}");
    }
}
