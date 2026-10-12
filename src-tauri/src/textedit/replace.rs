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

use super::text::{self, group_blocks, issue_text, read_page, Block, Fallback, RunChange, TextChange};
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
/// Time one run takes before it stops at a page boundary, like a long search is bounded.
const BUDGET: Duration = Duration::from_secs(20);
/// Longest replacement text.
const MAX_WITH: usize = 1000;
/// Slack when placing a find hit (MuPDF's character boxes) in a block's line boxes, in points.
const SLACK: f32 = 3.0;

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
}

/// A match that was left as it was.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub page: u32,
    pub text: String,
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
    fallbacks: Vec<Fallback>,
}

impl Outcome {
    fn skip(&mut self, page: u32, text: String, why: &str) {
        self.skipped_count += 1;
        if self.skipped.len() < MAX_LISTED {
            self.skipped.push(Skipped { page, text, reason: format!("skipped: {why}") });
        }
    }
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
        let touched = run(d, &q, &req.with, req.at.as_ref(), &mut out)?;
        Ok((!touched.is_empty()).then_some(touched))
    })?;
    Ok(Replaced {
        state,
        replaced: out.replaced,
        skipped: out.skipped,
        skipped_count: out.skipped_count,
        stopped: out.stopped,
        fallbacks: out.fallbacks,
        inserted,
    })
}

/// The pages replaced on (0-based).
fn run(d: &mut PdfDocument, q: &Query, with: &str, at: Option<&ReplaceAt>, out: &mut Outcome) -> Result<Vec<u32>> {
    let pages = d.page_count()?.max(0) as u32;
    let range = match at {
        Some(a) if a.page >= pages => return Err(EngineError::BadPage(a.page)),
        Some(a) => a.page..a.page + 1,
        None => 0..pages,
    };
    let started = Instant::now();
    let mut touched = Vec::new();
    for page in range {
        if out.replaced as usize >= MAX_REPLACEMENTS {
            out.stopped = Some(format!("Stopped after {MAX_REPLACEMENTS} replacements. Search again to replace the rest."));
            break;
        }
        if started.elapsed() > BUDGET {
            out.stopped = Some(format!("Stopped at page {} after {} seconds. Replace all again to go on.", page + 1, BUDGET.as_secs()));
            break;
        }
        if replace_page(d, page, q, with, at, out)? {
            touched.push(page);
        }
    }
    Ok(touched)
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
fn replace_page(d: &mut PdfDocument, page: u32, q: &Query, with: &str, at: Option<&ReplaceAt>, out: &mut Outcome) -> Result<bool> {
    let pc = read_page(d, page)?;
    let blocks = group_blocks(&pc.lines);
    let m = page_space(d, page)?;
    let boxes: Vec<Vec<[f32; 4]>> = blocks.iter().map(|b| b.lines.iter().map(|&l| rect32(&m, pc.lines[l].rect)).collect()).collect();
    let room = MAX_REPLACEMENTS.saturating_sub(out.replaced as usize);
    let text: Vec<Vec<char>> = blocks.iter().map(joined).collect();
    let found: Vec<Vec<(usize, usize)>> = text
        .iter()
        .zip(&blocks)
        .map(|(t, b)| if b.issue == Some(text::Issue::Hidden) { Vec::new() } else { search::find_spans(&t.iter().collect::<String>(), q, room.max(1)) })
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

    // The matches to replace, per block.
    let mut plan: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
    match at {
        None => {
            for (i, h) in hits.iter().enumerate() {
                if owners[i].is_none() {
                    out.skip(page, h.text.clone(), nowhere(h));
                }
            }
            for (bi, spans) in found.iter().enumerate() {
                if spans.is_empty() {
                    continue;
                }
                match blocks[bi].issue {
                    Some(issue) => {
                        for &(a, z) in spans {
                            out.skip(page, text[bi][a..=z].iter().collect(), issue_text(issue));
                        }
                    }
                    None => plan.push((bi, spans.clone())),
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
                out.skip(page, h.text.clone(), nowhere(h));
                return Ok(false);
            };
            if let Some(issue) = blocks[bi].issue {
                out.skip(page, h.text.clone(), issue_text(issue));
                return Ok(false);
            }
            let k = owners[..n].iter().filter(|o| **o == Some(bi)).count();
            let Some(&(s, e)) = found[bi].get(k) else {
                out.skip(page, h.text.clone(), "it could not be placed in its block of text; use Replace all");
                return Ok(false);
            };
            let t = &text[bi];
            let (before, after) = (loose(&t[..s]), loose(&t[e + 1..]));
            let (hb, ha) = (loose(&h.before.chars().collect::<Vec<_>>()), loose(&h.after.chars().collect::<Vec<_>>()));
            let (kb, ka) = (before.len().min(8), after.len().min(8));
            if !hb.ends_with(&before[before.len() - kb..]) || !ha.starts_with(&after[..ka]) {
                out.skip(page, h.text.clone(), "it could not be placed in its block of text; use Replace all");
                return Ok(false);
            }
            plan.push((bi, vec![(s, e)]));
        }
    }

    // From the last block to the first: an edit can only change the grouping of the lines after it.
    let mut changed = false;
    for (bi, mut spans) in plan.into_iter().rev() {
        let left = MAX_REPLACEMENTS.saturating_sub(out.replaced as usize);
        if left == 0 {
            out.stopped = Some(format!("Stopped after {MAX_REPLACEMENTS} replacements. Search again to replace the rest."));
            break;
        }
        spans.truncate(left);
        changed |= apply_block(d, page, bi, &blocks[bi], &spans, with, out);
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
            out.skip(page, all[s..=e].iter().collect(), why);
        }
    };
    let mut specs = Vec::new();
    let (mut at, mut si) = (0usize, 0usize);
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
                _ => t.push(c),
            }
        }
        at += n;
        if !hit {
            specs.push(RunChange { source: Some(ri), ..Default::default() });
        } else if !t.is_empty() {
            specs.push(RunChange { text: Some(t), source: Some(ri), ..Default::default() });
        }
    }
    if specs.is_empty() {
        skip_all(out, "the replacement would leave no text in the block; delete the block's text with Edit text");
        return false;
    }
    let change = TextChange { runs: Some(specs), ..Default::default() };
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
