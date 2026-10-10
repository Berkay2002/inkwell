//! Optional extras that can be dropped on request. All off by default.

use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};

use super::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Strip {
    /// Document information dictionary and XMP packet.
    pub metadata: bool,
    /// Embedded page thumbnails.
    pub thumbnails: bool,
    /// File attachments (names tree, associated files, attachment annotations).
    pub embedded_files: bool,
    /// Document, page and annotation JavaScript.
    pub javascript: bool,
    /// The bookmark tree.
    pub bookmarks: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StripReport {
    pub metadata: bool,
    pub thumbnails: u32,
    pub embedded_files: u32,
    pub javascript: u32,
    pub bookmarks: bool,
}

fn is_name(o: &PdfObject, key: &str, want: &[u8]) -> bool {
    o.get_dict(key)
        .ok()
        .flatten()
        .is_some_and(|v| v.as_name().is_ok_and(|n| n == want))
}

fn is_js_action(a: &PdfObject) -> bool {
    a.is_dict().unwrap_or(false) && is_name(a, "S", b"JavaScript")
}

/// Drops `key` from `o` when present; true if it was.
fn remove(o: &mut PdfObject, key: &str) -> bool {
    let had = o.get_dict(key).ok().flatten().is_some();
    had && o.dict_delete(key).is_ok()
}

pub(crate) fn strip(doc: &mut PdfDocument, s: &Strip, pages: usize) -> Result<StripReport, Error> {
    let mut r = StripReport::default();
    if *s == Strip::default() {
        return Ok(r);
    }
    let mut catalog = doc.catalog()?;

    if s.metadata {
        r.metadata = remove(&mut catalog, "Metadata");
        r.metadata |= remove(&mut doc.trailer()?, "Info");
    }
    if s.bookmarks {
        r.bookmarks = remove(&mut catalog, "Outlines");
    }
    if let Some(mut names) = catalog.get_dict("Names")?.filter(|n| n.is_dict().unwrap_or(false)) {
        if s.embedded_files {
            r.embedded_files += remove(&mut names, "EmbeddedFiles") as u32;
        }
        if s.javascript {
            r.javascript += remove(&mut names, "JavaScript") as u32;
        }
    }
    if s.embedded_files {
        r.embedded_files += remove(&mut catalog, "AF") as u32;
    }
    if s.javascript {
        r.javascript += remove(&mut catalog, "AA") as u32;
        if catalog.get_dict("OpenAction")?.is_some_and(|a| is_js_action(&a)) {
            r.javascript += remove(&mut catalog, "OpenAction") as u32;
        }
    }

    if s.thumbnails || s.javascript || s.embedded_files {
        for i in 0..pages {
            let mut page = doc.find_page(i as i32)?;
            if s.thumbnails {
                r.thumbnails += remove(&mut page, "Thumb") as u32;
            }
            if s.javascript {
                r.javascript += remove(&mut page, "AA") as u32;
            }
            let Some(mut annots) = page.get_dict("Annots")?.filter(|a| a.is_array().unwrap_or(false)) else {
                continue;
            };
            let mut drop_at = Vec::new();
            for (k, a) in annots.array_iter()?.enumerate() {
                let Ok(mut a) = a else { continue };
                if s.embedded_files && is_name(&a, "Subtype", b"FileAttachment") {
                    drop_at.push(k);
                    r.embedded_files += 1;
                    continue;
                }
                if s.javascript {
                    r.javascript += remove(&mut a, "AA") as u32;
                    if a.get_dict("A")?.is_some_and(|x| is_js_action(&x)) {
                        r.javascript += remove(&mut a, "A") as u32;
                    }
                }
            }
            for k in drop_at.into_iter().rev() {
                annots.array_delete(k as i32)?;
            }
        }
    }
    Ok(r)
}
