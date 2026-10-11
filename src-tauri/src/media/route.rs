//! The media route's HTTP side: the path, the range arithmetic and the headers. Pure functions, so
//! the byte ranges are tested without a window.

use crate::engine::DocId;

/// The most one ranged response carries (as Tauri's own asset protocol): the player asks again.
/// Every response is a copy of its slice, so a seek never copies the whole file.
pub const CHUNK: usize = 1000 * 1024;

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// `/m/<doc>/<annotation>.<index>` -> the document and the asset id.
pub fn parse(path: &str) -> Option<(DocId, String)> {
    let mut parts = path.trim_start_matches('/').split('/');
    if parts.next()? != "m" {
        return None;
    }
    let doc = parts.next()?.parse::<DocId>().ok()?;
    let id = parts.next()?;
    let ok = id.split_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()));
    (ok && parts.next().is_none()).then(|| (doc, id.to_owned()))
}

pub fn text(status: u16, message: &str) -> Reply {
    Reply { status, headers: vec![("Content-Type", "text/plain; charset=utf-8".into()), ("Cache-Control", "no-store".into()), ("X-Content-Type-Options", "nosniff".into())], body: message.as_bytes().to_vec() }
}

/// What a `Range` header asks for, in bytes `start..=end` of `len`.
enum Want {
    /// Absent, or a form this does not serve (several ranges, bad syntax): the whole body.
    All,
    Span(usize, usize),
    /// Past the end.
    Unsatisfiable,
}

fn want(header: Option<&str>, len: usize) -> Want {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else { return Want::All };
    if spec.contains(',') {
        return Want::All;
    }
    let Some((a, b)) = spec.split_once('-') else { return Want::All };
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = match (a.parse::<usize>(), b.parse::<usize>()) {
        // the last n bytes
        (Err(_), Ok(n)) if a.is_empty() => {
            if n == 0 {
                return Want::Unsatisfiable;
            }
            (len.saturating_sub(n), len.saturating_sub(1))
        }
        (Ok(s), Ok(e)) if e >= s => (s, e),
        (Ok(s), Err(_)) if b.is_empty() => (s, usize::MAX),
        _ => return Want::All,
    };
    if len == 0 || start >= len {
        return Want::Unsatisfiable;
    }
    Want::Span(start, end.min(len - 1))
}

/// The response for a request over `data`: 200 whole, 206 for a range (cut to [`CHUNK`]), 416 past the end.
pub fn respond(data: &[u8], mime: &str, range: Option<&str>) -> Reply {
    let len = data.len();
    let mut headers = vec![
        ("Content-Type", mime.to_owned()),
        ("Accept-Ranges", "bytes".into()),
        ("Cache-Control", "no-store".into()),
        // the bytes were recognised: the browser must not second-guess the type
        ("X-Content-Type-Options", "nosniff".into()),
    ];
    match want(range, len) {
        // shortcut: no Range, or several, copies the whole asset (up to the 256 MB decode cap), like Tauri's
        // asset protocol; upgrade to a streaming body if a player ever asks without a Range.
        Want::All => {
            headers.push(("Content-Length", len.to_string()));
            Reply { status: 200, headers, body: data.to_vec() }
        }
        Want::Span(start, end) => {
            let end = end.min(start + CHUNK - 1);
            headers.push(("Content-Range", format!("bytes {start}-{end}/{len}")));
            headers.push(("Content-Length", (end - start + 1).to_string()));
            Reply { status: 206, headers, body: data[start..=end].to_vec() }
        }
        Want::Unsatisfiable => {
            headers.push(("Content-Range", format!("bytes */{len}")));
            Reply { status: 416, headers, body: Vec::new() }
        }
    }
}
