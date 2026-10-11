import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import { applyState, queued } from "./edit";
import { usePlates } from "./platesView";
import {
  type BoxKind,
  type InkAt,
  type InkReport,
  type MarksPreview,
  type PageBoxes,
  type PageSeps,
  type ConvertReport,
  prepressApi,
} from "./prepressApi";
import {
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
  toPt,
  togglePlate,
  userMargins,
  wireView,
} from "./prepressLogic";

/**
 * Print production for the UI: Output Preview (a rail: plates, ink under the cursor, a total-ink limit, a strip of
 * plates) and three sheets (Convert Colors, Printer's Marks, Page Boxes). UI-agnostic; the screens are
 * components/prepress. Edits go through the edit core (`queued` + `applyState`): one undo step each, written by
 * Save. Nothing here is an assistant tool.
 */

const st = () => useStore.getState();
const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const cancelled = (e: unknown) => /cancel/i.test(msg(e));
const stem = (name: string) => name.replace(/\.pdf$/i, "");
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

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
  /** The ink at the last point the pointer was over. */
  probe: { page: number; at: InkAt } | null;
  error: string | null;
}

const freshDoc = (): OutputDoc => ({ settings: DEFAULT_OUTPUT, seps: null, report: null, reporting: false, showAreas: true, probe: null, error: null });

export type Phase = "options" | "confirm" | "running" | "done";

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

  openConvert: (doc?: number) => void;
  setConvert: (p: Partial<ConvertOptions>) => void;
  review: () => void;
  backToOptions: () => void;
  pickProfile: () => Promise<void>;
  runConvert: () => Promise<void>;
  cancelConvert: () => void;

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
  async function pump(doc: number) {
    if (asking.has(doc)) return;
    asking.add(doc);
    try {
      for (let q = waiting.get(doc); q; q = waiting.get(doc)) {
        waiting.delete(doc);
        if (!get().open || !st().docs[doc]) break;
        try {
          const at = await prepressApi.inkAt(doc, q.page, q.x, q.y);
          // the pointer may have left while this was asked
          if (get().open && get().docs[doc]) patchDoc(doc, { probe: { page: q.page, at } });
        } catch {
          // a point that cannot be read just keeps the last answer
        }
      }
    } finally {
      asking.delete(doc);
    }
  }

  function changeSettings(doc: number, f: (o: OutputSettings) => OutputSettings) {
    const o = get().docs[doc];
    if (!o) return;
    patchDoc(doc, { settings: f(o.settings) });
    void syncView(doc);
  }

  // ---- sheets ----

  async function marksBoxes(doc: number, page: number) {
    try {
      const boxes = await prepressApi.pageBoxes(doc, page);
      patchSheet<MarksSheet>("marks", { boxes });
    } catch (e) {
      patchSheet<MarksSheet>("marks", { error: msg(e) });
    }
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
        usePlates.setState({ doc: null, view: null });
        set({ open: false });
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
      await syncView(doc);
      await Promise.all([loadSeps(doc), loadReport(doc)]);
    },

    setMode: (doc, mode) => changeSettings(doc, (o) => ({ ...o, mode })),
    setOverprint: (doc, overprint) => changeSettings(doc, (o) => ({ ...o, overprint })),
    toggle: (doc, key) => changeSettings(doc, (o) => togglePlate(o, key)),
    solo: (doc, key) => changeSettings(doc, (o) => clickSolo(o, key, get().docs[doc]?.seps?.data ?? null)),
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
      patchDoc(doc, { probe: null });
    },

    // ---- Convert Colors ----

    openConvert: (docId) => {
      const d = docOf(docId);
      if (!d || sheet<ConvertSheet>("convert")?.phase === "running") return;
      set({ sheet: { kind: "convert", doc: d.id, phase: "options", opts: DEFAULT_CONVERT, progress: 0, error: null, job: 0, result: null } });
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
      const problem = convertProblem(s.opts, d.pageCount, d.currentPage);
      if (problem) return patchSheet<ConvertSheet>("convert", { phase: "options", error: problem });
      const doc = d.id;
      let to: string | null = null;
      if (s.opts.output === "newFile") {
        const f = await api.pickSave(convertedFileName(stem(d.name), s.opts), { doc }, "pdf");
        if (!f) return;
        to = f.token;
      }
      // the dialog was open a while
      const now = sheet<ConvertSheet>("convert");
      if (!now || now.phase !== "confirm" || !st().docs[doc]) return;
      const job = ++jobSeq;
      patchSheet<ConvertSheet>("convert", { phase: "running", progress: 0, error: null, job });
      let report: ConvertReport | null = null;
      try {
        const spec = convertSpec(s.opts, d.pageCount, d.currentPage);
        const out = await queued(doc, () =>
          prepressApi.convert(job, doc, spec, s.opts.intent === "profile" ? (s.opts.profile?.token ?? null) : null, (p) => {
            if (p.pages > 0) patchSheet<ConvertSheet>("convert", { progress: p.done / p.pages });
          }),
        );
        report = out.report;
        applyState(doc, out.state);
        let path: string | null = null;
        if (to !== null) {
          // Save as: the document then lives in the new file and the original stays as it was
          const saved = await queued(doc, () => api.saveDocument(doc, to));
          applyState(doc, saved);
          path = saved.path;
        }
        patchSheet<ConvertSheet>("convert", { phase: "done", progress: 1, result: { report: out.report, path } });
        void get().refresh(doc);
      } catch (e) {
        const text = cancelled(e)
          ? "Cancelled. Nothing was changed."
          : report
            ? `The colours were converted in the open document, but saving the new file failed: ${msg(e)} Undo takes the conversion back.`
            : msg(e);
        patchSheet<ConvertSheet>("convert", { phase: "confirm", progress: 0, error: text });
        void api.editState(doc).then((state) => applyState(doc, state), () => {});
      }
    },

    cancelConvert: () => {
      const s = sheet<ConvertSheet>("convert");
      if (s?.phase === "running") void api.toolCancel(s.job).catch(() => {});
    },

    // ---- Printer's marks ----

    openMarks: (docId) => {
      const d = docOf(docId);
      if (!d) return;
      set({ sheet: { kind: "marks", doc: d.id, page: d.currentPage, form: DEFAULT_MARKS, boxes: null, preview: null, busy: false, error: null, done: null } });
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
        if (sheet<MarksSheet>("marks")) patchSheet<MarksSheet>("marks", { preview: null, error: msg(e) });
      }
    },

    applyMarks: async () => {
      const s = sheet<MarksSheet>("marks");
      const d = s && st().docs[s.doc];
      if (!s || !d || s.busy) return;
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
      void (async () => {
        try {
          const boxes = await prepressApi.pageBoxes(d.id, page);
          const s = sheet<BoxesSheet>("boxes");
          if (s && s.doc === d.id) patchSheet<BoxesSheet>("boxes", { boxes, margins: currentMargins(boxes, s.box, s.ref) });
        } catch (e) {
          patchSheet<BoxesSheet>("boxes", { error: msg(e) });
        }
      })();
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
      const rect = newBox(s);
      const problem = rect && boxProblem(s.box, rect, s.boxes);
      if (problem) return patchSheet<BoxesSheet>("boxes", { error: problem });
      const pages = scopePages(s.scope, s.range, d.pageCount, s.page);
      if (pages === null) return patchSheet<BoxesSheet>("boxes", { error: "The page range is not valid." });
      patchSheet<BoxesSheet>("boxes", { busy: true, error: null, done: null });
      try {
        // each page measures the margins from its own reference box
        const spec = {
          ...(pages ? { pages } : {}),
          kind: s.box,
          from: s.ref,
          margin: userMargins(s.boxes.rotate, s.margins, isOutward(s.box, s.ref)),
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

    closeSheet: () => {
      const s = get().sheet;
      if (!s) return;
      if (s.kind === "convert" && s.phase === "running") return;
      if ((s.kind === "marks" || s.kind === "boxes") && s.busy) return;
      set({ sheet: null });
    },

    forget: (doc) => {
      waiting.delete(doc);
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
  const sh = usePrepress.getState().sheet;
  if (sh && !s.docs[sh.doc]) usePrepress.getState().forget(sh.doc);
});

/** Opens a tool on the active document (the tools list and the palette). */
export const openOutputPreview = () => usePrepress.getState().setOpen(true);
export const openConvertColors = () => usePrepress.getState().openConvert();
export const openPrinterMarks = () => usePrepress.getState().openMarks();
export const openPageBoxes = () => usePrepress.getState().openBoxes();

export { onlyPlate };
