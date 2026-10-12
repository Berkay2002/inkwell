import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import { applyState, queued } from "./edit";
import { usePlates } from "./platesView";
import {
  type BoxKind,
  type InkAt,
  type InkReport,
  type Inspected,
  type MarksPreview,
  type PageBoxes,
  type PageSeps,
  type Protection,
  type ConvertReport,
  prepressApi,
} from "./prepressApi";
import {
  CERTIFIED_REASON,
  type ConvertOptions,
  type MarksForm,
  type Margins,
  type OutputSettings,
  type RefKind,
  type Units,
  DEFAULT_CONVERT,
  DEFAULT_MARKS,
  DEFAULT_OUTPUT,
  ZERO,
  allPlates,
  anyMark,
  applyMargins,
  boxOf,
  boxProblem,
  clickSolo,
  convertProblem,
  convertSpec,
  convertedFileName,
  defaultRef,
  isOutward,
  marginsBetween,
  marksKey,
  marksSpec,
  onlyPlate,
  parseLimit,
  scopePages,
  shownMargins,
  toPt,
  togglePlate,
  wireView,
} from "./prepressLogic";

/**
 * Print production for the UI: Output Preview (a rail: plates, ink under the cursor, a total-ink limit, a strip of
 * plates) and three sheets (Convert Colors, Printer's Marks, Page Boxes). UI-agnostic; the screens are
 * components/prepress. Edits go through the edit core (`queued` + `applyState`): one undo step each, written by
 * Save. Nothing here is an assistant tool.
 */

const st = () => useStore.getState();
/** Undo conversion found the document past the conversion: nothing is undone and the sheet stays. */
const MOVED_ON = "The document changed since the conversion; nothing was undone. Undo it from History, or Keep.";
const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const cancelled = (e: unknown) => /cancel/i.test(msg(e));
const stem = (name: string) => name.replace(/\.pdf$/i, "");
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const samePath = (a: string, b: string) => a.replace(/\\/g, "/").toLowerCase() === b.replace(/\\/g, "/").toLowerCase();

let jobSeq = 1_850_000_000;

// ---- state ----

export interface OutputDoc {
  settings: OutputSettings;
  /** The inks of a page, read for a document generation. */
  seps: { page: number; rev: number; data: PageSeps } | null;
  /** The highest total ink of a page and where it is above the limit. */
  report: { page: number; rev: number; limit: number; data: InkReport } | null;
  reporting: boolean;
  /** Draw the areas above the limit on the page. */
  showAreas: boolean;
  /** The ink at the last point the pointer was over, for the page and generation it was read on. */
  probe: { page: number; rev: number; at: InkAt } | null;
  /** Certified (edits refused) or signed (edits stay incremental), as of document generation `rev`. */
  protection: (Protection & { rev: number }) | null;
  inspect: InspectState;
  error: string | null;
}

/**
 * The object inspector. `at` is the clicked point with the page and generation it was clicked on: it is dropped when
 * the generation moves or the page is gone (not when a scroll makes another page the current one). `found` is null
 * while the answer is on its way.
 */
export interface InspectState {
  on: boolean;
  at: { page: number; rev: number; x: number; y: number } | null;
  found: Inspected[] | null;
  error: string | null;
}

const NO_INSPECT: InspectState = { on: false, at: null, found: null, error: null };

const freshDoc = (): OutputDoc => ({
  settings: DEFAULT_OUTPUT,
  seps: null,
  report: null,
  reporting: false,
  showAreas: true,
  probe: null,
  protection: null,
  inspect: NO_INSPECT,
  error: null,
});

/** The inks of the page the document shows now: what was read for another page or generation is not them. */
export function currentSeps(o: OutputDoc | undefined, d: { currentPage: number; rev: number } | undefined): PageSeps | null {
  const s = o?.seps;
  return s && d && s.page === d.currentPage && s.rev === d.rev ? s.data : null;
}

/** `unsaved`: converted in the open document, writing the new file failed; only the save can be tried again. */
export type Phase = "options" | "confirm" | "running" | "unsaved" | "done";

export interface ConvertSheet {
  kind: "convert";
  doc: number;
  phase: Phase;
  opts: ConvertOptions;
  progress: number;
  error: string | null;
  job: number;
  /** After a run: what the engine says it did and where the new file went. */
  result: { report: ConvertReport; path: string | null } | null;
  /** `unsaved` only: the document's generation right after the conversion, and whether closing was asked about. */
  convertedRev: number | null;
  closing: boolean;
}

export interface MarksSheet {
  kind: "marks";
  doc: number;
  page: number;
  form: MarksForm;
  /** The page's boxes (for the preview's backdrop) and the last layout the engine drew. */
  boxes: PageBoxes | null;
  preview: { key: string; data: MarksPreview } | null;
  busy: boolean;
  error: string | null;
  done: string | null;
}

export interface BoxesSheet {
  kind: "boxes";
  doc: number;
  page: number;
  boxes: PageBoxes | null;
  /** The box to set and the one the margins are measured from. */
  box: BoxKind;
  ref: RefKind;
  /** Points, as seen on the page; negative is not allowed except through the other direction. */
  margins: Margins;
  /** One typed value moves all four sides. */
  linked: boolean;
  units: Units;
  scope: "all" | "page" | "range";
  range: string;
  busy: boolean;
  error: string | null;
  done: string | null;
}

export type Sheet = ConvertSheet | MarksSheet | BoxesSheet;

interface PrepressStore {
  /** The Output Preview rail. */
  open: boolean;
  docs: Record<number, OutputDoc>;
  sheet: Sheet | null;

  get: (doc: number) => OutputDoc;
  setOpen: (open: boolean, doc?: number) => void;
  /** Reads what the rail shows for the document's current page: the inks, the limit report, the view. */
  refresh: (doc: number) => Promise<void>;
  setMode: (doc: number, mode: OutputSettings["mode"]) => void;
  setOverprint: (doc: number, on: boolean) => void;
  toggle: (doc: number, key: string) => void;
  solo: (doc: number, key: string) => void;
  showAll: (doc: number) => void;
  /** The limit as typed: true when it was taken. */
  setLimit: (doc: number, text: string) => boolean;
  setShowAreas: (doc: number, on: boolean) => void;
  probe: (doc: number, page: number, x: number, y: number) => void;
  clearProbe: (doc: number) => void;
  /** The object inspector: a mode in which a click on a page asks what is painted there. Read-only. */
  setInspecting: (doc: number, on: boolean) => void;
  inspectAt: (doc: number, page: number, x: number, y: number) => Promise<void>;
  /** Drops the marker and the list; the mode stays. */
  clearInspect: (doc: number) => void;

  /** The sheet's page or the document's pages changed under it (undo, an assistant edit): read it again or close. */
  resync: () => void;

  openConvert: (doc?: number) => void;
  setConvert: (p: Partial<ConvertOptions>) => void;
  review: () => void;
  backToOptions: () => void;
  pickProfile: () => Promise<void>;
  runConvert: () => Promise<void>;
  /** After a failed write: choose a file again and save the converted document there. */
  saveConverted: () => Promise<void>;
  cancelConvert: () => void;
  /** After a failed write, closing asks first: the open document holds the conversion, and a Save would write it. */
  undoConversion: () => Promise<void>;
  keepConversion: () => void;

  openMarks: (doc?: number) => void;
  setMarks: (p: Partial<MarksForm>) => void;
  setMarkKind: (k: keyof MarksForm["marks"], on: boolean) => void;
  loadMarksPreview: () => Promise<void>;
  applyMarks: () => Promise<void>;

  openBoxes: (doc?: number, box?: BoxKind) => void;
  setBox: (box: BoxKind) => void;
  setRef: (ref: RefKind) => void;
  setMargin: (side: keyof Margins, pt: number) => void;
  setBoxes: (p: Partial<Pick<BoxesSheet, "linked" | "units" | "scope" | "range">>) => void;
  /** Margins back to what the page has now. */
  revertMargins: () => void;
  /** The usual 3 mm bleed beyond the trim box. */
  bleedPreset: () => void;
  applyBoxes: () => Promise<void>;

  closeSheet: () => void;
  /** The document holds a conversion that was not saved: closing it goes back to the sheet's Undo or Keep. True when held. */
  holdConversion: (doc: number) => boolean;
  forget: (doc: number) => void;
}

/** The box a sheet would write, as the page shows it: the reference moved by the margins. */
export function newBox(s: BoxesSheet) {
  if (!s.boxes) return null;
  return applyMargins(boxOf(s.boxes, s.ref), s.boxes.rotate, s.margins, isOutward(s.box, s.ref));
}

/** The margins of a box the page has, measured from `ref`. */
function currentMargins(b: PageBoxes, box: BoxKind, ref: RefKind): Margins {
  return marginsBetween(boxOf(b, ref), boxOf(b, box), b.rotate, isOutward(box, ref));
}

export const usePrepress = create<PrepressStore>((set, get) => {
  const patchDoc = (doc: number, p: Partial<OutputDoc>) => {
    if (!st().docs[doc]) return;
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? freshDoc()), ...p } } }));
  };
  const patchSheet = <T extends Sheet>(kind: T["kind"], p: Partial<T>) =>
    set((s) => (s.sheet && s.sheet.kind === kind ? { sheet: { ...s.sheet, ...p } as Sheet } : s));
  const sheet = <T extends Sheet>(kind: T["kind"]) => {
    const s = get().sheet;
    return s && s.kind === kind ? (s as T) : null;
  };
  const docOf = (id?: number) => {
    const doc = id ?? st().active;
    return doc != null ? st().docs[doc] : undefined;
  };

  // ---- Output Preview ----

  /** The view the tiles are drawn in: asked for again on every change, the engine returns the same id for the same view. */
  let viewSeq = 0;
  async function syncView(doc: number) {
    const seq = ++viewSeq;
    const d = get().docs[doc];
    if (!get().open || !d || !st().docs[doc]) return void usePlates.setState({ doc: null, view: null });
    try {
      const view = await prepressApi.view(doc, wireView(d.settings));
      if (seq !== viewSeq || !get().open) return;
      usePlates.setState({ doc, view });
    } catch (e) {
      if (seq === viewSeq) patchDoc(doc, { error: msg(e) });
    }
  }

  let reportJob = 0;
  async function loadReport(doc: number) {
    const d = st().docs[doc];
    const o = get().docs[doc];
    if (!d || !o || !get().open) return;
    const { settings } = o;
    const page = d.currentPage;
    const have = o.report;
    if (have && have.page === page && have.rev === d.rev && have.limit === settings.tacLimit) return;
    if (reportJob) void api.toolCancel(reportJob).catch(() => {});
    const job = ++jobSeq;
    reportJob = job;
    patchDoc(doc, { reporting: true, error: null });
    try {
      const [data] = await prepressApi.ink(job, doc, [page], settings.tacLimit, true);
      if (reportJob !== job) return;
      reportJob = 0;
      patchDoc(doc, { report: { page, rev: d.rev, limit: settings.tacLimit, data }, reporting: false });
    } catch (e) {
      if (reportJob !== job) return;
      reportJob = 0;
      patchDoc(doc, { reporting: false, ...(cancelled(e) ? {} : { error: msg(e) }) });
    }
  }

  async function loadSeps(doc: number) {
    const d = st().docs[doc];
    const o = get().docs[doc];
    if (!d || !o) return;
    const page = d.currentPage;
    if (o.seps && o.seps.page === page && o.seps.rev === d.rev) return;
    try {
      const data = await prepressApi.separations(doc, page);
      const now = st().docs[doc];
      if (!now || now.currentPage !== page || now.rev !== d.rev) return;
      patchDoc(doc, { seps: { page, rev: d.rev, data }, error: null });
    } catch (e) {
      patchDoc(doc, { error: msg(e) });
    }
  }

  /** The probe asks one at a time and only for the newest point: a moving pointer never queues. */
  const waiting = new Map<number, { page: number; x: number; y: number }>();
  const asking = new Set<number>();
  /** Counts the times the pointer left a document's page: an answer asked before is dropped. */
  const left = new Map<number, number>();
  async function pump(doc: number) {
    if (asking.has(doc)) return;
    asking.add(doc);
    try {
      for (let q = waiting.get(doc); q; q = waiting.get(doc)) {
        waiting.delete(doc);
        if (!get().open || !st().docs[doc]) break;
        const gone = left.get(doc) ?? 0;
        const rev = st().docs[doc]?.rev;
        try {
          const at = await prepressApi.inkAt(doc, q.page, q.x, q.y);
          // the pointer may have left, or the document changed, while this was asked
          if (get().open && get().docs[doc] && (left.get(doc) ?? 0) === gone && st().docs[doc]?.rev === rev && rev !== undefined) {
            patchDoc(doc, { probe: { page: q.page, rev, at } });
          }
        } catch {
          // a point that cannot be read just keeps the last answer
        }
      }
    } finally {
      asking.delete(doc);
    }
  }

  /** The newest click of a document: an answer for an older one is dropped. */
  const inspectJob = new Map<number, number>();

  function changeSettings(doc: number, f: (o: OutputSettings) => OutputSettings) {
    const o = get().docs[doc];
    if (!o) return;
    patchDoc(doc, { settings: f(o.settings) });
    void syncView(doc);
  }

  // ---- sheets ----

  /**
   * Whether the document is certified or signed, read again when its generation moved (it may have been signed or
   * certified since). The engine refuses a certified one itself; this is for the screens.
   */
  async function loadProtection(doc: number) {
    const rev = st().docs[doc]?.rev;
    if (rev === undefined || get().docs[doc]?.protection?.rev === rev) return;
    try {
      const protection = await prepressApi.protection(doc);
      patchDoc(doc, { protection: { ...protection, rev } });
    } catch {
      // unknown: the sheets stay usable and the engine says no where it must
    }
  }

  const certified = (doc: number) => get().docs[doc]?.protection?.certified === true;

  async function marksBoxes(doc: number, page: number) {
    const rev = st().docs[doc]?.rev;
    try {
      const boxes = await prepressApi.pageBoxes(doc, page);
      // a newer generation asks again
      if (st().docs[doc]?.rev === rev) patchSheet<MarksSheet>("marks", { boxes });
    } catch (e) {
      patchSheet<MarksSheet>("marks", { error: msg(e) });
    }
  }

  /** The boxes of the sheet's page, and the margins back to what the page has. */
  async function boxesOf(doc: number, page: number) {
    const rev = st().docs[doc]?.rev;
    try {
      const boxes = await prepressApi.pageBoxes(doc, page);
      const s = sheet<BoxesSheet>("boxes");
      if (s && s.doc === doc && s.page === page && st().docs[doc]?.rev === rev) patchSheet<BoxesSheet>("boxes", { boxes, margins: currentMargins(boxes, s.box, s.ref) });
    } catch (e) {
      patchSheet<BoxesSheet>("boxes", { error: msg(e) });
    }
  }

  /** The file to convert into, as a save token. Null: cancelled, or refused with the reason on the sheet. */
  async function convertTarget(d: { id: number; name: string; path: string }, opts: ConvertOptions): Promise<string | null> {
    const f = await api.pickSave(convertedFileName(stem(d.name), opts), { doc: d.id }, "pdf");
    if (!f) return null;
    // the confirmation promises the original is not touched (the engine checks again, by file identity)
    const back = (error: string) => (patchSheet<ConvertSheet>("convert", { error }), null);
    if (samePath(f.path, d.path)) return back("That is this document's own file. Choose another name, or convert in this document and Save.");
    if (Object.values(st().docs).some((o) => o.id !== d.id && samePath(o.path, f.path))) return back("That file is open in another tab. Choose another name.");
    return f.token;
  }

  /** A created file nobody had saved has a scratch copy; it is not needed once there is a file. */
  function dropScratch(doc: number) {
    const scratch = st().docs[doc]?.scratch;
    if (!scratch) return;
    st().patch(doc, { scratch: null });
    void api.discardScratch(scratch).catch(() => {});
  }

  return {
    open: false,
    docs: {},
    sheet: null,

    get: (doc) => get().docs[doc] ?? freshDoc(),

    setOpen: (open, docId) => {
      if (!open) {
        reportJob = 0;
        viewSeq++;
        waiting.clear();
        inspectJob.clear();
        usePlates.setState({ doc: null, view: null });
        // leaving Output Preview leaves the inspector too
        set((s) => ({ open: false, docs: Object.fromEntries(Object.entries(s.docs).map(([k, o]) => [k, { ...o, inspect: NO_INSPECT }])) }));
        return;
      }
      const doc = docId ?? st().active;
      if (doc == null || !st().docs[doc]) return;
      set((s) => ({ open: true, docs: s.docs[doc] ? s.docs : { ...s.docs, [doc]: freshDoc() } }));
      void get().refresh(doc);
    },

    refresh: async (doc) => {
      if (!get().open || !st().docs[doc]) return;
      if (!get().docs[doc]) set((s) => ({ docs: { ...s.docs, [doc]: freshDoc() } }));
      void loadProtection(doc);
      await syncView(doc);
      await Promise.all([loadSeps(doc), loadReport(doc)]);
    },

    setMode: (doc, mode) => changeSettings(doc, (o) => ({ ...o, mode })),
    setOverprint: (doc, overprint) => changeSettings(doc, (o) => ({ ...o, overprint })),
    toggle: (doc, key) => changeSettings(doc, (o) => togglePlate(o, key)),
    solo: (doc, key) => changeSettings(doc, (o) => clickSolo(o, key, currentSeps(get().docs[doc], st().docs[doc]))),
    showAll: (doc) => changeSettings(doc, allPlates),

    setLimit: (doc, text) => {
      const v = parseLimit(text);
      if (v === null || !get().docs[doc]) return false;
      if (v === get().docs[doc].settings.tacLimit) return true;
      changeSettings(doc, (o) => ({ ...o, tacLimit: v }));
      void loadReport(doc);
      return true;
    },

    setShowAreas: (doc, showAreas) => patchDoc(doc, { showAreas }),

    probe: (doc, page, x, y) => {
      if (!get().open || !get().docs[doc]) return;
      waiting.set(doc, { page, x, y });
      void pump(doc);
    },
    clearProbe: (doc) => {
      waiting.delete(doc);
      left.set(doc, (left.get(doc) ?? 0) + 1);
      patchDoc(doc, { probe: null });
    },

    setInspecting: (doc, on) => {
      if (!get().docs[doc]) return;
      inspectJob.set(doc, (inspectJob.get(doc) ?? 0) + 1);
      patchDoc(doc, { inspect: { ...NO_INSPECT, on } });
    },
    clearInspect: (doc) => {
      const o = get().docs[doc];
      if (!o) return;
      inspectJob.set(doc, (inspectJob.get(doc) ?? 0) + 1);
      patchDoc(doc, { inspect: { ...NO_INSPECT, on: o.inspect.on } });
    },
    inspectAt: async (doc, page, x, y) => {
      const d = st().docs[doc];
      if (!get().open || !d || !get().docs[doc]?.inspect.on) return;
      const at = { page, rev: d.rev, x, y };
      const job = (inspectJob.get(doc) ?? 0) + 1;
      inspectJob.set(doc, job);
      patchDoc(doc, { inspect: { on: true, at, found: null, error: null } });
      // a newer click, leaving the mode, or a new generation (or the page gone) while this was asked
      const stale = () => {
        const now = st().docs[doc];
        return inspectJob.get(doc) !== job || !get().open || !now || now.rev !== at.rev || page >= now.pageCount;
      };
      try {
        const found = await prepressApi.inspect(doc, page, x, y);
        if (!stale()) patchDoc(doc, { inspect: { on: true, at, found, error: null } });
      } catch (e) {
        if (!stale()) patchDoc(doc, { inspect: { on: true, at, found: null, error: msg(e) } });
      }
    },

    // ---- Convert Colors ----

    openConvert: (docId) => {
      const d = docOf(docId);
      if (!d || sheet<ConvertSheet>("convert")?.phase === "running") return;
      set({ sheet: { kind: "convert", doc: d.id, phase: "options", opts: DEFAULT_CONVERT, progress: 0, error: null, job: 0, result: null, convertedRev: null, closing: false } });
      void loadProtection(d.id);
    },

    setConvert: (p) => {
      const s = sheet<ConvertSheet>("convert");
      if (!s || s.phase === "running") return;
      patchSheet<ConvertSheet>("convert", { opts: { ...s.opts, ...p }, error: null });
    },

    review: () => {
      const s = sheet<ConvertSheet>("convert");
      const d = s && st().docs[s.doc];
      if (!s || !d || s.phase !== "options") return;
      const problem = convertProblem(s.opts, d.pageCount, d.currentPage);
      patchSheet<ConvertSheet>("convert", problem ? { error: problem } : { phase: "confirm", error: null });
    },

    backToOptions: () => {
      if (sheet<ConvertSheet>("convert")?.phase === "confirm") patchSheet<ConvertSheet>("convert", { phase: "options", error: null });
    },

    pickProfile: async () => {
      const s = sheet<ConvertSheet>("convert");
      if (!s || s.phase === "running") return;
      try {
        const f = await prepressApi.pickProfile();
        if (f) get().setConvert({ profile: { token: f.token, name: baseName(f.path) }, intent: "profile" });
      } catch (e) {
        patchSheet<ConvertSheet>("convert", { error: msg(e) });
      }
    },

    runConvert: async () => {
      const s = sheet<ConvertSheet>("convert");
      const d = s && st().docs[s.doc];
      if (!s || !d || s.phase !== "confirm") return;
      if (certified(d.id)) return patchSheet<ConvertSheet>("convert", { error: CERTIFIED_REASON });
      const problem = convertProblem(s.opts, d.pageCount, d.currentPage);
      if (problem) return patchSheet<ConvertSheet>("convert", { phase: "options", error: problem });
      const doc = d.id;
      let to: string | null = null;
      if (s.opts.output === "newFile") {
        to = await convertTarget(d, s.opts);
        if (to === null) return;
      }
      // the dialog was open a while
      const now = sheet<ConvertSheet>("convert");
      if (!now || now.phase !== "confirm" || !st().docs[doc]) return;
      const job = ++jobSeq;
      patchSheet<ConvertSheet>("convert", { phase: "running", progress: 0, error: null, job });
      try {
        const spec = convertSpec(s.opts, d.pageCount, d.currentPage);
        const profile = s.opts.intent === "profile" ? (s.opts.profile?.token ?? null) : null;
        // one command: the conversion and the Save as cannot be parted by another edit (an undo, the assistant)
        const out = await queued(doc, () =>
          prepressApi.convert(job, doc, spec, profile, to, (p) => {
            if (p.pages > 0) patchSheet<ConvertSheet>("convert", { progress: p.done / p.pages });
          }),
        );
        applyState(doc, out.state);
        if (to !== null && !out.saved) {
          // converted in the open document; only the write is tried again, never a second conversion
          return patchSheet<ConvertSheet>("convert", {
            phase: "unsaved",
            progress: 1,
            result: { report: out.report, path: null },
            convertedRev: out.state.rev,
            error: `The colours were converted in the open document, but saving the new file failed: ${out.saveError ?? "unknown error"} Undo takes the conversion back.`,
          });
        }
        if (out.saved) dropScratch(doc);
        patchSheet<ConvertSheet>("convert", { phase: "done", progress: 1, result: { report: out.report, path: out.saved ? out.state.path : null } });
        void get().refresh(doc);
      } catch (e) {
        // the engine rolls a failed or cancelled conversion back
        patchSheet<ConvertSheet>("convert", { phase: "confirm", progress: 0, error: cancelled(e) ? "Cancelled. Nothing was changed." : msg(e) });
        void api.editState(doc).then((state) => applyState(doc, state), () => {});
      }
    },

    saveConverted: async () => {
      const s = sheet<ConvertSheet>("convert");
      const d = s && st().docs[s.doc];
      if (!s || !d || s.phase !== "unsaved" || !s.result) return;
      const doc = d.id;
      const to = await convertTarget(d, s.opts);
      if (to === null) return;
      const report = s.result.report;
      patchSheet<ConvertSheet>("convert", { error: null });
      try {
        const state = await queued(doc, () => prepressApi.saveOther(doc, to));
        applyState(doc, state);
        dropScratch(doc);
        patchSheet<ConvertSheet>("convert", { phase: "done", result: { report, path: state.path }, error: null });
        void get().refresh(doc);
      } catch (e) {
        patchSheet<ConvertSheet>("convert", { error: `Saving the new file failed: ${msg(e)}` });
      }
    },

    cancelConvert: () => {
      const s = sheet<ConvertSheet>("convert");
      if (s?.phase === "running") void api.toolCancel(s.job).catch(() => {});
    },

    undoConversion: async () => {
      const s = sheet<ConvertSheet>("convert");
      const d = s && st().docs[s.doc];
      if (!s || !d || s.phase !== "unsaved") return;
      try {
        // The engine compares the generation under its edit lock: a step that landed since (an undo and redo, the
        // assistant, which does not go through this queue) is not what an undo would take back.
        const after = s.convertedRev == null ? null : await queued(s.doc, () => api.editUndoIf(s.doc, s.convertedRev as number));
        if (!after) return patchSheet<ConvertSheet>("convert", { error: MOVED_ON });
        applyState(s.doc, after);
      } catch (e) {
        return patchSheet<ConvertSheet>("convert", { error: String(e) === "stale" ? MOVED_ON : msg(e) });
      }
      if (sheet<ConvertSheet>("convert")?.phase === "unsaved") set({ sheet: null });
    },

    keepConversion: () => {
      if (sheet<ConvertSheet>("convert")?.phase === "unsaved") set({ sheet: null });
    },

    // ---- Printer's marks ----

    openMarks: (docId) => {
      const d = docOf(docId);
      if (!d) return;
      set({ sheet: { kind: "marks", doc: d.id, page: d.currentPage, form: DEFAULT_MARKS, boxes: null, preview: null, busy: false, error: null, done: null } });
      void loadProtection(d.id);
      void marksBoxes(d.id, d.currentPage);
      void get().loadMarksPreview();
    },

    setMarks: (p) => {
      const s = sheet<MarksSheet>("marks");
      if (!s || s.busy) return;
      patchSheet<MarksSheet>("marks", { form: { ...s.form, ...p }, error: null, done: null });
      void get().loadMarksPreview();
    },

    setMarkKind: (k, on) => {
      const s = sheet<MarksSheet>("marks");
      if (s) get().setMarks({ marks: { ...s.form.marks, [k]: on } });
    },

    loadMarksPreview: async () => {
      const s = sheet<MarksSheet>("marks");
      const d = s && st().docs[s.doc];
      if (!s || !d) return;
      if (!anyMark(s.form.marks)) return patchSheet<MarksSheet>("marks", { preview: null });
      const key = marksKey(s.page, d.rev, s.form);
      if (s.preview?.key === key) return;
      try {
        const data = await prepressApi.marksPreview(s.doc, s.page, marksSpec(s.form, [s.page]));
        const now = sheet<MarksSheet>("marks");
        // an older answer must not replace a newer one
        if (!now || marksKey(now.page, st().docs[now.doc]?.rev ?? -1, now.form) !== key) return;
        patchSheet<MarksSheet>("marks", { preview: { key, data }, error: null });
      } catch (e) {
        const now = sheet<MarksSheet>("marks");
        // an older failure must not replace a newer answer either
        if (!now || marksKey(now.page, st().docs[now.doc]?.rev ?? -1, now.form) !== key) return;
        patchSheet<MarksSheet>("marks", { preview: null, error: msg(e) });
      }
    },

    applyMarks: async () => {
      const s = sheet<MarksSheet>("marks");
      const d = s && st().docs[s.doc];
      if (!s || !d || s.busy) return;
      if (certified(d.id)) return patchSheet<MarksSheet>("marks", { error: CERTIFIED_REASON });
      if (!anyMark(s.form.marks)) return patchSheet<MarksSheet>("marks", { error: "Choose at least one kind of mark." });
      const pages = scopePages(s.form.scope, s.form.range, d.pageCount, s.page);
      if (pages === null) return patchSheet<MarksSheet>("marks", { error: "The page range is not valid." });
      patchSheet<MarksSheet>("marks", { busy: true, error: null, done: null });
      try {
        const out = await queued(d.id, () => prepressApi.marks(d.id, marksSpec(s.form, pages)));
        applyState(d.id, out.state);
        const n = pages ? pages.length : d.pageCount;
        patchSheet<MarksSheet>("marks", {
          busy: false,
          done: `Added printer's marks to ${n === 1 ? "1 page" : `${n} pages`}.${out.notes.length ? ` ${out.notes.join(". ")}.` : ""}`,
        });
        void marksBoxes(d.id, s.page);
        void get().refresh(d.id);
      } catch (e) {
        patchSheet<MarksSheet>("marks", { busy: false, error: msg(e) });
      }
    },

    // ---- Page boxes ----

    openBoxes: (docId, box = "trim") => {
      const d = docOf(docId);
      if (!d) return;
      const page = d.currentPage;
      const ref = defaultRef(box);
      set({
        sheet: { kind: "boxes", doc: d.id, page, boxes: null, box, ref, margins: ZERO, linked: false, units: "pt", scope: "all", range: "", busy: false, error: null, done: null },
      });
      void loadProtection(d.id);
      void boxesOf(d.id, page);
    },

    setBox: (box) => {
      const s = sheet<BoxesSheet>("boxes");
      if (!s || s.busy) return;
      const ref = s.ref === box ? defaultRef(box) : s.ref;
      patchSheet<BoxesSheet>("boxes", { box, ref, margins: s.boxes ? currentMargins(s.boxes, box, ref) : ZERO, error: null, done: null });
    },

    setRef: (ref) => {
      const s = sheet<BoxesSheet>("boxes");
      if (!s || s.busy || ref === s.box) return;
      patchSheet<BoxesSheet>("boxes", { ref, margins: s.boxes ? currentMargins(s.boxes, s.box, ref) : ZERO, error: null, done: null });
    },

    setMargin: (side, pt) => {
      const s = sheet<BoxesSheet>("boxes");
      if (!s || s.busy) return;
      const margins = s.linked ? { left: pt, bottom: pt, right: pt, top: pt } : { ...s.margins, [side]: pt };
      patchSheet<BoxesSheet>("boxes", { margins, error: null, done: null });
    },

    setBoxes: (p) => {
      const s = sheet<BoxesSheet>("boxes");
      if (!s || s.busy) return;
      // switching linked on makes the four the same as the one that was last typed (the left)
      const margins = p.linked && !s.linked ? { left: s.margins.left, bottom: s.margins.left, right: s.margins.left, top: s.margins.left } : s.margins;
      patchSheet<BoxesSheet>("boxes", { ...p, margins, error: null, done: null });
    },

    revertMargins: () => {
      const s = sheet<BoxesSheet>("boxes");
      if (s?.boxes && !s.busy) patchSheet<BoxesSheet>("boxes", { margins: currentMargins(s.boxes, s.box, s.ref), error: null, done: null });
    },

    bleedPreset: () => {
      const s = sheet<BoxesSheet>("boxes");
      if (!s || s.busy) return;
      const mm = toPt(3, "mm");
      patchSheet<BoxesSheet>("boxes", { box: "bleed", ref: "trim", margins: { left: mm, bottom: mm, right: mm, top: mm }, units: "mm", linked: true, error: null, done: null });
    },

    applyBoxes: async () => {
      const s = sheet<BoxesSheet>("boxes");
      const d = s && st().docs[s.doc];
      if (!s || !d || !s.boxes || s.busy) return;
      if (certified(d.id)) return patchSheet<BoxesSheet>("boxes", { error: CERTIFIED_REASON });
      const rect = newBox(s);
      const problem = rect && boxProblem(s.box, rect, s.boxes);
      if (problem) return patchSheet<BoxesSheet>("boxes", { error: problem });
      const pages = scopePages(s.scope, s.range, d.pageCount, s.page);
      if (pages === null) return patchSheet<BoxesSheet>("boxes", { error: "The page range is not valid." });
      patchSheet<BoxesSheet>("boxes", { busy: true, error: null, done: null });
      try {
        // each page measures the margins, as shown, from its own reference box and maps them by its own turn
        const spec = {
          ...(pages ? { pages } : {}),
          kind: s.box,
          from: s.ref,
          margin: shownMargins(s.margins, isOutward(s.box, s.ref)),
          shown: true,
        };
        const state = await queued(d.id, () => prepressApi.boxes(d.id, spec));
        applyState(d.id, state);
        const n = pages ? pages.length : d.pageCount;
        const fresh = await prepressApi.pageBoxes(d.id, s.page).catch(() => null);
        patchSheet<BoxesSheet>("boxes", {
          busy: false,
          done: `Set the ${s.box} box on ${n === 1 ? "1 page" : `${n} pages`}.`,
          ...(fresh ? { boxes: fresh, margins: currentMargins(fresh, s.box, s.ref) } : {}),
        });
      } catch (e) {
        patchSheet<BoxesSheet>("boxes", { busy: false, error: msg(e) });
      }
    },

    resync: () => {
      const s = get().sheet;
      const d = s && st().docs[s.doc];
      if (!s || s.kind === "convert" || !d) return;
      void loadProtection(s.doc);
      // an undo took the sheet's page away
      if (s.page >= d.pageCount) return set({ sheet: null });
      if (s.kind === "marks") {
        void marksBoxes(s.doc, s.page);
        void get().loadMarksPreview();
      } else void boxesOf(s.doc, s.page);
    },

    closeSheet: () => {
      const s = get().sheet;
      if (!s) return;
      if (s.kind === "convert" && s.phase === "running") return;
      // the open document holds the conversion; closing is Undo or Keep, a second Escape goes back to the sheet
      if (s.kind === "convert" && s.phase === "unsaved") return patchSheet<ConvertSheet>("convert", { closing: !s.closing });
      if ((s.kind === "marks" || s.kind === "boxes") && s.busy) return;
      set({ sheet: null });
    },

    holdConversion: (doc) => {
      const s = get().sheet;
      if (s?.kind !== "convert" || s.phase !== "unsaved" || s.doc !== doc) return false;
      patchSheet<ConvertSheet>("convert", { closing: true });
      return true;
    },

    forget: (doc) => {
      waiting.delete(doc);
      inspectJob.delete(doc);
      left.delete(doc);
      if (usePlates.getState().doc === doc) usePlates.setState({ doc: null, view: null });
      set((s) => {
        const { [doc]: _gone, ...rest } = s.docs;
        void _gone;
        return { docs: rest, sheet: s.sheet && s.sheet.doc === doc ? null : s.sheet };
      });
    },
  };
});

// A closed document takes its state with it.
useStore.subscribe((s, prev) => {
  if (s.docs === prev.docs) return;
  for (const id of Object.keys(usePrepress.getState().docs)) if (!s.docs[Number(id)]) usePrepress.getState().forget(Number(id));
  // a clicked point belongs to the generation it was clicked on and to its page
  for (const [id, o] of Object.entries(usePrepress.getState().docs)) {
    const at = o.inspect.at;
    const d = s.docs[Number(id)];
    if (at && d && (d.rev !== at.rev || at.page >= d.pageCount)) usePrepress.getState().clearInspect(Number(id));
  }
  const sh = usePrepress.getState().sheet;
  if (sh && !s.docs[sh.doc]) return usePrepress.getState().forget(sh.doc);
  // an undo, redo or an assistant edit moved the document under an open sheet
  const [now, before] = sh ? [s.docs[sh.doc], prev.docs[sh.doc]] : [];
  if (sh && now && before && (now.rev !== before.rev || now.pageCount !== before.pageCount)) usePrepress.getState().resync();
});

// A tile of the view in use failed: the engine may have dropped the view (it keeps a bounded number), so ask for it again.
usePlates.subscribe((s, prev) => {
  if (s.lost !== prev.lost && s.doc !== null && s.view === prev.view) void usePrepress.getState().refresh(s.doc);
});

/**
 * The handlers of a page while the object inspector is on. A press is taken before anything on the page hears it
 * (a field acts on mousedown, Sign and Measure on pointerdown), and a click asks what is painted there instead of
 * following a link. `at` maps the event to page points.
 */
export function inspectCapture<E extends { stopPropagation(): void; preventDefault(): void }>(doc: number, page: number, at: (e: E) => [number, number]) {
  const take = (e: E) => {
    e.stopPropagation();
    e.preventDefault();
  };
  return {
    onPointerDownCapture: take,
    onMouseDownCapture: take,
    onClickCapture: (e: E) => {
      take(e);
      void usePrepress.getState().inspectAt(doc, page, ...at(e));
    },
  };
}

/** Opens a tool on the active document (the tools list and the palette). */
export const openOutputPreview = () => usePrepress.getState().setOpen(true);
export const openConvertColors = () => usePrepress.getState().openConvert();
export const openPrinterMarks = () => usePrepress.getState().openMarks();
export const openPageBoxes = () => usePrepress.getState().openBoxes();

export { onlyPlate };
