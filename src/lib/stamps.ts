import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import { useAnnot } from "./annot";
import { type Gate, type Library, type Stamp, categoryProblem, customId, isPdfPath, nameFromPath, nameProblem, DEFAULT_CATEGORY } from "./stampsLogic";

/**
 * Custom stamps for the UI (src-tauri/src/annot/stamps.rs): the library in the app data folder, and the manager
 * that makes, renames, re-files and deletes them. The library never touches the open document; placing one is an
 * ordinary comment edit (`useAnnot.placeStamp`).
 */

export const stampApi = {
  list: () => invoke<Library>("stamp_list"),
  /** `file`: an open token from `pickSource("stamp", false)`; `page` is the 0-based page of a PDF. */
  create: (file: string, name: string, category: string, page: number | null) => invoke<Stamp>("stamp_create", { file, name, category, page }),
  update: (id: string, name: string | null, category: string | null) => invoke<Stamp>("stamp_update", { id, name, category }),
  remove: (id: string) => invoke<void>("stamp_delete", { id }),
  thumb: (id: string) => invoke<string>("stamp_thumb", { id }),
  gate: (doc: number) => invoke<Gate>("stamp_gate", { doc }),
};

/** A picture or PDF picked and not made into a stamp yet: the form the manager shows. */
export interface Draft {
  token: string;
  path: string;
  name: string;
  category: string;
  /** 1-based page typed for a PDF. */
  page: string;
  pdf: boolean;
}

interface StampStore {
  stamps: Stamp[];
  loaded: boolean;
  /** An engine call is running. */
  busy: boolean;
  /** A rejected action's message, shown in the manager. */
  error: string | null;
  /** `data:` URLs of the thumbnails read so far. */
  thumbs: Record<string, string>;
  /** Whether a stamp can be placed, per document, as last read. */
  gates: Record<number, Gate>;
  managerOpen: boolean;
  draft: Draft | null;

  load: () => Promise<void>;
  thumb: (id: string) => Promise<void>;
  readGate: (doc: number) => Promise<void>;
  openManager: () => void;
  closeManager: () => void;
  /** Opens the picture/PDF dialog; a pick becomes the draft. */
  pick: () => Promise<void>;
  setDraft: (p: Partial<Draft>) => void;
  cancelDraft: () => void;
  /** Makes the stamp from the draft. */
  make: () => Promise<boolean>;
  rename: (id: string, name: string) => Promise<void>;
  refile: (id: string, category: string) => Promise<void>;
  remove: (id: string) => Promise<void>;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const warn = (error: string) => useStore.getState().setUi({ error });

export const useStamps = create<StampStore>((set, get) => {
  /** Runs one library call; its failure is the manager's error line. */
  const run = async <T>(f: () => Promise<T>): Promise<T | null> => {
    set({ busy: true, error: null });
    try {
      return await f();
    } catch (e) {
      set({ error: msg(e) });
      return null;
    } finally {
      set({ busy: false });
    }
  };
  const replace = (s: Stamp) => set((st) => ({ stamps: st.stamps.map((x) => (x.id === s.id ? s : x)) }));
  const gateSeq = new Map<number, number>();

  return {
    stamps: [],
    loaded: false,
    busy: false,
    error: null,
    thumbs: {},
    gates: {},
    managerOpen: false,
    draft: null,

    load: async () => {
      try {
        const lib = await stampApi.list();
        set({ stamps: lib.stamps, loaded: true });
        if (lib.warning) warn(lib.warning);
      } catch (e) {
        set({ loaded: true, error: msg(e) });
      }
    },
    thumb: async (id) => {
      if (get().thumbs[id]) return;
      try {
        const url = await stampApi.thumb(id);
        set((s) => ({ thumbs: { ...s.thumbs, [id]: url } }));
      } catch {
        // left blank: the row still has its name
      }
    },
    readGate: async (doc) => {
      // Read again whenever the document moved (a save or an edit may have signed or certified it); the newest read wins.
      const n = (gateSeq.get(doc) ?? 0) + 1;
      gateSeq.set(doc, n);
      try {
        const g = await stampApi.gate(doc);
        if (gateSeq.get(doc) === n) set((s) => ({ gates: { ...s.gates, [doc]: g } }));
      } catch {
        // unreadable: the engine refuses a certified document's edit itself
      }
    },
    openManager: () => {
      set({ managerOpen: true, error: null });
      void get().load();
    },
    closeManager: () => set({ managerOpen: false, draft: null, error: null }),

    pick: async () => {
      const r = await run(() => api.pickSource("stamp", false));
      const f = r?.[0];
      if (!f) return;
      set({ draft: { token: f.token, path: f.path, name: nameFromPath(f.path), category: DEFAULT_CATEGORY, page: "1", pdf: isPdfPath(f.path) } });
    },
    setDraft: (p) => set((s) => (s.draft ? { draft: { ...s.draft, ...p } } : s)),
    cancelDraft: () => set({ draft: null, error: null }),
    make: async () => {
      const d = get().draft;
      if (!d) return false;
      const problem = nameProblem(d.name) ?? categoryProblem(d.category);
      if (problem) {
        set({ error: problem });
        return false;
      }
      const page = d.pdf ? Math.max(1, Math.floor(Number(d.page)) || 1) - 1 : null;
      const made = await run(() => stampApi.create(d.token, d.name, d.category, page));
      if (!made) return false;
      set((s) => ({ stamps: [...s.stamps, made], draft: null }));
      return true;
    },
    rename: async (id, name) => {
      const cur = get().stamps.find((s) => s.id === id);
      if (!cur || cur.name === name.trim()) return;
      const problem = nameProblem(name);
      if (problem) return set({ error: problem });
      const s = await run(() => stampApi.update(id, name, null));
      if (s) replace(s);
    },
    refile: async (id, category) => {
      const cur = get().stamps.find((s) => s.id === id);
      if (!cur || cur.category === (category.trim() || DEFAULT_CATEGORY)) return;
      const problem = categoryProblem(category);
      if (problem) return set({ error: problem });
      const s = await run(() => stampApi.update(id, null, category));
      if (s) replace(s);
    },
    remove: async (id) => {
      const ok = await run(() => stampApi.remove(id).then(() => true));
      if (!ok) return;
      set((s) => ({ stamps: s.stamps.filter((x) => x.id !== id) }));
      // The stamp the next click would place is gone: back to the first standard one.
      if (customId(useAnnot.getState().stamp) === id) useAnnot.getState().setStamp("Approved");
    },
  };
});
