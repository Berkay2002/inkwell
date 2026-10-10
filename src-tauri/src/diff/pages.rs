//! Page matching: which page of A is which page of B, independent of page order.
//!
//! 1. Pages with the same fingerprint (content + used resources) are paired, nearest index first.
//! 2. The rest are paired by similarity (word 1-/2-gram Jaccard; image identity for image-only pages)
//!    with an optimal assignment (Hungarian algorithm), dropping pairs below a threshold.
//! 3. Order differences are reported as moves: matched pages that are not on the longest increasing
//!    subsequence of A-indices in B order.

use super::snap::{Hash, Snap};
use std::collections::{HashMap, HashSet};

const THRESHOLD: f64 = 0.4;
/// Above this many unmatched pages per side the O(n^3) assignment is replaced by greedy matching.
const HUNGARIAN_MAX: usize = 300;
/// With many unmatched pages only pairs whose indices are this close (plus the length difference) are scored.
const WINDOW: usize = 40;

struct PageFeat {
    n: usize,
    uni: HashSet<u64>,
    bi: HashSet<u64>,
    imgs: HashSet<Hash>,
}

fn h64(s: &str) -> u64 {
    let h = blake3::hash(s.as_bytes());
    u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap())
}

fn feat(s: &Snap, i: usize) -> PageFeat {
    let words = s.words(i);
    let lc: Vec<String> = words.iter().map(|w| w.text.to_lowercase()).collect();
    let uni = lc.iter().map(|w| h64(w)).collect();
    let bi = lc.windows(2).map(|p| h64(&format!("{} {}", p[0], p[1]))).collect();
    let imgs = s.pages[i].images.iter().map(|im| im.hash).collect();
    PageFeat { n: lc.len(), uni, bi, imgs }
}

fn jaccard<T: std::hash::Hash + Eq>(a: &HashSet<T>, b: &HashSet<T>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    inter / (a.len() as f64 + b.len() as f64 - inter)
}

fn similarity(fa: &PageFeat, fb: &PageFeat, ia: usize, ib: usize, total: usize) -> f64 {
    let d = ia.abs_diff(ib);
    let mut s = 0.0f64;
    if fa.n > 0 && fb.n > 0 {
        s = 0.5 * jaccard(&fa.bi, &fb.bi) + 0.5 * jaccard(&fa.uni, &fb.uni);
    }
    // An image-only page (a scan) vs the same scan with a text layer.
    if fa.n.min(fb.n) < 3 && !fa.imgs.is_empty() && !fb.imgs.is_empty() {
        s = s.max(0.95 * jaccard(&fa.imgs, &fb.imgs));
    }
    // Pages with no text at all: only near neighbours may pair up (an image replaced in place).
    if fa.n == 0 && fb.n == 0 && d <= 3 {
        s = s.max(0.5 - 0.02 * d as f64);
    }
    if s > 0.0 { s + 0.01 * (1.0 - d as f64 / total.max(1) as f64) } else { 0.0 }
}

/// For each page of B the matching page of A.
pub fn match_pages(a: &Snap, b: &Snap) -> Vec<Option<usize>> {
    let (na, nb) = (a.pages.len(), b.pages.len());
    let mut b2a: Vec<Option<usize>> = vec![None; nb];
    let mut a_used = vec![false; na];

    // 1. identical fingerprints
    let mut by_fp: HashMap<Hash, Vec<usize>> = HashMap::new();
    for (i, p) in a.pages.iter().enumerate() {
        by_fp.entry(p.fp).or_default().push(i);
    }
    for (j, p) in b.pages.iter().enumerate() {
        if let Some(cands) = by_fp.get(&p.fp) {
            let best = cands.iter().copied().filter(|&i| !a_used[i]).min_by_key(|&i| i.abs_diff(j));
            if let Some(i) = best {
                a_used[i] = true;
                b2a[j] = Some(i);
            }
        }
    }

    // 2. similarity
    let ra: Vec<usize> = (0..na).filter(|&i| !a_used[i]).collect();
    let rb: Vec<usize> = (0..nb).filter(|&j| b2a[j].is_none()).collect();
    if !ra.is_empty() && !rb.is_empty() {
        let fa: Vec<PageFeat> = ra.iter().map(|&i| feat(a, i)).collect();
        let fb: Vec<PageFeat> = rb.iter().map(|&j| feat(b, j)).collect();
        let total = na.max(nb);
        let windowed = ra.len().max(rb.len()) > 200;
        let win = WINDOW + na.abs_diff(nb);
        let sim = |x: usize, y: usize| -> f64 {
            if windowed && ra[x].abs_diff(rb[y]) > win {
                return 0.0;
            }
            similarity(&fa[x], &fb[y], ra[x], rb[y], total)
        };
        let pairs: Vec<(usize, usize)> = if ra.len().max(rb.len()) <= HUNGARIAN_MAX {
            let cost: Vec<Vec<f64>> =
                (0..ra.len()).map(|x| (0..rb.len()).map(|y| { let s = sim(x, y); if s >= THRESHOLD { 1.0 - s } else { 2.0 } }).collect()).collect();
            hungarian(&cost).into_iter().enumerate().filter_map(|(x, y)| y.map(|y| (x, y))).collect()
        } else {
            // ponytail: greedy by descending similarity above HUNGARIAN_MAX unmatched pages; optimal assignment would be O(n^3)
            let mut all: Vec<(f64, usize, usize)> = Vec::new();
            for x in 0..ra.len() {
                for y in 0..rb.len() {
                    let s = sim(x, y);
                    if s >= THRESHOLD {
                        all.push((s, x, y));
                    }
                }
            }
            all.sort_by(|p, q| q.0.total_cmp(&p.0));
            let (mut ux, mut uy) = (vec![false; ra.len()], vec![false; rb.len()]);
            let mut out = Vec::new();
            for (_, x, y) in all {
                if !ux[x] && !uy[y] {
                    ux[x] = true;
                    uy[y] = true;
                    out.push((x, y));
                }
            }
            out
        };
        for (x, y) in pairs {
            if sim(x, y) >= THRESHOLD {
                b2a[rb[y]] = Some(ra[x]);
            }
        }
    }
    b2a
}

/// Minimum-cost assignment of rows to columns (rows or columns may be in surplus).
/// Returns, for each row, the assigned column.
fn hungarian(cost: &[Vec<f64>]) -> Vec<Option<usize>> {
    let (r, c) = (cost.len(), cost.first().map_or(0, |x| x.len()));
    if r == 0 || c == 0 {
        return vec![None; r];
    }
    if r > c {
        // transpose: assign columns to rows, then invert
        let t: Vec<Vec<f64>> = (0..c).map(|j| (0..r).map(|i| cost[i][j]).collect()).collect();
        let m = hungarian(&t);
        let mut out = vec![None; r];
        for (j, i) in m.into_iter().enumerate() {
            if let Some(i) = i {
                out[i] = Some(j);
            }
        }
        return out;
    }
    // e-maxx formulation, 1-based, n = r <= m = c
    let (n, m) = (r, c);
    let inf = f64::INFINITY;
    let (mut u, mut v) = (vec![0.0; n + 1], vec![0.0; m + 1]);
    let (mut p, mut way) = (vec![0usize; m + 1], vec![0usize; m + 1]);
    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0;
        let mut minv = vec![inf; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let (i0, mut delta, mut j1) = (p[j0], inf, 0);
            for j in 1..=m {
                if !used[j] {
                    let cur = cost[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    let mut out = vec![None; n];
    for j in 1..=m {
        if p[j] != 0 {
            out[p[j] - 1] = Some(j - 1);
        }
    }
    out
}

/// Indices (into `seq`) of one longest strictly increasing subsequence.
pub fn lis(seq: &[usize]) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new(); // index into seq of the smallest tail for each length
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, &x) in seq.iter().enumerate() {
        let pos = tails.partition_point(|&t| seq[t] < x);
        if pos > 0 {
            prev[i] = tails[pos - 1];
        }
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }
    let mut out = Vec::new();
    let mut k = tails.last().copied();
    while let Some(i) = k {
        out.push(i);
        k = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    out.reverse();
    out
}
