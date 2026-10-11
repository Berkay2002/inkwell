import { create } from "zustand";
import { DEFAULT_OPTS, type FindOpts, HISTORY_KEY, OPTS_KEY, parseHistory, parseOpts, pushHistory } from "./searchLogic";

// What the find box and the Search rail share: the options, the last queries (local only), and the text in the field.
// No store imports: store.ts runs searches with these options.

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* kept for the session */
  }
}

interface FindState {
  opts: FindOpts;
  /** Newest first, at most ten; kept in this app's local storage and nowhere else. */
  history: string[];
  /** The text in the field, per document, before it has been searched. */
  drafts: Record<number, string>;
  setOpt: <K extends keyof FindOpts>(key: K, value: FindOpts[K]) => void;
  remember: (query: string) => void;
  forget: (query: string) => void;
  clearHistory: () => void;
  setDraft: (doc: number, text: string) => void;
}

export const useFind = create<FindState>((set, get) => ({
  opts: parseOpts(read(OPTS_KEY)),
  history: parseHistory(read(HISTORY_KEY)),
  drafts: {},
  setOpt: (key, value) => {
    const opts = { ...get().opts, [key]: value };
    write(OPTS_KEY, JSON.stringify(opts));
    set({ opts });
  },
  remember: (query) => {
    const history = pushHistory(get().history, query);
    if (history === get().history) return;
    write(HISTORY_KEY, JSON.stringify(history));
    set({ history });
  },
  forget: (query) => {
    const history = get().history.filter((q) => q !== query);
    write(HISTORY_KEY, JSON.stringify(history));
    set({ history });
  },
  clearHistory: () => {
    write(HISTORY_KEY, "[]");
    set({ history: [] });
  },
  setDraft: (doc, text) => set((s) => ({ drafts: { ...s.drafts, [doc]: text } })),
}));

export { DEFAULT_OPTS };
