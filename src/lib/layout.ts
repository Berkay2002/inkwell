// Pure layout math for the continuous page list. No DOM, no React: unit-tested in layout.test.ts.

export const TILE = 512;
export const PAGE_GAP = 14;
export const PAD = 20;
/** Longest edge, in pixels, of the low-resolution preview that doubles as the thumbnail. */
export const PREVIEW_EDGE = 320;

export type Size = [number, number];
export type Rot = 0 | 1 | 2 | 3;

/** Page size in points as displayed, i.e. with the view rotation applied. */
export function rotated(s: Size, rot: Rot): Size {
  return rot % 2 === 0 ? s : [s[1], s[0]];
}

export interface Layout {
  /** Top of each page in CSS px, plus a final entry for the total height. */
  tops: number[];
  widths: number[];
  heights: number[];
  contentWidth: number;
  total: number;
}

export function computeLayout(sizes: Size[], rot: Rot, zoom: number, viewportW: number): Layout {
  const n = sizes.length;
  const tops = new Array<number>(n + 1);
  const widths = new Array<number>(n);
  const heights = new Array<number>(n);
  let y = PAD;
  let maxW = 0;
  for (let i = 0; i < n; i++) {
    const [w, h] = rotated(sizes[i], rot);
    widths[i] = w * zoom;
    heights[i] = h * zoom;
    tops[i] = y;
    y += heights[i] + PAGE_GAP;
    if (widths[i] > maxW) maxW = widths[i];
  }
  tops[n] = y - PAGE_GAP + PAD;
  return { tops, widths, heights, contentWidth: Math.max(viewportW, maxW + 2 * PAD), total: tops[n] };
}

/** Index of the page containing (or just after) content y. */
export function pageAt(tops: number[], y: number): number {
  let lo = 0;
  let hi = tops.length - 2;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (tops[mid] <= y) lo = mid;
    else hi = mid - 1;
  }
  return Math.max(0, lo);
}

/** Inclusive range of pages intersecting [top, bottom] in content px. */
export function visibleRange(tops: number[], heights: number[], top: number, bottom: number): [number, number] {
  const n = heights.length;
  if (n === 0) return [0, -1];
  let first = pageAt(tops, top);
  if (tops[first] + heights[first] < top && first < n - 1) first++;
  let last = pageAt(tops, bottom);
  if (last < first) last = first;
  return [first, last];
}

/** The page whose area covers the viewport centre line most: the "current page". */
export function currentPage(tops: number[], heights: number[], scrollTop: number, viewportH: number): number {
  const mid = scrollTop + viewportH / 2;
  const i = pageAt(tops, mid);
  // In the gap between two pages, prefer the one below the line.
  return mid > tops[i] + heights[i] && i < heights.length - 1 ? i + 1 : i;
}

/** Most common page size (rounded to whole points), used by fit-width / fit-page. */
export function typicalSize(sizes: Size[], rot: Rot): Size {
  if (sizes.length === 0) return [612, 792];
  const counts = new Map<string, { n: number; s: Size }>();
  let best: { n: number; s: Size } | undefined;
  for (const raw of sizes) {
    const s = rotated(raw, rot);
    const k = `${Math.round(s[0])}x${Math.round(s[1])}`;
    const e = counts.get(k) ?? { n: 0, s };
    e.n++;
    counts.set(k, e);
    if (!best || e.n > best.n) best = e;
  }
  return best!.s;
}

/** Render scale (device px per point), snapped to quarter-octaves so tiles are reused across zoom steps. */
export function bucketScale(zoom: number, dpr: number): number {
  const s = Math.min(16, Math.max(0.1, zoom * dpr));
  return Math.pow(2, Math.round(Math.log2(s) * 4) / 4);
}

export const scaleKey = (s: number) => Math.round(s * 1000);

/** Scale (thousandths) of the page preview / thumbnail: whole page in a single tile. */
export function previewScaleKey(size: Size): number {
  const edge = Math.max(size[0], size[1]);
  return Math.max(1, Math.floor((PREVIEW_EDGE / edge) * 1000));
}

export interface TileRef {
  page: number;
  rot: Rot;
  scale: number; // thousandths
  tx: number;
  ty: number;
  /** The document's edit generation; absent means 0 (never edited). */
  rev?: number;
}

/**
 * Tiles of one page needed to cover `view` (page-local CSS px rectangle) at `scale` px per point,
 * for a page shown at `zoom`. Ordered nearest-to-centre first.
 */
export function tilesForRect(
  page: number,
  rot: Rot,
  size: Size, // displayed size in points
  zoom: number,
  scale: number,
  view: { x0: number; y0: number; x1: number; y1: number },
  rev = 0,
): TileRef[] {
  const k = scale / zoom; // device px per CSS px
  const wPx = Math.ceil(size[0] * scale);
  const hPx = Math.ceil(size[1] * scale);
  const x0 = Math.max(0, Math.floor((view.x0 * k) / TILE));
  const y0 = Math.max(0, Math.floor((view.y0 * k) / TILE));
  const x1 = Math.min(Math.ceil(wPx / TILE) - 1, Math.floor((view.x1 * k) / TILE));
  const y1 = Math.min(Math.ceil(hPx / TILE) - 1, Math.floor((view.y1 * k) / TILE));
  const out: TileRef[] = [];
  const cx = ((view.x0 + view.x1) / 2) * k;
  const cy = ((view.y0 + view.y1) / 2) * k;
  for (let ty = y0; ty <= y1; ty++) {
    for (let tx = x0; tx <= x1; tx++) out.push({ page, rot, scale: scaleKey(scale), tx, ty, ...(rev ? { rev } : {}) });
  }
  const dist = (t: TileRef) => Math.hypot((t.tx + 0.5) * TILE - cx, (t.ty + 0.5) * TILE - cy);
  return out.sort((a, b) => dist(a) - dist(b));
}

export const tileKeyStr = (doc: number, t: TileRef) => `${doc}/${t.page}/${t.rot}/${t.scale}/${t.tx}/${t.ty}/${t.rev ?? 0}`;

/** CSS transform mapping an unrotated page-space box (points) onto the rotated page at `zoom`. */
export function pageSpaceTransform(size: Size, rot: Rot, zoom: number): string {
  const [w, h] = size;
  const t = [`translate(0px,0px)`, `translate(${h * zoom}px,0px)`, `translate(${w * zoom}px,${h * zoom}px)`, `translate(0px,${w * zoom}px)`][rot];
  return `${t} rotate(${rot * 90}deg) scale(${zoom})`;
}

export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
export const ZOOM_MIN = 0.1;
export const ZOOM_MAX = 16;
export const ZOOM_STEPS = [0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8, 12, 16];

export function stepZoom(z: number, dir: 1 | -1): number {
  if (dir > 0) return ZOOM_STEPS.find((s) => s > z * 1.001) ?? ZOOM_MAX;
  return [...ZOOM_STEPS].reverse().find((s) => s < z / 1.001) ?? ZOOM_MIN;
}
