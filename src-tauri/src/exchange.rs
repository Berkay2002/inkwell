//! Data files that comments (XFDF, FDF) and form data (XFDF, FDF, JSON) move through: the caps every
//! reader obeys, a small XML reader, and the text helpers both directions share. Every such file is
//! hostile input: nothing in it is run, opened or followed.
//!
//! Shapes follow ISO 19444-1 (XFDF) and ISO 32000-2 section 12.7.8 (FDF). No file written by Acrobat
//! was available to check them against. The XFDF and FDF code is written from the standards; pdfcraft's
//! `crates/xfdf` was read for ideas, and what the form data XML export takes from it (`xml_name`, the shape of
//! `xml_data`) is marked in `forms/data.rs` and listed in `third_party/pdfcraft/INKWELL-NOTICE.txt`.

use crate::engine::{DocId, Engine, EngineError};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Read;

type Result<T> = std::result::Result<T, EngineError>;

pub fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

/// What a certification (DocMDP `/P`, ISO 32000-2 12.8.2.2) leaves to a form data import: 1 nothing, 2 and 3 filling
/// in form fields. `None` is not certified. A certification that cannot be read is refused by the caller
/// (`certified(d)?`), never treated as none. Comments go through `outline::refuse_unless_annotating_allowed`.
pub fn forms_allowed(level: Option<i32>) -> bool {
    level.is_none_or(|p| p >= 2)
}

/// What the UI may offer for a document: each import, and whether the document is signed.
#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Gates {
    pub comments: bool,
    pub forms: bool,
    pub signed: bool,
}

pub fn gates(eng: &Engine, doc: DocId) -> Result<Gates> {
    eng.read_custom(doc, |d| {
        let signed = crate::outline::signed(d);
        // Unreadable: neither is offered (the import itself refuses it too).
        let forms = crate::outline::certified(d).is_ok_and(forms_allowed);
        Ok(Gates { comments: crate::outline::refuse_unless_annotating_allowed(d, "").is_ok(), forms, signed })
    })
}

/// Largest data file read.
pub const MAX_FILE: u64 = 32 << 20;
/// XML elements, or PDF objects walked, per file.
pub const MAX_NODES: usize = 100_000;
/// Nesting of XML elements, JSON values and form field trees.
pub const MAX_DEPTH: usize = 64;
/// Comments or fields taken from one file; the rest is counted and skipped.
pub const MAX_ITEMS: usize = 10_000;
/// Longest single string (a comment's text, a value, a name).
pub const MAX_STR: usize = 1 << 20;

/// Bytes one file may copy out of PDF objects in all. Objects can be shared (one string many entries refer
/// to), so counts of items do not bound what is copied; this does.
pub const MAX_COPIED: usize = 32 << 20;
/// Longest text of a `<data>` (a file attachment) or `<appearance>` element: they are the only ones that may pass
/// [`MAX_STR`]. One that is longer is not an error of the file: the element is marked [`Node::over`] and left empty.
pub const MAX_BLOB_TEXT: usize = 9 << 20;
/// Largest file attachment taken or written, decoded. Hex text doubles it, so it stays under [`MAX_BLOB_TEXT`].
pub const MAX_ATTACHMENT: usize = 4 << 20;
/// Attachments and appearance streams together, decoded, in one file read or written: over it they are left out and
/// counted. In hex they come to about twice this, which stays well under [`MAX_FILE`], so Inkwell can read what it writes.
pub const MAX_BLOBS: usize = 10 << 20;
/// Bytes one file may make Inkwell decode in all, counting what a cap stopped: a bomb that fails is not in
/// [`MAX_BLOBS`], so many comments sharing one would otherwise each cost a full cap of inflating.
pub const MAX_DECODE_WORK: usize = 256 << 20;
/// Bytes of dictionaries, arrays and names of appearances one file may hold at once (an estimate of their size in
/// memory): the same font read again by every comment is a copy each. Past it appearances are left out and counted.
pub const MAX_AP_STRUCT: usize = 64 << 20;
/// Least charged to [`MAX_DECODE_WORK`] for one decode: even a cap of 0 makes the decoder produce a chunk.
pub const MIN_DECODE_WORK: usize = 16 << 10;
/// The same bound for an export: the document is the person's own, so real comments may hold more than a
/// file we read, but a damaged one still must not multiply one shared string or array without end.
pub const MAX_EXPORT_COPIED: usize = 256 << 20;

/// What is left of [`MAX_COPIED`] for one read of a file or of a document's comments.
pub struct Budget {
    left: Cell<usize>,
    gone: Cell<bool>,
    /// Text entries dropped for being over [`MAX_STR`].
    too_long: Cell<usize>,
    /// Indirect strings and number lists already read, by object number: read once, then cloned.
    strings: RefCell<HashMap<i32, String>>,
    /// Indirect strings over [`MAX_STR`]: only the length is kept (they are dropped, so not charged).
    long: RefCell<HashMap<i32, usize>>,
    numbers: RefCell<HashMap<(i32, usize), Vec<f32>>>,
    /// What is left of [`MAX_BLOBS`] for attachments and appearance streams.
    blobs: Cell<usize>,
    /// What is left of [`MAX_DECODE_WORK`].
    work: Cell<usize>,
    /// What is left of [`MAX_AP_STRUCT`].
    structs: Cell<usize>,
}

impl Budget {
    pub fn new() -> Budget {
        Budget::with_limit(MAX_COPIED)
    }

    pub fn with_limit(bytes: usize) -> Budget {
        Budget { left: Cell::new(bytes), gone: Cell::new(false), too_long: Cell::new(0), strings: RefCell::default(), long: RefCell::default(), numbers: RefCell::default(), blobs: Cell::new(MAX_BLOBS), work: Cell::new(MAX_DECODE_WORK), structs: Cell::new(MAX_AP_STRUCT) }
    }

    /// Charges `n` bytes of attachment or appearance data; false (nothing charged) when they are not there. Unlike
    /// [`Budget::take`] the budget is not gone: the entry is left out and the file goes on.
    pub fn take_blob(&self, n: usize) -> bool {
        match self.blobs.get().checked_sub(n) {
            Some(rest) => {
                self.blobs.set(rest);
                true
            }
            None => false,
        }
    }

    /// Charges `n` bytes of appearance structure; false (nothing charged) when they are not there.
    pub fn take_struct(&self, n: usize) -> bool {
        match self.structs.get().checked_sub(n) {
            Some(rest) => {
                self.structs.set(rest);
                true
            }
            None => false,
        }
    }

    /// Gives back what an appearance that was not kept had charged to [`MAX_BLOBS`].
    pub fn restore_blobs(&self, left: usize) {
        self.blobs.set(left.min(MAX_BLOBS));
    }

    /// What is left of the file's decode allowance: a decode is capped to it, and not tried at 0.
    pub fn work_left(&self) -> usize {
        self.work.get()
    }

    /// Charges a decode that was tried: the bytes it produced, or its cap when it was stopped by it.
    pub fn spend_work(&self, n: usize) {
        self.work.set(self.work.get().saturating_sub(n.max(MIN_DECODE_WORK)));
    }

    /// What is left for attachments and appearance streams.
    pub fn blobs_left(&self) -> usize {
        self.blobs.get()
    }

    /// Charges `n` bytes. When they are not there the budget is gone for good and this says false: the
    /// caller copies nothing more. A copy already made is charged afterwards, so at most one object
    /// (itself bounded by the file) goes over.
    pub fn take(&self, n: usize) -> bool {
        match self.left.get().checked_sub(n) {
            Some(rest) if !self.gone.get() => {
                self.left.set(rest);
                true
            }
            _ => {
                self.gone.set(true);
                self.left.set(0);
                false
            }
        }
    }

    /// An indirect string read before (the clone is the caller's to charge).
    pub fn string_of(&self, object: i32) -> Option<String> {
        self.strings.borrow().get(&object).cloned()
    }

    pub fn keep_string(&self, object: i32, s: &str) {
        if self.take(s.len()) {
            self.strings.borrow_mut().insert(object, s.to_string());
        }
    }

    /// An indirect string over [`MAX_STR`] was read: its length is kept, nothing is copied or charged.
    pub fn keep_long(&self, object: i32, len: usize) {
        self.long.borrow_mut().insert(object, len);
    }

    /// The length of an indirect string read before, without copying it.
    pub fn string_len_of(&self, object: i32) -> Option<usize> {
        self.strings.borrow().get(&object).map(String::len).or_else(|| self.long.borrow().get(&object).copied())
    }

    /// An indirect number list read before, by object number and cap.
    pub fn numbers_of(&self, object: i32, cap: usize) -> Option<Vec<f32>> {
        self.numbers.borrow().get(&(object, cap)).cloned()
    }

    pub fn keep_numbers(&self, object: i32, cap: usize, v: &[f32]) {
        if self.take(v.len() * 4) {
            self.numbers.borrow_mut().insert((object, cap), v.to_vec());
        }
    }

    /// The file is over a count that fails closed (it is refused, not cut short).
    pub fn fail(&self) {
        self.gone.set(true);
        self.left.set(0);
    }

    pub fn gone(&self) -> bool {
        self.gone.get()
    }

    /// A text entry was left empty for being over [`MAX_STR`].
    pub fn note_too_long(&self) {
        self.too_long.set(self.too_long.get() + 1);
    }

    /// How many text entries [`Budget::note_too_long`] counted.
    pub fn too_long(&self) -> usize {
        self.too_long.get()
    }

    /// Refuses the file once the budget is gone.
    pub fn check(&self) -> Result<()> {
        if self.gone.get() {
            Err(bad("This file refers to the same data so many times, or has so many entries, that Inkwell does not read it."))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xfdf,
    Fdf,
    Json,
    /// Form data as comma-separated values (one header row, one record row).
    Csv,
    /// Form data as tab-delimited text.
    Tab,
    /// Form data as plain XML: `<fields><name>value</name>...`.
    Xml,
}

impl Format {
    /// By file extension (the person's choice in the save dialog); anything else is XFDF. `data` is form data, which
    /// also has JSON, CSV, tab text and XML; comments are XFDF or FDF only.
    pub fn of_path(path: &str, data: bool) -> Format {
        match std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("fdf") => Format::Fdf,
            Some("json") if data => Format::Json,
            Some("csv") if data => Format::Csv,
            Some("txt" | "tsv" | "tab") if data => Format::Tab,
            Some("xml") if data => Format::Xml,
            _ => Format::Xfdf,
        }
    }
}

/// What an import file is, from its first bytes (the extension is only a hint: a renamed file still opens).
pub fn sniff(bytes: &[u8]) -> Option<Format> {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let b = &b[b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(b.len())..];
    match b.first()? {
        b'%' if b.starts_with(b"%FDF") => Some(Format::Fdf),
        b'<' => Some(Format::Xfdf),
        b'{' => Some(Format::Json),
        _ => None,
    }
}

/// What a text data file says, as UTF-8: UTF-8 with or without a byte order mark, or UTF-16 with one (Excel's
/// "Unicode Text"). An FDF is binary and passes as it is. Anything else is refused: a guess at a code page would
/// put the wrong letters into a form without a word, and a NUL byte is UTF-16 saved without its mark.
pub fn utf8_of(bytes: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>> {
    use std::borrow::Cow;
    let refuse = || bad("This is not a text file Inkwell can read. It takes UTF-8, or UTF-16 with a byte order mark (Excel's \"Unicode Text\"); save it as one of those and try again.");
    if sniff(bytes) == Some(Format::Fdf) {
        return Ok(Cow::Borrowed(bytes));
    }
    let text: Cow<[u8]> = match bytes {
        // UTF-32 starts like UTF-16 little endian.
        [0xFF, 0xFE, 0, 0, ..] | [0, 0, 0xFE, 0xFF, ..] => return Err(refuse()),
        [0xFF, 0xFE, rest @ ..] | [0xFE, 0xFF, rest @ ..] => {
            let little = bytes[0] == 0xFF;
            if rest.len() % 2 != 0 {
                return Err(refuse());
            }
            let units = rest.chunks_exact(2).map(|c| if little { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
            let s: std::result::Result<String, _> = char::decode_utf16(units).collect();
            Cow::Owned(s.map_err(|_| refuse())?.into_bytes())
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => Cow::Borrowed(rest),
        _ => Cow::Borrowed(bytes),
    };
    if std::str::from_utf8(&text).is_err() || text.contains(&0) {
        return Err(refuse());
    }
    Ok(text)
}

/// Reads a data file: more than [`MAX_FILE`] bytes (by its length, or by what is really there) is refused.
pub fn read_capped(path: &str) -> Result<Vec<u8>> {
    let io = |e: std::io::Error| EngineError::Io(e.to_string());
    let f = std::fs::File::open(path).map_err(io)?;
    let too_big = || bad("This data file is larger than 32 MB, so Inkwell does not read it.");
    if f.metadata().map_err(io)?.len() > MAX_FILE {
        return Err(too_big());
    }
    let mut buf = Vec::new();
    f.take(MAX_FILE + 1).read_to_end(&mut buf).map_err(io)?;
    if buf.len() as u64 > MAX_FILE {
        return Err(too_big());
    }
    Ok(buf)
}

// ---- XML -----------------------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct Node {
    /// Local name, lower case (XFDF is read without regard to case and namespace prefix).
    pub name: String,
    /// Local name as written: plain form data XML names its fields by it.
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    /// The element's own text, in document order (children's text is not included): the text before its first child,
    /// then each child's `tail`.
    pub text: String,
    /// The text that follows this element inside its parent, before the next child or the parent's end (mixed content,
    /// which rich text is made of). It is also part of the parent's `text`.
    pub tail: String,
    /// A `data` or `appearance` element whose text was over [`MAX_BLOB_TEXT`]: `text` is empty.
    pub over: bool,
    pub kids: Vec<Node>,
}

impl Node {
    /// An attribute, matched without regard to case.
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.kids.iter().find(|k| k.name == name)
    }

    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.kids.iter().filter(move |k| k.name == name)
    }
}

/// Reads XML into a tree. No DTD (a `<!DOCTYPE` is refused, which is what stops entity expansion and
/// external entities), only the five predefined entities and character references, at most
/// [`MAX_NODES`] elements, [`MAX_DEPTH`] levels and [`MAX_STR`] bytes in any one string.
pub fn parse_xml(bytes: &[u8]) -> Result<Node> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let err = |m: &str| bad(format!("This file is not valid XFDF: {m}."));
    let mut rd = Reader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let mut count = 0usize;
    // An element and each of its attributes count as one node: a few elements with 100 000 attributes each are
    // as much work as 100 000 elements.
    let tick = |count: &mut usize| -> Result<()> {
        *count += 1;
        if *count > MAX_NODES {
            return Err(bad("This file has more than 100 000 elements and attributes, so Inkwell does not read it."));
        }
        Ok(())
    };
    let make = |e: &quick_xml::events::BytesStart, count: &mut usize| -> Result<Node> {
        tick(count)?;
        let tag = e.local_name().as_ref().to_string();
        let mut n = Node { name: tag.to_ascii_lowercase(), tag, ..Node::default() };
        for a in e.attributes() {
            tick(count)?;
            let a = a.map_err(|_| err("an attribute is malformed"))?;
            let key = a.key.local_name().as_ref().to_string();
            let full: &str = a.key.as_ref();
            if full.starts_with("xmlns") {
                continue;
            }
            let v = a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map_err(|_| err("an attribute has an unknown entity"))?;
            if v.len() > MAX_STR {
                return Err(bad("A value in this file is longer than 1 MB."));
            }
            n.attrs.push((key, v.into_owned()));
        }
        Ok(n)
    };
    let grow = |stack: &mut Vec<Node>, s: &str| -> Result<()> {
        let in_annots = stack.iter().any(|n| n.name == "annots");
        if let Some(top) = stack.last_mut() {
            // Only a comment's file or appearance may be longer than MAX_STR: not a form field that has this name.
            let blob = matches!(top.name.as_str(), "data" | "appearance") && in_annots;
            if top.over {
                return Ok(());
            }
            if top.text.len() + s.len() > if blob { MAX_BLOB_TEXT } else { MAX_STR } {
                if !blob {
                    return Err(bad("A value in this file is longer than 1 MB."));
                }
                top.over = true;
                top.text = String::new();
                return Ok(());
            }
            top.text.push_str(s);
            if let Some(k) = top.kids.last_mut() {
                k.tail.push_str(s);
            }
        }
        Ok(())
    };
    loop {
        let ev = rd.read_event_into(&mut buf).map_err(|e| err(&e.to_string()))?;
        match ev {
            Event::Start(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(bad("This file is nested more than 64 levels deep, so Inkwell does not read it."));
                }
                stack.push(make(&e, &mut count)?);
            }
            Event::Empty(e) => {
                let n = make(&e, &mut count)?;
                match stack.last_mut() {
                    Some(p) => p.kids.push(n),
                    None => root = root.or(Some(n)),
                }
            }
            Event::End(_) => {
                let n = stack.pop().ok_or_else(|| err("an end tag has no start"))?;
                match stack.last_mut() {
                    Some(p) => p.kids.push(n),
                    None => {
                        root = Some(n);
                        break;
                    }
                }
            }
            Event::Text(t) => {
                grow(&mut stack, &t.xml10_content())?;
            }
            Event::CData(c) => {
                grow(&mut stack, &c.xml10_content())?;
            }
            Event::GeneralRef(r) => {
                let s = match r.resolve_char_ref().map_err(|_| err("a character reference is invalid"))? {
                    Some(c) => c.to_string(),
                    None => match &*r {
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "amp" => "&".into(),
                        "quot" => "\"".into(),
                        "apos" => "'".into(),
                        _ => return Err(err("an entity is not one of the five XML ones")),
                    },
                };
                grow(&mut stack, &s)?;
            }
            Event::DocType(_) => return Err(bad("This file has a DOCTYPE, which Inkwell does not accept in XFDF.")),
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !stack.is_empty() {
        return Err(err("the file ends inside an element"));
    }
    root.ok_or_else(|| err("there is no root element"))
}

/// Text for an XML attribute or element body: the five predefined entities, and no character XML 1.0 cannot hold.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            '\r' => o.push_str("&#13;"),
            '\n' | '\t' => o.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => o.push(c),
        }
    }
    o
}

// ---- PDF syntax (FDF writing) --------------------------------------------------------------------

/// A PDF text string literal: plain ASCII as it is, anything else as UTF-16BE with a byte order mark.
pub fn pdf_text(s: &str) -> String {
    if s.bytes().all(|b| (0x20..0x7F).contains(&b)) {
        let mut o = String::with_capacity(s.len() + 2);
        o.push('(');
        for c in s.chars() {
            if matches!(c, '(' | ')' | '\\') {
                o.push('\\');
            }
            o.push(c);
        }
        o.push(')');
        o
    } else {
        let mut o = String::from("<FEFF");
        for u in s.encode_utf16() {
            o.push_str(&format!("{u:04X}"));
        }
        o.push('>');
        o
    }
}

/// A PDF name: everything outside the regular printable ASCII as `#xx` (UTF-8 bytes).
pub fn pdf_name(s: &str) -> String {
    let mut o = String::from("/");
    for b in s.bytes() {
        if (0x21..0x7F).contains(&b) && !b"#/%()<>[]{}".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("#{b:02X}"));
        }
    }
    o
}

// ---- FDF reading ---------------------------------------------------------------------------------

/// Opens FDF bytes with MuPDF's own parser (which reads `%FDF-` files and repairs a missing xref) and hands
/// `f` the `/FDF` dictionary of the catalog. A panic or a MuPDF error is "damaged file", never a failure of
/// the app. `f` must keep to [`MAX_NODES`] / [`MAX_DEPTH`] and never read `/JavaScript`, `/AA` or `/F`.
pub fn with_fdf<R>(bytes: &[u8], f: impl FnOnce(&mupdf::pdf::PdfObject) -> Result<R>) -> Result<R> {
    let damaged = || bad("This FDF file is damaged, so Inkwell cannot read it.");
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = mupdf::pdf::PdfDocument::from_bytes(bytes).map_err(|_| damaged())?;
        let root = doc.trailer().ok().and_then(|t| t.get_dict("Root").ok().flatten()).ok_or_else(damaged)?;
        let fdf = root.get_dict("FDF").ok().flatten().filter(|d| d.is_dict().unwrap_or(false)).ok_or_else(damaged)?;
        f(&fdf)
    }))
    .unwrap_or_else(|_| Err(damaged()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_comment_may_hold_a_long_data_or_appearance_element() {
        let big = "0".repeat(2 << 20);
        let ok = format!("<xfdf><annots><fileattachment><data>{big}</data></fileattachment></annots></xfdf>");
        assert_eq!(parse_xml(ok.as_bytes()).unwrap().child("annots").unwrap().kids[0].child("data").unwrap().text.len(), 2 << 20);
        for tag in ["data", "appearance"] {
            let form = format!("<xfdf><fields><field name=\"a\"><{tag}>{big}</{tag}></field></fields></xfdf>");
            assert!(parse_xml(form.as_bytes()).is_err(), "{tag} outside <annots> keeps the 1 MB limit");
        }
    }

    #[test]
    fn xml_reads_text_entities_and_namespaces() {
        let x = b"<?xml version=\"1.0\"?>\n<xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><text a=\"x &amp; &quot;y&quot;\"><contents>1 &lt; 2 &#38; &#x26; \xC3\xA9<![CDATA[ <raw> ]]></contents></text><f href=\"a.pdf\"/></xfdf>";
        let n = parse_xml(x).unwrap();
        assert_eq!(n.name, "xfdf");
        let t = n.child("text").unwrap();
        assert_eq!(t.attr("A"), Some("x & \"y\""));
        assert_eq!(t.child("contents").unwrap().text, "1 < 2 & & \u{e9} <raw> ");
        assert_eq!(n.child("f").unwrap().attr("href"), Some("a.pdf"));
    }

    #[test]
    fn xml_refuses_doctypes_entities_depth_and_truncation() {
        let laughs = b"<?xml version=\"1.0\"?><!DOCTYPE l [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;\">]><xfdf>&b;</xfdf>";
        assert!(parse_xml(laughs).is_err(), "billion laughs");
        let ext = b"<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY e SYSTEM \"file:///etc/passwd\">]><xfdf>&e;</xfdf>";
        assert!(parse_xml(ext).is_err(), "external entity");
        assert!(parse_xml(b"<xfdf>&undefined;</xfdf>").is_err(), "unknown entity without a DOCTYPE");
        let deep = format!("{}{}", "<a>".repeat(200), "</a>".repeat(200));
        assert!(parse_xml(deep.as_bytes()).is_err(), "depth");
        let wide = format!("<r>{}</r>", "<a/>".repeat(MAX_NODES + 1));
        assert!(parse_xml(wide.as_bytes()).is_err(), "element count");
        assert!(parse_xml(b"<xfdf><annots><text").is_err(), "truncated");
        assert!(parse_xml(b"").is_err() && parse_xml(&[0xFF, 0xFE, 0, 1, 2]).is_err(), "garbage");
        let long = format!("<r a=\"{}\"/>", "x".repeat(MAX_STR + 1));
        assert!(parse_xml(long.as_bytes()).is_err(), "string length");
    }

    #[test]
    fn attributes_count_toward_the_node_limit() {
        let attrs = |n: usize| (0..n).map(|i| format!("a{i}=\"1\"")).collect::<Vec<_>>().join(" ");
        assert!(parse_xml(format!("<r {}/>", attrs(MAX_NODES - 1)).as_bytes()).is_ok(), "right at the limit");
        let e = parse_xml(format!("<r {}/>", attrs(MAX_NODES)).as_bytes()).unwrap_err();
        assert!(e.to_string().contains("100 000"), "{e}");
        // Namespace declarations are read too, so they count.
        let ns = (0..MAX_NODES).map(|i| format!("xmlns:n{i}=\"u\"")).collect::<Vec<_>>().join(" ");
        assert!(parse_xml(format!("<r {ns}/>").as_bytes()).is_err());
    }

    #[test]
    fn sniff_and_names() {
        assert_eq!(sniff(b"  \n%FDF-1.2\n"), Some(Format::Fdf));
        assert_eq!(sniff(b"\xEF\xBB\xBF<?xml"), Some(Format::Xfdf));
        assert_eq!(sniff(b"{\"a\":1}"), Some(Format::Json));
        assert_eq!(sniff(b"%PDF-1.7"), None);
        assert_eq!(pdf_text("a (b) \\"), "(a \\(b\\) \\\\)");
        assert_eq!(pdf_text("\u{e9}"), "<FEFF00E9>");
        assert_eq!(pdf_name("A B#"), "/A#20B#23");
        assert_eq!(Format::of_path("C:\\x\\a.FDF", false), Format::Fdf);
        assert_eq!(Format::of_path("a.json", false), Format::Xfdf);
        assert_eq!(Format::of_path("a.json", true), Format::Json);
        assert_eq!(Format::of_path("a.CSV", true), Format::Csv);
        assert_eq!(Format::of_path("a.txt", true), Format::Tab);
        assert_eq!(Format::of_path("a.xml", true), Format::Xml);
        // Comments stay XFDF or FDF.
        assert_eq!(Format::of_path("a.csv", false), Format::Xfdf);
        assert_eq!(Format::of_path("a.xml", false), Format::Xfdf);
    }

    #[test]
    fn a_data_file_over_the_cap_is_refused_without_being_read() {
        let p = std::env::temp_dir().join(format!("inkwell-exchange-big-{}.xfdf", std::process::id()));
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(100 << 20).unwrap();
        drop(f);
        let e = read_capped(p.to_str().unwrap());
        std::fs::remove_file(&p).ok();
        assert!(matches!(e, Err(EngineError::Invalid(m)) if m.contains("32 MB")));
    }
}
