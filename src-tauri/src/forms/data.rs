//! Form data to and from FDF, XFDF and JSON: Acrobat's Forms > Manage Form Data > Export / Import.
//! The files are hostile input (see `crate::exchange` for the caps).
//!
//! Export reads each field's `/V` as it is stored (a rich text field's plain value, a check box's
//! export value or `Off`, every selected entry of a multi-select list) and runs no script. Import goes
//! through the same set-value path as typing, one field after another inside one edit step, so formats,
//! validation, calculations and signature locks apply exactly as when a person types. A name that is
//! not a field, a value the field cannot take and a locked or read-only field are reported and skipped.
//! Nothing a script asks for (submit, open a link, print) is acted on, and its alerts are dropped.

use super::scripts::{self, Rt};
use super::{
    FieldKind, FieldValue, Entry, bad, field_ff, fill_is_noop, flag, geti, get, on_state, options_of, per_field, rt_of, scan, set_value_rt, text, values_of,
};
use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use crate::exchange::{self, Budget, Format, MAX_DEPTH, MAX_ITEMS, MAX_NODES, MAX_STR, esc, pdf_name, pdf_text};
use mupdf::pdf::{FieldFlags, PdfDocument, PdfObject};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

type Result<T> = std::result::Result<T, EngineError>;

// ---- export --------------------------------------------------------------------------------------

/// One field's stored value.
struct Out {
    name: String,
    kind: FieldKind,
    values: Vec<String>,
    /// A list box that takes several entries: its value is always a list.
    multi: bool,
}

fn collect(doc: &PdfDocument) -> Result<Vec<Out>> {
    let entries = scan(doc)?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for e in per_field(&entries) {
        if matches!(e.kind, FieldKind::Button | FieldKind::Signature) || e.name.is_empty() || !seen.insert(e.name.clone()) {
            continue;
        }
        let mut values: Vec<String> = values_of(geti(&e.field, "V"));
        if matches!(e.kind, FieldKind::Checkbox | FieldKind::Radio) {
            // A box that has no /V but shows an on-state (the appearance state of one of its widgets).
            if values.first().is_none_or(|v| v.is_empty()) {
                let on = entries.iter().filter(|w| w.node == e.node).find_map(|w| get(&w.obj, "AS").as_ref().and_then(text).filter(|s| s != "Off"));
                values = vec![on.unwrap_or_else(|| "Off".into())];
            }
        }
        out.push(Out { name: e.name.clone(), kind: e.kind, values, multi: e.kind == FieldKind::List && flag(field_ff(e), FieldFlags::MULTI_SELECT) });
    }
    Ok(out)
}

fn json(fields: &[Out]) -> String {
    let s = |v: &str| serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
    let mut o = String::from("{\n");
    for (i, f) in fields.iter().enumerate() {
        let v = if f.multi || f.values.len() > 1 {
            format!("[{}]", f.values.iter().map(|v| s(v)).collect::<Vec<_>>().join(", "))
        } else {
            s(f.values.first().map_or("", String::as_str))
        };
        let _ = write!(o, "  {}: {}{}\n", s(&f.name), v, if i + 1 < fields.len() { "," } else { "" });
    }
    o.push('}');
    o.push('\n');
    o
}

/// Fields nested by the dots in their full names (partial names), in the order they were found.
#[derive(Default)]
struct Tree<'a> {
    leaf: Option<&'a Out>,
    kids: Vec<(&'a str, Tree<'a>)>,
}

fn tree(fields: &[Out]) -> Tree<'_> {
    let mut root = Tree::default();
    for f in fields {
        let mut node = &mut root;
        for part in f.name.split('.') {
            let i = match node.kids.iter().position(|(k, _)| *k == part) {
                Some(i) => i,
                None => {
                    node.kids.push((part, Tree::default()));
                    node.kids.len() - 1
                }
            };
            node = &mut node.kids[i].1;
        }
        node.leaf = Some(f);
    }
    root
}

fn xfdf(fields: &[Out], file: &str) -> String {
    fn walk(name: &str, t: &Tree, o: &mut String) {
        let _ = write!(o, "<field name=\"{}\">", esc(name));
        if let Some(f) = t.leaf {
            // An empty field is written as an empty value (as JSON "" and FDF () are), so importing clears it.
            for v in f.values.iter().map(String::as_str).chain(f.values.is_empty().then_some("")) {
                let _ = write!(o, "<value>{}</value>", esc(v));
            }
        }
        for (k, kid) in &t.kids {
            walk(k, kid, o);
        }
        o.push_str("</field>\n");
    }
    let mut o = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n");
    let _ = writeln!(o, "<f href=\"{}\"/>", esc(file));
    o.push_str("<fields>\n");
    for (k, kid) in &tree(fields).kids {
        walk(k, kid, &mut o);
    }
    o.push_str("</fields>\n</xfdf>\n");
    o
}

fn fdf(fields: &[Out], file: &str) -> Vec<u8> {
    fn value(f: &Out) -> String {
        match f.kind {
            FieldKind::Checkbox | FieldKind::Radio => pdf_name(f.values.first().map_or("Off", String::as_str)),
            _ if f.multi || f.values.len() > 1 => format!("[{}]", f.values.iter().map(|v| pdf_text(v)).collect::<Vec<_>>().join(" ")),
            _ => pdf_text(f.values.first().map_or("", String::as_str)),
        }
    }
    fn walk(name: &str, t: &Tree, o: &mut String) {
        let _ = write!(o, "<< /T {}", pdf_text(name));
        if let Some(f) = t.leaf {
            let _ = write!(o, " /V {}", value(f));
        }
        if !t.kids.is_empty() {
            o.push_str(" /Kids [");
            for (k, kid) in &t.kids {
                walk(k, kid, o);
                o.push(' ');
            }
            o.push(']');
        }
        o.push_str(" >>");
    }
    let mut list = String::new();
    for (k, kid) in &tree(fields).kids {
        walk(k, kid, &mut list);
        list.push(' ');
    }
    let obj1 = format!("<< /FDF << /Fields [{list}] /F {} >> >>", pdf_text(file));
    let mut out = b"%FDF-1.2\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let off = out.len();
    out.extend_from_slice(format!("1 0 obj\n{obj1}\nendobj\n").as_bytes());
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 2\n0000000000 65535 f \n{off:010} 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

/// What an export wrote.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Exported {
    pub count: usize,
}

/// The file bytes for every field's value. Read-only: allowed on a signed or certified document.
pub fn export(eng: &Engine, doc: DocId, format: Format, file_name: &str) -> Result<(Vec<u8>, Exported)> {
    let fields = eng.read_custom(doc, collect)?;
    let bytes = match format {
        Format::Json => json(&fields).into_bytes(),
        Format::Fdf => fdf(&fields, file_name),
        Format::Xfdf => xfdf(&fields, file_name).into_bytes(),
    };
    Ok((bytes, Exported { count: fields.len() }))
}

// ---- parsing the files ---------------------------------------------------------------------------

/// A value as the file gives it.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Text(String),
    Many(Vec<String>),
    Bool(bool),
    /// Something a field cannot take (a number-less object, a nested list ...); the reason.
    Invalid(&'static str),
}

#[derive(Debug, Default)]
pub struct Parsed {
    /// Full field name and value, in file order.
    pub fields: Vec<(String, Val)>,
    /// Entries left out (past the per-file limit, or with a name over 1 MB): counted, not read.
    pub over: usize,
    /// The first of them, with why, so the summary can name them.
    pub left_out: Vec<(String, &'static str)>,
}

impl Parsed {
    fn leave_out(&mut self, name: &str, why: &'static str) {
        self.over += 1;
        if self.left_out.len() < LISTED {
            self.left_out.push((name.chars().take(60).collect(), why));
        }
    }
}

const OVER_LIMIT: &str = "past the limit of 10 000 entries in one file";

pub fn parse_json(bytes: &[u8]) -> Result<Parsed> {
    // serde_json stops at 128 levels; the cap here is 64, and counting is cheap. Values are counted the same
    // way (commas and openings) so a 32 MB file of tiny pairs is refused before anything is built from it.
    let (mut depth, mut nodes, mut in_str, mut esc_next) = (0usize, 0usize, false, false);
    for &b in bytes {
        if in_str {
            match (esc_next, b) {
                (true, _) => esc_next = false,
                (false, b'\\') => esc_next = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_str = true,
                b'[' | b'{' => {
                    depth += 1;
                    nodes += 1;
                    if depth > MAX_DEPTH {
                        return Err(bad("This file is nested more than 64 levels deep, so Inkwell does not read it."));
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                b',' => nodes += 1,
                _ => {}
            }
            if nodes > MAX_NODES {
                return Err(bad("This file has more than 100 000 values, so Inkwell does not read it."));
            }
        }
    }
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| bad(format!("This is not a JSON file Inkwell can read: {e}")))?;
    let serde_json::Value::Object(map) = v else { return Err(bad("Form data JSON is one object: { \"field name\": value }.")) };
    let mut out = Parsed::default();
    for (k, v) in map {
        if out.fields.len() >= MAX_ITEMS {
            out.leave_out(&k, OVER_LIMIT);
            continue;
        }
        let str_ok = |s: &str| s.len() <= MAX_STR;
        let val = match v {
            serde_json::Value::String(s) if str_ok(&s) => Val::Text(s),
            serde_json::Value::String(_) => Val::Invalid("the value is longer than 1 MB"),
            serde_json::Value::Bool(b) => Val::Bool(b),
            serde_json::Value::Null => Val::Text(String::new()),
            serde_json::Value::Number(n) => Val::Text(n.to_string()),
            serde_json::Value::Array(a) if a.len() <= MAX_ITEMS => {
                let strings: Option<Vec<String>> = a
                    .into_iter()
                    .map(|x| match x {
                        serde_json::Value::String(s) if str_ok(&s) => Some(s),
                        serde_json::Value::Number(n) => Some(n.to_string()),
                        _ => None,
                    })
                    .collect();
                strings.map_or(Val::Invalid("a list takes text values only"), Val::Many)
            }
            _ => Val::Invalid("a field takes text, a list of text or on/off"),
        };
        if k.len() <= MAX_STR {
            out.fields.push((k, val));
        } else {
            out.leave_out(&k, "the field name is longer than 1 MB");
        }
    }
    Ok(out)
}

pub fn parse_xfdf(bytes: &[u8]) -> Result<Parsed> {
    let root = exchange::parse_xml(bytes)?;
    if root.name != "xfdf" {
        return Err(bad("This is not an XFDF file."));
    }
    let mut out = Parsed::default();
    let Some(fields) = root.child("fields") else { return Ok(out) };
    // Depth is bounded by the reader (64), so recursion here is too.
    fn walk(n: &exchange::Node, prefix: &str, out: &mut Parsed) {
        for f in n.children("field") {
            let Some(part) = f.attr("name") else { continue };
            let name = if prefix.is_empty() { part.to_string() } else { format!("{prefix}.{part}") };
            let values: Vec<String> = f.children("value").map(|v| v.text.clone()).collect();
            if !values.is_empty() {
                if out.fields.len() >= MAX_ITEMS {
                    out.leave_out(&name, OVER_LIMIT);
                } else {
                    let v = if values.len() == 1 { Val::Text(values.into_iter().next().unwrap_or_default()) } else { Val::Many(values) };
                    out.fields.push((name.clone(), v));
                }
            }
            walk(f, &name, out);
        }
    }
    walk(fields, "", &mut out);
    Ok(out)
}

/// An FDF `/V`. Every string copied is charged to `b` (one object can be referred to from many fields).
fn fdf_value(v: &PdfObject, b: &Budget) -> Val {
    let one = |o: &PdfObject| -> Option<String> {
        if b.gone() {
            return None;
        }
        // An indirect string is read once; each use is a clone and is charged.
        let object = if o.is_indirect().unwrap_or(false) { o.as_indirect().ok() } else { None };
        let s = match object.and_then(|n| b.string_of(n)) {
            Some(s) => Some(s),
            None => {
                let s = if o.is_string().unwrap_or(false) {
                    o.as_string().ok().or_else(|| o.as_bytes().ok().map(|b| super::decode_text(&b)))
                } else {
                    text(o)
                };
                if let (Some(n), Some(s)) = (object, &s) {
                    b.keep_string(n, s);
                }
                s
            }
        };
        if !b.take(s.as_ref().map_or(0, String::len)) {
            return None;
        }
        s.filter(|s| s.len() <= MAX_STR)
    };
    if v.is_array().unwrap_or(false) {
        let n = v.len().unwrap_or(0);
        if n > MAX_ITEMS {
            return Val::Invalid("a list of more than 10 000 values");
        }
        let all: Option<Vec<String>> = (0..n as i32).map(|i| v.get_array(i).ok().flatten().and_then(|x| one(&x))).collect();
        return all.map_or(Val::Invalid("a list takes text values only"), Val::Many);
    }
    if v.is_bool().unwrap_or(false) {
        return v.as_bool().map_or(Val::Invalid("not a value"), Val::Bool);
    }
    one(v).map_or(Val::Invalid("a field takes text, a list of text or on/off"), Val::Text)
}

/// Reads `/Fields` (with `/Kids`) by an explicit stack: depth, node count and revisits are bounded, and
/// only `/T`, `/V` and `/Kids` of a field are looked at: never `/AA`, `/JavaScript` or `/F`.
pub fn parse_fdf(bytes: &[u8]) -> Result<Parsed> {
    exchange::with_fdf(bytes, |root| {
        let mut out = Parsed::default();
        let budget = Budget::new();
        let Some(top) = get(root, "Fields").filter(|a| a.is_array().unwrap_or(false)) else { return Ok(out) };
        let mut stack: Vec<(PdfObject, String, usize)> = Vec::new();
        let kids_of = |a: &PdfObject, prefix: &str, depth: usize, stack: &mut Vec<(PdfObject, String, usize)>| {
            let n = a.len().unwrap_or(0);
            if n > MAX_NODES {
                // Fails closed: the file is refused, not read in part.
                budget.fail();
                return;
            }
            for i in (0..n as i32).rev() {
                // Each kid holds a copy of its parent's name.
                if !budget.take(prefix.len()) {
                    break;
                }
                if let Some(k) = a.get_array(i).ok().flatten() {
                    stack.push((k, prefix.to_string(), depth));
                }
            }
        };
        kids_of(&top, "", 0, &mut stack);
        let mut seen: HashSet<i32> = HashSet::new();
        let mut nodes = 0usize;
        while let Some((f, prefix, depth)) = stack.pop() {
            budget.check()?;
            nodes += 1;
            if nodes > MAX_NODES {
                return Err(bad("This FDF file has more than 100 000 fields, so Inkwell does not read it."));
            }
            if !f.is_dict().unwrap_or(false) || depth >= MAX_DEPTH {
                continue;
            }
            if f.is_indirect().unwrap_or(false) && f.as_indirect().is_ok_and(|n| !seen.insert(n)) {
                continue;
            }
            let part = get(&f, "T").as_ref().and_then(text).or_else(|| get(&f, "T").and_then(|t| t.as_bytes().ok().map(|b| super::decode_text(&b)))).unwrap_or_default();
            if !budget.take(part.len()) {
                break;
            }
            let name = match (prefix.is_empty(), part.is_empty()) {
                (_, true) => prefix.clone(),
                (true, false) => part,
                (false, false) => format!("{prefix}.{part}"),
            };
            if !budget.take(name.len()) {
                break;
            }
            if let Some(v) = get(&f, "V") {
                if out.fields.len() >= MAX_ITEMS {
                    out.leave_out(&name, OVER_LIMIT);
                } else if !name.is_empty() && budget.take(name.len()) {
                    out.fields.push((name.clone(), fdf_value(&v, &budget)));
                }
            }
            if let Some(k) = get(&f, "Kids").filter(|a| a.is_array().unwrap_or(false)) {
                kids_of(&k, &name, depth + 1, &mut stack);
            }
        }
        budget.check()?;
        Ok(out)
    })
}

/// A file's field values, by what it is (see [`exchange::sniff`]).
pub fn parse(bytes: &[u8]) -> Result<Parsed> {
    match exchange::sniff(bytes) {
        Some(Format::Xfdf) => parse_xfdf(bytes),
        Some(Format::Fdf) => parse_fdf(bytes),
        Some(Format::Json) => parse_json(bytes),
        None => Err(bad("This is not an XFDF, FDF or JSON file.")),
    }
}

// ---- import --------------------------------------------------------------------------------------

/// Skipped fields listed to the person; the count says how many there were in all.
const LISTED: usize = 50;

#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Fields whose value changed.
    pub set: usize,
    /// Fields that already had the value in the file.
    pub unchanged: usize,
    pub skipped: Vec<Skipped>,
    /// All skipped, listed or not.
    pub skipped_total: usize,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub summary: Summary,
    pub state: EditState,
    pub signed: bool,
}

fn skip(sum: &mut Summary, name: &str, reason: impl Into<String>) {
    sum.skipped_total += 1;
    if sum.skipped.len() < LISTED {
        sum.skipped.push(Skipped { name: name.to_string(), reason: reason.into() });
    }
}

/// What to fill `e` with for `val`, or why it cannot take it.
fn value_for(entries: &[Entry], e: &Entry, val: &Val) -> std::result::Result<FieldValue, String> {
    let strings: Vec<String> = match val {
        Val::Invalid(r) => return Err((*r).to_string()),
        Val::Bool(b) => {
            return if matches!(e.kind, FieldKind::Checkbox | FieldKind::Radio) { Ok(FieldValue::Checked(*b)) } else { Err("this field takes text, not on/off".into()) };
        }
        Val::Text(s) => vec![s.clone()],
        Val::Many(v) => v.clone(),
    };
    let ff = field_ff(e);
    match e.kind {
        FieldKind::Checkbox | FieldKind::Radio => {
            let [one] = strings.as_slice() else { return Err("a check box or radio button takes one value".into()) };
            let on: Vec<String> = entries.iter().filter(|w| w.node == e.node).map(|w| on_state(&w.obj).unwrap_or_else(|| "Yes".into())).collect();
            if one.is_empty() || one == "Off" || on.contains(one) {
                Ok(FieldValue::Text(one.clone()))
            } else {
                Err(format!("\"{one}\" is not one of its values ({})", on.join(", ")))
            }
        }
        FieldKind::Text => match strings.as_slice() {
            [] => Ok(FieldValue::Text(String::new())),
            [one] => Ok(FieldValue::Text(one.clone())),
            _ => Err("a text field takes one value".into()),
        },
        FieldKind::Combo | FieldKind::List => {
            let multi = e.kind == FieldKind::List && flag(ff, FieldFlags::MULTI_SELECT);
            if strings.len() > 1 && !multi {
                return Err("this field takes one value".into());
            }
            let open = e.kind == FieldKind::Combo && flag(ff, FieldFlags::EDIT);
            let options = options_of(&e.field);
            let mut fixed = Vec::with_capacity(strings.len());
            for s in strings {
                if s.is_empty() || open {
                    fixed.push(s);
                } else if let Some(o) = options.iter().find(|o| o.export == s).or_else(|| options.iter().find(|o| o.label == s)) {
                    fixed.push(o.export.clone());
                } else {
                    return Err(format!("\"{s}\" is not one of its choices"));
                }
            }
            Ok(if multi { FieldValue::Many(fixed) } else { FieldValue::Text(fixed.into_iter().next().unwrap_or_default()) })
        }
        FieldKind::Button | FieldKind::Signature => Err("this field has no value to fill".into()),
    }
}

/// Fills the fields of `parsed` in `rt`'s document. The caller has checked that it is not certified and
/// runs this inside one edit step. Returns the summary and the pages touched.
fn fill(rt: &Rt, parsed: &Parsed) -> Result<(Summary, Vec<u32>)> {
    let entries = rt.entries()?;
    let locks = scripts::locks_of(rt.doc(), entries);
    let mut by_name: HashMap<&str, &Entry> = HashMap::new();
    for e in per_field(entries) {
        by_name.entry(e.name.as_str()).or_insert(e);
    }
    let mut sum = Summary::default();
    let mut touched = Vec::new();
    for (name, val) in &parsed.fields {
        let Some(e) = by_name.get(name.as_str()).copied() else {
            skip(&mut sum, name, "there is no field with this name");
            continue;
        };
        if matches!(e.kind, FieldKind::Button | FieldKind::Signature) {
            skip(&mut sum, name, "this field has no value to fill");
            continue;
        }
        if flag(field_ff(e), FieldFlags::READ_ONLY) {
            skip(&mut sum, name, "the field is read-only");
            continue;
        }
        if scripts::locked(&locks, e) {
            skip(&mut sum, name, "the field is locked by a signature");
            continue;
        }
        let value = match value_for(entries, e, val) {
            Ok(v) => v,
            Err(r) => {
                skip(&mut sum, name, r);
                continue;
            }
        };
        if fill_is_noop(entries, e, &value) {
            sum.unchanged += 1;
            continue;
        }
        match set_value_rt(rt, e.page, e.xref, &value) {
            Ok(p) => {
                touched.extend(p);
                sum.set += 1;
            }
            Err(EngineError::Invalid(m)) => skip(&mut sum, name, m),
            Err(other) => skip(&mut sum, name, other.to_string()),
        }
    }
    for (name, why) in &parsed.left_out {
        skip(&mut sum, name, *why);
    }
    sum.skipped_total += parsed.over.saturating_sub(parsed.left_out.len());
    touched.sort_unstable();
    touched.dedup();
    Ok((sum, touched))
}

/// Imports parsed field values as one undo step (none when nothing changes). A certified document is
/// refused before anything changes; a signed one is changed incrementally and the result says so.
pub fn import_one(eng: &Engine, doc: DocId, parsed: &Parsed, source: EditSource) -> Result<Imported> {
    let signed = eng.read_custom(doc, |d| Ok(crate::outline::signed(d)))?;
    let (scripts, name) = eng.doc_scripts(doc)?;
    let mut summary = Summary::default();
    let state = eng.apply_custom_unless_noop(doc, "Import form data".into(), source, false, |d| {
        // A certification that allows form filling (`/P` 2 or 3) takes values into the fields that exist (nothing
        // else is made or changed); `/P` 1, and one that cannot be read, do not.
        if !exchange::forms_allowed(crate::outline::certified(d)?) {
            return Err(bad("This document is certified, and its certification does not allow filling in form fields. Importing form data would void it, so Inkwell leaves the document as it is."));
        }
        let rt = rt_of(d, &scripts, &name);
        let (s, pages) = fill(&rt, parsed)?;
        // Alerts and requests (submit, links, print) of the field scripts are dropped, never acted on.
        drop(rt.report.take());
        let changed = s.set > 0;
        summary = s;
        Ok(changed.then_some(pages))
    })?;
    Ok(Imported { summary, state, signed })
}

#[cfg(test)]
mod tests;
