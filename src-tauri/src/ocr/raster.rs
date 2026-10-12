//! Pixel work for the cleaned-up style: turn a tilted scan upright, remove specks, pack bilevel
//! pages. Plain functions on 8-bit rasters, so they can be tested without a PDF.

use crate::textedit::content::Matrix;

/// 8-bit pixels, gray (`n` = 1) or RGB (`n` = 3), rows top to bottom, no padding.
#[derive(Clone, Debug)]
pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub n: usize,
    pub px: Vec<u8>,
}

impl Raster {
    #[cfg(test)]
    pub fn filled(w: usize, h: usize, n: usize, value: &[u8]) -> Self {
        let mut px = Vec::with_capacity(w * h * n);
        for _ in 0..w * h {
            px.extend_from_slice(&value[..n]);
        }
        Self { w, h, n, px }
    }

    /// Brightness per pixel (Rec. 601 weights for RGB).
    pub fn luma(&self) -> Vec<u8> {
        if self.n == 1 {
            return self.px.clone();
        }
        self.px.chunks_exact(3).map(|p| ((299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000) as u8).collect()
    }
}

/// Otsu's threshold: the level that best separates dark from light pixels. Pixels below it are ink.
pub fn otsu(l: &[u8]) -> u8 {
    let mut hist = [0u64; 256];
    for &v in l {
        hist[v as usize] += 1;
    }
    let total = l.len() as f64;
    let sum_all: f64 = hist.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();
    let (mut w0, mut sum0, mut best, mut best_t) = (0.0f64, 0.0f64, -1.0f64, 128u8);
    for t in 0..256 {
        w0 += hist[t] as f64;
        if w0 == 0.0 {
            continue;
        }
        let w1 = total - w0;
        if w1 == 0.0 {
            break;
        }
        sum0 += t as f64 * hist[t] as f64;
        let (m0, m1) = (sum0 / w0, (sum_all - sum0) / w1);
        let between = w0 * w1 * (m0 - m1) * (m0 - m1);
        if between > best {
            best = between;
            // pixels <= t are the dark class: ink is "< threshold", so the threshold is t + 1
            best_t = (t as u8).saturating_add(1);
        }
    }
    best_t
}

/// The paper colour: the most common value of each channel among the pixels at or above `thr`
/// (a mean would be tinted by the anti-aliased edges of the text).
pub fn paper(r: &Raster, l: &[u8], thr: u8) -> Vec<u8> {
    let mut hist = vec![[0u32; 256]; r.n];
    let mut any = false;
    for (i, &v) in l.iter().enumerate() {
        if v >= thr {
            any = true;
            for (c, h) in hist.iter_mut().enumerate() {
                h[r.px[i * r.n + c] as usize] += 1;
            }
        }
    }
    if !any {
        return vec![255; r.n];
    }
    hist.iter().map(|h| h.iter().enumerate().max_by_key(|&(_, n)| n).map_or(255, |(v, _)| v as u8)).collect()
}

/// A copy of `r` where output pixel `(x, y)` takes the value at `src_from_dst` of its centre,
/// bilinearly; outside the source it is `fill`. Both are in pixel units (pixel `(i, j)` covers
/// `[i, i+1) × [j, j+1)`).
pub fn warp(r: &Raster, src_from_dst: &Matrix, fill: &[u8]) -> Raster {
    let [a, b, c, d, e, f] = src_from_dst.0;
    let (w, h, n) = (r.w, r.h, r.n);
    let mut out = vec![0u8; r.px.len()];
    let (wf, hf) = (w as f64, h as f64);
    for y in 0..h {
        let py = y as f64 + 0.5;
        for x in 0..w {
            let px = x as f64 + 0.5;
            let sx = a * px + c * py + e;
            let sy = b * px + d * py + f;
            let o = (y * w + x) * n;
            if sx < 0.0 || sy < 0.0 || sx >= wf || sy >= hf {
                out[o..o + n].copy_from_slice(&fill[..n]);
                continue;
            }
            let (fx, fy) = (sx - 0.5, sy - 0.5);
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let at = |xi: f64, yi: f64| -> usize { (yi.clamp(0.0, hf - 1.0) as usize * w + xi.clamp(0.0, wf - 1.0) as usize) * n };
            let (i00, i10, i01, i11) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
            for ch in 0..n {
                let top = f64::from(r.px[i00 + ch]) * (1.0 - tx) + f64::from(r.px[i10 + ch]) * tx;
                let bot = f64::from(r.px[i01 + ch]) * (1.0 - tx) + f64::from(r.px[i11 + ch]) * tx;
                out[o + ch] = (top * (1.0 - ty) + bot * ty).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Raster { w, h, n, px: out }
}

/// Most of what a scan's despeckler would do wrong is removing dots that belong to the text (full
/// stops, accents, the dot of an i). A speck is at most this many pixels (scaled to the resolution)
/// and has no other ink within `gap` pixels.
fn speck_limits(dpi: f32) -> (usize, usize) {
    // 4 pixels at 300 dpi (a 2 pixel dot), 1 at 150: a full stop of 10 pt text is 4 pixels across.
    let area = ((dpi / 300.0).powi(2) * 4.0).round() as usize;
    let gap = (dpi / 40.0).round() as usize;
    (area.max(1), gap.clamp(5, 14))
}

/// Removes isolated specks (paints them with the paper colour). Returns how many. Pages that are
/// mostly dark (a photograph, an inverted scan) are left alone.
pub fn despeckle(r: &mut Raster, dpi: f32) -> u32 {
    let (w, h) = (r.w, r.h);
    if w < 8 || h < 8 {
        return 0;
    }
    let l = r.luma();
    let thr = otsu(&l);
    let ink: Vec<bool> = l.iter().map(|&v| v < thr).collect();
    let dark = ink.iter().filter(|&&b| b).count();
    if dark == 0 || dark * 2 > w * h {
        return 0;
    }
    let (max_area, gap) = speck_limits(dpi);

    // Connected components (8-neighbourhood); only the small ones are kept.
    let mut seen = vec![false; w * h];
    let mut stack: Vec<u32> = Vec::new();
    let mut specks: Vec<(Vec<u32>, [usize; 4])> = Vec::new();
    for start in 0..w * h {
        if seen[start] || !ink[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start as u32);
        let mut count = 0usize;
        let mut pixels: Vec<u32> = Vec::new();
        let mut bbox = [usize::MAX, usize::MAX, 0usize, 0usize];
        while let Some(p) = stack.pop() {
            let p = p as usize;
            count += 1;
            if count <= max_area {
                pixels.push(p as u32);
            }
            let (x, y) = (p % w, p / w);
            bbox = [bbox[0].min(x), bbox[1].min(y), bbox[2].max(x), bbox[3].max(y)];
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let q = ny * w + nx;
                    if !seen[q] && ink[q] {
                        seen[q] = true;
                        stack.push(q as u32);
                    }
                }
            }
        }
        if count <= max_area {
            specks.push((pixels, bbox));
        }
    }
    if specks.is_empty() {
        return 0;
    }

    // Summed-area table of the ink: "no other ink within `gap`" is four lookups.
    let sw = w + 1;
    let mut sat = vec![0u32; sw * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(ink[y * w + x]);
            sat[(y + 1) * sw + x + 1] = sat[y * sw + x + 1] + row;
        }
    }
    let ink_in = |x0: usize, y0: usize, x1: usize, y1: usize| -> u32 {
        // [x0, x1) × [y0, y1)
        sat[y1 * sw + x1] + sat[y0 * sw + x0] - sat[y0 * sw + x1] - sat[y1 * sw + x0]
    };

    let colour = paper(r, &l, thr);
    let paper_luma = if r.n == 1 { u32::from(colour[0]) } else { (299 * u32::from(colour[0]) + 587 * u32::from(colour[1]) + 114 * u32::from(colour[2])) / 1000 };
    let n = r.n;
    let mut removed = 0;
    for (pixels, bb) in specks {
        let x0 = bb[0].saturating_sub(gap);
        let y0 = bb[1].saturating_sub(gap);
        let x1 = (bb[2] + 1 + gap).min(w);
        let y1 = (bb[3] + 1 + gap).min(h);
        if ink_in(x0, y0, x1, y1) as usize != pixels.len() {
            continue;
        }
        // The speck and its soft edge (a pixel around the box, as dark as noise).
        let (ex0, ey0, ex1, ey1) = (bb[0].saturating_sub(1), bb[1].saturating_sub(1), (bb[2] + 2).min(w), (bb[3] + 2).min(h));
        for y in ey0..ey1 {
            for x in ex0..ex1 {
                if u32::from(l[y * w + x]) + 10 < paper_luma {
                    r.px[(y * w + x) * n..(y * w + x + 1) * n].copy_from_slice(&colour);
                }
            }
        }
        removed += 1;
    }
    removed
}

/// How sharply the dark pixels of `r` line up in rows once the page is turned by `deg` (clockwise
/// positive, about its centre): the variance of the row totals. Higher is straighter. Works on a
/// reduced copy, so it is cheap enough to try a few angles.
pub fn row_sharpness(r: &Raster, deg: f32) -> f64 {
    let l = r.luma();
    let thr = otsu(&l);
    let k = (r.w.max(r.h) / 700).max(1);
    let (sw, sh) = (r.w / k, r.h / k);
    if sw < 8 || sh < 8 {
        return 0.0;
    }
    let (s, c) = f64::from(deg).to_radians().sin_cos();
    let (cx, cy) = (sw as f64 / 2.0, sh as f64 / 2.0);
    let span = (sw + sh) as usize;
    let mut rows = vec![0f64; span * 2 + 1];
    for y in 0..sh {
        for x in 0..sw {
            // a cell is ink when a fair share of its pixels are
            let mut dark = 0;
            for j in 0..k {
                for i in 0..k {
                    if l[(y * k + j) * r.w + x * k + i] < thr {
                        dark += 1;
                    }
                }
            }
            if dark * 6 < k * k {
                continue;
            }
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            let yy = -s * dx + c * dy;
            let idx = (yy.round() as i64 + span as i64).clamp(0, rows.len() as i64 - 1) as usize;
            rows[idx] += 1.0;
        }
    }
    let total: f64 = rows.iter().sum();
    if total == 0.0 {
        return 0.0;
    }
    rows.iter().map(|v| v * v).sum::<f64>() / total
}

/// Packs a gray raster as 1 bit per pixel, rows padded to bytes, 1 = white (pixels at or above
/// `thr`). Returns the bytes and the row length.
pub fn pack_bilevel(l: &[u8], w: usize, h: usize, thr: u8) -> (Vec<u8>, usize) {
    let stride = w.div_ceil(8);
    let mut out = vec![0u8; stride * h];
    for y in 0..h {
        for x in 0..w {
            if l[y * w + x] >= thr {
                out[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    (out, stride)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page of text-like bars: `lines` rows of short dark dashes, drawn along a baseline tilted by `deg`.
    fn text_page(w: usize, h: usize, deg: f64) -> Raster {
        let mut r = Raster::filled(w, h, 1, &[255]);
        let (s, c) = deg.to_radians().sin_cos();
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        for line in 0..14 {
            let ly = 60.0 + line as f64 * 28.0 - cy;
            let mut x = -(w as f64) / 2.0 + 40.0;
            while x < w as f64 / 2.0 - 40.0 {
                let word = 10.0 + ((line * 7) as i64 + (x as i64).div_euclid(13)).rem_euclid(4) as f64 * 6.0;
                for dx in 0..word as i64 {
                    for dy in -4..=4 {
                        let (fx, fy) = (x + dx as f64, ly + dy as f64);
                        // frame -> page: turn clockwise by deg
                        let (px, py) = (cx + fx * c - fy * s, cy + fx * s + fy * c);
                        if px >= 0.0 && py >= 0.0 && (px as usize) < w && (py as usize) < h {
                            r.px[py as usize * w + px as usize] = 0;
                        }
                    }
                }
                x += word + 9.0;
            }
        }
        r
    }

    fn straighten(r: &Raster, deg: f64) -> Raster {
        // dst pixel -> src pixel: turn about the centre by +deg (the page's displayed skew)
        let (s, c) = deg.to_radians().sin_cos();
        let (cx, cy) = (r.w as f64 / 2.0, r.h as f64 / 2.0);
        let m = Matrix([c, s, -s, c, cx - cx * c + cy * s, cy - cx * s - cy * c]);
        warp(r, &m, &[255])
    }

    #[test]
    fn otsu_splits_dark_from_light() {
        let mut l = vec![250u8; 900];
        l.extend(vec![20u8; 100]);
        let t = otsu(&l);
        assert!((21..=250).contains(&t), "threshold {t}");
    }

    #[test]
    fn rows_sharpen_at_the_right_angle() {
        let page = text_page(900, 520, 2.0);
        let (flat, right, wrong) = (row_sharpness(&page, 0.0), row_sharpness(&page, 2.0), row_sharpness(&page, -2.0));
        assert!(right > flat * 1.05, "right {right} flat {flat}");
        assert!(right > wrong * 1.05, "right {right} wrong {wrong}");
    }

    #[test]
    fn warp_straightens_a_tilted_page() {
        let page = text_page(900, 520, 2.0);
        let fixed = straighten(&page, 2.0);
        assert!(row_sharpness(&fixed, 0.0) > row_sharpness(&page, 0.0) * 1.05);
        // and the identity changes nothing
        let same = warp(&page, &Matrix::IDENTITY, &[255]);
        assert_eq!(same.px, page.px);
    }

    #[test]
    fn despeckle_removes_isolated_dots_and_keeps_text_dots() {
        let mut r = Raster::filled(200, 100, 1, &[255]);
        let put = |r: &mut Raster, x: usize, y: usize, w: usize, h: usize| {
            for yy in y..y + h {
                for xx in x..x + w {
                    r.px[yy * 200 + xx] = 0;
                }
            }
        };
        put(&mut r, 20, 20, 1, 1); // speck
        put(&mut r, 150, 70, 2, 1); // speck (2 pixels: the limit at 200 dpi)
        put(&mut r, 60, 30, 3, 30); // a stroke (big)
        put(&mut r, 65, 28, 1, 1); // a dot close to the stroke: part of the text
        put(&mut r, 100, 50, 12, 12); // a blob
        let removed = despeckle(&mut r, 200.0);
        assert_eq!(removed, 2);
        assert_eq!(r.px[20 * 200 + 20], 255);
        assert_eq!(r.px[70 * 200 + 150], 255);
        assert_eq!(r.px[28 * 200 + 65], 0, "the dot beside the stroke stays");
        assert_eq!(r.px[35 * 200 + 61], 0);
        assert_eq!(r.px[55 * 200 + 105], 0);
    }

    #[test]
    fn despeckle_leaves_a_dark_page_alone() {
        let mut r = Raster::filled(64, 64, 1, &[10]);
        r.px[5] = 200;
        assert_eq!(despeckle(&mut r, 200.0), 0);
    }

    #[test]
    fn bilevel_packing_is_one_bit_white() {
        let l = [0u8, 255, 0, 255, 255, 0, 0, 0, 255, 0];
        let (b, stride) = pack_bilevel(&l, 10, 1, 128);
        assert_eq!(stride, 2);
        assert_eq!(b, vec![0b0101_1000, 0b1000_0000]);
    }
}
