import { memo, useEffect, useRef, useState } from "react";
import type { ImageBlock } from "../../lib/api";
import { groupDelta, normBox } from "../../lib/formsLogic";
import {
  type Handle,
  type Rect4,
  clampRect,
  dims,
  dragHandle,
  dragTurn,
  isCorner,
  isQuarter,
  mapRect,
  marqueeImages,
  moveRect,
  placement,
  rectH,
  rectW,
  turnQuad,
  useImageEdit,
} from "../../lib/imageEdit";
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

/** What is being dragged: the whole image (or the group), one of its handles, the turn knob, or the crop rectangle. */
type Drag = { target?: { page: number; index: number }; what: "move" | "rotate" | Handle; crop: boolean; start: [number, number]; d: [number, number]; free: boolean; group?: boolean };

/** Cmd, Ctrl or Shift: adds to the picked images instead of replacing them. */
const additive = (e: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean }) => e.metaKey || e.ctrlKey || e.shiftKey;

function Layer({ docId, page, rev, rot, rawSize, zoom, visible }: Props) {
  const listed = useImageEdit((s) => s.docs[docId]?.pages[page]);
  const selected = useImageEdit((s) => s.docs[docId]?.selected ?? null);
  const crop = useImageEdit((s) => s.docs[docId]?.crop ?? null);
  const placing = useImageEdit((s) => s.docs[docId]?.placing ?? null);
  const extra = useImageEdit((s) => s.docs[docId]?.extra);
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDragState] = useState<Drag | null>(null);
  // The same drag, readable by every handler of one pointer event (a handle and its frame both hear it).
  const dragRef = useRef<Drag | null>(null);
  const setDrag = (g: Drag | null) => {
    dragRef.current = g;
    setDragState(g);
  };
  const [laid, setLaid] = useState<{ a: [number, number]; b: [number, number] } | null>(null);
  const [marquee, setMarquee] = useState<{ a: [number, number]; b: [number, number] } | null>(null);

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
  // The other picked images of this page (the anchor is `mine`).
  const others: ImageBlock[] =
    mine && selected
      ? (extra ?? []).flatMap((i) => {
          const b = listed?.images[i];
          return b && b.index !== mine.index ? [b] : [];
        })
      : [];
  const grouped = others.length > 0;
  const locked = listed?.locked ?? null;

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
    if (additive(e)) ie().toggle(docId, { page, index: b.index });
    else ie().select(docId, { page, index: b.index });
  };

  // ---- a marquee: a drag on the empty page, or Shift (Cmd) and a drag anywhere free of an image ----
  const track = (e: { clientX: number; clientY: number }, add: boolean) => {
    const a = toPage(e);
    let b = a;
    setMarquee({ a, b });
    const move = (ev: PointerEvent) => {
      b = toPage(ev);
      setMarquee({ a, b });
    };
    const done = (ev: PointerEvent, cancelled: boolean) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      setMarquee(null);
      if (cancelled) return;
      b = toPage(ev);
      // A click without a real box leaves the selection alone when adding, and clears it otherwise.
      if (Math.hypot(b[0] - a[0], b[1] - a[1]) * zoom < 4) {
        if (!add) ie().select(docId, null);
        return;
      }
      const list = ie().docs[docId]?.pages[page]?.images ?? [];
      ie().pickMany(docId, page, marqueeImages(list, normBox(a, b)), add);
    };
    const up = (ev: PointerEvent) => done(ev, false);
    const cancel = (ev: PointerEvent) => done(ev, true);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
  };

  // Shift (Cmd) and a drag on the page, where no image or text block is, picks the images the box touches.
  useEffect(() => {
    const host = space.current?.parentElement;
    if (!host || !visible) return;
    const onDown = (e: PointerEvent) => {
      if (e.button !== 0 || !(e.shiftKey || e.metaKey) || ie().docs[docId]?.placing || ie().docs[docId]?.crop) return;
      const t = e.target as HTMLElement | null;
      if (t?.closest?.(".ie-block, .ie-frame, .ie-h, .ie-rot, .te-block, .te-editor, input, textarea, select, button, a")) return;
      e.preventDefault();
      e.stopPropagation();
      track(e, true);
    };
    host.addEventListener("pointerdown", onDown, true);
    return () => host.removeEventListener("pointerdown", onDown, true);
  });

  // ---- dragging the image, its handles and the crop rectangle ----
  const startDrag = (e: React.PointerEvent, what: Drag["what"], cropping: boolean, target?: { page: number; index: number }) => {
    e.stopPropagation();
    e.preventDefault();
    leaveRail();
    if (locked && !cropping) {
      ie().setNotice(docId, locked);
      return;
    }
    // Shift or Cmd on a picked image takes it out of the picks (or adds it) instead of dragging it.
    if (what === "move" && !cropping && additive(e) && target) {
      ie().toggle(docId, target);
      return;
    }
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ what, crop: cropping, start: toPage(e), d: [0, 0], free: e.shiftKey, group: what === "move" && !cropping && grouped, target });
  };
  const dragTo = (e: React.PointerEvent) => {
    const g = dragRef.current;
    if (!g) return;
    const q = toPage(e);
    setDrag({ ...g, d: [q[0] - g.start[0], q[1] - g.start[1]], free: e.shiftKey });
  };
  /** The rectangle a drag makes of `r`: moved, or with the handle's sides moved. */
  const dragged = (r: Rect4, g: Drag): Rect4 =>
    g.what === "move" ? moveRect(r, g.d) : g.what === "rotate" ? r : dragHandle(r, g.what, g.d, g.free || g.crop);
  /** How far the picked group moves in a drag: as one block that stays on the page (the forms rule). */
  const groupMove = (g: Drag): [number, number] =>
    groupDelta([...(mine ? [mine.fullRect] : []), ...others.map((o) => o.fullRect)], g.d[0], g.d[1], rawSize);
  /** The turn a rotate drag has made so far, about the picked image's centre. */
  const turned = (g: Drag): number => {
    if (!mine || g.what !== "rotate") return 0;
    const c: [number, number] = [(mine.fullRect[0] + mine.fullRect[2]) / 2, (mine.fullRect[1] + mine.fullRect[3]) / 2];
    return dragTurn(c, g.start, [g.start[0] + g.d[0], g.start[1] + g.d[1]], g.free);
  };
  /** A drag the system took away (a lost capture, a cancelled touch) is dropped: nothing is sent. */
  const dragCancel = () => setDrag(null);
  const dragEnd = () => {
    const g = dragRef.current;
    setDrag(null);
    if (!g || !mine) return;
    const far = Math.hypot(g.d[0], g.d[1]) * zoom >= 3;
    if (!far) {
      // A click on a picked image of a group narrows the picks to it, as in the forms.
      if (g.group && g.target) ie().select(docId, g.target);
      return;
    }
    if (g.crop) {
      if (crop) ie().setCrop(docId, clampRect(dragged(crop.rect, g), mine.fullRect));
    } else if (g.what === "rotate") {
      const t = turned(g);
      if (Math.abs(t) >= 0.5) void ie().turn(docId, t);
    } else if (g.what === "move") {
      const d: [number, number] = g.group ? groupMove(g) : g.d;
      if (d[0] !== 0 || d[1] !== 0) void ie().move(docId, d);
    } else {
      // A turned picture is only ever scaled evenly (the engine refuses a shear).
      void ie().resizeTo(docId, dragged(mine.fullRect, g), (isCorner(g.what) && !g.free) || !isQuarter(mine.angle));
    }
  };

  const dragEvents = { onPointerMove: dragTo, onPointerUp: dragEnd, onPointerCancel: dragCancel, onLostPointerCapture: dragCancel };

  // ---- "place here" for an image file chosen with Image ----
  const laidRect = laid ? placement(laid.a, laid.b, rawSize) : null;

  const groupShift: [number, number] | null = drag && drag.group && !drag.crop && drag.what === "move" ? groupMove(drag) : null;
  const frame = mine
    ? groupShift
      ? moveRect(mine.fullRect, groupShift)
      : drag && !drag.crop && drag.what !== "rotate"
        ? dragged(mine.fullRect, drag)
        : mine.fullRect
    : null;
  // The picture's own outline: shown for a turned one, and while the knob is dragged.
  const turn = drag?.what === "rotate" ? turned(drag) : 0;
  const centre: [number, number] | null = mine ? [(mine.fullRect[0] + mine.fullRect[2]) / 2, (mine.fullRect[1] + mine.fullRect[3]) / 2] : null;
  const outline = mine && centre && (turn !== 0 || !isQuarter(mine.angle)) ? turnQuad(mine.quad, centre, turn) : null;
  const canTurn = mine != null && mine.editable && !crop && !locked && !grouped;
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
            e.preventDefault();
            leaveRail();
            track(e, additive(e));
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
      {marquee && <div className="ie-marquee" style={boxStyle(normBox(marquee.a, marquee.b))} />}

      {listed?.images.map((b) =>
        (mine && b.index === mine.index) || others.some((o) => o.index === b.index) ? null : (
          <div
            key={b.index}
            className={b.editable ? "ie-block" : "ie-block locked"}
            style={boxStyle(b.rect)}
            title={b.editable ? "Click to edit this image" : `Can't be edited here: ${b.reason ?? "this image"}`}
            onPointerDown={(e) => down(e, b)}
          />
        ),
      )}

      {others.map((o) => (
        <div
          key={`p${o.index}`}
          className="ie-frame group"
          style={boxStyle(groupShift ? moveRect(o.fullRect, groupShift) : o.fullRect)}
          role="group"
          aria-label="Picked image"
          onPointerDown={(e) => startDrag(e, "move", false, { page, index: o.index })}
          {...dragEvents}
        />
      ))}

      {outline && (
        <svg className="ie-quad" width={rawSize[0]} height={rawSize[1]} aria-hidden="true">
          <polygon points={[0, 1, 2, 3].map((i) => `${outline[2 * i]},${outline[2 * i + 1]}`).join(" ")} />
        </svg>
      )}

      {mine && frame && (
        <>
          {rectChanged(mine.rect, mine.fullRect) && <div className="ie-visible" style={boxStyle(mapRect(mine.rect, mine.fullRect, frame))} />}
          <div
            className={crop ? "ie-frame quiet" : "ie-frame"}
            style={boxStyle(frame)}
            role="group"
            aria-label="Selected image"
            onPointerDown={(e) => !crop && startDrag(e, "move", false, selected ?? undefined)}
            {...dragEvents}
          >
            {canTurn && (
              <span
                className="ie-rot"
                title="Drag to turn (hold Shift for steps of 15°)"
                onPointerDown={(e) => startDrag(e, "rotate", false)}
                {...dragEvents}
              />
            )}
            {!crop &&
              !locked &&
              !grouped &&
              HANDLES.filter((h) => isQuarter(mine.angle) || isCorner(h)).map((h) => (
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
