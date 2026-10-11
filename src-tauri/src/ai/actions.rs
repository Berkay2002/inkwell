//! The assistant's changing tools: redaction, comments, form filling and page organizing. Each one
//! goes through the document's own edit lane (docs/engine-rules.md 1-3) under
//! `EditSource::Assistant { turn }`, so a call is one undo step and a whole turn undoes as a unit,
//! and returns `Err` with a sentence the model can act on when it changed nothing.
//!
//! The classes (`tools.rs`): marking, commenting, filling and organizing are `Edit`; deleting
//! pages is `Destructive`; applying a redaction is `Permanent` and asks in every mode. The permission
//! check itself is the turn's gate, never a tool. Nothing here is provider-specific.
//!
//! Document text that comes back to the model (hit text, comments, field values) is data: it sits
//! between `<<<DOCUMENT` / `DOCUMENT>>>` markers and is defanged like every excerpt.

use super::text::defang;
use super::tools::{Class, Ctx, Lane, Output, Tool, UiAction, clip, int, page_list_label, pages_requested, tool};
use crate::annot::{self, AnnotDto, Kind, NewAnnot, Patch, Style};
use crate::engine::EngineError;
use crate::engine::edit::{EditOp, EditState};
use crate::forms::{self, FieldDto, FieldKind, FieldValue};
use crate::redact::app::{ApplyOutcome, FindQuery, HitDto, MarkDto};
use crate::redact::{Pattern, Sanitize};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

/// The author of comments the assistant adds.
const AUTHOR: &str = "Assistant";
/// More hits than this in one call is a sign of a too-loose query, not something to mark blindly.
const MAX_HITS: usize = 500;
const MAX_LIST: usize = 150;
/// A regex longer than this is not something a person asked for.
const MAX_REGEX: usize = 200;

/// Said after every change: the turn's text cache is from before it.
const STALE: &str = "Text and search results you read earlier in this turn show the document as it was before \
                     this change; ask the user to ask again if you need the new text.";

const SIGNED: &str = "This document is digitally signed, so this change would invalidate the signature. Tell the user; \
                      they can save a copy of the file and make the change there.";

pub(super) fn err(e: EngineError) -> String {
    match e {
        EngineError::Stale => "The document changed while this ran. Read it again and retry.".into(),
        EngineError::NoDoc => "The document was closed.".into(),
        e => e.to_string(),
    }
}

pub(super) fn lane<'a>(c: &'a Ctx) -> Result<Lane<'a>, String> {
    c.host.lane().ok_or_else(|| "This document cannot be changed from here.".to_string())
}

pub(super) fn state(l: &Lane) -> Result<EditState, String> {
    l.eng.edit_state(l.doc).map_err(err)
}

const REWRITE: &str = "This document is digitally signed and saving it would have to rewrite the whole file (Inkwell had \
                        to repair it when opening it, or it was redacted in this session), which destroys the \
                        signature. Tell the user; they can save a copy of the file and make the change there.";

/// What a tool does to a signed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Touch {
    /// Page content, structure, metadata or hidden information: any signature breaks.
    Content,
    /// Comments and markup: allowed unless the document is certified for less than annotations.
    Annotations,
    /// Form fields: allowed unless certified against all change or locked by a signature.
    Fields,
}

/// Refuses a change a signature forbids, with a sentence the model relays. `fields` are the fully
/// qualified names of the form fields a `Touch::Fields` change would fill. Run before the user is
/// asked (the tool's `check`) and again when the change is made.
pub(super) fn guard(l: &Lane, touch: Touch, fields: &[String]) -> Result<(), String> {
    let s = l.eng.read_custom(l.doc, |d| Ok(crate::signed::inspect(d))).map_err(err)?;
    if !s.signed {
        return Ok(());
    }
    // repaired on open, or redacted in this session: either way every save is a full rewrite
    if s.rewrite || l.eng.edit_state(l.doc).map_err(err)?.redacted {
        return Err(REWRITE.into());
    }
    match touch {
        Touch::Content => Err(SIGNED.into()),
        Touch::Annotations if s.certified.is_some_and(|p| p <= 2) => Err(
            "This document is certified so that comments are not allowed after signing, and adding one would invalidate \
             the signature. Tell the user; they can save a copy of the file and make the change there."
                .into(),
        ),
        Touch::Fields if s.certified == Some(1) => Err(
            "This document is certified so that no change at all is allowed after signing, so filling a field would \
             invalidate the signature. Tell the user; they can save a copy of the file and fill that."
                .into(),
        ),
        Touch::Fields => match fields.iter().find(|f| s.locks_field(f)) {
            Some(f) => Err(format!(
                "The field \"{f}\" is locked by a signature, so changing it would invalidate the signature. Nothing was changed; tell the user."
            )),
            None => Ok(()),
        },
        Touch::Annotations => Ok(()),
    }
}

/// The pre-approval check of a tool that only needs the signature guard.
pub(super) fn guard_check(touch: Touch) -> impl Fn(&Ctx, &Value) -> Result<Option<String>, String> + Send + Sync + 'static {
    move |c, _| match c.host.lane() {
        Some(l) => guard(&l, touch, &[]).map(|()| None),
        None => Ok(None),
    }
}

/// What a landed change returns: the text for the model and the new state for the UI.
pub(super) fn landed(l: &Lane, state: EditState, mut text: String) -> Output {
    l.edited.store(true, std::sync::atomic::Ordering::SeqCst);
    text.push(' ');
    text.push_str(STALE);
    Output { text, ui: Some(UiAction::Edited { state, redacted: None }), ..Default::default() }
}

pub(super) fn plural(n: usize, one: &str) -> String {
    if n == 1 { format!("{n} {one}") } else { format!("{n} {one}s") }
}

/// Document text as data.
pub(super) fn data_block(lines: &str) -> String {
    format!("<<<DOCUMENT\n{}\nDOCUMENT>>>", defang(lines.trim_end()))
}

pub(super) fn one_line(s: &str, max: usize) -> String {
    clip(&s.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

pub(super) fn str_arg<'a>(a: &'a Value, key: &str) -> Option<&'a str> {
    a.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

pub(super) fn bool_arg(a: &Value, key: &str, default: bool) -> bool {
    a.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// `pages` / `from`-`to` as 0-based indices, checked against the document. Empty: not given.
pub(super) fn pages_arg(a: &Value, count: u32) -> Result<Vec<u32>, String> {
    let mut out = Vec::new();
    for p in pages_requested(a) {
        if p < 1 || p > count as i64 {
            return Err(format!("Page {p} does not exist: the document has {}.", plural(count as usize, "page")));
        }
        if !out.contains(&(p as u32 - 1)) {
            out.push(p as u32 - 1);
        }
    }
    out.sort_unstable();
    Ok(out)
}

pub(super) fn pages_label(pages: &[u32]) -> String {
    page_list_label(&pages.iter().map(|p| *p as i64 + 1).collect::<Vec<_>>())
}

/// "p3#41": a mark, comment or field on page 3 with object number 41, as `list_*` tools print them.
fn id_of(page: u32, xref: i32) -> String {
    format!("p{}#{xref}", page + 1)
}

fn parse_id(s: &str) -> Option<(u32, i32)> {
    let (p, x) = s.trim().strip_prefix('p')?.split_once('#')?;
    Some((p.parse::<u32>().ok()?.checked_sub(1)?, x.parse().ok()?))
}

/// A position given as fractions of the page ("0.5" = the middle), the way the model sees a page image.
fn fraction(a: &Value, key: &str) -> Result<f32, String> {
    let v = a.get(key).and_then(Value::as_f64).ok_or_else(|| format!("\"{key}\" is required: a number from 0 to 1."))?;
    if !(0.0..=1.0).contains(&v) {
        return Err(format!("\"{key}\" must be between 0 and 1 (a fraction of the page), not {v}."));
    }
    Ok(v as f32)
}

/// The same hit, covered by a mark that is already there?
fn covered(h: &HitDto, marks: &[MarkDto]) -> bool {
    h.quads.iter().all(|q| {
        let (cx, cy) = ((q[0] + q[2] + q[4] + q[6]) / 4.0, (q[1] + q[3] + q[5] + q[7]) / 4.0);
        marks.iter().filter(|m| m.page == h.page).any(|m| {
            m.quads.iter().any(|m| {
                let (x0, x1) = (m[0].min(m[2]).min(m[4]).min(m[6]), m[0].max(m[2]).max(m[4]).max(m[6]));
                let (y0, y1) = (m[1].min(m[3]).min(m[5]).min(m[7]), m[1].max(m[3]).max(m[5]).max(m[7]));
                cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1
            })
        })
    })
}

fn find_hits(l: &Lane, q: &FindQuery, pages: &[u32]) -> Result<Vec<HitDto>, String> {
    let mut hits = crate::redact::app::find(l.eng, l.doc, q).map_err(err)?;
    if !pages.is_empty() {
        hits.retain(|h| pages.contains(&h.page));
    }
    Ok(hits)
}

fn pattern_arg(name: &str) -> Result<Pattern, String> {
    serde_json::from_value(json!(name)).map_err(|_| {
        format!("Unknown pattern \"{name}\". Use one of: email, phone, personnummer, ssn, iban, card.")
    })
}

/// What the query of a redaction or markup tool asks for (exactly one of text, pattern, regex).
fn query_arg(a: &Value) -> Result<(FindQuery, String), String> {
    query_with(a, "text")
}

/// The same, where the literal text is called `key` (add_comment's own "text" is the comment's words, so its
/// literal is "find").
fn query_with(a: &Value, key: &str) -> Result<(FindQuery, String), String> {
    let (text, pattern, regex) = (str_arg(a, key), str_arg(a, "pattern"), str_arg(a, "regex"));
    let case_sensitive = bool_arg(a, "case_sensitive", false);
    match (text, pattern, regex) {
        (Some(t), None, None) => Ok((
            FindQuery::Text { needle: t.to_string(), case_sensitive, whole_word: bool_arg(a, "whole_word", false) },
            format!("“{}”", one_line(t, 40)),
        )),
        (None, Some(p), None) => Ok((FindQuery::Pattern { pattern: pattern_arg(p)? }, format!("every {p}"))),
        (None, None, Some(r)) => {
            if r.chars().count() > MAX_REGEX {
                return Err(format!("The regex is too long (the limit is {MAX_REGEX} characters). Use a shorter or more specific one."));
            }
            let limit = crate::redact::REGEX_SIZE_LIMIT;
            regex::RegexBuilder::new(r).size_limit(limit).dfa_size_limit(limit).build().map_err(|e| format!("The regex does not compile: {e}"))?;
            Ok((FindQuery::Regex { pattern: r.to_string(), case_sensitive }, format!("/{}/", one_line(r, 40))))
        }
        (None, None, None) => Err(format!("Give one of \"{key}\", \"pattern\" or \"regex\".")),
        _ => Err(format!("Give only one of \"{key}\", \"pattern\" and \"regex\".")),
    }
}

/// `abcdef` -> `ab…ef`: enough for the user to recognise a match, not the whole of it.
fn mask(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= 6 {
        return "…".to_string();
    }
    format!("{}…{}", c[..2].iter().collect::<String>(), c[c.len() - 2..].iter().collect::<String>())
}

/// "p. 2: 3 matches" per page, in page order.
fn per_page(hits: &[HitDto], sample: bool) -> String {
    let mut pages: Vec<u32> = hits.iter().map(|h| h.page).collect();
    pages.sort_unstable();
    pages.dedup();
    let mut s = String::new();
    for p in pages {
        let on: Vec<&HitDto> = hits.iter().filter(|h| h.page == p).collect();
        let _ = write!(s, "- p. {}: {}", p + 1, plural(on.len(), "match").replace("matchs", "matches"));
        if sample {
            let shown: Vec<String> = on.iter().take(3).map(|h| format!("“{}”", defang(&mask(&one_line(&h.text, 60))))).collect();
            let _ = write!(s, " ({}{})", shown.join(", "), if on.len() > 3 { ", …" } else { "" });
        }
        s.push('\n');
    }
    s
}

// ---- the tools --------------------------------------------------------------------------------

pub fn tools() -> Vec<Arc<Tool>> {
    let page = |what: &str| json!({ "type": "integer", "minimum": 1, "description": what });
    let pages = json!({ "type": "array", "items": { "type": "integer", "minimum": 1 }, "description": "1-based page numbers" });
    let find_props = |extra: Value| {
        let mut p = json!({
            "text": { "type": "string", "description": "a word or phrase (matched across line breaks)" },
            "pattern": { "type": "string", "enum": ["email", "phone", "personnummer", "ssn", "iban", "card"],
                         "description": "a validated pattern: far fewer false matches than a regex" },
            "regex": { "type": "string", "description": "Rust regex syntax, for anything else (dates, reference numbers)" },
            "case_sensitive": { "type": "boolean", "description": "text and regex only; default false" },
            "whole_word": { "type": "boolean", "description": "text only: do not match inside a longer word; default false" },
            "pages": pages.clone(),
        });
        if let (Some(p), Some(e)) = (p.as_object_mut(), extra.as_object()) {
            p.extend(e.clone());
        }
        p
    };
    vec![
        tool(
            "redact_find_and_mark",
            "Marks text for redaction: a black box over every match, applied only later by redact_apply, so nothing is \
             removed yet and the user can review the marks in the Redact panel. Give exactly one of \"text\" (a word or \
             phrase, e.g. the user's name), \"pattern\" (email, phone, personnummer, ssn, iban, card) or \"regex\"; \
             \"pages\" limits it to some pages. Always mark first and let the user review; apply only when they ask \
             you to. Returns how many matches were marked on which pages. Changes the document (the user approves it).",
            json!({ "type": "object", "additionalProperties": false, "properties": find_props(json!({})) }),
            Class::Edit,
            |a| match query_arg(a) {
                Ok((_, what)) => format!("Mark {what} for redaction"),
                Err(_) => "Mark text for redaction".into(),
            },
            redact_find_and_mark,
        )
        .checked(guard_check(Touch::Annotations)),
        tool(
            "redact_mark_area",
            "Marks a rectangle of a page for redaction, for what is not text (a signature, a photo, a stamp). Give the \
             rectangle as fractions of the page as you see it in get_page_image: left and right of the width, top and \
             bottom of the height (0 is the left or top edge, 1 the right or bottom). Marks it only; redact_apply \
             removes the content later. Changes the document (the user approves it).",
            json!({ "type": "object", "additionalProperties": false, "required": ["page", "left", "top", "right", "bottom"], "properties": {
                "page": page("1-based page number"),
                "left": { "type": "number", "minimum": 0, "maximum": 1 },
                "top": { "type": "number", "minimum": 0, "maximum": 1 },
                "right": { "type": "number", "minimum": 0, "maximum": 1 },
                "bottom": { "type": "number", "minimum": 0, "maximum": 1 },
            } }),
            Class::Edit,
            |a| format!("Mark an area of p. {} for redaction", int(a, "page").unwrap_or(0)),
            redact_mark_area,
        )
        .checked(guard_check(Touch::Annotations)),
        tool(
            "redact_list_marks",
            "Lists the redaction marks in the document with their ids (like p3#41), page and marked text. Use it to \
             review before redact_apply or to find what to remove with redact_unmark.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            Class::Read,
            |_| "Listed the redaction marks".into(),
            redact_list_marks,
        ),
        tool(
            "redact_unmark",
            "Removes redaction marks (nothing was removed from the document yet, so this only takes boxes away). Give \
             \"marks\" (ids from redact_list_marks) or \"all\": true. Changes the document (the user approves it).",
            json!({ "type": "object", "additionalProperties": false, "properties": {
                "marks": { "type": "array", "items": { "type": "string" }, "description": "ids like p3#41" },
                "all": { "type": "boolean" },
            } }),
            Class::Edit,
            |a| match a.get("marks").and_then(Value::as_array) {
                _ if bool_arg(a, "all", false) => "Remove all redaction marks".into(),
                Some(m) if !m.is_empty() => format!("Remove {} redaction mark{}", m.len(), if m.len() == 1 { "" } else { "s" }),
                _ => "Remove redaction marks".into(),
            },
            redact_unmark,
        )
        .checked(guard_check(Touch::Annotations)),
        tool(
            "redact_apply",
            "PERMANENTLY redacts every marked item: the marked text, images and page content are removed from the \
             file, the document is rewritten and checked, and this cannot be undone in the saved file. The user is \
             asked to approve every time, in every mode. Call redact_list_marks first and pass \"expected_marks\" (how \
             many marks there are), \"pages\" (the pages that have marks) and \"marks_digest\" exactly as listed; it \
             refuses if the marks are not exactly those, so the user approves exactly what happens (the card shows \
             the first marked texts, and what else is removed with them). \"sanitize\" chooses the hidden \
             information to strip as well (metadata, hiddenLayers, hiddenText, scriptsActions, embeddedFiles, \
             annotations, formFieldsUnderMarks, bookmarks): all are on by default, so turn off only what the user \
             wants kept (annotations removes their comments). The file on disk is not overwritten until the user \
             saves; suggest Save as to keep the original.",
            json!({ "type": "object", "additionalProperties": false, "required": ["expected_marks", "pages", "marks_digest"], "properties": {
                "expected_marks": { "type": "integer", "minimum": 1 },
                "pages": pages.clone(),
                "marks_digest": { "type": "string", "description": "the digest redact_list_marks printed" },
                "sanitize": { "type": "object", "additionalProperties": false, "properties": {
                    "metadata": { "type": "boolean" }, "hiddenLayers": { "type": "boolean" },
                    "hiddenText": { "type": "boolean" }, "scriptsActions": { "type": "boolean" },
                    "embeddedFiles": { "type": "boolean" }, "annotations": { "type": "boolean" },
                    "formFieldsUnderMarks": { "type": "boolean" }, "bookmarks": { "type": "boolean" } } },
            } }),
            Class::Permanent,
            |a| {
                let n = int(a, "expected_marks").unwrap_or(0).max(0) as usize;
                let on = pages_requested(a);
                let what = if n == 0 { "the marks".to_string() } else { plural(n, "mark") };
                let also = sanitize_arg(a).map(|s| sanitize_words(&s)).unwrap_or_default();
                if on.is_empty() {
                    format!("Permanently redact {what}{also}")
                } else {
                    format!("Permanently redact {what} on {}{also}", page_list_label(&on))
                }
            },
            redact_apply,
        )
        .checked(apply_check),
        tool(
            "list_comments",
            "Lists the comments and markups in the document (highlights, notes, text boxes, replies) with their ids \
             (like p3#41), page, author and text.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            Class::Read,
            |_| "Listed the comments".into(),
            list_comments,
        ),
        tool(
            "add_comment",
            "Adds a comment. \"kind\" is highlight, underline, strikeout or squiggly (marks every match of \"find\" \
             on \"pages\", or of a \"pattern\" or \"regex\" given instead, such as every date, all in one step; or \
             just the first with \"first_only\"; \"text\" is an optional note attached to it), note \
             (a sticky note with \"text\" at \"find\" or at \"page\" with \"x\" and \"y\" as fractions of the page) or \
             reply (\"text\" answers the comment \"parent\" from list_comments). Changes the document (the user \
             approves it).",
            json!({ "type": "object", "additionalProperties": false, "required": ["kind"], "properties": {
                "kind": { "type": "string", "enum": ["highlight", "underline", "strikeout", "squiggly", "note", "reply"] },
                "find": { "type": "string", "description": "the text to mark (or to put a note next to)" },
                "pattern": { "type": "string", "enum": ["email", "phone", "personnummer", "ssn", "iban", "card"],
                             "description": "markup instead of find: a validated pattern" },
                "regex": { "type": "string", "description": "markup instead of find: Rust regex syntax, for anything else (dates, reference numbers)" },
                "pages": pages.clone(),
                "first_only": { "type": "boolean", "description": "mark only the first match" },
                "case_sensitive": { "type": "boolean" },
                "text": { "type": "string", "description": "the comment's own text" },
                "page": page("note without \"find\": 1-based page"),
                "x": { "type": "number", "minimum": 0, "maximum": 1 },
                "y": { "type": "number", "minimum": 0, "maximum": 1 },
                "parent": { "type": "string", "description": "reply: the id of the comment, like p3#41" },
            } }),
            Class::Edit,
            |a| {
                let kind = str_arg(a, "kind").unwrap_or("comment");
                let said = str_arg(a, "text").map(|t| format!(": “{}”", one_line(t, 80))).unwrap_or_default();
                match (kind, str_arg(a, "find")) {
                    ("note", Some(f)) => format!("Add a note next to “{}”{said}", one_line(f, 40)),
                    ("note", None) => format!("Add a note on p. {}{said}", int(a, "page").unwrap_or(0)),
                    ("reply", _) => format!("Reply to a comment{said}"),
                    (k, _) => match query_with(a, "find") {
                        Ok((_, what)) => {
                            let which = if bool_arg(a, "first_only", false) { "the first match of" } else { "every match of" };
                            format!("{} {which} {what}{}", cap(k), said.replacen(": ", " with the note ", 1))
                        }
                        Err(_) => format!("Add a {k}"),
                    },
                }
            },
            add_comment,
        )
        .checked(comment_check),
        tool(
            "list_form_fields",
            "Lists the form fields with their ids (like p2#17), names, types, current values, choices and whether \
             they are read-only or required. Empty when the document has no form.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            Class::Read,
            |_| "Listed the form fields".into(),
            list_form_fields,
        ),
        tool(
            "fill_form_fields",
            "Fills form fields: \"fields\" is a list of { \"field\": name or id from list_form_fields, \"value\": ... }. \
             A value is text for a text, drop-down or radio field (a radio group takes the choice's export value), true \
             or false for a check box, a list of strings for a multi-select list. All of it is one step; if one value is \
             refused (read-only, wrong format) nothing is changed and the reason names the field. Changes the \
             document (the user approves it).",
            json!({ "type": "object", "additionalProperties": false, "required": ["fields"], "properties": {
                "fields": { "type": "array", "minItems": 1, "maxItems": 100, "items": {
                    "type": "object", "additionalProperties": false, "required": ["field", "value"],
                    "properties": {
                        "field": { "type": "string", "description": "name or id from list_form_fields" },
                        "value": { "description": "text, true or false, or a list of strings" } } } },
            } }),
            Class::Edit,
            |a| {
                let n = a.get("fields").and_then(Value::as_array).map_or(0, Vec::len);
                format!("Fill {}", if n == 1 { "1 form field".to_string() } else { format!("{n} form fields") })
            },
            fill_form_fields,
        )
        .checked(fill_check),
        tool(
            "organize_pages",
            "Changes the page structure without removing anything. \"action\" is rotate (\"pages\" and \"degrees\": \
             90, 180 or 270 clockwise, or -90), move (\"pages\" go before position \"to\": 1 is the start, the page \
             count plus 1 the end; their order is kept) or insert_blank (a blank page before position \"at\", the \
             size of page \"like\"). Page numbers are 1-based and, after a move or an insert, renumbered. Changes the \
             document (the user approves it). Use delete_pages to remove pages.",
            json!({ "type": "object", "additionalProperties": false, "required": ["action"], "properties": {
                "action": { "type": "string", "enum": ["rotate", "move", "insert_blank"] },
                "pages": pages.clone(),
                "degrees": { "type": "integer", "enum": [-90, 90, 180, 270] },
                "to": { "type": "integer", "minimum": 1 },
                "at": { "type": "integer", "minimum": 1 },
                "like": page("insert_blank: the page whose size the new page takes"),
            } }),
            Class::Edit,
            |a| match str_arg(a, "action") {
                Some("rotate") => format!("Rotate {} by {}°", page_list_label(&pages_requested(a)), int(a, "degrees").unwrap_or(90)),
                Some("move") => format!("Move {} to position {}", page_list_label(&pages_requested(a)), int(a, "to").unwrap_or(0)),
                Some("insert_blank") => match int(a, "at") {
                    Some(at) => format!("Insert a blank page before p. {at}"),
                    None => "Insert a blank page at the end".into(),
                },
                _ => "Change the page order".into(),
            },
            organize_pages,
        )
        .checked(guard_check(Touch::Content)),
        tool(
            "delete_pages",
            "Deletes pages from the document (at least one page must remain). The user is asked to approve unless \
             they run the assistant in full-access mode; it is one undo step.",
            json!({ "type": "object", "additionalProperties": false, "required": ["pages"], "properties": { "pages": pages } }),
            Class::Destructive,
            |a| format!("Delete {}", page_list_label(&pages_requested(a))),
            delete_pages,
        )
        .checked(guard_check(Touch::Content)),
    ]
}

pub(super) fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + c.as_str())
}

// ---- redaction --------------------------------------------------------------------------------

fn redact_find_and_mark(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Annotations, &[])?;
    let (q, what) = query_arg(a)?;
    let count = state(&l)?.page_count;
    let pages = pages_arg(a, count)?;
    let hits = find_hits(&l, &q, &pages)?;
    if hits.is_empty() {
        return Err(format!("No match for {what} in the document text{}: nothing was marked. Scanned pages have no text to match; use redact_mark_area for those.",
            if pages.is_empty() { String::new() } else { format!(" on {}", pages_label(&pages)) }));
    }
    let marks = crate::redact::app::list_marks(l.eng, l.doc).map_err(err)?;
    let fresh: Vec<HitDto> = hits.iter().filter(|h| !covered(h, &marks)).cloned().collect();
    if fresh.is_empty() {
        return Err(format!("All {} for {what} are already marked for redaction. Nothing was changed.", plural(hits.len(), "match").replace("matchs", "matches")));
    }
    if fresh.len() > MAX_HITS {
        return Err(format!("{} for {what} is too many to mark in one go (the limit is {MAX_HITS}). Narrow it with \"pages\" or a more specific query.",
            plural(fresh.len(), "match").replace("matchs", "matches")));
    }
    let state = crate::redact::app::mark_hits(l.eng, l.doc, &fresh, l.source.clone()).map_err(err)?;
    let total = crate::redact::app::list_marks(l.eng, l.doc).map_err(err)?.len();
    let mut s = format!(
        "Marked {} for redaction ({what}); nothing is removed until redact_apply runs. The document now has {}.\n",
        plural(fresh.len(), "match").replace("matchs", "matches"),
        plural(total, "mark")
    );
    if fresh.len() < hits.len() {
        let _ = writeln!(s, "{} were already marked and were left alone.", hits.len() - fresh.len());
    }
    s.push_str(&data_block(&per_page(&fresh, matches!(q, FindQuery::Pattern { .. }))));
    s.push_str("\nTell the user to review the marks in the Redact panel (the Review in Redact button under your answer) and to say when to apply.");
    Ok(landed(&l, state, s))
}

fn redact_mark_area(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Annotations, &[])?;
    let st = state(&l)?;
    let page = super::tools::page_of(a, "page", st.page_count)?;
    let [w, h] = st.sizes[page as usize];
    let (x0, y0, x1, y1) = (fraction(a, "left")?, fraction(a, "top")?, fraction(a, "right")?, fraction(a, "bottom")?);
    if x1 <= x0 || y1 <= y0 {
        return Err("The rectangle is empty: \"right\" must be greater than \"left\" and \"bottom\" greater than \"top\".".into());
    }
    let marks = crate::redact::app::list_marks(l.eng, l.doc).map_err(err)?;
    let area = [x0 * w, y0 * h, x1 * w, y1 * h];
    if marks.iter().any(|m| {
        m.page == page && !m.whole_page && m.quads.iter().any(|q| (q[0] - area[0]).abs() < 1.0 && (q[1] - area[1]).abs() < 1.0 && (q[6] - area[2]).abs() < 1.0 && (q[7] - area[3]).abs() < 1.0)
    }) {
        return Err(format!("That area of p. {} is already marked. Nothing was changed.", page + 1));
    }
    let state = crate::redact::app::mark_area(l.eng, l.doc, page, area, l.source.clone()).map_err(err)?;
    let total = crate::redact::app::list_marks(l.eng, l.doc).map_err(err)?.len();
    Ok(landed(
        &l,
        state,
        format!("Marked an area of p. {} for redaction; nothing is removed until redact_apply runs. The document now has {}. Tell the user to review it in the Redact panel.",
            page + 1, plural(total, "mark")),
    ))
}

fn mark_line(m: &MarkDto) -> String {
    let what = if m.whole_page {
        "the whole page".to_string()
    } else {
        match &m.label {
            Some(t) if !t.trim().is_empty() => format!("“{}”", one_line(t, 80)),
            _ => "an area".to_string(),
        }
    };
    format!("[{}] p. {}: {what}", id_of(m.page, m.xref), m.page + 1)
}

fn redact_list_marks(c: &Ctx, _: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    let marks = crate::redact::app::list_marks(l.eng, l.doc).map_err(err)?;
    if marks.is_empty() {
        return Ok(Output::text("There are no redaction marks in the document."));
    }
    let mut pages: Vec<u32> = marks.iter().map(|m| m.page).collect();
    pages.dedup();
    let mut s = format!("{} on {}:\n", plural(marks.len(), "mark"), pages_label(&pages));
    s.push_str(&data_block(&marks.iter().take(MAX_LIST).map(mark_line).collect::<Vec<_>>().join("\n")));
    if marks.len() > MAX_LIST {
        let _ = write!(s, "\n(first {MAX_LIST} shown)");
    }
    let _ = write!(s, "\nmarks_digest: {} (pass it to redact_apply with expected_marks and pages)", crate::redact::app::marks_digest(&marks));
    Ok(Output::text(s))
}

fn redact_unmark(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Annotations, &[])?;
    let marks = crate::redact::app::list_marks(l.eng, l.doc).map_err(err)?;
    if bool_arg(a, "all", false) && a.get("marks").and_then(Value::as_array).is_some_and(|m| !m.is_empty()) {
        return Err("Give either \"marks\" or \"all\": true, not both. Nothing was changed.".into());
    }
    let chosen: Vec<(u32, i32)> = if bool_arg(a, "all", false) {
        marks.iter().map(|m| (m.page, m.xref)).collect()
    } else {
        let ids = a.get("marks").and_then(Value::as_array).ok_or("Give \"marks\" (ids from redact_list_marks) or \"all\": true.")?;
        let mut v = Vec::new();
        for id in ids.iter().filter_map(Value::as_str) {
            let (p, x) = parse_id(id).ok_or_else(|| format!("\"{id}\" is not a mark id (they look like p3#41; see redact_list_marks)."))?;
            if !marks.iter().any(|m| m.page == p && m.xref == x) {
                return Err(format!("There is no mark {id}. Nothing was changed; list the marks again."));
            }
            if !v.contains(&(p, x)) {
                v.push((p, x));
            }
        }
        v
    };
    if chosen.is_empty() {
        return Err("There are no redaction marks to remove. Nothing was changed.".into());
    }
    let state = crate::redact::app::unmark(l.eng, l.doc, &chosen, l.source.clone()).map_err(err)?;
    Ok(landed(&l, state, format!("Removed {}; {} left.", plural(chosen.len(), "redaction mark"), marks.len() - chosen.len())))
}

/// The `sanitize` argument of `redact_apply`: every key a known option, every value a boolean.
/// What is not named stays at its default (everything on).
fn sanitize_arg(a: &Value) -> Result<Sanitize, String> {
    const KEYS: [&str; 8] =
        ["metadata", "hiddenLayers", "hiddenText", "scriptsActions", "embeddedFiles", "annotations", "formFieldsUnderMarks", "bookmarks"];
    match a.get("sanitize") {
        None | Some(Value::Null) => Ok(Sanitize::default()),
        Some(Value::Object(o)) => {
            for (k, v) in o {
                if !KEYS.contains(&k.as_str()) {
                    return Err(format!("\"sanitize\" has no option \"{k}\". The options are: {}.", KEYS.join(", ")));
                }
                if !v.is_boolean() {
                    return Err(format!("\"sanitize.{k}\" must be true or false."));
                }
            }
            serde_json::from_value(Value::Object(o.clone())).map_err(|e| format!("\"sanitize\" is not valid: {e}"))
        }
        Some(_) => Err("\"sanitize\" must be an object of true / false options.".into()),
    }
}

/// What `redact_apply` will do besides removing the marked content, in the user's words: the
/// card must say what else disappears (every option is on by default, and comments go with it).
fn sanitize_words(s: &Sanitize) -> String {
    let mut also = Vec::new();
    let mut keeps = Vec::new();
    for (on, what) in [
        (s.metadata, "metadata"),
        (s.hidden_layers, "hidden layers"),
        (s.hidden_text, "hidden text"),
        (s.scripts_actions, "scripts and actions"),
        (s.embedded_files, "attached files"),
        (s.bookmarks, "bookmarks"),
    ] {
        if on { also.push(what) } else { keeps.push(what) }
    }
    let mut out = String::new();
    if s.annotations {
        out.push_str(", delete ALL comments and markup");
    } else {
        keeps.push("comments");
    }
    if also.len() == 6 {
        out.push_str(", strip metadata and all hidden data");
    } else if !also.is_empty() {
        let _ = write!(out, ", strip {}", also.join(", "));
    }
    if !s.form_fields_under_marks {
        keeps.push("form fields under marks");
    }
    if !keeps.is_empty() {
        let _ = write!(out, ", keep {}", keeps.join(", "));
    }
    out
}

/// The marks as the model listed them, checked against the document now: the list `redact_apply`
/// was told to apply and what is really there must be the same (count, pages and digest).
fn reviewed_marks(l: &Lane, a: &Value) -> Result<(Vec<MarkDto>, Vec<u32>), String> {
    let marks = crate::redact::app::list_marks(l.eng, l.doc).map_err(err)?;
    if marks.is_empty() {
        return Err("There is nothing marked for redaction. Nothing was changed.".into());
    }
    let expected = int(a, "expected_marks").unwrap_or(-1);
    let mut on: Vec<u32> = marks.iter().map(|m| m.page).collect();
    on.sort_unstable();
    on.dedup();
    let said: Vec<u32> = pages_requested(a).iter().filter(|p| **p >= 1).map(|p| *p as u32 - 1).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let digest = str_arg(a, "marks_digest").unwrap_or_default();
    if expected != marks.len() as i64 || said != on || digest != crate::redact::app::marks_digest(&marks) {
        return Err(format!(
            "The document has {} on {}, not what you listed (or the marks changed since). Nothing was redacted: call redact_list_marks and try again with exactly what it says, including marks_digest.",
            plural(marks.len(), "mark"),
            pages_label(&on)
        ));
    }
    Ok((marks, on))
}

/// The card of `redact_apply`: the first marked texts, so the user sees what goes, not a count.
fn apply_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    let Some(l) = c.host.lane() else { return Ok(None) };
    guard(&l, Touch::Content, &[])?;
    sanitize_arg(a)?;
    let (marks, on) = reviewed_marks(&l, a)?;
    let shown: Vec<String> = marks
        .iter()
        .take(3)
        .map(|m| {
            if m.whole_page {
                format!("all of p. {}", m.page + 1)
            } else {
                match &m.label {
                    Some(t) if !t.trim().is_empty() => format!("“{}”", one_line(t, 40)),
                    _ => format!("an area of p. {}", m.page + 1),
                }
            }
        })
        .collect();
    let more = marks.len().saturating_sub(shown.len());
    Ok(Some(format!(
        "{}{} · pp. {}",
        shown.join(", "),
        if more > 0 { format!(" and {more} more") } else { String::new() },
        on.iter().map(|p| (p + 1).to_string()).collect::<Vec<_>>().join(", ")
    )))
}

fn redact_apply(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Content, &[])?;
    let sanitize = sanitize_arg(a)?;
    let (_, on) = reviewed_marks(&l, a)?;
    let digest = str_arg(a, "marks_digest").unwrap_or_default().to_string();
    let applied = crate::redact::app::apply_expecting(l.eng, l.doc, sanitize, None, l.source.clone(), Some(&digest));
    let applied = match applied {
        Err(EngineError::Invalid(m)) if m == crate::redact::app::MARKS_CHANGED => {
            return Err("The marks changed while this was being applied. Nothing was redacted: call redact_list_marks and try again.".into());
        }
        r => r.map_err(err)?,
    };
    match applied {
        ApplyOutcome::Applied { state, marks: n, sanitize, verify, .. } => {
            let mut s = format!("Redacted {} on {}. ", plural(n, "mark"), pages_label(&on));
            let critical = verify.leaks.iter().filter(|k| k.critical).count();
            if verify.leaks.is_empty() {
                s.push_str("Checked: none of the marked text is left in the file. ");
            } else if critical == 0 {
                let mut places: Vec<&str> = verify.leaks.iter().map(|k| k.place.as_str()).collect();
                places.sort_unstable();
                places.dedup();
                let _ = write!(s, "Checked: the marked text is gone from where it was marked, but the same text also appears unmarked ({}). ", places.iter().take(5).copied().collect::<Vec<_>>().join("; "));
            }
            s.push_str("The file on disk is unchanged until the user saves; suggest Save as to keep the original. The Redact panel shows the verification.");
            let redacted = json!({ "marks": n, "sanitize": sanitize, "verify": verify });
            l.edited.store(true, std::sync::atomic::Ordering::SeqCst);
            s.push(' ');
            s.push_str(STALE);
            Ok(Output { text: s, ui: Some(UiAction::Edited { state, redacted: Some(redacted) }), ..Default::default() })
        }
        ApplyOutcome::Refused { leaks } => {
            let mut places: Vec<&str> = leaks.iter().map(|k| k.place.as_str()).collect();
            places.sort_unstable();
            places.dedup();
            Err(format!(
                "Inkwell refused to redact: marked content could still be recovered from the file ({}). Nothing was changed and the marks are still there. Tell the user; they can review in the Redact panel.",
                places.iter().take(5).copied().collect::<Vec<_>>().join("; ")
            ))
        }
        ApplyOutcome::Cancelled => Err("The redaction was cancelled. Nothing was changed.".into()),
    }
}

// ---- comments ---------------------------------------------------------------------------------

fn kind_word(k: Kind) -> &'static str {
    match k {
        Kind::Highlight => "highlight",
        Kind::Underline => "underline",
        Kind::StrikeOut => "strikeout",
        Kind::Squiggly => "squiggly",
        Kind::Note => "note",
        Kind::FreeText => "text box",
        Kind::Ink => "drawing",
        Kind::Rectangle => "rectangle",
        Kind::Ellipse => "ellipse",
        Kind::Line => "line",
        Kind::Arrow => "arrow",
        Kind::Polygon => "polygon",
        Kind::Stamp => "stamp",
        Kind::Other => "annotation",
    }
}

fn comment_line(d: &AnnotDto) -> String {
    let mut s = format!("[{}] p. {} {}", id_of(d.page, d.xref), d.page + 1, kind_word(d.kind));
    if !d.author.is_empty() {
        let _ = write!(s, " by {}", one_line(&d.author, 40));
    }
    if let Some(p) = d.parent {
        let _ = write!(s, " (reply to p{}#{p})", d.page + 1);
    }
    if d.resolved {
        s.push_str(" [resolved]");
    }
    if !d.contents.trim().is_empty() {
        let _ = write!(s, ": {}", one_line(&d.contents, 300));
    }
    s
}

fn list_comments(c: &Ctx, _: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    let all = annot::list_all(l.eng, l.doc).map_err(err)?;
    if all.is_empty() {
        return Ok(Output::text("The document has no comments."));
    }
    let mut s = format!("{}:\n", plural(all.len(), "comment"));
    s.push_str(&data_block(&all.iter().take(MAX_LIST).map(comment_line).collect::<Vec<_>>().join("\n")));
    if all.len() > MAX_LIST {
        let _ = write!(s, "\n(first {MAX_LIST} shown)");
    }
    Ok(Output::text(s))
}

/// The card of `add_comment`: how many matches a markup will land on, and where, so "every match"
/// is a number the user sees.
fn comment_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    let Some(l) = c.host.lane() else { return Ok(None) };
    guard(&l, Touch::Annotations, &[])?;
    let kind = str_arg(a, "kind").unwrap_or_default();
    if !matches!(kind, "highlight" | "underline" | "strikeout" | "squiggly") {
        return Ok(None);
    }
    let (q, what) = query_with(a, "find")?;
    let st = state(&l)?;
    let mut hits = find_hits(&l, &q, &pages_arg(a, st.page_count)?)?;
    if bool_arg(a, "first_only", false) {
        hits.truncate(1);
    }
    if hits.is_empty() {
        return Err(format!("{what} does not appear in the document text. Nothing was changed."));
    }
    let mut pages: Vec<u32> = hits.iter().map(|h| h.page).collect();
    pages.sort_unstable();
    pages.dedup();
    Ok(Some(format!("{} on {}", plural(hits.len(), "match").replace("matchs", "matches"), pages_label(&pages))))
}

fn add_comment(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    guard(&l, Touch::Annotations, &[])?;
    let kind = str_arg(a, "kind").ok_or("\"kind\" is required: highlight, underline, strikeout, squiggly, note or reply.")?;
    let text = a.get("text").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    let st = state(&l)?;
    let style = Style::default();
    match kind {
        "reply" => {
            let id = str_arg(a, "parent").ok_or("\"parent\" is required for a reply: the comment's id from list_comments.")?;
            let text = text.ok_or("\"text\" is required: what the reply says.")?;
            let (page, parent) = parse_id(id).ok_or_else(|| format!("\"{id}\" is not a comment id (they look like p3#41; see list_comments)."))?;
            if !annot::list_all(l.eng, l.doc).map_err(err)?.iter().any(|d| d.page == page && d.xref == parent) {
                return Err(format!("There is no comment {id}. Nothing was changed; list the comments again."));
            }
            let (state, _) = annot::reply_one(l.eng, l.doc, page, parent, text, AUTHOR, l.source.clone()).map_err(err)?;
            Ok(landed(&l, state, format!("Replied to the comment on p. {}.", page + 1)))
        }
        "note" => {
            let text = text.ok_or("\"text\" is required: what the note says.")?;
            let (page, at) = match str_arg(a, "find") {
                Some(f) => {
                    let q = FindQuery::Text { needle: f.to_string(), case_sensitive: bool_arg(a, "case_sensitive", false), whole_word: false };
                    let hits = find_hits(&l, &q, &pages_arg(a, st.page_count)?)?;
                    let h = hits.first().ok_or_else(|| format!("“{}” does not appear in the document text. Nothing was changed.", one_line(f, 40)))?;
                    let q = h.quads[0];
                    (h.page, [q[2].max(q[6]) + 4.0, q[1].min(q[3])])
                }
                None => {
                    let page = super::tools::page_of(a, "page", st.page_count)?;
                    let [w, h] = st.sizes[page as usize];
                    (page, [fraction(a, "x")? * w, fraction(a, "y")? * h])
                }
            };
            let (state, _) = annot::add_one(l.eng, l.doc, page, &NewAnnot::Note { at, text: text.to_string() }, &style, AUTHOR, l.source.clone()).map_err(err)?;
            Ok(landed(&l, state, format!("Added a note on p. {}.", page + 1)))
        }
        "highlight" | "underline" | "strikeout" | "squiggly" => {
            let markup = match kind {
                "highlight" => Kind::Highlight,
                "underline" => Kind::Underline,
                "strikeout" => Kind::StrikeOut,
                _ => Kind::Squiggly,
            };
            let (q, what) = query_with(a, "find").map_err(|e| format!("{e} \"find\" is the text to mark, copied from the document."))?;
            let mut hits = find_hits(&l, &q, &pages_arg(a, st.page_count)?)?;
            if hits.is_empty() {
                return Err(format!("{what} does not appear in the document text{}. Nothing was changed.",
                    if pages_requested(a).is_empty() { "" } else { " on those pages" }));
            }
            if bool_arg(a, "first_only", false) {
                hits.truncate(1);
            }
            if hits.len() > MAX_HITS {
                return Err(format!("{} is too many to mark in one go (the limit is {MAX_HITS}). Narrow it with \"pages\".", plural(hits.len(), "match").replace("matchs", "matches")));
            }
            let mut pages: Vec<u32> = hits.iter().map(|h| h.page).collect();
            pages.dedup();
            let label = format!("{} {} on {}", cap(kind_word(markup)), plural(hits.len(), "match").replace("matchs", "matches"), pages_label(&pages));
            let state = l
                .eng
                .apply_custom(l.doc, label, l.source.clone(), false, |d| {
                    for h in &hits {
                        let xref = annot::add(d, h.page, &NewAnnot::Markup { markup, quads: h.quads.clone() }, &style, AUTHOR)?;
                        if let Some(t) = text {
                            annot::edit(d, h.page, xref, &Patch { contents: Some(t.to_string()), ..Default::default() }, AUTHOR)?;
                        }
                    }
                    Ok(pages.clone())
                })
                .map_err(err)?;
            let mut s = format!("Added {} to {} on {}.\n", plural(hits.len(), kind_word(markup)), plural(hits.len(), "match").replace("matchs", "matches"), pages_label(&pages));
            s.push_str(&data_block(&per_page(&hits, false)));
            Ok(landed(&l, state, s))
        }
        other => Err(format!("Unknown kind \"{other}\". Use highlight, underline, strikeout, squiggly, note or reply.")),
    }
}

// ---- forms ------------------------------------------------------------------------------------

fn kind_label(k: FieldKind) -> &'static str {
    match k {
        FieldKind::Text => "text",
        FieldKind::Checkbox => "check box",
        FieldKind::Radio => "radio",
        FieldKind::Combo => "drop-down",
        FieldKind::List => "list",
        FieldKind::Button => "button",
        FieldKind::Signature => "signature (cannot be filled)",
    }
}

fn field_line(f: &FieldDto) -> String {
    let mut s = format!("[{}] \"{}\" {}", id_of(f.page, f.xref), f.name, kind_label(f.kind));
    match f.kind {
        FieldKind::Checkbox => {
            let _ = write!(s, " = {}", if f.checked { "on" } else { "off" });
        }
        FieldKind::Radio => {
            let _ = write!(s, " choice \"{}\"{}", f.export, if f.checked { " (selected)" } else { "" });
        }
        FieldKind::Button | FieldKind::Signature => {}
        _ if f.values.len() > 1 => {
            let _ = write!(s, " = [{}]", f.values.iter().map(|v| format!("\"{}\"", one_line(v, 60))).collect::<Vec<_>>().join(", "));
        }
        _ => {
            let _ = write!(s, " = \"{}\"", one_line(&f.value, 100));
        }
    }
    if !f.options.is_empty() {
        let _ = write!(s, " options: {}", f.options.iter().take(20).map(|o| format!("\"{}\"", one_line(&o.export, 40))).collect::<Vec<_>>().join(", "));
    }
    if f.read_only {
        s.push_str(" [read-only]");
    }
    if f.required {
        s.push_str(" [required]");
    }
    s
}

fn list_form_fields(c: &Ctx, _: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    let form = forms::list_all(l.eng, l.doc).map_err(err)?;
    let fields: Vec<&FieldDto> = form.fields.iter().filter(|f| !f.hidden).collect();
    if fields.is_empty() {
        return Ok(Output::text("The document has no form fields."));
    }
    let mut s = format!("{}{}:\n", plural(fields.len(), "form field"), if form.xfa { " (an XFA form: only its AcroForm fallback is shown)" } else { "" });
    s.push_str(&data_block(&fields.iter().take(MAX_LIST).map(|f| field_line(f)).collect::<Vec<_>>().join("\n")));
    if fields.len() > MAX_LIST {
        let _ = write!(s, "\n(first {MAX_LIST} shown)");
    }
    Ok(Output::text(s))
}

fn value_of(v: &Value) -> Result<FieldValue, String> {
    match v {
        Value::Bool(b) => Ok(FieldValue::Checked(*b)),
        Value::String(s) => Ok(FieldValue::Text(s.clone())),
        Value::Number(n) => Ok(FieldValue::Text(n.to_string())),
        Value::Array(a) => Ok(FieldValue::Many(a.iter().map(|x| x.as_str().map(str::to_string).ok_or("a list value holds strings")).collect::<Result<_, _>>()?)),
        _ => Err("a value is text, true or false, or a list of strings".into()),
    }
}

/// (page, object, value, qualified name) of every field a `fill_form_fields` call fills.
type FillPlan = Vec<(u32, i32, FieldValue, String)>;

/// Resolves the call's fields against the form the model was shown (visible fields only) and
/// refuses what cannot be filled, before anything is asked or changed.
fn fill_plan(l: &Lane, a: &Value) -> Result<FillPlan, String> {
    let items =a.get("fields").and_then(Value::as_array).filter(|f| !f.is_empty()).ok_or("\"fields\" is required: a list of { field, value }.")?;
    let form = forms::list_all(l.eng, l.doc).map_err(err)?;
    // the fields list_form_fields shows: a hidden field is not offered, so it is not filled either
    let visible: Vec<&FieldDto> = form.fields.iter().filter(|f| !f.hidden).collect();
    if visible.is_empty() {
        return Err("The document has no form fields. Nothing was changed.".into());
    }
    let mut plan: Vec<(u32, i32, FieldValue, String)> = Vec::new();
    for it in items {
        let key = it.get("field").and_then(Value::as_str).map(str::trim).filter(|k| !k.is_empty()).ok_or("each item needs a \"field\" (a name or id from list_form_fields).")?;
        let value = value_of(it.get("value").unwrap_or(&Value::Null)).map_err(|e| format!("Field \"{key}\": {e}."))?;
        let by_id = parse_id(key).and_then(|(p, x)| visible.iter().copied().find(|f| f.page == p && f.xref == x));
        let target = match by_id {
            Some(f) => f,
            None => {
                let exact: Vec<&FieldDto> = visible.iter().copied().filter(|f| f.name == key).collect();
                let found = if exact.is_empty() { visible.iter().copied().filter(|f| f.name.eq_ignore_ascii_case(key)).collect() } else { exact };
                let mut nodes: Vec<i32> = found.iter().map(|f| f.node).collect();
                nodes.sort_unstable();
                nodes.dedup();
                match (found.first(), nodes.len()) {
                    (Some(f), 1) => *f,
                    (None, _) => return Err(format!("There is no form field \"{key}\". Nothing was changed; see list_form_fields.")),
                    _ => return Err(format!("{} fields are named \"{key}\"; use an id from list_form_fields. Nothing was changed.", nodes.len())),
                }
            }
        };
        if target.read_only {
            return Err(format!("The field \"{}\" is read-only. Nothing was changed.", target.name));
        }
        plan.push((target.page, target.xref, value, target.name.clone()));
    }
    Ok(plan)
}

fn value_words(v: &FieldValue) -> String {
    match v {
        FieldValue::Checked(b) => if *b { "on".to_string() } else { "off".to_string() },
        FieldValue::Text(t) => format!("“{}”", one_line(t, 60)),
        FieldValue::Many(m) => format!("[{}]", m.iter().map(|x| one_line(x, 30)).collect::<Vec<_>>().join(", ")),
    }
}

/// The card of `fill_form_fields`: each field with the value it gets (so a value that came out of
/// the document cannot be approved blind), after the signature check.
fn fill_check(c: &Ctx, a: &Value) -> Result<Option<String>, String> {
    let Some(l) = c.host.lane() else { return Ok(None) };
    let plan = fill_plan(&l, a)?;
    let names: Vec<String> = plan.iter().map(|p| p.3.clone()).collect();
    guard(&l, Touch::Fields, &names)?;
    let mut lines: Vec<String> = plan.iter().take(8).map(|(_, _, v, n)| format!("{} = {}", one_line(n, 40), value_words(v))).collect();
    if plan.len() > 8 {
        lines.push(format!("and {} more", plan.len() - 8));
    }
    Ok(Some(lines.join(" · ")))
}

fn fill_form_fields(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    let plan = fill_plan(&l, a)?;
    guard(&l, Touch::Fields, &plan.iter().map(|p| p.3.clone()).collect::<Vec<_>>())?;
    let noop = l
        .eng
        .read_custom(l.doc, |d| {
            let mut all = true;
            for (p, x, v, _) in &plan {
                all &= forms::set_value_is_noop(d, *p, *x, v).unwrap_or(false);
            }
            Ok(all)
        })
        .map_err(err)?;
    if noop {
        return Err("Every field already has that value. Nothing was changed.".into());
    }
    let mut pages: Vec<u32> = plan.iter().map(|p| p.0).collect();
    pages.sort_unstable();
    pages.dedup();
    let label = if plan.len() == 1 { format!("Fill field on p. {}", pages[0] + 1) } else { format!("Fill {} fields", plan.len()) };
    let mut failed: Option<String> = None;
    let r = l.eng.apply_custom(l.doc, label, l.source.clone(), false, |d| {
        let mut touched = Vec::new();
        for (p, x, v, name) in &plan {
            match forms::set_value(d, *p, *x, v) {
                Ok(t) => touched.extend(t),
                Err(e) => {
                    failed = Some(format!("The field \"{name}\" was refused: {e}. Nothing was changed."));
                    return Err(e);
                }
            }
        }
        touched.sort_unstable();
        touched.dedup();
        Ok(touched)
    });
    match r {
        Ok(state) => {
            let lines = plan.iter().map(|(_, _, v, n)| format!("{n}: {}", match v {
                FieldValue::Checked(b) => if *b { "on".to_string() } else { "off".to_string() },
                FieldValue::Text(t) => format!("“{}”", one_line(t, 60)),
                FieldValue::Many(m) => format!("[{}]", m.iter().map(|x| one_line(x, 30)).collect::<Vec<_>>().join(", ")),
            })).collect::<Vec<_>>().join("\n");
            Ok(landed(&l, state, format!("Filled {}:\n{}\n", plural(plan.len(), "field"), data_block(&lines))))
        }
        Err(e) => Err(failed.unwrap_or_else(|| err(e))),
    }
}

// ---- pages ------------------------------------------------------------------------------------

/// Applies a page operation; `Err` when the engine found nothing to do.
fn apply_op(l: &Lane, op: EditOp) -> Result<EditState, String> {
    let before = state(l)?.rev;
    let s = l.eng.apply_from(l.doc, op, l.source.clone()).map_err(err)?;
    if s.rev == before {
        return Err("That would leave the pages exactly as they are. Nothing was changed.".into());
    }
    Ok(s)
}

fn organize_pages(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    let action = str_arg(a, "action").ok_or("\"action\" is required: rotate, move or insert_blank.")?;
    let count = state(&l)?.page_count;
    guard(&l, Touch::Content, &[])?;
    let pos = |key: &str, max: u32| -> Result<u32, String> {
        let v = int(a, key).ok_or_else(|| format!("\"{key}\" is required: a position from 1 to {max}."))?;
        if v < 1 || v > max as i64 {
            return Err(format!("\"{key}\" must be from 1 to {max}, not {v}."));
        }
        Ok(v as u32 - 1)
    };
    let (op, said) = match action {
        "rotate" => {
            let pages = pages_arg(a, count)?;
            if pages.is_empty() {
                return Err("\"pages\" is required: the pages to rotate.".into());
            }
            let deg = int(a, "degrees").ok_or("\"degrees\" is required: 90, 180, 270 or -90 (clockwise).")?;
            if deg % 90 != 0 || deg % 360 == 0 {
                return Err(format!("{deg}° is not a turn: use 90, 180, 270 or -90."));
            }
            let said = format!("Rotated {} by {}°.", pages_label(&pages), deg);
            (EditOp::Rotate { pages, quarter_turns: (deg / 90) as i32 }, said)
        }
        "move" => {
            let pages = pages_arg(a, count)?;
            if pages.is_empty() {
                return Err("\"pages\" is required: the pages to move.".into());
            }
            let to = pos("to", count + 1)?;
            let said = format!("Moved {} to position {}.", pages_label(&pages), to + 1);
            (EditOp::Move { pages, to }, said)
        }
        "insert_blank" => {
            let at = if a.get("at").is_some() { pos("at", count + 1)? } else { count };
            let like = if a.get("like").is_some() { pos("like", count)? } else { at.saturating_sub(1).min(count - 1) };
            (EditOp::InsertBlank { at, like }, format!("Inserted a blank page as p. {}.", at + 1))
        }
        other => return Err(format!("Unknown action \"{other}\". Use rotate, move or insert_blank.")),
    };
    let s = apply_op(&l, op)?;
    let n = s.page_count;
    Ok(landed(&l, s, format!("{said} The document has {}.", plural(n as usize, "page"))))
}

fn delete_pages(c: &Ctx, a: &Value) -> Result<Output, String> {
    let l = lane(c)?;
    let count = state(&l)?.page_count;
    let pages = pages_arg(a, count)?;
    if pages.is_empty() {
        return Err("\"pages\" is required: the pages to delete.".into());
    }
    if pages.len() as u32 >= count {
        return Err("That would delete every page; at least one page must remain. Nothing was changed.".into());
    }
    guard(&l, Touch::Content, &[])?;
    let s = apply_op(&l, EditOp::Delete { pages: pages.clone() })?;
    let n = s.page_count;
    Ok(landed(&l, s, format!("Deleted {}; the pages after it moved up. The document has {}.", pages_label(&pages), plural(n as usize, "page"))))
}

#[cfg(test)]
pub(super) mod tests;
