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

pub mod catalog;
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
/// Hits kept per page, across text, comments and form fields together (a page of "a" searched for "a" would
/// otherwise flood the list).
pub const PAGE_CAP: usize = 2000;
/// Hits one in-document search lists; after this the search stops and says it listed the first ones only.
pub const DOC_CAP: usize = 5000;
/// Longest query that is searched.
const QUERY_CAP: usize = 256;
/// The fold a catalog indexes in: case folded, accents dropped (every option set looks for less than this).
const WIDEST: SearchOpts = SearchOpts { case_sensitive: false, whole_words: false, ignore_accents: true, comments: false, bookmarks: false, fields: false };
/// Characters of the longest word a catalog lists, and words listed per page; a page that has more is always verified.
pub const MAX_TERM: usize = 64;
pub const MAX_TERMS: usize = 8192;

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

/// The words of a query in the widest fold (what a catalog's index is looked up with); none when the
/// query is only marks. See `PageChars::terms`.
pub fn index_needle(query: &str) -> Vec<String> {
    let mut f = Folder::new(WIDEST);
    let mut folded = Vec::new();
    for c in query.trim().chars().take(QUERY_CAP) {
        f.push(c, &mut folded);
    }
    folded.into_iter().collect::<String>().split(' ').filter(|w| !w.is_empty()).map(str::to_owned).collect()
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

    /// Appends the folded forms of `c` to `out`: a soft hyphen is nothing, white space is one space, letters are
    /// decomposed (so "é" and "e" + U+0301 are the same text, on both sides), then case-folded unless the case
    /// counts, and their marks dropped when accents are ignored.
    fn push(&mut self, c: char, out: &mut Vec<char>) {
        if c == '\u{ad}' {
            return;
        }
        let c = if c.is_whitespace() || c == '\u{a0}' { ' ' } else { c };
        if c == ' ' {
            out.push(c);
        } else if c.is_ascii() {
            self.case(c, out);
        } else {
            let mut buf = [0u8; 4];
            let s: &str = c.encode_utf8(&mut buf);
            let d = self.nfd.normalize(s);
            for x in d.chars() {
                if self.opts.ignore_accents && is_mark(x) {
                    continue;
                }
                self.case(x, out);
            }
        }
    }

    fn case(&self, c: char, out: &mut Vec<char>) {
        if self.opts.case_sensitive {
            out.push(c);
            return;
        }
        // Full case folding for what `to_lowercase` leaves apart: "Straße" is "STRASSE", a final sigma is a sigma.
        match c {
            'ß' | 'ẞ' => out.extend(['s', 's']),
            'ς' => out.push('σ'),
            'ſ' => out.push('s'),
            _ => out.extend(c.to_lowercase()),
        }
    }
}

/// One page's text as a catalog stores it (`PageChars::to_stored`): the characters, which of them are the
/// synthetic spaces between lines and which are line-end hyphens that join to the next line, and one box
/// (x0 y0 x1 y1, page points, origin top left) per line, with the index of its first character. A hit lists the
/// box of the line it is on, not the characters' own rectangles.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct StoredText {
    pub text: String,
    pub synth: Vec<u32>,
    pub skip: Vec<u32>,
    pub lines: Vec<(u32, [f32; 4])>,
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

    /// The page as a catalog keeps it: the characters and one box per line instead of one per character.
    pub fn to_stored(&self) -> StoredText {
        let mut st = StoredText::default();
        let mut line = 0u32;
        for (i, (c, r, l)) in self.chars.iter().enumerate() {
            st.text.push(*c);
            if r.is_none() {
                st.synth.push(i as u32);
            }
            if self.skip[i] {
                st.skip.push(i as u32);
            }
            if r.is_some() && *l != line {
                line = *l;
                st.lines.push((i as u32, [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY]));
            }
            if let (Some(r), Some(b)) = (r, st.lines.last_mut()) {
                b.1 = [b.1[0].min(r[0]), b.1[1].min(r[1]), b.1[2].max(r[2]), b.1[3].max(r[3])];
            }
        }
        st
    }

    /// A page from its stored form (which may come from a PDF's embedded index, so nothing in it is trusted:
    /// indexes out of range are ignored). Every character of a line gets the line's box.
    pub fn from_stored(st: &StoredText) -> PageChars {
        let n = st.text.chars().count();
        let mut skip = vec![false; n];
        for &i in &st.skip {
            if let Some(s) = skip.get_mut(i as usize) {
                *s = true;
            }
        }
        let mut synth = vec![false; n];
        for &i in &st.synth {
            if let Some(s) = synth.get_mut(i as usize) {
                *s = true;
            }
        }
        let mut k = 0usize;
        let chars = st
            .text
            .chars()
            .enumerate()
            .map(|(i, c)| {
                while k < st.lines.len() && st.lines[k].0 as usize <= i {
                    k += 1;
                }
                // `k` lines have started: the character is on line `k` (1-based), or on none yet.
                let rect = (!synth[i]).then(|| k.checked_sub(1).map(|j| st.lines[j].1)).flatten();
                (c, rect, k as u32)
            })
            .collect();
        PageChars { chars, skip }
    }

    /// Every match of `q`, in reading order, at most `PAGE_CAP`.
    pub fn find(&self, q: &Query, source: Source) -> Vec<Hit> {
        self.find_limit(q, source, PAGE_CAP)
    }

    /// Every match of `q`, in reading order, at most `limit`.
    pub fn find_limit(&self, q: &Query, source: Source, limit: usize) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        let mut spans = self.spans(q, false, limit);
        if self.skip.iter().any(|s| *s) {
            // A hyphen that ends a line is the writer's, not the author's ("co-" "operate"), unless it is: look
            // again with every such hyphen kept ("well-" "known"), and take what either reading finds.
            spans.extend(self.spans(q, true, limit));
            spans.sort_unstable();
            spans.dedup_by_key(|s| s.0);
            spans.truncate(limit);
        }
        spans.into_iter().map(|(start, end)| self.hit(start, end, source)).collect()
    }

    /// The folded characters of the page (each with the index of the character it came from), runs of white space
    /// collapsed, with the line-end hyphens dropped or kept.
    fn flat(&self, opts: SearchOpts, keep_hyphens: bool) -> Vec<(char, usize)> {
        let mut f = Folder::new(opts);
        let mut flat: Vec<(char, usize)> = Vec::with_capacity(self.chars.len());
        let mut buf: Vec<char> = Vec::new();
        for (i, (c, _, _)) in self.chars.iter().enumerate() {
            if self.skip[i] && !keep_hyphens {
                continue;
            }
            buf.clear();
            f.push(*c, &mut buf);
            for d in buf.iter() {
                flat.push((*d, i));
            }
        }
        collapse(flat)
    }

    /// The words (runs between spaces) of the page in the widest fold, over both readings of a line-end hyphen:
    /// what a catalog indexes. Any match of any query, under any options, lies inside a run of these words.
    /// Longer than `MAX_TERM` characters, or more than `MAX_TERMS` words: not listed, and `false` comes back (the
    /// page must then be verified for every query).
    pub fn terms(&self, out: &mut std::collections::BTreeSet<String>) -> bool {
        let mut complete = true;
        for keep in [false, true] {
            if keep && !self.skip.iter().any(|s| *s) {
                break;
            }
            let flat = self.flat(WIDEST, keep);
            for w in flat.split(|c| c.0 == ' ') {
                if w.is_empty() {
                    continue;
                }
                if w.len() > MAX_TERM || out.len() >= MAX_TERMS {
                    complete = false;
                } else {
                    out.insert(w.iter().map(|c| c.0).collect());
                }
            }
        }
        complete
    }

    /// Where `q` matches, as (first, last) character indexes, with the line-end hyphens dropped or kept.
    fn spans(&self, q: &Query, keep_hyphens: bool, limit: usize) -> Vec<(usize, usize)> {
        let flat = self.flat(q.opts, keep_hyphens);
        let n = q.needle.len();
        let word = |k: Option<&(char, usize)>| k.is_some_and(|c| c.0.is_alphanumeric());
        let mark = |k: Option<&(char, usize)>| k.is_some_and(|c| is_mark(c.0));
        let mut out = Vec::new();
        let mut i = 0;
        while i + n <= flat.len() && out.len() < limit {
            // With accents counting, "cafe" is not the start of "café" (a mark follows) nor a whole word of it.
            let bounded = !mark(flat.get(i + n))
                && (!q.opts.whole_words || (!word(i.checked_sub(1).and_then(|p| flat.get(p))) && !word(flat.get(i + n))));
            if bounded && flat[i..i + n].iter().map(|c| c.0).eq(q.needle.iter().copied()) {
                out.push((flat[i].1, flat[i + n - 1].1));
                i += n;
            } else {
                i += 1;
            }
        }
        out
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

/// Everything the matcher needs of one page: its text, and the comments and form-field values. `find_in_page`
/// is `extract_record` then `PageRec::find`; a catalog keeps the record and runs `find` alone, so both give the
/// same hits by construction.
pub struct PageRec {
    pub text: PageChars,
    /// The text of every markup annotation (not links, widgets, popups or redaction marks) and its bounds.
    pub comments: Vec<(String, [f32; 4])>,
    /// The value of every form field and its bounds.
    pub fields: Vec<(String, [f32; 4])>,
}

/// The record of `page` (0-based); comments and fields only when `extras` (the find options ask for them, or
/// a catalog is being built).
pub fn extract_record(doc: &Document, page: u32, extras: bool) -> Result<PageRec> {
    let p = doc.load_page(page as i32)?;
    let mut rec = PageRec { text: PageChars::of(&p)?, comments: Vec::new(), fields: Vec::new() };
    if !extras {
        return Ok(rec);
    }
    let origin = p.bounds()?;
    // Not every document is a PDF with annotations; the text hits stand either way.
    let Ok(pp) = PdfPage::try_from(p) else { return Ok(rec) };
    let rect = |r: mupdf::Rect| [r.x0 - origin.x0, r.y0 - origin.y0, r.x1 - origin.x0, r.y1 - origin.y0];
    for a in pp.annotations() {
        let Ok(t) = a.r#type() else { continue };
        if matches!(t, PdfAnnotationType::Link | PdfAnnotationType::Widget | PdfAnnotationType::Popup | PdfAnnotationType::Redact) {
            continue;
        }
        let Ok(Some(c)) = a.contents() else { continue };
        let Ok(b) = a.bounds() else { continue };
        rec.comments.push((c.to_string(), rect(b)));
    }
    for w in pp.widgets() {
        let Ok(Some(v)) = w.value() else { continue };
        let Ok(b) = w.annotation().bounds() else { continue };
        rec.fields.push((v, rect(b)));
    }
    Ok(rec)
}

impl PageRec {
    /// Every hit on the page: its text, and per the options its comments and form-field values. One budget per
    /// page across text, comments and fields: the page never lists more than `PAGE_CAP`.
    pub fn find(&self, q: &Query) -> Vec<Hit> {
        let mut hits = self.text.find(q, Source::Text);
        if !(q.opts.comments || q.opts.fields) || hits.len() >= PAGE_CAP {
            return hits;
        }
        let room = |hits: &Vec<Hit>| PAGE_CAP.saturating_sub(hits.len());
        if q.opts.comments {
            for (c, b) in &self.comments {
                for mut h in PageChars::of_text(c).find_limit(q, Source::Comment, room(&hits)) {
                    h.rects = vec![*b];
                    hits.push(h);
                }
            }
        }
        if q.opts.fields {
            for (v, b) in &self.fields {
                for mut h in PageChars::of_text(v).find_limit(q, Source::Field, room(&hits)) {
                    h.rects = vec![*b];
                    hits.push(h);
                }
            }
        }
        hits
    }
}

/// Every hit on `page` (0-based): its text, and per the options its comments and form-field values.
pub fn find_in_page(doc: &Document, page: u32, q: &Query) -> Result<Vec<Hit>> {
    Ok(extract_record(doc, page, q.opts.comments || q.opts.fields)?.find(q))
}

/// The hits of a bookmark title that goes to `page`.
pub fn bookmark_hits_of(page: u32, title: &str, q: &Query) -> Vec<(u32, Hit)> {
    PageChars::of_text(title).find(q, Source::Bookmark).into_iter().map(|h| (page, h)).collect()
}

/// Bookmarks whose title matches, as (page, hit); a bookmark that goes nowhere is not a result.
pub fn bookmark_hits(doc: &Document, q: &Query) -> Result<Vec<(u32, Hit)>> {
    fn walk(items: &[mupdf::Outline], q: &Query, out: &mut Vec<(u32, Hit)>) {
        for o in items {
            if let Some(d) = o.dest {
                out.extend(bookmark_hits_of(d.loc.page_number, &o.title, q));
            }
            walk(&o.down, q, out);
        }
    }
    let mut out = Vec::new();
    walk(&doc.outlines().map_err(EngineError::from)?, q, &mut out);
    Ok(out)
}
