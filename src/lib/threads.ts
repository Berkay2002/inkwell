import { create } from "zustand";
import * as api from "./api";
import type { ArticleThread, ThreadBead, ThreadInfo } from "./api";
import * as edits from "./edit";
import { ZOOM_MAX, ZOOM_MIN, clamp } from "./layout";
import { useStore } from "../store";

/**
 * Article threads for the UI (src-tauri/src/organize/threads.rs): the list per document, "Show beads", the Article
 * tool, and follow mode (reading an article bead by bead). The zoom and scroll of following live in
 * components/Threads.tsx; everything that decides *where to go next* is here and has no DOM.
 */

export type Rect = ThreadBead["rect"];
/** A bead by its thread and its own id (object numbers; see docs/threads.md). */
export interface BeadRef {
  doc: number;
  thread: number;
  bead: number;
}

// ---- pure helpers --------------------------------------------------------------------------

const EMPTY: ArticleThread[] = [];

export const threadOf = (list: ArticleThread[], id: number) => list.find((t) => t.id === id);

/** The bead after (`1`) or before (`-1`) `bead` in its thread's reading order; null at either end or when it is not there. */
export function stepBead(list: ArticleThread[], thread: number, bead: number, dir: 1 | -1): ThreadBead | null {
  const t = threadOf(list, thread);
  const i = t ? t.beads.findIndex((b) => b.id === bead) : -1;
  return i < 0 ? null : (t!.beads[i + dir] ?? null);
}

/** "1-3": the third bead of the first article, as drawn on the page. */
export function beadNumber(list: ArticleThread[], thread: number, bead: number): string {
  const n = list.findIndex((t) => t.id === thread);
  const b = n < 0 ? -1 : list[n].beads.findIndex((x) => x.id === bead);
  return b < 0 ? "" : `${n + 1}-${b + 1}`;
}

/** Where a bead went after the list was read again: itself, else the bead now at its place in the thread, else nothing. */
export function reconcile(list: ArticleThread[], thread: number, bead: number, hint: number): number | null {
  const t = threadOf(list, thread);
  if (!t || t.beads.length === 0) return null;
  if (t.beads.some((b) => b.id === bead)) return bead;
  return t.beads[clamp(hint, 0, t.beads.length - 1)].id;
}

/** The zoom that makes a bead's width fill the view (with `pad` px at each side); an upright view is `rot` 0 or 2. */
export function beadZoom(rect: Rect, rot: number, viewW: number, pad = 24): number {
  const w = Math.max(1, rot % 2 === 0 ? rect[2] - rect[0] : rect[3] - rect[1]);
  return clamp((viewW - 2 * pad) / w, ZOOM_MIN, ZOOM_MAX);
}

/** "Article 3": the first title not taken, so a deleted article's name is not reused while another has it. */
export function nextTitle(list: ArticleThread[]): string {
  const have = new Set(list.map((t) => t.title));
  let n = list.length + 1;
  while (have.has(`Article ${n}`)) n++;
  return `Article ${n}`;
}

/** The page tools that can be in the hand besides the Article tool (each is "select" or off when it is not). */
export interface ToolsHeld {
  redact: boolean;
  annot: string;
  forms: string;
  /** Edit mode's text tool, and the link tool. */
  text: string;
  link: string;
}

export const otherToolHeld = (t: ToolsHeld): boolean =>
  t.redact || t.annot !== "select" || t.forms !== "select" || t.text !== "select" || t.link !== "select";

/**
 * What the beads on a page take of the pointer. Outlines alone never do ("Show beads" must not get between the
 * person and the comment, form, text or link tools underneath): with nothing else in the hand the number tag does
 * (a click reads from that bead), and while an article is being read the whole bead does. With the Article tool
 * the beads are what it edits.
 */
export function beadPointer(s: { tool: boolean; follow: boolean; show: boolean; other: boolean }): "edit" | "all" | "tag" | "none" {
  if (s.tool) return "edit";
  if (s.other) return "none";
  return s.follow ? "all" : s.show ? "tag" : "none";
}

/** The reading keys (Enter, arrows) belong to the page: not to a button or a list that has focus. */
export const readsKeys = (focus: { body: boolean; inViewer: boolean }): boolean => focus.body || focus.inViewer;

/** The Properties form as it opens: an article with no title shows the name it is listed under. */
export const propsDraft = (t: ArticleThread): ThreadInfo => ({ ...t.info, title: t.info.title || t.title });

export const sameInfo = (a: ThreadInfo, b: ThreadInfo): boolean =>
  a.title === b.title && a.subject === b.subject && a.author === b.author && a.keywords === b.keywords;

// ---- the store -----------------------------------------------------------------------------

interface Threads {
  docs: Record<number, ArticleThread[]>;
  /** "Show beads": outlines with numbers on every page. */
  show: boolean;
  /** The Article tool is armed: a drag on a page makes a bead. */
  tool: boolean;
  /** The bead selected for editing (with the tool armed). Its thread is the one a new bead joins. */
  selected: BeadRef | null;
  /** Reading an article: the bead in view. */
  follow: BeadRef | null;
  /** The Properties sheet. */
  props: { doc: number; thread: number } | null;

  get: (doc: number) => ArticleThread[];
  load: (doc: number) => Promise<void>;
  setShow: (on: boolean) => void;
  setTool: (on: boolean) => void;
  select: (ref: BeadRef | null) => void;
  /** Finishes the thread being drawn: nothing is selected and its Properties open. */
  finish: () => void;
  openProps: (doc: number, thread: number) => void;
  closeProps: () => void;

  startFollow: (doc: number, thread: number, bead?: number) => void;
  stopFollow: () => void;
  /** To the next or previous bead; at the last one reading ends ("end"), at the first it stays ("start"). */
  step: (dir: 1 | -1) => "moved" | "end" | "start" | "none";

  start: (doc: number, page: number, rect: Rect) => Promise<void>;
  addBead: (doc: number, thread: number, after: number | null, page: number, rect: Rect) => Promise<void>;
  moveBead: (doc: number, thread: number, bead: number, page: number, rect: Rect) => Promise<void>;
  removeBead: (doc: number, thread: number, bead: number) => Promise<void>;
  removeThread: (doc: number, thread: number) => Promise<void>;
  describe: (doc: number, thread: number, info: ThreadInfo) => Promise<void>;
  reorder: (doc: number, thread: number, to: number) => Promise<void>;
}

const fail = (e: unknown) => useStore.getState().setUi({ error: String(e) });
const seq = new Map<number, number>();

export const useThreads = create<Threads>((set, get) => {
  /** One engine call, then the list again (the generation changed); false when it failed (the error is shown). */
  const run = async (doc: number, f: () => Promise<api.EditState>): Promise<boolean> => {
    try {
      await edits.run(doc, f);
    } catch (e) {
      fail(e);
      return false;
    }
    await get().load(doc);
    return true;
  };

  /**
   * Keeps `selected` and `follow` pointing at something that still exists after the list was read again. A deleted
   * bead leaves the selection empty (so Delete does not eat the next bead); reading moves on to its neighbour.
   */
  const settle = (doc: number, hints: Map<string, number>) => {
    const list = get().get(doc);
    const fix = (r: BeadRef | null, keepPlace: boolean): BeadRef | null => {
      if (!r || r.doc !== doc) return r;
      const hint = hints.get(`${r.thread}:${r.bead}`) ?? 0;
      const b = reconcile(list, r.thread, r.bead, hint);
      return b == null || (!keepPlace && b !== r.bead) ? null : { ...r, bead: b };
    };
    const { selected, follow, props } = get();
    set({
      selected: fix(selected, false),
      follow: fix(follow, true),
      props: props && props.doc === doc && !threadOf(list, props.thread) ? null : props,
    });
  };

  return {
    docs: {},
    show: false,
    tool: false,
    selected: null,
    follow: null,
    props: null,

    get: (doc) => get().docs[doc] ?? EMPTY,

    load: async (doc) => {
      const n = (seq.get(doc) ?? 0) + 1;
      seq.set(doc, n);
      // where each bead of the old list stood, so a deleted bead's selection moves to its neighbour
      const hints = new Map<string, number>();
      for (const t of get().get(doc)) t.beads.forEach((b, i) => hints.set(`${t.id}:${b.id}`, i));
      try {
        const list = await api.threadsList(doc);
        if (seq.get(doc) !== n) return;
        set((s) => ({ docs: { ...s.docs, [doc]: list } }));
        settle(doc, hints);
      } catch (e) {
        if (seq.get(doc) === n) fail(e);
      }
    },

    setShow: (show) => set({ show }),
    setTool: (tool) => set(tool ? { tool, follow: null } : { tool, selected: null }),
    select: (selected) => set({ selected }),
    finish: () => {
      const s = get().selected;
      set({ selected: null, ...(s ? { props: { doc: s.doc, thread: s.thread } } : {}) });
    },
    openProps: (doc, thread) => set({ props: { doc, thread } }),
    closeProps: () => set({ props: null }),

    startFollow: (doc, thread, bead) => {
      const t = threadOf(get().get(doc), thread);
      const b = bead ?? t?.beads[0]?.id;
      if (!t || b == null) return;
      set({ follow: { doc, thread, bead: b }, tool: false, selected: null });
    },
    stopFollow: () => set({ follow: null }),
    step: (dir) => {
      const f = get().follow;
      if (!f) return "none";
      const to = stepBead(get().get(f.doc), f.thread, f.bead, dir);
      if (to) {
        set({ follow: { ...f, bead: to.id } });
        return "moved";
      }
      if (dir === 1) {
        set({ follow: null });
        return "end";
      }
      return "start";
    },

    start: async (doc, page, rect) => {
      const before = get().get(doc);
      const title = nextTitle(before);
      if (!(await run(doc, () => api.threadStart(doc, title, page, rect)))) return;
      const added = get().get(doc).find((t) => !before.some((o) => o.id === t.id));
      if (added?.beads[0]) set({ selected: { doc, thread: added.id, bead: added.beads[0].id } });
    },

    addBead: async (doc, thread, after, page, rect) => {
      const had = new Set(threadOf(get().get(doc), thread)?.beads.map((b) => b.id));
      if (!(await run(doc, () => api.threadBeadAdd(doc, thread, after, page, rect)))) return;
      const added = threadOf(get().get(doc), thread)?.beads.find((b) => !had.has(b.id));
      if (added) set({ selected: { doc, thread, bead: added.id } });
    },

    moveBead: async (doc, thread, bead, page, rect) => void (await run(doc, () => api.threadBeadMove(doc, thread, bead, page, rect))),
    removeBead: async (doc, thread, bead) => void (await run(doc, () => api.threadBeadDelete(doc, thread, bead))),
    removeThread: async (doc, thread) => void (await run(doc, () => api.threadRemove(doc, thread))),
    describe: async (doc, thread, info) => void (await run(doc, () => api.threadDescribe(doc, thread, info))),
    reorder: async (doc, thread, to) => void (await run(doc, () => api.threadMove(doc, thread, to))),
  };
});
