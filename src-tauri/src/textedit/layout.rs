// Adapted from pdfcraft (MIT OR Apache-2.0), crates/edit/src/text.rs @ b8ddf71
//! Setting text: the faces a rewrite writes in (the document's own fonts, standard 14 fonts and
//! the bundled faces, embedded as subsets), the glyph items a block is made of, wrapping them into
//! lines, and writing the lines as operators.
//!
//! A block is rewritten from runs. A run whose text did not change keeps its glyphs as they were
//! shown: its codes in its own font and the TJ adjustments (kerning) between them. A run with new
//! text is encoded in its font where that font has the glyphs; characters it lacks fall back, as a
//! run, to a standard 14 font (text WinAnsi can show) or to the bundled Noto faces (anything else,
//! CJK included), which are embedded with only the glyphs used. Words are wrapped into the block's
//! width (CJK text may break between any two characters); justified lines spread their free width
//! over their spaces with TJ adjustments, which works for one- and two-byte fonts alike.

use super::bundled::{self, Subset};
use super::content::{Obj, Op, Pieces, num};
use super::font::{Metrics, STD14};
use super::text::{Align, Fallback, Run, DECO_THICK, STRIKE_AT, UNDERLINE_AT};
use crate::engine::EngineError;
use mupdf::Font;
use mupdf::pdf::{PdfDocument, PdfObject};
use std::rc::Rc;

type Result<T> = std::result::Result<T, EngineError>;

fn invalid(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

fn get(o: &PdfObject, key: &str) -> Option<PdfObject> {
    o.get_dict(key).ok().flatten()
}

/// WinAnsiEncoding code of `c`.
pub(crate) fn win_ansi_byte(c: char) -> Option<u8> {
    match c {
        '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => u8::try_from(u32::from(c)).ok(),
        _ => {
            let high = &super::encodings::WIN_ANSI[0x80..0xa0];
            let i = high.iter().position(|&u| u != 0 && u == u32::from(c))?;
            u8::try_from(0x80 + i).ok()
        }
    }
}

/// Han ideographs, kana, Hangul and CJK punctuation: text that may break between any two
/// characters (Korean text between syllables too, as browsers do by default).
fn breaks_anywhere(c: char) -> bool {
    matches!(
        c as u32,
        0x1100..=0x11FF
            | 0x2E80..=0x2FFF
            | 0x3000..=0x30FF
            | 0x3130..=0x318F
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7FF
            | 0xF900..=0xFAFF
            | 0xFF00..=0xFF60
    )
}

/// The standard 14 family closest to `base`: 0 Courier, 1 Times, 2 Helvetica.
pub(crate) fn family_of(base: &str) -> usize {
    let n = base.to_ascii_lowercase();
    if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console", "cmtt"].iter().any(|s| n.contains(s)) {
        0
    } else if !n.contains("sans")
        && ["times", "serif", "roman", "mincho", "cambria", "georgia", "palatino", "garamond", "cmr", "minion", "book"]
            .iter()
            .any(|s| n.contains(s))
    {
        1
    } else {
        2
    }
}

/// A font name without subset tag and style words, lower case: `ABCDEF+Calibri-BoldItalic` → `calibri`.
pub(crate) fn family_key(base: &str) -> String {
    let name = match base.split_once('+') {
        Some((p, rest)) if p.len() == 6 && p.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => base,
    };
    let mut s: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    for w in ["bolditalic", "boldoblique", "semibold", "bold", "italic", "oblique", "regular", "roman", "medium", "black", "psmt", "mt"] {
        if s.len() > w.len() && s.ends_with(w) {
            s.truncate(s.len() - w.len());
        }
    }
    s
}

/// A face new text is written in.
pub(crate) enum Face {
    /// A font of the document, by its resource name on the page.
    Doc { res: Vec<u8>, m: Rc<Metrics> },
    /// A standard 14 font in WinAnsiEncoding (not embedded).
    Std { res: Vec<u8>, font: Font, name: &'static str },
    /// A bundled face, embedded at the end with the glyphs used.
    Bundled { idx: usize, res: Vec<u8>, subset: Subset, font: Font },
}

impl Face {
    fn res(&self) -> &[u8] {
        match self {
            Face::Doc { res, .. } | Face::Std { res, .. } | Face::Bundled { res, .. } => res,
        }
    }

    /// Whether it can show `c`.
    fn has(&self, c: char) -> bool {
        match self {
            Face::Doc { m, .. } => m.encode(&c.to_string()).is_some(),
            Face::Std { .. } => win_ansi_byte(c).is_some(),
            Face::Bundled { font, .. } => bundled::glyph(font, c).is_some(),
        }
    }

    fn name(&self) -> String {
        match self {
            Face::Doc { m, .. } => m.base_font.clone(),
            Face::Std { name, .. } => (*name).to_string(),
            Face::Bundled { idx, .. } => bundled::FACES[*idx].ps_name.to_string(),
        }
    }
}

/// How one item is drawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    pub face: usize,
    /// The `Tf` size (text space).
    pub size: f64,
    /// The fill colour operators.
    pub fill: Vec<Op>,
    /// Draw bold by stroking the outline too (a bundled face without a bold style).
    pub synth_bold: bool,
    /// `Ts` (text space): the rise of super and subscript, 0 on the line.
    pub rise: f64,
    /// Drawn after the text object, as filled rectangles (see [`Deco`]).
    pub underline: bool,
    pub strike: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    /// Codes in the face's encoding.
    Code(Vec<u8>),
    /// A bundled face's glyph id (its code is known once the subset is made).
    Gid(u16),
    /// A space: its code, or none (the font has no space glyph; the gap is a TJ adjustment).
    Space(Option<Vec<u8>>),
    /// A hard line break.
    Break,
}

/// One glyph, space or break to set.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub style: usize,
    pub kind: Kind,
    /// Advance in thousandths of the text-space unit at size 1.
    pub w: f64,
    /// TJ adjustment before it.
    pub kern: f64,
    pub text: String,
}

/// What a run is set from.
pub(crate) struct RunSpec<'a> {
    /// The original run (its glyphs are kept when `text` is None and the font is its own).
    pub source: Option<&'a Run>,
    pub text: Option<String>,
    /// The face to use first (the run's own font unless another was asked for).
    pub primary: usize,
    /// Whether that face is the source run's own font.
    pub own_font: bool,
    pub size: f64,
    pub fill: Vec<Op>,
    pub bold: bool,
    pub italic: bool,
    /// The family for fallbacks: 0 Courier/mono, 1 Times/serif, 2 Helvetica/sans.
    pub family: usize,
    pub rise: f64,
    pub underline: bool,
    pub strike: bool,
}

/// The faces and styles of one rewrite, and the fonts it adds to the page.
pub(crate) struct Setter<'a> {
    pub doc: &'a mut PdfDocument,
    page: PdfObject,
    pub faces: Vec<Face>,
    pub styles: Vec<Style>,
    pub fallbacks: Vec<Fallback>,
}

impl<'a> Setter<'a> {
    pub fn new(doc: &'a mut PdfDocument, page: PdfObject) -> Self {
        Setter { doc, page, faces: Vec::new(), styles: Vec::new(), fallbacks: Vec::new() }
    }

    /// The page's /Resources /Font dict, created if missing (shared resources are extended in
    /// place: a new key is unused by every other page). Looked up each time: a `PdfObject`
    /// clone is a deep copy, so a kept handle to a direct dict would not see later changes.
    fn font_dict(&mut self) -> Result<PdfObject> {
        let mut res = match self.page.get_dict_inheritable("Resources")? {
            Some(r) if r.is_dict()? => r,
            _ => {
                let r = self.doc.new_dict()?;
                self.page.clone().dict_put("Resources", r)?;
                get(&self.page, "Resources").ok_or_else(|| invalid("the page has no resources"))?
            }
        };
        match get(&res, "Font") {
            Some(f) if f.is_dict()? => Ok(f),
            _ => {
                res.dict_put("Font", self.doc.new_dict()?)?;
                get(&res, "Font").ok_or_else(|| invalid("the page has no fonts"))
            }
        }
    }

    fn free_name(&mut self, base: &str) -> Result<Vec<u8>> {
        let fonts = self.font_dict()?;
        let taken = |n: &str| get(&fonts, n).is_some();
        let mut i = 0;
        loop {
            let n = format!("{base}{i}");
            if !taken(&n) && !self.faces.iter().any(|f| f.res() == n.as_bytes()) {
                return Ok(n.into_bytes());
            }
            i += 1;
        }
    }

    /// The document font named `res` on the page.
    pub fn doc_face(&mut self, res: &[u8]) -> Result<usize> {
        if let Some(i) = self.faces.iter().position(|f| matches!(f, Face::Doc { res: r, .. } if r == res)) {
            return Ok(i);
        }
        let fonts = self.font_dict()?;
        let m = get(&fonts, &String::from_utf8_lossy(res))
            .filter(|d| d.is_dict().unwrap_or(false))
            .map(|d| Metrics::from_dict(&d))
            .ok_or_else(|| invalid(format!("the page has no font {}", String::from_utf8_lossy(res))))?;
        self.faces.push(Face::Doc { res: res.to_vec(), m: Rc::new(m) });
        Ok(self.faces.len() - 1)
    }

    /// Document font object `num` (from the font list), named on this page if it isn't yet.
    pub fn doc_object_face(&mut self, num: i32) -> Result<usize> {
        let fonts = self.font_dict()?;
        for i in 0..fonts.dict_len()? as i32 {
            let v = fonts.get_dict_val(i)?;
            if v.as_ref().is_some_and(|v| v.is_indirect().unwrap_or(false) && v.as_indirect().ok() == Some(num)) {
                let k = fonts.get_dict_key(i)?.and_then(|k| k.as_name().ok()).unwrap_or_default();
                return self.doc_face(&k);
            }
        }
        let obj = self.doc.new_indirect(num, 0)?;
        if !obj.resolve()?.is_some_and(|o| get(&o, "Type").and_then(|t| t.as_name().ok()).as_deref() == Some(b"Font")) {
            return Err(invalid(format!("object {num} is not a font")));
        }
        let name = self.free_name("IwDoc")?;
        self.font_dict()?.dict_put(String::from_utf8_lossy(&name).as_ref(), obj)?;
        self.doc_face(&name)
    }

    /// A standard 14 font, added to the page (WinAnsi, not embedded).
    pub fn std_face(&mut self, name: &'static str) -> Result<usize> {
        if let Some(i) = self.faces.iter().position(|f| matches!(f, Face::Std { name: n, .. } if *n == name)) {
            return Ok(i);
        }
        let font = Font::new(name)?;
        // One the page already has from an earlier edit.
        if let Some(res) = self.find_std(name)? {
            self.faces.push(Face::Std { res, font, name });
            return Ok(self.faces.len() - 1);
        }
        let res = self.free_name("IwEd")?;
        let mut f = self.doc.new_dict()?;
        f.dict_put("Type", PdfObject::new_name("Font")?)?;
        f.dict_put("Subtype", PdfObject::new_name("Type1")?)?;
        f.dict_put("BaseFont", PdfObject::new_name(name)?)?;
        f.dict_put("Encoding", PdfObject::new_name("WinAnsiEncoding")?)?;
        let r = self.doc.add_object(&f)?;
        self.font_dict()?.dict_put(String::from_utf8_lossy(&res).as_ref(), r)?;
        self.faces.push(Face::Std { res, font, name });
        Ok(self.faces.len() - 1)
    }

    /// The page's `IwEd<n>` entry for standard font `name`, if an earlier edit added one.
    fn find_std(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        let fonts = self.font_dict()?;
        let is = |o: &PdfObject, k: &str, v: &str| get(o, k).and_then(|n| n.as_name().ok()).as_deref() == Some(v.as_bytes());
        for i in 0..fonts.dict_len()? as i32 {
            let (Some(k), Some(v)) = (fonts.get_dict_key(i)?, fonts.get_dict_val(i)?) else { continue };
            let k = k.as_name()?;
            if k.starts_with(b"IwEd")
                && !self.faces.iter().any(|f| f.res() == k.as_slice())
                && is(&v, "Subtype", "Type1")
                && is(&v, "BaseFont", name)
                && is(&v, "Encoding", "WinAnsiEncoding")
            {
                return Ok(Some(k));
            }
        }
        Ok(None)
    }

    /// A bundled face (embedded by [`Setter::finish`]).
    pub fn bundled_face(&mut self, idx: usize) -> Result<usize> {
        if let Some(i) = self.faces.iter().position(|f| matches!(f, Face::Bundled { idx: x, .. } if *x == idx)) {
            return Ok(i);
        }
        let res = self.free_name("IwNoto")?;
        let font = bundled::load(idx)?;
        self.faces.push(Face::Bundled { idx, res, subset: Subset::default(), font });
        Ok(self.faces.len() - 1)
    }

    /// Bundled face `idx` if it has a glyph for any of `chars`.
    fn bundled_covering(&mut self, idx: usize, chars: &[char]) -> Result<Option<usize>> {
        if let Some(i) = self.faces.iter().position(|f| matches!(f, Face::Bundled { idx: x, .. } if *x == idx)) {
            return Ok(chars.iter().any(|c| self.faces[i].has(*c)).then_some(i));
        }
        let font = bundled::load(idx)?;
        if !chars.iter().any(|c| bundled::glyph(&font, *c).is_some()) {
            return Ok(None);
        }
        let res = self.free_name("IwNoto")?;
        self.faces.push(Face::Bundled { idx, res, subset: Subset::default(), font });
        Ok(Some(self.faces.len() - 1))
    }

    /// A face from a font picker id: `doc:<object>`, `res:<resource name>`, `std:<name>` or
    /// `bundled:<id>` (see `fonts` in mod.rs).
    pub fn face_by_id(&mut self, id: &str) -> Result<usize> {
        let bad = || invalid(format!("unknown font {id}"));
        match id.split_once(':') {
            Some(("doc", n)) => self.doc_object_face(n.parse().map_err(|_| bad())?),
            Some(("res", n)) => self.doc_face(n.as_bytes()),
            Some(("std", n)) => {
                let name = STD14.iter().flatten().copied().find(|s| *s == n).ok_or_else(bad)?;
                self.std_face(name)
            }
            Some(("bundled", n)) => {
                let i = bundled::by_id(n).ok_or_else(bad)?;
                self.bundled_face(i)
            }
            _ => Err(bad()),
        }
    }

    fn style(&mut self, st: Style) -> usize {
        if let Some(i) = self.styles.iter().position(|s| *s == st) {
            return i;
        }
        self.styles.push(st);
        self.styles.len() - 1
    }

    /// The face `c` is set in (`f`, or for a bundled face a twin of it), its item kind and its
    /// advance in thousandths; None if `f` has no glyph for it.
    fn glyph(&mut self, f: usize, c: char) -> Result<Option<(usize, Kind, f64)>> {
        let t = c.to_string();
        let (idx, g, w) = match &self.faces[f] {
            Face::Doc { m, .. } => {
                let Some(bytes) = m.encode(&t) else { return Ok(None) };
                let w = m.codes(&bytes).iter().map(|(code, _)| m.width(*code) * 1000.0).sum();
                return Ok(Some((f, Kind::Code(bytes), w)));
            }
            Face::Std { font, .. } => {
                let Some(b) = win_ansi_byte(c) else { return Ok(None) };
                let w =
                    font.encode_character(c as i32).ok().and_then(|g| font.advance_glyph(g).ok()).map_or(500.0, |a| f64::from(a) * 1000.0);
                return Ok(Some((f, Kind::Code(vec![b]), w)));
            }
            Face::Bundled { idx, font, .. } => {
                let Some(g) = bundled::glyph(font, c) else { return Ok(None) };
                (*idx, g, bundled::advance(font, g))
            }
        };
        // A glyph already standing for another character (U+301C and U+FF5E share one) goes to
        // another subset of the same face, so each code keeps its own ToUnicode text.
        let twin = (f..self.faces.len())
            .find(|&j| matches!(&self.faces[j], Face::Bundled { idx: x, subset, .. } if *x == idx && subset.fits(g, &t)));
        let j = match twin {
            Some(j) => j,
            None => {
                let res = self.free_name("IwNoto")?;
                self.faces.push(Face::Bundled { idx, res, subset: Subset::default(), font: bundled::load(idx)? });
                self.faces.len() - 1
            }
        };
        if let Face::Bundled { subset, .. } = &mut self.faces[j] {
            subset.use_glyph(g, &t);
        }
        Ok(Some((j, Kind::Gid(g), w)))
    }

    /// The space item of face `f` in style `style` (a TJ gap of a quarter em when it has no
    /// space glyph).
    fn space(&mut self, f: usize, style: Style) -> Result<Item> {
        let (face, kind, w) = match self.glyph(f, ' ')? {
            Some((j, Kind::Code(b), w)) => (j, Kind::Space(Some(b)), w),
            Some((j, Kind::Gid(g), w)) => (j, Kind::Space(Some(self.gid_code(j, g))), w),
            _ => (f, Kind::Space(None), 250.0),
        };
        let style = self.style(Style { face, ..style });
        Ok(Item { style, kind, w, kern: 0.0, text: " ".into() })
    }

    fn gid_code(&self, f: usize, g: u16) -> Vec<u8> {
        match &self.faces[f] {
            Face::Bundled { subset, .. } => subset.code(g),
            _ => Vec::new(),
        }
    }

    /// The items of one run.
    pub fn items(&mut self, spec: &RunSpec) -> Result<Vec<Item>> {
        let base = Style {
            face: spec.primary,
            size: spec.size,
            fill: spec.fill.clone(),
            synth_bold: false,
            rise: spec.rise,
            underline: spec.underline,
            strike: spec.strike,
        };
        let base_style = self.style(base.clone());
        let mut out = Vec::new();
        // Kept text: the source glyphs as they were, kerning included.
        if let (None, Some(src), true) = (&spec.text, spec.source, spec.own_font) {
            for g in &src.glyphs {
                // A gap read as a space stays the gap it was (its kerning holds the rest of it).
                let kind = if g.bytes.is_empty() {
                    Kind::Space(None)
                } else if g.space {
                    Kind::Space(Some(g.bytes.clone()))
                } else {
                    Kind::Code(g.bytes.clone())
                };
                out.push(Item { style: base_style, kind, w: g.w, kern: g.kern, text: g.text.clone() });
            }
            return Ok(out);
        }
        let text = spec.text.clone().unwrap_or_else(|| spec.source.map(|r| r.text.clone()).unwrap_or_default());
        // Characters the primary face lacks go to one fallback for the run: a standard 14 font
        // when WinAnsi can show them all, else the bundled faces.
        let missing: Vec<char> = text.chars().filter(|c| !matches!(c, ' ' | '\n') && !self.faces[spec.primary].has(*c)).collect();
        let fallback = if missing.is_empty() {
            None
        } else if missing.iter().all(|c| win_ansi_byte(*c).is_some()) && !matches!(self.faces[spec.primary], Face::Std { .. }) {
            Some(self.std_face(STD14[spec.family][spec.bold as usize + 2 * spec.italic as usize])?)
        } else {
            // The bundled Latin face, if it has any of them (CJK goes to the CJK face below).
            self.bundled_covering(bundled::latin(spec.family == 0, spec.bold, spec.italic), &missing)?
        };
        if let Some(fb) = fallback {
            let what = self.faces[spec.primary].name();
            let c = missing.iter().copied().find(|c| self.faces[fb].has(*c)).unwrap_or(missing[0]);
            let subset = matches!(&self.faces[spec.primary], Face::Doc { m, .. } if m.subset);
            let reason = match (spec.own_font, subset) {
                (true, true) => format!("the embedded subset of {what} has no glyph for '{c}'"),
                (true, false) => format!("{what} has no glyph for '{c}'"),
                (false, _) => format!("the chosen font {what} has no glyph for '{c}'"),
            };
            let embedded = matches!(self.faces[fb], Face::Bundled { .. });
            self.fallbacks.push(Fallback { font: self.faces[fb].name(), embedded, reason });
        }
        let mut cjk: Option<usize> = None;
        let mut last_face = spec.primary;
        for c in text.chars() {
            match c {
                '\n' => out.push(Item { style: base_style, kind: Kind::Break, w: 0.0, kern: 0.0, text: "\n".into() }),
                ' ' => {
                    // In the face of the text before it, else the run's own face.
                    let face = if self.faces[last_face].has(' ') { last_face } else { spec.primary };
                    out.push(self.space(face, base.clone())?);
                }
                _ => {
                    let mut got = self.glyph(spec.primary, c)?;
                    if got.is_none() {
                        if let Some(fb) = fallback {
                            got = self.glyph(fb, c)?;
                        }
                    }
                    if got.is_none() {
                        // The bundled CJK face (bold drawn by stroking: it has no bold style).
                        let f = match cjk {
                            Some(f) => f,
                            None => {
                                let f = self.bundled_face(bundled::cjk())?;
                                cjk = Some(f);
                                if !self.fallbacks.iter().any(|x| x.font == self.faces[f].name()) {
                                    let reason = format!("{} has no glyph for '{c}'", self.faces[spec.primary].name());
                                    self.fallbacks.push(Fallback { font: self.faces[f].name(), embedded: true, reason });
                                }
                                f
                            }
                        };
                        got = self.glyph(f, c)?;
                    }
                    let (face, kind, w) = got.ok_or_else(|| invalid(format!("no available font can show '{c}' (U+{:04X})", c as u32)))?;
                    let synth_bold = spec.bold && matches!(self.faces[face], Face::Bundled { idx, .. } if idx == bundled::cjk());
                    let st = self.style(Style { face, synth_bold, ..base.clone() });
                    last_face = face;
                    out.push(Item { style: st, kind, w, kern: 0.0, text: c.to_string() });
                }
            }
        }
        Ok(out)
    }

    /// Embeds the bundled subsets this rewrite used and names them on the page.
    pub fn finish(&mut self) -> Result<()> {
        let faces = std::mem::take(&mut self.faces);
        let r = (|| {
            for f in &faces {
                if let Face::Bundled { idx, res, subset, font } = f {
                    // A face nothing was set in (picked, but every character went elsewhere).
                    if subset.is_empty() {
                        continue;
                    }
                    let font = bundled::embed(self.doc, *idx, font, subset)?;
                    self.font_dict()?.dict_put(String::from_utf8_lossy(res).as_ref(), font)?;
                }
            }
            Ok(())
        })();
        self.faces = faces;
        r
    }
}

/// Removes the fonts edits added to page `page` (`IwEd`, `IwNoto`, `IwDoc` names) that its
/// content no longer sets, so a replaced subset goes with the next full save. The page's
/// resources and `/Font` dict are copied onto it first (shallowly), so pages that share them
/// keep theirs.
pub(crate) fn forget_unused_fonts(doc: &mut PdfDocument, page: u32) -> Result<()> {
    let page = &doc.find_page(page as i32)?;
    let ours = |n: &[u8]| [&b"IwEd"[..], b"IwNoto", b"IwDoc"].iter().any(|p| n.starts_with(p));
    let Some(fonts) = page.get_dict_inheritable("Resources")?.and_then(|r| get(&r, "Font")) else { return Ok(()) };
    let mut gone = Vec::new();
    for i in 0..fonts.dict_len()? as i32 {
        if let Some(k) = fonts.get_dict_key(i)? {
            let k = k.as_name()?;
            if ours(&k) {
                gone.push(k);
            }
        }
    }
    if gone.is_empty() {
        return Ok(());
    }
    let streams = super::text::page_streams(page)?;
    let ops = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>()).parse();
    let used: std::collections::HashSet<&[u8]> = ops.iter().filter(|o| o.is("Tf")).filter_map(|o| o.name(0)).collect();
    gone.retain(|n| !used.contains(n.as_slice()));
    if gone.is_empty() {
        return Ok(());
    }
    let mut page = page.clone();
    let res = match page.get_dict_inheritable("Resources")? {
        Some(r) if r.is_dict()? => r.copy_dict()?,
        _ => doc.new_dict()?,
    };
    page.dict_put("Resources", res)?;
    let mut res = get(&page, "Resources").ok_or_else(|| invalid("the page has no resources"))?;
    res.dict_put("Font", fonts.copy_dict()?)?;
    let mut fonts = get(&res, "Font").ok_or_else(|| invalid("the page has no fonts"))?;
    for n in gone {
        fonts.dict_delete(String::from_utf8_lossy(&n).as_ref())?;
    }
    Ok(())
}

/// A line of items and whether it ends a paragraph.
pub(crate) struct LineOut {
    pub items: Vec<Item>,
    pub ends_para: bool,
}

/// Measures items in text space (after horizontal scaling): `cs` is the character spacing.
pub(crate) struct Measure<'a> {
    pub styles: &'a [Style],
    pub cs: f64,
    pub scale: f64,
}

impl Measure<'_> {
    pub fn width(&self, it: &Item) -> f64 {
        let size = self.styles[it.style].size;
        let glyph = match it.kind {
            Kind::Break => return 0.0,
            Kind::Space(None) => it.w / 1000.0 * size,
            _ => it.w / 1000.0 * size + self.cs,
        };
        (glyph - it.kern / 1000.0 * size) * self.scale
    }

    pub fn line(&self, items: &[Item]) -> f64 {
        items.iter().map(|i| self.width(i)).sum()
    }
}

fn is_space(it: &Item) -> bool {
    matches!(it.kind, Kind::Space(_))
}

/// Wraps items into lines of at most `width` (text space). Breaks go at spaces (which end the
/// line and are dropped), between CJK characters, and at hard breaks.
pub(crate) fn wrap(items: Vec<Item>, width: f64, m: &Measure) -> Vec<LineOut> {
    let mut out = Vec::new();
    let mut paras: Vec<Vec<Item>> = vec![Vec::new()];
    for it in items {
        if matches!(it.kind, Kind::Break) {
            paras.push(Vec::new());
        } else {
            paras.last_mut().expect("one").push(it);
        }
    }
    for para in paras {
        // Words: (spaces before, word).
        let mut words: Vec<(Vec<Item>, Vec<Item>)> = Vec::new();
        let mut spaces = Vec::new();
        let mut word: Vec<Item> = Vec::new();
        for it in para {
            if is_space(&it) {
                if !word.is_empty() {
                    words.push((std::mem::take(&mut spaces), std::mem::take(&mut word)));
                }
                spaces.push(it);
            } else {
                let cjk = it.text.chars().next().is_some_and(breaks_anywhere);
                if cjk && !word.is_empty() {
                    words.push((std::mem::take(&mut spaces), std::mem::take(&mut word)));
                }
                word.push(it);
                if cjk {
                    words.push((std::mem::take(&mut spaces), std::mem::take(&mut word)));
                }
            }
        }
        if !word.is_empty() {
            words.push((spaces, word));
        }
        let mut line: Vec<Item> = Vec::new();
        let mut w = 0.0;
        for (sp, mut wd) in words {
            let sw = m.line(&sp);
            let ww = m.line(&wd);
            if !line.is_empty() && w + sw + ww > width {
                out.push(LineOut { items: std::mem::take(&mut line), ends_para: false });
                w = 0.0;
            }
            if line.is_empty() {
                // A line starts with its first glyph: no spaces, no kerning against what came before.
                if let Some(first) = wd.first_mut() {
                    first.kern = 0.0;
                }
                w = m.line(&wd);
                line.extend(wd);
            } else {
                w += sw + ww;
                line.extend(sp);
                line.extend(wd);
            }
        }
        out.push(LineOut { items: line, ends_para: true });
    }
    out
}

/// Stroke operators for a fill colour (for synthetic bold).
fn stroke_of(fill: &[Op]) -> Vec<Op> {
    fill.iter()
        .map(|f| match f.op.as_slice() {
            b"g" => Op::new("G", f.operands.clone()),
            b"rg" => Op::new("RG", f.operands.clone()),
            b"k" => Op::new("K", f.operands.clone()),
            b"cs" => Op::new("CS", f.operands.clone()),
            b"sc" => Op::new("SC", f.operands.clone()),
            _ => Op::new("SCN", f.operands.clone()),
        })
        .collect()
}

/// The lines of a block whose line ends are kept (Find & Replace), one per source line in `at`'s order: `groups`
/// are the items of each, split at the kept line ends. A line stands where the line it was is: at its start for
/// left and justified text, on its middle or its end for centred and right-aligned text, and each but the last of
/// a justified block is stretched to `right`. A line that no longer fits between `left` and `right` is an error:
/// nothing is moved to make room. `spans` are the (start, end, baseline) of the block's lines in the first line's
/// text space.
pub(crate) fn keep_lines(
    groups: Vec<(usize, Vec<Item>)>,
    spans: &[(f64, f64, f64)],
    align: Align,
    (left, right): (f64, f64),
    tol: f64,
    m: &Measure,
) -> Option<(Vec<LineOut>, Vec<LineAt>)> {
    let mut lines = Vec::new();
    let mut at = Vec::new();
    let last = groups.len().saturating_sub(1);
    for (g, (src, mut items)) in groups.into_iter().enumerate() {
        // A line starts with its first glyph: no spaces, no kerning against what came before.
        while items.first().is_some_and(is_space) {
            items.remove(0);
        }
        while items.last().is_some_and(is_space) {
            items.pop();
        }
        let (x0, x1, y) = spans[src];
        let used = m.line(&items);
        let x = match align {
            Align::Center => (x0 + x1) / 2.0 - used / 2.0,
            Align::Right => x1 - used,
            _ => x0,
        };
        if x + used > right + tol || x < left - tol {
            return None;
        }
        if let Some(first) = items.first_mut() {
            first.kern = 0.0;
        }
        at.push(LineAt { x: x - left, down: spans[0].2 - y, width: right - x });
        lines.push(LineOut { items, ends_para: g == last });
    }
    Some((lines, at))
}

/// How the lines are placed.
pub(crate) struct Placement {
    pub align: Align,
    /// The line width to align within (text space).
    pub width: f64,
    /// Distance between baselines for the first style's size (text space); a line with larger
    /// text is set further down in proportion.
    pub lead: f64,
    pub first_size: f64,
    /// Line width for synthetic bold, in the current user space.
    pub stroke: f64,
    /// The render mode to return to after synthetic bold.
    pub render_mode: i64,
    /// Where each line goes, when the lines are not stacked by `lead` (Find & Replace keeps each line where it
    /// was): one entry per line.
    pub at: Vec<LineAt>,
}

/// Where one line stands: its start along the block's first line, how far below that line's baseline it is, and
/// the width a justified line is stretched to (all in text space).
pub(crate) struct LineAt {
    pub x: f64,
    pub down: f64,
    pub width: f64,
}

/// An underline or strikeout to draw: a filled rectangle in the text matrix's space (`x`, `y` its
/// bottom left), in the colour of the run it belongs to.
pub(crate) struct Deco {
    pub strike: bool,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub fill: Vec<Op>,
}

/// A decoration being built along a line: the style it runs over, and where it began and has got to.
struct Stretch {
    from: f64,
    to: f64,
    size: f64,
    rise: f64,
    fill: Vec<Op>,
}

/// The operators that show `lines`, after a `Tm` at the first line's start: `Td` between lines,
/// `Tf`, `Ts` and colour where the style changes, and one TJ per stretch of one style; and the
/// decorations of the underlined and struck-out stretches, one per line and style, measured on the
/// same positions the text takes (alignment, justification and kerning included).
pub(crate) fn emit(setter: &Setter, lines: &[LineOut], m: &Measure, p: &Placement) -> (Vec<Op>, Vec<Deco>) {
    let mut ops = Vec::new();
    let mut decos: Vec<Deco> = Vec::new();
    let mut cur_font: Option<(usize, f64)> = None;
    let mut cur_fill: Option<Vec<Op>> = None;
    let mut cur_rise = 0.0;
    let mut bold_on = false;
    let mut x = 0.0;
    // The baseline of the line being set, below the first one.
    let mut base = 0.0;
    let mut prev_biggest = p.first_size;
    for (li, line) in lines.iter().enumerate() {
        let items = &line.items;
        let used = m.line(items);
        let at = p.at.get(li);
        let free = (at.map_or(p.width, |a| a.width) - used).max(0.0);
        let dx = match (at, p.align) {
            (Some(a), _) => a.x,
            (None, Align::Center) => free / 2.0,
            (None, Align::Right) => free,
            _ => 0.0,
        };
        let biggest = items.iter().map(|i| setter.styles[i.style].size).fold(p.first_size, f64::max);
        // A line is set lower for its own largest text, and for half of the line above's (whose
        // descenders reach down), so smaller text under large text doesn't run into it.
        let down = match at {
            Some(a) => a.down + base,
            None if li == 0 => 0.0,
            None => p.lead * biggest.max((biggest + prev_biggest) / 2.0) / p.first_size.max(1e-9),
        };
        prev_biggest = biggest;
        base -= down;
        if li > 0 || dx != 0.0 {
            ops.push(Op::new("Td", vec![num(dx - x), num(-down)]));
        }
        x = dx;
        let spaces = items.iter().filter(|i| is_space(i)).count();
        let extra = if p.align == Align::Justify && !line.ends_para && spaces > 0 { free / spaces as f64 } else { 0.0 };
        let mut arr: Vec<Obj> = Vec::new();
        let mut text: Vec<u8> = Vec::new();
        let flush = |arr: &mut Vec<Obj>, text: &mut Vec<u8>| {
            if !text.is_empty() {
                arr.push(Obj::Str(std::mem::take(text)));
            }
        };
        let mut cur_style: Option<usize> = None;
        // Underline and strikeout stretches open on this line, and the position along it.
        let mut open: [Option<Stretch>; 2] = [None, None];
        let mut pos = dx;
        let close = |open: &mut Option<Stretch>, strike: bool, decos: &mut Vec<Deco>| {
            if let Some(s) = open.take().filter(|s| s.to > s.from) {
                let at = if strike { STRIKE_AT } else { UNDERLINE_AT };
                let h = s.size * DECO_THICK;
                decos.push(Deco { strike, x: s.from, y: base + s.rise + s.size * at - h / 2.0, w: s.to - s.from, h, fill: s.fill });
            }
        };
        for it in items {
            let st = &setter.styles[it.style];
            if cur_style != Some(it.style) {
                flush(&mut arr, &mut text);
                if !arr.is_empty() {
                    ops.push(Op::new("TJ", vec![Obj::Array(std::mem::take(&mut arr))]));
                }
                if cur_font != Some((st.face, st.size)) {
                    ops.push(Op::new("Tf", vec![Obj::Name(setter.faces[st.face].res().to_vec()), num(st.size)]));
                    cur_font = Some((st.face, st.size));
                }
                if cur_fill.as_ref() != Some(&st.fill) {
                    ops.extend(st.fill.iter().cloned());
                    cur_fill = Some(st.fill.clone());
                }
                if st.rise != cur_rise {
                    ops.push(Op::new("Ts", vec![num(st.rise)]));
                    cur_rise = st.rise;
                }
                if st.synth_bold != bold_on {
                    if st.synth_bold {
                        ops.push(Op::new("Tr", vec![num(2.0)]));
                        ops.push(Op::new("w", vec![num(p.stroke * st.size)]));
                        ops.extend(stroke_of(&st.fill));
                    } else {
                        ops.push(Op::new("Tr", vec![num(p.render_mode as f64)]));
                    }
                    bold_on = st.synth_bold;
                }
                cur_style = Some(it.style);
            }
            if it.kern != 0.0 {
                flush(&mut arr, &mut text);
                arr.push(num(it.kern));
            }
            match &it.kind {
                Kind::Code(b) => text.extend_from_slice(b),
                Kind::Gid(g) => text.extend(setter.gid_code(st.face, *g)),
                Kind::Space(Some(b)) => text.extend_from_slice(b),
                Kind::Space(None) => {
                    flush(&mut arr, &mut text);
                    arr.push(num(-it.w));
                }
                Kind::Break => {}
            }
            if is_space(it) && extra != 0.0 {
                flush(&mut arr, &mut text);
                arr.push(num(-extra * 1000.0 / (st.size * m.scale)));
            }
            // Its glyphs start after the adjustment before it and end where the next item starts.
            let from = pos - it.kern / 1000.0 * st.size * m.scale;
            pos += m.width(it) + if is_space(it) { extra } else { 0.0 };
            for (n, (on, strike)) in [(st.underline, false), (st.strike, true)].into_iter().enumerate() {
                let same = open[n].as_ref().is_some_and(|s| s.size == st.size && s.rise == st.rise && s.fill == st.fill);
                if !(on && same) {
                    close(&mut open[n], strike, &mut decos);
                }
                if on {
                    match open[n].as_mut() {
                        Some(s) => s.to = pos,
                        None => open[n] = Some(Stretch { from, to: pos, size: st.size, rise: st.rise, fill: st.fill.clone() }),
                    }
                }
            }
        }
        close(&mut open[0], false, &mut decos);
        close(&mut open[1], true, &mut decos);
        flush(&mut arr, &mut text);
        if !arr.is_empty() {
            ops.push(Op::new("TJ", vec![Obj::Array(arr)]));
        }
    }
    if bold_on {
        ops.push(Op::new("Tr", vec![num(p.render_mode as f64)]));
    }
    (ops, decos)
}

/// The face a run starts in when no font was picked: the run's own, or with another style a font
/// of the same family on the page, else the standard 14 face of the family and style.
pub(crate) fn restyled_face(setter: &mut Setter, run: &Run, bold: bool, italic: bool) -> Result<(usize, bool)> {
    if bold == run.bold && italic == run.italic {
        return Ok((setter.doc_face(&run.font)?, true));
    }
    let key = family_key(&run.base_font);
    let fonts = setter.font_dict()?;
    let mut found = None;
    for i in 0..fonts.dict_len()? as i32 {
        let (Some(k), Some(v)) = (fonts.get_dict_key(i)?, fonts.get_dict_val(i)?) else { continue };
        let Some(v) = v.resolve()? else { continue };
        if !v.is_dict()? {
            continue;
        }
        let m = Metrics::from_dict(&v);
        if m.limit.is_none() && m.has_to_unicode && family_key(&m.base_font) == key && m.bold == bold && m.italic == italic {
            found = Some(k.as_name()?);
            break;
        }
    }
    if let Some(res) = found {
        return Ok((setter.doc_face(&res)?, false));
    }
    let std = STD14[family_of(&run.base_font)][bold as usize + 2 * italic as usize];
    let f = setter.std_face(std)?;
    setter.fallbacks.push(Fallback {
        font: std.into(),
        embedded: false,
        reason: format!("{} has no {} style in this document", run.base_font, style_name(bold, italic)),
    });
    Ok((f, false))
}

pub(crate) fn style_name(bold: bool, italic: bool) -> &'static str {
    match (bold, italic) {
        (true, true) => "bold italic",
        (true, false) => "bold",
        (false, true) => "italic",
        (false, false) => "regular",
    }
}
