//! Create a PDF: from images (JPEG, PNG, GIF, BMP, TIFF with every page), from a plain-text file, from the
//! clipboard's image, or blank.
//!
//! The result is written into a scratch folder under the system temp dir, never next to the user's
//! files: the UI opens it as an unsaved tab and "Save" asks where it goes. Image pages are never
//! resampled: JPEG data is embedded as it is, everything else is Flate.
//!
//! Text wrapping and decoding rules adapted from pdfcraft (MIT OR Apache-2.0), crates/create/src/lib.rs @ b8ddf71.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mupdf::pdf::page::{InsertImageOptions, PageImageSource};
use mupdf::pdf::PdfDocument;
use mupdf::size::Size;
use mupdf::{Colorspace, Font, Image, Pixmap, Rect};
use serde::{Deserialize, Serialize};

use super::{safe_stem, Error, Progress, Run, Staging};

/// Where a created file lives until the user saves it.
pub fn scratch_root() -> PathBuf {
    std::env::temp_dir().join("inkwell-new")
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PageSize {
    /// Each page is its image plus margins (text and blank pages use Letter).
    #[default]
    Fit,
    A4,
    Letter,
}

impl PageSize {
    fn fixed(self) -> (f32, f32) {
        match self {
            Self::A4 => (595.0, 842.0),
            Self::Letter | Self::Fit => (612.0, 792.0),
        }
    }
}

fn default_margin() -> f32 {
    0.0
}

/// What the UI asks for. Files are open tokens, resolved by the command (see [`CreateSource`]).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateRequest {
    pub source: RequestSource,
    #[serde(default)]
    pub page_size: PageSize,
    /// Points on every side (0..=144). Text defaults to 72 when this is left at 0.
    #[serde(default = "default_margin")]
    pub margin: f32,
    /// Text only, points (4..=72).
    pub font_size: Option<f32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RequestSource {
    /// Open tokens in page order.
    Images { files: Vec<String> },
    /// Open token of a plain-text file.
    Text { file: String },
    Blank { pages: u32 },
    Clipboard,
}

/// A [`RequestSource`] with its tokens turned into paths.
#[derive(Clone, Debug)]
pub enum CreateSource {
    Images(Vec<ImageInput>),
    Text(TextInput),
    Blank(u32),
    Clipboard,
}

pub type ImageInput = PathBuf;
pub type TextInput = PathBuf;

impl RequestSource {
    pub fn resolve(&self, open: &dyn Fn(&str) -> Result<String, crate::engine::EngineError>) -> Result<CreateSource, Error> {
        Ok(match self {
            Self::Images { files } => CreateSource::Images(files.iter().map(|t| open(t).map(PathBuf::from)).collect::<Result<_, _>>()?),
            Self::Text { file } => CreateSource::Text(PathBuf::from(open(file)?)),
            Self::Blank { pages } => CreateSource::Blank(*pages),
            Self::Clipboard => CreateSource::Clipboard,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOutput {
    /// The scratch file (display and "open" only: the UI gets a token for it).
    pub path: String,
    pub pages: u32,
    pub warnings: Vec<String>,
}

const MAX_SIDE: f32 = 14400.0;

// ---------------------------------------------------------------- entry

pub fn create(
    source: &CreateSource,
    req: &CreateRequest,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<CreateOutput, Error> {
    if !(0.0..=144.0).contains(&req.margin) {
        return Err(Error::Invalid("margins must be between 0 and 144 points".into()));
    }
    let mut run = Run { cancel, progress, total: 1 };
    run.check()?;
    let mut doc = PdfDocument::new();
    let mut warnings = Vec::new();
    let name = match source {
        CreateSource::Images(files) => {
            if files.is_empty() {
                return Err(Error::Invalid("no images".into()));
            }
            run.total = files.len() as u32;
            run.step(0);
            for (i, f) in files.iter().enumerate() {
                run.check()?;
                let bytes = fs::read(f).map_err(|e| Error::Invalid(format!("{}: {e}", file_name(f))))?;
                let added = images_from_bytes(&mut doc, &bytes, req).map_err(|e| match e {
                    Error::Mupdf(_) | Error::Invalid(_) => Error::Invalid(format!("{} is not an image Inkwell can read", file_name(f))),
                    other => other,
                })?;
                if added == 0 {
                    warnings.push(format!("{} had no pages.", file_name(f)));
                }
                run.step(i as u32 + 1);
            }
            stem_of(&files[0])
        }
        CreateSource::Text(file) => {
            let raw = fs::read(file).map_err(|e| Error::Invalid(format!("{}: {e}", file_name(file))))?;
            text_pages(&mut doc, &decode_text(&raw), req, &mut run)?;
            stem_of(file)
        }
        CreateSource::Blank(n) => {
            if !(1..=500).contains(n) {
                return Err(Error::Invalid("a blank PDF has 1 to 500 pages".into()));
            }
            let (w, h) = req.page_size.fixed();
            for _ in 0..*n {
                doc.new_page(Size::new(w, h))?;
            }
            "Blank".to_string()
        }
        CreateSource::Clipboard => {
            let mut cb = arboard::Clipboard::new().map_err(|e| Error::Invalid(format!("the clipboard is not available: {e}")))?;
            let img = cb.get_image().map_err(|_| Error::Invalid("The clipboard has no image.".into()))?;
            rgba_page(&mut doc, img.width as u32, img.height as u32, &img.bytes, req)?;
            "Clipboard image".to_string()
        }
    };
    run.check()?;
    let pages = doc.page_count()? as u32;
    let out = write_scratch(&doc, &name)?;
    (run.progress)(Progress { done: run.total, total: run.total, fraction: 1.0 });
    Ok(CreateOutput { path: out.to_string_lossy().into_owned(), pages, warnings })
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn stem_of(p: &Path) -> String {
    p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into())
}

/// Writes `doc` to `<temp>/inkwell-new/<nonce>/<name>.pdf` and returns the path. Scratch folders
/// from earlier runs that are over two days old are swept first.
fn write_scratch(doc: &PdfDocument, name: &str) -> Result<PathBuf, Error> {
    use std::hash::{BuildHasher, Hasher};
    let root = scratch_root();
    fs::create_dir_all(&root)?;
    sweep(&root);
    let nonce = std::collections::hash_map::RandomState::new().build_hasher().finish();
    let dir = root.join(format!("{}-{nonce:08x}", std::process::id()));
    fs::create_dir(&dir)?;
    let result = (|| {
        let staging = Staging::new(&dir)?;
        let staged = "new.tmp";
        doc.save(&staging.path(staged).to_string_lossy())?;
        let out = dir.join(format!("{}.pdf", safe_stem(name)));
        staging.publish(&[(staged.to_string(), out.clone())])?;
        Ok(out)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    result
}

fn sweep(root: &Path) {
    let old = std::time::Duration::from_secs(2 * 24 * 3600);
    let Ok(rd) = fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let age = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        if age.is_some_and(|a| a > old) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// Removes a scratch file and its folder. Anything outside the scratch root is left alone.
pub fn discard_scratch(path: &Path) -> bool {
    let root = scratch_root();
    let (Ok(p), Ok(r)) = (fs::canonicalize(path), fs::canonicalize(&root)) else { return false };
    let Some(dir) = p.parent() else { return false };
    if dir.parent() != Some(r.as_path()) {
        return false;
    }
    fs::remove_dir_all(dir).is_ok()
}

// ---------------------------------------------------------------- image pages

/// Page size and drawn size, in points, for an image of `w` x `h` points.
fn place(size: PageSize, margin: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
    match size {
        PageSize::Fit => {
            let k = (MAX_SIDE / (w + 2.0 * margin).max(h + 2.0 * margin)).min(1.0);
            let (w, h) = (w * k, h * k);
            (w + 2.0 * margin, h + 2.0 * margin, w, h)
        }
        fixed => {
            let (pw, ph) = fixed.fixed();
            // landscape pages for landscape images
            let (pw, ph) = if w > h { (ph, pw) } else { (pw, ph) };
            let (aw, ah) = ((pw - 2.0 * margin).max(36.0), (ph - 2.0 * margin).max(36.0));
            // large images shrink to fit; small ones stay at their size
            let k = (aw / w).min(ah / h).min(1.0);
            (pw, ph, w * k, h * k)
        }
    }
}

fn add_image_page(doc: &mut PdfDocument, req: &CreateRequest, w_pt: f32, h_pt: f32, source: PageImageSource<'_>) -> Result<(), Error> {
    let (pw, ph, dw, dh) = place(req.page_size, req.margin, w_pt, h_pt);
    let mut page = doc.new_page(Size::new(pw, ph))?;
    let (x, y) = ((pw - dw) / 2.0, (ph - dh) / 2.0);
    page.insert_image(doc, Rect::new(x, y, x + dw, y + dh), source, InsertImageOptions::default())?;
    Ok(())
}

/// Points per pixel: the image's own resolution (MuPDF reports 96 dpi for files that state none); an
/// absurd value falls back to 72 dpi (a pixel is a point).
fn points_per_px(dpi: i32) -> f32 {
    if (36..=1200).contains(&dpi) { 72.0 / dpi as f32 } else { 1.0 }
}

/// Adds the pages of one image file; returns how many.
fn images_from_bytes(doc: &mut PdfDocument, bytes: &[u8], req: &CreateRequest) -> Result<u32, Error> {
    if bytes.len() >= 4 && (bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*")) {
        return tiff_pages(doc, bytes, req);
    }
    let img = Image::from_bytes(bytes)?;
    let (rx, ry) = img.resolution();
    let (w, h) = (img.width() as f32 * points_per_px(rx), img.height() as f32 * points_per_px(ry));
    add_image_page(doc, req, w, h, PageImageSource::Image(&img))?;
    Ok(1)
}

fn pixmap_rgb(w: u32, h: u32, channels: u8, data: &[u8]) -> Result<Pixmap, Error> {
    let cs = if channels == 1 { Colorspace::device_gray() } else { Colorspace::device_rgb() };
    let mut pix = Pixmap::new_with_w_h(&cs, w as i32, h as i32, false)?;
    let stride = pix.stride() as usize;
    let row = w as usize * channels as usize;
    let dst = pix.samples_mut();
    for y in 0..h as usize {
        dst[y * stride..y * stride + row].copy_from_slice(&data[y * row..(y + 1) * row]);
    }
    Ok(pix)
}

pub(super) fn rgba_page(doc: &mut PdfDocument, w: u32, h: u32, rgba: &[u8], req: &CreateRequest) -> Result<(), Error> {
    if w == 0 || h == 0 || rgba.len() < w as usize * h as usize * 4 {
        return Err(Error::Invalid("the image is empty".into()));
    }
    // over white
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            [0, 1, 2].map(|i| ((p[i] as u32 * a + 255 * (255 - a)) / 255) as u8)
        })
        .collect();
    let pix = pixmap_rgb(w, h, 3, &rgb)?;
    add_image_page(doc, req, w as f32, h as f32, PageImageSource::Pixmap(&pix))
}

/// Every page (IFD) of a TIFF, decoded to 8-bit grey or RGB.
// Adapted from pdfcraft (MIT OR Apache-2.0), crates/create/src/lib.rs `tiff_pages` @ b8ddf71
fn tiff_pages(doc: &mut PdfDocument, bytes: &[u8], req: &CreateRequest) -> Result<u32, Error> {
    use tiff::decoder::{Decoder, DecodingResult};
    use tiff::tags::{ResolutionUnit, Tag};
    use tiff::ColorType;
    let bad = |e: &dyn std::fmt::Display| Error::Invalid(format!("TIFF: {e}"));
    let mut dec = Decoder::new(Cursor::new(bytes)).map_err(|e| bad(&e))?;
    let mut n = 0u32;
    loop {
        let (w, h) = dec.dimensions().map_err(|e| bad(&e))?;
        let ct = dec.colortype().map_err(|e| bad(&e))?;
        let res = dec.read_image().map_err(|e| bad(&e))?;
        // 8-bit samples, interleaved, as the decoder lays them out
        let samples: Vec<u8> = match res {
            DecodingResult::U8(v) => v,
            DecodingResult::U16(v) => v.into_iter().map(|s| (s >> 8) as u8).collect(),
            _ => return Err(Error::Invalid("TIFF: unsupported sample format".into())),
        };
        let (wz, hz) = (w as usize, h as usize);
        let over_white = |c: u8, a: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
        let (channels, data): (u8, Vec<u8>) = match ct {
            ColorType::Gray(8 | 16) => (1, samples),
            ColorType::Gray(b @ (1 | 2 | 4)) => {
                let per_row = (wz * b as usize).div_ceil(8);
                let max = (1u16 << b) - 1;
                let mut out = Vec::with_capacity(wz * hz);
                for row in samples.chunks(per_row).take(hz) {
                    for x in 0..wz {
                        let bit = x * b as usize;
                        let v = (row[bit / 8] >> (8 - b as usize - bit % 8)) & max as u8;
                        out.push((v as u16 * 255 / max) as u8);
                    }
                }
                (1, out)
            }
            ColorType::GrayA(8 | 16) => (1, samples.chunks_exact(2).map(|p| over_white(p[0], p[1])).collect()),
            ColorType::RGB(8 | 16) => (3, samples),
            ColorType::RGBA(8 | 16) => (3, samples.chunks_exact(4).flat_map(|p| [0, 1, 2].map(|i| over_white(p[i], p[3]))).collect()),
            ColorType::CMYK(8 | 16) => (
                3,
                samples
                    .chunks_exact(4)
                    .flat_map(|p| {
                        let k = 255 - p[3] as u32;
                        [0, 1, 2].map(|i| (((255 - p[i] as u32) * k) / 255) as u8)
                    })
                    .collect(),
            ),
            ColorType::Palette(b @ (1 | 2 | 4 | 8)) => {
                let map = dec.get_tag_u16_vec(Tag::ColorMap).map_err(|e| bad(&e))?;
                let entries = 1usize << b;
                if map.len() < entries * 3 {
                    return Err(Error::Invalid("TIFF: palette is incomplete".into()));
                }
                let per_row = (wz * b as usize).div_ceil(8);
                let max = (1u16 << b) - 1;
                let mut out = Vec::with_capacity(wz * hz * 3);
                for row in samples.chunks(per_row).take(hz) {
                    for x in 0..wz {
                        let bit = x * b as usize;
                        let v = ((row[bit / 8] >> (8 - b as usize - bit % 8)) as u16 & max) as usize;
                        out.extend([map[v], map[entries + v], map[2 * entries + v]].map(|c| (c >> 8) as u8));
                    }
                }
                (3, out)
            }
            other => return Err(Error::Invalid(format!("TIFF: unsupported colour type {other:?}"))),
        };
        // 16-bit grey samples were narrowed above; the data is w*h bytes either way
        if data.len() < wz * hz * channels as usize {
            return Err(Error::Invalid("TIFF: the image data is shorter than its size".into()));
        }
        let unit = dec.find_tag_unsigned::<u16>(Tag::ResolutionUnit).ok().flatten();
        let dpi_of = |tag: Tag, dec: &mut Decoder<Cursor<&[u8]>>| -> i32 {
            let v = dec.get_tag_f64(tag).unwrap_or(0.0);
            match unit.and_then(ResolutionUnit::from_u16) {
                Some(ResolutionUnit::Centimeter) => (v * 2.54).round() as i32,
                Some(ResolutionUnit::None) => 0,
                _ => v.round() as i32,
            }
        };
        let (rx, ry) = (dpi_of(Tag::XResolution, &mut dec), dpi_of(Tag::YResolution, &mut dec));
        let pix = pixmap_rgb(w, h, channels, &data)?;
        add_image_page(doc, req, w as f32 * points_per_px(rx), h as f32 * points_per_px(ry), PageImageSource::Pixmap(&pix))?;
        n += 1;
        if !dec.more_images() {
            break;
        }
        dec.next_image().map_err(|e| bad(&e))?;
    }
    Ok(n)
}

// ---------------------------------------------------------------- text pages

/// The text of a plain-text file. A byte order mark picks UTF-8 or UTF-16; without one the bytes are
/// UTF-8 when valid, else the Windows code page they most likely are: 1254 (Turkish) or 1252 (Western).
// Adapted from pdfcraft (MIT OR Apache-2.0), crates/create/src/lib.rs `decode_text` @ b8ddf71
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |b: &[u8], le: bool| {
        let units: Vec<u16> = b.chunks_exact(2).map(|p| if le { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => return String::from_utf8_lossy(rest).into_owned(),
        [0xFF, 0xFE, rest @ ..] => return utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => return utf16(rest, false),
        _ => {}
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    // Windows-1254 and 1252 differ only in six bytes: Ğ İ Ş ğ ı ş in 1254 are Ð Ý Þ ð ý þ in 1252.
    // Of the Western languages only Icelandic and Faroese use those, and both also write á í ó ú,
    // which Turkish never does.
    let turkish = bytes.iter().any(|b| matches!(b, 0xD0 | 0xDD | 0xDE | 0xF0 | 0xFD | 0xFE))
        && !bytes.iter().any(|b| matches!(b, 0xC1 | 0xCD | 0xD3 | 0xDA | 0xE1 | 0xED | 0xF3 | 0xFA));
    const C1: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•',
        '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => C1[(b - 0x80) as usize],
            0xD0 if turkish => 'Ğ',
            0xDD if turkish => 'İ',
            0xDE if turkish => 'Ş',
            0xF0 if turkish => 'ğ',
            0xFD if turkish => 'ı',
            0xFE if turkish => 'ş',
            _ => b as char,
        })
        .collect()
}

/// Helvetica (MuPDF's built-in face) as an embedded CID font: every Unicode character the face has,
/// not just WinAnsi.
struct Face {
    font: Font,
}

impl Face {
    fn gid(&self, c: char) -> u16 {
        let g = self.font.encode_character(c as i32).unwrap_or(0);
        if g > 0 { g as u16 } else { self.font.encode_character('?' as i32).unwrap_or(0).max(0) as u16 }
    }

    /// Advance in em units.
    fn advance(&self, c: char) -> f32 {
        self.font.advance_glyph(self.gid(c) as i32).unwrap_or(0.5)
    }

    fn width(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.advance(c)).sum::<f32>() * size
    }
}

/// Greedy line breaking within `width` points; words longer than a line are broken by character.
// Adapted from pdfcraft (MIT OR Apache-2.0), crates/fonts/src/lib.rs `wrap_with` @ b8ddf71
fn wrap(face: &Face, text: &str, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if face.width(&candidate, size) <= width || line.is_empty() && face.width(word, size) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            for ch in word.chars() {
                if !line.is_empty() && face.width(&format!("{line}{ch}"), size) > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        lines.push(line);
    }
    lines
}

fn text_pages(doc: &mut PdfDocument, text: &str, req: &CreateRequest, run: &mut Run<'_>) -> Result<(), Error> {
    let size = req.font_size.filter(|s| s.is_finite()).map_or(11.0, |s| s.clamp(4.0, 72.0));
    let margin = if req.margin > 0.0 { req.margin } else { 72.0 };
    let (w, h) = req.page_size.fixed();
    let line_h = size * 1.25;
    let width = (w - 2.0 * margin).max(36.0);
    let per_page = (((h - 2.0 * margin) / line_h).floor() as usize).max(1);

    let face = Face { font: Font::new("Helvetica")? };
    let font_obj = doc.add_font(&face.font)?;

    // pages: a form feed forces a break, as does a full page
    let text = text.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ");
    let mut pages: Vec<Vec<String>> = vec![Vec::new()];
    for (i, chunk) in text.split('\u{c}').enumerate() {
        if i > 0 {
            pages.push(Vec::new());
        }
        for l in wrap(&face, chunk, size, width) {
            if pages.last().is_some_and(|p| p.len() >= per_page) {
                pages.push(Vec::new());
            }
            pages.last_mut().unwrap().push(l);
        }
    }
    // a trailing empty page (text ending in a form feed) is not wanted
    while pages.len() > 1 && pages.last().is_some_and(|p| p.iter().all(String::is_empty)) {
        pages.pop();
    }
    if pages.len() > 5000 {
        return Err(Error::Invalid("the text is too long for one PDF (over 5000 pages)".into()));
    }
    run.total = pages.len() as u32;
    run.step(0);
    for (i, lines) in pages.iter().enumerate() {
        run.check()?;
        let mut c = format!("BT /F1 {size} Tf {line_h:.3} TL {margin} {:.3} Td\n", h - margin - size);
        for l in lines {
            c.push('<');
            for ch in l.chars().filter(|c| !c.is_control()) {
                c.push_str(&format!("{:04x}", face.gid(ch)));
            }
            c.push_str("> Tj T*\n");
        }
        c.push_str("ET\n");
        let mut page = doc.new_page(Size::new(w, h))?;
        let mut fonts = doc.new_dict()?;
        fonts.dict_put("F1", font_obj.clone())?;
        let mut res = doc.new_dict()?;
        res.dict_put("Font", fonts)?;
        let mut obj = page.object();
        obj.dict_put("Resources", res)?;
        page.insert_contents(doc, c.as_bytes(), true)?;
        run.step(i as u32 + 1);
    }
    Ok(())
}
