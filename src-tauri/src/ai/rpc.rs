//! JSONL JSON-RPC over a child process's stdio (how `codex app-server` talks). No Tauri in here, so
//! tests can drive it with a stub server.
//!
//! One reader thread routes everything the server says: responses to the waiting `call`, server
//! requests (approvals, tool calls) are answered with an error so nothing ever blocks, and
//! notifications go to the sink registered for their `threadId`. One more thread drains stderr
//! (a full pipe stalls the child) and keeps the tail for error messages.

use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, thiserror::Error)]
pub enum RpcError {
    #[error("{0}")]
    Spawn(String),
    #[error("codex app-server is not running{}", .0.as_deref().map(|t| format!(": {t}")).unwrap_or_default())]
    Closed(Option<String>),
    #[error("codex did not answer in time")]
    Timeout,
    /// The server answered with a JSON-RPC error object.
    #[error("{message}")]
    Remote { code: i64, message: String },
}

/// What to start. Injectable so tests can run a stub instead of the real codex.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// Notification sent to a thread's sink when the process went away.
pub const CLOSED: &str = "__closed__";

pub struct Client {
    pub pid: u32,
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, Sender<Result<Value, RpcError>>>>,
    sinks: Mutex<HashMap<String, Sender<Value>>>,
    alive: AtomicBool,
    stderr_tail: Mutex<VecDeque<String>>,
}

impl Client {
    pub fn spawn(spec: &SpawnSpec) -> Result<Arc<Client>, RpcError> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| RpcError::Spawn(format!("could not start {}: {e}", spec.program.display())))?;
        job::adopt(&child);
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let stdin = child.stdin.take().expect("piped");
        let c = Arc::new(Client {
            pid: child.id(),
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            next_id: AtomicI64::new(0),
            pending: Mutex::new(HashMap::new()),
            sinks: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
            stderr_tail: Mutex::new(VecDeque::new()),
        });
        let r = c.clone();
        std::thread::Builder::new().name("ai-rpc-read".into()).spawn(move || r.read_loop(stdout)).ok();
        let r = c.clone();
        std::thread::Builder::new().name("ai-rpc-err".into()).spawn(move || r.drain_stderr(stderr)).ok();
        Ok(c)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Last lines the server wrote to stderr (for error messages).
    pub fn stderr_tail(&self) -> String {
        self.stderr_tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n")
    }

    pub fn kill(&self) {
        let _ = self.child.lock().unwrap().kill();
    }

    /// Sends a request and waits for its response.
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        // The reader clears `alive` before it drains `pending`, so checking after the insert cannot miss a death.
        if !self.is_alive() || self.send(&json!({ "id": id, "method": method, "params": params })).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(self.closed());
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(RpcError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(self.closed()),
        }
    }

    pub fn notify(&self, method: &str, params: Option<Value>) -> Result<(), RpcError> {
        let mut m = json!({ "method": method });
        if let Some(p) = params {
            m["params"] = p;
        }
        self.send(&m).map_err(|_| self.closed())
    }

    /// Notifications for `thread_id` arrive on the returned receiver until `unsubscribe`.
    pub fn subscribe(&self, thread_id: &str) -> Receiver<Value> {
        let (tx, rx) = mpsc::channel();
        self.sinks.lock().unwrap().insert(thread_id.to_string(), tx);
        if !self.is_alive() {
            self.sinks.lock().unwrap().remove(thread_id);
        }
        rx
    }

    pub fn unsubscribe(&self, thread_id: &str) {
        self.sinks.lock().unwrap().remove(thread_id);
    }

    fn closed(&self) -> RpcError {
        let t = self.stderr_tail();
        RpcError::Closed(if t.trim().is_empty() { None } else { Some(t) })
    }

    fn send(&self, msg: &Value) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(msg).expect("serializable");
        line.push(b'\n');
        let mut w = self.stdin.lock().unwrap();
        w.write_all(&line)?;
        w.flush()
    }

    fn drain_stderr(&self, stderr: impl std::io::Read) {
        for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            let mut t = self.stderr_tail.lock().unwrap();
            t.push_back(String::from_utf8_lossy(&line).trim_end().to_string());
            if t.len() > 50 {
                t.pop_front();
            }
        }
    }

    fn read_loop(&self, stdout: impl std::io::Read) {
        let mut r = BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim();
            if !line.starts_with('{') {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                self.route(v);
            }
        }
        self.alive.store(false, Ordering::SeqCst);
        let err = self.closed();
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(err.clone()));
        }
        for (id, tx) in self.sinks.lock().unwrap().drain() {
            let _ = tx.send(json!({ "method": CLOSED, "params": { "threadId": id } }));
        }
        let _ = self.child.lock().unwrap().wait();
    }

    fn route(&self, v: Value) {
        let method = v.get("method").and_then(Value::as_str);
        match (method, v.get("id")) {
            (Some(m), Some(id)) => {
                // approvals / tool calls / elicitations: refuse everything, never block
                let reply = json!({ "id": id, "error": { "code": -32601, "message": format!("inkwell does not support {m}") } });
                let _ = self.send(&reply);
            }
            (Some(_), None) => {
                let tid = v.pointer("/params/threadId").and_then(Value::as_str).map(str::to_string);
                if let Some(tid) = tid {
                    let dead = match self.sinks.lock().unwrap().get(&tid) {
                        Some(tx) => tx.send(v).is_err(),
                        None => false,
                    };
                    if dead {
                        self.sinks.lock().unwrap().remove(&tid);
                    }
                }
            }
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { return };
                let Some(tx) = self.pending.lock().unwrap().remove(&id) else { return };
                let out = match v.get("error") {
                    Some(e) => Err(RpcError::Remote {
                        code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: e.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string(),
                    }),
                    None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(out);
            }
            (None, None) => {}
        }
    }
}

/// A Job object with KILL_ON_JOB_CLOSE: the server dies with Inkwell, however Inkwell dies.
#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    static JOB: OnceLock<usize> = OnceLock::new();

    fn handle() -> HANDLE {
        *JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 { 0 } else { h as usize }
        }) as HANDLE
    }

    pub fn adopt(child: &std::process::Child) {
        let h = handle();
        if !h.is_null() {
            unsafe {
                AssignProcessToJobObject(h, child.as_raw_handle() as HANDLE);
            }
        }
    }
}

#[cfg(not(windows))]
mod job {
    pub fn adopt(_: &std::process::Child) {}
}
