//! Rich Text Format writer: writes the flowing page model (`flow.rs`) the way the Word writer does, as
//! paragraphs (Heading 1/2 or body), tables, inline PNG pictures and page breaks.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/export/src/lib.rs (`rtf_text`, `rtf`) @ b8ddf71.
//! Differences: a page setup, a style sheet plus direct formatting for headings, borders and cell edges
//! measured from the left margin, pictures, and the signed 16-bit rule applied to every UTF-16 unit.

use std::fmt::Write;

use super::flow::{allowed, col_twips, items, rgb8, Image, Item, Page, Table};

/// The most PNG bytes embedded in one file. A picture is written as hex (two characters a byte), and
/// readers such as WordPad slow down badly past a few tens of MB; the rest are skipped and counted.
pub const MAX_IMAGE_BYTES: usize = 32 << 20;

/// The document and how many pictures it left out (over [`MAX_IMAGE_BYTES`], or not a readable PNG).
pub struct Rtf {
    pub text: String,
    pub skipped_images: usize,
}

/// Text as RTF: `\`, `{` and `}` escaped, ASCII kept, everything else `\uN?` per UTF-16 unit (N is the
/// signed 16-bit value, `?` the one-character fallback `\uc1` skips). Tab and line feed become `\tab`
/// and `\line`; other control characters are dropped.
fn text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| allowed(*c)) {
        match c {
            '\\' | '{' | '}' => {
                o.push('\\');
                o.push(c);
            }
            '\t' => o.push_str("\\tab "),
            '\n' => o.push_str("\\line "),
            '\r' => {}
            c if (c as u32) < 128 => o.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    let _ = write!(o, "\\u{}?", *u as i16);
                }
            }
        }
    }
    o
}

/// Half-points for `\fs`, clamped like the Word writer's `w:sz`.
fn fs(size: f64) -> i64 {
    if size.is_finite() { (size * 2.0).round().clamp(2.0, 3276.0) as i64 } else { 24 }
}

/// `(width, height)` in pixels of a PNG, from its IHDR.
fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    let be = |at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let (w, h) = (be(16), be(20));
    (w > 0 && h > 0).then_some((w, h))
}

/// `brk` is `\pagebb` when the picture starts a page.
fn picture(im: &Image, text_w: f64, brk: &str, out: &mut String) -> bool {
    let Some((pw, ph)) = (im.ext == "png").then(|| png_size(&im.bytes)).flatten() else { return false };
    // Size on the page in twips, at most the text width, like the Word writer.
    let (w, h) = ((im.rect[2] - im.rect[0]).max(1.0), (im.rect[3] - im.rect[1]).max(1.0));
    let k = (text_w / w).min(1.0);
    let goal = |pt: f64| (pt * k * 20.0).round().clamp(1.0, 31680.0) as i64;
    let _ = writeln!(out, "{{\\pard\\plain{brk}\\sa120{{\\pict\\pngblip\\picw{pw}\\pich{ph}\\picwgoal{}\\pichgoal{}", goal(w), goal(h));
    for line in im.bytes.chunks(48) {
        for b in line {
            let _ = write!(out, "{b:02x}");
        }
        out.push('\n');
    }
    out.push_str("}\\par}\n");
    true
}

/// One run's character formatting: `\fsN`, bold, italic, colour (`cf` is the colour table entry).
fn char_fmt(size: f64, bold: bool, italic: bool, cf: &str) -> String {
    format!("\\fs{}{}{}{cf}", fs(size), if bold { "\\b" } else { "" }, if italic { "\\i" } else { "" })
}

/// A table: a row per detected row, single grey borders on every cell, cell edges from the grid widths
/// (`flow::col_twips`, measured from the left margin), the same padding of short rows as the Word writer.
fn table(t: &Table, text_w: f64, cf: &dyn Fn([f64; 3]) -> String, out: &mut String) {
    // Right edge of grid column i, in twips from the left margin.
    let right: Vec<i64> = col_twips(t, text_w)
        .into_iter()
        .scan(0i64, |x, w| {
            *x += w;
            Some(*x)
        })
        .collect();
    let border = |side: &str| format!("\\clbrdr{side}\\brdrs\\brdrw10\\brdrcf1");
    for (i, row) in t.rows.iter().enumerate() {
        // The header row repeats on the next page (`\\trhdr`).
        out.push_str(if i == 0 && t.has_header() { "\\trowd\\trhdr\\trgaph108\\trleft0" } else { "\\trowd\\trgaph108\\trleft0" });
        let mut column = 0;
        for c in row {
            column += c.span.max(1);
            let Some(x) = right.get(column.min(right.len()).saturating_sub(1)) else { continue };
            let _ = write!(out, "{}{}{}{}\\cellx{x}", border("t"), border("l"), border("b"), border("r"));
        }
        // Pad the grid so every row covers all columns.
        let used: usize = row.iter().map(|c| c.span.max(1)).sum();
        for x in right.iter().skip(used) {
            let _ = write!(out, "{}{}{}{}\\cellx{x}", border("t"), border("l"), border("b"), border("r"));
        }
        out.push('\n');
        for c in row {
            let _ = write!(out, "\\pard\\plain\\intbl{} {}\\cell", char_fmt(c.size, c.bold, c.italic, &cf(c.color)), text(&c.text));
        }
        for _ in used..right.len() {
            out.push_str("\\pard\\plain\\intbl\\cell");
        }
        out.push_str("\\row\n");
    }
}

/// A Rich Text document. Heading 1/2 paragraphs carry both the style (`\s1`, `\s2`, so outline views and
/// Word's navigation pane see them) and the same formatting directly (readers need not honour the style sheet).
pub fn rtf(pages: &[Page], title: &str) -> Rtf {
    let items = items(pages);
    // Colour table: entry 0 is the default (auto), 1 the table border grey, then each other colour once.
    let mut colors: Vec<[u8; 3]> = vec![[153, 153, 153]];
    let mut note = |c: [f64; 3]| {
        let c = rgb8(c);
        if c != [0, 0, 0] && !colors.contains(&c) {
            colors.push(c);
        }
    };
    for it in &items {
        match it {
            Item::Para(b, _) => note(b.color),
            Item::Table(t) => t.rows.iter().flatten().for_each(|c| note(c.color)),
            Item::Img(_) | Item::PageBreak => {}
        }
    }
    let cf = |c: [f64; 3]| {
        let c = rgb8(c);
        colors.iter().position(|e| *e == c).map_or(String::new(), |i| format!("\\cf{}", i + 1))
    };
    // Page size and margins as the Word writer takes them from the first page.
    let twips = |pt: f64| if pt.is_finite() { (pt * 20.0).round().clamp(144.0, 31680.0) as i64 } else { 12240 };
    let (pw, ph) = pages.first().map_or((12240, 15840), |p| (twips(p.width), twips(p.height)));
    let (mx, my) = ((pw / 8).min(1440), (ph / 8).min(1440));
    let text_w = (pw - 2 * mx) as f64 / 20.0;

    let mut s = String::from("{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1\n{\\fonttbl{\\f0\\fswiss Helvetica;}}\n{\\colortbl;");
    for [r, g, b] in &colors {
        let _ = write!(s, "\\red{r}\\green{g}\\blue{b};");
    }
    s.push_str("}\n{\\stylesheet{\\s0 Normal;}{\\s1\\sb240\\keepn\\b\\outlinelevel0 heading 1;}{\\s2\\sb200\\keepn\\b\\outlinelevel1 heading 2;}}\n");
    let _ = writeln!(s, "{{\\info{{\\title {}}}}}", text(title));
    let _ = writeln!(s, "\\paperw{pw}\\paperh{ph}\\margl{mx}\\margr{mx}\\margt{my}\\margb{my}");

    let mut skipped = 0;
    let mut image_bytes = 0usize;
    // A table is followed by an empty paragraph (another table next to it would merge, and the
    // document must not end inside one), as in the Word writer.
    let mut after_table = false;
    // A page break is `\pagebb` on the first paragraph of the next page (an `\page` of its own would leave an
    // empty paragraph at the top of it); before a table it needs an empty paragraph, as a cell cannot carry it.
    // Two breaks in a row are a blank page: its own empty paragraph.
    let mut page_break = false;
    for it in items {
        let mut brk = if !matches!(it, Item::PageBreak) && std::mem::take(&mut page_break) { "\\pagebb" } else { "" };
        if matches!(it, Item::Table(_)) {
            // Rows next to rows are one table to a reader, and \pagebb must not sit in a cell: a table that
            // follows a table or starts a page gets a paragraph of its own in front (carrying the break).
            if after_table || !brk.is_empty() {
                let _ = writeln!(s, "{{\\pard\\plain{brk}\\par}}");
            }
            brk = "";
        }
        after_table = matches!(it, Item::Table(_));
        match &it {
            Item::Para(b, lvl) => {
                let style = match lvl {
                    1 => "\\s1\\sb240\\keepn\\outlinelevel0\\b",
                    2 => "\\s2\\sb200\\keepn\\outlinelevel1\\b",
                    _ => "",
                };
                // A heading is bold by its style; the run's own bold is not needed twice.
                let fmt = char_fmt(b.size, b.bold && style.is_empty(), b.italic, &cf(b.color));
                let _ = writeln!(s, "{{\\pard\\plain{brk}\\sa120{style}{fmt} {}\\par}}", text(&b.text));
            }
            Item::Table(t) => table(t, text_w, &cf, &mut s),
            Item::Img(im) => {
                if image_bytes + im.bytes.len() > MAX_IMAGE_BYTES || !picture(im, text_w, brk, &mut s) {
                    skipped += 1;
                    page_break = !brk.is_empty(); // the break moves to the next item
                } else {
                    image_bytes += im.bytes.len();
                }
            }
            Item::PageBreak => {
                if std::mem::replace(&mut page_break, true) {
                    s.push_str("{\\pard\\plain\\pagebb\\par}\n");
                }
            }
        }
    }
    if after_table || page_break {
        s.push_str(if page_break { "{\\pard\\plain\\pagebb\\par}\n" } else { "{\\pard\\plain\\par}\n" });
    }
    s.push('}');
    Rtf { text: s, skipped_images: skipped }
}
