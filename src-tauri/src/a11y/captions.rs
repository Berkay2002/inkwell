//! Caption detection for Autotag. A caption is a short paragraph that opens with "Figure", "Fig." or
//! "Table" and a number, and sits directly above or below a figure (an image) or a table of the same
//! kind. Anything else that starts that way is body text: a long paragraph, a line far from the
//! figure, or "Figure 3 shows ..." (a lowercase word after the number is a sentence, not a label).

use mupdf::Rect;
use regex::Regex;
use std::sync::LazyLock;

/// A caption is at most this many lines and characters.
pub const MAX_LINES: usize = 2;
pub const MAX_CHARS: usize = 150;
/// The gap (points) between a caption and its figure or table, at most.
pub const MAX_GAP: f32 = 24.0;
/// Label paragraphs times targets on one page: past this the page keeps its paragraphs (the work is quadratic).
pub const MAX_WORK: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Figure,
    Table,
}

static LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)^\s*(fig(?:ure)?\.?|table)\s*([0-9]+(?:[.\-][0-9]+)*[a-z]?)\b(.*)$").expect("valid regex")
});

/// What a paragraph's first line says it labels: "Figure 3", "Fig. 3.2.", "TABLE 4: Sales".
pub fn label(text: &str) -> Option<Kind> {
    let c = LABEL.captures(text)?;
    // "Figure 3 shows ...", "Figure 3, above, shows ..." and "Figure 3 (above) shows ..." are sentences. ("Table 2
    // (continued)" is read as one too: a caption that is a parenthesis alone is the price of that.)
    let rest = &c[3];
    let first = rest.trim_start().chars().next();
    if first.is_some_and(|f| f.is_lowercase() || f == ',') || (first == Some('(') && rest.starts_with(char::is_whitespace)) {
        return None;
    }
    Some(if c[1].to_lowercase().starts_with("fig") { Kind::Figure } else { Kind::Table })
}

/// A figure or a table on the page.
pub struct Target {
    pub kind: Kind,
    pub bbox: Rect,
}

/// The target a caption belongs to: its index, and whether the caption is above it. `cap` is the box of the
/// whole paragraph; `lines` are the boxes of every line on the page and `own` the indices of the caption's own
/// lines (nothing else may sit between).
pub fn place(kind: Kind, cap: Rect, targets: &[Target], lines: &[Rect], own: &[usize]) -> Option<(usize, bool)> {
    let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| (a1.min(b1) - a0.max(b0)).max(0.0);
    // The nearest target of the kind that is beside the caption within `MAX_GAP`...
    let mut best: Option<(f32, usize, bool, f32, f32)> = None;
    for (i, t) in targets.iter().enumerate().filter(|(_, t)| t.kind == kind) {
        let b = t.bbox;
        let shared = overlap(cap.x0, cap.x1, b.x0, b.x1);
        if shared < 0.5 * (cap.x1 - cap.x0).min(b.x1 - b.x0) || shared <= 0.0 {
            continue;
        }
        // Slack of 2 pt for boxes that touch.
        let (gap, above, band) = if cap.y1 <= b.y0 + 2.0 {
            (b.y0 - cap.y1, true, (cap.y1, b.y0))
        } else if cap.y0 >= b.y1 - 2.0 {
            (cap.y0 - b.y1, false, (b.y1, cap.y0))
        } else {
            continue;
        };
        if gap <= MAX_GAP && best.is_none_or(|(g, ..)| gap < g) {
            best = Some((gap, i, above, band.0, band.1));
        }
    }
    // ...and nothing else may sit between them (one scan of the page's lines, for that target only).
    let (_, i, above, lo, hi) = best?;
    let between = lines
        .iter()
        .enumerate()
        .any(|(n, o)| !own.contains(&n) && o.y0 < hi - 1.0 && o.y1 > lo + 1.0 && overlap(o.x0, o.x1, cap.x0, cap.x1) > 0.0);
    (!between).then_some((i, above))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    #[test]
    fn a_label_is_a_name_and_a_number() {
        for (t, k) in [("Figure 3", Kind::Figure), ("figure 3.2: Sales", Kind::Figure), ("Fig. 4 Overview", Kind::Figure), ("FIG 2a. Detail", Kind::Figure), ("Table 1", Kind::Table), ("TABLE 12 - Totals", Kind::Table), ("Table 2.1. Costs", Kind::Table), ("Fig. 3(a) Overview", Kind::Figure)] {
            assert_eq!(label(t), Some(k), "{t}");
        }
        for t in ["Figure 3 shows the sales", "Figure 3, above, shows sales.", "Figure 3 (above) shows sales", "Table 2 is below", "Figures 3 and 4", "Figure", "Table of contents", "Tables 2", "The figure 3", "Table2x is"] {
            assert_eq!(label(t), None, "{t}");
        }
    }

    #[test]
    fn a_caption_is_next_to_a_target_of_its_kind_with_nothing_between() {
        let fig = Target { kind: Kind::Figure, bbox: r(100.0, 100.0, 300.0, 250.0) };
        let tab = Target { kind: Kind::Table, bbox: r(100.0, 400.0, 300.0, 500.0) };
        let ts = [fig, tab];
        // Below the figure, above the table.
        assert_eq!(place(Kind::Figure, r(100.0, 256.0, 300.0, 270.0), &ts, &[], &[]), Some((0, false)));
        assert_eq!(place(Kind::Table, r(100.0, 380.0, 300.0, 394.0), &ts, &[], &[]), Some((1, true)));
        // The wrong kind, too far (25 pt), beside it, a line in between.
        assert_eq!(place(Kind::Table, r(100.0, 256.0, 300.0, 270.0), &ts, &[], &[]), None);
        assert_eq!(place(Kind::Figure, r(100.0, 276.0, 300.0, 290.0), &ts, &[], &[]), None);
        assert_eq!(place(Kind::Figure, r(400.0, 256.0, 500.0, 270.0), &ts, &[], &[]), None);
        assert_eq!(place(Kind::Figure, r(100.0, 270.0, 300.0, 284.0), &ts, &[r(100.0, 254.0, 300.0, 266.0)], &[]), None);
        // A caption narrower than the figure that overlaps it is fine; one that only grazes it is not.
        assert_eq!(place(Kind::Figure, r(280.0, 256.0, 380.0, 270.0), &ts, &[], &[]), None);
        assert_eq!(place(Kind::Figure, r(150.0, 256.0, 250.0, 270.0), &ts, &[], &[]), Some((0, false)));
    }
}
