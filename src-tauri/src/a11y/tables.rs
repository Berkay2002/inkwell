//! Table detection for Autotag, from structured-text lines only (no ML). A table is a run of rows
//! that each hold the same number of cells (one cell is one structured-text line), whose columns
//! line up. It is conservative on purpose: a false table is worse than paragraphs, so anything
//! irregular (a row with a missing or merged cell, a cell that wraps onto two lines, cells drawn
//! column by column) stays paragraphs, and so do prose in columns, lists, forms and key-value pairs.
//!
//! Row = lines whose baselines are within `BASELINE_TOL`. Column = the cells at one position of
//! every row: their left edges, right edges or centers must agree within `ALIGN_TOL` (the header
//! row may be aligned differently from the body), and neighbouring columns must not overlap.
//! A table also needs evidence that it is one: a header row that differs (bold, larger, or under a
//! ruling line) or ruling lines (stroked lines, thin filled rectangles, stroked rectangle edges)
//! across its columns. A grid of plain text with neither is left alone, because nothing separates it
//! from key-value pairs, a table of contents or a letterhead.

use super::autotag::bbox_of;
use super::content::{self, Op};
use mupdf::{Matrix, Rect};
use std::sync::LazyLock;

/// Lines with baselines this close (points) are on one row.
pub const BASELINE_TOL: f32 = 2.0;
/// Cell edges (left, right or center) this close (points) are aligned.
pub const ALIGN_TOL: f32 = 4.0;
/// A ruling line this close (points) to the table's rows belongs to it.
const RULE_TOL: f32 = 6.0;
/// Thickest filled rectangle (points) that counts as a ruling line.
const RULE_THICK: f32 = 2.5;
/// Shortest ruling line (points).
const RULE_MIN: f32 = 8.0;
/// A page with more text lines than this keeps its paragraphs.
pub const MAX_ITEMS: usize = 6000;
const MAX_ROWS: usize = 300;
const MAX_COLS: usize = 16;
const MAX_RULES: usize = 5000;

/// A structured-text line that can be a cell (page space, y down).
pub struct Item<'a> {
    /// The caller's index of the line.
    pub id: usize,
    pub bbox: Rect,
    pub baseline: f32,
    pub size: f32,
    pub bold: bool,
    pub text: &'a str,
    /// The first and last marked-content run of the line, in content order.
    pub runs: (usize, usize),
}

/// A horizontal ruling line (page space).
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub x0: f32,
    pub x1: f32,
    pub y: f32,
}

#[derive(Debug, PartialEq)]
pub struct Table {
    /// The `Item::id` of each cell, row by row.
    pub rows: Vec<Vec<usize>>,
    /// The first row is a header row (TH).
    pub header: bool,
}

/// The horizontal ruling lines a content stream paints.
pub fn rules(ops: &[Op], page_ctm: &Matrix) -> Vec<Rule> {
    let mut out = Vec::new();
    let mut ctm = page_ctm.clone();
    let mut stack: Vec<Matrix> = Vec::new();
    let mut cur = (0f32, 0f32);
    let mut lines: Vec<((f32, f32), (f32, f32))> = Vec::new();
    let mut rects: Vec<Rect> = Vec::new();
    for op in ops {
        match op.name.as_slice() {
            b"q" => stack.push(ctm.clone()),
            b"Q" => ctm = stack.pop().unwrap_or_else(|| page_ctm.clone()),
            b"cm" => ctm = content::matrix(op) * &ctm,
            b"m" => cur = ctm.transform_xy(op.n(0), op.n(1)),
            b"l" => {
                let p = ctm.transform_xy(op.n(0), op.n(1));
                lines.push((cur, p));
                cur = p;
            }
            b"re" => {
                let (x, y) = (op.n(0), op.n(1));
                rects.push(bbox_of(&ctm, Rect { x0: x, y0: y, x1: x + op.n(2), y1: y + op.n(3) }));
            }
            b"n" => {
                lines.clear();
                rects.clear();
            }
            _ if content::paints_path(op) => {
                let stroked = matches!(op.name.as_slice(), b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
                for r in rects.drain(..) {
                    let (w, h) = (r.x1 - r.x0, r.y1 - r.y0);
                    if h <= RULE_THICK && w >= RULE_MIN {
                        out.push(Rule { x0: r.x0, x1: r.x1, y: (r.y0 + r.y1) / 2.0 });
                    } else if stroked && w >= RULE_MIN && h >= RULE_MIN {
                        out.push(Rule { x0: r.x0, x1: r.x1, y: r.y0 });
                        out.push(Rule { x0: r.x0, x1: r.x1, y: r.y1 });
                    }
                }
                for ((x0, y0), (x1, y1)) in lines.drain(..) {
                    if stroked && (y1 - y0).abs() <= 1.0 && (x1 - x0).abs() >= RULE_MIN {
                        out.push(Rule { x0: x0.min(x1), x1: x0.max(x1), y: (y0 + y1) / 2.0 });
                    }
                }
                if out.len() >= MAX_RULES {
                    out.truncate(MAX_RULES);
                    return out;
                }
            }
            _ => {}
        }
    }
    out
}

/// A cell that is only a list marker (a bullet, `1.`, `a)`, `iv.`).
static MARKER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^\s*(?:[•◦▪‣●■·○*\u{2013}\u{2014}-]|\(?[0-9]{1,3}[.)]|\(?[a-zA-Z][.)]|\(?[ivxIVX]{1,5}[.)])\s*$").expect("valid regex")
});

struct Row {
    /// Indices into the items, left to right.
    cells: Vec<usize>,
    base: f32,
    y0: f32,
    y1: f32,
}

/// Spread of a set of numbers.
fn spread(it: impl Iterator<Item = f32>) -> f32 {
    let (lo, hi) = it.fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
    hi - lo
}

fn aligned(cells: &[&Item]) -> bool {
    spread(cells.iter().map(|c| c.bbox.x0)) <= ALIGN_TOL
        || spread(cells.iter().map(|c| c.bbox.x1)) <= ALIGN_TOL
        || spread(cells.iter().map(|c| (c.bbox.x0 + c.bbox.x1) / 2.0)) <= ALIGN_TOL
}

/// The tables among `items`. `widgets` are the form fields' boxes: text near one is a form.
pub fn detect(items: &[Item], rules: &[Rule], widgets: &[Rect]) -> Vec<Table> {
    let mut found = Vec::new();
    if items.len() < 4 || items.len() > MAX_ITEMS {
        return found;
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[a].baseline.total_cmp(&items[b].baseline).then(items[a].bbox.x0.total_cmp(&items[b].bbox.x0)));
    let mut rows: Vec<Row> = Vec::new();
    for i in order {
        let it = &items[i];
        match rows.last_mut() {
            Some(r) if (it.baseline - r.base).abs() <= BASELINE_TOL => {
                r.cells.push(i);
                r.y0 = r.y0.min(it.bbox.y0);
                r.y1 = r.y1.max(it.bbox.y1);
            }
            _ => rows.push(Row { cells: vec![i], base: it.baseline, y0: it.bbox.y0, y1: it.bbox.y1 }),
        }
    }
    for r in &mut rows {
        r.cells.sort_by(|&a, &b| items[a].bbox.x0.total_cmp(&items[b].bbox.x0));
    }
    // Regions: rows closer together than one line.
    let mut start = 0;
    for end in 1..=rows.len() {
        let split = end == rows.len() || {
            let (p, n) = (&rows[end - 1], &rows[end]);
            n.y0 - p.y1 > (p.y1 - p.y0).max(n.y1 - n.y0)
        };
        if split {
            if let Some(t) = region(items, &rows[start..end], rules, widgets) {
                found.push(t);
            }
            start = end;
        }
    }
    found
}

/// The table in a region of nearby rows, if there is a regular one: it runs from the first row
/// with two or more cells to the last, and every row in between must have the same columns.
fn region(items: &[Item], rows: &[Row], rules: &[Rule], widgets: &[Rect]) -> Option<Table> {
    let multi: Vec<usize> = (0..rows.len()).filter(|&r| rows[r].cells.len() >= 2).collect();
    if multi.len() < 2 {
        return None;
    }
    let rows = &rows[multi[0]..=multi[multi.len() - 1]];
    let n = rows[0].cells.len();
    if rows.len() > MAX_ROWS || n > MAX_COLS || rows.iter().any(|r| r.cells.len() != n) {
        return None;
    }
    let cell = |r: usize, c: usize| &items[rows[r].cells[c]];
    // Columns: their extents must not touch, and each must line up.
    let mut cols: Vec<(f32, f32)> = Vec::new();
    for c in 0..n {
        let all: Vec<&Item> = (0..rows.len()).map(|r| cell(r, c)).collect();
        let (a, b) = all.iter().fold((f32::MAX, f32::MIN), |(a, b), i| (a.min(i.bbox.x0), b.max(i.bbox.x1)));
        if cols.last().is_some_and(|&(_, pb)| a <= pb) {
            return None;
        }
        // A header row may sit differently over its column (centered over numbers, say), but it
        // must still be above it.
        let body = &all[1..];
        let (ba, bb) = body.iter().fold((f32::MAX, f32::MIN), |(a, b), i| (a.min(i.bbox.x0), b.max(i.bbox.x1)));
        let over = all[0].bbox.x0 <= bb + ALIGN_TOL && all[0].bbox.x1 >= ba - ALIGN_TOL;
        if !(aligned(&all) || (aligned(body) && over)) {
            return None;
        }
        cols.push((a, b));
    }
    // Drawn row by row, each cell whole before the next: tag order and content order then agree.
    let cells: Vec<&Item> = rows.iter().flat_map(|r| r.cells.iter().map(|&i| &items[i])).collect();
    if cells.windows(2).any(|w| w[0].runs.1 >= w[1].runs.0) {
        return None;
    }
    let (x0, x1) = (cols[0].0, cols[n - 1].1);
    let (y0, y1) = (rows[0].y0, rows[rows.len() - 1].y1);
    let near = Rect { x0: x0 - RULE_TOL, y0: y0 - RULE_TOL, x1: x1 + RULE_TOL, y1: y1 + RULE_TOL };
    if widgets.iter().any(|w| w.x0 < near.x1 && w.x1 > near.x0 && w.y0 < near.y1 && w.y1 > near.y0) {
        return None;
    }

    // Not prose in columns, not a list (a column of markers), not a form (labels with colons).
    let words: usize = cells.iter().map(|c| c.text.split_whitespace().count()).sum();
    let mean = words as f32 / cells.len() as f32;
    let ends = |c: &&Item, set: &[char]| c.text.trim_end().ends_with(set);
    let sentences = cells.iter().filter(|c| ends(c, &['.', ',', ';'])).count() as f32 / cells.len() as f32;
    if mean >= 6.0 || (mean >= 4.0 && sentences >= 0.6) {
        return None;
    }
    if (0..n).any(|c| (0..rows.len()).all(|r| MARKER.is_match(cell(r, c).text))) {
        return None;
    }
    if cells.iter().filter(|c| ends(c, &[':'])).count() as f32 >= cells.len() as f32 * 0.4 {
        return None;
    }

    // Evidence: a ruling line across two columns near the rows, or a header row that differs.
    // Without a rule, a last column of nothing but small integers (a table of contents: title and
    // page, with a bold first entry or a number in front) is not evidence enough.
    let spans = |r: &Rule| cols.iter().filter(|&&(a, b)| r.x0 <= b && r.x1 >= a).count() >= 2;
    let ruled = rules.iter().any(|r| r.y >= near.y0 && r.y <= near.y1 && spans(r));
    let bold = |r: usize| (0..n).all(|c| cell(r, c).bold);
    let size = |r: usize| (0..n).map(|c| cell(r, c).size).fold(0.0, f32::max);
    let body_size = (1..rows.len()).map(size).fold(0.0, f32::max);
    // A bold totals row under the body does not make the header ordinary text.
    let body_end = rows.len() - usize::from(rows.len() >= 3);
    let styled = (bold(0) && !(1..body_end).any(bold)) || size(0) >= body_size * 1.1;
    let (b0, b1) = (cell(0, 0).baseline, cell(1, 0).baseline);
    let below = rules.iter().any(|r| r.y > b0 + 0.5 && r.y < b1 - 0.25 * cell(1, 0).size && spans(r));
    let header = styled || below;
    let page_numbers = (0..rows.len()).all(|r| {
        let t = cell(r, n - 1).text.trim();
        !t.is_empty() && t.len() <= 3 && t.bytes().all(|b| b.is_ascii_digit())
    });
    if !(ruled || header) || (page_numbers && !ruled) {
        return None;
    }
    Some(Table { rows: rows.iter().map(|r| r.cells.iter().map(|&i| items[i].id).collect()).collect(), header })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bold header row over `rows - 1` rows, `cols` columns, 12 pt cells every 18 pt.
    fn grid(rows: usize, cols: usize) -> Vec<(Rect, f32, bool, String)> {
        let mut v = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                let (x, y) = (50.0 + c as f32 * 90.0, 40.0 + r as f32 * 18.0);
                v.push((Rect { x0: x, y0: y - 9.0, x1: x + 40.0, y1: y + 3.0 }, y, r == 0, format!("c{r}x{c}")));
            }
        }
        v
    }

    fn items(g: &[(Rect, f32, bool, String)]) -> Vec<Item<'_>> {
        g.iter().enumerate().map(|(id, (bbox, baseline, bold, text))| Item { id, bbox: *bbox, baseline: *baseline, size: 12.0, bold: *bold, text, runs: (id, id) }).collect()
    }

    #[test]
    fn finds_a_regular_grid_and_caps_the_size() {
        let g = grid(4, 3);
        let t = detect(&items(&g), &[], &[]);
        assert_eq!(t.len(), 1);
        assert!(t[0].header);
        assert_eq!(t[0].rows, vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8], vec![9, 10, 11]]);
        // Too many rows, too many columns, too many lines on the page: paragraphs.
        assert!(detect(&items(&grid(MAX_ROWS + 1, 3)), &[], &[]).is_empty());
        assert!(detect(&items(&grid(4, MAX_COLS + 1)), &[], &[]).is_empty());
        assert!(!detect(&items(&grid(MAX_ROWS, 3)), &[], &[]).is_empty(), "the cap itself is allowed");
    }

    /// `n` small valid tables (4 rows by 8 columns) one under the other, a gap apart.
    fn many(n: usize) -> Vec<(Rect, f32, bool, String)> {
        let mut v = Vec::new();
        for t in 0..n {
            for (mut b, mut y, bold, text) in grid(4, 8) {
                let dy = t as f32 * 150.0;
                b.y0 += dy;
                b.y1 += dy;
                y += dy;
                v.push((b, y, bold, format!("t{t}{text}")));
            }
        }
        v
    }

    #[test]
    fn a_page_with_too_many_lines_keeps_its_paragraphs() {
        let fits = many(MAX_ITEMS / 32);
        assert!(fits.len() <= MAX_ITEMS);
        assert_eq!(detect(&items(&fits), &[], &[]).len(), MAX_ITEMS / 32, "every small table is found");
        let over = many(MAX_ITEMS / 32 + 1);
        assert!(over.len() > MAX_ITEMS);
        assert!(detect(&items(&over), &[], &[]).is_empty());
    }

    #[test]
    fn a_cell_split_around_another_cell_is_not_in_order() {
        let g = grid(3, 3);
        let mut it = items(&g);
        assert_eq!(detect(&it, &[], &[]).len(), 1);
        // Cell 0 is drawn in two pieces with cell 1 between them.
        it[0].runs = (0, 2);
        assert!(detect(&it, &[], &[]).is_empty());
    }

    #[test]
    fn baselines_within_the_tolerance_are_one_row_and_beyond_it_are_two() {
        let mut g = grid(3, 3);
        // A cell set 1.5 pt off its row's baseline is still in the row.
        g[4].1 += BASELINE_TOL - 0.5;
        assert_eq!(detect(&items(&g), &[], &[]).len(), 1);
        // 4 pt off, it is a row of its own (rows now have 2, 1, ... cells): no table.
        let mut g = grid(3, 3);
        g[4].1 += 4.0;
        g[4].0.y0 += 4.0;
        g[4].0.y1 += 4.0;
        assert!(detect(&items(&g), &[], &[]).is_empty());
    }

    #[test]
    fn a_header_row_is_styled_only_when_no_body_row_is_bold() {
        let mut g = grid(4, 3);
        assert!(detect(&items(&g), &[], &[])[0].header);
        // A bold last row (totals) leaves the header a header.
        for c in 9..12 {
            g[c].2 = true;
        }
        assert!(detect(&items(&g), &[], &[])[0].header);
        // A bold row in the body: bold marks nothing, so there is nothing to confirm the table.
        g[6].2 = true;
        g[7].2 = true;
        g[8].2 = true;
        assert!(detect(&items(&g), &[], &[]).is_empty());
        // With a rule under the last row it is a table again, with no header row.
        let under = Rule { x0: 50.0, x1: 330.0, y: 98.0 };
        let t = detect(&items(&g), &[under], &[]);
        assert_eq!(t.len(), 1);
        assert!(!t[0].header);
    }

    #[test]
    fn a_rule_across_the_columns_confirms_a_small_table_and_one_beside_it_does_not() {
        let g: Vec<_> = grid(2, 2).into_iter().map(|(b, y, _, t)| (b, y, false, t)).collect();
        assert!(detect(&items(&g), &[], &[]).is_empty(), "2 x 2 with nothing to confirm it");
        let across = Rule { x0: 50.0, x1: 230.0, y: 52.0 };
        let t = detect(&items(&g), &[across], &[]);
        assert_eq!(t.len(), 1);
        assert!(t[0].header, "the rule is between the two rows");
        let one_column = Rule { x0: 50.0, x1: 90.0, y: 52.0 };
        assert!(detect(&items(&g), &[one_column], &[]).is_empty());
        let far = Rule { x0: 50.0, x1: 230.0, y: 400.0 };
        assert!(detect(&items(&g), &[far], &[]).is_empty());
    }
}
