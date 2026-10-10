//! Editing: one authoritative document per file, a serial lane, an undo history, and saving.
//!
//! Follows `docs/engine-rules.md`. Every mutation runs under the entry's `editor` lock against
//! the authoritative document, never against a viewer replica. After each step the document is
//! written to memory as an incremental update (the previous snapshot plus the objects that
//! changed), and only then is the generation bumped; replicas reopen from that snapshot.
//!
//! Because a step only ever appends to the snapshot it was made from, the history is one buffer
//! and the length of each step: undo and redo reopen a shorter or longer prefix of it, and each
//! step costs the size of the objects it changed, not a copy of the file.
//!
//! The API other parts of the app (the UI commands, the assistant's edit tools) call:
//! [`Engine::apply`], [`Engine::undo`], [`Engine::redo`], [`Engine::edit_state`],
//! [`Engine::save`], [`Engine::extract`], [`Engine::split`] and [`Engine::merge`].

use super::{DocEntry, DocId, Engine, EngineError, Result, Snap, authenticate, check_pdf};
use mupdf::pdf::{PdfDocument, PdfGraftMap, PdfObject, PdfWriteOptions};
use mupdf::{Document, Size};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One edit, and one undo step. Page numbers are 0-based indices into the current generation.
///
/// Serialised for the UI and tools as `{"op": "rotate", "pages": [0, 2], "quarterTurns": 1}`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EditOp {
    /// Turns each page by `quarter_turns` x 90 degrees, clockwise when positive. Changes only
    /// the page's `/Rotate`, never its content.
    Rotate { pages: Vec<u32>, quarter_turns: i32 },
    /// Removes the pages. At least one page must remain.
    Delete { pages: Vec<u32> },
    /// Moves the pages, keeping their order, to the gap before page `to` (counted before the move;
    /// `to == page_count` is the end). This is the drop bar of a drag between thumbnails.
    Move { pages: Vec<u32>, to: u32 },
    /// Inserts an empty page at index `at` (`0..=page_count`) with the page box of page `like`.
    InsertBlank { at: u32, like: u32 },
    /// Inserts pages of another PDF at index `at`: `pages` of it (in its order), or all of them.
    InsertFile { at: u32, path: String, pages: Option<Vec<u32>>, password: Option<String> },
}

impl EditOp {
    /// What the step is called in the undo menu and the history, e.g. "Rotate pp. 2-3 by 90°".
    pub fn label(&self) -> String {
        match self {
            EditOp::Rotate { pages, quarter_turns } => format!("Rotate {} by {}°", page_ranges(pages), *quarter_turns as i64 * 90),
            EditOp::Delete { pages } => format!("Delete {}", page_ranges(pages)),
            EditOp::Move { pages, to } => format!("Move {} to position {}", page_ranges(pages), to + 1),
            EditOp::InsertBlank { at, .. } => format!("Insert blank page as p. {}", at + 1),
            EditOp::InsertFile { path, .. } => format!(
                "Insert pages from {}",
                Path::new(path).file_name().map_or_else(|| path.clone(), |f| f.to_string_lossy().into_owned())
            ),
        }
    }

    /// The pages (0-based) the step touched: in the document after it, but for a delete the
    /// pages it removed (numbered before it).
    fn touched(&self, count_before: usize, count_after: usize) -> Vec<u32> {
        let set = |p: &[u32]| p.iter().copied().collect::<BTreeSet<u32>>().into_iter().collect::<Vec<_>>();
        match self {
            EditOp::Rotate { pages, .. } | EditOp::Delete { pages } => set(pages),
            EditOp::Move { pages, to } => {
                let pages = set(pages);
                let first = to - pages.iter().filter(|&&p| p < *to).count() as u32;
                (first..first + pages.len() as u32).collect()
            }
            EditOp::InsertBlank { at, .. } | EditOp::InsertFile { at, .. } => {
                (*at..*at + count_after.saturating_sub(count_before) as u32).collect()
            }
        }
    }
}

/// "p. 4", "pp. 2-3, 7" (1-based).
fn page_ranges(pages: &[u32]) -> String {
    let set: BTreeSet<u32> = pages.iter().copied().collect();
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for p in set {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == p => *end = p,
            _ => runs.push((p, p)),
        }
    }
    let list = runs
        .iter()
        .map(|&(a, b)| if a == b { format!("{}", a + 1) } else { format!("{}-{}", a + 1, b + 1) })
        .collect::<Vec<_>>()
        .join(", ");
    if pages.len() == 1 { format!("p. {list}") } else { format!("pp. {list}") }
}

/// Who made an edit. All steps of one assistant turn form one group: undone and redone as a unit.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EditSource {
    #[default]
    User,
    Assistant { turn: String },
}

/// One entry of [`Engine::history`].
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    /// Pass to [`Engine::restore`]. 0 is the file as opened.
    pub index: usize,
    pub label: String,
    pub source: EditSource,
    /// Unix time in milliseconds.
    pub at_ms: u64,
    /// See [`EditOp::touched`]; empty for entry 0.
    pub pages: Vec<u32>,
    /// The document currently shows this entry.
    pub current: bool,
    /// This entry is what the file on disk holds.
    pub saved: bool,
}

/// The document as the UI should show it after a call.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditState {
    /// The generation every tile, text and list request must carry from now on.
    pub rev: u32,
    pub page_count: u32,
    /// Every page's size in points (`/Rotate` applied).
    pub sizes: Vec<[f32; 2]>,
    /// Where the document lives (changes on Save as).
    pub path: String,
    /// Content differs from the file on disk.
    pub dirty: bool,
    /// Label of the step Undo would revert / Redo would reapply.
    pub undo: Option<String>,
    pub redo: Option<String>,
    /// A redaction is part of what the document shows: every save is a full rewrite.
    pub redacted: bool,
}

/// Files written by [`Engine::split`].
#[derive(Serialize, Clone, Debug)]
pub struct SplitResult {
    pub files: Vec<String>,
}

/// The authoritative document, reading its snapshot in place.
struct AuthDoc {
    doc: PdfDocument,
    /// Declared after `doc` so the document is dropped first.
    _bytes: Arc<Vec<u8>>,
}

// SAFETY: only used under the entry's `editor` lock, through the locking thread's own context
// (the same contract as the replicas).
unsafe impl Send for AuthDoc {}

#[derive(Clone)]
struct Step {
    /// The whole file as of this step: the history can render any page before and after it.
    snap: Snap,
    label: String,
    source: EditSource,
    at_ms: u64,
    pages: Vec<u32>,
    /// Removed content (deleted pages): the bytes still carry it, so a save must rewrite the file.
    drops: bool,
    /// A redaction step: its snapshot is a full rewrite, and so is every save that includes it.
    redacts: bool,
    /// The snapshot is the file exactly as a save must write it (the redaction engine's verified
    /// bytes): saving this step writes them as they are, not a rewrite of them.
    exact: bool,
    sizes: Arc<Vec<[f32; 2]>>,
}

/// The edit lane's state for one file.
pub struct Editor {
    doc: AuthDoc,
    /// `steps[0]` is the file as opened.
    steps: Vec<Step>,
    cur: usize,
    /// The step whose content is in the file on disk.
    saved: Option<usize>,
}

type RedactionHook = Arc<dyn Fn(DocId) + Send + Sync>;
static REDACTION_HOOK: parking_lot::Mutex<Option<RedactionHook>> = parking_lot::Mutex::new(None);

/// Called (outside the edit lane) with the document after each redaction lands, so a version
/// store can purge what it kept from before it.
pub fn set_redaction_hook(hook: impl Fn(DocId) + Send + Sync + 'static) {
    *REDACTION_HOOK.lock() = Some(Arc::new(hook));
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn io(e: std::io::Error) -> EngineError {
    EngineError::Io(e.to_string())
}

fn invalid(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

fn open_snap(s: &Snap, entry: &DocEntry) -> Result<AuthDoc> {
    let _ = mupdf::Context::get();
    // SAFETY: snapshots are never mutated, and AuthDoc keeps the bytes alive past the document.
    let mut d = unsafe { Document::from_shared_bytes(&s.bytes[..s.len], "application/pdf")? };
    authenticate(&mut d, entry)?;
    Ok(AuthDoc { doc: PdfDocument::try_from(d)?, _bytes: s.bytes.clone() })
}

fn write_doc(doc: &PdfDocument, opts: PdfWriteOptions, capacity: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(capacity);
    doc.write_to_with_options(&mut out, opts)?;
    Ok(out)
}

/// A complete file without anything unreferenced or duplicated (deleted pages and redacted
/// content are really gone).
fn rewrite_opts() -> PdfWriteOptions {
    let mut o = PdfWriteOptions::default();
    o.set_garbage_level(3).set_compress(true);
    o
}

fn page_sizes(doc: &PdfDocument) -> Result<Vec<[f32; 2]>> {
    let n = doc.page_count()?.max(0);
    (0..n).map(|p| page_size(doc, p as u32)).collect()
}

fn page_size(doc: &PdfDocument, p: u32) -> Result<[f32; 2]> {
    let b = doc.load_page(p as i32)?.bounds()?;
    Ok([b.width(), b.height()])
}

/// Sorted, unique, in range, and not empty.
fn check_pages(pages: &[u32], count: usize) -> Result<Vec<u32>> {
    let set: BTreeSet<u32> = pages.iter().copied().collect();
    if set.is_empty() {
        return Err(invalid("no pages selected"));
    }
    if let Some(&p) = set.iter().find(|&&p| p as usize >= count) {
        return Err(EngineError::BadPage(p));
    }
    Ok(set.into_iter().collect())
}

impl Editor {
    fn open(entry: &DocEntry) -> Result<Editor> {
        let bytes = std::fs::read(entry.path()).map_err(io)?;
        let mut snap = Snap { len: bytes.len(), bytes: Arc::new(bytes) };
        let mut doc = open_snap(&snap, entry)?;
        // A file MuPDF had to repair takes no incremental updates: start from a clean copy of it.
        if !doc.doc.can_be_saved_incrementally() {
            let out = write_doc(&doc.doc, PdfWriteOptions::default(), snap.len)?;
            snap = Snap { len: out.len(), bytes: Arc::new(out) };
            doc = open_snap(&snap, entry)?;
        }
        let sizes = Arc::new(page_sizes(&doc.doc)?);
        let base = Step { snap, label: "Opened".into(), source: EditSource::User, at_ms: now_ms(), pages: vec![], drops: false, redacts: false, exact: false, sizes };
        Ok(Editor { doc, steps: vec![base], cur: 0, saved: Some(0) })
    }

    fn step(&self) -> &Step {
        &self.steps[self.cur]
    }

    /// The current step's file bytes.
    fn bytes(&self) -> &[u8] {
        let s = &self.step().snap;
        &s.bytes[..s.len]
    }

    /// The edited document as an incremental update of the current step.
    fn snapshot(&self) -> Result<Snap> {
        let mut opts = PdfWriteOptions::default();
        opts.set_incremental(true);
        let out = write_doc(&self.doc.doc, opts, self.step().snap.len + (64 << 10))?;
        Ok(Snap { len: out.len(), bytes: Arc::new(out) })
    }

    /// Records `step` after the current one, dropping anything that could have been redone.
    fn push(&mut self, mut step: Step) {
        self.steps.truncate(self.cur + 1);
        if self.saved.is_some_and(|s| s > self.cur) {
            self.saved = None;
        }
        // The new snapshot extends the current one, which every step sharing its buffer is a prefix
        // of: they can all read the new buffer, and the old one is freed.
        let old = self.step().snap.bytes.clone();
        if step.snap.bytes.starts_with(self.bytes()) {
            for s in &mut self.steps {
                if Arc::ptr_eq(&s.snap.bytes, &old) {
                    s.snap.bytes = step.snap.bytes.clone();
                }
            }
        } else {
            // Not expected from MuPDF; the history stays correct, it just costs a full copy.
            step.snap.len = step.snap.bytes.len();
        }
        self.steps.push(step);
        self.cur += 1;
    }

    /// Steps `i` and `i - 1` belong to the same assistant turn.
    fn joined(&self, i: usize) -> bool {
        i > 1 && matches!(&self.steps[i].source, EditSource::Assistant { .. }) && self.steps[i].source == self.steps[i - 1].source
    }

    /// Where Undo goes: before the current step, or before its whole assistant turn.
    fn undo_target(&self) -> Option<usize> {
        let mut i = self.cur.checked_sub(1)?;
        while self.joined(i + 1) {
            i -= 1;
        }
        Some(i)
    }

    /// Where Redo goes: the next step, or the end of its assistant turn.
    fn redo_target(&self) -> Option<usize> {
        let mut i = self.cur + 1;
        if i >= self.steps.len() {
            return None;
        }
        while i + 1 < self.steps.len() && self.joined(i + 1) {
            i += 1;
        }
        Some(i)
    }

    /// The label of the steps from `a` to `b` (inclusive).
    fn label(&self, a: usize, b: usize) -> String {
        let l = &self.steps[a].label;
        if a == b { l.clone() } else { format!("{l} and {} more", b - a) }
    }

    fn state(&self, entry: &DocEntry) -> EditState {
        EditState {
            dirty: self.saved != Some(self.cur),
            undo: self.undo_target().map(|t| self.label(t + 1, self.cur)),
            redo: self.redo_target().map(|t| self.label(self.cur + 1, t)),
            redacted: self.steps[..=self.cur].iter().any(|s| s.redacts),
            ..clean_state(entry)
        }
    }
}

fn clean_state(entry: &DocEntry) -> EditState {
    let l = entry.layout.read();
    EditState {
        rev: l.rev,
        page_count: l.page_count,
        sizes: l.sizes.clone(),
        path: entry.path(),
        dirty: false,
        undo: None,
        redo: None,
        redacted: false,
    }
}

/// Runs `op` on `doc`. Returns the new page sizes and whether content was dropped, or None
/// when the op changes nothing (no undo step then).
fn run(doc: &mut PdfDocument, op: &EditOp, sizes: &[[f32; 2]]) -> Result<Option<(Vec<[f32; 2]>, bool)>> {
    let n = sizes.len();
    match op {
        EditOp::Rotate { pages, quarter_turns } => {
            let pages = check_pages(pages, n)?;
            let quarter_turns = quarter_turns.rem_euclid(4);
            if quarter_turns == 0 {
                return Ok(None);
            }
            let mut out = sizes.to_vec();
            for &p in &pages {
                let mut page = doc.load_pdf_page(p as i32)?;
                let r = (page.rotation()? + quarter_turns * 90).rem_euclid(360);
                page.set_rotation(r)?;
                if quarter_turns % 2 != 0 {
                    out[p as usize] = [sizes[p as usize][1], sizes[p as usize][0]];
                }
            }
            Ok(Some((out, false)))
        }
        EditOp::Delete { pages } => {
            let pages = check_pages(pages, n)?;
            if pages.len() == n {
                return Err(invalid("a document needs at least one page"));
            }
            for &p in pages.iter().rev() {
                doc.delete_page(p as i32)?;
            }
            let out = (0..n as u32).filter(|p| pages.binary_search(p).is_err()).map(|p| sizes[p as usize]).collect();
            Ok(Some((out, true)))
        }
        EditOp::Move { pages, to } => {
            let pages = check_pages(pages, n)?;
            if *to as usize > n {
                return Err(EngineError::BadPage(*to));
            }
            let sel = |p: &u32| pages.binary_search(p).is_ok();
            let order: Vec<u32> = (0..*to)
                .filter(|p| !sel(p))
                .chain(pages.iter().copied())
                .chain((*to..n as u32).filter(|p| !sel(p)))
                .collect();
            if order.iter().enumerate().all(|(i, &p)| i as u32 == p) {
                return Ok(None);
            }
            let objs = pages.iter().map(|&p| doc.find_page(p as i32)).collect::<std::result::Result<Vec<_>, _>>()?;
            for &p in pages.iter().rev() {
                doc.delete_page(p as i32)?;
            }
            let at = (0..*to).filter(|p| !sel(p)).count() as i32;
            for (i, obj) in objs.iter().enumerate() {
                doc.insert_page(at + i as i32, obj)?;
            }
            Ok(Some((order.iter().map(|&p| sizes[p as usize]).collect(), false)))
        }
        EditOp::InsertBlank { at, like } => {
            if *at as usize > n {
                return Err(EngineError::BadPage(*at));
            }
            if *like as usize >= n {
                return Err(EngineError::BadPage(*like));
            }
            // Same box as the reference page (as Acrobat does): no content, no rotation.
            let reference = doc.load_pdf_page(*like as i32)?;
            let mb = reference.media_box()?;
            let boxed = reference.object().get_dict_inheritable("MediaBox")?;
            drop(reference);
            let page = doc.new_page_at(*at as i32, Size { width: mb.width(), height: mb.height() })?;
            if let Some(b) = boxed {
                page.object().dict_put("MediaBox", b.copy_array()?)?;
            }
            drop(page);
            let mut out = sizes.to_vec();
            out.insert(*at as usize, page_size(doc, *at)?);
            Ok(Some((out, false)))
        }
        EditOp::InsertFile { at, path, pages, password } => {
            if *at as usize > n {
                return Err(EngineError::BadPage(*at));
            }
            let src = open_source(path, password.as_deref())?;
            let count = src.page_count()?.max(0) as usize;
            let pages = match pages {
                Some(p) => check_pages(p, count)?,
                None => (0..count as u32).collect(),
            };
            graft_pages(doc, &src, &pages, *at as i32)?;
            let mut out = sizes.to_vec();
            for i in 0..pages.len() as u32 {
                out.insert((at + i) as usize, page_size(doc, at + i)?);
            }
            Ok(Some((out, false)))
        }
    }
}

/// Another PDF to take pages from.
fn open_source(path: &str, password: Option<&str>) -> Result<PdfDocument> {
    check_pdf(path)?;
    let mut d = Document::open(path)?;
    if d.needs_password()? {
        match password {
            None => return Err(EngineError::NeedsPassword),
            Some(pw) if !d.authenticate(pw)? => return Err(EngineError::BadPassword),
            _ => {}
        }
    }
    Ok(PdfDocument::try_from(d)?)
}

/// Page keys copied into a grafted page (MuPDF's list): nothing that points at other pages.
const PAGE_KEYS: [&str; 9] =
    ["Contents", "Resources", "MediaBox", "CropBox", "BleedBox", "TrimBox", "ArtBox", "Rotate", "UserUnit"];

/// Copies `pages` of `src` into `dst` at index `at`, sharing one graft map so resources used by
/// several pages (fonts, images) are copied once. Unlike MuPDF's own page graft, comments and
/// web links come along.
fn graft_pages(dst: &mut PdfDocument, src: &PdfDocument, pages: &[u32], at: i32) -> Result<()> {
    let mut map = dst.new_graft_map()?;
    for (i, &p) in pages.iter().enumerate() {
        let from = src.find_page(p as i32)?;
        let mut dict = dst.new_dict()?;
        dict.dict_put("Type", PdfObject::new_name("Page")?)?;
        for key in PAGE_KEYS {
            if let Some(v) = from.get_dict_inheritable(key)? {
                dict.dict_put(key, map.graft_object(&v)?)?;
            }
        }
        if let Some(annots) = from.get_dict("Annots")? {
            let copied = graft_annots(dst, &mut map, &annots)?;
            if copied.len()? > 0 {
                dict.dict_put("Annots", copied)?;
            }
        }
        let page = dst.add_object(&dict)?;
        dst.insert_page(at + i as i32, &page)?;
    }
    Ok(())
}

/// The annotations that stand on their own. Left out: form widgets (their fields live in the
/// source's AcroForm), popups (rebuilt by viewers), and links into the source's own pages. The
/// back-references (/P, /Popup, /Parent, /IRT) are dropped, or grafting would pull in the
/// whole source page tree.
fn graft_annots(dst: &mut PdfDocument, map: &mut PdfGraftMap, annots: &PdfObject) -> Result<PdfObject> {
    let mut out = dst.new_array()?;
    for a in annots.array_iter()? {
        let Some(a) = a?.resolve()? else { continue };
        if !a.is_dict()? {
            continue;
        }
        let sub = a.get_dict("Subtype")?.and_then(|s| s.as_name().ok()).unwrap_or_default();
        let web_link = a
            .get_dict("A")?
            .and_then(|act| act.get_dict("S").ok().flatten())
            .and_then(|s| s.as_name().ok())
            .is_some_and(|s| s == b"URI");
        if sub == b"Widget" || sub == b"Popup" || (sub == b"Link" && !web_link) {
            continue;
        }
        let mut c = a.copy_dict()?;
        for k in ["P", "Popup", "Parent", "IRT"] {
            c.dict_delete(k)?;
        }
        let g = map.graft_object(&c)?;
        out.array_push(dst.add_object(&g)?)?;
    }
    Ok(out)
}

/// A new PDF of `pages` of `src`.
fn new_pdf(parts: &[(&PdfDocument, &[u32])]) -> Result<Vec<u8>> {
    let mut out = PdfDocument::new();
    for (src, pages) in parts {
        let at = out.page_count()?;
        graft_pages(&mut out, src, pages, at)?;
    }
    write_doc(&out, rewrite_opts(), 1 << 20)
}

/// Opens the written file again and checks it has `pages` pages, first and last loadable.
fn verify(path: &Path, pages: u32, password: Option<&str>) -> Result<()> {
    let bad = || EngineError::Io("the written file did not read back correctly".into());
    let mut d = Document::open(path.to_string_lossy().as_ref()).map_err(|_| bad())?;
    if d.needs_password()? && !password.is_some_and(|pw| d.authenticate(pw).unwrap_or(false)) {
        return Err(bad());
    }
    if d.page_count()? != pages as i32 {
        return Err(bad());
    }
    d.load_page(0)?.bounds()?;
    d.load_page(pages as i32 - 1)?.bounds()?;
    Ok(())
}

/// `<dir>/.<name>.<pid>.inkwell-tmp`: hidden, next to the target so the rename stays on one volume.
fn temp_for(target: &Path) -> PathBuf {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Unique, and created with create_new: never follows a planted file or link.
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    target.with_file_name(format!(".{name}.{}.{nonce:x}.inkwell-tmp", std::process::id()))
}

fn write_synced(path: &Path, bytes: &[u8], create_new: bool) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).create(true).create_new(create_new).truncate(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Replaces (or creates) `target` with `bytes`. The bytes go to a temporary file next to it,
/// which is read back before it replaces the target: a failure at any point leaves the target as
/// it was. A link is followed: the file it points to is replaced, the link stays.
///
/// When no file can be created in the target's folder (but the file itself is writable), the
/// bytes are verified in %TEMP% and copied over the target, with a backup copy restored if that
/// fails.
fn write_atomic(target: &Path, bytes: &[u8], pages: u32, password: Option<&str>) -> Result<()> {
    let target = &resolve_link(target);
    let tmp = temp_for(target);
    match write_synced(&tmp, bytes, true) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && target.exists() => {
            let _ = std::fs::remove_file(&tmp);
            return copy_over(target, bytes, pages, password);
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(io(e));
        }
    }
    let r = verify(&tmp, pages, password).and_then(|()| {
        // A virus scanner or the search indexer may hold a new file for a moment (up to 2 s).
        let mut tries = 0;
        loop {
            match replace_file(&tmp, target) {
                Ok(()) => return Ok(()),
                Err(_) if tries < 40 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(io(e)),
            }
        }
    });
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

fn resolve_link(p: &Path) -> PathBuf {
    if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
        if let Ok(real) = std::fs::canonicalize(p) {
            return real;
        }
    }
    p.to_path_buf()
}

/// Moves `tmp` over `target`. On Windows an existing target is replaced with ReplaceFileW, which
/// keeps its ACLs, attributes and alternate data streams. Either `target` holds the new bytes
/// afterwards, or it is exactly as before.
#[cfg(windows)]
fn replace_file(tmp: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW};
    if !target.exists() {
        return std::fs::rename(tmp, target);
    }
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    // With a backup name, every failure leaves both files under their own names, but one: the
    // original was moved to the backup and the new file could not take its place. Undone below.
    let backup = tmp.with_extension("inkwell-bak");
    let ok = unsafe {
        ReplaceFileW(
            wide(target).as_ptr(),
            wide(tmp).as_ptr(),
            wide(&backup).as_ptr(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        let _ = std::fs::remove_file(&backup);
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if !target.exists() && backup.exists() {
        std::fs::rename(&backup, target)?;
    }
    let _ = std::fs::remove_file(&backup);
    Err(err)
}

#[cfg(not(windows))]
fn replace_file(tmp: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(tmp, target)
}

/// The fallback of [`write_atomic`] for a folder that takes no new files: verify in %TEMP%, keep a
/// copy of the target there, write the target in place, restore the copy if that fails.
fn copy_over(target: &Path, bytes: &[u8], pages: u32, password: Option<&str>) -> Result<()> {
    let dir = std::env::temp_dir();
    let tmp = temp_for(&dir.join(target.file_name().unwrap_or_default()));
    let backup = tmp.with_extension("inkwell-bak");
    let r = (|| {
        write_synced(&tmp, bytes, true).map_err(io)?;
        verify(&tmp, pages, password)?;
        std::fs::copy(target, &backup).map_err(io)?;
        let written = write_synced(target, bytes, false).map_err(io).and_then(|()| verify(target, pages, password));
        if written.is_err() {
            std::fs::copy(&backup, target).map_err(|e| {
                EngineError::Io(format!("the file could not be written, and restoring it failed too ({e}); the original is at {}", backup.display()))
            })?;
        }
        written
    })();
    let _ = std::fs::remove_file(&tmp);
    // Kept only when the original could not be put back.
    if !matches!(&r, Err(EngineError::Io(m)) if m.contains("restoring it failed")) {
        let _ = std::fs::remove_file(&backup);
    }
    r
}

/// Removes temporary files of saves to `path` that crashed more than an hour ago.
pub(super) fn clean_stale_temps(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return };
    let prefix = format!(".{}.", name.to_string_lossy());
    let Ok(list) = std::fs::read_dir(dir) else { return };
    let hour = std::time::Duration::from_secs(3600);
    for e in list.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let ours = n.starts_with(&prefix) && (n.ends_with(".inkwell-tmp") || n.ends_with(".inkwell-bak"));
        let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|a| a > hour));
        if ours && old && e.file_type().is_ok_and(|t| t.is_file()) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Writes a file that must not exist yet. Removed again if it does not read back.
fn write_new(target: &Path, bytes: &[u8], pages: u32) -> Result<()> {
    write_synced(target, bytes, true).map_err(io)?;
    verify(target, pages, None).inspect_err(|_| {
        let _ = std::fs::remove_file(target);
    })
}

fn same_path(a: &str, b: &str) -> bool {
    let canon = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p.to_lowercase()));
    a.eq_ignore_ascii_case(b) || canon(a) == canon(b)
}

/// `<base>_Part<n>.pdf` next to `source` for `n` in `1..=parts`, with ` (2)`, ` (3)` ... added to
/// every name when any of them is taken: an earlier split is never overwritten.
fn part_names(source: &Path, parts: usize) -> Vec<PathBuf> {
    let base = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Document".into());
    let names = |k: usize| -> Vec<PathBuf> {
        let suffix = if k == 1 { String::new() } else { format!(" ({k})") };
        (1..=parts).map(|n| source.with_file_name(format!("{base}_Part{n}{suffix}.pdf"))).collect()
    };
    (1..).map(names).find(|v| v.iter().all(|p| !p.exists())).expect("a free name")
}

impl Engine {
    /// The entry's edit lane, created on first use.
    fn lane<'a>(&self, entry: &DocEntry, lane: &'a mut Option<Editor>) -> Result<&'a mut Editor> {
        if entry.closed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::NoDoc);
        }
        if lane.is_none() {
            *lane = Some(Editor::open(entry)?);
        }
        Ok(lane.as_mut().expect("created above"))
    }

    /// Makes the current step the new generation: snapshot and sizes first, then the bump, all
    /// under one lock (rules 2-4).
    fn publish(&self, entry: &DocEntry, ed: &Editor) -> EditState {
        let step = ed.step();
        let rev = {
            let mut l = entry.layout.write();
            l.rev += 1;
            l.page_count = step.sizes.len() as u32;
            l.sizes = step.sizes.as_ref().clone();
            l.snap = Some(step.snap.clone());
            l.rev
        };
        // Old generations are never served again: free their display lists, and the replicas
        // that are idle (they would pin an old snapshot until next used).
        {
            let mut lists = self.lists.lock();
            let old: Vec<_> = lists.iter().map(|(k, _)| *k).filter(|k| k.0 == entry.id && k.1 < rev).collect();
            for k in old {
                lists.pop(&k);
            }
        }
        for slot in &entry.slots {
            if let Some(mut g) = slot.try_lock() {
                if g.as_ref().is_some_and(|d| d.rev != rev) {
                    *g = None;
                }
            }
        }
        ed.state(entry)
    }

    fn apply_locked(&self, entry: &DocEntry, ed: &mut Editor, op: &EditOp, source: EditSource) -> Result<EditState> {
        let sizes = ed.step().sizes.clone();
        // A panic must also roll back below, or the next edit would snapshot what it left behind.
        let ran = catch_unwind(AssertUnwindSafe(|| run(&mut ed.doc.doc, op, &sizes))).unwrap_or(Err(EngineError::Panic));
        let changed = ran.and_then(|c| match c {
            None => Ok(None),
            Some((after, drops)) => {
                let snap = ed.snapshot()?;
                let doc = open_snap(&snap, entry)?;
                let pages = op.touched(sizes.len(), after.len());
                let step =
                    Step { snap, label: op.label(), source, at_ms: now_ms(), pages, drops, redacts: false, exact: false, sizes: Arc::new(after) };
                Ok(Some((step, doc)))
            }
        });
        match changed {
            Ok(Some((step, doc))) => {
                ed.push(step);
                // From now on the authoritative document reads the new snapshot, so its next
                // update appends to it.
                ed.doc = doc;
                Ok(self.publish(entry, ed))
            }
            Ok(None) => Ok(ed.state(entry)),
            Err(e) => {
                // Throw away whatever the op did before failing.
                ed.doc = open_snap(&ed.step().snap, entry)?;
                Err(e)
            }
        }
    }

    /// Applies one edit as one undo step and returns the new state. Edits to the same document
    /// run one at a time; renders keep going meanwhile and switch to the new generation after.
    pub fn apply(&self, id: DocId, op: EditOp) -> Result<EditState> {
        self.apply_from(id, op, EditSource::User)
    }

    /// [`Engine::apply`] on behalf of `source`. Consecutive edits of one assistant turn are one
    /// group: a single [`Engine::undo`] reverts the whole turn.
    pub fn apply_from(&self, id: DocId, op: EditOp, source: EditSource) -> Result<EditState> {
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let ed = self.lane(&entry, &mut lane)?;
        self.apply_locked(&entry, ed, &op, source)
    }

    /// An edit that is not an [`EditOp`] (the redaction engine): `f` changes the document and
    /// returns the pages it touched. Same lane, history and generation as `apply`; if `f` fails,
    /// even halfway, the document is rolled back to the current step. `f` must not keep page
    /// handles. With `redaction`, the step's snapshot is a full garbage-collected rewrite (an
    /// incremental one would keep the removed content), every save including it rewrites the
    /// file, and the [`set_redaction_hook`] hook runs.
    pub fn apply_custom(
        &self,
        id: DocId,
        label: String,
        source: EditSource,
        redaction: bool,
        f: impl FnOnce(&mut PdfDocument) -> Result<Vec<u32>>,
    ) -> Result<EditState> {
        self.custom(id, label, source, redaction, |d| Ok((f(d)?, None)))
    }

    /// A redaction whose result is a finished file: `f` changes the document and returns the
    /// pages it touched and the bytes it verified (`redact::ApplyReport::bytes`). Those bytes
    /// become the step's snapshot, and a save of this step writes exactly them. Otherwise as
    /// [`Engine::apply_custom`] with `redaction`.
    pub fn apply_redaction(
        &self,
        id: DocId,
        label: String,
        source: EditSource,
        f: impl FnOnce(&mut PdfDocument) -> Result<(Vec<u32>, Vec<u8>)>,
    ) -> Result<EditState> {
        self.custom(id, label, source, true, |d| f(d).map(|(pages, bytes)| (pages, Some(bytes))))
    }

    /// Runs `f` on the authoritative document (marks included) without changing it. `f` must not
    /// change the document or keep page handles.
    pub fn read_custom<R>(&self, id: DocId, f: impl FnOnce(&PdfDocument) -> Result<R>) -> Result<R> {
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let ed = self.lane(&entry, &mut lane)?;
        catch_unwind(AssertUnwindSafe(|| f(&ed.doc.doc))).unwrap_or(Err(EngineError::Panic))
    }

    fn custom(
        &self,
        id: DocId,
        label: String,
        source: EditSource,
        redaction: bool,
        f: impl FnOnce(&mut PdfDocument) -> Result<(Vec<u32>, Option<Vec<u8>>)>,
    ) -> Result<EditState> {
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let ed = self.lane(&entry, &mut lane)?;
        let len = ed.step().snap.len;
        let ran = catch_unwind(AssertUnwindSafe(|| f(&mut ed.doc.doc))).unwrap_or(Err(EngineError::Panic));
        let changed = ran.and_then(|(pages, bytes)| {
            let exact = bytes.is_some();
            let snap = if let Some(b) = bytes {
                Snap { len: b.len(), bytes: Arc::new(b) }
            } else if redaction {
                let out = write_doc(&ed.doc.doc, rewrite_opts(), len)?;
                Snap { len: out.len(), bytes: Arc::new(out) }
            } else {
                ed.snapshot()?
            };
            let doc = open_snap(&snap, &entry)?;
            let sizes = Arc::new(page_sizes(&doc.doc)?);
            let drops = sizes.len() < ed.step().sizes.len();
            Ok((Step { snap, label, source, at_ms: now_ms(), pages, drops, redacts: redaction, exact, sizes }, doc))
        });
        match changed {
            Ok((step, doc)) => {
                ed.push(step);
                ed.doc = doc;
                let state = self.publish(&entry, ed);
                drop(lane);
                if redaction {
                    if let Some(hook) = REDACTION_HOOK.lock().clone() {
                        hook(id);
                    }
                }
                Ok(state)
            }
            Err(e) => {
                ed.doc = open_snap(&ed.step().snap, &entry)?;
                Err(e)
            }
        }
    }

    /// Drops every history entry before the current one (e.g. the versions from before a
    /// redaction): they can no longer be restored. Redo stays. Called right after a redaction,
    /// this frees every byte of the unredacted versions (a redaction step starts a new buffer).
    pub fn forget_history(&self, id: DocId) -> Result<EditState> {
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let Some(ed) = lane.as_mut() else { return Ok(clean_state(&entry)) };
        let cur = ed.cur;
        // The kept steps' bytes may still hold what dropped steps removed: saves keep rewriting.
        let (drops, redacts) = (ed.steps[..=cur].iter().any(|s| s.drops), ed.steps[..=cur].iter().any(|s| s.redacts));
        ed.steps.drain(..cur);
        ed.steps[0].drops |= drops;
        ed.steps[0].redacts |= redacts;
        ed.saved = ed.saved.and_then(|s| s.checked_sub(cur));
        ed.cur = 0;
        Ok(ed.state(&entry))
    }

    /// Reverts the last step (or the last assistant turn). A no-op (returning the state) when
    /// there is nothing to undo.
    pub fn undo(&self, id: DocId) -> Result<EditState> {
        self.travel(id, |ed| ed.undo_target())
    }

    /// Reapplies what was undone last. A no-op when there is nothing to redo.
    pub fn redo(&self, id: DocId) -> Result<EditState> {
        self.travel(id, |ed| ed.redo_target())
    }

    /// Shows the document as of history entry `index` (undo or redo up to it). Later entries
    /// stay redoable until the next edit.
    pub fn restore(&self, id: DocId, index: usize) -> Result<EditState> {
        // Checked under the lane lock: an edit in between may have dropped the entry.
        let mut bad = index > 0;
        let state = self.travel(id, |ed| {
            bad = index >= ed.steps.len();
            Some(index).filter(|_| !bad)
        })?;
        if bad {
            return Err(invalid(format!("no history entry {index}")));
        }
        Ok(state)
    }

    /// Every step since the file was opened, oldest first (entry 0 is the file as opened).
    pub fn history(&self, id: DocId) -> Result<Vec<HistoryEntry>> {
        let entry = self.entry(id)?;
        let lane = entry.editor.lock();
        let Some(ed) = lane.as_ref() else {
            let at_ms = now_ms();
            return Ok(vec![HistoryEntry { index: 0, label: "Opened".into(), source: EditSource::User, at_ms, pages: vec![], current: true, saved: true }]);
        };
        Ok(ed
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| HistoryEntry {
                index: i,
                label: s.label.clone(),
                source: s.source.clone(),
                at_ms: s.at_ms,
                pages: s.pages.clone(),
                current: i == ed.cur,
                saved: ed.saved == Some(i),
            })
            .collect())
    }

    fn travel(&self, id: DocId, target: impl FnOnce(&Editor) -> Option<usize>) -> Result<EditState> {
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let Some(ed) = lane.as_mut() else { return Ok(clean_state(&entry)) };
        let Some(to) = target(ed).filter(|&t| t != ed.cur) else { return Ok(ed.state(&entry)) };
        ed.doc = open_snap(&ed.steps[to].snap, &entry)?;
        ed.cur = to;
        Ok(self.publish(&entry, ed))
    }

    pub fn edit_state(&self, id: DocId) -> Result<EditState> {
        let entry = self.entry(id)?;
        let lane = entry.editor.lock();
        Ok(lane.as_ref().map_or_else(|| clean_state(&entry), |ed| ed.state(&entry)))
    }

    /// An error if `path` is open in Inkwell (other than as `except`).
    fn check_not_open(&self, path: &str, except: Option<DocId>) -> Result<()> {
        let open = self.docs.read().values().any(|e| Some(e.id) != except && same_path(&e.path(), path));
        if open {
            Err(invalid("that file is open in Inkwell; close it first"))
        } else {
            Ok(())
        }
    }

    /// Saves to the document's file, or to `to` (Save as; the document then lives there).
    ///
    /// When the edits only added or changed objects, the file is the original bytes plus
    /// incremental updates; after a delete it is rewritten without the removed content. Either
    /// way the bytes go to a temporary file that is read back, the replicas let go of the old
    /// file, and only then does a rename replace it.
    pub fn save(&self, id: DocId, to: Option<&str>) -> Result<EditState> {
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let target = to.map(str::to_string).unwrap_or_else(|| entry.path());
        let same = same_path(&target, &entry.path());
        if same && lane.as_ref().is_none_or(|ed| ed.saved == Some(ed.cur)) {
            return Ok(lane.as_ref().map_or_else(|| clean_state(&entry), |ed| ed.state(&entry)));
        }
        if !same {
            self.check_not_open(&target, Some(id))?;
        }
        let ed = self.lane(&entry, &mut lane)?;
        let rewrite = ed.steps[..=ed.cur].iter().any(|s| s.drops || s.redacts);
        let full;
        let bytes: &[u8] = if ed.step().exact {
            // The redaction engine's verified file: any other write of it is unverified.
            ed.bytes()
        } else if rewrite {
            // From a throwaway copy: a garbage-collecting write leaves its document unable to take
            // incremental updates, which every later edit needs.
            full = write_doc(&open_snap(&ed.step().snap, &entry)?.doc, rewrite_opts(), ed.step().snap.len)?;
            &full
        } else {
            ed.bytes()
        };
        let pages = ed.step().sizes.len() as u32;
        let password = entry.password.lock().clone();
        if same {
            // Replicas must read from memory before the file goes away under them.
            if entry.layout.read().snap.is_none() {
                self.publish(&entry, ed);
            }
            self.release_file(&entry);
        }
        write_atomic(Path::new(&target), bytes, pages, password.as_deref())?;
        if !same {
            *entry.path.write() = target;
        }
        ed.saved = Some(ed.cur);
        Ok(ed.state(&entry))
    }

    /// Drops every replica that has the file on disk open (rule 5). Waits for busy ones.
    fn release_file(&self, entry: &DocEntry) {
        for slot in &entry.slots {
            let mut g = slot.lock();
            if g.as_ref().is_some_and(|d| d._bytes.is_none()) {
                *g = None;
            }
        }
    }

    /// Writes `pages` of the document to a new PDF at `out` (replacing it), then deletes them
    /// from the document if `delete_after` (one undo step).
    pub fn extract(&self, id: DocId, pages: &[u32], out: &str, delete_after: bool) -> Result<EditState> {
        self.check_not_open(out, None)?;
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let ed = self.lane(&entry, &mut lane)?;
        let pages = check_pages(pages, ed.step().sizes.len())?;
        let bytes = new_pdf(&[(&ed.doc.doc, &pages)])?;
        write_atomic(Path::new(out), &bytes, pages.len() as u32, None)?;
        if delete_after {
            self.apply_locked(&entry, ed, &EditOp::Delete { pages }, EditSource::User)
        } else {
            Ok(ed.state(&entry))
        }
    }

    /// Splits the document into files of `every` pages, `<name>_Part<n>.pdf` next to it (the last
    /// one may be shorter). Never overwrites a file: see [`part_names`].
    pub fn split(&self, id: DocId, every: u32) -> Result<SplitResult> {
        if every == 0 {
            return Err(invalid("pages per file must be at least 1"));
        }
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let ed = self.lane(&entry, &mut lane)?;
        let n = ed.step().sizes.len() as u32;
        let chunks: Vec<Vec<u32>> =
            (0..n).step_by(every as usize).map(|s| (s..(s + every).min(n)).collect()).collect();
        let names = part_names(Path::new(&entry.path()), chunks.len());
        let mut files = Vec::new();
        for (k, (pages, name)) in chunks.iter().zip(&names).enumerate() {
            let r = new_pdf(&[(&ed.doc.doc, pages)]).and_then(|bytes| write_new(name, &bytes, pages.len() as u32));
            if let Err(e) = r {
                if files.is_empty() {
                    return Err(e);
                }
                let done: Vec<_> = files.iter().map(|f: &String| Path::new(f).file_name().unwrap_or_default().to_string_lossy()).collect();
                return Err(EngineError::Io(format!(
                    "part {} of {} could not be written ({e}); written: {}",
                    k + 1,
                    chunks.len(),
                    done.join(", ")
                )));
            }
            files.push(name.to_string_lossy().into_owned());
        }
        Ok(SplitResult { files })
    }

    /// Combines whole files, in order, into a new PDF at `out` (replacing it). Returns its page count.
    pub fn merge(&self, paths: &[String], out: &str) -> Result<u32> {
        if paths.is_empty() {
            return Err(invalid("no files to merge"));
        }
        self.check_not_open(out, None)?;
        let _ = mupdf::Context::get();
        let srcs = paths
            .iter()
            .map(|p| {
                open_source(p, None).map_err(|e| match e {
                    EngineError::NeedsPassword | EngineError::BadPassword => {
                        invalid(format!("{} is password protected", Path::new(p).display()))
                    }
                    e => e,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let pages: Vec<Vec<u32>> =
            srcs.iter().map(|s| Ok((0..s.page_count()?.max(0) as u32).collect())).collect::<Result<_>>()?;
        let parts: Vec<(&PdfDocument, &[u32])> = srcs.iter().zip(&pages).map(|(s, p)| (s, p.as_slice())).collect();
        let bytes = new_pdf(&parts)?;
        let total = pages.iter().map(Vec::len).sum::<usize>() as u32;
        drop(parts);
        drop(srcs);
        write_atomic(Path::new(out), &bytes, total, None)?;
        Ok(total)
    }
}

#[cfg(test)]
#[path = "tests_edit.rs"]
mod tests;
