//! The QuickJS runtime of one document, and the host functions its scripts reach (`__h`, see prelude.js).
//!
//! Runs on the document's worker thread only (nothing here is `Send`). A script's reach is what the
//! prelude defines plus the host functions below: no file, no network, no timers, no module loader.

use super::util::{self, PrintfArg};
use super::{
    Alert, Event, FieldState, FieldType, Job, LogLevel, LogLine, Outcome, Request, ScriptError, DISPLAY_HIDDEN, DISPLAY_VISIBLE, MAX_ALERTS_PER_JOB,
    MAX_LOG_PER_JOB, MAX_SCRIPT_BYTES, MAX_TEXT_BYTES,
};
use crate::forms::af;
use rquickjs::context::EvalOptions;
use rquickjs::{Context, Ctx, Function, Object, Runtime, Value};
use serde_json::{json, Value as Json};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PRELUDE: &str = include_str!("prelude.js");
/// The runtime's heap, the biggest garbage-collection step, the script stack.
const MEMORY_LIMIT: usize = 64 << 20;
const GC_THRESHOLD: usize = 8 << 20;
const STACK_LIMIT: usize = 1 << 20;

/// What the host functions read and write while a job runs.
pub(super) struct Host {
    pub fields: Vec<FieldState>,
    pub changed: BTreeSet<usize>,
    pub alerts: Vec<Alert>,
    pub requests: Vec<Request>,
    pub log: Vec<LogLine>,
    pub doc: super::DocInfo,
    /// The field whose script runs (for the log).
    pub field: Option<String>,
    /// Calls reported as unsupported already (once per runtime).
    pub seen: HashSet<String>,
}

type Shared = Rc<RefCell<Host>>;

pub(super) struct Js {
    runtime: Runtime,
    context: Context,
    host: Shared,
    /// Nanoseconds after `base` at which the running script is interrupted.
    deadline: Arc<AtomicU64>,
    base: Instant,
}

/// A string cut at `max` bytes on a character boundary.
pub(super) fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

fn text_of(v: &Json) -> String {
    match v {
        Json::Null => String::new(),
        Json::String(s) => s.clone(),
        Json::Bool(b) => b.to_string(),
        Json::Number(n) => n.as_f64().map_or_else(|| n.to_string(), |f| if f.is_finite() { af::js_number(f) } else { String::new() }),
        other => other.to_string(),
    }
}

fn texts_of(v: &Json) -> Vec<String> {
    match v {
        Json::Array(a) => a.iter().take(1 << 16).map(text_of).collect(),
        Json::Null => Vec::new(),
        other => vec![text_of(other)],
    }
}

/// A JavaScript number for a text value that reads as one, as Acrobat's `field.value` gives
/// (digits only, no leading zero and no more than 15 digits, so ids and zip codes stay text).
fn as_number(t: &str) -> Option<f64> {
    let t = t.trim();
    let digits = t.chars().filter(char::is_ascii_digit).count();
    if t.is_empty() || digits == 0 || digits > 15 || !t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-')) {
        return None;
    }
    if t.matches('-').count() > 1 || t.matches('.').count() > 1 || t[1..].contains('-') {
        return None;
    }
    let unsigned = t.trim_start_matches('-');
    if unsigned.len() > 1 && unsigned.starts_with('0') && !unsigned.starts_with("0.") {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn colour_json(c: &Option<Vec<String>>) -> Json {
    match c {
        None => json!(["T"]),
        Some(parts) => Json::Array(
            parts
                .iter()
                .enumerate()
                .map(|(i, p)| if i == 0 { json!(p) } else { json!(p.parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0)) })
                .collect(),
        ),
    }
}

/// `field.<prop>` as JSON.
pub(super) fn field_get(f: &FieldState, prop: &str) -> Json {
    let box_like = matches!(f.kind, FieldType::CheckBox | FieldType::RadioButton);
    match prop {
        "value" => match f.kind {
            _ if box_like => json!(f.value.first().map_or("Off", String::as_str)),
            FieldType::ListBox if f.value.len() > 1 => json!(f.value),
            _ => {
                let v = f.value.first().cloned().unwrap_or_default();
                as_number(&v).map_or_else(|| json!(v), |n| json!(n))
            }
        },
        "valueAsString" => json!(if box_like { f.value.first().cloned().unwrap_or_else(|| "Off".into()) } else { f.value.join(",") }),
        "defaultValue" => json!(f.default.first().cloned().unwrap_or_default()),
        "type" => json!(f.kind.name()),
        "name" => json!(f.name),
        "readonly" => json!(f.readonly),
        "required" => json!(f.required),
        "display" => json!(f.display),
        "hidden" => json!(f.display == DISPLAY_HIDDEN),
        "numItems" => json!(f.options.len()),
        "charLimit" => json!(f.char_limit.unwrap_or(0)),
        "exportValues" => json!(f.options.iter().map(|o| o.0.clone()).collect::<Vec<_>>()),
        "currentValueIndices" => {
            let idx: Vec<usize> = f.options.iter().enumerate().filter(|(_, o)| f.value.contains(&o.0)).map(|(i, _)| i).collect();
            if idx.len() == 1 { json!(idx[0]) } else { json!(idx) }
        }
        "textColor" => colour_json(&f.text_color),
        "fillColor" => colour_json(&f.fill_color),
        "strokeColor" => colour_json(&f.stroke_color),
        _ => {
            if let Some(rest) = prop.strip_prefix("item:") {
                let mut it = rest.split(':');
                let i: f64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let export = it.next() != Some("0");
                let i = if i < 0.0 { f.options.len().saturating_sub(1) } else { i as usize };
                return f.options.get(i).map_or(Json::Null, |o| json!(if export { &o.0 } else { &o.1 }));
            }
            if let Some(rest) = prop.strip_prefix("checked:") {
                let i: usize = rest.parse().unwrap_or(0);
                return json!(f.options.get(i).is_some_and(|o| f.value.first() == Some(&o.0)));
            }
            Json::Null
        }
    }
}

/// `field.<prop> = v`. Returns false when the property is not one that can be set.
pub(super) fn field_set(f: &mut FieldState, prop: &str, v: &Json) -> bool {
    let truthy = |v: &Json| match v {
        Json::Bool(b) => *b,
        Json::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Json::String(s) => !s.is_empty(),
        Json::Null => false,
        _ => true,
    };
    match prop {
        "value" => {
            let vals: Vec<String> = texts_of(v).into_iter().map(|s| truncate(s, MAX_TEXT_BYTES)).collect();
            f.value = match f.kind {
                FieldType::CheckBox | FieldType::RadioButton => vals.into_iter().filter(|v| v != "Off" && !v.is_empty()).take(1).collect(),
                FieldType::ListBox => vals,
                _ => vals.into_iter().take(1).collect(),
            };
        }
        "readonly" => f.readonly = truthy(v),
        "required" => f.required = truthy(v),
        "display" => f.display = v.as_f64().map_or(DISPLAY_VISIBLE, |n| (n as i32).clamp(0, 3)),
        "hidden" => f.display = if truthy(v) { DISPLAY_HIDDEN } else { DISPLAY_VISIBLE },
        "textColor" | "fillColor" | "strokeColor" => {
            let c = match v {
                Json::Array(a) if !a.is_empty() => Some(a.iter().take(5).map(text_of).collect::<Vec<_>>()),
                _ => None,
            };
            match prop {
                "textColor" => f.text_color = c,
                "fillColor" => f.fill_color = c,
                _ => f.stroke_color = c,
            }
        }
        "check" => {
            let (Some(i), on) = (v.get(0).and_then(Json::as_f64), v.get(1).is_none_or(truthy)) else { return false };
            let Some(export) = f.options.get(i.max(0.0) as usize).map(|o| o.0.clone()) else { return false };
            if on {
                f.value = vec![export];
            } else if f.value.first() == Some(&export) {
                f.value.clear();
            }
        }
        _ => return false,
    }
    true
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "MAC"
    } else if cfg!(target_os = "windows") {
        "WIN"
    } else {
        "UNIX"
    }
}

/// Only these reach the UI as requests to open (the UI still asks first).
fn url_allowed(u: &str) -> bool {
    let l = u.trim().to_ascii_lowercase();
    (l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:")) && !l.chars().any(char::is_control)
}

fn arg_inner(args_json: &str) -> String {
    let t = args_json.trim();
    t.strip_prefix('[').and_then(|r| r.strip_suffix(']')).unwrap_or(t).to_string()
}

fn merged(value: &str, change: &str, start: f64, end: f64) -> String {
    let chars: Vec<char> = value.chars().collect();
    let a = (start.max(0.0) as usize).min(chars.len());
    let b = (end.max(0.0) as usize).clamp(a, chars.len());
    let mut s: String = chars[..a].iter().collect();
    s.push_str(change);
    s.extend(&chars[b..]);
    s
}

fn install(ctx: &Ctx<'_>, host: &Shared) -> rquickjs::Result<()> {
    let h = Object::new(ctx.clone())?;
    macro_rules! native {
        ($name:literal, $f:expr) => {
            h.set($name, Function::new(ctx.clone(), $f)?)?
        };
    }
    let index_of = |host: &Shared, name: &str| host.borrow().fields.iter().position(|f| f.name == name);

    {
        let host = host.clone();
        native!("fexists", move |name: String| -> String {
            let hb = host.borrow();
            if hb.fields.iter().any(|f| f.name == name) {
                return name;
            }
            // A parent name ("total" for "total.0"): its first kid stands in, as for groups.
            let prefix = format!("{name}.");
            hb.fields.iter().find(|f| f.name.starts_with(&prefix)).map(|f| f.name.clone()).unwrap_or_default()
        });
    }
    {
        let host = host.clone();
        native!("fget", move |name: String, prop: String| -> String {
            let hb = host.borrow();
            hb.fields.iter().find(|f| f.name == name).map_or_else(|| "null".to_string(), |f| field_get(f, &prop).to_string())
        });
    }
    {
        let host = host.clone();
        native!("fset", move |name: String, prop: String, value: String| {
            let Some(i) = index_of(&host, &name) else { return };
            let v: Json = serde_json::from_str(&value).unwrap_or(Json::Null);
            let mut hb = host.borrow_mut();
            let before = hb.fields[i].clone();
            if field_set(&mut hb.fields[i], &prop, &v) && hb.fields[i] != before {
                hb.changed.insert(i);
            }
        });
    }
    {
        let host = host.clone();
        native!("nfields", move || -> f64 { host.borrow().fields.len() as f64 });
    }
    {
        let host = host.clone();
        native!("nth", move |i: f64| -> String { host.borrow().fields.get(i.max(0.0) as usize).map(|f| f.name.clone()).unwrap_or_default() });
    }
    {
        let host = host.clone();
        native!("doc_info", move || -> String {
            let hb = host.borrow();
            let info: serde_json::Map<String, Json> = hb.doc.info.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            json!({ "fileName": hb.doc.file_name, "numPages": hb.doc.num_pages, "page": hb.doc.page, "info": info }).to_string()
        });
    }
    {
        let host = host.clone();
        native!("alert", move |msg: String, title: String| {
            let mut hb = host.borrow_mut();
            if hb.alerts.len() >= MAX_ALERTS_PER_JOB {
                return;
            }
            let field = hb.field.clone();
            hb.alerts.push(Alert { message: truncate(msg, MAX_TEXT_BYTES), title: (!title.is_empty()).then(|| truncate(title, 1024)), field });
        });
    }
    {
        let host = host.clone();
        native!("req", move |kind: String, arg: String| {
            let mut hb = host.borrow_mut();
            let warn = |hb: &mut Host, m: String| {
                let field = hb.field.clone();
                hb.log.push(LogLine { level: LogLevel::Warn, field, message: m });
            };
            let req = match kind.as_str() {
                "reset" => Request::Reset { fields: texts_of(&serde_json::from_str(&arg).unwrap_or(Json::Null)).into_iter().take(1024).collect() },
                "print" => Request::Print,
                "beep" => Request::Beep,
                "calculate" => return,
                "page" => {
                    let n: f64 = arg.parse().unwrap_or(0.0);
                    let last = hb.doc.num_pages.saturating_sub(1);
                    let p = (n.max(0.0) as usize).min(last);
                    hb.doc.page = p;
                    Request::GoToPage { page: p }
                }
                "focus" => Request::Focus { field: arg },
                "launch" if url_allowed(&arg) => Request::LaunchUrl { url: truncate(arg, 4096) },
                "submit" if url_allowed(&arg) => Request::Submit { url: truncate(arg, 4096) },
                "launch" | "submit" => {
                    warn(&mut hb, format!("{} to \"{}\" is refused: only http, https and mailto links are opened", if kind == "launch" { "app.launchURL" } else { "submitForm" }, truncate(arg, 120)));
                    return;
                }
                _ => return,
            };
            if hb.requests.len() < 64 {
                hb.requests.push(req);
            }
        });
    }
    {
        let host = host.clone();
        native!("log", move |level: String, msg: String| {
            let mut hb = host.borrow_mut();
            if hb.log.len() < MAX_LOG_PER_JOB {
                let field = hb.field.clone();
                let level = if level == "error" { LogLevel::Error } else if level == "warn" { LogLevel::Warn } else { LogLevel::Info };
                hb.log.push(LogLine { level, field, message: truncate(msg, MAX_TEXT_BYTES) });
            }
        });
    }
    {
        let host = host.clone();
        native!("unsupported", move |label: String| {
            let mut hb = host.borrow_mut();
            let label = truncate(label, 200);
            if hb.seen.insert(label.clone()) && hb.log.len() < MAX_LOG_PER_JOB {
                let field = hb.field.clone();
                hb.log.push(LogLine { level: LogLevel::Warn, field, message: format!("{label} is not supported") });
            }
        });
    }
    native!("platform", || -> String { platform().to_string() });

    native!("af_format", |call: String, args: String, value: String| -> String {
        match af::parse_format(&format!("{call}({});", arg_inner(&args))) {
            Some(f) => truncate(af::format_value(&f, &value), MAX_TEXT_BYTES),
            None => value,
        }
    });
    native!("af_key", |call: String, args: String, field: String, value: String, opts: String| -> String {
        let Some(f) = af::parse_format(&format!("{call}({});", arg_inner(&args))) else { return json!({ "ok": true }).to_string() };
        let o: Json = serde_json::from_str(&opts).unwrap_or(Json::Null);
        if o["commit"].as_bool().unwrap_or(true) {
            return match af::keystroke(&f, &field, &value) {
                Ok(v) => json!({ "ok": true, "value": v }).to_string(),
                Err(m) => json!({ "ok": false, "msg": m }).to_string(),
            };
        }
        // Typing: the entry as it would be after this change must still be able to become a valid one.
        let change = o["change"].as_str().unwrap_or_default();
        let entry = merged(&value, change, o["start"].as_f64().unwrap_or(0.0), o["end"].as_f64().unwrap_or(0.0));
        json!({ "ok": af::partial_ok(&f, &entry) }).to_string()
    });
    native!("af_range", |args: String, value: String| -> String {
        let Some(v) = af::parse_validate(&format!("AFRange_Validate({});", arg_inner(&args))) else { return json!({ "ok": true }).to_string() };
        match af::validate(&v, &value) {
            Ok(()) => json!({ "ok": true }).to_string(),
            Err(m) => json!({ "ok": false, "msg": m }).to_string(),
        }
    });
    {
        let host = host.clone();
        native!("af_calc", move |op: String, fields: String| -> String {
            let names: Vec<String> = texts_of(&serde_json::from_str(&fields).unwrap_or(Json::Null));
            let hb = host.borrow();
            let lookup = |name: &str| -> Vec<String> {
                let prefix = format!("{name}.");
                hb.fields
                    .iter()
                    .filter(|f| !matches!(f.kind, FieldType::Button | FieldType::Signature) && (f.name == name || f.name.starts_with(&prefix)))
                    .map(|f| f.value.first().cloned().unwrap_or_default())
                    .collect()
            };
            let Some(op) = af::CalcOp::ALL.into_iter().find(|o| o.code() == op) else { return String::new() };
            af::calculate(&af::Calculate::Simple { op, fields: names }, &lookup).unwrap_or_default()
        });
    }
    native!("af_number", |s: String| -> String { af::make_number(&s).map_or_else(|| "null".to_string(), |n| json!(n).to_string()) });

    native!("printf", |fmt: String, args: String| -> String {
        let list: Vec<PrintfArg> = serde_json::from_str::<Vec<(Json, String)>>(&args)
            .unwrap_or_default()
            .into_iter()
            .map(|(n, s)| PrintfArg { num: n.as_f64().unwrap_or(f64::NAN), text: s })
            .collect();
        match util::printf(&fmt, &list) {
            Ok(s) => truncate(s, MAX_TEXT_BYTES),
            Err(e) => format!("\u{0}ERR:{e}"),
        }
    });
    native!("printd", |fmt: String, preset: f64, parts: String| -> String {
        let Ok(p) = serde_json::from_str::<[i64; 7]>(&parts) else { return String::new() };
        let f = if preset >= 0.0 { util::printd_preset(preset as i64).to_string() } else { fmt };
        truncate(util::printd(&f, p), MAX_TEXT_BYTES)
    });
    native!("printx", |fmt: String, src: String| -> String { truncate(util::printx(&fmt, &src), MAX_TEXT_BYTES) });
    native!("scand", |fmt: String, text: String| -> String { util::scand(&fmt, &text).map_or_else(|| "null".to_string(), |p| json!(p).to_string()) });
    #[cfg(test)]
    native!("sleep", |ms: f64| {
        // Not interruptible: stands in for a host call that does not return.
        std::thread::sleep(Duration::from_millis(ms.max(0.0) as u64));
    });
    ctx.globals().set("__h", h)?;
    Ok(())
}

/// The message of an exception thrown out of a script.
fn describe(ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
    if !matches!(err, rquickjs::Error::Exception) {
        return err.to_string();
    }
    let v = ctx.catch();
    if let Some(o) = v.as_object() {
        let name: Option<String> = o.get("name").ok();
        let message: Option<String> = o.get("message").ok();
        if let Some(m) = message {
            let line: Option<String> = o.get("stack").ok().and_then(|s: String| s.lines().find(|l| l.contains("eval_script")).map(|l| l.trim().to_string()));
            let base = match name {
                Some(n) if !n.is_empty() => format!("{n}: {m}"),
                _ => m,
            };
            return match line.and_then(|l| l.rsplit(':').next().map(str::to_string)).filter(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty()) {
                Some(n) => format!("{base} (line {n})"),
                None => base,
            };
        }
    }
    v.as_string().and_then(|s| s.to_string().ok()).unwrap_or_else(|| "the script threw a value".into())
}

impl Js {
    /// A fresh runtime with the Acrobat object model loaded.
    pub(super) fn new() -> Result<Js, String> {
        let runtime = Runtime::new().map_err(|e| e.to_string())?;
        runtime.set_memory_limit(MEMORY_LIMIT);
        runtime.set_gc_threshold(GC_THRESHOLD);
        runtime.set_max_stack_size(STACK_LIMIT);
        let base = Instant::now();
        let deadline = Arc::new(AtomicU64::new(u64::MAX));
        let d = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || base.elapsed().as_nanos() as u64 > d.load(Ordering::Relaxed))));
        let context = Context::full(&runtime).map_err(|e| e.to_string())?;
        let host = Rc::new(RefCell::new(Host {
            fields: Vec::new(),
            changed: BTreeSet::new(),
            alerts: Vec::new(),
            requests: Vec::new(),
            log: Vec::new(),
            doc: Default::default(),
            field: None,
            seen: HashSet::new(),
        }));
        let js = Js { runtime, context, host, deadline, base };
        js.arm(Duration::from_secs(5));
        let loaded = js.context.with(|ctx| -> Result<(), String> {
            install(&ctx, &js.host).map_err(|e| describe(&ctx, e))?;
            ctx.eval::<(), _>(PRELUDE).map_err(|e| describe(&ctx, e))
        });
        loaded?;
        Ok(js)
    }

    fn arm(&self, within: Duration) {
        let at = self.base.elapsed() + within;
        self.deadline.store(at.as_nanos() as u64, Ordering::Relaxed);
    }

    /// Runs a document-level script as it is (its `var`, `let` and functions stay for the field scripts).
    /// Fields and the document are those of `setup`.
    pub(super) fn run_doc_script(&mut self, name: &str, source: &str, within: Duration) -> Result<(), ScriptError> {
        self.arm(within);
        let r = self.context.with(|ctx| ctx.eval::<(), _>(source.replace('\0', " ")).map_err(|e| describe(&ctx, e)));
        self.disarm();
        r.map_err(|m| self.classify(m, name))
    }

    fn disarm(&self) {
        self.deadline.store(u64::MAX, Ordering::Relaxed);
    }

    /// Replaces what `getField` and `this.numPages` see (before the document's own scripts run).
    pub(super) fn setup(&mut self, fields: &[FieldState], doc: &super::DocInfo) {
        let mut h = self.host.borrow_mut();
        h.fields = fields.to_vec();
        h.doc = doc.clone();
        h.field = None;
        h.changed.clear();
        h.alerts.clear();
        h.requests.clear();
    }

    /// Takes what `run_doc_script` logged.
    pub(super) fn take_log(&mut self) -> Vec<LogLine> {
        std::mem::take(&mut self.host.borrow_mut().log)
    }

    fn classify(&self, message: String, _what: &str) -> ScriptError {
        if Instant::now() >= self.base && self.base.elapsed().as_nanos() as u64 > self.deadline.load(Ordering::Relaxed).min(u64::MAX - 1) || message.contains("interrupted") {
            ScriptError::Timeout
        } else if message.contains("out of memory") {
            ScriptError::Memory
        } else {
            ScriptError::Script(message)
        }
    }

    /// Runs one event script. Whatever it does is returned in the outcome and nothing else changes: on
    /// an error or a timeout the event result is as if no script ran.
    pub(super) fn run(&mut self, job: &Job) -> Outcome {
        let unchanged = |error: Option<ScriptError>| Outcome { rc: true, value: job.event.value.clone(), change: job.event.change.clone(), error, ..Default::default() };
        if job.script.len() > MAX_SCRIPT_BYTES {
            return unchanged(Some(ScriptError::Refused(format!("the script is too long ({} KiB; the limit is {} KiB)", job.script.len() / 1024, MAX_SCRIPT_BYTES / 1024))));
        }
        {
            let mut h = self.host.borrow_mut();
            h.fields = job.fields.clone();
            h.doc = job.doc.clone();
            h.field = Some(job.key.clone());
            h.changed.clear();
            h.alerts.clear();
            h.requests.clear();
            h.log.clear();
        }
        self.arm(job.timeout);
        let event = event_json(&job.event);
        // A block, so that `let` and `const` of one field's script do not collide with another's;
        // `var` and function declarations still reach the global object, as in Acrobat.
        let source = format!("{{\n{}\n}}", job.script.replace('\0', " "));
        let result = self.context.with(|ctx| -> Result<String, String> {
            let g = ctx.globals();
            let begin: Function = g.get("__begin").map_err(|e| describe(&ctx, e))?;
            begin.call::<_, ()>((event,)).map_err(|e| describe(&ctx, e))?;
            let mut o = EvalOptions::default();
            o.strict = false;
            ctx.eval_with_options::<Value, _>(source, o).map_err(|e| describe(&ctx, e))?;
            let end: Function = g.get("__end").map_err(|e| describe(&ctx, e))?;
            end.call::<_, String>(()).map_err(|e| describe(&ctx, e))
        });
        let timed_out = self.base.elapsed().as_nanos() as u64 > self.deadline.load(Ordering::Relaxed);
        self.disarm();
        let (log, alerts, requests, changed) = {
            let mut h = self.host.borrow_mut();
            let changed: Vec<FieldState> = h.changed.iter().map(|i| h.fields[*i].clone()).collect();
            (std::mem::take(&mut h.log), std::mem::take(&mut h.alerts), std::mem::take(&mut h.requests), changed)
        };
        match result {
            Ok(json) => {
                let r: Json = serde_json::from_str(&json).unwrap_or(Json::Null);
                Outcome {
                    rc: r.get("rc").and_then(Json::as_bool).unwrap_or(true),
                    value: truncate(r.get("value").and_then(Json::as_str).unwrap_or("").to_string(), MAX_TEXT_BYTES),
                    change: truncate(r.get("change").and_then(Json::as_str).unwrap_or("").to_string(), MAX_TEXT_BYTES),
                    changed,
                    alerts,
                    requests,
                    log,
                    error: None,
                    skipped: false,
                }
            }
            Err(message) => {
                let error = if timed_out || message.contains("interrupted") {
                    ScriptError::Timeout
                } else if message.contains("out of memory") {
                    ScriptError::Memory
                } else {
                    ScriptError::Script(message)
                };
                // What the script did before it failed is thrown away; what it logged stays.
                Outcome { log, ..unchanged(Some(error)) }
            }
        }
    }

    /// Frees what the last scripts left (after a memory error).
    pub(super) fn collect(&self) {
        self.runtime.run_gc();
    }
}

fn event_json(e: &Event) -> String {
    json!({
        "name": e.name,
        "type": "Field",
        "targetName": e.target,
        "value": e.value,
        "change": e.change,
        "willCommit": e.will_commit,
        "selStart": e.sel_start,
        "selEnd": e.sel_end,
        "commitKey": 0,
        "modifier": false,
        "shift": false,
    })
    .to_string()
}
