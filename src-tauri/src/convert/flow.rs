//! The page model both document writers (Word and RTF) share: paragraphs, tables and images reduced from
//! MuPDF's structured text, and the reading order, headings and table detection over them.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/export/src/lib.rs @ b8ddf71.
#![allow(dead_code)]

/// A paragraph of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub text: String,
    /// [x0, y0, x1, y1] in user space (y up).
    pub rect: [f64; 4],
    /// Font size in points.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// Text fill colour as sRGB components in 0..=1 (black when unknown).
    pub color: [f64; 3],
}

/// An image of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    /// "png" or "jpg".
    pub ext: &'static str,
    pub bytes: Vec<u8>,
    pub rect: [f64; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub blocks: Vec<Block>,
    pub images: Vec<Image>,
}

/// One cell of a detected table. `span` is how many grid columns the cell covers (a header
/// row that stretches over sub-columns).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// Text fill colour as sRGB components in 0..=1.
    pub color: [f64; 3],
    pub span: usize,
    /// Width in points of the cell's longest word, so a writer can keep the column from breaking it.
    pub word: f64,
}

/// A table detected from the page's text layout: rows of cells, and the left edge of each
/// grid column (user space, y up) so the writers can size the columns.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub rect: [f64; 4],
    pub cols: Vec<f64>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    /// How many leading rows are header rows (Word repeats them on the next page): the rows from the top that are
    /// all bold, when the first has two or more filled cells (a bold caption that joined as a row has one) and
    /// some later row is not bold.
    pub fn header_rows(&self) -> usize {
        let bold = |r: &Vec<Cell>| r.iter().any(|c| !c.text.is_empty()) && r.iter().all(|c| c.bold || c.text.is_empty());
        let filled = self.rows.first().map_or(0, |r| r.iter().filter(|c| !c.text.is_empty()).count());
        let n = self.rows.iter().take_while(|r| bold(r)).count();
        if filled >= 2 && n < self.rows.len() { n } else { 0 }
    }
}

/// What goes into the output, in order.
pub(super) enum Item<'a> {
    Para(&'a Block, u8),
    /// Owned: tables are detected per page inside [`items`], so they cannot borrow.
    Table(Table),
    Img(&'a Image),
    PageBreak,
}

/// Whether two of the rects (lines of one text block, y up) sit side by side on one row: they share more than
/// half of the shorter one's height and do not overlap horizontally. A paragraph's lines are stacked, so only
/// text drawn cell by cell along a row (MuPDF keeps those in one block, in stream order) passes.
pub(super) fn side_by_side(rects: &[[f64; 4]]) -> bool {
    let mut by: Vec<&[f64; 4]> = rects.iter().collect();
    by.sort_by(|a, b| (a[1] + a[3]).total_cmp(&(b[1] + b[3])).then(a[0].total_cmp(&b[0])));
    // shortcut: only neighbours in vertical order are compared, which finds a row of two or more
    // cells (whatever else sits in the block), and is linear after the sort.
    by.windows(2).any(|w| {
        let (a, b) = (w[0], w[1]);
        let shared = a[3].min(b[3]) - a[1].max(b[1]);
        shared > 0.5 * (a[3] - a[1]).min(b[3] - b[1]) && (a[2] <= b[0] || b[2] <= a[0])
    })
}

/// Whether a line is only a list label: one symbol (a bullet, a dash) or a short number or letter with its
/// closing `.` or `)` ("1.", "a)", "iv."). The text beside it is the item, not a cell of a row.
pub(super) fn list_label(text: &str) -> bool {
    let t = text.trim();
    let mut chars = t.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => !c.is_alphanumeric(),
        _ => {
            let Some(body) = t.strip_suffix(['.', ')']) else { return false };
            let n = body.chars().count();
            (1..=4).contains(&n)
                && (body.chars().all(|c| c.is_ascii_digit()) || n == 1 && body.chars().all(char::is_alphabetic) || body.chars().all(|c| "ivxlcdmIVXLCDM".contains(c)))
        }
    }
}

/// Which blocks of a page a table would consume (what `items` drops from the paragraph flow).
pub(super) fn consumed(blocks: &[Block]) -> Vec<bool> {
    tables(blocks).1
}

/// One block from consecutive pieces of a paragraph: their text in order, the union of their rects, the first
/// one's size and colour, bold and italic where most of the characters are.
fn merge(run: &[Block]) -> Block {
    let mut b = run[0].clone();
    let len = |f: &dyn Fn(&Block) -> bool| run.iter().filter(|p| f(p)).map(|p| p.text.len()).sum::<usize>() * 2 > run.iter().map(|p| p.text.len()).sum::<usize>();
    b.text = run.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join(" ");
    b.rect = run.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |r, p| [r[0].min(p.rect[0]), r[1].min(p.rect[1]), r[2].max(p.rect[2]), r[3].max(p.rect[3])]);
    b.bold = len(&|p| p.bold);
    b.italic = len(&|p| p.italic);
    b
}

/// Lines that sat side by side (`blocks[range]` of each group, ascending and disjoint, `orig` the block they came
/// from) were only worth splitting when they make up a table. A group no table takes a piece of goes back to its
/// original block (repeated until that holds on the whole page, as putting one back can change what a table
/// takes); in a group a table did take from, each run of pieces it left is one paragraph again, unless that
/// changes the tables found.
pub(super) fn unsplit(blocks: &mut Vec<Block>, groups: Vec<(Block, std::ops::Range<usize>)>) {
    if groups.is_empty() {
        return;
    }
    let pieces = std::mem::take(blocks);
    let mut whole = vec![false; groups.len()];
    // The page's blocks for the groups' current state, and where each group's blocks are in them.
    let assemble = |whole: &[bool]| -> (Vec<Block>, Vec<std::ops::Range<usize>>) {
        let (mut out, mut at) = (Vec::with_capacity(pieces.len()), Vec::with_capacity(groups.len()));
        let mut i = 0;
        for (g, (orig, r)) in groups.iter().enumerate() {
            out.extend(pieces[i..r.start].iter().cloned());
            let from = out.len();
            if whole[g] {
                out.push(orig.clone());
            } else {
                out.extend(pieces[r.clone()].iter().cloned());
            }
            at.push(from..out.len());
            i = r.end;
        }
        out.extend(pieces[i..].iter().cloned());
        (out, at)
    };
    let (mut out, mut at) = assemble(&whole);
    // shortcut: at most 8 rounds; a page that needs more keeps its last state (tables are found again from
    // the final blocks anyway, so the output stays consistent), and each round is one detection pass.
    for _ in 0..8 {
        let used = consumed(&out);
        let mut changed = false;
        for (g, r) in at.iter().enumerate() {
            if !whole[g] && !used[r.clone()].iter().any(|u| *u) {
                whole[g] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
        (out, at) = assemble(&whole);
    }
    // Runs of pieces a table left, in groups it took from.
    let used = consumed(&out);
    let mut merged = Vec::with_capacity(out.len());
    let mut groups_at = at.iter().enumerate().filter(|(g, _)| !whole[*g]).map(|(_, r)| r.clone()).peekable();
    let mut i = 0;
    let mut any = false;
    while i < out.len() {
        match groups_at.next_if(|r| r.start == i) {
            Some(r) => {
                let mut j = r.start;
                while j < r.end {
                    let mut k = j + 1;
                    if !used[j] {
                        while k < r.end && !used[k] {
                            k += 1;
                        }
                    }
                    any |= k - j > 1;
                    merged.push(if k - j > 1 { merge(&out[j..k]) } else { out[j].clone() });
                    j = k;
                }
                i = r.end;
            }
            None => {
                merged.push(out[i].clone());
                i += 1;
            }
        }
    }
    *blocks = if any && tables(&merged).0 == tables(&out).0 { merged } else { out };
}

/// The body text size: the size most of the document's characters are set in.
pub(super) fn body_size(pages: &[Page]) -> f64 {
    let mut by: Vec<(i64, usize)> = Vec::new();
    for b in pages.iter().flat_map(|p| &p.blocks) {
        let k = (b.size * 2.0).round() as i64;
        match by.iter_mut().find(|e| e.0 == k) {
            Some(e) => e.1 += b.text.len(),
            None => by.push((k, b.text.len())),
        }
    }
    by.into_iter().max_by_key(|e| e.1).map_or(11.0, |e| e.0 as f64 / 2.0)
}

/// Heading level for a block: 1 for ≥ 1.6× the body size, 2 for ≥ 1.25×, 0 for body text.
fn level(b: &Block, body: f64) -> u8 {
    let short = b.text.len() < 200;
    if short && b.size >= body * 1.6 {
        1
    } else if short && (b.size >= body * 1.25 || (b.bold && b.size >= body && b.text.len() < 80 && !b.text.ends_with('.'))) {
        2
    } else {
        0
    }
}

/// The estimated width in points of a block's longest word. A one-line block is measured by its own
/// width per character; anything taller (or without a usable rect) falls back to size x 0.6 per character.
fn word_width(b: &Block) -> f64 {
    let chars = b.text.chars().count().max(1) as f64;
    let (w, h) = (b.rect[2] - b.rect[0], b.rect[3] - b.rect[1]);
    let per = if w.is_finite() && w > 0.0 && h <= b.size * 1.6 { w / chars } else { b.size * 0.6 };
    let longest = b.text.split_whitespace().map(|word| word.chars().count()).max().unwrap_or(0);
    longest as f64 * if per.is_finite() { per } else { 6.6 }
}

/// A table cell's left and right padding in points (108 twips: Word's default, and RTF's `\trgaph108`).
pub(super) const CELL_PAD: f64 = 5.4;
/// No column is narrower than this, in points.
const MIN_COL: f64 = 20.0;

/// Grid column widths in twips for a writer: the detected column edges (the last column runs to the table's
/// rect), each column at least as wide as its longest single-column word plus the cell padding, then, when
/// the table is wider than `text_w` points, the widest columns shrunk first and never below that minimum.
/// Both writers take their widths from here so the two files agree.
pub(super) fn col_twips(t: &Table, text_w: f64) -> Vec<i64> {
    let n = t.cols.len();
    let mut min = vec![MIN_COL; n];
    for row in &t.rows {
        let mut col = 0;
        for c in row {
            if let Some(m) = min.get_mut(col).filter(|_| c.span <= 1) {
                *m = m.max(c.word * 1.1 + 2.0 * CELL_PAD);
            }
            col += c.span.max(1);
        }
    }
    let natural: Vec<f64> = (0..n)
        .map(|i| {
            let right = t.cols.get(i + 1).copied().unwrap_or(t.rect[2]);
            let w = right - t.cols[i];
            if w.is_finite() { w } else { 0.0 }
        })
        .collect();
    let at = |cap: f64| -> Vec<f64> { natural.iter().zip(&min).map(|(w, m)| w.min(cap).max(*m)).collect() };
    let mut w = at(f64::MAX);
    if text_w.is_finite() && w.iter().sum::<f64>() > text_w {
        // The largest cap on column width that fits (or none: every column at its minimum).
        let (mut lo, mut hi) = (0.0, w.iter().copied().fold(0.0, f64::max));
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if at(mid).iter().sum::<f64>() > text_w { hi = mid } else { lo = mid }
        }
        w = at(lo);
    }
    w.into_iter().map(|p| (p * 20.0).ceil().clamp(60.0, 31680.0) as i64).collect()
}

/// How far two cell left edges may drift (points) and still be the same grid column.
const COL_TOL: f64 = 4.0;
/// More grid columns than this isn't a table. It is Word's limit: a .docx whose table has 64 or
/// more columns doesn't open. It also bounds a table's size: every row is materialized to all
/// its columns, so a hostile page laid out as a staircase of blocks would otherwise make
/// (blocks / 2)² cells.
const MAX_COLS: usize = 63;
/// A cell is short: taller blocks are body text (or a multi-column layout), not table cells.
fn is_cell_like(b: &Block) -> bool {
    (b.rect[3] - b.rect[1]) <= b.size * 5.0 && !b.text.trim().is_empty()
}

/// Detect tables in a page's blocks from their text layout: columns are left edges shared by
/// several short blocks, rows are blocks whose vertical spans overlap. Returns the tables and,
/// per block, whether a table consumed it (so `items` can drop it from the paragraph flow).
fn tables(blocks: &[Block]) -> (Vec<Table>, Vec<bool>) {
    let mut consumed = vec![false; blocks.len()];
    // Candidate columns: cluster the left edges of cell-like blocks.
    let mut xs: Vec<(f64, usize)> = blocks.iter().enumerate().filter(|(_, b)| is_cell_like(b)).map(|(i, b)| (b.rect[0], i)).collect();
    xs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cols: Vec<(f64, Vec<usize>)> = Vec::new(); // (left edge, member block indices)
    for (x, i) in xs {
        match cols.last_mut() {
            Some((e, m)) if (x - *e).abs() <= COL_TOL => {
                *e = (*e * m.len() as f64 + x) / (m.len() + 1) as f64;
                m.push(i);
            }
            _ => cols.push((x, vec![i])),
        }
    }
    let cols: Vec<(f64, Vec<usize>)> = cols.into_iter().filter(|(_, m)| m.len() >= 2).collect();
    if cols.len() < 2 {
        return (Vec::new(), consumed);
    }
    // Each block's column (an index into `cols`), so lookups stay linear on pages with many blocks.
    let mut col_of: Vec<Option<usize>> = vec![None; blocks.len()];
    for (ci, (_, members)) in cols.iter().enumerate() {
        for &i in members {
            if let Some(c) = col_of.get_mut(i) {
                *c = Some(ci);
            }
        }
    }
    // The column whose edge is within tolerance of `x` (edges are in ascending order).
    let column_at = |x: f64| -> Option<usize> {
        let k = cols.partition_point(|(e, _)| *e < x - COL_TOL);
        cols.get(k).filter(|(e, _)| (*e - x).abs() <= COL_TOL).map(|_| k)
    };
    // Assign every cell-like block to its nearest column (within tolerance).
    let mut rows: Vec<Vec<(usize, usize)>> = Vec::new(); // (column index, block index), top to bottom
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by(|a, b| blocks[*b].rect[3].total_cmp(&blocks[*a].rect[3]).then(blocks[*a].rect[0].total_cmp(&blocks[*b].rect[0])));
    for &bi in &order {
        let b = &blocks[bi];
        if !is_cell_like(b) {
            continue;
        }
        let Some(ci) = column_at(b.rect[0]) else { continue };
        let joins = rows
            .last()
            .and_then(|row| row.first())
            .map(|(_, fi)| {
                let rb = &blocks[*fi];
                let overlap = rb.rect[3].min(b.rect[3]) - rb.rect[1].max(b.rect[1]);
                overlap > 0.5 * (rb.rect[3] - rb.rect[1]).min(b.rect[3] - b.rect[1])
            })
            .unwrap_or(false);
        if joins {
            if let Some(row) = rows.last_mut() {
                row.push((ci, bi));
            }
        } else {
            rows.push(vec![(ci, bi)]);
        }
    }
    // Runs of consecutive rows that have two or more distinct columns are tables.
    let mut out = Vec::new();
    let flush = |run: &mut Vec<Vec<(usize, usize)>>, out: &mut Vec<Table>, consumed: &mut Vec<bool>| {
        if run.len() < 2 {
            run.clear();
            return;
        }
        let mut used: Vec<usize> = run.iter().flatten().map(|(_, i)| *i).collect();
        used.sort_unstable();
        used.dedup();
        let mut edges: Vec<f64> = used.iter().filter_map(|&i| col_of.get(i).copied().flatten()).filter_map(|ci| cols.get(ci)).map(|c| c.0).collect();
        edges.sort_by(|a, b| a.total_cmp(b));
        edges.dedup_by(|a, b| (*a - *b).abs() <= COL_TOL);
        let ncols = edges.len();
        let ok = (ncols >= 3 || (ncols == 2 && run.len() >= 3)) && ncols <= MAX_COLS;
        if !ok {
            run.clear();
            return;
        }
        let right = used.iter().map(|&i| blocks[i].rect[2]).fold(f64::MIN, f64::max) + COL_TOL;
        let mut rows_out = Vec::new();
        let mut slots: Vec<Option<Cell>> = vec![None; ncols];
        for row in run.drain(..) {
            for slot in slots.iter_mut() {
                *slot = None;
            }
            for (_ci, bi) in row {
                let b = &blocks[bi];
                let Some(slot) = edges.iter().position(|e| (*e - b.rect[0]).abs() <= COL_TOL) else { continue };
                let span = edges[slot + 1..].iter().filter(|e| **e > b.rect[0] + COL_TOL && **e < b.rect[2] - COL_TOL).count() + 1;
                let cell = Cell { text: b.text.trim().to_string(), size: b.size, bold: b.bold, italic: b.italic, color: b.color, span, word: word_width(b) };
                match &mut slots[slot] {
                    Some(c) => {
                        c.text.push(' ');
                        c.text.push_str(&cell.text);
                        c.word = c.word.max(cell.word);
                    }
                    None => slots[slot] = Some(cell),
                }
                consumed[bi] = true;
            }
            let mut materialized = Vec::with_capacity(ncols);
            let mut column = 0;
            while column < ncols {
                match slots[column].take() {
                    Some(mut cell) => {
                        cell.span = cell.span.min(ncols - column).max(1);
                        column += cell.span;
                        materialized.push(cell);
                    }
                    None => {
                        column += 1;
                        materialized.push(Cell { text: String::new(), size: 11.0, bold: false, italic: false, color: [0.0; 3], span: 1, word: 0.0 });
                    }
                }
            }
            while materialized.last().is_some_and(|cell| cell.text.is_empty()) && materialized.iter().any(|cell| cell.span > 1) {
                materialized.pop();
            }
            rows_out.push(materialized);
        }
        let rect = [
            edges[0],
            used.iter().map(|&i| blocks[i].rect[1]).fold(f64::MAX, f64::min),
            right,
            used.iter().map(|&i| blocks[i].rect[3]).fold(f64::MIN, f64::max),
        ];
        out.push(Table { rect, cols: edges, rows: rows_out });
    };
    // Runs of consecutive multi-column rows are tables. Rows on either side of a run that
    // hold a single cell wide enough to span two columns (a header band, a totals row) join
    // the table; other single-column rows (captions, following text) stay paragraphs.
    // Below the run, only a row within the table's row pitch joins.
    let spans_run = |edges: &[f64], bi: usize| -> bool {
        let b = &blocks[bi];
        edges.iter().any(|e| (*e - b.rect[0]).abs() <= COL_TOL) && edges.iter().any(|e| *e > b.rect[0] + COL_TOL && *e < b.rect[2] - COL_TOL)
    };
    let run_edges = |run: &[Vec<(usize, usize)>]| -> Vec<f64> {
        let mut es: Vec<f64> =
            run.iter().flatten().filter_map(|(_, i)| col_of.get(*i).copied().flatten()).filter_map(|ci| cols.get(ci)).map(|c| c.0).collect();
        es.sort_by(|a, b| a.total_cmp(b));
        es.dedup_by(|a, b| (*a - *b).abs() <= COL_TOL);
        es
    };
    let qual: Vec<bool> = rows
        .iter()
        .map(|row| {
            let mut cs: Vec<usize> = row.iter().map(|(c, _)| *c).collect();
            cs.sort_unstable();
            cs.dedup();
            cs.len() >= 2
        })
        .collect();
    // Rows' vertical extents (y up), and the blocks no row holds (text between the rows of a run is not a table's).
    let top = |row: &Vec<(usize, usize)>| row.iter().map(|(_, i)| blocks[*i].rect[3]).fold(f64::MIN, f64::max);
    let bottom = |row: &Vec<(usize, usize)>| row.iter().map(|(_, i)| blocks[*i].rect[1]).fold(f64::MAX, f64::min);
    let mut in_row = vec![false; blocks.len()];
    for (_, bi) in rows.iter().flatten() {
        in_row[*bi] = true;
    }
    let loose: Vec<usize> = (0..blocks.len()).filter(|i| !in_row[*i] && !blocks[*i].text.trim().is_empty()).collect();
    // The median spacing of the rows' tops, or 1.5 lines for a run of one or two rows.
    let pitch_of = |run: &[Vec<(usize, usize)>]| -> f64 {
        let mut p: Vec<f64> = run.windows(2).map(|w| top(&w[0]) - top(&w[1])).collect();
        p.sort_by(|a, b| a.total_cmp(b));
        if p.len() >= 2 { p[p.len() / 2] } else { 1.5 * (top(&run[0]) - bottom(&run[0])).max(blocks[run[0][0].1].size) }
    };
    // Whether the rows `a` above `b` are one table's: not far apart (more than 3 pitches) and nothing between them
    // under the table's width. A running header and a footer have the same columns, a page apart.
    let together = |a: &Vec<(usize, usize)>, b: &Vec<(usize, usize)>, pitch: f64| -> bool {
        let gap = top(a) - top(b);
        if gap > 3.0 * pitch {
            return false;
        }
        if gap <= 1.5 * pitch {
            return true;
        }
        let (lo, hi) = (bottom(a), top(b));
        let x0 = a.iter().chain(b).map(|(_, i)| blocks[*i].rect[0]).fold(f64::MAX, f64::min);
        let x1 = a.iter().chain(b).map(|(_, i)| blocks[*i].rect[2]).fold(f64::MIN, f64::max);
        !loose.iter().any(|&k| {
            let r = &blocks[k].rect;
            r[3] <= lo + 1.0 && r[1] >= hi - 1.0 && r[2] > x0 && r[0] < x1
        })
    };
    let mut i = 0;
    while i < rows.len() {
        if !qual[i] {
            i += 1;
            continue;
        }
        let first = i;
        while i < rows.len() && qual[i] {
            i += 1;
        }
        let last = i; // qual rows: rows[first..last]
        let pitch = pitch_of(&rows[first..last]);
        let mut start = first;
        while start < last {
            let mut end = start + 1;
            while end < last && together(&rows[end - 1], &rows[end], pitch) {
                end += 1;
            }
            // run = rows[start..end]
            let edges = run_edges(&rows[start..end]);
            let pitch = if end - start == last - first { pitch } else { pitch_of(&rows[start..end]) };
            let mut pieces: Vec<Vec<(usize, usize)>> = Vec::new();
            // A line above or below joins only when it sits within the run's normal row pitch (1.5 times the
            // median spacing of the rows' tops, or 1.5 lines for a run of one or two rows). Text that starts at the
            // table's edge further away is a paragraph, even when it is as wide as the table.
            if start > 0 && rows[start - 1].len() == 1 && top(&rows[start - 1]) - top(&rows[start]) <= 1.5 * pitch && spans_run(&edges, rows[start - 1][0].1) {
                pieces.push(rows[start - 1].clone());
            }
            pieces.extend(rows[start..end].iter().cloned());
            if end < rows.len() && rows[end].len() == 1 && top(&rows[end - 1]) - top(&rows[end]) <= 1.5 * pitch && spans_run(&edges, rows[end][0].1) {
                pieces.push(rows[end].clone());
            }
            flush(&mut pieces, &mut out, &mut consumed);
            start = end;
        }
    }
    (out, consumed)
}

pub(super) fn items(pages: &[Page]) -> Vec<Item<'_>> {
    let body = body_size(pages);
    let mut out = Vec::new();
    for (i, p) in pages.iter().enumerate() {
        if i > 0 {
            out.push(Item::PageBreak);
        }
        let (tables, consumed) = tables(&p.blocks);
        // Blocks and images by their top edge, top to bottom.
        let mut parts: Vec<(f64, f64, Item)> = p
            .blocks
            .iter()
            .enumerate()
            .filter(|(bi, _)| !consumed.get(*bi).copied().unwrap_or(false))
            .map(|(_, b)| (b.rect[3], b.rect[0], Item::Para(b, level(b, body))))
            .collect();
        parts.extend(tables.into_iter().map(|t| (t.rect[3], t.rect[0], Item::Table(t))));
        parts.extend(p.images.iter().map(|im| (im.rect[3], im.rect[0], Item::Img(im))));
        parts.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)));
        out.extend(parts.into_iter().map(|x| x.2));
    }
    out
}

/// Whether a character may appear in the output. XML 1.0 forbids the C0 controls other than tab,
/// line feed and carriage return, and U+FFFE/U+FFFF. Text extracted from PDFs often holds them
/// (fonts without a usable `/ToUnicode`), and Word refuses a whole document over one (#72).
pub(super) fn allowed(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || !(c.is_ascii_control() || c == '\u{FFFE}' || c == '\u{FFFF}')
}

pub(super) fn rgb8(c: [f64; 3]) -> [u8; 3] {
    // The value is clamped to 0..=255 before the cast, so it cannot truncate.
    c.map(|v| if v.is_finite() { (v.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 })
}

/// The colour as `RRGGBB`, or `None` for black (the default every format already uses).
pub(super) fn hex(c: [f64; 3]) -> Option<String> {
    let [r, g, b] = rgb8(c);
    (r | g | b != 0).then(|| format!("{r:02X}{g:02X}{b:02X}"))
}
