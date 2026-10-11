import { useEffect, useRef } from "react";
import type { Rect4 } from "../store";

/** Tile canvases announce a repaint with this event (bubbling up to the page). */
export const TILE_DRAWN = "tiledrawn";

/** Largest side of the backing canvas: a photo that fills a page at 16x zoom stays within one texture. */
const MAX_SIDE = 4096;

/**
 * A picture on a dark page, shown in its own colours. The page's tiles are darkened by a CSS filter on their
 * canvases, which leaves the canvas pixels as rendered; this box copies those untouched pixels out of the tiles
 * (preview, previous scale, current tiles, in that order) and covers the filtered ones, so the colours are the
 * rendered ones exactly, not a second filter run backwards over a clamped 8-bit result.
 *
 * `rect` is in CSS px of the displayed page (the tiles' space): `PageView` passes it zoomed and rotated.
 * It repaints whenever a tile of the page repaints.
 */
export function NaturalImage({ rect }: { rect: Rect4 }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const [x0, y0, x1, y1] = rect;
  const w = x1 - x0;
  const h = y1 - y0;

  useEffect(() => {
    const canvas = ref.current!;
    const page = canvas.parentElement!;
    const ctx = canvas.getContext("2d")!;
    const k = Math.min(window.devicePixelRatio || 1, MAX_SIDE / Math.max(w, h, 1));
    const bw = Math.max(1, Math.round(w * k));
    const bh = Math.max(1, Math.round(h * k));
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    let raf = 0;
    const paint = () => {
      raf = 0;
      ctx.clearRect(0, 0, bw, bh);
      const origin = page.getBoundingClientRect();
      for (const t of page.querySelectorAll<HTMLCanvasElement>("canvas.tile")) {
        if (!t.dataset.ready || t.width === 0) continue;
        const b = t.getBoundingClientRect();
        const tl = b.left - origin.left;
        const tt = b.top - origin.top;
        // the overlap of this tile and the box, in page CSS px
        const ix0 = Math.max(x0, tl);
        const iy0 = Math.max(y0, tt);
        const ix1 = Math.min(x1, tl + b.width);
        const iy1 = Math.min(y1, tt + b.height);
        if (ix1 <= ix0 || iy1 <= iy0) continue;
        const sx = t.width / b.width;
        const sy = t.height / b.height;
        ctx.drawImage(t, (ix0 - tl) * sx, (iy0 - tt) * sy, (ix1 - ix0) * sx, (iy1 - iy0) * sy, (ix0 - x0) * k, (iy0 - y0) * k, (ix1 - ix0) * k, (iy1 - iy0) * k);
      }
    };
    const again = () => {
      if (!raf) raf = requestAnimationFrame(paint);
    };
    page.addEventListener(TILE_DRAWN, again);
    paint();
    return () => {
      page.removeEventListener(TILE_DRAWN, again);
      if (raf) cancelAnimationFrame(raf);
    };
  }, [x0, y0, x1, y1, w, h]);

  return <canvas ref={ref} className="nat-img" style={{ left: x0, top: y0, width: w, height: h }} />;
}
