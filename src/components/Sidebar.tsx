import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { OutlineItem } from "../lib/api";
import { computeLayout, previewScaleKey, rotated, type Size, visibleRange } from "../lib/layout";
import { type Want, tileClient } from "../lib/tiles";
import { useTextEdit } from "../lib/textEdit";
import { useToolPrefs } from "../lib/toolPrefs";
import { useTools } from "../lib/toolsStore";
import { type DocState, pageLabel, type SidebarPanel, useStore } from "../store";
import { AllToolsPanel } from "./AllTools";
import { AttachmentsPanel } from "./AttachmentsPanel";
import { ArticlesPanel } from "./threads/ArticlesPanel";
import { MediaPanel } from "./MediaPanel";
import { Icon, type IconName } from "./Icons";
import { ContextMenu, pageActions } from "./PageMenu";
import { TileCanvas } from "./TileCanvas";
import { BookmarkBar, BookmarkTree } from "./edit/BookmarkTree";

const THUMB_W = 150;

export function Sidebar({ doc }: { doc: DocState }) {
  const panel = useStore((s) => s.sidebar);
  const tools = useToolPrefs((s) => s.panelOpen);
  // the All tools panel takes the slot while it is open, whatever the sidebar was showing
  if (tools) return <AllToolsPanel doc={doc} />;
  if (panel === "none") return null;
  return (
    <aside className="sidebar">
      <div className="sidebar-tabs">
        <PanelTab name="thumbs" label="Pages" />
        <PanelTab name="outline" label="Outline" />
        <PanelTab name="attachments" label="Attachments" icon="paperclip" />
        <PanelTab name="articles" label="Articles" icon="article" />
        <PanelTab name="media" label="Media" icon="play" />
      </div>
      {panel === "thumbs" ? (
        <Thumbnails key={doc.id} doc={doc} />
      ) : panel === "outline" ? (
        <Outline key={doc.id} doc={doc} />
      ) : panel === "articles" ? (
        <ArticlesPanel key={doc.id} doc={doc} />
      ) : panel === "attachments" ? (
        <AttachmentsPanel key={doc.id} doc={doc} />
      ) : (
        <MediaPanel key={doc.id} doc={doc} />
      )}
    </aside>
  );
}

function PanelTab({ name, label, icon }: { name: Exclude<SidebarPanel, "none">; label: string; icon?: IconName }) {
  const on = useStore((s) => s.sidebar === name);
  return (
    <button
      className={on ? "seg on" : "seg"}
      style={icon ? { flex: "0 0 30px", padding: 0 } : undefined}
      title={icon ? label : undefined}
      aria-label={icon ? label : undefined}
      onClick={() => useStore.getState().setSidebar(name)}
    >
      {icon ? <Icon name={icon} size={16} filled={on} /> : label}
    </button>
  );
}

function Thumbnails({ doc }: { doc: DocState }) {
  const { id, sizes, rot, currentPage, rev } = doc;
  const el = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, h: 0 });
  const [menu, setMenu] = useState<{ x: number; y: number; page: number } | null>(null);
  // Pages a running (or just finished) OCR job has read.
  const done = useTools((s) => (s.sheet?.tool === "ocr" && s.sheet.doc === id ? s.sheet.donePages : null));
  const ticked = useMemo(() => (done ? new Set(done) : null), [done]);
  // Every page fits the column width (a landscape page among portrait ones too).
  const fitted = useMemo(() => sizes.map((s) => rotated(s, rot)).map(([w, h]) => [THUMB_W, (h * THUMB_W) / w] as Size), [sizes, rot]);
  const layout = useMemo(() => computeLayout(fitted, 0, 1, 0), [fitted]);

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
    const tile = { page: i, rot, scale: previewScaleKey(size), tx: 0, ty: 0, ...(rev ? { rev } : {}) };
    wanted.push({ d: Math.abs(tops[i] + heights[i] / 2 - mid), w: { doc: id, tile } });
    items.push(
      <button
        key={i}
        className={i === currentPage ? "thumb on" : "thumb"}
        style={{ top: tops[i], left: (THUMB_W + 40 - widths[i]) / 2, width: widths[i], height: heights[i] }}
        onClick={() => useStore.getState().goTo(id, i)}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY, page: i });
        }}
        title={doc.labels[i] ? `Page ${i + 1} (${doc.labels[i]})` : `Page ${i + 1}`}
      >
        <TileCanvas docId={id} tile={tile} size={size} zoom={THUMB_W / size[0]} stream={1} cover />
        <span className="thumb-num">{pageLabel(doc, i)}</span>
        {ticked?.has(i) && (
          <span className="thumb-tick" aria-label="Recognized">
            <Icon name="check" size={12} />
          </span>
        )}
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
      {menu && menu.page < doc.pageCount && (
        <ContextMenu x={menu.x} y={menu.y} items={pageActions(doc, [menu.page])} onClose={() => setMenu(null)} />
      )}
    </div>
  );
}

/** In Edit mode the outline is the bookmark editor (same store as the Edit rail's Bookmarks tab). */
function EditableOutline({ doc }: { doc: DocState }) {
  return (
    <div className="outline outline-edit">
      <BookmarkBar doc={doc.id} />
      <BookmarkTree doc={doc.id} />
      <p className="dim te-hint">Double-click to rename, drag to reorder or nest, right-click for more.</p>
    </div>
  );
}

function Outline({ doc }: { doc: DocState }) {
  const editing = useTextEdit((s) => s.open) && !doc.organize;
  if (editing) return <EditableOutline doc={doc} />;
  return <ReadOutline doc={doc} />;
}

function ReadOutline({ doc }: { doc: DocState }) {
  useEffect(() => useStore.getState().loadOutline(doc.id), [doc.id, doc.rev]);
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
