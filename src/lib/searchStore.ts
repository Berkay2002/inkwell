import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import { useFind } from "./findStore";
import { keyOf } from "./recent";
import { type FileGroup, FIRST_ITEMS, MORE_ITEMS, insertFile } from "./searchLogic";

/** The state of one search across files. */
export interface FolderRun {
  /** Engine search id once the command answered. */
  id: number | null;
  query: string;
  running: boolean;
  scanned: number;
  total: number;
  files: FileGroup[];
  /** Files with a hit, hits in all (a file may list fewer than it counted), files that could not be read. */
  matched: number;
  hits: number;
  skipped: number;
  /** PDFs left unread because they are only in the cloud. */
  cloudOnly: number;
  /** The folder held more PDFs than a search looks at. */
  capped: boolean;
  cancelled: boolean;
}

interface Rail {
  /** The Search rail is showing. */
  open: boolean;
  setOpen: (open: boolean) => void;
  /** What the rail lists: this file's hits or those across a folder. */
  scope: "file" | "folder";
  setScope: (s: "file" | "folder") => void;
  /** What search across files reads: a folder (a token for commands, the path for display) or PDFs picked one by one. */
  target: Target | null;
  recursive: boolean;
  setRecursive: (on: boolean) => void;
  run: FolderRun | null;
  /** Rows shown per file (by listing index); more are added with Show more. */
  shown: Record<number, number>;
  showMore: (index: number) => void;
  /** Asks for a folder and keeps it for the next search. */
  pickFolder: () => Promise<boolean>;
  /** Asks for PDFs (several) and keeps them for the next search. */
  pickFiles: () => Promise<boolean>;
  /** Runs `query` over the chosen folder or files. */
  startFolder: (query: string) => Promise<void>;
  cancelFolder: () => void;
  /** Opens file `g` (or switches to its tab) at its hit number `item` and highlights the query there. */
  openAt: (g: FileGroup, item: number) => Promise<void>;
}

export type Target = { kind: "folder"; token: string; path: string } | { kind: "files"; tokens: string[]; paths: string[] };

const emptyRun = (query: string): FolderRun => ({
  id: null, query, running: true, scanned: 0, total: 0, files: [], matched: 0, hits: 0, skipped: 0, cloudOnly: 0, capped: false, cancelled: false,
});

let runSeq = 0;

export const useSearch = create<Rail>((set, get) => ({
  open: false,
  setOpen: (open) => set({ open }),
  scope: "file",
  setScope: (scope) => set({ scope }),
  target: null,
  recursive: true,
  setRecursive: (recursive) => set({ recursive }),
  run: null,
  shown: {},
  showMore: (index) => set((s) => ({ shown: { ...s.shown, [index]: (s.shown[index] ?? FIRST_ITEMS) + MORE_ITEMS } })),

  pickFolder: async () => {
    try {
      const f = await api.pickSearchFolder();
      if (!f) return false;
      set({ target: { kind: "folder", token: f.token, path: f.path } });
      return true;
    } catch (e) {
      useStore.getState().setUi({ error: String(e) });
      return false;
    }
  },

  pickFiles: async () => {
    try {
      const picked = await api.pickOpen(true, "search");
      if (picked.length === 0) return false;
      set({ target: { kind: "files", tokens: picked.map((f) => f.token), paths: picked.map((f) => f.path) } });
      return true;
    } catch (e) {
      useStore.getState().setUi({ error: String(e) });
      return false;
    }
  },

  startFolder: async (query) => {
    const q = query.trim();
    const target = get().target;
    if (!q || !target) return;
    const prev = get().run;
    if (prev?.running && prev.id != null) void api.cancelSearchFiles(prev.id);
    const seq = ++runSeq;
    useFind.getState().remember(q);
    set({ run: emptyRun(q), shown: {} });

    // Files arrive from two workers at their own pace: one render per frame is enough.
    let files: FileGroup[] = [];
    let queued = false;
    const apply = (p: Partial<FolderRun>) => set((s) => (seq === runSeq && s.run ? { run: { ...s.run, ...p } } : s));
    const flush = () => {
      queued = false;
      apply({ files });
    };
    const later = () => {
      if (!queued) {
        queued = true;
        requestAnimationFrame(flush);
      }
    };
    let id: number | null = null;
    const handle = (e: api.FilesEvent) => {
      if (seq !== runSeq) {
        // A newer search took over: end this one as soon as its id is known.
        if (e.kind === "started") void api.cancelSearchFiles(e.search);
        return;
      }
      if (e.kind === "started") {
        // Known before the folder is listed, so Cancel reaches the walk too.
        id = e.search;
        apply({ id });
        return;
      }
      if (e.search !== id) return;
      if (e.kind === "file") {
        files = insertFile(files, {
          index: e.index,
          path: e.path,
          name: e.name,
          pageCount: e.pageCount,
          hits: e.hits,
          pages: e.pages,
          truncated: e.truncated,
          items: e.items,
        });
        later();
      } else if (e.kind === "progress") {
        apply({ scanned: e.scanned, total: e.total });
      } else {
        apply({ files, running: false, scanned: e.cancelled ? get().run?.scanned ?? 0 : e.files, total: e.files, matched: e.matched, hits: e.hits, skipped: e.skipped, cloudOnly: e.cloudOnly, capped: e.capped, cancelled: e.cancelled });
      }
    };

    try {
      const sid = await api.searchFiles(
        target.kind === "folder" ? { kind: "folder", token: target.token, recursive: get().recursive } : { kind: "files", tokens: target.tokens },
        q,
        useFind.getState().opts,
        handle,
      );
      if (seq !== runSeq) {
        void api.cancelSearchFiles(sid);
        return;
      }
      if (id == null) {
        id = sid;
        apply({ id: sid });
      }
    } catch (e) {
      if (seq === runSeq) {
        apply({ running: false });
        useStore.getState().setUi({ error: String(e) });
      }
    }
  },

  cancelFolder: () => {
    const r = get().run;
    if (!r?.running) return;
    // Its Done event says "cancelled"; until then the rail shows what it has.
    if (r.id != null) void api.cancelSearchFiles(r.id);
    else runSeq++;
    set({ run: { ...r, running: false, cancelled: true } });
  },

  openAt: async (g, item) => {
    const run = get().run;
    const hit = g.items[item];
    if (!run) return;
    const st = useStore.getState();
    // The open token is issued now, for this one result (not for every file in the list).
    let token: string | undefined;
    if (run.id != null) {
      try {
        token = (await api.searchFileToken(run.id, g.index)).token;
      } catch (e) {
        st.setUi({ error: String(e) });
        return;
      }
    }
    await st.openPath(g.path, token);
    const doc = Object.values(useStore.getState().docs).find((d) => keyOf(d.path) === keyOf(g.path));
    if (!doc) return;
    useFind.getState().setDraft(doc.id, run.query);
    useStore.getState().runSearch(doc.id, run.query, hit ? { page: hit.page, y: hit.rects[0]?.[1] ?? 0 } : undefined);
  },
}));
