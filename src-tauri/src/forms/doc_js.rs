//! Document JavaScript: every script a file carries, listed, read, checked and edited as plain text.
//!
//! The places: the named scripts of `/Names /JavaScript`, the document's actions (`/OpenAction` and the catalog's
//! `/AA` WC, WS, DS, WP, DP), the pages' actions (`/AA` O, C), and, counted and scanned but not edited here, the
//! field scripts (forms rail), link and bookmark actions. Nothing in this module runs a script: listing and reading
//! only look at the objects, the syntax check compiles in a throwaway engine that has no host functions and never
//! evaluates, and an edit writes objects. What the forms runtime itself runs, and when, is `forms/js.rs` and is not
//! changed here (docs/forms.md, "Document JavaScript").

use super::js::{MAX_DOC_SCRIPT_BYTES, MAX_SCRIPT_BYTES};
use super::{bad, decode_text, get, scan, text, Result};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine};
use crate::organize::attach::{self, Entry};
use mupdf::pdf::{PdfDocument, PdfObject};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

/// Steps listed for one action (a `/Next` chain, in order).
const MAX_STEPS: usize = 64;
/// Named scripts, pages, annotations of a page and bookmarks looked at: the rest is said to be left out.
const MAX_NAMED: usize = 4096;
const MAX_PAGES: i32 = 50_000;
const MAX_ANNOTS: usize = 2000;
const MAX_OUTLINE: usize = 50_000;
/// All the script text one listing reads, so a file of thousands of 1 MiB scripts cannot make it read gigabytes.
const SCAN_BUDGET: usize = 8 << 20;
/// Places named per capability.
const MAX_PLACES: usize = 8;
const MAX_NAME: usize = 255;

const DOC_EVENTS: [(&str, &str); 5] = [
    ("WC", "Document will close"),
    ("WS", "Document will save"),
    ("DS", "Document did save"),
    ("WP", "Document will print"),
    ("DP", "Document did print"),
];
const PAGE_EVENTS: [(&str, &str); 2] = [("O", "Page opens"), ("C", "Page closes")];
/// Triggers of an annotation or field `/AA`, as Acrobat names them.
const AA_LABELS: [(&str, &str); 14] = [
    ("K", "Keystroke"),
    ("F", "Format"),
    ("V", "Validate"),
    ("C", "Calculate"),
    ("E", "Mouse enters"),
    ("X", "Mouse exits"),
    ("D", "Mouse down"),
    ("U", "Mouse up"),
    ("Fo", "Gets focus"),
    ("Bl", "Loses focus"),
    ("PO", "Page opens"),
    ("PC", "Page closes"),
    ("PV", "Page visible"),
    ("PI", "Page hidden"),
];

// ---- what the sheet is given ---------------------------------------------------------------------

/// What one script is, without its text (read on its own, see [`source`]).
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// Bytes of source; 0 when it was not read.
    pub size: u64,
    /// Over the 1 MiB the document scripts may be: it is not shown or scanned here.
    pub too_large: bool,
    /// Not read: the listing had read its share of script text already.
    pub unread: bool,
    /// Keys of the capabilities the source mentions (see [`Capability`]).
    pub risks: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Named {
    /// `n:<name>`.
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub meta: Meta,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// `d:<event>:<i>` or `p:<page>:<event>:<i>`; `d:open:<i>` for the open action.
    pub id: String,
    pub javascript: bool,
    /// For another kind of action: what it does ("Go to a page", "Open a web address").
    pub what: String,
    #[serde(flatten)]
    pub meta: Meta,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    /// `d:<event>` (`d:open`) or `p:<page>:<event>`.
    pub id: String,
    pub label: String,
    /// 0-based page of a page action.
    pub page: Option<u32>,
    /// Empty when nothing is set.
    pub steps: Vec<Step>,
}

/// What the file has that is listed and counted here but edited elsewhere.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    /// Fields with at least one script, and the scripts they have (format, validate, calculate, keystroke, mouse...).
    pub fields: u32,
    pub field_scripts: u32,
    /// Link and other annotations with a JavaScript action.
    pub links: u32,
    /// Bookmarks with a JavaScript action.
    pub bookmarks: u32,
}

/// A thing scripts can do that reaches outside the file, found by a token scan of every script.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub key: String,
    pub label: String,
    /// Scripts that mention it.
    pub count: u32,
    /// The first few of them ("Document script \"init\"", "Field \"Total\": Calculate").
    pub places: Vec<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocJs {
    pub named: Vec<Named>,
    /// The open action, then WC, WS, DS, WP, DP: always all six.
    pub doc_actions: Vec<Slot>,
    /// Only pages that have an action.
    pub page_actions: Vec<Slot>,
    pub counts: Counts,
    pub capabilities: Vec<Capability>,
    /// Something was left out of the list or the scan (too many items, too much script text).
    pub incomplete: bool,
    /// The file is signed: an edit is saved as an update, and the signature will show the change.
    pub signed: bool,
    /// The file is certified (any `/P`): script edits are refused.
    pub certified: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub text: String,
    pub size: u64,
    pub too_large: bool,
}

/// A syntax problem: line and column are 1-based.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub ok: bool,
    pub message: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// Not compiled: over what the engine runs (256 KiB).
    pub too_large: bool,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// A named script; no `name` takes "Script n".
    Add { name: Option<String>, source: String },
    Rename { id: String, name: String },
    /// A named script's text, or the JavaScript at a step of an action (the head of an empty or non-JavaScript
    /// action is replaced by one; later steps of a chain keep their `/Next`).
    SetSource { id: String, source: String },
    /// A named script, or the whole action at a slot (its chain too).
    Delete { id: String },
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub state: EditState,
    /// The id of the script an `Add` made.
    pub id: Option<String>,
    /// Set when the file is signed and the edit changed it.
    pub warning: Option<String>,
}

// ---- reading ---------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Where {
    Open,
    Doc(&'static str),
    Page(u32, &'static str),
}

fn slot_id(w: Where) -> String {
    match w {
        Where::Open => "d:open".into(),
        Where::Doc(ev) => format!("d:{ev}"),
        Where::Page(p, ev) => format!("p:{p}:{ev}"),
    }
}

/// `d:WC:0`, `p:3:O:1`, `d:open:0`, or a slot id alone (index 0).
fn parse_ref(id: &str) -> Option<(Where, usize)> {
    let mut it = id.split(':');
    match it.next()? {
        "d" => {
            let ev = it.next()?;
            let w = if ev == "open" { Where::Open } else { Where::Doc(DOC_EVENTS.iter().find(|(e, _)| *e == ev)?.0) };
            Some((w, it.next().map_or(Some(0), |i| i.parse().ok())?))
        }
        "p" => {
            let page: u32 = it.next()?.parse().ok()?;
            let ev = it.next()?;
            let w = Where::Page(page, PAGE_EVENTS.iter().find(|(e, _)| *e == ev)?.0);
            Some((w, it.next().map_or(Some(0), |i| i.parse().ok())?))
        }
        _ => None,
    }
}

fn label_of(w: Where) -> String {
    match w {
        Where::Open => "Document opens".into(),
        Where::Doc(ev) => DOC_EVENTS.iter().find(|(e, _)| *e == ev).map_or(ev, |&(_, l)| l).to_string(),
        Where::Page(p, ev) => format!("Page {}: {}", p + 1, PAGE_EVENTS.iter().find(|(e, _)| *e == ev).map_or(ev, |&(_, l)| l).to_lowercase()),
    }
}

/// The script text one listing may still read, and whether it ran out.
struct Budget {
    left: usize,
    cut: bool,
}

enum Read {
    Text(String),
    Large,
    Unread,
    Missing,
}

/// The text of a `/JS` value (a string or a stream), at most [`MAX_DOC_SCRIPT_BYTES`] of it.
fn read_js(js: &PdfObject, budget: &mut Budget) -> Read {
    if budget.left == 0 {
        budget.cut = true;
        return Read::Unread;
    }
    let bytes = if js.is_string().unwrap_or(false) {
        match js.as_bytes() {
            Ok(b) => b,
            Err(_) => return Read::Missing,
        }
    } else if js.is_stream().unwrap_or(false) {
        match js.read_stream_capped(MAX_DOC_SCRIPT_BYTES + 1) {
            Ok(b) => b,
            Err(e) => {
                // A read that failed was still work done: it is charged like a full one, so a file of many
                // broken streams cannot make a listing read for ever.
                budget.left = budget.left.saturating_sub(MAX_DOC_SCRIPT_BYTES);
                return if e.is_stream_cap() { Read::Large } else { Read::Missing };
            }
        }
    } else {
        return Read::Missing;
    };
    budget.left = budget.left.saturating_sub(bytes.len().clamp(1, MAX_DOC_SCRIPT_BYTES));
    if bytes.len() > MAX_DOC_SCRIPT_BYTES {
        return Read::Large;
    }
    Read::Text(decode_text(&bytes))
}

struct Cap {
    key: &'static str,
    label: &'static str,
    re: Regex,
}

/// What a script may do that reaches outside the file, by token. Word boundaries, so `print(` does not match `println(`
/// or `printf(`. An approximation: the alert-in-a-loop check looks for an alert within 300 characters after a loop head.
fn caps() -> &'static [Cap] {
    static CAPS: OnceLock<Vec<Cap>> = OnceLock::new();
    CAPS.get_or_init(|| {
        let c = |key, label, pat: &str| Cap { key, label, re: Regex::new(pat).expect("a valid pattern") };
        vec![
            c("launchUrl", "Opens web addresses", r"\b(?:launchURL|getURL)\s*\("),
            c("submitForm", "Sends form data", r"\bsubmitForm\s*\("),
            c("mail", "Emails the file or its form data", r"\b(?:mailDoc|mailForm)\s*\("),
            c("print", "Prints", r"\bprint\s*\("),
            c("exportData", "Writes data or files out", r"\b(?:exportDataObject|exportAsFDF|exportAsXFDF|exportAsText|exportXFAData|saveAs)\s*\("),
            c("importData", "Reads data or files in", r"\b(?:importDataObject|importAnFDF|importTextData|importXFAData)\s*\("),
            c(
                "alertLoop",
                "Shows alerts in a loop or on a timer",
                r"\b(?:for|while)\s*\([^)]*\)[\s\S]{0,300}?\bapp\s*\.\s*alert\s*\(|\bdo\s*\{[\s\S]{0,300}?\bapp\s*\.\s*alert\s*\(|\bapp\s*\.\s*setInterval\s*\(",
            ),
        ]
    })
}

fn risks_of(src: &str) -> Vec<String> {
    caps().iter().filter(|c| c.re.is_match(src)).map(|c| c.key.to_string()).collect()
}

/// Everything one listing collects as it reads.
struct Scan {
    budget: Budget,
    found: Vec<(u32, Vec<String>)>,
    counts: Counts,
    incomplete: bool,
}

impl Scan {
    /// Reads the `/JS` of an action: its size and capabilities, and a note of where they were found.
    fn meta(&mut self, action: &PdfObject, place: impl FnOnce() -> String) -> Meta {
        let Some(js) = get(action, "JS") else { return Meta::default() };
        match read_js(&js, &mut self.budget) {
            Read::Text(t) => {
                let risks = risks_of(&t);
                if !risks.is_empty() {
                    let place = place();
                    for (i, c) in caps().iter().enumerate() {
                        if risks.iter().any(|r| r == c.key) {
                            let f = &mut self.found[i];
                            f.0 += 1;
                            if f.1.len() < MAX_PLACES {
                                f.1.push(place.clone());
                            }
                        }
                    }
                }
                Meta { size: t.len() as u64, too_large: false, unread: false, risks }
            }
            Read::Large => Meta { too_large: true, ..Meta::default() },
            Read::Unread => Meta { unread: true, ..Meta::default() },
            Read::Missing => Meta::default(),
        }
    }
}

/// Where a step hangs: the index of the step whose `/Next` holds it, and its place in that `/Next` when it is an array.
type At = Option<(usize, Option<i32>)>;

/// The action and what follows it by `/Next`, in the order they run, each with where it hangs. Each object once, at
/// most [`MAX_STEPS`].
fn walk(head: &PdfObject) -> Vec<(PdfObject, At)> {
    let mut out: Vec<(PdfObject, At)> = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(PdfObject, At)> = vec![(head.clone(), None)];
    while let Some((a, at)) = stack.pop() {
        if out.len() >= MAX_STEPS {
            break;
        }
        if a.is_indirect().unwrap_or(false) && !a.as_indirect().is_ok_and(|n| seen.insert(n)) {
            continue;
        }
        let is_dict = a.is_dict().unwrap_or(false);
        if !is_dict && !a.is_array().unwrap_or(false) {
            continue;
        }
        let me = out.len();
        if is_dict {
            if let Some(next) = get(&a, "Next") {
                if next.is_array().unwrap_or(false) {
                    for i in (0..next.len().unwrap_or(0).min(MAX_STEPS) as i32).rev() {
                        if let Some(n) = next.get_array(i).ok().flatten() {
                            stack.push((n, Some((me, Some(i)))));
                        }
                    }
                } else {
                    stack.push((next, Some((me, None))));
                }
            }
        }
        out.push((a, at));
    }
    out
}

fn chain(head: &PdfObject) -> Vec<PdfObject> {
    walk(head).into_iter().map(|(a, _)| a).collect()
}

fn is_js(a: &PdfObject) -> bool {
    a.is_dict().unwrap_or(false) && get(a, "S").and_then(|s| s.as_name().ok()).is_some_and(|n| n == b"JavaScript")
}

/// What a step that is not JavaScript does, in plain words.
fn what_of(a: &PdfObject) -> String {
    if a.is_array().unwrap_or(false) {
        return "Go to a page".into();
    }
    let s = get(a, "S").and_then(|s| s.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned()).unwrap_or_default();
    match s.as_str() {
        "GoTo" => "Go to a page".into(),
        "GoToR" => "Go to a page in another file".into(),
        "URI" => "Open a web address".into(),
        "Launch" => "Launch a file".into(),
        "Named" => format!("Menu command {}", get(a, "N").and_then(|n| n.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned()).unwrap_or_default()),
        "SubmitForm" => "Send form data".into(),
        "ResetForm" => "Reset the form".into(),
        "ImportData" => "Import form data".into(),
        "Hide" => "Show or hide a field".into(),
        "" => "Unknown action".into(),
        other => other.to_string(),
    }
}

fn steps_of(scan: &mut Scan, w: Where, head: &PdfObject) -> Vec<Step> {
    chain(head)
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let id = format!("{}:{i}", slot_id(w));
            if is_js(a) {
                let place = || label_of(w);
                Step { id, javascript: true, what: String::new(), meta: scan.meta(a, place) }
            } else {
                Step { id, javascript: false, what: what_of(a), meta: Meta::default() }
            }
        })
        .collect()
}

/// The JavaScript actions of an annotation or field dictionary: `/A` and every `/AA` trigger (with their chains).
fn holder_scripts(holder: &PdfObject) -> Vec<(String, PdfObject)> {
    let mut out = Vec::new();
    if let Some(a) = get(holder, "A") {
        out.extend(chain(&a).into_iter().filter(is_js).map(|s| ("Action".to_string(), s)));
    }
    if let Some(aa) = get(holder, "AA").filter(|a| a.is_dict().unwrap_or(false)) {
        if let Ok(it) = aa.dict_iter() {
            for (k, v) in it.filter_map(|kv| kv.ok()).take(32) {
                let key = text(&k).unwrap_or_default();
                let label = AA_LABELS.iter().find(|(e, _)| *e == key).map_or(key.clone(), |(_, l)| l.to_string());
                out.extend(chain(&v).into_iter().filter(is_js).map(|s| (label.clone(), s)));
            }
        }
    }
    out
}

/// Every script of the file: the named ones, the document and page actions in full, and the field, link and bookmark
/// scripts counted and scanned.
pub fn list(doc: &PdfDocument) -> Result<DocJs> {
    let mut sc = Scan {
        budget: Budget { left: SCAN_BUDGET, cut: false },
        found: caps().iter().map(|_| (0, Vec::new())).collect(),
        counts: Counts::default(),
        incomplete: false,
    };
    // named scripts
    let (entries, cut) = attach::tree_walk(doc, "JavaScript")?;
    let ids = attach::entry_ids(&entries);
    if cut || entries.len() > MAX_NAMED {
        sc.incomplete = true;
    }
    let mut named = Vec::new();
    for (e, id) in entries.iter().zip(ids).take(MAX_NAMED) {
        let spec = e.spec.resolve().ok().flatten().unwrap_or_else(|| e.spec.clone());
        let meta = sc.meta(&spec, || format!("Document script \"{}\"", e.key));
        named.push(Named { id, name: e.key.clone(), meta });
    }
    // In name order, as the document's scripts run.
    named.sort_by(|a, b| a.name.cmp(&b.name));
    // document actions
    let catalog = doc.catalog()?;
    let mut doc_actions = Vec::new();
    let open = get(&catalog, "OpenAction");
    let steps = open.map(|h| steps_of(&mut sc, Where::Open, &h)).unwrap_or_default();
    doc_actions.push(Slot { id: slot_id(Where::Open), label: label_of(Where::Open), page: None, steps });
    let aa = get(&catalog, "AA").filter(|a| a.is_dict().unwrap_or(false));
    for (ev, _) in DOC_EVENTS {
        let w = Where::Doc(ev);
        let steps = aa.as_ref().and_then(|a| get(a, ev)).map(|h| steps_of(&mut sc, w, &h)).unwrap_or_default();
        doc_actions.push(Slot { id: slot_id(w), label: label_of(w), page: None, steps });
    }
    // pages: their actions, and the annotations' scripts
    let mut page_actions = Vec::new();
    let pages = doc.page_count()?.max(0);
    if pages > MAX_PAGES {
        sc.incomplete = true;
    }
    for p in 0..pages.min(MAX_PAGES) {
        let Ok(page) = doc.find_page(p) else { continue };
        let paa = get(&page, "AA").filter(|a| a.is_dict().unwrap_or(false));
        for (ev, _) in PAGE_EVENTS {
            let w = Where::Page(p as u32, ev);
            if let Some(h) = paa.as_ref().and_then(|a| get(a, ev)) {
                let steps = steps_of(&mut sc, w, &h);
                page_actions.push(Slot { id: slot_id(w), label: label_of(w), page: Some(p as u32), steps });
            }
        }
        let Some(annots) = get(&page, "Annots").filter(|a| a.is_array().unwrap_or(false)) else { continue };
        let n = annots.len().unwrap_or(0);
        if n > MAX_ANNOTS {
            sc.incomplete = true;
        }
        for i in 0..n.min(MAX_ANNOTS) as i32 {
            let Some(a) = annots.get_array(i).ok().flatten() else { continue };
            if !a.is_dict().unwrap_or(false) || get(&a, "Subtype").and_then(|s| s.as_name().ok()).is_some_and(|s| s == b"Widget") {
                continue;
            }
            let scripts = holder_scripts(&a);
            if !scripts.is_empty() {
                sc.counts.links += 1;
            }
            for (trigger, s) in scripts {
                sc.meta(&s, || format!("Link on page {}: {}", p + 1, trigger.to_lowercase()));
            }
        }
    }
    // fields
    if let Ok(entries) = scan(doc) {
        let mut seen: HashSet<i32> = HashSet::new();
        let mut field_with_scripts: HashSet<i32> = HashSet::new();
        for e in &entries {
            for (holder, key) in [(&e.obj, e.xref), (&e.field, e.node)] {
                if !seen.insert(key) {
                    continue;
                }
                for (trigger, s) in holder_scripts(holder) {
                    sc.counts.field_scripts += 1;
                    field_with_scripts.insert(e.node);
                    sc.meta(&s, || format!("Field \"{}\": {}", e.name, trigger.to_lowercase()));
                }
            }
        }
        sc.counts.fields = field_with_scripts.len() as u32;
    }
    // bookmarks
    let mut stack: Vec<PdfObject> = get(&catalog, "Outlines").and_then(|o| get(&o, "First")).into_iter().collect();
    let mut seen = HashSet::new();
    let mut items = 0;
    while let Some(item) = stack.pop() {
        if items >= MAX_OUTLINE {
            sc.incomplete = true;
            break;
        }
        if !item.as_indirect().is_ok_and(|n| seen.insert(n)) && item.is_indirect().unwrap_or(false) {
            continue;
        }
        items += 1;
        stack.extend(get(&item, "Next"));
        stack.extend(get(&item, "First"));
        let scripts: Vec<PdfObject> = get(&item, "A").map(|a| chain(&a)).unwrap_or_default().into_iter().filter(is_js).collect();
        if !scripts.is_empty() {
            sc.counts.bookmarks += 1;
        }
        for s in scripts {
            let title = get(&item, "Title").and_then(|t| t.as_bytes().ok()).map(|b| decode_text(&b)).unwrap_or_default();
            sc.meta(&s, || format!("Bookmark \"{title}\""));
        }
    }
    let capabilities = caps()
        .iter()
        .zip(sc.found)
        .filter(|(_, (n, _))| *n > 0)
        .map(|(c, (count, places))| Capability { key: c.key.into(), label: c.label.into(), count, places })
        .collect();
    let (signed, certified) = (crate::outline::signed(doc), crate::outline::certified(doc).map_or(true, |c| c.is_some()));
    Ok(DocJs { named, doc_actions, page_actions, counts: sc.counts, capabilities, incomplete: sc.incomplete || sc.budget.cut, signed, certified })
}

/// The text of the script `id` names (a named script, or a step of an action).
pub fn source(doc: &PdfDocument, id: &str) -> Result<Source> {
    let missing = || bad("that script is no longer in the document");
    let js = if id.starts_with("n:") {
        let entries = attach::tree_of(doc, "JavaScript")?;
        let i = attach::find_entry(&entries, id).ok_or_else(missing)?;
        let spec = entries[i].spec.resolve()?.ok_or_else(missing)?;
        get(&spec, "JS")
    } else {
        let (w, i) = parse_ref(id).ok_or_else(|| bad("unknown script"))?;
        let head = head_of(doc, w)?.ok_or_else(missing)?;
        let step = chain(&head).into_iter().nth(i).ok_or_else(missing)?;
        get(&step, "JS")
    };
    let Some(js) = js else { return Ok(Source { id: id.into(), text: String::new(), size: 0, too_large: false }) };
    let mut b = Budget { left: usize::MAX, cut: false };
    Ok(match read_js(&js, &mut b) {
        Read::Text(t) => Source { id: id.into(), size: t.len() as u64, text: t, too_large: false },
        Read::Large => Source { id: id.into(), text: String::new(), size: 0, too_large: true },
        _ => Source { id: id.into(), text: String::new(), size: 0, too_large: false },
    })
}

// ---- the syntax check ------------------------------------------------------------------------------

/// Whether `source` parses as a script, without running it: compiled by QuickJS in a runtime of its own that has no
/// host functions (not the document's engine, which may be switched off or busy), within a memory and stack limit.
/// Only what the engine runs is checked: over 256 KiB it is reported as too large, not compiled.
pub fn check(source: &str) -> Check {
    use rquickjs::{qjs, Context, Runtime};
    if source.len() > MAX_SCRIPT_BYTES {
        return Check { ok: true, too_large: true, ..Check::default() };
    }
    let fail = |m: &str| Check { ok: false, message: Some(m.to_string()), ..Check::default() };
    let Ok(rt) = Runtime::new() else { return fail("the script engine could not start") };
    rt.set_memory_limit(32 << 20);
    rt.set_max_stack_size(512 << 10);
    let Ok(ctx) = Context::full(&rt) else { return fail("the script engine could not start") };
    let src = source.replace('\0', " ");
    ctx.with(|ctx| {
        // JS_EVAL_FLAG_COMPILE_ONLY: parse and make the bytecode, never run it.
        let flags = (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32;
        let c = std::ffi::CString::new(src.as_str()).unwrap_or_default();
        let name = c"script";
        let v = unsafe { qjs::JS_Eval(ctx.as_raw().as_ptr(), c.as_ptr(), src.len() as _, name.as_ptr(), flags) };
        if unsafe { qjs::JS_VALUE_GET_TAG(v) } != qjs::JS_TAG_EXCEPTION as i32 {
            unsafe { qjs::JS_FreeValue(ctx.as_raw().as_ptr(), v) };
            return Check { ok: true, ..Check::default() };
        }
        let e = ctx.catch();
        let obj = e.as_object();
        let message: Option<String> = obj.and_then(|o| o.get("message").ok());
        let stack: Option<String> = obj.and_then(|o| o.get("stack").ok());
        let (line, column) = stack.as_deref().and_then(position).map_or((None, None), |(l, c)| (Some(l), c));
        Check { ok: false, message: Some(message.unwrap_or_else(|| "syntax error".into())), line, column, too_large: false }
    })
}

/// The `script:<line>[:<column>]` of the first stack frame in the source being checked.
fn position(stack: &str) -> Option<(u32, Option<u32>)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"script:(\d+)(?::(\d+))?").expect("a valid pattern"));
    let c = re.captures(stack)?;
    Some((c[1].parse().ok()?, c.get(2).and_then(|m| m.as_str().parse().ok())))
}

// ---- writing ---------------------------------------------------------------------------------------

/// A script as a PDF string: plain when ASCII, else UTF-16BE with a byte order mark (what Acrobat reads back).
fn js_string(doc: &PdfDocument, s: &str) -> Result<PdfObject> {
    let s = s.replace('\0', " ");
    if s.is_ascii() {
        return Ok(PdfObject::new_string(&s)?);
    }
    let mut hex = String::from("<FEFF");
    for u in s.encode_utf16() {
        hex.push_str(&format!("{u:04X}"));
    }
    hex.push('>');
    Ok(doc.new_object_from_str(&hex)?)
}

/// A new, indirect JavaScript action.
fn new_action(doc: &mut PdfDocument, source: &str, next: Option<PdfObject>) -> Result<PdfObject> {
    let mut a = doc.new_dict()?;
    a.dict_put("Type", PdfObject::new_name("Action")?)?;
    a.dict_put("S", PdfObject::new_name("JavaScript")?)?;
    a.dict_put("JS", js_string(doc, source)?)?;
    if let Some(n) = next {
        a.dict_put("Next", n)?;
    }
    Ok(doc.add_object(&a)?)
}

fn head_of(doc: &PdfDocument, w: Where) -> Result<Option<PdfObject>> {
    Ok(match w {
        Where::Open => get(&doc.catalog()?, "OpenAction"),
        Where::Doc(ev) => get(&doc.catalog()?, "AA").and_then(|a| get(&a, ev)),
        Where::Page(p, ev) => {
            if p as i32 >= doc.page_count()? {
                return Err(bad("that page is no longer in the document"));
            }
            get(&doc.find_page(p as i32)?, "AA").and_then(|a| get(&a, ev))
        }
    })
}

/// The dictionary that holds the action's key (the catalog, or the `/AA` of the catalog or a page, made when `create`),
/// the key, and the dictionary that holds `/AA`.
fn holder(doc: &mut PdfDocument, w: Where, create: bool) -> Result<Option<(PdfObject, &'static str, Option<PdfObject>)>> {
    let (owner, key) = match w {
        Where::Open => return Ok(Some((doc.catalog()?, "OpenAction", None))),
        Where::Doc(ev) => (doc.catalog()?, ev),
        Where::Page(p, ev) => {
            if p as i32 >= doc.page_count()? {
                return Err(bad("that page is no longer in the document"));
            }
            (doc.find_page(p as i32)?, ev)
        }
    };
    let mut owner = owner;
    let aa = match get(&owner, "AA").filter(|a| a.is_dict().unwrap_or(false)) {
        // An /AA object other pages may share is copied before it is changed, so they keep theirs.
        Some(a) if a.is_indirect()? => {
            let copy = doc.add_object(&a.copy_dict()?)?;
            owner.dict_put("AA", copy.clone())?;
            copy
        }
        Some(a) => a,
        None if create => {
            let d = doc.new_dict()?;
            let d = doc.add_object(&d)?;
            owner.dict_put("AA", d.clone())?;
            d
        }
        None => return Ok(None),
    };
    Ok(Some((aa, key, Some(owner))))
}

fn set_step(doc: &mut PdfDocument, w: Where, index: usize, source: &str) -> Result<()> {
    let head = head_of(doc, w)?;
    let nodes = head.as_ref().map(walk).unwrap_or_default();
    match nodes.get(index).map(|n| &n.0) {
        // A later step is copied with every step before it, linked by /Next, and the copy of the head replaces the
        // head: an action object or a chain the file shares (one /AA /O for many pages, or a link too) is not
        // changed under the others.
        Some(step) if index > 0 && is_js(step) => {
            let mut new = step.copy_dict()?;
            new.dict_put("JS", js_string(doc, source)?)?;
            let mut at = nodes[index].1;
            while let Some((p, slot)) = at {
                let mut parent = nodes[p].0.copy_dict()?;
                let child = doc.add_object(&new)?;
                match slot {
                    Some(i) => {
                        let mut next = get(&parent, "Next").ok_or_else(|| bad("that step is no longer there"))?.copy_array()?;
                        next.array_put(i, child)?;
                        parent.dict_put("Next", next)?;
                    }
                    None => parent.dict_put("Next", child)?,
                }
                new = parent;
                at = nodes[p].1;
            }
            let head = doc.add_object(&new)?;
            let (mut h, key, _) = holder(doc, w, true)?.ok_or_else(|| bad("that action cannot be set here"))?;
            h.dict_put(key, head)?;
        }
        Some(_) if index > 0 => return Err(bad("that step is not JavaScript: clear the action to replace it")),
        None if index > 0 => return Err(bad("that step is no longer there")),
        _ => {
            // No action yet, or the head is another kind: a JavaScript one takes its place (keeping what follows).
            let next = head.as_ref().filter(|h| h.is_dict().unwrap_or(false)).and_then(|h| get(h, "Next"));
            let a = new_action(doc, source, next)?;
            let (mut h, key, _) = holder(doc, w, true)?.ok_or_else(|| bad("that action cannot be set here"))?;
            h.dict_put(key, a)?;
        }
    }
    Ok(())
}

fn clear_slot(doc: &mut PdfDocument, w: Where) -> Result<()> {
    let Some((mut h, key, owner)) = holder(doc, w, false)? else { return Ok(()) };
    h.dict_delete(key)?;
    // An /AA left empty is dropped with it.
    if let Some(mut owner) = owner {
        if h.dict_len().unwrap_or(1) == 0 {
            owner.dict_delete("AA")?;
        }
    }
    Ok(())
}

/// A name for a script: trimmed, not empty, no control characters, at most 255 characters.
fn clean_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(bad("a script needs a name"));
    }
    if n.chars().any(|c| c.is_control()) || n.chars().count() > MAX_NAME {
        return Err(bad("a script name is at most 255 characters, with no line breaks"));
    }
    Ok(n.to_string())
}

fn taken(entries: &[Entry]) -> HashSet<&str> {
    entries.iter().map(|e| e.key.as_str()).collect()
}

fn check_size(source: &str) -> Result<()> {
    if source.len() > MAX_DOC_SCRIPT_BYTES {
        return Err(bad("a document script may be at most 1 MiB"));
    }
    Ok(())
}

fn apply_op(doc: &mut PdfDocument, op: &Op, created: &mut Option<String>) -> Result<Option<Vec<u32>>> {
    match op {
        Op::Add { name, source } => {
            check_size(source)?;
            let mut entries = attach::tree_of_editable(doc, "JavaScript")?;
            let name = match name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                Some(n) => clean_name(n)?,
                None => {
                    let t = taken(&entries);
                    (1..).map(|i| format!("Script {i}")).find(|n| !t.contains(n.as_str())).unwrap_or_default()
                }
            };
            if taken(&entries).contains(name.as_str()) {
                return Err(bad(format!("a script named \"{name}\" already exists")));
            }
            let spec = new_action(doc, source, None)?;
            entries.push(Entry { key: name.clone(), spec });
            attach::write_tree_of(doc, "JavaScript", entries)?;
            *created = Some(format!("n:{name}"));
            Ok(Some(Vec::new()))
        }
        Op::Rename { id, name } => {
            let mut entries = attach::tree_of_editable(doc, "JavaScript")?;
            let i = attach::find_entry(&entries, id).ok_or_else(|| bad("that script is no longer in the document"))?;
            let name = clean_name(name)?;
            if entries[i].key == name {
                return Ok(None);
            }
            if taken(&entries).contains(name.as_str()) {
                return Err(bad(format!("a script named \"{name}\" already exists")));
            }
            entries[i].key = name.clone();
            attach::write_tree_of(doc, "JavaScript", entries)?;
            *created = Some(format!("n:{name}"));
            Ok(Some(Vec::new()))
        }
        Op::SetSource { id, source } => {
            check_size(source)?;
            if id.starts_with("n:") {
                let mut entries = attach::tree_of_editable(doc, "JavaScript")?;
                let i = attach::find_entry(&entries, id).ok_or_else(|| bad("that script is no longer in the document"))?;
                // A fresh action, so a spec two names share is not changed under the other one.
                entries[i].spec = new_action(doc, source, None)?;
                attach::write_tree_of(doc, "JavaScript", entries)?;
            } else {
                let (w, i) = parse_ref(id).ok_or_else(|| bad("unknown script"))?;
                set_step(doc, w, i, source)?;
            }
            Ok(Some(Vec::new()))
        }
        Op::Delete { id } => {
            if id.starts_with("n:") {
                let mut entries = attach::tree_of_editable(doc, "JavaScript")?;
                let i = attach::find_entry(&entries, id).ok_or_else(|| bad("that script is no longer in the document"))?;
                entries.remove(i);
                attach::write_tree_of(doc, "JavaScript", entries)?;
            } else {
                let (w, _) = parse_ref(id).ok_or_else(|| bad("unknown script"))?;
                if head_of(doc, w)?.is_none() {
                    return Ok(None);
                }
                clear_slot(doc, w)?;
            }
            Ok(Some(Vec::new()))
        }
    }
}

fn label_for(op: &Op) -> String {
    let named = |id: &str| id.strip_prefix("n:").map(|k| k.split('\u{1f}').next().unwrap_or(k).to_string());
    let slot = |id: &str| parse_ref(id).map_or_else(|| "script".to_string(), |(w, _)| label_of(w).to_lowercase());
    match op {
        Op::Add { name, .. } => format!("Add document script {}", name.as_deref().unwrap_or("").trim()).trim_end().to_string(),
        Op::Rename { id, name } => format!("Rename script {} to {}", named(id).unwrap_or_default(), name.trim()),
        Op::SetSource { id, .. } => match named(id) {
            Some(n) => format!("Edit script {n}"),
            None => format!("Set action: {}", slot(id)),
        },
        Op::Delete { id } => match named(id) {
            Some(n) => format!("Delete script {n}"),
            None => format!("Clear action: {}", slot(id)),
        },
    }
}

// ---- the engine's entry points ----------------------------------------------------------------------

pub fn list_all(eng: &Engine, doc: DocId) -> Result<DocJs> {
    eng.read_custom(doc, list)
}

pub fn source_of(eng: &Engine, doc: DocId, id: &str) -> Result<Source> {
    eng.read_custom(doc, |d| source(d, id))
}

/// One undo step. Replace, delete and clear are removing edits: a save rewrites the file, so the old text is not left
/// in its bytes; except in a signed file, where a rewrite would void the signatures, so the save stays an update (the
/// old text stays in the signed revision) and the result carries a warning. A certified file is refused at any `/P`.
/// An edit that changes nothing (the same text, the same name, an empty slot) is no step.
pub fn apply(eng: &Engine, doc: DocId, op: &Op, src: EditSource) -> Result<Applied> {
    let signed = eng.read_custom(doc, |d| Ok(crate::outline::signed(d)))?;
    let removes = !signed && matches!(op, Op::SetSource { .. } | Op::Delete { .. });
    let mut created = None;
    let mut changed = false;
    let state = eng.apply_custom_unless_noop(doc, label_for(op), src, removes, |d| {
        if crate::outline::certified(d)?.is_some() {
            return Err(bad(
                "This document is certified. Changing its scripts would void the certification, so Inkwell leaves them as they are.",
            ));
        }
        // The same text again changes nothing.
        if let Op::SetSource { id, source: s } = op {
            if source(d, id).is_ok_and(|cur| cur.text == *s && !cur.too_large && (id.starts_with("n:") || head_is_js(d, id))) {
                return Ok(None);
            }
        }
        let done = apply_op(d, op, &mut created)?;
        changed = done.is_some();
        Ok(done)
    })?;
    let warning = (signed && changed).then(|| "This document is signed. Its signature will show that the file was changed after it was signed.".to_string());
    Ok(Applied { state, id: created, warning })
}

/// The step `id` names exists and is JavaScript (an empty or other action is not a no-op to replace).
fn head_is_js(doc: &PdfDocument, id: &str) -> bool {
    parse_ref(id).and_then(|(w, i)| head_of(doc, w).ok().flatten().and_then(|h| chain(&h).into_iter().nth(i))).is_some_and(|s| is_js(&s))
}

#[cfg(test)]
mod tests;
