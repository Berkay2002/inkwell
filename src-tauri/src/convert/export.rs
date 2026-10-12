//! Export a PDF: pages to PNG / JPEG / TIFF, text (plain or layout-preserving), HTML / XHTML (from MuPDF's
//! structured text), the embedded images, Word and RTF.
//!
//! The source is what the document shows now (pending edits included): the engine's snapshot when
//! there is one, else the file on disk. Nothing here writes to the open document.

use std::collections::HashSet;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mupdf::pixmap::ImageFormat as MuFormat;
use mupdf::text_page::{TextBlockType, TextCharFlags, TextPage, TextPageFlags};
use mupdf::{Colorspace, Document, Matrix, Pixmap};
use serde::{Deserialize, Serialize};

use super::{docx, flow, rtf};
use super::{pages_from, safe_stem, unique_stem, Error, Progress, Run, Staging};
use crate::engine::{DocId, DocSource, Engine};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageFormat {
    Png,
    Jpeg,
    Tiff,
}

impl ImageFormat {
    fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
        }
    }
}

fn default_quality() -> u8 {
    90
}

fn default_min_side() -> u32 {
    32
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExportKind {
    /// One image per page, at `dpi`.
    #[serde(rename_all = "camelCase")]
    Images {
        format: ImageFormat,
        dpi: u32,
        #[serde(default)]
        gray: bool,
        /// 1..=100, JPEG only.
        #[serde(default = "default_quality")]
        quality: u8,
    },
    /// One `.txt`; `layout` keeps the columns and indentation of the page.
    #[serde(rename_all = "camelCase")]
    Text {
        #[serde(default)]
        layout: bool,
    },
    /// One `.html` (pages laid out as they look) or `.xhtml` (flowing text).
    #[serde(rename_all = "camelCase")]
    Html {
        #[serde(default)]
        xhtml: bool,
    },
    /// The images drawn on the pages, as PNG files; the same image on several pages is written once.
    #[serde(rename_all = "camelCase")]
    ExtractImages {
        /// Skip images whose shorter side is below this many pixels (icons, rules).
        #[serde(default = "default_min_side")]
        min_side: u32,
    },
    /// One `.docx`.
    Word,
    /// One `.rtf`: the same flowing reconstruction as Word.
    Rtf,
}

impl ExportKind {
    /// Writes several files into a folder (the destination is a folder), else one file.
    pub fn is_folder(&self) -> bool {
        matches!(self, Self::Images { .. } | Self::ExtractImages { .. })
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Self::Images { format, .. } => format.ext(),
            Self::Text { .. } => "txt",
            Self::Html { xhtml: false } => "html",
            Self::Html { xhtml: true } => "xhtml",
            Self::ExtractImages { .. } => "png",
            Self::Word => "docx",
            Self::Rtf => "rtf",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportRequest {
    pub doc: DocId,
    pub kind: ExportKind,
    /// One-based range like `1-3, 5`; none = every page.
    pub pages: Option<String>,
    /// A save token from `pick_save` (a file) or `pick_folder` (a folder), per `kind.is_folder()`.
    pub save_to: String,
}

/// Where the result goes, resolved by the command from the save token.
#[derive(Clone, Debug)]
pub enum Target {
    Folder(PathBuf),
    File(PathBuf),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutput {
    /// Files written (full paths, for display).
    pub files: Vec<String>,
    /// Folder holding them.
    pub folder: String,
    pub bytes: u64,
    /// Pages read.
    pub pages: u32,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------- source

fn open_source(eng: &Engine, doc: DocId) -> Result<(Document, String), Error> {
    // Exports write unprotected files: not from a file whose owner restricted copying or page changes.
    eng.ensure_extractable(doc)?;
    let entry = eng.doc_entry(doc)?;
    let stem = Path::new(&entry.path()).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (_, source) = eng.doc_source(doc)?;
    let mut d = match &source {
        DocSource::Path(p) => Document::open(p.as_str())?,
        DocSource::Bytes(b, n) => Document::from_bytes(&b[..*n], "application/pdf")?,
    };
    if d.needs_password()? {
        match eng.password(doc)? {
            Some(p) if d.authenticate(&p)? => {}
            Some(_) => return Err(Error::BadPassword),
            None => return Err(Error::NeedsPassword),
        }
    }
    Ok((d, safe_stem(&stem)))
}

// ---------------------------------------------------------------- entry

/// Runs an export. `progress` sees `done` only ever grow.
pub fn export(
    eng: &Engine,
    req: &ExportRequest,
    target: Target,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ExportOutput, Error> {
    validate(&req.kind)?;
    let (doc, stem) = open_source(eng, req.doc)?;
    export_into(&doc, &stem, &req.kind, req.pages.as_deref(), target, true, cancel, progress)
}

/// [`export`] that never replaces a file: a name that is taken by the time the files are moved into place fails
/// with `output-exists` and leaves what was there. For callers that chose the names themselves (the assistant).
pub fn export_new(
    eng: &Engine,
    req: &ExportRequest,
    target: Target,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ExportOutput, Error> {
    validate(&req.kind)?;
    let (doc, stem) = open_source(eng, req.doc)?;
    export_into(&doc, &stem, &req.kind, req.pages.as_deref(), target, false, cancel, progress)
}

/// [`export`] over a document the caller opened (the batch runner exports files that no tab has open).
/// `stem` names the files; `pages` is a one-based range like `1-3, 5`.
pub fn export_document(
    doc: &Document,
    stem: &str,
    kind: &ExportKind,
    pages: Option<&str>,
    target: Target,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ExportOutput, Error> {
    export_into(doc, stem, kind, pages, target, true, cancel, progress)
}

/// `replace`: whether a file at a final name may be replaced when the files are moved into place.
#[allow(clippy::too_many_arguments)]
fn export_into(
    doc: &Document,
    stem: &str,
    kind: &ExportKind,
    pages: Option<&str>,
    target: Target,
    replace: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<ExportOutput, Error> {
    validate(kind)?;
    let stem = safe_stem(stem);
    let count = doc.page_count()?.max(0) as u32;
    let pages = pages_from(pages, count)?;
    let mut run = Run { cancel, progress, total: pages.len() as u32 };
    run.check()?;
    run.step(0);

    let dest_dir = match &target {
        Target::Folder(d) => d.clone(),
        Target::File(f) => f.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")),
    };
    let staging = Staging::new(&dest_dir)?;
    let mut warnings = Vec::new();
    // (staged name, name suffix after the stem)
    let staged: Vec<String> = match kind {
        ExportKind::Images { format, dpi, gray, quality } => {
            images(doc, &pages, count, &stem, *format, *dpi, *gray, *quality, &staging, &mut run)?
        }
        ExportKind::ExtractImages { min_side } => extract_images(doc, &pages, count, &stem, *min_side, &staging, &mut run, &mut warnings)?,
        ExportKind::Text { layout } => single(&staging, &mut run, |out, run| text(doc, &pages, *layout, out, run))?,
        ExportKind::Html { xhtml } => single(&staging, &mut run, |out, run| html(doc, &pages, *xhtml, &stem, out, run))?,
        ExportKind::Word => single(&staging, &mut run, |out, run| word(doc, &pages, &stem, out, run))?,
        ExportKind::Rtf => single(&staging, &mut run, |out, run| rich_text(doc, &pages, &stem, out, run, &mut warnings))?,
    };
    run.check()?;

    // Move into place. A folder export picks a name that does not clash with files already there.
    let (finals, folder): (Vec<(String, PathBuf)>, PathBuf) = match target {
        Target::File(path) => {
            let name = staged.first().cloned().ok_or_else(|| Error::Invalid("nothing was exported".into()))?;
            (vec![(name, path.clone())], dest_dir)
        }
        Target::Folder(dir) => {
            let suffixes: Vec<String> = staged.iter().map(|n| n[stem.len()..].to_string()).collect();
            let unique = unique_stem(&dir, &stem, &suffixes);
            let files = staged.iter().zip(&suffixes).map(|(n, suf)| (n.clone(), dir.join(format!("{unique}{suf}")))).collect();
            (files, dir)
        }
    };
    if replace { staging.publish(&finals)? } else { staging.publish_new(&finals)? }
    let bytes = finals.iter().filter_map(|(_, p)| fs::metadata(p).ok()).map(|m| m.len()).sum();
    run.step(run.total);
    (run.progress)(Progress { done: run.total, total: run.total, fraction: 1.0 });
    Ok(ExportOutput {
        files: finals.iter().map(|(_, p)| p.to_string_lossy().into_owned()).collect(),
        folder: folder.to_string_lossy().into_owned(),
        bytes,
        pages: pages.len() as u32,
        warnings,
    })
}

fn validate(kind: &ExportKind) -> Result<(), Error> {
    match kind {
        ExportKind::Images { dpi, quality, .. } if !(36..=1200).contains(dpi) || !(1..=100).contains(quality) => {
            Err(Error::Invalid("resolution must be 36 to 1200 dpi and JPEG quality 1 to 100".into()))
        }
        _ => Ok(()),
    }
}

/// A one-file export: the closure writes the staged file; returns its (staged) name.
fn single(
    staging: &Staging,
    run: &mut Run<'_>,
    write: impl FnOnce(BufWriter<fs::File>, &mut Run<'_>) -> Result<(), Error>,
) -> Result<Vec<String>, Error> {
    let name = "export.tmp".to_string();
    let f = fs::File::create(staging.path(&name))?;
    write(BufWriter::new(f), run)?;
    Ok(vec![name])
}

// ---------------------------------------------------------------- pixels

/// 8-bit samples without padding, 1 (grey) or 3 (RGB) channels, composited over white.
struct Raster {
    w: u32,
    h: u32,
    channels: u8,
    data: Vec<u8>,
}

fn raster(p: &Pixmap) -> Result<Raster, Error> {
    let (w, h, n, stride) = (p.width() as usize, p.height() as usize, p.n() as usize, p.stride() as usize);
    let alpha = p.alpha();
    let cs = n - alpha as usize;
    let s = p.samples();
    let (channels, mut data) = match cs {
        1 => (1u8, Vec::with_capacity(w * h)),
        3 | 4 => (3u8, Vec::with_capacity(w * h * 3)),
        other => return Err(Error::Invalid(format!("unsupported colour layout with {other} components"))),
    };
    for y in 0..h {
        let row = &s[y * stride..y * stride + w * n];
        for px in row.chunks_exact(n) {
            // pixmaps are premultiplied: over white that is colour + (255 - alpha)
            let white = if alpha { 255 - px[n - 1] } else { 0 };
            let lift = |c: u8| c.saturating_add(white);
            match cs {
                1 => data.push(lift(px[0])),
                3 => data.extend([lift(px[0]), lift(px[1]), lift(px[2])]),
                _ => {
                    // CMYK, the way MuPDF's own naive conversion goes
                    let k = 255 - px[3] as u32;
                    data.extend((0..3).map(|i| lift((((255 - px[i] as u32) * k) / 255) as u8)));
                }
            }
        }
    }
    Ok(Raster { w: w as u32, h: h as u32, channels, data })
}

fn write_raster(r: &Raster, format: ImageFormat, dpi: u32, quality: u8, out: &Path) -> Result<(), Error> {
    use image::{ExtendedColorType, ImageEncoder};
    let file = BufWriter::new(fs::File::create(out)?);
    let color = if r.channels == 1 { ExtendedColorType::L8 } else { ExtendedColorType::Rgb8 };
    let bad = |e: &dyn std::fmt::Display| Error::Io(std::io::Error::other(e.to_string()));
    match format {
        ImageFormat::Png => image::codecs::png::PngEncoder::new(file).write_image(&r.data, r.w, r.h, color).map_err(|e| bad(&e)),
        ImageFormat::Jpeg => {
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
            enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(dpi.min(u16::MAX as u32) as u16));
            enc.write_image(&r.data, r.w, r.h, color).map_err(|e| bad(&e))
        }
        ImageFormat::Tiff => {
            use tiff::encoder::{colortype, Compression, Rational, TiffEncoder};
            use tiff::tags::ResolutionUnit;
            let mut file = file;
            let mut enc = TiffEncoder::new(&mut file).map_err(|e| bad(&e))?.with_compression(Compression::Lzw);
            let res = Rational { n: dpi, d: 1 };
            macro_rules! put {
                ($ct:ty) => {{
                    let mut img = enc.new_image::<$ct>(r.w, r.h).map_err(|e| bad(&e))?;
                    img.resolution(ResolutionUnit::Inch, res);
                    img.write_data(&r.data).map_err(|e| bad(&e))
                }};
            }
            if r.channels == 1 { put!(colortype::Gray8) } else { put!(colortype::RGB8) }?;
            file.flush()?;
            Ok(())
        }
    }
}

fn page_width(count: u32) -> usize {
    count.max(1).to_string().len()
}

#[allow(clippy::too_many_arguments)]
fn images(
    doc: &Document,
    pages: &[u32],
    count: u32,
    stem: &str,
    format: ImageFormat,
    dpi: u32,
    gray: bool,
    quality: u8,
    staging: &Staging,
    run: &mut Run<'_>,
) -> Result<Vec<String>, Error> {
    let scale = dpi as f32 / 72.0;
    let ctm = Matrix::new_scale(scale, scale);
    let cs = if gray { Colorspace::device_gray() } else { Colorspace::device_rgb() };
    let width = page_width(count);
    let mut names = Vec::new();
    for (i, &p) in pages.iter().enumerate() {
        run.check()?;
        let page = doc.load_page(p as i32)?;
        let b = page.bounds()?;
        // 20 000 px a side is already past what image viewers and Word cope with.
        let (pw, ph) = (b.width() * scale, b.height() * scale);
        if pw > 20_000.0 || ph > 20_000.0 {
            return Err(Error::Invalid(format!("page {} would be {}x{} px at {dpi} dpi: use a lower resolution", p + 1, pw as u32, ph as u32)));
        }
        let mut pix = page.to_pixmap(&ctm, &cs, false, true)?;
        let name = format!("{stem}-{:0width$}.{}", p + 1, format.ext());
        let out = staging.path(&name);
        if format == ImageFormat::Png {
            // MuPDF's own writer: it is the fast one and records the resolution
            pix.set_resolution(dpi as i32, dpi as i32);
            pix.save_as(&out.to_string_lossy(), MuFormat::PNG)?;
        } else {
            write_raster(&raster(&pix)?, format, dpi, quality, &out)?;
        }
        names.push(name);
        run.step(i as u32 + 1);
    }
    Ok(names)
}

#[allow(clippy::too_many_arguments)]
fn extract_images(
    doc: &Document,
    pages: &[u32],
    count: u32,
    stem: &str,
    min_side: u32,
    staging: &Staging,
    run: &mut Run<'_>,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>, Error> {
    let width = page_width(count);
    let mut seen: HashSet<[u8; 16]> = HashSet::new();
    let mut names = Vec::new();
    let mut failed = 0u32;
    for (i, &p) in pages.iter().enumerate() {
        run.check()?;
        let tp = doc.load_page(p as i32)?.to_text_page(TextPageFlags::PRESERVE_IMAGES)?;
        let mut k = 0;
        for block in tp.blocks() {
            if block.r#type() != TextBlockType::Image {
                continue;
            }
            let Some(img) = block.image() else { continue };
            if img.width().min(img.height()) < min_side {
                continue;
            }
            let Ok(pix) = img.to_pixmap() else {
                failed += 1;
                continue;
            };
            if let Ok(d) = pix.digest() {
                if !seen.insert(*d.as_bytes()) {
                    continue;
                }
            }
            let Ok(r) = raster(&pix) else {
                failed += 1;
                continue;
            };
            k += 1;
            let name = format!("{stem}-p{:0width$}-{k}.png", p + 1);
            write_raster(&r, ImageFormat::Png, 0, 0, &staging.path(&name))?;
            names.push(name);
        }
        run.step(i as u32 + 1);
    }
    if names.is_empty() {
        return Err(Error::Invalid(if failed > 0 { "the images in this document could not be decoded".into() } else { "there are no images in the selected pages".into() }));
    }
    if failed > 0 {
        warnings.push(format!("{failed} image{} could not be decoded and {} skipped.", if failed == 1 { "" } else { "s" }, if failed == 1 { "was" } else { "were" }));
    }
    Ok(names)
}

// ---------------------------------------------------------------- text

fn text(doc: &Document, pages: &[u32], layout: bool, mut out: impl Write, run: &mut Run<'_>) -> Result<(), Error> {
    for (i, &p) in pages.iter().enumerate() {
        run.check()?;
        let tp = doc.load_page(p as i32)?.to_text_page(TextPageFlags::empty())?;
        let body = if layout { layout_text(&tp) } else { tp.to_text()? };
        if i > 0 {
            // a form feed ends a page, as pdftotext writes it
            out.write_all(b"\x0c\n")?;
        }
        out.write_all(body.trim_end().as_bytes())?;
        out.write_all(b"\n")?;
        run.step(i as u32 + 1);
    }
    out.flush()?;
    Ok(())
}

/// Lines laid out on a character grid: rows by baseline, columns by left edge over the page's typical
/// character width, so columns, indents and tables keep their shape in a monospaced viewer.
fn layout_text(tp: &TextPage) -> String {
    struct Ln {
        x0: f32,
        y0: f32,
        y1: f32,
        text: String,
        cw: f32,
    }
    let mut lines: Vec<Ln> = Vec::new();
    for block in tp.blocks() {
        if block.r#type() != TextBlockType::Text {
            continue;
        }
        for line in block.lines() {
            let text: String = line.chars().filter_map(|c| c.char()).collect();
            if text.trim().is_empty() {
                continue;
            }
            let b = line.bounds();
            let n = text.chars().count() as f32;
            lines.push(Ln { x0: b.x0, y0: b.y0, y1: b.y1, text, cw: (b.x1 - b.x0) / n });
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let median = |mut v: Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v[v.len() / 2]
    };
    // The grid is the typical advance of a long line (short ones are dominated by side bearings).
    let long: Vec<f32> = lines.iter().filter(|l| l.text.chars().count() >= 8).map(|l| l.cw).collect();
    let cw = median(if long.is_empty() { lines.iter().map(|l| l.cw).collect() } else { long }).max(2.0);
    let lh = median(lines.iter().map(|l| l.y1 - l.y0).collect()).max(4.0);
    let left = lines.iter().map(|l| l.x0).fold(f32::MAX, f32::min);

    lines.sort_by(|a, b| ((a.y0 + a.y1) / 2.0).total_cmp(&((b.y0 + b.y1) / 2.0)).then(a.x0.total_cmp(&b.x0)));
    // rows: a line joins the current row when its centre is within half a line height of the row's
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut row_c = f32::MIN;
    for (i, l) in lines.iter().enumerate() {
        let c = (l.y0 + l.y1) / 2.0;
        if rows.is_empty() || c - row_c > lh * 0.5 {
            rows.push(Vec::new());
            row_c = c;
        }
        rows.last_mut().unwrap().push(i);
    }

    let mut out = String::new();
    let mut prev_bottom = None::<f32>;
    for row in &mut rows {
        row.sort_by(|&a, &b| lines[a].x0.total_cmp(&lines[b].x0));
        let top = row.iter().map(|&i| lines[i].y0).fold(f32::MAX, f32::min);
        if let Some(pb) = prev_bottom {
            // one blank line per extra line height of gap, at most two
            let blanks = (((top - pb) / lh).floor().max(0.0) as usize).min(2);
            out.push_str(&"\n".repeat(blanks));
        }
        let mut col = 0usize;
        let mut line = String::new();
        for &i in row.iter() {
            let l = &lines[i];
            let at = ((l.x0 - left) / cw).round().max(0.0) as usize;
            let pad = if line.is_empty() { at } else { at.saturating_sub(col).max(1) };
            line.push_str(&" ".repeat(pad));
            line.push_str(&l.text);
            col = at.max(col + pad) + l.text.chars().count();
            col = col.max(at + l.text.chars().count());
        }
        out.push_str(line.trim_end());
        out.push('\n');
        prev_bottom = Some(row.iter().map(|&i| lines[i].y1).fold(f32::MIN, f32::max));
    }
    out
}

// ---------------------------------------------------------------- HTML

fn html(doc: &Document, pages: &[u32], xhtml: bool, title: &str, mut out: impl Write, run: &mut Run<'_>) -> Result<(), Error> {
    let flags = TextPageFlags::PRESERVE_IMAGES | TextPageFlags::PRESERVE_WHITESPACE;
    let (mut head, mut tail) = (String::new(), String::new());
    for (i, &p) in pages.iter().enumerate() {
        run.check()?;
        let tp = doc.load_page(p as i32)?.to_text_page(flags)?;
        // MuPDF writes a complete document per call; the first one's head and tail frame the rest.
        let whole = if xhtml { tp.to_xhtml(p as i32)? } else { tp.to_html(p as i32, true)? };
        let (h, body, t) = split_body(&whole);
        if i == 0 {
            head = with_title(h, title);
            tail = t.to_string();
            out.write_all(head.as_bytes())?;
        }
        out.write_all(body.as_bytes())?;
        run.step(i as u32 + 1);
    }
    out.write_all(tail.as_bytes())?;
    out.flush()?;
    let _ = head;
    Ok(())
}

/// `(everything up to and including <body ...>, the inside, </body> to the end)`.
fn split_body(doc: &str) -> (&str, &str, &str) {
    let open = doc.find("<body").and_then(|i| doc[i..].find('>').map(|j| i + j + 1)).unwrap_or(0);
    let close = doc.rfind("</body>").unwrap_or(doc.len());
    (&doc[..open], &doc[open..close.max(open)], &doc[close.max(open)..])
}

fn with_title(head: &str, title: &str) -> String {
    let esc = title.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    match head.find("</head>") {
        Some(i) if !head.contains("<title") => format!("{}<title>{esc}</title>\n{}", &head[..i], &head[i..]),
        _ => head.to_string(),
    }
}

// ---------------------------------------------------------------- Word

/// The pages as the flowing model both document writers take (paragraphs, tables, PNG pictures).
fn flow_model(doc: &Document, pages: &[u32], run: &mut Run<'_>) -> Result<Vec<flow::Page>, Error> {
    use image::{ExtendedColorType, ImageEncoder};
    let mut model: Vec<flow::Page> = Vec::new();
    for (i, &p) in pages.iter().enumerate() {
        run.check()?;
        let page = doc.load_page(p as i32)?;
        let h = page.bounds()?.height() as f64;
        let w = page.bounds()?.width() as f64;
        let tp = page.to_text_page(TextPageFlags::PRESERVE_IMAGES)?;
        let mut out_page = flow::Page { width: w, height: h, blocks: Vec::new(), images: Vec::new() };
        // y runs up in the model
        let flip = |b: mupdf::Rect| [b.x0 as f64, h - b.y1 as f64, b.x1 as f64, h - b.y0 as f64];
        for block in tp.blocks() {
            match block.r#type() {
                TextBlockType::Text => {
                    let mut text = String::new();
                    let (mut size, mut argb) = (0.0f32, 0u32);
                    let (mut bold, mut italic, mut chars) = (0usize, 0usize, 0usize);
                    for line in block.lines() {
                        if !text.is_empty() && !text.ends_with(' ') {
                            text.push(' ');
                        }
                        for c in line.chars() {
                            let Some(ch) = c.char() else { continue };
                            if chars == 0 {
                                size = c.size();
                                argb = c.argb();
                            }
                            chars += 1;
                            text.push(ch);
                            if !ch.is_whitespace() {
                                bold += (c.flags().contains(TextCharFlags::BOLD) || c.font().is_some_and(|f| f.is_bold())) as usize;
                                italic += c.font().is_some_and(|f| f.is_italic()) as usize;
                            }
                        }
                    }
                    if text.trim().is_empty() {
                        continue;
                    }
                    let c = |shift: u32| ((argb >> shift) & 0xff) as f64 / 255.0;
                    out_page.blocks.push(flow::Block {
                        text: text.trim().to_string(),
                        rect: flip(block.bounds()),
                        size: size as f64,
                        bold: bold * 2 > chars,
                        italic: italic * 2 > chars,
                        color: [c(16), c(8), c(0)],
                    });
                }
                TextBlockType::Image => {
                    let Some(pix) = block.image().and_then(|im| im.to_pixmap().ok()) else { continue };
                    let Ok(r) = raster(&pix) else { continue };
                    let mut png = Vec::new();
                    let color = if r.channels == 1 { ExtendedColorType::L8 } else { ExtendedColorType::Rgb8 };
                    if image::codecs::png::PngEncoder::new(&mut png).write_image(&r.data, r.w, r.h, color).is_ok() {
                        out_page.images.push(flow::Image { ext: "png", bytes: png, rect: flip(block.bounds()) });
                    }
                }
                _ => {}
            }
        }
        model.push(out_page);
        run.step(i as u32 + 1);
    }
    Ok(model)
}

fn word(doc: &Document, pages: &[u32], title: &str, mut out: impl Write, run: &mut Run<'_>) -> Result<(), Error> {
    out.write_all(&docx::docx(&flow_model(doc, pages, run)?, title))?;
    out.flush()?;
    Ok(())
}

// ---------------------------------------------------------------- RTF

fn rich_text(doc: &Document, pages: &[u32], title: &str, mut out: impl Write, run: &mut Run<'_>, warnings: &mut Vec<String>) -> Result<(), Error> {
    let r = rtf::rtf(&flow_model(doc, pages, run)?, title);
    if r.skipped_images > 0 {
        warnings.push(format!(
            "{} picture{} left out (the file holds at most {} MB of pictures)",
            r.skipped_images,
            if r.skipped_images == 1 { "" } else { "s" },
            rtf::MAX_IMAGE_BYTES >> 20
        ));
    }
    out.write_all(r.text.as_bytes())?;
    out.flush()?;
    Ok(())
}
