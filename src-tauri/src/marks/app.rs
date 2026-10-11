//! The page-mark commands. Adding, updating and removing a mark is one edit step each, saved by
//! Save or Save as like any other (through `save_document`). Bates numbering across files writes
//! numbered copies into a folder the user picked (a `pick_folder` token).

use super::apply::{self, BatesFile, Info};
use super::{AddRequest, Bates, Date, Mark, MarkKind};
use crate::engine::edit::{EditSource, EditState, page_ranges};
use crate::engine::{DocId, Engine, EngineError};
use crate::paths::Paths;
use serde::Deserialize;
use tauri::{Manager, State, Window};

type Result<T> = std::result::Result<T, EngineError>;
type Eng<'a> = State<'a, std::sync::Arc<Engine>>;
type Reg<'a> = State<'a, Paths>;

async fn blocking<R: Send + 'static>(f: impl FnOnce() -> Result<R> + Send + 'static) -> Result<R> {
    tauri::async_runtime::spawn_blocking(f).await.unwrap_or_else(|_| Err(EngineError::Invalid("the task crashed".into())))
}

/// What the step is called: "Add header and footer to pp. 1-12".
fn add_label(req: &AddRequest) -> String {
    let verb = if req.replace { "Update" } else { "Add" };
    let noun = req.mark.kind().noun();
    let on = if req.replace { "on" } else { "to" };
    format!("{verb} {noun} {on} {}", page_ranges(&req.pages))
}

/// Which marks the document has and the settings they were made with (for Update and Remove).
#[tauri::command]
pub async fn marks_info(eng: Eng<'_>, doc: DocId) -> Result<Info> {
    let eng = eng.inner().clone();
    blocking(move || eng.read_custom(doc, |d| apply::info(d))).await
}

/// One undo step. A picture's `file` is an open token.
#[tauri::command]
pub async fn marks_add(window: Window, eng: Eng<'_>, reg: Reg<'_>, doc: DocId, mut request: AddRequest) -> Result<EditState> {
    let picture = match &mut request.mark {
        Mark::Watermark(w) => w.picture.as_mut(),
        Mark::Background(b) => b.picture.as_mut(),
        _ => None,
    };
    if let Some(p) = picture {
        p.file = reg.open(window.label(), &p.file)?;
    }
    let eng = eng.inner().clone();
    let label = add_label(&request);
    let replace = request.replace;
    blocking(move || {
        // An update leaves the old streams unreferenced: the save rewrites the file so they are gone.
        let f = move |d: &mut mupdf::pdf::PdfDocument| apply::add(d, &request);
        if replace {
            eng.apply_removing(doc, label, EditSource::User, f)
        } else {
            eng.apply_custom(doc, label, EditSource::User, false, f)
        }
    })
    .await
}

/// Removes the marks of `kind` from `pages` (all pages when omitted): one undo step.
#[tauri::command]
pub async fn marks_remove(eng: Eng<'_>, doc: DocId, kind: MarkKind, pages: Option<Vec<u32>>) -> Result<EditState> {
    let eng = eng.inner().clone();
    blocking(move || {
        let label = match &pages {
            Some(p) => format!("Remove {} from {}", kind.noun(), page_ranges(p)),
            None => format!("Remove {}", kind.noun()),
        };
        eng.apply_removing(doc, label, EditSource::User, move |d| {
            let all: Vec<u32> = (0..d.page_count()?.max(0) as u32).collect();
            let pages = pages.unwrap_or(all);
            // Only the pages that have the mark are listed as touched.
            let before = apply::info(d)?;
            let on: Vec<u32> = before.kinds.iter().find(|k| k.kind == kind).map(|k| k.pages.clone()).unwrap_or_default();
            let hit: Vec<u32> = pages.iter().copied().filter(|p| on.contains(p)).collect();
            if hit.is_empty() {
                return Err(EngineError::Invalid(format!("There is no {} to remove.", kind.noun())));
            }
            apply::remove(d, kind, &hit)?;
            Ok(hit)
        })
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatesRequest {
    /// Open tokens of the files, in the order they are numbered.
    pub files: Vec<String>,
    pub bates: Bates,
    #[serde(default)]
    pub date: Date,
    /// A `pick_folder` token: where the numbered copies are written.
    pub folder: String,
}

/// Numbers the files and writes a copy of each into the folder. The originals are not changed.
#[tauri::command]
pub async fn marks_bates_files(window: Window, eng: Eng<'_>, reg: Reg<'_>, request: BatesRequest) -> Result<Vec<BatesFile>> {
    let label = window.label().to_string();
    let eng = eng.inner().clone();
    let files = request.files.iter().map(|t| reg.open(&label, t)).collect::<Result<Vec<_>>>()?;
    let folder = reg.take_save(&label, &request.folder)?;
    blocking(move || {
        let out = apply::bates_files(&files, &request.bates, request.date, std::path::Path::new(&folder), &|out| {
            // A numbered copy never lands on a document that is open (a later save would corrupt it).
            eng.check_not_open(&out.to_string_lossy(), None)
        })?;
        let reg = window.state::<Paths>();
        reg.learn(&folder);
        for f in &out {
            reg.learn(&f.output);
        }
        Ok(out)
    })
    .await
}
