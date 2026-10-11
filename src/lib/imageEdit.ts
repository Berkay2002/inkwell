import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { ImageBlock, ImageChange } from "./api";
import { editImage } from "./edit";
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
}

export interface Crop {
  /** The part to keep, in page space. */
  rect: Rect4;
  /** Drop the pixels outside (a smaller file), instead of hiding them behind a clip. */
  pixels: boolean;
}

export interface DocImageEdit {
  pages: Record<number, PageImages>;
  selected: ImageRef | null;
  /** A quiet sentence in the rail: why an image is locked, or that the page changed under a change. */
  notice: string | null;
  crop: Crop | null;
  /** A file chosen with "Image", waiting for its place on a page. */
  placing: { file: string; name: string } | null;
}

const empty = (): DocImageEdit => ({ pages: {}, selected: null, notice: null, crop: null, placing: null });

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
  setNotice: (doc: number, notice: string | null) => void;

  /** Sends `change` for the selected image as one undo step. True when it was applied. */
  apply: (doc: number, change: ImageChange) => Promise<boolean>;
  move: (doc: number, d: [number, number]) => Promise<boolean>;
  resizeTo: (doc: number, rect: Rect4, keepAspect: boolean) => Promise<boolean>;
  rotate: (doc: number, quarterTurns: number) => Promise<boolean>;
  flip: (doc: number, horizontal: boolean) => Promise<boolean>;
  remove: (doc: number) => Promise<boolean>;
  /** Asks for an image file and draws it instead of the selected image. */
  replace: (doc: number) => Promise<boolean>;

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

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/**
 * Per document: counts every decision about which image is selected (a click, a settled change). A listing
 * that was started before the latest decision must not remap the selection from the box it was started at.
 */
const choices = new Map<number, number>();

export const useImageEdit = create<Store>((set, get) => {
  const patchDoc = (doc: number, p: Partial<DocImageEdit>) => {
    if ("selected" in p) choices.set(doc, (choices.get(doc) ?? 0) + 1);
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? empty()), ...p } } }));
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
    patchDoc(doc, { pages: { ...of(doc).pages, [page]: { rev: r.rev, images: r.images } } });
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
    const choice = choices.get(doc) ?? 0;
    const r = await fetchPage(doc, page);
    // Only a selection nobody touched while this listing was on its way is remapped: a change of ours that
    // settled meanwhile (or a click) already chose the image, and `was` is the box from before it.
    if (sel && was && !applying.has(doc) && (choices.get(doc) ?? 0) === choice) {
      const now = of(doc);
      if (now.selected?.page === page && now.selected.index === sel.index) {
        const same = closestImage(was.fullRect, r.images);
        if (!same) patchDoc(doc, { selected: null, crop: null });
        else if (same.index !== sel.index) patchDoc(doc, { selected: { page, index: same.index } });
      }
    }
    return r;
  };

  const applyNow = async (doc: number, change: ImageChange): Promise<boolean> => {
    const d = of(doc);
    const ref = d.selected;
    const image = selectedImage(doc);
    const page = ref ? d.pages[ref.page] : undefined;
    if (!ref || !image || !page) return false;
    applying.add(doc);
    try {
      const r = await editImage(doc, page.rev, ref.page, ref.index, change);
      if (r.kind === "error") {
        patchDoc(doc, { notice: `This change could not be applied: ${r.message}` });
        return false;
      }
      const fresh = await fetchPage(doc, ref.page);
      if (r.kind === "stale") {
        const same = closestImage(image.fullRect, fresh.images);
        patchDoc(doc, {
          selected: same ? { page: ref.page, index: same.index } : null,
          crop: null,
          notice: "The page changed under that change, so it was not applied. Try it again.",
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

    setNotice: (doc, notice) => patchDoc(doc, { notice }),

    apply: (doc, change) => queued(doc, () => applyNow(doc, change)),
    move: (doc, d) => get().apply(doc, { op: "move", dx: d[0], dy: d[1] }),
    resizeTo: (doc, rect, keepAspect) => get().apply(doc, { op: "resize", rect, keepAspect }),
    rotate: (doc, quarterTurns) => get().apply(doc, { op: "rotate", quarterTurns }),
    flip: (doc, horizontal) => get().apply(doc, { op: "flip", horizontal }),
    remove: (doc) => get().apply(doc, { op: "delete" }),

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
