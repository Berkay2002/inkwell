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

use super::text::{self, collapse, group_blocks, issue_text, read_page, Block, Fallback, RunChange, TextChange};
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

    // The matches to replace, per block.
    let mut plan: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
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
                        for &(a, z) in spans {
                            out.skip_in(page, &text[bi], a, z, issue_text(issue));
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
                    None if !spans.is_empty() => plan.push((bi, spans.clone())),
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
            plan.push((bi, vec![(s, e)]));
        }
    }

    // From the last block to the first: an edit can only change the grouping of the lines after it.
    let mut changed = false;
    for (bi, mut spans) in plan.into_iter().rev() {
        let left = bounds.max.saturating_sub(out.replaced as usize);
        let cut = spans.len() > left;
        if cut {
            out.stopped = Some(format!("Stopped after {} replacements. Replace all again to go on.", bounds.max));
            if out.replaced == 0 {
                // One block that alone is over the cap: part replaced, and the rest listed (a second run could
                // not tell its own work from the document's where the replacement holds the find text).
                for &(s, e) in &spans[left..] {
                    out.skip_in(page, &text[bi], s, e, &format!("the block has more matches than the {} one run replaces", bounds.max));
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
            for &(s, e) in &spans {
                out.skip_in(page, &text[bi], s, e, "a hyphen at the end of a line in the block would be lost");
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
        changed |= apply_block(d, page, bi, &blocks[bi], &spans, with, out);
        if cut {
            break;
        }
    }
    Ok(changed)
}

/// Rewrites one block with `spans` of its text replaced by `with`. Runs the spans don't touch keep their
/// glyphs; a span across runs puts the replacement in the style of the run it starts in. A refusal skips
/// the block's matches and leaves the page as it was. True when the block was rewritten.
fn apply_block(d: &mut PdfDocument, page: u32, index: usize, b: &Block, spans: &[(usize, usize)], with: &str, out: &mut Outcome) -> bool {
    let all: Vec<char> = joined(b);
    let skip_all = |out: &mut Outcome, why: &str| {
        for &(s, e) in spans {
            out.skip_in(page, &all, s, e, why);
        }
    };
    let mut specs = Vec::new();
    let (mut at, mut si) = (0usize, 0usize);
    // The lines of the block stay as they were: each space that stood for a line end (not one a match
    // swallows) is marked in the text it is in, and found again in the new text's coordinates.
    let ends: Vec<usize> = b.breaks.iter().copied().filter(|&p| all.get(p) == Some(&' ')).collect();
    let (mut line_ends, mut now) = (Vec::new(), 0usize);
    for (ri, run) in b.runs.iter().enumerate() {
        let (mut t, mut hit) = (String::new(), false);
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
                        t.push_str(with);
                    }
                }
                _ => {
                    if ends.contains(&g) {
                        t.push(END);
                    }
                    t.push(c);
                }
            }
        }
        if !hit {
            line_ends.extend(ends.iter().filter(|&&p| p >= at && p < at + n).map(|p| now + p - at));
            now += n;
            specs.push(RunChange { source: Some(ri), ..Default::default() });
        } else {
            // The text as the rewrite will read it (spaces collapsed): the marks go, their places stay. A
            // replacement that ends in a space against a line end would be collapsed by the rewrite and
            // shift what follows: that run's lines are left to reflow.
            let t = collapse(&t);
            let plain: String = t.chars().filter(|&c| c != END).collect();
            let kept = collapse(&plain) == plain;
            let mut k = 0;
            for c in t.chars() {
                if c == END {
                    if kept {
                        line_ends.push(now + k);
                    }
                } else {
                    k += 1;
                }
            }
            let plain = collapse(&plain);
            now += plain.chars().count();
            if !plain.is_empty() {
                specs.push(RunChange { text: Some(plain), source: Some(ri), ..Default::default() });
            }
        }
        at += n;
    }
    if specs.is_empty() {
        skip_all(out, "the replacement would leave no text in the block; delete the block's text with Edit text");
        return false;
    }
    let change = TextChange { runs: Some(specs), line_ends, ..Default::default() };
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
