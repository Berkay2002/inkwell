import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { BlockChange, RunChange, TextArea, TextBlock, TextFallback, TextRun } from "./api";
import { addText, editText, setLeaveGuard } from "./edit";

/**
 * Editing text in place: the viewer's Edit mode. A page's blocks come from one `text_blocks` call
 * per page and document generation (cached here); every change is one `text_edit` and so one
 * undo step. See docs/text-edit.md for what the engine can and cannot do.
 */

export type Rect4 = [number, number, number, number];
export type Rgb = [number, number, number];
export type EditTool = "select" | "text";
/** What the Edit rail is for: the page's text, its links, or the document's bookmarks. */
export type EditSection = "text" | "links" | "bookmarks";

/** Opens Edit mode on a section of its rail (from the tools panel, the palette): Organize is left first. */
export function openEditSection(doc: number, section: EditSection) {
  const st = useStore.getState();
  if (st.docs[doc]?.organize) st.patch(doc, { organize: false });
  const te = useTextEdit.getState();
  te.setOpen(true);
  te.setSection(section);
}

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
  /** Why the text can't be changed (a certified document); the format controls say so. */
  locked: string | null;
  signed: boolean;
}

export interface DocTextEdit {
  pages: Record<number, PageBlocks>;
  selected: BlockRef | null;
  draft: Draft | null;
  /** The editor's selected text, as offsets into its value; null when nothing is selected. */
  selection: [number, number] | null;
  adding: NewBox | null;
  /** A quiet sentence under the format controls: a font that was replaced, or why a block is locked. */
  notice: string | null;
  /** The engine refused what was typed: it is kept, and leaving the block twice drops it. */
  failed: boolean;
}

/** What "Add text" starts with (Acrobat's default is about 12 pt). */
export const NEW_TEXT = { size: 12, color: [0, 0, 0] as Rgb };

const empty = (): DocTextEdit => ({ pages: {}, selected: null, draft: null, selection: null, adding: null, notice: null, failed: false });

/** What a signed document's text edit says (the format section shows it). */
export const SIGNED_TEXT =
  "This document is signed. A change to its text is saved after the signature, which will show that the file was changed.";

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
  const stripped = name.replace(/[-,](Bold|Italic|Oblique|Regular|BoldItalic|BoldOblique|Roman|Medium|Light|Semibold)+(MT|PS)?$/i, "").replace(/(PSMT|MT|PS)$/, "");
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
  const face = `${fb.font.replace(/-/g, " ").replace("Oblique", "Italic")}${fb.embedded ? " (embedded in the file)" : ""}`;
  const why = fb.reason.trim();
  return why ? `${why[0].toUpperCase()}${why.slice(1)}, so this block is now set in ${face}.` : `This block is now set in ${face}.`;
}

export type Decor = "underline" | "strike" | "super" | "sub";

const has = (r: TextRun, d: Decor) => (d === "underline" ? r.underline : d === "strike" ? r.strike : r.rise === d);

/** Whether the runs add up to the block's text, so a selection's offsets can be cut into them. */
const cuttable = (b: TextBlock) => b.runs.length > 0 && b.runs.map((r) => r.text).join("") === b.text;

/** The runs a non-empty selection `sel` touches, or all of them: no selection, or runs that don't add up to the text. */
function touched(block: TextBlock, sel: [number, number] | null): TextRun[] {
  if (!sel || sel[0] >= sel[1] || !cuttable(block)) return block.runs;
  let at = 0;
  return block.runs.filter((r) => {
    const from = at;
    at += r.text.length;
    return from < sel[1] && at > sel[0];
  });
}

/** Whether `d` is on for what a press would change: the selection's runs, else the whole block. */
export function decorOn(block: TextBlock, d: Decor, sel: [number, number] | null = null): boolean {
  const runs = touched(block, sel);
  return runs.length > 0 && runs.every((r) => has(r, d));
}

/**
 * The change a press of `d` makes: off when it is on for everything touched, else on (super and sub
 * replace one another). With a selection the runs are cut at its ends and only the pieces inside are
 * marked, the others sent back as they were; without one, or when the runs don't add up to the text,
 * it is the block's own field and applies to every run.
 */
export function decorChange(block: TextBlock, d: Decor, sel: [number, number] | null = null): BlockChange {
  const on = decorOn(block, d, sel);
  const mark: RunChange = d === "underline" ? { underline: !on } : d === "strike" ? { strike: !on } : { rise: on ? null : d };
  if (!sel || sel[0] >= sel[1] || !cuttable(block)) return mark;
  const runs: RunChange[] = [];
  let at = 0;
  for (const r of block.runs) {
    const [from, to] = [at, (at += r.text.length)];
    const [lo, hi] = [Math.max(from, sel[0]), Math.min(to, sel[1])];
    if (lo >= hi) runs.push({ source: r.index });
    else if (lo === from && hi === to) runs.push({ source: r.index, ...mark });
    else {
      // Only a cut piece has text of its own: the whole run keeps its glyphs.
      if (lo > from) runs.push({ source: r.index, text: r.text.slice(0, lo - from) });
      runs.push({ source: r.index, text: r.text.slice(lo - from, hi - from), ...mark });
      if (hi < to) runs.push({ source: r.index, text: r.text.slice(hi - from) });
    }
  }
  return { runs };
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
  section: EditSection;
  /** Leaving the text section sends what was typed first. */
  setSection: (s: EditSection) => void;
  docs: Record<number, DocTextEdit>;

  /** Lists the page's blocks for the document's current generation (cached; one call per page and generation). */
  load: (doc: number, page: number) => Promise<void>;
  /** Selects a block (sending what was typed into the previous one first; it stays when that fails). */
  select: (doc: number, ref: BlockRef | null) => Promise<void>;
  setDraft: (doc: number, text: string) => void;
  /** The editor's selected text (offsets into its value), which decorations apply to. */
  setSelection: (doc: number, sel: [number, number] | null) => void;
  /** Toggles underline, strikeout or super/subscript on the selection, else the whole block: one undo step. */
  decorate: (doc: number, d: Decor) => Promise<void>;
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
  /** Drops what was typed (and a new box) without sending it: the way out of text the engine refused. */
  discard: (doc: number) => void;
  dismissNotice: (doc: number) => void;
  setNotice: (doc: number, notice: string | null) => void;
}

const inflight = new Map<string, Promise<void>>();
/** One change of a document at a time, in the order asked for: a commit that finds nothing left to send does nothing. */
const queues = new Map<number, Promise<unknown>>();
function queued<T>(doc: number, f: () => Promise<T>): Promise<T> {
  const run = (queues.get(doc) ?? Promise.resolve()).then(f, f);
  queues.set(doc, run.catch(() => undefined));
  return run;
}

export const useTextEdit = create<Store>((set, get) => {
  const patchDoc = (doc: number, p: Partial<DocTextEdit>) =>
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? empty()), ...p } } }));
  const of = (doc: number): DocTextEdit => get().docs[doc] ?? empty();
  const selectedBlock = (doc: number): TextBlock | null => {
    const d = of(doc);
    return d.selected ? (d.pages[d.selected.page]?.blocks[d.selected.index] ?? null) : null;
  };
  const docRev = (doc: number) => useStore.getState().docs[doc]?.rev ?? 0;
  /** Documents with a change in flight: that change settles its own selection. */
  const applying = new Set<number>();

  const fetchPage = async (doc: number, page: number) => {
    const r = await api.textBlocks(doc, page);
    patchDoc(doc, { pages: { ...of(doc).pages, [page]: { rev: r.rev, blocks: r.blocks, areas: r.areas, locked: r.locked, signed: r.signed } } });
    return r;
  };

  /**
   * Lists the page again. When the document moved on under a selection (undo, redo, a restored
   * version), the selection follows its block by position, or is dropped with its typed text: an
   * index must never point at another block.
   */
  const reload = async (doc: number, page: number) => {
    const before = of(doc);
    const sel = before.selected;
    const was = sel && sel.page === page ? (before.pages[page]?.blocks[sel.index] ?? null) : null;
    const r = await fetchPage(doc, page);
    if (sel && was && !applying.has(doc)) {
      const now = of(doc);
      if (now.selected?.page === page && now.selected.index === sel.index) {
        const same = closestBlock(was, r.blocks);
        if (!same) patchDoc(doc, { selected: null, draft: null });
        else if (same.index !== sel.index) {
          patchDoc(doc, { selected: { page, index: same.index }, draft: now.draft ? { ...now.draft, page, index: same.index } : null });
        }
      }
    }
    return r;
  };

  /** Sends the typed text and `patch` as one change. Runs inside the document's queue. */
  const applyNow = async (doc: number, patch: BlockChange) => {
    const d = of(doc);
    const ref = d.selected;
    const block = selectedBlock(doc);
    const page = ref ? d.pages[ref.page] : undefined;
    if (!ref || !block || !page) return;
    const sent = d.draft?.text ?? null;
    const change = changeFor(block, sent, patch);
    if (!change) {
      // Typed back to what it was: nothing to send.
      if (sent != null) patchDoc(doc, { draft: null });
      return;
    }
    applying.add(doc);
    try {
      const r = await editText(doc, page.rev, ref.page, ref.index, change);
      if (r.kind === "error") {
        // Keep everything typed: the block stays selected with its draft, and the person is told why.
        patchDoc(doc, { failed: true, notice: `This change could not be applied: ${r.message}` });
        return;
      }
      const fresh = await fetchPage(doc, ref.page);
      const moved: [number, number] = patch.offset ?? [0, 0];
      const same = closestBlock(block, fresh.blocks, r.kind === "ok" ? moved : [0, 0]);
      const cur = of(doc).draft;
      if (r.kind === "stale") {
        // The document moved on under us: the block is found again by position and keeps its draft.
        patchDoc(doc, {
          selected: same ? { page: ref.page, index: same.index } : null,
          draft: same && cur ? { ...cur, page: ref.page, index: same.index } : null,
          failed: false,
          notice: same ? null : "The page changed, so the text typed in that block was not applied.",
        });
        return;
      }
      // Only the text that was sent is settled: keystrokes made while it was in flight stay as the draft.
      const keep = cur && cur.text !== sent && same ? { ...cur, page: ref.page, index: same.index } : null;
      patchDoc(doc, {
        selected: same ? { page: ref.page, index: same.index } : null,
        draft: keep,
        failed: false,
        notice: r.value.fallback ? fallbackNote(r.value.fallback) : null,
      });
    } finally {
      applying.delete(doc);
    }
  };

  const commitNow = async (doc: number) => {
    // A refused text is not sent again until it is changed (typing clears `failed`).
    if (of(doc).selected && of(doc).draft && !of(doc).failed) await applyNow(doc, {});
  };

  return {
    open: false,
    setOpen: (open) => {
      if (open) return set({ open });
      // Leaving Edit sends what was typed (one undo step) and clears the selection.
      const ids = Object.keys(get().docs).map(Number);
      set({ open, tool: "select" });
      void Promise.all(ids.map((id) => get().commit(id))).then(() => {
        if (get().open) return;
        // Typed text that could not be sent stays (with its block) for the person to see on return.
        for (const id of ids) if (!of(id).failed) patchDoc(id, { selected: null, draft: null, adding: null, notice: null });
      });
    },
    tool: "select",
    setTool: (tool) => set({ tool }),
    section: "text",
    setSection: (section) => {
      if (section === get().section) return;
      const ids = Object.keys(get().docs).map(Number);
      set({ section, tool: "select" });
      // Typed text is sent (one undo step) when its layer goes away.
      if (section !== "text") void Promise.all(ids.map((id) => get().commit(id)));
    },
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
      // A change the engine refused keeps its block: moving on would lose the typed text.
      if (of(doc).failed) return;
      patchDoc(doc, { selected: ref, draft: null, selection: null, notice: null });
    },

    setDraft: (doc, text) => {
      const sel = of(doc).selected;
      if (sel) patchDoc(doc, { draft: { ...sel, text }, failed: false });
    },

    setSelection: (doc, selection) => {
      const cur = of(doc).selection;
      if (cur?.[0] !== selection?.[0] || cur?.[1] !== selection?.[1]) patchDoc(doc, { selection });
    },

    decorate: (doc, d) =>
      queued(doc, async () => {
        // Typed text goes first, so the runs the change is cut from are the document's own.
        const typed = of(doc).draft?.text;
        await commitNow(doc);
        const block = selectedBlock(doc);
        if (!block || of(doc).failed) return;
        // The engine tidies spaces: a selection made in text it changed no longer lines up.
        const sel = typed == null || typed === block.text ? of(doc).selection : null;
        await applyNow(doc, decorChange(block, d, sel));
      }),

    commit: (doc) => queued(doc, () => commitNow(doc)),
    apply: (doc, patch) => queued(doc, () => applyNow(doc, patch)),
    move: (doc, d) => get().apply(doc, { offset: d }),

    startAdding: (doc, page, at) => {
      patchDoc(doc, { adding: { page, at, text: "" }, selected: null, draft: null, failed: false, notice: null });
    },

    setAdding: (doc, text) => {
      const a = of(doc).adding;
      if (a) patchDoc(doc, { adding: { ...a, text }, failed: false });
    },

    finishAdding: (doc) =>
      queued(doc, async () => {
        const a = of(doc).adding;
        if (!a) return;
        if (!a.text.trim()) {
          patchDoc(doc, { adding: null, failed: false });
          set({ tool: "select" });
          return;
        }
        const box = { text: a.text, size: NEW_TEXT.size, color: NEW_TEXT.color, at: a.at };
        // One retry when the document moved on since the box was started.
        let r = await addText(doc, docRev(doc), a.page, box);
        if (r.kind === "stale") r = await addText(doc, docRev(doc), a.page, box);
        if (r.kind !== "ok") {
          // The box and what was typed stay, so it can be fixed or tried again.
          patchDoc(doc, { failed: true, notice: r.kind === "error" ? `This text could not be added: ${r.message}` : "The page changed; try adding the text again." });
          return;
        }
        patchDoc(doc, { adding: null, failed: false });
        set({ tool: "select" });
        const fresh = await fetchPage(doc, a.page);
        // The new block is the one that starts where it was put.
        const dist = (b: TextBlock) => Math.hypot(b.rect[0] - a.at[0], b.rect[1] - a.at[1]);
        const mine = fresh.blocks.filter((b) => b.editable).sort((p, q) => dist(p) - dist(q))[0];
        if (mine) patchDoc(doc, { selected: { page: a.page, index: mine.index } });
      }),

    discard: (doc) => {
      patchDoc(doc, { selected: null, draft: null, adding: null, failed: false, notice: null });
      set({ tool: "select" });
    },

    dismissNotice: (doc) => patchDoc(doc, { notice: null }),
    setNotice: (doc, notice) => patchDoc(doc, { notice }),
  };
});

// Typed text is part of the document's unsaved state: closing a tab or the window sends it first
// (edit.ts asks the person only when it could not be sent).
setLeaveGuard({
  flush: (id) => useTextEdit.getState().commit(id),
  unsent: (id) => {
    const d = useTextEdit.getState().docs[id];
    if (!d) return null;
    if (d.adding && d.adding.text.trim()) return d.adding.text;
    const b = d.selected ? d.pages[d.selected.page]?.blocks[d.selected.index] : null;
    return d.draft && b && d.draft.text !== b.text ? d.draft.text : null;
  },
});

useStore.subscribe((s, prev) => {
  const te = useTextEdit.getState();
  // A closed tab's block lists are not kept.
  const gone = Object.keys(te.docs).filter((k) => !s.docs[Number(k)] && prev.docs[Number(k)]);
  if (gone.length > 0) {
    useTextEdit.setState((t) => {
      const docs = { ...t.docs };
      for (const k of gone) delete docs[Number(k)];
      return { docs };
    });
  }
  // Leaving a tab, or turning to Organize, unmounts the editor without a blur: send what was typed.
  if (prev.active != null && s.active !== prev.active && s.docs[prev.active]) void te.commit(prev.active);
  for (const k of Object.keys(s.docs)) {
    const id = Number(k);
    if (s.docs[id].organize && prev.docs[id] && !prev.docs[id].organize) void te.commit(id);
  }
});
