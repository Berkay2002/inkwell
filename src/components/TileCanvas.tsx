import { memo, useEffect, useRef } from "react";
import { TILE, type Size, type TileRef, tileKeyStr } from "../lib/layout";
import { type Stream, tileClient } from "../lib/tiles";

interface Props {
  docId: number;
  tile: TileRef;
  /** Displayed page size in points (rotation already applied). */
  size: Size;
  /** CSS px per point. */
  zoom: number;
  stream?: Stream;
  /** Fill the whole page box (page preview) instead of positioning by tile index. */
  cover?: boolean;
  /** Stand-in from the previous render scale; never requested itself. */
  under?: boolean;
  onDrawn?: (key: string) => void;
}

/** One tile: positioned in page CSS space, painted by the tile client when its pixels arrive. */
export const TileCanvas = memo(function TileCanvas({ docId, tile, size, zoom, stream = 0, cover, under, onDrawn }: Props) {
  const ref = useRef<HTMLCanvasElement>(null);
  const onDrawnRef = useRef(onDrawn);
  onDrawnRef.current = onDrawn;
  const { page, rot, scale, tx, ty, rev } = tile;
  const key = tileKeyStr(docId, tile);

  useEffect(() => {
    const canvas = ref.current!;
    const ctx = canvas.getContext("2d")!;
    const sub = {
      drawn: false,
      draw(d: { w: number; h: number; rgba: Uint8ClampedArray }) {
        if (canvas.width !== d.w || canvas.height !== d.h) {
          canvas.width = d.w;
          canvas.height = d.h;
        }
        ctx.putImageData(new ImageData(d.rgba as Uint8ClampedArray<ArrayBuffer>, d.w, d.h), 0, 0);
        sub.drawn = true;
        canvas.dataset.ready = "1";
        onDrawnRef.current?.(key);
      },
    };
    const t = { page, rot, scale, tx, ty, rev };
    tileClient.register(stream, docId, t, sub);
    return () => tileClient.unregister(stream, docId, t, sub);
    // key covers docId/page/rot/scale/tx/ty/rev
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, stream]);

  const s = scale / 1000;
  const style = cover
    ? { left: 0, top: 0, width: "100%", height: "100%" }
    : {
        left: ((tx * TILE) / s) * zoom,
        top: ((ty * TILE) / s) * zoom,
        width: (Math.min(TILE, Math.ceil(size[0] * s) - tx * TILE) / s) * zoom,
        height: (Math.min(TILE, Math.ceil(size[1] * s) - ty * TILE) / s) * zoom,
      };
  return <canvas ref={ref} className={cover ? "tile preview" : under ? "tile under" : "tile"} style={style} />;
});
