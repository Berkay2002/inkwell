//! Web addresses in the text of the pages: `https://…`, `http://…`, `www.…` and `mailto:` /
//! e-mail addresses that have no link over them yet. Found line by line (an address broken over
//! two lines is not joined), each with the rectangle of the characters it is made of.

use super::{Result, gate_url, read_page};
use mupdf::pdf::PdfDocument;
use mupdf::text_page::{TextBlockType, TextPageFlags};
use regex::Regex;
use serde::Serialize;
use std::sync::LazyLock;

/// The preview lists this many.
const PREVIEW_ROWS: usize = 12;

static URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:\b(?:https?://|mailto:)[^\s<>"'\u{201C}\u{201D}]+|\bwww\.[a-z0-9-]+(?:\.[a-z0-9-]+)+[^\s<>"'\u{201C}\u{201D}]*|\b[a-z0-9._%+-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)*\.[a-z]{2,}\b)"#)
        .expect("valid regex")
});

pub struct Hit {
    pub page: u32,
    /// The address as it will be linked.
    pub url: String,
    /// One rectangle per line the address is on, page space.
    pub rects: Vec<[f32; 4]>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UrlRow {
    pub page: u32,
    pub url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UrlPreview {
    /// Links that would be made.
    pub count: u32,
    /// Pages they are on.
    pub pages: u32,
    /// The first of them.
    pub rows: Vec<UrlRow>,
}

pub fn preview(hits: &[Hit]) -> UrlPreview {
    let mut pages: Vec<u32> = hits.iter().map(|h| h.page).collect();
    pages.dedup();
    UrlPreview {
        count: hits.iter().map(|h| h.rects.len() as u32).sum(),
        pages: pages.len() as u32,
        rows: hits.iter().take(PREVIEW_ROWS).map(|h| UrlRow { page: h.page, url: h.url.clone() }).collect(),
    }
}

/// Trailing punctuation that ends a sentence, not an address; an unmatched closing bracket too.
fn trim_end(s: &str) -> &str {
    let mut s = s;
    loop {
        let Some(last) = s.chars().last() else { return s };
        let open = match last {
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => None,
        };
        let cut = match open {
            Some(o) => s.matches(o).count() < s.matches(last).count(),
            None => matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '\u{2019}'),
        };
        if cut {
            s = &s[..s.len() - last.len_utf8()];
        } else {
            return s;
        }
    }
}

fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = (a[2] - a[0]) * (a[3] - a[1]);
    area > 0.0 && w * h >= area * 0.3
}

/// The addresses of the whole document that no link covers, in reading order.
pub fn find(doc: &PdfDocument) -> Result<Vec<Hit>> {
    let n = doc.page_count()?;
    let mut out = Vec::new();
    for p in 0..n {
        let existing: Vec<[f32; 4]> = read_page(doc, p as u32, 0)?.links.into_iter().map(|l| l.rect).collect();
        let page = doc.load_pdf_page(p)?;
        let tp = page.to_text_page(TextPageFlags::empty())?;
        for b in tp.blocks() {
            if b.r#type() != TextBlockType::Text {
                continue;
            }
            for l in b.lines() {
                // The characters of the line, and where each starts in the line's string.
                let mut text = String::new();
                let mut boxes: Vec<(usize, [f32; 4])> = Vec::new();
                for ch in l.chars() {
                    let Some(c) = ch.char() else { continue };
                    let q = ch.quad();
                    let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
                    let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
                    let r = [
                        xs.iter().cloned().fold(f32::MAX, f32::min),
                        ys.iter().cloned().fold(f32::MAX, f32::min),
                        xs.iter().cloned().fold(f32::MIN, f32::max),
                        ys.iter().cloned().fold(f32::MIN, f32::max),
                    ];
                    boxes.push((text.len(), r));
                    text.push(c);
                }
                for m in URL.find_iter(&text) {
                    let s = trim_end(m.as_str());
                    if s.len() < 4 {
                        continue;
                    }
                    let (start, end) = (m.start(), m.start() + s.len());
                    let mut r: Option<[f32; 4]> = None;
                    for (off, bx) in &boxes {
                        if *off >= start && *off < end {
                            r = Some(match r {
                                None => *bx,
                                Some(a) => [a[0].min(bx[0]), a[1].min(bx[1]), a[2].max(bx[2]), a[3].max(bx[3])],
                            });
                        }
                    }
                    let Some(r) = r else { continue };
                    let lower = s.to_ascii_lowercase();
                    let full = if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
                        s.to_string()
                    } else if lower.starts_with("www.") {
                        format!("https://{s}")
                    } else {
                        format!("mailto:{s}")
                    };
                    let Ok(url) = gate_url(&full) else { continue };
                    if existing.iter().any(|e| overlaps(r, *e)) || out.iter().any(|h: &Hit| h.page == p as u32 && h.rects.iter().any(|x| overlaps(r, *x))) {
                        continue;
                    }
                    out.push(Hit { page: p as u32, url, rects: vec![r] });
                }
            }
        }
    }
    Ok(out)
}
