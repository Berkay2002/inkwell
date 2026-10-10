//! Reading order for recognised lines: an XY-cut over the line fragments.
//!
//! Engines emit lines in their own order. Windows OCR hands back a ruled table column by column
//! (every cell is its own "line"), which makes extraction and copy read `A-100 A-101 ... Hex bolt
//! ...` instead of row by row. The cut separates:
//!
//! * **columns of running text** (a vertical gutter that no line crosses, both sides made of
//!   lines of three or more words) are read one after the other;
//! * everything else is read **row by row** and left to right: a table's cells are one to three
//!   words, so a gutter between them is not a column break.
//!
//! Fragments are engine lines (split where a gap is wider than two word heights), never single
//! words, so a skewed line from an engine that does not deskew stays one unit.

use super::Word;

/// A gap this wide (in median word heights) can separate columns.
const GUTTER: f32 = 1.5;
/// Words further apart than this (in median word heights) are different fragments of a line.
const FRAGMENT_GAP: f32 = 2.0;
/// Both sides of a column cut need at least this many words in the median fragment.
const COLUMN_MIN_WORDS: usize = 3;

struct Frag {
    words: Vec<Word>,
    b: [f32; 4],
}

impl Frag {
    fn new(words: Vec<Word>) -> Self {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for w in &words {
            b = [b[0].min(w.bbox[0]), b[1].min(w.bbox[1]), b[2].max(w.bbox[2]), b[3].max(w.bbox[3])];
        }
        Self { words, b }
    }
    fn cy(&self) -> f32 {
        (self.b[1] + self.b[3]) / 2.0
    }
    fn h(&self) -> f32 {
        self.b[3] - self.b[1]
    }
}

pub(crate) fn reading_order(lines: Vec<Vec<Word>>) -> Vec<Vec<Word>> {
    let mut heights: Vec<f32> = lines.iter().flatten().map(|w| w.bbox[3] - w.bbox[1]).filter(|h| *h > 0.0).collect();
    if heights.len() < 2 {
        return lines;
    }
    heights.sort_by(|a, b| a.total_cmp(b));
    let mh = heights[heights.len() / 2].max(1.0);

    let mut frags = Vec::new();
    for mut l in lines {
        l.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
        let mut cur: Vec<Word> = Vec::new();
        for w in l {
            if cur.last().is_some_and(|p| w.bbox[0] - p.bbox[2] > FRAGMENT_GAP * mh) {
                frags.push(Frag::new(std::mem::take(&mut cur)));
            }
            cur.push(w);
        }
        if !cur.is_empty() {
            frags.push(Frag::new(cur));
        }
    }
    let mut out = Vec::new();
    cut(frags, mh, &mut out);
    out
}

fn cut(frags: Vec<Frag>, mh: f32, out: &mut Vec<Vec<Word>>) {
    if frags.len() > 1 {
        if let Some(x) = split_columns(&frags, mh) {
            let (left, right): (Vec<_>, Vec<_>) = frags.into_iter().partition(|f| f.b[2] <= x);
            cut(left, mh, out);
            cut(right, mh, out);
            return;
        }
        let bands = split_rows(frags);
        if bands.len() > 1 {
            for band in bands {
                cut(band, mh, out);
            }
            return;
        }
        // one band: nothing to cut
        return leaf(bands.into_iter().next().unwrap_or_default(), out);
    }
    leaf(frags, out);
}

/// x of the widest empty vertical gutter (no fragment crosses it) that has running text on both sides.
fn split_columns(frags: &[Frag], mh: f32) -> Option<f32> {
    let mut order: Vec<&Frag> = frags.iter().collect();
    order.sort_by(|a, b| a.b[0].total_cmp(&b.b[0]));
    let mut gaps: Vec<(f32, f32)> = Vec::new(); // (start, end)
    let mut right = order[0].b[2];
    for f in &order[1..] {
        if f.b[0] - right >= GUTTER * mh {
            gaps.push((right, f.b[0]));
        }
        right = right.max(f.b[2]);
    }
    gaps.sort_by(|a, b| (b.1 - b.0).total_cmp(&(a.1 - a.0)));
    let median_words = |side: Vec<usize>| {
        let mut v = side;
        v.sort_unstable();
        v[v.len() / 2]
    };
    gaps.into_iter().map(|(a, b)| (a + b) / 2.0).find(|&x| {
        let (l, r): (Vec<_>, Vec<_>) = frags.iter().partition(|f| f.b[2] <= x);
        !l.is_empty()
            && !r.is_empty()
            && median_words(l.iter().map(|f| f.words.len()).collect()) >= COLUMN_MIN_WORDS
            && median_words(r.iter().map(|f| f.words.len()).collect()) >= COLUMN_MIN_WORDS
    })
}

/// Horizontal bands separated by empty space, top to bottom.
fn split_rows(mut frags: Vec<Frag>) -> Vec<Vec<Frag>> {
    frags.sort_by(|a, b| a.b[1].total_cmp(&b.b[1]));
    let mut bands: Vec<Vec<Frag>> = Vec::new();
    let mut bottom = f32::MIN;
    for f in frags {
        if bands.is_empty() || f.b[1] > bottom {
            bands.push(Vec::new());
            bottom = f.b[3];
        } else {
            bottom = bottom.max(f.b[3]);
        }
        bands.last_mut().unwrap().push(f);
    }
    bands
}

fn leaf(mut frags: Vec<Frag>, out: &mut Vec<Vec<Word>>) {
    frags.sort_by(|a, b| a.cy().total_cmp(&b.cy()).then(a.b[0].total_cmp(&b.b[0])));
    let mut lines: Vec<(f32, f32, Vec<Word>)> = Vec::new(); // (cy, h, words)
    for f in frags {
        let (cy, h) = (f.cy(), f.h());
        match lines.last_mut() {
            Some((lcy, lh, words)) if (cy - *lcy).abs() < 0.35 * h.min(*lh) => words.extend(f.words),
            _ => lines.push((cy, h, f.words)),
        }
    }
    for (_, _, mut words) in lines {
        words.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
        out.push(words);
    }
}
