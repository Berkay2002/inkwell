//! Organize, the rest of it: combining files with options, attachments, page labels and page
//! transitions (docs/organize-plus.md).
//!
//! Every change to an open document is one `Engine::apply_custom` step (one undo step, kept by Save,
//! recorded in history). Combine writes a new file and touches no open document. The webview never
//! names a path: it passes tokens (paths.rs), `app.rs` resolves them.

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine};
use serde::Serialize;

pub mod app;
pub mod attach;
pub mod combine;
pub mod labels;
#[cfg(test)]
mod tests;
pub mod trans;

pub(crate) use crate::forms::{bad, get, put_int, put_name, put_str};
pub(crate) type Result<T> = std::result::Result<T, crate::engine::EngineError>;

use attach::{Attachment, NewFile};
use labels::{LabelRange, LabelSpec};
use trans::{PageTrans, TransChange};

/// "p. 4", "pp. 2-3, 7" (1-based).
pub(crate) fn pages_text(pages: &[u32]) -> String {
    let mut s: Vec<u32> = pages.to_vec();
    s.sort_unstable();
    s.dedup();
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for p in &s {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == *p => *end = *p,
            _ => runs.push((*p, *p)),
        }
    }
    let list = runs.iter().map(|&(a, b)| if a == b { format!("{}", a + 1) } else { format!("{}-{}", a + 1, b + 1) }).collect::<Vec<_>>().join(", ");
    if s.len() == 1 { format!("p. {list}") } else { format!("pp. {list}") }
}

// ---- page labels --------------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LabelsDto {
    /// The sections (none: the document has no page labels).
    pub ranges: Vec<LabelRange>,
    /// One label per page, or empty when there are no sections.
    pub labels: Vec<String>,
}

pub fn labels_get(eng: &Engine, doc: DocId) -> Result<LabelsDto> {
    eng.read_custom(doc, |d| {
        let ranges = labels::read(d)?;
        let count = d.page_count()?.max(0) as u32;
        Ok(LabelsDto { labels: labels::labels_for(&ranges, count), ranges })
    })
}

/// Labels pages `from..=to` (0-based) as `spec` says; pages outside keep what they show.
pub fn labels_apply(eng: &Engine, doc: DocId, from: u32, to: u32, spec: &LabelSpec, source: EditSource) -> Result<EditState> {
    let (old, next) = eng.read_custom(doc, |d| {
        let count = d.page_count()?.max(0) as u32;
        let old = labels::read(d)?;
        let next = labels::apply(&old, count, from, to, spec)?;
        Ok((old, next))
    })?;
    if old == next {
        return eng.edit_state(doc);
    }
    let pages: Vec<u32> = (from..=to).collect();
    let label = if from == to { format!("Page label on p. {}", from + 1) } else { format!("Page labels on pp. {}-{}", from + 1, to + 1) };
    eng.apply_custom(doc, label, source, false, |d| {
        labels::write(d, &next)?;
        Ok(pages)
    })
}

/// Removes every page label.
pub fn labels_clear(eng: &Engine, doc: DocId, source: EditSource) -> Result<EditState> {
    if eng.read_custom(doc, |d| labels::read(d))?.is_empty() {
        return eng.edit_state(doc);
    }
    eng.apply_custom(doc, "Remove page labels".to_string(), source, false, |d| {
        labels::write(d, &[])?;
        Ok(Vec::new())
    })
}

// ---- transitions --------------------------------------------------------------------------------

pub fn trans_get(eng: &Engine, doc: DocId) -> Result<Vec<PageTrans>> {
    eng.read_custom(doc, |d| trans::read_all(d))
}

/// Sets (or removes) the transition and auto-advance of `pages`.
pub fn trans_set(eng: &Engine, doc: DocId, pages: &[u32], change: &TransChange, source: EditSource) -> Result<EditState> {
    change.check()?;
    let n = eng.read_custom(doc, |d| Ok(d.page_count()?.max(0) as u32))?;
    let mut pages: Vec<u32> = pages.to_vec();
    pages.sort_unstable();
    pages.dedup();
    if pages.is_empty() {
        return Err(bad("no pages selected"));
    }
    if let Some(&p) = pages.iter().find(|&&p| p >= n) {
        return Err(crate::engine::EngineError::BadPage(p));
    }
    let same = eng.read_custom(doc, |d| {
        let want = PageTrans { transition: change.transition.map(|t| t.effective()), auto_advance: change.auto_advance };
        for &p in &pages {
            let have = trans::read_page(d, p)?;
            // The effect, not the bytes: a transition with a style it does not use is the same one.
            if have != want {
                return Ok(false);
            }
        }
        Ok(true)
    })?;
    if same {
        return eng.edit_state(doc);
    }
    let label = match (&change.transition, change.auto_advance) {
        (None, None) => format!("Remove page transition on {}", pages_text(&pages)),
        (Some(_), _) => format!("Page transition on {}", pages_text(&pages)),
        (None, Some(_)) => format!("Auto-advance on {}", pages_text(&pages)),
    };
    eng.apply_custom(doc, label, source, false, |d| trans::write(d, &pages, change))
}

// ---- attachments --------------------------------------------------------------------------------

pub fn attach_list(eng: &Engine, doc: DocId) -> Result<Vec<Attachment>> {
    eng.read_custom(doc, |d| attach::list(d))
}

/// The attachment's file name and bytes.
pub fn attach_data(eng: &Engine, doc: DocId, id: &str) -> Result<(String, Vec<u8>)> {
    eng.read_custom(doc, |d| attach::read_data(d, id))
}

pub fn attach_add(eng: &Engine, doc: DocId, files: Vec<NewFile>, source: EditSource) -> Result<EditState> {
    let label = match files.as_slice() {
        [one] => format!("Attach {}", attach::clean_name(&one.name)),
        many => format!("Attach {} files", many.len()),
    };
    eng.apply_custom(doc, label, source, false, |d| attach::add(d, &files))
}

/// Deleting runs as a removing edit: a save rewrites the file, so the bytes are really gone.
pub fn attach_remove(eng: &Engine, doc: DocId, ids: &[String], source: EditSource) -> Result<EditState> {
    let names: Vec<String> = eng.read_custom(doc, |d| {
        Ok(attach::list(d)?.into_iter().filter(|a| ids.contains(&a.id)).map(|a| a.name).collect())
    })?;
    if names.is_empty() {
        return eng.edit_state(doc);
    }
    let label = match names.as_slice() {
        [one] => format!("Delete attachment {one}"),
        many => format!("Delete {} attachments", many.len()),
    };
    eng.apply_removing(doc, label, source, |d| attach::remove(d, ids))
}

pub fn attach_describe(eng: &Engine, doc: DocId, id: &str, text: &str, source: EditSource) -> Result<EditState> {
    let (name, same) = eng.read_custom(doc, |d| {
        let found = attach::list(d)?.into_iter().find(|a| a.id == id);
        Ok(found.map_or((String::new(), false), |a| (a.name.clone(), a.description == text)))
    })?;
    if name.is_empty() {
        return Err(bad("that attachment is no longer in the document"));
    }
    if same {
        return eng.edit_state(doc);
    }
    eng.apply_custom(doc, format!("Describe attachment {name}"), source, false, |d| attach::describe(d, id, text))
}
