//! Combine files: PDFs (each with its own page range) become one new PDF, with a bookmark per file,
//! each file's own outline kept under it, an optional contents page, and form fields kept (a field
//! whose name is taken is renamed "name (2)" as Acrobat does).
//!
//! [`inspect`] and [`run`] share one preparation, so what the sheet predicts ("Combines 4 files,
//! 37 pages, 4 bookmarks") is what the run writes. Files that cannot be taken (password protected,
//! no copying allowed, unreadable) are left out with a reason; they never stop the others.
//!
//! A locked source goes in when its password came with the request (`SourceSpec::password`, held in
//! memory only: a `Secret` zeroes itself when dropped and is never serialised or printed). The result
//! has no password. With `keep_labels`, each source's `/PageLabels` travel with its pages (PDF 32000
//! 12.4.2): a source without labels gets plain decimal numbers that carry on the combined page numbers.
//!
//! The result is written to a new file; no open document is touched.

use super::labels::{self, LabelRange, LabelStyle};
use super::{Result, bad};
use crate::batch::action::Secret;
use crate::engine::edit::{OutNode, RawOut, Readback, graft_pages_ex, open_source, page_size, read_outline, rewrite_opts, write_atomic, write_doc};
use crate::engine::{Engine, EngineError};
use mupdf::pdf::{PdfDocument, PdfObject};
use mupdf::{Font, Size};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One file of the combination, in the order it comes. `path` is resolved from a token by the caller.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SourceSpec {
    pub path: String,
    /// "1-3, 6, 9-7" (1-based, in the order written); none or empty: every page.
    #[serde(default)]
    pub range: String,
    /// The bookmark's title; none: the file's name without .pdf.
    #[serde(default)]
    pub title: Option<String>,
    /// The open password of a locked file, typed in the sheet; none for a file that has none.
    #[serde(default)]
    pub password: Option<Secret>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CombineOptions {
    /// One bookmark per file, at its first page.
    pub bookmarks: bool,
    /// Each file's own bookmarks, kept (under its bookmark when there is one).
    pub keep_outlines: bool,
    /// A contents page in front, one line per file with its page number and a link.
    pub contents_page: bool,
    /// Each source's page labels, carried over to the pages it gives.
    #[serde(default = "yes")]
    pub keep_labels: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum FileState {
    /// Goes into the result.
    Ready,
    /// Needs a password: left out.
    Encrypted,
    /// The owner allows no copying or page changes: left out.
    NoCopy,
    /// Not a readable PDF: left out.
    Unreadable,
    /// The range does not fit the file: the run is refused until it is fixed.
    BadRange,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FilePlan {
    pub name: String,
    pub title: String,
    pub state: FileState,
    /// Why it is not Ready.
    pub message: Option<String>,
    /// Pages of the file.
    pub pages: u32,
    /// Pages that go into the result.
    pub used: u32,
    pub has_outline: bool,
    pub has_fields: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub files: Vec<FilePlan>,
    /// Pages of the result (the contents pages included).
    pub pages: u32,
    pub contents_pages: u32,
    pub bookmarks: u32,
    pub left_out: u32,
    pub can_run: bool,
    /// "Combines 4 files, 37 pages, 4 bookmarks".
    pub summary: String,
}

/// What a run wrote.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub plan: Plan,
    pub path: String,
}

/// A file that is going in: opened only when its turn comes in `run`, so the files not reached yet are
/// not held in memory.
struct Source {
    path: String,
    pages: Vec<u32>,
    title: String,
    /// The file's path, length and time as they were when the plan was made: a file that differs when its
    /// turn comes is not the one that was planned.
    key: Option<FactsKey>,
    password: Option<Secret>,
}

const TOC_SIZE: f32 = 12.0;
const TOC_LEADING: f32 = 22.0;
const TOC_MARGIN: f32 = 72.0;
const TOC_HEAD: f32 = 56.0;

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn count_nodes(n: &[RawOut]) -> u32 {
    n.iter().map(|o| 1 + count_nodes(&o.children)).sum()
}

fn out_nodes(n: &[OutNode]) -> u32 {
    n.iter().map(|o| 1 + out_nodes(&o.children)).sum()
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map_or_else(|| p.to_string(), |f| f.to_string_lossy().into_owned())
}

fn stem(p: &str) -> String {
    let n = file_name(p);
    n.strip_suffix(".pdf").or_else(|| n.strip_suffix(".PDF")).map_or(n.clone(), str::to_string)
}

/// "1-3, 6, 9-7": 1-based, in the order written, a backwards range reads backwards, "5-" runs to the
/// end. Empty: every page.
pub fn parse_order(text: &str, count: u32, name: &str) -> Result<Vec<u32>> {
    if text.trim().is_empty() || text.trim().eq_ignore_ascii_case("all") {
        return Ok((0..count).collect());
    }
    let out_of = |p: u32| bad(format!("Page {p} is not in {name}, which has {}", plural(count, "page", "pages")));
    let mut out = Vec::new();
    for part in text.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let num = |s: &str| s.trim().parse::<u32>().map_err(|_| bad(format!("“{part}” is not a page range. Write it like 1-3, 6, 9-7.")));
        let (a, b) = match part.split_once('-') {
            Some((a, b)) if b.trim().is_empty() => (num(a)?, count),
            Some((a, b)) => (num(a)?, num(b)?),
            None => {
                let n = num(part)?;
                (n, n)
            }
        };
        for p in [a, b] {
            if p < 1 || p > count {
                return Err(out_of(p));
            }
        }
        if a <= b {
            out.extend((a - 1)..b);
        } else {
            out.extend(((b - 1)..a).rev());
        }
    }
    if out.is_empty() {
        return Err(bad("the page range is empty"));
    }
    Ok(out)
}

/// Lines of a contents page of height `h`.
fn toc_per_page(h: f32) -> usize {
    (((h - 2.0 * TOC_MARGIN - TOC_HEAD) / TOC_LEADING).floor() as usize).max(1)
}

/// The page size of the contents pages: the first page's, unless that is too small to hold a list.
fn toc_size(first: [f32; 2]) -> (f32, f32) {
    if first[0] >= 300.0 && first[1] >= 400.0 { (first[0], first[1]) } else { (612.0, 792.0) }
}

/// What is known about one file without its range: enough to plan with, so a file that has not
/// changed is not parsed again for every keystroke in the sheet.
#[derive(Clone)]
struct Facts {
    state: FileState,
    message: Option<String>,
    pages: u32,
    has_outline: bool,
    has_fields: bool,
    /// Bookmarks the file's own outline holds.
    outline_nodes: u32,
    /// The first page's size in points.
    first_size: [f32; 2],
    /// The answer depends on the file's bytes alone, so it may be kept while size and time are the same.
    /// An open or read error may be a lock or a hiccup that passes: asked again, not remembered.
    stable: bool,
}

/// A file that is left out for what it is (password, no copying, no pages).
fn left_out(state: FileState, message: String) -> Facts {
    Facts { state, message: Some(message), pages: 0, has_outline: false, has_fields: false, outline_nodes: 0, first_size: [0.0, 0.0], stable: true }
}

/// A file that could not be read: left out now, asked again next time.
fn failed(message: String) -> Facts {
    Facts { stable: false, ..left_out(FileState::Unreadable, message) }
}

/// Opens `path` to graft pages from. `Err` holds why the file cannot go in: its state and a reason that
/// reads on after the file name.
fn open_for_graft(path: &str, password: Option<&str>) -> std::result::Result<PdfDocument, (FileState, String)> {
    let name = file_name(path);
    let src = match open_source(path, password) {
        Ok(d) => d,
        Err(EngineError::NeedsPassword) => return Err((FileState::Encrypted, format!("{name} is password protected"))),
        Err(EngineError::BadPassword) => return Err((FileState::Encrypted, format!("{name} does not open with the password given"))),
        Err(e) => return Err((FileState::Unreadable, format!("{name} could not be read ({e})"))),
    };
    if !crate::protect::Rights::from_p(src.permissions().bits() as i32).may_extract() {
        return Err((FileState::NoCopy, format!("{name} doesn't allow copying or page changes")));
    }
    Ok(src)
}

/// What planning needs from an opened file; `None` when it has no pages.
fn read_facts(src: &PdfDocument) -> Result<Option<Facts>> {
    let count = src.page_count()?.max(0) as u32;
    if count == 0 {
        return Ok(None);
    }
    let outline = read_outline(src)?;
    let has_outline = src.catalog()?.get_dict("Outlines")?.is_some_and(|o| !o.is_null().unwrap_or(true) && o.get_dict("First").ok().flatten().is_some());
    let has_fields = src.catalog()?.get_dict("AcroForm")?.and_then(|f| f.get_dict("Fields").ok().flatten()).is_some_and(|f| f.len().unwrap_or(0) > 0);
    let first_size = page_size(src, 0)?;
    Ok(Some(Facts { state: FileState::Ready, message: None, pages: count, has_outline, has_fields, outline_nodes: count_nodes(&outline), first_size, stable: true }))
}

/// Opens `path`, reads what planning needs and lets the document go again. A file that cannot be read
/// (at any step) is left out on its own: it never fails the whole look.
fn facts_of(path: &str, password: Option<&str>) -> Facts {
    let name = file_name(path);
    let src = match open_for_graft(path, password) {
        Ok(d) => d,
        Err((FileState::Unreadable, why)) => return failed(format!("{why}, so it is left out.")),
        Err((state, why)) => return left_out(state, format!("{why}, so it is left out.")),
    };
    match read_facts(&src) {
        Ok(Some(f)) => f,
        Ok(None) => left_out(FileState::Unreadable, format!("{name} has no pages, so it is left out.")),
        Err(e) => failed(format!("{name} could not be read ({e}), so it is left out.")),
    }
}

type FactsKey = (String, u64, Option<std::time::SystemTime>);

/// Facts of files seen by `inspect`, valid while the file's size and time are the same.
fn facts_cache() -> &'static std::sync::Mutex<std::collections::HashMap<FactsKey, Facts>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<FactsKey, Facts>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn facts_key(path: &str) -> Option<FactsKey> {
    let m = std::fs::metadata(path).ok()?;
    Some((path.to_string(), m.len(), m.modified().ok()))
}

/// The cached facts of `path`, or read now (and kept for the next look). A file opened with a password is
/// never cached: the key does not hold the password, and what it opened with must not answer for a look
/// that has none.
fn facts_cached(path: &str, password: Option<&str>) -> Facts {
    if password.is_some() {
        return facts_of(path, password);
    }
    let key = facts_key(path);
    if let Some(k) = &key {
        if let Some(f) = facts_cache().lock().ok().and_then(|c| c.get(k).cloned()) {
            return f;
        }
    }
    let facts = facts_of(path, None);
    if !facts.stable {
        return facts;
    }
    if let (Some(k), Ok(mut c)) = (key, facts_cache().lock()) {
        if c.len() >= 256 {
            c.clear();
        }
        c.insert(k, facts.clone());
    }
    facts
}

fn pw(s: &SourceSpec) -> Option<&str> {
    s.password.as_ref().map(Secret::as_str)
}

/// Decides what happens to every file, and what the result holds. `orders[i]` is the pages of file
/// `i` in the order they go in, for the files that are going in.
fn plan_from(specs: &[SourceSpec], opt: CombineOptions, facts: &[Facts]) -> Result<(Plan, Vec<Option<Vec<u32>>>)> {
    if specs.is_empty() {
        return Err(bad("add at least one file to combine"));
    }
    let mut files = Vec::new();
    let mut orders: Vec<Option<Vec<u32>>> = Vec::new();
    for (s, f) in specs.iter().zip(facts) {
        let name = file_name(&s.path);
        let title = s.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| stem(&s.path));
        let mut plan = FilePlan {
            name: name.clone(),
            title,
            state: f.state.clone(),
            message: f.message.clone(),
            pages: f.pages,
            used: 0,
            has_outline: f.has_outline,
            has_fields: f.has_fields,
        };
        if f.state != FileState::Ready {
            files.push(plan);
            orders.push(None);
            continue;
        }
        match parse_order(&s.range, f.pages, &name) {
            Ok(pages) => {
                plan.used = pages.len() as u32;
                files.push(plan);
                orders.push(Some(pages));
            }
            Err(e) => {
                plan.state = FileState::BadRange;
                plan.message = Some(e.to_string());
                files.push(plan);
                orders.push(None);
            }
        }
    }

    let ready: Vec<usize> = files.iter().enumerate().filter(|(_, f)| f.state == FileState::Ready).map(|(i, _)| i).collect();
    let body: u32 = ready.iter().map(|&i| files[i].used).sum();
    let toc_pages = match (opt.contents_page, ready.first()) {
        (true, Some(&first)) => ready.len().div_ceil(toc_per_page(toc_size(facts[first].first_size).1)) as u32,
        _ => 0,
    };
    let nested: u32 = if opt.keep_outlines { ready.iter().map(|&i| facts[i].outline_nodes).sum() } else { 0 };
    let per_file = if opt.bookmarks { ready.len() as u32 } else { 0 };
    // A "Contents" bookmark leads the list when there are bookmarks and a contents page.
    let lead = u32::from(toc_pages > 0 && per_file + nested > 0);
    let bookmarks = per_file + nested + lead;
    let left_out = files.iter().filter(|f| matches!(f.state, FileState::Encrypted | FileState::NoCopy | FileState::Unreadable)).count() as u32;
    let bad_range = files.iter().any(|f| f.state == FileState::BadRange);
    let can_run = !ready.is_empty() && !bad_range;
    let pages = body + toc_pages;
    let mut summary = format!("Combines {}, {}", plural(ready.len() as u32, "file", "files"), plural(pages, "page", "pages"));
    if bookmarks > 0 {
        summary.push_str(&format!(", {}", plural(bookmarks, "bookmark", "bookmarks")));
    }
    if toc_pages > 0 {
        summary.push_str(&format!(", {}", if toc_pages == 1 { "a contents page".to_string() } else { format!("{toc_pages} contents pages") }));
    }
    if left_out > 0 {
        summary.push_str(&format!(". {} left out", plural(left_out, "file is", "files are")));
    }
    if ready.is_empty() {
        summary = "Nothing to combine: every file is left out".into();
    }
    Ok((Plan { files, pages, contents_pages: toc_pages, bookmarks, left_out, can_run, summary }, orders))
}

/// Reads every file for a run, one at a time (each is let go before the next is opened), and decides what
/// happens to it. The third part is the first page's size of the first file that goes in.
fn prepare(specs: &[SourceSpec], opt: CombineOptions) -> Result<(Plan, Vec<Option<Source>>, Option<[f32; 2]>)> {
    if specs.is_empty() {
        return Err(bad("add at least one file to combine"));
    }
    let _ = mupdf::Context::get();
    // each key is taken before its file is read, so a change during the read shows up at the run
    let seen: Vec<(Option<FactsKey>, Facts)> = specs.iter().map(|s| (facts_key(&s.path), facts_of(&s.path, pw(s)))).collect();
    let facts: Vec<Facts> = seen.iter().map(|(_, f)| f.clone()).collect();
    let (plan, orders) = plan_from(specs, opt, &facts)?;
    let mut first = None;
    let sources = specs
        .iter()
        .zip(orders)
        .zip(&plan.files)
        .zip(seen)
        .map(|(((s, order), f), (key, facts))| {
            let pages = order?;
            first.get_or_insert(facts.first_size);
            Some(Source { path: s.path.clone(), pages, title: f.title.clone(), key, password: s.password.clone() })
        })
        .collect();
    Ok((plan, sources, first))
}

/// What a run would do, without writing anything. A file that has not changed since the last look
/// is not parsed again.
pub fn inspect(specs: &[SourceSpec], opt: CombineOptions) -> Result<Plan> {
    if specs.is_empty() {
        return Err(bad("add at least one file to combine"));
    }
    let _ = mupdf::Context::get();
    let facts: Vec<Facts> = specs.iter().map(|s| facts_cached(&s.path, pw(s))).collect();
    plan_from(specs, opt, &facts).map(|(p, _)| p)
}

// ---- writing the result ------------------------------------------------------------------------

/// A destination at the top of `page`, zoom left as it is.
fn page_dest(doc: &PdfDocument, page: &PdfObject) -> Result<PdfObject> {
    let mut d = doc.new_array()?;
    d.array_push(page.clone())?;
    d.array_push(PdfObject::new_name("XYZ")?)?;
    for _ in 0..3 {
        d.array_push(PdfObject::new_null())?;
    }
    Ok(d)
}

fn bookmark_node(title: &str, dest: Option<PdfObject>, children: Vec<OutNode>) -> OutNode {
    OutNode { title: title.to_string(), dest, uri: None, open: false, children }
}

/// Writes `nodes` as the document's /Outlines; returns the number of items.
fn write_outline(doc: &mut PdfDocument, nodes: &[OutNode]) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    let root = doc.new_dict()?;
    let mut root = doc.add_object(&root)?;
    root.dict_put("Type", PdfObject::new_name("Outlines")?)?;
    if let Some((first, last, count)) = outline_level(doc, &root, nodes)? {
        root.dict_put("First", first)?;
        root.dict_put("Last", last)?;
        root.dict_put("Count", PdfObject::new_int(count)?)?;
    }
    let mut catalog = doc.catalog()?;
    catalog.dict_put("Outlines", root)?;
    catalog.dict_put("PageMode", PdfObject::new_name("UseOutlines")?)?;
    Ok(())
}

/// One level of items under `parent`: first, last, and how many are visible.
fn outline_level(doc: &mut PdfDocument, parent: &PdfObject, nodes: &[OutNode]) -> Result<Option<(PdfObject, PdfObject, i32)>> {
    let mut refs: Vec<PdfObject> = Vec::new();
    let mut visible = 0i32;
    for n in nodes {
        let mut d = doc.new_dict()?;
        d.dict_put("Title", PdfObject::new_string(&n.title)?)?;
        d.dict_put("Parent", parent.clone())?;
        if let Some(dest) = &n.dest {
            d.dict_put("Dest", dest.clone())?;
        } else if let Some(u) = &n.uri {
            let mut a = doc.new_dict()?;
            a.dict_put("S", PdfObject::new_name("URI")?)?;
            a.dict_put("URI", PdfObject::new_string(u)?)?;
            d.dict_put("A", a)?;
        }
        let mut item = doc.add_object(&d)?;
        visible += 1;
        if let Some((first, last, count)) = outline_level(doc, &item, &n.children)? {
            item.dict_put("First", first)?;
            item.dict_put("Last", last)?;
            item.dict_put("Count", PdfObject::new_int(if n.open { count } else { -count })?)?;
            if n.open {
                visible += count;
            }
        }
        refs.push(item);
    }
    for i in 0..refs.len() {
        if i > 0 {
            let prev = refs[i - 1].clone();
            refs[i].dict_put("Prev", prev)?;
        }
        if i + 1 < refs.len() {
            let next = refs[i + 1].clone();
            refs[i].dict_put("Next", next)?;
        }
    }
    Ok(match (refs.first(), refs.last()) {
        (Some(f), Some(l)) => Some((f.clone(), l.clone(), visible)),
        _ => None,
    })
}

/// Helvetica as an embedded font, for the contents pages: every character it has.
pub(super) struct Face {
    pub(super) font: Font,
}

impl Face {
    pub(super) fn gid(&self, c: char) -> u16 {
        let g = self.font.encode_character(c as i32).unwrap_or(0);
        if g > 0 { g as u16 } else { self.font.encode_character('?' as i32).unwrap_or(0).max(0) as u16 }
    }

    pub(super) fn width(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.font.advance_glyph(self.gid(c) as i32).unwrap_or(0.5)).sum::<f32>() * size
    }

    pub(super) fn hex(&self, s: &str) -> String {
        s.chars().filter(|c| !c.is_control()).map(|c| format!("{:04x}", self.gid(c))).collect()
    }

    /// `s` cut with an ellipsis to fit `max` points.
    pub(super) fn fit(&self, s: &str, size: f32, max: f32) -> String {
        if self.width(s, size) <= max {
            return s.to_string();
        }
        let mut t: String = s.chars().collect();
        while !t.is_empty() && self.width(&format!("{t}…"), size) > max {
            t.pop();
        }
        format!("{t}…")
    }
}

/// Contents pages in front of the document: one line per `(title, destination page, number)`.
fn write_contents(doc: &mut PdfDocument, entries: &[(String, PdfObject, u32)], (w, h): (f32, f32)) -> Result<u32> {
    let face = Face { font: Font::new("Helvetica")? };
    let font_obj = doc.add_font(&face.font)?;
    let per = toc_per_page(h);
    let mut made = 0u32;
    for (n, chunk) in entries.chunks(per).enumerate() {
        let mut c = String::new();
        if n == 0 {
            c.push_str(&format!("BT /F1 20 Tf {TOC_MARGIN} {:.2} Td <{}> Tj ET\n", h - TOC_MARGIN - 20.0, face.hex("Contents")));
        }
        let mut links = doc.new_array()?;
        let mut y = h - TOC_MARGIN - TOC_HEAD;
        for (title, dest, number) in chunk {
            let num = number.to_string();
            let numw = face.width(&num, TOC_SIZE);
            let room = w - 2.0 * TOC_MARGIN - numw - 24.0;
            let shown = face.fit(title, TOC_SIZE, room);
            let tw = face.width(&shown, TOC_SIZE);
            c.push_str(&format!("BT /F1 {TOC_SIZE} Tf {TOC_MARGIN} {y:.2} Td <{}> Tj ET\n", face.hex(&shown)));
            c.push_str(&format!("BT /F1 {TOC_SIZE} Tf {:.2} {y:.2} Td <{}> Tj ET\n", w - TOC_MARGIN - numw, face.hex(&num)));
            let (x0, x1) = (TOC_MARGIN + tw + 8.0, w - TOC_MARGIN - numw - 8.0);
            if x1 > x0 {
                c.push_str(&format!("q 0.6 G 0.8 w 1 J [0.1 4] 0 d {x0:.2} {:.2} m {x1:.2} {:.2} l S Q\n", y + 2.0, y + 2.0));
            }
            let mut link = doc.new_dict()?;
            link.dict_put("Type", PdfObject::new_name("Annot")?)?;
            link.dict_put("Subtype", PdfObject::new_name("Link")?)?;
            let mut rect = doc.new_array()?;
            for v in [TOC_MARGIN, y - 5.0, w - TOC_MARGIN, y + TOC_SIZE + 3.0] {
                rect.array_push(PdfObject::new_real(v)?)?;
            }
            link.dict_put("Rect", rect)?;
            let mut border = doc.new_array()?;
            for _ in 0..3 {
                border.array_push(PdfObject::new_int(0)?)?;
            }
            link.dict_put("Border", border)?;
            link.dict_put("Dest", page_dest(doc, dest)?)?;
            links.array_push(doc.add_object(&link)?)?;
            y -= TOC_LEADING;
        }
        let mut page = doc.new_page_at(n as i32, Size::new(w, h))?;
        let mut fonts = doc.new_dict()?;
        fonts.dict_put("F1", font_obj.clone())?;
        let mut res = doc.new_dict()?;
        res.dict_put("Font", fonts)?;
        let mut obj = page.object();
        obj.dict_put("Resources", res)?;
        obj.dict_put("Annots", links)?;
        page.insert_contents(doc, c.as_bytes(), true)?;
        made += 1;
    }
    Ok(made)
}

/// The new document with every source's pages in it.
struct Grafted {
    dst: PdfDocument,
    /// The first page of each file that went in, in the new document.
    tops: Vec<PdfObject>,
    /// The outline each of them brought.
    kept: Vec<Vec<OutNode>>,
    /// (title, pages used) of every file that went in
    done: Vec<(String, u32)>,
    /// The label sections of the result, in its final page numbers (the contents pages counted); empty
    /// when the labels are not kept.
    labels: Vec<LabelRange>,
}

const CHANGED: &str = "Add the files again to see the new plan.";

/// Grafts the planned sources into a new document, each opened when its turn comes and closed as soon as
/// its pages are in, so big files are never all held at once. A file that is not the one that was planned
/// (its length or time differ from when the plan was made, or its pages no longer fit) stops the run.
fn graft_sources(sources: Vec<Option<Source>>, opt: CombineOptions, toc_pages: u32) -> Result<Grafted> {
    let mut dst = PdfDocument::new();
    let mut tops: Vec<PdfObject> = Vec::new();
    let mut kept: Vec<Vec<OutNode>> = Vec::new();
    let mut done: Vec<(String, u32)> = Vec::new();
    let mut sections: Vec<LabelRange> = Vec::new();
    for s in sources.into_iter().flatten() {
        let doc = open_for_graft(&s.path, s.password.as_ref().map(Secret::as_str)).map_err(|(_, why)| bad(format!("{why} now, so it can't be combined. {CHANGED}")))?;
        // Checked right after the open: the handle is of the file this key was read from, or the key differs.
        let changed = || bad(format!("{} has changed since it was added, so it can't be combined. {CHANGED}", file_name(&s.path)));
        if facts_key(&s.path) != s.key {
            return Err(changed());
        }
        // backstop: its pages must still be there
        let count = doc.page_count()?.max(0) as u32;
        if s.pages.iter().any(|&p| p >= count) {
            return Err(changed());
        }
        let at = dst.page_count()?;
        let g = graft_pages_ex(&mut dst, &doc, &s.pages, at, opt.keep_outlines)?;
        if opt.keep_labels {
            // A source whose labels cannot be read counts as one without: they never stop a combine.
            // (sections that only spell out plain decimal numbering are no labels: they carry on)
            let own = labels::normalize(labels::read(&doc).unwrap_or_default(), count);
            for (j, &p) in s.pages.iter().enumerate() {
                let r = carried(&own, p, toc_pages + at as u32 + j as u32);
                if !sections.last().is_some_and(|l| labels::continues(l, &r)) {
                    sections.push(r);
                }
            }
        }
        tops.push(g.pages[0].clone());
        kept.push(g.outline);
        done.push((s.title, s.pages.len() as u32));
    }
    Ok(Grafted { dst, tops, kept, done, labels: sections })
}

/// The label of result page `to` when it shows page `p` of a source whose sections are `own`: that
/// page's own label, or (no sections) a plain decimal number on the combined numbering, as Acrobat does.
fn carried(own: &[LabelRange], p: u32, to: u32) -> LabelRange {
    match own.partition_point(|s| s.page <= p).checked_sub(1).map(|i| &own[i]) {
        // PDF writes the start as a 32-bit integer
        Some(s) => LabelRange { page: to, style: s.style, prefix: s.prefix.clone(), start: s.start.saturating_add(p - s.page).min(i32::MAX as u32) },
        None => LabelRange { page: to, style: LabelStyle::Decimal, prefix: String::new(), start: to.saturating_add(1) },
    }
}

/// Combines the files into a new PDF at `out` (replacing it) and reports what it holds.
pub fn run(eng: &Engine, specs: &[SourceSpec], opt: CombineOptions, out: &str) -> Result<Report> {
    eng.check_not_open(out, None)?;
    let (plan, sources, first) = prepare(specs, opt)?;
    if let Some(f) = plan.files.iter().find(|f| f.state == FileState::BadRange) {
        return Err(bad(f.message.clone().unwrap_or_else(|| "a page range does not fit its file".into())));
    }
    if !plan.can_run {
        return Err(bad(match plan.files.iter().find_map(|f| f.message.clone()) {
            Some(m) if plan.files.len() == 1 => m,
            _ => "Every file is left out (password protected, no copying allowed or unreadable), so there is nothing to combine.".into(),
        }));
    }
    // The contents pages take the size of the first file's first page, as `inspect` assumed.
    let Some(first) = first else {
        return Err(bad("there is nothing to combine"));
    };
    let Grafted { mut dst, tops, kept, done, labels: mut sections } = graft_sources(sources, opt, plan.contents_pages)?;
    // Contents pages go in front; the page numbers they show count them.
    let mut toc_dest: Option<PdfObject> = None;
    if opt.contents_page {
        let toc_pages = plan.contents_pages;
        let mut entries = Vec::new();
        let mut at = toc_pages;
        for ((title, pages), top) in done.iter().zip(&tops) {
            entries.push((title.clone(), top.clone(), at + 1));
            at += pages;
        }
        let made = write_contents(&mut dst, &entries, toc_size(first))?;
        debug_assert_eq!(made, toc_pages);
        if made > 0 {
            toc_dest = Some(page_dest(&dst, &dst.find_page(0)?)?);
        }
    }
    // Bookmarks.
    let mut top_nodes: Vec<OutNode> = Vec::new();
    for (((title, _), top), nested) in done.iter().zip(&tops).zip(kept) {
        let nested = if opt.keep_outlines { nested } else { Vec::new() };
        if opt.bookmarks {
            top_nodes.push(bookmark_node(title, Some(page_dest(&dst, top)?), nested));
        } else {
            top_nodes.extend(nested);
        }
    }
    if !top_nodes.is_empty() {
        if let Some(d) = toc_dest {
            top_nodes.insert(0, bookmark_node("Contents", Some(d), Vec::new()));
        }
    }
    // What the plan promised is what is written; a file changed under the run is stopped before this
    // (`graft_sources`), so this is the backstop and never a panic.
    if out_nodes(&top_nodes) != plan.bookmarks {
        return Err(bad("a file changed while it was being combined. Add the files again to see the new plan."));
    }
    write_outline(&mut dst, &top_nodes)?;
    // Written last: adding the contents pages made MuPDF number the pages ("1", "1", "2" ...). The
    // contents pages are plain decimal; a result with nothing but decimal numbering has no labels.
    let total = dst.page_count()?.max(0) as u32;
    if plan.contents_pages > 0 && opt.keep_labels {
        sections.insert(0, LabelRange { page: 0, style: LabelStyle::Decimal, prefix: String::new(), start: 1 });
    }
    labels::write(&mut dst, &labels::normalize(sections, total))?;

    let bytes = write_doc(&dst, rewrite_opts(), 1 << 20)?;
    drop(dst);
    write_atomic(Path::new(out), &bytes, total, Readback::default())?;
    Ok(Report { plan, path: out.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::fixture::{self, Fixture};
    use std::time::{Duration, SystemTime};

    const OPT: CombineOptions = CombineOptions { bookmarks: true, keep_outlines: true, contents_page: false, keep_labels: true };

    fn scratch(test: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("inkwell-combine-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(d: &Path, name: &str, pages: usize, chapter_every: usize) -> String {
        let p = d.join(name);
        std::fs::write(&p, fixture::build(&Fixture { pages, chapter_every, ..Default::default() })).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn spec(path: &str) -> SourceSpec {
        SourceSpec { path: path.into(), range: String::new(), title: None, password: None }
    }

    /// Plans `specs` and grafts at once, the way a run does, returning the message when it stops.
    fn graft(specs: &[SourceSpec]) -> std::result::Result<u32, String> {
        let (plan, sources, _) = prepare(specs, OPT).map_err(|e| e.to_string())?;
        assert!(plan.can_run, "{:?}", plan.summary);
        let g = graft_sources(sources, OPT, 0).map_err(|e| e.to_string())?;
        Ok(g.dst.page_count().unwrap().max(0) as u32)
    }

    #[test]
    fn a_source_swapped_after_the_plan_is_not_combined() {
        let d = scratch("swap");
        let a = write(&d, "a.pdf", 2, 0);
        let b = write(&d, "b.pdf", 3, 1);
        let specs = [spec(&a), spec(&b)];

        // untouched: all five pages go in
        assert_eq!(graft(&specs), Ok(5));

        // replaced by a file with the same pages and no bookmarks: the plan counted bookmarks that are
        // not there any more (it used to be a debug assertion), so it stops
        let (_, sources, _) = prepare(&specs, OPT).unwrap();
        write(&d, "b.pdf", 3, 0);
        let e = graft_sources(sources, OPT, 0).err().expect("swapped file").to_string();
        assert!(e.contains("b.pdf has changed"), "{e}");

        // replaced by one with more pages
        let (_, sources, _) = prepare(&specs, OPT).unwrap();
        write(&d, "b.pdf", 6, 1);
        let e = graft_sources(sources, OPT, 0).err().expect("longer file").to_string();
        assert!(e.contains("b.pdf has changed"), "{e}");

        // the same bytes written again later: the time differs, so it is not the planned file either
        write(&d, "b.pdf", 3, 1);
        let (_, sources, _) = prepare(&specs, OPT).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&b).unwrap();
        f.set_modified(SystemTime::now() + Duration::from_secs(30)).unwrap();
        drop(f);
        let e = graft_sources(sources, OPT, 0).err().expect("touched file").to_string();
        assert!(e.contains("b.pdf has changed"), "{e}");

        // gone: says which file and that it can't be combined, no panic
        let (_, sources, _) = prepare(&specs, OPT).unwrap();
        std::fs::remove_file(&b).unwrap();
        let e = graft_sources(sources, OPT, 0).err().expect("deleted file").to_string();
        assert!(e.contains("b.pdf") && e.contains("can't be combined"), "{e}");
    }

    #[test]
    fn only_what_the_bytes_decide_is_remembered() {
        let d = scratch("cache");
        let good = write(&d, "good.pdf", 2, 0);
        let junk = d.join("junk.pdf");
        std::fs::write(&junk, b"not a pdf at all").unwrap();
        let junk = junk.to_string_lossy().into_owned();
        let remembered = |p: &str| facts_cache().lock().unwrap().contains_key(&facts_key(p).unwrap());

        assert_eq!(facts_cached(&good, None).state, FileState::Ready);
        assert!(remembered(&good));
        // an open or read error may be a lock that passes: asked again next time
        assert_eq!(facts_cached(&junk, None).state, FileState::Unreadable);
        assert!(!remembered(&junk));
    }

    #[test]
    fn ranges_keep_their_order_and_say_what_is_wrong() {
        assert_eq!(parse_order("", 4, "a.pdf").unwrap(), [0, 1, 2, 3]);
        assert_eq!(parse_order("1-2, 4", 4, "a.pdf").unwrap(), [0, 1, 3]);
        assert_eq!(parse_order("3-1, 4", 4, "a.pdf").unwrap(), [2, 1, 0, 3]);
        assert_eq!(parse_order("3-", 5, "a.pdf").unwrap(), [2, 3, 4]);
        assert_eq!(parse_order("2, 2", 5, "a.pdf").unwrap(), [1, 1]);
        let e = parse_order("1, 9", 4, "a.pdf").unwrap_err().to_string();
        assert!(e.contains("Page 9") && e.contains("a.pdf") && e.contains("4 pages"), "{e}");
        assert!(parse_order("x", 4, "a.pdf").is_err());
        assert!(parse_order("0", 4, "a.pdf").is_err());
    }
}
