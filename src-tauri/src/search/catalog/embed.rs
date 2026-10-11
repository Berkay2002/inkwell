//! The embedded index: a document's page records kept inside the PDF, so a catalog build (here or in another
//! Inkwell) takes them instead of reading the pages again. Opt-in, per document, one edit-core step each way.
//!
//! Where: `/Root /PieceInfo /Inkwell << /LastModified (D:...) /Private <stream> >>`, the place the PDF
//! specification gives to application data. The stream is a [`FileText`] (zstd of JSON) with a small dict:
//! `/Type /InkwellSearchIndex /Version /Pages /DocID`.
//!
//! Matching is by the file's `/ID` (the first string: MuPDF writes a new second one on every save), its page count and a fingerprint of what the text, comments,
//! field values and bookmarks come from (decoded content streams, the Form XObjects, patterns and `/ToUnicode`
//! maps under the pages' resources, the comments and field values as the records read them, the bookmarks), so a
//! file changed after the index was made, by Inkwell or anything else, is read again instead of answering from
//! old text. The index is untrusted input: the dict is checked before the blob is decoded, and every stream read
//! for the fingerprint is capped, with one budget for the file (over it: not current).

use super::{bookmarks_of, pdf_id, read_pages, FileText, MAX_BLOB, VERSION};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, Result};
use crate::forms::{bad, get, put_int, put_name, put_str};
use crate::search::extras_of;
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::Buffer;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

/// What one stream may decode to while fingerprinting, and all of them together for one file.
const PER_STREAM: usize = 32 << 20;
const TOTAL: usize = 256 << 20;
/// How deep Form XObjects and patterns nest before the file is not vouched for.
const DEPTH: u32 = 8;

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

/// What the stream's dict claims, read without the blob: the document id and the page count.
fn header(s: &PdfObject) -> Option<(String, u32)> {
    let id = s.get_dict("DocID").ok().flatten()?.as_string().ok()?;
    let pages = s.get_dict("Pages").ok().flatten()?.as_int().ok()?;
    Some((id, pages.max(0) as u32))
}

/// The embedded records when they were made for this file: same `/ID` (not empty), same page count, same
/// fingerprint, current format. Anything else, or anything that fails to read or inflate within bounds, is
/// `None`: the build reads the pages instead.
pub fn read(doc: &PdfDocument, pages: u32, id: &str) -> Option<FileText> {
    if id.is_empty() {
        return None;
    }
    let s = private(doc)?;
    // The dict first: a claim for another file or page count is not worth inflating.
    if header(&s)? != (id.to_string(), pages) {
        return None;
    }
    let bytes = s.read_stream_capped(MAX_BLOB as usize).ok()?;
    let mut t = FileText::decode(&bytes)?;
    if !(t.id == id && t.pages.len() == pages as usize && !t.sig.is_empty() && fingerprint(doc).as_deref() == Some(t.sig.as_str())) {
        return None;
    }
    // A bookmark that names a page the file does not have is not a result.
    t.bookmarks.retain(|(p, _)| *p < pages);
    Some(t)
}

/// Hashes decoded streams under a size budget.
struct Feed {
    h: blake3::Hasher,
    left: usize,
    per: usize,
}

impl Feed {
    fn stream(&mut self, s: &PdfObject) -> Option<()> {
        let b = s.read_stream_capped(self.per.min(self.left)).ok()?;
        self.left -= b.len();
        self.h.update(&(b.len() as u64).to_le_bytes());
        self.h.update(&b);
        Some(())
    }

    /// Hashes one record's text and rectangle, charged to the same budget as the streams.
    fn text(&mut self, tag: u8, t: &str, r: &[f32; 4]) -> Option<()> {
        self.left = self.left.checked_sub(t.len())?;
        self.h.update(&[tag]);
        self.h.update(&(t.len() as u64).to_le_bytes());
        self.h.update(t.as_bytes());
        r.iter().for_each(|v| {
            self.h.update(&v.to_le_bytes());
        });
        Some(())
    }

    /// The Form XObjects and tiling patterns under `res` (they can show text) and the fonts' `/Encoding` and
    /// `/ToUnicode` (they decide what a glyph reads as). `None`: too deep or over the budget, so the file is not vouched for.
    fn resources(&mut self, res: &PdfObject, depth: u32, seen: &mut HashSet<i32>) -> Option<()> {
        if depth > DEPTH {
            return None;
        }
        for cat in ["Font", "XObject", "Pattern"] {
            let Some(d) = res.get_dict(cat).ok().flatten().filter(|d| d.is_dict().unwrap_or(false)) else { continue };
            for i in 0..d.dict_len().ok()? as i32 {
                let Some(o) = d.get_dict_val(i).ok().flatten() else { continue };
                let (stream, nested) = if cat == "Font" {
                    // The encoding (a name, or a dict with /Differences) decides what a code reads as.
                    if let Some(e) = o.get_dict("Encoding").ok().flatten().and_then(|e| e.resolve().ok().flatten()) {
                        let e = e.to_string();
                        if e.len() > self.per.min(self.left) {
                            return None;
                        }
                        self.left -= e.len();
                        self.h.update(e.as_bytes());
                    }
                    (o.get_dict("ToUnicode").ok().flatten(), false)
                } else {
                    let form = cat == "Pattern" || o.get_dict("Subtype").ok().flatten().and_then(|n| n.as_name().ok().map(|n| n == b"Form")) == Some(true);
                    (Some(o).filter(|_| form), true)
                };
                let Some(s) = stream.filter(|s| s.is_stream().unwrap_or(false)) else { continue };
                // A stream drawn from many pages is hashed once; a cycle ends here.
                if !s.as_indirect().is_ok_and(|n| seen.insert(n)) {
                    continue;
                }
                self.stream(&s)?;
                if nested && let Some(r) = s.get_dict("Resources").ok().flatten() {
                    self.resources(&r, depth + 1, seen)?;
                }
            }
        }
        Some(())
    }
}

/// A hash of what the text, comments, field values and bookmarks of the pages come from. Decoded streams and the
/// values the records are made of, not object numbers or encodings, so a save that renumbers or recompresses
/// does not change it. `None` when a stream cannot be read within bounds: such a file is never "current".
pub fn fingerprint(doc: &PdfDocument) -> Option<String> {
    fingerprint_with(doc, PER_STREAM, TOTAL)
}

/// [`fingerprint`] with its bounds given (tests use small ones).
pub(super) fn fingerprint_with(doc: &PdfDocument, per_stream: usize, total: usize) -> Option<String> {
    let mut f = Feed { h: blake3::Hasher::new(), left: total, per: per_stream };
    let mut seen = HashSet::new();
    let n = doc.page_count().ok()?.max(0);
    for p in 0..n {
        f.h.update(&p.to_le_bytes());
        let Ok(page) = doc.find_page(p) else { continue };
        if let Some(c) = get(&page, "Contents") {
            let parts: Vec<PdfObject> = if c.is_array().unwrap_or(false) {
                (0..c.len().unwrap_or(0)).filter_map(|i| c.get_array(i as i32).ok().flatten()).collect()
            } else {
                vec![c]
            };
            for s in parts {
                f.stream(&s)?;
            }
        }
        if let Some(res) = page.get_dict_inheritable("Resources").ok().flatten() {
            f.resources(&res, 0, &mut seen)?;
        }
        // The same read as the stored records: comments, and field values wherever the field keeps them.
        if let Ok((comments, fields)) = doc.load_page(p).map_err(|_| ()).and_then(|pg| extras_of(pg).map_err(|_| ())) {
            for (t, r) in &comments {
                f.text(1, t, r)?;
            }
            for (t, r) in &fields {
                f.text(2, t, r)?;
            }
        }
    }
    for (p, title) in bookmarks_of(doc) {
        f.text(3, &title, &[p as f32, 0.0, 0.0, 0.0])?;
    }
    Some(f.h.finalize().to_hex().to_string())
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
    // The stored size from the dict: the raw bytes are not read just to count them.
    let raw = s.get_dict("Length").ok().flatten().and_then(|l| l.as_int().ok()).unwrap_or(0).max(0) as u64;
    let n = doc.page_count().unwrap_or(0).max(0) as usize;
    let id = pdf_id(doc);
    // The dict says which file and how many pages: a mismatch is "out of date" without reading the blob.
    let Some((hid, hp)) = header(&s) else { return Embedded { present: true, current: false, pages: 0, bytes: raw } };
    let mut e = Embedded { present: true, current: false, pages: hp, bytes: raw };
    if id.is_empty() || hid != id || hp as usize != n {
        return e;
    }
    e.current = s
        .read_stream_capped(MAX_BLOB as usize)
        .ok()
        .and_then(|b| FileText::decode(&b))
        .is_some_and(|t| t.id == id && t.pages.len() == n && !t.sig.is_empty() && fingerprint(doc).as_deref() == Some(t.sig.as_str()));
    e
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
    text.sig = fingerprint(doc).ok_or_else(|| bad("a page of this document is too large or damaged to index"))?;
    install(doc, &text)
}

#[cfg(test)]
pub(super) fn install_for_test(doc: &mut PdfDocument, text: &FileText) {
    install(doc, text).unwrap();
}

/// Puts `text` in the document as its index (replacing one that is there).
fn install(doc: &mut PdfDocument, text: &FileText) -> Result<Vec<u32>> {
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

/// Takes the index out of `doc` (and `/PieceInfo` with it when it held nothing else); true when there was one.
/// The error is MuPDF's, so edits outside the engine (redaction, compress) can use it as they are.
pub fn drop_index(doc: &PdfDocument) -> std::result::Result<bool, mupdf::Error> {
    let Some(mut p) = piece(doc) else { return Ok(false) };
    if get(&p, "Inkwell").is_none() {
        return Ok(false);
    }
    p.dict_delete("Inkwell")?;
    if p.dict_len()? == 0 {
        doc.catalog()?.dict_delete("PieceInfo")?;
    }
    Ok(true)
}

/// [`drop_index`] unless the index describes the file as it is now. Edits that take content out of a file
/// (pages, text, comments, bookmarks) call this: the old records would keep what was removed.
pub fn drop_if_stale(doc: &PdfDocument) -> std::result::Result<bool, mupdf::Error> {
    if status(doc).current {
        return Ok(false);
    }
    drop_index(doc)
}

/// Takes the index out (one edit step).
pub fn remove(doc: &mut PdfDocument) -> Result<Vec<u32>> {
    drop_index(doc)?;
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
