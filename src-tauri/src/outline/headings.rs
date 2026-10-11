//! Headings found from the text: lines set clearly larger than the body text (the size most
//! characters are set in), short, and not repeated on page after page. The largest size is level 1,
//! the next level 2, and so on. This is a guess to review, as Acrobat's own bookmarks from
//! structure are for an untagged document; the count is shown before anything is made.

use super::Result;
use crate::a11y::locate;
use crate::a11y::tree::{self, Tree};
use mupdf::pdf::PdfDocument;
use mupdf::text_page::{TextBlockType, TextPageFlags};
use serde::Serialize;
use std::collections::HashMap;

/// A heading is at least this much larger than the body text.
const HEADING_RATIO: f32 = 1.15;
const MAX_LINES: usize = 3;
const MAX_CHARS: usize = 160;
const MAX_LEVELS: usize = 5;
/// A title found on this share of the pages (and at least three) is a running head, not a heading.
const RUNNING_SHARE: f32 = 0.3;
/// The preview lists this many.
const PREVIEW_ROWS: usize = 40;

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub title: String,
    pub page: u32,
    /// Top left of the heading, page space.
    pub at: [f32; 2],
    /// 1 = largest.
    pub level: u8,
}

pub struct Found {
    pub rows: Vec<Row>,
    /// `"tags"`: the headings of the structure tree; `"size"`: guessed from the size of the text.
    pub source: &'static str,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeadingRow {
    pub title: String,
    pub page: u32,
    pub level: u8,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HeadingPreview {
    /// Bookmarks that would be made.
    pub count: u32,
    /// Distinct levels among them.
    pub levels: u8,
    /// The first of them.
    pub rows: Vec<HeadingRow>,
    /// Where they come from: `"tags"` (the file's structure tree) or `"size"` (the size of the text).
    pub source: String,
}

impl Found {
    pub fn preview(&self) -> HeadingPreview {
        let levels = self.rows.iter().map(|r| r.level).max().unwrap_or(0);
        HeadingPreview {
            count: self.rows.len() as u32,
            levels,
            rows: self
                .rows
                .iter()
                .take(PREVIEW_ROWS)
                .map(|r| HeadingRow { title: r.title.clone(), page: r.page, level: r.level })
                .collect(),
            source: self.source.to_string(),
        }
    }
}

struct Line {
    text: String,
    size: f32,
    block: usize,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

fn half(s: f32) -> i32 {
    (s * 2.0).round() as i32
}

fn page_lines(doc: &PdfDocument, p: i32) -> Result<Vec<Line>> {
    let page = doc.load_pdf_page(p)?;
    let tp = page.to_text_page(TextPageFlags::empty())?;
    let mut out = Vec::new();
    let mut block = 0;
    for b in tp.blocks() {
        if b.r#type() != TextBlockType::Text {
            continue;
        }
        let mut any = false;
        for l in b.lines() {
            let mut text = String::new();
            let mut size = 0f32;
            for ch in l.chars() {
                size = size.max(ch.size());
                if let Some(c) = ch.char() {
                    text.push(c);
                }
            }
            let text = super::clean_title(&text);
            if text.is_empty() {
                continue;
            }
            any = true;
            let bb = l.bounds();
            out.push(Line { text, size, block, x0: bb.x0, y0: bb.y0, x1: bb.x1, y1: bb.y1 });
        }
        if any {
            block += 1;
        }
    }
    Ok(out)
}

/// A line that is only a number, a page number, a rule or a bullet.
fn is_noise(t: &str) -> bool {
    let letters = t.chars().filter(|c| c.is_alphabetic()).count();
    letters < 2 && t.chars().count() < 12
}

/// First marked content found at or under element `i`, as (page, marked-content id).
fn first_content(t: &Tree, i: usize) -> Option<(u32, i32)> {
    let e = &t.elems[i];
    if let (Some(p), Some(m)) = (e.page, e.mcids.first()) {
        return Some((p, *m));
    }
    e.children.iter().find_map(|c| first_content(t, *c))
}

/// The headings the file's own tags name (H1 to H6, and H by the sections it sits in), in reading
/// order. The text is the block of text that starts where the heading's marked content starts.
/// Empty when the file is not tagged or has no heading elements.
fn find_tagged(doc: &PdfDocument) -> Result<Vec<Row>> {
    let pages = tree::page_index(doc)?;
    let t = Tree::read(doc, &pages)?;
    if !t.exists {
        return Ok(Vec::new());
    }
    let mut origins: HashMap<u32, HashMap<i32, [f32; 2]>> = HashMap::new();
    let mut lines: HashMap<u32, Vec<Line>> = HashMap::new();
    let mut rows: Vec<Row> = Vec::new();
    for i in t.preorder() {
        let level = match t.elems[i].ty.as_str() {
            "H1" => 1,
            "H2" => 2,
            "H3" => 3,
            "H4" => 4,
            "H5" => 5,
            "H6" => 6,
            "H" => t.ancestors(i).filter(|a| t.is(*a, "Sect")).count().clamp(1, 6) as u8,
            _ => continue,
        };
        let Some((page, mcid)) = first_content(&t, i) else { continue };
        if !origins.contains_key(&page) {
            let p = doc.find_page(page as i32)?;
            origins.insert(page, locate::mcid_text_origins(&p)?);
            lines.insert(page, page_lines(doc, page as i32)?);
        }
        let Some([x, y]) = origins[&page].get(&mcid).copied() else { continue };
        let ls = &lines[&page];
        let Some(hit) = ls.iter().find(|l| y >= l.y0 - 1.5 && y <= l.y1 + 1.5 && x >= l.x0 - 2.0 && x <= l.x1 + 2.0) else { continue };
        let first = ls.iter().position(|l| l.block == hit.block).unwrap_or(0);
        let title = ls[first..].iter().take_while(|l| l.block == hit.block).take(MAX_LINES).map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
        let title: String = title.chars().take(MAX_CHARS).collect();
        if is_noise(&title) {
            continue;
        }
        if rows.last().is_some_and(|r| r.page == page && r.title == title) {
            continue;
        }
        rows.push(Row { title, page, at: [hit.x0, hit.y0], level });
    }
    Ok(rows)
}

/// The headings of the whole document, in reading order: from the file's tags when it is tagged and
/// has heading elements, else guessed from the size of the text.
pub fn find(doc: &PdfDocument) -> Result<Found> {
    if let Ok(rows) = find_tagged(doc) {
        if !rows.is_empty() {
            return Ok(Found { rows, source: "tags" });
        }
    }
    find_by_size(doc)
}

/// The headings guessed from the size of the text.
fn find_by_size(doc: &PdfDocument) -> Result<Found> {
    let n = doc.page_count()?;
    let mut pages: Vec<Vec<Line>> = Vec::with_capacity(n.max(0) as usize);
    let mut chars_at: HashMap<i32, usize> = HashMap::new();
    for p in 0..n {
        let lines = page_lines(doc, p)?;
        for l in &lines {
            *chars_at.entry(half(l.size)).or_default() += l.text.chars().count();
        }
        pages.push(lines);
    }
    let body = chars_at.iter().max_by_key(|(s, c)| (**c, -**s)).map_or(24, |(s, _)| *s);
    let min = (body as f32 * HEADING_RATIO).ceil() as i32;

    // Candidates: a block of at most MAX_LINES lines, all larger than the body text; its text is
    // the lines joined, its size the largest in it.
    struct Cand {
        title: String,
        page: u32,
        at: [f32; 2],
        size: i32,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for (p, lines) in pages.iter().enumerate() {
        let mut b = 0;
        while b < lines.len() {
            let blk = lines[b].block;
            let end = lines[b..].iter().position(|l| l.block != blk).map_or(lines.len(), |k| b + k);
            let group = &lines[b..end];
            b = end;
            let size = group.iter().map(|l| half(l.size)).max().unwrap_or(0);
            let title = group.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join(" ");
            if size < min || group.len() > MAX_LINES || title.chars().count() > MAX_CHARS || is_noise(&title) {
                continue;
            }
            cands.push(Cand { title, page: p as u32, at: [group[0].x0, group[0].y0], size });
        }
    }

    // Running heads: the same title on many pages.
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for c in &cands {
        *seen.entry(c.title.as_str()).or_default() += 1;
    }
    let running = |t: &str| {
        let k = seen.get(t).copied().unwrap_or(0);
        k >= 3 && k as f32 >= RUNNING_SHARE * n.max(1) as f32
    };
    let cands: Vec<&Cand> = cands.iter().filter(|c| !running(&c.title)).collect();

    // Levels: the distinct sizes among the candidates, largest first.
    let mut sizes: Vec<i32> = cands.iter().map(|c| c.size).collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    let level = |s: i32| (sizes.iter().position(|x| *x == s).unwrap_or(0) + 1).min(MAX_LEVELS) as u8;
    let rows = cands.iter().map(|c| Row { title: c.title.clone(), page: c.page, at: c.at, level: level(c.size) }).collect();
    Ok(Found { rows, source: "size" })
}
