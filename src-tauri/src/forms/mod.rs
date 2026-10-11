//! Forms (phase 3b): fill every AcroForm field type, calculations and formatting, and "Prepare a form"
//! authoring (add, move, resize, delete, properties, tab order, auto-detect on a flat form).
//!
//! Every change is one `Engine::apply_custom` step: one undo step, kept by Save, recorded in history.
//! Geometry is in page space (points, y down, as the viewer shows the page); a widget is named by
//! `(page, xref)`. MuPDF builds the appearance streams other viewers show; MuPDF is built without
//! JavaScript, so format, keystroke, validate and calculate run natively from the AcroForm
//! function calls Acrobat writes (`af.rs`) and never touch a script engine, the network or files.
//!
//! A text field's `/V` keeps the raw value; its appearance shows the formatted one (as Acrobat does).
//! Form data export and import (FDF, XFDF, JSON) is not built (see docs/forms.md).

use crate::engine::edit::{EditSource, EditState};
use crate::engine::{DocId, Engine, EngineError};
use mupdf::pdf::{FieldFlags, PdfDocument, PdfObject, WidgetType};
use mupdf::Rect;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod af;
pub mod app;
mod author;
pub mod detect;
#[cfg(test)]
mod tests;

pub use author::*;

use af::{ButtonAction, Calculate, Format, Validate};

pub(crate) type Result<T> = std::result::Result<T, EngineError>;

pub(crate) fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    Text,
    Checkbox,
    Radio,
    Combo,
    List,
    Button,
    Signature,
}

/// A choice-field option: the exported value and what is shown.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Opt {
    pub export: String,
    pub label: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FieldDto {
    pub page: u32,
    pub xref: i32,
    /// The field this widget belongs to (equal for the buttons of a radio group).
    pub node: i32,
    pub name: String,
    pub kind: FieldKind,
    pub rect: [f32; 4],
    /// Position in the page's tab order (0 first).
    pub tab: u32,
    pub value: String,
    pub values: Vec<String>,
    pub default_value: String,
    /// Check boxes and radio buttons: this widget is on.
    pub checked: bool,
    /// The value this check box or radio button exports when on.
    pub export: String,
    pub options: Vec<Opt>,
    pub read_only: bool,
    pub required: bool,
    pub hidden: bool,
    pub multiline: bool,
    pub password: bool,
    pub comb: bool,
    pub editable: bool,
    pub multi_select: bool,
    pub no_toggle_off: bool,
    /// 0 means no limit.
    pub max_len: u32,
    /// 0 means auto.
    pub font_size: f32,
    pub text_color: [f32; 3],
    /// 0 left, 1 centre, 2 right.
    pub align: u8,
    pub border_color: Option<[f32; 3]>,
    pub fill_color: Option<[f32; 3]>,
    pub border_width: f32,
    pub format: Format,
    pub validate: Validate,
    pub calculate: Calculate,
    /// What the appearance shows for `value` (the format applied).
    pub shown: String,
    pub caption: String,
    pub action: Option<ButtonAction>,
    pub tooltip: String,
    /// The field carries a script Inkwell does not run (kept untouched in the file).
    pub script: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FormDto {
    pub fields: Vec<FieldDto>,
    /// The document is an XFA form: only its AcroForm fallback is shown.
    pub xfa: bool,
}

// ---- small helpers -----------------------------------------------------------------------------

pub(crate) fn rect_of(r: [f32; 4]) -> Rect {
    Rect { x0: r[0].min(r[2]), y0: r[1].min(r[3]), x1: r[0].max(r[2]), y1: r[1].max(r[3]) }
}

/// A non-null entry of a dictionary.
pub(crate) fn get(o: &PdfObject, k: &str) -> Option<PdfObject> {
    o.get_dict(k).ok().flatten().filter(|v| !v.is_null().unwrap_or(true))
}

/// The entry of a field dictionary, looked up the `/Parent` chain.
pub(crate) fn geti(o: &PdfObject, k: &str) -> Option<PdfObject> {
    o.get_dict_inheritable(k).ok().flatten().filter(|v| !v.is_null().unwrap_or(true))
}

/// A string or name as text.
pub(crate) fn text(o: &PdfObject) -> Option<String> {
    if o.is_string().unwrap_or(false) {
        o.as_string().ok()
    } else if o.is_name().unwrap_or(false) {
        o.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned())
    } else {
        None
    }
}

/// A string, name or (decoded) stream as text: script sources are written either way.
pub(crate) fn text_or_stream(o: &PdfObject) -> Option<String> {
    text(o).or_else(|| {
        if o.is_stream().unwrap_or(false) {
            o.read_stream().ok().map(|b| String::from_utf8_lossy(&b).into_owned())
        } else {
            None
        }
    })
}

pub(crate) fn nums(o: &PdfObject) -> Vec<f32> {
    let n = o.len().unwrap_or(0).min(64);
    (0..n as i32).filter_map(|i| o.get_array(i).ok().flatten().and_then(|v| v.as_float().ok())).collect()
}

pub(crate) fn int_of(o: &PdfObject, k: &str) -> Option<i32> {
    geti(o, k).and_then(|v| v.as_int().ok())
}

fn rgb_from(a: &[f32]) -> Option<[f32; 3]> {
    match a.len() {
        1 => Some([a[0]; 3]),
        3 => Some([a[0], a[1], a[2]]),
        4 => {
            let k = 1.0 - a[3];
            Some([(1.0 - a[0]) * k, (1.0 - a[1]) * k, (1.0 - a[2]) * k])
        }
        _ => None,
    }
}

pub(crate) fn put_name(o: &mut PdfObject, k: &str, v: &str) -> Result<()> {
    o.dict_put(k, PdfObject::new_name(v)?)?;
    Ok(())
}

pub(crate) fn put_str(o: &mut PdfObject, k: &str, v: &str) -> Result<()> {
    o.dict_put(k, PdfObject::new_string(v)?)?;
    Ok(())
}

pub(crate) fn put_int(o: &mut PdfObject, k: &str, v: i32) -> Result<()> {
    o.dict_put(k, PdfObject::new_int(v)?)?;
    Ok(())
}

pub(crate) fn put_nums(doc: &PdfDocument, o: &mut PdfObject, k: &str, v: &[f32]) -> Result<()> {
    let mut a = doc.new_array()?;
    for x in v {
        a.array_push(PdfObject::new_real(*x)?)?;
    }
    o.dict_put(k, a)?;
    Ok(())
}

// ---- the field model ---------------------------------------------------------------------------

/// One widget with what is needed to find its field.
pub(crate) struct Entry {
    pub page: u32,
    pub xref: i32,
    /// The xref of the field this widget belongs to (its own when it is the field).
    pub node: i32,
    pub tab: u32,
    pub kind: FieldKind,
    pub name: String,
    pub rect: [f32; 4],
    pub obj: PdfObject,
    pub field: PdfObject,
}

fn kind_of(t: WidgetType) -> Option<FieldKind> {
    Some(match t {
        WidgetType::Text => FieldKind::Text,
        WidgetType::Checkbox => FieldKind::Checkbox,
        WidgetType::RadioButton => FieldKind::Radio,
        WidgetType::Combobox => FieldKind::Combo,
        WidgetType::Listbox => FieldKind::List,
        WidgetType::Button => FieldKind::Button,
        WidgetType::Signature => FieldKind::Signature,
        WidgetType::Unknown => return None,
    })
}

/// The field a widget belongs to: the nearest object up the `/Parent` chain that has a `/T`.
fn field_of(w: &PdfObject) -> PdfObject {
    let mut cur = w.clone();
    for _ in 0..32 {
        if get(&cur, "T").is_some() {
            return cur;
        }
        match get(&cur, "Parent") {
            Some(p) => cur = p,
            None => break,
        }
    }
    w.clone()
}

fn full_name(w: &PdfObject) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(w.clone());
    for _ in 0..32 {
        let Some(c) = cur else { break };
        if let Some(t) = get(&c, "T").as_ref().and_then(text) {
            parts.push(t);
        }
        cur = get(&c, "Parent");
    }
    parts.reverse();
    parts.join(".")
}

pub(crate) fn scan(doc: &PdfDocument) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for p in 0..doc.page_count()?.max(0) as u32 {
        let page = doc.load_pdf_page(p as i32)?;
        for (i, w) in page.widgets().enumerate() {
            let Some(kind) = w.r#type().ok().and_then(kind_of) else { continue };
            let a = w.annotation();
            let (obj, xref, r) = (a.object(), a.xref()?, a.rect()?);
            let field = field_of(&obj);
            let own = get(&obj, "T").is_some() || get(&obj, "Parent").is_none();
            let node = if own { xref } else { field.as_indirect().unwrap_or(xref) };
            out.push(Entry { page: p, xref, node, tab: i as u32, kind, name: full_name(&obj), rect: [r.x0, r.y0, r.x1, r.y1], obj, field });
        }
    }
    Ok(out)
}

fn flag(ff: i32, f: FieldFlags) -> bool {
    ff & f.bits() != 0
}

fn field_ff(e: &Entry) -> i32 {
    int_of(&e.obj, "Ff").unwrap_or(0)
}

/// The values of a field as strings (`/V` is a string, a name or an array of strings).
fn values_of(v: Option<PdfObject>) -> Vec<String> {
    let Some(v) = v else { return Vec::new() };
    if v.is_array().unwrap_or(false) {
        (0..v.len().unwrap_or(0).min(4096) as i32).filter_map(|i| v.get_array(i).ok().flatten().and_then(|x| text(&x))).collect()
    } else {
        text(&v).into_iter().collect()
    }
}

fn options_of(o: &PdfObject) -> Vec<Opt> {
    let Some(a) = geti(o, "Opt") else { return Vec::new() };
    (0..a.len().unwrap_or(0).min(4096) as i32)
        .filter_map(|i| {
            let v = a.get_array(i).ok().flatten()?;
            if v.is_array().unwrap_or(false) {
                let export = v.get_array(0).ok().flatten().and_then(|x| text(&x))?;
                let label = v.get_array(1).ok().flatten().and_then(|x| text(&x)).unwrap_or_else(|| export.clone());
                Some(Opt { export, label })
            } else {
                let s = text(&v)?;
                Some(Opt { export: s.clone(), label: s })
            }
        })
        .collect()
}

/// An on-state name as shown. pdfTeX writes non-ASCII export values as the literal characters
/// backslash, 376, backslash, 377, backslash, 000, "e" ... (an escaped UTF-16BE string, backslashes
/// included) inside the name.
pub(crate) fn shown_name(n: &str) -> String {
    const BS: char = '\u{5c}';
    let Some(rest) = n.strip_prefix(&format!("{BS}376{BS}377")) else { return n.to_string() };
    let mut bytes = Vec::new();
    let mut it = rest.chars().peekable();
    while let Some(c) = it.next() {
        if c == BS {
            let oct: String = (0..3).filter_map(|_| it.next_if(|d| ('0'..='7').contains(d))).collect();
            match u8::from_str_radix(&oct, 8) {
                Ok(b) => bytes.push(b),
                Err(_) => return n.to_string(),
            }
        } else if c.is_ascii() {
            bytes.push(c as u8);
        } else {
            return n.to_string();
        }
    }
    if bytes.len() % 2 != 0 {
        return n.to_string();
    }
    let units: Vec<u16> = bytes.chunks(2).map(|p| u16::from_be_bytes([p[0], p[1]])).collect();
    String::from_utf16(&units).unwrap_or_else(|_| n.to_string())
}

/// The state name a check box or radio button turns on to: the `/AP /N` key that is not `Off`.
pub(crate) fn on_state(w: &PdfObject) -> Option<String> {
    let n = w.get_dict("AP").ok().flatten()?.get_dict("N").ok().flatten()?;
    if !n.is_dict().unwrap_or(false) {
        return None;
    }
    n.dict_iter().ok()?.filter_map(|kv| kv.ok()).filter_map(|(k, _)| text(&k)).find(|k| k != "Off")
}

/// The JavaScript of an additional action (`/AA /<key>`), on the widget or its field.
fn aa_js(e: &Entry, key: &str) -> Option<String> {
    [&e.obj, &e.field].into_iter().find_map(|o| {
        let a = get(o, "AA")?;
        let act = get(&a, key)?;
        text_or_stream(&get(&act, "JS")?)
    })
}

fn format_of(e: &Entry) -> Format {
    aa_js(e, "F").and_then(|js| af::parse_format(&js)).or_else(|| aa_js(e, "K").and_then(|js| af::parse_format(&js))).unwrap_or_default()
}

fn validate_of(e: &Entry) -> Validate {
    aa_js(e, "V").and_then(|js| af::parse_validate(&js)).unwrap_or_default()
}

fn calculate_of(e: &Entry) -> Calculate {
    aa_js(e, "C").and_then(|js| af::parse_calculate(&js)).unwrap_or_default()
}

fn has_unknown_script(e: &Entry) -> bool {
    let known = |k: &str, ok: bool| aa_js(e, k).is_some_and(|_| !ok);
    known("F", format_of(e) != Format::None) || known("V", validate_of(e) != Validate::None) || known("C", calculate_of(e) != Calculate::None)
}

fn action_of(e: &Entry) -> Option<ButtonAction> {
    let a = get(&e.obj, "A")?;
    let s = get(&a, "S").as_ref().and_then(text)?;
    Some(match s.as_str() {
        "JavaScript" => af::button_script(&text_or_stream(&get(&a, "JS")?)?),
        "ResetForm" => {
            let fs = get(&a, "Fields");
            let fields = fs
                .map(|f| (0..f.len().unwrap_or(0).min(1024) as i32).filter_map(|i| f.get_array(i).ok().flatten()).filter_map(|x| text(&x).or_else(|| Some(full_name(&x)))).collect())
                .unwrap_or_default();
            ButtonAction::Reset { fields }
        }
        "SubmitForm" => {
            let f = get(&a, "F");
            let url = f.as_ref().and_then(|f| text(f).or_else(|| get(f, "F").as_ref().and_then(text))).unwrap_or_default();
            ButtonAction::Submit { url }
        }
        "URI" => ButtonAction::Uri { uri: text(&get(&a, "URI")?)? },
        "Named" => ButtonAction::Named { name: text(&get(&a, "N")?)? },
        _ => ButtonAction::Script { source: s },
    })
}

/// `(font size, colour)` from the default appearance string (`/Helv 12 Tf 0 0 1 rg`).
pub(crate) fn parse_da(da: &str) -> (String, f32, [f32; 3]) {
    let t: Vec<&str> = da.split_whitespace().collect();
    let (mut font, mut size, mut color) = ("Helv".to_string(), 0.0, [0.0; 3]);
    for (i, w) in t.iter().enumerate() {
        let num = |j: usize| t.get(j).and_then(|s| s.parse::<f32>().ok());
        match *w {
            "Tf" if i >= 2 => {
                font = t[i - 2].trim_start_matches('/').to_string();
                size = num(i - 1).unwrap_or(0.0);
            }
            "g" if i >= 1 => color = [num(i - 1).unwrap_or(0.0); 3],
            "rg" if i >= 3 => color = [num(i - 3).unwrap_or(0.0), num(i - 2).unwrap_or(0.0), num(i - 1).unwrap_or(0.0)],
            "k" if i >= 4 => {
                let k = 1.0 - num(i - 1).unwrap_or(0.0);
                color = [(1.0 - num(i - 4).unwrap_or(0.0)) * k, (1.0 - num(i - 3).unwrap_or(0.0)) * k, (1.0 - num(i - 2).unwrap_or(0.0)) * k];
            }
            _ => {}
        }
    }
    (font, size, color)
}

fn da_of(o: &PdfObject) -> String {
    geti(o, "DA").as_ref().and_then(text).unwrap_or_else(|| {
        // The AcroForm default.
        o.document().and_then(|d| d.catalog().ok()).and_then(|c| get(&c, "AcroForm")).and_then(|f| get(&f, "DA")).as_ref().and_then(text).unwrap_or_default()
    })
}

fn dto_of(e: &Entry) -> FieldDto {
    let ff = field_ff(e);
    let o = &e.obj;
    let mk = get(o, "MK");
    let mk_color = |k: &str| mk.as_ref().and_then(|m| get(m, k)).and_then(|a| rgb_from(&nums(&a)));
    let (_, font_size, text_color) = parse_da(&da_of(o));
    let values = values_of(geti(&e.field, "V"));
    let value = values.first().cloned().unwrap_or_default();
    let is_button = matches!(e.kind, FieldKind::Checkbox | FieldKind::Radio);
    let export = if is_button { on_state(o).unwrap_or_default() } else { String::new() };
    let as_state = get(o, "AS").as_ref().and_then(text);
    let checked = is_button
        && match &as_state {
            Some(s) => s != "Off",
            None => !value.is_empty() && value != "Off" && (export.is_empty() || value == export),
        };
    let format = format_of(e);
    let shown = if matches!(e.kind, FieldKind::Text | FieldKind::Combo) { af::format_value(&format, &value) } else { value.clone() };
    let border_width = get(o, "BS").and_then(|b| get(&b, "W")).and_then(|w| w.as_float().ok()).unwrap_or(1.0);
    FieldDto {
        page: e.page,
        xref: e.xref,
        node: e.node,
        name: e.name.clone(),
        kind: e.kind,
        rect: e.rect,
        tab: e.tab,
        value: if is_button { String::new() } else { value },
        values,
        default_value: values_of(geti(&e.field, "DV")).into_iter().next().unwrap_or_default(),
        checked,
        export: shown_name(&export),
        options: options_of(o),
        read_only: flag(ff, FieldFlags::READ_ONLY),
        required: flag(ff, FieldFlags::REQUIRED),
        hidden: int_of(o, "F").unwrap_or(0) & 0b10 != 0,
        multiline: flag(ff, FieldFlags::MULTILINE),
        password: flag(ff, FieldFlags::PASSWORD),
        comb: flag(ff, FieldFlags::COMB),
        editable: flag(ff, FieldFlags::EDIT),
        multi_select: flag(ff, FieldFlags::MULTI_SELECT),
        no_toggle_off: flag(ff, FieldFlags::NO_TOGGLE_TO_OFF),
        max_len: int_of(o, "MaxLen").unwrap_or(0).max(0) as u32,
        font_size,
        text_color,
        align: int_of(o, "Q").unwrap_or(0).clamp(0, 2) as u8,
        border_color: mk_color("BC"),
        fill_color: mk_color("BG"),
        border_width,
        validate: validate_of(e),
        calculate: calculate_of(e),
        shown,
        format,
        caption: mk.as_ref().and_then(|m| get(m, "CA")).as_ref().and_then(text).unwrap_or_default(),
        action: if e.kind == FieldKind::Button { action_of(e) } else { None },
        tooltip: get(&e.field, "TU").or_else(|| get(o, "TU")).as_ref().and_then(text).unwrap_or_default(),
        script: has_unknown_script(e),
    }
}

/// Every form field widget of the document, page by page, in each page's tab order.
pub fn list(doc: &PdfDocument) -> Result<FormDto> {
    let fields = scan(doc)?.iter().map(dto_of).collect();
    Ok(FormDto { fields, xfa: doc.has_xfa_form().unwrap_or(false) })
}

// ---- appearance --------------------------------------------------------------------------------

/// Rebuilds the appearance stream of a widget. A field with a format keeps its raw `/V` but
/// draws the formatted text, as Acrobat does.
pub(crate) fn refresh(doc: &PdfDocument, e: &Entry) -> Result<()> {
    let page = doc.load_pdf_page(e.page as i32)?;
    let mut w = page.load_widget(e.xref)?.ok_or_else(|| bad(format!("no field {} on page {}", e.xref, e.page + 1)))?;
    let mut field = e.field.clone();
    let raw = geti(&field, "V").and_then(|v| text(&v));
    let shown = match (&raw, e.kind) {
        (Some(r), FieldKind::Text | FieldKind::Combo) => {
            let f = af::format_value(&format_of(e), r);
            (f != *r).then_some(f)
        }
        _ => None,
    };
    if let Some(s) = &shown {
        put_str(&mut field, "V", s)?;
    }
    w.request_resynthesis()?;
    let updated = w.update();
    if let (Some(_), Some(r)) = (&shown, &raw) {
        put_str(&mut field, "V", r)?;
    }
    updated?;
    Ok(())
}

// ---- filling -----------------------------------------------------------------------------------

/// What a field is set to: text, the strings selected in a multi-select list, or on/off.
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum FieldValue {
    Checked(bool),
    Text(String),
    Many(Vec<String>),
}

/// One entry per field (a field with several widgets is listed once).
fn per_field(entries: &[Entry]) -> Vec<&Entry> {
    let mut seen: Vec<i32> = Vec::new();
    entries.iter().filter(|e| if seen.contains(&e.node) { false } else { seen.push(e.node); true }).collect()
}

fn names_to_values(entries: &[Entry]) -> HashMap<String, String> {
    per_field(entries)
        .into_iter()
        .filter(|e| !matches!(e.kind, FieldKind::Button | FieldKind::Signature))
        .map(|e| (e.name.clone(), values_of(geti(&e.field, "V")).into_iter().next().unwrap_or_default()))
        .collect()
}

/// Runs every field's calculation once, in the AcroForm's `/CO` order (document order without
/// one), as Acrobat does after a value changes. Returns the pages whose appearance changed.
fn recalculate(doc: &PdfDocument, entries: &[Entry]) -> Result<Vec<u32>> {
    let calc: Vec<(&Entry, Calculate)> = per_field(entries)
        .into_iter()
        .filter(|e| matches!(e.kind, FieldKind::Text | FieldKind::Combo))
        .map(|e| (e, calculate_of(e)))
        .filter(|(_, c)| *c != Calculate::None)
        .collect();
    if calc.is_empty() {
        return Ok(Vec::new());
    }
    let order: Vec<i32> = doc
        .catalog()
        .ok()
        .and_then(|c| get(&c, "AcroForm"))
        .and_then(|f| get(&f, "CO"))
        .map(|a| (0..a.len().unwrap_or(0).min(4096) as i32).filter_map(|i| a.get_array(i).ok().flatten()?.as_indirect().ok()).collect())
        .unwrap_or_default();
    let mut calc = calc;
    calc.sort_by_key(|(e, _)| order.iter().position(|x| *x == e.node).unwrap_or(usize::MAX));
    let mut values = names_to_values(entries);
    let mut pages = Vec::new();
    for (e, c) in calc {
        let lookup = |name: &str| -> Vec<String> {
            let prefix = format!("{name}.");
            values.iter().filter(|(k, _)| k.as_str() == name || k.starts_with(&prefix)).map(|(_, v)| v.clone()).collect()
        };
        let Some(new) = af::calculate(&c, &lookup) else { continue };
        let old = values.get(&e.name).cloned().unwrap_or_default();
        if new == old {
            continue;
        }
        let mut field = e.field.clone();
        put_str(&mut field, "V", &new)?;
        values.insert(e.name.clone(), new);
        for w in entries.iter().filter(|w| w.node == e.node) {
            refresh(doc, w)?;
            pages.push(w.page);
        }
    }
    Ok(pages)
}

fn pages_of(mut v: Vec<u32>) -> Vec<u32> {
    v.sort_unstable();
    v.dedup();
    v
}

/// Sets a field. A value that does not fit the field's format or range is rejected with
/// Acrobat's message; the document is left unchanged. Returns the pages that changed.
pub fn set_value(doc: &mut PdfDocument, page: u32, xref: i32, value: &FieldValue) -> Result<Vec<u32>> {
    let entries = scan(doc)?;
    let e = entries.iter().find(|e| e.page == page && e.xref == xref).ok_or_else(|| bad(format!("no field {xref} on page {}", page + 1)))?;
    let ff = field_ff(e);
    if flag(ff, FieldFlags::READ_ONLY) {
        return Err(bad(format!("The field [ {} ] is read-only", e.name)));
    }
    let mut field = e.field.clone();
    let mut touched = vec![page];
    match e.kind {
        FieldKind::Button | FieldKind::Signature => return Err(bad("this field has no value to fill")),
        FieldKind::Checkbox | FieldKind::Radio => {
            let on = on_state(&e.obj).unwrap_or_else(|| "Yes".into());
            // A radio button is chosen by name or by being turned on.
            let want = match value {
                FieldValue::Checked(b) => b.then(|| on.clone()),
                FieldValue::Text(s) if s == "Off" || s.is_empty() => None,
                FieldValue::Text(s) => Some(s.clone()),
                FieldValue::Many(_) => return Err(bad("a check box takes on or off")),
            };
            if want.is_none() && e.kind == FieldKind::Radio && flag(ff, FieldFlags::NO_TOGGLE_TO_OFF) {
                return Ok(Vec::new());
            }
            put_name(&mut field, "V", want.as_deref().unwrap_or("Off"))?;
            for w in entries.iter().filter(|w| w.node == e.node) {
                let mine = on_state(&w.obj).unwrap_or_else(|| "Yes".into());
                let state = match (&want, e.kind) {
                    (Some(s), FieldKind::Radio) if *s == mine => mine,
                    (Some(_), FieldKind::Checkbox) if w.xref == xref => mine,
                    _ => "Off".to_string(),
                };
                let mut o = w.obj.clone();
                put_name(&mut o, "AS", &state)?;
                touched.push(w.page);
            }
        }
        FieldKind::Text | FieldKind::Combo | FieldKind::List => {
            let strings: Vec<String> = match value {
                FieldValue::Text(s) => vec![s.clone()],
                FieldValue::Many(v) => v.clone(),
                FieldValue::Checked(_) => return Err(bad("a text field takes text")),
            };
            let first = strings.first().cloned().unwrap_or_default();
            let mut stored = first.clone();
            if e.kind == FieldKind::Text {
                if !flag(ff, FieldFlags::MULTILINE) {
                    stored = stored.replace(['\r', '\n'], " ");
                }
                let max = int_of(&e.obj, "MaxLen").unwrap_or(0);
                if max > 0 && stored.chars().count() > max as usize {
                    return Err(bad(format!("The field [ {} ] holds at most {max} characters", e.name)));
                }
            }
            if matches!(e.kind, FieldKind::Text | FieldKind::Combo) {
                stored = af::keystroke(&format_of(e), &e.name, &stored).map_err(bad)?;
                af::validate(&validate_of(e), &stored).map_err(bad)?;
            }
            if strings.len() > 1 {
                let mut a = doc.new_array()?;
                for s in &strings {
                    a.array_push(PdfObject::new_string(s)?)?;
                }
                field.dict_put("V", a)?;
            } else {
                put_str(&mut field, "V", &stored)?;
            }
            for w in entries.iter().filter(|w| w.node == e.node) {
                refresh(doc, w)?;
                touched.push(w.page);
            }
        }
    }
    // Check boxes and radios keep the appearance they have (MuPDF draws both states); others were refreshed.
    touched.extend(recalculate(doc, &entries)?);
    Ok(pages_of(touched))
}

/// Resets fields to their default values: the fields whose widgets are listed (by xref), or the
/// whole form ("Clear Form") when `which` is `None`. Returns the pages that changed.
pub fn reset(doc: &mut PdfDocument, which: Option<&[i32]>) -> Result<Vec<u32>> {
    let entries = scan(doc)?;
    let mut done: Vec<i32> = Vec::new();
    let mut touched = Vec::new();
    for e in &entries {
        let pick = match which {
            None => true,
            Some(w) => w.contains(&e.xref) || w.contains(&e.node),
        };
        if !pick || done.contains(&e.node) || matches!(e.kind, FieldKind::Button | FieldKind::Signature) || flag(field_ff(e), FieldFlags::READ_ONLY) {
            continue;
        }
        done.push(e.node);
        let mut field = e.field.clone();
        let dv = geti(&e.field, "DV");
        match dv {
            Some(v) => field.dict_put("V", v)?,
            None => field.dict_delete("V")?,
        }
        let off = values_of(get(&field, "V")).first().cloned();
        for w in entries.iter().filter(|w| w.node == e.node) {
            if matches!(w.kind, FieldKind::Checkbox | FieldKind::Radio) {
                let mine = on_state(&w.obj).unwrap_or_else(|| "Yes".into());
                let state = if off.as_deref() == Some(mine.as_str()) { mine } else { "Off".to_string() };
                let mut o = w.obj.clone();
                put_name(&mut o, "AS", &state)?;
            } else {
                refresh(doc, w)?;
            }
            touched.push(w.page);
        }
    }
    // Reset may have emptied what calculations read.
    let entries = scan(doc)?;
    touched.extend(recalculate(doc, &entries)?);
    Ok(pages_of(touched))
}

// ---- engine wiring: one undo step each ---------------------------------------------------------

pub fn list_all(eng: &Engine, doc: DocId) -> Result<FormDto> {
    eng.read_custom(doc, |d| list(d))
}

pub fn set_value_one(eng: &Engine, doc: DocId, page: u32, xref: i32, value: &FieldValue, source: EditSource) -> Result<EditState> {
    eng.apply_custom(doc, format!("Fill field on p. {}", page + 1), source, false, |d| set_value(d, page, xref, value))
}

pub fn reset_one(eng: &Engine, doc: DocId, which: Option<Vec<i32>>, source: EditSource) -> Result<EditState> {
    let label = if which.is_none() { "Clear form".to_string() } else { "Reset fields".to_string() };
    eng.apply_custom(doc, label, source, false, |d| reset(d, which.as_deref()))
}
