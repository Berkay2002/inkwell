//! The assistant's tools for the rest of the app: page marks (header and footer, watermark,
//! background, Bates numbers), document properties, removing hidden information, the accessibility
//! and PDF/A / PDF/UA / PDF/X checks and their safe fixes, and opening a tool for the user.
//!
//! Same rules as `actions.rs`: a change goes through the document's edit lane as one undo step
//! under `EditSource::Assistant`, asks according to its class, refuses a signed document before the
//! card is shown, and is an `Err` with a sentence the model can act on when it changed nothing.
//! Nothing here is provider-specific and nothing here writes the user's file.
//!
//! What the assistant never does (digital signatures, passwords and permissions, printing,
//! overwriting the original, opening links) is listed with its reason in `src/lib/aiCoverage.json`,
//! which a test keeps in step with the app's tool list.

use super::actions::{Touch, data_block, err, guard, landed, lane, plural, str_arg};
use super::tools::{Class, Ctx, Lane, Output, Tool, UiAction, clip, int, page_list_label, page_of, pages_requested, tool};
use crate::a11y::{self, Fix};
use crate::marks::{AddRequest, Background, Bates, Date, HeaderFooter, Mark, MarkKind, Style, Watermark};
use crate::redact::Sanitize;
use crate::textedit::{self, BlockChange, RunChange};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

/// More than this in one answer is a wall the user will not read.
const MAX_REPORT: usize = 6_000;

pub fn tools() -> Vec<Arc<Tool>> {
    let pages = json!({ "type": "array", "items": { "type": "integer", "minimum": 1 }, "description": "1-based page numbers; omit for every page" });
    let text = |what: &str| json!({ "type": "string", "description": what });
    vec![
        tool(
            "page_marks",
            "Adds, updates or removes a header and footer, a watermark, a page background or Bates numbers on pages. \
             kind=header_footer takes the text of up to six places (top_left … bottom_right) with tokens: <<1>> page number, \
             <<n>> page count, <<1 of n>>, <<Page 1 of n>>, <<yyyy-mm-dd>> today's date. kind=watermark takes text (DRAFT, \
             CONFIDENTIAL), optional opacity (0.05-1), rotation (degrees, default 45), behind (true puts it behind the page). \
             kind=background takes color (#rrggbb) and opacity. kind=bates takes prefix, suffix, digits (3-15), start, position. \
             action=add fails when that kind is already on those pages (use update); update replaces it; remove takes it off. \
             Pictures are not supported: the user adds those in the tool (open_tool). One undo step.",
            json!({ "type": "object", "additionalProperties": false, "required": ["kind"], "properties": {
                "kind": { "type": "string", "enum": ["header_footer", "watermark", "background", "bates"] },
                "action": { "type": "string", "enum": ["add", "update", "remove"], "description": "default add" },
                "pages": pages,
                "top_left": text("header_footer: text at the top left"),
                "top_center": text("header_footer: text at the top centre"),
                "top_right": text("header_footer: text at the top right"),
                "bottom_left": text("header_footer: text at the bottom left"),
                "bottom_center": text("header_footer: text at the bottom centre, e.g. Page <<1 of n>>"),
                "bottom_right": text("header_footer: text at the bottom right"),
                "text": text("watermark: the words to stamp"),
                "color": text("#rrggbb: watermark, header/footer text or background colour"),
                "opacity": { "type": "number", "minimum": 0.05, "maximum": 1 },
                "rotation": { "type": "number", "description": "watermark: degrees counter-clockwise, default 45" },
                "behind": { "type": "boolean", "description": "watermark: behind the page content" },
                "font_size": { "type": "number", "minimum": 4, "maximum": 144, "description": "points; a watermark fits the page when omitted" },
                "bold": { "type": "boolean" },
                "prefix": text("bates: text before the number"),
                "suffix": text("bates: text after the number"),
                "digits": { "type": "integer", "minimum": 3, "maximum": 15, "description": "bates: width of the number, default 6" },
                "start": { "type": "integer", "minimum": 0, "description": "bates: first number, default 1" },
                "position": { "type": "string", "enum": POSITIONS, "description": "bates: where the number goes, default bottom_right" },
            } }),
            Class::Edit,
            marks_label,
            page_marks,
        )
        .checked(page_marks_check),
        tool(
            "set_document_properties",
            "Sets the document's title, author, subject or keywords (the file's properties, shown in the title bar and \
             by search engines). Give only what should change; an empty string clears a field. One undo step.",
            json!({ "type": "object", "additionalProperties": false, "properties": {
                "title": text("document title"),
                "author": text("author"),
                "subject": text("subject"),
                "keywords": text("keywords, comma separated"),
            } }),
            Class::Edit,
            properties_label,
            set_document_properties,
        )
        .checked(|c, _| guard_content(c)),
        tool(
            "edit_text",
            "Replaces a piece of existing text on a page, keeping its font, size and colour: find is the exact text as get_text shows it (it must appear once on that page; make it longer when it does not), replace is the new text (an empty string deletes it). Only text that is really in the page (not a scanned picture: run ocr_document first for that) and whose font can be edited. A long replacement reflows within the block. One undo step.",
            json!({ "type": "object", "additionalProperties": false, "required": ["page", "find", "replace"], "properties": {
                "page": { "type": "integer", "minimum": 1 },
                "find": { "type": "string", "description": "the exact text to replace (up to 500 characters)" },
                "replace": { "type": "string", "description": "the new text; empty deletes" },
            } }),
            Class::Edit,
            edit_text_label,
            edit_text,
        )
        .checked(edit_text_check),
        tool(
            "sanitize_document",
            "Removes hidden information from the document: metadata (author, creation tool, XMP), hidden layers, scripts and \
             actions (JavaScript, open actions), attached files, and optionally hidden text, comments and bookmarks. \
             Everything you name is deleted for good once the user saves; the user can still undo it before that. \
             Defaults: metadata, hiddenLayers, scriptsActions and embeddedFiles on; hiddenText (it also removes the text layer \
             of scanned pages), annotations (ALL comments and markup) and bookmarks off. Pass true or false to change them. \
             Use this for \"remove metadata / hidden data\"; to remove marked content use the redact_* tools instead.",
            json!({ "type": "object", "additionalProperties": false, "properties": {
                "metadata": { "type": "boolean" },
                "hiddenLayers": { "type": "boolean" },
                "hiddenText": { "type": "boolean" },
                "scriptsActions": { "type": "boolean" },
                "embeddedFiles": { "type": "boolean" },
                "annotations": { "type": "boolean", "description": "deletes ALL comments and markup" },
                "bookmarks": { "type": "boolean" },
            } }),
            Class::Destructive,
            sanitize_label,
            sanitize_document,
        )
        .checked(sanitize_check),
        tool(
            "check_accessibility",
            "Checks the document for accessibility problems (tags, title, language, alternate text, reading order, form \
             labels, tab order) and lists what fails, which ones fix_accessibility can repair, and the figures that need \
             alternate text (with the xref to give it). Reads the document as it is now, unsaved changes included.",
            json!({ "type": "object", "additionalProperties": false, "properties": {} }),
            Class::Read,
            |_| "Check accessibility".into(),
            check_accessibility,
        ),
        tool(
            "fix_accessibility",
            "Applies safe accessibility fixes as one undo step. fixes is a list of { fix, ... }: title { title }, language \
             { lang: en-US }, display_doc_title, tab_order, field_tooltips, figure_alt { xref, alt } (the xref comes from \
             check_accessibility; look at the figure with get_page_image first and describe it in one sentence), \
             autotag (best-effort tags for an untagged document: tell the user it is a guess to review).",
            json!({ "type": "object", "additionalProperties": false, "required": ["fixes"], "properties": {
                "fixes": { "type": "array", "minItems": 1, "maxItems": 60, "items": { "type": "object", "required": ["fix"], "properties": {
                    "fix": { "type": "string", "enum": ["title", "language", "display_doc_title", "tab_order", "field_tooltips", "figure_alt", "autotag"] },
                    "title": { "type": "string" },
                    "lang": { "type": "string", "description": "BCP 47 tag, e.g. en-US or sv-SE" },
                    "xref": { "type": "integer" },
                    "alt": { "type": "string", "description": "alternate text of the figure, one sentence" },
                } } },
            } }),
            Class::Edit,
            fix_label,
            fix_accessibility,
        )
        .checked(fix_check),
        tool(
            "check_preflight",
            "Checks the document against a standard: pdfa-1b, pdfa-2b, pdfa-3b, pdfa-2u (archiving), pdfua-1 (accessibility) \
             or pdfx-4 (print). Lists the rules that fail with examples, what a conversion could settle and what it could \
             not. It is a check, not a certification.",
            json!({ "type": "object", "additionalProperties": false, "required": ["profile"], "properties": {
                "profile": { "type": "string", "enum": ["pdfa-1b", "pdfa-2b", "pdfa-3b", "pdfa-2u", "pdfua-1", "pdfx-4"] },
            } }),
            Class::Read,
            |a| format!("Check against {}", profile_arg(a).map_or("a standard", |p| p.label())),
            check_preflight,
        ),
        tool(
            "open_tool",
            "Opens one of the app's tools for the user, for what you may not do yourself: protect (password), security \
             (permissions), merge, search-advanced, and the tools you can also run (compress, ocr, preflight, header-footer, \
             watermark, background, bates, export, create, compare, a11y). Says nothing about what to enter; tell the user in \
             your answer what to do there.",
            json!({ "type": "object", "additionalProperties": false, "required": ["tool"], "properties": {
                "tool": { "type": "string", "enum": OPEN_TOOLS },
            } }),
            Class::Read,
            |a| format!("Opened {}", str_arg(a, "tool").unwrap_or("a tool")),
            open_tool,
        ),
    ]
}

/// Tools `open_tool` may open (the ids of `src/lib/toolItems.ts` and `tools.ts`).
const OPEN_TOOLS: [&str; 15] = [
    "protect",
    "security",
    "merge",
    "search-advanced",
    "compress",
    "ocr",
    "preflight",
    "header-footer",
    "watermark",
    "background",
    "bates",
    "export",
    "create",
    "compare",
    "a11y",
];

// ---- shared -----------------------------------------------------------------------------------

pub(super) fn engine<'a>(c: &'a Ctx) -> Result<Lane<'a>, String> {
    c.host.lane().ok_or_else(|| "This document is not available to the assistant's tools from here.".to_string())
}

fn live_pages(l: &Lane) -> Result<u32, String> {
    l.eng.read_custom(l.doc, |d| Ok(d.page_count()?.max(0) as u32)).map_err(err)
}

/// The pre-approval check of a tool that only needs the signature guard (content change).
pub(super) fn guard_content(c: &Ctx) -> Result<Option<String>, String> {
    match c.host.lane() {
        Some(l) => guard(&l, Touch::Content, &[]).map(|()| None),
        None => Ok(None),
    }
}

fn on_pages(a: &Value) -> String {
    let p = pages_requested(a);
    if p.is_empty() { "all pages".into() } else { page_list_label(&p) }
}

fn pages_text(pages: &[u32], count: u32) -> String {
    if pages.len() as u32 == count {
        "all pages".into()
    } else {
        super::actions::pages_label(pages)
    }
}

pub(super) fn text_arg(a: &Value, key: &str, max: usize) -> Result<Option<String>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.chars().count() > max {
                return Err(format!("\"{key}\" is too long (at most {max} characters)."));
            }
            Ok(Some(s.to_string()))
        }
        Some(_) => Err(format!("\"{key}\" must be text.")),
    }
}

fn number(a: &Value, key: &str, lo: f64, hi: f64) -> Result<Option<f64>, String> {
    match a.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_f64().ok_or_else(|| format!("\"{key}\" must be a number."))?;
            if !(lo..=hi).contains(&n) {
                return Err(format!("\"{key}\" must be between {lo} and {hi}."));
            }
            Ok(Some(n))
        }
    }
}

/// `#rrggbb` as the 0-1 triple the marks use.
fn color_arg(a: &Value, key: &str) -> Result<Option<[f64; 3]>, String> {
    let Some(s) = str_arg(a, key) else { return Ok(None) };
    let h = s.strip_prefix('#').unwrap_or(s);
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("\"{key}\" must be a colour like #cc0000."));
    }
    let v = |i: usize| f64::from(u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0)) / 255.0;
    Ok(Some([v(0), v(2), v(4)]))
}

pub(super) fn unknown_keys(a: &Value, known: &[&str], what: &str) -> Result<(), String> {
    if let Some(o) = a.as_object() {
        if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("{what} has no option \"{k}\". The options are: {}.", known.join(", ")));
        }
    }
    Ok(())
}

// ---- page marks ---------------------------------------------------------------------------------

const POSITIONS: [&str; 6] = ["top_left", "top_center", "top_right", "bottom_left", "bottom_center", "bottom_right"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Add,
    Update,
    Remove,
}

fn action_of(a: &Value) -> Result<Action, String> {
    match str_arg(a, "action").unwrap_or("add") {
        "add" => Ok(Action::Add),
        "update" => Ok(Action::Update),
        "remove" => Ok(Action::Remove),
        o => Err(format!("Unknown action \"{o}\". Use add, update or remove.")),
    }
}

fn kind_of(a: &Value) -> Result<MarkKind, String> {
    match str_arg(a, "kind") {
        Some("header_footer") => Ok(MarkKind::HeaderFooter),
        Some("watermark") => Ok(MarkKind::Watermark),
        Some("background") => Ok(MarkKind::Background),
        Some("bates") => Ok(MarkKind::Bates),
        Some(o) => Err(format!("Unknown kind \"{o}\". Use header_footer, watermark, background or bates.")),
        None => Err("\"kind\" is required: header_footer, watermark, background or bates.".into()),
    }
}

fn marks_label(a: &Value) -> String {
    let noun = kind_of(a).map_or("page marks", |k| k.noun());
    let on = on_pages(a);
    match action_of(a) {
        Ok(Action::Remove) => format!("Remove {noun} from {on}"),
        Ok(Action::Update) => format!("Update {noun} on {on}"),
        _ => format!("Add {noun} to {on}"),
    }
}

fn style_arg(a: &Value, base: Style) -> Result<Style, String> {
    let mut s = base;
    if let Some(n) = number(a, "font_size", 4.0, 144.0)? {
        s.size = n;
    }
    if let Some(c) = color_arg(a, "color")? {
        s.color = c;
    }
    if let Some(b) = a.get("bold").and_then(Value::as_bool) {
        s.bold = b;
    }
    Ok(s)
}

fn today() -> Date {
    // `D:YYYYMMDD…` (UTC)
    let s = crate::annot::pdf_now();
    let n = |r: std::ops::Range<usize>| s.get(r).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    Date { year: n(2..6), month: n(6..8) as u32, day: n(8..10) as u32 }
}

fn mark_arg(kind: MarkKind, a: &Value, pages: &[u32]) -> Result<Mark, String> {
    match kind {
        MarkKind::HeaderFooter => {
            let mut hf = HeaderFooter::default();
            let mut any = false;
            for (i, k) in POSITIONS.iter().enumerate() {
                if let Some(t) = text_arg(a, k, 200)? {
                    any |= !t.is_empty();
                    hf.text[i] = t;
                }
            }
            if !any {
                return Err("Give the text for at least one place: top_left, top_center, top_right, bottom_left, bottom_center or \
                            bottom_right (tokens: <<1>>, <<1 of n>>, <<yyyy-mm-dd>>)."
                    .into());
            }
            hf.style = style_arg(a, hf.style)?;
            Ok(Mark::HeaderFooter(hf))
        }
        MarkKind::Watermark => {
            let text = text_arg(a, "text", 80)?
                .filter(|t| !t.is_empty())
                .ok_or("\"text\" is required for a watermark: the words to stamp, e.g. DRAFT.")?;
            let mut w = Watermark { text, ..Watermark::default() };
            w.style = style_arg(a, w.style.clone())?;
            if let Some(o) = number(a, "opacity", 0.05, 1.0)? {
                w.opacity = o;
            }
            if let Some(r) = number(a, "rotation", -360.0, 360.0)? {
                w.rotation = r;
            }
            if let Some(b) = a.get("behind").and_then(Value::as_bool) {
                w.behind = b;
            }
            Ok(Mark::Watermark(w))
        }
        MarkKind::Background => {
            let color = color_arg(a, "color")?.ok_or("\"color\" is required for a background: a colour like #fff7d6.")?;
            let mut b = Background { color, ..Background::default() };
            if let Some(o) = number(a, "opacity", 0.05, 1.0)? {
                b.opacity = o;
            }
            Ok(Mark::Background(b))
        }
        MarkKind::Bates => {
            let mut b = Bates::default();
            if let Some(p) = text_arg(a, "prefix", 30)? {
                b.prefix = p;
            }
            if let Some(s) = text_arg(a, "suffix", 30)? {
                b.suffix = s;
            }
            if let Some(d) = int(a, "digits") {
                if !(3..=15).contains(&d) {
                    return Err("\"digits\" must be from 3 to 15.".into());
                }
                b.digits = d as u32;
            }
            if let Some(s) = int(a, "start") {
                if s < 0 {
                    return Err("\"start\" must be 0 or more.".into());
                }
                b.start = s as u64;
            }
            if let Some(p) = str_arg(a, "position") {
                b.slot = POSITIONS.iter().position(|k| *k == p).ok_or_else(|| format!("Unknown position \"{p}\". Use one of: {}.", POSITIONS.join(", ")))? as u8;
            } else {
                b.slot = 5;
            }
            b.style = style_arg(a, b.style)?;
            if !crate::marks::bates_fits(b.digits, b.start, pages.len() as u64) {
                return Err(format!(
                    "The numbers would run past {} digits for {}. Use more digits or a lower start.",
                    b.digits,
                    plural(pages.len(), "page")
                ));
            }
            Ok(Mark::Bates(b))
        }
    }
}

/// What the user is asked to approve, in words.
fn mark_words(m: &Mark) -> String {
    match m {
        Mark::HeaderFooter(h) => POSITIONS
            .iter()
            .zip(h.text.iter())
            .filter(|(_, t)| !t.is_empty())
            .map(|(p, t)| format!("{}: “{}”", p.replace('_', " "), clip(t, 60)))
            .collect::<Vec<_>>()
            .join(" · "),
        Mark::Watermark(w) => format!("“{}” at {}% opacity, {}°{}", clip(&w.text, 60), (w.opacity * 100.0).round(), w.rotation, if w.behind { ", behind the page" } else { "" }),
        Mark::Background(b) => format!(
            "colour #{:02x}{:02x}{:02x} at {}% opacity",
            (b.color[0] * 255.0).round() as u8,
            (b.color[1] * 255.0).round() as u8,
            (b.color[2] * 255.0).round() as u8,
            (b.opacity * 100.0).round()
        ),
        Mark::Bates(b) => format!("numbers like {} at the {}", crate::marks::bates_text(b, b.start), POSITIONS[b.slot.min(5) as usize].replace('_', " ")),
    }
}

struct Plan {
    kind: MarkKind,
    action: Action,
    pages: Vec<u32>,
    count: u32,
    mark: Option<Mark>,
}

fn plan(l: &Lane, a: &Value) -> Result<Plan, String> {
    unknown_keys(
        a,
        &[
            "kind", "action", "pages", "from", "to", "top_left", "top_center", "top_right", "bottom_left", "bottom_center", "bottom_right", "text", "color",
            "opacity", "rotation", "behind", "font_size", "bold", "prefix", "suffix", "digits", "start", "position",
        ],
        "page_marks",
    )?;
    let action = action_of(a)?;
    let kind = kind_of(a)?;
    let count = live_pages(l)?;
    let mut pages = super::actions::pages_arg(a, count)?;
    if pages.is_empty() {
        pages = (0..count).collect();
    }
    let on: Vec<u32> = l
        .eng
        .read_custom(l.doc, |d| Ok(crate::marks::info(d)?.kinds.iter().find(|k| k.kind == kind).map(|k| k.pages.clone()).unwrap_or_default()))
        .map_err(err)?;
    let present: Vec<u32> = pages.iter().copied().filter(|p| on.contains(p)).collect();
    match action {
        Action::Remove => {
            if present.is_empty() {
                return Err(format!("There is no {} on {} to remove. Nothing was changed.", kind.noun(), pages_text(&pages, count)));
            }
            pages = present;
            return Ok(Plan { kind, action, pages, count, mark: None });
        }
        Action::Add if !present.is_empty() => {
            return Err(format!(
                "There is already {} on {}. Use action \"update\" to replace it, or \"remove\" it first. Nothing was changed.",
                kind.noun(),
                pages_text(&present, count)
            ));
        }
        _ => {}
    }
    let mark = mark_arg(kind, a, &pages)?;
    Ok(Plan { kind, action, pages, count, mark: Some(mark) })
}

fn page_marks_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    let Some(l) = c.host.lane() else { return Ok(None) };
    guard(&l, Touch::Content, &[])?;
    let p = plan(&l, a)?;
    Ok(Some(match &p.mark {
        Some(m) => format!("{} · {}", mark_words(m), pages_text(&p.pages, p.count)),
        None => pages_text(&p.pages, p.count),
    }))
}

fn page_marks(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Content, &[])?;
    let p = plan(&l, a)?;
    let noun = p.kind.noun();
    let on = pages_text(&p.pages, p.count);
    let (state, said) = match (p.action, p.mark) {
        (Action::Remove, _) => {
            let (kind, pages) = (p.kind, p.pages.clone());
            let label = format!("Remove {noun} from {}", super::actions::pages_label(&p.pages));
            let st = l
                .eng
                .apply_removing(l.doc, label, l.source.clone(), move |d| {
                    crate::marks::remove(d, kind, &pages)?;
                    Ok(pages)
                })
                .map_err(err)?;
            (st, format!("Removed the {noun} from {on}."))
        }
        (action, Some(mark)) => {
            let req = AddRequest { mark, pages: p.pages.clone(), replace: action == Action::Update, date: today() };
            let verb = if action == Action::Update { "Update" } else { "Add" };
            let label = format!("{verb} {noun} {} {}", if action == Action::Update { "on" } else { "to" }, super::actions::pages_label(&p.pages));
            let st = if action == Action::Update {
                l.eng.apply_removing(l.doc, label, l.source.clone(), move |d| crate::marks::add(d, &req))
            } else {
                l.eng.apply_custom(l.doc, label, l.source.clone(), false, move |d| crate::marks::add(d, &req))
            }
            .map_err(err)?;
            (st, format!("{} the {noun} on {on}.", if action == Action::Update { "Updated" } else { "Added" }))
        }
        (_, None) => return Err("Nothing was changed.".into()),
    };
    Ok(landed(&l, state, said))
}

// ---- properties ---------------------------------------------------------------------------------

const PROPERTIES: [(&str, &str); 4] = [("title", "Title"), ("author", "Author"), ("subject", "Subject"), ("keywords", "Keywords")];

fn properties_label(a: &Value) -> String {
    let parts: Vec<String> = PROPERTIES
        .iter()
        .filter_map(|(k, _)| {
            let v = a.get(*k)?.as_str()?.trim();
            Some(if v.is_empty() { format!("clear the {k}") } else { format!("{k} “{}”", clip(v, 50)) })
        })
        .collect();
    if parts.is_empty() { "Set the document properties".into() } else { format!("Set the document {}", parts.join(", ")) }
}

fn set_document_properties(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Content, &[])?;
    unknown_keys(a, &["title", "author", "subject", "keywords"], "set_document_properties")?;
    let mut set: Vec<(&'static str, &'static str, String)> = Vec::new();
    for (arg, key) in PROPERTIES {
        if let Some(v) = text_arg(a, arg, 500)? {
            set.push((arg, key, v));
        }
    }
    if set.is_empty() {
        return Err("Give at least one of title, author, subject or keywords.".into());
    }
    let now = c.host.meta();
    let current = |arg: &str| now.iter().find(|(k, _)| *k == arg).map(|(_, v)| v.as_str()).unwrap_or("");
    if set.iter().all(|(arg, _, v)| current(arg) == v) {
        return Err("The properties already have those values. Nothing was changed.".into());
    }
    let words = properties_label(a);
    let writes: Vec<(&'static str, String)> = set.iter().map(|(_, k, v)| (*k, v.clone())).collect();
    let st = l
        .eng
        .apply_custom(l.doc, "Edit document properties".into(), l.source.clone(), false, move |d| {
            let mut info = match d.trailer()?.get_dict("Info")?.filter(|i| i.is_dict().unwrap_or(false)) {
                Some(i) => i,
                None => {
                    let i = d.add_object(&d.new_dict()?)?;
                    d.trailer()?.dict_put("Info", i.clone())?;
                    i
                }
            };
            for (key, v) in &writes {
                if v.is_empty() {
                    if info.get_dict(*key)?.is_some() {
                        info.dict_delete(*key)?;
                    }
                } else {
                    info.dict_put(*key, a11y::text_obj(d, v)?)?;
                }
            }
            Ok(Vec::new())
        })
        .map_err(err)?;
    Ok(landed(&l, st, format!("{words}.")))
}

// ---- editing text ---------------------------------------------------------------------------------

fn edit_text_label(a: &Value) -> String {
    let page = int(a, "page").map(|p| format!(" on p. {p}")).unwrap_or_default();
    match (a.get("find").and_then(Value::as_str), a.get("replace").and_then(Value::as_str)) {
        (Some(f), Some("")) => format!("Delete “{}”{page}", clip(f, 50)),
        (Some(f), Some(r)) => format!("Replace “{}” with “{}”{page}", clip(f, 40), clip(r, 40)),
        _ => format!("Edit text{page}"),
    }
}

struct TextPlan {
    page: u32,
    rev: u32,
    index: u32,
    change: BlockChange,
    find: String,
    replace: String,
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plan_text(l: &Lane, a: &Value) -> Result<TextPlan, String> {
    unknown_keys(a, &["page", "find", "replace"], "edit_text")?;
    let page = page_of(a, "page", live_pages(l)?)?;
    let find = a.get("find").and_then(Value::as_str).filter(|f| !f.trim().is_empty()).ok_or("\"find\" is required: the exact text to replace.")?;
    if find.chars().count() > 500 {
        return Err("\"find\" is too long (at most 500 characters).".into());
    }
    let replace = a.get("replace").and_then(Value::as_str).ok_or("\"replace\" is required (an empty string deletes the text).")?;
    if replace.chars().count() > 2000 {
        return Err("\"replace\" is too long (at most 2000 characters).".into());
    }
    if find == replace {
        return Err("\"find\" and \"replace\" are the same. Nothing was changed.".into());
    }
    let blocks = textedit::blocks(l.eng, l.doc, page).map_err(err)?;
    let count = |text: &str, what: &str| text.matches(what).count();
    let mut normalized = false;
    let mut needle = find.to_string();
    let mut hits: Vec<(usize, usize)> =
        blocks.blocks.iter().enumerate().map(|(i, b)| (i, count(&b.text, &needle))).filter(|(_, n)| *n > 0).collect();
    if hits.is_empty() {
        // the line breaks of a block are the page's, not the reader's
        needle = collapse(find);
        hits = blocks.blocks.iter().enumerate().map(|(i, b)| (i, count(&collapse(&b.text), &needle))).filter(|(_, n)| *n > 0).collect();
        normalized = !hits.is_empty();
    }
    let total: usize = hits.iter().map(|(_, n)| n).sum();
    if total == 0 {
        let hint = match blocks.areas.first() {
            Some(area) => format!(" The page also has text that cannot be edited: {}.", area.reason),
            None => String::new(),
        };
        return Err(format!(
            "“{}” is not in the editable text of p. {}. Check the spelling against get_text; text in a scanned picture needs ocr_document first.{hint}",
            clip(find, 80),
            page + 1
        ));
    }
    if total > 1 {
        return Err(format!(
            "“{}” appears {total} times on p. {}. Give a longer \"find\" that appears once (a few more words around it).",
            clip(find, 80),
            page + 1
        ));
    }
    let b = &blocks.blocks[hits[0].0];
    if !b.editable {
        return Err(format!("That text cannot be edited: {}. Tell the user.", b.reason.as_deref().unwrap_or("its font does not allow it")));
    }
    let uniform = b
        .runs
        .windows(2)
        .all(|w| w[0].font == w[1].font && w[0].size == w[1].size && w[0].color == w[1].color && w[0].bold == w[1].bold && w[0].italic == w[1].italic);
    let change = if !normalized && !uniform {
        // the replaced text has to sit inside one run, so every run keeps its own style
        let k = b.runs.iter().position(|r| r.text.contains(&needle)).ok_or(
            "That text spans parts of the block that are formatted differently. Replace a shorter piece that sits in one style, or tell the user to edit it by hand.",
        )?;
        let runs = b
            .runs
            .iter()
            .enumerate()
            .map(|(i, r)| RunChange { source: Some(i), text: (i == k).then(|| r.text.replacen(&needle, replace, 1)), ..Default::default() })
            .collect();
        BlockChange { runs: Some(runs), ..Default::default() }
    } else {
        let base = if normalized { collapse(&b.text) } else { b.text.clone() };
        BlockChange { text: Some(base.replacen(&needle, replace, 1)), ..Default::default() }
    };
    Ok(TextPlan { page, rev: blocks.rev, index: b.index, change, find: find.to_string(), replace: replace.to_string() })
}

fn edit_text_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    let Some(l) = c.host.lane() else { return Ok(None) };
    guard(&l, Touch::Content, &[])?;
    let p = plan_text(&l, a)?;
    Ok(Some(format!("p. {}: “{}” → “{}”", p.page + 1, clip(&p.find, 80), clip(&p.replace, 80))))
}

fn edit_text(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Content, &[])?;
    let p = plan_text(&l, a)?;
    let done = textedit::edit(l.eng, l.doc, p.rev, p.page, p.index, &p.change, l.source.clone()).map_err(err)?;
    let mut said = format!("Replaced “{}” with “{}” on p. {}.", clip(&p.find, 60), clip(&p.replace, 60), p.page + 1);
    if let Some(f) = done.fallbacks.first() {
        let _ = write!(said, " The block's font could not show the new text, so {} was used ({}). Tell the user.", f.font, f.reason);
    }
    Ok(landed(&l, done.state, said))
}

// ---- sanitize -----------------------------------------------------------------------------------

fn sanitize_opts(a: &Value) -> Result<Sanitize, String> {
    const KEYS: [&str; 7] = ["metadata", "hiddenLayers", "hiddenText", "scriptsActions", "embeddedFiles", "annotations", "bookmarks"];
    unknown_keys(a, &KEYS, "sanitize_document")?;
    let mut s = Sanitize {
        metadata: true,
        hidden_layers: true,
        hidden_text: false,
        scripts_actions: true,
        embedded_files: true,
        annotations: false,
        form_fields_under_marks: false,
        bookmarks: false,
    };
    for k in KEYS {
        let Some(v) = a.get(k) else { continue };
        let b = v.as_bool().ok_or_else(|| format!("\"{k}\" must be true or false."))?;
        match k {
            "metadata" => s.metadata = b,
            "hiddenLayers" => s.hidden_layers = b,
            "hiddenText" => s.hidden_text = b,
            "scriptsActions" => s.scripts_actions = b,
            "embeddedFiles" => s.embedded_files = b,
            "annotations" => s.annotations = b,
            _ => s.bookmarks = b,
        }
    }
    Ok(s)
}

fn sanitize_words(s: &Sanitize) -> String {
    let all = [
        (s.metadata, "metadata"),
        (s.hidden_layers, "hidden layers"),
        (s.hidden_text, "hidden text (also the text layer of scans)"),
        (s.scripts_actions, "scripts and actions"),
        (s.embedded_files, "attached files"),
        (s.annotations, "ALL comments and markup"),
        (s.bookmarks, "bookmarks"),
    ];
    let on: Vec<&str> = all.iter().filter(|(b, _)| *b).map(|(_, n)| *n).collect();
    if on.is_empty() { "nothing".into() } else { on.join(", ") }
}

fn sanitize_label(a: &Value) -> String {
    match sanitize_opts(a) {
        Ok(s) => format!("Remove hidden information: {}", sanitize_words(&s)),
        Err(_) => "Remove hidden information".into(),
    }
}

fn sanitize_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    if let Some(l) = c.host.lane() {
        guard(&l, Touch::Content, &[])?;
    }
    let s = sanitize_opts(a)?;
    if s == Sanitize::NONE {
        return Err("Every option is off, so nothing would be removed.".into());
    }
    Ok(None)
}

fn sanitize_document(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Content, &[])?;
    let opts = sanitize_opts(a)?;
    let (state, r) = crate::redact::app::sanitize_doc(l.eng, l.doc, opts, l.source.clone()).map_err(err)?;
    let mut gone = Vec::new();
    for (n, what) in [
        (r.metadata, "metadata entries"),
        (r.hidden_layer_blocks, "blocks of hidden-layer content"),
        (r.hidden_text_pages, "pages of hidden text"),
        (r.scripts_actions, "scripts and actions"),
        (r.embedded_files, "attached files"),
        (r.annotations, "comments and markup"),
        (r.bookmarks, "bookmarks"),
    ] {
        if n > 0 {
            gone.push(format!("{n} {what}"));
        }
    }
    Ok(landed(&l, state, format!("Removed {}. The file on disk changes only when the user saves.", gone.join(", "))))
}

// ---- accessibility --------------------------------------------------------------------------------

fn check_accessibility(c: &Ctx, _: &Value) -> Result<Output, String> {
    let l = engine(c)?;
    let mut last = u32::MAX;
    let report = a11y::check_doc(
        l.eng,
        l.doc,
        &mut |p| {
            if p.done != last {
                last = p.done;
                (c.progress)(format!("Checking page {} of {}", p.done.max(1), p.pages));
            }
        },
        c.cancel,
    )
    .map_err(err)?;
    let count = |s: a11y::Status| report.results.iter().filter(|r| r.status == s).count();
    let mut s = format!(
        "Accessibility check: {}, {}: {} failed, {} to check by hand, {} passed.\n",
        plural(report.pages as usize, "page"),
        if report.tagged { "tagged" } else { "not tagged" },
        count(a11y::Status::Failed),
        count(a11y::Status::Manual),
        count(a11y::Status::Passed)
    );
    let mut body = String::new();
    for r in report.results.iter().filter(|r| r.status == a11y::Status::Failed) {
        let _ = writeln!(body, "- {} [{}] ({}): {}", r.name, r.rule.id(), r.group, r.hint);
        if let Some(f) = r.fix {
            let _ = writeln!(body, "  fix_accessibility can repair this: {f}");
        }
        for f in r.findings.iter().take(6) {
            let at = f.page.map(|p| format!("p. {} ", p + 1)).unwrap_or_default();
            let what = f.object.as_deref().unwrap_or("");
            let x = f.xref.map(|x| format!(" (xref {x})")).unwrap_or_default();
            let _ = writeln!(body, "  - {at}{what}{x} {}", clip(&f.message, 160));
        }
        if r.findings.len() > 6 {
            let _ = writeln!(body, "  - … and {} more", r.findings.len() - 6);
        }
    }
    let manual: Vec<&str> = report.results.iter().filter(|r| r.status == a11y::Status::Manual).map(|r| r.name).collect();
    if body.len() > MAX_REPORT {
        body = clip(&body, MAX_REPORT);
        body.push_str("\n(the rest is not shown)");
    }
    if !body.is_empty() {
        s.push_str(&data_block(&body));
        s.push('\n');
    }
    if !manual.is_empty() {
        let _ = writeln!(s, "Needs a person to judge: {}.", manual.join(", "));
    }
    if count(a11y::Status::Failed) == 0 {
        s.push_str("No rule fails.\n");
    }
    Ok(Output::text(s))
}

fn fix_key(s: &str) -> String {
    s.chars().filter(|c| *c != '_' && *c != '-').collect::<String>().to_lowercase()
}

fn fixes_arg(a: &Value) -> Result<Vec<Fix>, String> {
    unknown_keys(a, &["fixes"], "fix_accessibility")?;
    let list = a.get("fixes").and_then(Value::as_array).filter(|l| !l.is_empty()).ok_or("Give \"fixes\": a list of { fix, ... } objects.")?;
    if list.len() > 60 {
        return Err("At most 60 fixes in one call.".into());
    }
    let mut out = Vec::new();
    for f in list {
        let name = f.get("fix").and_then(Value::as_str).ok_or("Each fix needs a \"fix\" name.")?;
        let fix = match fix_key(name).as_str() {
            "title" => {
                let t = text_arg(f, "title", 300)?.filter(|t| !t.is_empty()).ok_or("The title fix needs \"title\".")?;
                Fix::Title { title: Some(t) }
            }
            "language" => {
                let l = text_arg(f, "lang", 35)?.filter(|t| !t.is_empty()).ok_or("The language fix needs \"lang\" (for example en-US).")?;
                Fix::Language { lang: l }
            }
            "displaydoctitle" => Fix::DisplayDocTitle,
            "taborder" => Fix::TabOrder,
            "fieldtooltips" => Fix::FieldTooltips,
            "autotag" => Fix::Autotag,
            "figurealt" => {
                let xref = int(f, "xref").filter(|x| *x > 0 && *x < i32::MAX as i64).ok_or("The figure_alt fix needs \"xref\" from check_accessibility.")?;
                let alt = text_arg(f, "alt", 500)?.filter(|t| !t.is_empty()).ok_or("The figure_alt fix needs \"alt\": one sentence describing the figure.")?;
                Fix::FigureAlt { xref: xref as i32, alt }
            }
            o => {
                return Err(format!(
                    "Unknown fix \"{o}\". Use title, language, display_doc_title, tab_order, field_tooltips, figure_alt or autotag."
                ))
            }
        };
        out.push(fix);
    }
    Ok(out)
}

fn fix_label(a: &Value) -> String {
    match fixes_arg(a) {
        Ok(f) if f.len() == 1 => f[0].label(),
        Ok(f) => {
            let mut words: Vec<String> = f.iter().map(Fix::label).collect();
            words.dedup();
            format!("Fix {}: {}", plural(f.len(), "accessibility issue"), clip(&words.join(", "), 120))
        }
        Err(_) => "Fix accessibility issues".into(),
    }
}

fn fix_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    if let Some(l) = c.host.lane() {
        guard(&l, Touch::Content, &[])?;
    }
    let fixes = fixes_arg(a)?;
    let alts: Vec<String> = fixes
        .iter()
        .filter_map(|f| match f {
            Fix::FigureAlt { xref, alt } => Some(format!("figure {xref}: “{}”", clip(alt, 60))),
            Fix::Title { title: Some(t) } => Some(format!("title: “{}”", clip(t, 60))),
            _ => None,
        })
        .collect();
    Ok((!alts.is_empty()).then(|| alts.join(" · ")))
}

fn fix_accessibility(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Content, &[])?;
    let fixes = fixes_arg(a)?;
    let said = fixes.iter().map(Fix::label).collect::<Vec<_>>().join(", ");
    let fix = if fixes.len() == 1 { fixes[0].clone() } else { Fix::Many { fixes } };
    let st = a11y::fix_doc_with(
        l.eng,
        l.doc,
        &fix,
        l.source.clone(),
        &mut |p| (c.progress)(format!("Tagging page {} of {}", p.done.max(1), p.pages)),
        c.cancel,
    )
    .map_err(err)?;
    Ok(landed(&l, st, format!("Applied: {said}.")))
}

// ---- preflight --------------------------------------------------------------------------------------

pub(super) fn profile_arg(a: &Value) -> Result<crate::preflight::Profile, String> {
    let name = str_arg(a, "profile").ok_or("\"profile\" is required: pdfa-1b, pdfa-2b, pdfa-3b, pdfa-2u, pdfua-1 or pdfx-4.")?;
    serde_json::from_value(json!(name)).map_err(|_| format!("Unknown profile \"{name}\". Use pdfa-1b, pdfa-2b, pdfa-3b, pdfa-2u, pdfua-1 or pdfx-4."))
}

fn check_preflight(c: &Ctx, a: &Value) -> Result<Output, String> {
    use crate::preflight::Status;
    let l = engine(c)?;
    let profile = profile_arg(a)?;
    let mut last = u32::MAX;
    let report = crate::preflight::check_doc(
        l.eng,
        l.doc,
        profile,
        &mut |p| {
            if p.done != last {
                last = p.done;
                (c.progress)(format!("Checking page {} of {}", p.done.max(1), p.pages));
            }
        },
        c.cancel,
    )
    .map_err(err)?;
    let mut s = format!("{} check of {}: ", profile.label(), plural(report.pages as usize, "page"));
    if report.conforms {
        s.push_str("no rule fails");
    } else {
        let _ = write!(s, "{} fail", plural(report.failed(), "rule"));
    }
    match &report.claimed {
        Some(claim) => {
            let _ = writeln!(s, ". The file claims {claim}.");
        }
        None => s.push_str(". The file claims no standard.\n"),
    }
    let mut body = String::new();
    for r in report.results.iter().filter(|r| r.status == Status::Failed) {
        let _ = writeln!(body, "- {} ({}, {}): {} finding{}", r.name, r.group, r.clause, r.count, if r.count == 1 { "" } else { "s" });
        for f in r.findings.iter().take(3) {
            let at = f.page.map(|p| format!("p. {} ", p + 1)).unwrap_or_default();
            let what = f.object.as_deref().unwrap_or("");
            let how = if let Some(n) = &f.note {
                format!(" [cannot be fixed: {n}]")
            } else if f.new_file {
                " [fixable in a new file]".to_string()
            } else if f.fixable {
                " [fixable]".to_string()
            } else {
                String::new()
            };
            let _ = writeln!(body, "  - {at}{what} {}{how}", clip(&f.message, 160));
        }
    }
    if body.len() > MAX_REPORT {
        body = clip(&body, MAX_REPORT);
        body.push_str("\n(the rest is not shown)");
    }
    if !body.is_empty() {
        s.push_str(&data_block(&body));
        s.push('\n');
    }
    if !report.plan.groups.is_empty() {
        let g: Vec<String> = report
            .plan
            .groups
            .iter()
            .filter(|g| g.issues > 0)
            .map(|g| format!("{} ({}{})", g.label, plural(g.issues as usize, "issue"), g.disabled.as_ref().map(|d| format!(", unavailable: {d}")).unwrap_or_default()))
            .collect();
        if !g.is_empty() {
            let _ = writeln!(s, "A conversion could settle: {}.", g.join(", "));
        }
        for r in &report.plan.remaining {
            let _ = writeln!(s, "It could not settle {}: {}.", plural(r.count as usize, "finding"), r.text);
        }
    }
    let manual = report.results.iter().filter(|r| r.status == Status::Manual).count();
    if manual > 0 {
        let _ = writeln!(s, "{} need a person's check.", plural(manual, "rule"));
    }
    Ok(Output::text(s))
}

// ---- opening a tool -------------------------------------------------------------------------------

#[cfg(test)]
mod tests;

fn open_tool(_: &Ctx, a: &Value) -> Result<Output, String> {
    let t = str_arg(a, "tool").ok_or("\"tool\" is required.")?;
    if !OPEN_TOOLS.contains(&t) {
        return Err(format!("There is no tool \"{t}\" to open. Use one of: {}.", OPEN_TOOLS.join(", ")));
    }
    Ok(Output {
        text: format!("Opened the {t} tool for the user. They choose and apply the settings there; tell them what to do."),
        ui: Some(UiAction::OpenTool { tool: t.to_string() }),
        ..Default::default()
    })
}
