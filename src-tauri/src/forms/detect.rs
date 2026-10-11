// Adapted from pdfcraft (MIT OR Apache-2.0), crates/forms/src/detect.rs @ b8ddf71
// Changes: `page_shapes` (content-stream parsing over cos objects) is replaced by `Shapes::add_rect` /
// `add_segment`, fed from MuPDF's page drawings; Serialize derives; geometry is in the viewer's
// page space flipped so y points up (`mod.rs` does the flip).
//! Prepare a form: automatic field detection. Finds where a printed form expects answers —
//! underscore runs, rules (horizontal lines), empty boxes and small squares — and names each new
//! field from the label next to it (Acrobat's auto-naming).
//!
//! [`detect`] is a pure function over the page's words and drawn shapes.

use serde::Serialize;

/// A piece of text on the page. `rect` is `[x0, y0, x1, y1]`, y up.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: [f64; 4],
}

/// Shapes drawn on the page, y up: rectangles and straight horizontal segments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shapes {
    pub boxes: Vec<[f64; 4]>,
    pub rules: Vec<[f64; 4]>,
}

impl Shapes {
    /// A rectangle (stroked or filled); a very flat one is a rule.
    pub fn add_rect(&mut self, r: [f64; 4]) {
        if r[3] - r[1] < 1.5 && r[2] - r[0] > 1.0 {
            let y = (r[1] + r[3]) / 2.0;
            self.rules.push([r[0], y, r[2], y]);
        } else {
            self.boxes.push(r);
        }
    }

    /// A stroked segment; only horizontal ones count.
    pub fn add_segment(&mut self, a: [f64; 2], b: [f64; 2]) {
        if (a[1] - b[1]).abs() < 0.5 && (a[0] - b[0]).abs() > 1.0 {
            self.rules.push([a[0].min(b[0]), a[1], a[0].max(b[0]), a[1]]);
        }
    }
}

/// What detection proposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Text,
    CheckBox,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub kind: Kind,
    pub rect: [f64; 4],
    /// From the nearby label, made a valid field name.
    pub name: String,
}

fn mid(r: [f64; 4]) -> f64 {
    (r[1] + r[3]) / 2.0
}

/// A label's words made into a field name: letters, digits and spaces, no trailing colon.
fn clean(label: &str) -> String {
    let s: String = label.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '/' { c } else { ' ' }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.chars().take(40).collect::<String>().trim().to_string()
}

/// The label for a field at `rect`: the words to its left on the same line, else the words
/// just above it.
fn label_for(words: &[Word], rect: [f64; 4], right_of: bool) -> String {
    let h = (rect[3] - rect[1]).max(8.0);
    let same_line = |w: &Word| (mid(w.rect) - mid(rect)).abs() < h.max(10.0) * 0.7;
    let mut left: Vec<&Word> = if right_of {
        words.iter().filter(|w| same_line(w) && w.rect[0] >= rect[2] - 1.0 && w.rect[0] - rect[2] < 200.0).collect()
    } else {
        words.iter().filter(|w| same_line(w) && w.rect[2] <= rect[0] + 1.0 && rect[0] - w.rect[2] < 250.0).collect()
    };
    left.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    if right_of {
        // The words that follow the box, up to the next gap wider than a few spaces.
        let mut picked = Vec::new();
        let mut x = rect[2];
        for w in left {
            if w.rect[0] - x > 24.0 {
                break;
            }
            picked.push(w.text.as_str());
            x = w.rect[2];
        }
        return clean(&picked.join(" "));
    }
    // The words that end nearest the field, back to a wide gap.
    let mut picked: Vec<&str> = Vec::new();
    let mut x = rect[0];
    for w in left.iter().rev() {
        if x - w.rect[2] > 24.0 && !picked.is_empty() {
            break;
        }
        if w.text.chars().all(|c| c == '_') {
            break;
        }
        picked.insert(0, w.text.as_str());
        x = w.rect[0];
    }
    let l = clean(&picked.join(" "));
    if !l.is_empty() {
        return l;
    }
    // Above: the nearest line of words over the field's left part.
    let above: Vec<&Word> = words
        .iter()
        .filter(|w| w.rect[1] >= rect[3] - 1.0 && w.rect[1] - rect[3] < 24.0 && w.rect[0] < rect[2] && w.rect[2] > rect[0] - 4.0)
        .collect();
    let Some(line) = above.iter().map(|w| mid(w.rect)).min_by(|a, b| a.total_cmp(b)) else { return String::new() };
    let mut on: Vec<&&Word> = above.iter().filter(|w| (mid(w.rect) - line).abs() < 3.0).collect();
    on.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    clean(&on.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "))
}

/// Propose fields for a page. `existing` are the page's widget rectangles (no candidate
/// overlaps them); names are unique against `taken`.
pub fn detect(words: &[Word], shapes: &Shapes, existing: &[[f64; 4]], taken: &[String]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let overlaps = |a: [f64; 4], b: [f64; 4]| a[0] < b[2] - 1.0 && b[0] < a[2] - 1.0 && a[1] < b[3] - 1.0 && b[1] < a[3] - 1.0;
    let push = |kind: Kind, rect: [f64; 4], label: String, out: &mut Vec<Candidate>| {
        if existing.iter().chain(out.iter().map(|c| &c.rect)).any(|e| overlaps(*e, rect)) {
            return;
        }
        out.push(Candidate { kind, rect, name: label });
    };
    // Text that isn't underscores (labels), and underscore runs (blanks).
    let (blanks, labels): (Vec<&Word>, Vec<&Word>) = words.iter().partition(|w| w.text.len() >= 3 && w.text.chars().all(|c| c == '_'));
    let labels: Vec<Word> = labels.into_iter().cloned().collect();
    for b in blanks {
        let h = (b.rect[3] - b.rect[1]).max(10.0);
        let rect = [b.rect[0], b.rect[1], b.rect[2], b.rect[1] + (h * 1.4).clamp(12.0, 22.0)];
        let l = label_for(&labels, rect, false);
        push(Kind::Text, rect, l, &mut out);
    }
    // Small squares: check boxes, labelled by the text to their right.
    for r in &shapes.boxes {
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        if (5.0..=20.0).contains(&w) && (5.0..=20.0).contains(&h) && (w / h - 1.0).abs() < 0.25 {
            let l = label_for(&labels, *r, true);
            push(Kind::CheckBox, *r, l, &mut out);
        }
    }
    // Empty boxes big enough to write in: text fields inside them.
    for r in &shapes.boxes {
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        let has_text = labels.iter().any(|x| overlaps(x.rect, *r));
        if w >= 40.0 && (12.0..=60.0).contains(&h) && !has_text {
            let inner = [r[0] + 1.0, r[1] + 1.0, r[2] - 1.0, r[3] - 1.0];
            let l = label_for(&labels, *r, false);
            push(Kind::Text, inner, l, &mut out);
        }
    }
    // Rules: a text field sitting on the line.
    for r in &shapes.rules {
        let w = r[2] - r[0];
        if w < 40.0 {
            continue;
        }
        // A line that underlines text (or a table border with text on it) isn't a blank.
        let rect = [r[0], r[1] + 1.0, r[2], r[1] + 17.0];
        if labels.iter().any(|x| overlaps(x.rect, rect)) {
            continue;
        }
        let l = label_for(&labels, rect, false);
        push(Kind::Text, rect, l, &mut out);
    }
    // Reading order: top to bottom, left to right.
    // Rows: fields whose middles are within 8 pt of the row's first.
    out.sort_by(|a, b| mid(b.rect).total_cmp(&mid(a.rect)));
    let mut rows = Vec::with_capacity(out.len());
    let (mut row, mut start) = (0usize, f64::INFINITY);
    for c in &out {
        if start - mid(c.rect) > 8.0 {
            row += 1;
            start = mid(c.rect);
        }
        rows.push(row);
    }
    let mut keyed: Vec<(usize, Candidate)> = rows.into_iter().zip(out).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.rect[0].total_cmp(&b.1.rect[0])));
    let mut out: Vec<Candidate> = keyed.into_iter().map(|(_, c)| c).collect();
    // Unique names (Acrobat numbers repeats); unlabelled fields get Text1, Check Box1, …
    let mut used: Vec<String> = taken.to_vec();
    for c in &mut out {
        let base = if c.name.is_empty() {
            match c.kind {
                Kind::Text => "Text".to_string(),
                Kind::CheckBox => "Check Box".to_string(),
            }
        } else {
            c.name.clone()
        };
        let mut name = if c.name.is_empty() { format!("{base}1") } else { base.clone() };
        let mut n = 1;
        while used.contains(&name) {
            n += 1;
            name = format!("{base}{n}");
        }
        used.push(name.clone());
        c.name = name;
    }
    out
}
