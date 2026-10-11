//! The fonts of a document: what is embedded and how much it weighs ("Audit space usage" > Fonts), and
//! the unembed pass (Acrobat's Optimized PDF > Fonts > Unembed).
//!
//! Only simple, non-symbolic Type 1, MMType1 and TrueType fonts can lose their file: a viewer picks a
//! stand-in from the name and the descriptor flags, and the single-byte encoding still decides which
//! glyph each code is. A composite (Type 0) font addresses glyphs by index in the embedded file, so
//! without the file the text would come out as other characters. A symbolic font has no encoding a
//! stand-in could follow.

use std::collections::{BTreeMap, HashSet};

use mupdf::pdf::{PdfDocument, PdfObject};
use serde::Serialize;

use super::Error;

const FILES: [&str; 3] = ["FontFile", "FontFile2", "FontFile3"];
/// FontDescriptor /Flags: bit 3 (value 4) is "symbolic".
const SYMBOLIC: i32 = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontUsage {
    /// `/BaseFont` as written, subset tag included ("ABCDEF+Arial").
    pub name: String,
    /// "Type1", "TrueType", "Type0" ...
    pub kind: String,
    pub embedded: bool,
    /// Embedded as a subset (the name carries the six-letter tag).
    pub subset: bool,
    /// Stored size of the font file(s).
    pub bytes: u64,
    /// The unembed pass is allowed to take this font's file out.
    pub can_unembed: bool,
    /// Why not, in words for the UI (set when embedded and `can_unembed` is false).
    pub why_not: Option<String>,
}

fn name_of(o: &PdfObject, key: &str) -> Option<String> {
    let v = o.get_dict(key).ok()??;
    String::from_utf8(v.as_name().ok()?).ok()
}

/// "ABCDEF+Arial" -> "Arial"; names without a tag come back as they are.
fn untagged(name: &str) -> &str {
    let b = name.as_bytes();
    if b.len() > 7 && b[6] == b'+' && b[..6].iter().all(u8::is_ascii_uppercase) { &name[7..] } else { name }
}

fn file_len(f: &PdfObject) -> u64 {
    match f.get_dict("Length").ok().flatten().and_then(|l| l.as_int().ok()) {
        Some(l) if l > 0 => l as u64,
        _ => f.read_raw_stream().map_or(0, |b| b.len() as u64),
    }
}

/// The descriptor a font dictionary uses (a composite font keeps it on its descendant).
fn descriptor(font: &PdfObject) -> Option<PdfObject> {
    let direct = font.get_dict("FontDescriptor").ok().flatten();
    if direct.is_some() {
        return direct;
    }
    let kids = font.get_dict("DescendantFonts").ok().flatten()?;
    let first = if kids.is_array().ok()? { kids.get_array(0).ok().flatten()? } else { kids };
    first.get_dict("FontDescriptor").ok().flatten()
}

struct Found {
    font: PdfObject,
    desc: Option<PdfObject>,
    kind: String,
    name: String,
}

fn fonts_of(doc: &PdfDocument) -> Result<Vec<Found>, Error> {
    let mut out = Vec::new();
    for x in 1..doc.xref_len()? {
        let Ok(o) = doc.new_indirect(x as i32, 0) else { continue };
        let Ok(Some(r)) = o.resolve() else { continue };
        if !r.is_dict().unwrap_or(false) || r.is_stream().unwrap_or(false) {
            continue;
        }
        let Some(kind) = name_of(&o, "Subtype") else { continue };
        let is_font = matches!(kind.as_str(), "Type1" | "MMType1" | "TrueType" | "Type0" | "Type3");
        // `Subtype /Type1` also names a few non-font dictionaries: a base font is what makes it one.
        let Some(name) = name_of(&o, "BaseFont") else {
            if is_font && kind == "Type3" {
                out.push(Found { desc: None, name: format!("Type 3 font {x}"), kind, font: o });
            }
            continue;
        };
        if is_font {
            out.push(Found { desc: descriptor(&o), kind, name, font: o });
        }
    }
    Ok(out)
}

/// One row per font name, with the stored size of its file(s); a descriptor shared by several font
/// dictionaries counts once. Largest first.
pub fn list(doc: &PdfDocument) -> Result<Vec<FontUsage>, Error> {
    let mut rows: BTreeMap<String, FontUsage> = BTreeMap::new();
    let mut counted: HashSet<i32> = HashSet::new();
    for f in fonts_of(doc)? {
        let mut file_bytes = 0u64;
        let mut embedded = false;
        let mut flags = None;
        if let Some(d) = &f.desc {
            flags = d.get_dict("Flags").ok().flatten().and_then(|v| v.as_int().ok());
            for key in FILES {
                if let Ok(Some(file)) = d.get_dict(key) {
                    embedded = true;
                    let id = file.as_indirect().ok().filter(|&n| n > 0);
                    if id.is_none_or(|n| counted.insert(n)) {
                        file_bytes += file_len(&file);
                    }
                }
            }
        }
        let simple = matches!(f.kind.as_str(), "Type1" | "MMType1" | "TrueType");
        let (can_unembed, why_not) = if !embedded {
            (false, None)
        } else if !simple {
            (false, Some("A composite font addresses glyphs by position in its file".to_string()))
        } else if flags.is_none_or(|fl| fl & SYMBOLIC != 0) {
            (false, Some("A symbolic font has no encoding a replacement could follow".to_string()))
        } else {
            (true, None)
        };
        let row = rows.entry(f.name.clone()).or_insert_with(|| FontUsage {
            subset: untagged(&f.name).len() != f.name.len(),
            name: f.name.clone(),
            kind: f.kind.clone(),
            embedded: false,
            bytes: 0,
            can_unembed: true,
            why_not: None,
        });
        row.embedded |= embedded;
        row.bytes += file_bytes;
        // Several dictionaries under one name: all of them must allow it.
        if embedded {
            if !can_unembed {
                row.can_unembed = false;
                row.why_not = row.why_not.take().or(why_not);
            }
        }
    }
    let mut v: Vec<FontUsage> = rows.into_values().map(|mut r| {
        if !r.embedded {
            r.can_unembed = false;
        }
        r
    }).collect();
    v.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));
    Ok(v)
}

/// What an unembed run did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unembedded {
    /// Font files taken out.
    pub files: u32,
    /// Stored size of those files.
    pub bytes: u64,
    /// Requested names that were skipped, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Drops the embedded file of every listed font that may lose it, and the subset tag from its name
/// (a tag is only for embedded subsets). The orphaned streams go with the writer's garbage pass.
pub fn unembed(doc: &mut PdfDocument, names: &[String], all: bool) -> Result<Unembedded, Error> {
    let mut done = Unembedded::default();
    let allowed: HashSet<String> = list(doc)?.into_iter().filter(|u| u.can_unembed).map(|u| u.name).collect();
    let mut want: HashSet<&str> = names.iter().map(String::as_str).collect();
    if all {
        want.extend(allowed.iter().map(String::as_str));
    }
    if want.is_empty() {
        return Ok(done);
    }
    // "Every font" skips what cannot be, silently: only the names that were asked for are reported.
    for n in names {
        if !allowed.contains(n) {
            let listed = list(doc)?.into_iter().find(|u| &u.name == n);
            let why = match listed {
                None => "No such font".to_string(),
                Some(u) if !u.embedded => "Not embedded".to_string(),
                Some(u) => u.why_not.unwrap_or_else(|| "Cannot be unembedded".to_string()),
            };
            done.skipped.push((n.clone(), why));
        }
    }
    let mut seen: HashSet<i32> = HashSet::new();
    for f in fonts_of(doc)? {
        if !want.contains(f.name.as_str()) || !allowed.contains(&f.name) {
            continue;
        }
        let mut font = f.font;
        if let Some(mut d) = f.desc {
            let first_time = d.as_indirect().ok().filter(|&n| n > 0).is_none_or(|n| seen.insert(n));
            if first_time {
                for key in FILES {
                    if let Ok(Some(file)) = d.get_dict(key) {
                        done.bytes += file_len(&file);
                        d.dict_delete(key)?;
                        done.files += 1;
                    }
                }
                if let Some(n) = name_of(&d, "FontName") {
                    if untagged(&n).len() != n.len() {
                        d.dict_put("FontName", PdfObject::new_name(untagged(&n))?)?;
                    }
                }
            }
        }
        if untagged(&f.name).len() != f.name.len() {
            font.dict_put("BaseFont", PdfObject::new_name(untagged(&f.name))?)?;
        }
    }
    Ok(done)
}
