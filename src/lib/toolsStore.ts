import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import { useAi } from "./aiStore";
import type { CompressAuditReply, CompressEstimate, CompressPreset, CompressReport, OcrBackend, OcrDetection, OcrFont, OcrLanguages, OcrOutcome, OcrPlan, OcrStyle, OcrUnsure } from "./api";
import { initialLanguages, previewPage, requestOptions, toggleLanguage } from "./ocrLogic";
import { parseRange } from "./pageRange";
import * as edits from "./edit";

/**
 * Compress and OCR share one sheet that changes state in place: options, running, done (or error).
 * The sheet is UI-agnostic state; components render it.
 */

/** Save dialog (shown by Rust) next to the document; a dismissed one cancels the run. */
async function askSaveTo(doc: number, name: string) {
  const to = await api.pickSave(name, { doc });
  if (!to) throw "cancelled";
  return to.token;
}

export type ToolKind = "compress" | "ocr";
export type Phase = "options" | "running" | "done" | "error";
export type OcrScope = "all" | "page" | "range";

export interface ToolSettings {
  /** What OCR was last run with: primary tags in priority order; null = never chosen. */
  ocrLanguages: string[] | null;
  ocrStyle: OcrStyle;
  ocrDpi: number;
  /** Leave pages that already have text alone. */
  ocrSkipText: boolean;
  /** The face editable text is set in. */
  ocrFont: OcrFont;
  compressPreset: CompressPreset;
}

const KEY = "inkwell.tools";
const DEFAULTS: ToolSettings = { ocrLanguages: null, ocrStyle: "searchable", ocrDpi: 200, ocrSkipText: true, ocrFont: "auto", compressPreset: "balanced" };

export function loadToolSettings(): ToolSettings {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    // The single language an older version kept.
    if (!raw.ocrLanguages && typeof raw.ocrLanguage === "string") raw.ocrLanguages = [raw.ocrLanguage.split("-")[0].toLowerCase()];
    const { ocrLanguage: _old, ocrBackend: _backend, ...rest } = raw;
    return { ...DEFAULTS, ...rest };
  } catch {
    return DEFAULTS;
  }
}

/** The before/after of one page for the chosen style. */
export interface PreviewState {
  phase: "loading" | "ready" | "error";
  /** 0-based. */
  page: number;
  before: string | null;
  after: string | null;
  message: string | null;
  converted: number;
  kept: number;
  job: number;
}

export interface Sheet {
  tool: ToolKind;
  doc: number;
  phase: Phase;
  /** 0..1 */
  progress: number;
  /** "Page 8 of 12" */
  status: string;
  error: string | null;
  // compress
  preset: CompressPreset;
  audit: CompressAuditReply | null;
  /** Sample-based sizes per preset; null while they are worked out (or when that failed). */
  estimates: CompressEstimate[] | null;
  /** Job id of the running estimate, so closing the rail can stop it. */
  estimateJob: number;
  compressed: CompressReport | null;
  // ocr
  scope: OcrScope;
  range: string;
  /** Primary tags in priority order. */
  languages: string[];
  /** The user changed the selection: a late detection does not replace it. */
  languagesTouched: boolean;
  backend: OcrBackend;
  style: OcrStyle;
  dpi: number;
  skipText: boolean;
  font: OcrFont;
  preview: PreviewState | null;
  available: OcrLanguages | null;
  detection: OcrDetection | null;
  detecting: boolean;
  plan: OcrPlan | null;
  recognized: OcrOutcome | null;
  /** 0-based pages finished so far in an OCR run (the thumbnails tick them). */
  donePages: number[];
  /** The document had edits when the sheet opened: compress saves them first. */
  needsSave: boolean;
}

interface ToolsStore {
  sheet: Sheet | null;
  settings: ToolSettings;
  open: (tool: ToolKind, doc?: number, opts?: { scope?: OcrScope; range?: string }) => void;
  close: () => void;
  set: (p: Partial<Sheet>) => void;
  setSettings: (p: Partial<ToolSettings>) => void;
  /** File picker for tesseract.exe (shown by the Rust side), then the languages are read again. */
  chooseTesseract: () => Promise<void>;
  /** Adds or removes a language of the OCR selection and works out the engine for it. */
  toggleLanguage: (tag: string) => void;
  /** Opens Windows' language settings (where a language pack is added). */
  openLanguageSettings: () => void;
  /** Reads one page and shows it before and after the chosen style, changing nothing. */
  showPreview: () => Promise<void>;
  closePreview: () => void;
  /** Scrolls to a word the editable-text run left in the picture. */
  reviewWord: (w: OcrUnsure) => void;
  start: (opts?: { saveAs?: boolean }) => Promise<void>;
  cancel: () => void;
  /** Undoes an in-place OCR run (it is one undo step). */
  undo: () => Promise<void>;
  openOutput: () => Promise<void>;
}

export const FRIENDLY: Record<string, string> = {
  "output-exists": "A file with that name already exists. Pick another name.",
  "needs-password": "This file needs its password.",
  "bad-password": "That password is not right.",
  "invalid-options": "Those settings are not valid.",
  "needs-permissions-password": "This file doesn't allow copying or page changes. Open it with its permissions password, or remove its security first.",
  "language-unavailable": "No installed OCR engine reads that language. The language list says how to add it.",
  "tesseract-not-found": "Tesseract was not found. Choose its location in the OCR options.",
  "signed-document": "This document is signed. Straightening or replacing its pictures would break the signature, so it was left alone. The searchable image style adds its text layer without touching the pictures.",
  stale: "The document changed while it was being read. Run it again.",
};

export const friendly = (e: unknown) => {
  const s = String(e);
  return FRIENDLY[s] ?? s;
};

let jobSeq = 0;
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const st = () => useStore.getState();

const blank = (tool: ToolKind, doc: number): Sheet => ({
  tool,
  doc,
  phase: "options",
  progress: 0,
  status: "",
  error: null,
  preset: "balanced",
  audit: null,
  estimates: null,
  estimateJob: 0,
  compressed: null,
  scope: "all",
  range: "",
  languages: [],
  languagesTouched: false,
  backend: "auto",
  style: "searchable",
  dpi: 200,
  skipText: true,
  font: "auto",
  preview: null,
  available: null,
  detection: null,
  detecting: false,
  plan: null,
  recognized: null,
  donePages: [],
  needsSave: false,
});

export const useTools = create<ToolsStore>((set, get) => {
  const patch = (p: Partial<Sheet>) => set((s) => (s.sheet ? { sheet: { ...s.sheet, ...p } } : s));
  /** Applies `p` only while this job's sheet is still the open one. */
  const live = (job: number) => (p: Partial<Sheet>) => job === current && patch(p);
  let current = 0;

  async function loadAudit(sheet: Sheet) {
    try {
      const audit = await api.compressAudit(sheet.doc);
      if (get().sheet?.doc !== sheet.doc || get().sheet?.tool !== "compress") return;
      patch({ audit });
    } catch (e) {
      patch({ phase: "error", error: friendly(e) });
      return;
    }
    // The sizes take longer than the audit: they arrive on their own, the panel shows "…" until then.
    const job = ++jobSeq;
    patch({ estimateJob: job });
    try {
      const estimates = await api.compressEstimate(job, sheet.doc);
      if (get().sheet?.estimateJob === job) patch({ estimates });
    } catch {
      /* cancelled, or a file the sampler cannot read: no prediction beats a wrong one */
    }
  }

  /** Stops a running estimate: the rail closed, or the real job is about to need the CPU. */
  const stopEstimate = () => {
    const job = get().sheet?.estimateJob;
    if (job) void api.toolCancel(job).catch(() => {});
  };

  const ocrOpen = (doc: number) => get().sheet?.doc === doc && get().sheet?.tool === "ocr";

  /** Works out which engine reads `languages` and what is missing; only the latest answer counts. */
  async function refreshPlan(doc: number, languages: string[]) {
    if (languages.length === 0) {
      if (ocrOpen(doc)) patch({ plan: null });
      return;
    }
    try {
      const plan = await api.ocrPlan(languages, "auto");
      const cur = get().sheet;
      if (cur && ocrOpen(doc) && cur.languages.join() === languages.join()) patch({ plan });
    } catch {
      /* the sheet keeps its last plan */
    }
  }

  async function loadLanguages(sheet: Sheet) {
    const s = get().settings;
    try {
      const available = await api.ocrLanguages();
      if (!ocrOpen(sheet.doc)) return;
      const languages = initialLanguages({ detection: null, saved: s.ocrLanguages, defaults: available.defaults, catalog: available.catalog });
      patch({ available, languages, detecting: true });
      void refreshPlan(sheet.doc, languages);
      // What the document is written in decides the default, unless the user already chose.
      try {
        const detection = await api.ocrDetect(sheet.doc);
        if (!ocrOpen(sheet.doc)) return;
        const cur = get().sheet!;
        if (cur.languagesTouched) {
          patch({ detection, detecting: false });
          return;
        }
        const next = initialLanguages({ detection, saved: s.ocrLanguages, defaults: available.defaults, catalog: available.catalog });
        patch({ detection, detecting: false, languages: next });
        void refreshPlan(sheet.doc, next);
      } catch {
        if (ocrOpen(sheet.doc)) patch({ detecting: false });
      }
    } catch (e) {
      patch({ phase: "error", error: friendly(e) });
    }
  }

  return {
    sheet: null,
    settings: loadToolSettings(),

    open: (tool, docId, opts) => {
      const doc = docId ?? st().active;
      const d = doc != null ? st().docs[doc] : undefined;
      if (!d) return;
      // One run at a time: opening again while one runs keeps that sheet.
      if (get().sheet?.phase === "running") return;
      const s = get().settings;
      const sheet: Sheet = {
        ...blank(tool, d.id),
        preset: s.compressPreset,
        style: s.ocrStyle,
        dpi: s.ocrDpi,
        skipText: s.ocrSkipText,
        font: s.ocrFont,
        needsSave: d.dirty,
        scope: opts?.scope ?? "all",
        range: opts?.range ?? "",
      };
      // One right-side panel at a time: the rail replaces the assistant; Organize has no rail.
      useAi.getState().setOpen(false);
      if (d.organize) st().patch(d.id, { organize: false });
      stopEstimate();
      set({ sheet });
      void (tool === "compress" ? loadAudit(sheet) : loadLanguages(sheet));
    },

    close: () => {
      if (get().sheet?.phase === "running") get().cancel();
      stopEstimate();
      set({ sheet: null });
    },

    set: (p) => {
      // A preview is of one set of choices: any change to them makes it stale.
      if ("style" in p || "dpi" in p || "font" in p || "scope" in p || "range" in p) get().closePreview();
      patch(p);
    },

    setSettings: (p) => {
      const settings = { ...get().settings, ...p };
      localStorage.setItem(KEY, JSON.stringify(settings));
      set({ settings });
    },

    chooseTesseract: async () => {
      const sheet = get().sheet;
      if (!sheet || (await api.ocrChooseTesseract().catch(() => null)) == null) return;
      await loadLanguages(sheet);
    },

    toggleLanguage: (tag) => {
      const sheet = get().sheet;
      if (!sheet) return;
      const languages = toggleLanguage(sheet.languages, tag);
      get().closePreview();
      patch({ languages, languagesTouched: true });
      void refreshPlan(sheet.doc, languages);
    },

    openLanguageSettings: () => {
      void api.openLanguageSettings().catch(() => {});
    },

    showPreview: async () => {
      const sheet = get().sheet;
      if (!sheet || sheet.tool !== "ocr" || sheet.phase !== "options") return;
      const d = st().docs[sheet.doc];
      if (!d) return;
      get().closePreview();
      const job = ++jobSeq;
      const pages = sheet.scope === "range" ? parseRange(sheet.range, d.pageCount) : sheet.scope === "page" ? [d.currentPage] : null;
      const page = previewPage(pages, d.currentPage);
      const base = { page, before: null, after: null, message: null, converted: 0, kept: 0, job };
      patch({ preview: { ...base, phase: "loading" } });
      const mine = () => get().sheet?.preview?.job === job;
      try {
        const p = await api.ocrPreview(job, page, {
          doc: d.id,
          languages: sheet.languages,
          backend: sheet.backend,
          ...requestOptions({ style: sheet.style, dpi: sheet.dpi, skipText: sheet.skipText }),
          ...(sheet.style === "editable" ? { font: sheet.font } : {}),
        });
        if (!mine()) return;
        const e = p.applied.editable;
        patch({ preview: { ...base, phase: "ready", before: p.before, after: p.after, converted: e?.words_converted ?? 0, kept: e?.words_kept ?? 0 } });
      } catch (e) {
        if (!mine()) return;
        if (String(e) === "cancelled") patch({ preview: null });
        else patch({ preview: { ...base, phase: "error", message: friendly(e) } });
      }
    },

    closePreview: () => {
      const p = get().sheet?.preview;
      if (!p) return;
      if (p.phase === "loading") void api.toolCancel(p.job).catch(() => {});
      patch({ preview: null });
    },

    reviewWord: (w) => {
      const doc = get().sheet?.doc;
      if (doc != null) st().goTo(doc, w.page, Math.max(0, w.bbox[1] - 80));
    },

    start: async (opts) => {
      const sheet = get().sheet;
      if (!sheet || sheet.phase === "running") return;
      const d = st().docs[sheet.doc];
      if (!d) return;
      stopEstimate();
      const job = ++jobSeq;
      current = job;
      const upd = live(job);
      patch({ phase: "running", progress: 0, status: "Starting", error: null, preview: null });
      try {
        if (sheet.tool === "compress") {
          // The engine reads the file on disk: pending edits are saved first (the button says so).
          if (d.dirty) {
            await edits.save(d.id);
            if (st().docs[d.id]?.dirty) throw new Error("Save the changes first.");
          }
          // A taken default name is asked for in a save dialog.
          const saveTo = sheet.audit?.outputExists ? await askSaveTo(d.id, baseName(sheet.audit.defaultOutput)) : undefined;
          const report = await api.compressStart(job, { doc: d.id, preset: sheet.preset, saveTo }, (p) =>
            upd({
              progress: p.fraction,
              status: p.stage === "images" && p.pages > 0 ? `Optimizing images, page ${p.page} of ${p.pages}` : STAGE[p.stage],
            }),
          );
          get().setSettings({ compressPreset: sheet.preset });
          upd({ phase: "done", progress: 1, compressed: report });
        } else {
          const cur = st().docs[d.id];
          const pages =
            sheet.scope === "page" ? String((cur?.currentPage ?? 0) + 1) : sheet.scope === "range" ? sheet.range : undefined;
          // The page thumbnails show which pages are done.
          if (st().sidebar !== "thumbs") st().setSidebar("thumbs");
          patch({ donePages: [] });
          const saveTo = opts?.saveAs ? await askSaveTo(d.id, `${baseName(d.path).replace(/\.pdf$/i, "")} (searchable).pdf`) : undefined;
          const out = await api.ocrStart(
            job,
            {
              doc: d.id,
              pages,
              languages: sheet.languages,
              backend: sheet.backend,
              ...requestOptions({ style: sheet.style, dpi: sheet.dpi, skipText: sheet.skipText }),
              ...(sheet.style === "editable" ? { font: sheet.font } : {}),
              saveTo,
            },
            (p) =>
              job === current &&
              set((s) =>
                s.sheet
                  ? {
                      sheet: {
                        ...s.sheet,
                        progress: p.total ? p.done / p.total : 0,
                        status: `Reading page ${p.page + 1} (${p.done} of ${p.total})`,
                        donePages: [...s.sheet.donePages, p.page],
                      },
                    }
                  : s,
              ),
          );
          if (out.state) edits.applyState(d.id, out.state);
          if (sheet.languages.length > 0) {
            get().setSettings({ ocrLanguages: sheet.languages, ocrStyle: sheet.style, ocrDpi: sheet.dpi, ocrSkipText: sheet.skipText, ocrFont: sheet.font });
          }
          upd({ phase: "done", progress: 1, recognized: out });
        }
      } catch (e) {
        const msg = String(e);
        // A cancel returns to the options, not to an error.
        if (msg === "cancelled") upd({ phase: "options", progress: 0, status: "" });
        else upd({ phase: "error", error: friendly(e) });
      }
    },

    cancel: () => {
      if (get().sheet?.phase !== "running") return;
      patch({ status: "Cancelling" });
      void api.toolCancel(current).catch(() => {});
    },

    undo: async () => {
      const s = get().sheet;
      if (!s?.recognized?.state) return;
      await edits.undo(s.doc);
      set({ sheet: null });
    },

    openOutput: async () => {
      const s = get().sheet;
      const path = s?.compressed?.output ?? s?.recognized?.output;
      if (!path) return;
      stopEstimate();
      set({ sheet: null });
      await st().openPath(path);
    },
  };
});

// The assistant opening closes an idle rail; a running tool keeps the right side to itself.
useAi.subscribe((s, prev) => {
  if (!s.open || prev.open) return;
  const t = useTools.getState();
  if (t.sheet?.phase === "running") useAi.setState({ open: false });
  else if (t.sheet) t.close();
});

const STAGE: Record<string, string> = {
  open: "Opening",
  scan: "Looking at what is in the file",
  images: "Optimizing images",
  fonts: "Subsetting fonts",
  write: "Writing the new file",
  verify: "Checking the result",
};

/** "8.4 MB", "612 KB" */
export function formatBytes(n: number): string {
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 0 : 1)} MB`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} KB`;
  return `${n} B`;
}

export const fileName = baseName;
