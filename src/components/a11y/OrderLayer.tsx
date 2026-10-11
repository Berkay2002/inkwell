import { memo, useState } from "react";
import { createPortal } from "react-dom";
import { useA11y } from "../../lib/a11y";
import { type Region, roleWord } from "../../lib/a11yDepthLogic";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";

interface Props {
  docId: number;
  page: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
}

/** The Reading order tool on one page: every region as a box with its number; a number is dragged onto another region. */
export const OrderLayer = memo(function OrderLayer(p: Props) {
  const on = useA11y((s) => s.open && s.view === "order");
  const o = useA11y((s) => s.docs[p.docId]?.order);
  if (!on || !o?.data || o.page !== p.page) return null;
  return <Layer {...p} regions={o.data.regions} selected={o.selected} />;
});

const st = () => useA11y.getState();

function Layer({ docId, rot, rawSize, zoom, regions, selected }: Props & { regions: Region[]; selected: number[] }) {
  // A certified document takes no edit: the numbers still select, but nothing is dragged.
  const certified = useA11y((s) => !!s.docs[docId]?.report?.certified);
  const [drag, setDrag] = useState<{ xref: number; n: number; over: number | null; moved: boolean; x: number; y: number } | null>(null);
  // In reading order in the DOM, so Tab follows it; a larger box sits lower, so a small region is on top of
  // the one that holds it and can be picked.
  const area = (r: Region) => (r.bbox ? (r.bbox[2] - r.bbox[0]) * (r.bbox[3] - r.bbox[1]) : 0);
  const drawn = regions.map((r, i) => ({ r, n: i + 1 })).filter((e) => e.r.bbox && e.r.xref !== 0);
  const layer = new Map([...drawn].sort((a, b) => area(b.r) - area(a.r)).map((e, k) => [e.r.xref, k + 1]));

  const select = (e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }, xref: number) =>
    st().selectRegion(docId, xref, e.shiftKey ? "range" : e.metaKey || e.ctrlKey ? "toggle" : "one");

  const over = (e: React.PointerEvent, self: number) => {
    // Whatever region box is under the pointer, in whatever way the page is turned.
    for (const el of document.elementsFromPoint(e.clientX, e.clientY)) {
      const v = (el as HTMLElement).dataset?.region;
      if (v !== undefined && Number(v) !== self) return Number(v);
    }
    return null;
  };

  return (
    <div
      className="te-space a11y-ro"
      style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
    >
      {drawn.map(({ r, n }) => {
        const [x0, y0, x1, y1] = r.bbox!;
        const sel = selected.includes(r.xref);
        const target = drag?.over === r.xref;
        return (
          <div
            key={r.xref}
            data-region={r.xref}
            className={`a11y-ro-box${sel ? " selected" : ""}${target ? " target" : ""}${drag?.xref === r.xref ? " dragging" : ""}`}
            style={{ left: x0, top: y0, width: x1 - x0, height: y1 - y0, zIndex: layer.get(r.xref) }}
            onPointerDown={(e) => {
              if (e.button !== 0) return;
              e.stopPropagation();
              select(e, r.xref);
            }}
          >
            <div
              className="a11y-ro-num"
              role="button"
              tabIndex={0}
              aria-pressed={sel}
              aria-label={`Region ${n} of ${regions.length}: ${roleWord(r.role)}${r.text ? `, ${r.text}` : ""}.${certified ? "" : " Alt and arrow up or down moves it in the reading order."}`}
              style={{ transform: `rotate(${-rot * 90}deg) scale(${1 / zoom})` }}
              onPointerDown={(e) => {
                if (e.button !== 0) return;
                e.stopPropagation();
                select(e, r.xref);
                if (certified) return;
                e.currentTarget.setPointerCapture(e.pointerId);
                setDrag({ xref: r.xref, n, over: null, moved: false, x: e.clientX, y: e.clientY });
              }}
              onPointerMove={(e) => drag && setDrag({ ...drag, moved: true, over: over(e, drag.xref), x: e.clientX, y: e.clientY })}
              onPointerUp={() => {
                const d = drag;
                setDrag(null);
                if (d?.moved && d.over !== null) void st().dropRegion(docId, d.xref, d.over);
              }}
              onPointerCancel={() => setDrag(null)}
              onKeyDown={(e) => {
                if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowLeft")) {
                  e.preventDefault();
                  void st().moveRegion(docId, r.xref, -1);
                } else if (e.altKey && (e.key === "ArrowDown" || e.key === "ArrowRight")) {
                  e.preventDefault();
                  void st().moveRegion(docId, r.xref, 1);
                } else if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  select(e, r.xref);
                }
              }}
            >
              {n}
            </div>
          </div>
        );
      })}
      {drag?.moved &&
        createPortal(
          <div className="a11y-ro-float" style={{ left: drag.x, top: drag.y }} aria-hidden="true">
            {drag.n}
          </div>,
          document.body,
        )}
    </div>
  );
}
