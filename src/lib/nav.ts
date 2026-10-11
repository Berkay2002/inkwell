import { message } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { Bookmark, BookmarkOp, BorderSpec, HeadingPreview, LinkAudit, LinkOp, NewLinkTarget, PageLink, PageLinks, UrlPreview } from "./api";
import { editBookmarks, editLinks } from "./edit";
import { dropViewPos, getViewPos } from "./viewPos";

/**
 * Bookmarks and links in Edit mode. The lists come from the engine (`bookmarks_list`,
 * `links_list`) once per document generation; every change is one engine call and so one undo
 * step. The rail and the page layer read this store; nothing here opens an address.
 */

export type Rect4 = [number, number, number, number];
export type Rgb = [number, number, number];
export type LinkTool = "select" | "draw";

export interface DocNav {
  /** The tree as listed; null until the first list. */
  bookmarks: api.Bookmarks | null;
  selectedBm: number | null;
  /** The bookmark whose name is being typed. */
  renaming: number | null;
  /** What "Bookmarks from headings" would make; null until asked. */
  headings: HeadingPreview | null;
  /** What "Create links from web addresses" would make; null until asked. */
  urls: UrlPreview | null;
  /** Every link of the document counted, broken ones listed; null until asked. */
  audit: LinkAudit | null;
  links: Record<number, { rev: number; links: PageLink[] }>;
  selectedLink: { page: number; id: number } | null;
  /** A rectangle just drawn: the link is made when its target is chosen in the rail. */
  pending: { page: number; rect: Rect4 } | null;
  /** A quiet sentence in the rail: what was just done, or why something wasn't. */
  notice: string | null;
}

const empty = (): DocNav => ({
  bookmarks: null,
  selectedBm: null,
  renaming: null,
  headings: null,
  urls: null,
  audit: null,
  links: {},
  selectedLink: null,
  pending: null,
  notice: null,
});

// ---- pure helpers (tested in nav.test.ts) ----

export interface Where {
  /** The parent's id; null at the top level. */
  parent: number | null;
  index: number;
  siblings: Bookmark[];
}

/** Where bookmark `id` sits in the tree. */
export function locate(items: Bookmark[], id: number, parent: number | null = null): Where | null {
  for (let i = 0; i < items.length; i++) {
    if (items[i].id === id) return { parent, index: i, siblings: items };
    const w = locate(items[i].children, id, items[i].id);
    if (w) return w;
  }
  return null;
}

export function findBookmark(items: Bookmark[], id: number): Bookmark | null {
  for (const b of items) {
    if (b.id === id) return b;
    const f = findBookmark(b.children, id);
    if (f) return f;
  }
  return null;
}

/** `id` is `anc` or lies under it. */
export function isUnder(items: Bookmark[], id: number, anc: number): boolean {
  const a = findBookmark(items, anc);
  return a != null && (a.id === id || findBookmark(a.children, id) != null);
}

/** Bookmarks in tree order with their depth, for the visible rows (closed groups hide their members). */
export function visibleRows(items: Bookmark[], depth = 0, out: { b: Bookmark; depth: number }[] = []) {
  for (const b of items) {
    out.push({ b, depth });
    if (b.open && b.children.length > 0) visibleRows(b.children, depth + 1, out);
  }
  return out;
}

/** A bookmark's name from selected text: one line, trimmed, not longer than a bookmark should be. */
export function titleFromText(text: string, max = 120): string {
  const t = text.replace(/\s+/g, " ").trim();
  return t.length > max ? `${t.slice(0, max - 1).trimEnd()}…` : t;
}

export const rgbToHex = (c: Rgb) => `#${c.map((v) => Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, "0")).join("")}`;
export function hexToRgb(h: string): Rgb | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(h.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

/** The rectangle of a drag, normalised, or null when it is too small to be a link. */
export function dragRect(a: [number, number], b: [number, number], min = 4): Rect4 | null {
  const r: Rect4 = [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
  return r[2] - r[0] < min || r[3] - r[1] < min ? null : r;
}

/** What a state dot shows: where a bookmark or link goes, or that it goes nowhere that works. */
export type DotState = "ok" | "external" | "broken" | "empty";

export function bookmarkState(b: Bookmark): DotState {
  switch (b.dest.kind) {
    case "broken":
      return "broken";
    case "uri":
      return "external";
    case "none":
      return b.children.length === 0 ? "empty" : "ok";
    default:
      return "ok";
  }
}

export const linkState = (t: api.LinkTarget): DotState => (t.kind === "broken" ? "broken" : t.kind === "uri" ? "external" : "ok");

/** The words for a state dot (the dot alone is never the only signal). */
export function stateLabel(s: DotState, what?: string): string {
  switch (s) {
    case "broken":
      return what ? `Broken: ${what}` : "Broken destination";
    case "external":
      return "Goes to a web address";
    case "empty":
      return "Goes nowhere";
    default:
      return "Destination found";
  }
}

export const isProblem = (s: DotState) => s === "broken" || s === "empty";

/** The bookmarks that are a problem, each under the parents it has (opened), for "Show problems". */
export function onlyProblems(items: Bookmark[]): Bookmark[] {
  const out: Bookmark[] = [];
  for (const b of items) {
    const kids = onlyProblems(b.children);
    if (isProblem(bookmarkState(b)) || kids.length > 0) out.push({ ...b, children: kids, open: kids.length > 0 ? true : b.open });
  }
  return out;
}

export function countProblems(items: Bookmark[]): number {
  return items.reduce((n, b) => n + (isProblem(bookmarkState(b)) ? 1 : 0) + countProblems(b.children), 0);
}

/** A short line for a link's target, for lists. */
export function targetLabel(t: api.LinkTarget): string {
  switch (t.kind) {
    case "page":
      return `Page ${t.page + 1}`;
    case "uri":
      return t.uri;
    case "file":
      return t.page != null ? `${t.name}, page ${t.page + 1}` : t.name;
    case "broken":
      return "Missing destination";
    default:
      return t.what ? `Action: ${t.what}` : "No target";
  }
}

// ---- the text selection as a place for a bookmark ----

export interface SelectionSpot {
  text: string;
  page: number;
  /** Top left of the selection, page space. */
  at: [number, number];
}

/** The selected text in the viewer and where it starts, or null when nothing is selected. */
export function selectionSpot(): SelectionSpot | null {
  const sel = typeof window !== "undefined" ? window.getSelection() : null;
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return null;
  const text = titleFromText(sel.toString());
  if (!text) return null;
  const range = sel.getRangeAt(0);
  const node = range.startContainer.nodeType === 1 ? (range.startContainer as Element) : range.startContainer.parentElement;
  const pageEl = node?.closest<HTMLElement>("[data-page]");
  const space = pageEl?.querySelector<HTMLElement>(".pagespace");
  if (!pageEl || !space) return null;
  const page = Number(pageEl.dataset.page);
  const box = range.getClientRects()[0] ?? range.getBoundingClientRect();
  const m = new DOMMatrix(getComputedStyle(space).transform).inverse();
  const r = space.parentElement!.getBoundingClientRect();
  const q = m.transformPoint({ x: box.left - r.left, y: box.top - r.top });
  return Number.isFinite(page) ? { text, page, at: [Math.max(0, q.x), Math.max(0, q.y)] } : null;
}

// ---- store ----

interface Store {
  linkTool: LinkTool;
  setLinkTool: (t: LinkTool) => void;
  docs: Record<number, DocNav>;
  get: (doc: number) => DocNav;
  patch: (doc: number, p: Partial<DocNav>) => void;

  loadBookmarks: (doc: number) => Promise<void>;
  selectBookmark: (doc: number, id: number | null) => void;
  setRenaming: (doc: number, id: number | null) => void;
  /** A new bookmark after the selected one: named from the selected text, at the text or the current view. */
  addBookmark: (doc: number, opts?: { inside?: boolean }) => Promise<void>;
  rename: (doc: number, id: number, title: string) => Promise<void>;
  remove: (doc: number, ids: number[]) => Promise<void>;
  moveBookmark: (doc: number, id: number, parent: number | null, index: number) => Promise<void>;
  setOpen: (doc: number, id: number, open: boolean) => Promise<void>;
  setStyle: (doc: number, id: number, style: { bold?: boolean; italic?: boolean; color?: Rgb | null }) => Promise<void>;
  /** The bookmark goes to what the viewer shows now. */
  setDestHere: (doc: number, id: number) => Promise<void>;
  askHeadings: (doc: number) => Promise<void>;
  makeFromHeadings: (doc: number, replace: boolean) => Promise<void>;

  loadLinks: (doc: number, page: number) => Promise<void>;
  selectLink: (doc: number, sel: { page: number; id: number } | null) => void;
  setPending: (doc: number, p: { page: number; rect: Rect4 } | null) => void;
  addLink: (doc: number, page: number, rect: Rect4, target: NewLinkTarget, border?: BorderSpec) => Promise<number | null>;
  setLinkTarget: (doc: number, page: number, id: number, target: NewLinkTarget) => Promise<boolean>;
  setLinkRect: (doc: number, page: number, id: number, rect: Rect4) => Promise<void>;
  setLinkBorder: (doc: number, page: number, id: number, border: BorderSpec) => Promise<void>;
  removeLinks: (doc: number, page: number, ids: number[]) => Promise<void>;
  askUrls: (doc: number) => Promise<void>;
  askAudit: (doc: number) => Promise<void>;
  /** Only broken bookmarks and links are listed. */
  showProblems: boolean;
  setShowProblems: (on: boolean) => void;
  makeFromUrls: (doc: number) => Promise<void>;

  /** The document moved on (an edit, undo, redo): lists are listed again. */
  onRev: (doc: number) => void;
  setNotice: (doc: number, n: string | null) => void;
}

const docRev = (doc: number) => useStore.getState().docs[doc]?.rev ?? 0;
const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

export const useNav = create<Store>((set, get) => {
  const of = (doc: number): DocNav => get().docs[doc] ?? empty();
  const patch = (doc: number, p: Partial<DocNav>) => set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? empty()), ...p } } }));
  const inflight = new Map<string, Promise<void>>();

  const reloadBookmarks = (doc: number) => {
    const key = `b:${doc}:${docRev(doc)}`;
    const running = inflight.get(key);
    if (running) return running;
    const job = api
      .bookmarksList(doc)
      .then((b) => {
        const d = of(doc);
        const sel = d.selectedBm != null && !findBookmark(b.items, d.selectedBm) ? null : d.selectedBm;
        patch(doc, { bookmarks: b, selectedBm: sel, renaming: d.renaming != null && !findBookmark(b.items, d.renaming) ? null : d.renaming });
      })
      .catch(() => undefined)
      .finally(() => inflight.delete(key));
    inflight.set(key, job);
    return job;
  };

  const reloadLinks = (doc: number, page: number) => {
    const key = `l:${doc}:${page}:${docRev(doc)}`;
    const running = inflight.get(key);
    if (running) return running;
    const job = api
      .linksList(doc, page)
      .then((l: PageLinks) => {
        const d = of(doc);
        const sel = d.selectedLink && d.selectedLink.page === page && !l.links.some((k) => k.id === d.selectedLink!.id) ? null : d.selectedLink;
        patch(doc, { links: { ...d.links, [page]: { rev: l.rev, links: l.links } }, selectedLink: sel });
      })
      .catch(() => undefined)
      .finally(() => inflight.delete(key));
    inflight.set(key, job);
    return job;
  };

  /** One bookmark change at the generation the tree was listed at; a stale one lists again and says so. */
  const bm = async (doc: number, op: BookmarkOp): Promise<api.BookmarkEdited | null> => {
    if (!of(doc).bookmarks) await reloadBookmarks(doc);
    const rev = of(doc).bookmarks?.rev ?? docRev(doc);
    const r = await editBookmarks(doc, rev, op);
    if (r.kind === "ok") {
      await reloadBookmarks(doc);
      return r.value;
    }
    if (r.kind === "stale") {
      await reloadBookmarks(doc);
      patch(doc, { notice: "The document changed while you were editing. Try that again." });
    }
    return null;
  };

  const lk = async (doc: number, page: number, op: LinkOp): Promise<api.LinkEdited | null> => {
    const known = of(doc).links[page];
    if (!known) await reloadLinks(doc, page);
    const rev = of(doc).links[page]?.rev ?? docRev(doc);
    const r = await editLinks(doc, rev, op);
    if (r.kind === "ok") return r.value;
    if (r.kind === "stale") {
      await reloadLinks(doc, page);
      patch(doc, { notice: "The document changed while you were editing. Try that again." });
    }
    return null;
  };

  return {
    linkTool: "select",
    setLinkTool: (linkTool) => set({ linkTool }),
    docs: {},
    get: of,
    patch,

    loadBookmarks: async (doc) => {
      const b = of(doc).bookmarks;
      if (b && b.rev === docRev(doc)) return;
      await reloadBookmarks(doc);
    },
    selectBookmark: (doc, id) => patch(doc, { selectedBm: id, notice: null }),
    setRenaming: (doc, id) => patch(doc, { renaming: id, ...(id != null ? { selectedBm: id } : {}) }),

    addBookmark: async (doc, opts) => {
      if (!of(doc).bookmarks) await reloadBookmarks(doc);
      const d = of(doc);
      const tree = d.bookmarks?.items ?? [];
      const spot = selectionSpot();
      const view = getViewPos(doc);
      const st = useStore.getState().docs[doc];
      const page = spot?.page ?? view?.page ?? st?.currentPage ?? 0;
      const at: [number, number] | null = spot ? spot.at : view && view.page === page ? [0, Math.max(0, view.yPt)] : null;
      const after = d.selectedBm != null ? locate(tree, d.selectedBm) : null;
      const inside = !!opts?.inside && after != null && d.selectedBm != null;
      const r = await bm(doc, {
        op: "add",
        parent: inside ? d.selectedBm : (after?.parent ?? null),
        index: inside ? null : after ? after.index + 1 : null,
        title: spot?.text ?? "Untitled",
        page,
        at,
      });
      if (r?.id != null) {
        // A bookmark made from nothing is named in place; one named by the text is taken as it is.
        patch(doc, { selectedBm: r.id, renaming: spot ? null : r.id, notice: null });
      }
    },

    rename: async (doc, id, title) => {
      patch(doc, { renaming: null });
      const cur = findBookmark(of(doc).bookmarks?.items ?? [], id);
      const t = title.replace(/\s+/g, " ").trim();
      if (!cur || !t || t === cur.title) return;
      await bm(doc, { op: "rename", id, title: t });
    },

    remove: async (doc, ids) => {
      if (ids.length === 0) return;
      const r = await bm(doc, { op: "delete", ids });
      if (r) patch(doc, { selectedBm: null, renaming: null, notice: r.count > 1 ? `${plural(r.count, "bookmark")} deleted.` : null });
    },

    moveBookmark: async (doc, id, parent, index) => {
      const tree = of(doc).bookmarks?.items ?? [];
      if (parent === id || (parent != null && isUnder(tree, parent, id))) return;
      // `index` counts the siblings as they are now; the engine counts them after the item is taken out.
      const w = locate(tree, id);
      let at = index;
      if (w && w.parent === parent && w.index < index) at -= 1;
      if (w && w.parent === parent && w.index === at) return;
      await bm(doc, { op: "move", id, parent, index: at });
    },

    setOpen: async (doc, id, open) => void (await bm(doc, { op: "setOpen", id, open })),

    setStyle: async (doc, id, style) => {
      const op: BookmarkOp = { op: "setStyle", id };
      if (style.bold !== undefined) op.bold = style.bold;
      if (style.italic !== undefined) op.italic = style.italic;
      if (style.color === null) op.resetColor = true;
      else if (style.color) op.color = style.color;
      await bm(doc, op);
    },

    setDestHere: async (doc, id) => {
      const view = getViewPos(doc);
      const st = useStore.getState().docs[doc];
      const page = view?.page ?? st?.currentPage ?? 0;
      await bm(doc, { op: "setDest", id, page, at: view ? [0, Math.max(0, view.yPt)] : null });
    },

    askHeadings: async (doc) => {
      try {
        patch(doc, { headings: await api.bookmarksHeadings(doc) });
      } catch (e) {
        patch(doc, { headings: null, notice: String(e) });
      }
    },

    makeFromHeadings: async (doc, replace) => {
      const r = await bm(doc, { op: "fromHeadings", replace });
      if (r) patch(doc, { headings: null, notice: `${plural(r.count, "bookmark")} made from the headings.` });
    },

    loadLinks: async (doc, page) => {
      const l = of(doc).links[page];
      if (l && l.rev === docRev(doc)) return;
      await reloadLinks(doc, page);
    },
    selectLink: (doc, sel) => patch(doc, { selectedLink: sel, notice: null, ...(sel ? { pending: null } : {}) }),
    setPending: (doc, pending) => patch(doc, { pending, notice: null, ...(pending ? { selectedLink: null } : {}) }),

    addLink: async (doc, page, rect, target, border) => {
      const r = await lk(doc, page, { op: "add", page, rect, target, ...(border ? { border } : {}) });
      if (!r) return null;
      await reloadLinks(doc, page);
      if (r.id != null) patch(doc, { selectedLink: { page, id: r.id }, pending: null, urls: null });
      return r.id;
    },

    setLinkTarget: async (doc, page, id, target) => {
      const r = await lk(doc, page, { op: "setTarget", page, id, target });
      if (r) await reloadLinks(doc, page);
      return r != null;
    },

    setLinkRect: async (doc, page, id, rect) => {
      const r = await lk(doc, page, { op: "setRect", page, id, rect });
      if (r) await reloadLinks(doc, page);
    },

    setLinkBorder: async (doc, page, id, border) => {
      const r = await lk(doc, page, { op: "setBorder", page, id, border });
      if (r) await reloadLinks(doc, page);
    },

    removeLinks: async (doc, page, ids) => {
      const r = await lk(doc, page, { op: "delete", page, ids });
      if (r) {
        await reloadLinks(doc, page);
        patch(doc, { selectedLink: null });
      }
    },

    askUrls: async (doc) => {
      try {
        patch(doc, { urls: await api.linksUrls(doc) });
      } catch (e) {
        patch(doc, { urls: null, notice: String(e) });
      }
    },

    askAudit: async (doc) => {
      try {
        patch(doc, { audit: await api.linksAudit(doc) });
      } catch {
        patch(doc, { audit: null });
      }
    },
    showProblems: false,
    setShowProblems: (showProblems) => set({ showProblems }),

    makeFromUrls: async (doc) => {
      const page = useStore.getState().docs[doc]?.currentPage ?? 0;
      const r = await lk(doc, page, { op: "fromUrls" });
      if (r) patch(doc, { urls: null, notice: `${plural(r.count, "link")} made from web addresses.` });
    },

    onRev: (doc) => {
      const d = get().docs[doc];
      if (!d) return;
      patch(doc, { headings: null, urls: null, audit: null });
      if (d.bookmarks) void reloadBookmarks(doc);
      for (const k of Object.keys(d.links)) void reloadLinks(doc, Number(k));
    },
    setNotice: (doc, notice) => patch(doc, { notice }),
  };
});

/** Asks before an address is opened: nothing in a document goes to the web on its own. */
export async function confirmOpen(url: string): Promise<boolean> {
  let shown: string;
  try {
    shown = await api.linkCheckUrl(url);
  } catch (e) {
    await message(String(e), { title: "Link", kind: "info" });
    return false;
  }
  const r = await message(`This link goes to:\n\n${shown}\n\nOpen it in your browser or mail program?`, {
    title: "Open link",
    kind: "warning",
    buttons: { ok: "Open", cancel: "Cancel" },
  });
  if (r === "Ok" || r === "Open") {
    await api.linkOpenUrl(shown);
    return true;
  }
  return false;
}

// A closed tab's lists are not kept.
useStore.subscribe((s, prev) => {
  const gone = Object.keys(useNav.getState().docs).filter((k) => !s.docs[Number(k)] && prev.docs[Number(k)]);
  if (gone.length === 0) return;
  useNav.setState((n) => {
    const docs = { ...n.docs };
    for (const k of gone) {
      delete docs[Number(k)];
      dropViewPos(Number(k));
    }
    return { docs };
  });
});
