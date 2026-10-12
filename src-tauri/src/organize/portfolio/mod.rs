//! PDF Portfolios: a PDF whose catalog has a /Collection over its embedded files. Reading the
//! schema, sort, folders and per-file values, creating one from picked files, and extracting.
//!
//! The files themselves are read, listed and described by `attach.rs`; this module adds what the
//! /Collection says about them. Nothing here runs a file: bytes are only written to a folder the
//! person picked, or (a PDF) opened as a document by the caller.
//!
//! Everything read from the file is untrusted: counts are capped, folder loops are refused and every
//! string that is shown has its invisible and control characters taken out.

use crate::organize::attach::{self, AttachKind, Attachment, Texts, capped_string, entry_ids, find_entry, stream_of, strip_invisible, tree};
use crate::organize::{Result, bad, get, put_int, put_str};
#[cfg(test)]
use crate::organize::put_name;
use crate::search::fold_text;
use mupdf::pdf::{PdfDocument, PdfObject};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

mod make;
pub use make::{Created, Extracted, Source, create, extract_all, extract_name};

const MAX_FIELDS: usize = 64;
const MAX_DEPTH: usize = 32;
const MAX_FOLDERS: usize = 10_000;
/// Bytes of cell text and descriptions one read keeps in all.
const CELL_BUDGET: usize = 32 << 20;

// ---- what the file says -------------------------------------------------------------------------

/// How a field's value is taken (the schema's /Subtype).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sub {
    Text,
    Date,
    Number,
    File,
    Desc,
    Size,
    ModDate,
    CreationDate,
    CompressedSize,
}

impl Sub {
    fn parse(name: &str) -> Sub {
        match name {
            "D" => Sub::Date,
            "N" => Sub::Number,
            "F" => Sub::File,
            "Desc" => Sub::Desc,
            "Size" => Sub::Size,
            "ModDate" => Sub::ModDate,
            "CreationDate" => Sub::CreationDate,
            "CompressedSize" => Sub::CompressedSize,
            _ => Sub::Text,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Sub::Text => "S",
            Sub::Date => "D",
            Sub::Number => "N",
            Sub::File => "F",
            Sub::Desc => "Desc",
            Sub::Size => "Size",
            Sub::ModDate => "ModDate",
            Sub::CreationDate => "CreationDate",
            Sub::CompressedSize => "CompressedSize",
        }
    }

    fn kind(self) -> &'static str {
        match self {
            Sub::Number | Sub::Size | Sub::CompressedSize => "number",
            Sub::Date | Sub::ModDate | Sub::CreationDate => "date",
            Sub::Text | Sub::File | Sub::Desc => "text",
        }
    }
}

/// One column of the collection's schema.
#[derive(Clone, Debug)]
struct Field {
    key: String,
    name: String,
    sub: Sub,
    visible: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioField {
    pub key: String,
    pub name: String,
    /// The schema's subtype: S, D, N, F, Desc, Size, ModDate, CreationDate, CompressedSize.
    pub subtype: String,
    /// "text", "number" or "date".
    pub kind: String,
    pub visible: bool,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SortKey {
    /// Index into `fields`.
    pub field: usize,
    pub ascending: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioFolder {
    pub id: u32,
    pub name: String,
    pub parent: Option<u32>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioFile {
    /// The attachment id (`n:<key>`): what `attachment_open`, `attachment_save` and `attachments_*` take.
    pub id: String,
    pub name: String,
    pub description: String,
    pub size: u64,
    pub mime: Option<String>,
    pub is_pdf: bool,
    pub has_data: bool,
    /// The folder it is in; none is the top level.
    pub folder: Option<u32>,
    /// What each field shows for this file, in `fields` order.
    pub cells: Vec<String>,
    /// Where this file's value stands among all files in each field (equal values share a rank; none: no
    /// value). Sorting by a column is a numeric sort of these, so the order is always the engine's.
    pub ranks: Vec<Option<u32>>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PortfolioInfo {
    /// The schema's fields in the author's column order (/O).
    pub fields: Vec<PortfolioField>,
    /// The collection's own sort, which `files` come in.
    pub sort: Vec<SortKey>,
    /// "details", "tile", "hidden" or "custom" (/View D, T, H, C).
    pub view: String,
    /// The file that opens first (/D).
    pub initial: Option<String>,
    /// Every folder, parents before children, siblings in name order.
    pub folders: Vec<PortfolioFolder>,
    /// Every file in the collection's sort order.
    pub files: Vec<PortfolioFile>,
}

pub(super) fn collection(doc: &PdfDocument) -> Result<Option<PdfObject>> {
    let c = get(&doc.catalog()?, "Collection").and_then(|c| c.resolve().ok().flatten());
    Ok(c.filter(|c| c.is_dict().unwrap_or(false)))
}

pub(super) fn resolved_dict(o: Option<PdfObject>) -> Option<PdfObject> {
    o.and_then(|o| o.resolve().ok().flatten()).filter(|o| o.is_dict().unwrap_or(false))
}

fn name_of(o: &PdfObject) -> Option<String> {
    o.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned())
}

fn shown(s: &str, max: usize) -> String {
    strip_invisible(s).replace('\n', " ").chars().take(max).collect()
}

/// The schema's fields in /O order (those without one follow, in the file's order).
fn read_schema(coll: &PdfObject) -> Result<Vec<Field>> {
    let Some(schema) = resolved_dict(get(coll, "Schema")) else { return Ok(Vec::new()) };
    let mut out: Vec<(i64, Field)> = Vec::new();
    for i in 0..schema.dict_len()?.min(4 * MAX_FIELDS) {
        let (Some(k), Some(v)) = (schema.get_dict_key(i as i32)?, schema.get_dict_val(i as i32)?) else { continue };
        let Some(key) = name_of(&k).filter(|k| k != "Type") else { continue };
        let Some(f) = resolved_dict(Some(v)) else { continue };
        if out.len() >= MAX_FIELDS {
            break;
        }
        let sub = get(&f, "Subtype").and_then(|s| name_of(&s)).map_or(Sub::Text, |s| Sub::parse(&s));
        let name = get(&f, "N").and_then(|n| capped_string(&n)).map(|n| shown(&n, 100)).filter(|n| !n.is_empty()).unwrap_or_else(|| shown(&key, 100));
        let visible = get(&f, "V").and_then(|v| v.as_bool().ok()).unwrap_or(true);
        let order = get(&f, "O").and_then(|o| o.as_int().ok()).map_or(i64::MAX, i64::from);
        out.push((order, Field { key, name, sub, visible }));
    }
    out.sort_by_key(|(o, _)| *o);
    Ok(out.into_iter().map(|(_, f)| f).collect())
}

/// /Sort as indexes into `fields`: /S is a key or an array of keys, /A a bool or an array of them (a
/// missing one is ascending).
fn read_sort(coll: &PdfObject, fields: &[Field]) -> Vec<SortKey> {
    let Some(sort) = resolved_dict(get(coll, "Sort")) else { return Vec::new() };
    let list = |o: PdfObject, f: &dyn Fn(&PdfObject) -> Option<String>| -> Vec<Option<String>> {
        let o = o.resolve().ok().flatten().unwrap_or(o);
        if o.is_array().unwrap_or(false) {
            (0..o.len().unwrap_or(0).min(MAX_FIELDS)).map(|i| o.get_array(i as i32).ok().flatten().and_then(|v| f(&v))).collect()
        } else {
            vec![f(&o)]
        }
    };
    let keys = get(&sort, "S").map(|s| list(s, &name_of)).unwrap_or_default();
    let asc: Vec<Option<bool>> = get(&sort, "A")
        .map(|a| {
            let a = a.resolve().ok().flatten().unwrap_or(a);
            if a.is_array().unwrap_or(false) {
                (0..a.len().unwrap_or(0).min(MAX_FIELDS)).map(|i| a.get_array(i as i32).ok().flatten().and_then(|v| v.as_bool().ok())).collect()
            } else {
                vec![a.as_bool().ok()]
            }
        })
        .unwrap_or_default();
    keys.into_iter()
        .enumerate()
        .filter_map(|(i, k)| {
            let k = k?;
            let field = fields.iter().position(|f| f.key == k)?;
            Some(SortKey { field, ascending: asc.get(i).copied().flatten().unwrap_or(true) })
        })
        .collect()
}

// ---- values -------------------------------------------------------------------------------------

/// A value as compared: strings are folded the way a search folds them, then compared with numbers
/// inside them in number order.
#[derive(Clone, Debug)]
enum Key {
    Missing,
    Text(String),
    Number(f64),
    Date(i64),
}

#[derive(Clone)]
struct Cell {
    text: String,
    key: Key,
}

impl Cell {
    fn none() -> Cell {
        Cell { text: String::new(), key: Key::Missing }
    }

    fn text(s: &str) -> Cell {
        let text = shown(s, 500);
        if text.is_empty() { Cell::none() } else { Cell { key: Key::Text(fold_text(&text)), text } }
    }

    fn number(n: f64, text: String) -> Cell {
        Cell { text, key: Key::Number(n) }
    }

    fn date(raw: &str) -> Cell {
        match parse_pdf_date(raw) {
            Some((secs, text)) => Cell { text, key: Key::Date(secs) },
            None => Cell::text(raw),
        }
    }
}

pub(super) fn size_text(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.*} KB", usize::from(n < 10 * 1024), n as f64 / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1} MB", n as f64 / 1_048_576.0),
        _ => format!("{:.1} GB", n as f64 / 1_073_741_824.0),
    }
}

fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// Days since 1970-01-01 of a civil date (Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A PDF date (`D:YYYYMMDDHHmmSS` and a zone) as seconds since the Unix epoch (UTC) and as it reads
/// ("2026-10-11 14:03", or the date alone when it has no time).
pub(super) fn parse_pdf_date(s: &str) -> Option<(i64, String)> {
    let d = s.trim().trim_start_matches("D:");
    let digits: String = d.chars().take_while(char::is_ascii_digit).take(14).collect();
    if digits.len() < 4 {
        return None;
    }
    let part = |a: usize, b: usize, def: i64| digits.get(a..b).and_then(|p| p.parse::<i64>().ok()).unwrap_or(def);
    let (y, mo, da, h, mi, se) = (part(0, 4, 0), part(4, 6, 1), part(6, 8, 1), part(8, 10, 0), part(10, 12, 0), part(12, 14, 0));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let rest = &d[digits.len()..];
    let offset = match rest.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let t: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
            let secs = t.get(0..2).and_then(|p| p.parse::<i64>().ok()).unwrap_or(0) * 3600 + t.get(2..4).and_then(|p| p.parse::<i64>().ok()).unwrap_or(0) * 60;
            if sign == '-' { -secs } else { secs }
        }
        _ => 0,
    };
    let secs = days_from_civil(y, mo, da) * 86_400 + h * 3600 + mi * 60 + se - offset;
    let text = if digits.len() > 8 { format!("{y:04}-{mo:02}-{da:02} {h:02}:{mi:02}") } else { format!("{y:04}-{mo:02}-{da:02}") };
    Some((secs, text))
}

/// A string, name or number the file gives as a value.
fn plain(o: &PdfObject, texts: &mut Texts) -> Option<(Option<f64>, String)> {
    if o.is_number().unwrap_or(false) {
        let n = if o.is_int().unwrap_or(false) { f64::from(o.as_int().ok()?) } else { f64::from(o.as_float().ok()?) };
        return n.is_finite().then(|| (Some(n), number_text(n)));
    }
    if o.is_string().unwrap_or(false) {
        return texts.get(o).map(|s| (None, s));
    }
    name_of(o).map(|s| (None, s))
}

/// The /CI value of `key`: its text with the sub-item's prefix (/P) in front, and the number it is when it is one.
fn ci_value(ci: &PdfObject, key: &str, texts: &mut Texts) -> Option<(Option<f64>, String, String)> {
    // The value as written (a reference stays one, so a string shared by many fields is read once).
    let raw = get(ci, key)?;
    let v = if raw.is_string().unwrap_or(false) { raw } else { raw.resolve().ok().flatten()? };
    if v.is_dict().unwrap_or(false) {
        let d = get(&v, "D")?;
        let d = if d.is_string().unwrap_or(false) { d } else { d.resolve().ok().flatten()? };
        let (n, t) = plain(&d, texts)?;
        let prefix = get(&v, "P").and_then(|p| texts.get(&p)).unwrap_or_default();
        return Some((n, t, prefix));
    }
    plain(&v, texts).map(|(n, t)| (n, t, String::new()))
}

/// What one file gives the cells: the objects its values come from, and which of them can be told apart
/// by object number (a repeated one is then read once, whatever number of files point at it).
struct Given<'a> {
    ci: Option<&'a PdfObject>,
    /// `Some(None)`: no /CI; `Some(Some(n))`: the indirect object n; `None`: a direct dictionary.
    ci_key: Option<Option<i32>>,
    spec: &'a PdfObject,
    stream: Option<&'a PdfObject>,
    stream_key: Option<Option<i32>>,
    meta: &'a Attachment,
}

type CiVal = (Option<f64>, String, String);

/// What reading the cells already worked out, by object number: a /CI dictionary or a stream that many
/// file specifications share has its strings decoded once.
#[derive(Default)]
struct Memo {
    ci: HashMap<(i32, String), Option<CiVal>>,
    params: HashMap<(i32, &'static str), Option<String>>,
    cells: HashMap<(Option<i32>, Option<i32>, usize), Cell>,
}

const MEMO_MAX: usize = 1_000_000;

/// The object-number key of an optional object: none when it is absent, its number when indirect, and
/// nothing to go by when it is direct.
fn num_key(o: Option<&PdfObject>) -> Option<Option<i32>> {
    match o {
        None => Some(None),
        Some(o) if o.is_indirect().unwrap_or(false) => o.as_indirect().ok().map(Some),
        Some(_) => None,
    }
}

/// The cell of one file for one field. What the file says of itself (its name, description, size and
/// dates: the file specification and the stream's /Params) comes first for the fields of those
/// subtypes, as ISO 32000 has it; /CI has the value of text, number and date fields, and stands in
/// for a fact the file does not give.
fn cell(f: &Field, at: usize, g: &Given, texts: &mut Texts, memo: &mut Memo) -> Cell {
    // A cell that depends on the /CI (and the stream) alone is the same for every file that has the same ones.
    let key = match f.sub {
        Sub::Text | Sub::Number | Sub::Date => g.ci_key.map(|c| (c, None, at)),
        Sub::Size | Sub::CompressedSize | Sub::ModDate | Sub::CreationDate => g.ci_key.zip(g.stream_key).map(|(c, s)| (c, s, at)),
        Sub::File | Sub::Desc => None,
    };
    if let Some(c) = key.as_ref().and_then(|k| memo.cells.get(k)) {
        return c.clone();
    }
    let c = cell_of(f, g, texts, memo);
    if let Some(k) = key.filter(|_| memo.cells.len() < MEMO_MAX) {
        memo.cells.insert(k, c.clone());
    }
    c
}

fn cell_of(f: &Field, g: &Given, texts: &mut Texts, memo: &mut Memo) -> Cell {
    let (spec, stream, meta) = (g.spec, g.stream, g.meta);
    let from_ci = g.ci.and_then(|ci| match g.ci_key {
        Some(Some(n)) if memo.ci.len() < MEMO_MAX => memo.ci.entry((n, f.key.clone())).or_insert_with(|| ci_value(ci, &f.key, texts)).clone(),
        _ => ci_value(ci, &f.key, texts),
    });
    let mut params = |k: &'static str, texts: &mut Texts| {
        let read = |texts: &mut Texts| {
            let p = resolved_dict(stream.and_then(|s| get(s, "Params")))?;
            get(&p, k).and_then(|v| plain(&v, texts)).map(|(_, t)| t)
        };
        match g.stream_key {
            Some(Some(n)) if memo.params.len() < MEMO_MAX => memo.params.entry((n, k)).or_insert_with(|| read(texts)).clone(),
            _ => read(texts),
        }
    };
    let as_size = |n: u64| Cell::number(n as f64, size_text(n));
    let ci_number = || match &from_ci {
        Some((Some(n), _, _)) if *n >= 0.0 => Some(*n as u64),
        _ => None,
    };
    match f.sub {
        Sub::File => Cell::text(&meta.name),
        Sub::Desc => {
            let own = get(spec, "Desc").and_then(|d| texts.get(&d)).filter(|d| !d.is_empty());
            match (own, &from_ci) {
                (Some(d), _) => Cell::text(&d),
                (None, Some((_, t, p))) => Cell::text(&format!("{p}{t}")),
                (None, None) => Cell::none(),
            }
        }
        Sub::Size => meta.has_data.then_some(meta.size).or_else(ci_number).map_or_else(Cell::none, as_size),
        Sub::CompressedSize => {
            let len = stream.and_then(|s| get(s, "Length")).and_then(|v| v.as_int().ok()).and_then(|n| u64::try_from(n).ok());
            len.or_else(ci_number).map_or_else(Cell::none, as_size)
        }
        Sub::ModDate | Sub::CreationDate => {
            let key = if f.sub == Sub::CreationDate { "CreationDate" } else { "ModDate" };
            match params(key, texts) {
                Some(t) => Cell::date(&t),
                None => from_ci.map_or_else(Cell::none, |(_, t, _)| Cell::date(&t)),
            }
        }
        Sub::Date => from_ci.map_or_else(Cell::none, |(_, t, _)| Cell::date(&t)),
        Sub::Number => match from_ci {
            Some((Some(n), t, p)) => Cell::number(n, format!("{p}{}", shown(&t, 100))),
            Some((None, t, p)) => match t.trim().parse::<f64>().ok().filter(|n| n.is_finite()) {
                Some(n) => Cell::number(n, format!("{p}{}", shown(&number_text(n), 100))),
                None => Cell::text(&format!("{p}{t}")),
            },
            None => Cell::none(),
        },
        Sub::Text => from_ci.map_or_else(Cell::none, |(_, t, p)| Cell::text(&format!("{p}{t}"))),
    }
}

// ---- order --------------------------------------------------------------------------------------

/// Natural order: digit runs compare as numbers (without parsing them: any length), the rest by character.
pub(super) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        s.push(c);
                    }
                    s
                };
                let (ra, rb) = (run(&mut x), run(&mut y));
                let (ta, tb) = (ra.trim_start_matches('0'), rb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| ra.len().cmp(&rb.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
}

fn cmp_key(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::Text(x), Key::Text(y)) => natural_cmp(x, y),
        (Key::Number(x), Key::Number(y)) => x.total_cmp(y),
        (Key::Date(x), Key::Date(y)) => x.cmp(y),
        _ => {
            let rank = |k: &Key| match k {
                Key::Missing => 0,
                Key::Text(_) => 1,
                Key::Number(_) => 2,
                Key::Date(_) => 3,
            };
            rank(a).cmp(&rank(b))
        }
    }
}

/// The rank of every value in one column: equal values share a rank, a missing value has none.
fn ranks(keys: &[&Key]) -> Vec<Option<u32>> {
    // Equal texts are compared once: one value repeated in many files costs one entry.
    let mut first_text: HashMap<&str, usize> = HashMap::new();
    let mut rep = vec![usize::MAX; keys.len()];
    let mut at: Vec<usize> = Vec::new();
    for (i, k) in keys.iter().enumerate() {
        match k {
            Key::Missing => {}
            Key::Text(t) => match first_text.get(t.as_str()) {
                Some(&r) => rep[i] = r,
                None => {
                    first_text.insert(t, i);
                    rep[i] = i;
                    at.push(i);
                }
            },
            _ => {
                rep[i] = i;
                at.push(i);
            }
        }
    }
    at.sort_by(|&a, &b| cmp_key(keys[a], keys[b]));
    let mut rank_of = vec![None; keys.len()];
    let mut rank = 0u32;
    for (n, &i) in at.iter().enumerate() {
        if n > 0 && cmp_key(keys[at[n - 1]], keys[i]) != Ordering::Equal {
            rank += 1;
        }
        rank_of[i] = Some(rank);
    }
    rep.iter().map(|&r| if r == usize::MAX { None } else { rank_of[r] }).collect()
}

// ---- folders ------------------------------------------------------------------------------------

/// `<12>name.pdf` as (12, "name.pdf"): how a file's tree key says which folder it is in.
pub(super) fn split_prefix(key: &str) -> (Option<u32>, &str) {
    if let Some(rest) = key.strip_prefix('<') {
        if let Some(i) = rest.find('>') {
            let d = &rest[..i];
            if !d.is_empty() && d.len() <= 9 && d.bytes().all(|b| b.is_ascii_digit()) {
                return (d.parse().ok(), &rest[i + 1..]);
            }
        }
    }
    (None, key)
}

/// The folder tree (/Folders: a root whose /Child is the first of a /Next chain, each with its own
/// /Child). A loop through /Child or /Next is refused.
fn read_folders(coll: &PdfObject) -> Result<Vec<PortfolioFolder>> {
    let Some(root_ref) = get(coll, "Folders") else { return Ok(Vec::new()) };
    let Some(root) = resolved_dict(Some(root_ref.clone())) else { return Ok(Vec::new()) };
    let mut seen: HashSet<i32> = HashSet::new();
    if root_ref.is_indirect()? {
        seen.insert(root_ref.as_indirect()?);
    }
    let mut out: Vec<PortfolioFolder> = Vec::new();
    let mut ids: HashSet<u32> = HashSet::new();
    let mut synthetic = 1u32 << 31;
    // (first sibling, parent id, depth)
    let mut stack: Vec<(Option<PdfObject>, Option<u32>, usize)> = vec![(get(&root, "Child"), None, 0)];
    while let Some((first, parent, depth)) = stack.pop() {
        if depth >= MAX_DEPTH {
            return Err(bad("the portfolio's folders are nested too deeply"));
        }
        let mut cur = first;
        while let Some(node) = cur {
            if node.is_indirect()? && !seen.insert(node.as_indirect()?) {
                return Err(bad("the portfolio's folders loop back on themselves"));
            }
            let Some(d) = resolved_dict(Some(node)) else { break };
            if out.len() >= MAX_FOLDERS {
                return Err(bad("the portfolio has too many folders"));
            }
            let id = get(&d, "ID").and_then(|i| i.as_int().ok()).and_then(|i| u32::try_from(i).ok()).filter(|i| !ids.contains(i)).unwrap_or_else(|| {
                synthetic += 1;
                synthetic
            });
            ids.insert(id);
            let name = get(&d, "Name").and_then(|n| n.as_string().ok()).map(|n| shown(&n, 200)).filter(|n| !n.is_empty()).unwrap_or_else(|| "Folder".into());
            out.push(PortfolioFolder { id, name, parent });
            if let Some(child) = get(&d, "Child") {
                stack.push((Some(child), Some(id), depth + 1));
            }
            cur = get(&d, "Next");
        }
    }
    // Parents before children, siblings by name.
    let mut by_parent: HashMap<Option<u32>, Vec<usize>> = HashMap::new();
    for (i, f) in out.iter().enumerate() {
        by_parent.entry(f.parent).or_default().push(i);
    }
    for v in by_parent.values_mut() {
        v.sort_by(|&a, &b| natural_cmp(&fold_text(&out[a].name), &fold_text(&out[b].name)));
    }
    let mut ordered = Vec::with_capacity(out.len());
    let mut todo: Vec<usize> = by_parent.get(&None).map(|v| v.iter().rev().copied().collect()).unwrap_or_default();
    while let Some(i) = todo.pop() {
        ordered.push(out[i].clone());
        if let Some(kids) = by_parent.get(&Some(out[i].id)) {
            todo.extend(kids.iter().rev());
        }
    }
    Ok(ordered)
}

// ---- reading ------------------------------------------------------------------------------------

/// The portfolio view of a document: none when it has no /Collection. A collection whose folders
/// loop is an error (the caller shows the document as a plain one).
pub fn read(doc: &PdfDocument) -> Result<Option<PortfolioInfo>> {
    let Some(coll) = collection(doc)? else { return Ok(None) };
    let mut fields = read_schema(&coll)?;
    // /Schema is optional: the file's own facts are the columns then. A schema with no visible file name
    // gets one in front, so a row always says which file it is.
    if fields.is_empty() {
        let own = [("Name", Sub::File), ("Description", Sub::Desc), ("Size", Sub::Size), ("Modified", Sub::ModDate)];
        fields = own.iter().map(|(k, sub)| Field { key: (*k).into(), name: (*k).into(), sub: *sub, visible: true }).collect();
    } else if !fields.iter().any(|f| f.sub == Sub::File && f.visible) {
        fields.insert(0, Field { key: "$name".into(), name: "Name".into(), sub: Sub::File, visible: true });
    }
    let sort = read_sort(&coll, &fields);
    let view = match get(&coll, "View").and_then(|v| name_of(&v)).as_deref() {
        Some("T") => "tile",
        Some("H") => "hidden",
        Some("C") => "custom",
        _ => "details",
    };
    let folders = read_folders(&coll)?;
    let known: HashSet<u32> = folders.iter().map(|f| f.id).collect();
    let initial_key = get(&coll, "D").and_then(|d| crate::forms::text(&d));

    let entries = tree(doc)?;
    let ids = entry_ids(&entries);
    let mut files: Vec<PortfolioFile> = Vec::with_capacity(entries.len());
    let mut cells: Vec<Vec<Cell>> = Vec::with_capacity(entries.len());
    let mut initial = None;
    let mut texts = Texts::default();
    // Text a file may put in the cells in all; a hostile file can repeat one big value in every cell.
    let mut budget = CELL_BUDGET;
    // File specification object number -> the row it first made: the same one again is not read again.
    let mut rows_by_spec: HashMap<i32, usize> = HashMap::new();
    let mut memo = Memo::default();
    for (e, id) in entries.iter().zip(ids) {
        let (prefix, plain_key) = split_prefix(&e.key);
        let meta = attach::info_with(&mut texts, &e.spec, plain_key, id.clone(), AttachKind::File, None, None);
        let spec = resolved_dict(Some(e.spec.clone()));
        let spec_num = if e.spec.is_indirect().unwrap_or(false) { e.spec.as_indirect().ok() } else { None };
        let ci = spec.as_ref().and_then(|s| resolved_dict(get(s, "CI")));
        let stream = stream_of(&e.spec);
        let stream_dict = stream.as_ref().and_then(|s| s.resolve().ok().flatten());
        let folder = spec
            .as_ref()
            .and_then(|s| get(s, "Folder"))
            .and_then(|v| v.as_int().ok())
            .and_then(|v| u32::try_from(v).ok())
            .or(prefix)
            .filter(|f| known.contains(f));
        let mut row: Vec<Cell> = match (&spec, spec_num.and_then(|n| rows_by_spec.get(&n))) {
            // The same specification again: its cells are the ones already made (the name column aside).
            (_, Some(&first)) => cells[first].clone(),
            (Some(spec), None) => {
                let given = Given {
                    ci: ci.as_ref(),
                    ci_key: num_key(get(spec, "CI").as_ref()),
                    spec,
                    stream: stream_dict.as_ref(),
                    stream_key: num_key(stream.as_ref()),
                    meta: &meta,
                };
                fields.iter().enumerate().map(|(at, f)| cell(f, at, &given, &mut texts, &mut memo)).collect()
            }
            (None, None) => fields.iter().map(|_| Cell::none()).collect(),
        };
        if let Some(n) = spec_num {
            rows_by_spec.entry(n).or_insert(cells.len());
        }
        for (f, c) in fields.iter().zip(&mut row) {
            if f.sub == Sub::File {
                *c = Cell::text(&meta.name);
            } else if c.text.len() > budget {
                *c = Cell::none();
            } else {
                budget -= c.text.len();
            }
        }
        let description: String = meta.description.chars().take(500).collect();
        let description = if description.len() > budget { String::new() } else { description };
        budget -= description.len();
        if initial.is_none() && initial_key.as_deref() == Some(e.key.as_str()) {
            initial = Some(id.clone());
        }
        files.push(PortfolioFile {
            id,
            name: meta.name,
            description,
            size: meta.size,
            mime: meta.mime,
            is_pdf: meta.is_pdf,
            has_data: meta.has_data,
            folder,
            cells: Vec::new(),
            ranks: Vec::new(),
        });
        cells.push(row);
    }
    let by_field: Vec<Vec<Option<u32>>> =
        (0..fields.len()).map(|c| ranks(&cells.iter().map(|r| &r[c].key).collect::<Vec<_>>())).collect();
    for (i, (f, row)) in files.iter_mut().zip(cells).enumerate() {
        f.ranks = by_field.iter().map(|col| col[i]).collect();
        f.cells = row.into_iter().map(|c| c.text).collect();
    }
    // The collection's own order; a stable sort keeps the name tree's order among equals.
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by(|&a, &b| {
        for k in &sort {
            let o = match (by_field[k.field][a], by_field[k.field][b]) {
                (None, None) => continue,
                (None, Some(_)) => return Ordering::Greater,
                (Some(_), None) => return Ordering::Less,
                (Some(x), Some(y)) => if k.ascending { x.cmp(&y) } else { y.cmp(&x) },
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    });
    let mut slots: Vec<Option<PortfolioFile>> = files.into_iter().map(Some).collect();
    let files = order.into_iter().filter_map(|i| slots[i].take()).collect();
    let fields = fields
        .into_iter()
        .map(|f| PortfolioField { key: f.key, name: f.name, subtype: f.sub.code().into(), kind: f.sub.kind().into(), visible: f.visible })
        .collect();
    Ok(Some(PortfolioInfo { fields, sort, view: view.into(), initial, folders, files }))
}

// ---- preview ------------------------------------------------------------------------------------

/// The most decoded bytes of an embedded PDF a thumbnail will open (the UI shows the file-type icon past it).
pub const THUMB_MAX: usize = 32 << 20;

/// One thumbnail is drawn at a time, however many the view asks for: a tile grid cannot pile up renders. A page
/// that runs long is aborted between content operators (`diff::visual`'s fit limit); one huge image or shading
/// is not interrupted, so a page declaring a picture over 50 MP (100 MP in all) is refused before it is drawn.
static ONE_AT_A_TIME: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Page 1 of the embedded PDF `id` as a PNG whose longest side is `side` pixels (48 to 1024). The file is
/// only read: a file over `cap` decoded bytes, one that is not a PDF, is encrypted or cannot be drawn is
/// refused with a short reason the view shows as a note. Nothing is written or run.
pub fn thumb(eng: &crate::engine::Engine, doc: crate::engine::DocId, id: &str, side: u32, cap: usize) -> Result<Vec<u8>> {
    use crate::diff::{BytesId, DiffError, render_page_fit};
    let _one = ONE_AT_A_TIME.lock();
    let (_, bytes) = eng.read_custom(doc, |d| attach::read_data_capped(d, id, cap))?;
    let r = render_page_fit(BytesId::of(&bytes), &bytes, 0, side.clamp(48, 1024)).map_err(|e| {
        bad(match e {
            DiffError::NeedsPassword { .. } | DiffError::BadPassword { .. } => "This PDF is password protected",
            DiffError::BadPage { .. } => "This PDF has no pages",
            DiffError::NotPdf { .. } => "This file is not a PDF Inkwell can open",
            DiffError::Slow => "This PDF's first page took too long to draw",
            DiffError::BigImage { .. } => "This PDF's first page holds a picture too large to preview",
            _ => "This PDF could not be drawn",
        })
    })?;
    r.to_png().map_err(|_| bad("This PDF could not be drawn"))
}

// ---- editing a portfolio ------------------------------------------------------------------------

/// `/CI` of the files `keys`, from the document's own schema: a field of the file's own facts (name,
/// description, size, dates) gets its value, whatever key the schema calls it by.
pub(super) fn fill_ci(doc: &mut PdfDocument, keys: &[String]) -> Result<()> {
    let Some(coll) = collection(doc)? else { return Ok(()) };
    let fields = read_schema(&coll)?;
    let wanted: HashSet<&str> = keys.iter().map(String::as_str).collect();
    for e in tree(doc)?.into_iter().filter(|e| wanted.contains(e.key.as_str())) {
        let Some(mut spec) = resolved_dict(Some(e.spec.clone())) else { continue };
        let meta = attach::info(&e.spec, split_prefix(&e.key).1, format!("n:{}", e.key), AttachKind::File, None, None);
        let stream = stream_of(&e.spec).and_then(|s| s.resolve().ok().flatten());
        let params = |k: &str| {
            let p = resolved_dict(stream.as_ref().and_then(|s| get(s, "Params")))?;
            get(&p, k).and_then(|v| capped_string(&v))
        };
        let mut ci = doc.new_dict()?;
        ci.dict_put("Type", PdfObject::new_name("CollectionItem")?)?;
        for f in &fields {
            match f.sub {
                Sub::File => put_str(&mut ci, &f.key, &meta.name)?,
                Sub::Desc => put_str(&mut ci, &f.key, &meta.description)?,
                Sub::Size if meta.has_data => put_int(&mut ci, &f.key, meta.size.min(i32::MAX as u64) as i32)?,
                Sub::CompressedSize => {
                    if let Some(n) = stream.as_ref().and_then(|s| get(s, "Length")).and_then(|l| l.as_int().ok()) {
                        put_int(&mut ci, &f.key, n)?;
                    }
                }
                Sub::ModDate => put_str(&mut ci, &f.key, &params("ModDate").unwrap_or_else(crate::annot::pdf_now))?,
                Sub::CreationDate => put_str(&mut ci, &f.key, &params("CreationDate").or_else(|| params("ModDate")).unwrap_or_else(crate::annot::pdf_now))?,
                _ => {}
            }
        }
        spec.dict_put("CI", ci)?;
    }
    Ok(())
}

/// After files were attached: in a portfolio each new one gets its /CI. `before` is the set of keys the
/// tree had.
pub(super) fn after_add(doc: &mut PdfDocument, before: &HashSet<String>) -> Result<()> {
    if collection(doc)?.is_none() {
        return Ok(());
    }
    let new: Vec<String> = tree(doc)?.into_iter().map(|e| e.key).filter(|k| !before.contains(k)).collect();
    fill_ci(doc, &new)
}

/// After files were removed: the initial document (/D) must not name one that is gone.
pub(super) fn after_remove(doc: &mut PdfDocument) -> Result<()> {
    let Some(mut coll) = collection(doc)? else { return Ok(()) };
    if let Some(d) = get(&coll, "D").and_then(|d| crate::forms::text(&d)) {
        if !tree(doc)?.iter().any(|e| e.key == d) {
            coll.dict_delete("D")?;
        }
    }
    Ok(())
}

/// After a description was set: /CI wins over /Desc when a portfolio's column is read, so it follows.
pub(super) fn after_describe(doc: &PdfDocument, id: &str, text: &str) -> Result<()> {
    let Some(coll) = collection(doc)? else { return Ok(()) };
    let mut entries = tree(doc)?;
    let Some(e) = find_entry(&entries, id).map(|i| entries.swap_remove(i)) else { return Ok(()) };
    let Some(mut ci) = resolved_dict(resolved_dict(Some(e.spec)).and_then(|s| get(&s, "CI"))) else { return Ok(()) };
    for f in read_schema(&coll)?.iter().filter(|f| f.sub == Sub::Desc) {
        if get(&ci, &f.key).is_some() {
            put_str(&mut ci, &f.key, text)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
