import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useStore } from "../store";
import { annotApi, useAnnot } from "./annot";
import { type EditState, pickSave } from "./api";
import { applyState } from "./edit";
import {
  type Measure, type MeasureKind, type PageScale, type Pt, type Quantity, type Scale, type SnapHit, type Snaps, UNSCALED,
  calibrate, type Unit, validScale,
} from "./measureLogic";

/**
 * Measuring for the UI (src-tauri/src/measure). Every change is one engine edit (one undo step) that Save keeps;
 * the list is re-read after each one (and after undo, redo and a restored version: edit.ts `applyState`).
 */

export const measureApi = {
  list: (doc: number) => invoke<Measure[]>("measure_list", { doc }),
  /** Why measuring is off for the document (a certification that forbids annotations); null when it is allowed. */
  lock: (doc: number) => invoke<string | null>("measure_lock", { doc }),
  /** Whether the document is certified at any `/P`: a page scale is a page-dictionary entry, which none allows. */
  protection: (doc: number) => invoke<{ certified: boolean; signed: boolean } | null>("a11y_protection", { doc }),
  scaleGet: (doc: number, page: number) => invoke<PageScale>("measure_scale_get", { doc, page }),
  scaleSet: (doc: number, page: number, scale: Scale) => invoke<EditState>("measure_scale_set", { doc, page, scale }),
  add: (doc: number, page: number, kind: MeasureKind, points: Pt[], scale: Scale | null, author: string) =>
    invoke<{ state: EditState; xref: number; quantity: Quantity }>("measure_add", { doc, page, kind, points, scale, author }),
  /** The value `points` would measure: under measurement `xref`'s own /Measure, else the page's at the first point. */
  value: (doc: number, page: number, kind: MeasureKind, points: Pt[], xref: number | null) =>
    invoke<Quantity>("measure_value", { doc, page, kind, points, scale: null, xref }),
  move: (doc: number, page: number, xref: number, points: Pt[]) =>
    invoke<{ state: EditState; xref: number; quantity: Quantity }>("measure_move", { doc, page, xref, points }),
  rescale: (doc: number, page: number, xref: number, scale: Scale) =>
    invoke<{ state: EditState; xref: number; quantity: Quantity }>("measure_rescale", { doc, page, xref, scale }),
  snap: (doc: number, page: number, x: number, y: number, radius: number, kinds: Snaps) =>
    invoke<SnapHit | null>("measure_snap", { doc, page, x, y, radius, kinds }),
  exportCsv: (doc: number, to: string) => invoke<string>("measure_export_csv", { doc, to }),
};

/** The bar's tools: select what is measured, the three measurements, and calibrating from a printed dimension. */
export type MeasureTool = "select" | MeasureKind | "calibrate";

export interface Ref {
  page: number;
  xref: number;
}

/** A calibration line that has been drawn and waits for its real length. */
export interface Calib {
  page: number;
  a: Pt;
  b: Pt;
}

export interface LastMeasure {
  kind: MeasureKind;
  page: number;
  quantity: Quantity;
}

export interface DocMeasure {
  items: Measure[];
  selected: Ref | null;
  /** By 0-based page, read when a page is first measured on. */
  scales: Record<number, PageScale>;
  /** The measurement made last in this session of the tool. */
  last: LastMeasure | null;
  calib: Calib | null;
  /** Where the last CSV export went. */
  exported: string | null;
  error: string | null;
}

const fresh = (): DocMeasure => ({ items: [], selected: null, scales: {}, last: null, calib: null, exported: null, error: null });

interface MeasureStore {
  /** The Measure bar and rail are showing. */
  open: boolean;
  setOpen: (open: boolean) => void;
  tool: MeasureTool;
  setTool: (t: MeasureTool) => void;
  snaps: Snaps;
  toggleSnap: (k: keyof Snaps) => void;

  /** By document, read once: the reason nothing may be measured on it, or null. Absent until read. */
  locks: Record<number, string | null>;
  /** The same for the page scale (the bar's scale chip, units and Calibrate): also off on a certified file at /P 3. */
  scaleLocks: Record<number, string | null>;
  /** By document: it is signed, so a change is saved as an update after the signature (read with the lock). */
  signed: Record<number, boolean>;
  ensureLock: (doc: number) => Promise<void>;

  docs: Record<number, DocMeasure>;
  get: (doc: number) => DocMeasure;
  refresh: (doc: number) => Promise<void>;
  /** The file changed under us (undo, redo, a restored version): scales and list are read again. */
  onRev: (doc: number) => void;
  /** Reads the page's scale once (a page without one measures in 1 in = 1 in). */
  ensureScale: (doc: number, page: number) => Promise<void>;
  select: (doc: number, ref: Ref | null) => void;
  add: (doc: number, page: number, kind: MeasureKind, points: Pt[]) => Promise<number | null>;
  remove: (doc: number, ref: Ref) => Promise<void>;
  setPageScale: (doc: number, page: number, scale: Scale) => Promise<boolean>;
  /** Moves the points of one measurement (one undo step, the same annotation); resolves to whether it was written. */
  move: (doc: number, ref: Ref, points: Pt[]) => Promise<boolean>;
  /** Gives one measurement another scale: it is made again with the same points (one undo step). */
  rescale: (doc: number, ref: Ref, scale: Scale) => Promise<void>;
  /** Draws the calibration line in, then `finishCalib` sets the page scale from its real length. */
  startCalib: (doc: number, c: Calib) => void;
  cancelCalib: (doc: number) => void;
  finishCalib: (doc: number, real: number, unit: Unit) => Promise<boolean>;
  exportCsv: (doc: number) => Promise<void>;
  /** The engine's snap for a point among the switched-on kinds; null when none is on. */
  snap: (doc: number, page: number, p: Pt, radius: number) => Promise<SnapHit | null>;
}

export const SIGNED_MEASURE =
  "This document is signed. A change is saved as an update after the signature, which will show that the file was changed.";
const SCALE_CERTIFIED = "This document is certified; a certification doesn't allow a page scale to be set.";
const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const stem = (p: string) => baseName(p).replace(/\.pdf$/i, "");

export const useMeasure = create<MeasureStore>((set, get) => {
  const who = () => useAnnot.getState().author.trim() || useAnnot.getState().defaultAuthor;
  const patch = (doc: number, p: Partial<DocMeasure>) =>
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? fresh()), ...p } } }));
  /** Runs one engine call, shows its new state, and re-reads the list. */
  const run = async <T>(doc: number, f: () => Promise<T>, state: (r: T) => EditState): Promise<T | null> => {
    const why = get().locks[doc];
    if (why) {
      patch(doc, { error: why });
      return null;
    }
    try {
      const r = await f();
      applyState(doc, state(r));
      await get().refresh(doc);
      patch(doc, { error: null });
      return r;
    } catch (e) {
      patch(doc, { error: msg(e) });
      return null;
    }
  };
  /** The scales of a document are read again after anything that may have changed them. */
  const dropScales = (doc: number) => patch(doc, { scales: {} });

  return {
    open: false,
    setOpen: (open) => set(open ? { open, tool: "distance" } : { open, tool: "select" }),
    tool: "distance",
    setTool: (tool) => set({ tool }),
    snaps: { endpoint: true, midpoint: true, intersection: true, line: true },
    toggleSnap: (k) => set((s) => ({ snaps: { ...s.snaps, [k]: !s.snaps[k] } })),

    locks: {},
    scaleLocks: {},
    signed: {},
    ensureLock: async (doc) => {
      if (get().locks[doc] !== undefined) return;
      try {
        const why = await measureApi.lock(doc);
        // Measurements are annotations (a /P 3 certification allows them), a page scale is not: no /P allows it.
        let signed = false;
        const scaleWhy =
          why ??
          (await measureApi.protection(doc).then(
            (p) => {
              signed = !!p?.signed;
              return p?.certified ? SCALE_CERTIFIED : null;
            },
            (e) => msg(e),
          ));
        set((s) => ({
          locks: { ...s.locks, [doc]: why },
          scaleLocks: { ...s.scaleLocks, [doc]: scaleWhy },
          signed: { ...s.signed, [doc]: signed },
        }));
      } catch (e) {
        // an unreadable answer is a refusal: nothing is written until the check has said it is fine
        set((s) => ({ locks: { ...s.locks, [doc]: msg(e) }, scaleLocks: { ...s.scaleLocks, [doc]: msg(e) } }));
      }
    },

    docs: {},
    get: (doc) => get().docs[doc] ?? fresh(),
    refresh: async (doc) => {
      try {
        const items = await measureApi.list(doc);
        const d = get().get(doc);
        const sel = d.selected;
        patch(doc, { items, selected: sel && items.some((m) => m.page === sel.page && m.xref === sel.xref) ? sel : null });
      } catch (e) {
        patch(doc, { error: msg(e) });
      }
    },
    onRev: (doc) => {
      set((s) => {
        const { [doc]: _, ...locks } = s.locks;
        const { [doc]: __, ...scaleLocks } = s.scaleLocks;
        return { locks, scaleLocks };
      });
      void get().ensureLock(doc);
      dropScales(doc);
      void get().refresh(doc);
    },
    ensureScale: async (doc, page) => {
      if (get().get(doc).scales[page]) return;
      try {
        const s = await measureApi.scaleGet(doc, page);
        patch(doc, { scales: { ...get().get(doc).scales, [page]: s } });
      } catch (e) {
        patch(doc, { error: msg(e) });
      }
    },
    select: (doc, selected) => patch(doc, { selected }),

    add: async (doc, page, kind, points) => {
      const r = await run(doc, () => measureApi.add(doc, page, kind, points, null, who()), (x) => x.state);
      if (!r) return null;
      patch(doc, { selected: { page, xref: r.xref }, last: { kind, page, quantity: r.quantity } });
      return r.xref;
    },
    remove: async (doc, ref) => {
      await run(doc, () => annotApi.remove(doc, [[ref.page, ref.xref]]), (s) => s);
      patch(doc, { selected: null });
    },
    setPageScale: async (doc, page, scale) => {
      if (!validScale(scale)) {
        patch(doc, { error: "A scale needs numbers above zero on both sides." });
        return false;
      }
      const why = get().scaleLocks[doc];
      if (why) {
        patch(doc, { error: why });
        return false;
      }
      const ok = await run(doc, () => measureApi.scaleSet(doc, page, scale), (s) => s);
      dropScales(doc);
      if (ok) await get().ensureScale(doc, page);
      return ok !== null;
    },
    move: async (doc, ref, points) => {
      const r = await run(doc, () => measureApi.move(doc, ref.page, ref.xref, points), (x) => x.state);
      return r !== null;
    },
    rescale: async (doc, ref, scale) => {
      if (!validScale(scale)) return patch(doc, { error: "A scale needs numbers above zero on both sides." });
      const r = await run(doc, () => measureApi.rescale(doc, ref.page, ref.xref, scale), (x) => x.state);
      if (r) patch(doc, { selected: { page: ref.page, xref: r.xref } });
    },

    startCalib: (doc, calib) => patch(doc, { calib }),
    cancelCalib: (doc) => patch(doc, { calib: null }),
    finishCalib: async (doc, real, unit) => {
      const c = get().get(doc).calib;
      if (!c) return false;
      const scale = calibrate(c.a, c.b, real, unit);
      if (!scale) {
        patch(doc, { error: "Type the real length as a number above zero." });
        return false;
      }
      const ok = await get().setPageScale(doc, c.page, scale);
      if (ok) patch(doc, { calib: null });
      return ok;
    },

    exportCsv: async (doc) => {
      const d = useStore.getState().docs[doc];
      if (!d) return;
      try {
        const to = await pickSave(`${stem(d.name)} measurements.csv`, { doc }, "csv");
        if (!to) return;
        const path = await measureApi.exportCsv(doc, to.token);
        patch(doc, { exported: path, error: null });
      } catch (e) {
        patch(doc, { error: msg(e) });
      }
    },

    snap: async (doc, page, p, radius) => {
      const on = get().snaps;
      if (!Object.values(on).some(Boolean)) return null;
      try {
        return await measureApi.snap(doc, page, p[0], p[1], radius, on);
      } catch {
        return null; // a page that cannot be read just has nothing to snap to
      }
    },
  };
});

/** A page's scale for the live label; 1 in = 1 in until it has been read. */
export const scaleOf = (d: DocMeasure | undefined, page: number): Scale => d?.scales[page]?.scale ?? UNSCALED;
