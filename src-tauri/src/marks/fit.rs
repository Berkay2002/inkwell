//! Two things done to a page's own content stream: shrinking it to fit between a mark's margins,
//! and finding (and cutting out) pagination artifacts other apps wrote.
//!
//! **Shrink.** A header, footer or Bates mark that asks for it has the page's content scaled
//! uniformly (never up) and centred between the bands its text uses, keeping the page size. The
//! scale is a `cm` in the `q %Inkwell` wrapper that already isolates the page's content from the
//! marks on top, followed by a clip to the visible area (so what the crop box hid stays hidden).
//! It sits in an artifact section of its own, `/Artifact <</PCMark /Shrink /HeaderFooter [t b l r]
//! …>> BDC … EMC`, whose dictionary says which kind of mark asked and for what bands: the cm is
//! rebuilt from those whenever a mark is added or removed, and the entries survive a rewrite of the
//! page's streams (autotagging and text editing merge them into one).
//!
//! **Other apps' artifacts.** ISO 32000-2 §14.8.2.2 defines `/Artifact <</Type /Pagination
//! /Subtype /Header|/Footer|/Watermark …>> BDC … EMC` and `/Type /Background`. Marks without our
//! `/PCMark` tag are found by the marked-content sequence alone. Acrobat's own private `/PieceInfo`
//! is not read (see docs/study/15-page-marks.md).

use super::apply::{add_stream, check_pages, contents_list, has_tag, set_contents, tag_of};
use super::geometry::{M, View, mul, n};
use super::{MarkKind, Style};
use crate::engine::{EngineError, Result};
use crate::textedit::content::{self as cs, Obj};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::Buffer;
use std::ops::Range;

/// The space a mark keeps clear, in points from the page edge: top, bottom, left, right.
pub(super) type Bands = [f64; 4];

/// Under this scale the page would be too small to read: the margins are refused instead.
const MIN_SCALE: f64 = 0.1;

fn invalid(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// The bands a text mark (six slots) keeps clear: a margin plus the height of the lines in the
/// slots that have text, for the top and the bottom; the left and right margins as set.
pub(super) fn text_bands(texts: &[String; 6], style: &Style, margins: &[f64; 4]) -> Bands {
    let size = style.size;
    let height = |group: &[String], margin: f64| {
        let lines = group.iter().filter(|t| !t.trim().is_empty()).map(|t| t.lines().count().max(1)).max();
        lines.map_or(0.0, |l| margin + size + size * 1.2 * (l - 1) as f64)
    };
    [height(&texts[..3], margins[0]), height(&texts[3..], margins[1]), margins[2], margins[3]]
}

/// The `cm` (user space) that puts the page's content into the box the bands leave, and the page's
/// visible rectangle in user space as `x y w h` (the content's own space, for the clip after it).
fn geometry(view: &View, b: Bands) -> Result<(M, [f64; 4])> {
    let (w, h) = (view.w, view.h);
    let [top, bottom, left, right] = b;
    let s = 1.0f64.min((h - top - bottom) / h).min((w - left - right) / w);
    if s.is_nan() || s < MIN_SCALE {
        return Err(invalid("The margins leave too little room to shrink the page. Make them smaller, or turn off \"Shrink pages to fit the margins\"."));
    }
    let (cx, cy) = ((left + w - right) / 2.0, (bottom + h - top) / 2.0);
    // The page in display space, scaled about its own centre, then moved to the centre of the box.
    let d: M = [s, 0.0, 0.0, s, cx - s * w / 2.0, cy - s * h / 2.0];
    let cm = mul(mul(view.to_display(), d), view.matrix);
    let corner = |x: f64, y: f64| (x * view.matrix[0] + y * view.matrix[2] + view.matrix[4], x * view.matrix[1] + y * view.matrix[3] + view.matrix[5]);
    let (a, c) = (corner(0.0, 0.0), corner(w, h));
    Ok((cm, [a.0.min(c.0), a.1.min(c.1), (a.0 - c.0).abs(), (a.1 - c.1).abs()]))
}

/// The `/Shrink` section for what the marks on the page asked.
fn section(view: &View, entries: &[(MarkKind, Bands)]) -> Result<String> {
    let mut all = [0.0f64; 4];
    for (_, e) in entries {
        for (a, v) in all.iter_mut().zip(e) {
            *a = a.max(*v);
        }
    }
    let (cm, clip) = geometry(view, all)?;
    let mut s = String::from("/Artifact <</PCMark /Shrink");
    for (kind, e) in entries {
        s.push_str(&format!(" /{} [{} {} {} {}]", kind.tag(), n(e[0]), n(e[1]), n(e[2]), n(e[3])));
    }
    s.push_str(">> BDC\n");
    s.push_str(&format!("{} {} {} {} {} {} cm\n", n(cm[0]), n(cm[1]), n(cm[2]), n(cm[3]), n(cm[4]), n(cm[5])));
    s.push_str(&format!("{} {} {} {} re W n\nEMC\n", n(clip[0]), n(clip[1]), n(clip[2]), n(clip[3])));
    Ok(s)
}

/// The `/Shrink` sections in a content stream: the byte range of each, and the entries of its
/// dictionary (which kind of mark asked, for which bands).
fn shrink_sections(data: &[u8]) -> Vec<(Range<usize>, Vec<(MarkKind, Bands)>)> {
    let ops = cs::parse(data);
    let mut out = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        let op = &ops[i];
        if op.op == b"BDC" && op.operands.len() >= 2 && op.operands[0].as_name() == Some(b"Artifact".as_slice()) {
            if let Obj::Dict(d) = &op.operands[1] {
                let tagged = d.iter().any(|(k, v)| k.as_slice() == b"PCMark" && v.as_name() == Some(b"Shrink".as_slice()));
                if tagged {
                    if let Some(j) = (i + 1..ops.len()).find(|&j| ops[j].op == b"EMC") {
                        let mut entries = Vec::new();
                        for (k, v) in d {
                            let kind = MarkKind::from_tag(&String::from_utf8_lossy(k));
                            if let (Some(kind), Obj::Array(a)) = (kind, v) {
                                let nums: Vec<f64> = a.iter().filter_map(Obj::as_f64).collect();
                                if let Ok(b) = <Bands>::try_from(nums) {
                                    entries.push((kind, b));
                                }
                            }
                        }
                        out.push((op.span.start..ops[j].span.end, entries));
                        i = j + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// `data` without `ranges`, as a new stream of the document (a newline where each went).
fn without(doc: &mut PdfDocument, data: &[u8], ranges: &[Range<usize>]) -> Result<PdfObject> {
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    for r in ranges {
        out.extend_from_slice(&data[at..r.start]);
        out.push(b'\n');
        at = r.end;
    }
    out.extend_from_slice(&data[at..]);
    let dict = doc.new_dict()?;
    Ok(doc.add_stream(&Buffer::from_bytes(&out)?, Some(&dict), false)?)
}

/// The page's content list with what `kind` asks of the shrink brought in (`bands`) or taken out
/// (`None`): the entries of every kind are read from the sections already there (the open wrapper's,
/// or ones left inline in a merged stream), changed, and written again as one section right after
/// the open wrapper's `q %Inkwell`. With no entry left the section is gone.
pub(super) fn set_shrink(doc: &mut PdfDocument, list: Vec<PdfObject>, view: &View, kind: MarkKind, bands: Option<Bands>) -> Result<Vec<PdfObject>> {
    let mut entries: Vec<(MarkKind, Bands)> = Vec::new();
    let mut found = false;
    let mut out: Vec<PdfObject> = Vec::with_capacity(list.len());
    // The open wrapper (the first `Wrap` stream): where it is in `out` and how many `q` it has.
    let mut open: Option<(usize, usize)> = None;
    for o in list {
        let tag = tag_of(&o);
        if tag.as_deref().is_some_and(|t| t != "Wrap") || !o.is_stream()? {
            out.push(o);
            continue;
        }
        let data = o.read_stream()?;
        let sections = if has_tag(&data) { shrink_sections(&data) } else { Vec::new() };
        found |= !sections.is_empty();
        entries.extend(sections.iter().flat_map(|(_, e)| e.iter().copied()));
        if tag.is_some() {
            if open.is_none() {
                let qs = cs::parse(&data).iter().filter(|op| op.op == b"q").count();
                open = Some((out.len(), qs));
            }
            out.push(o);
        } else if sections.is_empty() {
            out.push(o);
        } else {
            let ranges: Vec<Range<usize>> = sections.into_iter().map(|(r, _)| r).collect();
            out.push(without(doc, &data, &ranges)?);
        }
    }
    let kept = entries.iter().find(|(k, _)| *k == kind).map(|(_, b)| *b);
    entries.retain(|(k, _)| *k != kind);
    if let Some(b) = bands {
        // Adding again without removing first (or two marks of one kind) keeps the larger bands.
        let b = kept.map_or(b, |k| std::array::from_fn(|i| k[i].max(b[i])));
        entries.push((kind, b));
    }
    if entries.is_empty() && !found {
        return Ok(out);
    }
    let Some((at, qs)) = open else {
        // Nothing wraps the page's content (it has none): there is nothing to scale.
        return Ok(out);
    };
    let body = if entries.is_empty() { String::new() } else { section(view, &entries)? };
    let wrapper = format!("q %Inkwell\n{body}{}", "q\n".repeat(qs.saturating_sub(1)));
    out[at] = add_stream(doc, wrapper.as_bytes(), "Wrap", None)?;
    Ok(out)
}

/// Shrinks page `index` for a mark of `kind` that keeps `bands` clear. The mark's content is
/// already placed (so the page has its wrapper).
pub(super) fn apply_shrink(doc: &mut PdfDocument, index: u32, kind: MarkKind, bands: Bands) -> Result<()> {
    let view = View::of(&doc.load_pdf_page(index as i32)?)?;
    let mut page = doc.find_page(index as i32)?;
    let list = contents_list(&page)?;
    let list = set_shrink(doc, list, &view, kind, Some(bands))?;
    set_contents(doc, &mut page, list)
}

// ---- other apps' artifacts ---------------------------------------------------------------------------

/// A marked-content section of another app that counts as a mark of `kind`.
pub(super) struct Foreign {
    pub kind: MarkKind,
    /// `BDC` through `EMC`, nothing around it.
    pub range: Range<usize>,
    /// Cutting it out changes nothing else on the page.
    pub clean: bool,
}

/// What a quick look at a stream's bytes says about whether it may have such a section.
pub(super) fn may_have_foreign(data: &[u8]) -> bool {
    let has = |needle: &[u8]| data.windows(needle.len()).any(|w| w == needle);
    has(b"Pagination") || has(b"Background")
}

/// The kind of mark `/Artifact <<…>> BDC` is, when it is another app's: `/Type /Pagination` with a
/// `/Subtype` of Header, Footer, PageNum (header/footer), Watermark or Bates (or none), or `/Type
/// /Background`. Layout, Page, line numbers, redaction labels and anything unnamed are not marks.
fn foreign_kind(op: &cs::Op) -> Option<MarkKind> {
    if op.op != b"BDC" || op.operands.len() < 2 || op.operands[0].as_name() != Some(b"Artifact".as_slice()) {
        return None;
    }
    let Obj::Dict(d) = &op.operands[1] else { return None };
    let get = |key: &[u8]| d.iter().find(|(k, _)| k.as_slice() == key).map(|(_, v)| v);
    if get(b"PCMark").is_some() {
        return None;
    }
    match get(b"Type").and_then(Obj::as_name)? {
        b"Background" => Some(MarkKind::Background),
        b"Pagination" => match get(b"Subtype").map(|v| v.as_name()) {
            None | Some(Some(b"Header" | b"Footer" | b"PageNum")) => Some(MarkKind::HeaderFooter),
            Some(Some(b"Watermark")) => Some(MarkKind::Watermark),
            Some(Some(b"Bates")) => Some(MarkKind::Bates),
            _ => None,
        },
        _ => None,
    }
}

/// Whether cutting `inner` (the operators between a `BDC` and its `EMC`) leaves the rest of the
/// page as it was: `q`/`Q` and `BT`/`ET` balanced inside, and no `cm` outside a `q`…`Q` (it would
/// have moved what follows). Colour, font and the like set at the top level inside are not checked:
/// that would leave most files from word processors alone, and what follows sets its own.
fn clean_inside(inner: &[cs::Op]) -> bool {
    let (mut q, mut text) = (0i32, 0i32);
    for op in inner {
        match op.op.as_slice() {
            b"q" => q += 1,
            b"Q" => q -= 1,
            b"BT" => text += 1,
            b"ET" => text -= 1,
            b"cm" if q == 0 => return false,
            _ => {}
        }
        if q < 0 || text < 0 {
            return false;
        }
    }
    q == 0 && text == 0
}

/// The sections of other apps in a content stream, outermost first (what is inside one is cut with it).
pub(super) fn foreign_sections(data: &[u8]) -> Vec<Foreign> {
    let ops = cs::parse(data);
    let mut out = Vec::new();
    let mut i = 0;
    while i < ops.len() {
        let Some(kind) = foreign_kind(&ops[i]) else {
            i += 1;
            continue;
        };
        let mut depth = 1;
        let end = (i + 1..ops.len()).find(|&j| {
            match ops[j].op.as_slice() {
                b"BDC" | b"BMC" => depth += 1,
                b"EMC" => depth -= 1,
                _ => {}
            }
            depth == 0
        });
        match end {
            Some(j) => {
                out.push(Foreign { kind, range: ops[i].span.start..ops[j].span.end, clean: clean_inside(&ops[i + 1..j]) });
                i = j + 1;
            }
            // No matching end in this stream: it cannot be cut. What follows may still hold sections.
            None => {
                out.push(Foreign { kind, range: ops[i].span.start..data.len(), clean: false });
                i += 1;
            }
        }
    }
    out
}

/// Cuts the sections of other apps that count as `kind` out of `pages`. Returns how many were cut
/// and how many were left because they could not be cut cleanly.
pub fn remove_foreign(doc: &mut PdfDocument, kind: MarkKind, pages: &[u32]) -> Result<(usize, usize)> {
    crate::outline::refuse_if_certified(doc, "Removing from its pages")?;
    let count = doc.page_count()?.max(0) as u32;
    let pages = check_pages(pages, count)?;
    let (mut cut, mut left) = (0, 0);
    for &i in &pages {
        let mut page = doc.find_page(i as i32)?;
        let mut changed = false;
        let mut list = Vec::new();
        for o in contents_list(&page)? {
            if tag_of(&o).is_some() || !o.is_stream()? {
                list.push(o);
                continue;
            }
            let data = o.read_stream()?;
            if !may_have_foreign(&data) {
                list.push(o);
                continue;
            }
            let of_kind: Vec<Foreign> = foreign_sections(&data).into_iter().filter(|f| f.kind == kind).collect();
            left += of_kind.iter().filter(|f| !f.clean).count();
            let ranges: Vec<Range<usize>> = of_kind.iter().filter(|f| f.clean).map(|f| f.range.clone()).collect();
            if ranges.is_empty() {
                list.push(o);
            } else {
                cut += ranges.len();
                changed = true;
                list.push(without(doc, &data, &ranges)?);
            }
        }
        if changed {
            set_contents(doc, &mut page, list)?;
        }
    }
    Ok((cut, left))
}
