//! Saved signatures and initials: one encrypted file per record in `<data dir>/signatures/`.
//!
//! Same scheme as the history store (`history::crypto`): a random key wrapped with DPAPI in
//! `key.dpapi`, XChaCha20-Poly1305 per record. The record id and kind are bound into the AAD, so a
//! record can't be renamed or re-labelled. File = kind byte (`s`/`i`, also in the AAD) + sealed JSON.
//! The webview draws the PNGs; this only stores them. Nothing here ever sees a user's PDF.

use crate::engine::EngineError;
use crate::history::crypto::{self, Keys, Obj};
use crate::history::{DocId, write_atomic};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

type Result<T> = std::result::Result<T, EngineError>;

pub const MAX_PNG: usize = 2 << 20;
pub const MAX_PER_KIND: usize = 10;
/// A picture from the user's disk, before it is normalized to PNG.
pub const MAX_SOURCE: u64 = 4 << 20;
const MAX_SIDE: u32 = 4096;
const MAX_PIXELS: u64 = 4 << 20;
/// Records are not tied to a document: this plays the document part of the AAD.
const NO_DOC: DocId = DocId([0; 16]);

fn bad(m: impl Into<String>) -> EngineError {
    EngineError::Invalid(m.into())
}

fn io(e: impl std::fmt::Display) -> EngineError {
    EngineError::Io(e.to_string())
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Signature,
    Initials,
}

impl Kind {
    fn obj(self) -> Obj {
        if self == Kind::Signature { Obj::Signature } else { Obj::Initials }
    }
    fn from_byte(b: u8) -> Option<Kind> {
        [Kind::Signature, Kind::Initials].into_iter().find(|k| k.obj() as u8 == b)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Typed,
    Drawn,
    Image,
}

#[derive(Clone, Debug)]
pub struct Saved {
    pub id: String,
    pub kind: Kind,
    pub source: Source,
    /// The script font a typed signature was rendered in.
    pub font: Option<String>,
    /// Unix seconds.
    pub created: u64,
    pub png: Vec<u8>,
}

/// The encrypted part of a record.
#[derive(Serialize, Deserialize)]
struct Body {
    source: Source,
    font: Option<String>,
    created: u64,
    png: String,
}

/// 32 lowercase hex characters: the id is a file name, so nothing else gets near the disk.
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join("key.dpapi")
    }

    fn keys(&self, create: bool) -> Result<Option<Keys>> {
        match std::fs::read(self.key_path()) {
            Ok(b) => Ok(Some(Keys::new(&*crypto::unprotect(&b).map_err(io)?))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                let key = zeroize::Zeroizing::new(crypto::random::<32>());
                let sealed = crypto::protect(&key).map_err(io)?;
                // Create-new, so two first saves at once can't each keep a different key: the
                // key is written whole to a temp file and hard-linked into place (which fails if
                // the file exists), and the loser reads the winner's.
                std::fs::create_dir_all(&self.dir).map_err(io)?;
                let tmp = self.dir.join(format!("key.{}.tmp", crypto::random::<8>().iter().map(|b| format!("{b:02x}")).collect::<String>()));
                let written = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).and_then(|mut f| {
                    use std::io::Write;
                    f.write_all(&sealed)?;
                    f.sync_all()
                });
                let linked = written.and_then(|()| std::fs::hard_link(&tmp, self.key_path()));
                let _ = std::fs::remove_file(&tmp);
                match linked {
                    Ok(()) => Ok(Some(Keys::new(&key))),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => self.keys(false),
                    Err(e) => Err(io(e)),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    fn read(&self, keys: &Keys, id: &str) -> Result<Saved> {
        let raw = std::fs::read(self.dir.join(format!("{id}.rec"))).map_err(io)?;
        let kind = raw.first().and_then(|b| Kind::from_byte(*b)).ok_or_else(|| bad("not a saved signature"))?;
        let plain = keys
            .open(NO_DOC, kind.obj(), id.as_bytes(), &raw[1..])
            .map_err(|_| bad("the saved signature failed its integrity check"))?;
        let body: Body = serde_json::from_slice(&plain).map_err(|_| bad("the saved signature is damaged"))?;
        let png = base64::engine::general_purpose::STANDARD.decode(body.png).map_err(|_| bad("the saved signature is damaged"))?;
        Ok(Saved { id: id.to_owned(), kind, source: body.source, font: body.font, created: body.created, png })
    }

    /// Every record that opens, oldest first. A damaged record is left out (it can still be removed by id).
    pub fn list(&self) -> Result<Vec<Saved>> {
        let Some(keys) = self.keys(false)? else { return Ok(Vec::new()) };
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Ok(Vec::new()) };
        let mut out: Vec<Saved> = rd
            .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".rec").map(str::to_owned))
            .filter(|id| valid_id(id))
            .filter_map(|id| self.read(&keys, &id).ok())
            .collect();
        out.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Saved> {
        if !valid_id(id) {
            return Err(bad("no such saved signature"));
        }
        let keys = self.keys(false)?.ok_or_else(|| bad("no such saved signature"))?;
        self.read(&keys, id)
    }

    /// Stores `png` (already validated by [`normalize`]) as a new record.
    pub fn add(&self, kind: Kind, source: Source, font: Option<String>, png: Vec<u8>) -> Result<Saved> {
        if png.len() > MAX_PNG {
            return Err(bad("the image is larger than 2 MB"));
        }
        // ponytail: not locked, two simultaneous adds can pass the cap together. One user, one window; lock if that changes.
        if self.list()?.iter().filter(|s| s.kind == kind).count() >= MAX_PER_KIND {
            return Err(bad(format!("at most {MAX_PER_KIND} saved {}", if kind == Kind::Signature { "signatures" } else { "initials" })));
        }
        let keys = self.keys(true)?.expect("created");
        let id: String = crypto::random::<16>().iter().map(|b| format!("{b:02x}")).collect();
        let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let font = font.map(|f| f.chars().take(64).collect::<String>());
        let body = Body { source, font: font.clone(), created, png: base64::engine::general_purpose::STANDARD.encode(&png) };
        let sealed = keys.seal(NO_DOC, kind.obj(), id.as_bytes(), &serde_json::to_vec(&body).map_err(io)?);
        let mut file = vec![kind.obj() as u8];
        file.extend_from_slice(&sealed);
        write_atomic(&self.dir.join(format!("{id}.rec")), &file).map_err(io)?;
        Ok(Saved { id, kind, source, font, created, png })
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        if !valid_id(id) {
            return Err(bad("no such saved signature"));
        }
        std::fs::remove_file(self.dir.join(format!("{id}.rec"))).map_err(io)
    }
}

/// Width and height from a PNG's header.
pub fn png_size(png: &[u8]) -> [u32; 2] {
    let be = |i: usize| png.get(i..i + 4).map_or(0, |b| u32::from_be_bytes(b.try_into().unwrap()));
    [be(16), be(20)]
}

// Adapted from pdfcraft (MIT OR Apache-2.0), crates/engine/src/signature_image.rs @ b8ddf71
/// Checks `bytes` is a PNG (`png_only`) or a PNG/JPEG within the size limits, and returns it as a PNG
/// without metadata. Alpha is kept; with `knockout`, a fully opaque picture (a photo of a signature
/// on paper) has its near-white paper made transparent.
pub fn normalize(bytes: &[u8], png_only: bool, knockout: bool) -> Result<Vec<u8>> {
    use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
    use std::io::Cursor;
    let format = image::guess_format(bytes).map_err(|_| bad("choose a PNG or JPEG image"))?;
    if !(format == ImageFormat::Png || (!png_only && format == ImageFormat::Jpeg)) {
        return Err(bad(if png_only { "not a PNG image" } else { "choose a PNG or JPEG image" }));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(32 << 20);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| bad("the image is too large or damaged"))?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(bad("the image is too large (at most 4 megapixels)"));
    }
    let orientation = decoder.orientation().map_err(|_| bad("the image is damaged"))?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(|_| bad("the image is damaged"))?;
    img.apply_orientation(orientation);
    let mut rgba = img.into_rgba8();
    if knockout && rgba.pixels().all(|p| p.0[3] == 255) {
        for p in rgba.pixels_mut() {
            let white = f32::from(p.0[0].min(p.0[1]).min(p.0[2]));
            p.0[3] = (((235.0 - white) / 40.0).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(rgba).write_to(&mut out, ImageFormat::Png).map_err(|_| bad("the image could not be converted"))?;
    let out = out.into_inner();
    if out.len() > MAX_PNG {
        return Err(bad("the image is larger than 2 MB"));
    }
    Ok(out)
}

/// A PNG from the webview, as base64 (a `data:image/png;base64,` prefix is fine).
pub fn decode_png(b64: &str) -> Result<Vec<u8>> {
    let b64 = b64.split_once(',').map_or(b64, |(_, rest)| rest);
    // the encoded text is 4/3 of the bytes: refuse before decoding something huge
    if b64.len() > MAX_PNG * 4 / 3 + 8 {
        return Err(bad("the image is larger than 2 MB"));
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| bad("the image is not valid base64"))?;
    normalize(&bytes, true, false)
}
