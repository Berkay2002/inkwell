import { memo, useLayoutEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import { useAi } from "../lib/aiStore";
import { type Rot, type Size, type TileRef, pageSpaceTransform, tileKeyStr } from "../lib/layout";
import { cached } from "../lib/tiles";
import type { Rect4 } from "../store";
import { TileCanvas } from "./TileCanvas";

interface Props {
  docId: number;
  page: number;
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

  const lines = usePageText(docId, page, p.wantText);
  // The assistant's last clicked citation, outlined on its page.
  const mark = useAi((s) => (s.mark && s.mark.doc === docId && s.mark.page === page ? s.mark : null));

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
        className="pagespace"
        style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom) }}
      >
        {p.hits.map((r, i) => (
          <div key={i} className={r === p.currentHit ? "hit current" : "hit"} style={rectStyle(r)} />
        ))}
        {mark?.rects.map((r, i) => (
          <div key={`${mark.nonce}.${i}`} className="cite-mark" style={rectStyle(r)} />
        ))}
        {mark && mark.rects.length > 0 && (
          <span className="cite-badge" style={{ left: mark.rects[0][0] - 4, top: mark.rects[0][1] - 20 }}>
            {mark.n}
          </span>
        )}
        {lines && <TextLines lines={lines} />}
      </div>
    </div>
  );
});

const rectStyle = (r: Rect4) => ({ left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] });

function usePageText(doc: number, page: number, enabled: boolean) {
  const [lines, setLines] = useState<api.TextLine[] | null>(null);
  useLayoutEffect(() => {
    if (!enabled) return;
    let live = true;
    cached(`${doc}:${page}`, () => api.pageText(doc, page)).then((l) => live && setLines(l), () => {});
    return () => {
      live = false;
    };
  }, [doc, page, enabled]);
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
