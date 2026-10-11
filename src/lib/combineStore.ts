import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { CombineOptions, CombinePlan, FileRef } from "./api";
import { togglePage } from "./combineLogic";
import { previewScaleKey, type Size } from "./layout";

/**
 * The Combine files sheet. Each row is a PDF the user chose (an open token, never a path); the
 * engine's `combine_inspect` says what the result would be, the same code `combine_run` writes it
 * with. The row's range text is the source of truth: the page thumbnails only rewrite it.
 */

/** Spare tile stream of the engine: a thumbnail never delays the page view. */
const THUMB_STREAM = 2;

export interface CombineEntry {
  key: number;
  /** Open token. */
  token: string;
  path: string;
  name: string;
  /** Bookmark title as typed; "" takes the file's name. */
  title: string;
  /** "1-3, 6, 9-7"; "" is every page. */
  range: string;
  /** Engine handle that draws the thumbnails; null while opening, or for a file that cannot be drawn. */
  doc: number | null;
  pages: number | null;
  /** Page sizes as they stream in. */
  sizes: (Size | null)[];
  /** First page, as a data URL. */
  thumb: string | null;
  /** The page thumbnails are showing. */
  open: boolean;
}

export type CombinePhase = "setup" | "running";

export interface CombineSheet {
  entries: CombineEntry[];
  options: CombineOptions;
  plan: CombinePlan | null;
  planning: boolean;
  /** The engine could not plan (nothing usable in the list, a file named twice ...). */
  planError: string | null;
  phase: CombinePhase;
  /** A failed run: shown at the foot of the sheet. */
  error: string | null;
}

interface CombineStore {
  sheet: CombineSheet | null;
  open: (files?: FileRef[]) => void;
  close: () => void;
  /** Adds files at the end (picked, or dropped from Explorer). */
  add: (files: FileRef[]) => void;
  pick: () => Promise<void>;
  remove: (key: number) => void;
  /** Moves the row `from` to index `to`. */
  move: (from: number, to: number) => void;
  setRange: (key: number, range: string) => void;
  setTitle: (key: number, title: string) => void;
  toggleOpen: (key: number) => void;
  /** A click on page thumbnail `page` (0-based) of row `key`. */
  togglePage: (key: number, page: number) => void;
  setOptions: (p: Partial<CombineOptions>) => void;
  run: () => Promise<void>;
}

export const defaultCombineOptions = (): CombineOptions => ({ bookmarks: true, keepOutlines: true, contentsPage: false });

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const stem = (p: string) => baseName(p).replace(/\.pdf$/i, "");

let keys = 0;
let planSeq = 0;
let planTimer: ReturnType<typeof setTimeout> | undefined;
/** Tokens of the pages a drawn thumbnail was made for (doc:page): a thumbnail is drawn once. */
const drawn = new Map<string, string>();

const request = (e: CombineEntry): api.CombineFile => ({ file: e.token, range: e.range, title: e.title.trim() === "" ? null : e.title });

export const useCombine = create<CombineStore>((set, get) => {
  const patchSheet = (p: Partial<CombineSheet>) => set((s) => (s.sheet ? { sheet: { ...s.sheet, ...p } } : s));
  const patchEntry = (key: number, p: Partial<CombineEntry>) =>
    set((s) =>
      s.sheet ? { sheet: { ...s.sheet, entries: s.sheet.entries.map((e) => (e.key === key ? { ...e, ...p } : e)) } } : s,
    );

  /** Asks the engine what the list would give, a moment after the last change. */
  const schedulePlan = () => {
    clearTimeout(planTimer);
    const sheet = get().sheet;
    if (!sheet) return;
    if (sheet.entries.length === 0) {
      planSeq++;
      patchSheet({ plan: null, planning: false, planError: null });
      return;
    }
    patchSheet({ planning: true });
    planTimer = setTimeout(() => {
      const cur = get().sheet;
      if (!cur) return;
      const seq = ++planSeq;
      api
        .combineInspect(cur.entries.map(request), cur.options)
        .then((plan) => seq === planSeq && patchSheet({ plan, planning: false, planError: null }))
        .catch((e) => seq === planSeq && patchSheet({ plan: null, planning: false, planError: String(e) }));
    }, 220);
  };

  /** Opens the file for drawing: first page now, page sizes as they stream in. */
  const load = async (key: number, token: string) => {
    let doc: number | null = null;
    try {
      const o = await api.openDocument(token);
      doc = o.doc;
      if (o.status !== "opened") {
        // Locked files are planned (and left out) by the engine; there is nothing to draw.
        void api.closeDocument(o.doc);
        return;
      }
      if (!get().sheet?.entries.some((e) => e.key === key)) {
        void api.closeDocument(o.doc);
        return;
      }
      patchEntry(key, { doc: o.doc, pages: o.pageCount, sizes: Array.from({ length: o.pageCount }, () => null) });
      void api.pageSizes(o.doc, (start, sizes) =>
        set((s) => {
          const e = s.sheet?.entries.find((x) => x.key === key);
          if (!s.sheet || !e) return s;
          const next = e.sizes.slice();
          sizes.forEach((v, i) => {
            if (start + i < next.length) next[start + i] = v;
          });
          return { sheet: { ...s.sheet, entries: s.sheet.entries.map((x) => (x.key === key ? { ...x, sizes: next } : x)) } };
        }),
      );
      const url = await pageUrl(o.doc, 0, o.firstPage);
      patchEntry(key, { thumb: url });
    } catch {
      if (doc != null) void api.closeDocument(doc);
      patchEntry(key, { doc: null });
    }
  };

  const entryOf = (f: FileRef): CombineEntry => ({
    key: ++keys,
    token: f.token,
    path: f.path,
    name: baseName(f.path),
    title: "",
    range: "",
    doc: null,
    pages: null,
    sizes: [],
    thumb: null,
    open: false,
  });

  const release = (entries: CombineEntry[]) => {
    for (const e of entries) {
      if (e.doc != null) {
        void api.closeDocument(e.doc);
        for (const k of [...drawn.keys()]) if (k.startsWith(`${e.doc}:`)) drawn.delete(k);
      }
    }
  };

  return {
    sheet: null,

    open: (files) => {
      get().close();
      set({ sheet: { entries: [], options: defaultCombineOptions(), plan: null, planning: false, planError: null, phase: "setup", error: null } });
      if (files?.length) get().add(files);
    },

    close: () => {
      const s = get().sheet;
      if (!s || s.phase === "running") return;
      clearTimeout(planTimer);
      planSeq++;
      release(s.entries);
      set({ sheet: null });
    },

    add: (files) => {
      const s = get().sheet;
      if (!s || s.phase === "running" || files.length === 0) return;
      const fresh = files.map(entryOf);
      patchSheet({ entries: [...s.entries, ...fresh], error: null });
      fresh.forEach((e) => void load(e.key, e.token));
      schedulePlan();
    },

    pick: async () => {
      const files = await api.pickOpen(true);
      get().add(files);
    },

    remove: (key) => {
      const s = get().sheet;
      if (!s || s.phase === "running") return;
      release(s.entries.filter((e) => e.key === key));
      patchSheet({ entries: s.entries.filter((e) => e.key !== key) });
      schedulePlan();
    },

    move: (from, to) => {
      const s = get().sheet;
      if (!s || s.phase === "running" || from === to || from < 0 || to < 0 || from >= s.entries.length || to >= s.entries.length) return;
      const next = s.entries.slice();
      const [e] = next.splice(from, 1);
      next.splice(to, 0, e);
      patchSheet({ entries: next });
      schedulePlan();
    },

    setRange: (key, range) => {
      patchEntry(key, { range });
      schedulePlan();
    },

    setTitle: (key, title) => {
      patchEntry(key, { title });
      schedulePlan();
    },

    toggleOpen: (key) => {
      const e = get().sheet?.entries.find((x) => x.key === key);
      if (e) patchEntry(key, { open: !e.open });
    },

    togglePage: (key, page) => {
      const e = get().sheet?.entries.find((x) => x.key === key);
      if (!e || e.pages == null) return;
      const next = togglePage(e.range, e.pages, page);
      if (next != null) get().setRange(key, next);
    },

    setOptions: (p) => {
      const s = get().sheet;
      if (!s || s.phase === "running") return;
      patchSheet({ options: { ...s.options, ...p } });
      schedulePlan();
    },

    run: async () => {
      const s = get().sheet;
      if (!s || s.phase === "running" || !s.plan?.canRun || s.planning) return;
      const first = s.entries[0];
      try {
        const out = await api.pickSave(`${stem(first.path)} (combined).pdf`, { near: first.token });
        if (!out) return;
        patchSheet({ phase: "running", error: null });
        const done = await api.combineRun(s.entries.map(request), s.options, out.token);
        // The result opens in its own tab; the sheet's job is done.
        patchSheet({ phase: "setup" });
        get().close();
        await useStore.getState().openPath(done.file.path, done.file.token);
        useStore.getState().setUi({ error: `${done.plan.summary}. Saved as ${baseName(done.file.path)}` });
      } catch (e) {
        patchSheet({ phase: "setup", error: String(e) });
      }
    },
  };
});

/** Page `page` of an open document as a data URL (JPEG, preview scale), drawn once. */
export async function pageUrl(doc: number, page: number, size: Size): Promise<string | null> {
  const k = `${doc}:${page}`;
  const hit = drawn.get(k);
  if (hit) return hit;
  const t = await api.fetchTile(doc, page, 0, previewScaleKey(size), 0, 0, 1, 0, THUMB_STREAM, 0);
  if (!t) return null;
  const c = document.createElement("canvas");
  c.width = t.w;
  c.height = t.h;
  c.getContext("2d")!.putImageData(new ImageData(t.rgba as Uint8ClampedArray<ArrayBuffer>, t.w, t.h), 0, 0);
  const url = c.toDataURL("image/jpeg", 0.8);
  drawn.set(k, url);
  return url;
}
