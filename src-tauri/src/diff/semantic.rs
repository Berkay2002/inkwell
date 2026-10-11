//! Semantic comparison of two PDFs: the typed change list.

use super::pages::{lis, match_pages};
use super::snap::{self, ImageRef, PageSnap, Snap, Word};
use super::types::*;
use similar::{Algorithm, DiffOp, capture_diff_slices_deadline};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Time budget of one sequence diff (a page's words, the bookmark list).
const DIFF_BUDGET: Duration = Duration::from_millis(200);

/// Myers diff with a deadline: past it, the rest is reported as one coarse replace (still a
/// correct diff, just not minimal), so a huge or fully rewritten page can't hang the compare.
pub(super) fn seq_diff<T: Hash + Eq + Ord>(a: &[T], b: &[T]) -> Vec<DiffOp> {
    capture_diff_slices_deadline(Algorithm::Myers, a, b, Some(Instant::now() + DIFF_BUDGET))
}

/// Equal words between two edits that still count as one edit region.
const GAP: usize = 3;
/// Unchanged words kept around a text edit for context.
const CTX: usize = 2;

/// Stops a comparison at the next page boundary when `cancel` is set, and reports where it is.
struct Ctl<'a> {
    cancel: &'a AtomicBool,
    on: &'a mut dyn FnMut(Progress),
}

impl Ctl<'_> {
    fn tick(&mut self, stage: Stage, done: u32, total: u32) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(DiffError::Cancelled);
        }
        (self.on)(Progress { stage, done, total });
        Ok(())
    }
}

pub fn compare(a: &[u8], b: &[u8], opts: &CompareOptions, cancel: &AtomicBool, on: &mut dyn FnMut(Progress)) -> Result<Comparison> {
    let mut ctl = Ctl { cancel, on };
    if a == b {
        // Same bytes: nothing to look at, but still validate the file and report its pages.
        let n = snap::open_doc(a, Side::A, opts.password_a.as_deref())?.page_count()?.max(0) as u32;
        let ids: Vec<Option<u32>> = (0..n).map(Some).collect();
        return Ok(Comparison {
            pages: PageMap { a_pages: n, b_pages: n, b_to_a: ids, removed_a: vec![] },
            changes: vec![],
            identical: true,
        });
    }
    let sa = Snap::open(a, Side::A, opts.password_a.as_deref(), &mut |d, t| ctl.tick(Stage::First, d, t).is_ok())?;
    let sb = Snap::open(b, Side::B, opts.password_b.as_deref(), &mut |d, t| ctl.tick(Stage::Second, d, t).is_ok())?;
    ctl.tick(Stage::Pages, 0, 0)?;
    let b2a = match_pages(&sa, &sb);
    let (na, nb) = (sa.pages.len(), sb.pages.len());
    let mut a_seen = vec![false; na];
    for i in b2a.iter().flatten() {
        a_seen[*i] = true;
    }
    let map = PageMap {
        a_pages: na as u32,
        b_pages: nb as u32,
        b_to_a: b2a.iter().map(|m| m.map(|i| i as u32)).collect(),
        removed_a: (0..na).filter(|i| !a_seen[*i]).map(|i| i as u32).collect(),
    };

    let mut ch: Vec<Change> = Vec::new();
    // An unmatched A page and an unmatched B page in the same gap between matched pages are one
    // replaced page (paired in order), not a removal plus an addition.
    let gap_b = |j: usize| b2a[..j].iter().filter(|m| m.is_some()).count();
    let a_matched: Vec<usize> = b2a.iter().flatten().copied().collect();
    let gap_a = |i: usize| a_matched.iter().filter(|&&x| x < i).count();
    let mut free_a: Vec<usize> = map.removed_a.iter().map(|&i| i as usize).collect();
    for (j, m) in b2a.iter().enumerate() {
        if m.is_some() {
            continue;
        }
        match free_a.iter().position(|&i| gap_a(i) == gap_b(j)) {
            Some(k) => {
                let i = free_a.remove(k);
                ch.push(page_change(ChangeKind::PageReplaced, Some(i), Some(j), format!("Page {} replaced", j + 1), Detail::None));
            }
            None => ch.push(page_change(ChangeKind::PageAdded, None, Some(j), format!("Page {} added", j + 1), Detail::None)),
        }
    }
    for i in free_a {
        ch.push(page_change(ChangeKind::PageRemoved, Some(i), None, format!("Page {} removed", i + 1), Detail::None));
    }
    // moves: matched pages not on the longest increasing run of A-indices in B order
    let matched: Vec<(usize, usize)> = b2a.iter().enumerate().filter_map(|(j, m)| m.map(|i| (i, j))).collect();
    let seq: Vec<usize> = matched.iter().map(|m| m.0).collect();
    let keep: HashSet<usize> = lis(&seq).into_iter().collect();
    for (k, &(i, j)) in matched.iter().enumerate() {
        if !keep.contains(&k) {
            ch.push(page_change(
                ChangeKind::PageMoved,
                Some(i),
                Some(j),
                format!("Page {} moved to {}", i + 1, j + 1),
                Detail::PageMoved { from: i as u32, to: j as u32 },
            ));
        }
    }
    for (k, &(i, j)) in matched.iter().enumerate() {
        ctl.tick(Stage::Pages, k as u32, matched.len() as u32)?;
        compare_page(&sa, &sb, i, j, &mut ch);
    }

    ctl.tick(Stage::Document, 0, 0)?;
    metadata_changes(&sa, &sb, opts, &mut ch);
    bookmark_changes(&sa, &sb, &map, &mut ch);
    attachment_changes(&sa, &sb, &mut ch);

    ch.sort_by_key(|c| (c.b_page.or(c.a_page).unwrap_or(u32::MAX), c.kind));
    let identical = ch.is_empty();
    Ok(Comparison { pages: map, changes: ch, identical })
}

fn page_change(kind: ChangeKind, a: Option<usize>, b: Option<usize>, label: String, detail: Detail) -> Change {
    Change {
        kind,
        a_page: a.map(|p| p as u32),
        b_page: b.map(|p| p as u32),
        rect_a: None,
        rect_b: None,
        label,
        detail,
    }
}

fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(40).collect::<String>())
    } else {
        s.to_string()
    }
}

fn close(a: &Rect4, b: &Rect4, tol: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

fn union(rs: impl IntoIterator<Item = Rect4>) -> Option<Rect4> {
    rs.into_iter().reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

fn area(r: &Rect4) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

fn overlap(a: &Rect4, b: &Rect4) -> f32 {
    area(&[a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])])
}

fn iou(a: &Rect4, b: &Rect4) -> f32 {
    let i = overlap(a, b);
    let u = area(a) + area(b) - i;
    if u <= 0.0 { 0.0 } else { i / u }
}

fn center_in(w: &Rect4, r: &Rect4) -> bool {
    let (cx, cy) = ((w[0] + w[2]) / 2.0, (w[1] + w[3]) / 2.0);
    cx >= r[0] && cx <= r[2] && cy >= r[1] && cy <= r[3]
}

// ---- one matched page pair --------------------------------------------------------------------

fn compare_page(sa: &Snap, sb: &Snap, ia: usize, ib: usize, ch: &mut Vec<Change>) {
    let (pa, pb) = (&sa.pages[ia], &sb.pages[ib]);
    let n = ib + 1;
    let (a_page, b_page) = (Some(ia), Some(ib));

    if pa.rotation != pb.rotation {
        ch.push(page_change(
            ChangeKind::PageRotated,
            a_page,
            b_page,
            format!("Page {n} rotated from {}° to {}°", pa.rotation, pb.rotation),
            Detail::PageRotated { from: pa.rotation, to: pb.rotation },
        ));
    }
    for ((name, ra), (_, rb)) in pa.boxes.iter().zip(&pb.boxes) {
        let same = match (ra, rb) {
            (None, None) => true,
            (Some(x), Some(y)) => close(x, y, 0.5),
            _ => false,
        };
        if !same {
            ch.push(page_change(
                ChangeKind::PageBoxChanged,
                a_page,
                b_page,
                format!("{name} of page {n} changed"),
                Detail::PageBox { name: name.to_string(), old: *ra, new: *rb },
            ));
        }
    }

    let content_changed = pa.fp != pb.fp;
    let vanished = annotation_changes(pa, pb, ia, ib, content_changed, ch);
    widget_changes(pa, pb, ia, ib, ch);
    if content_changed {
        content_changes(sa, sb, ia, ib, &vanished, ch);
    }
}

fn human_annot(subtype: &str) -> &str {
    match subtype {
        "Text" => "Note",
        "FreeText" => "Text box",
        "StrikeOut" => "Strikethrough",
        "Ink" => "Drawing",
        "Square" => "Rectangle",
        "Circle" => "Ellipse",
        "FileAttachment" => "Attachment",
        "Redact" => "Redaction mark",
        s => s,
    }
}

fn annot_same(a: &AnnotInfo, b: &AnnotInfo) -> Vec<&'static str> {
    let mut d = Vec::new();
    if a.contents != b.contents {
        d.push("contents");
    }
    let colour_same = match (&a.colour, &b.colour) {
        (None, None) => true,
        (Some(x), Some(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (p - q).abs() < 0.01),
        _ => false,
    };
    if !colour_same {
        d.push("colour");
    }
    if !close(&a.rect, &b.rect, 0.5) {
        d.push("rect");
    }
    d
}

/// Returns the page-space rects of Redact annotations that disappeared while the page content
/// changed (they were applied, not deleted).
fn annotation_changes(pa: &PageSnap, pb: &PageSnap, ia: usize, ib: usize, content_changed: bool, ch: &mut Vec<Change>) -> Vec<Rect4> {
    let (a, b) = (&pa.annots, &pb.annots);
    let (mut ma, mut mb): (Vec<Option<usize>>, Vec<Option<usize>>) = (vec![None; a.len()], vec![None; b.len()]);
    // 1. same /NM, 2. identical, 3. same type and overlapping (or same text)
    for pass in 0..3 {
        let mut cands: Vec<(f32, usize, usize)> = Vec::new();
        for (x, p) in a.iter().enumerate().filter(|(x, _)| ma[*x].is_none()) {
            for (y, q) in b.iter().enumerate().filter(|(y, _)| mb[*y].is_none()) {
                if p.subtype != q.subtype {
                    continue;
                }
                let s = match pass {
                    0 if p.name.is_some() && p.name == q.name => 3.0,
                    1 if annot_same(p, q).is_empty() => 2.0,
                    2 => {
                        let i = iou(&p.rect, &q.rect);
                        if i >= 0.3 || (p.contents == q.contents && !p.contents.is_empty()) { i + (p.contents == q.contents) as u8 as f32 } else { continue }
                    }
                    _ => continue,
                };
                cands.push((s, x, y));
            }
        }
        cands.sort_by(|p, q| q.0.total_cmp(&p.0));
        for (_, x, y) in cands {
            if ma[x].is_none() && mb[y].is_none() {
                ma[x] = Some(y);
                mb[y] = Some(x);
            }
        }
    }

    let n = ib + 1;
    let mut vanished = Vec::new();
    for (x, p) in a.iter().enumerate() {
        match ma[x] {
            None if p.subtype == "Redact" && content_changed => vanished.push(p.rect),
            None => ch.push(Change {
                kind: ChangeKind::AnnotationRemoved,
                a_page: Some(ia as u32),
                b_page: Some(ib as u32),
                rect_a: Some(p.rect),
                rect_b: None,
                label: format!("{} removed on p. {n}{}", human_annot(&p.subtype), with_text(&p.contents)),
                detail: Detail::Annotation { subtype: p.subtype.clone(), old: Some(p.clone()), new: None, changed: vec![] },
            }),
            Some(y) => {
                let q = &b[y];
                let d = annot_same(p, q);
                if !d.is_empty() {
                    let mut parts = Vec::new();
                    if d.contains(&"contents") {
                        parts.push(format!("text '{}' -> '{}'", clip(&p.contents), clip(&q.contents)));
                    }
                    if d.contains(&"colour") {
                        parts.push("colour".into());
                    }
                    if d.contains(&"rect") {
                        parts.push("moved or resized".into());
                    }
                    ch.push(Change {
                        kind: ChangeKind::AnnotationChanged,
                        a_page: Some(ia as u32),
                        b_page: Some(ib as u32),
                        rect_a: Some(p.rect),
                        rect_b: Some(q.rect),
                        label: format!("{} changed on p. {n}: {}", human_annot(&p.subtype), parts.join(", ")),
                        detail: Detail::Annotation {
                            subtype: p.subtype.clone(),
                            old: Some(p.clone()),
                            new: Some(q.clone()),
                            changed: d.iter().map(|s| s.to_string()).collect(),
                        },
                    });
                }
            }
        }
    }
    for q in b.iter().enumerate().filter(|(y, _)| mb[*y].is_none()).map(|(_, q)| q) {
        ch.push(Change {
            kind: ChangeKind::AnnotationAdded,
            a_page: Some(ia as u32),
            b_page: Some(ib as u32),
            rect_a: None,
            rect_b: Some(q.rect),
            label: format!("{} added on p. {n}{}", human_annot(&q.subtype), with_text(&q.contents)),
            detail: Detail::Annotation { subtype: q.subtype.clone(), old: None, new: Some(q.clone()), changed: vec![] },
        });
    }
    vanished
}

fn with_text(s: &str) -> String {
    if s.trim().is_empty() { String::new() } else { format!(": '{}'", clip(s)) }
}

fn widget_changes(pa: &PageSnap, pb: &PageSnap, ia: usize, ib: usize, ch: &mut Vec<Change>) {
    let first = |ws: &'_ [snap::WidgetInfo]| -> HashMap<String, usize> {
        let mut m = HashMap::new();
        for (i, w) in ws.iter().enumerate() {
            m.entry(w.name.clone()).or_insert(i);
        }
        m
    };
    let (fa, fb) = (first(&pa.widgets), first(&pb.widgets));
    let mut names: Vec<&String> = fa.keys().chain(fb.keys()).collect();
    names.sort();
    names.dedup();
    let show = |v: &Option<String>| match v.as_deref() {
        None | Some("") => "(empty)".to_string(),
        Some(s) => format!("'{}'", clip(s)),
    };
    for name in names {
        let (wa, wb) = (fa.get(name).map(|&i| &pa.widgets[i]), fb.get(name).map(|&i| &pb.widgets[i]));
        let (old, new) = (wa.and_then(|w| w.value.clone()), wb.and_then(|w| w.value.clone()));
        if wa.is_some() && wb.is_some() && old.clone().unwrap_or_default() == new.clone().unwrap_or_default() {
            continue;
        }
        ch.push(Change {
            kind: ChangeKind::FormFieldChanged,
            a_page: Some(ia as u32),
            b_page: Some(ib as u32),
            rect_a: wa.map(|w| w.rect),
            rect_b: wb.map(|w| w.rect),
            label: format!("Form field '{name}' changed on p. {}: {} -> {}", ib + 1, show(&old), show(&new)),
            detail: Detail::FormField { name: name.clone(), old, new },
        });
    }
}

// ---- content (text, images, OCR, redactions) --------------------------------------------------

fn content_changes(sa: &Snap, sb: &Snap, ia: usize, ib: usize, vanished: &[Rect4], ch: &mut Vec<Change>) {
    let (pa, pb) = (&sa.pages[ia], &sb.pages[ib]);
    let n = ib + 1;
    let mut explained = false;

    explained |= image_changes(sa, sb, ia, ib, ch);

    let ocr_added = pb.ocr && !pa.ocr;
    let mut wa: Vec<Word> = sa.words(ia).to_vec();
    let mut wb: Vec<Word> = sb.words(ib).to_vec();
    if ocr_added {
        explained = true;
        ch.push(Change {
            kind: ChangeKind::OcrLayerAdded,
            a_page: Some(ia as u32),
            b_page: Some(ib as u32),
            rect_a: None,
            rect_b: union(wb.iter().map(|w| w.rect)),
            label: format!("Text layer (OCR) added on p. {n}"),
            detail: Detail::Ocr { words: wb.len() as u32 },
        });
    }

    if !vanished.is_empty() {
        explained = true;
        let removed = wa.iter().filter(|w| vanished.iter().any(|r| center_in(&w.rect, r))).count() as u32;
        wa.retain(|w| !vanished.iter().any(|r| center_in(&w.rect, r)));
        wb.retain(|w| !vanished.iter().any(|r| center_in(&w.rect, r)));
        ch.push(Change {
            kind: ChangeKind::RedactionApplied,
            a_page: Some(ia as u32),
            b_page: Some(ib as u32),
            rect_a: union(vanished.iter().copied()),
            rect_b: None,
            label: format!("Redaction applied on p. {n} ({} {} removed)", removed, if removed == 1 { "word" } else { "words" }),
            detail: Detail::Redaction { via: RedactionVia::Annotation, rects: vanished.to_vec(), words_removed: removed },
        });
    }

    if !ocr_added {
        let before = ch.len();
        text_changes(sa, sb, ia, ib, &wa, &wb, ch);
        let new = &ch[before..];
        let only_format = !new.is_empty() && new.iter().all(|c| c.kind == ChangeKind::FormatChanged);
        if only_format {
            // A restyled word explains the page only when nothing else it paints moved: otherwise a font tweak
            // would hide a graphics edit. A recoloured word often has its colour set outside the text object.
            let recoloured = new.iter().any(|c| matches!(&c.detail, Detail::Format { props, .. } if props.iter().any(|p| p.name == "colour")));
            explained |= if recoloured { pa.gfx_nc == pb.gfx_nc } else { pa.gfx == pb.gfx };
        } else {
            explained |= !new.is_empty();
        }
    }

    if !explained {
        ch.push(page_change(
            ChangeKind::PageContentChanged,
            Some(ia),
            Some(ib),
            format!("Page {n} content changed (graphics or layout, no text change)"),
            Detail::None,
        ));
    }
}

fn image_changes(sa: &Snap, sb: &Snap, ia: usize, ib: usize, ch: &mut Vec<Change>) -> bool {
    let (mut ra, mut rb): (Vec<&ImageRef>, Vec<&ImageRef>) = (sa.pages[ia].images.iter().collect(), sb.pages[ib].images.iter().collect());
    // drop images present on both sides (multiset)
    let mut k = 0;
    while k < ra.len() {
        if let Some(pos) = rb.iter().position(|y| y.hash == ra[k].hash) {
            rb.remove(pos);
            ra.remove(k);
        } else {
            k += 1;
        }
    }
    if ra.is_empty() && rb.is_empty() {
        return false;
    }
    // pair leftovers by resource name first, then in order
    let mut pairs: Vec<(Option<&ImageRef>, Option<&ImageRef>)> = Vec::new();
    let mut rb_left: Vec<Option<&ImageRef>> = rb.into_iter().map(Some).collect();
    let mut ra_left: Vec<&ImageRef> = Vec::new();
    for x in ra {
        if let Some(slot) = rb_left.iter_mut().find(|s| s.is_some_and(|y| y.name == x.name)) {
            pairs.push((Some(x), slot.take()));
        } else {
            ra_left.push(x);
        }
    }
    let mut rb_rest = rb_left.into_iter().flatten();
    for x in ra_left {
        pairs.push((Some(x), rb_rest.next()));
    }
    for y in rb_rest {
        pairs.push((None, Some(y)));
    }
    let (rects_a, rects_b) = (sa.image_rects(ia), sb.image_rects(ib));
    let single = pairs.len() == 1;
    let info = |i: &ImageRef| ImageInfo { name: i.name.clone(), width: i.width, height: i.height, hash: i.hash.iter().take(8).map(|b| format!("{b:02x}")).collect() };
    let n = ib + 1;
    for (x, y) in pairs {
        let what = match (x, y) {
            (Some(_), Some(_)) => "replaced",
            (None, _) => "added",
            _ => "removed",
        };
        ch.push(Change {
            kind: ChangeKind::ImageReplaced,
            a_page: Some(ia as u32),
            b_page: Some(ib as u32),
            rect_a: if single && rects_a.len() == 1 && x.is_some() { Some(rects_a[0]) } else { None },
            rect_b: if single && rects_b.len() == 1 && y.is_some() { Some(rects_b[0]) } else { None },
            label: format!("Image {what} on p. {n}"),
            detail: Detail::Image { old: x.map(info), new: y.map(info) },
        });
    }
    true
}

fn text_changes(sa: &Snap, sb: &Snap, ia: usize, ib: usize, wa: &[Word], wb: &[Word], ch: &mut Vec<Change>) {
    let ta: Vec<&str> = wa.iter().map(|w| w.text.as_str()).collect();
    let tb: Vec<&str> = wb.iter().map(|w| w.text.as_str()).collect();
    let mut atoms: Vec<(RunOp, Option<usize>, Option<usize>)> = Vec::new();
    for op in seq_diff(&ta, &tb) {
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                atoms.extend((0..len).map(|k| (RunOp::Equal, Some(old_index + k), Some(new_index + k))))
            }
            DiffOp::Delete { old_index, old_len, .. } => atoms.extend((0..old_len).map(|k| (RunOp::Delete, Some(old_index + k), None))),
            DiffOp::Insert { new_index, new_len, .. } => atoms.extend((0..new_len).map(|k| (RunOp::Insert, None, Some(new_index + k)))),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                atoms.extend((0..old_len).map(|k| (RunOp::Delete, Some(old_index + k), None)));
                atoms.extend((0..new_len).map(|k| (RunOp::Insert, None, Some(new_index + k))));
            }
        }
    }
    format_changes(ia, ib, wa, wb, &atoms, ch);
    // edit regions: changed atoms separated by at most GAP unchanged words
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (i, _) in atoms.iter().enumerate().filter(|(_, a)| a.0 != RunOp::Equal) {
        match groups.last_mut() {
            Some(g) if i - g.1 - 1 <= GAP => g.1 = i,
            _ => groups.push((i, i)),
        }
    }
    if groups.is_empty() {
        return;
    }
    let n = ib + 1;
    let mut new_fills: Option<Vec<Rect4>> = None;
    for &(s, e) in &groups {
        // regions are more than 2 * CTX words apart, so their context never overlaps
        let (lo, hi) = (s.saturating_sub(CTX), (e + CTX).min(atoms.len() - 1));
        let mut runs: Vec<TextRun> = Vec::new();
        let (mut del, mut ins) = (Vec::new(), Vec::new());
        let (mut rects_a, mut rects_b) = (Vec::new(), Vec::new());
        for &(op, x, y) in &atoms[lo..=hi] {
            let word_a = x.map(|i| &wa[i]);
            let word_b = y.map(|i| &wb[i]);
            let text = word_a.or(word_b).map(|w| w.text.clone()).unwrap_or_default();
            match op {
                RunOp::Delete => {
                    del.push(text.clone());
                    rects_a.push(word_a.unwrap().rect);
                }
                RunOp::Insert => {
                    ins.push(text.clone());
                    rects_b.push(word_b.unwrap().rect);
                }
                RunOp::Equal => {}
            }
            match runs.last_mut() {
                Some(r) if r.op == op => {
                    r.text.push(' ');
                    r.text.push_str(&text);
                    r.boxes_a.extend(word_a.map(|w| w.rect));
                    r.boxes_b.extend(word_b.map(|w| w.rect));
                }
                _ => runs.push(TextRun {
                    op,
                    text,
                    boxes_a: word_a.map(|w| w.rect).into_iter().collect(),
                    boxes_b: word_b.map(|w| w.rect).into_iter().collect(),
                }),
            }
        }
        // text gone and an opaque box now sits where it was: a redaction done by drawing
        if ins.is_empty() {
            let fills = new_fills.get_or_insert_with(|| new_opaque_fills(sa, sb, ia, ib));
            let covered = rects_a.iter().filter(|w| fills.iter().any(|f| overlap(w, f) >= 0.8 * area(w))).count();
            if !rects_a.is_empty() && covered == rects_a.len() {
                ch.push(Change {
                    kind: ChangeKind::RedactionApplied,
                    a_page: Some(ia as u32),
                    b_page: Some(ib as u32),
                    rect_a: union(rects_a.iter().copied()),
                    rect_b: None,
                    label: format!("Redaction applied on p. {n} ({} {} removed)", del.len(), if del.len() == 1 { "word" } else { "words" }),
                    detail: Detail::Redaction { via: RedactionVia::OpaqueBox, rects: rects_a.clone(), words_removed: del.len() as u32 },
                });
                continue;
            }
        }
        let (d, i) = (clip(&del.join(" ")), clip(&ins.join(" ")));
        let label = match (del.is_empty(), ins.is_empty()) {
            (false, false) => format!("Text changed on p. {n}: '{d}' -> '{i}'"),
            (true, false) => format!("Text added on p. {n}: '{i}'"),
            _ => format!("Text removed on p. {n}: '{d}'"),
        };
        ch.push(Change {
            kind: ChangeKind::TextChanged,
            a_page: Some(ia as u32),
            b_page: Some(ib as u32),
            rect_a: union(rects_a),
            rect_b: union(rects_b),
            label,
            detail: Detail::Text { runs },
        });
    }
}

/// What two spellings of a font name must share to be the same font: "Arial,Bold", "Arial-BoldMT" and "ArialMT-Bold"
/// are one face. Lower case, "," and "-" alike, and the "MT" / "PS" / "PSMT" foundry suffixes dropped from each part.
pub(super) fn font_key(name: &str) -> String {
    let lower = name.to_ascii_lowercase().replace(' ', "");
    let mut parts: Vec<String> = Vec::new();
    for part in lower.split([',', '-']).filter(|p| !p.is_empty()) {
        let mut p = part;
        loop {
            let q = ["psmt", "mt", "ps"].iter().find_map(|s| p.strip_suffix(s).filter(|r| !r.is_empty()));
            match q {
                Some(r) => p = r,
                None => break,
            }
        }
        parts.push(p.replace("oblique", "italic"));
    }
    parts.join("-")
}

type FormatRun<'a> = (Vec<FormatProp>, Vec<&'a Word>, Vec<&'a Word>);

/// Same words, set differently: consecutive equal words whose font, size or colour changed the same way
/// are one change.
fn format_changes(ia: usize, ib: usize, wa: &[Word], wb: &[Word], atoms: &[(RunOp, Option<usize>, Option<usize>)], ch: &mut Vec<Change>) {
    let colour_far = |p: u32, q: u32| (0..3).any(|k| ((p >> (8 * k)) & 0xFF).abs_diff((q >> (8 * k)) & 0xFF) > 8);
    let props = |x: &Word, y: &Word| -> Vec<FormatProp> {
        let (sx, sy) = (&x.style, &y.style);
        let mut v = Vec::new();
        if sx.font != sy.font && font_key(&sx.font) != font_key(&sy.font) {
            v.push(FormatProp { name: "font".into(), old: sx.font.to_string(), new: sy.font.to_string() });
        }
        if sx.size2 != sy.size2 {
            let pt = |s: i32| if s % 2 == 0 { format!("{}", s / 2) } else { format!("{}.5", s / 2) };
            v.push(FormatProp { name: "size".into(), old: pt(sx.size2), new: pt(sy.size2) });
        }
        if colour_far(sx.rgb, sy.rgb) {
            v.push(FormatProp { name: "colour".into(), old: format!("#{:06x}", sx.rgb), new: format!("#{:06x}", sy.rgb) });
        }
        v
    };
    let mut run: Option<FormatRun> = None;
    let mut done: Vec<FormatRun> = Vec::new();
    for &(op, x, y) in atoms {
        let p = match (op, x, y) {
            (RunOp::Equal, Some(x), Some(y)) => Some((props(&wa[x], &wb[y]), &wa[x], &wb[y])).filter(|(p, _, _)| !p.is_empty()),
            _ => None,
        };
        let same = matches!((&p, &run), (Some((p, _, _)), Some(r)) if r.0 == *p);
        if let (true, Some((_, x, y)), Some(r)) = (same, &p, run.as_mut()) {
            r.1.push(x);
            r.2.push(y);
        } else {
            done.extend(run.take());
            run = p.map(|(p, x, y)| (p, vec![x], vec![y]));
        }
    }
    done.extend(run);
    let n = ib + 1;
    for (props, xs, ys) in done {
        let text = clip(&ys.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "));
        let what = props.iter().map(|p| format!("{} {} -> {}", p.name, p.old, p.new)).collect::<Vec<_>>().join(", ");
        ch.push(Change {
            kind: ChangeKind::FormatChanged,
            a_page: Some(ia as u32),
            b_page: Some(ib as u32),
            rect_a: union(xs.iter().map(|w| w.rect)),
            rect_b: union(ys.iter().map(|w| w.rect)),
            label: format!("Formatting changed on p. {n}: '{text}': {what}"),
            detail: Detail::Format { text, props },
        });
    }
}

/// Opaque fills on B's page that A's page did not have (page-sized backgrounds ignored).
fn new_opaque_fills(sa: &Snap, sb: &Snap, ia: usize, ib: usize) -> Vec<Rect4> {
    let page_area = sb.pages[ib].boxes[0].1.map_or(f32::MAX, |r| area(&r));
    let old = sa.opaque_fills(ia);
    sb.opaque_fills(ib)
        .into_iter()
        .filter(|f| area(f) < 0.9 * page_area && !old.iter().any(|g| close(f, g, 1.0)))
        .collect()
}

// ---- document level ---------------------------------------------------------------------------

/// Producer / tool bookkeeping that changes on every save: never worth a line in the change list.
pub(super) fn is_noise(source: MetadataSource, key: &str, creation_unchanged: bool) -> bool {
    match source {
        MetadataSource::Info => matches!(key, "Producer" | "Creator" | "ModDate" | "Trapped"),
        MetadataSource::Xmp => {
            key.starts_with("pdfx:PTEX")
                || matches!(
                    key,
                    "xmp:CreatorTool"
                        | "xmp:ModifyDate"
                        | "xmp:MetadataDate"
                        | "pdf:Producer"
                        | "pdf:Trapped"
                        | "dc:format"
                        | "xmpMM:DocumentID"
                        | "xmpMM:InstanceID"
                        | "xmpMM:VersionID"
                        | "xmpMM:History"
                        | "xmpMM:OriginalDocumentID"
                        | "xmpMM:RenditionClass"
                )
                || (key == "xmp:CreateDate" && creation_unchanged)
        }
    }
}

fn info_map(s: &Snap) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let Ok(trailer) = s.doc.trailer() else { return m };
    let Some(info) = snap::gd(&trailer, "Info").filter(|i| i.is_dict().unwrap_or(false)) else { return m };
    if let Ok(it) = info.dict_iter() {
        for (k, v) in it.flatten() {
            let key = String::from_utf8_lossy(&k.as_name().unwrap_or_default()).into_owned();
            let val = if v.is_string().unwrap_or(false) {
                snap::text_string(&v)
            } else if v.is_name().unwrap_or(false) {
                Some(String::from_utf8_lossy(&v.as_name().unwrap_or_default()).into_owned())
            } else {
                None
            };
            if let Some(val) = val {
                m.insert(key, val);
            }
        }
    }
    m
}

/// Leaf elements and rdf:Description attributes of the XMP packet (`prefix:name` -> text, list items joined).
fn xmp_map(s: &Snap) -> BTreeMap<String, String> {
    let mut m: BTreeMap<String, String> = BTreeMap::new();
    let Some(meta) = s.doc.catalog().ok().and_then(|c| snap::gd(&c, "Metadata")) else { return m };
    let bytes = meta.read_stream().unwrap_or_default();
    let xml = String::from_utf8_lossy(&bytes);
    let mut stack: Vec<String> = Vec::new();
    let mut rest: &str = &xml;
    let mut text = String::new();
    let add = |m: &mut BTreeMap<String, String>, key: &str, val: &str| {
        let val = val.trim();
        if !val.is_empty() {
            m.entry(key.to_string()).and_modify(|e| { e.push_str("; "); e.push_str(val); }).or_insert_with(|| val.to_string());
        }
    };
    while let Some(lt) = rest.find('<') {
        text.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            text.clear();
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim();
            if stack.last().map(String::as_str) == Some(name) {
                // a leaf: no child element closed since its start tag
                let key = if name == "rdf:li" {
                    stack.iter().rev().find(|n| !n.starts_with("rdf:")).cloned()
                } else if name.starts_with("rdf:") { None } else { Some(name.to_string()) };
                if let Some(k) = key.filter(|_| !text.trim().is_empty()) {
                    add(&mut m, &k, &text);
                }
                stack.pop();
            }
            text.clear();
            continue;
        }
        let selfclose = tag.ends_with('/');
        let body = tag.trim_end_matches('/');
        let name = body.split_whitespace().next().unwrap_or("");
        if name == "rdf:Description" || selfclose {
            // attributes carry simple properties
            let mut attrs = &body[name.len()..];
            while let Some(eq) = attrs.find("=\"") {
                let key = attrs[..eq].trim().rsplit(char::is_whitespace).next().unwrap_or("");
                let after = &attrs[eq + 2..];
                let Some(end) = after.find('"') else { break };
                if key.contains(':') && !key.starts_with("xmlns") && key != "rdf:about" && key != "rdf:parseType" {
                    add(&mut m, key, &after[..end]);
                }
                attrs = &after[end + 1..];
            }
        }
        if !selfclose {
            stack.push(name.to_string());
        }
        text.clear();
    }
    m
}

/// All real metadata differences as ONE change "Metadata changed (n)"; the fields are in its detail.
fn metadata_changes(sa: &Snap, sb: &Snap, opts: &CompareOptions, ch: &mut Vec<Change>) {
    let (ia, ib) = (info_map(sa), info_map(sb));
    let creation_unchanged = ia.get("CreationDate") == ib.get("CreationDate");
    let sets = [(MetadataSource::Info, ia, ib), (MetadataSource::Xmp, xmp_map(sa), xmp_map(sb))];
    let mut fields = Vec::new();
    for (source, a, b) in sets {
        let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
        for k in keys {
            if !opts.include_volatile_metadata && is_noise(source, k, creation_unchanged) {
                continue;
            }
            // An empty value and a missing one are the same to a reader (Acrobat writes empty Author/Title).
            let get = |m: &BTreeMap<String, String>| m.get(k).filter(|v| !v.trim().is_empty()).cloned();
            let (old, new) = (get(&a), get(&b));
            if old != new {
                fields.push(MetadataField { source, field: k.clone(), old, new });
            }
        }
    }
    if fields.is_empty() {
        return;
    }
    ch.push(Change {
        kind: ChangeKind::MetadataChanged,
        a_page: None,
        b_page: None,
        rect_a: None,
        rect_b: None,
        label: format!("Metadata changed ({})", fields.len()),
        detail: Detail::Metadata { fields },
    });
}

fn flatten_outline(o: &[mupdf::Outline], depth: u32, out: &mut Vec<(u32, String, Option<u32>)>) {
    for item in o {
        out.push((depth, item.title.trim().to_string(), item.dest.map(|d| d.loc.page_number.max(0) as u32)));
        flatten_outline(&item.down, depth + 1, out);
    }
}

fn bookmark_changes(sa: &Snap, sb: &Snap, map: &PageMap, ch: &mut Vec<Change>) {
    let (mut la, mut lb) = (Vec::new(), Vec::new());
    flatten_outline(&sa.doc.outlines().unwrap_or_default(), 0, &mut la);
    flatten_outline(&sb.doc.outlines().unwrap_or_default(), 0, &mut lb);
    let a_to_b = map.a_to_b();
    let key = |e: &(u32, String, Option<u32>)| format!("{}\u{1}{}", e.0, e.1);
    let (ka, kb): (Vec<String>, Vec<String>) = (la.iter().map(key).collect(), lb.iter().map(key).collect());
    let mut items: Vec<BookmarkChange> = Vec::new();
    let removed = |e: &(u32, String, Option<u32>)| BookmarkChange { op: BookmarkOp::Removed, depth: e.0, title: e.1.clone(), old_title: None, old_page: e.2, new_page: None };
    let added = |e: &(u32, String, Option<u32>)| BookmarkChange { op: BookmarkOp::Added, depth: e.0, title: e.1.clone(), old_title: None, old_page: None, new_page: e.2 };
    for op in seq_diff(&ka, &kb) {
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                for k in 0..len {
                    let (x, y) = (&la[old_index + k], &lb[new_index + k]);
                    // A's target page translated into B's numbering, so a moved page is not a retarget
                    let target = x.2.and_then(|p| a_to_b.get(p as usize).copied().flatten());
                    if x.2.is_some() != y.2.is_some() || (x.2.is_some() && target != y.2) {
                        items.push(BookmarkChange { op: BookmarkOp::Retargeted, depth: y.0, title: y.1.clone(), old_title: None, old_page: x.2, new_page: y.2 });
                    }
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => items.extend(la[old_index..old_index + old_len].iter().map(removed)),
            DiffOp::Insert { new_index, new_len, .. } => items.extend(lb[new_index..new_index + new_len].iter().map(added)),
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                for k in 0..old_len.max(new_len) {
                    match (la.get(old_index + k).filter(|_| k < old_len), lb.get(new_index + k).filter(|_| k < new_len)) {
                        (Some(x), Some(y)) if x.0 == y.0 => items.push(BookmarkChange {
                            op: BookmarkOp::Renamed,
                            depth: y.0,
                            title: y.1.clone(),
                            old_title: Some(x.1.clone()),
                            old_page: x.2,
                            new_page: y.2,
                        }),
                        (x, y) => {
                            items.extend(x.map(removed));
                            items.extend(y.map(added));
                        }
                    }
                }
            }
        }
    }
    if items.is_empty() {
        return;
    }
    let count = |op| items.iter().filter(|i| i.op == op).count();
    let parts: Vec<String> = [
        (BookmarkOp::Added, "added"),
        (BookmarkOp::Removed, "removed"),
        (BookmarkOp::Renamed, "renamed"),
        (BookmarkOp::Retargeted, "pointing elsewhere"),
    ]
    .iter()
    .filter(|(op, _)| count(*op) > 0)
    .map(|(op, w)| format!("{} {w}", count(*op)))
    .collect();
    ch.push(Change {
        kind: ChangeKind::BookmarksChanged,
        a_page: None,
        b_page: None,
        rect_a: None,
        rect_b: None,
        label: format!("Bookmarks changed: {}", parts.join(", ")),
        detail: Detail::Bookmarks { items },
    });
}

fn attachments(s: &Snap) -> BTreeMap<String, (u64, [u8; 32])> {
    let mut m = BTreeMap::new();
    for f in s.doc.embedded_files().unwrap_or_default() {
        let data = s.doc.load_embedded_file(&f.name).ok().flatten().unwrap_or_default();
        m.insert(f.name.clone(), (data.len() as u64, *blake3::hash(&data).as_bytes()));
    }
    m
}

fn attachment_changes(sa: &Snap, sb: &Snap, ch: &mut Vec<Change>) {
    let (a, b) = (attachments(sa), attachments(sb));
    let mut items = Vec::new();
    for (name, (size, h)) in &a {
        match b.get(name) {
            None => items.push(AttachmentChange { op: AttachmentOp::Removed, name: name.clone(), old_size: Some(*size), new_size: None }),
            Some((s2, h2)) if h2 != h => items.push(AttachmentChange { op: AttachmentOp::Changed, name: name.clone(), old_size: Some(*size), new_size: Some(*s2) }),
            _ => {}
        }
    }
    for (name, (size, _)) in &b {
        if !a.contains_key(name) {
            items.push(AttachmentChange { op: AttachmentOp::Added, name: name.clone(), old_size: None, new_size: Some(*size) });
        }
    }
    if items.is_empty() {
        return;
    }
    let names = |op| items.iter().filter(|i| i.op == op).map(|i| i.name.clone()).collect::<Vec<_>>();
    let mut parts = Vec::new();
    for (op, w) in [(AttachmentOp::Added, "added"), (AttachmentOp::Removed, "removed"), (AttachmentOp::Changed, "changed")] {
        let n = names(op);
        if !n.is_empty() {
            parts.push(format!("{w} {}", n.join(", ")));
        }
    }
    ch.push(Change {
        kind: ChangeKind::AttachmentsChanged,
        a_page: None,
        b_page: None,
        rect_a: None,
        rect_b: None,
        label: format!("Attachments: {}", parts.join("; ")),
        detail: Detail::Attachments { items },
    });
}
