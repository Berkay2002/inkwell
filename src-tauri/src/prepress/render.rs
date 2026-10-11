//! A page painted into CMYK planes (one more per spot colour, overprint simulated) and the views
//! composed from them: the plates the person switched on, or a heat map of ink coverage.

use super::seps::MAX_SPOT_PLANES;
use super::{DEFAULT_TAC_LIMIT, PageSeps, Result, bad};
use crate::engine::cache::{Tile, TileKey};
use crate::engine::{Engine, TILE, view_matrix};
use mupdf::{Colorspace, Device, DisplayList, IRect, Matrix, Pixmap, Rect, SeparationBehavior, Separations};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The process inks, in plane order.
pub const PROCESS: [&str; 4] = ["Cyan", "Magenta", "Yellow", "Black"];

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// The page as the inks switched on would print it.
    #[default]
    Plates,
    /// A heat map of total ink coverage; above the limit is red.
    Coverage,
}

/// What a print production tile shows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub mode: Mode,
    /// Cyan, Magenta, Yellow, Black: shown or not.
    pub process: [bool; 4],
    /// Spot colours that are switched off, by name.
    pub hidden_spots: Vec<String>,
    /// Simulate overprint, and keep every spot colour on a plane of its own. Without it the page
    /// is painted knocked out and the spot colours are folded into CMYK through their alternates
    /// (so only the process plates can be switched).
    pub overprint: bool,
    /// Per cent; coverage above it is flagged.
    pub tac_limit: f32,
}

impl Default for View {
    fn default() -> Self {
        View { mode: Mode::Plates, process: [true; 4], hidden_spots: Vec::new(), overprint: true, tac_limit: DEFAULT_TAC_LIMIT }
    }
}

impl View {
    /// Whether plane `i` (0-3 process, then the spots) is on.
    fn plane_on(&self, seps: &PageSeps, i: usize) -> bool {
        if i < 4 { self.process[i] } else { !self.hidden_spots.iter().any(|h| *h == seps.spots[i - 4].name) }
    }
}

/// The painted page: `n` bytes per pixel, C M Y K then one per spot, rows tight.
pub(crate) struct Planes {
    pub w: usize,
    pub h: usize,
    pub n: usize,
    pub data: Vec<u8>,
}

fn rgba(rgb: [u8; 3]) -> u32 {
    (rgb[0] as u32) << 24 | (rgb[1] as u32) << 16 | (rgb[2] as u32) << 8 | 0xff
}

fn cmyk32(c: [u8; 4]) -> u32 {
    let b = |v: u8| (v as u32 * 255 / 100) & 0xff;
    b(c[0]) << 24 | b(c[1]) << 16 | b(c[2]) << 8 | b(c[3])
}

/// Paints the part `rect` (device pixels) of `list` under `m`.
pub(crate) fn paint_planes(list: &DisplayList, seps: &PageSeps, overprint: bool, m: &Matrix, rect: IRect) -> Result<Planes> {
    let cs = Colorspace::device_cmyk();
    let spots = if overprint { seps.spots.len().min(MAX_SPOT_PLANES) } else { 0 };
    let mut pix = if overprint {
        let mut set = Separations::new()?;
        // only the spots that get a plane are named: MuPDF turns every other one it knows into a plane of an
        // internal pixmap, which would be too wide; an unnamed spot is painted through its alternate
        for s in seps.spots.iter().take(spots) {
            set.add(&s.name, rgba(s.rgb), cmyk32(s.cmyk))?;
        }
        for i in 0..spots {
            set.set_behavior(i, SeparationBehavior::Spot)?;
        }
        Pixmap::new_with_separations(&cs, rect, &set, false)?
    } else {
        Pixmap::new_with_rect(&cs, rect, false)?
    };
    // 0xff is paper: for a subtractive pixmap MuPDF clears every ink plane to none
    pix.clear_with(0xff)?;
    {
        let dev = Device::from_pixmap_with_clip(&pix, rect)?;
        let area = Rect::new(rect.x0 as f32, rect.y0 as f32, rect.x1 as f32, rect.y1 as f32);
        list.run(&dev, m, area)?;
    }
    let n = pix.n() as usize;
    if n != 4 + spots || pix.spot_planes() != spots {
        return Err(bad("the page could not be painted with its separations"));
    }
    let (w, h) = (rect.width() as usize, rect.height() as usize);
    let stride = pix.stride() as usize;
    let samples = pix.samples();
    let data = if stride == w * n {
        samples[..w * h * n].to_vec()
    } else {
        let mut v = Vec::with_capacity(w * h * n);
        for row in 0..h {
            v.extend_from_slice(&samples[row * stride..row * stride + w * n]);
        }
        v
    };
    Ok(Planes { w, h, n, data })
}

/// The inks on screen: white paper, each plane darkening it towards its own colour (the process
/// planes the way MuPDF shows DeviceCMYK).
pub(crate) fn compose_plates(p: &Planes, seps: &PageSeps, view: &View) -> Vec<u8> {
    let on: Vec<bool> = (0..p.n).map(|i| view.plane_on(seps, i)).collect();
    let mut out = Vec::with_capacity(p.w * p.h * 4);
    for px in p.data.chunks_exact(p.n) {
        let g = |i: usize| if on[i] { px[i] as u32 } else { 0 };
        let k = g(3);
        let mut rgb = [255 - (g(0) + k).min(255), 255 - (g(1) + k).min(255), 255 - (g(2) + k).min(255)];
        for (j, spot) in seps.spots.iter().enumerate().take(p.n.saturating_sub(4)) {
            let t = g(4 + j);
            if t == 0 {
                continue;
            }
            for (c, v) in rgb.iter_mut().enumerate() {
                // a tint t of an ink whose solid colour is `e` leaves (255 - t * (255 - e) / 255) of the light
                *v = *v * (255 - t * (255 - spot.rgb[c] as u32) / 255) / 255;
            }
        }
        out.extend_from_slice(&[rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, 255]);
    }
    out
}

/// Total ink of one pixel in per cent, over the planes that are on.
pub(crate) fn coverage_of(px: &[u8], on: &[bool]) -> f32 {
    px.iter().zip(on).filter(|(_, on)| **on).map(|(v, _)| *v as f32).sum::<f32>() * 100.0 / 255.0
}

/// The heat map: paper white at no ink, blue through green and yellow as coverage rises, red above
/// the limit.
pub(crate) fn heat_colour(cov: f32, limit: f32) -> [u8; 3] {
    if cov <= 0.5 {
        return [255, 255, 255];
    }
    if cov > limit {
        return [214, 24, 28];
    }
    let t = (cov / limit.max(1.0)).clamp(0.0, 1.0);
    // 0 -> pale blue, 0.5 -> green, 1 -> orange
    let stops: [[f32; 3]; 3] = [[190.0, 215.0, 245.0], [120.0, 200.0, 120.0], [250.0, 170.0, 40.0]];
    let (a, b, u) = if t < 0.5 { (stops[0], stops[1], t * 2.0) } else { (stops[1], stops[2], (t - 0.5) * 2.0) };
    [0, 1, 2].map(|i| (a[i] + (b[i] - a[i]) * u).round() as u8)
}

pub(crate) fn compose_coverage(p: &Planes, seps: &PageSeps, view: &View) -> Vec<u8> {
    let on: Vec<bool> = (0..p.n).map(|i| view.plane_on(seps, i)).collect();
    let mut out = Vec::with_capacity(p.w * p.h * 4);
    for px in p.data.chunks_exact(p.n) {
        let c = heat_colour(coverage_of(px, &on), view.tac_limit);
        out.extend_from_slice(&[c[0], c[1], c[2], 255]);
    }
    out
}

/// One 512-pixel tile of a page in `view`, laid out exactly like `Engine::render_tile`'s tiles.
pub fn render_view_tile(eng: &Engine, seps: &PageSeps, key: &TileKey, view: &View) -> Result<Tile> {
    let entry = eng.doc_entry(key.doc)?;
    let list: Arc<DisplayList> = eng.page_list(&entry, key.rev, key.page)?;
    let scale = key.scale as f32 / 1000.0;
    let (m, pw, ph) = view_matrix(list.bounds(), key.rot, scale);
    let x0 = key.tx as i32 * TILE as i32;
    let y0 = key.ty as i32 * TILE as i32;
    if x0 >= pw || y0 >= ph {
        return Err(bad("tile outside page"));
    }
    let rect = IRect { x0, y0, x1: (x0 + TILE as i32).min(pw), y1: (y0 + TILE as i32).min(ph) };
    let planes = paint_planes(&list, seps, view.overprint, &m, rect)?;
    let rgba = match view.mode {
        Mode::Plates => compose_plates(&planes, seps, view),
        Mode::Coverage => compose_coverage(&planes, seps, view),
    };
    Ok(Tile { w: planes.w as u32, h: planes.h as u32, rgba: Arc::new(rgba) })
}
