//! Article threads (PDF 32000-1 section 12.4.3): the catalog's /Threads array, each thread a circular,
//! doubly linked list of beads (/F first bead; bead /T thread, /N next, /V previous, /P page, /R
//! rectangle), and every page's /B array listing its beads. Readers follow a story across columns and
//! pages with them (docs/threads.md).
//!
//! A file is untrusted: every walk is iterative, keeps a visited set and stops at a cap. A thread whose
//! chain does not close, whose bead names another thread or sits on a page that is not in the document,
//! or whose rectangle is empty is reported `damaged` with the readable prefix, and only deleting it or
//! editing its properties is offered. Every other step rewrites its thread from a list of beads
//! (`commit`), so the invariants (ring closed both ways, /F valid, /T on every bead, pages' /B in sync)
//! hold by construction.
//!
//! Ids are object numbers: they are valid for the generation they were listed at, and every step looks
//! its ids up again and refuses one that is not currently there.

use super::{Result, bad, get, put_name, put_str};
use crate::forms::nums;
use mupdf::Matrix;
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Threads read from a document, and created in one.
pub const MAX_THREADS: usize = 1_000;
/// Beads read from one thread, and made in one.
pub const MAX_BEADS: usize = 10_000;
/// The smallest side of a bead, in points.
const MIN_SIDE: f32 = 3.0;
/// Characters kept of each entry of a thread's /I.
const MAX_TEXT: usize = 1_000;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub keywords: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bead {
    pub id: i32,
    /// 0-based page.
    pub page: u32,
    /// Where it is on the page: x0, y0, x1, y1 in points, y down, the page as it is shown.
    pub rect: [f32; 4],
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: i32,
    /// The /I title, or "Article n".
    pub title: String,
    pub info: Info,
    /// In reading order; for a damaged thread only the readable prefix.
    pub beads: Vec<Bead>,
    /// Why the thread cannot be edited, when it cannot.
    pub damaged: Option<String>,
}

// ---- reading ------------------------------------------------------------------------------------

/// The document's pages by object number, and back.
struct Pages {
    by_num: HashMap<i32, u32>,
    nums: Vec<i32>,
}

impl Pages {
    fn of(doc: &PdfDocument) -> Result<Self> {
        let n = doc.page_count()?.max(0);
        let mut by_num = HashMap::with_capacity(n as usize);
        let mut nums = Vec::with_capacity(n as usize);
        for i in 0..n {
            let o = doc.find_page(i)?;
            let num = if o.is_indirect()? { o.as_indirect()? } else { 0 };
            by_num.insert(num, i as u32);
            nums.push(num);
        }
        Ok(Self { by_num, nums })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flaw {
    Foreign,
    NoPage,
    BadRect,
}

impl Flaw {
    fn text(self) -> &'static str {
        match self {
            Self::Foreign => "A bead belongs to another article.",
            Self::NoPage => "A bead is on a page that is not in this document.",
            Self::BadRect => "A bead has an empty or invalid rectangle.",
        }
    }
}

/// A bead as the file has it.
struct Raw {
    obj: PdfObject,
    id: i32,
    prev: Option<i32>,
    page: Option<u32>,
    /// /R in the page's own (user) space, normalized.
    user: Option<[f32; 4]>,
    flaw: Option<Flaw>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum End {
    Closed,
    Open,
    Loop,
    Cap,
    Broken,
}

struct Ring {
    beads: Vec<Raw>,
    end: End,
}

impl Ring {
    /// How many beads are readable: the ones before the first flaw.
    fn good(&self) -> usize {
        self.beads.iter().position(|b| b.flaw.is_some()).unwrap_or(self.beads.len())
    }

    fn damage(&self) -> Option<String> {
        if let Some(f) = self.beads.iter().find_map(|b| b.flaw) {
            return Some(f.text().into());
        }
        match self.end {
            End::Closed => {
                // The ring closes; are the links back right (each /V names the bead before)?
                let n = self.beads.len();
                let wrong = (0..n).any(|i| self.beads[i].prev != Some(self.beads[(i + n - 1) % n].id));
                wrong.then(|| "The links back to the previous beads are wrong.".into())
            }
            End::Open => Some("The chain of beads does not come back to its first bead.".into()),
            End::Loop => Some("The chain of beads loops back into its middle.".into()),
            End::Cap => Some(format!("The article has more than {MAX_BEADS} beads.")),
            End::Broken => Some("A link in the chain of beads is broken.".into()),
        }
    }
}

fn indirect_num(o: &PdfObject) -> Option<i32> {
    o.is_indirect().ok()?.then(|| o.as_indirect().ok()).flatten().filter(|n| *n > 0)
}

fn rect_user(d: &PdfObject) -> Option<[f32; 4]> {
    let v = nums(&get(d, "R")?);
    let [a, b, c, e] = <[f32; 4]>::try_from(v).ok()?;
    let r = [a.min(c), b.min(e), a.max(c), b.max(e)];
    (r.iter().all(|x| x.is_finite()) && r[2] - r[0] >= 1.0 && r[3] - r[1] >= 1.0).then_some(r)
}

fn raw_bead(r: &PdfObject, d: &PdfObject, id: i32, tid: i32, pages: &Pages) -> Raw {
    let prev = get(d, "V").and_then(|v| indirect_num(&v));
    let page = get(d, "P").and_then(|p| indirect_num(&p)).and_then(|n| pages.by_num.get(&n).copied());
    let user = rect_user(d);
    // A missing /T is allowed on a bead (the spec makes it optional after the first); another thread's is not.
    let foreign = get(d, "T").is_some_and(|t| indirect_num(&t) != Some(tid));
    let flaw = if foreign {
        Some(Flaw::Foreign)
    } else if page.is_none() {
        Some(Flaw::NoPage)
    } else if user.is_none() {
        Some(Flaw::BadRect)
    } else {
        None
    };
    Raw { obj: r.share(), id, prev, page, user, flaw }
}

/// The chain of beads of `thread` from /F: iterative, loop-safe and capped.
fn walk(thread: &PdfObject, tid: i32, pages: &Pages) -> Result<Ring> {
    let mut ring = Ring { beads: Vec::new(), end: End::Open };
    let Some(first) = get(thread, "F") else { return Ok(ring) };
    let Some(first_id) = indirect_num(&first) else {
        ring.end = End::Broken;
        return Ok(ring);
    };
    let mut seen: HashSet<i32> = HashSet::new();
    let mut cur = first;
    loop {
        let Some(id) = indirect_num(&cur) else {
            ring.end = End::Broken;
            break;
        };
        if !seen.insert(id) {
            ring.end = End::Loop;
            break;
        }
        let Some(d) = cur.resolve()?.filter(|d| d.is_dict().unwrap_or(false)) else {
            ring.end = End::Broken;
            break;
        };
        if ring.beads.len() >= MAX_BEADS {
            ring.end = End::Cap;
            break;
        }
        ring.beads.push(raw_bead(&cur, &d, id, tid, pages));
        match get(&d, "N") {
            None => break,
            Some(n) if indirect_num(&n) == Some(first_id) => {
                ring.end = End::Closed;
                break;
            }
            Some(n) => cur = n,
        }
    }
    Ok(ring)
}

/// The entries of /Threads that are indirect objects, at most [`MAX_THREADS`]: (id, object).
fn thread_objs(doc: &PdfDocument) -> Result<Vec<(i32, PdfObject)>> {
    let Some(arr) = get(&doc.catalog()?, "Threads").filter(|a| a.is_array().unwrap_or(false)) else { return Ok(Vec::new()) };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for i in 0..arr.len()?.min(MAX_THREADS * 2) {
        if out.len() >= MAX_THREADS {
            break;
        }
        let Some(t) = arr.get_array(i as i32)? else { continue };
        let Some(id) = indirect_num(&t) else { continue };
        if !seen.insert(id) || !t.resolve()?.is_some_and(|d| d.is_dict().unwrap_or(false)) {
            continue;
        }
        out.push((id, t));
    }
    Ok(out)
}

fn clip(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect::<String>().trim().to_string()
}

fn info_of(thread: &PdfObject) -> Info {
    let Some(i) = get(thread, "I").filter(|i| i.is_dict().unwrap_or(false)) else { return Info::default() };
    let s = |k: &str| get(&i, k).and_then(|v| v.as_string().ok()).map(|v| clip(&v)).unwrap_or_default();
    Info { title: s("Title"), subject: s("Subject"), author: s("Author"), keywords: s("Keywords") }
}

/// Where the page's user space lands on the page as it is shown (y down, its box at the origin).
fn page_matrix(doc: &PdfDocument, pages: &Pages, p: u32) -> Result<Matrix> {
    let num = *pages.nums.get(p as usize).ok_or(crate::engine::EngineError::BadPage(p))?;
    doc.new_indirect(num, 0)?.page_ctm().map_err(Into::into)
}

fn map_rect(m: &Matrix, r: [f32; 4]) -> [f32; 4] {
    let pts = [m.transform_xy(r[0], r[1]), m.transform_xy(r[2], r[1]), m.transform_xy(r[0], r[3]), m.transform_xy(r[2], r[3])];
    let xs = pts.map(|p| p.0);
    let ys = pts.map(|p| p.1);
    [
        xs.iter().copied().fold(f32::MAX, f32::min),
        ys.iter().copied().fold(f32::MAX, f32::min),
        xs.iter().copied().fold(f32::MIN, f32::max),
        ys.iter().copied().fold(f32::MIN, f32::max),
    ]
}

fn view(doc: &PdfDocument, id: i32, obj: &PdfObject, n: usize, pages: &Pages, ctms: &mut HashMap<u32, Matrix>) -> Result<Thread> {
    let thread = obj.resolve()?.unwrap_or_else(|| obj.share());
    let ring = walk(&thread, id, pages)?;
    let info = info_of(&thread);
    let mut beads = Vec::new();
    for b in &ring.beads[..ring.good()] {
        let (Some(page), Some(user)) = (b.page, b.user) else { continue };
        let m = match ctms.get(&page) {
            Some(m) => m.clone(),
            None => {
                let m = page_matrix(doc, pages, page)?;
                ctms.insert(page, m.clone());
                m
            }
        };
        beads.push(Bead { id: b.id, page, rect: map_rect(&m, user) });
    }
    let title = if info.title.is_empty() { format!("Article {}", n + 1) } else { info.title.clone() };
    Ok(Thread { id, title, info, beads, damaged: ring.damage() })
}

/// Every thread of the document, in /Threads order.
pub fn read_all(doc: &PdfDocument) -> Result<Vec<Thread>> {
    let objs = thread_objs(doc)?;
    if objs.is_empty() {
        return Ok(Vec::new());
    }
    let pages = Pages::of(doc)?;
    let mut ctms = HashMap::new();
    objs.iter().enumerate().map(|(n, (id, o))| view(doc, *id, o, n, &pages, &mut ctms)).collect()
}

// ---- writing ------------------------------------------------------------------------------------

/// A bead to be written: an existing object to reuse (or none), its page and /R (user space).
struct Entry {
    obj: Option<PdfObject>,
    page: u32,
    user: [f32; 4],
}

impl Entry {
    fn of(b: &Raw) -> Option<Self> {
        Some(Self { obj: Some(b.obj.share()), page: b.page?, user: b.user? })
    }
}

/// Makes `thread`'s ring from `entries` (empty: no beads left) and brings the /B arrays of every page it
/// had beads on or has now in line: this thread's beads in ring order after the ones already there that
/// are not its own. Returns the pages touched.
fn commit(doc: &mut PdfDocument, pages: &Pages, thread: &PdfObject, old: &[Raw], entries: Vec<Entry>) -> Result<Vec<u32>> {
    if entries.len() > MAX_BEADS {
        return Err(bad(format!("an article can have at most {MAX_BEADS} beads")));
    }
    // Every bead object exists before any link is written.
    let mut refs: Vec<PdfObject> = Vec::with_capacity(entries.len());
    for e in &entries {
        refs.push(match &e.obj {
            Some(o) => o.share(),
            None => {
                let mut d = doc.new_dict()?;
                put_name(&mut d, "Type", "Bead")?;
                doc.add_object(&d)?
            }
        });
    }
    let n = refs.len();
    for (i, e) in entries.iter().enumerate() {
        let mut b = refs[i].share();
        b.dict_put("T", thread.share())?;
        let num = *pages.nums.get(e.page as usize).ok_or(crate::engine::EngineError::BadPage(e.page))?;
        b.dict_put("P", doc.new_indirect(num, 0)?)?;
        let mut r = doc.new_array()?;
        for v in e.user {
            r.array_push(PdfObject::new_real(v)?)?;
        }
        b.dict_put("R", r)?;
        b.dict_put("N", refs[(i + 1) % n].share())?;
        b.dict_put("V", refs[(i + n - 1) % n].share())?;
    }
    if let Some(first) = refs.first() {
        let mut t = thread.share();
        t.dict_put("F", first.share())?;
    }
    // /B of every page this thread was or is on.
    let mut touched: Vec<u32> = old.iter().filter_map(|b| b.page).chain(entries.iter().map(|e| e.page)).collect();
    touched.sort_unstable();
    touched.dedup();
    let mine: HashSet<i32> = old.iter().map(|b| b.id).chain(refs.iter().filter_map(indirect_num)).collect();
    for &p in &touched {
        let page = doc.find_page(p as i32)?;
        let mut b = doc.new_array()?;
        if let Some(cur) = get(&page, "B").filter(|c| c.is_array().unwrap_or(false)) {
            for i in 0..cur.len()?.min(MAX_BEADS * 2) {
                let Some(item) = cur.get_array(i as i32)? else { continue };
                if indirect_num(&item).is_some_and(|x| mine.contains(&x)) {
                    continue;
                }
                b.array_push(item.share())?;
            }
        }
        for (i, e) in entries.iter().enumerate() {
            if e.page == p {
                b.array_push(refs[i].share())?;
            }
        }
        let mut page = page.share();
        if b.len()? == 0 {
            page.dict_delete("B")?;
        } else {
            page.dict_put("B", b)?;
        }
    }
    Ok(touched)
}

/// Takes `id` out of /Threads (the key goes with the last thread).
fn unlist(doc: &mut PdfDocument, keep: impl Fn(i32) -> bool) -> Result<()> {
    let mut cat = doc.catalog()?;
    let objs = thread_objs(doc)?;
    let mut arr = doc.new_array()?;
    for (id, o) in &objs {
        if keep(*id) {
            arr.array_push(o.share())?;
        }
    }
    if arr.len()? == 0 {
        cat.dict_delete("Threads")?;
    } else {
        cat.dict_put("Threads", arr)?;
    }
    Ok(())
}

/// A thread of the document, found again by id, with its ring.
struct Loaded {
    obj: PdfObject,
    ring: Ring,
}

fn load(doc: &PdfDocument, pages: &Pages, id: i32) -> Result<Loaded> {
    let (_, obj) = thread_objs(doc)?.into_iter().find(|(i, _)| *i == id).ok_or_else(|| bad("that article is no longer in the document"))?;
    let ring = walk(&obj.resolve()?.unwrap_or_else(|| obj.share()), id, pages)?;
    Ok(Loaded { obj, ring })
}

impl Loaded {
    /// The entries of a sound thread, or the reason it cannot be changed.
    fn entries(&self) -> Result<Vec<Entry>> {
        if self.ring.damage().is_some() {
            return Err(bad("this article is damaged: delete it to start again"));
        }
        Ok(self.ring.beads.iter().filter_map(Entry::of).collect())
    }

    fn bead(&self, id: i32) -> Result<usize> {
        self.ring.beads.iter().position(|b| b.id == id).ok_or_else(|| bad("that bead is no longer in the article"))
    }
}

/// A rectangle from the page as it is shown, as /R for `page`: inside the page and at least 3 pt a side.
fn user_rect(doc: &PdfDocument, pages: &Pages, page: u32, r: [f32; 4]) -> Result<[f32; 4]> {
    if page as usize >= pages.nums.len() {
        return Err(crate::engine::EngineError::BadPage(page));
    }
    if !r.iter().all(|x| x.is_finite()) {
        return Err(bad("the bead's rectangle is not a number"));
    }
    let [w, h] = crate::engine::edit::page_size(doc, page)?;
    let c = [r[0].min(r[2]).clamp(0.0, w), r[1].min(r[3]).clamp(0.0, h), r[0].max(r[2]).clamp(0.0, w), r[1].max(r[3]).clamp(0.0, h)];
    if c[2] - c[0] < MIN_SIDE || c[3] - c[1] < MIN_SIDE {
        return Err(bad("the bead is too small (at least 3 points a side, inside the page)"));
    }
    let inv = page_matrix(doc, pages, page)?.invert().ok_or_else(|| bad("the page has no usable size"))?;
    Ok(map_rect(&inv, c))
}

pub fn same_rect(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.01)
}

/// A new thread of one bead; the title may be empty.
pub fn create(doc: &mut PdfDocument, title: &str, page: u32, rect: [f32; 4]) -> Result<Vec<u32>> {
    let pages = Pages::of(doc)?;
    if thread_objs(doc)?.len() >= MAX_THREADS {
        return Err(bad(format!("a document can have at most {MAX_THREADS} articles")));
    }
    let user = user_rect(doc, &pages, page, rect)?;
    let mut t = doc.new_dict()?;
    put_name(&mut t, "Type", "Thread")?;
    write_info(doc, &mut t, &Info { title: title.into(), ..Info::default() })?;
    let thread = doc.add_object(&t)?;
    let mut cat = doc.catalog()?;
    match get(&cat, "Threads").filter(|a| a.is_array().unwrap_or(false)) {
        Some(mut a) => a.array_push(thread.share())?,
        None => {
            let mut a = doc.new_array()?;
            a.array_push(thread.share())?;
            cat.dict_put("Threads", a)?;
        }
    }
    commit(doc, &pages, &thread, &[], vec![Entry { obj: None, page, user }])
}

/// A bead after `after` (the end of the article when none).
pub fn add_bead(doc: &mut PdfDocument, thread: i32, after: Option<i32>, page: u32, rect: [f32; 4]) -> Result<Vec<u32>> {
    let pages = Pages::of(doc)?;
    let l = load(doc, &pages, thread)?;
    let mut entries = l.entries()?;
    if entries.len() >= MAX_BEADS {
        return Err(bad(format!("an article can have at most {MAX_BEADS} beads")));
    }
    let at = match after {
        Some(b) => l.bead(b)? + 1,
        None => entries.len(),
    };
    let user = user_rect(doc, &pages, page, rect)?;
    entries.insert(at, Entry { obj: None, page, user });
    commit(doc, &pages, &l.obj, &l.ring.beads, entries)
}

/// Moves or resizes a bead (and puts it on another page when `page` differs).
pub fn move_bead(doc: &mut PdfDocument, thread: i32, bead: i32, page: u32, rect: [f32; 4]) -> Result<Vec<u32>> {
    let pages = Pages::of(doc)?;
    let l = load(doc, &pages, thread)?;
    let mut entries = l.entries()?;
    let i = l.bead(bead)?;
    entries[i].page = page;
    entries[i].user = user_rect(doc, &pages, page, rect)?;
    commit(doc, &pages, &l.obj, &l.ring.beads, entries)
}

/// Takes a bead out; the last one takes the article with it.
pub fn delete_bead(doc: &mut PdfDocument, thread: i32, bead: i32) -> Result<Vec<u32>> {
    let pages = Pages::of(doc)?;
    let l = load(doc, &pages, thread)?;
    let mut entries = l.entries()?;
    let i = l.bead(bead)?;
    if entries.len() == 1 {
        return delete_thread(doc, thread);
    }
    entries.remove(i);
    commit(doc, &pages, &l.obj, &l.ring.beads, entries)
}

/// Removes the article, damaged or not, and its beads from the pages' /B.
pub fn delete_thread(doc: &mut PdfDocument, thread: i32) -> Result<Vec<u32>> {
    let pages = Pages::of(doc)?;
    let l = load(doc, &pages, thread)?;
    unlist(doc, |i| i != thread)?;
    let mut touched = commit(doc, &pages, &l.obj, &l.ring.beads, Vec::new())?;
    if l.ring.damage().is_some() {
        // Beads past the break are not in the ring but may still be on a page's /B.
        for p in 0..pages.nums.len() as u32 {
            let mut page = doc.find_page(p as i32)?;
            let Some(cur) = get(&page, "B").filter(|c| c.is_array().unwrap_or(false)) else { continue };
            let mut keep = doc.new_array()?;
            let mut dropped = false;
            for i in 0..cur.len()?.min(MAX_BEADS * 2) {
                let Some(item) = cur.get_array(i as i32)? else { continue };
                let ours = item.resolve()?.and_then(|b| get(&b, "T")).is_some_and(|t| indirect_num(&t) == Some(thread));
                if ours {
                    dropped = true;
                } else {
                    keep.array_push(item.share())?;
                }
            }
            if dropped {
                if keep.len()? == 0 {
                    page.dict_delete("B")?;
                } else {
                    page.dict_put("B", keep)?;
                }
                touched.push(p);
            }
        }
        touched.sort_unstable();
        touched.dedup();
    }
    Ok(touched)
}

/// The info as it is kept: no control characters, trimmed, at most 1000 characters each.
pub fn clean(info: &Info) -> Info {
    Info { title: clip(&info.title), subject: clip(&info.subject), author: clip(&info.author), keywords: clip(&info.keywords) }
}

fn write_info(doc: &PdfDocument, thread: &mut PdfObject, info: &Info) -> Result<()> {
    let info = clean(info);
    let fields = [("Title", &info.title), ("Subject", &info.subject), ("Author", &info.author), ("Keywords", &info.keywords)];
    let mut d = doc.new_dict()?;
    let mut any = false;
    for (k, v) in fields {
        if !v.is_empty() {
            put_str(&mut d, k, v)?;
            any = true;
        }
    }
    if any {
        thread.dict_put("I", d)?;
    } else {
        thread.dict_delete("I")?;
    }
    Ok(())
}

/// Title, subject, author and keywords.
pub fn set_info(doc: &mut PdfDocument, thread: i32, info: &Info) -> Result<Vec<u32>> {
    let pages = Pages::of(doc)?;
    let l = load(doc, &pages, thread)?;
    let mut t = l.obj.resolve()?.unwrap_or_else(|| l.obj.share());
    write_info(doc, &mut t, info)?;
    Ok(Vec::new())
}

/// Moves the article to position `to` of /Threads.
pub fn reorder(doc: &mut PdfDocument, thread: i32, to: usize) -> Result<Vec<u32>> {
    let objs = thread_objs(doc)?;
    let from = objs.iter().position(|(i, _)| *i == thread).ok_or_else(|| bad("that article is no longer in the document"))?;
    let mut order: Vec<PdfObject> = objs.iter().map(|(_, o)| o.share()).collect();
    let moved = order.remove(from);
    order.insert(to.min(order.len()), moved);
    let mut arr = doc.new_array()?;
    for o in order {
        arr.array_push(o)?;
    }
    doc.catalog()?.dict_put("Threads", arr)?;
    Ok(Vec::new())
}

/// After pages were deleted: beads whose page is gone are unlinked, and an article left with no beads
/// goes. Runs in the same edit step as the delete. The beads that are still on a page keep their order.
pub fn after_delete(doc: &mut PdfDocument) -> Result<()> {
    let objs = thread_objs(doc)?;
    if objs.is_empty() {
        return Ok(());
    }
    let pages = Pages::of(doc)?;
    let mut gone: Vec<i32> = Vec::new();
    for (id, obj) in &objs {
        let ring = walk(&obj.resolve()?.unwrap_or_else(|| obj.share()), *id, &pages)?;
        if !ring.beads.iter().any(|b| b.flaw == Some(Flaw::NoPage)) {
            continue;
        }
        let entries: Vec<Entry> = ring.beads.iter().filter(|b| b.flaw.is_none()).filter_map(Entry::of).collect();
        if entries.is_empty() {
            gone.push(*id);
        }
        commit(doc, &pages, obj, &ring.beads, entries)?;
    }
    if !gone.is_empty() {
        unlist(doc, |i| !gone.contains(&i))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
