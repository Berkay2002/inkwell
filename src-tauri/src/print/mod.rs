//! Printing: the sheets a document prints as, a live preview of any of them, and the job that sends
//! them to a Windows printer.
//!
//! One plan feeds both: `plan` turns the settings into sheets (layout.rs), `render::render` draws a
//! sheet at any resolution. The preview draws at screen size, the printer at its own resolution, so
//! the dialog shows what comes out. A job never touches the open document's state: it parses a private
//! copy of what the document shows now (unsaved edits included), so printing is not an edit and has no
//! undo step or history entry.
//!
//! Tests print to "Microsoft Print to PDF" with an output file and read the result back (never a real
//! printer). See docs/print.md.

pub mod app;
pub mod layout;
pub mod render;
#[cfg(windows)]
pub mod win;
/// Non-Windows builds (engine tests on macOS/Linux): no printers.
#[cfg(not(windows))]
pub mod win {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Printer {
        pub name: String,
        pub is_default: bool,
        pub to_file: bool,
    }
    #[derive(serde::Serialize)]
    pub struct Capabilities;
    pub fn printers() -> Vec<Printer> {
        Vec::new()
    }
    pub fn capabilities(_printer: &str) -> Option<Capabilities> {
        None
    }
}
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

pub use layout::{Layout, Orientation, Range, Sheet, Subset};
pub use render::Content;

use crate::engine::{DocId, Engine};
use layout::LayoutError;
use render::{RenderError, Source};

/// What the UI's promise rejects with: a code it matches on (`cancelled`, `needs-password`,
/// `bad-password`, `no-printer`, `not-permitted`, `stale`) or a message to show.
#[derive(Debug, thiserror::Error)]
pub enum PrintError {
    #[error("cancelled")]
    Cancelled,
    #[error("no-printer")]
    NoPrinter,
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Printer(String),
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error(transparent)]
    Engine(#[from] crate::engine::EngineError),
}

impl From<LayoutError> for PrintError {
    fn from(e: LayoutError) -> Self {
        PrintError::Invalid(e.to_string())
    }
}

impl From<PrintError> for crate::tools::ToolError {
    fn from(e: PrintError) -> Self {
        use crate::engine::EngineError as E;
        crate::tools::ToolError(match &e {
            PrintError::Render(RenderError::NeedsPassword) => "needs-password".into(),
            PrintError::Render(RenderError::BadPassword) => "bad-password".into(),
            PrintError::Render(RenderError::NotPermitted) => "not-permitted".into(),
            PrintError::Engine(E::Stale) | PrintError::Render(RenderError::Engine(E::Stale)) => "stale".into(),
            other => other.to_string(),
        })
    }
}

#[cfg(windows)]
impl From<win::WinError> for PrintError {
    fn from(e: win::WinError) -> Self {
        match e {
            win::WinError::Cancelled => PrintError::Cancelled,
            win::WinError::Printer(m) => PrintError::Printer(m),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Duplex {
    #[default]
    Off,
    LongEdge,
    ShortEdge,
}

/// The paper the sheets are laid out on, in points (portrait). `id` is the driver's DMPAPER number
/// from `print_capabilities`; none means the driver's default paper (preview only).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Paper {
    pub id: Option<u16>,
    pub width: f64,
    pub height: f64,
}

impl Default for Paper {
    fn default() -> Self {
        // A4
        Paper { id: None, width: 595.28, height: 841.89 }
    }
}

/// Everything the Print sheet sets. The same value drives the preview and the job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// None: the system default printer.
    #[serde(default)]
    pub printer: Option<String>,
    #[serde(default)]
    pub paper: Paper,
    /// Input tray, the driver's bin number.
    #[serde(default)]
    pub tray: Option<u16>,
    #[serde(default)]
    pub orientation: Orientation,
    #[serde(default = "all")]
    pub range: Range,
    #[serde(default)]
    pub subset: Subset,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default = "one")]
    pub copies: u32,
    #[serde(default = "yes")]
    pub collate: bool,
    #[serde(default)]
    pub duplex: Duplex,
    #[serde(default)]
    pub grayscale: bool,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub content: Content,
}

fn all() -> Range {
    Range::All
}
fn one() -> u32 {
    1
}
fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            printer: None,
            paper: Paper::default(),
            tray: None,
            orientation: Orientation::Auto,
            range: Range::All,
            subset: Subset::All,
            reverse: false,
            copies: 1,
            collate: true,
            duplex: Duplex::Off,
            grayscale: false,
            layout: Layout::default(),
            content: Content::default(),
        }
    }
}

impl Settings {
    /// Is the job printed on both sides? A booklet's back sides make no sense without it, but only the
    /// user can say the printer does it, so Booklet follows the Duplex setting like everything else.
    /// A booklet of only the fronts or only the backs is run through the printer a side at a time.
    fn two_sided(&self) -> bool {
        self.duplex != Duplex::Off && !self.booklet_one_side()
    }

    fn booklet_one_side(&self) -> bool {
        matches!(self.layout, Layout::Booklet { sides, .. } if sides != layout::BookletSides::Both)
    }

    /// The edge two-sided sheets flip on: a booklet folds on the short edge whatever was asked.
    #[cfg(windows)]
    fn edge(&self) -> win::Edge {
        match (self.duplex, &self.layout) {
            _ if !self.two_sided() => win::Edge::None,
            (Duplex::Off, _) => win::Edge::None,
            (_, Layout::Booklet { .. }) => win::Edge::Short,
            (Duplex::LongEdge, _) => win::Edge::Long,
            (Duplex::ShortEdge, _) => win::Edge::Short,
        }
    }
}

/// The sheets a document prints as under some settings.
#[derive(Debug, Clone)]
pub struct Plan {
    pub sheets: Vec<Sheet>,
    /// The chosen source pages in print order (0-based).
    pub selected: Vec<u32>,
    /// The edit generation the sizes came from.
    pub rev: u32,
}

/// The plan for document `doc` under `s`, from the page sizes of its current generation.
pub fn plan(eng: &Engine, doc: DocId, s: &Settings) -> Result<Plan, PrintError> {
    let entry = eng.doc_entry(doc)?;
    // sizes and generation are read apart; an edit between the two just means "plan again"
    let (rev, sizes) = loop {
        let rev = entry.generation();
        let sizes = entry.sizes();
        if entry.generation() == rev {
            break (rev, sizes);
        }
    };
    let sizes: Vec<(f64, f64)> = sizes.iter().map(|s| (s[0] as f64, s[1] as f64)).collect();
    let selected = layout::select_pages(sizes.len() as u32, &s.range, s.subset, s.reverse)?;
    // Both sides of a paper share one orientation, so two-sided Auto follows the first page.
    let orientation = match s.orientation {
        Orientation::Auto if s.two_sided() => {
            let d = sizes[selected[0] as usize];
            if d.0 > d.1 { Orientation::Landscape } else { Orientation::Portrait }
        }
        o => o,
    };
    let sheets = layout::layout(&sizes, &selected, (s.paper.width, s.paper.height), orientation, &s.layout)?;
    Ok(Plan { sheets, selected, rev })
}

/// What the dialog shows about the plan.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanInfo {
    /// Pages chosen.
    pub pages: u32,
    /// Sides of paper per copy (one per sheet of the plan).
    pub sheets: u32,
    /// Physical sheets of paper for all copies.
    pub papers: u32,
    /// Sides sent to the printer for all copies (blank backs included): the total a job's progress counts to.
    pub sides: u32,
    /// For each side: its size in points and the source pages on it (1-based), in the order drawn.
    pub faces: Vec<Face>,
    pub edit_rev: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Face {
    pub width: f64,
    pub height: f64,
    pub pages: Vec<u32>,
}

pub fn plan_info(eng: &Engine, doc: DocId, s: &Settings) -> Result<PlanInfo, PrintError> {
    let p = plan(eng, doc, s)?;
    let faces: Vec<Face> =
        p.sheets.iter().map(|f| Face { width: f.w, height: f.h, pages: f.placed.iter().map(|x| x.page + 1).collect() }).collect();
    let n = faces.len();
    let sent = layout::sequence(n, s.copies.clamp(1, 999), s.collate, s.two_sided()).len();
    Ok(PlanInfo {
        pages: p.selected.len() as u32,
        sheets: n as u32,
        papers: if s.two_sided() { sent.div_ceil(2) } else { sent } as u32,
        sides: sent as u32,
        faces,
        edit_rev: p.rev,
    })
}

/// A PNG of side `sheet` (0-based) whose longer edge is `max_px` pixels, made by the same code the
/// printer path uses.
pub fn preview_png(eng: &Engine, doc: DocId, s: &Settings, sheet: u32, max_px: u32) -> Result<Vec<u8>, PrintError> {
    let p = plan(eng, doc, s)?;
    let side = p.sheets.get(sheet as usize).ok_or_else(|| PrintError::Invalid(format!("no sheet {}", sheet + 1)))?;
    let (mut src, rev) = Source::open(eng, doc, s.content)?;
    if rev != p.rev {
        return Err(PrintError::Engine(crate::engine::EngineError::Stale));
    }
    let scale = max_px.clamp(64, 4000) as f64 / side.w.max(side.h);
    let (w, h) = render::sheet_px(side, scale);
    let band = mupdf::IRect { x0: 0, y0: 0, x1: w, y1: h };
    let pix = render::render(&mut src, side, scale, band, s.grayscale).map_err(PrintError::from)?;
    let mut out = Vec::new();
    pix.write_to(&mut out, mupdf::ImageFormat::PNG).map_err(RenderError::from)?;
    Ok(out)
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Sides sent so far, of `total` (copies counted).
    pub done: u32,
    pub total: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub printer: String,
    pub pages: u32,
    /// Sides sent, copies counted (blank backs included).
    pub sides: u32,
}

/// Prints `doc`. `output` sends the result to a file instead of the printer's port (for print-to-file
/// drivers such as Microsoft Print to PDF; the spooler would otherwise ask where). `progress` sees
/// `done` only grow.
#[cfg(windows)]
pub fn run(
    eng: &Engine,
    doc: DocId,
    s: &Settings,
    title: &str,
    output: Option<PathBuf>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Report, PrintError> {
    let printer = match &s.printer {
        Some(p) => p.clone(),
        None => win::default_printer().ok_or(PrintError::NoPrinter)?,
    };
    let (copies, two_sided) = (s.copies.clamp(1, 999), s.two_sided());
    let check = || if cancel.load(Ordering::Relaxed) { Err(PrintError::Cancelled) } else { Ok(()) };
    check()?;
    let mut job = win::Job::start(win::JobOptions {
        printer: printer.clone(),
        title: title.to_string(),
        output,
        paper: s.paper.id,
        tray: s.tray,
        duplex: s.edge(),
        gray: s.grayscale,
    })?;
    // With no paper chosen the layout used a default (A4); a printer loaded with something else is
    // laid out on what it has, so nothing is cut off or left off-centre.
    let settings = match device_paper(&s.paper, job.paper_points()) {
        Some(paper) => Settings { paper, ..s.clone() },
        None => s.clone(),
    };
    let s = &settings;
    let p = match plan(eng, doc, s) {
        Ok(p) => p,
        Err(e) => {
            job.abort();
            return Err(e);
        }
    };
    let (mut src, rev) = match Source::open(eng, doc, s.content) {
        Ok(x) => x,
        Err(e) => {
            job.abort();
            return Err(e.into());
        }
    };
    if rev != p.rev {
        job.abort();
        return Err(PrintError::Engine(crate::engine::EngineError::Stale));
    }
    let order = layout::sequence(p.sheets.len(), copies, s.collate, two_sided);
    let total = order.len() as u32;
    progress(Progress { done: 0, total });
    let result = (|| -> Result<(), PrintError> {
        let mut landscape = p.sheets.first().is_some_and(|f| f.landscape());
        for (n, item) in order.iter().enumerate() {
            check()?;
            if let Some(k) = item {
                landscape = p.sheets[*k].landscape();
                src.retain(&p.sheets[*k].placed.iter().map(|x| x.page).collect::<Vec<_>>());
            }
            job.begin_sheet(landscape)?;
            if let Some(k) = item {
                send_sheet(&mut job, &mut src, &p.sheets[*k], s.grayscale, cancel)?;
            }
            job.end_sheet()?;
            progress(Progress { done: n as u32 + 1, total });
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            job.finish()?;
            Ok(Report { printer, pages: p.selected.len() as u32, sides: total })
        }
        Err(e) => {
            job.abort();
            Err(e)
        }
    }
}

/// The paper to lay out on instead of `asked`, when none was chosen (`id` is none) and the device
/// context's own paper (in points, either orientation) differs from it by more than a point.
#[cfg(windows)]
fn device_paper(asked: &Paper, device: (f64, f64)) -> Option<Paper> {
    if asked.id.is_some() || !(device.0 >= 72.0 && device.1 >= 72.0) {
        return None;
    }
    let (aw, ah) = (asked.width.min(asked.height), asked.width.max(asked.height));
    let (dw, dh) = (device.0.min(device.1), device.0.max(device.1));
    if (aw - dw).abs() > 1.0 || (ah - dh).abs() > 1.0 {
        Some(Paper { id: None, width: dw, height: dh })
    } else {
        None
    }
}

/// Bytes of one band's bitmap the job keeps in memory at a time.
#[cfg(windows)]
const BAND_BYTES: usize = 6 << 20;

#[cfg(windows)]
fn send_sheet(job: &mut win::Job, src: &mut Source, sheet: &Sheet, gray: bool, cancel: &AtomicBool) -> Result<(), PrintError> {
    let g = job.geometry;
    let dpi = g.dpi_x.clamp(72, win::MAX_DPI);
    let scale = dpi as f64 / 72.0;
    let (w, h) = render::sheet_px(sheet, scale);
    // device pixels per rendered pixel, each way
    let (kx, ky) = (g.dpi_x as f64 / dpi as f64, g.dpi_y as f64 / (scale * 72.0));
    let rows = ((BAND_BYTES / (w.max(1) as usize * 3)).max(32) as i32).min(h);
    let stride = (w as usize * 3 + 3) & !3;
    let mut bgr = vec![0u8; stride * rows as usize];
    let mut y = 0;
    while y < h {
        if cancel.load(Ordering::Relaxed) {
            return Err(PrintError::Cancelled);
        }
        let y1 = (y + rows).min(h);
        if render::band_has_ink(sheet, scale, y, y1) {
            let band = mupdf::IRect { x0: 0, y0: y, x1: w, y1 };
            let pix = render::render(src, sheet, scale, band, gray)?;
            let (n, ps) = (pix.n() as usize, pix.stride() as usize);
            let samples = pix.samples();
            for r in 0..(y1 - y) as usize {
                let row = &samples[r * ps..r * ps + w as usize * n];
                let out = &mut bgr[r * stride..r * stride + w as usize * 3];
                if n >= 3 {
                    for (o, px) in out.chunks_exact_mut(3).zip(row.chunks_exact(n)) {
                        o.copy_from_slice(&[px[2], px[1], px[0]]);
                    }
                } else {
                    for (o, px) in out.chunks_exact_mut(3).zip(row.chunks_exact(n)) {
                        o.copy_from_slice(&[px[0]; 3]);
                    }
                }
            }
            let dest_y0 = (y as f64 * ky).round() as i32;
            let dest_y1 = (y1 as f64 * ky).round() as i32;
            let dest = (0, dest_y0, (w as f64 * kx).round() as i32, (dest_y1 - dest_y0).max(1));
            job.draw_band(&bgr[..stride * (y1 - y) as usize], w, y1 - y, dest)?;
        }
        y = y1;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn run(
    _eng: &Engine,
    _doc: DocId,
    _s: &Settings,
    _title: &str,
    _output: Option<PathBuf>,
    _cancel: &AtomicBool,
    _progress: &mut dyn FnMut(Progress),
) -> Result<Report, PrintError> {
    Err(PrintError::Printer("printing is only available on Windows".into()))
}
