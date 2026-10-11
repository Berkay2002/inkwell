import { memo, useEffect, useMemo, useRef, useState } from "react";
import { scaleOf, useMeasure } from "../../lib/measure";
import {
  type Measure, type MeasureKind, type Pt, type SnapKind, MIN_POINTS, numText, place, quantityOf, UNSCALED,
} from "../../lib/measureLogic";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";

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
const SNAP_NAME: Record<SnapKind | "close", string> = { endpoint: "End point", midpoint: "Midpoint", line: "On line", close: "Close" };
type Hover = { at: Pt; snap: SnapKind | "close" | null };

/** Measure mode's layer on one page: drawing capture, live label, snap marker, and the hit lines of what is measured. */
export const MeasureLayer = memo(function MeasureLayer(p: Props) {
  const open = useMeasure((s) => s.open);
  return open ? <Layer {...p} /> : null;
});

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
  const kind: MeasureKind | null = tool === "calibrate" ? "distance" : tool === "select" ? null : tool;
  const hasScale = !!d?.scales[page];

  useEffect(() => {
    if (!hasScale) void ms().ensureScale(docId, page);
  }, [docId, page, hasScale]);

  // A tool change abandons what is half drawn.
  useEffect(() => {
    setPts([]);
    setHover(null);
    hoverRef.current = null;
  }, [tool]);

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
        e.stopPropagation();
      } else if (e.key === "Enter") finish(pts);
      else if (e.key === "Backspace") {
        e.preventDefault();
        setPts(pts.slice(0, -1));
      }
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

  // The engine answers one snap at a time; the pointer keeps moving, so only the newest request's answer is used.
  const busy = useRef(false);
  const want = useRef<{ raw: Pt; shift: boolean } | null>(null);
  const context = () => {
    const last = pts.length ? pts[pts.length - 1] : null;
    const close = tool === "area" && pts.length >= 3 ? pts[0] : null;
    return { last, close, radius: REACH / zoom };
  };
  const move = (e: React.PointerEvent) => {
    if (!kind) return;
    const raw = toPage(e);
    const c = context();
    const h = place(raw, c.last, e.shiftKey, null, c.close, c.radius);
    hoverRef.current = h;
    setHover(h);
    if (e.shiftKey || h.snap === "close") return void (want.current = null);
    want.current = { raw, shift: false };
    if (busy.current) return;
    const pump = () => {
      const w = want.current;
      if (!w) return;
      want.current = null;
      busy.current = true;
      void ms()
        .snap(docId, page, w.raw, c.radius)
        .then((hit) => {
          busy.current = false;
          if (want.current) return pump(); // the pointer moved on: ask for where it is now
          const next = place(w.raw, c.last, false, hit, c.close, c.radius);
          hoverRef.current = next;
          setHover(next);
        });
    };
    pump();
  };
  const down = (e: React.PointerEvent) => {
    if (!kind || e.button !== 0) return;
    const at = hoverRef.current?.at ?? place(toPage(e), pts.length ? pts[pts.length - 1] : null, e.shiftKey, null, null, 0).at;
    const closing = hoverRef.current?.snap === "close";
    if (kind === "distance") return pts.length === 0 ? setPts([at]) : finish([pts[0], at]);
    if (closing) return finish(pts);
    setPts([...pts, at]);
  };

  const style = (at: Pt) => ({ left: at[0], top: at[1] });
  const path = (p: Pt[]) => p.map((q) => `${q[0]},${q[1]}`).join(" ");
  const live = kind && pts.length > 0 && hover ? [...pts, hover.at] : pts;
  const label = (() => {
    if (!kind || !hover) return null;
    if (tool === "calibrate") return live.length >= 2 ? `${numText(quantityOf("distance", live, UNSCALED).value)} in on the page` : "Click one end of a printed dimension";
    if (live.length < 2) return "";
    return quantityOf(kind === "area" && live.length < 3 ? "perimeter" : kind, live, scale).text;
  })();
  const sz = 5 / zoom;

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
            return (
              <Shape
                key={m.xref}
                points={path(m.points)}
                className={`ms-hit${on ? " on" : ""}${m.kind === "area" ? " area" : ""}`}
                style={{ pointerEvents: tool === "select" ? "stroke" : "none" }}
                onPointerDown={(e) => {
                  e.stopPropagation();
                  ms().select(docId, { page, xref: m.xref });
                }}
              >
                <title>{`${m.contents || m.quantity.text} (${m.kind})`}</title>
              </Shape>
            );
          })}
          {calib && <line className="ms-calib" x1={calib.a[0]} y1={calib.a[1]} x2={calib.b[0]} y2={calib.b[1]} />}
          {kind && live.length > 0 &&
            (kind === "area" && live.length >= 3 ? <polygon className="ms-draft area" points={path(live)} /> : <polyline className="ms-draft" points={path(live)} />)}
          {pts.map((q, i) => (
            <circle key={i} className="ms-vertex" cx={q[0]} cy={q[1]} r={sz * 0.7} />
          ))}
          {kind && hover?.snap && (
            <g className={`ms-snap ${hover.snap}`} transform={`translate(${hover.at[0]} ${hover.at[1]})`}>
              {hover.snap === "midpoint" ? (
                <polygon points={`0,${-sz * 1.3} ${sz * 1.2},${sz} ${-sz * 1.2},${sz}`} />
              ) : hover.snap === "line" ? (
                <path d={`M${-sz} 0H${sz}M0 ${-sz}V${sz}`} />
              ) : (
                <rect x={-sz} y={-sz} width={sz * 2} height={sz * 2} />
              )}
            </g>
          )}
        </svg>
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
      {kind && !calib && <div className="an-capture ms-capture" onPointerDown={down} onPointerMove={move} onPointerLeave={() => setHover(null)} onDoubleClick={() => finish(pts)} />}
    </>
  );
}
