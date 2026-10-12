//! The one place a model's tool call is checked and run, whichever provider it came from: finds
//! the tool, applies the permission mode (waiting for the user when it needs to), runs it through
//! Inkwell's own code and reports every step as an `Activity`. Every outcome, a refusal included,
//! is a value the caller hands back to the model; a turn is never left hanging.
//!
//! Codex reaches it from its stream loop (`item/tool/call`), Claude from the MCP server's request
//! threads. Neither provider's wire format is in here.

use super::chat::{ActState, ChatEvent, Sink, TurnCtx};
use super::tools::{self, Class, Ctx, Mode, Tool};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// How a call that needed the user's say ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Apply,
    Reject,
    /// No answer within the limit: final, a late Apply is ignored.
    TimedOut,
    /// The turn was stopped or the tab closed while the card waited.
    Stopped,
}

/// What a document's session shares with its tool calls.
pub(super) struct Gate<'a> {
    pub tools: &'a RwLock<Vec<Arc<Tool>>>,
    /// Tool calls waiting for Apply / Reject, by call id.
    pub approvals: &'a Mutex<HashMap<String, Sender<bool>>>,
    /// Read at each call, not at the start of the turn: the user may have tightened it since.
    pub mode: &'a Mutex<Mode>,
    pub cancel: &'a AtomicBool,
    pub closed: &'a AtomicBool,
    /// How long a card may wait, in ms.
    pub approval_ms: u64,
}

/// Arguments as the model sent them: some models hand them over as a JSON string.
pub(super) fn parse_args(v: Option<&Value>) -> Value {
    let args = match v {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    };
    if args.is_null() { json!({}) } else { args }
}

/// Runs one call. `page` is the page in view, which `go_to_page` and `show_source` move within the turn.
pub(super) fn run_call(
    gate: &Gate,
    call_id: &str,
    name: &str,
    args: &Value,
    ctx: &TurnCtx,
    page: &mut u32,
    sink: &Sink,
) -> Result<tools::Output, String> {
    let tool = gate.tools.read().unwrap().iter().find(|t| t.name == name).cloned();
    let Some(tool) = tool else {
        let why = format!("Unknown tool \"{name}\".");
        sink(ChatEvent::Activity {
            id: call_id.to_string(),
            tool: name.to_string(),
            class: Class::Read,
            label: "Called an unknown tool".into(),
            state: ActState::Failed,
            detail: Some(why.clone()),
        });
        return Err(why);
    };
    let label = (tool.label)(args);
    let act = |state: ActState, detail: Option<String>| {
        sink(ChatEvent::Activity {
            id: call_id.to_string(),
            tool: name.to_string(),
            class: tool.class,
            label: label.clone(),
            state,
            detail,
        });
    };
    if gate.closed.load(Ordering::SeqCst) || gate.cancel.load(Ordering::SeqCst) {
        act(ActState::Failed, Some("stopped".into()));
        return Err("The request was stopped.".into());
    }
    let progress = |s: String| act(ActState::Running, Some(s));
    let tc = Ctx {
        text: ctx.text,
        host: ctx.host,
        name: &ctx.req.name,
        current_page: *page,
        selection: ctx.req.selection.as_ref(),
        cancel: gate.cancel,
        progress: &progress,
    };
    // a change that cannot happen is refused before the user is asked, and the card says exactly
    // what will change
    let mut card = None;
    if let Some(check) = &tool.check {
        match catch_unwind(AssertUnwindSafe(|| check(&tc, args))) {
            Ok(Ok(detail)) => card = detail,
            Ok(Err(e)) => {
                act(ActState::Failed, Some(e.clone()));
                return Err(e);
            }
            Err(_) => {
                let e = "The tool failed unexpectedly.".to_string();
                act(ActState::Failed, Some(e.clone()));
                return Err(e);
            }
        }
    }
    let mode = *gate.mode.lock().unwrap();
    if mode.needs_approval(tool.class) {
        // registered before the card is announced: the answer may come back at once
        let (tx, rx) = mpsc::channel();
        gate.approvals.lock().unwrap().insert(call_id.to_string(), tx);
        act(ActState::Waiting, card);
        let (why, text) = match await_decision(gate, call_id, rx) {
            Decision::Apply => (None, None),
            Decision::Reject => (
                None,
                Some(
                    "The user rejected this change, so nothing was changed. Do not try it again; \
                     say what you would have done and ask how they want to proceed.",
                ),
            ),
            Decision::TimedOut => (
                Some("no answer in time"),
                Some(
                    "The user did not answer in time, so the change was NOT made and cannot be made any more. \
                     Do not try it again unless they ask; say what you would have done.",
                ),
            ),
            Decision::Stopped => (Some("stopped"), Some("The request was stopped, so nothing was changed.")),
        };
        if let Some(text) = text {
            act(ActState::Rejected, why.map(str::to_string));
            return Err(text.into());
        }
    }
    act(ActState::Running, None);
    let out = catch_unwind(AssertUnwindSafe(|| (tool.run)(&tc, args)))
        .unwrap_or_else(|_| Err("The tool failed unexpectedly.".into()));
    match &out {
        Ok(o) => {
            if let Some(ui) = &o.ui {
                if let Some(p) = ui.page() {
                    *page = p;
                }
                sink(ChatEvent::Ui { action: ui.clone() });
            }
            act(ActState::Done, None);
        }
        Err(e) => act(ActState::Failed, Some(e.clone())),
    }
    out
}

/// Waits for the user's Apply / Reject. A cancel, a closed tab and the timeout each end it, and
/// the call is final then: the pending entry is removed, so a late Apply finds nothing to answer.
fn await_decision(gate: &Gate, call_id: &str, rx: Receiver<bool>) -> Decision {
    let deadline = Instant::now() + Duration::from_millis(gate.approval_ms);
    let decision = loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(true) => break Decision::Apply,
            Ok(false) => break Decision::Reject,
            Err(RecvTimeoutError::Timeout) => {
                if gate.cancel.load(Ordering::SeqCst) || gate.closed.load(Ordering::SeqCst) {
                    break Decision::Stopped;
                }
                if Instant::now() > deadline {
                    break Decision::TimedOut;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break Decision::Stopped,
        }
    };
    // `decide` takes the sender out of this map: once it is gone (or the receiver dropped), a
    // late answer is a no-op. Removing under the lock settles a decide racing the timeout.
    gate.approvals.lock().unwrap().remove(call_id);
    decision
}
