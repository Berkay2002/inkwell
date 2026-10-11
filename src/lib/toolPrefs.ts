import { create } from "zustand";
import { MAX_PINNED, MAX_RECENT, cleanIds, pushRecent, togglePin } from "./toolsCatalog";

const KEY = "inkwell.toolPrefs";

interface ToolPrefs {
  /** Tool ids, most recently used first (at most `MAX_RECENT`). */
  recent: string[];
  /** Tool ids shown as buttons in the toolbar, in the order pinned (at most `MAX_PINNED`). */
  pinned: string[];
  /** The All tools panel in the left slot; per session, not stored. */
  panelOpen: boolean;
  used: (id: string) => void;
  /** Returns false when three tools are already pinned. */
  togglePin: (id: string) => boolean;
  setPanel: (open: boolean) => void;
}

function load(): { recent: string[]; pinned: string[] } {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, unknown>;
    return { recent: cleanIds(v.recent, MAX_RECENT), pinned: cleanIds(v.pinned, MAX_PINNED) };
  } catch {
    return { recent: [], pinned: [] };
  }
}

const save = (s: { recent: string[]; pinned: string[] }) => {
  try {
    localStorage.setItem(KEY, JSON.stringify({ recent: s.recent, pinned: s.pinned }));
  } catch {
    // storage may be unavailable; the lists then last for the session
  }
};

export const useToolPrefs = create<ToolPrefs>((set, get) => ({
  ...load(),
  panelOpen: false,
  used: (id) => {
    const recent = pushRecent(get().recent, id);
    set({ recent });
    save({ recent, pinned: get().pinned });
  },
  togglePin: (id) => {
    const r = togglePin(get().pinned, id);
    if (r.full) return false;
    set({ pinned: r.pinned });
    save({ recent: get().recent, pinned: r.pinned });
    return true;
  },
  setPanel: (panelOpen) => set({ panelOpen }),
}));
