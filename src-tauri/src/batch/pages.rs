//! The page range of a step: its syntax is checked when the action is validated, and it is clipped to the
//! pages a file really has when a step runs. The syntax is the page range of the OCR and Convert tools
//! (`ocr::page_spans`): `1-3, 5, 8-`, one-based, `;` allowed for `,`, an open end runs to the last page.

use crate::ocr::page_spans;

/// A range longer than this is not one (an imported file is untrusted).
pub const MAX_LEN: usize = 200;

/// Whether `spec` is a page range, without a document: the engine's own sentence when it is not.
pub fn check(spec: &str) -> Result<(), String> {
    if spec.chars().count() > MAX_LEN {
        return Err(format!("A page range is at most {MAX_LEN} characters."));
    }
    page_spans(spec).map(drop).map_err(|_| format!("\"{}\" is not a page range (for example 1-3, 5, 8-).", spec.trim()))
}

/// The pages of a range that a file of `count` pages has.
pub struct Selection {
    /// Zero-based, ascending, without repeats.
    pub pages: Vec<u32>,
    /// The range names pages beyond the last one (they are left out).
    pub past_end: bool,
}

/// `spec` against a file of `count` pages: what exists is selected, what does not is left out and reported
/// through `past_end`. An empty `spec` is every page.
pub fn resolve(spec: &str, count: u32) -> Result<Selection, String> {
    if spec.trim().is_empty() {
        return Ok(Selection { pages: (0..count).collect(), past_end: false });
    }
    check(spec)?;
    let mut pages = Vec::new();
    let mut past_end = false;
    for (a, b) in page_spans(spec).map_err(|e| e.to_string())? {
        let last = b.unwrap_or(count);
        past_end |= last > count || a > count;
        if a <= count {
            pages.extend(a - 1..last.min(count));
        }
    }
    pages.sort_unstable();
    pages.dedup();
    Ok(Selection { pages, past_end })
}

/// Zero-based pages as one-based runs: `1-3, 5`.
pub fn describe(pages: &[u32]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let mut j = i;
        while j + 1 < pages.len() && pages[j + 1] == pages[j] + 1 {
            j += 1;
        }
        out.push(if i == j { (pages[i] + 1).to_string() } else { format!("{}-{}", pages[i] + 1, pages[j] + 1) });
        i = j + 1;
    }
    out.join(", ")
}
