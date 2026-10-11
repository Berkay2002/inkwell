import { Channel, invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useStore } from "../store";
import { type EditState, pickSave, toolCancel } from "./api";
import { applyState } from "./edit";
import { a11yApi } from "./a11y";
import { suggestLang } from "./a11yLogic";
import {
  type Claim,
  type Converted,
  type FixGroup,
  type Output,
  type Finding,
  type ProfileId,
  type Progress,
  type Rect4,
  type Report,
  type RuleResult,
  type Ticks,
  a11yFixOf,
  convertedName,
  groupsFor,
  ticked,
} from "./preflightLogic";

/**
 * Preflight for the UI (src-tauri/src/preflight/app.rs). The check, the conversion and the report
 * export run on their own threads with progress and cancel. A conversion in place is one undo
 * step; a conversion to a new file leaves the open document as it is.
 */

interface ConvertArg {
  groups: FixGroup[];
  lang: string | null;
}

export const preflightApi = {
  check: (job: number, doc: number, profile: ProfileId, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<Report>("preflight_check", { job, doc, profile, onProgress: ch });
  },
  claim: (doc: number) => invoke<Claim | null>("preflight_claim", { doc }),
  fix: (job: number, doc: number, profile: ProfileId, convert: ConvertArg, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<EditState>("preflight_fix", { job, doc, profile, convert, onProgress: ch });
  },
  convert: (job: number, doc: number, profile: ProfileId, convert: ConvertArg, to: string, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<Converted>("preflight_convert", { job, doc, profile, convert, to, onProgress: ch });
  },
  exportReport: (job: number, doc: number, profile: ProfileId, format: "html" | "pdf", date: string, to: string, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<string>("preflight_export_report", { job, doc, profile, format, date, to, onProgress: ch });
  },
};

export type Phase = "idle" | "checking" | "converting" | "exporting";

export interface Shown {
  page: number;
  rect: Rect4 | null;
  /** Restarts the highlight's flash. */
  nonce: number;
}

export interface DocPreflight {
  report: Report | null;
  /** The document generation and profile the report was read from. */
  checkedRev: number | null;
  checkedProfile: ProfileId | null;
  phase: Phase;
  progress: Progress | null;
  job: number;
  /** The revision whose check was cancelled: it is not started again by itself (Check again does). */
  skipRev: number | null;
  /** The revision a check failed on, so a failing check is not retried in a loop. */
  errorRev: number | null;
  error: string | null;
  /** The groups the person switched on or off; a group not in it keeps its default. */
  ticks: Ticks;
  /** Where "Convert" puts the result. */
  output: Output;
  /** The last conversion to a new file. */
  converted: { path: string; report: Report; fixed: string[] } | null;
  /** Where the last report export went. */
  exported: string | null;
  shown: Shown | null;
  collapsed: Record<string, boolean>;
  /** What the XMP says; read for the Document properties badge. */
  claim: Claim | null;
  claimRev: number | null;
}

const fresh = (): DocPreflight => ({
  report: null,
  checkedRev: null,
  checkedProfile: null,
  phase: "idle",
  progress: null,
  job: 0,
  skipRev: null,
  errorRev: null,
  error: null,
  ticks: {},
  output: "newFile",
  converted: null,
  exported: null,
  shown: null,
  collapsed: {},
  claim: null,
  claimRev: null,
});

interface PreflightStore {
  open: boolean;
  setOpen: (open: boolean) => void;
  profile: ProfileId;
  setProfile: (profile: ProfileId) => void;
  /** Language the accessibility group sets when the document has none. */
  lang: string;
  setLang: (lang: string) => void;
  docs: Record<number, DocPreflight>;
  get: (doc: number) => DocPreflight;
  check: (doc: number) => Promise<void>;
  setTick: (doc: number, group: FixGroup, on: boolean) => void;
  setOutput: (doc: number, output: Output) => void;
  /** Convert with the ticked groups: into a new file the person names, or in place as one undo step. */
  convert: (doc: number) => Promise<void>;
  /** One rule's fix: its group, in place (one undo step); PDF/UA rules run the accessibility checker's fix. */
  fixRule: (doc: number, x: RuleResult) => Promise<boolean>;
  cancel: (doc: number) => void;
  exportReport: (doc: number, format: "html" | "pdf") => Promise<void>;
  show: (doc: number, f: Finding) => void;
  clearShown: (doc: number) => void;
  toggleGroup: (doc: number, name: string) => void;
  /** Reads what the document's XMP claims. */
  refreshClaim: (doc: number) => Promise<void>;
  /** Opens the rail on the claimed profile and runs it. */
  verify: (doc: number) => void;
  dismissConverted: (doc: number) => void;
  forget: (doc: number) => void;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const CANCELLED = "cancelled";

let jobSeq = 1_950_000_000;
let nonce = 0;
const st = () => useStore.getState();

export const usePreflight = create<PreflightStore>((set, get) => {
  // A progress or completion that arrives after the tab closed must not bring its entry back.
  const patch = (doc: number, p: Partial<DocPreflight>) => {
    if (!st().docs[doc]) return;
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? fresh()), ...p } } }));
  };
  /** The check each document is waiting for; an older answer is dropped. */
  const latest = new Map<number, number>();
  const claimSeq = new Map<number, number>();

  const busy = (p: Phase) => p === "converting" || p === "exporting";
  const convertArg = (groups: FixGroup[]): ConvertArg => ({ groups, lang: get().lang });

  return {
    open: false,
    setOpen: (open) => {
      // Opening the rail is asking for a check again, so a cancel from before is forgotten.
      if (open) {
        set((s) => ({
          open,
          docs: Object.fromEntries(Object.entries(s.docs).map(([id, d]) => [id, d.skipRev === null ? d : { ...d, skipRev: null }])),
        }));
      } else {
        set({ open });
        for (const [id, d] of Object.entries(get().docs)) if (d.phase === "checking") get().cancel(Number(id));
      }
    },
    profile: "pdfa-2b",
    setProfile: (profile) => {
      if (get().profile === profile) return;
      set((s) => ({
        profile,
        docs: Object.fromEntries(Object.entries(s.docs).map(([id, d]) => [id, { ...d, ticks: {}, converted: null, shown: null, skipRev: null, errorRev: null }])),
      }));
    },
    lang: suggestLang(typeof navigator === "undefined" ? undefined : navigator.language),
    setLang: (lang) => set({ lang }),
    docs: {},
    get: (doc) => get().docs[doc] ?? fresh(),

    check: async (doc) => {
      const d = st().docs[doc];
      if (!d) return;
      const cur = get().get(doc);
      if (busy(cur.phase)) return;
      if (cur.phase === "checking") void toolCancel(cur.job).catch(() => {});
      const job = ++jobSeq;
      latest.set(doc, job);
      const rev = d.rev;
      const profile = get().profile;
      patch(doc, { phase: "checking", progress: null, job, error: null, skipRev: null, errorRev: null });
      const done = () => (get().get(doc).phase === "checking" ? ({ phase: "idle", progress: null } as const) : {});
      try {
        const report = await preflightApi.check(job, doc, profile, (p) => latest.get(doc) === job && patch(doc, { progress: p }));
        if (latest.get(doc) !== job) return;
        patch(doc, { report, checkedRev: rev, checkedProfile: profile, shown: null, ...done() });
      } catch (e) {
        if (latest.get(doc) !== job) return;
        patch(doc, { ...done(), ...(msg(e) === CANCELLED ? {} : { error: msg(e), errorRev: rev }) });
      }
    },

    setTick: (doc, group, on) => patch(doc, { ticks: { ...get().get(doc).ticks, [group]: on } }),
    setOutput: (doc, output) => patch(doc, { output }),

    convert: async (doc) => {
      const d = st().docs[doc];
      const cur = get().get(doc);
      const report = cur.report;
      if (!d || !report || busy(cur.phase)) return;
      const profile = report.profile;
      const groups = ticked(report.plan, cur.ticks);
      const arg = convertArg(groups);
      let to: string | null = null;
      if (cur.output === "newFile") {
        const stem = d.name.replace(/\.pdf$/i, "");
        const f = await pickSave(convertedName(stem, profile), { doc }, "pdf");
        if (!f) return;
        to = f.token;
      }
      // The dialog was open a while: the document may have moved on or a step may have started.
      const now = get().get(doc);
      if (!st().docs[doc]) return;
      if (busy(now.phase)) return patch(doc, { error: "Wait for the current step to finish, then convert again." });
      if (now.phase === "checking") {
        latest.delete(doc);
        void toolCancel(now.job).catch(() => {});
      }
      const job = ++jobSeq;
      patch(doc, { phase: "converting", progress: null, job, error: null, converted: null });
      try {
        if (to === null) {
          const state = await preflightApi.fix(job, doc, profile, arg, (p) => patch(doc, { progress: p }));
          applyState(doc, state);
          patch(doc, { phase: "idle", progress: null, ticks: {} });
        } else {
          const out = await preflightApi.convert(job, doc, profile, arg, to, (p) => patch(doc, { progress: p }));
          patch(doc, { phase: "idle", progress: null, converted: { path: out.path, report: out.report, fixed: out.fixed } });
        }
      } catch (e) {
        patch(doc, { phase: "idle", progress: null, ...(msg(e) === CANCELLED ? {} : { error: msg(e) }) });
        const after = get().get(doc);
        if (!after.report || after.checkedRev !== st().docs[doc]?.rev) void get().check(doc);
      }
    },

    fixRule: async (doc, x) => {
      const d = st().docs[doc];
      const cur = get().get(doc);
      const report = cur.report;
      if (!d || !report || busy(cur.phase) || x.status !== "failed") return false;
      if (cur.phase === "checking") {
        latest.delete(doc);
        void toolCancel(cur.job).catch(() => {});
      }
      const job = ++jobSeq;
      patch(doc, { phase: "converting", progress: null, job, error: null, converted: null });
      try {
        const a11y = x.a11yFix ? a11yFixOf(x.a11yFix, get().lang) : null;
        const state = a11y
          ? await a11yApi.fix(job, doc, a11y as never, (p) => patch(doc, { progress: p }))
          : await preflightApi.fix(job, doc, report.profile, convertArg(groupsFor(x, report.plan, get().get(doc).ticks)), (p) => patch(doc, { progress: p }));
        applyState(doc, state);
        patch(doc, { phase: "idle", progress: null });
        return true;
      } catch (e) {
        patch(doc, { phase: "idle", progress: null, ...(msg(e) === CANCELLED ? {} : { error: msg(e) }) });
        const after = get().get(doc);
        if (!after.report || after.checkedRev !== st().docs[doc]?.rev) void get().check(doc);
        return false;
      }
    },

    cancel: (doc) => {
      const d = get().get(doc);
      if (d.phase === "idle") return;
      void toolCancel(d.job).catch(() => {});
      // A cancelled check says so itself; the others come back with "cancelled".
      if (d.phase === "checking") {
        latest.delete(doc);
        patch(doc, { phase: "idle", progress: null, skipRev: st().docs[doc]?.rev ?? null });
      }
    },

    exportReport: async (doc, format) => {
      const d = st().docs[doc];
      if (!d) return;
      const profile = get().get(doc).report?.profile ?? get().profile;
      if (get().get(doc).phase !== "idle") return;
      try {
        const label = profile.toUpperCase().replace("PDFA", "PDF-A").replace("PDFUA", "PDF-UA").replace("PDFX", "PDF-X");
        const to = await pickSave(`${d.name.replace(/\.pdf$/i, "")} Preflight ${label}.${format}`, { doc }, format === "pdf" ? "pdf" : "html");
        if (!to) return;
        const now = get().get(doc);
        if (!st().docs[doc]) return;
        if (busy(now.phase)) return patch(doc, { error: "Wait for the current step to finish, then export the report again." });
        if (now.phase === "checking") {
          latest.delete(doc);
          void toolCancel(now.job).catch(() => {});
        }
        const job = ++jobSeq;
        patch(doc, { phase: "exporting", progress: null, job, error: null, exported: null });
        const date = new Date().toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" });
        const path = await preflightApi.exportReport(job, doc, profile, format, date, to.token, (p) => patch(doc, { progress: p }));
        patch(doc, { phase: "idle", progress: null, exported: path });
      } catch (e) {
        patch(doc, { phase: "idle", progress: null, ...(msg(e) === CANCELLED ? {} : { error: msg(e) }) });
      }
    },

    show: (doc, f) => {
      if (f.page === null) return;
      st().goTo(doc, f.page, f.rect ? Math.max(0, f.rect[1] - 48) : undefined);
      patch(doc, { shown: { page: f.page, rect: f.rect, nonce: ++nonce } });
    },
    clearShown: (doc) => patch(doc, { shown: null }),
    toggleGroup: (doc, name) => {
      const c = get().get(doc).collapsed;
      patch(doc, { collapsed: { ...c, [name]: !c[name] } });
    },

    refreshClaim: async (doc) => {
      const d = st().docs[doc];
      if (!d) return;
      const rev = d.rev;
      const n = (claimSeq.get(doc) ?? 0) + 1;
      claimSeq.set(doc, n);
      try {
        const claim = await preflightApi.claim(doc);
        if (claimSeq.get(doc) === n) patch(doc, { claim, claimRev: rev });
      } catch {
        if (claimSeq.get(doc) === n) patch(doc, { claim: null, claimRev: rev });
      }
    },
    verify: (doc) => {
      const c = get().get(doc).claim;
      if (st().active !== doc) st().activate(doc);
      if (c?.profile) get().setProfile(c.profile);
      get().setOpen(true);
      void get().check(doc);
    },
    dismissConverted: (doc) => patch(doc, { converted: null }),
    forget: (doc) =>
      set((s) => {
        const { [doc]: _gone, ...rest } = s.docs;
        void _gone;
        latest.delete(doc);
        claimSeq.delete(doc);
        return { docs: rest };
      }),
  };
});

/** The Tools menu and palette entry: the rail opens and reads the active document. */
export function openPreflight() {
  if (st().active != null) usePreflight.getState().setOpen(true);
}

// A closed document takes its report with it.
useStore.subscribe((s, prev) => {
  if (s.docs === prev.docs) return;
  for (const id of Object.keys(usePreflight.getState().docs)) if (!s.docs[Number(id)]) usePreflight.getState().forget(Number(id));
});
