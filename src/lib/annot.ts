import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { EditState } from "./api";
import type { Annot, Filter, Kind, Pt, Quad8, Rect4, RGB } from "./annotLogic";
import { applyState } from "./edit";

/**
 * Comments for the UI (src-tauri/src/annot). Every change is one engine edit (one undo step) that
 * Save keeps; the list is re-read after each one, so undo and redo stay in step with the panel.
 */

export type NewAnnot =
  | { type: "markup"; markup: Kind; quads: Quad8[] }
  | { type: "note"; at: Pt; text: string }
  | { type: "freeText"; rect: Rect4; text: string; fontSize?: number }
  | { type: "ink"; strokes: Pt[][] }
  | { type: "shape"; shape: Kind; points: Pt[] }
  | { type: "stamp"; name: string; rect: Rect4 };

export interface Style {
  color?: RGB;
  fill?: RGB;
  opacity?: number;
  width?: number;
}
export interface Patch extends Style {
  noFill?: boolean;
  contents?: string;
  rect?: Rect4;
  fontSize?: number;
}

export const annotApi = {
  list: (doc: number) => invoke<Annot[]>("annot_list", { doc }),
  add: (doc: number, page: number, what: NewAnnot, style: Style | null, author: string) =>
    invoke<{ state: EditState; xref: number }>("annot_add", { doc, page, what, style, author }),
  edit: (doc: number, page: number, xref: number, patch: Patch, author: string) =>
    invoke<EditState>("annot_edit", { doc, page, xref, patch, author }),
  remove: (doc: number, items: [number, number][]) => invoke<EditState>("annot_delete", { doc, items }),
  reply: (doc: number, page: number, parent: number, text: string, author: string) =>
    invoke<{ state: EditState; xref: number }>("annot_reply", { doc, page, parent, text, author }),
  state: (doc: number, page: number, parent: number, state: string, author: string) =>
    invoke<EditState>("annot_state", { doc, page, parent, state, author }),
};

/** The comment toolbar: select, text markup, note, text box, pen, one of the shapes, stamp. */
export type Tool =
  | "select" | "highlight" | "underline" | "strikeOut" | "squiggly" | "note" | "freeText" | "pen"
  | "rectangle" | "ellipse" | "line" | "arrow" | "polygon" | "stamp";

export const SHAPE_TOOLS: Tool[] = ["rectangle", "ellipse", "line", "arrow", "polygon"];
export const MARKUP_TOOLS: Tool[] = ["highlight", "underline", "strikeOut", "squiggly"];

export interface Ref {
  page: number;
  xref: number;
}

export interface DocAnnots {
  items: Annot[];
  selected: Ref | null;
  /** The thread or box whose text is being edited on the page. */
  editing: Ref | null;
  filter: Filter;
  query: string;
  error: string | null;
}

const fresh = (): DocAnnots => ({ items: [], selected: null, editing: null, filter: "all", query: "", error: null });

const AUTHOR_KEY = "inkwell.author";
const loadAuthor = () => {
  try {
    return localStorage.getItem(AUTHOR_KEY) ?? "";
  } catch {
    return "";
  }
};

interface AnnotStore {
  /** The Comments rail is showing. */
  open: boolean;
  setOpen: (open: boolean) => void;
  tool: Tool;
  setTool: (t: Tool) => void;
  /** Stamp the next click places. */
  stamp: string;
  setStamp: (s: string) => void;
  /** Colour, opacity and thickness new annotations get; null colour = the tool's own default. */
  style: Style;
  setStyle: (s: Style) => void;
  /** From settings; every comment is written with it. */
  author: string;
  setAuthor: (a: string) => void;

  docs: Record<number, DocAnnots>;
  get: (doc: number) => DocAnnots;
  refresh: (doc: number) => Promise<void>;
  select: (doc: number, ref: Ref | null) => void;
  setEditing: (doc: number, ref: Ref | null) => void;
  setFilter: (doc: number, f: Filter) => void;
  setQuery: (doc: number, q: string) => void;
  add: (doc: number, page: number, what: NewAnnot, style?: Style) => Promise<number | null>;
  edit: (doc: number, page: number, xref: number, patch: Patch) => Promise<void>;
  remove: (doc: number, refs: Ref[]) => Promise<void>;
  reply: (doc: number, page: number, parent: number, text: string) => Promise<void>;
  setResolved: (doc: number, page: number, parent: number, resolved: boolean) => Promise<void>;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));

export const useAnnot = create<AnnotStore>((set, get) => {
  const patch = (doc: number, p: Partial<DocAnnots>) =>
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? fresh()), ...p } } }));
  /** Runs one engine call, shows its new state, and re-reads the list. */
  const run = async <T>(doc: number, f: () => Promise<T>, state: (r: T) => EditState): Promise<T | null> => {
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

  return {
    open: false,
    setOpen: (open) => set(open ? { open } : { open, tool: "select" }),
    tool: "select",
    setTool: (tool) => set({ tool }),
    stamp: "Approved",
    setStamp: (stamp) => set({ stamp }),
    style: {},
    setStyle: (style) => set({ style }),
    author: loadAuthor(),
    setAuthor: (author) => {
      try {
        localStorage.setItem(AUTHOR_KEY, author);
      } catch {
        /* no storage: kept for the session */
      }
      set({ author });
    },

    docs: {},
    get: (doc) => get().docs[doc] ?? fresh(),
    refresh: async (doc) => {
      try {
        const items = await annotApi.list(doc);
        const d = get().get(doc);
        const has = (r: Ref | null) => (r && items.some((a) => a.page === r.page && a.xref === r.xref) ? r : null);
        patch(doc, { items, selected: has(d.selected), editing: has(d.editing) });
      } catch (e) {
        patch(doc, { error: msg(e) });
      }
    },
    select: (doc, selected) => patch(doc, { selected, editing: null }),
    setEditing: (doc, editing) => patch(doc, { editing, selected: editing ?? get().get(doc).selected }),
    setFilter: (doc, filter) => patch(doc, { filter }),
    setQuery: (doc, query) => patch(doc, { query }),

    add: async (doc, page, what, style) => {
      const { author } = get();
      const r = await run(doc, () => annotApi.add(doc, page, what, style ?? (Object.keys(get().style).length ? get().style : null), author), (x) => x.state);
      if (!r) return null;
      patch(doc, { selected: { page, xref: r.xref } });
      return r.xref;
    },
    edit: async (doc, page, xref, p) => {
      await run(doc, () => annotApi.edit(doc, page, xref, p, get().author), (s) => s);
    },
    remove: async (doc, refs) => {
      if (refs.length === 0) return;
      await run(doc, () => annotApi.remove(doc, refs.map((r) => [r.page, r.xref])), (s) => s);
      patch(doc, { selected: null, editing: null });
    },
    reply: async (doc, page, parent, text) => {
      if (!text.trim()) return;
      await run(doc, () => annotApi.reply(doc, page, parent, text.trim(), get().author), (x) => x.state);
    },
    setResolved: async (doc, page, parent, resolved) => {
      await run(doc, () => annotApi.state(doc, page, parent, resolved ? "Completed" : "None", get().author), (s) => s);
    },
  };
});
