//! The embedded index: a document's page records kept inside the PDF, so a catalog build (here or in another
//! Inkwell) takes them instead of reading the pages again. Opt-in, per document, one edit-core step each way.
//!
//! Where: `/Root /PieceInfo /Inkwell << /LastModified (D:...) /Private <stream> >>`, the place the PDF
//! specification gives to application data. The stream is a [`FileText`] (zstd of JSON) with a small dict:
//! `/Type /InkwellSearchIndex /Version /Pages /DocID`.
//!
//! Matching is by the file's `/ID`, its page count and a fingerprint of what the pages hold (decoded content
//! streams, and the type, rectangle, text and value of each annotation), so a file whose pages were changed after
//! the index was made, by Inkwell or anything else, is read again instead of answering from old text.

use super::{pdf_id, read_pages, FileText, MAX_BLOB, VERSION};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, Result};
use crate::forms::{bad, get, put_int, put_name, put_str};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::Buffer;
use serde::Serialize;
use std::sync::atomic::AtomicBool;

fn dict(o: PdfObject) -> Option<PdfObject> {
    o.resolve().ok().flatten().filter(|d| d.is_dict().unwrap_or(false))
}

fn piece(doc: &PdfDocument) -> Option<PdfObject> {
    get(&doc.catalog().ok()?, "PieceInfo").and_then(dict)
}

/// The index's stream, if the document has one of ours.
fn private(doc: &PdfDocument) -> Option<PdfObject> {
    let ours = get(&piece(doc)?, "Inkwell").and_then(dict)?;
    // Kept as the indirect reference: streams are read and tested by object number.
    get(&ours, "Private").filter(|p| p.is_stream().unwrap_or(false))
}

/// The embedded records when they were made for this file: same `/ID` (not empty), same page count, current
/// format. Anything else, or anything that fails to read or inflate within bounds, is `None`: the build reads
/// the pages instead.
pub fn read(doc: &PdfDocument, pages: u32, id: &str) -> Option<FileText> {
    if id.is_empty() {
        return None;
    }
    let bytes = private(doc)?.read_stream_capped(MAX_BLOB as usize).ok()?;
    let t = FileText::decode(&bytes)?;
    (t.id == id && t.pages.len() == pages as usize && !t.sig.is_empty() && t.sig == fingerprint(doc)).then_some(t)
}

/// A hash of what the text and comments of the pages come from. Decoded streams and the values of the keys that
/// matter, not object numbers or encodings, so a save that renumbers or recompresses does not change it.
pub fn fingerprint(doc: &PdfDocument) -> String {
    let mut h = blake3::Hasher::new();
    let n = doc.page_count().unwrap_or(0).max(0);
    for p in 0..n {
        let Ok(page) = doc.find_page(p) else { continue };
        h.update(&p.to_le_bytes());
        if let Some(c) = get(&page, "Contents") {
            let parts: Vec<PdfObject> = if c.is_array().unwrap_or(false) {
                (0..c.len().unwrap_or(0)).filter_map(|i| c.get_array(i as i32).ok().flatten()).collect()
            } else {
                vec![c]
            };
            for s in parts {
                h.update(&s.read_stream().unwrap_or_default());
                h.update(b"\0");
            }
        }
        let annots = get(&page, "Annots").and_then(dict_or_array);
        for i in 0..annots.as_ref().map_or(0, |a| a.len().unwrap_or(0)) {
            let Some(a) = annots.as_ref().and_then(|l| l.get_array(i as i32).ok().flatten()).and_then(|a| a.resolve().ok().flatten()) else { continue };
            for k in ["Subtype", "Rect", "Contents", "V", "T"] {
                if let Some(v) = get(&a, k) {
                    h.update(k.as_bytes());
                    h.update(v.to_string().as_bytes());
                }
            }
        }
    }
    h.finalize().to_hex().to_string()
}

fn dict_or_array(o: PdfObject) -> Option<PdfObject> {
    o.resolve().ok().flatten().filter(|a| a.is_array().unwrap_or(false))
}

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Embedded {
    pub present: bool,
    /// The records are for this file as it is: same `/ID`, page count and page content (else a build ignores them).
    pub current: bool,
    pub pages: u32,
    pub bytes: u64,
}

pub fn status(doc: &PdfDocument) -> Embedded {
    let Some(s) = private(doc) else { return Embedded::default() };
    let raw = s.read_raw_stream().map(|b| b.len() as u64).unwrap_or(0);
    let Some(t) = s.read_stream_capped(MAX_BLOB as usize).ok().and_then(|b| FileText::decode(&b)) else {
        return Embedded { present: true, current: false, pages: 0, bytes: raw };
    };
    let n = doc.page_count().unwrap_or(0).max(0) as usize;
    let current = !t.id.is_empty() && t.id == pdf_id(doc) && t.pages.len() == n && !t.sig.is_empty() && t.sig == fingerprint(doc);
    Embedded { present: true, current, pages: t.pages.len() as u32, bytes: raw }
}

/// Writes (or replaces) the index of the document as it is now. A document without `/ID` gets one first: the
/// index is matched by it.
pub fn add(doc: &mut PdfDocument) -> Result<Vec<u32>> {
    if pdf_id(doc).is_empty() {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|e| bad(e.to_string()))?;
        let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
        let mut ids = doc.new_array()?;
        ids.array_push(PdfObject::new_string(&hex)?)?;
        ids.array_push(PdfObject::new_string(&hex)?)?;
        doc.trailer()?.dict_put("ID", ids)?;
    }
    let mut text = read_pages(doc, &AtomicBool::new(false)).map_err(|_| bad("could not read the pages"))?;
    text.sig = fingerprint(doc);
    let bytes = text.encode();
    if bytes.is_empty() || bytes.len() as u64 > MAX_BLOB {
        return Err(bad("the search index of this document is too large to embed"));
    }
    let mut head = doc.new_dict()?;
    put_name(&mut head, "Type", "InkwellSearchIndex")?;
    put_int(&mut head, "Version", VERSION as i32)?;
    put_int(&mut head, "Pages", text.pages.len() as i32)?;
    put_str(&mut head, "DocID", &text.id)?;
    let stream = doc.add_stream(&Buffer::from_bytes(&bytes)?, Some(&head), false)?;
    let mut ours = doc.new_dict()?;
    put_str(&mut ours, "LastModified", &crate::annot::pdf_now())?;
    ours.dict_put("Private", stream)?;
    let mut catalog = doc.catalog()?;
    match piece(doc) {
        Some(mut p) => p.dict_put("Inkwell", ours)?,
        None => {
            let mut p = doc.new_dict()?;
            p.dict_put("Inkwell", ours)?;
            catalog.dict_put("PieceInfo", p)?;
        }
    }
    Ok(Vec::new())
}

/// Takes the index out (and `/PieceInfo` with it when it held nothing else).
pub fn remove(doc: &mut PdfDocument) -> Result<Vec<u32>> {
    let Some(mut p) = piece(doc) else { return Ok(Vec::new()) };
    if get(&p, "Inkwell").is_none() {
        return Ok(Vec::new());
    }
    p.dict_delete("Inkwell")?;
    if p.dict_len()? == 0 {
        doc.catalog()?.dict_delete("PieceInfo")?;
    }
    Ok(Vec::new())
}

pub fn embedded(eng: &Engine, doc: DocId) -> Result<Embedded> {
    eng.read_custom(doc, |d| Ok(status(d)))
}

/// One undo step; saved by Save. Replacing an index runs as a removing edit, so a save rewrites the file and
/// the old records are really gone.
pub fn embed(eng: &Engine, doc: DocId, source: EditSource) -> Result<EditState> {
    let had = eng.read_custom(doc, |d| Ok(private(d).is_some()))?;
    let label = if had { "Update embedded search index" } else { "Embed search index" }.to_string();
    if had {
        eng.apply_removing(doc, label, source, add)
    } else {
        eng.apply_custom(doc, label, source, false, add)
    }
}

/// One undo step; the bytes are gone from the file once saved.
pub fn unembed(eng: &Engine, doc: DocId, source: EditSource) -> Result<EditState> {
    if !eng.read_custom(doc, |d| Ok(private(d).is_some()))? {
        return eng.edit_state(doc);
    }
    eng.apply_removing(doc, "Remove embedded search index".to_string(), source, remove)
}
