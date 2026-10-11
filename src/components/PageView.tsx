import { memo, useLayoutEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import type { Shown } from "../lib/a11y";
import { useAi } from "../lib/aiStore";
import { type Rot, type Size, type TileRef, pageSpaceTransform, tileKeyStr } from "../lib/layout";
import { type Mark, useRedact } from "../lib/redact";
import { cached } from "../lib/tiles";
import type { Rect4 } from "../store";
import { AnnotLayer } from "./comments/AnnotLayer";
import { LinkLayer } from "./edit/LinkLayer";
import { TextEditLayer } from "./edit/TextEditLayer";
import { FormLayer } from "./forms/FormLayer";
import { TileCanvas } from "./TileCanvas";

interface Props {
  docId: number;
  page: number;
  /** Edit generation: the page's text changes with it. */
  rev: number;
  rot: Rot;
  /** Displayed size in points. */
  size: Size;
  /** Unrotated size in points (the text layer lives in this space). */
  rawSize: Size;
  zoom: number;
  top: number;
  left: number;
  /** Tiles at the settled render scale that cover the visible part of the page. */
  tiles: TileRef[];
  scale: number;
  preview: TileRef;
  wantText: boolean;
  hits: Rect4[];
  currentHit: Rect4 | null;
  /** Redaction marks on this page. */
  marks: Mark[];
  /** The accessibility checker's "Show": the finding to outline on this page. */
  a11y: Shown | null;
}

export const PageView = memo(function PageView(p: Props) {
  const { docId, page, rot, size, rawSize, zoom, tiles, scale, preview } = p;

  // Keep the previous render scale's tiles underneath until the new ones have painted, so a zoom
  // step never flashes back to the blurry preview.
  const last = useRef({ scale, tiles });
  const [under, setUnder] = useState<TileRef[] | null>(null);
  const drawn = useRef(new Set<string>());
  useLayoutEffect(() => {
    if (last.current.scale !== scale) setUnder(last.current.tiles);
    else if (under && tiles.length === 0) setUnder(null);
    last.current = { scale, tiles };
  });
  const onDrawn = (key: string) => {
    drawn.current.add(key);
    if (under && tiles.every((t) => drawn.current.has(tileKeyStr(docId, t)))) setUnder(null);
  };
  const underTiles = under?.filter((u) => u.scale !== scale) ?? [];

  const lines = usePageText(docId, p.rev, page, p.wantText);
  // The assistant's last clicked citation, highlighted on its page.
  const mark = useAi((s) => (s.mark && s.mark.doc === docId && s.mark.page === page ? s.mark : null));

  // "Mark text or area": a drag over the page makes an area mark (page points, y down).
  const armed = useRedact((s) => s.tool);
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<{ a: [number, number]; b: [number, number] } | null>(null);
  const toPage = (e: React.PointerEvent): [number, number] => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = e.currentTarget.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [Math.min(Math.max(q.x, 0), rawSize[0]), Math.min(Math.max(q.y, 0), rawSize[1])];
  };

  return (
    <div className="page" style={{ top: p.top, left: p.left, width: size[0] * zoom, height: size[1] * zoom }} data-page={page}>
      <TileCanvas docId={docId} tile={preview} size={size} zoom={zoom} cover />
      {underTiles.map((t) => (
        <TileCanvas key={tileKeyStr(docId, t)} docId={docId} tile={t} size={size} zoom={zoom} under />
      ))}
      {tiles.map((t) => (
        <TileCanvas key={tileKeyStr(docId, t)} docId={docId} tile={t} size={size} zoom={zoom} onDrawn={onDrawn} />
      ))}
      <div
        ref={space}
        className="pagespace"
        style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom) }}
      >
        {p.hits.map((r, i) => (
          <div key={i} className={r === p.currentHit ? "hit current" : "hit"} style={rectStyle(r)} />
        ))}
        {p.marks.map((m) => (
          <div key={m.xref} className={m.wholePage ? "rd-mark page-wide" : "rd-mark"} style={{ ...rectStyle(markRect(m, rawSize)), "--z": zoom } as React.CSSProperties} />
        ))}
        {p.a11y && (
          <div
            key={p.a11y.nonce}
            className={p.a11y.rect ? "a11y-mark" : "a11y-mark whole"}
            style={{ ...rectStyle(p.a11y.rect ?? [0, 0, rawSize[0], rawSize[1]]), "--z": zoom } as React.CSSProperties}
          />
        )}
        {drag && <div className="rd-mark drawing" style={{ ...rectStyle(dragRect(drag)), "--z": zoom } as React.CSSProperties} />}
        {mark?.rects.map((r, i) => (
          <div key={`${mark.nonce}.${i}`} className="cite-mark" style={rectStyle(r)} />
        ))}
        {mark && mark.rects.length > 0 && (
          // Just before the start of the sentence, upright and the same size at any zoom or rotation.
          <span
            key={`b${mark.nonce}`}
            className="cite-badge"
            style={{
              left: mark.rects[0][0],
              top: mark.rects[0][1],
              transform: `translate(-100%, -50%) rotate(${-rot * 90}deg) scale(${1 / zoom})`,
              transformOrigin: "100% 50%",
              marginLeft: -3 / zoom,
            }}
          >
            {mark.n}
          </span>
        )}
        {lines && <TextLines lines={lines} />}
      </div>
      <FormLayer docId={docId} page={page} rot={rot} rawSize={rawSize} zoom={zoom} />
      <AnnotLayer docId={docId} page={page} rot={rot} rawSize={rawSize} zoom={zoom} />
      <TextEditLayer docId={docId} page={page} rev={p.rev} rot={rot} rawSize={rawSize} zoom={zoom} visible={p.wantText} />
      <LinkLayer docId={docId} page={page} rev={p.rev} rot={rot} rawSize={rawSize} zoom={zoom} visible={p.wantText} />
      {armed && (
        <div
          className="rd-capture"
          onPointerDown={(e) => {
            e.currentTarget.setPointerCapture(e.pointerId);
            const a = toPage(e);
            setDrag({ a, b: a });
          }}
          onPointerMove={(e) => drag && setDrag({ a: drag.a, b: toPage(e) })}
          onPointerUp={() => {
            if (!drag) return;
            const [x0, y0, x1, y1] = dragRect(drag);
            setDrag(null);
            if (x1 - x0 >= 3 && y1 - y0 >= 3) void useRedact.getState().mark(docId, { kind: "area", page, area: [x0, y0, x1, y1] });
          }}
          onPointerCancel={() => setDrag(null)}
        />
      )}
    </div>
  );
});

const dragRect = (d: { a: [number, number]; b: [number, number] }): Rect4 => [
  Math.min(d.a[0], d.b[0]), Math.min(d.a[1], d.b[1]), Math.max(d.a[0], d.b[0]), Math.max(d.a[1], d.b[1]),
];

/** A mark's box in page space: the bounds of its quads; a whole-page mark reaches far past the page, so it is clipped. */
export function markRect(m: Mark, size: Size): Rect4 {
  if (m.wholePage || m.quads.length === 0) return [0, 0, size[0], size[1]];
  const xs = m.quads.flatMap((q) => [q[0], q[2], q[4], q[6]]);
  const ys = m.quads.flatMap((q) => [q[1], q[3], q[5], q[7]]);
  return [Math.max(0, Math.min(...xs)), Math.max(0, Math.min(...ys)), Math.min(size[0], Math.max(...xs)), Math.min(size[1], Math.max(...ys))];
}

const rectStyle = (r: Rect4) => ({ left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] });

function usePageText(doc: number, rev: number, page: number, enabled: boolean) {
  const [lines, setLines] = useState<api.TextLine[] | null>(null);
  useLayoutEffect(() => {
    if (!enabled) return;
    let live = true;
    cached(`${doc}:${rev}:${page}`, () => api.pageText(doc, page)).then((l) => live && setLines(l), () => {});
    return () => {
      live = false;
    };
  }, [doc, rev, page, enabled]);
  return enabled ? lines : null;
}

let measureCtx: CanvasRenderingContext2D | null = null;
// Closest to Helvetica/Arial metrics, which most PDFs use; per-line scaleX absorbs the rest.
const FONT = 'Arial, "Segoe UI", sans-serif';

/** Transparent, selectable text laid over the page; each line is stretched to its true width. */
const TextLines = memo(function TextLines({ lines }: { lines: api.TextLine[] }) {
  const styled = useMemo(() => {
    measureCtx ??= document.createElement("canvas").getContext("2d")!;
    const ctx = measureCtx;
    return lines.map((l) => {
      const size = Math.max(1, l.s || l.b[3] - l.b[1]);
      ctx.font = `${size}px ${FONT}`;
      const natural = ctx.measureText(l.t).width || 1;
      const k = (l.b[2] - l.b[0]) / natural;
      return {
        t: l.t,
        style: {
          left: l.b[0],
          top: l.b[1],
          height: l.b[3] - l.b[1],
          lineHeight: `${l.b[3] - l.b[1]}px`,
          fontSize: size,
          transform: `scaleX(${k})`,
        },
      };
    });
  }, [lines]);
  return (
    <div className="textlayer">
      {styled.map((l, i) => (
        <div key={i} style={l.style}>
          {l.t}
        </div>
      ))}
    </div>
  );
});
