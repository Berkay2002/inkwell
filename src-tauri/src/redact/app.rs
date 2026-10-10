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
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HitDto {
    pub page: u32,
    pub text: String,
    pub quads: Vec<QuadDto>,
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
    let q = match q.clone() {
        FindQuery::Text { needle, case_sensitive, whole_word } => Query::Text { needle, case_sensitive, whole_word },
        FindQuery::Pattern { pattern } => Query::Pattern(pattern),
    };
    let hits = eng.read_custom(doc, |d| super::find(d, &q).map_err(err))?;
    Ok(hits.iter().map(|h| HitDto { page: h.page as u32, text: h.text.clone(), quads: h.quads.iter().map(quad_dto).collect() }).collect())
}

fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// One mark per hit, one undo step.
pub fn mark_hits(eng: &Engine, doc: DocId, hits: &[HitDto], source: EditSource) -> Result<EditState> {
    let hits: Vec<Hit> = hits
        .iter()
        .map(|h| Hit { page: h.page as i32, text: h.text.clone(), quads: h.quads.iter().map(quad_of).collect() })
        .collect();
    let mut pages: Vec<u32> = hits.iter().map(|h| h.page as u32).collect();
    pages.dedup();
    let label = format!("Mark {} item{} for redaction", hits.len(), s(hits.len()));
    eng.apply_custom(doc, label, source, false, |d| {
        super::mark_hits(d, &hits, &MarkStyle::default()).map_err(err)?;
        Ok(pages)
    })
}

/// `area` in page space (y down): `[x0, y0, x1, y1]`.
pub fn mark_area(eng: &Engine, doc: DocId, page: u32, area: [f32; 4], source: EditSource) -> Result<EditState> {
    let [x0, y0, x1, y1] = area;
    let rect = Rect { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) };
    eng.apply_custom(doc, "Mark an area for redaction".into(), source, false, |d| {
        super::add_area_mark(d, page as i32, rect, &MarkStyle::default()).map_err(err)?;
        Ok(vec![page])
    })
}

pub fn mark_pages(eng: &Engine, doc: DocId, pages: &[u32], source: EditSource) -> Result<EditState> {
    let label = format!("Mark {} page{} for redaction", pages.len(), s(pages.len()));
    let pages = pages.to_vec();
    eng.apply_custom(doc, label, source, false, |d| {
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
        for &(p, xref) in &marks {
            if !super::remove_mark(d, p as i32, xref).map_err(err)? {
                return Err(EngineError::Invalid(format!("no redaction mark {xref} on page {}", p + 1)));
            }
        }
        Ok(marks.iter().map(|m| m.0).collect())
    })
}

pub fn list_marks(eng: &Engine, doc: DocId) -> Result<Vec<MarkDto>> {
    eng.read_custom(doc, |d| {
        let marks = super::list_marks(d).map_err(err)?;
        Ok(marks
            .iter()
            .map(|m| MarkDto {
                page: m.page as u32,
                xref: m.xref,
                whole_page: m.quads.iter().any(|q| q.ul.x < -1.0e5),
                quads: m.quads.iter().map(quad_dto).collect(),
            })
            .collect())
    })
}

/// What `sanitize` would remove if Apply ran now (counts for the confirm), on a throwaway copy.
pub fn preview_sanitize(eng: &Engine, doc: DocId, opts: &Sanitize) -> Result<SanitizeReport> {
    eng.read_custom(doc, |d| {
        let bytes = super::save_to_vec(d).map_err(err)?;
        let mut copy = PdfDocument::from_bytes(&bytes).map_err(|e| EngineError::Mupdf(e.to_string()))?;
        let t = super::collect_targets(&copy, opts.form_fields_under_marks).map_err(err)?;
        super::sanitize(&mut copy, opts, &t.needles).map_err(err)
    })
}

/// Applies every mark as one undo step labelled "Redact n items", then, with `save_to`, saves the
/// verified bytes there (Save as: the document then lives there; the original file is untouched).
pub fn apply(eng: &Engine, doc: DocId, sanitize: Sanitize, save_to: Option<String>, source: EditSource) -> Result<ApplyOutcome> {
    let n = list_marks(eng, doc)?.len();
    if n == 0 {
        return Err(EngineError::Invalid("nothing is marked for redaction".into()));
    }
    let opts = ApplyOptions { sanitize, ..ApplyOptions::default() };
    let mut report = None;
    let mut leaks = None;
    let label = format!("Redact {n} item{}", s(n));
    let r = eng.apply_redaction(doc, label, source, |d| match super::apply(d, &opts) {
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
        // Ask first: a cancelled dialog leaves the document as it was.
        let save_to = if options.save {
            match pick_save_target(&app, &redacted_path(&eng.edit_state(doc)?.path)) {
                Some(p) => Some(p),
                None => return Ok(ApplyOutcome::Cancelled),
            }
        } else {
            None
        };
        apply(&eng, doc, options.sanitize, save_to, EditSource::User)
    })
    .await
}

#[cfg(test)]
mod tests;
