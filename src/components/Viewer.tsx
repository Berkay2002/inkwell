import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  PAD,
  type TileRef,
  anchoredScroll,
  bucketScale,
  clamp,
  computeLayout,
  currentPage,
  fitZoom,
  pageAt,
  previewScaleKey,
  rotated,
  scaleKey,
  tilesForRect,
  typicalSize,
  visibleRange,
} from "../lib/layout";
import { useA11y } from "../lib/a11y";
import { usePreflight } from "../lib/preflight";
import { type Mark, useRedact } from "../lib/redact";
import { type Want, tileClient } from "../lib/tiles";
import { setViewPos } from "../lib/viewPos";
import { type DocState, type Rect4, useStore } from "../store";
import { PageView } from "./PageView";

/** What the viewport is anchored to, so zoom / resize / late page sizes don't move the content. */
interface Anchor {
  page: number;
  yPt: number;
  vy: number;
  xFrac: number;
  vx: number;
}

/** Extra CSS px around the viewport for which tiles are prefetched. */
const MARGIN = 256;
const EMPTY: never[] = [];
const NO_MARKS: Mark[] = [];

export function Viewer({ doc }: { doc: DocState }) {
  const { id, sizes, rot, zoom, zoomMode, rev } = doc;
  const el = useRef<HTMLDivElement>(null);
  const [vp, setVp] = useState({ w: 0, h: 0 });
  const [pos, setPos] = useState({ top: 0, left: 0 });
  const dpr = window.devicePixelRatio || 1;

  // Tiles are requested at the zoom the user settled on; while zooming they are only restretched.
  const [settled, setSettled] = useState(zoom);
  useEffect(() => {
    const t = setTimeout(() => setSettled(zoom), 140);
    return () => clearTimeout(t);
  }, [zoom]);

  const layout = useMemo(() => computeLayout(sizes, rot, zoom, vp.w), [sizes, rot, zoom, vp.w]);
  const cur = useRef({ layout, zoom, doc });
  cur.current = { layout, zoom, doc };

  const anchor = useRef<Anchor | null>(null);
  const saved = useRef(doc.scroll);

  const track = () => {
    const s = el.current!;
    const { layout: L, zoom: z } = cur.current;
    const at = (vy: number, vx: number): Anchor => {
      const y = s.scrollTop + vy;
      const page = pageAt(L.tops, y);
      return { page, yPt: (y - L.tops[page]) / z, vy, xFrac: (s.scrollLeft + vx) / L.contentWidth, vx };
    };
    anchor.current = at(s.clientHeight / 2, s.clientWidth / 2);
    const top = at(0, 0);
    saved.current = { page: top.page, yPt: top.yPt };
    // A jump to a page leaves the view PAD px above it, so the top of the view is still in the gap
    // after the previous page: that is the page "in view", not the bottom of the one before.
    const next = top.page + 1;
    const inGap = next < L.tops.length - 1 && L.tops[next] - s.scrollTop <= PAD + 1;
    setViewPos(cur.current.doc.id, inGap ? { page: next, yPt: 0 } : saved.current);
    return at;
  };

  useLayoutEffect(() => {
    const s = el.current!;
    setVp({ w: s.clientWidth, h: s.clientHeight });
    // The stage changes width with the window, the sidebar and the docked assistant. Re-anchor on
    // the top of the view (same page, same offset) for the new size, and refit in the same render
    // so no frame shows the old zoom in the new width.
    const ro = new ResizeObserver(() => {
      const w = s.clientWidth;
      const h = s.clientHeight;
      const { layout: L, zoom: z, doc: d } = cur.current;
      if (L.tops.length > 1) {
        const page = pageAt(L.tops, s.scrollTop);
        anchor.current = { page, yPt: (s.scrollTop - L.tops[page]) / z, vy: 0, xFrac: (s.scrollLeft + w / 2) / L.contentWidth, vx: w / 2 };
      }
      setVp({ w, h });
      const fz = fitZoom(d.zoomMode, typicalSize(d.sizes, d.rot), w, h);
      if (fz != null && Math.abs(fz - z) > 1e-3) useStore.getState().patch(id, { zoom: fz });
    });
    ro.observe(s);
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      const r = s.getBoundingClientRect();
      const a = track()(e.clientY - r.top, e.clientX - r.left);
      anchor.current = a;
      const st = useStore.getState();
      const d = st.docs[id];
      if (d) st.setZoom(id, d.zoom * Math.exp(-clamp(e.deltaY, -60, 60) * 0.004));
    };
    s.addEventListener("wheel", onWheel, { passive: false });
    return () => {
      ro.disconnect();
      s.removeEventListener("wheel", onWheel);
      tileClient.clear(0);
      useStore.getState().patch(id, { scroll: saved.current });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);

  // Fit modes follow the mode, the typical page size and the viewport (resizes refit in the observer).
  useLayoutEffect(() => {
    const z = fitZoom(zoomMode, typicalSize(sizes, rot), vp.w, vp.h);
    if (z != null && Math.abs(z - zoom) > 1e-3) useStore.getState().patch(id, { zoom: z });
  }, [id, zoomMode, sizes, rot, vp.w, vp.h, zoom]);

  // Keep the anchored content in place whenever the layout changes (zoom, rotation, late sizes).
  useLayoutEffect(() => {
    const s = el.current!;
    const a: Anchor = anchor.current ?? {
      page: Math.min(doc.scroll.page, Math.max(0, layout.tops.length - 2)),
      yPt: doc.scroll.yPt,
      vy: 0,
      xFrac: 0.5,
      vx: s.clientWidth / 2,
    };
    const to = anchoredScroll(layout, zoom, a, s.clientWidth, zoomMode !== "custom");
    s.scrollTop = to.top;
    s.scrollLeft = to.left;
    anchor.current = a;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [layout]);

  // Navigation requests (page jumps, search hits).
  const navSeen = useRef(doc.nav?.nonce);
  useLayoutEffect(() => {
    const n = doc.nav;
    if (!n || n.nonce === navSeen.current) return;
    navSeen.current = n.nonce;
    const y = n.y !== undefined && rot === 0 ? n.y * zoom : -PAD;
    const s = el.current!;
    s.scrollTop = layout.tops[n.page] + y;
    track();
  }, [doc.nav, layout, rot, zoom]);

  const onScroll = () => {
    const s = el.current!;
    track();
    setPos({ top: s.scrollTop, left: s.scrollLeft });
    const p = currentPage(layout.tops, layout.heights, s.scrollTop, s.clientHeight);
    if (p !== cur.current.doc.currentPage) useStore.getState().patch(id, { currentPage: p });
  };

  const hitsByPage = useMemo(() => {
    const m = new Map<number, Rect4[]>();
    for (const h of doc.search.hits) {
      const l = m.get(h.page);
      if (l) l.push(...h.rects);
      else m.set(h.page, [...h.rects]);
    }
    return m;
  }, [doc.search.hits]);
  const a11yOpen = useA11y((s) => s.open);
  const a11yShown = useA11y((s) => s.docs[id]?.shown) ?? null;
  const pfOpen = usePreflight((s) => s.open);
  const pfShown = usePreflight((s) => s.docs[id]?.shown) ?? null;
  const redactMarks = useRedact((s) => s.docs[id]?.marks);
  const marksByPage = useMemo(() => {
    const m = new Map<number, Mark[]>();
    for (const k of redactMarks ?? []) m.set(k.page, [...(m.get(k.page) ?? []), k]);
    return m;
  }, [redactMarks]);
  const currentHit = doc.search.hits[doc.search.current]?.rects ?? EMPTY;

  const stable = useRef(new Map<number, { sig: string; tiles: TileRef[] }>());
  const keepSame = (page: number, tiles: TileRef[]) => {
    const sig = tiles.map((t) => `${t.rot}/${t.scale}/${t.tx}/${t.ty}/${t.rev ?? 0}`).join("|");
    const e = stable.current.get(page);
    if (e?.sig === sig) return e.tiles;
    stable.current.set(page, { sig, tiles });
    return tiles;
  };

  const { tops, heights, widths, contentWidth, total } = layout;
  const vTop = pos.top;
  const vBot = pos.top + vp.h;
  // Zooming in lags behind (the stretched old tiles stand in); zooming out re-renders at once,
  // otherwise the visible area would be covered with far too many high-resolution tiles.
  const renderScale = scaleKey(bucketScale(Math.min(settled, zoom), dpr));
  const s = renderScale / 1000;
  const pages: React.ReactNode[] = [];
  const previews: Want[] = [];
  const farPreviews: Want[] = [];
  const real: { d: number; w: Want }[] = [];

  if (vp.h > 0 && sizes.length > 0) {
    const [vf, vl] = visibleRange(tops, heights, vTop, vBot);
    const [mf, ml] = visibleRange(tops, heights, vTop - vp.h, vBot + vp.h);
    const [tf, tl] = visibleRange(tops, heights, vTop - MARGIN, vBot + MARGIN);
    for (let i = mf; i <= ml; i++) {
      const size = rotated(sizes[i], rot);
      const left = (contentWidth - widths[i]) / 2;
      const tiles =
        i >= tf && i <= tl
          ? keepSame(
              i,
              tilesForRect(i, rot, size, zoom, s, {
                x0: pos.left - left - MARGIN,
                x1: pos.left + vp.w - left + MARGIN,
                y0: vTop - tops[i] - MARGIN,
                y1: vBot - tops[i] + MARGIN,
              }, rev),
            )
          : EMPTY;
      const preview: TileRef = { page: i, rot, scale: previewScaleKey(size), tx: 0, ty: 0, ...(rev ? { rev } : {}) };
      (i >= vf && i <= vl ? previews : farPreviews).push({ doc: id, tile: preview });
      const d = i < vf ? vf - i : i > vl ? i - vl : 0;
      for (const t of tiles) real.push({ d, w: { doc: id, tile: t } });
      pages.push(
        <PageView
          key={i}
          docId={id}
          page={i}
          rev={rev}
          rot={rot}
          size={size}
          rawSize={sizes[i]}
          zoom={zoom}
          top={tops[i]}
          left={left}
          tiles={tiles}
          scale={renderScale}
          preview={preview}
          wantText={i >= vf && i <= vl}
          hits={hitsByPage.get(i) ?? EMPTY}
          currentHit={currentHit}
          marks={marksByPage.get(i) ?? NO_MARKS}
          a11y={a11yOpen && a11yShown?.page === i ? a11yShown : pfOpen && pfShown?.page === i ? pfShown : null}
        />,
      );
    }
  }

  real.sort((a, b) => a.d - b.d);
  const wanted = [...previews, ...real.map((r) => r.w), ...farPreviews];
  useEffect(() => tileClient.want(0, wanted));

  return (
    <div ref={el} className="viewer" tabIndex={0} onScroll={onScroll}>
      <div className="pages" style={{ width: contentWidth, height: total }}>
        {pages}
      </div>
    </div>
  );
}
