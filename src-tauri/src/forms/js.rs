// Adapted from pdfcraft (MIT OR Apache-2.0), crates/js/src/lib.rs @ b8ddf71
// Changes: QuickJS (rquickjs) instead of boa, for an interrupt handler and a heap cap; the object model is
// plain JavaScript (js/prelude.js) over a few host functions (js/engine.rs); one runtime per document on its
// own thread, with time limits and strikes; util.scand added; the AF functions call forms/af.rs.
//! Form JavaScript: the scripts of an AcroForm (`/AA` K, F, V, C, document-level `/Names /JavaScript`)
//! run in a sandboxed QuickJS runtime, one per open document, on a thread of its own.
//!
//! The rest of the forms code talks to a document's [`Scripts`]: it sends a [`Job`] (one event script and
//! a snapshot of the fields) and gets an [`Outcome`] (the event result, the fields the script changed, the
//! alerts and requests it raised). The runtime keeps its globals between events. A script that runs too
//! long is interrupted and its field is switched off for the session; three such timeouts switch all
//! scripts of the document off (the native `af.rs` code still runs the recognised calls). A worker that
//! stops answering is abandoned. See docs/forms-js.md.

mod engine;
mod util;

use parking_lot::Mutex;
use serde::Serialize;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// `display.*`: visible, hidden, noPrint, noView.
pub const DISPLAY_VISIBLE: i32 = 0;
pub const DISPLAY_HIDDEN: i32 = 1;
pub const DISPLAY_NO_PRINT: i32 = 2;
pub const DISPLAY_NO_VIEW: i32 = 3;

/// One script: a keystroke, a format or a validate (250 ms), a calculate (1 s).
pub const EVENT_TIME: Duration = Duration::from_millis(250);
pub const CALC_TIME: Duration = Duration::from_secs(1);
/// All calculations after one change.
pub const CASCADE_TIME: Duration = Duration::from_secs(2);
/// The document-level scripts at open.
pub const OPEN_TIME: Duration = Duration::from_secs(2);
/// How long past a script's own limit the worker may take to answer before it is given up on.
const GRACE: Duration = Duration::from_secs(2);
/// Timeouts after which no script of the document runs.
const MAX_STRIKES: u32 = 3;
/// All the script time one step (a fill with its calculations, a reset) or one listing may use. After it the rest of
/// the step runs natively only (the recognised calls), a strike is counted and the log says so.
pub const JS_BUDGET: Duration = Duration::from_millis(2500);
/// The alerts of one step that reach the UI; the rest are counted in the log.
pub const MAX_ALERTS_PER_STEP: usize = 5;

/// A script longer than this is refused. The document-level scripts together too, at four times as much.
pub const MAX_SCRIPT_BYTES: usize = 256 << 10;
pub const MAX_DOC_SCRIPT_BYTES: usize = 1 << 20;
/// Strings crossing from a script to the host (a value, a change, an alert, printf output) are cut here.
pub const MAX_TEXT_BYTES: usize = 64 << 10;
const MAX_LOG_LINES: usize = 256;
const MAX_LOG_PER_JOB: usize = 64;
const MAX_ALERTS_PER_JOB: usize = 16;
/// `util.printf` output and field width and precision.
const MAX_PRINTF_OUTPUT_BYTES: usize = MAX_TEXT_BYTES;
const MAX_PRINTF_WIDTH_PRECISION: usize = 4096;
/// The worker's stack. The script's own is capped lower, inside QuickJS.
const WORKER_STACK: usize = 8 << 20;

/// The kinds of field, as `field.type` names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    Text,
    CheckBox,
    RadioButton,
    ComboBox,
    ListBox,
    Button,
    Signature,
}

impl FieldType {
    pub fn name(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::CheckBox => "checkbox",
            FieldType::RadioButton => "radiobutton",
            FieldType::ComboBox => "combobox",
            FieldType::ListBox => "listbox",
            FieldType::Button => "button",
            FieldType::Signature => "signature",
        }
    }
}

/// A field as scripts see it (one per field, not per widget).
#[derive(Clone, Debug, PartialEq)]
pub struct FieldState {
    pub name: String,
    pub kind: FieldType,
    /// Text: the text. Check box and radio: the selected export value (empty when off). Choice: the
    /// selected export values.
    pub value: Vec<String>,
    pub default: Vec<String>,
    pub readonly: bool,
    pub required: bool,
    pub display: i32,
    /// Choice options (export value, shown text); check boxes and radios: their export values, one per widget.
    pub options: Vec<(String, String)>,
    pub char_limit: Option<usize>,
    /// Acrobat colour arrays (`["RGB", r, g, b]`, `["G", g]`, `["CMYK", ...]`); `None` is transparent.
    pub text_color: Option<Vec<String>>,
    pub fill_color: Option<Vec<String>>,
    pub stroke_color: Option<Vec<String>>,
}

/// A field property a script can set. Only the properties a script touched are written back to the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prop {
    Value,
    ReadOnly,
    Required,
    Display,
    TextColor,
    FillColor,
    StrokeColor,
}

/// A field a script wrote to: the field as the script left it and the properties it set.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldChange {
    pub state: FieldState,
    pub props: BTreeSet<Prop>,
}

/// About the document, for `this.numPages`, `this.info` and the like.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocInfo {
    pub file_name: String,
    pub num_pages: usize,
    /// The current page, 0-based.
    pub page: usize,
    /// Document information (`this.info.title` ...): lower-case key, value.
    pub info: Vec<(String, String)>,
}

/// The event a script runs for (`event.*`).
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// `Keystroke`, `Validate`, `Calculate`, `Format`.
    pub name: &'static str,
    /// The field's full name.
    pub target: Option<String>,
    pub value: String,
    pub change: String,
    pub will_commit: bool,
    pub sel_start: f64,
    pub sel_end: f64,
}

impl Event {
    pub fn new(name: &'static str, target: &str, value: &str) -> Event {
        Event { name, target: Some(target.into()), value: value.into(), change: String::new(), will_commit: true, sel_start: 0.0, sel_end: 0.0 }
    }
}

/// One script run.
#[derive(Clone, Debug)]
pub struct Job {
    /// What is switched off if the script times out, and what the log names: the field's full name.
    pub key: String,
    pub event: Event,
    pub script: String,
    pub fields: Vec<FieldState>,
    pub doc: DocInfo,
    pub timeout: Duration,
}

/// Something a script asked the viewer to do. The engine never does any of it: the UI asks first, and
/// only http, https and mailto links come through.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Request {
    /// `this.resetForm([names])` (empty: every field).
    Reset { fields: Vec<String> },
    Print,
    /// `this.pageNum = n` (0-based).
    GoToPage { page: usize },
    LaunchUrl { url: String },
    /// `this.submitForm(url)`: never sent by Inkwell.
    Submit { url: String },
    /// `field.setFocus()`.
    Focus { field: String },
    Beep,
}

/// `app.alert(...)`: shown by the UI, never a native dialog.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// A line of the document's script log (the Scripts note on the forms rail).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub level: LogLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ScriptError {
    /// Interrupted at its time limit.
    Timeout,
    /// Hit the heap cap.
    Memory,
    /// A syntax error or an exception.
    Script(String),
    /// Not run: too long.
    Refused(String),
    /// The worker panicked (its runtime is rebuilt).
    Panic,
    /// The worker did not answer in time and was given up on.
    Lost,
}

/// What running a script did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// `event.rc`: false rejects the keystroke or the value.
    pub rc: bool,
    /// `event.value` after the script.
    pub value: String,
    pub change: String,
    /// Fields the script wrote to, as they are now, with the properties it set.
    pub changed: Vec<FieldChange>,
    pub alerts: Vec<Alert>,
    pub requests: Vec<Request>,
    pub log: Vec<LogLine>,
    pub error: Option<ScriptError>,
    /// Not run (scripts are off, or this field's script is).
    pub skipped: bool,
}

/// The document-level scripts (`/Names /JavaScript`), in name order.
#[derive(Clone, Debug, Default)]
pub struct DocScripts {
    pub scripts: Vec<(String, String)>,
    /// A script defines or assigns an `AF...` function, so the native fast path would run the wrong code.
    pub overrides_af: bool,
}

/// What may define or replace an `AF...` function: a declaration, an assignment or property named `AF...`, a string
/// that starts with `AF` (a computed name), any assignment to a bracketed name, and the calls that define properties
/// or evaluate text. Conservative on purpose: the engine path gives the same answers as the native one, only slower.
const OVERRIDES_AF: &str = concat!(
    r"\bfunction\s+AF\w+",
    r"|\b(?:var|let|const)\s+AF\w+",
    r"|\bAF[A-Za-z0-9_]+\s*(?:=[^=]|:)",
    r"|\]\s*=[^=]",
    r#"|["'`]\s*AF"#,
    r"|\b(?:defineProperty|defineProperties|__defineGetter__|__defineSetter__|setPrototypeOf|assign|Reflect|Proxy|eval|Function|with)\b",
);

impl DocScripts {
    pub fn new(scripts: Vec<(String, String)>) -> DocScripts {
        static RE: OnceLock<regex::Regex> = OnceLock::new();
        let re = RE.get_or_init(|| regex::Regex::new(OVERRIDES_AF).expect("a valid pattern"));
        let overrides_af = scripts.iter().any(|(_, s)| re.is_match(s));
        DocScripts { scripts, overrides_af }
    }
}

// ---- the per-document handle ----------------------------------------------------------------------

enum Msg {
    Run(Box<Job>, Sender<Outcome>),
}

struct Worker {
    tx: Sender<Msg>,
    /// Set by the worker while it builds a new runtime after a panic or a stale queue: the next job gets that time on top.
    rebuilding: Arc<AtomicBool>,
    /// Kept to lower the priority of a worker that is given up on (it may be spinning where no interrupt reaches).
    thread: std::thread::JoinHandle<()>,
}

impl Worker {
    /// A worker that stopped answering is left to run on its own: at the lowest priority there is.
    fn demote(&self) {
        demote(&self.thread);
    }
}

/// Lowers a worker thread to idle priority (Windows; elsewhere the OS decides).
fn demote(thread: &std::thread::JoinHandle<()>) {
    #[cfg(windows)]
    // SAFETY: the handle belongs to a thread this process started and is open as long as the JoinHandle is.
    unsafe {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Threading::{SetThreadPriority, THREAD_PRIORITY_IDLE};
        SetThreadPriority(thread.as_raw_handle() as _, THREAD_PRIORITY_IDLE);
    }
    #[cfg(not(windows))]
    let _ = thread;
}

/// The worker's own priority: a little below the UI's, so a script cannot take the machine from typing and drawing.
fn be_polite() {
    #[cfg(windows)]
    // SAFETY: plain calls on the current thread's pseudo handle.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[derive(Default)]
struct Meta {
    /// Off for a session when the person turns scripts off for the file. (Acrobat-like default: on.)
    off: bool,
    log: VecDeque<LogLine>,
    /// Fields whose script timed out: not run again this session.
    disabled: HashSet<String>,
    strikes: u32,
    /// Three timeouts, or a worker that stopped answering.
    all_off: bool,
    /// A worker was given up on while it was running: its thread may still be spinning (a native call QuickJS cannot
    /// interrupt). One is all the app gets per file: scripts are not turned on again for it until the app exits
    /// (see [`LOST_FILES`]).
    lost: bool,
}

/// The files (by path) a worker was given up on while it ran, for the life of the app: opening one again must not
/// start another thread that may spin.
static LOST_FILES: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// The key of a file in [`LOST_FILES`]: its canonical path where there is one (lower case on Windows).
fn lost_key(path: &str) -> String {
    let p = std::fs::canonicalize(path).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string());
    if cfg!(windows) { p.to_lowercase() } else { p }
}

/// The scripts of one open document: the on/off switch, the log and the runtime (started on first use).
#[derive(Default)]
pub struct Scripts {
    meta: Mutex<Meta>,
    worker: Mutex<Option<Worker>>,
    doc_js: Mutex<Option<Arc<DocScripts>>>,
    /// Whether the document has scripts the engine runs (kept by the listing, so the banner needs no edit lane).
    has_scripts: Mutex<Option<bool>>,
    /// The file's key in [`LOST_FILES`] (empty: not tracked).
    lost_key: String,
}

impl Scripts {
    pub fn new() -> Scripts {
        Scripts::default()
    }

    /// The scripts of the document at `path`. A file whose worker was given up on earlier in this app's life starts
    /// with scripts off and refused: no second thread is started for it.
    pub fn for_path(path: &str) -> Scripts {
        let key = lost_key(path);
        let was_lost = LOST_FILES.lock().as_ref().is_some_and(|s| s.contains(&key));
        let s = Scripts { lost_key: key, ..Scripts::default() };
        if was_lost {
            {
                let mut m = s.meta.lock();
                m.lost = true;
                m.off = true;
                m.all_off = true;
            }
            s.note(LogLevel::Error, None, "scripts are off for this file: its script engine stopped answering earlier, and cannot be started again until the app exits");
        }
        s
    }

    /// Remembers for the life of the app that this file's worker was given up on.
    fn mark_lost(&self) {
        self.meta.lock().lost = true;
        if !self.lost_key.is_empty() {
            LOST_FILES.lock().get_or_insert_with(HashSet::new).insert(self.lost_key.clone());
        }
    }

    /// Scripts run for this document (the person has not turned them off, and none has taken them all down).
    pub fn enabled(&self) -> bool {
        !self.meta.lock().off
    }

    /// True when the runtime may be used now (`enabled` and not taken down by timeouts).
    pub fn usable(&self) -> bool {
        let m = self.meta.lock();
        !m.off && !m.all_off
    }

    /// Scripts may be turned on (again): not once a worker was given up on while it ran.
    pub fn can_enable(&self) -> bool {
        !self.meta.lock().lost
    }

    /// Whether the document has scripts, as the last listing saw it (`None` before one).
    pub fn has_scripts(&self) -> Option<bool> {
        *self.has_scripts.lock()
    }

    pub fn set_has_scripts(&self, has: bool) {
        *self.has_scripts.lock() = Some(has);
    }

    /// The person turns scripts on or off for this file. Turning them on gives a runtime that was taken down
    /// a fresh start: strikes and switched-off fields are forgotten, a worker given up on is replaced. Returns
    /// false when they cannot be turned on (see [`Scripts::can_enable`]); they stay off.
    pub fn set_enabled(&self, on: bool) -> bool {
        let rebuild = {
            let mut m = self.meta.lock();
            if on && m.lost {
                m.off = true;
                push_log(
                    &mut m.log,
                    [LogLine {
                        level: LogLevel::Error,
                        field: None,
                        message: "scripts cannot be turned on again in this file: the script engine stopped answering once and is left abandoned until the app exits".into(),
                    }],
                );
                return false;
            }
            m.off = !on;
            if on {
                m.strikes = 0;
                m.disabled.clear();
                let was_down = m.all_off;
                m.all_off = false;
                was_down
            } else {
                false
            }
        };
        if rebuild {
            *self.worker.lock() = None;
        }
        true
    }

    pub fn log(&self) -> Vec<LogLine> {
        self.meta.lock().log.iter().cloned().collect()
    }

    pub fn note(&self, level: LogLevel, field: Option<&str>, message: impl Into<String>) {
        let line = LogLine { level, field: field.map(str::to_string), message: message.into() };
        push_log(&mut self.meta.lock().log, [line]);
    }

    /// The document-level scripts: read once with `load` (the caller reads the document), then kept.
    pub fn doc_scripts(&self, load: impl FnOnce() -> DocScripts) -> Arc<DocScripts> {
        let mut g = self.doc_js.lock();
        g.get_or_insert_with(|| Arc::new(load())).clone()
    }

    /// A step used up its script time ([`JS_BUDGET`]): that counts like a timeout toward taking the scripts down.
    pub fn over_budget(&self) {
        let mut m = self.meta.lock();
        m.strikes += 1;
        let mut lines = vec![LogLine {
            level: LogLevel::Error,
            field: None,
            message: format!("a fill, reset or listing used more than {} ms of script time; the rest of it ran without scripts", JS_BUDGET.as_millis()),
        }];
        if m.strikes >= MAX_STRIKES {
            m.all_off = true;
            lines.push(LogLine { level: LogLevel::Error, field: None, message: STRIKES_OUT.into() });
        }
        push_log(&mut m.log, lines);
    }

    /// Runs `job`. Never blocks longer than the job's time limit plus two seconds, never panics, and leaves
    /// the event result as it was on any failure.
    pub fn run(&self, job: Job) -> Outcome {
        let (key, value, change) = (job.key.clone(), job.event.value.clone(), job.event.change.clone());
        let skipped = |error: Option<ScriptError>| Outcome { rc: true, value: value.clone(), change: change.clone(), skipped: error.is_none(), error, ..Default::default() };
        {
            let m = self.meta.lock();
            if m.off || m.all_off || m.disabled.contains(&key) {
                return skipped(None);
            }
        }
        if job.script.len() > MAX_SCRIPT_BYTES {
            let why = format!("the script is too long ({} KiB; the limit is {} KiB)", job.script.len() / 1024, MAX_SCRIPT_BYTES / 1024);
            self.note(LogLevel::Error, Some(&key), format!("{why}, it was not run"));
            return skipped(Some(ScriptError::Refused(why)));
        }
        let mut worker = self.worker.lock();
        if worker.is_none() {
            let doc_js = self.doc_js.lock().clone().unwrap_or_default();
            match spawn(doc_js, &job.fields, &job.doc) {
                Ok((w, log)) => {
                    push_log(&mut self.meta.lock().log, log);
                    *worker = Some(w);
                }
                Err((why, abandoned)) => {
                    self.take_down(&format!("the script engine could not start: {why}"));
                    if abandoned {
                        self.mark_lost();
                    }
                    return skipped(Some(ScriptError::Lost));
                }
            }
        }
        let (rtx, rrx) = channel();
        let timeout = job.timeout;
        let sent = worker.as_ref().is_some_and(|w| w.tx.send(Msg::Run(Box::new(job), rtx)).is_ok());
        // A worker still building its new runtime (after a panic, or a queue it had to drop) has not begun this job:
        // that time is not the script's, and a slow rebuild is not a lost worker.
        let rebuild = if worker.as_ref().is_some_and(|w| w.rebuilding.load(Ordering::SeqCst)) { OPEN_TIME + GRACE } else { Duration::ZERO };
        let answer = if sent { rrx.recv_timeout(timeout + GRACE + rebuild) } else { Err(RecvTimeoutError::Disconnected) };
        let mut out = match answer {
            Ok(o) => o,
            Err(why) => {
                // The worker is stuck or gone: give up on it, and on scripts, until the person turns them back on.
                let stuck = matches!(why, RecvTimeoutError::Timeout);
                if stuck {
                    // Still running, where no interrupt reaches: at the lowest priority, and the only one this file gets.
                    if let Some(w) = worker.as_ref() {
                        w.demote();
                    }
                    self.mark_lost();
                }
                *worker = None;
                if stuck {
                    self.take_down("the script engine stopped answering; scripts are off for this file, and cannot be turned on again in this session (the stuck script is left running at the lowest priority until the app exits)");
                } else {
                    self.take_down("the script engine stopped; scripts are off for this file until you turn them on again");
                }
                return Outcome { rc: true, value: value.clone(), change: change.clone(), error: Some(ScriptError::Lost), ..Default::default() };
            }
        };
        drop(worker);
        let mut lines = std::mem::take(&mut out.log);
        let mut m = self.meta.lock();
        match &out.error {
            Some(ScriptError::Timeout) => {
                m.disabled.insert(key.clone());
                m.strikes += 1;
                lines.push(LogLine {
                    level: LogLevel::Error,
                    field: Some(key.clone()),
                    message: format!("the script ran longer than {} ms and was stopped; its scripts are off for this session", timeout.as_millis()),
                });
                if m.strikes >= MAX_STRIKES {
                    m.all_off = true;
                    lines.push(LogLine { level: LogLevel::Error, field: None, message: STRIKES_OUT.into() });
                }
            }
            Some(ScriptError::Memory) => lines.push(LogLine { level: LogLevel::Error, field: Some(key.clone()), message: "the script used more memory than it may and was stopped".into() }),
            Some(ScriptError::Script(msg)) => lines.push(LogLine { level: LogLevel::Error, field: Some(key.clone()), message: msg.clone() }),
            Some(ScriptError::Panic) => {
                // A runtime that keeps failing is no better than one that keeps timing out.
                m.strikes += 1;
                lines.push(LogLine { level: LogLevel::Error, field: Some(key.clone()), message: "the script engine failed and was restarted".into() });
                if m.strikes >= MAX_STRIKES {
                    m.all_off = true;
                    lines.push(LogLine { level: LogLevel::Error, field: None, message: STRIKES_OUT.into() });
                }
            }
            _ => {}
        }
        push_log(&mut m.log, lines);
        out
    }

    fn take_down(&self, why: &str) {
        let mut m = self.meta.lock();
        m.all_off = true;
        push_log(&mut m.log, [LogLine { level: LogLevel::Error, field: None, message: why.to_string() }]);
    }

    /// Test hook: the log as one string per line.
    #[cfg(test)]
    pub fn log_text(&self) -> Vec<String> {
        self.log().into_iter().map(|l| l.message).collect()
    }
}

const STRIKES_OUT: &str = "three scripts timed out, used up their time or crashed the engine: scripts are off for this file (the recognised calls still run natively)";

fn push_log(log: &mut VecDeque<LogLine>, lines: impl IntoIterator<Item = LogLine>) {
    for l in lines {
        if log.len() >= MAX_LOG_LINES {
            log.pop_front();
        }
        log.push_back(l);
    }
}

// ---- the worker thread ----------------------------------------------------------------------------

/// A runtime with the document-level scripts run in it, in name order, within [`OPEN_TIME`] altogether.
fn build(doc_js: &DocScripts, fields: &[FieldState], doc: &DocInfo, log: &mut Vec<LogLine>) -> Result<engine::Js, String> {
    let mut js = engine::Js::new()?;
    js.setup(fields, doc);
    let started = Instant::now();
    let mut total = 0usize;
    for (name, src) in &doc_js.scripts {
        total += src.len();
        if src.len() > MAX_SCRIPT_BYTES || total > MAX_DOC_SCRIPT_BYTES {
            log.push(LogLine { level: LogLevel::Error, field: None, message: format!("document script \"{name}\" was not run: the document scripts together may be 1 MiB, one 256 KiB") });
            continue;
        }
        let left = OPEN_TIME.saturating_sub(started.elapsed());
        if left.is_zero() {
            log.push(LogLine { level: LogLevel::Error, field: None, message: format!("document script \"{name}\" was not run: the scripts took more than {} s to load", OPEN_TIME.as_secs()) });
            continue;
        }
        if let Err(e) = js.run_doc_script(name, src, left) {
            let message = match e {
                ScriptError::Timeout => format!("document script \"{name}\" ran too long and was stopped"),
                ScriptError::Memory => format!("document script \"{name}\" used more memory than it may and was stopped"),
                ScriptError::Script(m) => format!("document script \"{name}\": {m}"),
                other => format!("document script \"{name}\" failed: {other:?}"),
            };
            log.push(LogLine { level: LogLevel::Error, field: None, message });
        }
        log.extend(js.take_log());
    }
    Ok(js)
}

/// A worker thread with its runtime built. `Err` has the reason and whether the thread was left running.
fn spawn(doc_js: Arc<DocScripts>, fields: &[FieldState], doc: &DocInfo) -> Result<(Worker, Vec<LogLine>), (String, bool)> {
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = channel();
    let (itx, irx) = channel();
    let rebuilding = Arc::new(AtomicBool::new(false));
    let flag = rebuilding.clone();
    let (fields, doc) = (fields.to_vec(), doc.clone());
    let thread = std::thread::Builder::new()
        .name("forms-js".into())
        .stack_size(WORKER_STACK)
        .spawn(move || work(rx, itx, doc_js, fields, doc, flag))
        .map_err(|e| (e.to_string(), false))?;
    match irx.recv_timeout(OPEN_TIME + GRACE) {
        Ok(Ok(log)) => Ok((Worker { tx, thread, rebuilding }, log)),
        Ok(Err(e)) => Err((e, false)),
        Err(RecvTimeoutError::Timeout) => {
            demote(&thread);
            Err(("it did not start in time".into(), true))
        }
        Err(RecvTimeoutError::Disconnected) => Err(("it stopped while starting".into(), false)),
    }
}

/// The reply for a job whose runtime failed.
fn panicked(job: &Job) -> Outcome {
    Outcome { rc: true, value: job.event.value.clone(), change: job.event.change.clone(), error: Some(ScriptError::Panic), ..Default::default() }
}

/// A new runtime in place of one that panicked or was left with jobs it could not finish. Its globals are the
/// document scripts' again. `None` when that fails too.
fn rebuilt(doc_js: &DocScripts, job: &Job) -> Option<engine::Js> {
    let mut ignore = Vec::new();
    catch_unwind(AssertUnwindSafe(|| build(doc_js, &job.fields, &job.doc, &mut ignore))).ok().and_then(Result::ok)
}

fn work(rx: Receiver<Msg>, init: Sender<Result<Vec<LogLine>, String>>, doc_js: Arc<DocScripts>, fields: Vec<FieldState>, doc: DocInfo, rebuilding: Arc<AtomicBool>) {
    be_polite();
    let mut log = Vec::new();
    let built = catch_unwind(AssertUnwindSafe(|| build(&doc_js, &fields, &doc, &mut log)));
    let mut js = match built {
        Ok(Ok(js)) => js,
        Ok(Err(e)) => {
            let _ = init.send(Err(e));
            return;
        }
        Err(_) => {
            let _ = init.send(Err("it failed while starting".into()));
            return;
        }
    };
    let _ = init.send(Ok(log));
    while let Ok(Msg::Run(job, reply)) = rx.recv() {
        match catch_unwind(AssertUnwindSafe(|| js.run(&job))) {
            Ok(out) => {
                if matches!(out.error, Some(ScriptError::Memory)) {
                    js.collect();
                }
                // Jobs a script queued and the deadline cut short (a promise callback that never ends) are not left for
                // the next script to run: that runtime is dropped and another started. The flag is up before the reply
                // goes, so the next job's wait cannot miss it.
                let stale = js.stale();
                if stale {
                    rebuilding.store(true, Ordering::SeqCst);
                }
                let _ = reply.send(out);
                if stale {
                    let fresh = rebuilt(&doc_js, &job);
                    rebuilding.store(false, Ordering::SeqCst);
                    match fresh {
                        Some(fresh) => js = fresh,
                        None => return,
                    }
                }
            }
            Err(_) => {
                // Drop the runtime that panicked and start another, with the document scripts again.
                rebuilding.store(true, Ordering::SeqCst);
                let _ = reply.send(panicked(&job));
                let fresh = rebuilt(&doc_js, &job);
                rebuilding.store(false, Ordering::SeqCst);
                match fresh {
                    Some(fresh) => js = fresh,
                    None => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
