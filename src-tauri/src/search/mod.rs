//! Find with Acrobat's options: match case, whole words, ignore accents, and whether comments, bookmarks and
//! form-field values are searched too. Every hit carries the rectangles to highlight (one per line it covers)
//! and the words around it, so the results panel can list it in context.
//!
//! The page text comes from MuPDF's structured text (characters with their quads); the matching is ours so the
//! options and the snippets are exact. Folder search (`files.rs`) runs this same matcher over every file.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/render/src/text.rs @ b8ddf71 (find_opts: fold, collapse
//! whitespace, bounded by word characters), translated to MuPDF characters and extended with accents,
//! hyphenated line ends and snippets.

pub mod files;
#[cfg(test)]
mod tests;

use crate::engine::{EngineError, Result};
use mupdf::pdf::{PdfAnnotationType, PdfPage};
use mupdf::text_page::{TextBlockType, TextPageFlags};
use mupdf::Document;
use serde::{Deserialize, Serialize};

/// Characters of context kept on each side of a hit.
pub const CONTEXT: usize = 48;
/// Hits kept per page (a page of "a" searched for "a" would otherwise flood the list).
const PAGE_CAP: usize = 2000;
/// Longest query that is searched.
const QUERY_CAP: usize = 256;

/// The find options. All off is a plain, case-insensitive text search.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchOpts {
    pub case_sensitive: bool,
    pub whole_words: bool,
    pub ignore_accents: bool,
    pub comments: bool,
    pub bookmarks: bool,
    pub fields: bool,
}

/// Where a hit was found.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Text,
    Comment,
    Field,
    Bookmark,
}

/// One match: where to draw it and what surrounds it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    /// One rectangle per line the match covers, in unrotated page space (points, origin top-left).
    /// Empty for a bookmark.
    pub rects: Vec<[f32; 4]>,
    pub before: String,
    pub text: String,
    pub after: String,
    pub source: Source,
}

/// A query ready to match: folded and with runs of white space collapsed, as the page text is.
pub struct Query {
    needle: Vec<char>,
    opts: SearchOpts,
}

impl Query {
    /// `None` when there is nothing to look for.
    pub fn new(query: &str, opts: SearchOpts) -> Option<Query> {
        let mut folded = Vec::new();
        let mut f = Folder::new(opts);
        for c in query.trim().chars().take(QUERY_CAP) {
            f.push(c, &mut folded);
        }
        let needle = collapse(folded.into_iter().map(|c| (c, 0)).collect());
        let needle = trim_spaces(needle.into_iter().map(|(c, _)| c).collect());
        (!needle.is_empty()).then_some(Query { needle, opts })
    }

    pub fn opts(&self) -> SearchOpts {
        self.opts
    }
}

fn trim_spaces(mut v: Vec<char>) -> Vec<char> {
    while v.last() == Some(&' ') {
        v.pop();
    }
    while v.first() == Some(&' ') {
        v.remove(0);
    }
    v
}

/// Collapses a run of spaces to one; keeps the source index of the first char of a run.
fn collapse(v: Vec<(char, usize)>) -> Vec<(char, usize)> {
    let mut out: Vec<(char, usize)> = Vec::with_capacity(v.len());
    for (c, i) in v {
        if c == ' ' && out.last().is_some_and(|k| k.0 == ' ') {
            continue;
        }
        out.push((c, i));
    }
    out
}

/// Case and accent folding of one character (it may become several, or none for a bare accent).
struct Folder {
    opts: SearchOpts,
    nfd: icu_normalizer::DecomposingNormalizerBorrowed<'static>,
}

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

impl Folder {
    fn new(opts: SearchOpts) -> Folder {
        Folder { opts, nfd: icu_normalizer::DecomposingNormalizerBorrowed::new_nfd() }
    }

    /// Appends the folded forms of `c` to `out`.
    fn push(&mut self, c: char, out: &mut Vec<char>) {
        let c = if c.is_whitespace() || c == '\u{a0}' { ' ' } else { c };
        if c == ' ' || (c.is_ascii() && self.opts.case_sensitive) {
            out.push(c);
            return;
        }
        let one = |d: char, out: &mut Vec<char>| {
            if self.opts.case_sensitive {
                out.push(d);
            } else {
                out.extend(d.to_lowercase());
            }
        };
        if self.opts.ignore_accents && !c.is_ascii() {
            let mut buf = [0u8; 4];
            let s: &str = c.encode_utf8(&mut buf);
            let d = self.nfd.normalize(s).into_owned();
            for x in d.chars().filter(|x| !is_mark(*x)) {
                one(x, out);
            }
        } else {
            one(c, out);
        }
    }
}

/// The characters of one page, in reading order, with a synthetic space between lines.
pub struct PageChars {
    /// What the page shows, one entry per character; `None` rect for a synthetic space.
    chars: Vec<(char, Option<[f32; 4]>, u32)>,
    /// Characters left out of matching (a hyphen at a line end joined to the next line).
    skip: Vec<bool>,
}

impl PageChars {
    /// The text of the page in MuPDF's structured text, rectangles relative to the page origin.
    pub fn of(page: &mupdf::Page) -> Result<PageChars> {
        let origin = page.bounds()?;
        let tp = page.to_text_page(TextPageFlags::empty())?;
        let mut chars: Vec<(char, Option<[f32; 4]>, u32)> = Vec::new();
        let mut skip: Vec<bool> = Vec::new();
        let mut line_no = 0u32;
        for block in tp.blocks() {
            if block.r#type() != TextBlockType::Text {
                continue;
            }
            // Lines of one block: collect so the next line's first character is known at a line end.
            let lines: Vec<Vec<(char, [f32; 4])>> = block
                .lines()
                .map(|l| {
                    l.chars()
                        .filter_map(|c| {
                            let ch = c.char()?;
                            let q = c.quad();
                            let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
                            let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
                            let min = |a: [f32; 4]| a.into_iter().fold(f32::INFINITY, f32::min);
                            let max = |a: [f32; 4]| a.into_iter().fold(f32::NEG_INFINITY, f32::max);
                            Some((ch, [min(xs) - origin.x0, min(ys) - origin.y0, max(xs) - origin.x0, max(ys) - origin.y0]))
                        })
                        .collect()
                })
                .collect();
            for (li, line) in lines.iter().enumerate() {
                if line.is_empty() {
                    continue;
                }
                line_no += 1;
                let next_starts_word = lines.get(li + 1).and_then(|n| n.first()).is_some_and(|(c, _)| c.is_alphabetic());
                let n = line.len();
                let mut joins = false;
                for (ci, (c, r)) in line.iter().enumerate() {
                    // A hyphen that ends a line and is followed by a word on the next one is the writer's, not the
                    // author's: "co-" "operate" reads as "cooperate".
                    let hyphen = ci + 1 == n && matches!(c, '-' | '\u{ad}' | '\u{2010}') && next_starts_word && ci > 0 && line[ci - 1].0.is_alphabetic();
                    joins = hyphen;
                    chars.push((*c, Some(*r), line_no));
                    skip.push(hyphen);
                }
                if !joins && li + 1 < lines.len() {
                    chars.push((' ', None, line_no));
                    skip.push(false);
                }
            }
            // Blocks are separate paragraphs.
            if chars.last().is_some_and(|c| c.0 != ' ') {
                chars.push((' ', None, line_no));
                skip.push(false);
            }
        }
        Ok(PageChars { chars, skip })
    }

    /// A page of plain strings, for tests and for text that is not on a page (a comment).
    pub fn of_text(s: &str) -> PageChars {
        let chars: Vec<_> = s.chars().map(|c| (c, None, 0)).collect();
        let skip = vec![false; chars.len()];
        PageChars { chars, skip }
    }

    /// Every match of `q`, in reading order.
    pub fn find(&self, q: &Query, source: Source) -> Vec<Hit> {
        let mut f = Folder::new(q.opts);
        let mut flat: Vec<(char, usize)> = Vec::with_capacity(self.chars.len());
        let mut buf: Vec<char> = Vec::new();
        for (i, (c, _, _)) in self.chars.iter().enumerate() {
            if self.skip[i] {
                continue;
            }
            buf.clear();
            f.push(*c, &mut buf);
            for d in buf.iter() {
                flat.push((*d, i));
            }
        }
        let flat = collapse(flat);
        let n = q.needle.len();
        let word = |k: Option<&(char, usize)>| k.is_some_and(|c| c.0.is_alphanumeric());
        let mut hits = Vec::new();
        let mut i = 0;
        while i + n <= flat.len() && hits.len() < PAGE_CAP {
            let bounded = !q.opts.whole_words || (!word(i.checked_sub(1).and_then(|p| flat.get(p))) && !word(flat.get(i + n)));
            if bounded && flat[i..i + n].iter().map(|c| c.0).eq(q.needle.iter().copied()) {
                let (start, end) = (flat[i].1, flat[i + n - 1].1);
                hits.push(self.hit(start, end, source));
                i += n;
            } else {
                i += 1;
            }
        }
        hits
    }

    /// The hit that covers characters `start..=end`.
    fn hit(&self, start: usize, end: usize, source: Source) -> Hit {
        let mut rects: Vec<[f32; 4]> = Vec::new();
        let mut line = None;
        for (_, r, l) in &self.chars[start..=end] {
            let Some(r) = r else { continue };
            if line == Some(*l) {
                let k = rects.last_mut().expect("a rect for this line");
                k[0] = k[0].min(r[0]);
                k[1] = k[1].min(r[1]);
                k[2] = k[2].max(r[2]);
                k[3] = k[3].max(r[3]);
            } else {
                rects.push(*r);
                line = Some(*l);
            }
        }
        let text = |a: usize, b: usize| -> String { self.chars[a..b].iter().map(|c| c.0).collect() };
        let before = tidy(&text(start.saturating_sub(CONTEXT * 2), start));
        let after = tidy(&text(end + 1, (end + 1 + CONTEXT * 2).min(self.chars.len())));
        Hit {
            rects,
            before: tail(&before, CONTEXT),
            text: tidy(&text(start, end + 1)),
            after: head(&after, CONTEXT),
            source,
        }
    }
}

/// White space collapsed to single spaces (the leading and trailing one stay, they separate words).
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = if c.is_whitespace() { ' ' } else { c };
        if c == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(c);
    }
    out
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn tail(s: &str, n: usize) -> String {
    let k = s.chars().count();
    s.chars().skip(k.saturating_sub(n)).collect()
}

/// Every hit on `page` (0-based): its text, and per the options its comments and form-field values.
pub fn find_in_page(doc: &Document, page: u32, q: &Query) -> Result<Vec<Hit>> {
    let p = doc.load_page(page as i32)?;
    let mut hits = PageChars::of(&p)?.find(q, Source::Text);
    if !(q.opts.comments || q.opts.fields) {
        return Ok(hits);
    }
    let origin = p.bounds()?;
    // Not every document is a PDF with annotations; the text hits stand either way.
    let Ok(pp) = PdfPage::try_from(p) else { return Ok(hits) };
    let rect = |r: mupdf::Rect| [r.x0 - origin.x0, r.y0 - origin.y0, r.x1 - origin.x0, r.y1 - origin.y0];
    if q.opts.comments {
        for a in pp.annotations() {
            let Ok(t) = a.r#type() else { continue };
            if matches!(t, PdfAnnotationType::Link | PdfAnnotationType::Widget | PdfAnnotationType::Popup | PdfAnnotationType::Redact) {
                continue;
            }
            let Ok(Some(c)) = a.contents() else { continue };
            let Ok(b) = a.bounds() else { continue };
            for mut h in PageChars::of_text(c).find(q, Source::Comment) {
                h.rects = vec![rect(b)];
                hits.push(h);
            }
        }
    }
    if q.opts.fields {
        for w in pp.widgets() {
            let Ok(Some(v)) = w.value() else { continue };
            let Ok(b) = w.annotation().bounds() else { continue };
            for mut h in PageChars::of_text(&v).find(q, Source::Field) {
                h.rects = vec![rect(b)];
                hits.push(h);
            }
        }
    }
    Ok(hits)
}

/// Bookmarks whose title matches, as (page, hit); a bookmark that goes nowhere is not a result.
pub fn bookmark_hits(doc: &Document, q: &Query) -> Result<Vec<(u32, Hit)>> {
    fn walk(items: &[mupdf::Outline], q: &Query, out: &mut Vec<(u32, Hit)>) {
        for o in items {
            if let Some(d) = o.dest {
                for h in PageChars::of_text(&o.title).find(q, Source::Bookmark) {
                    out.push((d.loc.page_number, h));
                }
            }
            walk(&o.down, q, out);
        }
    }
    let mut out = Vec::new();
    walk(&doc.outlines().map_err(EngineError::from)?, q, &mut out);
    Ok(out)
}
