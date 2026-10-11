import { create } from "zustand";
import * as api from "./lib/api";
import type { OutlineItem } from "./lib/api";
import { type Rot, type Size, clamp, ZOOM_MAX, ZOOM_MIN } from "./lib/layout";
import { keyOf } from "./lib/recent";
import { dropCached } from "./lib/tiles";
import { useRecents } from "./recents";

export type ZoomMode = "fitWidth" | "fitPage" | "custom";
export type Theme = "system" | "light" | "dark";
export type SidebarPanel = "none" | "thumbs" | "outline";
export type Rect4 = [number, number, number, number];

export interface Hit {
  page: number;
  rect: Rect4;
}

export interface SearchState {
  query: string;
  /** Engine search id; events from older searches are ignored. */
  id: number | null;
  hits: Hit[];
  /** Index into `hits`, -1 when nothing is selected yet. */
  current: number;
  done: boolean;
}

export interface NavRequest {
  page: number;
  /** Optional y offset inside the page, in points (unrotated page space). */
  y?: number;
  nonce: number;
}

export interface DocState {
  id: number;
  path: string;
  name: string;
  title: string | null;
  pageCount: number;
  sizes: Size[];
  rot: Rot;
  zoomMode: ZoomMode;
  zoom: number;
  currentPage: number;
  /** Edit generation reported by the engine; tiles are keyed by it. */
  rev: number;
  /** Edited since opened or last saved (see lib/edit.ts). */
  dirty: boolean;
  /** Open token of a created file that nobody has saved yet: Save asks where it goes, closing asks first. */
  scratch: string | null;
  /** What Undo / Redo would do, null when there is nothing. */
  undoLabel: string | null;
  redoLabel: string | null;
  /** The Organize pages view is showing instead of the reader. */
  organize: boolean;
  /** Where the viewer was when the tab was last left. */
  scroll: { page: number; yPt: number };
  outline: OutlineItem[] | null;
  search: SearchState;
  nav: NavRequest | null;
}

const emptySearch = (): SearchState => ({ query: "", id: null, hits: [], current: -1, done: true });

const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

const loadTheme = (): Theme => (localStorage.getItem("inkwell.theme") as Theme | null) ?? "system";

interface Store {
  docs: Record<number, DocState>;
  order: number[];
  active: number | null;
  sidebar: SidebarPanel;
  theme: Theme;
  paletteOpen: boolean;
  searchOpen: boolean;
  password: { doc: number; path: string; error?: string } | null;
  error: string | null;

  /** Opens (or switches to) a file: `token` from a dialog or drop, else Rust must know `path` (recent list). */
  openPath: (path: string, token?: string, scratch?: boolean) => Promise<void>;
  submitPassword: (password: string) => Promise<void>;
  cancelPassword: () => void;
  closeDoc: (id: number) => void;
  /** The file behind tab `id` was rewritten and its engine document closed: opens it again in the same tab slot, keeping the view. */
  reloadInPlace: (id: number, path: string) => Promise<void>;
  activate: (id: number) => void;
  cycleTab: (dir: 1 | -1) => void;
  patch: (id: number, p: Partial<DocState>) => void;
  setSizes: (id: number, start: number, sizes: Size[]) => void;
  setZoom: (id: number, zoom: number) => void;
  setZoomMode: (id: number, m: ZoomMode) => void;
  rotate: (id: number, dir: 1 | -1) => void;
  goTo: (id: number, page: number, y?: number) => void;
  setSidebar: (p: SidebarPanel) => void;
  toggleSidebar: (p: Exclude<SidebarPanel, "none">) => void;
  setTheme: (t: Theme) => void;
  setUi: (p: Partial<Pick<Store, "paletteOpen" | "searchOpen" | "error">>) => void;
  runSearch: (id: number, query: string) => void;
  stepHit: (id: number, dir: 1 | -1) => void;
  loadOutline: (id: number) => void;
}

let navNonce = 0;
const latestSearch = new Map<number, number>();
let searchSeq = 0;
/** Paths being opened right now (lower-cased): a second request joins the first instead of opening twice. */
const opening = new Map<string, Promise<void>>();

export const useStore = create<Store>((set, get) => {
  const addDoc = (info: api.OpenInfo, locked = false, scratch: string | null = null) => {
    const doc: DocState = {
      id: info.doc,
      path: info.path,
      name: baseName(info.path),
      title: info.title,
      pageCount: info.pageCount,
      sizes: Array.from({ length: info.pageCount }, () => info.firstPage),
      rot: 0,
      zoomMode: "fitWidth",
      zoom: 1,
      currentPage: 0,
      rev: 0,
      dirty: false,
      scratch,
      undoLabel: null,
      redoLabel: null,
      organize: false,
      scroll: { page: 0, yPt: 0 },
      outline: null,
      search: emptySearch(),
      nav: null,
    };
    // A created file is not a recent until it is saved somewhere.
    if (!scratch) void useRecents.getState().record(info, locked);
    set((s) => ({
      docs: { ...s.docs, [doc.id]: doc },
      order: s.order.includes(doc.id) ? s.order : [...s.order, doc.id],
      active: doc.id,
      password: null,
    }));
    // Page sizes stream in after the first page is already visible. An edit reports all sizes
    // itself, so chunks of the file as opened are dropped once one lands.
    void api.pageSizes(doc.id, (start, sizes) => get().docs[doc.id]?.rev === 0 && get().setSizes(doc.id, start, sizes));
  };

  return {
    docs: {},
    order: [],
    active: null,
    sidebar: "thumbs",
    theme: loadTheme(),
    paletteOpen: false,
    searchOpen: false,
    password: null,
    error: null,

    openPath: (path, token, scratch = false) => {
      const k = keyOf(path);
      const existing = Object.values(get().docs).find((d) => keyOf(d.path) === k);
      if (existing) {
        set({ active: existing.id });
        return Promise.resolve();
      }
      const running = opening.get(k);
      if (running) return running;
      const job = (async () => {
        try {
          const file = token ?? (await api.knownFile(path)).token;
          const r = scratch ? await api.openDocument(file, true) : await api.openDocument(file);
          if (r.status === "opened") {
            // Rust canonicalised the path: a different spelling of an open file is the same tab.
            const dup = Object.values(get().docs).find((d) => keyOf(d.path) === keyOf(r.path));
            if (dup) {
              void api.closeDocument(r.doc);
              set({ active: dup.id });
            } else addDoc(r, false, scratch ? file : null);
          } else {
            // only one password prompt at a time: release the one being replaced
            const old = get().password;
            if (old && old.doc !== r.doc) void api.closeDocument(old.doc);
            set({ password: { doc: r.doc, path: r.path } });
          }
        } catch (e) {
          set({ error: `Could not open ${baseName(path)}: ${String(e)}` });
        } finally {
          opening.delete(k);
        }
      })();
      opening.set(k, job);
      return job;
    },

    submitPassword: async (password) => {
      const p = get().password;
      if (!p) return;
      try {
        addDoc(await api.authenticate(p.doc, password), true);
      } catch (e) {
        set({ password: { ...p, error: String(e) === "bad-password" ? "Wrong password, try again." : String(e) } });
      }
    },

    cancelPassword: () => {
      const p = get().password;
      if (p) void api.closeDocument(p.doc);
      set({ password: null });
    },

    closeDoc: (id) => {
      const scratch = get().docs[id]?.scratch;
      void api.cancelSearch(id);
      // the engine releases the file first; the scratch file goes right after
      void Promise.resolve(api.closeDocument(id)).finally(() => scratch && void api.discardScratch(scratch).catch(() => {}));
      dropCached(`${id}:`);
      set((s) => {
        const order = s.order.filter((d) => d !== id);
        const docs = { ...s.docs };
        delete docs[id];
        let active = s.active;
        if (active === id) {
          const i = s.order.indexOf(id);
          active = order[Math.min(i, order.length - 1)] ?? null;
        }
        return { docs, order, active };
      });
    },

    reloadInPlace: async (id, path) => {
      const old = get().docs[id];
      if (!old) return get().openPath(path);
      void api.cancelSearch(id);
      dropCached(`${id}:`);
      try {
        const r = await api.openDocument(path);
        if (r.status !== "opened") {
          if (r.status === "needsPassword") void api.closeDocument(r.doc);
          throw new Error("the restored file needs a password");
        }
        const wasActive = get().active === id;
        const at = get().order.indexOf(id);
        addDoc(r);
        set((s) => {
          const docs = { ...s.docs };
          delete docs[id];
          const order = s.order.filter((d) => d !== id && d !== r.doc);
          order.splice(Math.min(at, order.length), 0, r.doc);
          const keep = { rot: old.rot, zoomMode: old.zoomMode, zoom: old.zoom, scroll: old.scroll, organize: false };
          docs[r.doc] = { ...docs[r.doc], ...keep };
          return { docs, order, active: wasActive ? r.doc : s.active === r.doc ? (s.order.find((d) => d !== r.doc && d !== id) ?? r.doc) : s.active };
        });
        get().goTo(r.doc, Math.min(old.currentPage, r.pageCount - 1));
      } catch (e) {
        get().closeDoc(id);
        set({ error: `Could not reopen ${old.name}: ${String(e)}` });
      }
    },

    activate: (id) => set({ active: id }),

    cycleTab: (dir) => {
      const { order, active } = get();
      if (order.length < 2 || active == null) return;
      set({ active: order[(order.indexOf(active) + dir + order.length) % order.length] });
    },

    patch: (id, p) =>
      set((s) => (s.docs[id] ? { docs: { ...s.docs, [id]: { ...s.docs[id], ...p } } } : s)),

    setSizes: (id, start, sizes) =>
      set((s) => {
        const d = s.docs[id];
        if (!d) return s;
        const next = d.sizes.slice();
        sizes.forEach((v, i) => {
          if (start + i < next.length) next[start + i] = v;
        });
        return { docs: { ...s.docs, [id]: { ...d, sizes: next } } };
      }),

    setZoom: (id, zoom) => get().patch(id, { zoom: clamp(zoom, ZOOM_MIN, ZOOM_MAX), zoomMode: "custom" }),
    setZoomMode: (id, zoomMode) => get().patch(id, { zoomMode }),
    rotate: (id, dir) => {
      const d = get().docs[id];
      if (d) get().patch(id, { rot: ((d.rot + dir + 4) % 4) as Rot });
    },

    goTo: (id, page, y) => {
      const d = get().docs[id];
      if (!d) return;
      const p = clamp(Math.round(page), 0, d.pageCount - 1);
      get().patch(id, { nav: { page: p, y, nonce: ++navNonce }, currentPage: p });
    },

    setSidebar: (sidebar) => set({ sidebar }),
    toggleSidebar: (p) => set((s) => ({ sidebar: s.sidebar === p ? "none" : p })),
    setTheme: (theme) => {
      localStorage.setItem("inkwell.theme", theme);
      set({ theme });
    },
    setUi: (p) => set(p),

    runSearch: (id, query) => {
      const d = get().docs[id];
      if (!d) return;
      const q = query.trim();
      if (!q) {
        latestSearch.delete(id);
        void api.cancelSearch(id);
        return get().patch(id, { search: emptySearch() });
      }
      const seq = ++searchSeq;
      latestSearch.set(id, seq);
      const mine = { ...emptySearch(), query, done: false };
      get().patch(id, { search: mine });

      let buffer: Hit[] = [];
      let queued = false;
      let searchId: number | null = null;
      const flush = () => {
        queued = false;
        const cur = get().docs[id];
        if (!cur || cur.search.id !== searchId) return;
        const hits = cur.search.hits.concat(buffer);
        buffer = [];
        hits.sort((a, b) => a.page - b.page || a.rect[1] - b.rect[1] || a.rect[0] - b.rect[0]);
        let current = cur.search.current;
        if (current === -1 && hits.length > 0) {
          current = Math.max(0, hits.findIndex((h) => h.page >= cur.currentPage));
          get().goTo(id, hits[current].page, Math.max(0, hits[current].rect[1] - 80));
        }
        get().patch(id, { search: { ...cur.search, hits, current } });
      };

      const pending: api.SearchEvent[] = [];
      const handle = (e: api.SearchEvent) => {
        if (e.search !== searchId) return;
        if (e.kind === "hits") {
          for (const r of e.rects) buffer.push({ page: e.page, rect: r });
          if (!queued) {
            queued = true;
            requestAnimationFrame(flush);
          }
        } else {
          requestAnimationFrame(() => {
            flush();
            const cur = get().docs[id];
            if (cur && cur.search.id === searchId) get().patch(id, { search: { ...cur.search, done: true } });
          });
        }
      };

      api
        .search(id, q, (e) => (searchId == null ? pending.push(e) : handle(e)))
        .then((sid) => {
          if (latestSearch.get(id) !== seq || !get().docs[id]) return;
          searchId = sid;
          get().patch(id, { search: { ...get().docs[id].search, id: sid } });
          pending.splice(0).forEach(handle);
        })
        .catch((e) => set({ error: String(e) }));
    },

    stepHit: (id, dir) => {
      const d = get().docs[id];
      if (!d || d.search.hits.length === 0) return;
      const n = d.search.hits.length;
      const current = (d.search.current + dir + n) % n;
      get().patch(id, { search: { ...d.search, current } });
      const h = d.search.hits[current];
      get().goTo(id, h.page, Math.max(0, h.rect[1] - 80));
    },

    loadOutline: (id) => {
      const d = get().docs[id];
      if (!d || d.outline) return;
      api
        .outline(id)
        .then((o) => get().patch(id, { outline: o }))
        .catch(() => get().patch(id, { outline: [] }));
    },
  };
});

export const useActiveDoc = () => useStore((s) => (s.active != null ? s.docs[s.active] : undefined));
