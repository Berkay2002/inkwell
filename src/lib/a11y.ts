import { Channel, invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useStore } from "../store";
import { type EditState, pickSave, toolCancel } from "./api";
import { applyState } from "./edit";
import { type Finding, type Fix, type Preview, type Progress, type Rect4, type Report, safeFixes, stem, suggestLang } from "./a11yLogic";

/**
 * The accessibility checker for the UI (src-tauri/src/a11y/app.rs). The check and every fix run
 * on their own threads with progress and cancel; a fix is one undo step, and the report is read
 * again after any change of the document (a fix, an undo, an edit).
 */

export const a11yApi = {
  check: (job: number, doc: number, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<Report>("a11y_check", { job, doc, onProgress: ch });
  },
  fix: (job: number, doc: number, fix: Fix, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<EditState>("a11y_fix", { job, doc, fix, onProgress: ch });
  },
  preview: (doc: number) => invoke<Preview>("a11y_preview", { doc }),
  exportReport: (job: number, doc: number, format: "html" | "pdf", date: string, to: string, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<string>("a11y_export_report", { job, doc, format, date, to, onProgress: ch });
  },
};

export type Phase = "idle" | "checking" | "fixing" | "autotagging" | "exporting";

export interface Shown {
  page: number;
  rect: Rect4 | null;
  /** Restarts the highlight's flash. */
  nonce: number;
}

export interface DocA11y {
  report: Report | null;
  /** The document generation the report was read from. */
  checkedRev: number | null;
  phase: Phase;
  progress: Progress | null;
  job: number;
  /** What Autotag would do; read when the report says the document is untagged. */
  preview: Preview | null;
  /** The person dismissed the Autotag offer for now. */
  notNow: boolean;
  error: string | null;
  /** Where the last report export went. */
  exported: string | null;
  shown: Shown | null;
  /** Groups collapsed by name. */
  collapsed: Record<string, boolean>;
}

const fresh = (): DocA11y => ({
  report: null,
  checkedRev: null,
  phase: "idle",
  progress: null,
  job: 0,
  preview: null,
  notNow: false,
  error: null,
  exported: null,
  shown: null,
  collapsed: {},
});

interface A11yStore {
  open: boolean;
  setOpen: (open: boolean) => void;
  /** Language the Primary language fix and "Fix all" set. */
  lang: string;
  setLang: (lang: string) => void;
  docs: Record<number, DocA11y>;
  get: (doc: number) => DocA11y;
  check: (doc: number) => Promise<void>;
  /** One fix, one undo step. */
  fix: (doc: number, fix: Fix) => Promise<boolean>;
  fixAll: (doc: number) => Promise<void>;
  autotag: (doc: number) => Promise<void>;
  cancel: (doc: number) => void;
  exportReport: (doc: number, format: "html" | "pdf") => Promise<void>;
  /** Jumps to the finding's page and highlights it. */
  show: (doc: number, f: Finding) => void;
  clearShown: (doc: number) => void;
  toggleGroup: (doc: number, name: string) => void;
  dismissAutotag: (doc: number) => void;
  forget: (doc: number) => void;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const CANCELLED = "cancelled";

let jobSeq = 1_900_000_000;
let nonce = 0;
const st = () => useStore.getState();

export const useA11y = create<A11yStore>((set, get) => {
  const patch = (doc: number, p: Partial<DocA11y>) =>
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? fresh()), ...p } } }));
  /** The check each document is waiting for; an older answer is dropped. */
  const latest = new Map<number, number>();

  async function loadPreview(doc: number, r: Report) {
    if (r.tagged) return patch(doc, { preview: null });
    try {
      const p = await a11yApi.preview(doc);
      if (get().docs[doc]) patch(doc, { preview: p });
    } catch {
      patch(doc, { preview: null });
    }
  }

  return {
    open: false,
    setOpen: (open) => {
      set({ open });
      if (!open) {
        for (const [id, d] of Object.entries(get().docs)) if (d.phase === "checking") get().cancel(Number(id));
      }
    },
    lang: suggestLang(typeof navigator === "undefined" ? undefined : navigator.language),
    setLang: (lang) => set({ lang }),
    docs: {},
    get: (doc) => get().docs[doc] ?? fresh(),

    check: async (doc) => {
      const d = st().docs[doc];
      if (!d) return;
      const busy = get().get(doc);
      // Fixes and exports hold the document; the report is read again when they are done.
      if (busy.phase === "fixing" || busy.phase === "autotagging" || busy.phase === "exporting") return;
      if (busy.phase === "checking") void toolCancel(busy.job).catch(() => {});
      const job = ++jobSeq;
      latest.set(doc, job);
      const rev = d.rev;
      patch(doc, { phase: "checking", progress: null, job, error: null });
      try {
        const report = await a11yApi.check(job, doc, (p) => latest.get(doc) === job && patch(doc, { progress: p }));
        if (latest.get(doc) !== job) return;
        patch(doc, { report, checkedRev: rev, phase: "idle", progress: null, shown: null });
        await loadPreview(doc, report);
      } catch (e) {
        if (latest.get(doc) !== job) return;
        patch(doc, { phase: "idle", progress: null, ...(msg(e) === CANCELLED ? {} : { error: msg(e) }) });
      }
    },

    fix: async (doc, fix) => {
      const cur = get().get(doc);
      if (cur.phase === "fixing" || cur.phase === "autotagging" || cur.phase === "exporting") return false;
      // A check that is still running reads the document the fix is about to change.
      if (cur.phase === "checking") {
        latest.delete(doc);
        void toolCancel(cur.job).catch(() => {});
      }
      const job = ++jobSeq;
      const auto = fix.fix === "autotag";
      patch(doc, { phase: auto ? "autotagging" : "fixing", progress: null, job, error: null });
      try {
        const state = await a11yApi.fix(job, doc, fix, (p) => patch(doc, { progress: p }));
        applyState(doc, state);
        patch(doc, { phase: "idle", progress: null });
        return true;
      } catch (e) {
        patch(doc, { phase: "idle", progress: null, ...(msg(e) === CANCELLED ? {} : { error: msg(e) }) });
        // The report on screen is still right unless a check was stopped to make room for this fix.
        const now = get().get(doc);
        if (!now.report || now.checkedRev !== st().docs[doc]?.rev) void get().check(doc);
        return false;
      }
    },

    fixAll: async (doc) => {
      const r = get().get(doc).report;
      if (!r) return;
      const fixes = safeFixes(r, get().lang);
      if (!fixes.length) return;
      await get().fix(doc, fixes.length === 1 ? fixes[0] : { fix: "many", fixes });
    },

    autotag: async (doc) => {
      await get().fix(doc, { fix: "autotag" });
    },

    cancel: (doc) => {
      const d = get().get(doc);
      if (d.phase === "idle") return;
      void toolCancel(d.job).catch(() => {});
      // A cancelled check says so itself; the others come back with "cancelled".
      if (d.phase === "checking") {
        latest.delete(doc);
        patch(doc, { phase: "idle", progress: null });
      }
    },

    exportReport: async (doc, format) => {
      const d = st().docs[doc];
      if (!d) return;
      const cur = get().get(doc);
      if (cur.phase !== "idle") return;
      try {
        const to = await pickSave(`${stem(d.name)} Accessibility Report.${format}`, { doc }, format === "pdf" ? "pdf" : "html");
        if (!to) return;
        const job = ++jobSeq;
        patch(doc, { phase: "exporting", progress: null, job, error: null, exported: null });
        const date = new Date().toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" });
        const path = await a11yApi.exportReport(job, doc, format, date, to.token, (p) => patch(doc, { progress: p }));
        patch(doc, { phase: "idle", progress: null, exported: path });
      } catch (e) {
        patch(doc, { phase: "idle", progress: null, ...(msg(e) === CANCELLED ? {} : { error: msg(e) }) });
      }
    },

    show: (doc, f) => {
      if (f.page === null) return;
      // A little above the element, so it is not pressed against the top edge.
      st().goTo(doc, f.page, f.rect ? Math.max(0, f.rect[1] - 48) : undefined);
      patch(doc, { shown: { page: f.page, rect: f.rect, nonce: ++nonce } });
    },
    clearShown: (doc) => patch(doc, { shown: null }),
    toggleGroup: (doc, name) => {
      const c = get().get(doc).collapsed;
      patch(doc, { collapsed: { ...c, [name]: !c[name] } });
    },
    dismissAutotag: (doc) => patch(doc, { notNow: true }),
    forget: (doc) =>
      set((s) => {
        const { [doc]: _gone, ...rest } = s.docs;
        void _gone;
        latest.delete(doc);
        return { docs: rest };
      }),
  };
});

/** The Tools menu and palette entry: the rail opens and reads the active document. */
export function openA11y() {
  if (st().active != null) useA11y.getState().setOpen(true);
}

// A closed document takes its report with it.
useStore.subscribe((s, prev) => {
  if (s.docs === prev.docs) return;
  for (const id of Object.keys(useA11y.getState().docs)) if (!s.docs[Number(id)]) useA11y.getState().forget(Number(id));
});
