import { memo, useEffect, useRef, useState } from "react";
import type { ImageBlock } from "../../lib/api";
import { type Handle, type Rect4, clampRect, dims, dragHandle, isCorner, mapRect, moveRect, placement, rectH, rectW, useImageEdit } from "../../lib/imageEdit";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import { useTextEdit } from "../../lib/textEdit";
import "./imageEdit.css";

interface Props {
  docId: number;
  page: number;
  /** Document generation: the page's images are listed again when it changes. */
  rev: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
  /** Near the viewport: only these pages list their images. */
  visible: boolean;
}

const ie = () => useImageEdit.getState();
const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
const LABEL: Record<Handle, string> = {
  nw: "Resize from the top left",
  n: "Resize from the top",
  ne: "Resize from the top right",
  e: "Resize from the right",
  se: "Resize from the bottom right",
  s: "Resize from the bottom",
  sw: "Resize from the bottom left",
  w: "Resize from the left",
};

/** Edit mode's layer of images on one page: outlines, the frame of the selected one, crop and "place here". */
export const ImageEditLayer = memo(function ImageEditLayer(p: Props) {
  // Links and Bookmarks have their own layers and keys: the pictures are only live in the Text section.
  const live = useTextEdit((s) => s.open && s.section === "text");
  return live ? <Layer {...p} /> : null;
});

/** What is being dragged: the whole image, one of its handles, or the crop rectangle (and its handles). */
type Drag = { what: "move" | Handle; crop: boolean; start: [number, number]; d: [number, number]; free: boolean };

function Layer({ docId, page, rev, rot, rawSize, zoom, visible }: Props) {
  const listed = useImageEdit((s) => s.docs[docId]?.pages[page]);
  const selected = useImageEdit((s) => s.docs[docId]?.selected ?? null);
  const crop = useImageEdit((s) => s.docs[docId]?.crop ?? null);
  const placing = useImageEdit((s) => s.docs[docId]?.placing ?? null);
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDragState] = useState<Drag | null>(null);
  // The same drag, readable by every handler of one pointer event (a handle and its frame both hear it).
  const dragRef = useRef<Drag | null>(null);
  const setDrag = (g: Drag | null) => {
    dragRef.current = g;
    setDragState(g);
  };
  const [laid, setLaid] = useState<{ a: [number, number]; b: [number, number] } | null>(null);

  useEffect(() => {
    if (visible) void ie().load(docId, page);
  }, [docId, page, rev, visible]);

  const toPage = (e: { clientX: number; clientY: number }): [number, number] => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = space.current!.parentElement!.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [Math.min(Math.max(q.x, 0), rawSize[0]), Math.min(Math.max(q.y, 0), rawSize[1])];
  };

  const mine: ImageBlock | null = selected && selected.page === page ? (listed?.images[selected.index] ?? null) : null;

  // Keys for the selected image: Delete removes it, the arrows nudge it by a point (ten with Shift, along the screen).
  useEffect(() => {
    if (!mine) return;
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName) || t.isContentEditable)) return;
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const cropping = ie().docs[docId]?.crop != null;
      if (e.key === "Escape") {
        e.preventDefault();
        if (cropping) ie().cancelCrop(docId);
        else ie().select(docId, null);
        return;
      }
      // Keys in the Edit rail or a bookmark list belong to what has the focus there (Enter presses a button).
      if (t?.closest?.(".bm-list, .te-dock")) return;
      if (e.key === "Enter" && cropping) {
        e.preventDefault();
        void ie().applyCrop(docId);
      } else if ((e.key === "Delete" || e.key === "Backspace") && !cropping) {
        e.preventDefault();
        void ie().remove(docId);
      } else if (e.key.startsWith("Arrow") && !cropping) {
        e.preventDefault();
        const n = e.shiftKey ? 10 : 1;
        const s: [number, number] = e.key === "ArrowLeft" ? [-n, 0] : e.key === "ArrowRight" ? [n, 0] : e.key === "ArrowUp" ? [0, -n] : [0, n];
        // The view's rotation undone: what points right on the screen points elsewhere in the page.
        const p: [number, number] = [s, [s[1], -s[0]] as [number, number], [-s[0], -s[1]] as [number, number], [-s[1], s[0]] as [number, number]][rot];
        void ie().move(docId, p);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [mine, docId, rot]);

  /** Pointer events on the page are default-prevented, so a rail button would keep the focus (and the keys). */
  const leaveRail = () => {
    const a = document.activeElement as HTMLElement | null;
    if (a?.closest?.(".te-dock, .bm-list")) a.blur();
  };

  const down = (e: React.PointerEvent, b: ImageBlock) => {
    e.stopPropagation();
    e.preventDefault();
    leaveRail();
    if (!b.editable) {
      ie().select(docId, null);
      ie().setNotice(docId, b.reason ? `This image can't be edited here: ${b.reason}.` : "This image can't be edited here.");
      return;
    }
    ie().select(docId, { page, index: b.index });
  };

  // ---- dragging the image, its handles and the crop rectangle ----
  const startDrag = (e: React.PointerEvent, what: Drag["what"], cropping: boolean) => {
    e.stopPropagation();
    e.preventDefault();
    leaveRail();
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ what, crop: cropping, start: toPage(e), d: [0, 0], free: e.shiftKey });
  };
  const dragTo = (e: React.PointerEvent) => {
    const g = dragRef.current;
    if (!g) return;
    const q = toPage(e);
    setDrag({ ...g, d: [q[0] - g.start[0], q[1] - g.start[1]], free: e.shiftKey });
  };
  /** The rectangle a drag makes of `r`: moved, or with the handle's sides moved. */
  const dragged = (r: Rect4, g: Drag): Rect4 =>
    g.what === "move" ? moveRect(r, g.d) : dragHandle(r, g.what, g.d, g.free || g.crop);
  /** A drag the system took away (a lost capture, a cancelled touch) is dropped: nothing is sent. */
  const dragCancel = () => setDrag(null);
  const dragEnd = () => {
    const g = dragRef.current;
    setDrag(null);
    if (!g || !mine || Math.hypot(g.d[0], g.d[1]) * zoom < 3) return;
    if (g.crop) {
      if (crop) ie().setCrop(docId, clampRect(dragged(crop.rect, g), mine.fullRect));
    } else if (g.what === "move") {
      void ie().move(docId, g.d);
    } else {
      void ie().resizeTo(docId, dragged(mine.fullRect, g), isCorner(g.what) && !g.free);
    }
  };

  const dragEvents = { onPointerMove: dragTo, onPointerUp: dragEnd, onPointerCancel: dragCancel, onLostPointerCapture: dragCancel };

  // ---- "place here" for an image file chosen with Image ----
  const laidRect = laid ? placement(laid.a, laid.b, rawSize) : null;

  const frame = mine ? (drag && !drag.crop ? dragged(mine.fullRect, drag) : mine.fullRect) : null;
  const cropRect = crop && mine ? (drag?.crop ? clampRect(dragged(crop.rect, drag), mine.fullRect) : crop.rect) : null;

  return (
    <div
      ref={space}
      className="te-space"
      style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
    >
      {mine && !crop && (
        <div
          className="te-bg"
          onPointerDown={(e) => {
            e.stopPropagation();
            leaveRail();
            ie().select(docId, null);
          }}
        />
      )}
      {placing && (
        <div
          className="te-capture ie-place"
          onPointerDown={(e) => {
            e.stopPropagation();
            e.preventDefault();
            e.currentTarget.setPointerCapture(e.pointerId);
            const a = toPage(e);
            setLaid({ a, b: a });
          }}
          onPointerMove={(e) => laid && setLaid({ a: laid.a, b: toPage(e) })}
          onPointerUp={() => {
            if (!laid) return;
            const rect = placement(laid.a, laid.b, rawSize);
            setLaid(null);
            void ie().place(docId, page, rect);
          }}
          onPointerCancel={() => setLaid(null)}
        />
      )}
      {laidRect && laid && <div className="ie-ghost" style={boxStyle(laidRect)} />}

      {listed?.images.map((b) =>
        mine && b.index === mine.index ? null : (
          <div
            key={b.index}
            className={b.editable ? "ie-block" : "ie-block locked"}
            style={boxStyle(b.rect)}
            title={b.editable ? "Click to edit this image" : `Can't be edited here: ${b.reason ?? "this image"}`}
            onPointerDown={(e) => down(e, b)}
          />
        ),
      )}

      {mine && frame && (
        <>
          {rectChanged(mine.rect, mine.fullRect) && <div className="ie-visible" style={boxStyle(mapRect(mine.rect, mine.fullRect, frame))} />}
          <div
            className={crop ? "ie-frame quiet" : "ie-frame"}
            style={boxStyle(frame)}
            role="group"
            aria-label="Selected image"
            onPointerDown={(e) => !crop && startDrag(e, "move", false)}
            {...dragEvents}
          >
            {!crop &&
              HANDLES.map((h) => (
                <span
                  key={h}
                  className={`ie-h ${h}`}
                  title={`${LABEL[h]}${isCorner(h) ? " (hold Shift to stretch)" : ""}`}
                  onPointerDown={(e) => startDrag(e, h, false)}
                  {...dragEvents}
                />
              ))}
          </div>
        </>
      )}

      {mine && cropRect && (
        <>
          {dims(mine.fullRect, cropRect).map((r, i) => (
            <div key={i} className="ie-dim" style={boxStyle(r)} />
          ))}
          <div
            className="ie-crop"
            style={boxStyle(cropRect)}
            title="Drag to move the part that stays"
            onPointerDown={(e) => startDrag(e, "move", true)}
            {...dragEvents}
          >
            {HANDLES.map((h) => (
              <span
                key={h}
                className={`ie-h ${h}`}
                title={LABEL[h].replace("Resize", "Crop")}
                onPointerDown={(e) => startDrag(e, h, true)}
                {...dragEvents}
              />
            ))}
          </div>
        </>
      )}
    </div>
  );
}

const boxStyle = (r: Rect4) => ({ left: r[0], top: r[1], width: Math.max(rectW(r), 0), height: Math.max(rectH(r), 0) });
const rectChanged = (a: Rect4, b: Rect4) => a.some((v, i) => Math.abs(v - b[i]) > 0.5);
