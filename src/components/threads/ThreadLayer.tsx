import { memo, useMemo, useRef, useState } from "react";
import type { ArticleThread, ThreadBead } from "../../lib/api";
import { type Handle, type Rect4, HANDLES, move, resize } from "../../lib/formsLogic";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import { beadNumber, beadPointer, stepBead, useThreads } from "../../lib/threads";
import { Icon } from "../Icons";
import { useOtherToolHeld } from "./held";
import "./threads.css";

interface Props {
  docId: number;
  page: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
}

type Pt = [number, number];
const th = () => useThreads.getState();
/** A bead smaller than this on either side is a click, not a drag. */
const MIN_BEAD = 8;

/**
 * The beads of the article threads on one page: outlines with numbers ("Show beads"), the bead being read and the
 * one after it, and with the Article tool the hit areas to select, move and resize them and a capture layer to
 * drag new beads out.
 */
export const ThreadLayer = memo(function ThreadLayer(p: Props) {
  const list = useThreads((s) => s.docs[p.docId]);
  const show = useThreads((s) => s.show);
  const tool = useThreads((s) => s.tool);
  const follow = useThreads((s) => (s.follow?.doc === p.docId ? s.follow : null));
  if (!list || !(show || tool || follow)) return null;
  return <Layer {...p} list={list} show={show} tool={tool} follow={follow?.bead ?? null} followThread={follow?.thread ?? null} />;
});

type Drag = { mode: "move" | { resize: Handle }; start: Pt; rect0: Rect4; rect: Rect4; moved: boolean; bead: number };
type Item = { t: ArticleThread; b: ThreadBead };

function Layer({ docId, page, rot, rawSize, zoom, list, show, tool, follow, followThread }: Props & {
  list: ArticleThread[]; show: boolean; tool: boolean; follow: number | null; followThread: number | null;
}) {
  const selected = useThreads((s) => (s.selected?.doc === docId ? s.selected : null));
  const other = useOtherToolHeld();
  const pointer = beadPointer({ tool, follow: follow != null, show, other });
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [draft, setDraft] = useState<{ a: Pt; b: Pt } | null>(null);

  const mine = useMemo<Item[]>(() => list.flatMap((t) => t.beads.filter((b) => b.page === page).map((b) => ({ t, b }))), [list, page]);
  const next = follow != null && followThread != null ? stepBead(list, followThread, follow, 1)?.id : undefined;
  if (mine.length === 0 && !tool) return null;

  const toPage = (e: { clientX: number; clientY: number }): Pt => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = space.current!.parentElement!.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [Math.min(Math.max(q.x, 0), rawSize[0]), Math.min(Math.max(q.y, 0), rawSize[1])];
  };
  const style = (r: Rect4) => ({ left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] });
  const boxOf = (a: Pt, b: Pt): Rect4 => [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];

  const startDrag = (e: React.PointerEvent, it: Item, mode: Drag["mode"]) => {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    if (!(selected && selected.thread === it.t.id && selected.bead === it.b.id)) th().select({ doc: docId, thread: it.t.id, bead: it.b.id });
    setDrag({ mode, start: toPage(e), rect0: it.b.rect, rect: it.b.rect, moved: false, bead: it.b.id });
  };
  const onDragMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const q = toPage(e);
    const [dx, dy] = [q[0] - drag.start[0], q[1] - drag.start[1]];
    const rect = drag.mode === "move" ? move(drag.rect0, dx, dy, rawSize) : resize(drag.rect0, drag.mode.resize, dx, dy, rawSize);
    setDrag({ ...drag, rect, moved: drag.moved || Math.hypot(dx, dy) * zoom > 3 });
  };
  const onDragEnd = (it: Item) => {
    const d = drag;
    setDrag(null);
    // one undo step per gesture: the engine is asked once, when the pointer is let go
    if (d?.moved) void th().moveBead(docId, it.t.id, it.b.id, page, d.rect);
  };

  /** A drag that ends joins the selected article after the selected bead, else it starts an article. */
  const create = (clickDeselects = false) => {
    const d = draft;
    setDraft(null);
    if (!d) return;
    const r = boxOf(d.a, d.b);
    if (r[2] - r[0] < MIN_BEAD || r[3] - r[1] < MIN_BEAD) {
      // a click on bare page lets go of the selection, so the next drag starts a new article
      if (clickDeselects) th().select(null);
      return;
    }
    const sel = selected ? list.find((t) => t.id === selected.thread) : undefined;
    if (sel && !sel.damaged) void th().addBead(docId, sel.id, selected!.bead, page, r);
    else void th().start(docId, page, r);
  };
  const begin = (e: React.PointerEvent, at: Pt) => {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    setDraft({ a: at, b: at });
  };

  const cls = (it: Item) => {
    const on = tool && selected?.thread === it.t.id && selected.bead === it.b.id;
    return [
      "th-bead",
      on ? "on" : "",
      !tool && it.b.id === follow ? "cur" : "",
      !tool && it.b.id === next ? "next" : "",
      it.t.damaged ? "bad" : "",
      pointer === "edit" && !it.t.damaged ? "edit" : "",
      pointer === "all" ? "grab" : "",
      pointer === "tag" ? "tag" : "",
    ].filter(Boolean).join(" ");
  };

  return (
    <div
      ref={space}
      className={`th-space${tool ? " tool" : ""}`}
      style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
    >
      {tool && (
        <div
          className="th-capture"
          style={{ width: rawSize[0], height: rawSize[1] }}
          onPointerDown={(e) => begin(e, toPage(e))}
          onPointerMove={(e) => draft && setDraft({ a: draft.a, b: toPage(e) })}
          onPointerUp={() => create(true)}
          onPointerCancel={() => setDraft(null)}
        />
      )}
      {mine.map((it) => {
        const r = drag && drag.bead === it.b.id ? drag.rect : it.b.rect;
        const on = tool && selected?.thread === it.t.id && selected.bead === it.b.id;
        const label = `${beadNumber(list, it.t.id, it.b.id)} ${it.t.title}`;
        return (
          <div
            key={it.b.id}
            className={cls(it)}
            style={style(r)}
            title={it.t.damaged ? `${it.t.title}: ${it.t.damaged}` : label}
            // reading: a click on a bead follows the article from there; the tool: it selects, a double click reads
            onPointerDown={(e) => tool && !it.t.damaged && startDrag(e, it, "move")}
            onPointerMove={onDragMove}
            onPointerUp={() => onDragEnd(it)}
            onClick={() => !tool && th().startFollow(docId, it.t.id, it.b.id)}
            onDoubleClick={() => tool && th().startFollow(docId, it.t.id, it.b.id)}
          >
            <span className="th-num" style={{ transform: `rotate(${-rot * 90}deg) scale(${1 / zoom})` }}>
              {beadNumber(list, it.t.id, it.b.id)}
            </span>
            {on && HANDLES.map((h) => (
              <span
                key={h}
                className={`th-h ${h}`}
                onPointerDown={(e) => startDrag(e, it, { resize: h })}
                onPointerMove={onDragMove}
                onPointerUp={() => onDragEnd(it)}
              />
            ))}
            {on && !drag && (
              <span
                className="th-plus"
                title="Drag from here to add the next bead"
                onPointerDown={(e) => begin(e, [Math.min(r[2], rawSize[0]), Math.min(r[3], rawSize[1])])}
                onPointerMove={(e) => draft && setDraft({ a: draft.a, b: toPage(e) })}
                onPointerUp={() => create()}
                onPointerCancel={() => setDraft(null)}
              >
                <Icon name="plus" size={14} />
              </span>
            )}
          </div>
        );
      })}
      {draft && <div className="th-draft" style={style(boxOf(draft.a, draft.b))} />}
    </div>
  );
}
