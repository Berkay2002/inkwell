//! The inks a page uses: process CMYK and the spot colours (Separation and DeviceN colorants).

use super::{PROCESS, Result};
use crate::engine::{DocId, Engine};
use mupdf::Colorspace;
use serde::Serialize;

/// One spot colour: its name and how it looks on screen (the colour of its alternate at full tint).
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Spot {
    pub name: String,
    pub rgb: [u8; 3],
    /// The same colour in CMYK, per cent.
    pub cmyk: [u8; 4],
}

/// What a page paints with.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PageSeps {
    /// Always Cyan, Magenta, Yellow, Black: the order of the process planes.
    pub process: [&'static str; 4],
    /// Spot colours, in plane order.
    pub spots: Vec<Spot>,
    /// The page paints with overprint (so the overprint preview can differ from the plain one).
    pub overprint: bool,
    /// Spots past the first `MAX_SPOT_PLANES`: a pixmap holds 32 components, so these are painted into
    /// CMYK through their alternates and cannot be switched on their own.
    pub folded: usize,
}

/// Spot colours that get a plane of their own: MuPDF holds 32 components in a pixmap, and a group's
/// alpha takes one, so 32 less the four process inks less one.
pub const MAX_SPOT_PLANES: usize = 27;

impl PageSeps {
    /// The plane index of a spot, counted after the four process planes.
    pub fn plane_of(&self, name: &str) -> Option<usize> {
        self.spots.iter().position(|s| s.name == name).filter(|&i| i < MAX_SPOT_PLANES).map(|i| 4 + i)
    }
}

/// Lists the inks of page `page` (0-based) of the document as it is now.
pub fn page_separations(eng: &Engine, doc: DocId, page: u32) -> Result<PageSeps> {
    eng.read_custom(doc, |pdf| {
        let count = pdf.page_count()?.max(0) as u32;
        if page >= count {
            return Err(crate::engine::EngineError::BadPage(page));
        }
        let p = pdf.load_page(page as i32)?;
        let overprint = p.uses_overprint().unwrap_or(false);
        let seps = p.separations()?;
        let rgb = Colorspace::device_rgb();
        let cmyk = Colorspace::device_cmyk();
        let mut spots = Vec::new();
        for i in 0..seps.len() {
            let Some(name) = seps.name(i) else { continue };
            // Colorants of a DeviceN that are the process inks are painted by the process planes.
            if PROCESS.contains(&name.as_str()) || name == "All" || name == "None" || spots.iter().any(|s: &Spot| s.name == name) {
                continue;
            }
            let to = |cs: &Colorspace| seps.equivalent(i, cs).unwrap_or_default();
            let (r, c) = (to(&rgb), to(&cmyk));
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let pct = |v: f32| (v.clamp(0.0, 1.0) * 100.0).round() as u8;
            spots.push(Spot {
                name,
                rgb: if r.len() == 3 { [byte(r[0]), byte(r[1]), byte(r[2])] } else { [128, 128, 128] },
                cmyk: if c.len() == 4 { [pct(c[0]), pct(c[1]), pct(c[2]), pct(c[3])] } else { [0, 0, 0, 100] },
            });
        }
        let folded = spots.len().saturating_sub(MAX_SPOT_PLANES);
        Ok(PageSeps { process: PROCESS, spots, overprint, folded })
    })
}
