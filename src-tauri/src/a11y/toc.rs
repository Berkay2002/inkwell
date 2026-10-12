//! Table of contents detection for Autotag, from structured-text lines only. An entry is one row of
//! the page (lines whose baselines are within `BASELINE_TOL`) that ends in a page number, arabic or
//! roman, set apart from its title by a dot leader or by a wide gap. A run of at least three entries
//! one under the other, whose page numbers line up and never go back, is a table of contents.
//!
//! It is conservative on purpose: a false TOC is worse than paragraphs. A row that is anything else
//! (a heading, a wrapped title, a second column of entries) ends the run, so those pages stay
//! paragraphs. Tables are found first and keep their cells: a ruled two-column list is a table.

use super::tables::{ALIGN_TOL, BASELINE_TOL, Item};
use mupdf::Rect;

/// Entries needed for a table of contents.
pub const MIN_ENTRIES: usize = 3;
const MAX_ENTRIES: usize = 400;
/// A page number this far (points, and two font sizes) from its title needs no dot leader.
const GAP_MIN: f32 = 24.0;
/// Leader dots needed between a title and its page number.
const LEADER_DOTS: usize = 3;
/// An entry starts at most this many line heights below the one before it.
const SPACING: f32 = 1.5;

#[derive(Debug, PartialEq)]
pub struct Toc {
    /// The `Item::id` of each line, entry by entry, left to right.
    pub rows: Vec<Vec<usize>>,
}

/// An arabic page number (1 to 4 digits).
fn arabic(s: &str) -> Option<u32> {
    (!s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten().filter(|&n| n > 0)
}

const ROMAN: [(u32, &str); 9] = [(100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];

/// A roman page number, all lower or all upper case, in its canonical spelling, below 400: "iv" and "XII", not
/// "iiii", and not words such as "mix" or "did" (only i, v, x, l and c count).
pub fn roman(s: &str) -> Option<u32> {
    let one = |set: &[char]| !s.is_empty() && s.len() <= 8 && s.chars().all(|c| set.contains(&c));
    if !(one(&['i', 'v', 'x', 'l', 'c']) || one(&['I', 'V', 'X', 'L', 'C'])) {
        return None;
    }
    let l = s.to_ascii_lowercase();
    let val = |c: char| match c {
        'i' => 1,
        'v' => 5,
        'x' => 10,
        'l' => 50,
        _ => 100,
    };
    let v: Vec<i32> = l.chars().map(val).collect();
    let n: i32 = v.iter().enumerate().map(|(i, &x)| if v.get(i + 1).is_some_and(|&y| y > x) { -x } else { x }).sum();
    let mut rest = n.max(0) as u32;
    let mut canon = String::new();
    for (value, text) in ROMAN {
        while rest >= value {
            canon += text;
            rest -= value;
        }
    }
    (n > 0 && canon == l).then_some(n as u32)
}

/// Where a page number sorts: roman front matter comes before arabic pages.
fn page_key(s: &str) -> Option<(u8, u32)> {
    arabic(s).map(|n| (1, n)).or_else(|| roman(s).map(|n| (0, n)))
}

fn is_leader_char(c: char) -> bool {
    matches!(c, '.' | '\u{2026}' | '\u{b7}') || c.is_whitespace()
}

/// Dots in a leader (an ellipsis counts as three).
fn dots(s: &str) -> usize {
    s.chars().map(|c| match c {
        '.' | '\u{b7}' => 1,
        '\u{2026}' => 3,
        _ => 0,
    }).sum()
}

/// A line that is nothing but the leader between a title and its page number.
fn leader_only(s: &str) -> bool {
    s.chars().all(is_leader_char) && dots(s) >= LEADER_DOTS
}

/// `title ..... 12`: the title and the page number, when the text ends in a leader and a page number.
fn split_leader(text: &str) -> Option<(&str, &str)> {
    let t = text.trim();
    let cut = t.rfind(is_leader_char).map(|i| i + t[i..].chars().next().map_or(1, char::len_utf8))?;
    let (head, page) = t.split_at(cut);
    page_key(page)?;
    let title = head.trim_end_matches(is_leader_char);
    (dots(&head[title.len()..]) >= LEADER_DOTS).then_some((title, page))
}

/// A section number in front of a title: `1`, `2.3`, `A`, `IV.`.
fn section_number(s: &str) -> bool {
    let t = s.trim().trim_end_matches('.');
    !t.is_empty()
        && t.len() <= 8
        && (t.split('.').all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
            || (t.len() == 1 && t.bytes().all(|b| b.is_ascii_uppercase()))
            || roman(t).is_some())
}

struct Entry {
    /// Indices into the items, left to right.
    cells: Vec<usize>,
    y0: f32,
    y1: f32,
    key: (u8, u32),
    /// Right edge of the page number.
    right: f32,
    /// Left edge of the page number, when it is a line of its own.
    left: Option<f32>,
}

fn has_letter(s: &str) -> bool {
    s.chars().any(char::is_alphabetic)
}

/// The entry a row of lines makes, if it is one.
fn entry(items: &[Item], row: &[usize]) -> Option<Entry> {
    // A leader set as a line of its own is left out of the row (and says the row has a leader).
    let body: Vec<usize> = row.iter().copied().filter(|&i| !leader_only(items[i].text)).collect();
    let mut leader = body.len() != row.len();
    let last = &items[*body.last()?];
    let (page, left) = match body.len() {
        // `title ..... 12` in one line.
        1 => {
            let (title, page) = split_leader(last.text)?;
            has_letter(title).then_some((page, None))?
        }
        // The page number is a line of its own, after the title (and a section number).
        2 | 3 => {
            let title = &items[body[body.len() - 2]];
            let bare = title.text.trim_end_matches(is_leader_char);
            leader |= dots(&title.text[bare.len()..]) >= LEADER_DOTS;
            let wide = last.bbox.x0 - title.bbox.x1 >= GAP_MIN.max(2.0 * title.size);
            let numbered = body.len() == 2 || section_number(items[body[0]].text);
            (has_letter(bare) && numbered && (leader || wide)).then_some((last.text.trim(), Some(last.bbox.x0)))?
        }
        _ => return None,
    };
    let key = page_key(page)?;
    let (y0, y1) = row.iter().fold((f32::MAX, f32::MIN), |(a, b), &i| (a.min(items[i].bbox.y0), b.max(items[i].bbox.y1)));
    Some(Entry { cells: row.to_vec(), y0, y1, key, right: last.bbox.x1, left })
}

fn spread(it: impl Iterator<Item = f32>) -> f32 {
    let (lo, hi) = it.fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    hi - lo
}

/// The tables of contents among `items` (the caller leaves out the cells of tables). `widgets` are the
/// form fields' boxes: text near one is a form.
pub fn detect(items: &[Item], widgets: &[Rect]) -> Vec<Toc> {
    let mut found = Vec::new();
    if items.len() < MIN_ENTRIES {
        return found;
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[a].baseline.total_cmp(&items[b].baseline).then(items[a].bbox.x0.total_cmp(&items[b].bbox.x0)));
    // Rows by baseline.
    let mut rows: Vec<(f32, Vec<usize>)> = Vec::new();
    for i in order {
        match rows.last_mut() {
            Some((base, cells)) if (items[i].baseline - *base).abs() <= BASELINE_TOL => cells.push(i),
            _ => rows.push((items[i].baseline, vec![i])),
        }
    }
    let mut run: Vec<Entry> = Vec::new();
    let mut flush = |run: &mut Vec<Entry>| {
        if let Some(t) = accept(items, run, widgets) {
            found.push(t);
        }
        run.clear();
    };
    for (_, cells) in &rows {
        match entry(items, cells) {
            Some(e) => {
                let near = run.last().is_none_or(|p| e.y0 - p.y1 <= SPACING * (p.y1 - p.y0).max(e.y1 - e.y0));
                if !near {
                    flush(&mut run);
                }
                run.push(e);
            }
            None => flush(&mut run),
        }
    }
    flush(&mut run);
    found
}

/// A run of entries, as a table of contents when it holds together.
fn accept(items: &[Item], run: &[Entry], widgets: &[Rect]) -> Option<Toc> {
    if run.len() < MIN_ENTRIES || run.len() > MAX_ENTRIES {
        return None;
    }
    // The page numbers are one column: right edges line up, or the left edges of numbers set apart do.
    let right = spread(run.iter().map(|e| e.right)) <= ALIGN_TOL;
    let left = run.iter().all(|e| e.left.is_some()) && spread(run.iter().filter_map(|e| e.left)) <= ALIGN_TOL;
    if !(right || left) || run.windows(2).any(|w| w[1].key < w[0].key) {
        return None;
    }
    // Drawn entry by entry, each line whole before the next: tag order and content order then agree.
    let cells: Vec<&Item> = run.iter().flat_map(|e| e.cells.iter().map(|&i| &items[i])).collect();
    if cells.windows(2).any(|w| w[0].runs.1 >= w[1].runs.0) {
        return None;
    }
    let (x0, x1) = cells.iter().fold((f32::MAX, f32::MIN), |(a, b), c| (a.min(c.bbox.x0), b.max(c.bbox.x1)));
    let (y0, y1) = (run[0].y0, run[run.len() - 1].y1);
    if widgets.iter().any(|w| w.x0 < x1 && w.x1 > x0 && w.y0 < y1 && w.y1 > y0) {
        return None;
    }
    Some(Toc { rows: run.iter().map(|e| e.cells.iter().map(|&i| items[i].id).collect()).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roman_numbers_are_strict() {
        for (s, n) in [("i", 1), ("iv", 4), ("xii", 12), ("XIV", 14), ("xl", 40), ("xcix", 99), ("cc", 200)] {
            assert_eq!(roman(s), Some(n), "{s}");
        }
        // Words, other spellings, mixed case, and numerals past 399.
        for s in ["", "mix", "did", "mild", "iiii", "vx", "iix", "Iv", "civil", "cd", "m", "ic", "xxxx"] {
            assert_eq!(roman(s), None, "{s}");
        }
    }

    #[test]
    fn page_numbers_come_after_a_leader_or_a_gap() {
        assert_eq!(split_leader("Introduction ......... 12"), Some(("Introduction", "12")));
        assert_eq!(split_leader("Introduction.........xii"), Some(("Introduction", "xii")));
        assert_eq!(split_leader("Chapter 1 . . . . 7"), Some(("Chapter 1", "7")));
        assert_eq!(split_leader("Preface \u{2026} iii"), Some(("Preface", "iii")));
        // No leader, a leader of two dots, no title, no page number, a word where the number goes.
        for t in ["Introduction 12", "Introduction .. 12", "Introduction .........", "Results ......... mix", "Results ..... 12345"] {
            assert_eq!(split_leader(t), None, "{t}");
        }
    }
}
