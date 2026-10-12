import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { ImageBlock, ImageChange } from "./api";
import { editImage, editImages, moveImagesToPage, pasteImages } from "./edit";
import { useTextEdit } from "./textEdit";

/**
 * Editing the pictures of a page in place, in Edit mode: a page's images come from one `image_blocks`
 * call per page and document generation (cached here); every change is one `image_edit` and so one undo
 * step. See docs/image-edit.md for what the engine can and cannot do.
 */

export type Rect4 = [number, number, number, number];
export type Handle = "n" | "s" | "e" | "w" | "nw" | "ne" | "sw" | "se";

export interface ImageRef {
  page: number;
  index: number;
}

interface PageImages {
  /** The document generation the list belongs to. */
  rev: number;
  images: ImageBlock[];
  /** Why the page's images can't be changed (a certified document); the controls say so. */
  locked: string | null;
  signed: boolean;
  /** Why they can't be copied out (the owner's permissions). */
  copyLocked: string | null;
}

export interface Crop {
  /** The part to keep, in page space. */
  rect: Rect4;
  /** Drop the pixels outside (a smaller file), instead of hiding them behind a clip. */
  pixels: boolean;
}

export interface DocImageEdit {
  pages: Record<number, PageImages>;
  /** The anchor: the picked image the rail and the handles belong to (the last one picked). */
  selected: ImageRef | null;
  /** Further picked images, of the anchor's page (shift or cmd click, or a marquee). */
  extra: number[];
  /** A quiet sentence in the rail: why an image is locked, or that the page changed under a change. */
  notice: string | null;
  crop: Crop | null;
  /** A file chosen with "Image", waiting for its place on a page. */
  placing: { file: string; name: string } | null;
}

const empty = (): DocImageEdit => ({ pages: {}, selected: null, extra: [], notice: null, crop: null, placing: null });

/** Every picked image, the anchor last. */
export function picksOf(d: Pick<DocImageEdit, "selected" | "extra">): ImageRef[] {
  if (!d.selected) return [];
  const page = d.selected.page;
  return [...d.extra.map((index) => ({ page, index })), d.selected];
}

/** A click toggles `ref` in the picks of its page: an image that was picked goes, another becomes the anchor. */
export function togglePick(d: Pick<DocImageEdit, "selected" | "extra">, ref: ImageRef): ImageRef[] {
  const cur = picksOf(d);
  if (cur.length === 0 || cur[0].page !== ref.page) return [ref];
  return cur.some((p) => p.index === ref.index) ? cur.filter((p) => p.index !== ref.index) : [...cur, ref];
}

// ---- pure helpers (tested in imageEdit.test.ts) ----

/** The smallest side a drag can make an image, in points. */
export const MIN_SIDE = 8;
/** The side of the square an image is put in when the page is clicked instead of dragged. */
export const CLICK_BOX = 180;

export const rectW = (r: Rect4) => r[2] - r[0];
export const rectH = (r: Rect4) => r[3] - r[1];
const centre = (r: Rect4): [number, number] => [(r[0] + r[2]) / 2, (r[1] + r[3]) / 2];
export const moveRect = (r: Rect4, d: [number, number]): Rect4 => [r[0] + d[0], r[1] + d[1], r[2] + d[0], r[3] + d[1]];

/** A corner keeps the image's proportions; a side stretches one way. */
export const isCorner = (h: Handle) => h.length === 2;

/**
 * `r` after the handle `h` was dragged by `d` (page space). The side or corner opposite stays where it
 * is; a corner keeps the proportions of `r` unless `free`; no side gets shorter than `min`.
 */
export function dragHandle(r: Rect4, h: Handle, d: [number, number], free = false, min = MIN_SIDE): Rect4 {
  let [x0, y0, x1, y1] = r;
  const west = h.includes("w");
  const east = h.includes("e");
  const north = h.includes("n");
  const south = h.includes("s");
  if (west) x0 += d[0];
  if (east) x1 += d[0];
  if (north) y0 += d[1];
  if (south) y1 += d[1];
  const ow = Math.max(rectW(r), 1e-6);
  const oh = Math.max(rectH(r), 1e-6);
  if (isCorner(h) && !free) {
    // The side that moved further decides the scale, never below the smallest side.
    const sx = (x1 - x0) / ow;
    const sy = (y1 - y0) / oh;
    const s = Math.max(Math.max(sx, sy), min / Math.min(ow, oh));
    const w = ow * s;
    const hh = oh * s;
    if (west) x0 = x1 - w;
    else x1 = x0 + w;
    if (north) y0 = y1 - hh;
    else y1 = y0 + hh;
    return [x0, y0, x1, y1];
  }
  if (x1 - x0 < min) {
    if (west) x0 = x1 - min;
    else x1 = x0 + min;
  }
  if (y1 - y0 < min) {
    if (north) y0 = y1 - min;
    else y1 = y0 + min;
  }
  return [x0, y0, x1, y1];
}

/** `r` kept inside `bounds` (a drag past the page's edge stops there). */
export function clampRect(r: Rect4, bounds: Rect4): Rect4 {
  return [Math.max(bounds[0], r[0]), Math.max(bounds[1], r[1]), Math.min(bounds[2], r[2]), Math.min(bounds[3], r[3])];
}

/** The box a dragged-out image (corners `a` and `b`) takes, or a square at `a` for a click (or a drag too thin to mean it). */
export function placement(a: [number, number], b: [number, number], page: [number, number]): Rect4 {
  const dx = Math.abs(b[0] - a[0]);
  const dy = Math.abs(b[1] - a[1]);
  if (dx < MIN_SIDE || dy < MIN_SIDE) {
    const side = Math.min(CLICK_BOX, page[0], page[1]);
    const x = Math.min(Math.max(a[0], 0), page[0] - side);
    const y = Math.min(Math.max(a[1], 0), page[1] - side);
    return [x, y, x + side, y + side];
  }
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
}

/**
 * Where an image's whole box ends up after `change`: what the list is searched for afterwards. Null when
 * the image is gone.
 */
export function expectedBox(b: ImageBlock, change: ImageChange): Rect4 | null {
  const f = b.fullRect;
  switch (change.op) {
    case "delete":
      return null;
    case "move":
      return moveRect(f, [change.dx, change.dy]);
    case "resize": {
      if (!change.keepAspect) return change.rect;
      const s = Math.min(rectW(change.rect) / Math.max(rectW(f), 1e-6), rectH(change.rect) / Math.max(rectH(f), 1e-6));
      return [change.rect[0], change.rect[1], change.rect[0] + rectW(f) * s, change.rect[1] + rectH(f) * s];
    }
    case "rotate": {
      if (((change.quarterTurns % 2) + 2) % 2 === 0) return f;
      const [cx, cy] = centre(f);
      const hw = rectH(f) / 2;
      const hh = rectW(f) / 2;
      return [cx - hw, cy - hh, cx + hw, cy + hh];
    }
    case "angle":
      // The centre stays, which is what the list is searched by.
      return f;
    case "crop":
      return change.pixels ? clampRect(change.rect, f) : f;
    case "add":
      return change.rect;
    default:
      return f;
  }
}

/**
 * The image of `next` nearest to `box` by its centre, or null when none is near enough (it is another
 * image, or none). Only images that can be edited are offered.
 */
export function closestImage(box: Rect4, next: ImageBlock[]): ImageBlock | null {
  const [cx, cy] = centre(box);
  let best: ImageBlock | null = null;
  let bestDist = Infinity;
  for (const n of next) {
    if (!n.editable) continue;
    const [x, y] = centre(n.fullRect);
    const dist = Math.hypot(x - cx, y - cy);
    if (dist < bestDist) {
      best = n;
      bestDist = dist;
    }
  }
  return best && bestDist <= Math.max(12, Math.max(rectW(box), rectH(box)) * 0.35) ? best : null;
}

/** `r`, which lay in `from`, in the same place of `to` (the visible part of an image whose box is being resized). */
export function mapRect(r: Rect4, from: Rect4, to: Rect4): Rect4 {
  const sx = rectW(to) / Math.max(rectW(from), 1e-6);
  const sy = rectH(to) / Math.max(rectH(from), 1e-6);
  return [to[0] + (r[0] - from[0]) * sx, to[1] + (r[1] - from[1]) * sy, to[0] + (r[2] - from[0]) * sx, to[1] + (r[3] - from[1]) * sy];
}

/** The four strips of `outer` around `inner`: what a crop would hide. */
export function dims(outer: Rect4, inner: Rect4): Rect4[] {
  return [
    [outer[0], outer[1], outer[2], inner[1]],
    [outer[0], inner[3], outer[2], outer[3]],
    [outer[0], inner[1], inner[0], inner[3]],
    [inner[2], inner[1], outer[2], inner[3]],
  ];
}

// ---- several images: marquee, align, free angle ----

/**
 * The editable images of a page a marquee `box` touches. The rule is "touch": any overlap counts, which
 * includes an image the box contains and one that contains the box (the forms marquee does the same).
 * What shows of an image is tested, and a turned image by its own outline, not its bounding box.
 */
export function marqueeImages(images: ImageBlock[], box: Rect4): number[] {
  const [x0, y0, x1, y1] = box;
  return images
    .filter((b) => {
      if (!b.editable) return false;
      const clipped = b.rect.some((v, i) => Math.abs(v - b.fullRect[i]) > 0.5);
      return clipped || isQuarter(b.angle) ? b.rect[0] < x1 && b.rect[2] > x0 && b.rect[1] < y1 && b.rect[3] > y0 : quadTouches(b.quad, box);
    })
    .map((b) => b.index);
}

/** True when the convex quadrilateral `q` (x, y pairs) overlaps the axis-aligned `box` (separating axes). */
export function quadTouches(q: ImageBlock["quad"], box: Rect4): boolean {
  const pts: [number, number][] = [0, 1, 2, 3].map((i) => [q[2 * i], q[2 * i + 1]]);
  const [bx0, by0, bx1, by1] = box;
  if (Math.max(...pts.map((p) => p[0])) <= bx0 || Math.min(...pts.map((p) => p[0])) >= bx1) return false;
  if (Math.max(...pts.map((p) => p[1])) <= by0 || Math.min(...pts.map((p) => p[1])) >= by1) return false;
  const corners: [number, number][] = [
    [bx0, by0],
    [bx1, by0],
    [bx1, by1],
    [bx0, by1],
  ];
  for (let i = 0; i < 4; i++) {
    const a = pts[i];
    const b = pts[(i + 1) % 4];
    const n: [number, number] = [b[1] - a[1], a[0] - b[0]];
    const proj = (p: [number, number]) => p[0] * n[0] + p[1] * n[1];
    const [lo, hi] = [Math.min(...pts.map(proj)), Math.max(...pts.map(proj))];
    const cs = corners.map(proj);
    if (Math.max(...cs) <= lo || Math.min(...cs) >= hi) return false;
  }
  return true;
}

/** The most pictures one change takes (the engine's `MAX_BATCH`): the picks stop here. */
export const MAX_PICKS = 100;

/** The box around several rectangles. */
export function groupBox(rects: Rect4[]): Rect4 {
  return [Math.min(...rects.map((r) => r[0])), Math.min(...rects.map((r) => r[1])), Math.max(...rects.map((r) => r[2])), Math.max(...rects.map((r) => r[3]))];
}

/** How far each paste after the first lands from the one before it, in points: visible, not far. */
export const PASTE_STEP = 16;

/**
 * Where the top left corner of a paste goes: the copied group's own place, `nth` steps down and right
 * (`nth` counts the pastes already made on that page, plus one when the page is the one it was copied from,
 * so a paste never hides exactly under its original).
 */
export function pastePoint(rect: Rect4, nth: number, step = PASTE_STEP): [number, number] {
  return [rect[0] + step * nth, rect[1] + step * nth];
}

/**
 * Where a group dropped on another page has its top left corner: the pointer's place there, less how far the
 * pointer was inside the group when it was picked up (`grab`: the pointer at the start less the group's corner).
 */
export function dropPoint(pointer: [number, number], start: [number, number], corner: [number, number]): [number, number] {
  return [pointer[0] - (start[0] - corner[0]), pointer[1] - (start[1] - corner[1])];
}

export type AlignHow = "left" | "hcenter" | "right" | "top" | "vcenter" | "bottom";

/** How far each rectangle moves to line up with the group's box (page space): one move per image. */
export function alignDeltas(rects: Rect4[], how: AlignHow): [number, number][] {
  if (rects.length === 0) return [];
  const g = groupBox(rects);
  return rects.map((r) => {
    switch (how) {
      case "left":
        return [g[0] - r[0], 0];
      case "right":
        return [g[2] - r[2], 0];
      case "hcenter":
        return [(g[0] + g[2]) / 2 - (r[0] + r[2]) / 2, 0];
      case "top":
        return [0, g[1] - r[1]];
      case "bottom":
        return [0, g[3] - r[3]];
      case "vcenter":
        return [0, (g[1] + g[3]) / 2 - (r[1] + r[3]) / 2];
    }
  });
}

/** An angle brought into (-180, 180]. */
export function normAngle(deg: number): number {
  const a = ((((deg + 180) % 360) + 360) % 360) - 180;
  return a === -180 ? 180 : a;
}

/** True for a multiple of 90 degrees (an upright or quarter-turned image: its box is its shape). */
export const isQuarter = (deg: number) => Math.abs(deg / 90 - Math.round(deg / 90)) < 0.0002;

/** Round to a hundredth of a degree: what the field shows and sends. */
export const roundAngle = (deg: number) => Math.round(deg * 100) / 100;

/**
 * The turn (degrees clockwise) that takes an image now at `current` to `typed`: the shortest way, null when
 * it is the same. The typed value is what counts (to a hundredth); the turn is measured from where the image
 * really is, so an angle that was stored a little off is not carried on.
 */
export function turnTo(current: number, typed: number): number | null {
  const d = Math.round(normAngle(roundAngle(typed) - current) * 1e4) / 1e4;
  return Math.abs(d) < 0.005 ? null : d;
}

/** The picture's own width and height in points (the sides of its outline, not of the box that holds it turned). */
export function ownSize(b: ImageBlock): [number, number] {
  const q = b.quad;
  return [Math.hypot(q[2] - q[0], q[3] - q[1]), Math.hypot(q[6] - q[0], q[7] - q[1])];
}

/**
 * The frame box to send for the picture to have the own size `w` × `h`, its top left staying. A turned picture is
 * scaled evenly (by `w` over its own width): its frame grows by the same factor, so the picture's centre moves.
 * An upright or quarter-turned one is its frame; a quarter turn swaps the sides.
 */
export function sizeRect(b: ImageBlock, w: number, h: number): Rect4 {
  const [x, y] = b.fullRect;
  if (!isQuarter(b.angle)) {
    const s = w / Math.max(ownSize(b)[0], 1e-6);
    return [x, y, x + rectW(b.fullRect) * s, y + rectH(b.fullRect) * s];
  }
  return Math.round(Math.abs(b.angle) / 90) % 2 === 1 ? [x, y, x + h, y + w] : [x, y, x + w, y + h];
}

/** The angle of `p` seen from `c` in page space (y down), degrees clockwise from straight up. */
export function bearing(c: [number, number], p: [number, number]): number {
  return (Math.atan2(p[0] - c[0], c[1] - p[1]) * 180) / Math.PI;
}

/** The turn a rotate-handle drag makes: from where the pointer started to where it is, snapped to 15 degrees with `snap`. */
export function dragTurn(c: [number, number], from: [number, number], to: [number, number], snap: boolean): number {
  const d = normAngle(bearing(c, to) - bearing(c, from));
  return snap ? normAngle(Math.round(d / 15) * 15) : roundAngle(d);
}

/** `quad` turned `deg` degrees clockwise (as seen) about `c`: the preview of a turn. */
export function turnQuad(quad: ImageBlock["quad"], c: [number, number], deg: number): ImageBlock["quad"] {
  const [s, k] = [Math.sin((deg * Math.PI) / 180), Math.cos((deg * Math.PI) / 180)];
  const out = quad.slice() as ImageBlock["quad"];
  for (let i = 0; i < 4; i++) {
    const x = quad[2 * i] - c[0];
    const y = quad[2 * i + 1] - c[1];
    out[2 * i] = c[0] + x * k - y * s;
    out[2 * i + 1] = c[1] + x * s + y * k;
  }
  return out;
}

/** The same image's resolution in pixels per inch along its own width (a turned image included). */
export function ppi(b: ImageBlock): number {
  const len = Math.hypot(b.matrix[0], b.matrix[1]);
  return len > 0 ? b.width / (len / 72) : 0;
}

/** How the image's data is compressed, for a person: JPEG, Flate ... */
export function codecName(filter: string[]): string {
  const names: Record<string, string> = {
    DCTDecode: "JPEG",
    DCT: "JPEG",
    JPXDecode: "JPEG 2000",
    CCITTFaxDecode: "CCITT fax",
    CCF: "CCITT fax",
    JBIG2Decode: "JBIG2",
    FlateDecode: "Flate",
    Fl: "Flate",
    LZWDecode: "LZW",
    LZW: "LZW",
    RunLengthDecode: "Run length",
    RL: "Run length",
  };
  const pick = filter.find((f) => ["DCTDecode", "DCT", "JPXDecode", "CCITTFaxDecode", "CCF", "JBIG2Decode"].includes(f)) ?? filter.find((f) => names[f]);
  return pick ? names[pick] : filter.length === 0 ? "Uncompressed" : filter[0].replace(/Decode$/, "");
}

/** `DeviceRGB` as a person says it: RGB. */
export function colorName(cs: string): string {
  const n: Record<string, string> = {
    DeviceRGB: "RGB",
    CalRGB: "RGB",
    DeviceGray: "Grey",
    CalGray: "Grey",
    DeviceCMYK: "CMYK",
    ICCBased: "ICC colour",
    Indexed: "Indexed colour",
    Lab: "Lab colour",
    ImageMask: "Stencil mask",
    Separation: "Spot colour",
    DeviceN: "Spot colours",
  };
  return n[cs] ?? cs;
}

/** One line about an image for the rail: `1200 × 800 px · 150 ppi`. */
export function imageSummary(b: ImageBlock): string {
  const r = Math.round(ppi(b));
  return `${b.width} × ${b.height} px${r > 0 ? ` · ${r} ppi` : ""}`;
}

/** "Image 2 of 3" style text for a list entry. */
export const imageLabel = (b: ImageBlock) => `${b.width} × ${b.height} px`;

// ---- store ----

interface Store {
  docs: Record<number, DocImageEdit>;

  /** Lists the page's images for the document's current generation (cached; one call per page and generation). */
  load: (doc: number, page: number) => Promise<void>;
  select: (doc: number, ref: ImageRef | null) => void;
  /** Shift or cmd click: adds the image to the picks of its page, or takes it out (one page at a time). */
  toggle: (doc: number, ref: ImageRef) => void;
  /** A marquee on `page`: the images it touched (added to the picks with `add`, else replacing them). */
  pickMany: (doc: number, page: number, indices: number[], add: boolean) => void;
  setNotice: (doc: number, notice: string | null) => void;

  /**
   * Sends `change` for the selected image as one undo step. True when it was applied. `target`: the image the
   * caller means (a typed size is committed as a field loses focus, when another image may be selected already):
   * that image is changed, and the selection stays where it is.
   */
  apply: (doc: number, change: ImageChange, target?: ImageRef) => Promise<boolean>;
  move: (doc: number, d: [number, number]) => Promise<boolean>;
  resizeTo: (doc: number, rect: Rect4, keepAspect: boolean, target?: ImageRef) => Promise<boolean>;
  rotate: (doc: number, quarterTurns: number) => Promise<boolean>;
  flip: (doc: number, horizontal: boolean) => Promise<boolean>;
  remove: (doc: number) => Promise<boolean>;
  /** Turns the picked images by `degrees` clockwise about their own centres. */
  turn: (doc: number, degrees: number, target?: ImageRef) => Promise<boolean>;
  /** Lines the picked images up with the edge or centre of the group, one undo step. */
  align: (doc: number, how: AlignHow) => Promise<boolean>;
  /** Asks for an image file and draws it instead of the selected image. */
  replace: (doc: number) => Promise<boolean>;

  /** What the picture clipboard holds (shared by the open documents), and where it came from. */
  clip: { info: api.ImageClipInfo; from: { doc: number; page: number } | null } | null;
  /** Pastes made on each `doc:page` since the last copy, for the offset. */
  pastes: Record<string, number>;
  /** Copies the picked pictures to the clipboard (inline ones can only be moved). */
  copy: (doc: number) => Promise<boolean>;
  /** Pastes the clipboard on `page`; the pasted pictures become the picks. */
  paste: (doc: number, page: number) => Promise<boolean>;
  /** Moves the picked pictures to `toPage`, the group's top left corner at `at` there (page space). */
  transfer: (doc: number, toPage: number, at: [number, number]) => Promise<boolean>;

  /** Asks for an image file; the next place chosen on a page gets it. */
  startPlacing: (doc: number) => Promise<void>;
  cancelPlacing: (doc: number) => void;
  /** Puts the chosen file in `rect` of `page` and selects the new image. */
  place: (doc: number, page: number, rect: Rect4) => Promise<boolean>;

  startCrop: (doc: number) => void;
  setCrop: (doc: number, rect: Rect4) => void;
  setCropPixels: (doc: number, pixels: boolean) => void;
  applyCrop: (doc: number) => Promise<boolean>;
  cancelCrop: (doc: number) => void;
}

const inflight = new Map<string, Promise<void>>();
/** One change of a document at a time, in the order asked for. */
const queues = new Map<number, Promise<unknown>>();
function queued<T>(doc: number, f: () => Promise<T>): Promise<T> {
  const run = (queues.get(doc) ?? Promise.resolve()).then(f, f);
  queues.set(doc, run.catch(() => undefined));
  return run;
}

const STALE = "The page changed under that change, so it was not applied. Try it again.";

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/**
 * Per document: counts every decision about which image is selected (a click, a settled change). A listing
 * that was started before the latest decision must not remap the selection from the box it was started at.
 */
const choices = new Map<number, number>();

export const useImageEdit = create<Store>((set, get) => {
  const patchDoc = (doc: number, p: Partial<DocImageEdit>) => {
    if ("selected" in p) choices.set(doc, (choices.get(doc) ?? 0) + 1);
    // A new anchor starts a new pick unless the caller says which others stay.
    const q: Partial<DocImageEdit> = "selected" in p && !("extra" in p) ? { ...p, extra: [] } : p;
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? empty()), ...q } } }));
  };
  const of = (doc: number): DocImageEdit => get().docs[doc] ?? empty();
  const docRev = (doc: number) => useStore.getState().docs[doc]?.rev ?? 0;
  const selectedImage = (doc: number): ImageBlock | null => {
    const d = of(doc);
    return d.selected ? (d.pages[d.selected.page]?.images[d.selected.index] ?? null) : null;
  };
  /** Documents with a change in flight: that change settles its own selection. */
  const applying = new Set<number>();

  const fetchPage = async (doc: number, page: number) => {
    const r = await api.imageBlocks(doc, page);
    const entry: PageImages = { rev: r.rev, images: r.images, locked: r.locked ?? null, signed: r.signed ?? false, copyLocked: r.copyLocked ?? null };
    patchDoc(doc, { pages: { ...of(doc).pages, [page]: entry } });
    return r;
  };

  /**
   * Lists the page again. When the document moved on under a selection (undo, redo, a restored version),
   * the selection follows its image by position, or is dropped: an index must never point at another image.
   */
  const reload = async (doc: number, page: number) => {
    const before = of(doc);
    const sel = before.selected;
    const was = sel && sel.page === page ? (before.pages[page]?.images[sel.index] ?? null) : null;
    const wasExtra = sel && sel.page === page ? before.extra.map((i) => before.pages[page]?.images[i] ?? null) : [];
    const choice = choices.get(doc) ?? 0;
    const r = await fetchPage(doc, page);
    // Only a selection nobody touched while this listing was on its way is remapped: a change of ours that
    // settled meanwhile (or a click) already chose the image, and `was` is the box from before it.
    if (sel && was && !applying.has(doc) && (choices.get(doc) ?? 0) === choice) {
      const now = of(doc);
      if (now.selected?.page === page && now.selected.index === sel.index) {
        // Every pick follows its image by position; one that is gone is dropped, and two never land on one image.
        const used = new Set<number>();
        const find = (b: ImageBlock) => {
          const m = closestImage(b.fullRect, r.images.filter((x) => !used.has(x.index)));
          if (m) used.add(m.index);
          return m;
        };
        const same = find(was);
        const extra = wasExtra.flatMap((b) => (b ? [find(b)] : [])).flatMap((m) => (m ? [m.index] : []));
        if (!same) patchDoc(doc, { selected: null, crop: null });
        else if (same.index !== sel.index || extra.length !== now.extra.length || extra.some((v, i) => v !== now.extra[i])) {
          patchDoc(doc, { selected: { page, index: same.index }, extra });
        }
      }
    }
    return r;
  };

  /** Why the page of the selected image takes no change (a certified document), with the notice set; null when it may go. */
  const lockedFor = (doc: number): string | null => {
    const d = of(doc);
    const why = d.selected ? (d.pages[d.selected.page]?.locked ?? null) : null;
    if (why) patchDoc(doc, { notice: why });
    return why;
  };

  /**
   * One change to each picked image (`make` gives them for the picked editable images, null skipping one) as one
   * undo step. The picks are found again by position afterwards.
   */
  const applyGroupNow = async (doc: number, make: (images: ImageBlock[]) => (ImageChange | null)[]): Promise<boolean> => {
    const d = of(doc);
    const refs = picksOf(d);
    const page = refs.length > 0 ? d.pages[refs[0].page] : undefined;
    if (!page || refs.length === 0) return false;
    if (lockedFor(doc)) return false;
    const images = refs.map((r) => page.images[r.index]).filter((b): b is ImageBlock => !!b);
    const changes = make(images);
    const items = images.flatMap((b, i) => (b.editable && changes[i] ? [{ b, change: changes[i] as ImageChange }] : []));
    if (items.length === 0) return false;
    const pageNo = refs[0].page;
    applying.add(doc);
    try {
      const r = await editImages(doc, page.rev, pageNo, items.map((it) => ({ index: it.b.index, change: it.change })));
      if (r.kind === "error") {
        patchDoc(doc, { notice: `This change could not be applied: ${r.message}` });
        return false;
      }
      const fresh = await fetchPage(doc, pageNo);
      const used = new Set<number>();
      const find = (b: ImageBlock, c: ImageChange) => {
        const box = expectedBox(b, c);
        const m = box ? closestImage(box, fresh.images.filter((x) => !used.has(x.index))) : null;
        if (m) used.add(m.index);
        return m;
      };
      const found = r.kind === "ok" ? items.map((it) => find(it.b, it.change)) : items.map((it) => find(it.b, { op: "move", dx: 0, dy: 0 }));
      const mine = found.flatMap((m) => (m ? [m.index] : []));
      if (mine.length === 0) patchDoc(doc, { selected: null, crop: null, notice: r.kind === "stale" ? STALE : null });
      else {
        patchDoc(doc, { selected: { page: pageNo, index: mine[mine.length - 1] }, extra: mine.slice(0, -1), crop: null, notice: r.kind === "stale" ? STALE : null });
      }
      return r.kind === "ok";
    } finally {
      applying.delete(doc);
    }
  };

  /** A group change when more than one image is picked (null: one or none, the caller sends it for the one). */
  const grouped = (doc: number, make: (images: ImageBlock[]) => (ImageChange | null)[]): Promise<boolean> | null =>
    picksOf(of(doc)).length > 1 ? queued(doc, () => applyGroupNow(doc, make)) : null;

  const applyNow = async (doc: number, change: ImageChange, target?: ImageRef): Promise<boolean> => {
    const d = of(doc);
    const ref = target ?? d.selected;
    const page = ref ? d.pages[ref.page] : undefined;
    const image = ref ? (page?.images[ref.index] ?? null) : null;
    if (!ref || !image || !page) return false;
    if (page.locked) {
      patchDoc(doc, { notice: page.locked });
      return false;
    }
    applying.add(doc);
    try {
      const r = await editImage(doc, page.rev, ref.page, ref.index, change);
      if (r.kind === "error") {
        patchDoc(doc, { notice: `This change could not be applied: ${r.message}` });
        return false;
      }
      const fresh = await fetchPage(doc, ref.page);
      // Another image was selected (or none) meanwhile: that choice stands; it is found again by position in
      // the new list.
      const cur = of(doc).selected;
      if (!cur || !(cur.page === ref.page && cur.index === ref.index)) {
        const was = cur && cur.page === ref.page ? (d.pages[cur.page]?.images[cur.index] ?? null) : null;
        if (cur && was) {
          const m = closestImage(was.fullRect, fresh.images);
          patchDoc(doc, { selected: m ? { page: cur.page, index: m.index } : null });
        }
        if (r.kind === "stale") patchDoc(doc, { notice: STALE });
        return r.kind === "ok";
      }
      if (r.kind === "stale") {
        const same = closestImage(image.fullRect, fresh.images);
        patchDoc(doc, {
          selected: same ? { page: ref.page, index: same.index } : null,
          crop: null,
          notice: STALE,
        });
        return false;
      }
      const box = expectedBox(image, change);
      const same = box ? closestImage(box, fresh.images) : null;
      patchDoc(doc, { selected: same ? { page: ref.page, index: same.index } : null, crop: null, notice: null });
      return true;
    } finally {
      applying.delete(doc);
    }
  };

  const pickImage = async (): Promise<{ token: string; name: string } | null> => {
    const [f] = await api.pickSource("images", false);
    return f ? { token: f.token, name: baseName(f.path) } : null;
  };

  return {
    docs: {},

    load: (doc, page) => {
      if (!useStore.getState().docs[doc]) return Promise.resolve();
      const rev = docRev(doc);
      if (of(doc).pages[page]?.rev === rev) return Promise.resolve();
      const key = `${doc}:${page}:${rev}`;
      const running = inflight.get(key);
      if (running) return running;
      const job = reload(doc, page)
        .then(() => undefined)
        .catch(() => undefined)
        .finally(() => inflight.delete(key));
      inflight.set(key, job);
      return job;
    },

    select: (doc, ref) => {
      const cur = of(doc).selected;
      if (cur && ref && cur.page === ref.page && cur.index === ref.index) return;
      patchDoc(doc, { selected: ref, crop: null, notice: null });
      // One thing is selected at a time: a picture taken from the rail's list lets go of the text block.
      if (ref) {
        const te = useTextEdit.getState();
        if (te.docs[doc]?.selected) void te.select(doc, null);
      }
    },

    toggle: (doc, ref) => {
      const next = togglePick(of(doc), ref);
      if (next.length === 0) return get().select(doc, null);
      if (next.length > MAX_PICKS) return patchDoc(doc, { notice: `At most ${MAX_PICKS} images can be picked at once.` });
      const anchor = next[next.length - 1];
      patchDoc(doc, { selected: anchor, extra: next.slice(0, -1).map((p) => p.index), crop: null, notice: null });
      const te = useTextEdit.getState();
      if (te.docs[doc]?.selected) void te.select(doc, null);
    },

    pickMany: (doc, page, indices, add) => {
      const keep = add && of(doc).selected?.page === page ? picksOf(of(doc)).map((p) => p.index) : [];
      const all = [...keep.filter((i) => !indices.includes(i)), ...indices];
      if (all.length === 0) {
        if (!add) get().select(doc, null);
        return;
      }
      // A change takes at most MAX_PICKS images: the first ones stay picked and the rail says so.
      const kept = all.slice(0, MAX_PICKS);
      const notice = all.length > kept.length ? `That is ${all.length} images; the first ${MAX_PICKS} are picked, since a change takes at most ${MAX_PICKS} at once.` : null;
      patchDoc(doc, { selected: { page, index: kept[kept.length - 1] }, extra: kept.slice(0, -1), crop: null, notice });
      const te = useTextEdit.getState();
      if (te.docs[doc]?.selected) void te.select(doc, null);
    },

    setNotice: (doc, notice) => patchDoc(doc, { notice }),

    apply: (doc, change, target) => queued(doc, () => applyNow(doc, change, target)),
    move: (doc, d) => grouped(doc, (im) => im.map(() => ({ op: "move", dx: d[0], dy: d[1] }))) ?? get().apply(doc, { op: "move", dx: d[0], dy: d[1] }),
    resizeTo: (doc, rect, keepAspect, target) => get().apply(doc, { op: "resize", rect, keepAspect }, target),
    rotate: (doc, quarterTurns) => grouped(doc, (im) => im.map(() => ({ op: "rotate", quarterTurns }))) ?? get().apply(doc, { op: "rotate", quarterTurns }),
    flip: (doc, horizontal) => grouped(doc, (im) => im.map(() => ({ op: "flip", horizontal }))) ?? get().apply(doc, { op: "flip", horizontal }),
    remove: (doc) => grouped(doc, (im) => im.map(() => ({ op: "delete" }))) ?? get().apply(doc, { op: "delete" }),
    turn: (doc, degrees, target) => grouped(doc, (im) => im.map(() => ({ op: "angle", degrees }))) ?? get().apply(doc, { op: "angle", degrees }, target),
    align: (doc, how) => {
      const run = (im: ImageBlock[]) => {
        const deltas = alignDeltas(im.map((b) => b.fullRect), how);
        return deltas.map(([dx, dy]): ImageChange | null => (Math.abs(dx) < 0.01 && Math.abs(dy) < 0.01 ? null : { op: "move", dx, dy }));
      };
      return grouped(doc, run) ?? Promise.resolve(false);
    },

    clip: null,
    pastes: {},

    copy: (doc) =>
      queued(doc, async () => {
        const d = of(doc);
        const refs = picksOf(d);
        const page = refs.length > 0 ? d.pages[refs[0].page] : undefined;
        if (!page || refs.length === 0) return false;
        if (page.copyLocked) {
          patchDoc(doc, { notice: page.copyLocked });
          return false;
        }
        const picks = refs.flatMap((r) => (page.images[r.index] ? [page.images[r.index]] : []));
        const copyable = picks.filter((b) => b.editable && b.kind === "xObject");
        if (copyable.length === 0) {
          patchDoc(doc, { notice: "A picture drawn inline can be moved to another page, but not copied." });
          return false;
        }
        try {
          const info = await api.imageCopy(doc, page.rev, refs[0].page, copyable.map((b) => b.index));
          set({ clip: { info, from: { doc, page: refs[0].page } }, pastes: {} });
          const skipped = picks.length - copyable.length;
          patchDoc(doc, { notice: skipped > 0 ? `${skipped} of the pictures were not copied: they are drawn inline, or inside text.` : null });
          return true;
        } catch (e) {
          patchDoc(doc, { notice: String(e) === "stale" ? STALE : `The pictures could not be copied: ${String(e)}` });
          return false;
        }
      }),

    paste: (doc, page) =>
      queued(doc, async () => {
        let clip = get().clip;
        if (!clip) {
          const info = await api.imageClipInfo().catch(() => null);
          if (info) {
            clip = { info, from: null };
            set({ clip });
          }
        }
        if (!clip) {
          patchDoc(doc, { notice: "No pictures are copied." });
          return false;
        }
        const cur = of(doc).pages[page];
        const target = cur && cur.rev === docRev(doc) ? cur : await fetchPage(doc, page).then(() => of(doc).pages[page]);
        if (!target) return false;
        if (target.locked) {
          patchDoc(doc, { notice: target.locked });
          return false;
        }
        const key = `${doc}:${page}`;
        const sameSpot = clip.from?.doc === doc && clip.from.page === page;
        const at = pastePoint(clip.info.rect, (get().pastes[key] ?? 0) + (sameSpot ? 1 : 0));
        applying.add(doc);
        try {
          const r = await pasteImages(doc, target.rev, page, at);
          if (r.kind === "error") {
            patchDoc(doc, { notice: `The pictures could not be pasted: ${r.message}` });
            return false;
          }
          const fresh = await fetchPage(doc, page);
          if (r.kind === "stale") {
            patchDoc(doc, { notice: STALE });
            return false;
          }
          set((s) => ({ pastes: { ...s.pastes, [key]: (s.pastes[key] ?? 0) + 1 } }));
          // The pasted pictures are drawn last on the page: they are the picks.
          const mine = fresh.images.slice(-clip.info.count).map((b) => b.index);
          if (mine.length > 0) patchDoc(doc, { selected: { page, index: mine[mine.length - 1] }, extra: mine.slice(0, -1), crop: null, notice: null });
          const te = useTextEdit.getState();
          if (te.docs[doc]?.selected) void te.select(doc, null);
          return true;
        } finally {
          applying.delete(doc);
        }
      }),

    transfer: (doc, toPage, at) =>
      queued(doc, async () => {
        const d = of(doc);
        const refs = picksOf(d);
        const from = refs.length > 0 ? refs[0].page : -1;
        const src = from >= 0 ? d.pages[from] : undefined;
        if (!src || from === toPage) return false;
        if (src.locked) {
          patchDoc(doc, { notice: src.locked });
          return false;
        }
        const moving = refs.flatMap((r) => (src.images[r.index]?.editable ? [r.index] : []));
        if (moving.length === 0) return false;
        applying.add(doc);
        try {
          const r = await moveImagesToPage(doc, src.rev, from, moving, toPage, at);
          if (r.kind === "error") {
            patchDoc(doc, { notice: `The pictures could not be moved: ${r.message}` });
            return false;
          }
          await fetchPage(doc, from);
          const fresh = await fetchPage(doc, toPage);
          if (r.kind === "stale") {
            patchDoc(doc, { selected: null, notice: STALE });
            return false;
          }
          const mine = fresh.images.slice(-moving.length).map((b) => b.index);
          if (mine.length > 0) patchDoc(doc, { selected: { page: toPage, index: mine[mine.length - 1] }, extra: mine.slice(0, -1), crop: null, notice: null });
          return true;
        } finally {
          applying.delete(doc);
        }
      }),

    replace: async (doc) => {
      if (!selectedImage(doc)) return false;
      const f = await pickImage();
      if (!f) return false;
      return get().apply(doc, { op: "replace", file: f.token });
    },

    startPlacing: async (doc) => {
      const f = await pickImage();
      if (!f) return;
      patchDoc(doc, { placing: { file: f.token, name: f.name }, selected: null, crop: null, notice: null });
      // Placing and typing a new text box are two ways of clicking the page.
      useTextEdit.getState().setTool("select");
    },

    cancelPlacing: (doc) => patchDoc(doc, { placing: null }),

    place: (doc, page, rect) =>
      queued(doc, async () => {
        const placing = of(doc).placing;
        if (!placing) return false;
        const change: ImageChange = { op: "add", file: placing.file, rect, keepAspect: true };
        // One retry when the document moved on since the page was listed.
        let r = await editImage(doc, docRev(doc), page, null, change);
        if (r.kind === "stale") r = await editImage(doc, docRev(doc), page, null, change);
        if (r.kind !== "ok") {
          patchDoc(doc, {
            notice: r.kind === "error" ? `This image could not be added: ${r.message}` : "The page changed; try placing the image again.",
          });
          return false;
        }
        patchDoc(doc, { placing: null, notice: null });
        const fresh = await fetchPage(doc, page);
        const mine = closestImage(rect, fresh.images);
        if (mine) patchDoc(doc, { selected: { page, index: mine.index } });
        return true;
      }),

    startCrop: (doc) => {
      const b = selectedImage(doc);
      if (b && b.editable) patchDoc(doc, { crop: { rect: [...b.rect] as Rect4, pixels: false } });
    },
    setCrop: (doc, rect) => {
      const c = of(doc).crop;
      if (c) patchDoc(doc, { crop: { ...c, rect } });
    },
    setCropPixels: (doc, pixels) => {
      const c = of(doc).crop;
      if (c) patchDoc(doc, { crop: { ...c, pixels } });
    },
    applyCrop: async (doc) => {
      const c = of(doc).crop;
      const b = selectedImage(doc);
      if (!c || !b) return false;
      // Nothing cut off: nothing to do.
      const same = c.rect.every((v, i) => Math.abs(v - b.rect[i]) < 0.5);
      if (same) {
        patchDoc(doc, { crop: null });
        return true;
      }
      return get().apply(doc, { op: "crop", rect: clampRect(c.rect, b.fullRect), pixels: c.pixels });
    },
    cancelCrop: (doc) => patchDoc(doc, { crop: null }),
  };
});

// A text block taken up, or the text tool, ends what the image editor was doing on that document.
useTextEdit.subscribe((s, prev) => {
  const ie = useImageEdit.getState();
  for (const k of Object.keys(s.docs)) {
    const id = Number(k);
    const sel = s.docs[id]?.selected;
    if (sel && !prev.docs[id]?.selected && ie.docs[id]?.selected) ie.select(id, null);
  }
  if (s.tool === "text" && prev.tool !== "text") {
    for (const k of Object.keys(ie.docs)) if (ie.docs[Number(k)]?.placing) ie.cancelPlacing(Number(k));
  }
  // Leaving Edit, or its Text section (Links and Bookmarks have their own keys on the page), drops the
  // selection, the crop and a file waiting for its place.
  if ((prev.open && !s.open) || (prev.section === "text" && s.section !== "text")) {
    useImageEdit.setState((t) => {
      const docs: Record<number, DocImageEdit> = {};
      for (const k of Object.keys(t.docs)) {
        choices.set(Number(k), (choices.get(Number(k)) ?? 0) + 1);
        docs[Number(k)] = { ...t.docs[Number(k)], selected: null, crop: null, placing: null, notice: null };
      }
      return { docs };
    });
  }
});

useStore.subscribe((s, prev) => {
  // A closed tab's image lists are not kept.
  const ie = useImageEdit.getState();
  const gone = Object.keys(ie.docs).filter((k) => !s.docs[Number(k)] && prev.docs[Number(k)]);
  if (gone.length > 0) {
    useImageEdit.setState((t) => {
      const docs = { ...t.docs };
      for (const k of gone) delete docs[Number(k)];
      return { docs };
    });
  }
});
