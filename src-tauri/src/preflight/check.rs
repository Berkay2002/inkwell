// Adapted from pdfcraft (MIT OR Apache-2.0), crates/preflight/src/lib.rs @ b8ddf71
//! The rules: one pass over the pages (content, resources, annotations), one over the catalog and
//! the file's objects, then the report and the conversion plan.

use super::fonts::{self, FontPlan};
use super::xmp;
use super::{Claim, Family, Finding, FixGroup, Plan, PlanGroup, Profile, Remaining, Report, Result, Rule, RuleResult, Status, bad};
use crate::a11y::content::{self, Operand};
use crate::a11y::locate;
use crate::a11y::tree::{name, text, xref_of};
use crate::a11y::{self, Progress};
use mupdf::pdf::{PdfDocument, PdfObject};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

/// Findings beyond this many per rule are counted but not listed.
const MAX_FINDINGS: usize = 200;
const MAX_FORM_DEPTH: usize = 12;
/// Marks an embedded-PDF finding as a manual check rather than a failure.
pub(crate) const NOTE_MANUAL: &str = "manual";

pub(crate) const FORBIDDEN_ACTIONS: &[&str] = &["JavaScript", "Launch", "Sound", "Movie", "ResetForm", "ImportData", "Hide", "SetOCGState", "Rendition", "Trans", "GoTo3DView"];
pub(crate) const NAMED_OK: &[&str] = &["NextPage", "PrevPage", "FirstPage", "LastPage"];
/// Annotation subtypes ISO 32000-1 defines.
const ANNOT_32000: &[&str] = &[
    "Text", "Link", "FreeText", "Line", "Square", "Circle", "Polygon", "PolyLine", "Highlight", "Underline", "Squiggly", "StrikeOut", "Stamp", "Caret", "Ink", "Popup",
    "FileAttachment", "Sound", "Movie", "Widget", "Screen", "PrinterMark", "TrapNet", "Watermark", "3D", "Redact",
];
/// ... and PDF 1.4 (what PDF/A-1 is built on).
const ANNOT_14: &[&str] = &[
    "Text", "Link", "FreeText", "Line", "Square", "Circle", "Polygon", "PolyLine", "Highlight", "Underline", "Squiggly", "StrikeOut", "Stamp", "Caret", "Ink", "Popup",
    "FileAttachment", "Sound", "Movie", "Widget", "PrinterMark", "TrapNet",
];

fn val_name(o: &PdfObject) -> Option<String> {
    match o.is_name() {
        Ok(true) => o.as_name().ok().map(|n| String::from_utf8_lossy(&n).into_owned()),
        _ => None,
    }
}

pub(crate) fn dict_of(o: &PdfObject, key: &str) -> Option<PdfObject> {
    o.get_dict(key).ok().flatten().filter(|v| v.is_dict().unwrap_or(false))
}

fn number(o: &PdfObject) -> Option<f32> {
    if o.is_number().unwrap_or(false) { o.as_float().ok() } else { None }
}

fn is_true(o: &PdfObject, key: &str) -> bool {
    o.get_dict(key).ok().flatten().is_some_and(|v| v.is_bool().unwrap_or(false) && v.as_bool().unwrap_or(false))
}

/// The filters of a stream, in order.
pub(crate) fn filters(s: &PdfObject) -> Vec<String> {
    let Ok(Some(f)) = s.get_dict("Filter") else { return Vec::new() };
    if f.is_name().unwrap_or(false) {
        return val_name(&f).into_iter().collect();
    }
    let mut out = Vec::new();
    if f.is_array().unwrap_or(false) {
        if let Ok(it) = f.array_iter() {
            for x in it.flatten() {
                out.extend(val_name(&x));
            }
        }
    }
    out
}

fn res_lookup(res: Option<&PdfObject>, cat: &str, key: &[u8]) -> Option<PdfObject> {
    let d = dict_of(res?, cat)?;
    d.get_dict(String::from_utf8_lossy(key).into_owned()).ok().flatten()
}

fn device_index(n: &str) -> Option<usize> {
    match n {
        "DeviceGray" | "G" => Some(0),
        "DeviceRGB" | "RGB" => Some(1),
        "DeviceCMYK" | "CMYK" => Some(2),
        _ => None,
    }
}

/// The device space a colour space object ultimately paints in (an Indexed space's base too).
fn device_of(cs: &PdfObject) -> Option<usize> {
    if let Some(n) = val_name(cs) {
        return device_index(&n);
    }
    if cs.is_array().unwrap_or(false) {
        let first = cs.get_array(0).ok().flatten().and_then(|f| val_name(&f))?;
        if first == "Indexed" || first == "I" {
            return cs.get_array(1).ok().flatten().and_then(|b| device_of(&b));
        }
    }
    None
}

/// What a conversion needs from the pages: the objects the rules would judge.
#[derive(Default)]
pub(crate) struct Collected {
    pub fonts: Vec<PdfObject>,
    pub images: Vec<PdfObject>,
    pub gstates: Vec<PdfObject>,
    pub forms: Vec<PdfObject>,
}

/// An output intent as the rules read it.
struct Intent {
    subtype: String,
    /// The profile is a well-formed ICC profile.
    valid: bool,
    /// Colour components of the destination profile (1, 3, 4).
    comps: Option<i64>,
}

struct Ctx<'a> {
    doc: &'a PdfDocument,
    profile: Profile,
    fam: Family,
    found: HashMap<Rule, Vec<Finding>>,
    seen_fonts: HashSet<i32>,
    seen_font_names: HashSet<String>,
    seen_forms: HashSet<i32>,
    seen_gs: HashSet<i32>,
    seen_images: HashSet<i32>,
    seen_patterns: HashSet<i32>,
    /// First page that uses DeviceGray, DeviceRGB, DeviceCMYK.
    device: [Option<u32>; 3],
    default_cs: [bool; 3],
    /// Fonts that cannot be embedded.
    blocked: usize,
    /// Object numbers the catalog's /AF array lists (PDF/A-3 associates embedded files there).
    catalog_af: Option<HashSet<i32>>,
    /// Collect the fonts in use instead of judging them (the conversion's survey).
    collect: Option<Collected>,
}

impl Ctx<'_> {
    fn add(&mut self, rule: Rule, f: Finding) {
        if self.collect.is_none() && rule.applies(self.profile) {
            self.found.entry(rule).or_default().push(f);
        }
    }

    fn on(&self, rule: Rule) -> bool {
        rule.applies(self.profile)
    }

    fn mark_device(&mut self, page: u32, i: usize) {
        if self.device[i].is_none() {
            self.device[i] = Some(page);
        }
    }

    fn defaults(&mut self, res: Option<&PdfObject>) {
        let Some(cs) = res.and_then(|r| dict_of(r, "ColorSpace")) else { return };
        for (i, k) in ["DefaultGray", "DefaultRGB", "DefaultCMYK"].iter().enumerate() {
            if cs.get_dict(*k).ok().flatten().is_some() {
                self.default_cs[i] = true;
            }
        }
    }

    // ---- content ---------------------------------------------------------------------------------

    fn scan_content(&mut self, page: u32, data: &[u8], res: Option<&PdfObject>, depth: usize) -> Result<()> {
        self.defaults(res);
        for op in content::parse(data) {
            match op.name.as_slice() {
                b"g" | b"G" => self.mark_device(page, 0),
                b"rg" | b"RG" => self.mark_device(page, 1),
                b"k" | b"K" => self.mark_device(page, 2),
                b"cs" | b"CS" => {
                    if let Some(n) = op.operands.first().and_then(Operand::name) {
                        let n = String::from_utf8_lossy(n).into_owned();
                        let dev = device_index(&n).or_else(|| res_lookup(res, "ColorSpace", n.as_bytes()).and_then(|c| device_of(&c)));
                        if let Some(i) = dev {
                            self.mark_device(page, i);
                        }
                    }
                }
                b"Tf" => {
                    if let Some(f) = op.operands.first().and_then(Operand::name).and_then(|n| res_lookup(res, "Font", n)) {
                        self.font(page, &f)?;
                    }
                }
                b"gs" => {
                    if let Some(g) = op.operands.first().and_then(Operand::name).and_then(|n| res_lookup(res, "ExtGState", n)) {
                        self.gstate(page, &g)?;
                    }
                }
                b"Do" => {
                    if let Some(x) = op.operands.first().and_then(Operand::name).and_then(|n| res_lookup(res, "XObject", n)) {
                        self.xobject(page, &x, res, depth)?;
                    }
                }
                b"sh" => {
                    if let Some(s) = op.operands.first().and_then(Operand::name).and_then(|n| res_lookup(res, "Shading", n)) {
                        self.shading(page, &s);
                    }
                }
                b"scn" | b"SCN" => {
                    if let Some(n) = op.operands.last().and_then(Operand::name) {
                        if let Some(p) = res_lookup(res, "Pattern", n) {
                            self.pattern(page, &p, res, depth)?;
                        }
                    }
                }
                b"BI" => {
                    let raw = &data[op.start..op.end.min(data.len())];
                    let head = raw.iter().position(|_| true).map(|_| String::from_utf8_lossy(&raw[..raw.len().min(400)]).into_owned()).unwrap_or_default();
                    let head = head.split("ID").next().unwrap_or("").to_string();
                    for (needle, i) in [("/RGB", 1), ("/DeviceRGB", 1), ("/CMYK", 2), ("/DeviceCMYK", 2), ("/G ", 0), ("/DeviceGray", 0)] {
                        if head.contains(needle) && (head.contains("/CS") || head.contains("/ColorSpace")) {
                            self.mark_device(page, i);
                        }
                    }
                    if head.contains("/Interpolate true") || head.contains("/I true") {
                        self.add(
                            Rule::ImageInterpolate,
                            Finding { page: Some(page), object: Some("Inline image".into()), message: format!("An inline image on page {} asks to be interpolated", page + 1), fixable: false, note: Some("an inline image rewritten as an image XObject".into()), ..Finding::default() },
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn form_content(&mut self, page: u32, x: &PdfObject, parent: Option<&PdfObject>, depth: usize) -> Result<()> {
        let n = xref_of(x)?;
        if n != 0 && !self.seen_forms.insert(n) {
            return Ok(());
        }
        if depth >= MAX_FORM_DEPTH {
            return Ok(());
        }
        if let Some(c) = &mut self.collect {
            c.forms.push(x.share());
        }
        if self.fam == Family::A1 {
            if let Some(g) = dict_of(x, "Group") {
                if name(&g, "S")?.as_deref() == Some("Transparency") {
                    self.add(
                        Rule::Transparency,
                        Finding {
                            page: Some(page),
                            object: Some("Transparency group".into()),
                            xref: (n != 0).then_some(n),
                            message: format!("A form on page {} is a transparency group", page + 1),
                            fixable: true,
                            ..Finding::default()
                        },
                    );
                }
            }
        }
        let inner = dict_of(x, "Resources");
        let res = inner.as_ref().or(parent);
        let data = x.read_stream().unwrap_or_default();
        self.scan_content(page, &data, res, depth + 1)?;
        self.resources_only(page, res, depth)
    }

    /// Pattern and shading resources are checked where they are declared: a pattern's colour space
    /// is only reached by painting with it.
    fn resources_only(&mut self, _page: u32, _res: Option<&PdfObject>, _depth: usize) -> Result<()> {
        Ok(())
    }

    fn xobject(&mut self, page: u32, x: &PdfObject, res: Option<&PdfObject>, depth: usize) -> Result<()> {
        if !x.is_stream()? {
            return Ok(());
        }
        match name(x, "Subtype")?.as_deref() {
            Some("Form") => self.form_content(page, x, res, depth),
            Some("Image") => self.image(page, x),
            Some("PS") => {
                self.add(
                    Rule::PsXObject,
                    Finding { page: Some(page), object: Some("PostScript XObject".into()), xref: Some(xref_of(x)?).filter(|n| *n != 0), message: format!("A PostScript XObject on page {}", page + 1), note: Some("PostScript XObjects removed in the source".into()), ..Finding::default() },
                );
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn image(&mut self, page: u32, x: &PdfObject) -> Result<()> {
        let n = xref_of(x)?;
        if n != 0 && !self.seen_images.insert(n) {
            return Ok(());
        }
        let at = |what: String, fixable: bool, note: Option<&str>| Finding {
            page: Some(page),
            object: Some("Image".into()),
            xref: (n != 0).then_some(n),
            message: what,
            fixable,
            note: note.map(str::to_string),
            ..Finding::default()
        };
        if let Some(cs) = x.get_dict("ColorSpace")? {
            if let Some(i) = device_of(&cs) {
                self.mark_device(page, i);
            }
        }
        if let Some(c) = &mut self.collect {
            c.images.push(x.share());
            return Ok(());
        }
        if is_true(x, "Interpolate") {
            self.add(Rule::ImageInterpolate, at(format!("An image on page {} asks to be interpolated", page + 1), true, None));
        }
        if x.get_dict("Alternates")?.is_some() {
            self.add(Rule::ImageAlternates, at(format!("An image on page {} has alternate images", page + 1), true, None));
        }
        if x.get_dict("OPI")?.is_some() {
            self.add(Rule::ImageAlternates, at(format!("An image on page {} has OPI information", page + 1), true, None));
        }
        if filters(x).iter().any(|f| f == "JPXDecode") {
            self.add(Rule::ImageJpx, at(format!("An image on page {} is JPEG 2000", page + 1), false, Some("images re-encoded without JPEG 2000")));
        }
        if self.fam == Family::A1 && (x.get_dict("SMask")?.is_some() || x.get_dict("SMaskInData")?.and_then(|v| v.as_int().ok()).unwrap_or(0) > 0) {
            self.add(Rule::Transparency, at(format!("An image on page {} has a soft mask", page + 1), true, None));
        }
        Ok(())
    }

    fn gstate(&mut self, page: u32, g: &PdfObject) -> Result<()> {
        let n = xref_of(g)?;
        if n != 0 && !self.seen_gs.insert(n) {
            return Ok(());
        }
        if let Some(c) = &mut self.collect {
            c.gstates.push(g.share());
            return Ok(());
        }
        let at = |what: String, group: bool| Finding {
            page: Some(page),
            object: Some("Graphics state".into()),
            xref: (n != 0).then_some(n),
            message: what,
            fixable: group,
            ..Finding::default()
        };
        if self.fam == Family::A1 {
            for k in ["CA", "ca"] {
                if let Some(v) = g.get_dict(k)?.as_ref().and_then(number) {
                    if v < 1.0 {
                        self.add(Rule::Transparency, at(format!("Page {} uses opacity {v} ({k})", page + 1), true));
                    }
                }
            }
            if let Some(s) = g.get_dict("SMask")? {
                if val_name(&s).as_deref() != Some("None") {
                    self.add(Rule::Transparency, at(format!("Page {} uses a soft mask", page + 1), true));
                }
            }
            if let Some(b) = g.get_dict("BM")? {
                let first = if b.is_array().unwrap_or(false) { b.get_array(0)?.and_then(|f| val_name(&f)) } else { val_name(&b) };
                if !matches!(first.as_deref(), None | Some("Normal") | Some("Compatible")) {
                    self.add(Rule::Transparency, at(format!("Page {} uses the blend mode {}", page + 1, first.unwrap_or_default()), true));
                }
            }
        }
        if let Some(t) = g.get_dict("TR")? {
            if val_name(&t).as_deref() != Some("Identity") {
                self.add(Rule::TransferFunctions, at(format!("Page {} sets a transfer function (TR)", page + 1), true));
            }
        }
        if let Some(t) = g.get_dict("TR2")? {
            if !matches!(val_name(&t).as_deref(), Some("Default") | Some("Identity")) {
                self.add(Rule::TransferFunctions, at(format!("Page {} sets a transfer function (TR2)", page + 1), true));
            }
        }
        if g.get_dict("HT")?.is_some_and(|h| val_name(&h).as_deref() != Some("Default")) && self.fam == Family::A1 {
            // A custom halftone: not repaired, and not a rule this report lists separately.
        }
        Ok(())
    }

    fn shading(&mut self, page: u32, s: &PdfObject) {
        if let Ok(Some(cs)) = s.get_dict("ColorSpace") {
            if let Some(i) = device_of(&cs) {
                self.mark_device(page, i);
            }
        }
    }

    fn pattern(&mut self, page: u32, p: &PdfObject, res: Option<&PdfObject>, depth: usize) -> Result<()> {
        let n = xref_of(p)?;
        if n != 0 && !self.seen_patterns.insert(n) {
            return Ok(());
        }
        if let Some(sh) = p.get_dict("Shading")? {
            self.shading(page, &sh);
        }
        if p.is_stream()? && depth < MAX_FORM_DEPTH {
            let inner = dict_of(p, "Resources");
            let r = inner.as_ref().or(res);
            let data = p.read_stream().unwrap_or_default();
            self.scan_content(page, &data, r, depth + 1)?;
        }
        Ok(())
    }

    fn font(&mut self, page: u32, f: &PdfObject) -> Result<()> {
        let x = xref_of(f)?;
        let base = name(f, "BaseFont")?.unwrap_or_else(|| "(unnamed)".into());
        if x != 0 {
            if !self.seen_fonts.insert(x) {
                return Ok(());
            }
        } else if !self.seen_font_names.insert(base.clone()) {
            return Ok(());
        }
        if let Some(c) = &mut self.collect {
            c.fonts.push(f.share());
            return Ok(());
        }
        let display = base.split_once('+').filter(|(p, _)| p.len() == 6).map(|(_, r)| r.to_string()).unwrap_or_else(|| base.clone());
        if self.on(Rule::FontsEmbedded) {
            match fonts::classify(f)? {
                FontPlan::Embedded => {}
                plan => {
                    let (fixable, note, how) = match &plan {
                        FontPlan::Builtin { program } => (true, None, format!("; it can be embedded as the built-in {program}")),
                        FontPlan::Installed { ps_name, .. } => (true, None, format!("; it can be embedded from the installed {ps_name}")),
                        FontPlan::Blocked { reason, note } => {
                            self.blocked += 1;
                            (false, Some(note.to_string()), format!("; {reason}"))
                        }
                        FontPlan::Embedded => unreachable!(),
                    };
                    self.add(
                        Rule::FontsEmbedded,
                        Finding {
                            page: Some(page),
                            object: Some(format!("Font {display}")),
                            xref: (x != 0).then_some(x),
                            message: format!("Font {display} on page {} isn't embedded{how}", page + 1),
                            fixable,
                            note,
                            ..Finding::default()
                        },
                    );
                }
            }
        }
        if self.on(Rule::FontsCidToGid) && name(f, "Subtype")?.as_deref() == Some("Type0") {
            if let Some(d) = fonts::descendant(f)? {
                let embedded = dict_of(&d, "FontDescriptor").is_some_and(|fd| fd.get_dict("FontFile2").ok().flatten().is_some());
                if name(&d, "Subtype")?.as_deref() == Some("CIDFontType2") && embedded && d.get_dict("CIDToGIDMap")?.is_none() {
                    self.add(
                        Rule::FontsCidToGid,
                        Finding {
                            page: Some(page),
                            object: Some(format!("Font {display}")),
                            xref: Some(xref_of(&d)?).filter(|n| *n != 0).or((x != 0).then_some(x)),
                            message: format!("Font {display} on page {} has no CIDToGIDMap", page + 1),
                            fixable: true,
                            ..Finding::default()
                        },
                    );
                }
            }
        }
        if self.on(Rule::FontsUnicode) && name(f, "Subtype")?.as_deref() != Some("Type3") && !a11y::maps_to_unicode(f)? {
            self.add(
                Rule::FontsUnicode,
                Finding {
                    page: Some(page),
                    object: Some(format!("Font {display}")),
                    xref: (x != 0).then_some(x),
                    message: format!("Font {display} on page {} doesn't map its characters to Unicode", page + 1),
                    note: Some("the text re-created with a Unicode mapping (or OCR)".into()),
                    ..Finding::default()
                },
            );
        }
        Ok(())
    }

    // ---- actions ---------------------------------------------------------------------------------

    /// Forbidden actions in `act` and its Next chain.
    fn action(&mut self, page: Option<u32>, owner: &str, xref: i32, rect: Option<locate::Box4>, act: &PdfObject) -> Result<()> {
        let mut cur = Some(act.share());
        for _ in 0..8 {
            let Some(a) = cur.take() else { break };
            if !a.is_dict()? {
                break;
            }
            let s = name(&a, "S")?.unwrap_or_default();
            let named_bad = s == "Named" && name(&a, "N")?.is_some_and(|n| !NAMED_OK.contains(&n.as_str()));
            if FORBIDDEN_ACTIONS.contains(&s.as_str()) || named_bad {
                let what = if named_bad { format!("Named ({})", name(&a, "N")?.unwrap_or_default()) } else { s.clone() };
                let at = page.map(|p| format!(" on page {}", p + 1)).unwrap_or_default();
                self.add(
                    Rule::ActionsForbidden,
                    Finding { page, object: Some(owner.to_string()), xref: (xref != 0).then_some(xref), rect, message: format!("{owner}{at} has a {what} action", ), fixable: true, ..Finding::default() },
                );
            }
            cur = match a.get_dict("Next")? {
                Some(n) if n.is_array().unwrap_or(false) => {
                    for x in n.array_iter()?.flatten() {
                        self.action(page, owner, xref, rect, &x)?;
                    }
                    None
                }
                Some(n) => Some(n),
                None => None,
            };
        }
        Ok(())
    }

    fn owner_actions(&mut self, page: Option<u32>, owner: &str, o: &PdfObject, rect: Option<locate::Box4>) -> Result<()> {
        let x = xref_of(o)?;
        if let Some(a) = o.get_dict("A")? {
            self.action(page, owner, x, rect, &a)?;
        }
        if let Some(aa) = dict_of(o, "AA") {
            let at = page.map(|p| format!(" on page {}", p + 1)).unwrap_or_default();
            self.add(
                Rule::ActionsAdditional,
                Finding { page, object: Some(owner.to_string()), xref: (x != 0).then_some(x), rect, message: format!("{owner}{at} has additional actions (AA)"), fixable: true, ..Finding::default() },
            );
            for i in 0..aa.dict_len()? {
                if let Some(a) = aa.get_dict_val(i as i32)? {
                    self.action(page, owner, x, rect, &a)?;
                }
            }
        }
        Ok(())
    }

    // ---- pages -----------------------------------------------------------------------------------

    fn page(&mut self, p: u32, page: &PdfObject) -> Result<()> {
        let res = page.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
        let data = a11y::page_content(page)?;
        self.scan_content(p, &data, res.as_ref(), 0)?;
        if self.fam == Family::A1 {
            if let Some(g) = dict_of(page, "Group") {
                if name(&g, "S")?.as_deref() == Some("Transparency") {
                    self.add(
                        Rule::Transparency,
                        Finding {
                            page: Some(p),
                            object: Some("Page group".into()),
                            xref: Some(xref_of(page)?).filter(|n| *n != 0),
                            message: format!("Page {} is a transparency group", p + 1),
                            fixable: true,
                            ..Finding::default()
                        },
                    );
                }
            }
        }
        if self.on(Rule::PageBoxes) && page.get_dict("TrimBox")?.is_none() && page.get_dict("ArtBox")?.is_none() {
            self.add(
                Rule::PageBoxes,
                Finding {
                    page: Some(p),
                    object: Some("Page".into()),
                    xref: Some(xref_of(page)?).filter(|n| *n != 0),
                    message: format!("Page {} has neither a TrimBox nor an ArtBox", p + 1),
                    fixable: true,
                    ..Finding::default()
                },
            );
        }
        if self.on(Rule::ActionsAdditional) || self.on(Rule::ActionsForbidden) {
            if dict_of(page, "AA").is_some() {
                self.owner_actions(Some(p), "Page", page, None)?;
            }
        }
        for (x, a) in a11y::annots(page)? {
            self.annotation(p, page, x, &a)?;
        }
        // Popups carry flags too, but are exempt from them; nothing else to read.
        Ok(())
    }

    fn annotation(&mut self, p: u32, page: &PdfObject, x: i32, a: &PdfObject) -> Result<()> {
        let sub = name(a, "Subtype")?.unwrap_or_default();
        let rect = locate::annot_box(page, a)?;
        let owner = format!("{sub} annotation");
        let at = |msg: String, fixable: bool, note: Option<&str>| Finding {
            page: Some(p),
            object: Some(owner.clone()),
            xref: (x != 0).then_some(x),
            rect,
            message: msg,
            fixable,
            note: note.map(str::to_string),
            ..Finding::default()
        };
        if annotation_forbidden(self.fam, &sub) {
            self.add(Rule::AnnotationTypes, at(format!("A {sub} annotation on page {} isn't allowed", p + 1), true, None));
            return Ok(());
        }
        let f = a.get_dict("F")?.and_then(|v| v.as_int().ok()).unwrap_or(0);
        if f & 4 == 0 || f & (1 | 2 | 32) != 0 {
            let why = if f & 4 == 0 { "isn't set to print" } else { "is hidden" };
            self.add(Rule::AnnotationFlags, at(format!("A {sub} annotation on page {} {why}", p + 1), true, None));
        }
        let ap = dict_of(a, "AP");
        let has_n = ap.as_ref().is_some_and(|d| d.get_dict("N").ok().flatten().is_some());
        if !has_n && !matches!(sub.as_str(), "Link" | "Popup") {
            let zero = rect.is_some_and(|r| (r[2] - r[0]).abs() < 1e-3 && (r[3] - r[1]).abs() < 1e-3);
            if !zero {
                let widget = sub == "Widget";
                let need = self.doc.catalog().ok().and_then(|c| dict_of(&c, "AcroForm")).is_some_and(|f| is_true(&f, "NeedAppearances"));
                let fixable = widget && need;
                self.add(
                    Rule::AnnotationAppearance,
                    at(format!("A {sub} annotation on page {} has no appearance stream", p + 1), fixable, (!fixable).then_some("an appearance stream from the program that made them")),
                );
            }
        }
        // Appearance streams are content: their fonts, colours and graphics states count.
        if let Some(ap) = ap {
            for k in ["N", "R", "D"] {
                let Some(e) = ap.get_dict(k)? else { continue };
                if e.is_stream()? {
                    self.form_content(p, &e, None, 0)?;
                } else if e.is_dict()? {
                    for i in 0..e.dict_len()? {
                        if let Some(s) = e.get_dict_val(i as i32)? {
                            if s.is_stream()? {
                                self.form_content(p, &s, None, 0)?;
                            }
                        }
                    }
                }
            }
        }
        self.owner_actions(Some(p), &owner, a, rect)?;
        // A file attachment holds an embedded file.
        if sub == "FileAttachment" {
            if let Some(fs) = a.get_dict("FS")? {
                self.filespec(Some(p), &fs, rect)?;
            }
        }
        Ok(())
    }

    // ---- embedded files --------------------------------------------------------------------------

    fn filespec(&mut self, page: Option<u32>, fs: &PdfObject, rect: Option<locate::Box4>) -> Result<()> {
        if !fs.is_dict()? || dict_of(fs, "EF").is_none() {
            return Ok(());
        }
        let x = xref_of(fs)?;
        let fname = text(fs, "UF")?.or(text(fs, "F")?).unwrap_or_else(|| "(unnamed)".into());
        let base = |msg: String, fixable: bool, note: Option<&str>| Finding {
            page,
            object: Some(format!("Embedded file {fname}")),
            xref: (x != 0).then_some(x),
            rect,
            message: msg,
            fixable,
            note: note.map(str::to_string),
            ..Finding::default()
        };
        match self.fam {
            Family::A1 => self.add(Rule::EmbeddedFiles, base(format!("The embedded file {fname} isn't allowed in PDF/A-1"), true, None)),
            _ if self.profile == Profile::Pdfa3b => {
                let af = name(fs, "AFRelationship")?;
                if af.is_none() {
                    self.add(Rule::EmbeddedFiles, base(format!("The embedded file {fname} has no AFRelationship"), true, None));
                }
                // The file's media type (a MIME /Subtype on its stream) and its modification date.
                let stream = embedded_stream(fs);
                if let Some(s) = &stream {
                    if name(s, "Subtype")?.is_none() {
                        self.add(Rule::EmbeddedFiles, base(format!("The embedded file {fname} has no media type (/Subtype on its stream)"), true, None));
                    }
                    let dated = dict_of(s, "Params").is_some_and(|p| p.get_dict("ModDate").ok().flatten().is_some());
                    if !dated {
                        self.add(Rule::EmbeddedFiles, base(format!("The embedded file {fname} has no modification date (/Params /ModDate)"), false, Some(NOTE_MANUAL)));
                    }
                }
                // Listed in the catalog's /AF (document-level files; attachment annotations are not checked).
                if page.is_none() && x != 0 {
                    if let Some(listed) = &self.catalog_af {
                        if !listed.contains(&x) {
                            self.add(Rule::EmbeddedFiles, base(format!("The embedded file {fname} isn't listed in the document's /AF array"), true, None));
                        }
                    }
                }
            }
            _ => {
                if embedded_is_pdf(fs) {
                    self.add(Rule::EmbeddedFiles, base(format!("The embedded PDF {fname} must itself conform to PDF/A"), false, Some(NOTE_MANUAL)));
                } else {
                    self.add(Rule::EmbeddedFiles, base(format!("The embedded file {fname} isn't a PDF/A file (PDF/A-3 allows other files)"), true, None));
                }
            }
        }
        Ok(())
    }

    fn name_tree(&mut self, node: &PdfObject, depth: usize, out: &mut Vec<PdfObject>) -> Result<()> {
        if depth > 16 || !node.is_dict()? {
            return Ok(());
        }
        if let Some(names) = node.get_dict("Names")?.filter(|n| n.is_array().unwrap_or(false)) {
            let n = names.len()?;
            let mut i = 1;
            while i < n {
                if let Some(v) = names.get_array(i as i32)? {
                    out.push(v);
                }
                i += 2;
            }
        }
        if let Some(kids) = node.get_dict("Kids")?.filter(|n| n.is_array().unwrap_or(false)) {
            for k in kids.array_iter()?.flatten().take(5000) {
                self.name_tree(&k, depth + 1, out)?;
            }
        }
        Ok(())
    }

    // ---- the catalog -----------------------------------------------------------------------------

    fn catalog(&mut self, head: &[u8]) -> Result<Vec<Intent>> {
        let doc = self.doc;
        let cat = doc.catalog()?;
        let trailer = doc.trailer()?;

        // File.
        if self.on(Rule::FileHeader) && !header_ok(head) {
            let why = if !head.starts_with(b"%PDF-1.") || !head.get(7).is_some_and(|c| (b'0'..=b'7').contains(c)) {
                "the file doesn't start with %PDF-1.0 to %PDF-1.7"
            } else {
                "the comment with binary bytes after the header line is missing"
            };
            self.add(Rule::FileHeader, Finding { object: Some("File header".into()), message: format!("The file header is wrong: {why}"), fixable: true, new_file: true, ..Finding::default() });
        }
        if self.on(Rule::TrailerId) {
            let ok = trailer.get_dict("ID")?.is_some_and(|id| id.is_array().unwrap_or(false) && id.len().unwrap_or(0) == 2);
            if !ok {
                self.add(Rule::TrailerId, Finding { object: Some("Trailer".into()), message: "The trailer has no /ID".into(), fixable: true, ..Finding::default() });
            }
        }
        if self.on(Rule::Encryption) && trailer.get_dict("Encrypt")?.is_some() {
            self.add(Rule::Encryption, Finding { object: Some("Trailer".into()), message: "The document is encrypted".into(), fixable: true, new_file: true, ..Finding::default() });
        }
        if self.on(Rule::OptionalContent) {
            if let Some(oc) = dict_of(&cat, "OCProperties") {
                if self.fam == Family::A1 {
                    self.add(
                        Rule::OptionalContent,
                        Finding { object: Some("Layers".into()), message: "The document has layers (optional content), which PDF/A-1 doesn't have".into(), note: Some("layers removed (PDF/A-1 has none)".into()), ..Finding::default() },
                    );
                } else {
                    let mut configs = Vec::new();
                    configs.extend(dict_of(&oc, "D"));
                    if let Some(cs) = oc.get_dict("Configs")?.filter(|c| c.is_array().unwrap_or(false)) {
                        configs.extend(cs.array_iter()?.flatten().filter(|c| c.is_dict().unwrap_or(false)));
                    }
                    if configs.iter().any(|c| c.get_dict("AS").ok().flatten().is_some()) {
                        self.add(Rule::OptionalContent, Finding { object: Some("Layers".into()), message: "A layer configuration sets automatic states (/AS)".into(), fixable: true, ..Finding::default() });
                    }
                }
            }
        }

        // Metadata.
        let info = dict_of(&trailer, "Info");
        let props = info_props(info.as_ref());
        let meta = cat.get_dict("Metadata")?.filter(|m| m.is_stream().unwrap_or(false));
        let xmp_text = match &meta {
            Some(m) => Some(String::from_utf8_lossy(&m.read_stream().unwrap_or_default()).into_owned()),
            None => None,
        };
        if self.on(Rule::XmpPresent) && meta.is_none() {
            self.add(Rule::XmpPresent, Finding { object: Some("Metadata".into()), message: "The document has no XMP metadata".into(), fixable: true, ..Finding::default() });
        }
        if let (Some(x), Some(m)) = (&xmp_text, &meta) {
            if self.on(Rule::XmpUnfiltered) && !filters(m).is_empty() {
                self.add(Rule::XmpUnfiltered, Finding { object: Some("Metadata".into()), xref: Some(xref_of(m)?).filter(|n| *n != 0), message: "The metadata stream is compressed (it has a filter)".into(), fixable: true, ..Finding::default() });
            }
            if self.on(Rule::XmpId) {
                let (ok, claim) = match self.fam {
                    Family::Ua => (xmp::value(x, "pdfuaid:part").is_some_and(|v| v.trim() == "1"), "PDF/UA-1"),
                    Family::X => (xmp::value(x, "pdfxid:GTS_PDFXVersion").is_some_and(|v| v.trim().eq_ignore_ascii_case("PDF/X-4")), "PDF/X-4"),
                    _ => {
                        let want = self.profile.pdfa().map(|(p, c)| (p, c.to_string()));
                        (xmp::pdfa_id(x) == want, self.profile.label())
                    }
                };
                if !ok {
                    let found = claim_of(x).map(|c| format!(" (it identifies the file as {c})")).unwrap_or_default();
                    self.add(Rule::XmpId, Finding { object: Some("Metadata".into()), message: format!("The metadata doesn't identify the file as {claim}{found}"), fixable: true, ..Finding::default() });
                }
            }
            if self.on(Rule::XmpSync) {
                for (ikey, xkey, ival) in xmp::mirrored(&props) {
                    if self.fam == Family::Ua && xkey != "dc:title" {
                        continue;
                    }
                    let got = xmp::value(x, xkey);
                    let same = match (&got, xkey) {
                        (Some(g), "xmp:CreateDate" | "xmp:ModifyDate") => xmp::iso_date(&ival).map(|d| xmp::norm_date(&d)) == Some(xmp::norm_date(g)),
                        (Some(g), _) => g.trim() == ival.trim(),
                        (None, _) => false,
                    };
                    if !same {
                        let msg = match got {
                            Some(g) => format!("{ikey} is \"{}\" in the document information but \"{}\" in the XMP", ival.trim(), g.trim()),
                            None => format!("{ikey} is in the document information but not in the XMP"),
                        };
                        self.add(Rule::XmpSync, Finding { object: Some("Metadata".into()), message: msg, fixable: true, ..Finding::default() });
                    }
                }
            }
        }
        if self.on(Rule::InfoDates) {
            for (k, v) in [("CreationDate", &props.created), ("ModDate", &props.modified)] {
                if v.is_none() {
                    self.add(Rule::InfoDates, Finding { object: Some("Document information".into()), message: format!("The document information has no {k}"), fixable: true, ..Finding::default() });
                }
            }
        }
        if self.on(Rule::Trapped) {
            let ok = info.as_ref().and_then(|i| i.get_dict("Trapped").ok().flatten()).is_some_and(|t| {
                let s = val_name(&t).or_else(|| t.as_string().ok()).unwrap_or_default();
                matches!(s.as_str(), "True" | "False")
            });
            if !ok {
                self.add(Rule::Trapped, Finding { object: Some("Document information".into()), message: "The document information has no Trapped entry (True or False)".into(), fixable: true, ..Finding::default() });
            }
        }

        // Colour: the output intents.
        let mut intents = Vec::new();
        if let Some(ois) = cat.get_dict("OutputIntents")?.filter(|o| o.is_array().unwrap_or(false)) {
            for oi in ois.array_iter()?.flatten() {
                if !oi.is_dict()? {
                    continue;
                }
                let subtype = name(&oi, "S")?.unwrap_or_default();
                let (valid, comps) = match oi.get_dict("DestOutputProfile")?.filter(|p| p.is_stream().unwrap_or(false)) {
                    Some(p) => icc_facts(&p),
                    None => (false, None),
                };
                intents.push(Intent { subtype, valid, comps });
            }
        }

        // Actions at the top of the document.
        if self.on(Rule::ActionsForbidden) || self.on(Rule::ActionsAdditional) {
            if let Some(a) = cat.get_dict("OpenAction")?.filter(|a| a.is_dict().unwrap_or(false)) {
                self.action(None, "Document open action", 0, None, &a)?;
            }
            if dict_of(&cat, "AA").is_some() {
                self.owner_actions(None, "Document", &cat, None)?;
            }
            self.outline_actions(&cat)?;
            self.field_actions(&cat)?;
        }
        let names = dict_of(&cat, "Names");
        if self.on(Rule::JavaScript) && names.as_ref().is_some_and(|n| n.get_dict("JavaScript").ok().flatten().is_some()) {
            self.add(Rule::JavaScript, Finding { object: Some("Document JavaScript".into()), message: "The document has document-level JavaScript".into(), fixable: true, ..Finding::default() });
        }
        if let Some(af) = dict_of(&cat, "AcroForm") {
            if self.on(Rule::NeedAppearances) && is_true(&af, "NeedAppearances") {
                self.add(Rule::NeedAppearances, Finding { object: Some("Form".into()), message: "The form asks viewers to draw its fields (NeedAppearances)".into(), fixable: true, ..Finding::default() });
            }
            if self.on(Rule::Xfa) && af.get_dict("XFA")?.is_some() {
                self.add(Rule::Xfa, Finding { object: Some("Form".into()), message: "The form has XFA data".into(), fixable: true, ..Finding::default() });
            }
        }
        if self.on(Rule::EmbeddedFiles) {
            if self.profile == Profile::Pdfa3b {
                let mut listed = HashSet::new();
                if let Some(af) = cat.get_dict("AF")?.filter(|a| a.is_array().unwrap_or(false)) {
                    for e in af.array_iter()?.flatten() {
                        let n = xref_of(&e)?;
                        if n != 0 {
                            listed.insert(n);
                        }
                    }
                }
                self.catalog_af = Some(listed);
            }
            if let Some(ef) = names.as_ref().and_then(|n| dict_of(n, "EmbeddedFiles")) {
                let mut specs = Vec::new();
                self.name_tree(&ef, 0, &mut specs)?;
                for s in specs {
                    self.filespec(None, &s, None)?;
                }
            }
        }
        Ok(intents)
    }

    fn outline_actions(&mut self, cat: &PdfObject) -> Result<()> {
        let Some(root) = dict_of(cat, "Outlines") else { return Ok(()) };
        let mut stack = vec![root.get_dict("First")?];
        let mut seen = 0;
        while let Some(item) = stack.pop() {
            let Some(it) = item else { continue };
            seen += 1;
            if seen > 5000 || !it.is_dict()? {
                break;
            }
            if let Some(a) = it.get_dict("A")?.filter(|a| a.is_dict().unwrap_or(false)) {
                self.action(None, "Bookmark", xref_of(&it)?, None, &a)?;
            }
            stack.push(it.get_dict("Next")?);
            stack.push(it.get_dict("First")?);
        }
        Ok(())
    }

    /// Fields that are not widgets themselves (parents) can carry AA too.
    fn field_actions(&mut self, cat: &PdfObject) -> Result<()> {
        let Some(af) = dict_of(cat, "AcroForm") else { return Ok(()) };
        let Some(fields) = af.get_dict("Fields")?.filter(|f| f.is_array().unwrap_or(false)) else { return Ok(()) };
        let mut stack: Vec<PdfObject> = fields.array_iter()?.flatten().collect();
        let mut seen = 0;
        while let Some(f) = stack.pop() {
            seen += 1;
            if seen > 20000 {
                break;
            }
            if !f.is_dict()? {
                continue;
            }
            // A merged field/widget is read with its page; parents here.
            if name(&f, "Subtype")?.as_deref() != Some("Widget") && f.get_dict("AA")?.is_some() {
                self.owner_actions(None, "Form field", &f, None)?;
            }
            if let Some(kids) = f.get_dict("Kids")?.filter(|k| k.is_array().unwrap_or(false)) {
                stack.extend(kids.array_iter()?.flatten());
            }
        }
        Ok(())
    }

    // ---- objects ---------------------------------------------------------------------------------

    fn objects(&mut self, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<()> {
        let want = self.on(Rule::Lzw) || self.on(Rule::ExternalStreams) || self.on(Rule::ObjectStreams);
        if !want {
            return Ok(());
        }
        let n = self.doc.count_objects()?;
        let step = (n / 50).max(1000);
        for i in 1..n as i32 {
            if i as u32 % step == 0 {
                if cancel.load(Ordering::Relaxed) {
                    return Err(bad("cancelled"));
                }
                progress(Progress { done: i as u32, pages: n, stage: "objects" });
            }
            let o = self.doc.new_indirect(i, 0)?;
            if !o.is_stream().unwrap_or(false) {
                continue;
            }
            let fl = filters(&o);
            let ty = name(&o, "Type")?.unwrap_or_default();
            if self.on(Rule::ObjectStreams) && (ty == "ObjStm" || ty == "XRef") {
                self.add(Rule::ObjectStreams, Finding { object: Some(if ty == "XRef" { "Cross-reference stream" } else { "Object stream" }.into()), xref: Some(i), message: format!("Object {i} is a {} (PDF 1.5)", if ty == "XRef" { "cross-reference stream" } else { "object stream" }), fixable: true, new_file: true, ..Finding::default() });
                continue;
            }
            if self.on(Rule::Lzw) && fl.iter().any(|f| f == "LZWDecode" || f == "LZW") {
                // Re-encodable only when LZW is not followed by an image codec.
                let plain = fl.iter().all(|f| matches!(f.as_str(), "LZWDecode" | "LZW" | "ASCII85Decode" | "A85" | "ASCIIHexDecode" | "AHx" | "FlateDecode" | "Fl"));
                self.add(
                    Rule::Lzw,
                    Finding {
                        object: Some("Stream".into()),
                        xref: Some(i),
                        message: format!("Stream {i} uses LZW compression"),
                        fixable: plain,
                        note: (!plain).then(|| "an image re-encoded without LZW".to_string()),
                        ..Finding::default()
                    },
                );
            }
            if self.on(Rule::ExternalStreams) && (o.get_dict("F")?.is_some() || o.get_dict("FFilter")?.is_some()) {
                // An embedded file's own F is its Filespec's name, which sits on the dictionary, not the stream.
                if o.get_dict("Type")?.is_none() || ty != "EmbeddedFile" {
                    self.add(
                        Rule::ExternalStreams,
                        Finding { object: Some("Stream".into()), xref: Some(i), message: format!("Stream {i} refers to an external file"), note: Some("the files they refer to put inside the document".into()), ..Finding::default() },
                    );
                }
            }
        }
        Ok(())
    }
}

/// Annotation subtypes the family does not allow.
pub(crate) fn annotation_forbidden(fam: Family, sub: &str) -> bool {
    match fam {
        Family::A1 => matches!(sub, "Sound" | "Movie" | "FileAttachment") || !ANNOT_14.contains(&sub),
        _ => matches!(sub, "Sound" | "Movie" | "RichMedia") || !ANNOT_32000.contains(&sub),
    }
}

/// The stream of an embedded file's filespec (`/EF /F`, else `/UF`).
pub(crate) fn embedded_stream(fs: &PdfObject) -> Option<PdfObject> {
    dict_of(fs, "EF").and_then(|e| e.get_dict("F").ok().flatten().or_else(|| e.get_dict("UF").ok().flatten()))
}

/// How many bytes of an attachment are looked at to tell whether it is a PDF.
const SNIFF_BYTES: usize = 1024;

/// An embedded file that is itself a PDF (by media type or by its first bytes; only the first
/// kilobyte is decoded, however big the attachment is).
pub(crate) fn embedded_is_pdf(fs: &PdfObject) -> bool {
    embedded_stream(fs).is_some_and(|s| {
        name(&s, "Subtype").ok().flatten().is_some_and(|m| m.eq_ignore_ascii_case("application#2Fpdf") || m.eq_ignore_ascii_case("application/pdf"))
            || s.read_stream_head(SNIFF_BYTES).map(|b| b.starts_with(b"%PDF")).unwrap_or(false)
    })
}

fn header_ok(head: &[u8]) -> bool {
    if head.len() < 9 || !head.starts_with(b"%PDF-1.") || !(b'0'..=b'7').contains(&head[7]) {
        return false;
    }
    let Some(nl) = head.iter().position(|b| *b == b'\n' || *b == b'\r') else { return false };
    let rest = &head[nl..];
    let rest = rest.strip_prefix(b"\r\n").or_else(|| rest.strip_prefix(b"\n")).or_else(|| rest.strip_prefix(b"\r")).unwrap_or(rest);
    rest.first() == Some(&b'%') && rest.len() >= 5 && rest[1..5].iter().all(|b| *b >= 128)
}

/// `(well formed, components)` of an ICC profile stream.
fn icc_facts(p: &PdfObject) -> (bool, Option<i64>) {
    let data = p.read_stream().unwrap_or_default();
    let n = p.get_dict("N").ok().flatten().and_then(|v| v.as_int().ok()).map(i64::from);
    if data.len() < 128 || &data[36..40] != b"acsp" {
        return (false, n);
    }
    let space = match &data[16..20] {
        b"GRAY" => Some(1),
        b"RGB " => Some(3),
        b"CMYK" => Some(4),
        _ => None,
    };
    let size_ok = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize <= data.len();
    let consistent = match (n, space) {
        (Some(n), Some(s)) => n == s,
        _ => true,
    };
    (size_ok && space.is_some() && consistent, space.or(n))
}

pub(crate) fn info_props(info: Option<&PdfObject>) -> xmp::InfoProps {
    let Some(i) = info else { return xmp::InfoProps::default() };
    let t = |k: &str| text(i, k).ok().flatten();
    xmp::InfoProps {
        title: t("Title"),
        author: t("Author"),
        subject: t("Subject"),
        keywords: t("Keywords"),
        producer: t("Producer"),
        creator: t("Creator"),
        created: t("CreationDate"),
        modified: t("ModDate"),
    }
}

fn claim_of(x: &str) -> Option<String> {
    if let Some((p, c)) = xmp::pdfa_id(x) {
        return Some(format!("PDF/A-{p}{}", c.to_lowercase()));
    }
    if let Some(p) = xmp::value(x, "pdfuaid:part").and_then(|v| v.trim().parse::<u8>().ok()) {
        return Some(format!("PDF/UA-{p}"));
    }
    xmp::value(x, "pdfxid:GTS_PDFXVersion").map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

pub(crate) fn claim(doc: &PdfDocument) -> Result<Option<Claim>> {
    let cat = doc.catalog()?;
    let Some(m) = cat.get_dict("Metadata")?.filter(|m| m.is_stream().unwrap_or(false)) else { return Ok(None) };
    let x = String::from_utf8_lossy(&m.read_stream().unwrap_or_default()).into_owned();
    Ok(claim_of(&x).map(|label| Claim { profile: super::claimed_profile(&label), label }))
}

// ---- the run -------------------------------------------------------------------------------------

pub(crate) fn run(doc: &PdfDocument, head: &[u8], profile: Profile, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Report> {
    let n = doc.page_count()?.max(0) as u32;
    let claimed = claim(doc)?.map(|c| c.label);
    if profile.family() == Family::Ua {
        return run_ua(doc, head, profile, n, claimed, progress, cancel);
    }
    let mut c = Ctx {
        doc,
        profile,
        fam: profile.family(),
        found: HashMap::new(),
        seen_fonts: HashSet::new(),
        seen_font_names: HashSet::new(),
        seen_forms: HashSet::new(),
        seen_gs: HashSet::new(),
        seen_images: HashSet::new(),
        seen_patterns: HashSet::new(),
        device: [None; 3],
        default_cs: [false; 3],
        blocked: 0,
        catalog_af: None,
        collect: None,
    };
    let intents = c.catalog(head)?;
    for p in 0..n {
        if cancel.load(Ordering::Relaxed) {
            return Err(bad("cancelled"));
        }
        let page = doc.find_page(p as i32)?;
        c.page(p, &page)?;
        progress(Progress { done: p + 1, pages: n, stage: "check" });
    }
    c.objects(progress, cancel)?;
    colour_rules(&mut c, &intents);
    Ok(finish(c, n, claimed))
}

fn colour_rules(c: &mut Ctx, intents: &[Intent]) {
    let [gray, rgb, cmyk] = c.device;
    let used_at = [0, 1, 2].map(|i| c.device[i].is_some() && !c.default_cs[i]);
    let used = |i: usize| used_at[i];
    let any = used(0) || used(1) || used(2);
    let want = if c.fam == Family::X { "GTS_PDFX" } else { "GTS_PDFA1" };
    let intent = intents.iter().find(|i| i.subtype == want);
    let first_page = [gray, rgb, cmyk].into_iter().flatten().min();
    if c.on(Rule::OutputIntent) {
        if c.fam == Family::X {
            if intent.is_none() {
                c.add(Rule::OutputIntent, Finding { object: Some("Output intent".into()), message: "The document has no PDF/X output intent (GTS_PDFX)".into(), note: Some("a print condition (an output intent) you choose".into()), ..Finding::default() });
            }
        } else if intent.is_none() && any {
            let cm = used(2);
            let names: Vec<&str> = [(used(0), "DeviceGray"), (used(1), "DeviceRGB"), (used(2), "DeviceCMYK")].into_iter().filter(|(u, _)| *u).map(|(_, n)| n).collect();
            c.add(
                Rule::OutputIntent,
                Finding {
                    page: first_page,
                    object: Some("Output intent".into()),
                    message: format!("{} is used but the document has no PDF/A output intent", names.join(" and ")),
                    fixable: !cm,
                    note: cm.then(|| "a CMYK output condition you choose".to_string()),
                    ..Finding::default()
                },
            );
        }
    }
    if c.on(Rule::OutputIntentProfile) {
        for i in intents.iter().filter(|i| i.subtype == want) {
            if !i.valid {
                c.add(
                    Rule::OutputIntentProfile,
                    Finding { object: Some("Output intent".into()), message: "The output intent's destination profile isn't a valid ICC profile".into(), note: Some("a valid ICC profile you choose".into()), ..Finding::default() },
                );
                break;
            }
        }
    }
    if c.on(Rule::DeviceColour) {
        if let Some(i) = intent.filter(|i| i.valid) {
            if used(2) && i.comps != Some(4) {
                c.add(
                    Rule::DeviceColour,
                    Finding { page: cmyk, object: Some("Colour".into()), message: "DeviceCMYK is used, but the output intent isn't a CMYK profile".into(), note: Some("colour converted to the output intent's colour space".into()), ..Finding::default() },
                );
            }
            if used(1) && i.comps != Some(3) {
                c.add(
                    Rule::DeviceColour,
                    Finding { page: rgb, object: Some("Colour".into()), message: "DeviceRGB is used, but the output intent isn't an RGB profile".into(), note: Some("colour converted to the output intent's colour space".into()), ..Finding::default() },
                );
            }
        }
    }
    if c.on(Rule::RgbContent) {
        if let Some(p) = rgb {
            c.add(Rule::RgbContent, Finding { page: Some(p), object: Some("Colour".into()), message: format!("DeviceRGB is used (first on page {})", p + 1), ..Finding::default() });
        }
    }
}

fn finish(mut c: Ctx, pages: u32, claimed: Option<String>) -> Report {
    let profile = c.profile;
    let mut results = Vec::new();
    for rule in Rule::ALL {
        if !rule.applies(profile) {
            continue;
        }
        let mut findings = c.found.remove(&rule).unwrap_or_default();
        let count = findings.len() as u32;
        let status = if findings.is_empty() {
            Status::Passed
        } else if rule == Rule::RgbContent {
            Status::Info
        } else if findings.iter().all(|f| f.note.as_deref() == Some(NOTE_MANUAL)) {
            Status::Manual
        } else {
            Status::Failed
        };
        if findings.len() > MAX_FINDINGS {
            let more = findings.len() - MAX_FINDINGS;
            findings.truncate(MAX_FINDINGS);
            findings.push(Finding { message: format!("... and {more} more"), ..Finding::default() });
        }
        let d = rule.def();
        results.push(RuleResult { rule: d.id, name: d.name, group: d.group, clause: clause_text(rule, profile), status, hint: d.hint, fix: d.fix, a11y_fix: None, count, findings });
    }
    let plan = plan_of(&results, profile);
    let conforms = !results.iter().any(|r| r.status == Status::Failed);
    Report { profile, pages, claimed, conforms, results, plan }
}

/// What the pages use that a conversion needs to know.
pub(crate) struct Survey {
    pub found: Collected,
    /// Device colour spaces in use (gray, RGB, CMYK) that no Default colour space covers.
    pub device: [bool; 3],
}

/// Reads every page once: the fonts in use and the device colour spaces.
pub(crate) fn survey(doc: &PdfDocument, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Survey> {
    let n = doc.page_count()?.max(0) as u32;
    let mut c = Ctx {
        doc,
        profile: Profile::Pdfa2b,
        fam: Family::A23,
        found: HashMap::new(),
        seen_fonts: HashSet::new(),
        seen_font_names: HashSet::new(),
        seen_forms: HashSet::new(),
        seen_gs: HashSet::new(),
        seen_images: HashSet::new(),
        seen_patterns: HashSet::new(),
        device: [None; 3],
        default_cs: [false; 3],
        blocked: 0,
        catalog_af: None,
        collect: Some(Collected::default()),
    };
    for p in 0..n {
        if cancel.load(Ordering::Relaxed) {
            return Err(bad("cancelled"));
        }
        let page = doc.find_page(p as i32)?;
        c.page(p, &page)?;
        progress(Progress { done: p + 1, pages: n, stage: "survey" });
    }
    let device = [0, 1, 2].map(|i| c.device[i].is_some() && !c.default_cs[i]);
    Ok(Survey { found: c.collect.take().unwrap_or_default(), device })
}

/// What the clause column shows: the ISO clause in the PDF/A families, the standard otherwise.
fn clause_text(rule: Rule, profile: Profile) -> &'static str {
    let c = rule.clause(profile);
    match profile.family() {
        Family::X => "ISO 15930-7",
        Family::Ua => "ISO 14289-1",
        _ if c.contains('?') => "ISO 19005",
        _ => c,
    }
}

fn run_ua(doc: &PdfDocument, head: &[u8], profile: Profile, pages: u32, claimed: Option<String>, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> Result<Report> {
    let a = a11y::check(doc, progress, cancel)?;
    // The preflight rules that PDF/UA adds: identification, XMP title, embedded fonts.
    let mut c = Ctx {
        doc,
        profile,
        fam: Family::Ua,
        found: HashMap::new(),
        seen_fonts: HashSet::new(),
        seen_font_names: HashSet::new(),
        seen_forms: HashSet::new(),
        seen_gs: HashSet::new(),
        seen_images: HashSet::new(),
        seen_patterns: HashSet::new(),
        device: [None; 3],
        default_cs: [false; 3],
        blocked: 0,
        catalog_af: None,
        collect: None,
    };
    c.catalog(head)?;
    for p in 0..pages {
        if cancel.load(Ordering::Relaxed) {
            return Err(bad("cancelled"));
        }
        let page = doc.find_page(p as i32)?;
        let res = page.get_dict_inheritable("Resources")?.filter(|r| r.is_dict().unwrap_or(false));
        let data = a11y::page_content(&page)?;
        c.scan_content(p, &data, res.as_ref(), 0)?;
    }
    let mut rep = finish(c, pages, claimed);
    for r in &a.results {
        let status = match r.status {
            a11y::Status::Passed => Status::Passed,
            a11y::Status::Failed => Status::Failed,
            a11y::Status::Manual => Status::Manual,
            a11y::Status::Skipped => continue,
        };
        let fix_tag = r.fix.filter(|f| matches!(*f, "title" | "displayDocTitle" | "language" | "tabOrder" | "fieldTooltips"));
        let note = match r.fix {
            Some("autotag") | Some("figureAlt") => "the Accessibility checker (Autotag, alternate text)",
            _ => "a person to review or retag it",
        };
        let findings: Vec<Finding> = r
            .findings
            .iter()
            .map(|f| Finding {
                page: f.page,
                object: f.object.clone(),
                xref: f.xref,
                rect: f.rect,
                message: f.message.clone(),
                fixable: fix_tag.is_some() && f.xref.is_none_or(|_| true),
                new_file: false,
                note: fix_tag.is_none().then(|| note.to_string()),
            })
            .collect();
        rep.results.push(RuleResult {
            rule: r.rule.id(),
            name: r.name,
            group: r.group,
            clause: "ISO 14289-1",
            status,
            hint: r.hint,
            fix: fix_tag.map(|_| FixGroup::Accessibility),
            a11y_fix: fix_tag,
            count: findings.len() as u32,
            findings,
        });
    }
    // Preflight's own groups first, then the accessibility ones in the checker's order.
    rep.plan = plan_of(&rep.results, profile);
    rep.conforms = !rep.results.iter().any(|r| r.status == Status::Failed);
    Ok(rep)
}

fn effect(g: FixGroup, profile: Profile) -> String {
    match g {
        FixGroup::Fonts => "Embeds the fonts that are missing (built-in base-14 clones or installed fonts) and subsets them".into(),
        FixGroup::Colour => "Adds an sRGB output intent".into(),
        FixGroup::Metadata => format!("Identifies the file as {} and matches the XMP to the document information", profile.label()),
        FixGroup::Actions => "Removes JavaScript, launch and other forbidden actions".into(),
        FixGroup::Annotations => "Sets annotations to print and removes forbidden annotation types".into(),
        FixGroup::Forms => "Draws form appearances into the file and removes XFA".into(),
        FixGroup::Images => "Turns off image interpolation and removes alternates and OPI".into(),
        FixGroup::Files => if profile == Profile::Pdfa3b { "Marks each embedded file's relationship".into() } else { "Removes the embedded files".into() },
        FixGroup::Structure => "Adds the file identifier and re-encodes LZW streams; a new file also gets a new header, no encryption and no object streams".into(),
        FixGroup::Appearance => "Makes transparency opaque and drops transfer functions; this changes how pages look".into(),
        FixGroup::Accessibility => "Sets the title, language, tab order and field tooltips".into(),
    }
}

pub(crate) fn plan_of(results: &[RuleResult], profile: Profile) -> Plan {
    let mut plan = Plan::default();
    let mut groups: HashMap<FixGroup, (u32, u32)> = HashMap::new();
    let mut remaining: Vec<(String, u32)> = Vec::new();
    let mut font_total = 0u32;
    let mut font_fixable = 0u32;
    for r in results.iter().filter(|r| r.status == Status::Failed) {
        // The findings are capped; the count of the rest is carried by `count`.
        let listed: Vec<&Finding> = r.findings.iter().filter(|f| !f.message.starts_with("... and ")).collect();
        let extra = r.count.saturating_sub(listed.len() as u32);
        for f in &listed {
            plan.total += 1;
            let fixable = f.fixable && r.fix.is_some();
            if r.rule == "fonts-embedded" {
                font_total += 1;
                font_fixable += fixable as u32;
            }
            if fixable {
                let e = groups.entry(r.fix.unwrap()).or_default();
                e.0 += 1;
                e.1 += f.new_file as u32;
            } else {
                let note = f.note.clone().unwrap_or_else(|| "a fix that isn't automatic".into());
                match remaining.iter_mut().find(|(n, _)| *n == note) {
                    Some(e) => e.1 += 1,
                    None => remaining.push((note, 1)),
                }
            }
        }
        // Findings past the cap behave like the last listed ones of the rule.
        if extra > 0 {
            plan.total += extra;
            if let (Some(g), Some(last)) = (r.fix, listed.last()) {
                if last.fixable {
                    groups.entry(g).or_default().0 += extra;
                    continue;
                }
            }
            let note = listed.last().and_then(|f| f.note.clone()).unwrap_or_else(|| "a fix that isn't automatic".into());
            match remaining.iter_mut().find(|(n, _)| *n == note) {
                Some(e) => e.1 += extra,
                None => remaining.push((note, extra)),
            }
        }
    }
    let mut ids: Vec<FixGroup> = groups.keys().copied().collect();
    ids.sort();
    for id in ids {
        let (issues, new_file_issues) = groups[&id];
        let disabled = (id == FixGroup::Fonts && font_total > 0 && font_fixable == 0).then(|| "None of the missing fonts can be embedded".to_string());
        plan.groups.push(PlanGroup { id, label: id.label(), effect: effect(id, profile), issues, new_file_issues, default_on: id.default_on(), disabled });
    }
    plan.remaining = remaining.into_iter().map(|(text, count)| Remaining { count, text }).collect();
    plan.needs_lang = results.iter().any(|r| r.rule == "primary-language" && r.status == Status::Failed);
    plan
}
