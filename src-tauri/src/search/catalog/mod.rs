//! Catalog: a full-text index over a folder of PDFs, so a search does not read every file again.
//!
//! The index is our own small inverted index, not a search library's: a query must give the same hits as
//! `search_files` for every option set, and that is only certain when the same matcher decides. So the index
//! only narrows the pages (words of the page text in the widest fold: case folded, accents dropped, both
//! readings of a line-end hyphen), and `PageRec::find` on the stored page record decides, exactly as it does
//! when a file is read (`search/mod.rs`). Everything lives in `<app data>/catalogs/<id>/`, never in the
//! user's folders:
//!
//! - `catalog.json` ([`Meta`]): name, folder, counts, and the generation that is live. Written last, by rename:
//!   that is the commit. A build that is cancelled or crashes never renames it, so the old index stays.
//! - `table-<generation>.json` ([`Table`]): one row per listed PDF (path, size, mtime, head/tail hash, `/ID`, pages, status).
//! - `index-<generation>.bin`: the postings, with a format version and a blake3 over them. Corrupt or old: not
//!   trusted, rebuilt.
//! - `text/<fid>.zst`: per file, the stored page records ([`FileText`]), zstd. A file id is never reused, so a
//!   changed file is a new id and the old blob stays valid until the commit.
//!
//! Paths are stored here and shown, but the webview never sends one: commands take and return tokens (paths.rs).

pub mod app;
pub mod embed;
#[cfg(test)]
mod tests;

use super::files::{be_polite, collect, list_pdfs, FilesEvent, FolderSearches, Skip, PER_FILE, TOTAL};
use super::{bookmark_hits_of, extract_record, PageChars, PageRec, Query, SearchOpts, StoredText};
use crate::engine::{EngineError, Result};
use mupdf::pdf::PdfDocument;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// Format version of every file of a catalog; another one is rebuilt.
pub const VERSION: u32 = 1;
/// Bytes of stored page records (text, comments and field values, as the JSON a blob holds) kept for one file;
/// later pages are left out and the file says so. Bookmark titles get a quarter of it again, so a blob of our
/// own always inflates to well under `MAX_BLOB` and is never refused by `FileText::decode`.
pub const MAX_TEXT: usize = 20 << 20;
/// What one file's stored records may inflate to (the embedded index of a PDF is untrusted input).
const MAX_BLOB: u64 = 96 << 20;
/// A PDF up to this size is read into memory and opened from there, so no handle is left on the file (a build
/// must not block the user's Save-over, delete or rename of it).
const MAX_READ: u64 = 128 << 20;
/// Pages of one file that are indexed, and bookmarks kept.
const MAX_PAGES: usize = 100_000;
const MAX_BOOKMARKS: usize = 50_000;
/// Bytes read from the start and from the end of a file for its signature.
const SAMPLE: u64 = 64 << 10;
const MAGIC: &[u8; 8] = b"INKWCAT\0";
const PARALLEL: usize = 2;

// ---- what is stored ----------------------------------------------------------------------------------------

/// How a listed file was handled.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Indexed,
    /// Could not be opened.
    Damaged,
    /// Needs a password: nothing was read.
    Password,
}

/// One row of the files table.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileRec {
    pub fid: u32,
    pub path: String,
    pub size: u64,
    /// Modified time, milliseconds since the Unix epoch.
    pub mtime: u64,
    /// blake3 (hex) of the first and last 64 KB.
    pub head_tail: String,
    /// The PDF's `/ID` (first string, hex); empty when it has none.
    pub pdf_id: String,
    pub pages: u32,
    pub status: Status,
    /// Stored text hit `MAX_TEXT`: later pages are not indexed.
    pub cut: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Table {
    pub version: u32,
    pub generation: u32,
    pub next_fid: u32,
    /// In listing order: a file's position here is the `index` of its result, as in a folder search.
    pub files: Vec<FileRec>,
}

/// `catalog.json`: what the list shows, and which generation is live.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub version: u32,
    pub id: String,
    pub name: String,
    /// For display, and where a refresh looks; set when the user chose the folder.
    pub folder: String,
    pub recursive: bool,
    pub created: u64,
    /// When the live generation was built (0: never).
    pub built: u64,
    pub generation: u32,
    pub files: u32,
    pub indexed: u32,
    pub pages: u64,
    pub damaged: u32,
    pub password: u32,
    pub cloud_only: u32,
    pub cut: u32,
    pub capped: bool,
    pub index_len: u64,
}

/// One page as stored: the matcher's record (`PageRec`) with the text in its compact form.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct StoredPage {
    pub text: StoredText,
    pub comments: Vec<(String, [f32; 4])>,
    pub fields: Vec<(String, [f32; 4])>,
}

impl StoredPage {
    pub fn rec(&self) -> PageRec {
        PageRec { text: PageChars::from_stored(&self.text), comments: self.comments.clone(), fields: self.fields.clone() }
    }
}

/// Everything one file contributes: its page records and bookmark titles. This is also what a PDF carries when
/// its index is embedded (`embed.rs`), so a build can take it instead of reading the pages.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct FileText {
    pub version: u32,
    /// The PDF's `/ID` it was made for.
    pub id: String,
    pub pages: Vec<StoredPage>,
    /// (page, title) of every outline entry that goes to a page.
    pub bookmarks: Vec<(u32, String)>,
    pub cut: bool,
    /// Only for an embedded index: what the pages' content and annotations hashed to (`embed::fingerprint`), so
    /// text changed after the index was made, by anything, is noticed. Empty in a catalog's own blobs.
    #[serde(default)]
    pub sig: String,
}

impl FileText {
    /// `None` when the records inflate to more than `MAX_BLOB`: what `decode` would refuse is never written.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let raw = serde_json::to_vec(self).ok()?;
        if raw.len() as u64 > MAX_BLOB {
            return None;
        }
        zstd::encode_all(raw.as_slice(), 3).ok().filter(|b| !b.is_empty())
    }

    /// `None` for anything that is not a current, sane record: wrong version, too big once inflated, or broken.
    pub fn decode(bytes: &[u8]) -> Option<FileText> {
        let mut raw = Vec::new();
        zstd::stream::read::Decoder::new(bytes).ok()?.take(MAX_BLOB + 1).read_to_end(&mut raw).ok()?;
        if raw.len() as u64 > MAX_BLOB {
            return None;
        }
        let t: FileText = serde_json::from_slice(&raw).ok()?;
        (t.version == VERSION && t.pages.len() <= MAX_PAGES && t.bookmarks.len() <= MAX_BOOKMARKS).then_some(t)
    }
}

// ---- paths of one catalog ----------------------------------------------------------------------------------

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("catalog.json")
}
fn table_path(dir: &Path, generation: u32) -> PathBuf {
    dir.join(format!("table-{generation}.json"))
}
fn index_path(dir: &Path, generation: u32) -> PathBuf {
    dir.join(format!("index-{generation}.bin"))
}
fn text_dir(dir: &Path) -> PathBuf {
    dir.join("text")
}
fn text_path(dir: &Path, fid: u32) -> PathBuf {
    text_dir(dir).join(format!("{fid}.zst"))
}

/// Ids are 32 hex digits we made; anything else never becomes a path.
pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn io(e: impl std::fmt::Display) -> EngineError {
    EngineError::Io(e.to_string())
}

/// Writes to a temp file and renames over the target: a crash never leaves half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_tmp_rename(path, bytes, false)
}

/// [`write_atomic`] with the bytes flushed to disk before the rename: for what a commit stands on (a rename can
/// reach the disk before the data it names). The per-file blobs do not pay for it; a torn one fails to decode.
fn write_durable(path: &Path, bytes: &[u8]) -> Result<()> {
    write_tmp_rename(path, bytes, true)
}

fn write_tmp_rename(path: &Path, bytes: &[u8], sync: bool) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::File::create(&tmp).map_err(io)?;
    f.write_all(bytes).map_err(io)?;
    if sync {
        f.sync_all().map_err(io)?;
    }
    drop(f);
    std::fs::rename(&tmp, path).map_err(io)
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn stamp(m: &std::fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

pub fn load_meta(dir: &Path) -> Result<Meta> {
    let b = std::fs::read(meta_path(dir)).map_err(io)?;
    let m: Meta = serde_json::from_slice(&b).map_err(io)?;
    if m.version != VERSION || !valid_id(&m.id) {
        return Err(EngineError::Invalid("this catalog was made by another version".into()));
    }
    Ok(m)
}

/// What can be read of a catalog whose `catalog.json` is damaged or from another version: a catalog with no
/// live generation, so it is listed, can be built again (which rewrites the entry) or deleted. `None` when the
/// folder is not a catalog at all.
fn unusable_meta(dir: &Path) -> Option<Meta> {
    let id = dir.file_name()?.to_string_lossy().into_owned();
    if !valid_id(&id) || !meta_path(dir).is_file() {
        return None;
    }
    let v: serde_json::Value = std::fs::read(meta_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let name: String = text("name").trim().chars().take(80).collect();
    Some(Meta {
        version: VERSION,
        id,
        name: if name.is_empty() { "Catalog".to_string() } else { name },
        folder: text("folder"),
        recursive: v.get("recursive").and_then(|x| x.as_bool()).unwrap_or(true),
        created: v.get("created").and_then(|x| x.as_u64()).unwrap_or(0),
        built: 0,
        generation: 0,
        files: 0,
        indexed: 0,
        pages: 0,
        damaged: 0,
        password: 0,
        cloud_only: 0,
        cut: 0,
        capped: false,
        index_len: 0,
    })
}

/// [`load_meta`], or the entry of a catalog that has to be built again.
fn load_meta_or_unusable(dir: &Path) -> Result<Meta> {
    load_meta(dir).or_else(|e| unusable_meta(dir).ok_or(e))
}

fn save_meta(dir: &Path, m: &Meta) -> Result<()> {
    write_durable(&meta_path(dir), &serde_json::to_vec_pretty(m).map_err(io)?)
}

/// A new, empty catalog for `folder` under `root`.
pub fn create(root: &Path, name: &str, folder: &str, recursive: bool) -> Result<Meta> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(io)?;
    let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let dir = root.join(&id);
    std::fs::create_dir_all(text_dir(&dir)).map_err(io)?;
    let m = Meta {
        version: VERSION,
        id,
        name: name.trim().chars().take(80).collect(),
        folder: folder.to_string(),
        recursive,
        created: now_ms(),
        built: 0,
        generation: 0,
        files: 0,
        indexed: 0,
        pages: 0,
        damaged: 0,
        password: 0,
        cloud_only: 0,
        cut: 0,
        capped: false,
        index_len: 0,
    };
    save_meta(&dir, &m)?;
    Ok(m)
}

/// Every catalog under `root`, oldest first; one whose entry is unreadable is listed as not built. A folder
/// that is not a catalog is left alone.
pub fn list(root: &Path) -> Vec<Meta> {
    let mut out: Vec<Meta> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let dir = e.path();
            load_meta(&dir).ok().filter(|m| e.file_name().to_string_lossy() == m.id).or_else(|| unusable_meta(&dir))
        })
        .collect();
    out.sort_by_key(|m| (m.created, m.id.clone()));
    out
}

pub fn delete(root: &Path, id: &str) -> Result<()> {
    if !valid_id(id) {
        return Err(EngineError::Invalid("unknown catalog".into()));
    }
    let dir = root.join(id);
    if dir.is_dir() {
        std::fs::remove_dir_all(dir).map_err(io)?;
    }
    Ok(())
}

// ---- postings ----------------------------------------------------------------------------------------------

fn put_var(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_var(b: &[u8], pos: &mut usize) -> Option<u64> {
    let (mut v, mut shift) = (0u64, 0u32);
    loop {
        let x = *b.get(*pos)?;
        *pos += 1;
        v |= u64::from(x & 0x7f) << shift;
        if x & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// A term's postings are a run of chunks: file id, page count, first page, then the gaps.
fn walk_chunks(b: &[u8], mut f: impl FnMut(u32, &[u32], std::ops::Range<usize>)) -> Option<()> {
    let mut pos = 0;
    let mut pages: Vec<u32> = Vec::new();
    while pos < b.len() {
        let start = pos;
        let fid = u32::try_from(get_var(b, &mut pos)?).ok()?;
        let n = get_var(b, &mut pos)? as usize;
        if n > b.len() {
            return None;
        }
        pages.clear();
        let mut last = 0u64;
        for i in 0..n {
            let d = get_var(b, &mut pos)?;
            last = if i == 0 { d } else { last.checked_add(d)? };
            pages.push(u32::try_from(last).ok()?);
        }
        f(fid, &pages, start..pos);
    }
    Some(())
}

#[derive(Default)]
struct Postings {
    terms: HashMap<String, Vec<u8>>,
}

impl Postings {
    /// Adds one file's terms (`""` holds the pages that must always be verified).
    fn add_file(&mut self, fid: u32, by_term: &BTreeMap<String, Vec<u32>>) {
        for (t, pages) in by_term {
            let c = self.terms.entry(t.clone()).or_default();
            put_var(c, u64::from(fid));
            put_var(c, pages.len() as u64);
            let mut last = 0u32;
            for (i, p) in pages.iter().enumerate() {
                put_var(c, u64::from(if i == 0 { *p } else { p - last }));
                last = *p;
            }
        }
    }

    /// The chunks of `old` that belong to files in `keep`.
    fn keep(&mut self, old: &Index, keep: &HashSet<u32>) {
        for (t, bytes) in old.terms.iter().zip(&old.chunks) {
            let mut out = Vec::new();
            let _ = walk_chunks(bytes, |fid, _, r| {
                if keep.contains(&fid) {
                    out.extend_from_slice(&bytes[r]);
                }
            });
            if !out.is_empty() {
                self.terms.entry(t.clone()).or_default().extend_from_slice(&out);
            }
        }
    }

    fn encode(self) -> Vec<u8> {
        let sorted: BTreeMap<String, Vec<u8>> = self.terms.into_iter().collect();
        let mut payload = Vec::new();
        put_var(&mut payload, sorted.len() as u64);
        for (t, c) in &sorted {
            put_var(&mut payload, t.len() as u64);
            payload.extend_from_slice(t.as_bytes());
            put_var(&mut payload, c.len() as u64);
            payload.extend_from_slice(c);
        }
        let mut out = Vec::with_capacity(payload.len() + 44);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(blake3::hash(&payload).as_bytes());
        out.extend_from_slice(&payload);
        out
    }
}

/// A catalog's live generation in memory: the files table and the postings.
pub struct Index {
    pub table: Table,
    terms: Vec<String>,
    chunks: Vec<Vec<u8>>,
}

/// The index file of `generation` when its magic, version and checksum are right (whole file, header first).
fn checked_index(dir: &Path, generation: u32) -> Option<Vec<u8>> {
    let b = std::fs::read(index_path(dir, generation)).ok()?;
    let ok = b.len() >= 44 && &b[..8] == MAGIC && b[8..12] == VERSION.to_le_bytes() && blake3::hash(&b[44..]).as_bytes() == &b[12..44];
    ok.then_some(b)
}

fn load_table(dir: &Path, generation: u32) -> Option<Table> {
    let t: Table = serde_json::from_slice(&std::fs::read(table_path(dir, generation)).ok()?).ok()?;
    (t.version == VERSION && t.generation == generation).then_some(t)
}

impl Index {
    /// `None` when there is no live generation, or a file of it is missing, old or damaged.
    pub fn load(dir: &Path, meta: &Meta) -> Option<Index> {
        if meta.generation == 0 {
            return None;
        }
        let table = load_table(dir, meta.generation)?;
        let b = checked_index(dir, meta.generation)?;
        let payload = &b[44..];
        let mut pos = 0;
        let n = get_var(payload, &mut pos)? as usize;
        if n > payload.len() {
            return None;
        }
        let (mut terms, mut chunks) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for _ in 0..n {
            let l = get_var(payload, &mut pos)? as usize;
            let t = std::str::from_utf8(payload.get(pos..pos.checked_add(l)?)?).ok()?.to_string();
            pos += l;
            let l = get_var(payload, &mut pos)? as usize;
            let c = payload.get(pos..pos.checked_add(l)?)?.to_vec();
            pos += l;
            terms.push(t);
            chunks.push(c);
        }
        terms.windows(2).all(|w| w[0] < w[1]).then_some(Index { table, terms, chunks })
    }

    fn exact(&self, w: &str) -> Option<&Vec<u8>> {
        self.terms.binary_search_by(|t| t.as_str().cmp(w)).ok().map(|i| &self.chunks[i])
    }

    /// The (file, page) pairs listed under every term `want` accepts, and under the always-verify term.
    fn pages_where(&self, want: impl Fn(&str) -> bool) -> HashSet<u64> {
        let mut out = HashSet::new();
        let mut add = |b: &[u8]| {
            let _ = walk_chunks(b, |fid, pages, _| out.extend(pages.iter().map(|p| u64::from(fid) << 32 | u64::from(*p))));
        };
        if let Some(w) = self.exact("") {
            add(w);
        }
        for (t, c) in self.terms.iter().zip(&self.chunks) {
            if !t.is_empty() && want(t) {
                add(c);
            }
        }
        out
    }

    /// The pages that can hold a match of the query words (in the widest fold): file id to its pages, ascending.
    /// `None`: no words to look for (a query of marks only), every page is verified.
    ///
    /// A match is a run of words of the page: the first query word ends a page word, the last one starts one,
    /// the ones between are page words, and one word is inside a page word. Whole words only makes the verify
    /// stricter, so the same rule serves every option set.
    pub fn candidates(&self, words: &[String]) -> Option<BTreeMap<u32, Vec<u32>>> {
        if words.is_empty() {
            return None;
        }
        let last = words.len() - 1;
        let mut sets: Vec<HashSet<u64>> = words
            .iter()
            .enumerate()
            .map(|(i, w)| match (words.len(), i) {
                (1, _) => self.pages_where(|t| t.contains(w.as_str())),
                (_, 0) => self.pages_where(|t| t.ends_with(w.as_str())),
                (_, i) if i == last => self.pages_where(|t| t.starts_with(w.as_str())),
                _ => self.pages_where(|t| t == w),
            })
            .collect();
        sets.sort_by_key(HashSet::len);
        let mut acc = sets.remove(0);
        for s in &sets {
            acc.retain(|k| s.contains(k));
        }
        let mut out: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for k in acc {
            out.entry((k >> 32) as u32).or_default().push(k as u32);
        }
        out.values_mut().for_each(|v| v.sort_unstable());
        Some(out)
    }
}

/// The words a query looks for in an index (the widest fold; empty when it is only marks).
pub fn query_words(query: &str) -> Vec<String> {
    super::index_needle(query)
}

/// What a file's pages index: term to pages, and `""` for the pages whose words were not all listed.
fn file_terms(t: &FileText) -> BTreeMap<String, Vec<u32>> {
    let mut marks: HashMap<u32, Vec<&str>> = HashMap::new();
    for (p, title) in &t.bookmarks {
        marks.entry(*p).or_default().push(title);
    }
    let mut out: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (p, page) in t.pages.iter().enumerate() {
        let p = p as u32;
        let mut set = BTreeSet::new();
        let mut complete = PageChars::from_stored(&page.text).terms(&mut set);
        for (c, _) in page.comments.iter().chain(&page.fields) {
            complete &= PageChars::of_text(c).terms(&mut set);
        }
        for title in marks.get(&p).into_iter().flatten() {
            complete &= PageChars::of_text(title).terms(&mut set);
        }
        if !complete {
            out.entry(String::new()).or_default().push(p);
        }
        for term in set {
            out.entry(term).or_default().push(p);
        }
    }
    out
}

// ---- reading one file --------------------------------------------------------------------------------------

/// The `/ID` of a document as hex of its first string; empty when it has none. The second string is left out
/// on purpose: MuPDF writes a new one on every save, Inkwell's own snapshots included, so an index matched on it
/// would never be current. What changed is the fingerprint's job (`embed::fingerprint`).
pub fn pdf_id(doc: &PdfDocument) -> String {
    let first = doc
        .trailer()
        .ok()
        .and_then(|t| t.get_dict("ID").ok().flatten())
        .and_then(|a| a.get_array(0).ok().flatten())
        .and_then(|s| s.as_bytes().ok());
    first.map(|b| b.iter().map(|x| format!("{x:02x}")).collect()).unwrap_or_default()
}

/// The records of every page of `doc`, with the characters that count toward `MAX_TEXT`.
pub fn read_pages(doc: &PdfDocument, cancel: &AtomicBool) -> std::result::Result<FileText, Skip> {
    let n = (doc.page_count().map_err(|_| Skip::Unreadable)?.max(0) as usize).min(MAX_PAGES);
    let mut t = FileText { version: VERSION, id: pdf_id(doc), ..Default::default() };
    let mut used = 0usize;
    for p in 0..n {
        if cancel.load(Ordering::Relaxed) {
            return Err(Skip::Cancelled);
        }
        if used >= MAX_TEXT {
            t.cut = true;
            t.pages.push(StoredPage::default());
            continue;
        }
        let Ok(rec) = extract_record(doc, p as u32, true) else {
            t.pages.push(StoredPage::default());
            continue;
        };
        let page = StoredPage { text: rec.text.to_stored(), comments: rec.comments, fields: rec.fields };
        // What it takes in the blob, so the cap and what `decode` accepts are the same measure. A page that does
        // not fit is left out, however big: one page cannot take the file past the cap.
        let size = serde_json::to_vec(&page).map_or(usize::MAX, |b| b.len());
        if size > MAX_TEXT - used {
            used = MAX_TEXT;
            t.cut = true;
            t.pages.push(StoredPage::default());
            continue;
        }
        used += size;
        t.pages.push(page);
    }
    let mut room = MAX_TEXT / 4;
    t.bookmarks = bookmarks_of(doc)
        .into_iter()
        .take_while(|(_, title)| {
            room = room.saturating_sub(title.len() + 16);
            room > 0
        })
        .collect();
    Ok(t)
}

pub(super) fn bookmarks_of(doc: &PdfDocument) -> Vec<(u32, String)> {
    fn walk(items: &[mupdf::Outline], out: &mut Vec<(u32, String)>) {
        for o in items {
            if out.len() >= MAX_BOOKMARKS {
                return;
            }
            if let Some(d) = o.dest {
                out.push((d.loc.page_number, o.title.chars().take(1000).collect()));
            }
            walk(&o.down, out);
        }
    }
    let mut out = Vec::new();
    walk(&doc.outlines().unwrap_or_default(), &mut out);
    out
}

/// The hash of the first and last `SAMPLE` bytes of something `size` bytes long (a file, or the bytes of one).
fn sample_of<R: Read + std::io::Seek>(r: &mut R, size: u64) -> String {
    use std::io::SeekFrom;
    let mut h = blake3::Hasher::new();
    let mut b = Vec::new();
    let _ = (&mut *r).take(SAMPLE).read_to_end(&mut b);
    h.update(&b);
    if size > 2 * SAMPLE && r.seek(SeekFrom::Start(size - SAMPLE)).is_ok() {
        b.clear();
        let _ = (&mut *r).take(SAMPLE).read_to_end(&mut b);
        h.update(&b);
    }
    h.finalize().to_hex().to_string()
}

fn head_tail(path: &Path, size: u64) -> String {
    match std::fs::File::open(path) {
        Ok(mut f) => sample_of(&mut f, size),
        Err(_) => blake3::Hasher::new().finalize().to_hex().to_string(),
    }
}

// ---- build and refresh -------------------------------------------------------------------------------------

/// What a build or refresh did. `extracted` counts the files whose pages were read for text, which is what
/// an unchanged file never costs.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub files: u32,
    pub kept: u32,
    pub extracted: u32,
    pub from_embedded: u32,
    pub added: u32,
    pub changed: u32,
    pub removed: u32,
    pub damaged: u32,
    pub password: u32,
    pub cloud_only: u32,
    pub pages: u64,
    pub cut: u32,
    pub capped: bool,
    pub cancelled: bool,
}

struct Job {
    slot: usize,
    fid: u32,
    path: PathBuf,
    size: u64,
    mtime: u64,
}

struct Shared<'a> {
    dir: &'a Path,
    cancel: &'a AtomicBool,
    queue: Mutex<VecDeque<Job>>,
    done: Mutex<Vec<Option<FileRec>>>,
    postings: Mutex<Postings>,
    extracted: AtomicU32,
    embedded: AtomicU32,
    finished: AtomicU32,
    total: u32,
    progress: &'a (dyn Fn(u32, u32) + Sync),
    /// Files up to this size are read into memory before they are opened (`MAX_READ`).
    max_read: u64,
    /// Blobs written by this run (removed again when it is cancelled).
    written: Mutex<Vec<u32>>,
}

enum Probe {
    Text(FileText, bool),
    Bad(Status),
    Cancelled,
}

/// Indexes one file: the record for the table (`None` when cancelled).
fn index_file(sh: &Shared, job: &Job) -> Option<FileRec> {
    let mut rec = FileRec {
        fid: job.fid,
        path: job.path.to_string_lossy().into_owned(),
        size: job.size,
        mtime: job.mtime,
        head_tail: String::new(),
        pdf_id: String::new(),
        pages: 0,
        status: Status::Damaged,
        cut: false,
    };
    // Read whole, so no handle stays on the file while it is indexed (a Save-over, delete or rename of the PDF
    // must not wait for a build) and a long path is no problem; the sample is hashed from the same bytes.
    // shortcut: a file over `max_read` is opened by path (and so held while it is read), because a copy of it in
    // memory per thread is too much; upgrade by reading it in chunks into a memory-backed stream.
    let bytes = if job.size <= sh.max_read {
        match std::fs::read(&job.path) {
            Ok(b) => Some(b),
            Err(_) => {
                rec.head_tail = head_tail(&job.path, job.size);
                return Some(rec);
            }
        }
    } else {
        None
    };
    rec.head_tail = match &bytes {
        Some(b) => sample_of(&mut std::io::Cursor::new(b), job.size),
        None => head_tail(&job.path, job.size),
    };
    // A file that makes MuPDF panic is skipped like a damaged one.
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Probe {
        let opened = match &bytes {
            Some(b) => PdfDocument::from_bytes(b),
            None => PdfDocument::open(&*job.path),
        };
        let Ok(doc) = opened else { return Probe::Bad(Status::Damaged) };
        if doc.needs_password().unwrap_or(true) {
            return Probe::Bad(Status::Password);
        }
        let Ok(n) = doc.page_count() else { return Probe::Bad(Status::Damaged) };
        let id = pdf_id(&doc);
        if let Some(t) = embed::read(&doc, n.max(0) as u32, &id) {
            return Probe::Text(t, true);
        }
        match read_pages(&doc, sh.cancel) {
            Ok(t) => Probe::Text(t, false),
            Err(Skip::Cancelled) => Probe::Cancelled,
            Err(Skip::Unreadable) => Probe::Bad(Status::Damaged),
        }
    }))
    .unwrap_or(Probe::Bad(Status::Damaged));
    match read {
        Probe::Cancelled => return None,
        Probe::Bad(s) => rec.status = s,
        Probe::Text(t, from_embedded) => {
            rec.status = Status::Indexed;
            rec.pdf_id = t.id.clone();
            rec.pages = t.pages.len() as u32;
            rec.cut = t.cut;
            if from_embedded {
                sh.embedded.fetch_add(1, Ordering::Relaxed);
            } else {
                sh.extracted.fetch_add(1, Ordering::Relaxed);
            }
            let written = t.encode().map(|blob| write_atomic(&text_path(sh.dir, job.fid), &blob));
            if !matches!(written, Some(Ok(()))) {
                rec.status = Status::Damaged;
                rec.pages = 0;
                return Some(rec);
            }
            sh.written.lock().unwrap().push(job.fid);
            sh.postings.lock().unwrap().add_file(job.fid, &file_terms(&t));
        }
    }
    Some(rec)
}

fn worker(sh: &Shared) {
    be_polite();
    // This thread's MuPDF context exists before any document it will drop.
    let _ = mupdf::Context::get();
    loop {
        if sh.cancel.load(Ordering::Relaxed) {
            return;
        }
        let Some(job) = sh.queue.lock().unwrap().pop_front() else { return };
        #[cfg(test)]
        if tests::worker_dies_on(&job.path) {
            panic!("test: the worker dies on this file");
        }
        let Some(rec) = index_file(sh, &job) else { return };
        sh.done.lock().unwrap()[job.slot] = Some(rec);
        let n = sh.finished.fetch_add(1, Ordering::SeqCst) + 1;
        (sh.progress)(n, sh.total);
    }
}

/// Builds the catalog's index from scratch (`full`) or brings it up to date: a file whose size and
/// modified time are unchanged is kept without being opened, a file that moved is kept by its size,
/// time and sample, the rest is read again. `progress(done, total)` is called per file, from the build's threads.
/// Cancelling leaves the live generation as it was.
pub fn build(dir: &Path, full: bool, cancel: &AtomicBool, progress: &(dyn Fn(u32, u32) + Sync)) -> Result<Summary> {
    build_with(dir, full, cancel, progress, MAX_READ)
}

/// [`build`] with the size up to which a file is read into memory given (tests use a small one).
pub(super) fn build_with(dir: &Path, full: bool, cancel: &AtomicBool, progress: &(dyn Fn(u32, u32) + Sync), max_read: u64) -> Result<Summary> {
    let mut meta = load_meta_or_unusable(dir)?;
    let folder = PathBuf::from(&meta.folder);
    if !folder.is_dir() {
        return Err(EngineError::Io(format!("{} is not a folder", folder.display())));
    }
    let mut sum = Summary::default();
    let listing = list_pdfs(&folder, meta.recursive, cancel);
    sum.cloud_only = listing.cloud_only;
    sum.capped = listing.capped;
    if listing.cancelled || cancel.load(Ordering::Relaxed) {
        sum.cancelled = true;
        return Ok(sum);
    }
    // A catalog that is damaged or from another version is read again from the files, not trusted.
    let old = if full { None } else { Index::load(dir, &meta) };
    let old_recs: HashMap<&str, &FileRec> = old.iter().flat_map(|i| i.table.files.iter()).map(|r| (r.path.as_str(), r)).collect();
    let listed: HashSet<String> = listing.files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    // Old rows whose path is gone: a moved file may match one of them. (Only an indexed row is kept: a file that
    // failed is read again, whatever its size and time say: it may have been locked, or half copied.)
    let mut gone: HashMap<(u64, u64), Vec<&FileRec>> = HashMap::new();
    for r in old.iter().flat_map(|i| i.table.files.iter()).filter(|r| r.status == Status::Indexed && !listed.contains(&r.path)) {
        gone.entry((r.size, r.mtime)).or_default().push(r);
    }
    let mut taken: HashSet<u32> = HashSet::new();
    // File ids are never reused, not even by a full rebuild: the live generation's blobs stay valid until the commit.
    let blobs = std::fs::read_dir(text_dir(dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().split('.').next().and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);
    let prev = load_table(dir, meta.generation).map_or(0, |t| t.next_fid);
    let mut next_fid = prev.max(blobs + 1).max(1);
    let total = listing.files.len() as u32;
    let mut slots: Vec<Option<FileRec>> = Vec::with_capacity(listing.files.len());
    let mut jobs: VecDeque<Job> = VecDeque::new();
    let mut kept_fids: HashSet<u32> = HashSet::new();
    // Every file that is to be read, to find the ones no worker came back from.
    let mut planned: Vec<(usize, u32, PathBuf, u64, u64)> = Vec::new();
    for (slot, path) in listing.files.iter().enumerate() {
        let p = path.to_string_lossy().into_owned();
        let Ok(m) = std::fs::metadata(path) else {
            slots.push(None);
            continue;
        };
        let (size, mtime) = (m.len(), stamp(&m));
        let same = old_recs.get(p.as_str()).filter(|r| r.status == Status::Indexed && r.size == size && r.mtime == mtime);
        let moved = || {
            let c = gone.get(&(size, mtime))?;
            let h = head_tail(path, size);
            c.iter().find(|r| !taken.contains(&r.fid) && r.head_tail == h).copied()
        };
        let found = same.copied().or_else(|| if old_recs.contains_key(p.as_str()) { None } else { moved() });
        if let Some(r) = found {
            taken.insert(r.fid);
            kept_fids.insert(r.fid);
            sum.kept += 1;
            slots.push(Some(FileRec { path: p, ..r.clone() }));
        } else {
            match old_recs.get(p.as_str()) {
                // A failure tried again is neither new nor changed.
                Some(r) if r.status != Status::Indexed && r.size == size && r.mtime == mtime => {}
                Some(_) => sum.changed += 1,
                None => sum.added += 1,
            }
            planned.push((slot, next_fid, path.clone(), size, mtime));
            jobs.push_back(Job { slot, fid: next_fid, path: path.clone(), size, mtime });
            next_fid += 1;
            slots.push(None);
        }
    }
    sum.removed = old_recs.values().filter(|r| !listed.contains(&r.path) && !taken.contains(&r.fid)).count() as u32;
    let to_read = jobs.len() as u32;
    // Kept files are done at once.
    progress(total - to_read, total);
    let sh = Shared {
        dir,
        cancel,
        queue: Mutex::new(jobs),
        done: Mutex::new(slots),
        postings: Mutex::new(Postings::default()),
        extracted: AtomicU32::new(0),
        embedded: AtomicU32::new(0),
        finished: AtomicU32::new(total - to_read),
        total,
        progress,
        max_read,
        written: Mutex::new(Vec::new()),
    };
    std::fs::create_dir_all(text_dir(dir)).map_err(io)?;
    std::thread::scope(|s| {
        for k in 0..PARALLEL.min(to_read as usize) {
            let sh = &sh;
            let _ = std::thread::Builder::new().name(format!("inkwell-catalog-{k}")).spawn_scoped(s, move || {
                // A panic in one worker leaves its file without a row; it is given one (damaged) below.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker(sh)));
            });
        }
    });
    sum.extracted = sh.extracted.load(Ordering::Relaxed);
    sum.from_embedded = sh.embedded.load(Ordering::Relaxed);
    let mut rows = sh.done.into_inner().unwrap();
    let written = sh.written.into_inner().unwrap();
    // A cancelled run commits nothing and leaves no new blobs behind. (A file that vanished while listing has no
    // row; it is simply not in the new table.)
    if cancel.load(Ordering::Relaxed) {
        for fid in written {
            let _ = std::fs::remove_file(text_path(dir, fid));
        }
        sum.cancelled = true;
        return Ok(sum);
    }
    // A file that was to be read and has no row (its worker died) is counted as damaged, not dropped silently.
    for (slot, fid, path, size, mtime) in planned {
        if rows[slot].is_none() {
            rows[slot] = Some(FileRec {
                fid,
                path: path.to_string_lossy().into_owned(),
                size,
                mtime,
                head_tail: head_tail(&path, size),
                pdf_id: String::new(),
                pages: 0,
                status: Status::Damaged,
                cut: false,
            });
        }
    }
    let files: Vec<FileRec> = rows.into_iter().flatten().collect();
    let mut postings = sh.postings.into_inner().unwrap();
    if let Some(o) = &old {
        postings.keep(o, &kept_fids);
    }
    let generation = meta.generation + 1;
    let table = Table { version: VERSION, generation, next_fid, files };
    let index = postings.encode();
    write_durable(&table_path(dir, generation), &serde_json::to_vec(&table).map_err(io)?)?;
    write_durable(&index_path(dir, generation), &index)?;
    sum.files = table.files.len() as u32;
    sum.damaged = table.files.iter().filter(|r| r.status == Status::Damaged).count() as u32;
    sum.password = table.files.iter().filter(|r| r.status == Status::Password).count() as u32;
    sum.pages = table.files.iter().map(|r| u64::from(r.pages)).sum();
    sum.cut = table.files.iter().filter(|r| r.cut).count() as u32;
    meta.generation = generation;
    meta.built = now_ms();
    meta.files = sum.files;
    meta.indexed = sum.files - sum.damaged - sum.password;
    meta.pages = sum.pages;
    meta.damaged = sum.damaged;
    meta.password = sum.password;
    meta.cloud_only = sum.cloud_only;
    meta.cut = sum.cut;
    meta.capped = sum.capped;
    meta.index_len = index.len() as u64;
    save_meta(dir, &meta)?;
    clean(dir, &table);
    Ok(sum)
}

/// Removes what the live generation does not use: older generations, blobs of files that are gone, temp files.
fn clean(dir: &Path, table: &Table) {
    let live: HashSet<String> = table.files.iter().filter(|r| r.status == Status::Indexed).map(|r| format!("{}.zst", r.fid)).collect();
    for e in std::fs::read_dir(text_dir(dir)).into_iter().flatten().flatten() {
        if !live.contains(&*e.file_name().to_string_lossy()) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let keep = [table_path(dir, table.generation), index_path(dir, table.generation), meta_path(dir)];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with("table-") || name.starts_with("index-") || name.ends_with(".tmp");
        if ours && !keep.contains(&p) {
            let _ = std::fs::remove_file(p);
        }
    }
}

// ---- forgetting a file -------------------------------------------------------------------------------------

/// `p` as the file system names it, for comparing a saved file with the paths of a listing.
fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Takes the file at `path` out of the catalog in `dir`: its row, its stored text and its postings, as a new
/// generation. Used when the file was replaced by a version with content taken out of it (a saved redaction):
/// the catalog must not keep, or show, what was removed. The file is listed as new until the next refresh reads
/// it again. Returns how many rows went. Nothing runs against a build of the same catalog: the caller holds it.
pub fn forget(dir: &Path, path: &Path) -> Result<u32> {
    // Nothing here says which blob is whose (no readable entry or table), and the next build reads every file
    // again anyway: none is kept, and no index with words in it.
    let wipe = || {
        let _ = std::fs::remove_dir_all(text_dir(dir));
        let _ = std::fs::create_dir_all(text_dir(dir));
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.file_name().to_string_lossy().starts_with("index-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    };
    let Ok(meta) = load_meta(dir) else {
        wipe();
        return Ok(0);
    };
    let Some(table) = (meta.generation > 0).then(|| load_table(dir, meta.generation)).flatten() else {
        wipe();
        return Ok(0);
    };
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let want = canon(path);
    // The name is compared first: only a row that could be the file is looked up on disk.
    let gone: HashSet<u32> = table
        .files
        .iter()
        .filter(|r| {
            let p = Path::new(&r.path);
            p.file_name().map(|n| n.to_string_lossy().to_lowercase()) == name && (canon(p) == want || p == path)
        })
        .map(|r| r.fid)
        .collect();
    if gone.is_empty() {
        return Ok(0);
    }
    // The text goes first: whatever happens next, it is not on disk.
    for fid in &gone {
        let _ = std::fs::remove_file(text_path(dir, *fid));
    }
    // Fails closed: when the postings cannot be rewritten (or are not trusted, so cannot be filtered), the live
    // index is deleted, which makes the catalog "needs a rebuild" rather than keep the words of the old file.
    let rewrite = || -> Result<()> {
        let old = Index::load(dir, &meta).ok_or_else(|| EngineError::Invalid("the index of this catalog is damaged".into()))?;
        let keep: HashSet<u32> = table.files.iter().map(|r| r.fid).filter(|f| !gone.contains(f)).collect();
        let mut postings = Postings::default();
        postings.keep(&old, &keep);
        let generation = meta.generation + 1;
        let files: Vec<FileRec> = table.files.iter().filter(|r| !gone.contains(&r.fid)).cloned().collect();
        let table = Table { version: VERSION, generation, next_fid: table.next_fid, files };
        let index = postings.encode();
        write_durable(&table_path(dir, generation), &serde_json::to_vec(&table).map_err(io)?)?;
        write_durable(&index_path(dir, generation), &index)?;
        let count = |s: Status| table.files.iter().filter(|r| r.status == s).count() as u32;
        let mut meta = meta.clone();
        meta.generation = generation;
        meta.files = table.files.len() as u32;
        meta.indexed = count(Status::Indexed);
        meta.damaged = count(Status::Damaged);
        meta.password = count(Status::Password);
        meta.pages = table.files.iter().map(|r| u64::from(r.pages)).sum();
        meta.cut = table.files.iter().filter(|r| r.cut).count() as u32;
        meta.index_len = index.len() as u64;
        save_meta(dir, &meta)?;
        clean(dir, &table);
        Ok(())
    };
    if let Err(e) = rewrite() {
        let _ = std::fs::remove_file(index_path(dir, meta.generation));
        return Err(e);
    }
    Ok(gone.len() as u32)
}

// ---- saved while the catalog was busy ----------------------------------------------------------------------

/// Paths saved (with redactions) while the catalog was held by a build: `forget` runs for them when it is free.
/// On disk, so a quit in between does not leave the old text behind.
fn pending_path(dir: &Path) -> PathBuf {
    dir.join("forget-pending.json")
}

/// One writer at a time: the list is read, changed and written back.
static PENDING: Mutex<()> = Mutex::new(());
/// More than this and the oldest are dropped from the list (a catalog that long busy is rebuilt anyway).
const MAX_PENDING: usize = 1000;

/// The paths waiting for catalog `dir`.
pub fn pending(dir: &Path) -> Vec<PathBuf> {
    let _g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    read_pending(dir)
}

fn read_pending(dir: &Path) -> Vec<PathBuf> {
    let v: Vec<String> = std::fs::read(pending_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    v.into_iter().map(PathBuf::from).collect()
}

fn write_pending(dir: &Path, list: &[PathBuf]) -> Result<()> {
    if list.is_empty() {
        return match std::fs::remove_file(pending_path(dir)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(io(e)),
            _ => Ok(()),
        };
    }
    let names: Vec<String> = list.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    write_durable(&pending_path(dir), &serde_json::to_vec(&names).map_err(io)?)
}

/// Adds `path` to the list of catalog `dir`.
pub fn defer_forget(dir: &Path, path: &Path) -> Result<()> {
    let _g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = read_pending(dir);
    if !list.iter().any(|p| p == path) {
        list.push(path.to_path_buf());
    }
    let over = list.len().saturating_sub(MAX_PENDING);
    list.drain(..over);
    write_pending(dir, &list)
}

/// Takes `done` off the list (what was added meanwhile stays).
pub fn settle_pending(dir: &Path, done: &[PathBuf]) -> Result<()> {
    let _g = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = read_pending(dir);
    list.retain(|p| !done.contains(p));
    write_pending(dir, &list)
}

// ---- is it out of date -------------------------------------------------------------------------------------

/// What a refresh would change, from a listing and the files' sizes and times (nothing is opened).
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stale {
    pub added: u32,
    pub changed: u32,
    pub removed: u32,
    /// Never built, or the live generation is missing or from another version: it must be built.
    pub unusable: bool,
}

impl Stale {
    pub fn any(&self) -> bool {
        self.unusable || self.added + self.changed + self.removed > 0
    }
}

pub fn check(dir: &Path, cancel: &AtomicBool) -> Result<Stale> {
    let meta = load_meta_or_unusable(dir)?;
    // The live generation must be all there and pass its checksum, or a refresh would trust a damaged index.
    let table = load_table(dir, meta.generation).filter(|_| meta.generation > 0 && checked_index(dir, meta.generation).is_some());
    let Some(table) = table else { return Ok(Stale { unusable: true, ..Default::default() }) };
    let folder = PathBuf::from(&meta.folder);
    if !folder.is_dir() {
        return Err(EngineError::Io(format!("{} is not a folder", folder.display())));
    }
    let listing = list_pdfs(&folder, meta.recursive, cancel);
    let known: HashMap<&str, &FileRec> = table.files.iter().map(|r| (r.path.as_str(), r)).collect();
    let mut s = Stale::default();
    let mut seen = 0u32;
    for p in &listing.files {
        match (known.get(&*p.to_string_lossy()), std::fs::metadata(p)) {
            (Some(r), Ok(m)) => {
                seen += 1;
                if r.size != m.len() || r.mtime != stamp(&m) {
                    s.changed += 1;
                }
            }
            (None, _) => s.added += 1,
            _ => {}
        }
    }
    s.removed = (table.files.len() as u32).saturating_sub(seen);
    Ok(s)
}

/// The size and time of indexing a folder, from a listing and the files' sizes (nothing is opened).
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Estimate {
    pub files: u32,
    pub bytes: u64,
    pub secs: u32,
    pub index_bytes: u64,
    pub capped: bool,
}

/// Rough rates: text extraction and layout run at about 25 MB/s on the two threads, plus 20 ms to open a file;
/// the index and the stored text come to about 3 % of the PDFs plus 2 KB a file. A guess to set expectations.
pub fn estimate(folder: &Path, recursive: bool, cancel: &AtomicBool) -> Estimate {
    let l = list_pdfs(folder, recursive, cancel);
    let bytes: u64 = l.files.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    let files = l.files.len() as u64;
    Estimate {
        files: files as u32,
        bytes,
        secs: (files as f64 * 0.02 / PARALLEL as f64 + bytes as f64 / (25.0 * 1_048_576.0)).ceil() as u32,
        index_bytes: bytes / 100 * 3 + files * 2048,
        capped: l.capped,
    }
}

// ---- search ------------------------------------------------------------------------------------------------

fn under(root: &Path, p: &Path) -> bool {
    p.starts_with(root) && !p.components().any(|c| matches!(c, Component::ParentDir))
}

fn read_text(dir: &Path, fid: u32) -> Option<FileText> {
    FileText::decode(&std::fs::read(text_path(dir, fid)).ok()?)
}

/// Searches the catalog. Events are those of a folder search (`FilesEvent`), the result files are recorded in
/// `searches` for `window` so `search_file_token`, cancel and the rail work as they do there. Returns the
/// search id. A file's hits are those of the matcher on its stored page records, for the pages the index
/// cannot rule out.
pub fn search(dir: &Path, query: &str, opts: SearchOpts, searches: &FolderSearches, window: &str, sink: &dyn Fn(FilesEvent)) -> Result<u64> {
    let (id, cancel) = searches.begin(window);
    sink(FilesEvent::Started { search: id });
    let end = |matched, hits, skipped, files, cloud_only, cancelled, capped, stale| {
        searches.finish(id);
        sink(FilesEvent::Done { search: id, files, matched, hits, skipped, cloud_only, cancelled, capped, stale });
    };
    let loaded = load_meta(dir).ok().and_then(|m| Index::load(dir, &m).map(|i| (m, i)));
    let Some((meta, idx)) = loaded else {
        end(0, 0, 0, 0, 0, false, false, 0);
        return Err(EngineError::Invalid("this catalog is damaged or was never built. Build it again.".into()));
    };
    let total = idx.table.files.len() as u32;
    let skipped = idx.table.files.iter().filter(|r| r.status != Status::Indexed).count() as u32;
    let Some(q) = Query::new(query, opts) else {
        // Nothing to look for: nothing is read, so nothing is skipped (as in a folder search).
        end(0, 0, 0, total, meta.cloud_only, false, meta.capped, 0);
        return Ok(id);
    };
    let cands = idx.candidates(&query_words(query));
    let root = PathBuf::from(&meta.folder);
    let (mut matched, mut hits_total, mut listed, mut unreadable, mut stale) = (0u32, 0u32, 0usize, 0u32, 0u32);
    let mut cancelled = false;
    for (i, rec) in idx.table.files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            break;
        }
        if rec.status == Status::Indexed {
            let pages: Option<Vec<u32>> = match &cands {
                None => Some((0..rec.pages).collect()),
                Some(c) => c.get(&rec.fid).cloned(),
            };
            // The stored text is what the file said when it was indexed. A file that is not the one that was read
            // (saved, redacted, replaced) is left out and counted, never answered from old text; a path that is not
            // under the folder is not looked at.
            let on_disk = || {
                let p = Path::new(&rec.path);
                under(&root, p).then(|| std::fs::metadata(p).ok()).flatten().is_some_and(|m| m.len() == rec.size && stamp(&m) == rec.mtime)
            };
            if let Some(pages) = pages {
                match if on_disk() { Some(read_text(dir, rec.fid)) } else { None } {
                    None => stale += 1,
                    Some(None) => unreadable += 1,
                    Some(Some(text)) => {
                        let want_marks = q.opts().bookmarks;
                        let room = TOTAL.saturating_sub(listed);
                        let page_set: HashSet<u32> = pages.iter().copied().collect();
                        let marks = want_marks.then(|| {
                            let mut v = Vec::new();
                            for (p, title) in &text.bookmarks {
                                if cands.is_none() || page_set.contains(p) {
                                    v.extend(bookmark_hits_of(*p, title, &q));
                                }
                            }
                            v
                        });
                        let r = collect(
                            rec.pages,
                            pages.iter().copied(),
                            PER_FILE.min(room),
                            &cancel,
                            |p| text.pages.get(p as usize).map(|s| s.rec().find(&q)).unwrap_or_default(),
                            marks,
                        );
                        match r {
                            Err(_) => {
                                cancelled = true;
                                break;
                            }
                            Ok(r) if r.hits > 0 => {
                                let p = PathBuf::from(&rec.path);
                                // The index is a file on disk: a path that is not under the folder is not offered.
                                if under(&root, &p) {
                                    listed += r.items.len();
                                    matched += 1;
                                    hits_total += r.hits;
                                    searches.record(window, id, i as u32, p.clone());
                                    sink(FilesEvent::File {
                                        search: id,
                                        index: i as u32,
                                        path: rec.path.clone(),
                                        name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                        page_count: r.page_count,
                                        hits: r.hits,
                                        pages: r.pages,
                                        truncated: r.truncated || r.items.len() < r.hits as usize,
                                        items: r.items,
                                    });
                                }
                            }
                            Ok(_) => {}
                        }
                    }
                }
            }
        }
        sink(FilesEvent::Progress { search: id, scanned: i as u32 + 1, total });
    }
    end(matched, hits_total, skipped + unreadable, total, meta.cloud_only, cancelled, meta.capped, stale);
    Ok(id)
}
