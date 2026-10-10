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
    /// What the step is called in the undo menu.
    pub fn label(&self) -> String {
        let n = |p: &Vec<u32>| if p.len() == 1 { "page".to_string() } else { format!("{} pages", p.len()) };
        match self {
            EditOp::Rotate { pages, .. } => format!("Rotate {}", n(pages)),
            EditOp::Delete { pages } => format!("Delete {}", n(pages)),
            EditOp::Move { pages, .. } => format!("Move {}", n(pages)),
            EditOp::InsertBlank { .. } => "Insert blank page".into(),
            EditOp::InsertFile { .. } => "Insert pages".into(),
        }
    }
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
    snap: Snap,
    label: String,
    /// Removed content (deleted pages): the bytes still carry it, so a save must rewrite the file.
    drops: bool,
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

/// A complete file without anything unreferenced (deleted pages are really gone).
fn rewrite_opts() -> PdfWriteOptions {
    let mut o = PdfWriteOptions::default();
    o.set_garbage_level(1).set_compress(true);
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
        let base = Step { snap, label: String::new(), drops: false, sizes };
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

    fn state(&self, entry: &DocEntry) -> EditState {
        EditState {
            dirty: self.saved != Some(self.cur),
            undo: (self.cur > 0).then(|| self.steps[self.cur].label.clone()),
            redo: self.steps.get(self.cur + 1).map(|s| s.label.clone()),
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
    }
}

/// Runs `op` on `doc`. Returns the new page sizes and whether content was dropped, or None
/// when the op changes nothing (no undo step then).
fn run(doc: &mut PdfDocument, op: &EditOp, sizes: &[[f32; 2]]) -> Result<Option<(Vec<[f32; 2]>, bool)>> {
    let n = sizes.len();
    match op {
        EditOp::Rotate { pages, quarter_turns } => {
            let pages = check_pages(pages, n)?;
            if quarter_turns.rem_euclid(4) == 0 {
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
    target.with_file_name(format!(".{name}.{}.inkwell-tmp", std::process::id()))
}

fn write_synced(path: &Path, bytes: &[u8], create_new: bool) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).create(true).create_new(create_new).truncate(true).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Replaces (or creates) `target` with `bytes`. The bytes go to a temporary file next to it,
/// which is read back before it is renamed over the target: a failure at any point leaves the
/// target as it was.
fn write_atomic(target: &Path, bytes: &[u8], pages: u32, password: Option<&str>) -> Result<()> {
    let tmp = temp_for(target);
    let r = (|| {
        write_synced(&tmp, bytes, false).map_err(io)?;
        verify(&tmp, pages, password)?;
        // A virus scanner or the search indexer may hold a new file for a moment (up to 2 s).
        let mut tries = 0;
        loop {
            match std::fs::rename(&tmp, target) {
                Ok(()) => return Ok(()),
                Err(_) if tries < 40 => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(io(e)),
            }
        }
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
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

    fn apply_locked(&self, entry: &DocEntry, ed: &mut Editor, op: &EditOp) -> Result<EditState> {
        let sizes = ed.step().sizes.clone();
        let changed = run(&mut ed.doc.doc, op, &sizes).and_then(|c| match c {
            None => Ok(None),
            Some((sizes, drops)) => {
                let snap = ed.snapshot()?;
                let doc = open_snap(&snap, entry)?;
                Ok(Some((Step { snap, label: op.label(), drops, sizes: Arc::new(sizes) }, doc)))
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
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let ed = self.lane(&entry, &mut lane)?;
        self.apply_locked(&entry, ed, &op)
    }

    /// Reverts the last step. A no-op (returning the state) when there is nothing to undo.
    pub fn undo(&self, id: DocId) -> Result<EditState> {
        self.travel(id, -1)
    }

    /// Reapplies the step undone last. A no-op when there is nothing to redo.
    pub fn redo(&self, id: DocId) -> Result<EditState> {
        self.travel(id, 1)
    }

    fn travel(&self, id: DocId, dir: isize) -> Result<EditState> {
        let entry = self.entry(id)?;
        let mut lane = entry.editor.lock();
        let Some(ed) = lane.as_mut() else { return Ok(clean_state(&entry)) };
        let to = ed.cur as isize + dir;
        if to < 0 || to as usize >= ed.steps.len() {
            return Ok(ed.state(&entry));
        }
        ed.doc = open_snap(&ed.steps[to as usize].snap, &entry)?;
        ed.cur = to as usize;
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
        let rewrite = ed.steps[1..=ed.cur].iter().any(|s| s.drops);
        let full;
        let bytes: &[u8] = if rewrite {
            full = write_doc(&ed.doc.doc, rewrite_opts(), ed.step().snap.len)?;
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
            self.apply_locked(&entry, ed, &EditOp::Delete { pages })
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
        for (pages, name) in chunks.iter().zip(&names) {
            let bytes = new_pdf(&[(&ed.doc.doc, pages)])?;
            write_new(name, &bytes, pages.len() as u32)?;
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
