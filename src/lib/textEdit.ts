import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { BlockChange, TextArea, TextBlock, TextFallback } from "./api";
import { addText, editText } from "./edit";

/**
 * Editing text in place: the viewer's Edit mode. A page's blocks come from one `text_blocks` call
 * per page and document generation (cached here); every change is one `text_edit` and so one
 * undo step. See docs/text-edit.md for what the engine can and cannot do.
 */

export type Rect4 = [number, number, number, number];
export type Rgb = [number, number, number];
export type EditTool = "select" | "text";

export interface BlockRef {
  page: number;
  index: number;
}

/** Text typed into the selected block and not yet sent. */
export interface Draft extends BlockRef {
  text: string;
}

/** A new text box being typed. `at` is its top left in page space. */
export interface NewBox {
  page: number;
  at: [number, number];
  text: string;
}

interface PageBlocks {
  /** The document generation the blocks belong to. */
  rev: number;
  blocks: TextBlock[];
  areas: TextArea[];
}

export interface DocTextEdit {
  pages: Record<number, PageBlocks>;
  selected: BlockRef | null;
  draft: Draft | null;
  adding: NewBox | null;
  /** A quiet sentence under the format controls: a font that was replaced, or why a block is locked. */
  notice: string | null;
}

/** What "Add text" starts with (Acrobat's default is about 12 pt). */
export const NEW_TEXT = { size: 12, color: [0, 0, 0] as Rgb };

const empty = (): DocTextEdit => ({ pages: {}, selected: null, draft: null, adding: null, notice: null });

// ---- pure helpers (tested in textEdit.test.ts) ----

/** Hidden-layer text is not offered at all; everything else is shown, locked or not. */
export const isOffered = (b: TextBlock) => b.issue !== "hidden";

/** A CSS generic family for a PDF base font name (the engine's own rule for picking a standard face). */
export function cssFamily(font: string): string {
  const n = font.toLowerCase();
  if (["courier", "mono", "consolas", "menlo", "monaco", "lucida console", "cmtt"].some((s) => n.includes(s))) return '"Courier New", Courier, monospace';
  if (!n.includes("sans") && ["times", "serif", "roman", "mincho", "cambria", "georgia", "palatino", "garamond", "cmr", "minion", "book"].some((s) => n.includes(s)))
    return '"Times New Roman", Times, serif';
  return 'Arial, "Segoe UI", sans-serif';
}

/** `ABCDEF+Arial-BoldMT` as a person would say it: `Arial`. */
export function displayFont(base: string): string {
  const name = base.replace(/^[A-Z]{6}\+/, "");
  const stripped = name.replace(/[-,](Bold|Italic|Oblique|Regular|BoldItalic|BoldOblique|Roman|Medium|Light|Semibold)+(MT|PS)?$/i, "").replace(/(MT|PS)$/, "");
  return (stripped || name).replace(/([a-z])([A-Z])/g, "$1 $2");
}

export const cssColor = (c: Rgb) => `rgb(${c.map((v) => Math.round(Math.max(0, Math.min(1, v)) * 255)).join(" ")})`;

export const hexOf = (c: Rgb) =>
  "#" +
  c
    .map((v) =>
      Math.round(Math.max(0, Math.min(1, v)) * 255)
        .toString(16)
        .padStart(2, "0"),
    )
    .join("");

export function rgbOf(hex: string): Rgb | null {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex.trim());
  return m ? [parseInt(m[1], 16) / 255, parseInt(m[2], 16) / 255, parseInt(m[3], 16) / 255] : null;
}

/** The block in `next` that `prev` became: nearest top left after moving by `d`, same editable state. */
export function closestBlock(prev: TextBlock, next: TextBlock[], d: [number, number] = [0, 0]): TextBlock | null {
  let best: TextBlock | null = null;
  let bestDist = Infinity;
  for (const b of next) {
    if (!b.editable) continue;
    const dist = Math.hypot(b.rect[0] - (prev.rect[0] + d[0]), b.rect[1] - (prev.rect[1] + d[1]));
    if (dist < bestDist) {
      best = b;
      bestDist = dist;
    }
  }
  // A block that is far from where it was is another block.
  return best && bestDist <= Math.max(24, prev.size * 2) ? best : null;
}

/** The change to send: the typed text when it differs, with `patch`. Null when there is nothing to send. */
export function changeFor(block: TextBlock, draft: string | null, patch: BlockChange = {}): BlockChange | null {
  const change: BlockChange = { ...patch };
  if (draft != null && draft !== block.text) change.text = draft;
  return Object.keys(change).length === 0 ? null : change;
}

/** One sentence for a font that was replaced. The engine's reason names the original font and the letter. */
export function fallbackNote(fb: TextFallback): string {
  const face = fb.font.replace(/-/g, " ").replace("Oblique", "Italic");
  return `${fb.reason[0].toUpperCase()}${fb.reason.slice(1)}, so this block is now set in ${face}${fb.embedded ? " (embedded in the file)" : ""}.`;
}

/** A guess at where in `text` a click at (x, y) landed in a block of `lines` lines: proportional, not exact. */
export function caretFor(block: TextBlock, text: string, x: number, y: number): number {
  const [x0, y0, x1, y1] = block.rect;
  const n = Math.max(1, block.lines.length);
  const row = Math.min(n - 1, Math.max(0, Math.floor(((y - y0) / Math.max(1, y1 - y0)) * n)));
  const across = Math.min(1, Math.max(0, (x - x0) / Math.max(1, x1 - x0)));
  return Math.round(((row + across) / n) * text.length);
}

// ---- store ----

interface Store {
  /** Edit mode is on. */
  open: boolean;
  setOpen: (open: boolean) => void;
  tool: EditTool;
  setTool: (t: EditTool) => void;
  docs: Record<number, DocTextEdit>;

  /** Lists the page's blocks for the document's current generation (cached; one call per page and generation). */
  load: (doc: number, page: number) => Promise<void>;
  /** Selects a block (committing what was typed into the previous one first). */
  select: (doc: number, ref: BlockRef | null) => Promise<void>;
  setDraft: (doc: number, text: string) => void;
  /** Sends the typed text, if it differs. */
  commit: (doc: number) => Promise<void>;
  /** Applies `patch` (and any typed text) to the selected block as one undo step. */
  apply: (doc: number, patch: BlockChange) => Promise<void>;
  /** Moves the selected block by `d` (page space). */
  move: (doc: number, d: [number, number]) => Promise<void>;
  startAdding: (doc: number, page: number, at: [number, number]) => void;
  setAdding: (doc: number, text: string) => void;
  /** Puts the typed text on the page, or drops it when it is empty. */
  finishAdding: (doc: number) => Promise<void>;
  dismissNotice: (doc: number) => void;
  setNotice: (doc: number, notice: string | null) => void;
}

const inflight = new Map<string, Promise<void>>();

export const useTextEdit = create<Store>((set, get) => {
  const patchDoc = (doc: number, p: Partial<DocTextEdit>) =>
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? empty()), ...p } } }));
  const of = (doc: number): DocTextEdit => get().docs[doc] ?? empty();
  const selectedBlock = (doc: number): TextBlock | null => {
    const d = of(doc);
    return d.selected ? (d.pages[d.selected.page]?.blocks[d.selected.index] ?? null) : null;
  };
  const docRev = (doc: number) => useStore.getState().docs[doc]?.rev ?? 0;

  /** Lists the page again whatever is cached. */
  const reload = async (doc: number, page: number) => {
    const r = await api.textBlocks(doc, page);
    patchDoc(doc, { pages: { ...of(doc).pages, [page]: { rev: r.rev, blocks: r.blocks, areas: r.areas } } });
    return r;
  };

  return {
    open: false,
    setOpen: (open) => set(open ? { open } : { open, tool: "select" }),
    tool: "select",
    setTool: (tool) => set({ tool }),
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

    select: async (doc, ref) => {
      const cur = of(doc).selected;
      if (cur && ref && cur.page === ref.page && cur.index === ref.index) return;
      await get().commit(doc);
      patchDoc(doc, { selected: ref, draft: null, notice: null });
    },

    setDraft: (doc, text) => {
      const sel = of(doc).selected;
      if (sel) patchDoc(doc, { draft: { ...sel, text } });
    },

    commit: async (doc) => {
      const d = of(doc);
      const block = selectedBlock(doc);
      if (!d.selected || !block || !d.draft) return;
      const change = changeFor(block, d.draft.text);
      if (!change) return void patchDoc(doc, { draft: null });
      await get().apply(doc, {});
    },

    apply: async (doc, patch) => {
      const d = of(doc);
      const ref = d.selected;
      const block = selectedBlock(doc);
      const page = ref ? d.pages[ref.page] : undefined;
      if (!ref || !block || !page) return;
      const change = changeFor(block, d.draft?.text ?? null, patch);
      if (!change) return;
      const r = await editText(doc, page.rev, ref.page, ref.index, change);
      patchDoc(doc, { draft: null });
      if (!r) {
        // The document moved on under us: list the page again and keep the selection by position.
        const fresh = await reload(doc, ref.page);
        const same = closestBlock(block, fresh.blocks);
        patchDoc(doc, { selected: same ? { page: ref.page, index: same.index } : null });
        return;
      }
      const fresh = await reload(doc, ref.page);
      const moved: [number, number] = patch.offset ?? [0, 0];
      const same = closestBlock(block, fresh.blocks, moved);
      patchDoc(doc, {
        selected: same ? { page: ref.page, index: same.index } : null,
        notice: r.fallback ? fallbackNote(r.fallback) : null,
      });
    },

    move: (doc, d) => get().apply(doc, { offset: d }),

    startAdding: (doc, page, at) => {
      patchDoc(doc, { adding: { page, at, text: "" }, selected: null, draft: null, notice: null });
    },

    setAdding: (doc, text) => {
      const a = of(doc).adding;
      if (a) patchDoc(doc, { adding: { ...a, text } });
    },

    finishAdding: async (doc) => {
      const a = of(doc).adding;
      patchDoc(doc, { adding: null });
      set({ tool: "select" });
      if (!a || !a.text.trim()) return;
      const rev = docRev(doc);
      const s = await addText(doc, rev, a.page, { text: a.text, size: NEW_TEXT.size, color: NEW_TEXT.color, at: a.at });
      const fresh = await reload(doc, a.page);
      // The new block is the one that starts where it was put.
      const mine = fresh.blocks.filter((b) => b.editable).sort((p, q) => Math.hypot(p.rect[0] - a.at[0], p.rect[1] - a.at[1]) - Math.hypot(q.rect[0] - a.at[0], q.rect[1] - a.at[1]))[0];
      if (s && mine) patchDoc(doc, { selected: { page: a.page, index: mine.index } });
    },

    dismissNotice: (doc) => patchDoc(doc, { notice: null }),
    setNotice: (doc, notice) => patchDoc(doc, { notice }),
  };
});

// A closed tab's block lists are not kept.
useStore.subscribe((s, prev) => {
  const gone = Object.keys(useTextEdit.getState().docs).filter((k) => !s.docs[Number(k)] && prev.docs[Number(k)]);
  if (gone.length === 0) return;
  useTextEdit.setState((t) => {
    const docs = { ...t.docs };
    for (const k of gone) delete docs[Number(k)];
    return { docs };
  });
});
