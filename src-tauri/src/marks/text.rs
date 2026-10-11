//! Text of marks: the standard 14 faces in WinAnsiEncoding, their widths, and string literals.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/fonts/src/lib.rs @ b8ddf71 (`win_ansi_byte`,
//! `win_ansi`, `literal`, `first_non_win_ansi`).

use super::{Family, Style};
use mupdf::Font;
use std::collections::HashMap;

/// The base font and the resource name (`/IWHelvBI`…) of a style.
pub(super) fn face(s: &Style) -> (&'static str, String) {
    let k = usize::from(s.bold) + 2 * usize::from(s.italic);
    let (base, short) = match s.family {
        Family::Helvetica => (["Helvetica", "Helvetica-Bold", "Helvetica-Oblique", "Helvetica-BoldOblique"], "H"),
        Family::Times => (["Times-Roman", "Times-Bold", "Times-Italic", "Times-BoldItalic"], "T"),
        Family::Courier => (["Courier", "Courier-Bold", "Courier-Oblique", "Courier-BoldOblique"], "C"),
    };
    let var = ["", "B", "I", "BI"][k];
    (base[k], format!("IW{short}{var}"))
}

/// WinAnsiEncoding code of `c` (ISO 32000-2 Annex D).
pub(super) fn win_ansi_byte(c: char) -> Option<u8> {
    const HIGH: [(u32, u8); 27] = [
        (0x20AC, 0x80), (0x201A, 0x82), (0x0192, 0x83), (0x201E, 0x84), (0x2026, 0x85), (0x2020, 0x86), (0x2021, 0x87),
        (0x02C6, 0x88), (0x2030, 0x89), (0x0160, 0x8A), (0x2039, 0x8B), (0x0152, 0x8C), (0x017D, 0x8E), (0x2018, 0x91),
        (0x2019, 0x92), (0x201C, 0x93), (0x201D, 0x94), (0x2022, 0x95), (0x2013, 0x96), (0x2014, 0x97), (0x02DC, 0x98),
        (0x2122, 0x99), (0x0161, 0x9A), (0x203A, 0x9B), (0x0153, 0x9C), (0x017E, 0x9E), (0x0178, 0x9F),
    ];
    match c {
        '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => u8::try_from(u32::from(c)).ok(),
        '\t' => Some(b' '),
        _ => HIGH.iter().find(|(u, _)| *u == u32::from(c)).map(|(_, b)| *b),
    }
}

/// The first character the standard fonts cannot draw (it would become a `?`).
pub(super) fn first_undrawable<'a>(lines: impl IntoIterator<Item = &'a str>) -> Option<char> {
    lines.into_iter().flat_map(str::chars).find(|c| win_ansi_byte(*c).is_none())
}

pub(super) fn undrawable_message(c: char) -> String {
    format!("the standard fonts can't draw \"{c}\" (U+{:04X}); only Western European characters can be used in a mark", u32::from(c))
}

/// The text as a PDF literal string, delimiters escaped.
pub(super) fn literal(text: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for c in text.chars() {
        let b = win_ansi_byte(c).unwrap_or(b'?');
        match b {
            b'(' | b')' | b'\\' => out.extend_from_slice(&[b'\\', b]),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\n' => out.extend_from_slice(b"\\n"),
            _ => out.push(b),
        }
    }
    out.push(b')');
    out
}

/// Widths in the real metrics of the base faces, which MuPDF carries.
#[derive(Default)]
pub(super) struct Measure {
    fonts: HashMap<&'static str, Option<Font>>,
}

impl Measure {
    /// The advance of `text` in points.
    pub(super) fn width(&mut self, s: &Style, text: &str) -> f64 {
        let (base, _) = face(s);
        let font = self.fonts.entry(base).or_insert_with(|| Font::new(base).ok());
        let mono = s.family == Family::Courier;
        let em: f64 = text
            .chars()
            .map(|c| {
                if mono {
                    return 0.6;
                }
                font.as_ref()
                    .and_then(|f| f.encode_character(c as i32).ok().filter(|g| *g > 0 || c == ' ').and_then(|g| f.advance_glyph(g).ok()))
                    .map_or(0.55, f64::from)
            })
            .sum();
        em * s.size
    }
}
