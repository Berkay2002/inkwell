//! Inkwell's tools as an MCP server (streamable HTTP, JSON responses) for one `claude` child.
//!
//! Bound to 127.0.0.1 on a port the OS picks, for the life of that child only. Every request must
//! carry the bearer token generated for this child (it travels to the child in a private config
//! file, never on a command line), a `Host` naming this very address and no `Origin` header (a web
//! page cannot call it). A request failing any of that gets an answer without a tool being touched.
//!
//! The tool handler runs on the request's own thread: it may wait for the user (an approval card)
//! for minutes, and other requests keep being served meanwhile.

use crate::ai::tools::{self, Tool};
use base64::Engine as _;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// What the handler gets: tool name, arguments, and the id the model gave the call (Claude Code puts
/// it in `_meta`), which the activity trail and the approval card use.
pub type Handler = Arc<dyn Fn(&str, Value, Option<String>) -> Result<tools::Output, String> + Send + Sync>;

const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 2 * 1024 * 1024;
/// A request line that does not arrive in this time is dropped (a connection that never speaks).
const READ_TIMEOUT: Duration = Duration::from_secs(15);

pub struct McpServer {
    pub port: u16,
    pub token: String,
    stop: Arc<AtomicBool>,
}

impl McpServer {
    pub fn start(tools: Arc<RwLock<Vec<Arc<Tool>>>>, handler: Handler) -> std::io::Result<McpServer> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let token = random_token();
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let tok = token.clone();
        std::thread::Builder::new().name("claude-mcp".into()).spawn(move || {
            while !st.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let (tools, handler, tok) = (tools.clone(), handler.clone(), tok.clone());
                        let _ = std::thread::Builder::new()
                            .name("claude-mcp-conn".into())
                            .spawn(move || serve(conn, port, &tok, &tools, &handler));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(40)),
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        })?;
        Ok(McpServer { port, token, stop })
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }

    /// Stops accepting. Calls already running finish (the turn's cancel flag ends their waits).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 256 bits from the OS, hex.
fn random_token() -> String {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).expect("the OS random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn same(a: &str, b: &str) -> bool {
    // constant time over the length of the shorter; the length itself is not secret
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn read_request(conn: &TcpStream) -> Result<Request, (u16, &'static str)> {
    let mut r = BufReader::new(conn.try_clone().map_err(|_| (500, "io"))?);
    let mut line = String::new();
    r.read_line(&mut line).map_err(|_| (400, "bad request"))?;
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else { return Err((400, "bad request")) };
    let (method, path) = (method.to_string(), path.to_string());
    let mut headers = Vec::new();
    let mut total = line.len();
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h).map_err(|_| (400, "bad request"))?;
        total += n;
        if n == 0 || total > MAX_HEADER {
            return Err((431, "headers too large"));
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let req = Request { method, path, headers, body: Vec::new() };
    if req.header("transfer-encoding").is_some() {
        return Err((411, "send a Content-Length"));
    }
    Ok(Request { body: read_body(&mut r, &req)?, ..req })
}

fn read_body(r: &mut BufReader<TcpStream>, req: &Request) -> Result<Vec<u8>, (u16, &'static str)> {
    let len: usize = req.header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err((413, "body too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|_| (400, "short body"))?;
    Ok(body)
}

fn write_response(conn: &mut TcpStream, status: u16, body: Option<&Value>) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let data = body.map(|b| serde_json::to_vec(b).expect("serializable")).unwrap_or_default();
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: {}\r\n", data.len());
    if body.is_some() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str("\r\n");
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(&data);
    let _ = conn.flush();
    let _ = conn.shutdown(Shutdown::Both);
}

fn serve(mut conn: TcpStream, port: u16, token: &str, tools: &RwLock<Vec<Arc<Tool>>>, handler: &Handler) {
    // Winsock gives an accepted socket the listener's non-blocking mode: reads would fail with
    // WouldBlock whenever the request is not there yet
    let _ = conn.set_nonblocking(false);
    let _ = conn.set_read_timeout(Some(READ_TIMEOUT));
    let req = match read_request(&conn) {
        Ok(r) => r,
        Err((code, _)) => return write_response(&mut conn, code, None),
    };
    // a browser page, or a request meant for some other name, is never served
    let host_ok = req
        .header("host")
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    if req.header("origin").is_some() || !host_ok {
        return write_response(&mut conn, 403, None);
    }
    let authorized = req
        .header("authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| same(t, token));
    if !authorized {
        return write_response(&mut conn, 401, None);
    }
    if req.path.split('?').next() != Some("/mcp") {
        return write_response(&mut conn, 404, None);
    }
    match req.method.as_str() {
        // no server-initiated stream: the client carries on without one
        "GET" => write_response(&mut conn, 405, None),
        "DELETE" => write_response(&mut conn, 200, None),
        "POST" => {
            // a tool call may wait for the user: the read timeout is for the request only
            let _ = conn.set_read_timeout(None);
            let msg: Value = match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(_) => return write_response(&mut conn, 400, None),
            };
            match rpc(&msg, tools, handler) {
                Some(reply) => write_response(&mut conn, 200, Some(&reply)),
                None => write_response(&mut conn, 202, None),
            }
        }
        _ => write_response(&mut conn, 405, None),
    }
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// One JSON-RPC message. None for a notification (nothing to answer).
fn rpc(msg: &Value, tools: &RwLock<Vec<Arc<Tool>>>, handler: &Handler) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
    let p = &msg["params"];
    let result = match method {
        "initialize" => {
            let v = p.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-03-26");
            json!({
                "protocolVersion": v,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "inkwell", "version": env!("CARGO_PKG_VERSION") },
            })
        }
        "ping" => json!({}),
        "tools/list" => {
            let list: Vec<Value> = tools
                .read()
                .unwrap()
                .iter()
                .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.schema }))
                .collect();
            json!({ "tools": list })
        }
        "tools/call" => {
            let name = p.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = crate::ai::gate::parse_args(p.get("arguments"));
            let tool_use = p.pointer("/_meta/claudecode~1toolUseId").and_then(Value::as_str).map(str::to_string);
            tool_result(&handler(name, args, tool_use))
        }
        _ => return Some(error(&id, -32601, &format!("method not found: {method}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// A tool's outcome as an MCP `CallToolResult`: text, plus the page picture when there is one. A
/// failure is a result with `isError`, which the model reads and reacts to.
pub fn tool_result(r: &Result<tools::Output, String>) -> Value {
    match r {
        Ok(o) => {
            let mut content = vec![json!({ "type": "text", "text": o.text })];
            if let Some(png) = &o.image {
                let data = base64::engine::general_purpose::STANDARD.encode(png);
                content.push(json!({ "type": "image", "data": data, "mimeType": "image/png" }));
            }
            json!({ "content": content, "isError": false })
        }
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}
