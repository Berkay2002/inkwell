//! The redaction engine wired into the app: every change goes through edit-core's lane (one undo
//! step each), and Apply hands edit-core the verified bytes, which a save then writes as they are.

use super::{ApplyOptions, Hit, Leak, MarkStyle, Pattern, Query, RedactError, Sanitize, SanitizeReport, VerifyReport};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use mupdf::pdf::PdfDocument;
use mupdf::{Point, Quad, Rect};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, EngineError>;

fn err(e: RedactError) -> EngineError {
    match e {
        RedactError::Pdf(e) => EngineError::Mupdf(e.to_string()),
        RedactError::NeedsPassword => EngineError::NeedsPassword,
        e => EngineError::Invalid(e.to_string()),
    }
}

/// A quad as `[ulx, uly, urx, ury, llx, lly, lrx, lry]` in page space (y down).
pub type QuadDto = [f32; 8];

fn quad_dto(q: &Quad) -> QuadDto {
    [q.ul.x, q.ul.y, q.ur.x, q.ur.y, q.ll.x, q.ll.y, q.lr.x, q.lr.y]
}

fn quad_of(d: &QuadDto) -> Quad {
    let p = |i: usize| Point { x: d[i], y: d[i + 1] };
    Quad { ul: p(0), ur: p(2), ll: p(4), lr: p(6) }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FindQuery {
    Text { needle: String, case_sensitive: bool, whole_word: bool },
    Pattern { pattern: Pattern },
    /// For the assistant: anything the named patterns do not cover (dates, reference numbers).
    Regex { pattern: String, case_sensitive: bool },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HitDto {
    pub page: u32,
    pub text: String,
    pub quads: Vec<QuadDto>,
    /// Surrounding text for the snippet; display only (`mark_hits` ignores it).
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MarkDto {
    pub page: u32,
    /// Handle for `unmark`.
    pub xref: i32,
    pub quads: Vec<QuadDto>,
    /// A "redact pages" mark (it reaches far past the page; draw it clipped to the page).
    pub whole_page: bool,
    /// The marked text, for text marks.
    pub label: Option<String>,
}

/// What Apply did. `Refused`: the engine found recoverable content and the document is as before.
#[derive(Serialize, Debug)]
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ApplyOutcome {
    Applied { state: EditState, marks: usize, sanitize: SanitizeReport, verify: VerifyReport, saved_to: Option<String> },
    Refused { leaks: Vec<Leak> },
    /// The save dialog was cancelled before anything was applied.
    Cancelled,
}

/// Version history's entry point (its branch is not merged yet; its API is
/// `redactions_applied(doc, keep, bytes, meta)`): called with the verified bytes after every
/// redaction that lands, before any save. Until it is wired, edit-core's own redaction hook
/// (`set_redaction_hook`) is what purges pre-redaction state.
pub fn on_redactions_applied(_doc: DocId, _bytes: &[u8]) {}

pub fn find(eng: &Engine, doc: DocId, q: &FindQuery) -> Result<Vec<HitDto>> {
    find_capped(eng, doc, q, None, usize::MAX, None)
}

/// [`find`] over the zero-based pages in `only` (every page when `None`), stopping at `max` hits and when `cancel` is set.
pub fn find_capped(
    eng: &Engine,
    doc: DocId,
    q: &FindQuery,
    only: Option<&[u32]>,
    max: usize,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Vec<HitDto>> {
    let q = match q.clone() {
        FindQuery::Text { needle, case_sensitive, whole_word } => Query::Text { needle, case_sensitive, whole_word },
        FindQuery::Pattern { pattern } => Query::Pattern(pattern),
        FindQuery::Regex { pattern, case_sensitive } => Query::Regex { pattern, case_sensitive },
    };
    let hits = eng.read_custom(doc, |d| super::find_capped(d, &q, only, max, cancel).map_err(err))?;
    Ok(hits.iter().map(|h| HitDto { page: h.page as u32, text: h.text.clone(), quads: h.quads.iter().map(quad_dto).collect(), before: h.before.clone(), after: h.after.clone() }).collect())
}

fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// One mark per hit, one undo step.
pub fn mark_hits(eng: &Engine, doc: DocId, hits: &[HitDto], source: EditSource) -> Result<EditState> {
    let hits: Vec<Hit> = hits
        .iter()
        .map(|h| Hit { page: h.page as i32, text: h.text.clone(), quads: h.quads.iter().map(quad_of).collect(), before: String::new(), after: String::new() })
        .collect();
    let mut pages: Vec<u32> = hits.iter().map(|h| h.page as u32).collect();
    pages.dedup();
    let label = format!("Mark {} item{} for redaction", hits.len(), s(hits.len()));
    eng.apply_custom(doc, label, source, false, |d| {
        crate::outline::refuse_unless_annotating_allowed(d, "Marking text for redaction")?;
        super::mark_hits(d, &hits, &MarkStyle::default()).map_err(err)?;
        Ok(pages)
    })
}

/// `area` in page space (y down): `[x0, y0, x1, y1]`.
pub fn mark_area(eng: &Engine, doc: DocId, page: u32, area: [f32; 4], source: EditSource) -> Result<EditState> {
    let [x0, y0, x1, y1] = area;
    let rect = Rect { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) };
    eng.apply_custom(doc, "Mark an area for redaction".into(), source, false, |d| {
        crate::outline::refuse_unless_annotating_allowed(d, "Marking an area for redaction")?;
        super::add_area_mark(d, page as i32, rect, &MarkStyle::default()).map_err(err)?;
        Ok(vec![page])
    })
}

pub fn mark_pages(eng: &Engine, doc: DocId, pages: &[u32], source: EditSource) -> Result<EditState> {
    let label = format!("Mark {} page{} for redaction", pages.len(), s(pages.len()));
    let pages = pages.to_vec();
    eng.apply_custom(doc, label, source, false, |d| {
        crate::outline::refuse_unless_annotating_allowed(d, "Marking pages for redaction")?;
        for &p in &pages {
            super::mark_page(d, p as i32, &MarkStyle::default()).map_err(err)?;
        }
        Ok(pages)
    })
}

pub fn unmark(eng: &Engine, doc: DocId, marks: &[(u32, i32)], source: EditSource) -> Result<EditState> {
    let label = format!("Remove {} redaction mark{}", marks.len(), s(marks.len()));
    let marks = marks.to_vec();
    eng.apply_custom(doc, label, source, false, |d| {
        crate::outline::refuse_unless_annotating_allowed(d, "Removing a redaction mark")?;
        for &(p, xref) in &marks {
            if !super::remove_mark(d, p as i32, xref).map_err(err)? {
                return Err(EngineError::Invalid(format!("no redaction mark {xref} on page {}", p + 1)));
            }
        }
        Ok(marks.iter().map(|m| m.0).collect())
    })
}

fn mark_dtos(d: &PdfDocument) -> Result<Vec<MarkDto>> {
    let marks = super::list_marks(d).map_err(err)?;
    Ok(marks
        .iter()
        .map(|m| MarkDto {
            page: m.page as u32,
            xref: m.xref,
            whole_page: m.quads.iter().any(|q| q.ul.x < -1.0e5),
            quads: m.quads.iter().map(quad_dto).collect(),
            label: m.label.clone(),
        })
        .collect())
}

pub fn list_marks(eng: &Engine, doc: DocId) -> Result<Vec<MarkDto>> {
    eng.read_custom(doc, mark_dtos)
}

/// A fingerprint of what is marked: which marks, where and what text. Two lists with the same
/// digest are the same redaction; the assistant hands it back to apply exactly what was reviewed.
pub fn marks_digest(marks: &[MarkDto]) -> String {
    let mut keys: Vec<String> = marks
        .iter()
        .map(|m| {
            let q: Vec<String> = m.quads.iter().map(|q| format!("{q:?}")).collect();
            format!("{}#{}|{}|{}", m.page, m.xref, q.join(","), m.label.as_deref().unwrap_or(""))
        })
        .collect();
    keys.sort_unstable();
    // FNV-1a, 64 bit: stable across runs and builds
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in keys.join("\n").bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// What `sanitize` would remove if Apply ran now (counts for the confirm), on a throwaway copy.
pub fn preview_sanitize(eng: &Engine, doc: DocId, opts: &Sanitize) -> Result<SanitizeReport> {
    eng.read_custom(doc, |d| {
        let bytes = super::save_to_vec(d).map_err(err)?;
        let mut copy = PdfDocument::from_bytes(&bytes).map_err(|e| EngineError::Mupdf(e.to_string()))?;
        let t = super::collect_targets(&copy, opts.form_fields_under_marks).map_err(err)?;
        // Apply removes the marks before it sanitizes: they are not comments to count.
        for m in super::list_marks(&copy).map_err(err)? {
            super::remove_mark(&mut copy, m.page, m.xref).map_err(err)?;
        }
        super::sanitize(&mut copy, opts, &t.needles).map_err(err)
    })
}

/// Redaction rewrites the whole file: a certified document is refused, and a signed one unless the result goes to
/// a new file (`in_place` false), which leaves the signed original as it is.
fn ensure_applicable(eng: &Engine, doc: DocId, in_place: bool) -> Result<()> {
    eng.read_custom(doc, |d| applicable(d, in_place))
}

/// [`ensure_applicable`] on the open document: it runs again inside the edit lane, so a signature added between the
/// first look and the step is caught too.
fn applicable(d: &PdfDocument, in_place: bool) -> Result<()> {
    crate::outline::refuse_if_certified(d, "Redacting it")?;
    if in_place && crate::outline::signed(d) {
        return Err(EngineError::Invalid(
            "This document is signed, and redacting it in place would break its signature: save the redacted copy as a new file".into(),
        ));
    }
    Ok(())
}

/// Applies every mark as one undo step labelled "Redact n items", then, with `save_to`, saves the
/// verified bytes there (Save as: the document then lives there; the original file is untouched).
pub fn apply(eng: &Engine, doc: DocId, sanitize: Sanitize, save_to: Option<String>, source: EditSource) -> Result<ApplyOutcome> {
    apply_expecting(eng, doc, sanitize, save_to, source, None)
}

/// The text of the `Invalid` error [`apply_expecting`] fails with when the marks are not the ones
/// the caller reviewed.
pub const MARKS_CHANGED: &str = "the redaction marks changed";

/// [`apply`], but with `expect` (a [`marks_digest`]) checked inside the edit lane, under the
/// document's lock: nothing can change the marks between the check and the redaction.
pub fn apply_expecting(
    eng: &Engine,
    doc: DocId,
    sanitize: Sanitize,
    save_to: Option<String>,
    source: EditSource,
    expect: Option<&str>,
) -> Result<ApplyOutcome> {
    ensure_applicable(eng, doc, save_to.is_none())?;
    let n = list_marks(eng, doc)?.len();
    if n == 0 {
        return Err(EngineError::Invalid("nothing is marked for redaction".into()));
    }
    let opts = ApplyOptions { sanitize, password: eng.password(doc)?.map(|p| p.to_string()), ..ApplyOptions::default() };
    let mut report = None;
    let mut leaks = None;
    let label = format!("Redact {n} item{}", s(n));
    let in_place = save_to.is_none();
    let r = eng.apply_redaction(doc, label, source, |d| {
        applicable(d, in_place)?;
        if let Some(want) = expect {
            if marks_digest(&mark_dtos(d)?) != want {
                return Err(EngineError::Invalid(MARKS_CHANGED.into()));
            }
        }
        match super::apply(d, &opts) {
            Ok(mut rep) => {
                let bytes = std::mem::take(&mut rep.bytes);
                let pages = rep.pages.iter().map(|&p| p as u32).collect();
                on_redactions_applied(doc, &bytes);
                report = Some(rep);
                Ok((pages, bytes))
            }
            Err(RedactError::Leaked(l)) => {
                leaks = Some(l);
                Err(EngineError::Invalid("redaction refused".into()))
            }
            Err(e) => Err(err(e)),
        }
    });
    if let Some(leaks) = leaks {
        return Ok(ApplyOutcome::Refused { leaks });
    }
    let mut state = r?;
    let rep = report.expect("set when the redaction landed");
    if let Some(path) = &save_to {
        state = eng.save(doc, Some(path))?;
    }
    Ok(ApplyOutcome::Applied { state, marks: rep.marks, sanitize: rep.sanitize, verify: rep.verify, saved_to: save_to })
}

/// `<folder>/<base>_Redacted.pdf` beside `path`.
pub fn redacted_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    let stem = p.file_stem().map_or_else(|| "document".into(), |s| s.to_string_lossy().into_owned());
    p.with_file_name(format!("{stem}_Redacted.pdf")).to_string_lossy().into_owned()
}

// ---- Tauri commands (the UI is the user; the assistant calls the functions above with its turn) ----

type Eng<'a> = tauri::State<'a, std::sync::Arc<Engine>>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or(Err(EngineError::Panic))
}

#[tauri::command]
pub async fn redact_find(eng: Eng<'_>, doc: DocId, query: FindQuery) -> Result<Vec<HitDto>> {
    let eng = eng.inner().clone();
    blocking(move || find(&eng, doc, &query)).await
}

/// Exactly one of `hits`, `area` (with `page`) or `pages`.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MarkWhat {
    Hits { hits: Vec<HitDto> },
    Area { page: u32, area: [f32; 4] },
    Pages { pages: Vec<u32> },
}

#[tauri::command]
pub async fn redact_mark(eng: Eng<'_>, doc: DocId, what: MarkWhat) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || match what {
        MarkWhat::Hits { hits } => mark_hits(&eng, doc, &hits, EditSource::User),
        MarkWhat::Area { page, area } => mark_area(&eng, doc, page, area, EditSource::User),
        MarkWhat::Pages { pages } => mark_pages(&eng, doc, &pages, EditSource::User),
    })
    .await
}

/// `marks`: `(page, xref)` pairs from `redact_list_marks`.
#[tauri::command]
pub async fn redact_unmark(eng: Eng<'_>, doc: DocId, marks: Vec<(u32, i32)>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || unmark(&eng, doc, &marks, EditSource::User)).await
}

#[tauri::command]
pub async fn redact_list_marks(eng: Eng<'_>, doc: DocId) -> Result<Vec<MarkDto>> {
    let eng = eng.inner().clone();
    blocking(move || list_marks(&eng, doc)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactOptions {
    pub sanitize: Sanitize,
    /// What `sanitize` would remove now (the confirm's counts).
    pub preview: SanitizeReport,
    /// The file name the save dialog suggests ("Saves as …").
    pub save_name: String,
}

/// The confirm's content: the sanitize options (defaults unless given), their counts, and the
/// name Apply suggests for the redacted copy.
#[tauri::command]
pub async fn redact_options(eng: Eng<'_>, doc: DocId, sanitize: Option<Sanitize>) -> Result<RedactOptions> {
    let eng = eng.inner().clone();
    blocking(move || {
        let sanitize = sanitize.unwrap_or_default();
        let preview = preview_sanitize(&eng, doc, &sanitize)?;
        let path = redacted_path(&eng.edit_state(doc)?.path);
        let save_name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(RedactOptions { sanitize, preview, save_name })
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyArgs {
    #[serde(default)]
    pub sanitize: Sanitize,
    /// Ask where to save the redacted copy (a native dialog, on the Rust side), then save there.
    /// False: apply only.
    #[serde(default)]
    pub save: bool,
}

/// The one place a save target is chosen: the user picks it in a native dialog, never the webview
/// (a path from the webview would be an arbitrary file write). `None`: cancelled.
// ponytail: dialog only; switch to the shared path-token registry (paths.rs) once edit-core's lands.
fn pick_save_target(app: &tauri::AppHandle, suggested: &str) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let p = std::path::Path::new(suggested);
    let mut d = app.dialog().file().add_filter("PDF", &["pdf"]);
    if let Some(dir) = p.parent() {
        d = d.set_directory(dir);
    }
    if let Some(name) = p.file_name() {
        d = d.set_file_name(name.to_string_lossy());
    }
    d.blocking_save_file()?.into_path().ok().map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn redact_apply(app: tauri::AppHandle, eng: Eng<'_>, doc: DocId, options: ApplyArgs) -> Result<ApplyOutcome> {
    let eng = eng.inner().clone();
    blocking(move || {
        // Refused before any dialog.
        ensure_applicable(&eng, doc, !options.save)?;
        // Ask first: a cancelled dialog leaves the document as it was.
        let save_to = if options.save {
            match pick_save_target(&app, &redacted_path(&eng.edit_state(doc)?.path)) {
                Some(p) => Some(p),
                None => return Ok(ApplyOutcome::Cancelled),
            }
        } else {
            None
        };
        let out = apply(&eng, doc, options.sanitize, save_to, EditSource::User)?;
        // The redacted copy may have replaced a file a catalog lists: no catalog keeps that file's old text.
        if let ApplyOutcome::Applied { saved_to: Some(p), .. } = &out {
            crate::search::catalog::app::forget_saved(&app, p);
        }
        Ok(out)
    })
    .await
}

/// Sanitize without a redaction (the assistant's "remove hidden information"): one undo step that
/// removes what `o` names. `apply_sanitizing`: the step is a full rewrite, so what was removed (the
/// trailer's /Info included) is not left in the document or in the saved bytes.
pub fn sanitize_doc(eng: &Engine, doc: DocId, o: Sanitize, source: EditSource) -> Result<(EditState, SanitizeReport)> {
    let mut report = SanitizeReport::default();
    let state = eng.apply_sanitizing(doc, "Remove hidden information".into(), source, |d| {
        crate::outline::refuse_if_certified(d, "Removing its hidden information")?;
        if crate::outline::signed(d) {
            return Err(EngineError::Invalid(
                "This document is signed, and removing its hidden information rewrites the whole file, which would break the signature. Remove the signature first, or do this on a copy without it.".into(),
            ));
        }
        report = super::sanitize(d, &o, &[]).map_err(err)?;
        let r = &report;
        let removed = r.metadata + r.hidden_layer_blocks + r.hidden_text_pages + r.scripts_actions + r.embedded_files + r.annotations + r.form_fields + r.bookmarks;
        if removed == 0 {
            // an error rolls the step back: a no-op leaves no undo step
            return Err(EngineError::Invalid("There is nothing of the kinds asked for to remove in this document.".into()));
        }
        Ok((0..d.page_count()?.max(0) as u32).collect())
    })?;
    Ok((state, report))
}

#[cfg(test)]
mod tests;
