import { create } from "zustand";
import { useStore } from "../store";
import { replaceText } from "./edit";
import { useFind } from "./findStore";
import { type ReplaceReport, reportOf } from "./replaceLogic";
import { useTextEdit } from "./textEdit";

/**
 * Find & Replace in the Find bar while Edit text is on. The matches are Find's (its query and options);
 * the engine edits the text blocks that hold them (src-tauri/src/textedit/replace.rs), one undo step each run.
 */

interface ReplaceStore {
  /** What to put in place of the find text; kept while the app runs. */
  text: string;
  setText: (t: string) => void;
  busy: boolean;
  /** What the last run did, per document (kept until the next run). */
  report: Record<number, ReplaceReport | null>;
  /** Replaces the current match and moves on to the next. */
  one: (doc: number) => Promise<void>;
  /** Replaces every match in the document. */
  all: (doc: number) => Promise<void>;
}

/** Why the document's text can't be changed (certified), from any page Edit text has listed. */
export function lockedReason(doc: number): string | null {
  const pages = useTextEdit.getState().docs[doc]?.pages ?? {};
  for (const p of Object.values(pages)) if (p.locked) return p.locked;
  return null;
}

export const useReplace = create<ReplaceStore>((set, get) => {
  const say = (doc: number, report: ReplaceReport) => set((s) => ({ report: { ...s.report, [doc]: report } }));

  /** Sends what was typed in a block first (one undo step of its own), then lets go of the selection:
   * its index may name another block once the text is replaced. */
  const settle = async (doc: number) => {
    await useTextEdit.getState().commit(doc);
    useTextEdit.getState().discard(doc);
  };

  const run = async (doc: number, one: boolean) => {
    const d = useStore.getState().docs[doc];
    if (!d || get().busy || !d.search.query.trim()) return;
    const hit = one ? d.search.hits[d.search.current] : undefined;
    if (one && (!hit || hit.source !== "text" || !hit.rects[0])) return;
    set({ busy: true });
    try {
      await settle(doc);
      const cur = useStore.getState().docs[doc];
      if (!cur) return;
      const find = d.search.query;
      const at = hit ? { page: hit.page, rect: hit.rects[0] } : undefined;
      const r = await replaceText(doc, cur.rev, { find, with: get().text, opts: useFind.getState().opts, at });
      if (r.kind === "stale") {
        say(doc, { summary: "The document changed. Searching again.", lines: [], more: 0, notes: [] });
        useStore.getState().runSearch(doc, find);
      } else if (r.kind === "ok") {
        say(doc, reportOf(r.value));
        // Replace one goes on to the next match, past any the replacement itself holds.
        if (hit && r.value.replaced > 0) {
          const [x, y] = hit.rects[0];
          useStore.getState().runSearch(doc, find, { page: hit.page, x, y, skip: r.value.inserted });
        }
      }
      // An error is already on the shared error line.
    } finally {
      set({ busy: false });
    }
  };

  return {
    text: "",
    setText: (text) => set({ text }),
    busy: false,
    report: {},
    one: (doc) => run(doc, true),
    all: (doc) => run(doc, false),
  };
});
