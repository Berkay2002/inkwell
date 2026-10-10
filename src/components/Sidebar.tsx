import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { OutlineItem } from "../lib/api";
import { computeLayout, previewScaleKey, rotated, typicalSize, visibleRange } from "../lib/layout";
import { type Want, tileClient } from "../lib/tiles";
import { type DocState, useStore } from "../store";
import { Icon } from "./Icons";
import { TileCanvas } from "./TileCanvas";

const THUMB_W = 150;

export function Sidebar({ doc }: { doc: DocState }) {
  const panel = useStore((s) => s.sidebar);
  if (panel === "none") return null;
  return (
    <aside className="sidebar">
      <div className="sidebar-tabs">
        <PanelTab name="thumbs" label="Pages" />
        <PanelTab name="outline" label="Outline" />
      </div>
      {panel === "thumbs" ? <Thumbnails key={doc.id} doc={doc} /> : <Outline key={doc.id} doc={doc} />}
    </aside>
  );
}

function PanelTab({ name, label }: { name: "thumbs" | "outline"; label: string }) {
  const on = useStore((s) => s.sidebar === name);
  return (
    <button className={on ? "seg on" : "seg"} onClick={() => useStore.getState().setSidebar(name)}>
      {label}
    </button>
  );
}

function Thumbnails({ doc }: { doc: DocState }) {
  const { id, sizes, rot, currentPage } = doc;
  const el = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, h: 0 });
  const zoom = THUMB_W / typicalSize(sizes, rot)[0];
  const layout = useMemo(() => computeLayout(sizes, rot, zoom, 0), [sizes, rot, zoom]);

  useLayoutEffect(() => {
    const s = el.current!;
    const sync = () => setView({ top: s.scrollTop, h: s.clientHeight });
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(s);
    return () => {
      ro.disconnect();
      tileClient.clear(1);
    };
  }, []);

  // Keep the current page's thumbnail in view when the main view moves.
  useEffect(() => {
    const s = el.current;
    if (!s) return;
    const top = layout.tops[currentPage];
    const bottom = top + layout.heights[currentPage];
    if (top < s.scrollTop || bottom > s.scrollTop + s.clientHeight) {
      s.scrollTop = top - (s.clientHeight - layout.heights[currentPage]) / 2;
    }
    // only when the page changes, not on every layout tweak
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentPage]);

  const { tops, heights, widths, total } = layout;
  const [vf, vl] = view.h > 0 ? visibleRange(tops, heights, view.top - view.h / 2, view.top + view.h * 1.5) : [0, -1];
  const items: React.ReactNode[] = [];
  const wanted: { d: number; w: Want }[] = [];
  const mid = view.top + view.h / 2;
  for (let i = vf; i <= vl; i++) {
    const size = rotated(sizes[i], rot);
    const tile = { page: i, rot, scale: previewScaleKey(size), tx: 0, ty: 0 };
    wanted.push({ d: Math.abs(tops[i] + heights[i] / 2 - mid), w: { doc: id, tile } });
    items.push(
      <button
        key={i}
        className={i === currentPage ? "thumb on" : "thumb"}
        style={{ top: tops[i], left: (THUMB_W + 40 - widths[i]) / 2, width: widths[i], height: heights[i] }}
        onClick={() => useStore.getState().goTo(id, i)}
        title={`Page ${i + 1}`}
      >
        <TileCanvas docId={id} tile={tile} size={size} zoom={zoom} stream={1} cover />
        <span className="thumb-num">{i + 1}</span>
      </button>,
    );
  }
  wanted.sort((a, b) => a.d - b.d);
  useEffect(
    () =>
      tileClient.want(
        1,
        wanted.map((x) => x.w),
      ),
  );

  return (
    <div className="thumbs" ref={el} onScroll={() => setView({ top: el.current!.scrollTop, h: el.current!.clientHeight })}>
      <div style={{ position: "relative", height: total, width: THUMB_W + 40 }}>{items}</div>
    </div>
  );
}

function Outline({ doc }: { doc: DocState }) {
  useEffect(() => useStore.getState().loadOutline(doc.id), [doc.id]);
  if (!doc.outline) return <div className="empty">Loading…</div>;
  if (doc.outline.length === 0) return <div className="empty">This document has no outline.</div>;
  return (
    <div className="outline">
      {doc.outline.map((o, i) => (
        <OutlineNode key={i} item={o} doc={doc.id} depth={0} />
      ))}
    </div>
  );
}

const OutlineNode = memo(function OutlineNode({ item, doc, depth }: { item: OutlineItem; doc: number; depth: number }) {
  const [open, setOpen] = useState(depth === 0);
  const has = item.children.length > 0;
  return (
    <div>
      <div className="outline-row" style={{ paddingLeft: 8 + depth * 14 }}>
        <button className={open ? "twisty open" : "twisty"} style={{ visibility: has ? "visible" : "hidden" }} onClick={() => setOpen(!open)}>
          <Icon name="right" size={12} />
        </button>
        <button
          className="outline-title"
          disabled={item.page == null}
          onClick={() => item.page != null && useStore.getState().goTo(doc, item.page)}
          title={item.title}
        >
          {item.title || "(untitled)"}
        </button>
      </div>
      {open && item.children.map((c, i) => <OutlineNode key={i} item={c} doc={doc} depth={depth + 1} />)}
    </div>
  );
});
