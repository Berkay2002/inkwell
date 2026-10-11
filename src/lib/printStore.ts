import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type {
  PaperInfo,
  PrintSavedPdf,
  PrintContent,
  PrintDuplex,
  PrintLayout,
  PrintOrientation,
  PrintPageOrder,
  PrintPlan,
  PrintRange,
  PrintReport,
  PrintSettings,
  PrintSizeMode,
  PrintSubset,
  PrinterCaps,
  PrinterInfo,
} from "./api";
import { friendly } from "./toolsStore";

/**
 * The Print sheet's state. `PrintOptions` is what the user sets (flat, so every control binds to one
 * field); `toSettings` turns it into what the engine takes. The engine plans the sheets and draws the
 * preview with the same code the printer path uses, so the store only asks (`refresh`) and shows.
 */

export type PrintScope = "all" | "current" | "range";
export type PrintMode = "size" | "multiple" | "booklet" | "poster";
export type PrintPhase = "options" | "running" | "done" | "error";

export interface PrintOptions {
  /** null: the system default printer. */
  printer: string | null;
  paperId: number | null;
  tray: number | null;
  orientation: PrintOrientation;
  scope: PrintScope;
  range: string;
  subset: PrintSubset;
  reverse: boolean;
  copies: number;
  collate: boolean;
  duplex: PrintDuplex;
  grayscale: boolean;
  mode: PrintMode;
  sizeMode: PrintSizeMode;
  percent: number;
  perSheet: number;
  order: PrintPageOrder;
  border: boolean;
  autoRotate: boolean;
  sides: "both" | "frontOnly" | "backOnly";
  binding: "left" | "right";
  /** Booklet creep in mm (0 to 3). */
  creepMm: number;
  /** Poster: tile scale in percent, overlap in mm (0 to 50), cut marks, labels. */
  tileScale: number;
  overlapMm: number;
  cutMarks: boolean;
  labels: boolean;
  content: PrintContent;
}

export const defaultOptions = (): PrintOptions => ({
  printer: null,
  paperId: null,
  tray: null,
  orientation: "auto",
  scope: "all",
  range: "",
  subset: "all",
  reverse: false,
  copies: 1,
  collate: true,
  duplex: "off",
  grayscale: false,
  mode: "size",
  sizeMode: "fit",
  percent: 100,
  perSheet: 2,
  order: "horizontal",
  border: false,
  autoRotate: true,
  sides: "both",
  binding: "left",
  creepMm: 0,
  tileScale: 100,
  overlapMm: 6,
  cutMarks: false,
  labels: false,
  content: "documentAndMarkups",
});

/** Acrobat's pages-per-sheet list, plus the custom grids the engine takes. */
export const PER_SHEET = [2, 4, 6, 9, 16] as const;

const A4: PaperInfo = { id: 9, name: "A4", width: 595.28, height: 841.89 };

/** What there is to choose from with no printer installed: Save as PDF still needs a paper size. */
const STANDARD_PAPERS: PaperInfo[] = [
  A4,
  { id: 1, name: "Letter", width: 612, height: 792 },
  { id: 5, name: "Legal", width: 612, height: 1008 },
  { id: 8, name: "A3", width: 841.89, height: 1190.55 },
  { id: 11, name: "A5", width: 419.53, height: 595.28 },
];

/** The printer's papers; the standard ones when there is no printer at all (not when its driver is silent). */
export function paperList(caps: PrinterCaps | null, printer: string | null): PaperInfo[] {
  return caps ? caps.papers : printer ? [] : STANDARD_PAPERS;
}

const MM = 72 / 25.4;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, Number.isFinite(v) ? v : lo));

/** The paper the sheet starts on: the one that matches the first page, else A4, else the driver's first. */
export function pickPaper(papers: PaperInfo[], first: { w: number; h: number } | null): PaperInfo | null {
  if (first) {
    const [w, h] = first.w <= first.h ? [first.w, first.h] : [first.h, first.w];
    const m = papers.find((p) => Math.abs(p.width - w) < 3 && Math.abs(p.height - h) < 3);
    if (m) return m;
  }
  return papers.find((p) => p.id === 9) ?? papers[0] ?? null;
}

export function toSettings(o: PrintOptions, caps: PrinterCaps | null, current: number): PrintSettings {
  const paper = paperList(caps, o.printer).find((p) => p.id === o.paperId) ?? null;
  const range: PrintRange =
    o.scope === "current" ? { kind: "current", page: current } : o.scope === "range" ? { kind: "pages", text: o.range } : { kind: "all" };
  const layout: PrintLayout =
    o.mode === "multiple"
      ? { kind: "multiple", perSheet: o.perSheet, order: o.order, border: o.border, autoRotate: o.autoRotate }
      : o.mode === "booklet"
        ? { kind: "booklet", sides: o.sides, binding: o.binding, creep: clamp(o.creepMm, 0, 3) * MM }
        : o.mode === "poster"
          ? { kind: "poster", scale: clamp(o.tileScale, 1, 10000), overlap: clamp(o.overlapMm, 0, 50) * MM, cutMarks: o.cutMarks, labels: o.labels }
          : { kind: "size", mode: o.sizeMode, percent: o.percent };
  return {
    printer: o.printer,
    paper: paper ? { id: paper.id, width: paper.width, height: paper.height } : { id: null, width: A4.width, height: A4.height },
    tray: o.tray,
    orientation: o.orientation,
    range,
    subset: o.subset,
    reverse: o.reverse,
    copies: Math.max(1, Math.min(caps?.maxCopies && caps.maxCopies > 1 ? caps.maxCopies : 999, Math.round(o.copies) || 1)),
    collate: o.collate,
    // A booklet of only the fronts or only the backs goes through the printer a side at a time.
    duplex: (caps && !caps.duplex) || (o.mode === "booklet" && o.sides !== "both") ? "off" : o.duplex,
    // the toggle is off without a printer, and a remembered grey must not tint the preview of a file that keeps colour
    grayscale: o.printer ? o.grayscale : false,
    layout,
    content: o.content,
  };
}

/** The promise above the Print button, in the words a person would say: "6 sheets of A4, both sides, 2 copies". */
export function outcomeLine(plan: PrintPlan | null, o: PrintOptions, caps: PrinterCaps | null): string {
  if (!plan) return "";
  const paper = paperList(caps, o.printer).find((p) => p.id === o.paperId)?.name.replace(/\s*\(.*$/, "");
  const s = toSettings(o, caps, 0);
  const parts = [`${plan.papers} ${plan.papers === 1 ? "sheet" : "sheets"} of ${paper || "paper"}`];
  if (s.duplex !== "off") parts.push("both sides");
  if (s.copies > 1) parts.push(`${s.copies} copies`);
  if (s.grayscale) parts.push("grayscale");
  if (o.mode === "poster") {
    // the same grid on every page when the pages are alike, which is the case worth stating
    const t = plan.faces[0]?.tile;
    if (t && plan.faces.every((f) => f.tile?.cols === t.cols && f.tile?.rows === t.rows)) parts.push(`${t.cols} × ${t.rows} tiles per page`);
  }
  if (o.mode === "booklet") {
    const pages = Math.ceil(plan.pages / 4) * 4;
    parts.push(pages > plan.pages ? `${pages} pages, ${pages - plan.pages} blank` : `${pages} pages`);
    // outermost sheet unshifted, innermost by the full amount
    if (s.layout.kind === "booklet" && s.layout.creep > 0 && pages > 4) parts.push(`creep up to ${(s.layout.creep / MM).toFixed(1)} mm at the centre`);
  }
  return parts.join(", ");
}

/** What the sheet remembers between runs: the choices people set once, not the page range or copies. */
const REMEMBERED = [
  "printer",
  "mode",
  "sizeMode",
  "perSheet",
  "order",
  "border",
  "autoRotate",
  "sides",
  "binding",
  "creepMm",
  "tileScale",
  "overlapMm",
  "cutMarks",
  "labels",
  "content",
  "grayscale",
  "collate",
  "duplex",
  "orientation",
] as const;
const KEY = "inkwell.print";

export function loadRemembered(): Partial<PrintOptions> {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Record<string, unknown>;
    const base = defaultOptions() as unknown as Record<string, unknown>;
    const out: Record<string, unknown> = {};
    for (const k of REMEMBERED) if (k in raw && typeof raw[k] === typeof base[k]) out[k] = raw[k];
    // a printer that was null (the default) is stored as a missing key
    if (typeof raw.printer === "string") out.printer = raw.printer;
    return out as Partial<PrintOptions>;
  } catch {
    return {};
  }
}

function remember(o: PrintOptions) {
  try {
    const out: Record<string, unknown> = {};
    for (const k of REMEMBERED) out[k] = o[k];
    localStorage.setItem(KEY, JSON.stringify(out));
  } catch {
    /* private mode or full: the sheet just forgets */
  }
}

export interface PrintSheetState {
  doc: number;
  name: string;
  pageCount: number;
  currentPage: number;
  /** Size of the first page in points, for choosing paper. */
  first: { w: number; h: number } | null;
  options: PrintOptions;
  printers: PrinterInfo[];
  fileDirSet: boolean;
  caps: PrinterCaps | null;
  plan: PrintPlan | null;
  /** The engine's message when the options do not make a plan (a bad range). */
  planError: string | null;
  /** Side shown in the preview (0-based). */
  face: number;
  /** Object URL of the preview PNG; revoked when replaced. */
  previewUrl: string | null;
  previewing: boolean;
  /** Width in CSS px of the box the preview fills; the engine draws the sheet at that size x DPR. */
  previewPx: number;
  phase: PrintPhase;
  progress: { done: number; total: number };
  error: string | null;
  report: PrintReport | null;
  /** The file a print-to-file printer wrote (the report's destination), for the done card. */
  savedAs: string | null;
  /** What the running job is: sending to the printer, or writing the plan as a PDF. */
  job: "print" | "pdf";
  /** Save as PDF's result, for the done card. */
  saved: PrintSavedPdf | null;
}

interface PrintStore {
  sheet: PrintSheetState | null;
  open: (doc?: number) => void;
  close: () => void;
  set: (p: Partial<PrintOptions>) => void;
  setFace: (n: number) => void;
  setPreviewPx: (px: number) => void;
  start: () => Promise<void>;
  /** Writes the plan as a new PDF (asks where) and offers it in a new tab. */
  savePdf: () => Promise<void>;
  openSaved: () => Promise<void>;
  cancel: () => void;
  /** From a failure card back to the options. */
  back: () => void;
}

const st = () => useStore.getState();
const stem = (n: string) => n.replace(/\.[^.]+$/, "");

let jobSeq = 0;
let curJob = 0;
let refreshTimer: ReturnType<typeof setTimeout> | undefined;
let refreshSeq = 0;

export const usePrint = create<PrintStore>((set, get) => {
  const patch = (p: Partial<PrintSheetState>) => set((s) => (s.sheet ? { sheet: { ...s.sheet, ...p } } : s));

  const settingsOf = (s: PrintSheetState) => toSettings(s.options, s.caps, s.currentPage);

  /** Plan and preview for the current options. Debounced; an older answer never overwrites a newer one. */
  function refresh(now = false) {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(run, now ? 0 : 140);
  }

  async function run() {
    const s = get().sheet;
    if (!s) return;
    const seq = ++refreshSeq;
    const live = () => seq === refreshSeq && get().sheet?.doc === s.doc;
    const settings = settingsOf(s);
    let plan: PrintPlan;
    try {
      plan = await api.printPlan(s.doc, settings);
    } catch (e) {
      if (live()) patch({ plan: null, planError: friendly(e), previewing: false });
      return;
    }
    if (!live()) return;
    const face = Math.min(get().sheet!.face, Math.max(0, plan.sheets - 1));
    patch({ plan, planError: null, face, previewing: true });
    try {
      const px = Math.round(get().sheet!.previewPx * (globalThis.devicePixelRatio || 1));
      const png = await api.printPreview(s.doc, settings, face, px);
      if (!live()) return;
      const url = URL.createObjectURL(new Blob([png], { type: "image/png" }));
      const old = get().sheet?.previewUrl;
      patch({ previewUrl: url, previewing: false });
      if (old) URL.revokeObjectURL(old);
    } catch (e) {
      if (!live()) return;
      // an edit landed between the plan and the drawing: ask again once
      if (String(e) === "stale") refresh(true);
      else patch({ previewing: false, planError: friendly(e) });
    }
  }

  async function loadPrinters(doc: number) {
    try {
      const list = await api.printPrinters();
      if (get().sheet?.doc !== doc) return;
      const cur = get().sheet!.options.printer;
      const usable = cur != null && list.printers.some((p) => p.name === cur);
      const name = usable ? cur : (list.printers.find((p) => p.isDefault) ?? list.printers[0])?.name ?? null;
      patch({ printers: list.printers, fileDirSet: list.fileDirSet });
      await loadCaps(doc, name);
    } catch (e) {
      patch({ phase: "error", error: friendly(e) });
    }
  }

  async function loadCaps(doc: number, printer: string | null) {
    let caps: PrinterCaps | null = null;
    if (printer) {
      try {
        caps = await api.printCapabilities(printer);
      } catch {
        caps = null;
      }
    }
    const s = get().sheet;
    if (!s || s.doc !== doc) return;
    const papers = paperList(caps, printer);
    const paper = papers.find((p) => p.id === s.options.paperId) ?? pickPaper(papers, s.first);
    const tray = caps?.trays.some((t) => t.id === s.options.tray) ? s.options.tray : null;
    patch({ caps, options: { ...s.options, printer, paperId: paper?.id ?? null, tray } });
    refresh(true);
  }

  return {
    sheet: null,

    open: (docId) => {
      const id = docId ?? st().active;
      const d = id != null ? st().docs[id] : undefined;
      if (!d) return;
      if (get().sheet?.phase === "running") return;
      const options = { ...defaultOptions(), ...loadRemembered() };
      const sz = d.sizes[0];
      const first = sz ? { w: sz[0], h: sz[1] } : null;
      set({
        sheet: {
          doc: d.id,
          name: d.name,
          pageCount: d.pageCount,
          currentPage: d.currentPage,
          first,
          options,
          printers: [],
          fileDirSet: false,
          caps: null,
          plan: null,
          planError: null,
          face: 0,
          previewUrl: null,
          previewing: false,
          previewPx: 520,
          phase: "options",
          progress: { done: 0, total: 0 },
          error: null,
          report: null,
          savedAs: null,
          job: "print",
          saved: null,
        },
      });
      void loadPrinters(d.id);
    },

    close: () => {
      const s = get().sheet;
      if (!s) return;
      if (s.phase === "running") get().cancel();
      clearTimeout(refreshTimer);
      refreshSeq++;
      if (s.previewUrl) URL.revokeObjectURL(s.previewUrl);
      set({ sheet: null });
    },

    set: (p) => {
      const s = get().sheet;
      if (!s || s.phase === "running") return;
      const options = { ...s.options, ...p };
      // Changing the printer reads its capabilities (paper, trays, two-sided) before the preview.
      patch({ options, ...(p.scope != null || p.range != null || p.subset != null || p.reverse != null ? { face: 0 } : {}) });
      if (p.printer !== undefined && p.printer !== s.options.printer) {
        void loadCaps(s.doc, p.printer);
      } else {
        refresh();
      }
      remember(options);
    },

    setFace: (n) => {
      const s = get().sheet;
      if (!s || !s.plan) return;
      patch({ face: Math.max(0, Math.min(s.plan.sheets - 1, n)) });
      refresh(true);
    },

    setPreviewPx: (px) => {
      const s = get().sheet;
      if (!s || Math.abs(px - s.previewPx) < 24) return;
      patch({ previewPx: px });
      refresh();
    },

    start: async () => {
      const s = get().sheet;
      if (!s || s.phase === "running" || !s.plan || s.planError) return;
      const printer = s.printers.find((p) => p.name === s.options.printer);
      let saveTo: string | null = null;
      if (printer?.toFile && !s.fileDirSet) {
        const pdf = /pdf/i.test(printer.name);
        const ref = await api.pickSave(`${stem(s.name)}${pdf ? ".pdf" : ""}`, { doc: s.doc }, pdf ? "pdf" : "any");
        if (!ref) return;
        saveTo = ref.token;
        patch({ savedAs: ref.path });
      }
      const job = (curJob = ++jobSeq);
      patch({ phase: "running", job: "print", error: null, progress: { done: 0, total: s.plan.sides }, report: null, saved: null });
      try {
        const report = await api.printStart(job, s.doc, settingsOf(s), saveTo, (p) => {
          if (get().sheet?.doc === s.doc) patch({ progress: p });
        });
        if (get().sheet?.doc === s.doc) patch({ phase: "done", report });
      } catch (e) {
        if (get().sheet?.doc !== s.doc) return;
        if (String(e) === "cancelled") patch({ phase: "options" });
        else patch({ phase: "error", error: friendly(e) });
      }
    },

    savePdf: async () => {
      const s = get().sheet;
      if (!s || s.phase === "running" || !s.plan || s.planError) return;
      const o = s.options;
      const tag = o.mode === "poster" ? "poster" : o.mode === "booklet" ? "booklet" : o.mode === "multiple" ? `${o.perSheet}-up` : "print";
      const ref = await api.pickSave(`${stem(s.name)}-${tag}.pdf`, { doc: s.doc }, "pdf");
      if (!ref || get().sheet?.doc !== s.doc) return;
      const job = (curJob = ++jobSeq);
      patch({ phase: "running", job: "pdf", error: null, progress: { done: 0, total: s.plan.sheets }, report: null, saved: null });
      try {
        const saved = await api.printSavePdf(job, s.doc, settingsOf(s), ref.token, (p) => {
          if (get().sheet?.doc === s.doc) patch({ progress: p });
        });
        if (get().sheet?.doc === s.doc) patch({ phase: "done", saved });
      } catch (e) {
        if (get().sheet?.doc !== s.doc) return;
        if (String(e) === "cancelled") patch({ phase: "options" });
        else patch({ phase: "error", error: friendly(e) });
      }
    },

    openSaved: async () => {
      const f = get().sheet?.saved?.file;
      if (!f) return;
      get().close();
      await st().openPath(f.path, f.token);
    },

    cancel: () => {
      if (curJob) void api.toolCancel(curJob).catch(() => {});
    },

    back: () => patch({ phase: "options", error: null }),
  };
});
