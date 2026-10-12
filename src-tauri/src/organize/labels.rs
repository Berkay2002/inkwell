//! Page labels: the document's /PageLabels number tree as a list of sections, the label every page
//! shows (i, ii, iii / 1, 2 / A-1), and Acrobat's "Page labels" dialog as one edit.
//!
//! A section starts at a page index and runs to the next section. Labelling pages `from..=to`
//! writes a section at `from` and, when pages follow, one at `to + 1` that carries on what that
//! page showed before, so the dialog never changes pages outside its range (as in Acrobat).

use super::{Result, bad, get, put_int, put_name, put_str};
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum LabelStyle {
    /// Prefix only.
    None,
    #[default]
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperAlpha,
    LowerAlpha,
}

impl LabelStyle {
    fn name(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Decimal => Some("D"),
            Self::UpperRoman => Some("R"),
            Self::LowerRoman => Some("r"),
            Self::UpperAlpha => Some("A"),
            Self::LowerAlpha => Some("a"),
        }
    }

    fn from_name(n: &str) -> Self {
        match n {
            "D" => Self::Decimal,
            "R" => Self::UpperRoman,
            "r" => Self::LowerRoman,
            "A" => Self::UpperAlpha,
            "a" => Self::LowerAlpha,
            _ => Self::None,
        }
    }
}

fn one() -> u32 {
    1
}

/// One section of the numbering.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LabelRange {
    /// 0-based index of the section's first page.
    pub page: u32,
    pub style: LabelStyle,
    #[serde(default)]
    pub prefix: String,
    /// The number the first page of the section shows.
    #[serde(default = "one")]
    pub start: u32,
}

impl LabelRange {
    /// What the `offset`-th page of the section is called.
    pub fn label(&self, offset: u32) -> String {
        let n = self.start.saturating_add(offset);
        let number = match self.style {
            LabelStyle::None => String::new(),
            LabelStyle::Decimal => n.to_string(),
            LabelStyle::UpperRoman => roman(n).to_uppercase(),
            LabelStyle::LowerRoman => roman(n),
            LabelStyle::UpperAlpha => alpha(n).to_uppercase(),
            LabelStyle::LowerAlpha => alpha(n),
        };
        format!("{}{number}", self.prefix)
    }
}

/// i, ii, iii … (lower case); numbers over 3999 fall back to decimal.
fn roman(mut n: u32) -> String {
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    const T: [(u32, &str); 13] =
        [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// a … z, aa … zz, aaa … (the PDF way: the letter repeats, it does not carry).
fn alpha(n: u32) -> String {
    if n == 0 {
        return String::new();
    }
    let letter = (b'a' + ((n - 1) % 26) as u8) as char;
    std::iter::repeat_n(letter, ((n - 1) / 26 + 1) as usize).collect()
}

const MAX_ENTRIES: usize = 100_000;
/// Everything a walk may look at in all: nodes, `/Nums` pairs and `/Kids` entries together. The depth
/// cap alone does not stop a node whose `/Kids` are itself many times over.
const MAX_WORK: usize = 1_000_000;

/// The sections, sorted by page. A document whose first section starts after page 0 gets the
/// implicit decimal section the spec describes in front of it. No /PageLabels: an empty list.
pub fn read(doc: &PdfDocument) -> Result<Vec<LabelRange>> {
    let Some(root) = get(&doc.catalog()?, "PageLabels") else { return Ok(Vec::new()) };
    let mut raw: Vec<(i64, PdfObject)> = Vec::new();
    walk(&root, 0, &mut raw, &mut 0)?;
    raw.sort_by_key(|(k, _)| *k);
    let mut out: Vec<LabelRange> = Vec::new();
    for (k, v) in raw {
        let Some(v) = v.resolve()?.filter(|v| v.is_dict().unwrap_or(false)) else { continue };
        let Ok(page) = u32::try_from(k) else { continue };
        let style = get(&v, "S").and_then(|s| s.as_name().ok()).map_or(LabelStyle::None, |n| LabelStyle::from_name(&String::from_utf8_lossy(&n)));
        let prefix = get(&v, "P").and_then(|p| p.as_string().ok()).unwrap_or_default();
        let start = get(&v, "St").and_then(|s| s.as_int().ok()).filter(|&s| s >= 1).map_or(1, |s| s as u32);
        if out.last().is_some_and(|l| l.page == page) {
            out.pop();
        }
        out.push(LabelRange { page, style, prefix, start });
    }
    if out.first().is_some_and(|f| f.page > 0) {
        out.insert(0, LabelRange { page: 0, style: LabelStyle::Decimal, prefix: String::new(), start: 1 });
    }
    Ok(out)
}

/// Number-tree leaves; depth, entries and total work (`work`) bounded.
fn walk(node: &PdfObject, depth: usize, out: &mut Vec<(i64, PdfObject)>, work: &mut usize) -> Result<()> {
    *work += 1;
    if *work > MAX_WORK {
        return Ok(());
    }
    let Some(node) = node.resolve()? else { return Ok(()) };
    if depth > 8 || !node.is_dict()? {
        return Ok(());
    }
    if let Some(nums) = get(&node, "Nums") {
        let n = nums.len().unwrap_or(0);
        let mut i = 0;
        while i + 1 < n && out.len() < MAX_ENTRIES && *work < MAX_WORK {
            *work += 1;
            let key = nums.get_array(i as i32)?.and_then(|k| k.as_int().ok());
            if let (Some(k), Some(v)) = (key, nums.get_array(i as i32 + 1)?) {
                out.push((k as i64, v));
            }
            i += 2;
        }
    }
    if let Some(kids) = get(&node, "Kids") {
        for i in 0..kids.len().unwrap_or(0).min(MAX_ENTRIES) {
            *work += 1;
            if out.len() >= MAX_ENTRIES || *work >= MAX_WORK {
                break;
            }
            if let Some(k) = kids.get_array(i as i32)? {
                walk(&k, depth + 1, out, work)?;
            }
        }
    }
    Ok(())
}

/// Every page's label (empty string: the page has none and shows its number).
pub fn labels_for(ranges: &[LabelRange], count: u32) -> Vec<String> {
    if ranges.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count as usize);
    let mut r = 0;
    for p in 0..count {
        while r + 1 < ranges.len() && ranges[r + 1].page <= p {
            r += 1;
        }
        let range = &ranges[r];
        out.push(if range.page <= p { range.label(p - range.page) } else { String::new() });
    }
    out
}

/// Every page's label of an open document.
pub fn all(doc: &PdfDocument) -> Result<Vec<String>> {
    let count = doc.page_count()?.max(0) as u32;
    Ok(labels_for(&read(doc)?, count))
}

/// The section a page is in.
fn section(ranges: &[LabelRange], page: u32) -> Option<&LabelRange> {
    ranges.iter().rev().find(|r| r.page <= page)
}

/// `r` (a later section) only carries on what `l` already shows.
pub(super) fn continues(l: &LabelRange, r: &LabelRange) -> bool {
    l.style == r.style && l.prefix == r.prefix && (r.style == LabelStyle::None || l.start.saturating_add(r.page - l.page) == r.start)
}

/// Sorted, one per page, inside the document, and without a section that only carries on the one
/// before it (the same thing as not having it).
pub fn normalize(mut ranges: Vec<LabelRange>, count: u32) -> Vec<LabelRange> {
    ranges.sort_by_key(|r| r.page);
    let mut out: Vec<LabelRange> = Vec::new();
    for r in ranges {
        if r.page >= count {
            continue;
        }
        if out.last().is_some_and(|l| l.page == r.page) {
            out.pop();
        }
        out.push(r);
    }
    let mut kept: Vec<LabelRange> = Vec::new();
    for r in out {
        if !kept.last().is_some_and(|l| continues(l, &r)) {
            kept.push(r);
        }
    }
    // The implicit first section needs no entry.
    if kept.len() == 1 && kept[0] == (LabelRange { page: 0, style: LabelStyle::Decimal, prefix: String::new(), start: 1 }) {
        kept.clear();
    }
    kept
}

/// What the dialog asks for.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LabelSpec {
    pub style: LabelStyle,
    #[serde(default)]
    pub prefix: String,
    #[serde(default = "one")]
    pub start: u32,
    /// Take the numbering of the section before `from` instead (Acrobat's "Extend numbering used
    /// in preceding section to selected pages"); style, prefix and start are ignored.
    #[serde(default)]
    pub extend: bool,
}

/// The sections after labelling pages `from..=to` (0-based) as `spec` says.
pub fn apply(ranges: &[LabelRange], count: u32, from: u32, to: u32, spec: &LabelSpec) -> Result<Vec<LabelRange>> {
    if from > to || to >= count {
        return Err(bad("the page range is outside the document"));
    }
    if spec.start == 0 || spec.start > 99_999 {
        return Err(bad("the starting number must be between 1 and 99999"));
    }
    let new = if spec.extend {
        let before = if from == 0 { None } else { section(ranges, from - 1) };
        let Some(prev) = before else {
            return Err(bad("there is no section before the first page to extend"));
        };
        LabelRange { page: from, style: prev.style, prefix: prev.prefix.clone(), start: prev.start.saturating_add(from - prev.page) }
    } else {
        LabelRange { page: from, style: spec.style, prefix: spec.prefix.clone(), start: spec.start }
    };
    // What the page after the range showed before is kept (a section there, carried on).
    let after = to + 1;
    let restore = (after < count && !ranges.iter().any(|r| r.page == after))
        .then(|| section(ranges, after).map(|r| LabelRange { page: after, start: r.start.saturating_add(after - r.page), ..r.clone() }))
        .flatten()
        // No labels before at all: the pages after the range stay as they were, plain numbers.
        .or_else(|| (after < count && ranges.is_empty()).then(|| LabelRange { page: after, style: LabelStyle::Decimal, prefix: String::new(), start: after + 1 }));
    let mut out: Vec<LabelRange> = ranges.iter().filter(|r| r.page < from || r.page > to).cloned().collect();
    if ranges.is_empty() && from > 0 {
        // The pages before the range were plain 1, 2, 3 …
        out.push(LabelRange { page: 0, style: LabelStyle::Decimal, prefix: String::new(), start: 1 });
    }
    out.push(new);
    out.extend(restore);
    Ok(normalize(out, count))
}

/// Writes the sections as the document's /PageLabels (none: the entry is removed).
pub fn write(doc: &mut PdfDocument, ranges: &[LabelRange]) -> Result<()> {
    let mut catalog = doc.catalog()?;
    if ranges.is_empty() {
        catalog.dict_delete("PageLabels")?;
        return Ok(());
    }
    let mut nums = doc.new_array()?;
    for r in ranges {
        nums.array_push(PdfObject::new_int(r.page as i32)?)?;
        let mut d = doc.new_dict()?;
        if let Some(s) = r.style.name() {
            put_name(&mut d, "S", s)?;
        }
        if !r.prefix.is_empty() {
            put_str(&mut d, "P", &r.prefix)?;
        }
        if r.start != 1 {
            put_int(&mut d, "St", r.start as i32)?;
        }
        nums.array_push(d)?;
    }
    let mut root = doc.new_dict()?;
    root.dict_put("Nums", nums)?;
    catalog.dict_put("PageLabels", root)?;
    Ok(())
}

/// Puts the sections `old` (what `read` gave before an edit that moved pages around) back.
pub fn restore(doc: &mut PdfDocument, old: &[LabelRange]) -> Result<()> {
    let count = doc.page_count()?.max(0) as u32;
    write(doc, &normalize(old.to_vec(), count))
}

/// After pages were deleted. `old` is what `read` gave before the delete (MuPDF shifts the section
/// starts itself when it deletes a page; this replaces that with the rule below), `deleted` the sorted
/// indices numbered before the delete, `before` the page count then. Each section starts at its first
/// page that is left and numbers on from its start, so the pages after a deleted one close up
/// (Acrobat renumbers a section the same way).
pub fn after_delete(doc: &mut PdfDocument, old: &[LabelRange], deleted: &[u32], before: u32) -> Result<()> {
    if old.is_empty() {
        return write(doc, &[]);
    }
    let gone = |p: u32| deleted.binary_search(&p).is_ok();
    let shift = |p: u32| p - deleted.iter().filter(|&&d| d < p).count() as u32;
    let mut out = Vec::new();
    for (i, r) in old.iter().enumerate() {
        let end = old.get(i + 1).map_or(before, |n| n.page);
        if let Some(first) = (r.page..end.min(before)).find(|&p| !gone(p)) {
            out.push(LabelRange { page: shift(first), ..r.clone() });
        }
    }
    let count = doc.page_count()?.max(0) as u32;
    write(doc, &normalize(out, count))
}

/// After `n` pages were inserted at index `at`; `old` is what `read` gave before (MuPDF shifts the
/// section starts itself, and adds a first section when pages go in at the front: this replaces
/// that). Sections after the gap move down. Pages put in front of a section's first page join the
/// section before it (and the first section grows at the front).
pub fn after_insert(doc: &mut PdfDocument, old: &[LabelRange], at: u32, n: u32) -> Result<()> {
    if old.is_empty() {
        return write(doc, &[]);
    }
    if n == 0 {
        return write(doc, old);
    }
    let out = old.iter().cloned().map(|r| if r.page > at || (r.page == at && at > 0) { LabelRange { page: r.page + n, ..r } } else { r }).collect();
    let count = doc.page_count()?.max(0) as u32;
    write(doc, &normalize(out, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(page: u32, style: LabelStyle, prefix: &str, start: u32) -> LabelRange {
        LabelRange { page, style, prefix: prefix.into(), start }
    }

    #[test]
    fn numbers_are_written_the_pdf_way() {
        assert_eq!(roman(4), "iv");
        assert_eq!(roman(1994), "mcmxciv");
        assert_eq!(alpha(1), "a");
        assert_eq!(alpha(26), "z");
        assert_eq!(alpha(27), "aa");
        assert_eq!(alpha(53), "aaa");
        assert_eq!(r(0, LabelStyle::UpperRoman, "", 3).label(1), "IV");
        assert_eq!(r(0, LabelStyle::Decimal, "A-", 1).label(0), "A-1");
        assert_eq!(r(0, LabelStyle::None, "Cover", 5).label(7), "Cover");
    }

    #[test]
    fn a_range_keeps_the_pages_around_it() {
        let spec = LabelSpec { style: LabelStyle::LowerRoman, prefix: String::new(), start: 1, extend: false };
        // no labels yet: pages 0-2 become i, ii, iii and the rest stay 4, 5, 6 …
        let out = apply(&[], 8, 0, 2, &spec).unwrap();
        assert_eq!(labels_for(&out, 8), ["i", "ii", "iii", "4", "5", "6", "7", "8"]);
        // a range in the middle of plain numbering
        let out = apply(&[], 6, 2, 3, &spec).unwrap();
        assert_eq!(labels_for(&out, 6), ["1", "2", "i", "ii", "5", "6"]);
        // inside an existing section: the part after carries on
        let base = vec![r(0, LabelStyle::Decimal, "A-", 1)];
        let out = apply(&base, 6, 1, 2, &spec).unwrap();
        assert_eq!(labels_for(&out, 6), ["A-1", "i", "ii", "A-4", "A-5", "A-6"]);
    }

    #[test]
    fn extend_takes_the_section_before() {
        let base = vec![r(0, LabelStyle::UpperRoman, "", 1), r(3, LabelStyle::Decimal, "", 1)];
        let spec = LabelSpec { style: LabelStyle::Decimal, prefix: String::new(), start: 1, extend: true };
        let out = apply(&base, 6, 3, 3, &spec).unwrap();
        assert_eq!(labels_for(&out, 6), ["I", "II", "III", "IV", "2", "3"]);
        assert!(apply(&base, 6, 0, 1, &spec).is_err(), "nothing before the first page");
    }

    #[test]
    fn redundant_sections_are_dropped() {
        let v = vec![r(0, LabelStyle::Decimal, "", 1), r(2, LabelStyle::Decimal, "", 3)];
        assert!(normalize(v, 5).is_empty());
        let v = vec![r(0, LabelStyle::LowerRoman, "", 1), r(2, LabelStyle::LowerRoman, "", 3), r(4, LabelStyle::Decimal, "", 1)];
        assert_eq!(normalize(v, 5).len(), 2);
    }
}
