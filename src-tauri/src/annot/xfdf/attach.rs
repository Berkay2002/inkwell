//! The file a FileAttachment comment carries, in XFDF (`<data>`) and FDF (`/FS` with `/EF`).
//!
//! The bytes are data: they are never opened, run or fetched, and the name is display text only (it goes through
//! the same cleaning as the Attachments panel's). Each file is at most [`MAX_ATTACHMENT`] decoded, and all files
//! and appearances of one file together at most `MAX_BLOBS` (see `crate::exchange`); a larger one is left out and
//! counted. A stream is decoded under the cap, so a small compressed one cannot inflate past it. A file that
//! is a link to another file (no `/EF`) is not followed: it is left out and counted.
//!
//! `<data>` follows ISO 19444-1 as far as it was read: `mode` raw or filtered (Flate), `encoding` hex (also
//! accepted: ascii and base64). It was not checked against a file Acrobat wrote.

use super::ap::Why;
use crate::engine::EngineError;
use crate::exchange::{self, Budget, MAX_ATTACHMENT, Node, pdf_name, pdf_text};
use crate::forms::get;
use crate::organize::attach::{clean_name, mime_for, stream_of, strip_invisible};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use mupdf::Buffer;
use mupdf::pdf::{PdfDocument, PdfObject};
use std::fmt::Write as _;
use std::io::{Read, Write};

pub(super) const EXTERNAL: Why = "attached file that is a link to a file outside the comment";
pub(super) const OVER: Why = "attached file over 4 MB, or that unpacks to more than that";
pub(super) const NO_ROOM: Why = "attached file past the limit of 10 MB for attached files and appearances in one file";
pub(super) const DAMAGED: Why = "attached file that is damaged or not understood";

/// Longest name and description kept.
const NAME_MAX: usize = 255;
const DESC_MAX: usize = 1024;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AttachFile {
    /// Display text: no folder, nothing a file system refuses.
    pub name: String,
    pub desc: String,
    pub mime: String,
    /// A PDF date (`D:...`), or empty.
    pub modified: String,
    pub data: Vec<u8>,
}

fn name_of(raw: &str) -> String {
    let n = clean_name(&strip_invisible(raw));
    clean_name(&n.chars().take(NAME_MAX).collect::<String>())
}

fn desc_of(raw: &str) -> String {
    let mut s = strip_invisible(raw);
    if s.len() > DESC_MAX {
        let mut n = DESC_MAX;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
    }
    s
}

/// `type/subtype` with plain characters only: it is copied into a name.
fn mime_ok(m: &str) -> bool {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_');
    m.len() <= 100 && m.split_once('/').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && a.chars().all(plain) && b.chars().all(plain))
}

/// `D:` and digits, then at most a zone: what a PDF date is.
fn date_ok(d: &str) -> bool {
    d.len() <= 24 && d.strip_prefix("D:").is_some_and(|r| r.len() >= 4 && r.chars().all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | 'Z' | '\'')))
}

/// What a file may be decoded to: the per-file cap, or what is left of the total.
fn room(b: &Budget) -> usize {
    MAX_ATTACHMENT.min(b.blobs_left())
}

fn finish(f: AttachFile, b: &Budget) -> Result<AttachFile, Why> {
    if f.data.len() > MAX_ATTACHMENT {
        return Err(OVER);
    }
    if !b.take_blob(f.data.len()) {
        return Err(NO_ROOM);
    }
    Ok(f)
}

/// The file of an annotation or FDF comment (its `/FS`), or why it is not carried. `None` when it has none.
pub(super) fn read(o: &PdfObject, b: &Budget) -> Option<Result<AttachFile, Why>> {
    let fs = get(o, "FS")?;
    Some(read_spec(&fs, b))
}

fn read_spec(fs: &PdfObject, b: &Budget) -> Result<AttachFile, Why> {
    let dict = fs.resolve().ok().flatten().unwrap_or_else(|| fs.clone());
    if !dict.is_dict().unwrap_or(false) {
        return Err(EXTERNAL);
    }
    let stream = stream_of(fs).ok_or(EXTERNAL)?;
    let cap = room(b);
    if !b.take_work(cap) {
        return Err(NO_ROOM);
    }
    let data = stream.read_stream_capped(cap).map_err(|e| {
        if !e.is_stream_cap() {
            DAMAGED
        } else if cap < MAX_ATTACHMENT {
            NO_ROOM
        } else {
            OVER
        }
    })?;
    let first = |k: &[&str]| k.iter().map(|k| super::string(&dict, k, b)).find(|s| !s.is_empty()).unwrap_or_default();
    let mime = get(&stream, "Subtype").filter(|s| s.is_name().unwrap_or(false)).and_then(|s| s.as_name().ok()).and_then(|n| String::from_utf8(n).ok()).filter(|m| mime_ok(m));
    let params = get(&stream, "Params").and_then(|p| p.resolve().ok().flatten().filter(|p| p.is_dict().unwrap_or(false)));
    let modified = params.map(|p| super::string(&p, "ModDate", b)).filter(|d| date_ok(d)).unwrap_or_default();
    let name = name_of(&first(&["UF", "F"]));
    let mime = mime.unwrap_or_else(|| mime_for(&name).to_string());
    finish(AttachFile { name, desc: desc_of(&super::string(&dict, "Desc", b)), mime, modified, data }, b)
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if digits.len() % 2 != 0 {
        return None;
    }
    let v = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    digits.chunks_exact(2).map(|p| Some(v(p[0])? << 4 | v(p[1])?)).collect()
}

fn hex(d: &[u8]) -> String {
    let mut s = String::with_capacity(d.len() * 2);
    for b in d {
        let _ = write!(s, "{b:02X}");
    }
    s
}

fn inflate(z: &[u8], cap: usize) -> Result<Vec<u8>, Why> {
    let mut out = Vec::new();
    ZlibDecoder::new(z).take(cap as u64 + 1).read_to_end(&mut out).map_err(|_| DAMAGED)?;
    if out.len() > cap { Err(OVER) } else { Ok(out) }
}

/// The file of a `<data>` element; `file` is the annotation's `file` attribute.
pub(super) fn from_data(e: &Node, file: &str, b: &Budget) -> Result<AttachFile, Why> {
    use base64::Engine as _;
    if e.over {
        return Err(OVER);
    }
    let encoding = e.attr("encoding").unwrap_or("hex").to_ascii_lowercase();
    let text = e.text.trim();
    let stored: Vec<u8> = match encoding.as_str() {
        // The text is checked before it is decoded: two characters a byte.
        "hex" => {
            if text.len() / 2 > MAX_ATTACHMENT {
                return Err(OVER);
            }
            unhex(text).ok_or(DAMAGED)?
        }
        "base64" => {
            let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(t).map_err(|_| DAMAGED)?
        }
        _ => text.as_bytes().to_vec(),
    };
    let cap = room(b);
    let filtered = e.attr("mode").is_some_and(|m| m.eq_ignore_ascii_case("filtered"));
    if filtered && !b.take_work(cap) {
        return Err(NO_ROOM);
    }
    let data = if filtered {
        inflate(&stored, cap).map_err(|w| if w == OVER && cap < MAX_ATTACHMENT { NO_ROOM } else { w })?
    } else {
        stored
    };
    if data.len() > cap && cap < MAX_ATTACHMENT {
        return Err(NO_ROOM);
    }
    let name = name_of(file);
    let mime = e.attr("MIME-type").filter(|m| mime_ok(m)).map_or_else(|| mime_for(&name).to_string(), str::to_string);
    let modified = e.attr("modification").filter(|d| date_ok(d)).unwrap_or("").to_string();
    finish(AttachFile { name, desc: desc_of(e.attr("description").unwrap_or("")), mime, modified, data }, b)
}

/// The `<data>` element.
pub(super) fn to_data(f: &AttachFile) -> String {
    let mut s = format!("<data mode=\"raw\" encoding=\"hex\" length=\"{}\" MIME-type=\"{}\"", f.data.len(), exchange::esc(&f.mime));
    if !f.modified.is_empty() {
        let _ = write!(s, " modification=\"{}\"", exchange::esc(&f.modified));
    }
    if !f.desc.is_empty() {
        let _ = write!(s, " description=\"{}\"", exchange::esc(&f.desc));
    }
    let _ = write!(s, ">{}</data>", hex(&f.data));
    s
}

fn deflate(d: &[u8]) -> Result<Vec<u8>, EngineError> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(d).map_err(|e| exchange::bad(e.to_string()))?;
    z.finish().map_err(|e| exchange::bad(e.to_string()))
}

/// Writes the file into `doc` (the stream and a file specification, not registered in the document's embedded
/// files) and returns the specification.
pub(super) fn install(doc: &mut PdfDocument, f: &AttachFile) -> Result<PdfObject, EngineError> {
    let z = deflate(&f.data)?;
    let mut sd = doc.new_dict()?;
    sd.dict_put("Type", PdfObject::new_name("EmbeddedFile")?)?;
    sd.dict_put("Subtype", PdfObject::new_name(&f.mime)?)?;
    sd.dict_put("Filter", PdfObject::new_name("FlateDecode")?)?;
    let mut params = doc.new_dict()?;
    params.dict_put("Size", PdfObject::new_int(i32::try_from(f.data.len()).unwrap_or(i32::MAX))?)?;
    if !f.modified.is_empty() {
        params.dict_put("ModDate", PdfObject::new_string(&f.modified)?)?;
    }
    sd.dict_put("Params", params)?;
    let stream = doc.add_stream(&Buffer::from_bytes(&z)?, Some(&sd), true)?;
    let mut ef = doc.new_dict()?;
    ef.dict_put("F", stream.clone())?;
    ef.dict_put("UF", stream)?;
    let mut fs = doc.new_dict()?;
    fs.dict_put("Type", PdfObject::new_name("Filespec")?)?;
    fs.dict_put("F", PdfObject::new_string(&f.name)?)?;
    fs.dict_put("UF", PdfObject::new_string(&f.name)?)?;
    if !f.desc.is_empty() {
        fs.dict_put("Desc", PdfObject::new_string(&f.desc)?)?;
    }
    fs.dict_put("EF", ef)?;
    Ok(doc.add_object(&fs)?)
}

/// The file as two FDF objects: its stream is object `first`, its specification `first + 1`.
pub(super) fn write_objects(f: &AttachFile, first: usize) -> Result<Vec<Vec<u8>>, EngineError> {
    let z = deflate(&f.data)?;
    let mut params = format!("<< /Size {}", f.data.len());
    if !f.modified.is_empty() {
        let _ = write!(params, " /ModDate {}", pdf_text(&f.modified));
    }
    params.push_str(" >>");
    let mut stream = format!("{first} 0 obj\n<< /Type /EmbeddedFile /Subtype {} /Params {params} /Filter /FlateDecode /Length {} >>\nstream\n", pdf_name(&f.mime), z.len()).into_bytes();
    stream.extend_from_slice(&z);
    stream.extend_from_slice(b"\nendstream\nendobj\n");
    let mut spec = format!("{} 0 obj\n<< /Type /Filespec /F {} /UF {}", first + 1, pdf_text(&f.name), pdf_text(&f.name));
    if !f.desc.is_empty() {
        let _ = write!(spec, " /Desc {}", pdf_text(&f.desc));
    }
    let _ = write!(spec, " /EF << /F {first} 0 R /UF {first} 0 R >> >>\nendobj\n");
    Ok(vec![stream, spec.into_bytes()])
}
