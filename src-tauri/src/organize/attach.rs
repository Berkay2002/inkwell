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
    /// The file's own time of last change (Unix seconds), kept as /Params /ModDate.
    pub modified: Option<i64>,
}

const MAX_ENTRIES: usize = 50_000;

/// The largest attachment taken in or read out.
pub const MAX_ATTACH: u64 = 512 << 20;

/// One entry of a name tree (the embedded files; the document-level JavaScript, see `forms::doc_js`).
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) spec: PdfObject,
}

fn names_root(doc: &PdfDocument, tree_key: &str) -> Result<Option<PdfObject>> {
    let Some(names) = get(&doc.catalog()?, "Names") else { return Ok(None) };
    Ok(names.resolve()?.filter(|n| n.is_dict().unwrap_or(false)).and_then(|n| get(&n, tree_key)))
}

/// The embedded files' entries, flattened (a /Kids tree too), in key order.
pub(super) fn tree(doc: &PdfDocument) -> Result<Vec<Entry>> {
    tree_of(doc, "EmbeddedFiles")
}

/// The entries of the `/Names /<tree_key>` tree, flattened, with the walk's visited set and caps.
pub(crate) fn tree_of(doc: &PdfDocument, tree_key: &str) -> Result<Vec<Entry>> {
    /// Nodes looked at in all: /Kids may point at each other or at one node many times over.
    const MAX_NODES: usize = 100_000;
    fn walk(node: &PdfObject, depth: usize, out: &mut Vec<Entry>, seen: &mut HashSet<i32>, nodes: &mut usize) -> Result<()> {
        if depth > 8 || *nodes >= MAX_NODES {
            return Ok(());
        }
        // A node reached twice is read once (a loop or a repeat in /Kids).
        if node.is_indirect()? && !seen.insert(node.as_indirect()?) {
            return Ok(());
        }
        *nodes += 1;
        let Some(node) = node.resolve()? else { return Ok(()) };
        if !node.is_dict()? {
            return Ok(());
        }
        if let Some(names) = get(&node, "Names") {
            let n = names.len().unwrap_or(0);
            let mut i = 0;
            while i + 1 < n && out.len() < MAX_ENTRIES {
                // A key in UTF-16 is not a Rust string as it stands: read its bytes as a PDF text string.
                let key = names.get_array(i as i32)?.and_then(|k| k.as_string().ok().or_else(|| k.as_bytes().ok().map(|b| crate::outline::decode_text(&b))));
                if let (Some(key), Some(spec)) = (key, names.get_array(i as i32 + 1)?) {
                    out.push(Entry { key, spec });
                }
                i += 2;
            }
        }
        if let Some(kids) = get(&node, "Kids") {
            for i in 0..kids.len().unwrap_or(0).min(MAX_ENTRIES) {
                if out.len() >= MAX_ENTRIES || *nodes >= MAX_NODES {
                    break;
                }
                if let Some(k) = kids.get_array(i as i32)? {
                    walk(&k, depth + 1, out, seen, nodes)?;
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if let Some(root) = names_root(doc, tree_key)? {
        walk(&root, 0, &mut out, &mut HashSet::new(), &mut 0)?;
    }
    Ok(out)
}

/// The id of each entry (`n:<key>`); the second and later entries with one key get the index after a
/// separator, so every id names one entry.
pub(crate) fn entry_ids(entries: &[Entry]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    entries.iter().enumerate().map(|(i, e)| if seen.insert(&e.key) { format!("n:{}", e.key) } else { format!("n:{}{ID_SEP}{i}", e.key) }).collect()
}

const ID_SEP: char = '\u{1f}';

/// The index of the entry an `n:` id names (the first one with that key, or the one an [`entry_ids`] suffix says).
pub(crate) fn find_entry(entries: &[Entry], id: &str) -> Option<usize> {
    let key = id.strip_prefix("n:")?;
    if let Some(i) = entries.iter().position(|e| e.key == key) {
        return Some(i);
    }
    let (base, at) = key.rsplit_once(ID_SEP)?;
    let i: usize = at.parse().ok()?;
    entries.get(i).filter(|e| e.key == base).map(|_| i)
}

/// Writes `entries` as the one flat, sorted embedded-files tree (none: the entry is removed).
pub(super) fn write_tree(doc: &mut PdfDocument, entries: Vec<Entry>) -> Result<()> {
    write_tree_of(doc, "EmbeddedFiles", entries)
}

/// A name tree key as a PDF string: plain when ASCII, else UTF-16BE with a byte order mark (what readers decode).
fn key_object(doc: &PdfDocument, key: &str) -> Result<PdfObject> {
    if key.is_ascii() && !key.contains('\0') {
        return Ok(PdfObject::new_string(key)?);
    }
    let mut hex = String::from("<FEFF");
    for u in key.replace('\0', " ").encode_utf16() {
        hex.push_str(&format!("{u:04X}"));
    }
    hex.push('>');
    Ok(doc.new_object_from_str(&hex)?)
}

/// Writes `entries` as `/Names /<tree_key>`, one flat, sorted name tree (none: the entry is removed).
pub(crate) fn write_tree_of(doc: &mut PdfDocument, tree_key: &str, mut entries: Vec<Entry>) -> Result<()> {
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
        names.dict_delete(tree_key)?;
        return Ok(());
    }
    entries.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
    let mut array = doc.new_array()?;
    for e in &entries {
        array.array_push(key_object(doc, &e.key)?)?;
        array.array_push(e.spec.clone())?;
    }
    let mut root = doc.new_dict()?;
    root.dict_put("Names", array)?;
    names.dict_put(tree_key, root)?;
    Ok(())
}

/// The embedded file's stream of a file specification.
pub(super) fn stream_of(spec: &PdfObject) -> Option<PdfObject> {
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

/// The longest text taken from one string of the file: a string can be megabytes, and a hostile file
/// can point any number of fields at it.
const TEXT_MAX: usize = 2048;

/// The string `o` as text, cut to [`TEXT_MAX`] bytes.
pub(super) fn capped_string(o: &PdfObject) -> Option<String> {
    let mut s = o.as_string().ok()?;
    if s.len() > TEXT_MAX {
        let mut n = TEXT_MAX;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
    }
    Some(s)
}

/// [`capped_string`] remembered by object number, so a string many fields point at is read once.
#[derive(Default)]
pub(super) struct Texts {
    seen: std::collections::HashMap<i32, Option<String>>,
}

impl Texts {
    pub(super) fn get(&mut self, o: &PdfObject) -> Option<String> {
        let num = if o.is_indirect().unwrap_or(false) { o.as_indirect().ok() } else { None };
        if let Some(t) = num.and_then(|n| self.seen.get(&n)) {
            return t.clone();
        }
        let t = capped_string(o);
        if let Some(n) = num.filter(|_| self.seen.len() < 100_000) {
            self.seen.insert(n, t.clone());
        }
        t
    }
}

/// The longest name kept of an attachment (no file system takes more).
const NAME_MAX: usize = 255;

pub(super) fn info(spec: &PdfObject, fallback: &str, id: String, kind: AttachKind, page: Option<u32>, note: Option<String>) -> Attachment {
    info_with(&mut Texts::default(), spec, fallback, id, kind, page, note)
}

pub(super) fn info_with(texts: &mut Texts, spec: &PdfObject, fallback: &str, id: String, kind: AttachKind, page: Option<u32>, note: Option<String>) -> Attachment {
    let resolved = spec.resolve().ok().flatten().filter(|s| s.is_dict().unwrap_or(false));
    let mut text = |k: &str| resolved.as_ref().and_then(|s| get(s, k)).and_then(|v| texts.get(&v)).filter(|s| !s.is_empty());
    // The name comes from the file: no folder, nothing that reorders or hides text, nothing a file
    // system refuses (it becomes the save dialog's default and a scratch file's name).
    let name = clean_name(&text("UF").or_else(|| text("F")).unwrap_or_else(|| fallback.to_string()));
    let name = if name.chars().count() > NAME_MAX {
        // Cut the stem, so the extension (what says the kind) stays.
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 && name[i..].chars().count() <= 20 => name.split_at(i),
            _ => (name.as_str(), ""),
        };
        format!("{}{ext}", stem.chars().take(NAME_MAX - ext.chars().count()).collect::<String>())
    } else {
        name
    };
    let description = strip_invisible(&text("Desc").or(note).unwrap_or_default());
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
        description,
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
    let entries = tree(doc)?;
    let mut texts = Texts::default();
    let mut out: Vec<Attachment> = entries
        .iter()
        .zip(entry_ids(&entries))
        .map(|(e, id)| info_with(&mut texts, &e.spec, &e.key, id, AttachKind::File, None, None))
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
            out.push(info_with(&mut texts, &fs, "attachment", format!("a:{num}"), AttachKind::Annotation, Some(p as u32), note));
        }
    }
    Ok(out)
}

/// Where an attachment's file specification is.
pub(super) fn locate(doc: &PdfDocument, id: &str) -> Result<PdfObject> {
    if id.starts_with("n:") {
        let mut entries = tree(doc)?;
        return find_entry(&entries, id).map(|i| entries.swap_remove(i).spec).ok_or_else(|| bad("that attachment is no longer in the document"));
    }
    if let Some(n) = id.strip_prefix("a:").and_then(|n| n.parse::<i32>().ok()) {
        let a = doc.new_indirect(n, 0)?.resolve()?.filter(|a| a.is_dict().unwrap_or(false));
        return a.and_then(|a| get(&a, "FS")).ok_or_else(|| bad("that attachment is no longer in the document"));
    }
    Err(bad("unknown attachment"))
}

/// The file's name and its bytes (refused over [`MAX_ATTACH`] once decoded).
pub fn read_data(doc: &PdfDocument, id: &str) -> Result<(String, Vec<u8>)> {
    read_data_capped(doc, id, MAX_ATTACH as usize)
}

/// [`read_data`] with the size limit given. The limit holds for the decoded bytes, whatever the
/// stream declares about itself, so a small compressed stream cannot inflate past it.
pub fn read_data_capped(doc: &PdfDocument, id: &str, cap: usize) -> Result<(String, Vec<u8>)> {
    let spec = locate(doc, id)?;
    let key = id.strip_prefix("n:").map_or("attachment", |k| k.split(ID_SEP).next().unwrap_or(k));
    let meta = info(&spec, key, id.to_string(), AttachKind::File, None, None);
    let Some(stream) = stream_of(&spec) else {
        return Err(bad("this attachment links to a file outside the document, it has no bytes to save"));
    };
    if meta.size > cap as u64 {
        return Err(bad(format!("{} is over {} MB, too large to open or save", meta.name, cap >> 20)));
    }
    match stream.read_stream_capped(cap) {
        Ok(bytes) => Ok((meta.name, bytes)),
        Err(e) if e.is_stream_cap() => {
            Err(bad(format!("{} is over {} MB once unpacked, too large to open or save", meta.name, cap >> 20)))
        }
        Err(e) => Err(e.into()),
    }
}

/// A name no other entry of the tree has: `x.txt`, then `x (2).txt`, `x (3).txt` …
pub(super) fn unique(taken: &HashSet<String>, name: &str) -> String {
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

/// Characters that reorder or hide text (bidi overrides and isolates, marks, zero-width, BOM).
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}')
}

/// `s` without control characters (a line break stays) and without the invisible ones.
pub(super) fn strip_invisible(s: &str) -> String {
    s.chars().filter(|&c| (!c.is_control() || c == '\n') && !is_invisible(c)).collect()
}

/// Names Windows keeps for devices, with or without an extension.
fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    // COM1-9 and LPT1-9, and with superscript digits (devices on current Windows too).
    let mut c = stem.chars();
    let head: String = c.by_ref().take(3).collect();
    matches!(head.as_str(), "COM" | "LPT") && matches!(c.next(), Some('1'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}')) && c.next().is_none()
}

/// The file name without any folder, and nothing a file system or a viewer would trip on.
pub fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let s: String = base.chars().filter(|&c| !c.is_control() && !is_invisible(c) && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')).collect();
    // Windows drops trailing dots and spaces: trim until nothing is left to drop ("a. ." is "a").
    let mut s = s.as_str();
    loop {
        let t = s.trim().trim_matches('.');
        if t.len() == s.len() {
            break;
        }
        s = t;
    }
    let s = s.to_string();
    if s.is_empty() {
        "attachment".into()
    } else if is_reserved(&s) {
        format!("_{s}")
    } else {
        s
    }
}

pub(super) fn mime_for(name: &str) -> &'static str {
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
        add_one(doc, f)?;
    }
    Ok(Vec::new())
}

/// Attaches one file to a document whose tree is already flat; the key it got (its name, made unique).
pub(super) fn add_one(doc: &mut PdfDocument, f: &NewFile) -> Result<String> {
    let taken: HashSet<String> = tree(doc)?.into_iter().map(|e| e.key.to_lowercase()).collect();
    let name = unique(&taken, &clean_name(&f.name));
    let mut opts = EmbeddedFileOptions::new(&name);
    opts.mime_type = Some(mime_for(&name));
    opts.add_checksum = true;
    opts.modified = f.modified;
    let made = doc.add_embedded_file(&name, &f.data, opts)?;
    if !f.description.is_empty() {
        if let Some(mut spec) = doc.new_indirect(made.xref, 0)?.resolve()? {
            put_str(&mut spec, "Desc", &f.description)?;
        }
    }
    // MuPDF appended it; keep the keys in order.
    let all = tree(doc)?;
    write_tree(doc, all)?;
    Ok(name)
}

/// Takes the attachments out (document-level: the name tree and /AF; annotations: the page).
/// The caller applies this as a removing edit so a save leaves no trace of the bytes.
pub fn remove(doc: &mut PdfDocument, ids: &[String]) -> Result<Vec<u32>> {
    let mut touched = Vec::new();
    let mut gone_specs: Vec<i32> = Vec::new();
    let mut entries = tree(doc)?;
    let before = entries.len();
    let mut annots: Vec<i32> = Vec::new();
    let mut gone: Vec<usize> = Vec::new();
    for id in ids {
        if id.starts_with("n:") {
            gone.extend(find_entry(&entries, id));
        } else if let Some(n) = id.strip_prefix("a:").and_then(|n| n.parse::<i32>().ok()) {
            annots.push(n);
        } else {
            return Err(bad("unknown attachment"));
        }
    }
    gone.sort_unstable();
    gone.dedup();
    for &i in gone.iter().rev() {
        let e = entries.remove(i);
        if e.spec.is_indirect().unwrap_or(false) {
            gone_specs.push(e.spec.as_indirect()?);
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

    #[test]
    fn windows_device_names_and_trailing_dots_do_not_get_through() {
        for n in ["CONIN$", "conout$.txt", "COM\u{b9}", "lpt\u{b2}.log", "COM\u{b3}.txt", "COM9", "nul.tar.gz"] {
            assert!(clean_name(n).starts_with('_'), "{n} -> {}", clean_name(n));
        }
        for n in ["COM10", "COMX", "console.txt", "COM\u{2074}"] {
            assert_eq!(clean_name(n), n);
        }
        // Windows drops trailing dots and spaces, so a name must not rely on them
        assert_eq!(clean_name("a. ."), "a");
        assert_eq!(clean_name(" . a . . "), "a");
        assert_eq!(clean_name(". ."), "attachment");
    }

    #[test]
    fn names_lose_bidi_tricks_and_device_names() {
        assert_eq!(clean_name("evil\u{202E}fdp.exe"), "evilfdp.exe");
        assert_eq!(clean_name("CON"), "_CON");
        assert_eq!(clean_name("nul.txt"), "_nul.txt");
        assert_eq!(clean_name("COM1.log"), "_COM1.log");
        assert_eq!(clean_name("COM0.log"), "COM0.log");
        assert_eq!(clean_name("a:b.txt"), "ab.txt");
        assert_eq!(clean_name("\u{200B}\u{FEFF}"), "attachment");
        assert_eq!(strip_invisible("a\u{202E}b\nc\u{7}"), "ab\nc");
    }

    #[test]
    fn a_stream_over_the_cap_is_refused() {
        let mut doc = PdfDocument::new();
        let file = NewFile { name: "zeros.bin".into(), data: vec![0u8; 3 << 20], description: String::new(), modified: None };
        add(&mut doc, &[file]).unwrap();
        let (name, bytes) = read_data_capped(&doc, "n:zeros.bin", 4 << 20).unwrap();
        assert_eq!((name.as_str(), bytes.len()), ("zeros.bin", 3 << 20));
        let e = read_data_capped(&doc, "n:zeros.bin", 1 << 20).unwrap_err().to_string();
        assert!(e.contains("too large"), "{e}");
    }

    /// The size a stream declares is not trusted: with no /Params /Size, and a /Length that is the few
    /// kilobytes of Flate data, only the capped read can notice that it inflates past the cap.
    #[test]
    fn a_small_flate_stream_that_inflates_past_the_cap_is_stopped_by_the_read() {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;
        let mut doc = PdfDocument::new();
        let file = NewFile { name: "bomb.bin".into(), data: vec![1u8; 16], description: String::new(), modified: None };
        add(&mut doc, &[file]).unwrap();
        let spec = locate(&doc, "n:bomb.bin").unwrap();
        let mut stream = stream_of(&spec).unwrap();
        let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
        z.write_all(&vec![0u8; 8 << 20]).unwrap();
        let packed = z.finish().unwrap();
        assert!(packed.len() < 64 << 10, "{}", packed.len());
        stream.write_raw_stream_buffer(&mupdf::Buffer::from_bytes(&packed).unwrap()).unwrap();
        stream.dict_put("Filter", PdfObject::new_name("FlateDecode").unwrap()).unwrap();
        stream.dict_delete("Params").unwrap();
        stream.dict_delete("DecodeParms").ok();
        // what the pre-check sees is the packed length, well under the cap
        let declared = info(&spec, "bomb.bin", "n:bomb.bin".into(), AttachKind::File, None, None).size;
        assert!(declared > 0 && declared < 1 << 20, "declared {declared}");
        let e = read_data_capped(&doc, "n:bomb.bin", 1 << 20).unwrap_err().to_string();
        assert!(e.contains("once unpacked"), "{e}");
        // a cap above the real size reads all of it
        let (_, bytes) = read_data_capped(&doc, "n:bomb.bin", 16 << 20).unwrap();
        assert_eq!(bytes.len(), 8 << 20);
    }
}
