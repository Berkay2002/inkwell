//! The object inspector: what is painted at a point of a page, topmost first, with its colour space,
//! its values, its CMYK equivalent and whether it overprints.
//!
//! The page's display list runs through a device that records every fill, stroke, text run and image;
//! one that covers the point (tested by painting it into a 3 x 3 pixel probe, so the page's own
//! geometry decides) is reported. Clips are honoured; shadings count where no clip excludes them.

use super::{Result, bad};
use crate::engine::{DocId, Engine};
use mupdf::{ColorParams, Colorspace, Device, Function, Image, Matrix, NativeDevice, Path, Pixmap, Rect, Shade, StrokeState, Text};
use serde::Serialize;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Fill,
    Stroke,
    Text,
    StrokedText,
    Image,
    ImageMask,
    Shading,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Inspected {
    pub kind: ObjectKind,
    /// `DeviceCMYK`, `ICCBased`, `Separation / DeviceN`, ... as MuPDF names the space.
    pub color_space: String,
    /// `gray`, `rgb`, `cmyk`, `lab`, `indexed`, `spot`, or `shading` (a shading's colours are not read).
    pub model: &'static str,
    /// Names of the colorants of a spot space.
    pub colorants: Vec<String>,
    /// The components as the page gives them (0 to 1).
    pub values: Vec<f32>,
    /// The colour in DeviceCMYK, per cent. None for an image or a shading.
    pub cmyk: Option<[f32; 4]>,
    /// The ink of that CMYK, per cent (the four added), plus any spot tint.
    pub ink: Option<f32>,
    pub overprint: bool,
    /// Overprint mode 1: a zero component of a CMYK colour leaves the plate beneath alone.
    pub overprint_mode: bool,
    pub alpha: f32,
    /// `[x0, y0, x1, y1]` in page points from the top-left of the page as shown.
    pub bounds: [f32; 4],
    /// The first font of a text run.
    pub font: Option<String>,
    /// Pixels, bits per component of an image.
    pub image: Option<[u32; 3]>,
}

struct Probe {
    pix: Pixmap,
    dev: Device,
    gray: Colorspace,
    at: (f32, f32),
}

impl Probe {
    fn new(at: (f32, f32)) -> Result<Self> {
        let gray = Colorspace::device_gray();
        let pix = Pixmap::new(&gray, 0, 0, 3, 3, false)?;
        let dev = Device::from_pixmap(&pix)?;
        Ok(Probe { pix, dev, gray, at })
    }

    /// The page matrix moved so the point is the centre of the middle pixel.
    fn moved(&self, ctm: &Matrix) -> Matrix {
        let mut m = Matrix::new(ctm.a, ctm.b, ctm.c, ctm.d, ctm.e, ctm.f);
        m.concat(Matrix::new_translate(1.5 - self.at.0, 1.5 - self.at.1));
        m
    }

    fn hit(&mut self, paint: impl FnOnce(&Device, &Colorspace) -> std::result::Result<(), mupdf::Error>) -> bool {
        if self.pix.clear_with(0xff).is_err() || paint(&self.dev, &self.gray).is_err() {
            return false;
        }
        self.pix.pixel(1, 1).map(|p| p.components().first().is_some_and(|v| *v < 255)).unwrap_or(false)
    }
}

struct State {
    probe: Probe,
    /// One entry per open clip: whether the probed point is inside it (clips this inspector cannot
    /// test count as inside).
    clips: Vec<bool>,
    tile_depth: u32,
    found: Vec<Inspected>,
}

impl State {
    fn clipped_out(&self) -> bool {
        self.clips.iter().any(|inside| !inside)
    }
}

struct Recorder(Rc<RefCell<State>>);

fn model_of(cs: &Colorspace) -> &'static str {
    if cs.is_separation() {
        "spot"
    } else if cs.is_indexed() {
        "indexed"
    } else if cs.is_lab() || cs.is_lab_icc() {
        "lab"
    } else if cs.is_cmyk() {
        "cmyk"
    } else if cs.is_rgb() {
        "rgb"
    } else {
        "gray"
    }
}

fn colour(kind: ObjectKind, cs: &Colorspace, color: &[f32], alpha: f32, cp: ColorParams, bounds: Rect) -> Inspected {
    let n = cs.n() as usize;
    let values: Vec<f32> = color.iter().copied().take(n).collect();
    let cmyk = cs.convert_color(&values, &Colorspace::device_cmyk(), None, ColorParams::default()).ok().filter(|c| c.len() == 4);
    let mut colorants = Vec::new();
    if cs.is_separation() {
        colorants = (0..cs.n()).filter_map(|i| cs.colorant(i)).collect();
    }
    let spot_tint: f32 = if cs.is_separation() { values.iter().sum::<f32>() * 100.0 } else { 0.0 };
    let pct = cmyk.as_ref().map(|c| [c[0] * 100.0, c[1] * 100.0, c[2] * 100.0, c[3] * 100.0]);
    Inspected {
        kind,
        color_space: if cs.is_separation() { "Separation / DeviceN".into() } else if cs.is_icc() && !cs.is_device() { format!("ICCBased ({})", cs.name()) } else { cs.name().to_string() },
        model: model_of(cs),
        colorants,
        values,
        ink: pct.map(|p| p.iter().sum::<f32>()).map(|t| if cs.is_separation() { t.max(spot_tint) } else { t }),
        cmyk: pct,
        overprint: cp.op(),
        overprint_mode: cp.opm(),
        alpha,
        bounds: [bounds.x0, bounds.y0, bounds.x1, bounds.y1],
        font: None,
        image: None,
    }
}

impl Recorder {
    fn push(&self, hit: bool, f: impl FnOnce() -> Inspected) {
        let mut s = self.0.borrow_mut();
        if s.tile_depth == 0 && hit && !s.clipped_out() {
            s.found.push(f());
        }
    }
}

#[allow(unused_variables)]
impl NativeDevice for Recorder {
    fn fill_path(&mut self, path: &Path, even_odd: bool, cmt: Matrix, cs: &Colorspace, color: &[f32], alpha: f32, cp: ColorParams) {
        let hit = {
            let mut s = self.0.borrow_mut();
            let m = s.probe.moved(&cmt);
            s.probe.hit(|d, g| d.fill_path(path, even_odd, &m, g, &[0.0], 1.0, ColorParams::default()))
        };
        let b = path.bounds(&StrokeState::default(), &cmt).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        self.push(hit, || colour(ObjectKind::Fill, cs, color, alpha, cp, b));
    }

    fn stroke_path(&mut self, path: &Path, stroke: &StrokeState, cmt: Matrix, cs: &Colorspace, color: &[f32], alpha: f32, cp: ColorParams) {
        let hit = {
            let mut s = self.0.borrow_mut();
            let m = s.probe.moved(&cmt);
            s.probe.hit(|d, g| d.stroke_path(path, stroke, &m, g, &[0.0], 1.0, ColorParams::default()))
        };
        let b = path.bounds(stroke, &cmt).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        self.push(hit, || colour(ObjectKind::Stroke, cs, color, alpha, cp, b));
    }

    fn clip_path(&mut self, path: &Path, even_odd: bool, cmt: Matrix, scissor: Rect) {
        let mut s = self.0.borrow_mut();
        let m = s.probe.moved(&cmt);
        let inside = s.probe.hit(|d, g| d.fill_path(path, even_odd, &m, g, &[0.0], 1.0, ColorParams::default()));
        s.clips.push(inside);
    }

    fn clip_stroke_path(&mut self, path: &Path, stroke_state: &StrokeState, cmt: Matrix, scissor: Rect) {
        self.0.borrow_mut().clips.push(true);
    }

    fn fill_text(&mut self, text: &Text, cmt: Matrix, cs: &Colorspace, color: &[f32], alpha: f32, cp: ColorParams) {
        let hit = {
            let mut s = self.0.borrow_mut();
            let m = s.probe.moved(&cmt);
            s.probe.hit(|d, g| d.fill_text(text, &m, g, &[0.0], 1.0, ColorParams::default()))
        };
        let b = text.bounds(&StrokeState::default(), &cmt).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        let font = text.spans().next().map(|sp| sp.font().name().to_string());
        self.push(hit, || Inspected { font, ..colour(ObjectKind::Text, cs, color, alpha, cp, b) });
    }

    fn stroke_text(&mut self, text: &Text, stroke_state: &StrokeState, cmt: Matrix, cs: &Colorspace, color: &[f32], alpha: f32, cp: ColorParams) {
        let hit = {
            let mut s = self.0.borrow_mut();
            let m = s.probe.moved(&cmt);
            s.probe.hit(|d, g| d.stroke_text(text, stroke_state, &m, g, &[0.0], 1.0, ColorParams::default()))
        };
        let b = text.bounds(stroke_state, &cmt).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        let font = text.spans().next().map(|sp| sp.font().name().to_string());
        self.push(hit, || Inspected { font, ..colour(ObjectKind::StrokedText, cs, color, alpha, cp, b) });
    }

    fn clip_text(&mut self, text: &Text, cmt: Matrix, scissor: Rect) {
        self.0.borrow_mut().clips.push(true);
    }

    fn clip_stroke_text(&mut self, text: &Text, stroke_state: &StrokeState, cmt: Matrix, scissor: Rect) {
        self.0.borrow_mut().clips.push(true);
    }

    fn fill_shade(&mut self, shade: &Shade, cmt: Matrix, alpha: f32, cp: ColorParams) {
        let b = Rect::new(0.0, 0.0, 0.0, 0.0);
        self.push(true, || Inspected {
            kind: ObjectKind::Shading,
            color_space: "Shading".into(),
            model: "shading",
            colorants: vec![],
            values: vec![],
            cmyk: None,
            ink: None,
            overprint: cp.op(),
            overprint_mode: cp.opm(),
            alpha,
            bounds: [b.x0, b.y0, b.x1, b.y1],
            font: None,
            image: None,
        });
    }

    fn fill_image(&mut self, img: &Image, cmt: Matrix, alpha: f32, cp: ColorParams) {
        let (hit, b) = unit_square_hit(&self.0, &cmt);
        let cs = img.color_space();
        self.push(hit, || {
            let mut o = match &cs {
                Some(cs) => colour(ObjectKind::Image, cs, &[], alpha, cp, b),
                None => colour(ObjectKind::Image, &Colorspace::device_gray(), &[], alpha, cp, b),
            };
            o.values.clear();
            o.cmyk = None;
            o.ink = None;
            o.image = Some([img.width(), img.height(), img.bits_per_components() as u32]);
            o
        });
    }

    fn fill_image_mask(&mut self, img: &Image, cmt: Matrix, cs: &Colorspace, color: &[f32], alpha: f32, cp: ColorParams) {
        let (hit, b) = unit_square_hit(&self.0, &cmt);
        self.push(hit, || {
            let mut o = colour(ObjectKind::ImageMask, cs, color, alpha, cp, b);
            o.image = Some([img.width(), img.height(), 1]);
            o
        });
    }

    fn clip_image_mask(&mut self, img: &Image, cmt: Matrix, scissor: Rect) {
        self.0.borrow_mut().clips.push(true);
    }

    fn pop_clip(&mut self) {
        self.0.borrow_mut().clips.pop();
    }

    fn begin_mask(&mut self, area: Rect, luminosity: bool, cs: &Colorspace, color: &[f32], cp: ColorParams) {
        // the soft mask's own drawing is not page content; the mask ends with a pop_clip
        self.0.borrow_mut().tile_depth += 1;
    }

    fn end_mask(&mut self, f: &Function) {
        let mut s = self.0.borrow_mut();
        s.tile_depth = s.tile_depth.saturating_sub(1);
        s.clips.push(true);
    }

    fn begin_tile(&mut self, area: Rect, view: Rect, xstep: f32, ystep: f32, cmt: Matrix, id: Option<std::num::NonZero<i32>>, doc_id: Option<std::num::NonZero<i32>>) -> Option<std::num::NonZero<i32>> {
        self.0.borrow_mut().tile_depth += 1;
        None
    }

    fn end_tile(&mut self) {
        let mut s = self.0.borrow_mut();
        s.tile_depth = s.tile_depth.saturating_sub(1);
    }
}

fn unit_square_hit(state: &Rc<RefCell<State>>, ctm: &Matrix) -> (bool, Rect) {
    let s = state.borrow();
    let (x, y) = s.probe.at;
    let corners = [ctm.transform_xy(0.0, 0.0), ctm.transform_xy(1.0, 0.0), ctm.transform_xy(0.0, 1.0), ctm.transform_xy(1.0, 1.0)];
    let b = Rect::new(
        corners.iter().map(|c| c.0).fold(f32::MAX, f32::min),
        corners.iter().map(|c| c.1).fold(f32::MAX, f32::min),
        corners.iter().map(|c| c.0).fold(f32::MIN, f32::max),
        corners.iter().map(|c| c.1).fold(f32::MIN, f32::max),
    );
    let hit = ctm.invert().is_some_and(|inv| {
        let (u, v) = inv.transform_xy(x, y);
        (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)
    });
    (hit, b)
}

/// What page `page` paints at `(x, y)` (page points from the top-left of the page as shown), topmost
/// first, at most `limit` objects.
pub fn inspect_at(eng: &Engine, doc: DocId, page: u32, x: f32, y: f32, limit: usize) -> Result<Vec<Inspected>> {
    let entry = eng.doc_entry(doc)?;
    let list = eng.page_list(&entry, entry.generation(), page)?;
    let b = list.bounds();
    if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 || x > b.width() || y > b.height() {
        return Err(bad("the point is outside the page"));
    }
    // the display list is in page space with its origin at the bounds' corner
    let (px, py) = (b.x0 + x, b.y0 + y);
    let state = Rc::new(RefCell::new(State { probe: Probe::new((px, py))?, clips: Vec::new(), tile_depth: 0, found: Vec::new() }));
    let dev = Device::from_native(Recorder(state.clone()))?;
    list.run(&dev, &Matrix::IDENTITY, Rect::INF)?;
    drop(dev);
    let mut found = std::mem::take(&mut state.borrow_mut().found);
    found.reverse();
    found.truncate(limit);
    // report positions the way the caller gave them
    for o in &mut found {
        o.bounds = [o.bounds[0] - b.x0, o.bounds[1] - b.y0, o.bounds[2] - b.x0, o.bounds[3] - b.y0];
    }
    Ok(found)
}
