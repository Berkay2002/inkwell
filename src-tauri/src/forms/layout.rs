//! Layout tools for form fields, as in Acrobat's Prepare Form: align, centre, distribute and match the size of a
//! selection, nudge it, duplicate a field to other pages, make rows and columns of copies, and rename a field
//! (merging it into another of its kind when asked). Each tool is one edit-core step.
//!
//! Positions are page space (what the page shows, /Rotate and the crop box applied: `Entry::rect`), written back through
//! the annotation so the inverse transform is MuPDF's. The anchor of a selection is its **last** field, as in Acrobat.

use super::author::{acro_form, follow_rename, remove_ref, seed_on_state, user_rect};
use super::scripts;
use super::*;

/// `(page, widget xref)`, as `forms_delete` takes it.
pub type Item = (u32, i32);

const MAX_ITEMS: usize = 1000;
const MAX_COPIES: usize = 500;
const MAX_PAGES: usize = 2000;
const MAX_NAME: usize = 512;
/// Smaller moves than this are not moves (float noise from the page transform).
const EPS: f32 = 0.01;

/// Every layout tool leaves a certified document alone, at any `/P` (P 3 allows form fill and comments, not moving or
/// renaming fields). Called first inside each edit closure, so a refusal records no step.
fn refuse_certified(doc: &PdfDocument) -> Result<()> {
    crate::outline::refuse_if_certified(doc, "Changing its form fields")
}

/// What an align puts on the anchor's line. `HCenter` lines the horizontal centres up (one column), `VCenter` the
/// vertical centres (one row): Acrobat's Align > Horizontally and Vertically.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    HCenter,
    VCenter,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Axis {
    Horizontal,
    Vertical,
    Both,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Dim {
    Width,
    Height,
    Both,
}

// ---- selection ---------------------------------------------------------------------------------

/// The listed widgets, in order; a widget listed twice counts once, at its last place (the anchor is the last one).
fn pick<'a>(entries: &'a [Entry], items: &[Item], min: usize, what: &str) -> Result<Vec<&'a Entry>> {
    if items.len() > MAX_ITEMS {
        return Err(bad(format!("{what} takes at most {MAX_ITEMS} fields at once")));
    }
    let mut out: Vec<&Entry> = Vec::with_capacity(items.len());
    for &(page, xref) in items {
        let e = entries
            .iter()
            .find(|e| e.xref == xref && e.xref > 0 && e.page == page)
            .ok_or_else(|| bad(format!("no field {xref} on page {}", page + 1)))?;
        out.retain(|o| o.xref != xref);
        out.push(e);
    }
    if out.len() < min {
        return Err(bad(format!("{what} needs at least {min} field{}", if min == 1 { "" } else { "s" })));
    }
    Ok(out)
}

/// A signed signature field stays where it is, and so does a field a signature covers (its /Lock, a FieldMDP).
fn guard(doc: &PdfDocument, entries: &[Entry], picked: &[&Entry]) -> Result<()> {
    let locks = scripts::locks_of(doc, entries);
    for e in picked {
        if e.kind == FieldKind::Signature {
            if geti(&e.field, "V").is_some_and(|v| v.is_dict().unwrap_or(false)) {
                return Err(bad(format!("The signature field [ {} ] is signed and stays where it is", e.name)));
            }
        } else if scripts::locked(&locks, e) {
            return Err(bad(format!("The field [ {} ] is locked by a signature", e.name)));
        }
    }
    Ok(())
}

fn one_page(picked: &[&Entry], what: &str) -> Result<u32> {
    let page = picked[0].page;
    if picked.iter().any(|e| e.page != page) {
        return Err(bad(format!("{what} works on fields of one page")));
    }
    Ok(page)
}

fn bounds_of(doc: &PdfDocument, page: u32) -> Result<[f32; 4]> {
    let b = doc.load_pdf_page(page as i32)?.bounds()?;
    Ok([b.x0, b.y0, b.x1, b.y1])
}

fn bbox(es: &[&Entry]) -> [f32; 4] {
    es.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, e| [b[0].min(e.rect[0]), b[1].min(e.rect[1]), b[2].max(e.rect[2]), b[3].max(e.rect[3])])
}

fn shifted(r: [f32; 4], dx: f32, dy: f32) -> [f32; 4] {
    [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]
}

fn pages_of_entries(es: &[&Entry]) -> Vec<u32> {
    pages_of(es.iter().map(|e| e.page).collect())
}

/// Writes the new rectangles, redraws those widgets, and returns the pages touched; `None` when nothing moved.
fn commit(doc: &mut PdfDocument, moves: Vec<(&Entry, [f32; 4])>) -> Result<Option<Vec<u32>>> {
    let moves: Vec<_> = moves.into_iter().filter(|(e, r)| (0..4).any(|i| (r[i] - e.rect[i]).abs() > EPS)).collect();
    if moves.is_empty() {
        return Ok(None);
    }
    for (_, r) in &moves {
        if !r.iter().all(|v| v.is_finite()) || r[2] - r[0] < 2.0 || r[3] - r[1] < 2.0 {
            return Err(bad("a field needs a rectangle of at least 2 x 2 points"));
        }
    }
    let mut pages: Vec<u32> = moves.iter().map(|(e, _)| e.page).collect();
    pages.sort_unstable();
    pages.dedup();
    for &pg in &pages {
        let p = doc.load_pdf_page(pg as i32)?;
        for (e, r) in moves.iter().filter(|(e, _)| e.page == pg) {
            let w = p.load_widget(e.xref)?.ok_or_else(|| bad(format!("no field {} on page {}", e.xref, pg + 1)))?;
            w.into_annotation().set_rect(rect_of(*r))?;
        }
    }
    redraw_all(doc, &moves.iter().map(|(e, _)| e.xref).collect::<Vec<_>>())?;
    Ok(Some(pages))
}

/// Rebuilds the appearance of these widgets from one fresh scan.
fn redraw_all(doc: &PdfDocument, xrefs: &[i32]) -> Result<()> {
    for e in scan(doc)?.iter().filter(|e| xrefs.contains(&e.xref)) {
        refresh(doc, e)?;
    }
    Ok(())
}

// ---- geometry ----------------------------------------------------------------------------------

/// Moves every field but the anchor (the last one) onto the anchor's edge or centre line. Same page.
pub fn align(doc: &mut PdfDocument, items: &[Item], edge: Edge) -> Result<Option<Vec<u32>>> {
    refuse_certified(doc)?;
    let entries = scan(doc)?;
    let picked = pick(&entries, items, 2, "Align")?;
    guard(doc, &entries, &picked)?;
    one_page(&picked, "Align")?;
    let a = picked[picked.len() - 1].rect;
    let moves = picked[..picked.len() - 1]
        .iter()
        .map(|e| {
            let r = e.rect;
            let (dx, dy) = match edge {
                Edge::Left => (a[0] - r[0], 0.0),
                Edge::Right => (a[2] - r[2], 0.0),
                Edge::Top => (0.0, a[1] - r[1]),
                Edge::Bottom => (0.0, a[3] - r[3]),
                Edge::HCenter => ((a[0] + a[2]) / 2.0 - (r[0] + r[2]) / 2.0, 0.0),
                Edge::VCenter => (0.0, (a[1] + a[3]) / 2.0 - (r[1] + r[3]) / 2.0),
            };
            (*e, shifted(r, dx, dy))
        })
        .collect();
    commit(doc, moves)
}

/// Centres the selection, as a group, on its page (per page when it spans several).
pub fn center(doc: &mut PdfDocument, items: &[Item], axis: Axis) -> Result<Option<Vec<u32>>> {
    refuse_certified(doc)?;
    let entries = scan(doc)?;
    let picked = pick(&entries, items, 1, "Centre")?;
    guard(doc, &entries, &picked)?;
    let mut moves = Vec::new();
    for pg in pages_of_entries(&picked) {
        let group: Vec<&Entry> = picked.iter().copied().filter(|e| e.page == pg).collect();
        let (g, b) = (bbox(&group), bounds_of(doc, pg)?);
        let dx = if axis == Axis::Vertical { 0.0 } else { (b[0] + b[2]) / 2.0 - (g[0] + g[2]) / 2.0 };
        let dy = if axis == Axis::Horizontal { 0.0 } else { (b[1] + b[3]) / 2.0 - (g[1] + g[3]) / 2.0 };
        moves.extend(group.into_iter().map(|e| (e, shifted(e.rect, dx, dy))));
    }
    commit(doc, moves)
}

/// Equal gaps between the fields along `axis`, the two outermost staying where they are. Three or more, one page.
pub fn distribute(doc: &mut PdfDocument, items: &[Item], axis: Axis) -> Result<Option<Vec<u32>>> {
    refuse_certified(doc)?;
    if axis == Axis::Both {
        return Err(bad("Distribute works along one direction at a time"));
    }
    let entries = scan(doc)?;
    let mut picked = pick(&entries, items, 3, "Distribute")?;
    guard(doc, &entries, &picked)?;
    one_page(&picked, "Distribute")?;
    let i = usize::from(axis == Axis::Vertical);
    picked.sort_by(|a, b| a.rect[i].total_cmp(&b.rect[i]));
    let (first, last) = (picked[0].rect[i], picked[picked.len() - 1].rect[i + 2]);
    let sizes: f32 = picked.iter().map(|e| e.rect[i + 2] - e.rect[i]).sum();
    let gap = (last - first - sizes) / (picked.len() - 1) as f32;
    let mut at = first;
    let moves = picked
        .iter()
        .map(|e| {
            let d = at - e.rect[i];
            at += e.rect[i + 2] - e.rect[i] + gap;
            (*e, if i == 0 { shifted(e.rect, d, 0.0) } else { shifted(e.rect, 0.0, d) })
        })
        .collect();
    commit(doc, moves)
}

/// Gives every field the anchor's width and/or height, keeping its top-left corner (pulled back onto the page if the
/// new size would run off it).
pub fn match_size(doc: &mut PdfDocument, items: &[Item], dim: Dim) -> Result<Option<Vec<u32>>> {
    refuse_certified(doc)?;
    let entries = scan(doc)?;
    let picked = pick(&entries, items, 2, "Set to the same size")?;
    guard(doc, &entries, &picked)?;
    let page = one_page(&picked, "Set to the same size")?;
    let b = bounds_of(doc, page)?;
    let a = picked[picked.len() - 1].rect;
    let (w, h) = (a[2] - a[0], a[3] - a[1]);
    let moves = picked[..picked.len() - 1]
        .iter()
        .map(|e| {
            let r = e.rect;
            let (nw, nh) = (if dim == Dim::Height { r[2] - r[0] } else { w }, if dim == Dim::Width { r[3] - r[1] } else { h });
            let x0 = r[0].min(b[2] - nw).max(b[0].min(r[0]));
            let y0 = r[1].min(b[3] - nh).max(b[1].min(r[1]));
            (*e, [x0, y0, x0 + nw, y0 + nh])
        })
        .collect();
    commit(doc, moves)
}

/// Moves the selection by `(dx, dy)` points (page space, y down), the group kept on its page.
pub fn nudge(doc: &mut PdfDocument, items: &[Item], dx: f32, dy: f32) -> Result<Option<Vec<u32>>> {
    refuse_certified(doc)?;
    if !dx.is_finite() || !dy.is_finite() || dx.abs() > 10_000.0 || dy.abs() > 10_000.0 {
        return Err(bad("that is not a distance on the page"));
    }
    let entries = scan(doc)?;
    let picked = pick(&entries, items, 1, "Move")?;
    guard(doc, &entries, &picked)?;
    let mut moves = Vec::new();
    for pg in pages_of_entries(&picked) {
        let group: Vec<&Entry> = picked.iter().copied().filter(|e| e.page == pg).collect();
        let (g, b) = (bbox(&group), bounds_of(doc, pg)?);
        // A group already past an edge may not go further past it; one inside stays inside.
        let ddx = if dx < 0.0 { dx.max((b[0] - g[0]).min(0.0)) } else { dx.min((b[2] - g[2]).max(0.0)) };
        let ddy = if dy < 0.0 { dy.max((b[1] - g[1]).min(0.0)) } else { dy.min((b[3] - g[3]).max(0.0)) };
        moves.extend(group.into_iter().map(|e| (e, shifted(e.rect, ddx, ddy))));
    }
    commit(doc, moves)
}

// ---- structure: kids, parents, copies ----------------------------------------------------------

/// Keys that belong to the field, not to one widget: they go to the parent when a field gets a second widget.
const FIELD_KEYS: [&str; 14] = ["FT", "T", "TU", "TM", "Ff", "V", "DV", "AA", "MaxLen", "Opt", "Lock", "RV", "I", "TI"];
/// Keys a new widget takes from the one it copies (looked up the parent chain, so inherited ones are written in).
const WIDGET_KEYS: [&str; 11] = ["Type", "Subtype", "F", "MK", "BS", "Border", "C", "DA", "Q", "DS", "A"];

fn replace_ref(a: &mut PdfObject, old: i32, new: &PdfObject) -> Result<()> {
    for i in 0..a.len()? as i32 {
        if a.get_array(i)?.is_some_and(|v| v.is_indirect().unwrap_or(false) && v.as_indirect().ok() == Some(old)) {
            a.array_put(i, new.clone())?;
        }
    }
    Ok(())
}

/// Where an object hangs: its parent's /Kids, or /AcroForm /Fields when it has no parent. Also the calculation order.
fn replace_in_form(doc: &mut PdfDocument, parent: Option<&PdfObject>, old: i32, new: &PdfObject) -> Result<()> {
    match parent {
        Some(p) => {
            if let Some(mut k) = get(p, "Kids") {
                replace_ref(&mut k, old, new)?;
            }
        }
        None => {
            if let Some(mut f) = get(&acro_form(doc)?, "Fields") {
                replace_ref(&mut f, old, new)?;
            }
        }
    }
    if let Some(mut co) = get(&acro_form(doc)?, "CO") {
        replace_ref(&mut co, old, new)?;
    }
    Ok(())
}

fn detach(doc: &mut PdfDocument, parent: Option<&PdfObject>, num: i32) -> Result<()> {
    match parent {
        Some(p) => {
            if let Some(mut k) = get(p, "Kids") {
                remove_ref(&mut k, num)?;
            }
        }
        None => {
            if let Some(mut f) = get(&acro_form(doc)?, "Fields") {
                remove_ref(&mut f, num)?;
            }
        }
    }
    if let Some(mut co) = get(&acro_form(doc)?, "CO") {
        remove_ref(&mut co, num)?;
    }
    Ok(())
}

/// A group node that lost its last kid goes too, up the chain.
fn prune(doc: &mut PdfDocument, mut node: Option<PdfObject>) -> Result<()> {
    for _ in 0..32 {
        let Some(n) = node else { break };
        if !get(&n, "Kids").is_some_and(|k| k.len().unwrap_or(1) == 0) {
            break;
        }
        let parent = get(&n, "Parent");
        if let Ok(num) = n.as_indirect() {
            detach(doc, parent.as_ref(), num)?;
        }
        node = parent;
    }
    Ok(())
}

/// Makes the widget of a field that is its own widget (one dictionary) a kid of a new parent that holds the field's
/// keys, as a field with several widgets is stored. Returns a reference to the parent. The field, in the form and
/// the calculation order, is now the parent.
fn into_kids(doc: &mut PdfDocument, e: &Entry) -> Result<PdfObject> {
    if e.xref <= 0 {
        return Err(bad("this field's widget is stored inside the page and cannot be given a sibling"));
    }
    // A reference, so every write lands in the document's own dictionary.
    let mut w = doc.new_indirect(e.xref, 0)?;
    let old_parent = get(&w, "Parent");
    let mut p = doc.new_dict()?;
    for k in FIELD_KEYS {
        if let Some(v) = get(&w, k) {
            p.dict_put(k, v)?;
            w.dict_delete(k)?;
        }
    }
    if let Some(op) = &old_parent {
        p.dict_put("Parent", op.clone())?;
    }
    let mut kids = doc.new_array()?;
    kids.array_push(w.clone())?;
    p.dict_put("Kids", kids)?;
    let p_ref = doc.add_object(&p)?;
    replace_in_form(doc, old_parent.as_ref(), e.xref, &p_ref)?;
    w.dict_put("Parent", p_ref.clone())?;
    Ok(p_ref)
}

/// A new widget dictionary, added to the document. `field` (the fields keys, written in whole) makes it a field of
/// its own named `field.0`; without it the widget is a kid and takes them from its parent.
fn new_widget(doc: &mut PdfDocument, src: &Entry, rect: Rect, parent: &PdfObject, field: Option<&str>) -> Result<PdfObject> {
    let mut n = doc.new_dict()?;
    for k in WIDGET_KEYS {
        if let Some(v) = geti(&src.obj, k) {
            n.dict_put(k, v.clone())?;
        }
    }
    let mut r = doc.new_array()?;
    rect.encode_into(&mut r)?;
    n.dict_put("Rect", r)?;
    n.dict_put("Parent", parent.clone())?;
    let value = geti(&src.field, "V");
    if let Some(name) = field {
        put_str(&mut n, "T", name)?;
        for k in ["FT", "Ff", "TU", "DV", "MaxLen", "Opt"] {
            if let Some(v) = geti(&src.obj, k) {
                n.dict_put(k, v.clone())?;
            }
        }
        // Calculations are not copied (the copy is not in the calculation order); formats and validation are.
        if let Some(aa) = geti(&src.obj, "AA").filter(|a| a.is_dict().unwrap_or(false)) {
            let mut c = aa.clone();
            c.dict_delete("C")?;
            if c.dict_len()? > 0 {
                n.dict_put("AA", c)?;
            }
        }
        // An empty value of its own: it must not take the original's through the parent.
        match src.kind {
            FieldKind::Checkbox => put_name(&mut n, "V", "Off")?,
            FieldKind::Text | FieldKind::Combo | FieldKind::List => put_str(&mut n, "V", "")?,
            _ => {}
        }
    }
    if src.kind == FieldKind::Checkbox {
        let on = on_state(&src.obj).unwrap_or_else(|| "Yes".into());
        seed_on_state(doc, &mut n, &on)?;
        let is_on = field.is_none() && value.as_ref().and_then(text).is_some_and(|v| v == on);
        put_name(&mut n, "AS", if is_on { &on } else { "Off" })?;
    }
    Ok(doc.add_object(&n)?)
}

/// Puts a widget on a page's /Annots, in an array of the page's own (a shared one would put it on every page that
/// lists it).
fn put_on_page(doc: &mut PdfDocument, page: u32, w: PdfObject) -> Result<()> {
    let mut page_obj = doc.find_page(page as i32)?;
    match get(&page_obj, "Annots").filter(|a| a.is_array().unwrap_or(false)) {
        Some(a) if a.is_indirect().unwrap_or(false) => {
            let mut own = a.copy_array()?;
            own.array_push(w)?;
            page_obj.dict_put("Annots", own)?;
        }
        Some(mut a) => a.array_push(w)?,
        None => {
            let mut a = doc.new_array()?;
            a.array_push(w)?;
            page_obj.dict_put("Annots", a)?;
        }
    }
    Ok(())
}

fn fit_inside(r: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    if w > b[2] - b[0] + EPS || h > b[3] - b[1] + EPS {
        return None;
    }
    let x0 = r[0].min(b[2] - w).max(b[0]);
    let y0 = r[1].min(b[3] - h).max(b[1]);
    Some([x0, y0, x0 + w, y0 + h])
}

fn single(doc: &PdfDocument, entries: &[Entry], item: Item, what: &str) -> Result<()> {
    let picked = pick(entries, &[item], 1, what)?;
    guard(doc, entries, &picked)?;
    match picked[0].kind {
        FieldKind::Radio => Err(bad("A radio button belongs to its group: draw another one with the radio tool instead")),
        FieldKind::Signature => Err(bad("A signature field is not copied")),
        _ => Ok(()),
    }
}

/// The same field again on other pages: new widgets of the field (one name, one value), at the same place, pulled
/// back inside a page that is smaller. Returns the pages touched.
pub fn duplicate_to_pages(doc: &mut PdfDocument, item: Item, pages: &[u32]) -> Result<Vec<u32>> {
    refuse_certified(doc)?;
    let entries = scan(doc)?;
    single(doc, &entries, item, "Duplicate")?;
    let e = pick(&entries, &[item], 1, "Duplicate")?[0];
    let count = doc.page_count()?.max(0) as u32;
    let mut pages: Vec<u32> = pages.iter().copied().filter(|p| *p != e.page).collect();
    pages.sort_unstable();
    pages.dedup();
    if pages.len() > MAX_PAGES {
        return Err(bad(format!("Duplicate takes at most {MAX_PAGES} pages")));
    }
    if pages.is_empty() {
        return Err(bad("choose at least one other page"));
    }
    if let Some(p) = pages.iter().find(|p| **p >= count) {
        return Err(bad(format!("the document has no page {}", p + 1)));
    }
    // Rectangles first: a page too small refuses the whole step before anything is written.
    let mut rects = Vec::with_capacity(pages.len());
    for &pg in &pages {
        let r = fit_inside(e.rect, bounds_of(doc, pg)?).ok_or_else(|| bad(format!("the field does not fit on page {}", pg + 1)))?;
        rects.push(user_rect(doc, pg, r)?);
    }
    let parent = if e.node == e.xref { into_kids(doc, e)? } else { get(&e.obj, "Parent").ok_or_else(|| bad("the field has no parent"))? };
    let mut kids = get(&parent, "Kids").ok_or_else(|| bad("the field has no /Kids"))?;
    let mut made = Vec::with_capacity(pages.len());
    for (&pg, rect) in pages.iter().zip(rects) {
        let w = new_widget(doc, e, rect, &parent, None)?;
        kids.array_push(w.clone())?;
        put_on_page(doc, pg, w.clone())?;
        made.push(w.as_indirect()?);
    }
    redraw_all(doc, &made)?;
    Ok(pages)
}

/// What `copies` did.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Copied {
    pub made: usize,
    /// Places in the grid that would run off the page.
    pub skipped: usize,
}

/// A grid of `rows` x `cols` copies of the field, the original at the top left, `gap_x` and `gap_y` points between
/// them; to the right and down. The original becomes `name.0` and the copies `name.1`, `name.2`, ... in row order
/// (Acrobat's naming); a place that would run off the page is skipped.
pub fn copies(doc: &mut PdfDocument, item: Item, rows: u32, cols: u32, gap_x: f32, gap_y: f32) -> Result<(Copied, Vec<u32>)> {
    refuse_certified(doc)?;
    let cells = u64::from(rows) * u64::from(cols);
    if rows == 0 || cols == 0 || cells < 2 {
        return Err(bad("choose at least two places: more than one row or column"));
    }
    if cells - 1 > MAX_COPIES as u64 {
        return Err(bad(format!("at most {MAX_COPIES} copies at once")));
    }
    if !gap_x.is_finite() || !gap_y.is_finite() || gap_x.abs() > 10_000.0 || gap_y.abs() > 10_000.0 {
        return Err(bad("that is not a distance on the page"));
    }
    let entries = scan(doc)?;
    single(doc, &entries, item, "Create copies")?;
    let e = pick(&entries, &[item], 1, "Create copies")?[0];
    if entries.iter().filter(|o| o.node == e.node).count() != 1 {
        return Err(bad("This field has several widgets. Copy a field that has one."));
    }
    if aa_js(e, "C").is_some() {
        return Err(bad("A calculated field is not copied: its calculation would not follow"));
    }
    let b = bounds_of(doc, e.page)?;
    let (w, h) = (e.rect[2] - e.rect[0], e.rect[3] - e.rect[1]);
    let mut spots = Vec::new();
    let mut skipped = 0;
    for r in 0..rows {
        for c in 0..cols {
            if r == 0 && c == 0 {
                continue;
            }
            let x0 = e.rect[0] + c as f32 * (w + gap_x);
            let y0 = e.rect[1] + r as f32 * (h + gap_y);
            let rect = [x0, y0, x0 + w, y0 + h];
            if rect[0] >= b[0] - EPS && rect[1] >= b[1] - EPS && rect[2] <= b[2] + EPS && rect[3] <= b[3] + EPS {
                spots.push(user_rect(doc, e.page, rect)?);
            } else {
                skipped += 1;
            }
        }
    }
    if spots.is_empty() {
        return Err(bad("no copy fits on the page at that spacing"));
    }

    // The field becomes a group `name` of fields `0`, `1`, ...: the original is `name.0`.
    let mut w_obj = doc.new_indirect(e.xref, 0)?;
    let owner = if e.node == e.xref {
        let leaf = get(&e.obj, "T").as_ref().and_then(text).filter(|t| !t.is_empty()).ok_or_else(|| bad("this field has no name"))?;
        let old_parent = get(&e.obj, "Parent");
        let mut p = doc.new_dict()?;
        put_str(&mut p, "T", &leaf)?;
        if let Some(op) = &old_parent {
            p.dict_put("Parent", op.clone())?;
        }
        let mut kids = doc.new_array()?;
        kids.array_push(w_obj.clone())?;
        p.dict_put("Kids", kids)?;
        let p_ref = doc.add_object(&p)?;
        // The calculation order keeps its entry: the widget stays a field (`name.0`), it only moved down.
        match &old_parent {
            Some(op) => {
                if let Some(mut k) = get(op, "Kids") {
                    replace_ref(&mut k, e.xref, &p_ref)?;
                }
            }
            None => {
                if let Some(mut f) = get(&acro_form(doc)?, "Fields") {
                    replace_ref(&mut f, e.xref, &p_ref)?;
                }
            }
        }
        w_obj.dict_put("Parent", p_ref.clone())?;
        p_ref
    } else {
        get(&e.obj, "Parent").ok_or_else(|| bad("the field has no parent"))?
    };
    put_str(&mut w_obj, "T", "0")?;
    let mut kids = get(&owner, "Kids").ok_or_else(|| bad("the field has no /Kids"))?;
    let mut made = Vec::with_capacity(spots.len());
    for (i, rect) in spots.iter().enumerate() {
        let n = new_widget(doc, e, *rect, &owner, Some(&(i + 1).to_string()))?;
        kids.array_push(n.clone())?;
        put_on_page(doc, e.page, n.clone())?;
        made.push(n.as_indirect()?);
    }
    follow_rename(doc, &entries, &e.name, &format!("{}.0", e.name))?;
    redraw_all(doc, &made)?;
    Ok((Copied { made: made.len(), skipped }, vec![e.page]))
}

// ---- rename ------------------------------------------------------------------------------------

fn valid_name(name: &str) -> Result<()> {
    if name.is_empty() || name.split('.').any(|p| p.trim().is_empty()) {
        return Err(bad("a field needs a name, and no part of a dotted name may be empty"));
    }
    if name.len() > MAX_NAME || name.chars().any(char::is_control) {
        return Err(bad("that is not a usable field name"));
    }
    Ok(())
}

/// `name` for the field of `item`, the full dotted name. A field of the same kind that has the name is refused unless
/// `merge`, which makes this field's widgets part of it (the other's value stays). Returns the pages touched, `None`
/// when the name is the one it has.
pub fn rename(doc: &mut PdfDocument, item: Item, name: &str, merge: bool) -> Result<Option<Vec<u32>>> {
    refuse_certified(doc)?;
    let full = name.trim();
    valid_name(full)?;
    let entries = scan(doc)?;
    let picked = pick(&entries, &[item], 1, "Rename")?;
    guard(doc, &entries, &picked)?;
    let e = picked[0];
    if full == e.name {
        return Ok(None);
    }
    let mine: Vec<&Entry> = entries.iter().filter(|o| o.node == e.node).collect();
    if mine.iter().any(|m| m.xref <= 0) {
        return Err(bad("this field's widgets are stored inside the page and cannot be renamed"));
    }
    let mut target: Option<&Entry> = None;
    for o in entries.iter().filter(|o| o.node != e.node) {
        if o.name == full {
            if o.kind != e.kind {
                return Err(bad(format!("A {} named {full} already exists, so this {} cannot take that name or merge into it", noun(o.kind), noun(e.kind))));
            }
            if !merge {
                return Err(bad(format!("A field named {full} already exists")));
            }
            if matches!(e.kind, FieldKind::Radio | FieldKind::Signature) {
                return Err(bad("Radio groups and signature fields are not merged"));
            }
            target = Some(o);
        } else if o.name.starts_with(&format!("{full}.")) {
            return Err(bad(format!("{full} is already the name of a group of fields")));
        } else if full.starts_with(&format!("{}.", o.name)) {
            return Err(bad(format!("{} is a field, so no field can be named below it", o.name)));
        }
    }
    if let Some(t) = target {
        guard(doc, &entries, &[t])?;
    }
    let (parent_name, leaf) = full.rsplit_once('.').unwrap_or(("", full));
    let field = if e.node == e.xref { doc.new_indirect(e.xref, 0)? } else { e.field.clone() };
    let field_num = if e.node == e.xref { Some(e.xref) } else { field.as_indirect().ok().filter(|n| *n > 0) };
    let old_parent = if e.node == e.xref { get(&e.obj, "Parent") } else { get(&field, "Parent") };

    // The group the field goes into: the target's, or `parent_name`'s (made when it does not exist).
    let owner: Option<PdfObject> = if let Some(t) = target {
        Some(if t.node == t.xref { into_kids(doc, t)? } else { get(&t.obj, "Parent").ok_or_else(|| bad("the field has no parent"))? })
    } else if parent_name.is_empty() {
        None
    } else {
        Some(group_for(doc, &entries, parent_name)?)
    };

    if target.is_some() {
        // Merge: every widget becomes a kid of the target's group; this field's own keys are dropped with its node.
        let owner = owner.ok_or_else(|| bad("the target has no group"))?;
        let mut kids = get(&owner, "Kids").ok_or_else(|| bad("the target has no /Kids"))?;
        for m in &mine {
            let mut w = doc.new_indirect(m.xref, 0)?;
            let from = get(&w, "Parent");
            if m.node == m.xref {
                detach(doc, from.as_ref(), m.xref)?;
                for k in FIELD_KEYS {
                    if get(&w, k).is_some() {
                        w.dict_delete(k)?;
                    }
                }
            } else if let Some(f) = &from {
                if let Some(mut k) = get(f, "Kids") {
                    remove_ref(&mut k, m.xref)?;
                }
            }
            w.dict_put("Parent", owner.clone())?;
            kids.array_push(w)?;
        }
        if let (Some(n), true) = (field_num, e.node != e.xref) {
            detach(doc, old_parent.as_ref(), n)?;
            prune(doc, old_parent)?;
        } else if e.node == e.xref {
            prune(doc, old_parent)?;
        }
    } else {
        let mut f = field;
        let same_group = parent_name == old_parent.as_ref().map(full_name).unwrap_or_default();
        put_str(&mut f, "T", leaf)?;
        if !same_group {
            let num = field_num.ok_or_else(|| bad("this field is stored inside another object and cannot be moved"))?;
            detach(doc, old_parent.as_ref(), num)?;
            // Calculations keep their place: detach dropped the entry, so it goes back.
            if let Some(co) = get(&acro_form(doc)?, "CO") {
                if entries.iter().any(|o| o.node == e.node && aa_js(o, "C").is_some()) {
                    let mut co = co;
                    co.array_push(doc.new_indirect(num, 0)?)?;
                }
            }
            match &owner {
                Some(g) => {
                    f.dict_put("Parent", g.clone())?;
                    get(g, "Kids").ok_or_else(|| bad("the group has no /Kids"))?.array_push(doc.new_indirect(num, 0)?)?;
                }
                None => {
                    f.dict_delete("Parent")?;
                    get(&acro_form(doc)?, "Fields").ok_or_else(|| bad("the form has no field list"))?.array_push(doc.new_indirect(num, 0)?)?;
                }
            }
            prune(doc, old_parent)?;
        }
    }
    follow_rename(doc, &entries, &e.name, full)?;
    Ok(Some(pages_of(mine.iter().map(|m| m.page).collect())))
}

/// The group node with this full name (every part made when missing), as a reference.
fn group_for(doc: &mut PdfDocument, entries: &[Entry], name: &str) -> Result<PdfObject> {
    // The groups the file already has, from the parents of its fields.
    let mut groups: HashMap<String, PdfObject> = HashMap::new();
    for e in entries {
        let mut cur = if e.node == e.xref { get(&e.obj, "Parent") } else { get(&e.field, "Parent") };
        for _ in 0..32 {
            let Some(c) = cur else { break };
            if get(&c, "T").is_some() {
                groups.entry(full_name(&c)).or_insert_with(|| c.clone());
            }
            cur = get(&c, "Parent");
        }
    }
    let mut at: Option<PdfObject> = None;
    let mut path = String::new();
    for part in name.split('.') {
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(part);
        let next = match groups.get(&path) {
            Some(g) => g.clone(),
            None => {
                let mut n = doc.new_dict()?;
                put_str(&mut n, "T", part)?;
                if let Some(p) = &at {
                    n.dict_put("Parent", p.clone())?;
                }
                n.dict_put("Kids", doc.new_array()?)?;
                let r = doc.add_object(&n)?;
                match &at {
                    Some(p) => get(p, "Kids").ok_or_else(|| bad("the group has no /Kids"))?.array_push(r.clone())?,
                    None => get(&acro_form(doc)?, "Fields").ok_or_else(|| bad("the form has no field list"))?.array_push(r.clone())?,
                }
                groups.insert(path.clone(), r.clone());
                r
            }
        };
        at = Some(next);
    }
    at.ok_or_else(|| bad("a field needs a name"))
}

// ---- engine wiring: one undo step each ---------------------------------------------------------

fn fields_label(n: usize) -> String {
    format!("{n} field{}", if n == 1 { "" } else { "s" })
}

fn pages_label(items: &[Item]) -> String {
    let mut p: Vec<u32> = items.iter().map(|i| i.0).collect();
    p.sort_unstable();
    p.dedup();
    match p.as_slice() {
        [one] => format!("on p. {}", one + 1),
        _ => "across pages".into(),
    }
}

pub fn align_one(eng: &Engine, doc: DocId, items: &[Item], edge: Edge, source: EditSource) -> Result<EditState> {
    let how = match edge {
        Edge::Left => "left",
        Edge::Right => "right",
        Edge::Top => "top",
        Edge::Bottom => "bottom",
        Edge::HCenter => "horizontal centres",
        Edge::VCenter => "vertical centres",
    };
    let label = format!("Align {} by {how} {}", fields_label(items.len()), pages_label(items));
    eng.apply_custom_unless_noop(doc, label, source, false, |d| align(d, items, edge))
}

pub fn center_one(eng: &Engine, doc: DocId, items: &[Item], axis: Axis, source: EditSource) -> Result<EditState> {
    let label = format!("Centre {} on the page {}", fields_label(items.len()), pages_label(items));
    eng.apply_custom_unless_noop(doc, label, source, false, |d| center(d, items, axis))
}

pub fn distribute_one(eng: &Engine, doc: DocId, items: &[Item], axis: Axis, source: EditSource) -> Result<EditState> {
    let label = format!("Distribute {} {}", fields_label(items.len()), pages_label(items));
    eng.apply_custom_unless_noop(doc, label, source, false, |d| distribute(d, items, axis))
}

pub fn match_size_one(eng: &Engine, doc: DocId, items: &[Item], dim: Dim, source: EditSource) -> Result<EditState> {
    let label = format!("Set {} to the same size {}", fields_label(items.len()), pages_label(items));
    eng.apply_custom_unless_noop(doc, label, source, false, |d| match_size(d, items, dim))
}

pub fn nudge_one(eng: &Engine, doc: DocId, items: &[Item], dx: f32, dy: f32, source: EditSource) -> Result<EditState> {
    let label = format!("Move {} {}", fields_label(items.len()), pages_label(items));
    eng.apply_custom_unless_noop(doc, label, source, false, |d| nudge(d, items, dx, dy))
}

pub fn duplicate_one(eng: &Engine, doc: DocId, item: Item, pages: &[u32], source: EditSource) -> Result<EditState> {
    let label = format!("Duplicate field to {} page{}", pages.len(), if pages.len() == 1 { "" } else { "s" });
    eng.apply_custom(doc, label, source, false, |d| duplicate_to_pages(d, item, pages))
}

pub fn copies_one(eng: &Engine, doc: DocId, item: Item, rows: u32, cols: u32, gap: (f32, f32), source: EditSource) -> Result<(EditState, Copied)> {
    let mut done = Copied { made: 0, skipped: 0 };
    let state = eng.apply_custom(doc, format!("Create {rows} x {cols} copies on p. {}", item.0 + 1), source, false, |d| {
        let (c, pages) = copies(d, item, rows, cols, gap.0, gap.1)?;
        done = c;
        Ok(pages)
    })?;
    Ok((state, done))
}

pub fn rename_one(eng: &Engine, doc: DocId, item: Item, name: &str, merge: bool, source: EditSource) -> Result<EditState> {
    let label = if merge { "Merge field".to_string() } else { "Rename field".to_string() };
    eng.apply_custom_unless_noop(doc, label, source, false, |d| rename(d, item, name, merge))
}
