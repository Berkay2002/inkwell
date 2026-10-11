//! What goes on each sheet of paper: pure geometry, no MuPDF and no printer, so the preview, the
//! printer and the tests all read the same plan.
//!
//! Adapted from pdfcraft (MIT OR Apache-2.0), crates/print/src/lib.rs and range.rs @ b8ddf71.
//! Differences: sheet space is y-down with its origin at the top left (the GDI and MuPDF device
//! convention) and a placement carries its own scale/rotation instead of a PDF matrix. The sheets
//! are drawn by `render` or written as a PDF by `pdf`, from the same plan. Poster cut marks sit on
//! the edges of the overlap strip (pdfcraft puts them at the printable corners), and booklets get
//! creep, which pdfcraft does not have.
//!
//! Orders and counts follow Acrobat's Print dialog (docs/study/12-print.md): the pages are chosen
//! (range, odd or even among them, reversed), then laid on sheets by Size, Multiple, Booklet or
//! Poster.

use serde::{Deserialize, Serialize};

/// The unprintable margin assumed around a sheet for Fit, Multiple and Booklet (0.25 in; printers
/// vary between 0.17 and 0.25 in, and the same number in the preview keeps it honest).
pub const MARGIN: f64 = 18.0;
/// Gap between cells of a Multiple sheet.
const GAP: f64 = 6.0;
const MAX_CELLS: usize = 100;
/// Sheets one poster job may need, over all its pages.
pub const MAX_POSTER_SHEETS: usize = 1024;
/// 50 mm: the largest overlap between poster tiles.
pub const MAX_OVERLAP: f64 = 50.0 / 25.4 * 72.0;
/// 3 mm: the largest booklet creep.
pub const MAX_CREEP: f64 = 3.0 / 25.4 * 72.0;
/// Size of a poster label's text, in points.
pub const LABEL_PT: f64 = 7.0;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LayoutError {
    #[error("there are no pages to print")]
    NoPages,
    #[error("{0}")]
    Invalid(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Orientation {
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Subset {
    #[default]
    All,
    Odd,
    Even,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SizeMode {
    #[default]
    Fit,
    Actual,
    Shrink,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PageOrder {
    #[default]
    Horizontal,
    HorizontalReversed,
    Vertical,
    VerticalReversed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BookletSides {
    #[default]
    Both,
    FrontOnly,
    BackOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Binding {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Layout {
    /// One page per sheet. `percent` only counts for `Custom`.
    Size { mode: SizeMode, #[serde(default = "hundred")] percent: f64 },
    /// `per_sheet` pages in a grid (2, 4, 6, 9, 16 or any count up to 100).
    Multiple {
        per_sheet: u32,
        #[serde(default)]
        order: PageOrder,
        #[serde(default)]
        border: bool,
        #[serde(default = "yes")]
        auto_rotate: bool,
    },
    /// Saddle-stitched spreads, two pages a side, landscape.
    Booklet {
        #[serde(default)]
        sides: BookletSides,
        #[serde(default)]
        binding: Binding,
        /// Points the pages of the innermost sheet sit closer to the spine than those of the
        /// outermost, growing evenly between (paper thickness).
        #[serde(default)]
        creep: f64,
    },
    /// Each page enlarged by `scale` percent and cut into sheet-sized tiles that overlap by
    /// `overlap` points, in reading order.
    Poster {
        #[serde(default = "hundred")]
        scale: f64,
        #[serde(default = "default_overlap")]
        overlap: f64,
        #[serde(default)]
        cut_marks: bool,
        #[serde(default)]
        labels: bool,
    },
}

fn hundred() -> f64 {
    100.0
}
fn yes() -> bool {
    true
}
/// 6 mm.
fn default_overlap() -> f64 {
    6.0 / 25.4 * 72.0
}

impl Default for Layout {
    fn default() -> Self {
        Layout::Size { mode: SizeMode::Fit, percent: 100.0 }
    }
}

/// Where a page lands on a sheet. The page's display space (`/Rotate` applied, origin at its
/// bounds' top left) is scaled by `scale`, optionally turned 90 degrees counter-clockwise, and its
/// top left corner put at `(x, y)` on the sheet; everything outside `clip` is cut off.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Placement {
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub rotate: bool,
    /// `[x0, y0, x1, y1]` in sheet space.
    pub clip: [f64; 4],
}

impl Placement {
    /// The affine matrix `[a b c d e f]` from the page's own coordinates (the bounds MuPDF reports,
    /// `origin` its top left, `width` its width) to sheet points.
    pub fn matrix(&self, origin: (f64, f64), width: f64) -> [f64; 6] {
        let s = self.scale;
        if self.rotate {
            // (u, v) -> (v, width - u): the top edge goes to the left edge.
            [0.0, -s, s, 0.0, self.x - s * origin.1, self.y + s * width + s * origin.0]
        } else {
            [s, 0.0, 0.0, s, self.x - s * origin.0, self.y - s * origin.1]
        }
    }
}

/// One side of a sheet of paper.
#[derive(Clone, Debug, PartialEq, Default, Serialize)]
pub struct Sheet {
    /// Paper size in points, as printed (already turned for landscape).
    pub w: f64,
    pub h: f64,
    pub placed: Vec<Placement>,
    /// Page borders (Multiple), as rectangles in sheet space.
    pub borders: Vec<[f64; 4]>,
    /// Poster cut marks: `[x0, y0, x1, y1]` hairlines in the margin, in sheet space.
    pub lines: Vec<[f64; 4]>,
    /// Poster labels, in the margin.
    pub labels: Vec<Label>,
    /// Poster: which tile of its page this side is.
    pub tile: Option<Tile>,
}

/// A line of text on a sheet, Helvetica at `LABEL_PT`, its baseline starting at `(x, y)`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Label {
    pub x: f64,
    pub y: f64,
    pub text: String,
}

/// Position of a poster tile in its page's grid (all 0-based, `cols` x `rows` tiles).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Tile {
    pub col: u32,
    pub row: u32,
    pub cols: u32,
    pub rows: u32,
}

impl Sheet {
    pub fn landscape(&self) -> bool {
        self.w > self.h
    }
}

fn oriented(paper: (f64, f64), o: Orientation, landscape_wanted: bool) -> (f64, f64) {
    let (w, h) = (paper.0.min(paper.1), paper.0.max(paper.1));
    match o {
        Orientation::Portrait => (w, h),
        Orientation::Landscape => (h, w),
        Orientation::Auto if landscape_wanted => (h, w),
        Orientation::Auto => (w, h),
    }
}

/// Place a page of display size `(dw, dh)` centred in `cell`, scaled by `s`, optionally turned.
fn place_in(page: u32, (dw, dh): (f64, f64), cell: [f64; 4], s: f64, rotate: bool, clip: [f64; 4]) -> Placement {
    let (cw, ch) = (cell[2] - cell[0], cell[3] - cell[1]);
    let (pw, ph) = if rotate { (dh * s, dw * s) } else { (dw * s, dh * s) };
    Placement { page, x: cell[0] + (cw - pw) / 2.0, y: cell[1] + (ch - ph) / 2.0, scale: s, rotate, clip }
}

fn fit_scale((dw, dh): (f64, f64), (cw, ch): (f64, f64)) -> f64 {
    if dw <= 0.0 || dh <= 0.0 { 1.0 } else { (cw / dw).min(ch / dh) }
}

/// Acrobat's pages-per-sheet choices as a grid (columns x rows on a portrait sheet).
pub fn grid(per_sheet: u32) -> (usize, usize) {
    match per_sheet {
        2 => (1, 2),
        4 => (2, 2),
        6 => (2, 3),
        9 => (3, 3),
        16 => (4, 4),
        n => {
            let c = (n as f64).sqrt().ceil().max(1.0) as usize;
            (c, (n as usize).div_ceil(c).max(1))
        }
    }
}

/// The sheets for the chosen `pages` (0-based, print order) given every source page's display size.
pub fn layout(sizes: &[(f64, f64)], pages: &[u32], paper: (f64, f64), orientation: Orientation, how: &Layout) -> Result<Vec<Sheet>, LayoutError> {
    let pages: Vec<u32> = pages.iter().copied().filter(|p| (*p as usize) < sizes.len()).collect();
    if pages.is_empty() {
        return Err(LayoutError::NoPages);
    }
    // 14400 points (200 in) is the PDF limit for a page; anything larger is a mistake, not paper.
    if !(paper.0 >= 72.0 && paper.1 >= 72.0 && paper.0 <= 14400.0 && paper.1 <= 14400.0) {
        return Err(LayoutError::Invalid("invalid paper size".into()));
    }
    let size_of = |p: u32| -> Result<(f64, f64), LayoutError> {
        let d = sizes[p as usize];
        if d.0.is_finite() && d.1.is_finite() && d.0 > 0.0 && d.1 > 0.0 {
            Ok(d)
        } else {
            Err(LayoutError::Invalid(format!("page {} has no size", p + 1)))
        }
    };
    let mut sheets = Vec::new();
    match *how {
        Layout::Size { mode, percent } => {
            for &p in &pages {
                let d = size_of(p)?;
                let (w, h) = oriented(paper, orientation, d.0 > d.1);
                let fit = fit_scale(d, (w - 2.0 * MARGIN, h - 2.0 * MARGIN));
                let s = match mode {
                    SizeMode::Fit => fit,
                    SizeMode::Actual => 1.0,
                    SizeMode::Shrink => fit.min(1.0),
                    SizeMode::Custom if percent.is_finite() && percent > 0.0 && percent <= 10_000.0 => percent / 100.0,
                    SizeMode::Custom => return Err(LayoutError::Invalid("the scale must be between 0% and 10000%".into())),
                };
                sheets.push(Sheet { w, h, placed: vec![place_in(p, d, [0.0, 0.0, w, h], s, false, [0.0, 0.0, w, h])], ..Sheet::default() });
            }
        }
        Layout::Multiple { per_sheet, order, border, auto_rotate } => {
            let (cols, rows) = grid(per_sheet);
            let cells = cols * rows;
            if per_sheet == 0 || cells > MAX_CELLS {
                return Err(LayoutError::Invalid("pages per sheet must be between 1 and 100".into()));
            }
            // A grid wider than tall wants a landscape sheet, and the reverse: 2 per sheet
            // (1 x 2 on portrait) prints side by side on landscape paper.
            let first = size_of(pages[0])?;
            let (gc, gr, sheet_landscape) = if cols == rows {
                (cols, rows, first.0 > first.1)
            } else if orientation == Orientation::Landscape || (orientation == Orientation::Auto && cols < rows) {
                (rows, cols, true)
            } else {
                (cols, rows, false)
            };
            let (w, h) = oriented(paper, orientation, sheet_landscape);
            let (aw, ah) = (w - 2.0 * MARGIN, h - 2.0 * MARGIN);
            let (cw, ch) = ((aw - GAP * (gc - 1) as f64) / gc as f64, (ah - GAP * (gr - 1) as f64) / gr as f64);
            if cw <= 0.0 || ch <= 0.0 {
                return Err(LayoutError::Invalid("the page grid does not fit on the paper".into()));
            }
            for chunk in pages.chunks(gc * gr) {
                let mut sheet = Sheet { w, h, ..Sheet::default() };
                for (k, &p) in chunk.iter().enumerate() {
                    let (col, row) = match order {
                        PageOrder::Horizontal => (k % gc, k / gc),
                        PageOrder::HorizontalReversed => (gc - 1 - k % gc, k / gc),
                        PageOrder::Vertical => (k / gr, k % gr),
                        PageOrder::VerticalReversed => (gc - 1 - k / gr, k % gr),
                    };
                    let x0 = MARGIN + col as f64 * (cw + GAP);
                    let y0 = MARGIN + row as f64 * (ch + GAP);
                    let cell = [x0, y0, x0 + cw, y0 + ch];
                    let d = size_of(p)?;
                    let rotate = auto_rotate && (d.0 > d.1) != (cw > ch) && (d.0 - d.1).abs() > 1.0;
                    let s = fit_scale(if rotate { (d.1, d.0) } else { d }, (cw, ch));
                    let pl = place_in(p, d, cell, s, rotate, cell);
                    if border {
                        let (pw, ph) = if rotate { (d.1 * s, d.0 * s) } else { (d.0 * s, d.1 * s) };
                        sheet.borders.push([pl.x, pl.y, pl.x + pw, pl.y + ph]);
                    }
                    sheet.placed.push(pl);
                }
                sheets.push(sheet);
            }
        }
        Layout::Booklet { sides, binding, creep } => {
            if !(creep.is_finite() && (0.0..=MAX_CREEP + 1e-6).contains(&creep)) {
                return Err(LayoutError::Invalid("the creep must be between 0 and 3 mm".into()));
            }
            // Padded to a multiple of 4 with blanks (None).
            let n4 = pages.len().div_ceil(4) * 4;
            let papers = n4 / 4;
            let at = |i: usize| pages.get(i).copied();
            let (w, h) = oriented(paper, Orientation::Landscape, true);
            let half = (w - 2.0 * MARGIN) / 2.0;
            // A single sheet has no inner one to move: no creep, no room kept for it.
            let creep = if papers > 1 { creep } else { 0.0 };
            // Each page keeps `creep` clear of the fold so the inner sheets can slide toward it
            // without crossing: the cell itself moves, clip included.
            let cells = [[MARGIN, MARGIN, MARGIN + half - creep, h - MARGIN], [MARGIN + half + creep, MARGIN, w - MARGIN, h - MARGIN]];
            for k in 0..papers {
                // sheet k from the outside: 0 for the outermost, `creep` for the innermost
                let shift = if papers > 1 { creep * k as f64 / (papers - 1) as f64 } else { 0.0 };
                let front = (at(n4 - 1 - 2 * k), at(2 * k));
                let back = (at(2 * k + 1), at(n4 - 2 - 2 * k));
                let mut faces = Vec::new();
                if sides != BookletSides::BackOnly {
                    faces.push(front);
                }
                if sides != BookletSides::FrontOnly {
                    faces.push(back);
                }
                for (l, r) in faces {
                    let (l, r) = if binding == Binding::Right { (r, l) } else { (l, r) };
                    let mut sheet = Sheet { w, h, ..Sheet::default() };
                    for (p, cell, dx) in [(l, cells[0], shift), (r, cells[1], -shift)] {
                        if let Some(p) = p {
                            let cell = [cell[0] + dx, cell[1], cell[2] + dx, cell[3]];
                            let d = size_of(p)?;
                            let s = fit_scale(d, (cell[2] - cell[0], cell[3] - cell[1]));
                            sheet.placed.push(place_in(p, d, cell, s, false, cell));
                        }
                    }
                    sheets.push(sheet);
                }
            }
        }
        Layout::Poster { scale, overlap, cut_marks, labels } => {
            if !(scale.is_finite() && scale > 0.0 && scale <= 10_000.0) {
                return Err(LayoutError::Invalid("the tile scale must be between 0% and 10000%".into()));
            }
            if !(overlap.is_finite() && (0.0..=MAX_OVERLAP + 1e-6).contains(&overlap)) {
                return Err(LayoutError::Invalid("the overlap must be between 0 and 50 mm".into()));
            }
            let s = scale / 100.0;
            let mut total = 0usize;
            for &p in &pages {
                let d = size_of(p)?;
                let (w, h) = oriented(paper, orientation, d.0 > d.1);
                let (tw, th) = (w - 2.0 * MARGIN, h - 2.0 * MARGIN);
                if overlap * 2.0 >= tw.min(th) {
                    return Err(LayoutError::Invalid("the overlap is larger than half a tile".into()));
                }
                // Each tile advances by `step`; the last one may reach past the page.
                let (sw, sh) = (d.0 * s, d.1 * s);
                let (step_x, step_y) = (tw - overlap, th - overlap);
                let count = |len: f64, step: f64| ((len - overlap) / step - 1e-9).ceil().max(1.0) as usize;
                let (cols, rows) = (count(sw, step_x), count(sh, step_y));
                // counted before anything is allocated: a big page at a big scale is a mistake
                total = total.saturating_add(cols.saturating_mul(rows));
                if total > MAX_POSTER_SHEETS {
                    return Err(LayoutError::Invalid(format!("the poster would need more than {MAX_POSTER_SHEETS} sheets")));
                }
                for row in 0..rows {
                    for col in 0..cols {
                        let (x, y) = (MARGIN - col as f64 * step_x, MARGIN - row as f64 * step_y);
                        let clip = [MARGIN.max(x), MARGIN.max(y), (MARGIN + tw).min(x + sw), (MARGIN + th).min(y + sh)];
                        let mut sheet = Sheet {
                            w,
                            h,
                            placed: vec![Placement { page: p, x, y, scale: s, rotate: false, clip }],
                            tile: Some(Tile { col: col as u32, row: row as u32, cols: cols as u32, rows: rows as u32 }),
                            ..Sheet::default()
                        };
                        if cut_marks {
                            // Ticks in the margin where a neighbour's printed area begins or ends:
                            // the edges of the overlap strip (the printable edge when there is none).
                            let mut vertical = Vec::new();
                            let mut horizontal = Vec::new();
                            if col > 0 {
                                vertical.push(MARGIN + overlap);
                            }
                            if col + 1 < cols {
                                vertical.push(MARGIN + tw - overlap);
                            }
                            if row > 0 {
                                horizontal.push(MARGIN + overlap);
                            }
                            if row + 1 < rows {
                                horizontal.push(MARGIN + th - overlap);
                            }
                            for x in vertical {
                                sheet.lines.push([x, 2.0, x, MARGIN - 2.0]);
                                sheet.lines.push([x, h - MARGIN + 2.0, x, h - 2.0]);
                            }
                            for y in horizontal {
                                sheet.lines.push([2.0, y, MARGIN - 2.0, y]);
                                sheet.lines.push([w - MARGIN + 2.0, y, w - 2.0, y]);
                            }
                        }
                        if labels {
                            let text = format!("Page {}, row {} of {}, column {} of {}", p + 1, row + 1, rows, col + 1, cols);
                            // Helvetica averages about half an em a character
                            let x = ((w - text.len() as f64 * LABEL_PT * 0.5) / 2.0).max(2.0);
                            sheet.labels.push(Label { x, y: MARGIN - 6.0, text });
                        }
                        sheets.push(sheet);
                    }
                }
            }
        }
    }
    Ok(sheets)
}

/// What the Pages to Print section chooses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Range {
    All,
    Current { page: u32 },
    Pages { text: String },
}

/// The pages to print (0-based, in print order). `Pages` is `1-3, 5, 8-` (one-based; a bad range is an
/// error naming it). `Odd` and `Even` count among the chosen pages, as Acrobat does; `reverse` flips the
/// order last.
pub fn select_pages(count: u32, range: &Range, subset: Subset, reverse: bool) -> Result<Vec<u32>, LayoutError> {
    let mut pages: Vec<u32> = match range {
        Range::All => (0..count).collect(),
        Range::Current { page } if *page < count => vec![*page],
        Range::Current { page } => return Err(LayoutError::Invalid(format!("page {} is out of range (1-{count})", page + 1))),
        Range::Pages { text } if text.trim().is_empty() => (0..count).collect(),
        Range::Pages { text } => {
            crate::ocr::parse_page_range(text, count).map_err(|_| LayoutError::Invalid(format!("\"{}\" is not a page range of 1-{count}", text.trim())))?
        }
    };
    pages = match subset {
        Subset::All => pages,
        Subset::Odd => pages.into_iter().step_by(2).collect(),
        Subset::Even => pages.into_iter().skip(1).step_by(2).collect(),
    };
    if reverse {
        pages.reverse();
    }
    if pages.is_empty() {
        return Err(LayoutError::NoPages);
    }
    Ok(pages)
}

/// The order sheets are sent in once copies, collation and two-sided printing are applied: indices into
/// the sheets, `None` for a blank back. Collated copies repeat the whole job (each copy ends on a full
/// sheet of paper when printing two-sided); uncollated copies repeat each sheet of paper in place.
pub fn sequence(sheets: usize, copies: u32, collate: bool, two_sided: bool) -> Vec<Option<usize>> {
    let copies = copies.clamp(1, 999) as usize;
    let unit = if two_sided { 2 } else { 1 };
    let papers: Vec<Vec<Option<usize>>> = (0..sheets.div_ceil(unit))
        .map(|k| (0..unit).map(|j| (k * unit + j < sheets).then_some(k * unit + j)).collect())
        .collect();
    let mut out = Vec::new();
    if collate {
        for _ in 0..copies {
            papers.iter().for_each(|p| out.extend(p));
        }
    } else {
        for p in &papers {
            for _ in 0..copies {
                out.extend(p);
            }
        }
    }
    out
}
