//! Stream bytes of a data file or of a document's comments, decoded under the file's allowance.
//!
//! The filters are run here, one stage at a time, and not by MuPDF, because only the last stage's output can be
//! bounded there: a Flate stream that inflates to gigabytes of white space and an ASCII hex filter after it ends up as
//! one byte and passes any cap on the result. Every stage's output is capped and charged to the file's decode allowance
//! (`Budget::spend_work`), and a stream that many comments share is decoded once (`Budget::stream_known`); each use of
//! the decoded bytes is charged their size again.
//!
//! What is decoded: Flate, ASCII hex and ASCII 85, in the shapes `[ASCII]? [Flate]{0,2}` (at most one ASCII filter,
//! and only first, on bytes that are in the file: after a decompressor it would eat the white space a bomb is made of).
//! Anything else is [`Unpack::Unsupported`]: the image codecs (DCT, JPX, JBIG2, CCITT, which are for images, and only
//! an image of an appearance is read at all, never decoded here), LZW, run length, `/Crypt`, a Flate predictor
//! (outside of an image's size check), more than three filters.

use super::ap::{dict_of, int_of, name_of, number_of};
use crate::exchange::{Budget, MIN_DECODE_WORK, Unpack, Unpacked};
use crate::forms::get;
use flate2::read::ZlibDecoder;
use mupdf::pdf::PdfObject;
use std::io::Read;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Hex,
    A85,
    Flate,
}

/// Most filters of one stream.
const MAX_FILTERS: usize = 3;

fn stage_of(name: &str) -> Result<Stage, Unpack> {
    match name {
        "FlateDecode" | "Fl" => Ok(Stage::Flate),
        "ASCIIHexDecode" | "AHx" => Ok(Stage::Hex),
        "ASCII85Decode" | "A85" => Ok(Stage::A85),
        _ => Err(Unpack::Unsupported),
    }
}

fn has_predictor(d: Option<PdfObject>) -> bool {
    dict_of(d).is_some_and(|d| int_of(get(&d, "Predictor")).is_some_and(|p| p > 1))
}

/// The filters of `s` in order, or why they are not decoded. `predictors`: a Flate predictor is let through (the
/// caller only wants to know how much the stream inflates to).
fn chain(s: &PdfObject, predictors: bool) -> Result<Vec<Stage>, Unpack> {
    let Some(f) = get(s, "Filter") else { return Ok(Vec::new()) };
    let f = f.resolve().ok().flatten().unwrap_or(f);
    let stages: Vec<Stage> = if f.is_array().unwrap_or(false) {
        let len = f.len().map_err(|_| Unpack::Damaged)?;
        if len > MAX_FILTERS {
            return Err(Unpack::Unsupported);
        }
        (0..len as i32).map(|i| name_of(f.get_array(i).ok().flatten()).ok_or(Unpack::Damaged).and_then(|n| stage_of(&n))).collect::<Result<_, _>>()?
    } else {
        vec![stage_of(&name_of(Some(f)).ok_or(Unpack::Damaged)?)?]
    };
    if stages.iter().skip(1).any(|s| *s != Stage::Flate) {
        return Err(Unpack::Unsupported);
    }
    if !predictors {
        let parms = get(s, "DecodeParms").or_else(|| get(s, "DP"));
        let parms = parms.map(|p| p.resolve().ok().flatten().unwrap_or(p));
        let bad = match parms {
            Some(p) if p.is_array().unwrap_or(false) => (0..p.len().unwrap_or(0).min(MAX_FILTERS) as i32).any(|i| has_predictor(p.get_array(i).ok().flatten())),
            p => has_predictor(p),
        };
        if bad {
            return Err(Unpack::Unsupported);
        }
    }
    Ok(stages)
}

fn flate(z: &[u8], limit: usize) -> Result<Vec<u8>, Unpack> {
    let mut out = Vec::new();
    match ZlibDecoder::new(z).take(limit as u64).read_to_end(&mut out) {
        Ok(_) => Ok(out),
        // A truncated stream gives what it has, as readers do.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !out.is_empty() => Ok(out),
        Err(_) => Err(Unpack::Damaged),
    }
}

fn hex(d: &[u8], limit: usize) -> Result<Vec<u8>, Unpack> {
    let mut out = Vec::with_capacity((d.len() / 2).min(limit) + 1);
    let mut high: Option<u8> = None;
    for &c in d {
        if out.len() > limit {
            break;
        }
        let v = match c {
            b'>' => break,
            c if c.is_ascii_whitespace() || c == 0 => continue,
            c => (c as char).to_digit(16).ok_or(Unpack::Damaged)? as u8,
        };
        match high.take() {
            Some(h) => out.push(h << 4 | v),
            None => high = Some(v),
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    Ok(out)
}

fn a85(d: &[u8], limit: usize) -> Result<Vec<u8>, Unpack> {
    let d = d.strip_prefix(b"<~").unwrap_or(d);
    let mut out = Vec::new();
    let mut group = [84u8; 5];
    let mut n = 0;
    let value = |g: &[u8; 5]| g.iter().fold(0u32, |a, &x| a.wrapping_mul(85).wrapping_add(u32::from(x)));
    for &c in d {
        if out.len() > limit {
            break;
        }
        match c {
            b'~' => break,
            c if c.is_ascii_whitespace() || c == 0 => {}
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = c - b'!';
                n += 1;
                if n == 5 {
                    out.extend_from_slice(&value(&group).to_be_bytes());
                    n = 0;
                }
            }
            _ => return Err(Unpack::Damaged),
        }
    }
    match n {
        0 => {}
        1 => return Err(Unpack::Damaged),
        _ => {
            group[n..].fill(84);
            out.extend_from_slice(&value(&group).to_be_bytes()[..n - 1]);
        }
    }
    Ok(out)
}

/// Runs `stages` over `raw`: each stage's output is held to `cap` (and to what is left of the allowance) and charged.
pub(super) fn run(raw: Vec<u8>, stages: &[Stage], cap: usize, b: &Budget) -> Result<Vec<u8>, Unpack> {
    // The set-up of a decode, and reading what is stored.
    b.spend_work(MIN_DECODE_WORK + raw.len());
    let mut data = raw;
    for st in stages {
        let room = cap.min(b.work_left());
        if room == 0 {
            return Err(Unpack::NoWork);
        }
        let out = match st {
            Stage::Hex => hex(&data, room),
            Stage::A85 => a85(&data, room),
            Stage::Flate => flate(&data, room + 1),
        }?;
        if out.len() > room {
            b.spend_work(room);
            return Err(if room < cap { Unpack::NoWork } else { Unpack::Over });
        }
        b.spend_work(out.len());
        data = out;
    }
    if data.len() > cap {
        return Err(Unpack::Over);
    }
    Ok(data)
}

fn load(s: &PdfObject, cap: usize, predictors: bool, b: &Budget) -> Unpacked {
    let num = number_of(s);
    if let Some(r) = num.and_then(|n| b.stream_known(n, cap, predictors)) {
        // Decoding is not done again, but using the bytes is work: each comment that shares them copies and reads them.
        if let Ok(d) = &r {
            if b.work_left() < d.len() {
                return Err(Unpack::NoWork);
            }
            b.spend_work(d.len());
        }
        return r;
    }
    let r = chain(s, predictors).and_then(|stages| {
        let raw = s.read_raw_stream().map_err(|_| Unpack::Damaged)?;
        run(raw, &stages, cap, b)
    });
    let r = r.map(Rc::new);
    if let Some(n) = num {
        b.stream_keep(n, cap, predictors, &r);
    }
    r
}

/// The decoded bytes of stream `s`, at most `cap`.
pub(super) fn unpack(s: &PdfObject, cap: usize, b: &Budget) -> Unpacked {
    load(s, cap, false, b)
}

/// Whether the Flate stream `s` (an image's, which stays as it is stored) inflates to at most `cap`.
pub(super) fn inflates_within(s: &PdfObject, cap: usize, b: &Budget) -> Result<(), Unpack> {
    load(s, cap, true, b).map(|_| ())
}
