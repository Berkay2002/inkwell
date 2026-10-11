//! Rich media (docs/media.md): the video, sound, 3D and Flash a PDF carries, listed and, for the
//! formats a browser plays natively, read out to a `<video>` or `<audio>` over the `inkwell://` media
//! route. Nothing here edits the document, runs an embedded program or fetches an outside file.
//!
//! The PDF is untrusted: every walk has a visited set and a budget, a declared type is checked
//! against the bytes (never the other way round), and only a fixed list of formats is playable.

use crate::engine::{DocId, Engine, EngineError};
use crate::forms::{bad, get};
use crate::organize::attach::{clean_name, stream_of, strip_invisible};
use mupdf::pdf::{PdfAnnotationType, PdfDocument, PdfObject};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Arc;

pub mod app;
pub mod route;
#[cfg(test)]
mod tests;

type Result<T> = std::result::Result<T, EngineError>;

/// The largest asset played (decoded bytes). Above it: "too large to play here".
pub const MAX_PLAY: usize = 256 << 20;
/// Per page and per document (hostile files list thousands).
const MAX_PAGE_ITEMS: usize = 1_000;
const MAX_DOC_ITEMS: usize = 10_000;
/// Entries of one page's /Annots looked at.
const MAX_SCANNED: usize = 100_000;
/// /Annots entries screened in the whole document: pages may share one huge array.
const MAX_DOC_SCANNED: usize = 500_000;
/// Annotations of the pages that are loaded (MuPDF builds each one, about 100 us): the same shared
/// array behind every page would load it again for each.
const MAX_DOC_LOADED: usize = 50_000;
const MAX_ASSETS: usize = 500;
/// Name-tree nodes looked at per annotation (kids may repeat or loop).
const MAX_TREE_NODES: usize = 2_000;
const MAX_ACTIONS: usize = 32;
const MAX_TITLE: usize = 160;
/// Bytes read to recognise a format: enough for every signature below.
const HEAD: usize = 64;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    RichMedia,
    Screen,
    Movie,
    Sound,
    ThreeD,
}

impl Kind {
    fn of(t: PdfAnnotationType) -> Option<Kind> {
        Some(match t {
            PdfAnnotationType::RichMedia => Kind::RichMedia,
            PdfAnnotationType::Screen => Kind::Screen,
            PdfAnnotationType::Movie => Kind::Movie,
            PdfAnnotationType::Sound => Kind::Sound,
            PdfAnnotationType::ThreeD => Kind::ThreeD,
            _ => return None,
        })
    }

    fn of_subtype(name: &[u8]) -> Option<Kind> {
        Some(match name {
            b"RichMedia" => Kind::RichMedia,
            b"Screen" => Kind::Screen,
            b"Movie" => Kind::Movie,
            b"Sound" => Kind::Sound,
            b"3D" => Kind::ThreeD,
            _ => return None,
        })
    }
}

/// `video` or `audio`: which element plays it.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Element {
    Video,
    Audio,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    /// `<annotation object number>.<index>`: what the media route takes.
    pub id: String,
    pub name: String,
    /// The type found in the bytes (never the declared one); none when not recognised.
    pub mime: Option<String>,
    pub size: u64,
    /// The asset's bytes are in the file (an outside file or a link is never loaded).
    pub embedded: bool,
    pub playable: bool,
    /// Why it is not playable.
    pub reason: Option<String>,
    pub element: Option<Element>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub page: u32,
    pub xref: i32,
    pub kind: Kind,
    /// Page space, as comments use it (points, y down, as the viewer shows the page).
    pub rect: [f32; 4],
    pub title: String,
    pub playable: bool,
    /// The asset a click plays.
    pub play: Option<String>,
    /// Why nothing plays.
    pub reason: Option<String>,
    /// The item carries JavaScript (an action, or a script file): it is never run.
    pub scripts: bool,
    pub assets: Vec<Asset>,
}

// ---- formats -----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Mp4Video,
    Mp4Audio,
    Mov,
    WebM,
    Mkv,
    Mp3,
    Aac,
    Wav,
    Ogg,
    Swf,
    Flv,
    U3d,
    Prc,
    Exe,
    Script,
    Unknown,
}

impl Format {
    /// What the first bytes say.
    fn sniff(h: &[u8]) -> Format {
        let at = |i: usize, s: &[u8]| h.len() >= i + s.len() && &h[i..i + s.len()] == s;
        if at(4, b"ftyp") {
            return match h.get(8..12) {
                Some(b"M4A ") | Some(b"M4B ") | Some(b"M4P ") => Format::Mp4Audio,
                Some(b"qt  ") => Format::Mov,
                _ => Format::Mp4Video,
            };
        }
        if [b"moov".as_slice(), b"mdat", b"wide", b"free", b"skip"].iter().any(|a| at(4, a)) {
            return Format::Mov;
        }
        if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
            return if h.windows(4).any(|w| w == b"webm") { Format::WebM } else { Format::Mkv };
        }
        if at(0, b"OggS") {
            return Format::Ogg;
        }
        if at(0, b"RIFF") && at(8, b"WAVE") {
            return Format::Wav;
        }
        if at(0, b"FWS") || at(0, b"CWS") || at(0, b"ZWS") {
            return Format::Swf;
        }
        if at(0, b"FLV\x01") {
            return Format::Flv;
        }
        if at(0, b"U3D\0") {
            return Format::U3d;
        }
        if at(0, b"PRC") {
            return Format::Prc;
        }
        if at(0, b"MZ")
            || at(0, &[0x7F, b'E', b'L', b'F'])
            || [[0xFE, 0xED, 0xFA, 0xCE], [0xFE, 0xED, 0xFA, 0xCF], [0xCE, 0xFA, 0xED, 0xFE], [0xCF, 0xFA, 0xED, 0xFE], [0xCA, 0xFE, 0xBA, 0xBE]]
                .iter()
                .any(|m| at(0, m))
        {
            return Format::Exe;
        }
        if at(0, b"#!") {
            return Format::Script;
        }
        if at(0, b"ID3") {
            return Format::Mp3;
        }
        // A frame sync: the layer bits tell MP3 (01, 10, 11) from ADTS AAC (00).
        if h.len() >= 2 && h[0] == 0xFF && h[1] & 0xE0 == 0xE0 {
            return if h[1] & 0x06 == 0 && h[1] & 0xF0 == 0xF0 { Format::Aac } else if h[1] & 0x06 != 0 { Format::Mp3 } else { Format::Unknown };
        }
        Format::Unknown
    }

    fn playable(self) -> bool {
        matches!(self, Format::Mp4Video | Format::Mp4Audio | Format::WebM | Format::Mp3 | Format::Aac | Format::Wav | Format::Ogg)
    }

    /// The `Content-Type` served for it (a fixed list: the declared type is never echoed).
    fn mime(self) -> Option<&'static str> {
        Some(match self {
            Format::Mp4Video => "video/mp4",
            Format::Mp4Audio => "audio/mp4",
            Format::WebM => "video/webm",
            Format::Mp3 => "audio/mpeg",
            Format::Aac => "audio/aac",
            Format::Wav => "audio/wav",
            Format::Ogg => "audio/ogg",
            _ => return None,
        })
    }

    fn element(self) -> Option<Element> {
        match self {
            Format::Mp4Video | Format::WebM => Some(Element::Video),
            f if f.playable() => Some(Element::Audio),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Format::Mp4Video => "MP4 video",
            Format::Mp4Audio => "MP4 audio",
            Format::Mov => "a QuickTime movie",
            Format::WebM => "WebM",
            Format::Mkv => "Matroska video",
            Format::Mp3 => "MP3",
            Format::Aac => "AAC",
            Format::Wav => "WAV",
            Format::Ogg => "Ogg",
            Format::Swf => "Flash (SWF)",
            Format::Flv => "Flash video (FLV)",
            Format::U3d => "3D (U3D)",
            Format::Prc => "3D (PRC)",
            Format::Exe => "an executable program",
            Format::Script => "a script",
            Format::Unknown => "an unrecognised format",
        }
    }

    /// Same container (an MP4 holds video or audio).
    fn same(self, o: Format) -> bool {
        self == o || matches!((self, o), (Format::Mp4Video, Format::Mp4Audio) | (Format::Mp4Audio, Format::Mp4Video))
    }

    /// The format a content type or a file name claims; none for what is not a media type.
    fn declared(s: &str) -> Option<Format> {
        let s = s.trim().to_ascii_lowercase();
        let s = s.split(';').next().unwrap_or("").trim();
        if s.contains('/') {
            return Some(match s {
                "video/mp4" | "audio/mp4" | "video/x-m4v" | "video/m4v" | "audio/x-m4a" | "audio/m4a" => Format::Mp4Video,
                "video/quicktime" => Format::Mov,
                "video/webm" | "audio/webm" => Format::WebM,
                "video/x-matroska" => Format::Mkv,
                "audio/mpeg" | "audio/mp3" => Format::Mp3,
                "audio/aac" | "audio/x-aac" => Format::Aac,
                "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => Format::Wav,
                "audio/ogg" | "video/ogg" | "application/ogg" => Format::Ogg,
                "application/x-shockwave-flash" => Format::Swf,
                "video/x-flv" => Format::Flv,
                m if m.contains("javascript") || m.contains("ecmascript") => Format::Script,
                _ => return None,
            });
        }
        Some(match s.rsplit_once('.')?.1 {
            "mp4" | "m4v" | "m4a" => Format::Mp4Video,
            "mov" | "qt" => Format::Mov,
            "webm" => Format::WebM,
            "mkv" => Format::Mkv,
            "mp3" => Format::Mp3,
            "aac" => Format::Aac,
            "wav" => Format::Wav,
            "ogg" | "oga" | "ogv" | "opus" => Format::Ogg,
            "swf" => Format::Swf,
            "flv" => Format::Flv,
            "u3d" => Format::U3d,
            "prc" => Format::Prc,
            "js" | "as" => Format::Script,
            "exe" | "dll" | "scr" | "com" | "bat" | "cmd" | "msi" => Format::Exe,
            _ => return None,
        })
    }
}

// ---- collecting an annotation's assets ---------------------------------------------------------

/// One asset as the file describes it, before its bytes are looked at.
struct Raw {
    name: String,
    /// The stream with the bytes; none for an outside file or a link.
    stream: Option<PdfObject>,
    /// Content types and names that claim a type.
    declared: Vec<String>,
    /// A reason that decides it whatever the bytes are.
    forced: Option<&'static str>,
    /// Raw sound samples: no player takes them.
    samples: bool,
    /// The object number of what it came from (the same file spec is listed once).
    key: Option<i32>,
}

fn dict_of(o: &PdfObject) -> Option<PdfObject> {
    o.resolve().ok().flatten().filter(|d| d.is_dict().unwrap_or(false))
}

fn str_of(o: &PdfObject, k: &str) -> Option<String> {
    get(o, k).and_then(|v| v.as_string().ok()).map(|s| strip_invisible(&s)).filter(|s| !s.trim().is_empty())
}

fn name_of(o: &PdfObject, k: &str) -> Option<String> {
    get(o, k).filter(|v| v.is_name().unwrap_or(false)).and_then(|v| v.as_name().ok()).map(|n| String::from_utf8_lossy(&n).into_owned())
}

fn obj_num(o: &PdfObject) -> Option<i32> {
    o.is_indirect().unwrap_or(false).then(|| o.as_indirect().ok()).flatten()
}

/// The decoded size a stream declares (`/Params /Size`, else its stored length).
fn stream_size(s: &PdfObject) -> u64 {
    let params = get(s, "Params").and_then(|p| dict_of(&p));
    params
        .and_then(|p| get(&p, "Size"))
        .and_then(|v| v.as_int().ok())
        .or_else(|| get(s, "Length").and_then(|v| v.as_int().ok()))
        .unwrap_or(0)
        .max(0) as u64
}

/// A file specification (a dictionary or a plain string) as an asset.
fn from_spec(spec: &PdfObject, fallback: &str, declared: Option<String>, out: &mut Vec<Raw>) {
    if out.len() >= MAX_ASSETS {
        return;
    }
    let mut raw = Raw { name: String::new(), stream: None, declared: declared.into_iter().collect(), forced: None, samples: false, key: obj_num(spec) };
    if let Some(d) = dict_of(spec) {
        let n = str_of(&d, "UF").or_else(|| str_of(&d, "F")).unwrap_or_else(|| fallback.to_string());
        raw.name = clean_name(&n);
        raw.stream = stream_of(&d);
        raw.key = raw.key.or_else(|| raw.stream.as_ref().and_then(obj_num));
    } else if let Some(s) = spec.is_string().ok().filter(|b| *b).and_then(|_| spec.as_string().ok()) {
        raw.name = clean_name(&s);
    } else {
        return;
    }
    if let Some(k) = raw.key {
        if out.iter().any(|r| r.key == Some(k)) {
            return;
        }
    }
    raw.declared.push(raw.name.clone());
    if let Some(s) = &raw.stream {
        if let Some(t) = name_of(s, "Subtype") {
            raw.declared.push(t.replace("#2F", "/"));
        }
    }
    out.push(raw);
}

/// The leaves of a name tree (name, value). A node is read once; the walk is bounded.
fn name_tree(root: PdfObject, cap: usize) -> Vec<(String, PdfObject)> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![(root, 0usize)];
    let mut nodes = 0usize;
    while let Some((n, depth)) = stack.pop() {
        if nodes >= MAX_TREE_NODES || out.len() >= cap {
            break;
        }
        if let Some(num) = obj_num(&n) {
            if !seen.insert(num) {
                continue;
            }
        }
        let Some(n) = dict_of(&n) else { continue };
        nodes += 1;
        if let Some(names) = get(&n, "Names") {
            let len = names.len().unwrap_or(0);
            let mut i = 0;
            while i + 1 < len && out.len() < cap {
                let key = names.get_array(i as i32).ok().flatten().and_then(|k| k.as_string().ok());
                if let (Some(key), Some(v)) = (key, names.get_array(i as i32 + 1).ok().flatten()) {
                    out.push((key, v));
                }
                i += 2;
            }
        }
        if depth < 8 {
            if let Some(kids) = get(&n, "Kids") {
                let len = kids.len().unwrap_or(0).min(MAX_TREE_NODES);
                // what is queued counts against the budget, or a wide node fans out without end
                let room = MAX_TREE_NODES.saturating_sub(nodes + stack.len());
                let kids: Vec<PdfObject> = (0..len).filter_map(|i| kids.get_array(i as i32).ok().flatten()).take(room).collect();
                // popped in order
                stack.extend(kids.into_iter().rev().map(|k| (k, depth + 1)));
            }
        }
    }
    out
}

/// The action dictionaries of an annotation: /A and every /AA value, and what /Next chains to.
fn actions(annot: &PdfObject) -> Vec<PdfObject> {
    let mut out: Vec<PdfObject> = Vec::new();
    let mut seen = HashSet::new();
    let mut queue: Vec<PdfObject> = Vec::new();
    if let Some(a) = get(annot, "A") {
        queue.push(a);
    }
    if let Some(aa) = get(annot, "AA").and_then(|d| dict_of(&d)) {
        for i in 0..aa.dict_len().unwrap_or(0).min(MAX_ACTIONS) {
            if let Some(v) = aa.get_dict_val(i as i32).ok().flatten() {
                queue.push(v);
            }
        }
    }
    while let Some(a) = queue.pop() {
        if out.len() >= MAX_ACTIONS {
            break;
        }
        if let Some(n) = obj_num(&a) {
            if !seen.insert(n) {
                continue;
            }
        }
        let Some(d) = dict_of(&a) else { continue };
        if let Some(next) = get(&d, "Next") {
            if next.is_array().unwrap_or(false) {
                for i in 0..next.len().unwrap_or(0).min(MAX_ACTIONS) {
                    queue.extend(next.get_array(i as i32).ok().flatten());
                }
            } else {
                queue.push(next);
            }
        }
        out.push(d);
    }
    out
}

fn is_script_action(a: &PdfObject) -> bool {
    name_of(a, "S").as_deref() == Some("JavaScript") || get(a, "JS").is_some()
}

/// A media clip's data (`/D`): a file specification, a URL, or the media bytes as a stream.
fn clip(c: &PdfObject, depth: usize, out: &mut Vec<Raw>) {
    let Some(c) = dict_of(c) else { return };
    let Some(d) = get(&c, "D") else { return };
    // a clip section points at the clip it is cut from
    if name_of(&c, "S").as_deref() == Some("MCS") {
        if depth < 4 {
            clip(&d, depth + 1, out);
        }
        return;
    }
    let ct = str_of(&c, "CT");
    let label = str_of(&c, "N").unwrap_or_else(|| "media".into());
    if d.is_stream().unwrap_or(false) {
        let mut declared: Vec<String> = ct.into_iter().collect();
        declared.extend(name_of(&d, "Subtype").map(|t| t.replace("#2F", "/")));
        if out.len() < MAX_ASSETS {
            out.push(Raw { name: clean_name(&label), key: obj_num(&d), stream: Some(d), declared, forced: None, samples: false });
        }
    } else {
        from_spec(&d, &label, ct, out);
    }
}

/// A rendition: a media rendition's clip (best-effort and must-honor too), or a selector's list.
fn rendition(r: &PdfObject, depth: usize, out: &mut Vec<Raw>) {
    let Some(r) = dict_of(r) else { return };
    if depth > 4 {
        return;
    }
    match name_of(&r, "S").as_deref() {
        Some("SR") => {
            if let Some(list) = get(&r, "R").filter(|l| l.is_array().unwrap_or(false)) {
                for i in 0..list.len().unwrap_or(0).min(MAX_ACTIONS) {
                    if let Some(x) = list.get_array(i as i32).ok().flatten() {
                        rendition(&x, depth + 1, out);
                    }
                }
            }
        }
        _ => {
            if let Some(c) = get(&r, "C") {
                clip(&c, 0, out);
            }
            for k in ["MH", "BE"] {
                if let Some(c) = get(&r, k).and_then(|v| dict_of(&v)).and_then(|v| get(&v, "C")) {
                    clip(&c, 0, out);
                }
            }
        }
    }
}

/// The annotation's assets in a fixed order (the media route recomputes it), and whether scripts
/// are attached.
fn collect(annot: &PdfObject, kind: Kind) -> (Vec<Raw>, bool) {
    let mut out: Vec<Raw> = Vec::new();
    let mut scripts = actions(annot).iter().any(is_script_action);
    match kind {
        Kind::RichMedia => {
            let content = get(annot, "RichMediaContent").and_then(|c| dict_of(&c));
            if let Some(c) = &content {
                // the instances first (what the page plays), then the rest of the asset tree
                let mut three_d: Vec<usize> = Vec::new();
                if let Some(configs) = get(c, "Configurations").filter(|v| v.is_array().unwrap_or(false)) {
                    for i in 0..configs.len().unwrap_or(0).min(MAX_ACTIONS) {
                        let Some(cfg) = configs.get_array(i as i32).ok().flatten().and_then(|v| dict_of(&v)) else { continue };
                        let Some(inst) = get(&cfg, "Instances").filter(|v| v.is_array().unwrap_or(false)) else { continue };
                        for j in 0..inst.len().unwrap_or(0).min(MAX_ACTIONS) {
                            let Some(ins) = inst.get_array(j as i32).ok().flatten().and_then(|v| dict_of(&v)) else { continue };
                            let Some(asset) = get(&ins, "Asset") else { continue };
                            let before = out.len();
                            from_spec(&asset, "media", None, &mut out);
                            if name_of(&ins, "Subtype").as_deref() == Some("3D") && out.len() > before {
                                three_d.push(before);
                            }
                        }
                    }
                }
                for i in three_d {
                    out[i].forced = Some("3D content is not supported");
                }
                if let Some(root) = get(c, "Assets") {
                    for (key, spec) in name_tree(root, MAX_ASSETS) {
                        from_spec(&spec, &key, None, &mut out);
                    }
                }
            }
        }
        Kind::Screen => {
            for a in actions(annot) {
                if name_of(&a, "S").as_deref() == Some("Rendition") {
                    if let Some(r) = get(&a, "R") {
                        rendition(&r, 0, &mut out);
                    }
                }
            }
        }
        Kind::Movie => {
            if let Some(f) = get(annot, "Movie").and_then(|m| dict_of(&m)).and_then(|m| get(&m, "F")) {
                from_spec(&f, "movie", None, &mut out);
            }
        }
        Kind::Sound => {
            if let Some(s) = get(annot, "Sound") {
                match get(&s, "F") {
                    // a sound file outside the document
                    Some(f) => from_spec(&f, "sound", None, &mut out),
                    None if s.is_stream().unwrap_or(false) => {
                        let declared = name_of(&s, "Subtype").map(|t| t.replace("#2F", "/")).into_iter().collect();
                        out.push(Raw { name: "sound".into(), key: obj_num(&s), stream: Some(s), declared, forced: None, samples: true });
                    }
                    None => {}
                }
            }
        }
        Kind::ThreeD => {
            if let Some(d) = get(annot, "3DD").filter(|d| d.is_stream().unwrap_or(false)) {
                let fmt = name_of(&d, "Subtype").map(|t| format!("3D model ({t})")).unwrap_or_else(|| "3D model".into());
                out.push(Raw { name: fmt, key: obj_num(&d), stream: Some(d), declared: Vec::new(), forced: Some("3D content is not supported"), samples: false });
            }
        }
    }
    scripts |= out.iter().any(|r| r.declared.iter().any(|d| Format::declared(d) == Some(Format::Script)));
    (out, scripts)
}

// ---- classifying -------------------------------------------------------------------------------

struct Verdict {
    format: Format,
    size: u64,
    embedded: bool,
    /// Why it is not playable.
    reason: Option<String>,
}

fn classify(raw: &Raw) -> Verdict {
    let Some(stream) = &raw.stream else {
        return Verdict { format: Format::Unknown, size: 0, embedded: false, reason: Some("external media, not loaded".into()) };
    };
    let size = stream_size(stream);
    let head = stream.read_stream_head(HEAD).unwrap_or_default();
    let format = Format::sniff(&head);
    let claimed: Vec<Format> = raw.declared.iter().filter_map(|d| Format::declared(d)).collect();
    let reason = if let Some(r) = raw.forced {
        Some(r.to_string())
    } else if !format.playable() {
        Some(match format {
            Format::Unknown if raw.samples => "sound with raw samples, not playable here".into(),
            Format::Unknown if claimed.contains(&Format::Script) => "a script, never run".into(),
            Format::Unknown => match claimed.first() {
                Some(c) => format!("claims to be {} but the data is not", c.label()),
                None => "not a recognised audio or video format".into(),
            },
            Format::Swf => "Flash (SWF) is end of life and is never run".into(),
            Format::Flv => "Flash video (FLV) is not supported".into(),
            Format::U3d | Format::Prc => "3D content is not supported".into(),
            Format::Exe => "an executable program, never run".into(),
            Format::Script => "a script, never run".into(),
            Format::Mov => "QuickTime movies are not played here".into(),
            Format::Mkv => "Matroska video is not played here".into(),
            f => format!("{} is not supported", f.label()),
        })
    } else if let Some(c) = claimed.iter().find(|c| !c.same(format)) {
        Some(format!("declared as {} but the data is {}", c.label(), format.label()))
    } else if size > MAX_PLAY as u64 {
        Some(format!("over {} MB, too large to play here", MAX_PLAY >> 20))
    } else {
        None
    };
    Verdict { format, size, embedded: true, reason }
}

fn asset_of(xref: i32, index: usize, raw: &Raw) -> Asset {
    let v = classify(raw);
    let playable = v.reason.is_none();
    Asset {
        id: format!("{xref}.{index}"),
        name: raw.name.clone(),
        mime: v.format.mime().map(str::to_owned),
        size: v.size,
        embedded: v.embedded,
        playable,
        reason: v.reason,
        element: if playable { v.format.element() } else { None },
    }
}

fn title_of(o: &PdfObject, assets: &[Asset]) -> String {
    let t = str_of(o, "T").or_else(|| str_of(o, "Contents")).or_else(|| assets.first().map(|a| a.name.clone())).unwrap_or_default();
    t.trim().chars().take(MAX_TITLE).collect()
}

fn item_of(page: u32, xref: i32, kind: Kind, rect: [f32; 4], o: &PdfObject) -> MediaItem {
    let (raws, scripts) = collect(o, kind);
    let assets: Vec<Asset> = raws.iter().enumerate().map(|(i, r)| asset_of(xref, i, r)).collect();
    let play = assets.iter().find(|a| a.playable).map(|a| a.id.clone());
    let reason = match (&play, kind, assets.first()) {
        (Some(_), ..) => None,
        (None, Kind::ThreeD, _) => Some("3D content is not supported".to_string()),
        (None, _, Some(a)) => a.reason.clone(),
        (None, ..) => Some("no media in this annotation".to_string()),
    };
    MediaItem { page, xref, kind, rect, title: title_of(o, &assets), playable: play.is_some(), play, reason, scripts, assets }
}

// ---- listing and reading -----------------------------------------------------------------------

/// The length of the page's raw /Annots when it holds a media annotation: only those pages are loaded.
/// The length is taken off `budget`; a page that does not fit is not looked at.
fn media_annots(page: &PdfObject, budget: &mut usize) -> Option<usize> {
    let annots = get(page, "Annots").and_then(|a| a.resolve().ok().flatten())?;
    let len = annots.len().unwrap_or(0).min(MAX_SCANNED);
    if len > *budget {
        *budget = 0;
        return None;
    }
    *budget -= len;
    let any = (0..len).any(|i| {
        annots
            .get_array(i as i32)
            .ok()
            .flatten()
            .and_then(|a| dict_of(&a))
            .and_then(|a| name_of(&a, "Subtype"))
            .is_some_and(|s| Kind::of_subtype(s.as_bytes()).is_some())
    });
    any.then_some(len)
}

/// Every media annotation of the document, in page order.
pub fn list(doc: &PdfDocument) -> Result<Vec<MediaItem>> {
    let mut out = Vec::new();
    let mut scan = MAX_DOC_SCANNED;
    let mut load = MAX_DOC_LOADED;
    for p in 0..doc.page_count()?.max(0) {
        if out.len() >= MAX_DOC_ITEMS || scan == 0 {
            break;
        }
        let Some(len) = media_annots(&doc.find_page(p)?, &mut scan) else { continue };
        if len > load {
            continue;
        }
        load -= len;
        let page = doc.load_pdf_page(p)?;
        let mut here = 0;
        for a in page.annotations() {
            let Some(kind) = a.r#type().ok().and_then(Kind::of) else { continue };
            if here >= MAX_PAGE_ITEMS || out.len() >= MAX_DOC_ITEMS {
                break;
            }
            here += 1;
            let (Ok(r), Ok(xref)) = (a.rect(), a.xref()) else { continue };
            out.push(item_of(p as u32, xref, kind, [r.x0, r.y0, r.x1, r.y1], &a.object()));
        }
    }
    Ok(out)
}

pub fn list_all(eng: &Engine, doc: DocId) -> Result<Vec<MediaItem>> {
    eng.read_custom(doc, |d| list(d))
}

/// A playable asset's bytes, with the type its bytes have.
pub struct Loaded {
    pub mime: &'static str,
    pub bytes: Arc<Vec<u8>>,
}

fn read(doc: &PdfDocument, id: &str) -> Result<Loaded> {
    let (xref, index) = id
        .split_once('.')
        .and_then(|(x, i)| Some((x.parse::<i32>().ok()?, i.parse::<usize>().ok()?)))
        .filter(|(x, _)| *x > 0)
        .ok_or_else(|| bad("unknown media"))?;
    let annot = doc.new_indirect(xref, 0)?.resolve()?.filter(|a| a.is_dict().unwrap_or(false)).ok_or_else(|| bad("unknown media"))?;
    let kind = name_of(&annot, "Subtype").and_then(|s| Kind::of_subtype(s.as_bytes())).ok_or_else(|| bad("unknown media"))?;
    let (raws, _) = collect(&annot, kind);
    let raw = raws.get(index).ok_or_else(|| bad("unknown media"))?;
    // The same verdict the list gave: what is not playable is never read out.
    let a = asset_of(xref, index, raw);
    if let Some(r) = a.reason {
        return Err(bad(r));
    }
    let stream = raw.stream.as_ref().ok_or_else(|| bad("unknown media"))?;
    let bytes = match stream.read_stream_capped(MAX_PLAY) {
        Ok(b) => b,
        Err(e) if e.is_stream_cap() => return Err(bad(format!("over {} MB, too large to play here", MAX_PLAY >> 20))),
        Err(e) => return Err(e.into()),
    };
    // The bytes read are what was classified (a stream that changes between reads is not played).
    let mime = Format::sniff(&bytes).mime().ok_or_else(|| bad("unknown media"))?;
    Ok(Loaded { mime, bytes: Arc::new(bytes) })
}

/// The one asset kept decoded, so the player's many range requests decode it once.
#[derive(Default)]
pub struct Cache(Mutex<Option<Slot>>);

struct Slot {
    doc: DocId,
    id: String,
    /// The document's generation it was read at.
    generation: u32,
    loaded: Arc<Loaded>,
}

impl Cache {
    pub fn clear(&self) {
        *self.0.lock() = None;
    }

    /// The asset's bytes: from the slot, or read from the document (the lock is held while it is
    /// read, so concurrent range requests do not decode it twice).
    pub fn get(&self, eng: &Engine, doc: DocId, id: &str) -> Result<Arc<Loaded>> {
        let generation = eng.doc_entry(doc)?.generation();
        let mut slot = self.0.lock();
        if let Some(s) = slot.as_ref() {
            if s.doc == doc && s.id == id && s.generation == generation {
                return Ok(s.loaded.clone());
            }
        }
        *slot = None;
        let loaded = Arc::new(eng.read_custom(doc, |d| read(d, id))?);
        *slot = Some(Slot { doc, id: id.to_owned(), generation, loaded: loaded.clone() });
        Ok(loaded)
    }
}

/// Answers one request of the media route (`/m/<doc>/<id>`): the bytes with the range asked for.
/// Only an asset of an open document that [`list`] calls playable is ever served.
pub fn serve(eng: &Engine, cache: &Cache, doc: DocId, id: &str, range: Option<&str>) -> route::Reply {
    match cache.get(eng, doc, id) {
        Ok(l) => route::respond(&l.bytes, l.mime, range),
        Err(EngineError::Invalid(m)) => route::text(404, &m),
        Err(EngineError::NoDoc) => route::text(404, "document is not open"),
        Err(e) => route::text(500, &e.to_string()),
    }
}
