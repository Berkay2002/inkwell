import { memo, useEffect, useMemo, useRef, useState } from "react";
import { scaleOf, useMeasure } from "../../lib/measure";
import {
  type Hover, type Measure, type MeasureKind, type Pt, KIND_NAME, MIN_POINTS, clickAt, latestOnly, neighbour, nudge, inBox, numText, onPage, place,
  quantityOf, UNSCALED,
} from "../../lib/measureLogic";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import { useRemoteLabel } from "./useRemoteLabel";

interface Props {
  docId: number;
  page: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
}

const EMPTY: Measure[] = [];
const ms = () => useMeasure.getState();
/** Pointer reach of a snap, in screen px (the engine takes it in points: divided by the zoom). */
const REACH = 9;
const SNAP_NAME: Record<NonNullable<Hover["snap"]>, string> = { endpoint: "End point", midpoint: "Midpoint", intersection: "Intersection", line: "On line", close: "Close" };

/** Measure mode's layer on one page: drawing capture, live label, snap marker, and the hit lines of what is measured. */
export const MeasureLayer = memo(function MeasureLayer(p: Props) {
  const open = useMeasure((s) => s.open);
  return open ? <Layer {...p} /> : null;
});

/** A measurement's points being moved: dragged with the pointer or nudged with the arrow keys. */
interface Edit {
  xref: number;
  i: number;
  /** All the points as they would be. */
  pts: Pt[];
  moved: boolean;
  /** The snap the dragged point sits on, for the marker and the name next to it (null: none, or moved by the keys). */
  snap: Hover["snap"];
  /** Written; the points stay on show until the list has been read again. */
  saving: boolean;
}
/** Idle time after the last arrow key before the move is written (one undo step for a run of keys). */
const NUDGE_MS = 600;

interface HandleProps {
  kind: MeasureKind;
  pts: Pt[];
  r: number;
  onDown: (e: React.PointerEvent<SVGCircleElement>, i: number) => void;
  onMove: (e: React.PointerEvent<SVGCircleElement>) => void;
  onUp: (e: React.PointerEvent<SVGCircleElement>) => void;
  onCancel: () => void;
  /** The pointer capture ended without a pointer up (the window lost it, the element went away). */
  onLost: () => void;
  onKey: (e: React.KeyboardEvent<SVGCircleElement>, i: number) => void;
  onBlur: () => void;
}

/** One focusable handle per point of the selected measurement: drag it, or move it with the arrow keys. */
export function Handles({ kind, pts, r, onDown, onMove, onUp, onCancel, onLost, onKey, onBlur }: HandleProps) {
  return (
    <g>
      {pts.map((q, i) => (
        <circle
          key={i}
          className="ms-handle"
          cx={q[0]}
          cy={q[1]}
          r={r}
          tabIndex={0}
          role="button"
          aria-label={`${KIND_NAME[kind]} point ${i + 1} of ${pts.length}. Drag it, or use the arrow keys (Shift for 10 points).`}
          onPointerDown={(e) => onDown(e, i)}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={onCancel}
          onLostPointerCapture={onLost}
          onKeyDown={(e) => onKey(e, i)}
          onBlur={onBlur}
        />
      ))}
    </g>
  );
}

const typing = (e: KeyboardEvent) => !!(e.target as HTMLElement | null)?.closest?.("input, textarea, select");

function Layer({ docId, page, rot, rawSize, zoom }: Props) {
  const tool = useMeasure((s) => s.tool);
  const d = useMeasure((s) => s.docs[docId]);
  const items = d?.items ?? EMPTY;
  const selected = d?.selected ?? null;
  const calib = d?.calib && d.calib.page === page ? d.calib : null;
  const scale = scaleOf(d, page);
  const mine = useMemo(() => items.filter((m) => m.page === page), [items, page]);
  const space = useRef<HTMLDivElement>(null);
  const [pts, setPts] = useState<Pt[]>([]);
  const [hover, setHover] = useState<Hover | null>(null);
  const hoverRef = useRef<Hover | null>(null);
  const locked = useMeasure((s) => !!s.locks[docId]);
  const kind: MeasureKind | null = locked || tool === "select" ? null : tool === "calibrate" ? "distance" : tool;
  const hasScale = !!d?.scales[page];
  const pagePlain = d?.scales[page]?.plain ?? true;
  const selM = selected ? (mine.find((m) => m.xref === selected.xref) ?? null) : null;
  const canEdit = tool === "select" && !locked && !calib && !!selM;
  const [edit, setEdit] = useState<Edit | null>(null);
  const editRef = useRef<Edit | null>(null);
  editRef.current = edit;
  /** A pointer drag of a handle is under way (the arrow keys edit without one). */
  const holding = useRef(false);
  const timer = useRef<number | null>(null);
  const stopTimer = () => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = null;
  };
  /** Writes what the arrow keys moved and the timer has not yet (set below, where `commit` is). */
  const flush = useRef(() => {});
  useEffect(() => () => flush.current(), []);

  useEffect(() => {
    if (!hasScale) void ms().ensureScale(docId, page);
  }, [docId, page, hasScale]);

  // The engine answers snaps one at a time while the pointer keeps moving: an answer counts only if nothing has
  // happened since it was asked (a move, Shift, the pointer leaving, a tool change).
  const asked = useRef(latestOnly()).current;

  // A tool change abandons what is half drawn.
  useEffect(() => {
    flush.current();
    asked.cancel();
    setPts([]);
    setHover(null);
    hoverRef.current = null;
    holding.current = false;
    stopTimer();
    setEdit(null);
  }, [tool, locked]);
  // What is being edited was deleted, or the file changed under it (undo): the edit is dropped.
  useEffect(() => {
    if (editRef.current && !mine.some((m) => m.xref === editRef.current!.xref)) setEdit(null);
  }, [mine]);

  const finish = (all: Pt[]) => {
    // A double-click makes the same point twice; so does clicking the last point again.
    const clean = all.filter((q, i) => i === 0 || Math.hypot(q[0] - all[i - 1][0], q[1] - all[i - 1][1]) > 1e-6);
    setPts([]);
    if (!kind || clean.length < MIN_POINTS[kind]) return;
    if (tool === "calibrate") ms().startCalib(docId, { page, a: clean[0], b: clean[1] });
    else void ms().add(docId, page, kind, clean);
  };

  useEffect(() => {
    if (pts.length === 0) return;
    const k = (e: KeyboardEvent) => {
      if (typing(e)) return;
      if (e.key === "Escape") {
        setPts([]);
      } else if (e.key === "Enter") finish(pts);
      else if (e.key === "Backspace") {
        e.preventDefault();
        setPts(pts.slice(0, -1));
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });

  /** Where the pointer is in page space, off the page too. */
  const atPage = (e: { clientX: number; clientY: number }): Pt => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = space.current!.parentElement!.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [q.x, q.y];
  };
  const toPage = (e: { clientX: number; clientY: number }): Pt => {
    const q = atPage(e);
    return [Math.min(Math.max(q[0], 0), rawSize[0]), Math.min(Math.max(q[1], 0), rawSize[1])];
  };

  const busy = useRef(false);
  type Ctx = { last: Pt | null; close: Pt | null; radius: number };
  const want = useRef<{ raw: Pt; c: Ctx; t: number; apply: (h: Hover) => void } | null>(null);
  const context = (): Ctx => {
    const last = pts.length ? pts[pts.length - 1] : null;
    const close = tool === "area" && pts.length >= 3 ? pts[0] : null;
    return { last, close, radius: REACH / zoom };
  };
  /** Where a point at `raw` lands: at once without the engine, then (when it answers) snapped; `apply` takes each. */
  const track = (raw: Pt, c: Ctx, shift: boolean, apply: (h: Hover) => void) => {
    const t = asked.start();
    const h = place(raw, c.last, shift, null, c.close, c.radius);
    apply(h);
    if (shift || h.snap === "close") return void (want.current = null);
    want.current = { raw, c, t, apply };
    if (busy.current) return;
    const pump = () => {
      const w = want.current;
      if (!w) return;
      want.current = null;
      busy.current = true;
      void ms()
        .snap(docId, page, w.raw, w.c.radius)
        .then((hit) => {
          busy.current = false;
          if (want.current) return pump(); // the pointer moved on: ask for where it is now
          if (!asked.current(w.t)) return; // something happened since (Shift, leaving, a new tool)
          w.apply(place(w.raw, w.c.last, false, hit, w.c.close, w.c.radius));
        });
    };
    pump();
  };
  const move = (e: React.PointerEvent) => {
    if (!kind) return;
    track(toPage(e), context(), e.shiftKey, (h) => {
      hoverRef.current = h;
      setHover(h);
    });
  };
  const leave = () => {
    asked.cancel();
    want.current = null;
    hoverRef.current = null;
    setHover(null);
  };
  const down = (e: React.PointerEvent) => {
    if (!kind || e.button !== 0) return;
    const { at, snap } = clickAt(hoverRef.current, toPage(e), pts.length ? pts[pts.length - 1] : null, e.shiftKey);
    if (kind === "distance") return pts.length === 0 ? setPts([at]) : finish([pts[0], at]);
    if (snap === "close") return finish(pts);
    setPts([...pts, at]);
  };

  // ---- moving the points of the selected measurement ----
  const commit = (e: Edit) => {
    stopTimer();
    holding.current = false;
    const was = ms().get(docId).items.find((m) => m.xref === e.xref);
    if (!e.moved || !was || was.points.every((p, j) => p[0] === e.pts[j][0] && p[1] === e.pts[j][1])) return setEdit(null);
    // the ref too: a blur or a click that follows in this event must see it is being written
    editRef.current = { ...e, snap: null, saving: true };
    setEdit(editRef.current);
    // The new points stay on show until the list has been read again; a refusal puts the old ones back (the panel says why).
    void ms().move(docId, { page, xref: e.xref }, e.pts).then(() => setEdit((cur) => (cur?.saving ? null : cur)));
  };
  const cancel = () => {
    holding.current = false;
    asked.cancel();
    want.current = null;
    stopTimer();
    setEdit(null);
  };
  flush.current = () => {
    stopTimer();
    const c = editRef.current;
    if (c?.moved && !c.saving && !holding.current) commit(c);
  };
  const grab = (e: React.PointerEvent<SVGCircleElement>, m: Measure, i: number) => {
    if (e.button !== 0 || editRef.current?.saving) return;
    e.stopPropagation();
    e.preventDefault(); // no text selection; the handle is focused below
    e.currentTarget.setPointerCapture(e.pointerId);
    const cur = editRef.current;
    e.currentTarget.focus(); // moving focus from another handle writes the keys pending on it
    const same = cur?.xref === m.xref;
    holding.current = true;
    stopTimer();
    setEdit({ xref: m.xref, i, pts: same ? cur.pts : m.points, moved: same && cur.moved, snap: null, saving: false });
  };
  const drag = (e: React.PointerEvent, m: Measure) => {
    const cur = editRef.current;
    if (!holding.current || !cur || cur.saving) return;
    track(toPage(e), { last: neighbour(m.kind, cur.pts, cur.i), close: null, radius: REACH / zoom }, e.shiftKey, (h) =>
      setEdit((c) => {
        if (!c || c.saving) return c;
        const from = m.points[c.i];
        return {
          ...c,
          pts: c.pts.map((p, j) => (j === c.i ? h.at : p)),
          moved: c.moved || Math.hypot(h.at[0] - from[0], h.at[1] - from[1]) * zoom > 2,
          snap: h.snap,
        };
      }),
    );
  };
  const release = (e: React.PointerEvent<SVGCircleElement>) => {
    if (!holding.current) return;
    // First: releasing the capture ends in a lostpointercapture, which must not read as a lost drag.
    holding.current = false;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
    asked.cancel();
    want.current = null;
    // Let go off the page or outside the viewer (outside the window the pointer is clamped to its edge, and a webview
    // may report 0,0, which a zoomed page can cover): the point goes back, as with Esc.
    const viewer = space.current?.closest(".viewer");
    if (!onPage(atPage(e), rawSize) || (viewer && !inBox([e.clientX, e.clientY], viewer.getBoundingClientRect()))) return cancel();
    if (editRef.current && !editRef.current.saving) commit(editRef.current);
  };
  const press = (e: React.KeyboardEvent, m: Measure, i: number) => {
    const cur = editRef.current;
    if (cur?.saving) return;
    if (e.key === "Escape" && cur) {
      e.preventDefault();
      e.stopPropagation();
      return cancel();
    }
    if (e.key === "Enter" && cur?.moved) {
      e.preventDefault();
      return commit(cur);
    }
    if (holding.current) return;
    const next = nudge(cur && cur.xref === m.xref ? cur.pts : m.points, i, e.key, e.shiftKey ? 10 : 1, rot, rawSize);
    if (!next) return;
    e.preventDefault();
    e.stopPropagation();
    setEdit({ xref: m.xref, i, pts: next, moved: true, snap: null, saving: false });
    stopTimer();
    timer.current = window.setTimeout(() => {
      const c = editRef.current;
      if (c?.moved && !c.saving) commit(c);
    }, NUDGE_MS);
  };
  const em = edit ? (mine.find((m) => m.xref === edit.xref) ?? null) : null;
  const ownLabel = !!em && em.plain && !!em.scale;
  const editRemote = useRemoteLabel(edit?.moved && em && !ownLabel ? { doc: docId, page, kind: em.kind, pts: edit.pts, xref: em.xref } : null);
  const editLabel = !edit?.moved || !em ? null : ownLabel ? quantityOf(em.kind, edit.pts, em.scale!).text : editRemote;

  // The value the rail shows while this layer's drag is on (cleared when it ends, only by the layer that set it).
  const liveText = edit?.moved && editLabel ? editLabel : null;
  const liveXref = edit?.xref;
  useEffect(() => {
    if (liveText === null || liveXref === undefined) return;
    ms().setLive({ doc: docId, xref: liveXref, text: liveText });
    return () => ms().setLive(null);
  }, [docId, liveXref, liveText]);

  const style = (at: Pt) => ({ left: at[0], top: at[1] });
  const path = (p: Pt[]) => p.map((q) => `${q[0]},${q[1]}`).join(" ");
  const live = kind && pts.length > 0 && hover ? [...pts, hover.at] : pts;
  const liveKind: MeasureKind | null = kind === "area" && live.length < 3 ? "perimeter" : kind;
  // The page's own scale is worked out here; a file's own /D, /A or several viewports are valued by the engine.
  const drawRemote = useRemoteLabel(liveKind && tool !== "calibrate" && hover && live.length >= 2 && !pagePlain ? { doc: docId, page, kind: liveKind, pts: live, xref: null } : null);
  const label = (() => {
    if (!kind || !hover) return null;
    if (tool === "calibrate") return live.length >= 2 ? `${numText(quantityOf("distance", live, UNSCALED).value)} in on the page` : "Click one end of a printed dimension";
    if (live.length < 2) return "";
    return pagePlain ? quantityOf(liveKind!, live, scale).text : (drawRemote ?? "");
  })();
  const sz = 5 / zoom;
  // The snap marker: at the pointer while drawing, on the dragged point while a handle is held.
  const mark = kind ? hover : edit?.snap && !edit.saving ? { at: edit.pts[edit.i], snap: edit.snap } : null;

  return (
    <>
      <div
        ref={space}
        className="an-space"
        style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
      >
        <svg className="ms-svg" width={rawSize[0]} height={rawSize[1]}>
          {mine.map((m) => {
            const on = selected?.xref === m.xref;
            const Shape = m.kind === "area" ? "polygon" : "polyline";
            const shown = edit?.xref === m.xref ? edit.pts : m.points;
            return (
              <g key={m.xref}>
              {on && <Shape points={path(shown)} className={`ms-sel${m.kind === "area" ? " area" : ""}`} />}
              <Shape
                points={path(shown)}
                className="ms-hit"
                style={{ pointerEvents: tool === "select" ? "stroke" : "none" }}
                onPointerDown={(e) => {
                  e.stopPropagation();
                  ms().select(docId, { page, xref: m.xref });
                }}
              >
                <title>{`${m.contents || m.quantity.text} (${m.kind})`}</title>
              </Shape>
              </g>
            );
          })}
          {canEdit && selM && (
            <Handles
              kind={selM.kind}
              pts={edit?.xref === selM.xref ? edit.pts : selM.points}
              r={sz * 1.2}
              onDown={(e, i) => grab(e, selM, i)}
              onMove={(e) => drag(e, selM)}
              onUp={release}
              onCancel={cancel}
              onLost={() => holding.current && cancel()}
              onKey={(e, i) => press(e, selM, i)}
              onBlur={() => {
                const c = editRef.current;
                if (c?.moved && !c.saving && !holding.current) commit(c);
              }}
            />
          )}
          {calib && <line className="ms-calib" x1={calib.a[0]} y1={calib.a[1]} x2={calib.b[0]} y2={calib.b[1]} />}
          {kind && live.length > 0 &&
            (kind === "area" && live.length >= 3 ? <polygon className="ms-draft area" points={path(live)} /> : <polyline className="ms-draft" points={path(live)} />)}
          {pts.map((q, i) => (
            <circle key={i} className="ms-vertex" cx={q[0]} cy={q[1]} r={sz * 0.7} />
          ))}
          {mark?.snap && (
            <g className={`ms-snap ${mark.snap}`} transform={`translate(${mark.at[0]} ${mark.at[1]})`}>
              {mark.snap === "midpoint" ? (
                <polygon points={`0,${-sz * 1.3} ${sz * 1.2},${sz} ${-sz * 1.2},${sz}`} />
              ) : mark.snap === "intersection" ? (
                <path d={`M${-sz} ${-sz}L${sz} ${sz}M${sz} ${-sz}L${-sz} ${sz}`} />
              ) : mark.snap === "line" ? (
                <path d={`M${-sz} 0H${sz}M0 ${-sz}V${sz}`} />
              ) : (
                <rect x={-sz} y={-sz} width={sz * 2} height={sz * 2} />
              )}
            </g>
          )}
        </svg>
        {edit?.moved && (editLabel || mark?.snap) && (
          <div
            className="ms-label"
            style={{ ...style(edit.pts[edit.i]), transform: `rotate(${-rot * 90}deg) scale(${1 / zoom}) translate(14px, 14px)` }}
            aria-hidden="true"
          >
            {editLabel ? <b>{editLabel}</b> : null}
            {!kind && mark?.snap && <span>{SNAP_NAME[mark.snap]}</span>}
          </div>
        )}
        {kind && hover && (
          <div
            className="ms-label"
            style={{ ...style(hover.at), transform: `rotate(${-rot * 90}deg) scale(${1 / zoom}) translate(14px, 14px)` }}
            aria-hidden="true"
          >
            {label ? <b>{label}</b> : null}
            {hover.snap && <span>{SNAP_NAME[hover.snap]}</span>}
          </div>
        )}
      </div>
      {kind && !calib && <div className="an-capture ms-capture" onPointerDown={down} onPointerMove={move} onPointerLeave={leave} onDoubleClick={() => finish(pts)} />}
    </>
  );
}
