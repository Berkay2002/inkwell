import { memo, useEffect, useRef, useState } from "react";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import { type Rect4, dragRect, targetLabel, useNav } from "../../lib/nav";
import { useTextEdit } from "../../lib/textEdit";

interface Props {
  docId: number;
  page: number;
  /** Document generation: the page's links are listed again when it changes. */
  rev: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
  /** Near the viewport: only these pages list their links. */
  visible: boolean;
}

const nav = () => useNav.getState();
const MIN_SIDE = 6;

type Corner = "nw" | "ne" | "sw" | "se";
type Drag = { mode: "move" | Corner; start: [number, number]; d: [number, number] };

/** Edit mode's links layer on one page: link outlines, the one being drawn, the selected one with handles. */
export const LinkLayer = memo(function LinkLayer(p: Props) {
  const open = useTextEdit((s) => s.open);
  const section = useTextEdit((s) => s.section);
  return open && section === "links" ? <Layer {...p} /> : null;
});

function Layer({ docId, page, rev, rot, rawSize, zoom, visible }: Props) {
  const tool = useNav((s) => s.linkTool);
  const listed = useNav((s) => s.docs[docId]?.links[page]);
  const selected = useNav((s) => s.docs[docId]?.selectedLink ?? null);
  const pending = useNav((s) => s.docs[docId]?.pending ?? null);
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [draw, setDraw] = useState<{ a: [number, number]; b: [number, number] } | null>(null);

  useEffect(() => {
    if (visible) void nav().loadLinks(docId, page);
  }, [docId, page, rev, visible]);

  const toPage = (e: { clientX: number; clientY: number }): [number, number] => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = space.current!.parentElement!.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [Math.min(Math.max(q.x, 0), rawSize[0]), Math.min(Math.max(q.y, 0), rawSize[1])];
  };

  const mine = selected && selected.page === page ? (listed?.links.find((k) => k.id === selected.id) ?? null) : null;

  // Delete removes the selected link, unless a field is being typed in.
  useEffect(() => {
    if (!mine) return;
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Delete" && e.key !== "Backspace") return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      // Delete in the bookmark tree, or anywhere in the Edit rail, belongs to what is selected there.
      if (t?.closest?.(".bm-list, .te-dock")) return;
      e.preventDefault();
      void nav().removeLinks(docId, page, [mine.id]);
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [mine, docId, page]);

  const startDrag = (e: React.PointerEvent, mode: Drag["mode"]) => {
    e.stopPropagation();
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ mode, start: toPage(e), d: [0, 0] });
  };
  const dragTo = (e: React.PointerEvent) => {
    if (!drag) return;
    const q = toPage(e);
    setDrag({ ...drag, d: [q[0] - drag.start[0], q[1] - drag.start[1]] });
  };
  const framed = (r: Rect4): Rect4 => {
    if (!drag) return r;
    const [dx, dy] = drag.d;
    if (drag.mode === "move") {
      const w = r[2] - r[0];
      const h = r[3] - r[1];
      const x = Math.min(Math.max(r[0] + dx, 0), rawSize[0] - w);
      const y = Math.min(Math.max(r[1] + dy, 0), rawSize[1] - h);
      return [x, y, x + w, y + h];
    }
    const next: Rect4 = [...r];
    if (drag.mode.includes("w")) next[0] = Math.min(r[2] - MIN_SIDE, r[0] + dx);
    else next[2] = Math.max(r[0] + MIN_SIDE, r[2] + dx);
    if (drag.mode.includes("n")) next[1] = Math.min(r[3] - MIN_SIDE, r[1] + dy);
    else next[3] = Math.max(r[1] + MIN_SIDE, r[3] + dy);
    return next;
  };
  const dragEnd = () => {
    const g = drag;
    setDrag(null);
    if (!g || !mine || Math.hypot(g.d[0], g.d[1]) * zoom < 3) return;
    void nav().setLinkRect(docId, page, mine.id, framed(mine.rect));
  };

  // ---- drawing a new link ----
  const drawDown = (e: React.PointerEvent) => {
    e.stopPropagation();
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    const a = toPage(e);
    setDraw({ a, b: a });
  };
  const drawMove = (e: React.PointerEvent) => draw && setDraw({ ...draw, b: toPage(e) });
  const drawEnd = () => {
    const d = draw;
    setDraw(null);
    if (!d) return;
    const r = dragRect(d.a, d.b, MIN_SIDE);
    if (r) {
      nav().setPending(docId, { page, rect: r });
      nav().setLinkTool("select");
    }
  };

  return (
    <div
      ref={space}
      className="te-space lk-space"
      style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
    >
      {tool === "draw" && (
        <div className="lk-capture" onPointerDown={drawDown} onPointerMove={drawMove} onPointerUp={drawEnd} onPointerCancel={() => setDraw(null)} />
      )}
      {tool === "select" && (selected || pending) && (
        <div
          className="te-bg"
          onPointerDown={(e) => {
            e.stopPropagation();
            if (pending) nav().setPending(docId, null);
            nav().selectLink(docId, null);
          }}
        />
      )}
      {listed?.links.map((k) =>
        mine && k.id === mine.id ? null : (
          <div
            key={k.id}
            className={k.border.visible ? "lk-rect shown" : "lk-rect"}
            style={box(k.rect)}
            title={targetLabel(k.target)}
            onPointerDown={(e) => {
              if (tool !== "select") return;
              e.stopPropagation();
              e.preventDefault();
              nav().selectLink(docId, { page, id: k.id });
            }}
          />
        ),
      )}
      {mine && (
        <div className="lk-sel" style={box(framed(mine.rect))} onPointerDown={(e) => startDrag(e, "move")} onPointerMove={dragTo} onPointerUp={dragEnd}>
          {(["nw", "ne", "sw", "se"] as Corner[]).map((c) => (
            <span
              key={c}
              className={`lk-h ${c}`}
              onPointerDown={(e) => startDrag(e, c)}
              onPointerMove={dragTo}
              onPointerUp={dragEnd}
              aria-label={`Resize ${c}`}
            />
          ))}
        </div>
      )}
      {pending && pending.page === page && <div className="lk-rect pending" style={box(pending.rect)} />}
      {draw && (() => {
        const r = dragRect(draw.a, draw.b, 0) ?? [draw.a[0], draw.a[1], draw.a[0], draw.a[1]];
        return <div className="lk-rect pending" style={box(r as Rect4)} />;
      })()}
    </div>
  );
}

function box(r: Rect4): React.CSSProperties {
  return { left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] };
}
