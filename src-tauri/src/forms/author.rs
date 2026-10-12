//! "Prepare a form": create, move, resize and delete fields, edit their properties, set the tab
//! order, and find fields on a flat form. Fields are built by hand as AcroForm dictionaries
//! (MuPDF only creates signature widgets); `widget.update()` then draws their appearance.

use super::af::{self, ButtonAction, Calculate, Format, Validate};
use super::detect::{self, Kind as DetectKind};
use super::*;
use mupdf::pdf::PdfPage;
use mupdf::{Buffer, Matrix};
use mupdf::text_page::TextPageFlags;

/// What to create. `rect` is in page space.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NewField {
    pub kind: FieldKind,
    pub rect: [f32; 4],
    pub name: Option<String>,
    /// A radio button joins the group of this name.
    pub group: Option<String>,
    pub export: Option<String>,
    pub caption: Option<String>,
    pub options: Option<Vec<Opt>>,
}

/// Changes to a field's properties; absent entries stay.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FieldPatch {
    pub name: Option<String>,
    pub tooltip: Option<String>,
    pub required: Option<bool>,
    pub read_only: Option<bool>,
    pub hidden: Option<bool>,
    pub default_value: Option<String>,
    pub multiline: Option<bool>,
    pub password: Option<bool>,
    pub comb: Option<bool>,
    pub editable: Option<bool>,
    pub multi_select: Option<bool>,
    /// 0 removes the limit.
    pub max_len: Option<u32>,
    pub options: Option<Vec<Opt>>,
    pub export: Option<String>,
    pub font_size: Option<f32>,
    pub text_color: Option<[f32; 3]>,
    pub align: Option<u8>,
    pub border_color: Option<[f32; 3]>,
    #[serde(default)]
    pub no_border: bool,
    pub fill_color: Option<[f32; 3]>,
    #[serde(default)]
    pub no_fill: bool,
    pub border_width: Option<f32>,
    pub format: Option<Format>,
    pub validate: Option<Validate>,
    pub calculate: Option<Calculate>,
    pub caption: Option<String>,
    pub action: Option<ButtonAction>,
}

const SIZES: [(&str, FieldKind); 7] = [
    ("Text", FieldKind::Text),
    ("Check Box", FieldKind::Checkbox),
    ("Radio Button", FieldKind::Radio),
    ("Dropdown", FieldKind::Combo),
    ("List Box", FieldKind::List),
    ("Button", FieldKind::Button),
    ("Signature", FieldKind::Signature),
];

fn base_name(k: FieldKind) -> &'static str {
    SIZES.iter().find(|(_, x)| *x == k).map_or("Field", |(n, _)| n)
}

/// An export value is the name a box or button turns on to: not empty, and never `Off`, which is
/// what every box and button shows when it is not chosen.
fn check_export(export: &str) -> Result<()> {
    if export.trim().is_empty() {
        return Err(bad("an export value cannot be empty"));
    }
    if export == "Off" {
        return Err(bad("an export value cannot be Off, which is the name of the unchosen state"));
    }
    Ok(())
}

/// `Text1`, `Text2`, ... the first one that no field uses.
fn unique_name(taken: &[String], base: &str) -> String {
    (1..).map(|n| format!("{base}{n}")).find(|n| !taken.contains(n)).unwrap_or_default()
}

pub(crate) fn ensure_dict(doc: &PdfDocument, parent: &mut PdfObject, key: &str) -> Result<PdfObject> {
    if let Some(d) = get(parent, key).filter(|d| d.is_dict().unwrap_or(false)) {
        return Ok(d);
    }
    // `PdfObject::clone` is a deep copy: edits must land in the dictionary the parent holds.
    parent.dict_put(key, doc.new_dict()?)?;
    get(parent, key).ok_or_else(|| bad("could not create the dictionary"))
}

/// The AcroForm dictionary, created with the default appearance and resources if missing.
pub(crate) fn acro_form(doc: &mut PdfDocument) -> Result<PdfObject> {
    let mut catalog = doc.catalog()?;
    let mut form = ensure_dict(doc, &mut catalog, "AcroForm")?;
    if get(&form, "Fields").is_none() {
        form.dict_put("Fields", doc.new_array()?)?;
    }
    if get(&form, "DA").is_none() {
        put_str(&mut form, "DA", "/Helv 0 Tf 0 g")?;
    }
    let mut dr = ensure_dict(doc, &mut form, "DR")?;
    let mut fonts = ensure_dict(doc, &mut dr, "Font")?;
    for (key, base) in [("Helv", "Helvetica"), ("ZaDb", "ZapfDingbats")] {
        if get(&fonts, key).is_none() {
            let mut f = doc.new_dict()?;
            put_name(&mut f, "Type", "Font")?;
            put_name(&mut f, "Subtype", "Type1")?;
            put_name(&mut f, "BaseFont", base)?;
            if key == "Helv" {
                put_name(&mut f, "Encoding", "WinAnsiEncoding")?;
            }
            let r = doc.add_object(&f)?;
            fonts.dict_put(key, r)?;
        }
    }
    Ok(form)
}

/// An appearance that draws nothing, for an unsigned signature box: MuPDF never draws or writes its own (a
/// red "SIGN" arrow), and a file that checks for one (PDF/A) finds it.
fn blank_appearance(doc: &mut PdfDocument, w: &mut PdfObject) -> Result<()> {
    let mut form = doc.new_dict()?;
    put_name(&mut form, "Type", "XObject")?;
    put_name(&mut form, "Subtype", "Form")?;
    put_nums(doc, &mut form, "BBox", &[0.0, 0.0, 1.0, 1.0])?;
    let stream = doc.add_stream(&Buffer::from_bytes(b"q Q")?, Some(&form), false)?;
    let mut ap = doc.new_dict()?;
    ap.dict_put("N", stream)?;
    w.dict_put("AP", ap)?;
    Ok(())
}

/// Gives an `/AP /N` a stand-in stream under the on-state's name so MuPDF keeps that name when
/// it draws the real appearance.
pub(crate) fn seed_on_state(doc: &mut PdfDocument, w: &mut PdfObject, on: &str) -> Result<()> {
    let mut form = doc.new_dict()?;
    put_name(&mut form, "Type", "XObject")?;
    put_name(&mut form, "Subtype", "Form")?;
    put_nums(doc, &mut form, "BBox", &[0.0, 0.0, 1.0, 1.0])?;
    let stream = doc.add_stream(&Buffer::from_bytes(b"q Q")?, Some(&form), false)?;
    let mut n = doc.new_dict()?;
    n.dict_put(on, stream)?;
    let mut ap = doc.new_dict()?;
    ap.dict_put("N", n)?;
    w.dict_put("AP", ap)?;
    Ok(())
}

pub(crate) fn color_array(doc: &PdfDocument, o: &mut PdfObject, k: &str, c: [f32; 3]) -> Result<()> {
    put_nums(doc, o, k, &c)
}

fn page_inverse(doc: &PdfDocument, page: u32) -> Result<Matrix> {
    let p = doc.load_pdf_page(page as i32)?;
    p.ctm()?.invert().ok_or_else(|| bad("the page transform cannot be inverted"))
}

/// Normalised user-space `/Rect` for a page-space rectangle.
pub(crate) fn user_rect(doc: &PdfDocument, page: u32, r: [f32; 4]) -> Result<Rect> {
    let r = rect_of(r);
    if r.x1 - r.x0 < 2.0 || r.y1 - r.y0 < 2.0 {
        return Err(bad("a field needs a rectangle of at least 2 x 2 points"));
    }
    let u = r.transform(&page_inverse(doc, page)?);
    Ok(Rect { x0: u.x0.min(u.x1), y0: u.y0.min(u.y1), x1: u.x0.max(u.x1), y1: u.y0.max(u.y1) })
}

fn default_size(kind: FieldKind, r: &Rect) -> f32 {
    match kind {
        FieldKind::Text | FieldKind::Combo | FieldKind::List => 0.0,
        _ => (r.y1 - r.y0 - 4.0).clamp(6.0, 12.0),
    }
}

/// Authoring (adding, moving, deleting or changing a field, and the tab and calculation order) changes the structure
/// of the form, which no certification allows: `/P` names what may change after it, and this is none of those. A
/// document whose certification cannot be read is refused too.
pub(crate) fn refuse_if_certified(doc: &PdfDocument) -> Result<()> {
    if crate::outline::certified(doc).map_or(true, |c| c.is_some()) {
        return Err(bad("This document is certified, and certification does not allow changes to its form fields. Inkwell leaves them as they are."));
    }
    Ok(())
}

/// Creates one field on `page`; returns the xref of its (first) widget.
pub fn add(doc: &mut PdfDocument, page: u32, what: &NewField) -> Result<i32> {
    refuse_if_certified(doc)?;
    let entries = scan(doc)?;
    let taken: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let rect = user_rect(doc, page, what.rect)?;
    let form = acro_form(doc)?;
    let mut widget = doc.new_dict()?;
    put_name(&mut widget, "Type", "Annot")?;
    put_name(&mut widget, "Subtype", "Widget")?;
    put_int(&mut widget, "F", 4)?;
    let mut r = doc.new_array()?;
    rect.encode_into(&mut r)?;
    widget.dict_put("Rect", r)?;
    let mut bs = doc.new_dict()?;
    put_int(&mut bs, "W", 1)?;
    put_name(&mut bs, "S", "S")?;
    widget.dict_put("BS", bs)?;
    let mut mk = doc.new_dict()?;
    let ft = match what.kind {
        FieldKind::Text | FieldKind::Combo | FieldKind::List => "Tx",
        FieldKind::Signature => "Sig",
        _ => "Btn",
    };
    let ft = if matches!(what.kind, FieldKind::Combo | FieldKind::List) { "Ch" } else { ft };
    let mut ff = 0i32;
    let zadb = matches!(what.kind, FieldKind::Checkbox | FieldKind::Radio);
    match what.kind {
        FieldKind::Checkbox | FieldKind::Radio => {
            if what.kind == FieldKind::Radio {
                ff = FieldFlags::RADIO.bits() | FieldFlags::NO_TOGGLE_TO_OFF.bits();
            }
            put_str(&mut mk, "CA", if what.kind == FieldKind::Radio { "l" } else { "4" })?;
            put_name(&mut widget, "AS", "Off")?;
        }
        FieldKind::Button => {
            ff = FieldFlags::PUSHBUTTON.bits();
            put_str(&mut mk, "CA", what.caption.as_deref().unwrap_or("Button"))?;
            color_array(doc, &mut mk, "BG", [0.82, 0.84, 0.88])?;
            color_array(doc, &mut mk, "BC", [0.0, 0.0, 0.0])?;
        }
        FieldKind::Combo => ff = FieldFlags::COMBO.bits(),
        _ => {}
    }
    widget.dict_put("MK", mk)?;
    if what.kind != FieldKind::Signature {
        let da = if zadb {
            format!("/ZaDb {} Tf 0 g", default_size(what.kind, &rect))
        } else {
            format!("/Helv {} Tf 0 g", default_size(what.kind, &rect))
        };
        put_str(&mut widget, "DA", &da)?;
    }
    if matches!(what.kind, FieldKind::Combo | FieldKind::List) {
        let mut a = doc.new_array()?;
        for o in what.options.as_deref().unwrap_or(&[]) {
            if o.export == o.label {
                a.array_push(PdfObject::new_string(&o.export)?)?;
            } else {
                let mut p = doc.new_array()?;
                p.array_push(PdfObject::new_string(&o.export)?)?;
                p.array_push(PdfObject::new_string(&o.label)?)?;
                a.array_push(p)?;
            }
        }
        widget.dict_put("Opt", a)?;
    }

    // The group a radio button joins, or the new field's own name.
    let existing_group = what.group.as_ref().and_then(|g| entries.iter().find(|e| e.kind == FieldKind::Radio && e.name == *g));
    if what.kind == FieldKind::Radio && what.group.is_some() && existing_group.is_none() {
        return Err(bad(format!("no radio group named {}", what.group.as_deref().unwrap_or(""))));
    }
    // A button that joins a group is written into the group's /Kids: a group a signature covers stays as it is.
    if let Some(g) = existing_group {
        super::layout::guard(doc, &entries, &[g])?;
    }
    let name = what.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        if let Some(g) = existing_group { g.name.clone() } else { unique_name(&taken, base_name(what.kind)) }
    });
    if existing_group.is_none() && taken.contains(&name) {
        return Err(bad(format!("A field named {name} already exists")));
    }

    let widget_ref;
    if what.kind == FieldKind::Radio {
        let kids = existing_group.map_or(0, |g| entries.iter().filter(|e| e.node == g.node).count());
        let export = what.export.clone().unwrap_or_else(|| format!("Choice{}", kids + 1));
        check_export(&export)?;
        seed_on_state(doc, &mut widget, &export)?;
        let mut parent = match existing_group {
            Some(g) => g.field.clone(),
            None => {
                let mut p = doc.new_dict()?;
                put_name(&mut p, "FT", "Btn")?;
                put_str(&mut p, "T", &name)?;
                put_int(&mut p, "Ff", ff)?;
                put_name(&mut p, "V", "Off")?;
                p.dict_put("Kids", doc.new_array()?)?;
                let pr = doc.add_object(&p)?;
                get(&form, "Fields").ok_or_else(|| bad("the form has no field list"))?.array_push(pr.clone())?;
                pr
            }
        };
        let (parent, parent_ref) = match existing_group {
            Some(_) => (parent.clone(), parent.clone()),
            None => (parent.clone(), std::mem::replace(&mut parent, PdfObject::new_null())),
        };
        widget.dict_put("Parent", parent_ref.clone())?;
        widget_ref = doc.add_object(&widget)?;
        let mut kids = get(&parent, "Kids").ok_or_else(|| bad("a radio group has no /Kids"))?;
        kids.array_push(widget_ref.clone())?;
    } else {
        put_name(&mut widget, "FT", ft)?;
        put_str(&mut widget, "T", &name)?;
        if ff != 0 {
            put_int(&mut widget, "Ff", ff)?;
        }
        if what.kind == FieldKind::Checkbox {
            let export = what.export.clone().unwrap_or_else(|| "Yes".into());
            check_export(&export)?;
            seed_on_state(doc, &mut widget, &export)?;
            put_name(&mut widget, "V", "Off")?;
        }
        if what.kind == FieldKind::Signature {
            blank_appearance(doc, &mut widget)?;
        }
        widget_ref = doc.add_object(&widget)?;
        get(&form, "Fields").ok_or_else(|| bad("the form has no field list"))?.array_push(widget_ref.clone())?;
    }
    let xref = widget_ref.as_indirect()?;

    // On the page (the end of /Annots is the end of the tab order).
    let mut page_obj = doc.find_page(page as i32)?;
    match get(&page_obj, "Annots").filter(|a| a.is_array().unwrap_or(false)) {
        Some(mut a) => a.array_push(widget_ref)?,
        None => {
            let mut a = doc.new_array()?;
            a.array_push(widget_ref)?;
            page_obj.dict_put("Annots", a)?;
        }
    }
    redraw(doc, page, xref)?;
    Ok(xref)
}

/// Rebuilds one widget's appearance (formatted value included).
fn redraw(doc: &PdfDocument, page: u32, xref: i32) -> Result<()> {
    let entries = scan_page(doc, page)?;
    let e = entries.iter().find(|e| e.xref == xref).ok_or_else(|| bad(format!("no field {xref} on page {}", page + 1)))?;
    refresh(doc, e)
}

fn scan_page(doc: &PdfDocument, page: u32) -> Result<Vec<Entry>> {
    Ok(scan(doc)?.into_iter().filter(|e| e.page == page).collect())
}

/// Moves and resizes a widget (`rect` in page space).
pub fn move_to(doc: &mut PdfDocument, page: u32, xref: i32, rect: [f32; 4]) -> Result<()> {
    refuse_if_certified(doc)?;
    let r = rect_of(rect);
    if r.x1 - r.x0 < 2.0 || r.y1 - r.y0 < 2.0 {
        return Err(bad("a field needs a rectangle of at least 2 x 2 points"));
    }
    let entries = scan(doc)?;
    let e = entries.iter().find(|e| e.page == page && e.xref == xref).ok_or_else(|| bad(format!("no field {xref} on page {}", page + 1)))?;
    super::layout::guard(doc, &entries, &[e])?;
    {
        let p = doc.load_pdf_page(page as i32)?;
        let w = p.load_widget(xref)?.ok_or_else(|| bad(format!("no field {xref} on page {}", page + 1)))?;
        let mut a = w.into_annotation();
        a.set_rect(r)?;
    }
    redraw(doc, page, xref)
}

/// Removes fields (every widget of a field named by a listed widget when `whole`, else just the
/// listed widgets; a radio group that loses its last button goes too).
pub fn delete(doc: &mut PdfDocument, page: u32, xrefs: &[i32]) -> Result<()> {
    refuse_if_certified(doc)?;
    let entries = scan(doc)?;
    for x in xrefs {
        if !entries.iter().any(|e| e.page == page && e.xref == *x) {
            return Err(bad(format!("no field {x} on page {}", page + 1)));
        }
    }
    let picked: Vec<&Entry> = entries.iter().filter(|e| e.page == page && xrefs.contains(&e.xref)).collect();
    super::layout::guard(doc, &entries, &picked)?;
    let form = doc.catalog()?.get_dict("AcroForm")?;
    let mut p = doc.load_pdf_page(page as i32)?;
    for x in xrefs {
        let Some(e) = entries.iter().find(|e| e.page == page && e.xref == *x) else { continue };
        let w = p.load_widget(*x)?.ok_or_else(|| bad("the field has gone"))?;
        p.delete_widget(w)?;
        // Out of its parent's /Kids, or the form's /Fields.
        if e.node != e.xref {
            let siblings = entries.iter().filter(|s| s.node == e.node && s.xref != e.xref && !xrefs.contains(&s.xref)).count();
            if let Some(mut kids) = get(&e.field, "Kids") {
                remove_ref(&mut kids, *x)?;
            }
            if siblings == 0 {
                if let Some(f) = form.as_ref().and_then(|f| get(f, "Fields")) {
                    let mut f = f;
                    remove_ref(&mut f, e.node)?;
                }
            }
        } else if let Some(mut f) = form.as_ref().and_then(|f| get(f, "Fields")) {
            remove_ref(&mut f, *x)?;
        }
        // A field with other widgets left keeps its place in the calculation order.
        let left = entries.iter().filter(|s| s.node == e.node && !xrefs.contains(&s.xref)).count();
        if left == 0 {
            if let Some(mut co) = form.as_ref().and_then(|f| get(f, "CO")) {
                remove_ref(&mut co, e.node)?;
            }
        }
    }
    let entries = scan(doc)?;
    // The remaining calculations may have read the deleted fields.
    let _ = entries;
    Ok(())
}

pub(crate) fn remove_ref(a: &mut PdfObject, num: i32) -> Result<()> {
    let n = a.len()? as i32;
    for i in (0..n).rev() {
        if a.get_array(i)?.is_some_and(|v| v.is_indirect().unwrap_or(false) && v.as_indirect().ok() == Some(num)) {
            a.array_delete(i)?;
        }
    }
    Ok(())
}

pub(crate) fn set_flag(ff: &mut i32, f: FieldFlags, on: bool) {
    if on {
        *ff |= f.bits();
    } else {
        *ff &= !f.bits();
    }
}

/// The additional-action keys a field may carry (PDF 32000, table 196).
const AA_KEYS: [&str; 14] = ["E", "X", "D", "U", "Fo", "Bl", "PO", "PC", "PV", "PI", "K", "F", "V", "C"];

/// An additional action running `js`, replacing the one under `key`. The /AA dictionary is built
/// whole and put once: a nested dictionary changed through an old handle can be left behind by the
/// document's journal.
pub(crate) fn put_js(doc: &PdfDocument, field: &mut PdfObject, key: &str, js: Option<String>) -> Result<()> {
    let mut aa = doc.new_dict()?;
    let mut n = 0;
    if let Some(old) = get(field, "AA") {
        for k in AA_KEYS.into_iter().filter(|k| *k != key) {
            if let Some(v) = get(&old, k) {
                aa.dict_put(k, v)?;
                n += 1;
            }
        }
    }
    if let Some(js) = js {
        let mut a = doc.new_dict()?;
        put_name(&mut a, "S", "JavaScript")?;
        put_str(&mut a, "JS", &js)?;
        aa.dict_put(key, a)?;
        n += 1;
    }
    if n > 0 {
        field.dict_put("AA", aa)?;
    } else {
        field.dict_delete("AA")?;
    }
    Ok(())
}

pub fn edit(doc: &mut PdfDocument, page: u32, xref: i32, patch: &FieldPatch) -> Result<()> {
    refuse_if_certified(doc)?;
    let entries = scan(doc)?;
    let e = entries.iter().find(|e| e.page == page && e.xref == xref).ok_or_else(|| bad(format!("no field {xref} on page {}", page + 1)))?;
    // Every change, not only a new name: a field a signature covers, and a signed signature field, stay as they are.
    super::layout::guard(doc, &entries, &[e])?;
    let mut field = e.field.clone();
    let mut widget = e.obj.clone();
    let siblings: Vec<&Entry> = entries.iter().filter(|s| s.node == e.node).collect();
    let mut renamed: Option<(String, String)> = None;
    if let Some(n) = &patch.name {
        let n = n.trim();
        if n.is_empty() {
            return Err(bad("a field needs a name"));
        }
        // A dotted name sets the last part; the parents keep their names. What is checked is the full
        // name that is written: the parents' names and that last part.
        let leaf = n.rsplit('.').next().unwrap_or(n).trim();
        if leaf.is_empty() {
            return Err(bad("a field needs a name"));
        }
        let parents = get(&field, "Parent").map(|p| full_name(&p)).unwrap_or_default();
        let full = if parents.is_empty() { leaf.to_string() } else { format!("{parents}.{leaf}") };
        if full != e.name {
            super::layout::valid_name(&full)?;
            let clash = |o: &Entry| o.node != e.node && (o.name == full || o.name.starts_with(&format!("{full}.")) || full.starts_with(&format!("{}.", o.name)));
            if entries.iter().any(clash) {
                return Err(bad(format!("A field named {full} already exists")));
            }
            check_rename(doc, &entries, &e.name, &full)?;
            renamed = Some((e.name.clone(), full));
        }
        let own_t = get(&field, "T").as_ref().and_then(text).unwrap_or_default();
        if own_t != leaf {
            put_str(&mut field, "T", leaf)?;
        }
    }
    let mut ff = int_of(&e.obj, "Ff").unwrap_or(0);
    let before = ff;
    if let Some(v) = patch.required {
        set_flag(&mut ff, FieldFlags::REQUIRED, v);
    }
    if let Some(v) = patch.read_only {
        set_flag(&mut ff, FieldFlags::READ_ONLY, v);
    }
    if e.kind == FieldKind::Text {
        if let Some(v) = patch.multiline {
            set_flag(&mut ff, FieldFlags::MULTILINE, v);
        }
        if let Some(v) = patch.password {
            set_flag(&mut ff, FieldFlags::PASSWORD, v);
        }
        if let Some(v) = patch.comb {
            set_flag(&mut ff, FieldFlags::COMB, v);
        }
    }
    if e.kind == FieldKind::Combo {
        if let Some(v) = patch.editable {
            set_flag(&mut ff, FieldFlags::EDIT, v);
        }
    }
    if e.kind == FieldKind::List {
        if let Some(v) = patch.multi_select {
            set_flag(&mut ff, FieldFlags::MULTI_SELECT, v);
        }
    }
    if ff != before {
        put_int(&mut field, "Ff", ff)?;
        if get(&e.obj, "Ff").is_some() && !std::ptr::eq(&field, &widget) && field.as_indirect().ok() != widget.as_indirect().ok() {
            // A widget that repeats /Ff would shadow the field's.
            let _ = widget.dict_delete("Ff");
        }
    }
    if let Some(t) = &patch.tooltip {
        if t.is_empty() {
            field.dict_delete("TU")?;
        } else {
            put_str(&mut field, "TU", t)?;
        }
    }
    if let Some(h) = patch.hidden {
        let f = int_of(&widget, "F").unwrap_or(4);
        for s in &siblings {
            let mut o = s.obj.clone();
            put_int(&mut o, "F", if h { (f | 2) & !4 } else { (f & !2) | 4 })?;
        }
    }
    if let Some(v) = &patch.default_value {
        if v.is_empty() {
            field.dict_delete("DV")?;
        } else {
            put_str(&mut field, "DV", v)?;
        }
    }
    if let Some(m) = patch.max_len {
        if m == 0 {
            widget.dict_delete("MaxLen")?;
        } else {
            put_int(&mut widget, "MaxLen", m.min(i32::MAX as u32) as i32)?;
        }
    }
    if let Some(opts) = &patch.options {
        let mut a = doc.new_array()?;
        for o in opts {
            if o.export == o.label {
                a.array_push(PdfObject::new_string(&o.export)?)?;
            } else {
                let mut p = doc.new_array()?;
                p.array_push(PdfObject::new_string(&o.export)?)?;
                p.array_push(PdfObject::new_string(&o.label)?)?;
                a.array_push(p)?;
            }
        }
        field.dict_put("Opt", a.clone())?;
        if get(&e.obj, "Opt").is_some() {
            widget.dict_put("Opt", a)?;
        }
    }
    if patch.font_size.is_some() || patch.text_color.is_some() {
        let (font, size, color) = parse_da(&da_of(&e.obj));
        let size = patch.font_size.map_or(size, |s| if s.is_finite() { s.clamp(0.0, 300.0) } else { 0.0 });
        let c = patch.text_color.map_or(color, |c| c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }));
        let da = format!("/{font} {size} Tf {} {} {} rg", c[0], c[1], c[2]);
        for s in &siblings {
            let mut o = s.obj.clone();
            put_str(&mut o, "DA", &da)?;
        }
    }
    if let Some(a) = patch.align {
        put_int(&mut widget, "Q", i32::from(a.min(2)))?;
    }
    if patch.no_border || patch.border_color.is_some() || patch.no_fill || patch.fill_color.is_some() || patch.caption.is_some() {
        let mut mk = ensure_dict(doc, &mut widget, "MK")?;
        if patch.no_border {
            mk.dict_delete("BC")?;
        } else if let Some(c) = patch.border_color {
            color_array(doc, &mut mk, "BC", c)?;
        }
        if patch.no_fill {
            mk.dict_delete("BG")?;
        } else if let Some(c) = patch.fill_color {
            color_array(doc, &mut mk, "BG", c)?;
        }
        if let Some(c) = &patch.caption {
            put_str(&mut mk, "CA", c)?;
        }
        widget.dict_put("MK", mk)?;
    }
    if let Some(w) = patch.border_width {
        let mut bs = ensure_dict(doc, &mut widget, "BS")?;
        bs.dict_put("W", PdfObject::new_real(w.clamp(0.0, 20.0))?)?;
        widget.dict_put("BS", bs)?;
    }
    // Scripts: what Acrobat's Format, Validate and Calculate tabs write.
    if let Some(f) = &patch.format {
        match af::format_js(f) {
            Some((fmt, key)) => {
                put_js(doc, &mut field, "F", (!fmt.is_empty()).then_some(fmt))?;
                put_js(doc, &mut field, "K", (!key.is_empty()).then_some(key))?;
            }
            None => {
                put_js(doc, &mut field, "F", None)?;
                put_js(doc, &mut field, "K", None)?;
            }
        }
    }
    if let Some(v) = &patch.validate {
        put_js(doc, &mut field, "V", af::validate_js(v))?;
    }
    if let Some(c) = &patch.calculate {
        put_js(doc, &mut field, "C", af::calculate_js(c))?;
        let mut form = acro_form(doc)?;
        let num = field.as_indirect().unwrap_or(e.node);
        let mut co = doc.new_array()?;
        if let Some(old) = get(&form, "CO") {
            for i in 0..old.len()? as i32 {
                if let Some(v) = old.get_array(i)? {
                    if !(v.is_indirect().unwrap_or(false) && v.as_indirect().ok() == Some(num)) {
                        co.array_push(v)?;
                    }
                }
            }
        }
        if af::calculate_js(c).is_some() {
            co.array_push(doc.new_indirect(num, 0)?)?;
        }
        form.dict_put("CO", co)?;
    }
    if let Some(a) = &patch.action {
        let mut act = doc.new_dict()?;
        match a {
            ButtonAction::Reset { fields, exclude } => {
                put_name(&mut act, "S", "ResetForm")?;
                if *exclude && !fields.is_empty() {
                    put_int(&mut act, "Flags", 1)?;
                }
                if !fields.is_empty() {
                    let mut arr = doc.new_array()?;
                    for f in fields {
                        arr.array_push(PdfObject::new_string(f)?)?;
                    }
                    act.dict_put("Fields", arr)?;
                }
            }
            ButtonAction::Uri { uri } => {
                put_name(&mut act, "S", "URI")?;
                put_str(&mut act, "URI", uri)?;
            }
            ButtonAction::Submit { url } => {
                put_name(&mut act, "S", "SubmitForm")?;
                let mut fs = doc.new_dict()?;
                put_name(&mut fs, "FS", "URL")?;
                put_str(&mut fs, "F", url)?;
                act.dict_put("F", fs)?;
            }
            ButtonAction::Named { name } => {
                put_name(&mut act, "S", "Named")?;
                put_name(&mut act, "N", name)?;
            }
            ButtonAction::Alert { message } => {
                put_name(&mut act, "S", "JavaScript")?;
                put_str(&mut act, "JS", &format!("app.alert({});", af_js_string(message)))?;
            }
            ButtonAction::Script { .. } => return Err(bad("Inkwell does not write scripts")),
        }
        widget.dict_put("A", act)?;
    }
    if let (Some(export), FieldKind::Checkbox | FieldKind::Radio) = (&patch.export, e.kind) {
        check_export(export)?;
        let was_on = get(&e.obj, "AS").as_ref().and_then(text).is_some_and(|s| s != "Off");
        seed_on_state(doc, &mut widget, export)?;
        put_name(&mut widget, "AS", if was_on { export } else { "Off" })?;
        if was_on {
            put_name(&mut field, "V", export)?;
        }
    }
    // Calculations that list the old name follow the field to its new one.
    if let Some((old, new)) = &renamed {
        follow_rename(doc, &entries, old, new)?;
    }
    for s in &siblings {
        refresh(doc, s)?;
    }
    Ok(())
}

/// The calculated fields whose `AFSimple_Calculate` list names `old` (or a field below it): one entry per field.
fn followers<'a>(entries: &'a [Entry], old: &str) -> Vec<(&'a Entry, af::CalcOp, Vec<String>)> {
    let below = format!("{old}.");
    let mut seen: Vec<i32> = Vec::new();
    let mut out = Vec::new();
    for e in entries {
        if seen.contains(&e.node) {
            continue;
        }
        seen.push(e.node);
        let Calculate::Simple { op, fields } = calculate_of(e) else { continue };
        if fields.iter().any(|f| f == old || f.starts_with(&below)) {
            out.push((e, op, fields));
        }
    }
    out
}

/// A field may take the name `new` in place of `old` when no signature covers `new` and the calculations that list
/// `old` are free to follow. The one check of every rename (properties, layout); run before anything is written.
pub(crate) fn check_rename(doc: &PdfDocument, entries: &[Entry], old: &str, new: &str) -> Result<()> {
    if super::scripts::name_locked(&super::scripts::locks_of(doc, entries), new) {
        return Err(bad(format!("The name [ {new} ] is covered by a signature, so no field can take it")));
    }
    check_follow_rename(doc, entries, old)
}

/// Renaming `old` rewrites the calculations that list it; one a signature locks cannot be rewritten, so the rename
/// is refused. Callers run this before writing anything.
pub(crate) fn check_follow_rename(doc: &PdfDocument, entries: &[Entry], old: &str) -> Result<()> {
    let todo = followers(entries, old);
    if todo.is_empty() {
        return Ok(());
    }
    let locks = super::scripts::locks_of(doc, entries);
    match todo.iter().find(|(e, _, _)| super::scripts::locked(&locks, e)) {
        Some((e, _, _)) => Err(bad(format!(
            "The calculation of [ {} ] is locked by a signature and would have to follow the new name of [ {old} ]",
            e.name
        ))),
        None => Ok(()),
    }
}

/// `AFSimple_Calculate` lists that name the field `old` (or a field below it) name `new` instead.
pub(crate) fn follow_rename(doc: &PdfDocument, entries: &[Entry], old: &str, new: &str) -> Result<()> {
    check_follow_rename(doc, entries, old)?;
    let below = format!("{old}.");
    for (e, op, fields) in followers(entries, old) {
        let fields = fields
            .into_iter()
            .map(|f| if f == old { new.to_string() } else if let Some(rest) = f.strip_prefix(&below) { format!("{new}.{rest}") } else { f })
            .collect();
        let mut field = e.field.clone();
        put_js(doc, &mut field, "C", af::calculate_js(&Calculate::Simple { op, fields }))?;
    }
    Ok(())
}

fn af_js_string(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            '\n' => o.push_str("\\n"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ---- tab order ---------------------------------------------------------------------------------

/// Orders the page's widgets: `order` first, any others after, in the slots the widgets
/// already hold in `/Annots`. `tabs` writes the page's `/Tabs` (`R`ow, `C`olumn, `S`tructure) or
/// removes it so the array order rules.
pub fn set_tab_order(doc: &mut PdfDocument, page: u32, order: &[i32], tabs: Option<&str>) -> Result<()> {
    refuse_if_certified(doc)?;
    let entries = scan_page(doc, page)?;
    let mut seen: Vec<i32> = Vec::with_capacity(order.len());
    for x in order {
        if !entries.iter().any(|e| e.xref == *x) {
            return Err(bad(format!("no field {x} on page {}", page + 1)));
        }
        if seen.contains(x) {
            return Err(bad(format!("field {x} is listed twice in the tab order")));
        }
        seen.push(*x);
    }
    let mut want: Vec<i32> = order.to_vec();
    for e in &entries {
        if !want.contains(&e.xref) {
            want.push(e.xref);
        }
    }
    let mut page_obj = doc.find_page(page as i32)?;
    let mut annots = get(&page_obj, "Annots").ok_or_else(|| bad("the page has no annotations"))?;
    let n = annots.len()? as i32;
    let slots: Vec<i32> = (0..n)
        .filter(|i| {
            annots.get_array(*i).ok().flatten().and_then(|v| v.as_indirect().ok()).is_some_and(|x| entries.iter().any(|e| e.xref == x))
        })
        .collect();
    for (slot, x) in slots.into_iter().zip(&want) {
        annots.array_put(slot, doc.new_indirect(*x, 0)?)?;
    }
    match tabs {
        Some(t) => put_name(&mut page_obj, "Tabs", t)?,
        None => page_obj.dict_delete("Tabs")?,
    }
    Ok(())
}

/// Row order (top to bottom, left to right within a row) or column order (left to right,
/// top to bottom within a column) of the page's widgets, by their rectangles.
pub fn arrange(doc: &mut PdfDocument, page: u32, by_row: bool) -> Result<()> {
    refuse_if_certified(doc)?;
    let entries = scan_page(doc, page)?;
    let mut items: Vec<(i32, [f32; 4])> = entries.iter().map(|e| (e.xref, e.rect)).collect();
    let mid = |r: &[f32; 4]| ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    // Bands of 8 points keep a slightly ragged row or column together.
    let band = |v: f32| (v / 8.0).round() as i32;
    if by_row {
        items.sort_by(|a, b| (band(mid(&a.1).1), mid(&a.1).0 as i32).cmp(&(band(mid(&b.1).1), mid(&b.1).0 as i32)));
    } else {
        items.sort_by(|a, b| (band(mid(&a.1).0), mid(&a.1).1 as i32).cmp(&(band(mid(&b.1).0), mid(&b.1).1 as i32)));
    }
    let order: Vec<i32> = items.into_iter().map(|i| i.0).collect();
    set_tab_order(doc, page, &order, Some(if by_row { "R" } else { "C" }))
}

// ---- detection ---------------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub kind: FieldKind,
    pub name: String,
    pub rect: [f32; 4],
}

fn words_of(p: &PdfPage, h: f64) -> Result<Vec<detect::Word>> {
    let tp = p.to_text_page(TextPageFlags::empty())?;
    let mut out = Vec::new();
    for block in tp.blocks() {
        for line in block.lines() {
            let mut cur = String::new();
            let mut r = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            let mut flush = |cur: &mut String, r: &mut [f64; 4]| {
                if !cur.is_empty() {
                    // Flipped so y points up, as `detect` expects.
                    out.push(detect::Word { text: std::mem::take(cur), rect: [r[0], h - r[3], r[2], h - r[1]] });
                }
                *r = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
            };
            for ch in line.chars() {
                match ch.char() {
                    Some(c) if !c.is_whitespace() => {
                        let q = ch.quad();
                        let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
                        let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
                        r[0] = r[0].min(f64::from(xs.iter().copied().fold(f32::MAX, f32::min)));
                        r[2] = r[2].max(f64::from(xs.iter().copied().fold(f32::MIN, f32::max)));
                        r[1] = r[1].min(f64::from(ys.iter().copied().fold(f32::MAX, f32::min)));
                        r[3] = r[3].max(f64::from(ys.iter().copied().fold(f32::MIN, f32::max)));
                        cur.push(c);
                    }
                    _ => flush(&mut cur, &mut r),
                }
            }
            flush(&mut cur, &mut r);
        }
    }
    Ok(out)
}

fn shapes_of(p: &PdfPage, h: f64) -> Result<detect::Shapes> {
    use mupdf::drawing::DrawingItem;
    let mut s = detect::Shapes::default();
    // The page's own (rotated) coordinates, as the text above.
    let plain: &mupdf::Page = p;
    for d in plain.drawings()? {
        for it in &d.items {
            match it {
                DrawingItem::Rect { rect, .. } => s.add_rect([f64::from(rect.x0), h - f64::from(rect.y1), f64::from(rect.x1), h - f64::from(rect.y0)]),
                DrawingItem::Line(a, b) => s.add_segment([f64::from(a.x), h - f64::from(a.y)], [f64::from(b.x), h - f64::from(b.y)]),
                DrawingItem::Quad(q) => {
                    let xs = [q.ul.x, q.ur.x, q.ll.x, q.lr.x];
                    let ys = [q.ul.y, q.ur.y, q.ll.y, q.lr.y];
                    let (x0, x1) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
                    let (y0, y1) = (ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max));
                    s.add_rect([f64::from(x0), h - f64::from(y1), f64::from(x1), h - f64::from(y0)]);
                }
                DrawingItem::Curve(..) => {}
            }
        }
    }
    s.close_boxes();
    Ok(s)
}

/// Where a printed form expects answers on `page`. Nothing is changed.
pub fn detect_fields(doc: &PdfDocument, page: u32) -> Result<Vec<Detected>> {
    let p = doc.load_pdf_page(page as i32)?;
    let b = p.bounds()?;
    let h = f64::from(b.y1 - b.y0);
    let words = words_of(&p, h)?;
    let shapes = shapes_of(&p, h)?;
    let entries = scan(doc)?;
    let taken: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    let existing: Vec<[f64; 4]> = entries
        .iter()
        .filter(|e| e.page == page)
        .map(|e| [f64::from(e.rect[0]), h - f64::from(e.rect[3]), f64::from(e.rect[2]), h - f64::from(e.rect[1])])
        .collect();
    Ok(detect::detect(&words, &shapes, &existing, &taken)
        .into_iter()
        .map(|c| Detected {
            kind: match c.kind {
                DetectKind::CheckBox => FieldKind::Checkbox,
                DetectKind::Signature => FieldKind::Signature,
                DetectKind::Text => FieldKind::Text,
            },
            name: c.name,
            rect: [c.rect[0] as f32, (h - c.rect[3]) as f32, c.rect[2] as f32, (h - c.rect[1]) as f32],
        })
        .collect())
}

// ---- engine wiring: one undo step each ---------------------------------------------------------

pub(crate) fn noun(k: FieldKind) -> &'static str {
    match k {
        FieldKind::Text => "text field",
        FieldKind::Checkbox => "check box",
        FieldKind::Radio => "radio button",
        FieldKind::Combo => "drop-down",
        FieldKind::List => "list box",
        FieldKind::Button => "button",
        FieldKind::Signature => "signature field",
    }
}

pub fn add_one(eng: &Engine, doc: DocId, page: u32, what: &NewField, source: EditSource) -> Result<(EditState, i32)> {
    let mut xref = 0;
    let state = eng.apply_custom(doc, format!("Add {} on p. {}", noun(what.kind), page + 1), source, false, |d| {
        xref = add(d, page, what)?;
        Ok(vec![page])
    })?;
    Ok((state, xref))
}

/// Several fields at once (what detection found): one undo step.
pub fn add_many(eng: &Engine, doc: DocId, page: u32, items: &[NewField], source: EditSource) -> Result<EditState> {
    let n = items.len();
    eng.apply_custom(doc, format!("Add {n} field{} on p. {}", if n == 1 { "" } else { "s" }, page + 1), source, false, |d| {
        for it in items {
            add(d, page, it)?;
        }
        Ok(vec![page])
    })
}

pub fn move_one(eng: &Engine, doc: DocId, page: u32, xref: i32, rect: [f32; 4], source: EditSource) -> Result<EditState> {
    eng.apply_custom(doc, format!("Move or resize field on p. {}", page + 1), source, false, |d| {
        move_to(d, page, xref, rect)?;
        Ok(vec![page])
    })
}

pub fn edit_one(eng: &Engine, doc: DocId, page: u32, xref: i32, patch: &FieldPatch, source: EditSource) -> Result<EditState> {
    eng.apply_custom(doc, format!("Change field properties on p. {}", page + 1), source, false, |d| {
        edit(d, page, xref, patch)?;
        Ok(vec![page])
    })
}

pub fn delete_many(eng: &Engine, doc: DocId, items: &[(u32, i32)], source: EditSource) -> Result<EditState> {
    let items = items.to_vec();
    let mut pages: Vec<u32> = items.iter().map(|i| i.0).collect();
    pages.sort_unstable();
    pages.dedup();
    let n = items.len();
    eng.apply_custom(doc, format!("Delete {n} field{}", if n == 1 { "" } else { "s" }), source, false, |d| {
        for &pg in &pages {
            let xs: Vec<i32> = items.iter().filter(|i| i.0 == pg).map(|i| i.1).collect();
            delete(d, pg, &xs)?;
        }
        Ok(pages.clone())
    })
}

/// Writes `/AcroForm /CO`: the calculated fields (`node` of each) in the order they run. Every field with a calculate script
/// that is not listed follows, in its current order. Returns false when this is the order already written.
pub fn set_calc_order(doc: &mut PdfDocument, nodes: &[i32]) -> Result<bool> {
    refuse_if_certified(doc)?;
    let entries = scan(doc)?;
    let current = calc_order_of(doc, &entries);
    let mut seen: Vec<i32> = Vec::with_capacity(nodes.len());
    for n in nodes {
        if !entries.iter().any(|e| e.node == *n && aa_js(e, "C").is_some()) {
            return Err(bad(format!("field {n} has no calculation")));
        }
        if seen.contains(n) {
            return Err(bad(format!("field {n} is listed twice in the calculation order")));
        }
        seen.push(*n);
    }
    let mut want = seen.clone();
    want.extend(current.iter().copied().filter(|n| !seen.contains(n)));
    if want == written_calc_order(doc) {
        return Ok(false);
    }
    let mut form = acro_form(doc)?;
    let mut co = doc.new_array()?;
    for n in &want {
        co.array_push(doc.new_indirect(*n, 0)?)?;
    }
    form.dict_put("CO", co)?;
    Ok(true)
}

pub fn calc_order_one(eng: &Engine, doc: DocId, nodes: &[i32], source: EditSource) -> Result<EditState> {
    // The same order is not a step in the history.
    let nodes_v = nodes.to_vec();
    if !eng.read_custom(doc, |d| {
        let entries = scan(d)?;
        let mut want = nodes_v.clone();
        want.extend(calc_order_of(d, &entries).into_iter().filter(|n| !nodes_v.contains(n)));
        Ok(want != written_calc_order(d))
    })? {
        return eng.edit_state(doc);
    }
    eng.apply_custom(doc, "Change calculation order".into(), source, false, |d| {
        set_calc_order(d, nodes)?;
        Ok(vec![])
    })
}

pub fn tab_order_one(eng: &Engine, doc: DocId, page: u32, order: &[i32], source: EditSource) -> Result<EditState> {
    eng.apply_custom(doc, format!("Set tab order on p. {}", page + 1), source, false, |d| {
        set_tab_order(d, page, order, None)?;
        Ok(vec![page])
    })
}

pub fn arrange_one(eng: &Engine, doc: DocId, page: u32, by_row: bool, source: EditSource) -> Result<EditState> {
    let label = if by_row { "Use row order" } else { "Use column order" };
    eng.apply_custom(doc, format!("{label} on p. {}", page + 1), source, false, |d| {
        arrange(d, page, by_row)?;
        Ok(vec![page])
    })
}

pub fn detect_one(eng: &Engine, doc: DocId, page: u32) -> Result<Vec<Detected>> {
    eng.read_custom(doc, |d| detect_fields(d, page))
}
