//! JSONL JSON-RPC over a child process's stdio (how `codex app-server` talks). No Tauri in here, so
//! tests can drive it with a stub server.
//!
//! One reader thread routes everything the server says: responses to the waiting `call`, server
//! requests (approvals, elicitations) are answered with an error so nothing ever blocks, dynamic
//! tool calls (`item/tool/call`) are handed to the sink of their thread (the turn answers them
//! with `respond`; a thread nobody listens to gets a refusal), and notifications go to the sink
//! registered for their `threadId`. Tool handlers never run on the reader thread: they may wait
//! for the user. One writer thread owns stdin, fed
//! by a channel, so a slow pipe never blocks a caller or the reader. One more thread drains stderr
//! (a full pipe stalls the child) and keeps the tail for error messages.

use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, Weak};
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
/// The one server request Inkwell answers: a call of one of the dynamic tools it registered.
pub const TOOL_CALL: &str = "item/tool/call";

pub struct Client {
    pub pid: u32,
    /// Lines for the writer thread (the only owner of stdin).
    outbox: Sender<Vec<u8>>,
    child: Mutex<Child>,
    /// This server's own Job object: `kill` takes grandchildren with it.
    job: Option<job::Job>,
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
        let job = job::Job::new();
        if let Some(j) = &job {
            j.adopt(&child);
        }
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let stdin = child.stdin.take().expect("piped");
        let (outbox, lines) = mpsc::channel();
        let c = Arc::new(Client {
            pid: child.id(),
            outbox,
            child: Mutex::new(child),
            job,
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
        // Holds only a Weak: the thread ends when the Client (and with it the channel's sender) is gone.
        let weak = Arc::downgrade(&c);
        std::thread::Builder::new().name("ai-rpc-write".into()).spawn(move || write_loop(stdin, lines, weak)).ok();
        Ok(c)
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Last lines the server wrote to stderr (for error messages).
    pub fn stderr_tail(&self) -> String {
        self.stderr_tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n")
    }

    /// Ends the server and everything it started.
    pub fn kill(&self) {
        if let Some(j) = &self.job {
            j.terminate();
        }
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

    /// Answers a server request (a tool call) with `result`. `id` is echoed exactly as received.
    pub fn respond(&self, id: &Value, result: Value) -> Result<(), RpcError> {
        self.send(&json!({ "id": id, "result": result })).map_err(|_| self.closed())
    }

    /// Answers a server request with a JSON-RPC error.
    pub fn refuse(&self, id: &Value, message: &str) {
        let _ = self.send(&json!({ "id": id, "error": { "code": -32601, "message": message } }));
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

    /// Queues one message for the writer thread. Fails only when the writer is gone.
    fn send(&self, msg: &Value) -> Result<(), ()> {
        let mut line = serde_json::to_vec(msg).expect("serializable");
        line.push(b'\n');
        self.outbox.send(line).map_err(|_| ())
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
        // Reap the process without holding the child lock across a blocking wait (`kill` needs it).
        for _ in 0..50 {
            match self.child.lock().unwrap().try_wait() {
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                _ => return,
            }
        }
        self.kill();
        let _ = self.child.lock().unwrap().wait();
    }

    fn route(&self, v: Value) {
        let method = v.get("method").and_then(Value::as_str);
        match (method, v.get("id")) {
            (Some(m), Some(id)) => {
                // A tool call goes to its thread's sink (the stream loop of the turn answers it); when
                // nobody listens any more (the tab was closed) it is refused at once.
                if m == TOOL_CALL {
                    let tid = v.pointer("/params/threadId").and_then(Value::as_str).map(str::to_string);
                    let delivered = tid
                        .is_some_and(|t| self.sinks.lock().unwrap().get(&t).is_some_and(|tx| tx.send(v.clone()).is_ok()));
                    if !delivered {
                        self.refuse(id, "the document is not open any more");
                    }
                    return;
                }
                // approvals / elicitations / anything else: refuse, never block
                self.refuse(id, &format!("inkwell does not support {m}"));
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

fn write_loop(mut stdin: ChildStdin, lines: Receiver<Vec<u8>>, client: Weak<Client>) {
    for line in lines {
        if stdin.write_all(&line).and_then(|_| stdin.flush()).is_err() {
            // a broken pipe: end the process so pending calls fail instead of waiting for their timeout
            if let Some(c) = client.upgrade() {
                c.kill();
            }
            return;
        }
    }
}

/// One Job object per server with KILL_ON_JOB_CLOSE: the server (and anything it spawned) dies with
/// Inkwell however Inkwell dies, and `terminate` ends the whole tree on demand.
#[cfg(windows)]
pub(super) mod job {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

    /// The handle as an integer, so the type is Send + Sync.
    pub struct Job(usize);

    impl Job {
        pub fn new() -> Option<Job> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(h);
                    return None;
                }
                Some(Job(h as usize))
            }
        }

        pub fn adopt(&self, child: &std::process::Child) -> bool {
            unsafe {
                AssignProcessToJobObject(self.0 as HANDLE, child.as_raw_handle() as HANDLE) != 0
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0 as HANDLE, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0 as HANDLE);
            }
        }
    }
}

#[cfg(not(windows))]
pub(super) mod job {
    pub struct Job;
    impl Job {
        pub fn new() -> Option<Job> {
            Some(Job)
        }
        pub fn adopt(&self, _: &std::process::Child) -> bool {
            true
        }
        pub fn terminate(&self) {}
    }
}
