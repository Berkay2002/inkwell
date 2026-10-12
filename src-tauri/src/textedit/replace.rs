//! Find & Replace while editing text (Acrobat's Edit > Find & Replace), on top of the block edit.
//!
//! Matching is Find's own (`search::Query`: case, whole words, accents) run over the runs of each text
//! block, the text `rewrite_block` re-sets, so a replacement is exactly an Edit text change of that block:
//! same fonts, same fallback, same report. Replace all is planned page by page from one reading of the page
//! and applied block by block from the last to the first; nothing is searched again after a replacement,
//! so a replacement that contains the find string cannot loop. The whole run is one `apply_removing` step.
//!
//! A match that Edit text would refuse (Type 3, no ToUnicode, invisible text, a match across two blocks,
//! text in a form) is skipped and listed with its page and reason; a block is rewritten whole or not at all.
//! Comments, form-field values and bookmarks are not replaced.

use super::text::{self, group_blocks, issue_text, read_page, Block, Fallback, KeepLines, RunChange, TextChange};
use super::{page_space, rect32, Result};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::search::{self, Query, SearchOpts, Source};
use mupdf::pdf::PdfDocument;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Replacements one run makes; after this it stops and says so (search again for the rest).
pub const MAX_REPLACEMENTS: usize = 2000;
/// Skipped matches listed with their reason; the rest are only counted.
const MAX_LISTED: usize = 200;
/// Time one run takes before it stops at the next block or page, like a long search is bounded.
const BUDGET: Duration = Duration::from_secs(20);
/// Longest replacement text.
const MAX_WITH: usize = 1000;
/// Stands before a space in a run's new text while it is built: the space is where a line of the block ended.
const END: char = '\u{E000}';
/// Stand before and after a replacement in a run's new text while it is built.
const OPEN: char = '\u{E001}';
const CLOSE: char = '\u{E002}';
/// Words, and the characters they are cut from, shown on each side of a skipped match.
const AROUND_WORDS: usize = 3;
const AROUND_CHARS: usize = 40;
/// Slack when placing a find hit (MuPDF's character boxes) in a block's line boxes, in points.
const SLACK: f32 = 3.0;

/// Where a stopped run left off, for the next request's `from`: the page, and on it only the blocks before
/// `upto` are still to do (blocks are replaced from the last to the first; none means all of the page).
/// Pages before it are done: a replacement that holds the find text must not be replaced again.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceFrom {
    pub page: u32,
    pub upto: Option<u32>,
}

/// What one run may use: its time and its replacements (the defaults are the constants; tests shorten them).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub budget: Duration,
    pub max: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { budget: BUDGET, max: MAX_REPLACEMENTS }
    }
}

/// The match to replace alone: the page and the first rectangle of the current hit, as Find listed it.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceAt {
    pub page: u32,
    pub rect: [f32; 4],
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceRequest {
    pub find: String,
    pub with: String,
    /// Case and whole words as Find has them; comments, fields and bookmarks are always left out.
    #[serde(default)]
    pub opts: SearchOpts,
    /// Replace only this match; none replaces every match in the document.
    pub at: Option<ReplaceAt>,
    /// Go on from where a stopped run left off (`Replaced::resume`); only for a Replace all.
    #[serde(default)]
    pub from: Option<ReplaceFrom>,
}

/// A match that was left as it was.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub page: u32,
    pub text: String,
    /// A few words before and after the match (with the space next to it), to tell equal rows apart.
    pub before: String,
    pub after: String,
    /// `skipped: <why>`.
    pub reason: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Replaced {
    pub state: EditState,
    pub replaced: u32,
    /// The first `MAX_LISTED` skipped matches; `skipped_count` is all of them.
    pub skipped: Vec<Skipped>,
    pub skipped_count: u32,
    /// Why the run ended before the end of the document (the cap or the time limit).
    pub stopped: Option<String>,
    /// Where to go on from when it stopped: send it back as `from`.
    pub resume: Option<ReplaceFrom>,
    /// Fonts used instead of a run's own, as Edit text reports them.
    pub fallbacks: Vec<Fallback>,
    /// How many times the find text occurs in the replacement itself: the next match is that many
    /// further on once the search is run again.
    pub inserted: u32,
}

#[derive(Default)]
struct Outcome {
    replaced: u32,
    skipped: Vec<Skipped>,
    skipped_count: u32,
    stopped: Option<String>,
    resume: Option<ReplaceFrom>,
    fallbacks: Vec<Fallback>,
}

impl Outcome {
    fn skip(&mut self, page: u32, text: String, (before, after): (String, String), why: &str) {
        self.skipped_count += 1;
        if self.skipped.len() < MAX_LISTED {
            self.skipped.push(Skipped { page, text, before, after, reason: format!("skipped: {why}") });
        }
    }

    /// A match Find listed, with the words Find has around it.
    fn skip_hit(&mut self, page: u32, h: &search::Hit, why: &str) {
        self.skip(page, h.text.clone(), around(&h.before, &h.after), why);
    }

    /// Match number `k` of a block's text, `t[s..=e]`: as Find listed it when `listed` has its rows (they read
    /// the text as the page has it, with the hyphens the block's text drops), else with the words around it there.
    fn skip_span(&mut self, page: u32, listed: &[&search::Hit], t: &[char], k: usize, (s, e): (usize, usize), why: &str) {
        match listed.get(k) {
            Some(h) => self.skip_hit(page, h, why),
            None => self.skip_in(page, t, s, e, why),
        }
    }

    /// The match `t[s..=e]` of a block's text, with the words around it there.
    fn skip_in(&mut self, page: u32, t: &[char], s: usize, e: usize, why: &str) {
        let (before, after): (String, String) = (t[s.saturating_sub(AROUND_CHARS)..s].iter().collect(), t[e + 1..(e + 1 + AROUND_CHARS).min(t.len())].iter().collect());
        self.skip(page, t[s..=e].iter().collect(), around(&before, &after), why);
    }
}

/// The last words of `before` and the first of `after`, as a skipped row shows them.
fn around(before: &str, after: &str) -> (String, String) {
    let mut b: Vec<&str> = before.split_whitespace().rev().take(AROUND_WORDS).collect();
    b.reverse();
    let (mut b, mut a) = (b.join(" "), after.split_whitespace().take(AROUND_WORDS).collect::<Vec<_>>().join(" "));
    if before.ends_with(char::is_whitespace) && !b.is_empty() {
        b.push(' ');
    }
    if after.starts_with(char::is_whitespace) && !a.is_empty() {
        a.insert(0, ' ');
    }
    (b, a)
}

fn invalid(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// `s` shortened to `n` characters for a step's label.
fn short(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n { format!("{t}…") } else { t }
}

/// Replaces `req.find` with `req.with` in the document (every match, or the one at `req.at`), as one
/// undo step. `Stale` when the document has changed since the matches were listed at `rev`.
pub fn replace(eng: &Engine, doc: DocId, rev: u32, req: &ReplaceRequest, source: EditSource) -> Result<Replaced> {
    replace_within(eng, doc, rev, req, source, Limits::default())
}

/// [`replace`] with the limits of the run given.
pub fn replace_within(eng: &Engine, doc: DocId, rev: u32, req: &ReplaceRequest, source: EditSource, limits: Limits) -> Result<Replaced> {
    let entry = eng.doc_entry(doc)?;
    let opts = SearchOpts { comments: false, bookmarks: false, fields: false, ..req.opts };
    let q = Query::new(&req.find, opts).ok_or_else(|| invalid("Type the text to find first."))?;
    if req.with.chars().count() > MAX_WITH {
        return Err(invalid(format!("The replacement can be at most {MAX_WITH} characters.")));
    }
    if req.with.chars().any(|c| c.is_control()) {
        return Err(invalid("The replacement can't contain a line break or other control characters."));
    }
    let inserted = search::find_spans(&req.with, &q, 100).len() as u32;
    let label = match &req.at {
        Some(a) => format!("Replace text on p. {}", a.page + 1),
        None => format!("Replace \"{}\" with \"{}\"", short(req.find.trim(), 24), short(&req.with, 24)),
    };
    let mut out = Outcome::default();
    let state = eng.apply_custom_unless_noop(doc, label, source, true, |d| {
        if entry.generation() != rev {
            return Err(EngineError::Stale);
        }
        crate::outline::refuse_if_certified(d, "Replacing its text")?;
        let touched = run(d, &q, &req.with, req.at.as_ref(), req.from, inserted, &mut out, limits)?;
        Ok((!touched.is_empty()).then_some(touched))
    })?;
    Ok(Replaced {
        state,
        replaced: out.replaced,
        skipped: out.skipped,
        skipped_count: out.skipped_count,
        stopped: out.stopped,
        resume: out.resume,
        fallbacks: out.fallbacks,
        inserted,
    })
}

/// The time one run may take. It is only checked once something was replaced, so every run gets somewhere
/// ("Replace all again to go on" is then true) and a document with nothing to replace is searched whole.
struct Bounds {
    started: Instant,
    budget: Duration,
    max: usize,
    /// How often the find text occurs in the replacement: a block it is replaced in again would grow.
    inserted: u32,
}

impl Bounds {
    fn over(&self, out: &Outcome) -> bool {
        out.replaced > 0 && self.started.elapsed() > self.budget
    }
}

/// The pages replaced on (0-based).
#[allow(clippy::too_many_arguments)]
fn run(d: &mut PdfDocument, q: &Query, with: &str, at: Option<&ReplaceAt>, from: Option<ReplaceFrom>, inserted: u32, out: &mut Outcome, limits: Limits) -> Result<Vec<u32>> {
    let pages = d.page_count()?.max(0) as u32;
    let from = from.filter(|_| at.is_none());
    let range = match (at, from) {
        (Some(a), _) if a.page >= pages => return Err(EngineError::BadPage(a.page)),
        (_, Some(f)) if f.page >= pages => return Err(EngineError::BadPage(f.page)),
        (Some(a), _) => a.page..a.page + 1,
        (None, Some(f)) => f.page..pages,
        (None, None) => 0..pages,
    };
    let bounds = Bounds { started: Instant::now(), budget: limits.budget, max: limits.max, inserted };
    let mut touched = Vec::new();
    for page in range {
        if out.stopped.is_some() {
            break;
        }
        if bounds.over(out) {
            out.stopped = Some(stopped_at(page, &bounds));
            out.resume = Some(ReplaceFrom { page, upto: None });
            break;
        }
        let upto = from.filter(|f| f.page == page).and_then(|f| f.upto).map(|u| u as usize);
        if replace_page(d, page, q, with, at, upto, out, &bounds)? {
            touched.push(page);
        }
    }
    Ok(touched)
}

fn stopped_at(page: u32, bounds: &Bounds) -> String {
    format!("Stopped at page {} after {} seconds. Replace all again to go on.", page + 1, bounds.budget.as_secs())
}

/// The block (index) whose lines hold every rectangle of a find hit; none when it lies elsewhere or
/// across two blocks.
fn owner(boxes: &[Vec<[f32; 4]>], rects: &[[f32; 4]]) -> Option<usize> {
    let mut found = None;
    for r in rects {
        let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        let b = boxes.iter().position(|ls| ls.iter().any(|l| cx >= l[0] - SLACK && cx <= l[2] + SLACK && cy >= l[1] - SLACK && cy <= l[3] + SLACK))?;
        if found.is_some_and(|f| f != b) {
            return None;
        }
        found = Some(b);
    }
    found
}

/// The block's runs as one string: what `rewrite_block` re-sets, and what the matches index into.
fn joined(b: &Block) -> Vec<char> {
    b.runs.iter().flat_map(|r| r.text.chars()).collect()
}

/// Letters and digits only, lower case: context compared loosely (ligatures and line-end hyphens differ
/// between MuPDF's text and the content stream).
fn loose(s: &[char]) -> Vec<char> {
    s.iter().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Plans and applies the replacements of one page; true when its content changed.
#[allow(clippy::too_many_arguments)]
fn replace_page(d: &mut PdfDocument, page: u32, q: &Query, with: &str, at: Option<&ReplaceAt>, upto: Option<usize>, out: &mut Outcome, bounds: &Bounds) -> Result<bool> {
    let pc = read_page(d, page)?;
    let blocks = group_blocks(&pc.lines);
    let m = page_space(d, page)?;
    let boxes: Vec<Vec<[f32; 4]>> = blocks.iter().map(|b| b.lines.iter().map(|&l| rect32(&m, pc.lines[l].rect)).collect()).collect();
    let room = bounds.max.saturating_sub(out.replaced as usize);
    let text: Vec<Vec<char>> = blocks.iter().map(joined).collect();
    let found: Vec<Vec<(usize, usize)>> = text
        .iter()
        .zip(&blocks)
        .map(|(t, b)| if b.issue == Some(text::Issue::Hidden) { Vec::new() } else { search::find_spans(&t.iter().collect::<String>(), q, room + 1) })
        .collect();
    // What Find lists on this page: the matches that sit in no one block are accounted for as skipped.
    let hits: Vec<search::Hit> = search::find_in_page(d, page, q)?.into_iter().filter(|h| h.source == Source::Text).collect();
    let owners: Vec<Option<usize>> = hits.iter().map(|h| owner(&boxes, &h.rects)).collect();
    let nowhere = |h: &search::Hit| -> &'static str {
        let inside = |a: &text::Area| {
            let r = rect32(&m, a.rect);
            h.rects.first().is_some_and(|f| {
                let (cx, cy) = ((f[0] + f[2]) / 2.0, (f[1] + f[3]) / 2.0);
                cx >= r[0] - SLACK && cx <= r[2] + SLACK && cy >= r[1] - SLACK && cy <= r[3] + SLACK
            })
        };
        match pc.areas.iter().find(|a| inside(a)) {
            Some(a) => issue_text(a.issue),
            None => "it runs across two text blocks, or is not part of a block of text on the page",
        }
    };

    // Find and the block's own text can read a block differently (Find matches across a line-end hyphen the
    // block text has dropped; a whole word at a block's edge on a line that goes on has no hit): the
    // matches then can't be told apart, and the block is left alone.
    let mut mine = vec![0usize; blocks.len()];
    for o in owners.iter().flatten() {
        mine[*o] += 1;
    }
    let uneven = |bi: usize| if found[bi].len() > room { mine[bi] < found[bi].len() } else { mine[bi] != found[bi].len() };
    const UNEVEN: &str = "the block's text reads differently from what Find lists, so its matches can't be placed";
    // Find's rows for a block's matches, when there are as many as the block's text has (the nth is the nth).
    let listed_for = |bi: usize| -> Vec<&search::Hit> {
        let l: Vec<&search::Hit> = hits.iter().zip(&owners).filter(|(_, o)| **o == Some(bi)).map(|(h, _)| h).collect();
        if l.len() == found[bi].len() { l } else { Vec::new() }
    };

    // The matches to replace, per block.
    // (block, its matches, the number of the first of them among the block's matches).
    let mut plan: Vec<(usize, Vec<(usize, usize)>, usize)> = Vec::new();
    match at {
        None => {
            // On a page a run went on from, what is after `upto` was done (or listed) by the run before.
            for (i, h) in hits.iter().enumerate() {
                if owners[i].is_none() && upto.is_none() {
                    out.skip_hit(page, h, nowhere(h));
                }
            }
            for (bi, spans) in found.iter().enumerate().take(upto.unwrap_or(usize::MAX)) {
                match blocks[bi].issue {
                    Some(issue) => {
                        let listed = listed_for(bi);
                        for (k, &sp) in spans.iter().enumerate() {
                            out.skip_span(page, &listed, &text[bi], k, sp, issue_text(issue));
                        }
                    }
                    None if uneven(bi) => {
                        let listed: Vec<&search::Hit> = hits.iter().zip(&owners).filter(|(_, o)| **o == Some(bi)).map(|(h, _)| h).collect();
                        for i in 0..listed.len().max(spans.len()) {
                            match listed.get(i) {
                                Some(h) => out.skip_hit(page, h, UNEVEN),
                                None => out.skip_in(page, &text[bi], spans[i].0, spans[i].1, UNEVEN),
                            }
                        }
                    }
                    None if !spans.is_empty() => plan.push((bi, spans.clone(), 0)),
                    None => {}
                }
            }
        }
        Some(a) => {
            // shortcut: the match is told by its place in Find's list for the page (the nth hit of a block
            // is the nth match in its text), checked against the text around it; upgrade to character
            // geometry if blocks whose lines MuPDF and the content stream order differently show up.
            let near = |h: &search::Hit| h.rects.first().is_some_and(|r| r.iter().zip(a.rect).all(|(x, y)| (x - y).abs() < 0.5));
            let Some(n) = hits.iter().position(near) else { return Err(EngineError::Stale) };
            let h = &hits[n];
            let Some(bi) = owners[n] else {
                out.skip_hit(page, h, nowhere(h));
                return Ok(false);
            };
            if let Some(issue) = blocks[bi].issue {
                out.skip_hit(page, h, issue_text(issue));
                return Ok(false);
            }
            if uneven(bi) {
                out.skip_hit(page, h, UNEVEN);
                return Ok(false);
            }
            let k = owners[..n].iter().filter(|o| **o == Some(bi)).count();
            let Some(&(s, e)) = found[bi].get(k) else {
                out.skip_hit(page, h, "it could not be placed in its block of text; use Replace all");
                return Ok(false);
            };
            let t = &text[bi];
            let (before, after) = (loose(&t[..s]), loose(&t[e + 1..]));
            let (hb, ha) = (loose(&h.before.chars().collect::<Vec<_>>()), loose(&h.after.chars().collect::<Vec<_>>()));
            let (kb, ka) = (before.len().min(8), after.len().min(8));
            if !hb.ends_with(&before[before.len() - kb..]) || !ha.starts_with(&after[..ka]) {
                out.skip_hit(page, h, "it could not be placed in its block of text; use Replace all");
                return Ok(false);
            }
            plan.push((bi, vec![(s, e)], k));
        }
    }

    // From the last block to the first: an edit can only change the grouping of the lines after it.
    let mut changed = false;
    for (bi, mut spans, first) in plan.into_iter().rev() {
        let listed = listed_for(bi);
        let listed = &listed[first.min(listed.len())..];
        let left = bounds.max.saturating_sub(out.replaced as usize);
        let cut = spans.len() > left;
        if cut {
            out.stopped = Some(format!("Stopped after {} replacements. Replace all again to go on.", bounds.max));
            if out.replaced == 0 {
                // One block that alone is over the cap: part replaced, and the rest listed (a second run could
                // not tell its own work from the document's where the replacement holds the find text).
                for (k, &sp) in spans.iter().enumerate().skip(left) {
                    out.skip_span(page, listed, &text[bi], k, sp, &format!("the block has more matches than the {} one run replaces", bounds.max));
                }
                out.resume = Some(ReplaceFrom { page, upto: Some(bi as u32) });
            } else if bounds.inserted > 0 {
                // A cut block would be part replaced, and replaced again from its start by the next run: it is
                // left whole, and the next run starts with it.
                out.resume = Some(ReplaceFrom { page, upto: Some(bi as u32 + 1) });
                break;
            } else {
                out.resume = Some(ReplaceFrom { page, upto: Some(bi as u32 + 1) });
            }
            spans.truncate(left);
        }
        if spans.is_empty() {
            break;
        }
        // Rewriting a block drops its line-end hyphens (the block text joins "well-" "known" as "wellknown"),
        // so a block that has one is left alone unless a match, one that is still replaced, covers it.
        if blocks[bi].joins.iter().any(|&h| !spans.iter().any(|&(s, e)| s < h && e >= h)) {
            for (k, &sp) in spans.iter().enumerate() {
                out.skip_span(page, listed, &text[bi], k, sp, "a hyphen at the end of a line in the block would be lost");
            }
            if cut {
                break;
            }
            continue;
        }
        // Each block is rewritten whole, so stopping between two is safe (every re-read costs the page's parse).
        if bounds.over(out) {
            out.stopped = Some(stopped_at(page, bounds));
            out.resume = Some(ReplaceFrom { page, upto: Some(bi as u32 + 1) });
            break;
        }
        changed |= apply_block(d, page, bi, &blocks[bi], &spans, with, listed, out);
        if cut {
            break;
        }
    }
    Ok(changed)
}

/// Rewrites one block with `spans` of its text replaced by `with`. Runs the spans don't touch keep their
/// glyphs; a span across runs puts the replacement in the style of the run it starts in. The block's lines
/// stay where they are (see [`TextChange::keep`]). A refusal skips the block's matches and leaves the
/// page as it was. True when the block was rewritten.
#[allow(clippy::too_many_arguments)]
fn apply_block(d: &mut PdfDocument, page: u32, index: usize, b: &Block, spans: &[(usize, usize)], with: &str, listed: &[&search::Hit], out: &mut Outcome) -> bool {
    let all: Vec<char> = joined(b);
    let skip_all = |out: &mut Outcome, why: &str| {
        for (k, &sp) in spans.iter().enumerate() {
            out.skip_span(page, listed, &all, k, sp, why);
        }
    };
    let change = match keep_change(b, spans, &vec![with; spans.len()], false) {
        Ok(c) => c,
        Err(why) => {
            skip_all(out, &why);
            return false;
        }
    };
    match text::rewrite_block_expecting(d, page, index, &change, Some(&b.text)) {
        Ok(fallbacks) => {
            out.replaced += spans.len() as u32;
            for f in fallbacks {
                if !out.fallbacks.iter().any(|g| g.font == f.font && g.reason == f.reason) {
                    out.fallbacks.push(f);
                }
            }
            true
        }
        Err(e) => {
            // A refused rewrite may have named a font on the page before it stopped: tidy that away.
            let _ = super::layout::forget_unused_fonts(d, page);
            let why = match e {
                EngineError::Invalid(m) => m,
                e => e.to_string(),
            };
            skip_all(out, &why);
            false
        }
    }
}

/// The change that sets block `b` again with `with[i]` in place of each span `spans[i]` (inclusive character
/// ranges of the block's runs text) and its lines kept (see [`TextChange::keep`]): the runs the spans don't
/// touch are as they were, the line ends are marked in the text and found again in the new text's coordinates.
/// With `wrap` (Edit text) a changed line may grow past the block's width and a replacement may hold line
/// breaks; Find & Replace has neither. `Err` is the reason the lines can't be kept.
pub(super) fn keep_change(b: &Block, spans: &[(usize, usize)], with: &[&str], wrap: bool) -> std::result::Result<TextChange, String> {
    let all: Vec<char> = joined(b);
    // Each space that stood for a line end (not one a match swallows) is marked in the text it is in with
    // the line that follows it, and so is each replacement; the marks are found again in the new text's
    // coordinates. The text itself is kept as it is (double spaces, no-break spaces).
    let cant = "the lines of the block can't be kept where they are";
    let marks = [END, OPEN, CLOSE];
    if all.iter().any(|c| marks.contains(c)) || with.iter().any(|w| w.chars().any(|c| marks.contains(&c))) || b.breaks.iter().any(|&(p, _)| all.get(p) != Some(&' ')) {
        return Err(cant.into());
    }
    // A match that covers a dropped hyphen takes in the line after it, so its words after the match would
    // join the first line: when a space follows the match, that space is where the second line goes on. A
    // match that stops inside the word has no such place, and the block is left as it is.
    let mut breaks = b.breaks.clone();
    let mut forced = Vec::new();
    for (&j, &n) in b.joins.iter().zip(&b.join_lines) {
        if let Some(&(_, e)) = spans.iter().find(|&&(s, e)| s < j && e >= j) {
            if breaks.iter().any(|&(p, _)| p == e + 1) || e + 1 >= all.len() {
                continue;
            }
            if all[e + 1] != ' ' {
                return Err("the match ends inside a word that a hyphen at the end of a line splits".into());
            }
            breaks.push((e + 1, n));
            forced.push(n);
        }
    }
    breaks.sort_by_key(|&(p, _)| p);
    let mut specs = Vec::new();
    let (mut at, mut si) = (0usize, 0usize);
    let (mut ends, mut changed, mut now): (Vec<(usize, usize)>, Vec<(usize, usize)>, usize) = (Vec::new(), Vec::new(), 0);
    for (ri, run) in b.runs.iter().enumerate() {
        let (mut t, mut hit) = (String::new(), false);
        let mut srcs = Vec::new();
        let n = run.text.chars().count();
        for (ci, c) in run.text.chars().enumerate() {
            let g = at + ci;
            while si < spans.len() && spans[si].1 < g {
                si += 1;
            }
            match spans.get(si) {
                Some(&(s, e)) if g >= s && g <= e => {
                    hit = true;
                    if g == s {
                        // Where it meets a space of the text, the replacement's own is not one more.
                        let mut w = with[si];
                        if s > 0 && all[s - 1].is_whitespace() {
                            w = w.trim_start();
                        }
                        if all.get(e + 1).is_some_and(|c| c.is_whitespace()) {
                            w = w.trim_end();
                        }
                        t.push(OPEN);
                        t.push_str(w);
                        t.push(CLOSE);
                    }
                }
                _ => {
                    if let Some(&(_, src)) = breaks.iter().find(|&&(p, _)| p == g) {
                        t.push(END);
                        srcs.push(src);
                    }
                    t.push(c);
                }
            }
        }
        if !hit {
            ends.extend(breaks.iter().filter(|&&(p, _)| p >= at && p < at + n).map(|&(p, src)| (now + p - at, src)));
            now += n;
            specs.push(RunChange { source: Some(ri), ..Default::default() });
        } else {
            // The text as the rewrite will read it: the marks go, their places stay. A space just before a
            // line end goes (a word replaced by nothing at a line's end), and so does one of the two that a
            // replacement by nothing would leave side by side.
            let (mut plain, mut srcs, mut open) = (String::new(), srcs.into_iter(), 0usize);
            let mut chars = t.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    END => {
                        if plain.ends_with(' ') {
                            plain.pop();
                            let len = now + plain.chars().count();
                            for r in changed.iter_mut() {
                                *r = (r.0.min(len), r.1.min(len));
                            }
                        }
                        ends.push((now + plain.chars().count(), srcs.next().expect("a line for each mark")));
                    }
                    OPEN => open = plain.chars().count(),
                    CLOSE => {
                        let end = plain.chars().count();
                        changed.push((now + open, now + end));
                        if end == open && plain.ends_with(char::is_whitespace) && chars.peek().is_some_and(|c| c.is_whitespace()) {
                            chars.next();
                        }
                    }
                    c => plain.push(c),
                }
            }
            now += plain.chars().count();
            if !plain.is_empty() {
                specs.push(RunChange { text: Some(plain), source: Some(ri), ..Default::default() });
            }
        }
        at += n;
    }
    // The line a replacement took the first word of is no longer as it was, whichever run its end is in.
    for &(pos, src) in &ends {
        if forced.contains(&src) {
            changed.push((pos, pos + 1));
        }
    }
    // Two ends in one place (the words between them were replaced by nothing): the later line is the one that
    // stays; the gap the emptied line leaves is kept.
    ends.dedup_by(|a, b| {
        let same = a.0 == b.0;
        if same {
            b.1 = a.1;
        }
        same
    });
    if specs.is_empty() {
        return Err("the replacement would leave no text in the block; delete the block's text with Edit text".into());
    }
    Ok(TextChange { runs: Some(specs), keep: Some(KeepLines { ends, changed, wrap }), ..Default::default() })
}

/// Most characters of a block's text that Edit text compares with what was typed; a longer block is set as a whole.
const MAX_EDIT_CHARS: usize = 20_000;

/// Edit text on a block of two or more lines, as the change that keeps its lines: `new` is the typed text, a
/// single paragraph in which the block's lines run together with a space between them. It is compared with the
/// block's words, each changed stretch of words is a replacement (in the style of the run it starts in, see
/// [`keep_change`]) and every line it doesn't touch stays where it was. A stretch across a line end joins those
/// two lines; a typed line break starts a new line.
///
/// None when the lines can't be told from the text, and the block is set as a whole (its lines wrapped to its
/// widest, as before): a block of one line (it grows to the margin), a block that holds a line-end hyphen the
/// edit doesn't cover, whose runs text is not the text the user was shown, whose lines have their own character
/// spacing or scale, an edit that rewrites most of the words (a paragraph typed over, which reflows), or none.
pub(super) fn keep_edit(b: &Block, new: &str) -> Option<TextChange> {
    let old = joined(b);
    if b.lines.len() < 2 || old.len() > MAX_EDIT_CHARS || b.text.chars().ne(old.iter().copied()) {
        return None;
    }
    let new: Vec<char> = new.replace("\r\n", "\n").chars().collect();
    if new.len() > MAX_EDIT_CHARS {
        return None;
    }
    let (ow, nw) = (words(&old), words(&new));
    if ow.is_empty() || nw.is_empty() {
        return None;
    }
    let text = |s: &[char], (a, z): (usize, usize)| s[a..z].iter().collect::<String>();
    let (os, ns): (Vec<String>, Vec<String>) = (ow.iter().map(|&r| text(&old, r)).collect(), nw.iter().map(|&r| text(&new, r)).collect());
    // What stands between word `i` and the next, in the old text and in the new.
    let sep_old = |i: usize| &old[ow[i].1..ow[i + 1].0];
    let sep_new = |j: usize| &new[nw[j].1..nw[j + 1].0];
    // Changed stretches as (old words a..b, new words c..d).
    let mut hunks: Vec<[usize; 4]> = Vec::new();
    let mut matched = 0;
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &os, &ns) {
        match op {
            similar::DiffOp::Equal { old_index, new_index, len } => {
                matched += len;
                // Words that are the same but not what stands between them: a line break typed, or spaces.
                for k in 0..len.saturating_sub(1) {
                    if sep_old(old_index + k) != sep_new(new_index + k) {
                        hunks.push([old_index + k, old_index + k + 2, new_index + k, new_index + k + 2]);
                    }
                }
            }
            op => {
                let (o, n) = (op.old_range(), op.new_range());
                // A deletion and an insertion that follow one another are one replacement.
                match hunks.last_mut() {
                    Some(h) if h[1] == o.start && h[3] == n.start => (h[1], h[3]) = (o.end, n.end),
                    _ => hunks.push([o.start, o.end, n.start, n.end]),
                }
            }
        }
    }
    // Most of the words are new (or most are gone): a paragraph typed over, which is laid out again whole.
    if hunks.is_empty() || matched * 2 < ow.len().min(nw.len()) {
        return None;
    }
    for h in &mut hunks {
        let [mut a, mut b, mut c, mut d] = *h;
        // A stretch that only adds words is set with the word it follows, so the line end that is after it
        // stays after the words (they are on the line they were typed in).
        if a == b {
            if a > 0 && c > 0 {
                (a, c) = (a - 1, c - 1);
            } else if b < ow.len() && d < nw.len() {
                (b, d) = (b + 1, d + 1);
            } else {
                return None;
            }
        }
        // Where the space before or after the stretch is not the one that was there, the word beyond is set too.
        if a > 0 && c > 0 && sep_old(a - 1) != sep_new(c - 1) {
            (a, c) = (a - 1, c - 1);
        }
        if b < ow.len() && d < nw.len() && sep_old(b - 1) != sep_new(d - 1) {
            (b, d) = (b + 1, d + 1);
        }
        *h = [a, b, c, d];
    }
    hunks.sort_unstable();
    let mut merged: Vec<[usize; 4]> = Vec::new();
    for h in hunks {
        match merged.last_mut() {
            Some(m) if h[0] <= m[1] => (m[1], m[3]) = (m[1].max(h[1]), m[3].max(h[3])),
            _ => merged.push(h),
        }
    }
    let spans: Vec<(usize, usize)> = merged.iter().map(|h| (ow[h[0]].0, ow[h[1] - 1].1 - 1)).collect();
    // A line-end hyphen is dropped by the rewrite: only an edit that covers it may go through.
    if b.joins.iter().any(|&j| !spans.iter().any(|&(s, e)| s < j && e >= j)) {
        return None;
    }
    let with: Vec<String> = merged.iter().map(|h| if h[2] == h[3] { String::new() } else { text(&new, (nw[h[2]].0, nw[h[3] - 1].1)) }).collect();
    keep_change(b, &spans, &with.iter().map(String::as_str).collect::<Vec<_>>(), true).ok()
}

/// A change that sets the block's own text again (a move, a colour, bold, underline, ...) as the change that
/// keeps its lines: every line without a restyled run is left exactly as it was, and a restyled line keeps its
/// place (and may wrap if it got wider, see [`KeepLines::wrap`]). None when `change` has text, a size, a width,
/// a line spacing or an alignment (the lines are laid out again), or when the lines can't be kept (see
/// [`keep_edit`]); the block is then set as a whole.
pub(super) fn keep_format(b: &Block, change: &TextChange) -> Option<TextChange> {
    let old = joined(b);
    if b.lines.len() < 2
        || !b.joins.is_empty()
        || old.len() > MAX_EDIT_CHARS
        || change.text.is_some()
        || change.size.is_some()
        || change.width.is_some()
        || change.line_spacing.is_some()
        || change.align.is_some()
        || b.breaks.iter().any(|&(p, _)| old.get(p) != Some(&' '))
    {
        return None;
    }
    let runs: Vec<RunChange> = match &change.runs {
        Some(r) => r.clone(),
        None => (0..b.runs.len()).map(|i| RunChange { source: Some(i), ..Default::default() }).collect(),
    };
    let block_style = change.color.is_some() || change.bold.is_some() || change.italic.is_some() || change.underline.is_some() || change.strike.is_some() || change.rise.is_some();
    let (mut at, mut changed, mut same) = (0usize, Vec::new(), old.iter());
    for rc in &runs {
        let src = b.runs.get(rc.source?)?;
        let t = rc.text.as_deref().unwrap_or(&src.text);
        let n = t.chars().count();
        // The runs set the block's own text again, only their style may differ.
        if !t.chars().all(|c| same.next() == Some(&c)) {
            return None;
        }
        let styled = block_style
            || rc.font.is_some()
            || rc.size.is_some()
            || rc.color.is_some()
            || rc.bold.is_some()
            || rc.italic.is_some()
            || rc.underline.is_some()
            || rc.strike.is_some()
            || rc.rise.is_some();
        if rc.size.is_some() {
            return None;
        }
        if styled {
            changed.push((at, at + n));
        }
        at += n;
    }
    if same.next().is_some() {
        return None;
    }
    Some(TextChange { runs: Some(runs), keep: Some(KeepLines { ends: b.breaks.clone(), changed, wrap: true }), ..change.clone() })
}

/// The words of `s` as character ranges: runs of what is not a space.
fn words(s: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = None;
    for (i, &c) in s.iter().enumerate() {
        match (text::paragraph_separator(c), from) {
            (false, None) => from = Some(i),
            (true, Some(a)) => {
                out.push((a, i));
                from = None;
            }
            _ => {}
        }
    }
    if let Some(a) = from {
        out.push((a, s.len()));
    }
    out
}
