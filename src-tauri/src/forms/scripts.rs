//! The glue between the form model (`mod.rs`) and the script engine (`js.rs`): what a script is told about
//! the fields, what a script's changes do to the document, and which events take the native fast path.
//!
//! One [`Rt`] lives for one engine step (one `apply_custom` closure, one listing). It owns the field scan
//! (the entries are live handles, so a value written during the step is read back by the next script),
//! the document's script switch and what the scripts raised (alerts, requests).

use super::af::{self, Calculate, Format, Validate};
use super::author::{color_array, ensure_dict, set_flag};
use super::js::{
    Alert, DocInfo, DocScripts, Event, FieldState, FieldType, Job, LogLevel, Outcome, Request, Scripts, CALC_TIME, CASCADE_TIME, DISPLAY_HIDDEN, DISPLAY_NO_PRINT, DISPLAY_NO_VIEW,
    DISPLAY_VISIBLE, EVENT_TIME, MAX_DOC_SCRIPT_BYTES,
};
use super::{
    Entry, FieldKind, FieldValue, Result, aa_js, da_of, field_ff, flag, get, geti, int_of, names_to_values, nums, on_state, options_of, parse_da, per_field, put_int, put_nums, put_str,
    refresh_with, text, text_or_stream, values_of, write_values,
};
use mupdf::pdf::{FieldFlags, PdfDocument, PdfObject};
use serde::Serialize;
use std::cell::{OnceCell, RefCell};
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

pub(crate) struct Rt<'a> {
    doc: &'a PdfDocument,
    scripts: Option<&'a Scripts>,
    doc_js: Option<std::sync::Arc<DocScripts>>,
    file_name: String,
    entries: OnceCell<Vec<Entry>>,
    info: OnceCell<DocInfo>,
    pub report: RefCell<Report>,
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
        Rt { doc, scripts, doc_js, file_name: file_name.to_string(), entries: OnceCell::new(), info: OnceCell::new(), report: RefCell::new(Report::default()) }
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
    pub(crate) fn run(&self, fields: Option<&[FieldState]>, e: &Entry, event: Event, script: String, timeout: Duration) -> Outcome {
        let skipped = Outcome { rc: true, value: event.value.clone(), change: event.change.clone(), skipped: true, ..Default::default() };
        let Some(scripts) = self.scripts.filter(|s| s.usable()) else { return skipped };
        let owned;
        let fields = match fields {
            Some(f) => f,
            None => {
                let Ok(entries) = self.entries() else { return skipped };
                owned = field_states(entries);
                &owned
            }
        };
        let out = scripts.run(Job { key: e.name.clone(), event, script, fields: fields.to_vec(), doc: self.info().clone(), timeout });
        let mut rep = self.report.borrow_mut();
        rep.alerts.extend(out.alerts.iter().cloned());
        rep.requests.extend(out.requests.iter().cloned());
        out
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
    pub(crate) fn keystroke(&self, e: &Entry, value: &str) -> std::result::Result<(String, Vec<FieldState>), String> {
        match self.key_plan(e) {
            Plan::None => Ok((value.to_string(), Vec::new())),
            Plan::Fast(f) => af::keystroke(&f, &e.name, value).map(|v| (v, Vec::new())),
            Plan::Js(src) => {
                let out = self.run(None, e, Event::new("Keystroke", &e.name, value), src, EVENT_TIME);
                self.verdict(out, value, format!("The value entered does not match the format of the field [ {} ]", e.name))
            }
        }
    }

    /// The Validate event: the fields the script changed, or the message that rejects the value.
    pub(crate) fn validate(&self, e: &Entry, value: &str) -> std::result::Result<Vec<FieldState>, String> {
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
    fn verdict(&self, out: Outcome, value: &str, default: String) -> std::result::Result<(String, Vec<FieldState>), String> {
        if out.skipped || out.error.is_some() {
            return Ok((value.to_string(), Vec::new()));
        }
        if !out.rc {
            // The alert is the reason: the UI shows it as the error, not a second time as an alert.
            let mut rep = self.report.borrow_mut();
            let keep = rep.alerts.len().saturating_sub(out.alerts.len());
            let why: Vec<String> = rep.alerts.drain(keep..).map(|a| a.message).collect();
            let msg = why.join("\n");
            return Err(if msg.is_empty() { default } else { msg });
        }
        Ok((out.value, out.changed))
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

/// Applies the fields a script changed (`changed`, as the script left them) to the document: the value
/// (without running its keystroke or validate, as in Acrobat), read-only, required, visibility and colours.
/// Returns the pages touched.
pub(crate) fn apply_changes(rt: &Rt, changed: &[FieldState]) -> Result<Vec<u32>> {
    let doc = rt.doc();
    let entries = rt.entries()?;
    let mut pages = Vec::new();
    for new in changed {
        let Some(e) = entries.iter().find(|e| e.name == new.name) else { continue };
        let cur = field_state(entries, e);
        if cur == *new {
            continue;
        }
        let widgets: Vec<&Entry> = entries.iter().filter(|w| w.node == e.node).collect();
        pages.extend(widgets.iter().map(|w| w.page));
        let mut redraw = false;
        if cur.value != new.value && !matches!(e.kind, FieldKind::Button | FieldKind::Signature) {
            pages.extend(write_values(rt, e, &new.value)?);
        }
        if cur.readonly != new.readonly || cur.required != new.required {
            let mut ff = field_ff(e);
            set_flag(&mut ff, FieldFlags::READ_ONLY, new.readonly);
            set_flag(&mut ff, FieldFlags::REQUIRED, new.required);
            let mut field = e.field.clone();
            put_int(&mut field, "Ff", ff)?;
        }
        if cur.display != new.display {
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
        }
        if cur.fill_color != new.fill_color || cur.stroke_color != new.stroke_color {
            for w in &widgets {
                let mut o = w.obj.clone();
                if cur.fill_color != new.fill_color {
                    put_mk_colour(doc, &mut o, "BG", &new.fill_color)?;
                }
                if cur.stroke_color != new.stroke_color {
                    put_mk_colour(doc, &mut o, "BC", &new.stroke_color)?;
                }
            }
            redraw = true;
        }
        if cur.text_color != new.text_color {
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
                }
            }
        }
        if redraw && matches!(e.kind, FieldKind::Text | FieldKind::Combo | FieldKind::List) {
            for w in &widgets {
                refresh_with(rt, w)?;
            }
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
                    let states = field_states(entries);
                    let out = rt.run(Some(&states), e, Event::new("Calculate", &e.name, &current), src.clone(), CALC_TIME.min(left));
                    if out.skipped || out.error.is_some() {
                        continue;
                    }
                    // What the script did to other fields counts as a change.
                    if !out.changed.is_empty() {
                        pages.extend(apply_changes(rt, &out.changed)?);
                        values = names_to_values(entries);
                        changed_any = true;
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

/// Whether the document has scripts at all (document-level, or on any field), for the banner.
pub(crate) fn has_scripts(doc: &PdfDocument, entries: &[Entry]) -> bool {
    !load_doc_js(doc).scripts.is_empty() || entries.iter().any(|e| ["K", "F", "V", "C"].into_iter().any(|k| aa_js(e, k).is_some_and(|js| !is_plain_af(k, &js))))
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
