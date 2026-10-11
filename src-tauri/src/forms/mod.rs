//! Forms (phase 3b): fill every AcroForm field type, calculations and formatting, and "Prepare a form"
//! authoring (add, move, resize, delete, properties, tab order, auto-detect on a flat form).
//!
//! Every change is one `Engine::apply_custom` step: one undo step, kept by Save, recorded in history.
//! Geometry is in page space (points, y down, as the viewer shows the page); a widget is named by
//! `(page, xref)`. MuPDF builds the appearance streams other viewers show; MuPDF is built without
//! JavaScript. The AcroForm function calls Acrobat writes for format, keystroke, validate and calculate
//! run natively (`af.rs`, the fast path); every other field script runs in the sandboxed script engine
//! (`js.rs`, glue in `scripts.rs`) unless the person turned scripts off for the file. Neither touches
//! the network or files.
//!
//! A text field's `/V` keeps the raw value; its appearance shows the formatted one (as Acrobat does).
//! Form data export and import (FDF, XFDF, JSON) is not built (see docs/forms.md).

use crate::annot::{AnnotWalk, Which};
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
pub mod js;
mod scripts;
#[cfg(test)]
mod tests;

pub use author::*;
pub use scripts::{Report, Stepped};
use scripts::Rt;

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
    /// The fields that calculate (`node` of each), in the order they run: the `/AcroForm /CO` array first, then the others in
    /// listing order. What the Calculation order dialog lists.
    pub calc_order: Vec<i32>,
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
    if o.is_string().unwrap_or(false) {
        // `as_string` fails on UTF-16 (Acrobat writes non-ASCII scripts that way).
        return text(o).or_else(|| o.as_bytes().ok().map(|b| decode_text(&b)));
    }
    text(o).or_else(|| if o.is_stream().unwrap_or(false) { o.read_stream().ok().map(|b| decode_text(&b)) } else { None })
}

/// Bytes of a PDF text string or script stream: UTF-16 with a byte order mark, UTF-8, or Latin-1-ish text.
fn decode_text(b: &[u8]) -> String {
    let utf16 = |rest: &[u8], be: bool| {
        let units: Vec<u16> = rest.chunks_exact(2).map(|p| if be { u16::from_be_bytes([p[0], p[1]]) } else { u16::from_le_bytes([p[0], p[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match b {
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(b).into_owned(),
    }
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

/// Every widget of the document, page by page. Only pages that have widgets no earlier page had are
/// loaded (see [`AnnotWalk`]): a widget that several pages list is returned once, for the first
/// (or the page its `/P` names); one listed through different arrays with no `/P` stays with the first
/// page and is not fillable on the later one.
pub(crate) fn scan(doc: &PdfDocument) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    let mut walk = AnnotWalk::default();
    for p in 0..doc.page_count()?.max(0) as u32 {
        let Some(fresh) = walk.page(doc, p, Which::Widgets)? else { continue };
        let mut pending = fresh.nums;
        let page = doc.load_pdf_page(p as i32)?;
        for (i, w) in page.widgets().enumerate() {
            let Some(kind) = w.r#type().ok().and_then(kind_of) else { continue };
            let a = w.annotation();
            let (obj, xref, r) = (a.object(), a.xref()?, a.rect()?);
            if !(if xref == 0 { fresh.direct } else { pending.remove(&xref) }) {
                continue;
            }
            let field = field_of(&obj);
            let own = get(&obj, "T").is_some() || get(&obj, "Parent").is_none();
            let node = if own { xref } else { field.as_indirect().unwrap_or(xref) };
            out.push(Entry { page: p, xref, node, tab: i as u32, kind, name: full_name(&obj), rect: [r.x0, r.y0, r.x1, r.y1], obj, field });
        }
    }
    // Better no list than a form with fields missing: edits and calculations read all of them.
    if walk.incomplete {
        return Err(bad("this file's form is too large to edit safely"));
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
    aa_js(e, "F").and_then(|js| af::parse_format(&js)).or_else(|| aa_js(e, "K").and_then(|js| af::parse_keystroke(&js))).unwrap_or_default()
}

fn calculate_of(e: &Entry) -> Calculate {
    aa_js(e, "C").and_then(|js| af::parse_calculate(&js)).unwrap_or_default()
}

/// The field has a format, keystroke, validate or calculate script that is not one of the AF calls
/// (so it needs the script engine).
fn has_unknown_script(e: &Entry) -> bool {
    let unknown = |k: &str, known: &dyn Fn(&str) -> bool| aa_js(e, k).is_some_and(|js| !known(&js));
    unknown("F", &|js| af::parse_format(js).is_some())
        || unknown("K", &|js| af::parse_keystroke(js).is_some())
        || unknown("V", &|js| af::parse_validate(js).is_some())
        || unknown("C", &|js| af::parse_calculate(js).is_some())
}

/// The Format tab as the editor shows it (a custom script when the scripts are not Acrobat's own calls).
fn format_for_edit(e: &Entry) -> Format {
    af::format_for_editor(aa_js(e, "F").as_deref(), aa_js(e, "K").as_deref())
}

fn validate_for_edit(e: &Entry) -> Validate {
    af::validate_for_editor(aa_js(e, "V").as_deref())
}

fn calculate_for_edit(e: &Entry) -> Calculate {
    af::calculate_for_editor(aa_js(e, "C").as_deref())
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
            // Flags bit 1: reset every field except the listed ones.
            let exclude = int_of(&a, "Flags").unwrap_or(0) & 1 != 0;
            ButtonAction::Reset { fields, exclude }
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

// Adapted from pdfcraft (MIT OR Apache-2.0), crates/forms/src/appearance.rs @ b8ddf71 (parse_da)
/// `(font name, font size, colour)` from the default appearance string (`/Helv 12 Tf 0 0 1 rg`).
/// The size is finite and 0..=300 (0 is automatic) and the colour components finite, so
/// `NaN` or `inf` in a file are never written back.
pub(crate) fn parse_da(da: &str) -> (String, f32, [f32; 3]) {
    let t: Vec<&str> = da.split_whitespace().collect();
    let (mut font, mut size, mut color) = ("Helv".to_string(), 0.0, [0.0; 3]);
    for (i, w) in t.iter().enumerate() {
        let num = |j: usize| t.get(j).and_then(|s| s.parse::<f32>().ok()).filter(|v| v.is_finite());
        match *w {
            "Tf" if i >= 2 => {
                font = t[i - 2].trim_start_matches('/').to_string();
                size = num(i - 1).filter(|s| *s >= 0.0).unwrap_or(0.0).min(300.0);
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

fn dto_of(rt: &Rt, snapshot: Option<&[js::FieldState]>, e: &Entry) -> FieldDto {
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
    let format = format_for_edit(e);
    let shown = if matches!(e.kind, FieldKind::Text | FieldKind::Combo) { rt.shown(snapshot, e, &value) } else { value.clone() };
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
        validate: validate_for_edit(e),
        calculate: calculate_for_edit(e),
        shown,
        format,
        caption: mk.as_ref().and_then(|m| get(m, "CA")).as_ref().and_then(text).unwrap_or_default(),
        action: if e.kind == FieldKind::Button { action_of(e) } else { None },
        tooltip: get(&e.field, "TU").or_else(|| get(o, "TU")).as_ref().and_then(text).unwrap_or_default(),
        script: !rt.js_on() && has_unknown_script(e),
    }
}

/// Every form field widget of the document, page by page, in each page's tab order.
pub fn list(doc: &PdfDocument) -> Result<FormDto> {
    list_with(&Rt::off(doc))
}

/// [`list`] with the format scripts of the fields run by `rt`.
fn list_with(rt: &Rt) -> Result<FormDto> {
    let entries = rt.entries()?;
    rt.remember_has_scripts(entries);
    let snapshot = rt.js_on().then(|| scripts::field_states(entries));
    let fields = entries.iter().map(|e| dto_of(rt, snapshot.as_deref(), e)).collect();
    Ok(FormDto { fields, calc_order: calc_order_of(rt.doc(), entries), xfa: rt.doc().has_xfa_form().unwrap_or(false) })
}

// ---- appearance --------------------------------------------------------------------------------

/// Rebuilds the appearance stream of a widget. A field with a format keeps its raw `/V` but
/// draws the formatted text, as Acrobat does.
pub(crate) fn refresh(doc: &PdfDocument, e: &Entry) -> Result<()> {
    refresh_with(&Rt::off(doc), e)
}

/// [`refresh`] with the Format script of the field run by `rt` when it is not a plain AF call.
pub(crate) fn refresh_with(rt: &Rt, e: &Entry) -> Result<()> {
    settle_need_appearances_rt(rt, e.xref)?;
    refresh_one(rt, e)
}

/// A form that says `/NeedAppearances true` makes MuPDF redraw every field from its raw `/V` when
/// a page loads, so a formatted display (`$1,234.50`) would come back as `1234.5`. Once this
/// editor writes appearances the flag is cleared, after the other fields that draw from a value get
/// theirs too (many such forms carry no box or button appearances at all), so what the file shows is
/// what was written.
pub(crate) fn settle_need_appearances(doc: &PdfDocument, except: i32) -> Result<()> {
    // Preflight and other callers outside a fill: never run form scripts.
    settle_need_appearances_rt(&Rt::off(doc), except)
}

fn settle_need_appearances_rt(rt: &Rt, except: i32) -> Result<()> {
    let doc = rt.doc();
    let Some(mut acro) = doc.catalog().ok().and_then(|c| get(&c, "AcroForm")) else { return Ok(()) };
    if !get(&acro, "NeedAppearances").and_then(|n| n.as_bool().ok()).unwrap_or(false) {
        return Ok(());
    }
    acro.dict_put("NeedAppearances", PdfObject::new_bool(false))?;
    for o in scan(doc)? {
        if o.xref != except && matches!(o.kind, FieldKind::Text | FieldKind::Combo | FieldKind::List | FieldKind::Checkbox | FieldKind::Radio) {
            refresh_one(rt, &o)?;
        }
    }
    Ok(())
}

fn refresh_one(rt: &Rt, e: &Entry) -> Result<()> {
    let doc = rt.doc();
    let page = doc.load_pdf_page(e.page as i32)?;
    let mut w = page.load_widget(e.xref)?.ok_or_else(|| bad(format!("no field {} on page {}", e.xref, e.page + 1)))?;
    let mut field = e.field.clone();
    let raw = geti(&field, "V").and_then(|v| text(&v));
    let shown = match (&raw, e.kind) {
        (Some(r), FieldKind::Text | FieldKind::Combo) => {
            let f = rt.shown(None, e, r);
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

/// The `/AcroForm /CO` array: field object numbers, in the order the calculations run.
pub(crate) fn written_calc_order(doc: &PdfDocument) -> Vec<i32> {
    doc.catalog()
        .ok()
        .and_then(|c| get(&c, "AcroForm"))
        .and_then(|f| get(&f, "CO"))
        .map(|a| (0..a.len().unwrap_or(0).min(4096) as i32).filter_map(|i| a.get_array(i).ok().flatten()?.as_indirect().ok()).collect())
        .unwrap_or_default()
}

/// Every field with a calculate script (`node`), in running order: those in `/CO` first, in its order, then the rest as listed.
fn calc_order_of(doc: &PdfDocument, entries: &[Entry]) -> Vec<i32> {
    let mut calc: Vec<i32> = per_field(entries).into_iter().filter(|e| aa_js(e, "C").is_some()).map(|e| e.node).collect();
    let order = written_calc_order(doc);
    calc.sort_by_key(|n| order.iter().position(|x| x == n).unwrap_or(usize::MAX));
    calc
}

fn names_to_values(entries: &[Entry]) -> HashMap<String, String> {
    per_field(entries)
        .into_iter()
        .filter(|e| !matches!(e.kind, FieldKind::Button | FieldKind::Signature))
        .map(|e| (e.name.clone(), values_of(geti(&e.field, "V")).into_iter().next().unwrap_or_default()))
        .collect()
}

fn pages_of(mut v: Vec<u32>) -> Vec<u32> {
    v.sort_unstable();
    v.dedup();
    v
}

/// What turning a check box or radio button on or off stores: the field's `/V` and the `/AS` of
/// each of its widgets (every widget whose on-state is the chosen name turns on, the others off).
/// `None` when a radio group that cannot be switched off is asked to be.
fn button_plan(entries: &[Entry], e: &Entry, value: &FieldValue) -> Result<Option<(String, Vec<(i32, String)>)>> {
    let on = on_state(&e.obj).unwrap_or_else(|| "Yes".into());
    // A radio button is chosen by name or by being turned on.
    let want = match value {
        FieldValue::Checked(b) => b.then(|| on.clone()),
        FieldValue::Text(s) if s == "Off" || s.is_empty() => None,
        FieldValue::Text(s) => Some(s.clone()),
        FieldValue::Many(_) => return Err(bad("a check box takes on or off")),
    };
    if want.is_none() && e.kind == FieldKind::Radio && flag(field_ff(e), FieldFlags::NO_TOGGLE_TO_OFF) {
        return Ok(None);
    }
    let states = entries
        .iter()
        .filter(|w| w.node == e.node)
        .map(|w| {
            let mine = on_state(&w.obj).unwrap_or_else(|| "Yes".into());
            let state = match &want {
                Some(s) if *s == mine => mine,
                _ => "Off".to_string(),
            };
            (w.xref, state)
        })
        .collect();
    Ok(Some((want.unwrap_or_else(|| "Off".into()), states)))
}

/// True when `set_value` would leave the document exactly as it is (a radio button that is already
/// chosen, text typed that is already there): such a fill is not an undo step.
fn set_value_is_noop(doc: &PdfDocument, page: u32, xref: i32, value: &FieldValue) -> Result<bool> {
    let entries = scan(doc)?;
    let Some(e) = entries.iter().find(|e| e.page == page && e.xref == xref) else { return Ok(false) };
    if flag(field_ff(e), FieldFlags::READ_ONLY) {
        return Ok(false);
    }
    let current = values_of(geti(&e.field, "V"));
    Ok(match e.kind {
        FieldKind::Button | FieldKind::Signature => false,
        FieldKind::Checkbox | FieldKind::Radio => match button_plan(&entries, e, value) {
            Ok(None) => true,
            Ok(Some((v, states))) => {
                let cur_v = current.first().map_or("Off", String::as_str);
                let same_as = |w: i32, state: &str| {
                    entries.iter().find(|x| x.xref == w).is_some_and(|x| get(&x.obj, "AS").as_ref().and_then(text).as_deref().unwrap_or("Off") == state)
                };
                (cur_v == v || (cur_v.is_empty() && v == "Off")) && states.iter().all(|(w, s)| same_as(*w, s))
            }
            Err(_) => false,
        },
        FieldKind::Text | FieldKind::Combo | FieldKind::List => {
            let typed: Vec<&String> = match value {
                FieldValue::Text(s) => vec![s],
                FieldValue::Many(v) => v.iter().collect(),
                FieldValue::Checked(_) => return Ok(false),
            };
            let typed: Vec<&str> = typed.into_iter().map(String::as_str).filter(|s| !s.is_empty()).collect();
            let current: Vec<&str> = current.iter().map(String::as_str).filter(|s| !s.is_empty()).collect();
            typed == current
        }
    })
}

/// Sets a field. A value that does not fit the field's format or range is rejected with
/// Acrobat's message; the document is left unchanged. Returns the pages that changed. (Native code only;
/// [`set_value_one`] and [`set_value_report`] run the field's scripts too.)
pub fn set_value(doc: &mut PdfDocument, page: u32, xref: i32, value: &FieldValue) -> Result<Vec<u32>> {
    set_value_rt(&Rt::off(doc), page, xref, value)
}

fn set_value_rt(rt: &Rt, page: u32, xref: i32, value: &FieldValue) -> Result<Vec<u32>> {
    let entries = rt.entries()?;
    let e = entries.iter().find(|e| e.page == page && e.xref == xref).ok_or_else(|| bad(format!("no field {xref} on page {}", page + 1)))?;
    let ff = field_ff(e);
    if flag(ff, FieldFlags::READ_ONLY) {
        return Err(bad(format!("The field [ {} ] is read-only", e.name)));
    }
    // Typing never silently breaks a signature: a field a signature covers (its /Lock, a FieldMDP or a DocMDP of 1) is refused.
    if e.kind != FieldKind::Signature && scripts::locked(&scripts::locks_of(entries), e) {
        return Err(bad(format!("The field [ {} ] is locked by a signature", e.name)));
    }
    let mut touched = vec![page];
    let mut changed = Vec::new();
    let value = match e.kind {
        FieldKind::Button | FieldKind::Signature => return Err(bad("this field has no value to fill")),
        FieldKind::Checkbox | FieldKind::Radio => value.clone(),
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
                let (kept, by_key) = rt.keystroke(e, &stored).map_err(bad)?;
                stored = kept;
                changed.extend(by_key);
                changed.extend(rt.validate(e, &stored).map_err(bad)?);
            }
            if strings.len() > 1 { FieldValue::Many(strings) } else { FieldValue::Text(stored) }
        }
    };
    touched.extend(store(rt, e, &value)?);
    // What the key and validate scripts did to other fields (values, colours, visibility ...).
    touched.extend(scripts::apply_changes(rt, &changed)?);
    touched.extend(scripts::cascade(rt)?);
    Ok(pages_of(touched))
}

/// Stores `value` in the field of `e` and redraws it: its `/V`, and the `/AS` of each widget of a
/// check box or radio group. No keystroke, validate or read-only check. Returns the pages it drew.
/// (Check boxes and radios keep the appearance they have: MuPDF draws both states.)
fn store(rt: &Rt, e: &Entry, value: &FieldValue) -> Result<Vec<u32>> {
    let doc = rt.doc();
    let entries = rt.entries()?;
    let mut field = e.field.clone();
    let mut touched = Vec::new();
    match e.kind {
        FieldKind::Button | FieldKind::Signature => return Err(bad("this field has no value to fill")),
        FieldKind::Checkbox | FieldKind::Radio => {
            let Some((v, states)) = button_plan(entries, e, value)? else { return Ok(Vec::new()) };
            put_name(&mut field, "V", &v)?;
            for (w, state) in states {
                let w = entries.iter().find(|x| x.xref == w).expect("a widget of the field");
                let mut o = w.obj.clone();
                put_name(&mut o, "AS", &state)?;
                touched.push(w.page);
            }
        }
        FieldKind::Text | FieldKind::Combo | FieldKind::List => {
            match value {
                FieldValue::Many(v) if v.len() > 1 => {
                    let mut a = doc.new_array()?;
                    for s in v {
                        a.array_push(PdfObject::new_string(s)?)?;
                    }
                    field.dict_put("V", a)?;
                }
                FieldValue::Many(v) => put_str(&mut field, "V", v.first().map_or("", String::as_str))?,
                FieldValue::Text(s) => put_str(&mut field, "V", s)?,
                FieldValue::Checked(_) => return Err(bad("a text field takes text")),
            }
            for w in entries.iter().filter(|w| w.node == e.node) {
                refresh_with(rt, w)?;
                touched.push(w.page);
            }
        }
    }
    Ok(touched)
}

/// A script's value for a field, stored (see [`store`]).
pub(crate) fn write_values(rt: &Rt, e: &Entry, values: &[String]) -> Result<Vec<u32>> {
    store(rt, e, &scripts::as_field_value(values, e.kind))
}

/// Resets fields to their default values: the fields whose widgets are listed (by xref), or the
/// whole form ("Clear Form") when `which` is `None`. Returns the pages that changed.
pub fn reset(doc: &mut PdfDocument, which: Option<&[i32]>) -> Result<Vec<u32>> {
    reset_rt(&Rt::off(doc), which)
}

fn reset_rt(rt: &Rt, which: Option<&[i32]>) -> Result<Vec<u32>> {
    let entries = rt.entries()?;
    let locks = scripts::locks_of(entries);
    let mut done: Vec<i32> = Vec::new();
    let mut touched = Vec::new();
    for e in entries {
        // Only fields that hold something other than their default are touched.
        if done.contains(&e.node) || !reset_differs(entries, &locks, e, which) {
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
                refresh_with(rt, w)?;
            }
            touched.push(w.page);
        }
    }
    if done.is_empty() {
        return Ok(Vec::new());
    }
    // Reset may have emptied what calculations read.
    touched.extend(scripts::cascade(rt)?);
    Ok(pages_of(touched))
}

/// True when resetting `which` would change something (so an empty reset is not an undo step).
fn reset_would_change(doc: &PdfDocument, which: Option<&[i32]>) -> Result<bool> {
    let entries = scan(doc)?;
    let locks = scripts::locks_of(&entries);
    Ok(entries.iter().any(|e| reset_differs(&entries, &locks, e, which)))
}

/// Whether `e` is one of the fields being reset and holds something other than its default. A field a signature
/// covers is left as it is.
fn reset_differs(entries: &[Entry], locks: &[scripts::Lock], e: &Entry, which: Option<&[i32]>) -> bool {
    let pick = match which {
        None => true,
        Some(w) => w.contains(&e.xref) || w.contains(&e.node),
    };
    if !pick || matches!(e.kind, FieldKind::Button | FieldKind::Signature) || flag(field_ff(e), FieldFlags::READ_ONLY) || scripts::locked(locks, e) {
        return false;
    }
    let is_button = matches!(e.kind, FieldKind::Checkbox | FieldKind::Radio);
    let norm = |v: Vec<String>| -> Vec<String> { v.into_iter().filter(|s| !s.is_empty() && !(is_button && s == "Off")).collect() };
    let dv = norm(values_of(geti(&e.field, "DV")));
    if norm(values_of(geti(&e.field, "V"))) != dv {
        return true;
    }
    // The widgets of a box or button show the default too.
    is_button
        && entries.iter().filter(|w| w.node == e.node).any(|w| {
            let mine = on_state(&w.obj).unwrap_or_else(|| "Yes".into());
            let want = if dv.first() == Some(&mine) { mine } else { "Off".to_string() };
            get(&w.obj, "AS").as_ref().and_then(text).as_deref().unwrap_or("Off") != want
        })
}

// ---- engine wiring: one undo step each ---------------------------------------------------------

fn rt_of<'a>(d: &'a PdfDocument, scripts: &'a js::Scripts, name: &str) -> Rt<'a> {
    Rt::new(d, Some(scripts), name)
}

pub fn list_all(eng: &Engine, doc: DocId) -> Result<FormDto> {
    let (scripts, name) = eng.doc_scripts(doc)?;
    eng.read_custom(doc, |d| list_with(&rt_of(d, &scripts, &name)))
}

/// What the Scripts note of the forms rail shows.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ScriptsState {
    pub has_scripts: bool,
    pub enabled: bool,
    /// They can run now (not taken down by timeouts or a worker that stopped answering).
    pub usable: bool,
    /// They may be turned on again (not after the worker stopped answering: it is left abandoned until the app exits).
    pub can_enable: bool,
    pub log: Vec<js::LogLine>,
}

/// The scripts of a document as the banner shows them. Does not wait for the edit lane once a listing has been
/// made (a long step or listing holds the lane); the first call reads the document.
pub fn scripts_state(eng: &Engine, doc: DocId) -> Result<ScriptsState> {
    let (scripts, _) = eng.doc_scripts(doc)?;
    let has_scripts = match scripts.has_scripts() {
        Some(h) => h,
        None => {
            let h = eng.read_custom(doc, |d| Ok(scripts::has_scripts_with(None, d, &scan(d)?)))?;
            scripts.set_has_scripts(h);
            h
        }
    };
    Ok(ScriptsState { has_scripts, enabled: scripts.enabled(), usable: scripts.usable(), can_enable: scripts.can_enable(), log: scripts.log() })
}

/// Turns the scripts of one open document on or off (kept in memory, not in the file).
pub fn scripts_enable(eng: &Engine, doc: DocId, on: bool) -> Result<ScriptsState> {
    let (scripts, _) = eng.doc_scripts(doc)?;
    // Refused after a worker stopped answering; the state says so and the log has the reason.
    scripts.set_enabled(on);
    scripts_state(eng, doc)
}

pub fn set_value_one(eng: &Engine, doc: DocId, page: u32, xref: i32, value: &FieldValue, source: EditSource) -> Result<EditState> {
    set_value_report(eng, doc, page, xref, value, source).map(|s| s.state)
}

/// [`set_value_one`] with what the scripts raised (alerts to show, requests to ask the person about).
pub fn set_value_report(eng: &Engine, doc: DocId, page: u32, xref: i32, value: &FieldValue, source: EditSource) -> Result<Stepped> {
    // Filling what is already there changes nothing, so it is not a step in the history.
    if eng.read_custom(doc, |d| Ok(set_value_is_noop(d, page, xref, value).unwrap_or(false)))? {
        return Ok(Stepped { state: eng.edit_state(doc)?, report: Report::default() });
    }
    let (scripts, name) = eng.doc_scripts(doc)?;
    let mut report = Report::default();
    let state = eng.apply_custom(doc, format!("Fill field on p. {}", page + 1), source, false, |d| {
        let rt = rt_of(d, &scripts, &name);
        let r = set_value_rt(&rt, page, xref, value);
        report = rt.report.take();
        r
    })?;
    Ok(Stepped { state, report })
}

pub fn reset_one(eng: &Engine, doc: DocId, which: Option<Vec<i32>>, source: EditSource) -> Result<EditState> {
    reset_report(eng, doc, which, source).map(|s| s.state)
}

pub fn reset_report(eng: &Engine, doc: DocId, which: Option<Vec<i32>>, source: EditSource) -> Result<Stepped> {
    // A reset with nothing to reset is not a step in the history.
    if !eng.read_custom(doc, |d| reset_would_change(d, which.as_deref()))? {
        return Ok(Stepped { state: eng.edit_state(doc)?, report: Report::default() });
    }
    let label = if which.is_none() { "Clear form".to_string() } else { "Reset fields".to_string() };
    let (scripts, name) = eng.doc_scripts(doc)?;
    let mut report = Report::default();
    let state = eng.apply_custom(doc, label, source, false, |d| {
        let rt = rt_of(d, &scripts, &name);
        let r = reset_rt(&rt, which.as_deref());
        report = rt.report.take();
        r
    })?;
    Ok(Stepped { state, report })
}
