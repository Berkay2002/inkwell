//! Attachments: the files a PDF carries, as document-level embedded files (/Names /EmbeddedFiles)
//! and as file-attachment annotations on pages. Listing, adding, reading out, describing and
//! deleting. Nothing here ever runs a file: the bytes are only read out for Save or for a PDF the
//! caller checks and opens as a document.

use super::{Result, bad, get, put_str};
use mupdf::pdf::{EmbeddedFileOptions, PdfDocument, PdfObject};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AttachKind {
    /// In the document's embedded files.
    File,
    /// The paperclip or pin on a page.
    Annotation,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// `n:<key>` (document-level, by its name in the name tree) or `a:<object number>` (an annotation).
    pub id: String,
    /// The file's name.
    pub name: String,
    pub description: String,
    pub size: u64,
    pub mime: Option<String>,
    pub kind: AttachKind,
    /// 0-based page of an annotation.
    pub page: Option<u32>,
    /// The attachment carries its bytes (a link to an outside file does not).
    pub has_data: bool,
    /// Looks like a PDF by its name or type: "Open in a new tab" is offered.
    pub is_pdf: bool,
}

pub struct NewFile {
    pub name: String,
    pub data: Vec<u8>,
    pub description: String,
}

const MAX_ENTRIES: usize = 50_000;

/// One entry of the embedded-files name tree.
struct Entry {
    key: String,
    spec: PdfObject,
}

fn names_root(doc: &PdfDocument) -> Result<Option<PdfObject>> {
    let Some(names) = get(&doc.catalog()?, "Names") else { return Ok(None) };
    Ok(names.resolve()?.filter(|n| n.is_dict().unwrap_or(false)).and_then(|n| get(&n, "EmbeddedFiles")))
}

/// The tree's entries, flattened (a /Kids tree too), in key order.
fn tree(doc: &PdfDocument) -> Result<Vec<Entry>> {
    fn walk(node: &PdfObject, depth: usize, out: &mut Vec<Entry>) -> Result<()> {
        let Some(node) = node.resolve()? else { return Ok(()) };
        if depth > 8 || !node.is_dict()? {
            return Ok(());
        }
        if let Some(names) = get(&node, "Names") {
            let n = names.len().unwrap_or(0);
            let mut i = 0;
            while i + 1 < n && out.len() < MAX_ENTRIES {
                let key = names.get_array(i as i32)?.and_then(|k| k.as_string().ok());
                if let (Some(key), Some(spec)) = (key, names.get_array(i as i32 + 1)?) {
                    out.push(Entry { key, spec });
                }
                i += 2;
            }
        }
        if let Some(kids) = get(&node, "Kids") {
            for i in 0..kids.len().unwrap_or(0).min(MAX_ENTRIES) {
                if let Some(k) = kids.get_array(i as i32)? {
                    walk(&k, depth + 1, out)?;
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if let Some(root) = names_root(doc)? {
        walk(&root, 0, &mut out)?;
    }
    Ok(out)
}

/// Writes `entries` as one flat, sorted name tree (none: the entry is removed).
fn write_tree(doc: &mut PdfDocument, mut entries: Vec<Entry>) -> Result<()> {
    let mut catalog = doc.catalog()?;
    let mut names = match get(&catalog, "Names").and_then(|n| n.resolve().ok().flatten()).filter(|n| n.is_dict().unwrap_or(false)) {
        Some(n) => n,
        None => {
            if entries.is_empty() {
                return Ok(());
            }
            let n = doc.new_dict()?;
            let n = doc.add_object(&n)?;
            catalog.dict_put("Names", n.clone())?;
            n
        }
    };
    if entries.is_empty() {
        names.dict_delete("EmbeddedFiles")?;
        return Ok(());
    }
    entries.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
    let mut array = doc.new_array()?;
    for e in &entries {
        array.array_push(PdfObject::new_string(&e.key)?)?;
        array.array_push(e.spec.clone())?;
    }
    let mut root = doc.new_dict()?;
    root.dict_put("Names", array)?;
    names.dict_put("EmbeddedFiles", root)?;
    Ok(())
}

/// The embedded file's stream of a file specification.
fn stream_of(spec: &PdfObject) -> Option<PdfObject> {
    let spec = spec.resolve().ok().flatten()?;
    if !spec.is_dict().ok()? {
        return None;
    }
    let ef = get(&spec, "EF")?.resolve().ok().flatten()?;
    // `is_stream` is true for the reference to a stream object, not for the dictionary it resolves to.
    ["UF", "F", "Unix", "DOS", "Mac"]
        .iter()
        .filter_map(|k| get(&ef, k))
        .find(|s| s.is_stream().unwrap_or(false))
}

fn info(spec: &PdfObject, fallback: &str, id: String, kind: AttachKind, page: Option<u32>, note: Option<String>) -> Attachment {
    let resolved = spec.resolve().ok().flatten().filter(|s| s.is_dict().unwrap_or(false));
    let text = |k: &str| resolved.as_ref().and_then(|s| get(s, k)).and_then(|v| v.as_string().ok()).filter(|s| !s.is_empty());
    let name = text("UF").or_else(|| text("F")).unwrap_or_else(|| fallback.to_string());
    let name = name.rsplit(['/', '\\']).next().unwrap_or(&name).to_string();
    let stream = stream_of(spec);
    let size = stream
        .as_ref()
        .and_then(|s| get(s, "Params").and_then(|p| p.resolve().ok().flatten()).and_then(|p| get(&p, "Size")).and_then(|v| v.as_int().ok()))
        .or_else(|| stream.as_ref().and_then(|s| get(s, "Length")).and_then(|v| v.as_int().ok()))
        .unwrap_or(0)
        .max(0) as u64;
    let mime = stream.as_ref().and_then(|s| get(s, "Subtype")).and_then(|v| v.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned());
    let is_pdf = name.to_lowercase().ends_with(".pdf") || mime.as_deref() == Some("application/pdf");
    Attachment {
        id,
        description: text("Desc").or(note).unwrap_or_default(),
        name,
        size,
        mime,
        kind,
        page,
        has_data: stream.is_some(),
        is_pdf,
    }
}

/// Every attachment of the document: embedded files first (by name), then the pages' file
/// attachments in page order.
pub fn list(doc: &PdfDocument) -> Result<Vec<Attachment>> {
    let mut out: Vec<Attachment> = tree(doc)?
        .iter()
        .map(|e| info(&e.spec, &e.key, format!("n:{}", e.key), AttachKind::File, None, None))
        .collect();
    let pages = doc.page_count()?.max(0);
    for p in 0..pages {
        let page = doc.find_page(p)?;
        let Some(annots) = get(&page, "Annots").and_then(|a| a.resolve().ok().flatten()) else { continue };
        for i in 0..annots.len().unwrap_or(0).min(2000) {
            let Some(item) = annots.get_array(i as i32)? else { continue };
            let num = if item.is_indirect()? { item.as_indirect()? } else { continue };
            let Some(a) = item.resolve()? else { continue };
            if !a.is_dict()? || get(&a, "Subtype").and_then(|s| s.as_name().ok()).as_deref() != Some(b"FileAttachment".as_slice()) {
                continue;
            }
            let Some(fs) = get(&a, "FS") else { continue };
            let note = get(&a, "Contents").and_then(|c| c.as_string().ok()).filter(|c| !c.is_empty());
            out.push(info(&fs, "attachment", format!("a:{num}"), AttachKind::Annotation, Some(p as u32), note));
        }
    }
    Ok(out)
}

/// Where an attachment's file specification is.
fn locate(doc: &PdfDocument, id: &str) -> Result<PdfObject> {
    if let Some(key) = id.strip_prefix("n:") {
        return tree(doc)?.into_iter().find(|e| e.key == key).map(|e| e.spec).ok_or_else(|| bad("that attachment is no longer in the document"));
    }
    if let Some(n) = id.strip_prefix("a:").and_then(|n| n.parse::<i32>().ok()) {
        let a = doc.new_indirect(n, 0)?.resolve()?.filter(|a| a.is_dict().unwrap_or(false));
        return a.and_then(|a| get(&a, "FS")).ok_or_else(|| bad("that attachment is no longer in the document"));
    }
    Err(bad("unknown attachment"))
}

/// The file's name and its bytes.
pub fn read_data(doc: &PdfDocument, id: &str) -> Result<(String, Vec<u8>)> {
    let spec = locate(doc, id)?;
    let key = id.strip_prefix("n:").unwrap_or("attachment");
    let meta = info(&spec, key, id.to_string(), AttachKind::File, None, None);
    let Some(stream) = stream_of(&spec) else {
        return Err(bad("this attachment links to a file outside the document, it has no bytes to save"));
    };
    Ok((meta.name, stream.read_stream()?))
}

/// A name no other entry of the tree has: `x.txt`, then `x (2).txt`, `x (3).txt` …
fn unique(taken: &HashSet<String>, name: &str) -> String {
    if !taken.contains(&name.to_lowercase()) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|c| !taken.contains(&c.to_lowercase()))
        .unwrap_or_else(|| name.to_string())
}

/// The file name without any folder, and nothing a file system or a viewer would trip on.
pub fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base.chars().filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')).collect();
    let s = s.trim().trim_matches('.').trim().to_string();
    if s.is_empty() { "attachment".into() } else { s }
}

fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_lowercase).as_deref() {
        Some("pdf") => "application/pdf",
        Some("txt" | "log" | "md") => "text/plain",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("html" | "htm") => "text/html",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("zip") => "application/zip",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}

/// Attaches `files` (names made unique among the embedded files). Returns the pages touched: none.
pub fn add(doc: &mut PdfDocument, files: &[NewFile]) -> Result<Vec<u32>> {
    if files.is_empty() {
        return Err(bad("no files to attach"));
    }
    // One flat, sorted tree first: MuPDF's own helper appends to a flat array only.
    let existing = tree(doc)?;
    write_tree(doc, existing)?;
    for f in files {
        let taken: HashSet<String> = tree(doc)?.into_iter().map(|e| e.key.to_lowercase()).collect();
        let name = unique(&taken, &clean_name(&f.name));
        let mut opts = EmbeddedFileOptions::new(&name);
        opts.mime_type = Some(mime_for(&name));
        opts.add_checksum = true;
        let made = doc.add_embedded_file(&name, &f.data, opts)?;
        if !f.description.is_empty() {
            if let Some(mut spec) = doc.new_indirect(made.xref, 0)?.resolve()? {
                put_str(&mut spec, "Desc", &f.description)?;
            }
        }
        // MuPDF appended it; keep the keys in order.
        let all = tree(doc)?;
        write_tree(doc, all)?;
    }
    Ok(Vec::new())
}

/// Takes the attachments out (document-level: the name tree and /AF; annotations: the page).
/// The caller applies this as a removing edit so a save leaves no trace of the bytes.
pub fn remove(doc: &mut PdfDocument, ids: &[String]) -> Result<Vec<u32>> {
    let mut touched = Vec::new();
    let mut gone_specs: Vec<i32> = Vec::new();
    let mut entries = tree(doc)?;
    let before = entries.len();
    let mut annots: Vec<i32> = Vec::new();
    for id in ids {
        if let Some(key) = id.strip_prefix("n:") {
            if let Some(i) = entries.iter().position(|e| e.key == key) {
                let e = entries.remove(i);
                if e.spec.is_indirect().unwrap_or(false) {
                    gone_specs.push(e.spec.as_indirect()?);
                }
            }
        } else if let Some(n) = id.strip_prefix("a:").and_then(|n| n.parse::<i32>().ok()) {
            annots.push(n);
        } else {
            return Err(bad("unknown attachment"));
        }
    }
    if entries.len() != before {
        write_tree(doc, entries)?;
        // The associated-files list (/AF) must not point at a file that is gone.
        let mut catalog = doc.catalog()?;
        if let Some(mut af) = get(&catalog, "AF").and_then(|a| a.resolve().ok().flatten()).filter(|a| a.is_array().unwrap_or(false)) {
            for i in (0..af.len()?).rev() {
                let hit = af.get_array(i as i32)?.is_some_and(|v| v.is_indirect().unwrap_or(false) && v.as_indirect().is_ok_and(|n| gone_specs.contains(&n)));
                if hit {
                    af.array_delete(i as i32)?;
                }
            }
            if af.len()? == 0 {
                catalog.dict_delete("AF")?;
            }
        }
    }
    if !annots.is_empty() {
        let pages = doc.page_count()?.max(0);
        for p in 0..pages {
            let page = doc.find_page(p)?;
            let Some(mut list) = get(&page, "Annots").and_then(|a| a.resolve().ok().flatten()).filter(|a| a.is_array().unwrap_or(false)) else { continue };
            let mut changed = false;
            for i in (0..list.len()?).rev() {
                let Some(item) = list.get_array(i as i32)? else { continue };
                let num = if item.is_indirect()? { item.as_indirect()? } else { continue };
                let popup_of = item.resolve()?.and_then(|a| get(&a, "Parent")).filter(|p| p.is_indirect().unwrap_or(false)).and_then(|p| p.as_indirect().ok());
                if annots.contains(&num) || popup_of.is_some_and(|n| annots.contains(&n)) {
                    list.array_delete(i as i32)?;
                    changed = true;
                }
            }
            if changed {
                touched.push(p as u32);
            }
        }
    }
    Ok(touched)
}

/// Sets the description (an empty text removes it).
pub fn describe(doc: &mut PdfDocument, id: &str, text: &str) -> Result<Vec<u32>> {
    let spec = locate(doc, id)?;
    let Some(mut spec) = spec.resolve()?.filter(|s| s.is_dict().unwrap_or(false)) else {
        return Err(bad("this attachment has no file specification to describe"));
    };
    if text.is_empty() {
        spec.dict_delete("Desc")?;
    } else {
        put_str(&mut spec, "Desc", text)?;
    }
    if let Some(n) = id.strip_prefix("a:").and_then(|n| n.parse::<i32>().ok()) {
        if let Some(mut a) = doc.new_indirect(n, 0)?.resolve()?.filter(|a| a.is_dict().unwrap_or(false)) {
            if text.is_empty() {
                a.dict_delete("Contents")?;
            } else {
                put_str(&mut a, "Contents", text)?;
            }
        }
    }
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_unique_and_clean() {
        let taken: HashSet<String> = ["a.txt".to_string(), "a (2).txt".to_string()].into();
        assert_eq!(unique(&taken, "A.txt"), "A (3).txt");
        assert_eq!(unique(&taken, "b.txt"), "b.txt");
        assert_eq!(clean_name("C:\\x\\y\\report?.pdf"), "report.pdf");
        assert_eq!(clean_name("../.."), "attachment");
        assert_eq!(mime_for("a.PDF"), "application/pdf");
    }
}
