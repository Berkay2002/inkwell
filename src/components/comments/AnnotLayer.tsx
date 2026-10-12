import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useAnnot, type Ref } from "../../lib/annot";
import {
  type Annot, type Handle, type Pt, type Rect4, COLORS, HANDLES, KIND_LABEL, clampRect, hitTest, isBoxed, moveRect,
  normRect, placePopover, rectToScreen, resizeRect, rgbHex, roots, stampSize, dateStampName,
} from "../../lib/annotLogic";
import { customId, fitAspect } from "../../lib/stampsLogic";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import { Icon } from "../Icons";

interface Props {
  docId: number;
  page: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
}

const EMPTY: Annot[] = [];
const DRAW_TOOLS = ["note", "freeText", "pen", "rectangle", "ellipse", "line", "arrow", "polygon", "stamp"];
const SHAPES = ["rectangle", "ellipse", "line", "arrow"];

type Drag = { mode: "move" | { resize: Handle }; start: Pt; rect0: Rect4; rect: Rect4; moved: boolean; ref: Ref };
type Draft = { kind: "box"; a: Pt; b: Pt } | { kind: "ink"; pts: Pt[] } | { kind: "poly"; pts: Pt[]; hover: Pt };

const an = () => useAnnot.getState();

/** Comment mode's layer on one page: hit areas, selection frame, drawing capture and the popover. */
export const AnnotLayer = memo(function AnnotLayer(p: Props) {
  const open = useAnnot((s) => s.open);
  return open ? <Layer {...p} /> : null;
});

function Layer({ docId, page, rot, rawSize, zoom }: Props) {
  const tool = useAnnot((s) => s.tool);
  const items = useAnnot((s) => s.docs[docId]?.items ?? EMPTY);
  const selected = useAnnot((s) => s.docs[docId]?.selected ?? null);
  const stamp = useAnnot((s) => s.stamp);
  const mine = useMemo(() => roots(items).filter((a) => a.page === page), [items, page]);
  const sel = selected && selected.page === page ? (mine.find((a) => a.xref === selected.xref) ?? null) : null;
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [box, setBox] = useState<Rect4 | null>(null);

  // A tool change or Esc abandons what is half drawn.
  useEffect(() => {
    setDraft(null);
    setBox(null);
  }, [tool]);
  useEffect(() => {
    if (!draft && !box) return;
    const k = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setDraft(null);
        setBox(null);
      } else if (e.key === "Enter" && draft?.kind === "poly") finishPoly(draft.pts);
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });

  const toPage = (e: { clientX: number; clientY: number }): Pt => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = space.current!.parentElement!.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [Math.min(Math.max(q.x, 0), rawSize[0]), Math.min(Math.max(q.y, 0), rawSize[1])];
  };

  const done = (keep = false) => {
    if (!keep) an().setTool("select");
  };
  const finishPoly = (pts: Pt[]) => {
    setDraft(null);
    if (pts.length >= 3) void an().add(docId, page, { type: "shape", shape: "polygon", points: pts });
    done();
  };

  // ---- select, move, resize ----
  const startDrag = (e: React.PointerEvent, a: Annot, mode: Drag["mode"]) => {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    if (!(selected && selected.xref === a.xref)) an().select(docId, { page, xref: a.xref });
    setDrag({ mode, start: toPage(e), rect0: a.rect, rect: a.rect, moved: false, ref: { page, xref: a.xref } });
  };
  const onDragMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const q = toPage(e);
    const [dx, dy] = [q[0] - drag.start[0], q[1] - drag.start[1]];
    let rect = drag.mode === "move" ? moveRect(drag.rect0, dx, dy) : resizeRect(drag.rect0, drag.mode.resize, dx, dy);
    // A picture stamp keeps its proportions while it is resized.
    if (drag.mode !== "move" && mine.find((a) => a.xref === drag.ref.xref)?.picture) {
      const [x0, y0, x1, y1] = drag.rect0;
      rect = fitAspect(drag.rect0, clampRect(rect, rawSize), (x1 - x0) / Math.max(1e-3, y1 - y0));
    }
    setDrag({ ...drag, rect, moved: drag.moved || Math.hypot(dx, dy) * zoom > 3 });
  };
  const onDragEnd = () => {
    const d = drag;
    setDrag(null);
    if (!d?.moved) return;
    const r = d.mode === "move" ? d.rect : clampRect(d.rect, rawSize);
    void an().edit(docId, d.ref.page, d.ref.xref, { rect: r });
  };

  // ---- drawing tools ----
  const down = (e: React.PointerEvent) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    const q = toPage(e);
    if (tool === "note") {
      void an()
        .add(docId, page, { type: "note", at: q, text: "" })
        .then((x) => x != null && an().setEditing(docId, { page, xref: x }));
      return done();
    }
    if (tool === "stamp") {
      const custom = customId(stamp);
      if (custom) {
        void an().placeStamp(docId, page, custom, q);
        return done();
      }
      const name = stamp === "Date" ? dateStampName() : stamp;
      const [w, h] = stampSize(name);
      const r = clampRect([q[0] - w / 2, q[1] - h / 2, q[0] + w / 2, q[1] + h / 2], rawSize);
      void an().add(docId, page, { type: "stamp", name, rect: r });
      return done();
    }
    if (tool === "polygon") {
      if (draft?.kind === "poly") {
        const first = draft.pts[0];
        if (draft.pts.length >= 3 && Math.hypot(q[0] - first[0], q[1] - first[1]) * zoom < 8) return finishPoly(draft.pts);
        return setDraft({ kind: "poly", pts: [...draft.pts, q], hover: q });
      }
      return setDraft({ kind: "poly", pts: [q], hover: q });
    }
    if (tool === "pen") return setDraft({ kind: "ink", pts: [q] });
    setDraft({ kind: "box", a: q, b: q });
  };
  const move = (e: React.PointerEvent) => {
    if (!draft) return;
    const q = toPage(e);
    if (draft.kind === "box") setDraft({ ...draft, b: q });
    else if (draft.kind === "ink") {
      const last = draft.pts[draft.pts.length - 1];
      if (Math.hypot(q[0] - last[0], q[1] - last[1]) >= 1) setDraft({ kind: "ink", pts: [...draft.pts, q] });
    } else setDraft({ ...draft, hover: q });
  };
  const up = () => {
    const d = draft;
    if (!d || d.kind === "poly") return;
    setDraft(null);
    if (d.kind === "ink") {
      if (d.pts.length >= 2) void an().add(docId, page, { type: "ink", strokes: [d.pts] });
      return;
    }
    const big = Math.hypot(d.b[0] - d.a[0], d.b[1] - d.a[1]) * zoom >= 6;
    if (tool === "freeText") {
      const r = big ? normRect(d.a, d.b) : ([d.a[0], d.a[1], d.a[0] + 160, d.a[1] + 36] as Rect4);
      return setBox(clampRect(r, rawSize));
    }
    if (big && SHAPES.includes(tool)) {
      void an().add(docId, page, { type: "shape", shape: tool as "rectangle", points: [d.a, d.b] });
      done();
    }
  };

  const frameRect = drag && sel && drag.ref.xref === sel.xref ? drag.rect : sel?.rect;
  const z = zoom;
  const style = (r: Rect4) => ({ left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] });

  return (
    <>
      <div
        ref={space}
        className="an-space"
        style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
      >
        {tool === "select" &&
          mine.map((a) => (
            <div
              key={a.xref}
              className={`an-hit${a.resolved ? " resolved" : ""}`}
              style={style(a.rect)}
              title={KIND_LABEL[a.kind]}
              onPointerDown={(e) => startDrag(e, a, "move")}
              onPointerMove={onDragMove}
              onPointerUp={onDragEnd}
              onDoubleClick={() => (a.kind === "note" || a.kind === "freeText") && an().setEditing(docId, { page, xref: a.xref })}
            />
          ))}
        {sel && frameRect && tool === "select" && (
          <div className="an-frame" style={style(frameRect)}>
            {isBoxed(sel.kind) &&
              HANDLES.map((h) => (
                <span
                  key={h}
                  className={`an-h ${h}`}
                  onPointerDown={(e) => startDrag(e, sel, { resize: h })}
                  onPointerMove={onDragMove}
                  onPointerUp={onDragEnd}
                />
              ))}
          </div>
        )}
        {draft && <Draft draft={draft} size={rawSize} tool={tool} />}
        {box && <BoxEditor rect={box} onDone={(t) => {
          setBox(null);
          if (t.trim()) void an().add(docId, page, { type: "freeText", rect: box, text: t.trim(), fontSize: 12 });
          done();
        }} />}
      </div>
      {DRAW_TOOLS.includes(tool) && !box && (
        <div className="an-capture" onPointerDown={down} onPointerMove={move} onPointerUp={up} onDoubleClick={() => draft?.kind === "poly" && finishPoly(draft.pts.slice(0, -1))} />
      )}
      {tool === "select" && sel && !drag && (
        <Popover doc={docId} a={sel} anchor={rectToScreen(sel.rect, rawSize, rot, z)} />
      )}
    </>
  );
}

function Draft({ draft, size, tool }: { draft: Draft; size: Size; tool: string }) {
  const pts = draft.kind === "box" ? [] : draft.kind === "ink" ? draft.pts : [...draft.pts, draft.hover];
  const path = pts.map((q, i) => `${i ? "L" : "M"}${q[0]} ${q[1]}`).join("");
  let shape: React.ReactNode = null;
  if (draft.kind === "box") {
    const [x0, y0, x1, y1] = normRect(draft.a, draft.b);
    if (tool === "ellipse") shape = <ellipse cx={(x0 + x1) / 2} cy={(y0 + y1) / 2} rx={(x1 - x0) / 2} ry={(y1 - y0) / 2} />;
    else if (tool === "line" || tool === "arrow") shape = <line x1={draft.a[0]} y1={draft.a[1]} x2={draft.b[0]} y2={draft.b[1]} />;
    else shape = <rect x={x0} y={y0} width={x1 - x0} height={y1 - y0} className={tool === "freeText" ? "box" : ""} />;
  } else shape = <path d={path} fill={draft.kind === "poly" ? "var(--accent-soft)" : "none"} />;
  return (
    <svg className="an-draft" width={size[0]} height={size[1]}>
      {shape}
    </svg>
  );
}

/** The new text box's editor, in page space so it scales with the page. */
function BoxEditor({ rect, onDone }: { rect: Rect4; onDone: (text: string) => void }) {
  const [t, setT] = useState("");
  const finished = useRef(false);
  const end = (text: string) => {
    if (finished.current) return;
    finished.current = true;
    onDone(text);
  };
  return (
    <textarea
      className="an-box-edit"
      style={{ left: rect[0], top: rect[1], width: rect[2] - rect[0], height: rect[3] - rect[1], fontSize: 12 }}
      autoFocus
      value={t}
      placeholder="Type here"
      onChange={(e) => setT(e.target.value)}
      onBlur={() => end(t)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") end("");
        else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) end(t);
      }}
    />
  );
}

const SIZES = [10, 12, 14, 18, 24];

/** Colour, opacity, thickness and text of the selected comment, just above it. */
function Popover({ doc, a, anchor }: { doc: number; a: Annot; anchor: Rect4 }) {
  const editing = useAnnot((s) => s.docs[doc]?.editing);
  const [text, setText] = useState(a.contents);
  const [opacity, setOpacity] = useState(a.opacity);
  const [width, setWidth] = useState(a.width);
  const focus = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    setText(a.contents);
    setOpacity(a.opacity);
    setWidth(a.width);
  }, [a.xref, a.contents, a.opacity, a.width]);
  useEffect(() => {
    if (editing && editing.xref === a.xref) focus.current?.focus();
  }, [editing, a.xref]);

  const edit = (p: Parameters<ReturnType<typeof an>["edit"]>[3]) => void an().edit(doc, a.page, a.xref, p);
  const hasText = a.kind === "note" || a.kind === "freeText";
  const hasWidth = ["ink", "rectangle", "ellipse", "line", "arrow", "polygon"].includes(a.kind);
  const colored = a.kind !== "stamp";
  // Placed in viewport px against the viewer's box, then converted to the page's own coordinates.
  const el = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<[number, number] | null>(null);
  useLayoutEffect(() => {
    const me = el.current;
    const page = me?.parentElement;
    const viewer = page?.closest(".viewer");
    if (!me || !page || !viewer) return;
    const p = page.getBoundingClientRect();
    const v = viewer.getBoundingClientRect();
    const [l, t] = placePopover(
      [p.left + anchor[0], p.top + anchor[1], p.left + anchor[2], p.top + anchor[3]],
      [me.offsetWidth, me.offsetHeight],
      [v.left, v.top, v.right, v.bottom],
    );
    setPos((o) => (o && Math.abs(o[0] - (l - p.left)) < 0.5 && Math.abs(o[1] - (t - p.top)) < 0.5 ? o : [l - p.left, t - p.top]));
  });
  return (
    <div
      ref={el}
      className="an-pop"
      style={{ left: pos?.[0] ?? 0, top: pos?.[1] ?? 0, visibility: pos ? "visible" : "hidden" }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      {hasText && (
        <textarea
          ref={focus}
          className="an-pop-text"
          rows={2}
          value={text}
          placeholder={a.kind === "note" ? "Write a comment" : "Text"}
          aria-label="Comment text"
          onChange={(e) => setText(e.target.value)}
          onBlur={() => text !== a.contents && edit({ contents: text })}
        />
      )}
      <div className="an-pop-row">
        {colored &&
          COLORS.map((c) => (
            <button
              key={c.name}
              className={`an-dot${rgbHex(a.color) === rgbHex(c.rgb) ? " on" : ""}`}
              style={{ background: rgbHex(c.rgb) }}
              title={c.name}
              aria-label={c.name}
              onClick={() => edit({ color: c.rgb })}
            />
          ))}
        <span className="grow" />
        {a.kind === "freeText" && (
          <select className="an-size" value={a.fontSize ?? 12} aria-label="Font size" onChange={(e) => edit({ fontSize: Number(e.target.value) })}>
            {[...new Set([...SIZES, Math.round(a.fontSize ?? 12)])].sort((x, y) => x - y).map((s) => (
              <option key={s} value={s}>{s}</option>
            ))}
          </select>
        )}
        <button className="an-trash" title="Delete" aria-label="Delete comment" onClick={() => void an().remove(doc, [{ page: a.page, xref: a.xref }])}>
          <Icon name="trash" size={15} />
        </button>
      </div>
      {colored && (
        <label className="an-pop-row an-slider">
          <span className="dim">Opacity</span>
          <input
            type="range" min={0.2} max={1} step={0.05} value={opacity}
            onChange={(e) => setOpacity(Number(e.target.value))}
            onPointerUp={() => opacity !== a.opacity && edit({ opacity })}
            onKeyUp={() => opacity !== a.opacity && edit({ opacity })}
          />
          <span className="an-val">{Math.round(opacity * 100)}%</span>
        </label>
      )}
      {hasWidth && (
        <label className="an-pop-row an-slider">
          <span className="dim">Thickness</span>
          <input
            type="range" min={0.5} max={12} step={0.5} value={width}
            onChange={(e) => setWidth(Number(e.target.value))}
            onPointerUp={() => width !== a.width && edit({ width })}
            onKeyUp={() => width !== a.width && edit({ width })}
          />
          <span className="an-val">{width} pt</span>
        </label>
      )}
    </div>
  );
}

export { hitTest };
