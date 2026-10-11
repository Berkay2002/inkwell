//! The glue between the form model (`mod.rs`) and the script engine (`js.rs`): what a script is told about
//! the fields, what a script's changes do to the document, and which events take the native fast path.
//!
//! One [`Rt`] lives for one engine step (one `apply_custom` closure, one listing). It owns the field scan
//! (the entries are live handles, so a value written during the step is read back by the next script),
//! the document's script switch and what the scripts raised (alerts, requests).

use super::af::{self, Calculate, Format, Validate};
use super::author::{color_array, ensure_dict, set_flag};
use super::js::{
    Alert, DocInfo, DocScripts, Event, FieldChange, FieldState, FieldType, Job, LogLevel, Outcome, Prop, Request, Scripts, CALC_TIME, CASCADE_TIME, DISPLAY_HIDDEN, DISPLAY_NO_PRINT, DISPLAY_NO_VIEW,
    DISPLAY_VISIBLE, EVENT_TIME, JS_BUDGET, MAX_ALERTS_PER_STEP, MAX_DOC_SCRIPT_BYTES,
};
use super::{
    Entry, FieldKind, FieldValue, Result, aa_js, da_of, field_ff, flag, get, geti, int_of, names_to_values, nums, on_state, options_of, parse_da, per_field, put_int, put_nums, put_str,
    refresh_with, text, text_or_stream, values_of, write_values,
};
use mupdf::pdf::{FieldFlags, PdfDocument, PdfObject};
use serde::Serialize;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// What the scripts of a step raised, for the UI.
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub alerts: Vec<Alert>,
    pub requests: Vec<Request>,
}

/// The edit state after a change and what the scripts raised during it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Stepped {
    #[serde(flatten)]
    pub state: crate::engine::edit::EditState,
    #[serde(flatten)]
    pub report: Report,
}

/// How a field event is handled: nothing to run, the native code of `af.rs`, or the script engine.
pub(crate) enum Plan<T> {
    None,
    Fast(T),
    Js(String),
}

/// The requests of one step that reach the UI.
const MAX_REQUESTS_PER_STEP: usize = 64;
/// A script slice shorter than this is not worth starting: the budget counts as used up.
const MIN_SLICE: Duration = Duration::from_millis(10);

pub(crate) struct Rt<'a> {
    doc: &'a PdfDocument,
    scripts: Option<&'a Scripts>,
    doc_js: Option<std::sync::Arc<DocScripts>>,
    file_name: String,
    entries: OnceCell<Vec<Entry>>,
    info: OnceCell<DocInfo>,
    pub report: RefCell<Report>,
    /// Script time this step has used, and whether that used up [`JS_BUDGET`] (the rest of the step is native only).
    spent: Cell<Duration>,
    over: Cell<bool>,
    /// Alerts held back past [`MAX_ALERTS_PER_STEP`], and how many the last script's run added to the report.
    suppressed: Cell<usize>,
    last_added: Cell<usize>,
    /// What a keystroke script changed and is not written yet: the validate script that follows sees it.
    pending: RefCell<Vec<FieldChange>>,
}

fn kind_type(k: FieldKind) -> FieldType {
    match k {
        FieldKind::Text => FieldType::Text,
        FieldKind::Checkbox => FieldType::CheckBox,
        FieldKind::Radio => FieldType::RadioButton,
        FieldKind::Combo => FieldType::ComboBox,
        FieldKind::List => FieldType::ListBox,
        FieldKind::Button => FieldType::Button,
        FieldKind::Signature => FieldType::Signature,
    }
}

/// Every `/Names /JavaScript` entry of the document, in name order, with the name tree's depth and size bounded.
pub(crate) fn load_doc_js(doc: &PdfDocument) -> DocScripts {
    fn walk(node: &PdfObject, out: &mut Vec<(String, String)>, depth: usize, seen: &mut usize) {
        if depth > 8 || *seen > 4096 {
            return;
        }
        *seen += 1;
        if let Some(names) = get(node, "Names") {
            let n = names.len().unwrap_or(0).min(4096) as i32;
            let mut i = 0;
            while i + 1 < n {
                let name = names.get_array(i).ok().flatten().and_then(|o| text(&o)).unwrap_or_default();
                let js = names.get_array(i + 1).ok().flatten().and_then(|a| get(&a, "JS")).as_ref().and_then(text_or_stream);
                if let Some(js) = js {
                    out.push((name, js));
                }
                i += 2;
            }
        }
        if let Some(kids) = get(node, "Kids") {
            for i in 0..kids.len().unwrap_or(0).min(256) as i32 {
                if let Some(k) = kids.get_array(i).ok().flatten() {
                    walk(&k, out, depth + 1, seen);
                }
            }
        }
    }
    let mut out = Vec::new();
    if let Some(root) = doc.catalog().ok().and_then(|c| get(&c, "Names")).and_then(|n| get(&n, "JavaScript")) {
        let mut seen = 0;
        walk(&root, &mut out, 0, &mut seen);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    // A pathological document must not make the list itself huge; the engine applies the real limits.
    let mut total = 0usize;
    out.retain(|(_, s)| {
        total += s.len();
        total <= MAX_DOC_SCRIPT_BYTES * 4
    });
    DocScripts::new(out)
}

fn doc_info(doc: &PdfDocument, file_name: &str) -> DocInfo {
    let mut info = Vec::new();
    if let Some(i) = doc.trailer().ok().and_then(|t| get(&t, "Info")) {
        for (k, lower) in [("Title", "title"), ("Author", "author"), ("Subject", "subject"), ("Keywords", "keywords"), ("Creator", "creator"), ("Producer", "producer")] {
            if let Some(v) = get(&i, k).as_ref().and_then(text) {
                info.push((lower.to_string(), v));
            }
        }
    }
    DocInfo { file_name: file_name.to_string(), num_pages: doc.page_count().unwrap_or(0).max(0) as usize, page: 0, info }
}

fn rgb_parts(c: [f32; 3]) -> Vec<String> {
    vec!["RGB".into(), c[0].to_string(), c[1].to_string(), c[2].to_string()]
}

/// The numbers of an Acrobat colour array (`["RGB", r, g, b]`), clamped to 0..=1.
fn parts_f32(p: &[String]) -> Vec<f32> {
    p.iter().skip(1).map(|s| s.parse::<f32>().ok().filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 1.0)).collect()
}

/// The widget's `/F` as Acrobat's `display`.
fn display_of(f: i32) -> i32 {
    if f & 2 != 0 {
        DISPLAY_HIDDEN
    } else if f & 32 != 0 {
        DISPLAY_NO_VIEW
    } else if f & 4 == 0 {
        DISPLAY_NO_PRINT
    } else {
        DISPLAY_VISIBLE
    }
}

/// A field (all its widgets) as scripts see it, read from the live objects.
pub(crate) fn field_state(entries: &[Entry], e: &Entry) -> FieldState {
    let mk = get(&e.obj, "MK");
    let mk_col = |k: &str| {
        let n = mk.as_ref().and_then(|m| get(m, k)).map(|a| nums(&a))?;
        match n.len() {
            1 => Some(vec!["G".to_string(), n[0].to_string()]),
            3 => Some(rgb_parts([n[0], n[1], n[2]])),
            4 => Some(["CMYK".to_string()].into_iter().chain(n.iter().map(|v| v.to_string())).collect()),
            _ => None,
        }
    };
    let widgets: Vec<&Entry> = entries.iter().filter(|w| w.node == e.node).collect();
    let boxed = matches!(e.kind, FieldKind::Checkbox | FieldKind::Radio);
    let mut value = values_of(geti(&e.field, "V"));
    if boxed {
        value.retain(|v| v != "Off" && !v.is_empty());
        if value.is_empty() {
            // /V missing: the state a widget shows.
            if let Some(s) = widgets.iter().find_map(|w| get(&w.obj, "AS").as_ref().and_then(text).filter(|s| s != "Off")) {
                value.push(s);
            }
        }
        value.truncate(1);
    }
    let options = if boxed {
        widgets.iter().filter_map(|w| on_state(&w.obj)).map(|s| (s.clone(), s)).collect()
    } else {
        options_of(&e.obj).into_iter().map(|o| (o.export, o.label)).collect()
    };
    let (_, _, text_color) = parse_da(&da_of(&e.obj));
    let ff = field_ff(e);
    let max = int_of(&e.obj, "MaxLen").unwrap_or(0);
    FieldState {
        name: e.name.clone(),
        kind: kind_type(e.kind),
        value,
        default: values_of(geti(&e.field, "DV")),
        readonly: flag(ff, FieldFlags::READ_ONLY),
        required: flag(ff, FieldFlags::REQUIRED),
        display: display_of(int_of(&e.obj, "F").unwrap_or(4)),
        options,
        char_limit: (max > 0).then_some(max as usize),
        text_color: Some(rgb_parts(text_color)),
        fill_color: mk_col("BG"),
        stroke_color: mk_col("BC"),
    }
}

/// Every field once, as scripts see it.
pub(crate) fn field_states(entries: &[Entry]) -> Vec<FieldState> {
    per_field(entries).into_iter().map(|e| field_state(entries, e)).collect()
}

impl<'a> Rt<'a> {
    /// Scripts run when `scripts` is given and not turned off or taken down. The document-level scripts are
    /// read the first time.
    pub(crate) fn new(doc: &'a PdfDocument, scripts: Option<&'a Scripts>, file_name: &str) -> Rt<'a> {
        let scripts = scripts.filter(|s| s.usable());
        let doc_js = scripts.map(|s| s.doc_scripts(|| load_doc_js(doc)));
        Rt {
            doc,
            scripts,
            doc_js,
            file_name: file_name.to_string(),
            entries: OnceCell::new(),
            info: OnceCell::new(),
            report: RefCell::new(Report::default()),
            spent: Cell::new(Duration::ZERO),
            over: Cell::new(false),
            suppressed: Cell::new(0),
            last_added: Cell::new(0),
            pending: RefCell::new(Vec::new()),
        }
    }

    /// Whether the document has scripts the engine runs, for the banner (read with the listing, kept by the scripts).
    pub(crate) fn remember_has_scripts(&self, entries: &[Entry]) {
        if let Some(s) = self.scripts {
            s.set_has_scripts(has_scripts_with(self.doc_js.as_deref(), self.doc, entries));
        }
    }

    /// Native code only.
    pub(crate) fn off(doc: &'a PdfDocument) -> Rt<'a> {
        Rt::new(doc, None, "")
    }

    pub(crate) fn doc(&self) -> &'a PdfDocument {
        self.doc
    }

    pub(crate) fn entries(&self) -> Result<&[Entry]> {
        if self.entries.get().is_none() {
            let _ = self.entries.set(super::scan(self.doc)?);
        }
        Ok(self.entries.get().map(Vec::as_slice).unwrap_or_default())
    }

    /// This step has used its script time: scripts are skipped for the rest of it.
    pub(crate) fn budget_gone(&self) -> bool {
        self.over.get()
    }

    /// Scripts may run now.
    pub(crate) fn js_on(&self) -> bool {
        self.scripts.is_some_and(|s| s.usable())
    }

    /// The recognised calls may run natively. A document that redefines an `AF...` function gets its own.
    fn fast_ok(&self) -> bool {
        !(self.js_on() && self.doc_js.as_ref().is_some_and(|d| d.overrides_af))
    }

    pub(crate) fn note(&self, level: LogLevel, field: Option<&str>, message: String) {
        if let Some(s) = self.scripts {
            s.note(level, field, message);
        }
    }

    // ---- which events run where ------------------------------------------------------------------

    /// The Format event (what the field shows).
    pub(crate) fn display_plan(&self, e: &Entry) -> Plan<Format> {
        let fallback = || match super::format_of(e) {
            Format::None => Plan::None,
            f if self.fast_ok() => Plan::Fast(f),
            _ => Plan::None,
        };
        match aa_js(e, "F") {
            Some(src) => {
                if self.fast_ok() {
                    if let Some(f) = af::parse_format(&src) {
                        return Plan::Fast(f);
                    }
                }
                if self.js_on() { Plan::Js(src) } else { fallback() }
            }
            // A mask is only in the keystroke script.
            None => fallback(),
        }
    }

    /// The Keystroke event on commit. A field without a keystroke script still holds to its format.
    pub(crate) fn key_plan(&self, e: &Entry) -> Plan<Format> {
        let fallback = || match super::format_of(e) {
            Format::None => Plan::None,
            f if self.fast_ok() => Plan::Fast(f),
            _ => Plan::None,
        };
        match aa_js(e, "K") {
            Some(src) => {
                if self.fast_ok() {
                    if let Some(f) = af::parse_keystroke(&src) {
                        return Plan::Fast(f);
                    }
                }
                if self.js_on() { Plan::Js(src) } else { fallback() }
            }
            None => fallback(),
        }
    }

    pub(crate) fn validate_plan(&self, e: &Entry) -> Plan<Validate> {
        let Some(src) = aa_js(e, "V") else { return Plan::None };
        if self.fast_ok() {
            if let Some(v) = af::parse_validate(&src) {
                return Plan::Fast(v);
            }
        }
        if self.js_on() { Plan::Js(src) } else { Plan::None }
    }

    pub(crate) fn calc_plan(&self, e: &Entry) -> Plan<Calculate> {
        let Some(src) = aa_js(e, "C") else { return Plan::None };
        if self.fast_ok() {
            if let Some(c) = af::parse_calculate(&src) {
                return Plan::Fast(c);
            }
        }
        if self.js_on() { Plan::Js(src) } else { Plan::None }
    }

    // ---- running a script --------------------------------------------------------------------------

    fn info(&self) -> &DocInfo {
        self.info.get_or_init(|| doc_info(self.doc, &self.file_name))
    }

    /// Runs one event script of `e`. `fields` is what the script sees; `None` takes a snapshot now.
    ///
    /// The snapshot of a keystroke or validate takes the event's value as the field's own: the entry is not
    /// stored yet, and `event.target.value` is what was typed.
    pub(crate) fn run(&self, fields: Option<&[FieldState]>, e: &Entry, event: Event, script: String, timeout: Duration) -> Outcome {
        let skipped = Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), skipped: true, ..Default::default() };
        let Some(scripts) = self.scripts.filter(|s| s.usable()) else { return skipped };
        // The step's script time is used up: the rest of it runs natively only.
        let left = JS_BUDGET.saturating_sub(self.spent.get());
        if self.over.get() || left < MIN_SLICE {
            self.exhaust(scripts);
            return skipped;
        }
        let owned;
        let fields = match fields {
            Some(f) => f,
            None => {
                let Ok(entries) = self.entries() else { return skipped };
                let mut states = field_states(entries);
                lay_over(&mut states, &self.pending.borrow());
                if matches!(event.name, "Keystroke" | "Validate") {
                    if let Some(f) = states.iter_mut().find(|f| f.name == e.name && matches!(f.kind, FieldType::Text | FieldType::ComboBox)) {
                        f.value = if event.value.is_empty() { Vec::new() } else { vec![event.value.clone()] };
                    }
                }
                owned = states;
                &owned
            }
        };
        let started = Instant::now();
        let out = scripts.run(Job { key: e.name.clone(), event, script, fields: fields.to_vec(), doc: self.info().clone(), timeout: timeout.min(left) });
        self.spent.set(self.spent.get() + started.elapsed());
        if self.spent.get() >= JS_BUDGET {
            self.exhaust(scripts);
        }
        let mut rep = self.report.borrow_mut();
        // A script that raises alerts in a loop shows a few of them; the rest are counted in the log.
        let room = MAX_ALERTS_PER_STEP.saturating_sub(rep.alerts.len());
        let added = out.alerts.len().min(room);
        rep.alerts.extend(out.alerts.iter().take(added).cloned());
        self.suppressed.set(self.suppressed.get() + out.alerts.len() - added);
        self.last_added.set(added);
        let room = MAX_REQUESTS_PER_STEP.saturating_sub(rep.requests.len());
        rep.requests.extend(out.requests.iter().take(room).cloned());
        out
    }

    /// The step has used up its script time: native only from here, and a strike toward taking the scripts down.
    fn exhaust(&self, scripts: &Scripts) {
        if !self.over.replace(true) {
            scripts.over_budget();
        }
    }

    /// What the field shows for the stored value `raw`.
    pub(crate) fn shown(&self, fields: Option<&[FieldState]>, e: &Entry, raw: &str) -> String {
        if raw.is_empty() {
            return String::new();
        }
        match self.display_plan(e) {
            Plan::None => raw.to_string(),
            Plan::Fast(f) => af::format_value(&f, raw),
            Plan::Js(src) => {
                let out = self.run(fields, e, Event::new("Format", &e.name, raw), src, EVENT_TIME);
                if out.skipped || out.error.is_some() { raw.to_string() } else { out.value }
            }
        }
    }

    /// The Keystroke event on commit: the value to store and the fields the script changed, or the
    /// message that rejects the entry.
    pub(crate) fn keystroke(&self, e: &Entry, value: &str) -> std::result::Result<(String, Vec<FieldChange>), String> {
        match self.key_plan(e) {
            Plan::None => Ok((value.to_string(), Vec::new())),
            Plan::Fast(f) => af::keystroke(&f, &e.name, value).map(|v| (v, Vec::new())),
            Plan::Js(src) => {
                let out = self.run(None, e, Event::new("Keystroke", &e.name, value), src, EVENT_TIME);
                let (kept, changed) = self.verdict(out, value, format!("The value entered does not match the format of the field [ {} ]", e.name))?;
                self.pending.borrow_mut().extend(changed.iter().cloned());
                Ok((kept, changed))
            }
        }
    }

    /// The Validate event: the fields the script changed, or the message that rejects the value.
    pub(crate) fn validate(&self, e: &Entry, value: &str) -> std::result::Result<Vec<FieldChange>, String> {
        match self.validate_plan(e) {
            Plan::None => Ok(Vec::new()),
            Plan::Fast(v) => af::validate(&v, value).map(|()| Vec::new()),
            Plan::Js(src) => {
                let out = self.run(None, e, Event::new("Validate", &e.name, value), src, EVENT_TIME);
                self.verdict(out, value, format!("Invalid value entered for the field [ {} ]", e.name)).map(|(_, changed)| changed)
            }
        }
    }

    /// A key or validate script's result: `event.rc` false rejects with the alert text (or `default`);
    /// a failed or skipped script changes nothing.
    fn verdict(&self, out: Outcome, value: &str, default: String) -> std::result::Result<(String, Vec<FieldChange>), String> {
        if out.skipped || out.error.is_some() {
            return Ok((value.to_string(), Vec::new()));
        }
        if !out.rc {
            // The alert is the reason: the UI shows it as the error, not a second time as an alert.
            let mut rep = self.report.borrow_mut();
            let keep = rep.alerts.len().saturating_sub(self.last_added.get());
            rep.alerts.truncate(keep);
            let why: Vec<String> = out.alerts.into_iter().map(|a| a.message).collect();
            let msg = why.join("\n");
            return Err(if msg.is_empty() { default } else { msg });
        }
        Ok((out.value, out.changed))
    }
}

/// Lays what earlier scripts of the step set (and the document does not hold yet) over a snapshot, property by property.
fn lay_over(states: &mut [FieldState], changes: &[FieldChange]) {
    for c in changes {
        let Some(s) = states.iter_mut().find(|s| s.name == c.state.name) else { continue };
        for p in &c.props {
            match p {
                Prop::Value => s.value = c.state.value.clone(),
                Prop::ReadOnly => s.readonly = c.state.readonly,
                Prop::Required => s.required = c.state.required,
                Prop::Display => s.display = c.state.display,
                Prop::TextColor => s.text_color = c.state.text_color.clone(),
                Prop::FillColor => s.fill_color = c.state.fill_color.clone(),
                Prop::StrokeColor => s.stroke_color = c.state.stroke_color.clone(),
            }
        }
    }
}

// ---- what a script changed, written to the document -------------------------------------------------

/// Sets or clears one `/MK` colour of a widget from an Acrobat colour array.
fn put_mk_colour(doc: &PdfDocument, widget: &mut PdfObject, key: &str, c: &Option<Vec<String>>) -> Result<()> {
    let mut mk = ensure_dict(doc, widget, "MK")?;
    let v = c.as_deref().map(parts_f32).unwrap_or_default();
    match (c.as_deref().and_then(|p| p.first()).map(String::as_str), v.len()) {
        (Some("RGB"), 3) => color_array(doc, &mut mk, key, [v[0], v[1], v[2]])?,
        (Some("G"), 1) | (Some("CMYK"), 4) => put_nums(doc, &mut mk, key, &v)?,
        // Transparent, or not a colour.
        _ => mk.dict_delete(key)?,
    }
    widget.dict_put("MK", mk)?;
    Ok(())
}

/// What a signed document keeps scripts from changing: every field, or those a signature's `/Lock` names.
pub(crate) enum Lock {
    All,
    Include(Vec<String>),
    Exclude(Vec<String>),
}

/// The names in a `/Lock /Fields` array.
fn lock_names(lock: &PdfObject) -> Vec<String> {
    let Some(a) = get(lock, "Fields") else { return Vec::new() };
    (0..a.len().unwrap_or(0).min(4096) as i32).filter_map(|i| a.get_array(i).ok().flatten().as_ref().and_then(text)).collect()
}

/// What a signature's `/Lock` dictionary covers.
fn lock_of(l: &PdfObject) -> Lock {
    match get(l, "Action").as_ref().and_then(text).as_deref() {
        Some("Include") => Lock::Include(lock_names(l)),
        Some("Exclude") => Lock::Exclude(lock_names(l)),
        _ => Lock::All,
    }
}

/// Every lock the `/Reference` entries of a signature value carry: each FieldMDP method names its fields like `/Lock`
/// does, a DocMDP with `/P 1` allows no change at all. An approval signature with neither locks nothing.
fn reference_locks(v: &PdfObject) -> Vec<Lock> {
    let Some(refs) = get(v, "Reference") else { return Vec::new() };
    (0..refs.len().unwrap_or(0).min(16) as i32)
        .filter_map(|i| {
            let r = refs.get_array(i).ok().flatten()?;
            let params = get(&r, "TransformParams")?;
            match get(&r, "TransformMethod").as_ref().and_then(text).as_deref() {
                Some("FieldMDP") => Some(lock_of(&params)),
                Some("DocMDP") if int_of(&params, "P") == Some(1) => Some(Lock::All),
                _ => None,
            }
        })
        .collect()
}

/// The locks of the signatures that carry a signature (`/V`): the field's `/Lock` and every one its `/Reference`
/// entries carry (Acrobat writes both when it certifies with locked fields, so a DocMDP `/P 1` anywhere locks
/// everything). A signature that names neither (a plain approval signature) leaves the form to be filled.
pub(crate) fn locks_of(entries: &[Entry]) -> Vec<Lock> {
    per_field(entries)
        .into_iter()
        .filter(|e| e.kind == FieldKind::Signature)
        .flat_map(|e| {
            let Some(v) = geti(&e.field, "V").filter(|v| v.is_dict().unwrap_or(false)) else { return Vec::new() };
            let mut locks = reference_locks(&v);
            locks.extend(get(&e.field, "Lock").or_else(|| get(&e.obj, "Lock")).map(|l| lock_of(&l)));
            locks
        })
        .collect()
}

/// Whether a signature in the document certifies it (DocMDP `/P` 2 or 3, 2 when absent). Such a document allows form
/// values to change, and nothing else: read-only, required, display, colours and text size would break the certification.
fn certified(entries: &[Entry]) -> bool {
    per_field(entries).into_iter().filter(|e| e.kind == FieldKind::Signature).any(|e| {
        let Some(v) = geti(&e.field, "V").filter(|v| v.is_dict().unwrap_or(false)) else { return false };
        let Some(refs) = get(&v, "Reference") else { return false };
        (0..refs.len().unwrap_or(0).min(16) as i32).any(|i| {
            let Some(r) = refs.get_array(i).ok().flatten() else { return false };
            get(&r, "TransformMethod").as_ref().and_then(text).as_deref() == Some("DocMDP")
                && matches!(get(&r, "TransformParams").and_then(|p| int_of(&p, "P")).unwrap_or(2), 2 | 3)
        })
    })
}

/// A script may not write to a signature widget, nor to a field a signature in the document covers.
pub(crate) fn locked(locks: &[Lock], e: &Entry) -> bool {
    let named = |names: &[String]| names.iter().any(|n| e.name == *n || e.name.starts_with(&format!("{n}.")));
    e.kind == FieldKind::Signature
        || locks.iter().any(|l| match l {
            Lock::All => true,
            Lock::Include(n) => named(n),
            Lock::Exclude(n) => !named(n),
        })
}

/// Applies what scripts wrote to fields to the document: the value (without running its keystroke or
/// validate, as in Acrobat), read-only, required, visibility and colours. Only the properties a script set
/// are written (a snapshot taken before the typed value was stored must not undo it), and nothing is written
/// to a signature or to a field a signature in the document covers. Returns the pages touched.
pub(crate) fn apply_changes(rt: &Rt, changed: &[FieldChange]) -> Result<Vec<u32>> {
    rt.pending.borrow_mut().clear();
    let doc = rt.doc();
    let entries = rt.entries()?;
    let locks = locks_of(entries);
    let values_only = certified(entries);
    let mut pages = Vec::new();
    for ch in changed {
        let new = &ch.state;
        let Some(e) = entries.iter().find(|e| e.name == new.name) else { continue };
        if locked(&locks, e) {
            rt.note(LogLevel::Warn, Some(&new.name), format!("a script's change to [ {} ] was not written: the document is signed", new.name));
            continue;
        }
        // A certified document takes values only: the other properties would break the certification.
        let props: BTreeSet<Prop> = if values_only { ch.props.iter().copied().filter(|p| *p == Prop::Value).collect() } else { ch.props.clone() };
        if props.len() < ch.props.len() {
            rt.note(LogLevel::Warn, Some(&new.name), format!("a script's change to the properties of [ {} ] was not written: the document is certified, so scripts may change values only", new.name));
        }
        let cur = field_state(entries, e);
        let widgets: Vec<&Entry> = entries.iter().filter(|w| w.node == e.node).collect();
        let mut touched = false;
        let mut redraw = false;
        if props.contains(&Prop::Value) && cur.value != new.value && !matches!(e.kind, FieldKind::Button | FieldKind::Signature) {
            pages.extend(write_values(rt, e, &new.value)?);
            touched = true;
        }
        let (ro, req) = (props.contains(&Prop::ReadOnly), props.contains(&Prop::Required));
        if (ro && cur.readonly != new.readonly) || (req && cur.required != new.required) {
            let mut ff = field_ff(e);
            if ro {
                set_flag(&mut ff, FieldFlags::READ_ONLY, new.readonly);
            }
            if req {
                set_flag(&mut ff, FieldFlags::REQUIRED, new.required);
            }
            let mut field = e.field.clone();
            put_int(&mut field, "Ff", ff)?;
            touched = true;
        }
        if props.contains(&Prop::Display) && cur.display != new.display {
            for w in &widgets {
                let f = int_of(&w.obj, "F").unwrap_or(4);
                let f = match new.display {
                    DISPLAY_HIDDEN => (f | 2) & !4,
                    DISPLAY_NO_PRINT => f & !(2 | 4 | 32),
                    DISPLAY_NO_VIEW => (f | 32 | 4) & !2,
                    _ => (f | 4) & !(2 | 32),
                };
                let mut o = w.obj.clone();
                put_int(&mut o, "F", f)?;
            }
            touched = true;
        }
        let fill = props.contains(&Prop::FillColor) && cur.fill_color != new.fill_color;
        let stroke = props.contains(&Prop::StrokeColor) && cur.stroke_color != new.stroke_color;
        if fill || stroke {
            for w in &widgets {
                let mut o = w.obj.clone();
                if fill {
                    put_mk_colour(doc, &mut o, "BG", &new.fill_color)?;
                }
                if stroke {
                    put_mk_colour(doc, &mut o, "BC", &new.stroke_color)?;
                }
            }
            redraw = true;
            touched = true;
        }
        if props.contains(&Prop::TextColor) && cur.text_color != new.text_color {
            if let Some(parts) = &new.text_color {
                let v = parts_f32(parts);
                let ops = match (parts.first().map(String::as_str), v.len()) {
                    (Some("G"), 1) => Some(format!("{} g", v[0])),
                    (Some("RGB"), 3) => Some(format!("{} {} {} rg", v[0], v[1], v[2])),
                    (Some("CMYK"), 4) => Some(format!("{} {} {} {} k", v[0], v[1], v[2], v[3])),
                    _ => None,
                };
                if let Some(ops) = ops {
                    let (font, size, _) = parse_da(&da_of(&e.obj));
                    for w in &widgets {
                        let mut o = w.obj.clone();
                        put_str(&mut o, "DA", &format!("/{font} {size} Tf {ops}"))?;
                    }
                    redraw = true;
                    touched = true;
                }
            }
        }
        if redraw && matches!(e.kind, FieldKind::Text | FieldKind::Combo | FieldKind::List) {
            for w in &widgets {
                refresh_with(rt, w)?;
            }
        }
        if touched {
            pages.extend(widgets.iter().map(|w| w.page));
        }
    }
    Ok(pages)
}

// ---- the calculation cascade ----------------------------------------------------------------------

/// How many times the calculation order is run over the form when values keep changing.
const MAX_PASSES: usize = 8;

/// Runs every field's calculation in the AcroForm's `/CO` order (document order without one), again
/// while a pass changed something (at most [`MAX_PASSES`] passes), as Acrobat does after a value
/// changes. Returns the pages whose appearance changed.
pub(crate) fn cascade(rt: &Rt) -> Result<Vec<u32>> {
    let entries = rt.entries()?;
    let mut calc: Vec<(&Entry, Plan<Calculate>)> = per_field(entries)
        .into_iter()
        .filter(|e| matches!(e.kind, FieldKind::Text | FieldKind::Combo))
        .map(|e| (e, rt.calc_plan(e)))
        .filter(|(_, p)| !matches!(p, Plan::None))
        .collect();
    if calc.is_empty() {
        return Ok(Vec::new());
    }
    let order = super::written_calc_order(rt.doc());
    calc.sort_by_key(|(e, _)| order.iter().position(|x| *x == e.node).unwrap_or(usize::MAX));
    let started = Instant::now();
    let mut values = names_to_values(entries);
    let locks = locks_of(entries);
    let mut pages = Vec::new();
    'passes: for _ in 0..MAX_PASSES {
        let mut changed_any = false;
        for (e, plan) in &calc {
            let current = values.get(&e.name).cloned().unwrap_or_default();
            let new = match plan {
                Plan::None => continue,
                Plan::Fast(c) => {
                    let lookup = |name: &str| -> Vec<String> {
                        let prefix = format!("{name}.");
                        values.iter().filter(|(k, _)| k.as_str() == name || k.starts_with(&prefix)).map(|(_, v)| v.clone()).collect()
                    };
                    match af::calculate(c, &lookup) {
                        Some(v) => v,
                        None => continue,
                    }
                }
                Plan::Js(src) => {
                    let left = CASCADE_TIME.saturating_sub(started.elapsed());
                    if left.is_zero() {
                        rt.note(LogLevel::Error, None, format!("the calculations ran longer than {} s and were stopped", CASCADE_TIME.as_secs()));
                        break 'passes;
                    }
                    if rt.budget_gone() {
                        continue;
                    }
                    let states = field_states(entries);
                    let out = rt.run(Some(&states), e, Event::new("Calculate", &e.name, &current), src.clone(), CALC_TIME.min(left));
                    if out.skipped || out.error.is_some() {
                        continue;
                    }
                    // What the script did to other fields counts as a change.
                    if !out.changed.is_empty() {
                        let written = apply_changes(rt, &out.changed)?;
                        if !written.is_empty() {
                            pages.extend(written);
                            values = names_to_values(entries);
                            changed_any = true;
                        }
                    }
                    if !out.rc {
                        continue;
                    }
                    out.value
                }
            };
            if new == current {
                continue;
            }
            if locked(&locks, e) {
                rt.note(LogLevel::Warn, Some(&e.name), format!("the result of [ {} ] was not written: the document is signed", e.name));
                continue;
            }
            pages.extend(write_values(rt, e, std::slice::from_ref(&new))?);
            values.insert(e.name.clone(), new);
            changed_any = true;
        }
        if !changed_any {
            break;
        }
    }
    Ok(pages)
}

/// Whether the document has scripts at all (document-level, or on any field), for the banner. `doc_js` is the
/// document-level list when it was read already.
pub(crate) fn has_scripts_with(doc_js: Option<&DocScripts>, doc: &PdfDocument, entries: &[Entry]) -> bool {
    let doc_level = match doc_js {
        Some(d) => !d.scripts.is_empty(),
        None => !load_doc_js(doc).scripts.is_empty(),
    };
    doc_level || entries.iter().any(|e| ["K", "F", "V", "C"].into_iter().any(|k| aa_js(e, k).is_some_and(|js| !is_plain_af(k, &js))))
}

/// A script that is exactly one AF call needs no engine.
fn is_plain_af(key: &str, js: &str) -> bool {
    match key {
        "F" => af::parse_format(js).is_some(),
        "K" => af::parse_keystroke(js).is_some(),
        "V" => af::parse_validate(js).is_some(),
        _ => af::parse_calculate(js).is_some(),
    }
}

/// A script's values for a field of this kind.
pub(crate) fn as_field_value(v: &[String], kind: FieldKind) -> FieldValue {
    match kind {
        FieldKind::List if v.len() > 1 => FieldValue::Many(v.to_vec()),
        _ => FieldValue::Text(v.first().cloned().unwrap_or_default()),
    }
}

/// The report a step leaves behind: what a script raised past the cap is said once in the log.
impl Drop for Rt<'_> {
    fn drop(&mut self) {
        let n = self.suppressed.get();
        if n > 0 {
            self.note(LogLevel::Warn, None, format!("{n} more alert{} from scripts {} not shown (a step shows {MAX_ALERTS_PER_STEP})", if n == 1 { "" } else { "s" }, if n == 1 { "was" } else { "were" }));
        }
    }
}
