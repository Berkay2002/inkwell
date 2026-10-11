import { listen } from "@tauri-apps/api/event";
import { message } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { useStore } from "../store";
import { useAi } from "./aiStore";
import { useRedact } from "./redact";
import * as api from "./history";
import type { Comparison, HistoryState, Source, VersionInfo } from "./history";
import { type Category, type Filter, parentOf, versionTitle, when } from "./historyLogic";

export interface DocHistory {
  state: HistoryState | null;
  versions: VersionInfo[];
}

/** Compare mode: two sources side by side. `doc` is null for two arbitrary files. */
export interface CompareReq {
  doc: number | null;
  a: Source;
  b: Source;
  names?: [string, string];
  /** "setup" shows the choose-what-to-compare page first; the Compare button clears it. */
  stage?: "setup";
  /** The categories the result starts with (the setup page's content switches); unset = all. */
  show?: Category[];
}

type Changes = Comparison | "loading" | { error: string };

interface HStore {
  open: boolean;
  filter: Filter;
  onlyNamed: boolean;
  docs: Record<number, DocHistory>;
  /** The version shown read-only in the main view, per document. */
  selected: Record<number, string>;
  /** Changes of a version against its parent, keyed "doc:version". */
  changes: Record<string, Changes>;
  /** The change the stepper is on (index into the comparison's changes), per document. */
  current: Record<number, number | null>;
  renaming: string | null;
  dismissed: Record<number, boolean>;
  compare: CompareReq | null;

  setOpen: (open: boolean) => void;
  toggle: () => void;
  setFilter: (f: Filter) => void;
  setOnlyNamed: (b: boolean) => void;
  refresh: (doc: number, tries?: number) => Promise<void>;
  select: (doc: number, id: string | null) => void;
  setCurrent: (doc: number, i: number | null) => void;
  setRenaming: (id: string | null) => void;
  rename: (doc: number, id: string, name: string | null) => Promise<void>;
  restore: (doc: number, id: string) => Promise<void>;
  recover: (doc: number) => Promise<void>;
  saveCopy: (doc: number, id: string) => Promise<void>;
  dismissRecovery: (doc: number) => void;
  confirmSame: (doc: number, candidate: string | null) => Promise<void>;
  deleteHistory: (doc: number) => Promise<void>;
  neverKeep: (doc: number, never: boolean) => Promise<void>;
  openCompare: (c: CompareReq) => void;
  setCompare: (p: Partial<CompareReq>) => void;
  closeCompare: () => void;
}

const toast = (error: string) => useStore.getState().setUi({ error });
const fail = (what: string) => (e: unknown) => toast(`${what}: ${String(e)}`);
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

export const useHistory = create<HStore>((set, get) => {
  const patchDoc = (doc: number, f: (d: DocHistory) => DocHistory) =>
    set((s) => ({ docs: { ...s.docs, [doc]: f(s.docs[doc] ?? { state: null, versions: [] }) } }));

  /** The engine closed the document to rewrite its file: opens the file again in the same tab. */
  const reopen = async (doc: number, r: api.RestoreOut, message: string) => {
    await useStore.getState().reloadInPlace(doc, r.path);
    const now = useStore.getState().active;
    if (get().open && now != null) void get().refresh(now);
    toast(message);
  };

  return {
    open: false,
    filter: "all",
    onlyNamed: false,
    docs: {},
    selected: {},
    changes: {},
    current: {},
    renaming: null,
    dismissed: {},
    compare: null,

    setOpen: (open) => {
      set({ open });
      if (open) {
        // one docked panel at a time
        const ai = useAi.getState();
        if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
        useRedact.getState().setOpen(false);
        const a = useStore.getState().active;
        if (a != null) void get().refresh(a);
      } else set({ selected: {}, renaming: null });
    },
    toggle: () => get().setOpen(!get().open),
    setFilter: (filter) => set({ filter }),
    setOnlyNamed: (onlyNamed) => set({ onlyNamed }),

    refresh: async (doc, tries = 0) => {
      if (!useStore.getState().docs[doc]) return;
      try {
        const state = await api.historyState(doc);
        const versions = state.status === "ready" ? await api.historyVersions(doc) : (get().docs[doc]?.versions ?? []);
        if (!useStore.getState().docs[doc]) return;
        patchDoc(doc, () => ({ state, versions }));
        if (state.status === "loading" && tries < 40) setTimeout(() => void get().refresh(doc, tries + 1), 300);
      } catch (e) {
        patchDoc(doc, (d) => ({ ...d, state: { status: "error", message: String(e), recovery: null, candidates: [] } }));
      }
    },

    select: (doc, id) => {
      set((s) => {
        const selected = { ...s.selected };
        if (id) selected[doc] = id;
        else delete selected[doc];
        return { selected, current: { ...s.current, [doc]: null }, renaming: null };
      });
      if (!id) return;
      const key = `${doc}:${id}`;
      if (get().changes[key]) return;
      const versions = get().docs[doc]?.versions ?? [];
      const v = versions.find((x) => x.id === id);
      const parent = v && parentOf(versions, v);
      if (!parent) {
        set((s) => ({ changes: { ...s.changes, [key]: { pages: { aPages: 0, bPages: 0, bToA: [], removedA: [] }, changes: [], identical: true } } }));
        return;
      }
      set((s) => ({ changes: { ...s.changes, [key]: "loading" } }));
      api.historyCompare(doc, { kind: "version", id: parent.id }, { kind: "version", id }).then(
        (c) => set((s) => ({ changes: { ...s.changes, [key]: c } })),
        (e) => set((s) => ({ changes: { ...s.changes, [key]: { error: String(e) } } })),
      );
    },

    setCurrent: (doc, i) => set((s) => ({ current: { ...s.current, [doc]: i } })),
    setRenaming: (renaming) => set({ renaming }),

    rename: async (doc, id, name) => {
      set({ renaming: null });
      try {
        const v = await api.historyName(doc, id, name);
        patchDoc(doc, (d) => ({ ...d, versions: d.versions.map((x) => (x.id === id ? v : x)) }));
      } catch (e) {
        fail("Could not name the version")(e);
      }
    },

    restore: async (doc, id) => {
      const d = useStore.getState().docs[doc];
      const v = get().docs[doc]?.versions.find((x) => x.id === id);
      if (d?.dirty) {
        // The unsaved edits are kept as an autosave version first, so they stay recoverable.
        const ok = await message(`${d.name} has unsaved changes. Restoring replaces them (they stay in the version history as an autosave).`, {
          title: "Inkwell",
          kind: "warning",
          buttons: { ok: "Restore", cancel: "Cancel" },
        });
        if (ok !== "Ok" && ok !== "Restore") return;
      }
      try {
        const r = await api.historyRestore(doc, id);
        await reopen(doc, r, `Restored ${v ? `"${versionTitle(v)}" from ${when(v.created, Date.now())}` : "the version"}${d ? ` in ${d.name}` : ""}`);
      } catch (e) {
        fail("Could not restore")(e);
        void get().refresh(doc);
      }
    },

    recover: async (doc) => {
      try {
        await reopen(doc, await api.historyRecover(doc), "Recovered your unsaved changes");
      } catch (e) {
        fail("Could not recover")(e);
      }
    },

    saveCopy: async (doc, id) => {
      const d = useStore.getState().docs[doc];
      const v = get().docs[doc]?.versions.find((x) => x.id === id);
      const stem = (d?.name ?? "document").replace(/\.pdf$/i, "");
      try {
        const dest = await api.historySaveCopy(doc, id, `${stem} (${v ? when(v.created, Date.now()).replace(/[,:]/g, "").replace(/\s+/g, " ") : "copy"}).pdf`);
        if (dest) toast(`Saved a copy as ${baseName(dest)}`);
      } catch (e) {
        fail("Could not save a copy")(e);
      }
    },

    dismissRecovery: (doc) => set((s) => ({ dismissed: { ...s.dismissed, [doc]: true } })),

    confirmSame: async (doc, candidate) => {
      try {
        await api.historyConfirmSame(doc, candidate);
      } catch (e) {
        fail("History")(e);
      }
      await get().refresh(doc);
    },

    deleteHistory: async (doc) => {
      try {
        await api.historyDelete(doc);
        get().select(doc, null);
      } catch (e) {
        fail("Could not delete history")(e);
      }
      patchDoc(doc, () => ({ state: null, versions: [] }));
      await get().refresh(doc);
    },

    neverKeep: async (doc, never) => {
      try {
        await api.historyNeverKeep(doc, never);
      } catch (e) {
        fail("History")(e);
      }
      get().select(doc, null);
      patchDoc(doc, () => ({ state: null, versions: [] }));
      await get().refresh(doc);
    },

    openCompare: (compare) => {
      set({ compare });
    },
    setCompare: (p) => set((s) => (s.compare ? { compare: { ...s.compare, ...p } } : s)),
    closeCompare: () => set({ compare: null }),
  };
});

// One docked panel at a time (the assistant's floating and bar layouts do not dock).
useAi.subscribe((s, prev) => {
  if (s.open && !prev.open && s.settings.mode === "docked") useHistory.setState({ open: false, selected: {}, renaming: null });
});

// A closed tab forgets its history view.
useStore.subscribe((s, prev) => {
  for (const k of Object.keys(prev.docs)) {
    const id = Number(k);
    if (s.docs[id]) continue;
    useHistory.setState((h) => {
      const docs = { ...h.docs };
      const selected = { ...h.selected };
      const current = { ...h.current };
      const dismissed = { ...h.dismissed };
      delete docs[id];
      delete selected[id];
      delete current[id];
      delete dismissed[id];
      const changes = Object.fromEntries(Object.entries(h.changes).filter(([key]) => !key.startsWith(`${id}:`)));
      return { docs, selected, current, dismissed, changes, compare: h.compare?.doc === id ? null : h.compare };
    });
  }
});

let started = false;
/** Backend events and window focus. Called once from the app shell. */
export function startHistoryEvents() {
  if (started) return;
  started = true;
  void listen<number>("history-changed", (e) => void useHistory.getState().refresh(e.payload));
  // A change made outside Inkwell is recorded when the window gets focus back.
  window.addEventListener("focus", () => {
    const doc = useStore.getState().active;
    if (doc == null || useHistory.getState().docs[doc]?.state?.status !== "ready") return;
    api.historyCheck(doc).then(
      (v) => v && void useHistory.getState().refresh(doc),
      () => {},
    );
  });
}

// The Redact rail docks too.
useAi.subscribe((s, prev) => {
  if (s.open && !prev.open && s.settings.mode === "docked") useRedact.getState().setOpen(false);
});
useRedact.subscribe((s, prev) => {
  if (!s.open || prev.open) return;
  useHistory.getState().setOpen(false);
  const ai = useAi.getState();
  if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
});
