import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import { useAi } from "./aiStore";
import type {
  CompressAuditReply,
  CompressEstimate,
  CompressOptions,
  CompressPreset,
  CompressReport,
  NamedCompressPreset,
  OcrBackend,
  OcrLanguages,
  OcrOutcome,
} from "./api";
import * as edits from "./edit";
import { forPreset, normalizeOptions, validateOptions } from "./optimizer";

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
/** The Compress sheet shows the three presets, or the Advanced settings (Acrobat's Optimized PDF). */
export type CompressMode = "presets" | "advanced";
export type Phase = "options" | "running" | "done" | "error";
export type OcrScope = "all" | "page" | "range";

export interface ToolSettings {
  /** BCP-47 tag of the language OCR starts with. */
  ocrLanguage: string | null;
  ocrBackend: OcrBackend;
  compressPreset: CompressPreset;
  compressMode: CompressMode;
  /** The Advanced settings as they were left (without the fonts picked for one document). */
  compressAdvanced: CompressOptions | null;
}

const KEY = "inkwell.tools";
const DEFAULTS: ToolSettings = {
  ocrLanguage: null,
  ocrBackend: "windows",
  compressPreset: "balanced",
  compressMode: "presets",
  compressAdvanced: null,
};

export function loadToolSettings(): ToolSettings {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
  } catch {
    return DEFAULTS;
  }
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
  mode: CompressMode;
  /** The Advanced settings; null until the engine's defaults have arrived. */
  advanced: CompressOptions | null;
  /** The engine's three presets, the starting points of the Advanced settings. */
  defaults: Record<CompressPreset, CompressOptions> | null;
  /** Saved settings by name. */
  saved: NamedCompressPreset[];
  /** The saved settings the sheet last loaded or saved (so a change can say "changed"). */
  savedName: string | null;
  savedError: string | null;
  /** Size of the Advanced settings; null while it is worked out. */
  customEstimate: CompressEstimate | null;
  customEstimateJob: number;
  // ocr
  scope: OcrScope;
  range: string;
  languages: string[];
  backend: OcrBackend;
  force: boolean;
  available: OcrLanguages | null;
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
  setMode: (m: CompressMode) => void;
  /** Changes the Advanced settings and asks for a new size. */
  setAdvanced: (fn: (o: CompressOptions) => CompressOptions) => void;
  /** Resets the Advanced settings to one of the three presets. */
  startFrom: (p: CompressPreset) => void;
  savePreset: (name: string) => Promise<void>;
  loadPreset: (name: string) => void;
  deletePreset: (name: string) => Promise<void>;
  /** File picker for tesseract.exe (shown by the Rust side), then the languages are read again. */
  chooseTesseract: () => Promise<void>;
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
  "language-unavailable": "That OCR language is not installed.",
  "tesseract-not-found": "Tesseract was not found. Set its location in the OCR options.",
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
  mode: "presets",
  advanced: null,
  defaults: null,
  saved: [],
  savedName: null,
  savedError: null,
  customEstimate: null,
  customEstimateJob: 0,
  scope: "all",
  range: "",
  languages: [],
  backend: "windows",
  force: false,
  available: null,
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
    // The Advanced settings start from the engine's own presets; saved settings come from its data folder.
    void loadAdvanced(sheet);
    // The sizes take longer than the audit: they arrive on their own, the panel shows "…" until then.
    const job = ++jobSeq;
    patch({ estimateJob: job });
    try {
      const estimates = await api.compressEstimate(job, sheet.doc);
      if (get().sheet?.estimateJob === job) patch({ estimates });
    } catch {
      /* cancelled, or a file the sampler cannot read: no prediction beats a wrong one */
    }
    if (get().sheet?.mode === "advanced") scheduleEstimate(0);
  }

  async function loadAdvanced(sheet: Sheet) {
    try {
      const [defaults, saved] = await Promise.all([api.compressPresetOptions(), api.compressPresets().catch(() => [])]);
      const cur = get().sheet;
      if (!cur || cur.doc !== sheet.doc || cur.tool !== "compress") return;
      const base = defaults[get().settings.compressPreset] ?? defaults.balanced;
      const kept = get().settings.compressAdvanced;
      patch({
        defaults,
        saved: saved.map((p) => ({ ...p, options: normalizeOptions(defaults.balanced, p.options) })),
        advanced: forPreset(kept ? normalizeOptions(base, kept) : base),
      });
    } catch {
      /* no engine defaults: the Advanced tab stays empty and the presets still work */
    }
  }

  let estimateTimer: ReturnType<typeof setTimeout> | undefined;
  /** The size of the Advanced settings, a moment after the last change (it recompresses a sample). */
  function scheduleEstimate(delay = 600) {
    clearTimeout(estimateTimer);
    const s = get().sheet;
    if (!s || s.tool !== "compress" || s.mode !== "advanced" || !s.advanced) return;
    if (s.customEstimateJob) void api.toolCancel(s.customEstimateJob).catch(() => {});
    if (validateOptions(s.advanced)) return void patch({ customEstimate: null, customEstimateJob: 0 });
    patch({ customEstimate: null });
    estimateTimer = setTimeout(async () => {
      const sheet = get().sheet;
      if (!sheet?.advanced || sheet.phase !== "options" || sheet.mode !== "advanced") return;
      const job = ++jobSeq;
      patch({ customEstimateJob: job });
      try {
        const customEstimate = await api.compressEstimateCustom(job, sheet.doc, sheet.advanced);
        if (get().sheet?.customEstimateJob === job) patch({ customEstimate });
      } catch {
        /* cancelled, or a file the sampler cannot read: no prediction beats a wrong one */
      }
    }, delay);
  }

  /** Stops a running estimate: the rail closed, or the real job is about to need the CPU. */
  const stopEstimate = () => {
    clearTimeout(estimateTimer);
    const sheet = get().sheet;
    for (const job of [sheet?.estimateJob, sheet?.customEstimateJob]) {
      if (job) void api.toolCancel(job).catch(() => {});
    }
  };

  async function loadLanguages(sheet: Sheet) {
    const s = get().settings;
    try {
      const available = await api.ocrLanguages();
      const tess = s.ocrBackend === "tesseract" && available.tesseract != null;
      const backend: OcrBackend = tess ? "tesseract" : "windows";
      const have = backend === "tesseract" ? available.tesseract!.languages : available.windows;
      const wanted = s.ocrLanguage && have.includes(s.ocrLanguage) ? s.ocrLanguage : have[0];
      if (get().sheet?.doc === sheet.doc && get().sheet?.tool === "ocr") {
        patch({ available, backend, languages: wanted ? [wanted] : [] });
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
        mode: s.compressMode ?? "presets",
        backend: s.ocrBackend,
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

    set: (p) => patch(p),

    setSettings: (p) => {
      const settings = { ...get().settings, ...p };
      localStorage.setItem(KEY, JSON.stringify(settings));
      set({ settings });
    },

    setMode: (mode) => {
      patch({ mode });
      get().setSettings({ compressMode: mode });
      if (mode === "advanced") scheduleEstimate(0);
      else {
        clearTimeout(estimateTimer);
        const job = get().sheet?.customEstimateJob;
        if (job) void api.toolCancel(job).catch(() => {});
      }
    },

    setAdvanced: (fn) => {
      const cur = get().sheet?.advanced;
      if (!cur) return;
      const next = { ...fn(cur), preset: "custom" as const };
      patch({ advanced: next, savedError: null });
      get().setSettings({ compressAdvanced: forPreset(next) });
      scheduleEstimate();
    },

    startFrom: (p) => {
      const base = get().sheet?.defaults?.[p];
      if (!base) return;
      patch({ savedName: null });
      get().setAdvanced(() => ({ ...base }));
    },

    savePreset: async (name) => {
      const o = get().sheet?.advanced;
      if (!o) return;
      try {
        const saved = await api.compressPresetSave(name, forPreset(o));
        const base = get().sheet?.defaults?.balanced;
        patch({
          saved: base ? saved.map((p) => ({ ...p, options: normalizeOptions(base, p.options) })) : saved,
          savedName: name.trim(),
          savedError: null,
        });
      } catch (e) {
        patch({ savedError: friendly(e) });
      }
    },

    loadPreset: (name) => {
      const sheet = get().sheet;
      const p = sheet?.saved.find((x) => x.name === name);
      if (!sheet || !p) return patch({ savedName: null });
      // Fonts are picked per document: the ones in this document stay as they were.
      patch({ savedName: p.name });
      get().setAdvanced((cur) => ({ ...p.options, unembedFonts: cur.unembedFonts }));
    },

    deletePreset: async (name) => {
      try {
        const saved = await api.compressPresetDelete(name);
        const base = get().sheet?.defaults?.balanced;
        patch({
          saved: base ? saved.map((p) => ({ ...p, options: normalizeOptions(base, p.options) })) : saved,
          savedName: get().sheet?.savedName === name ? null : (get().sheet?.savedName ?? null),
          savedError: null,
        });
      } catch (e) {
        patch({ savedError: friendly(e) });
      }
    },

    chooseTesseract: async () => {
      const sheet = get().sheet;
      if (!sheet || (await api.ocrChooseTesseract().catch(() => null)) == null) return;
      get().setSettings({ ocrBackend: "tesseract" });
      await loadLanguages(sheet);
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
      patch({ phase: "running", progress: 0, status: "Starting", error: null });
      try {
        if (sheet.tool === "compress") {
          // The engine reads the file on disk: pending edits are saved first (the button says so).
          if (d.dirty) {
            await edits.save(d.id);
            if (st().docs[d.id]?.dirty) throw new Error("Save the changes first.");
          }
          // A taken default name is asked for in a save dialog.
          const saveTo = sheet.audit?.outputExists ? await askSaveTo(d.id, baseName(sheet.audit.defaultOutput)) : undefined;
          // Advanced: the whole set of settings; presets: just the name, the engine knows the rest.
          const advanced = sheet.mode === "advanced" && sheet.advanced ? sheet.advanced : undefined;
          const req = advanced ? { doc: d.id, preset: sheet.preset, custom: advanced, saveTo } : { doc: d.id, preset: sheet.preset, saveTo };
          const report = await api.compressStart(job, req, (p) =>
            upd({
              progress: p.fraction,
              status:
                p.stage === "images" && p.pages > 0
                  ? `Optimizing images, page ${p.page} of ${p.pages}`
                  : p.stage === "flatten" && p.pages > 0
                    ? `Flattening transparency, ${p.page} of ${p.pages} pages`
                    : STAGE[p.stage],
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
              force: sheet.force,
              backend: sheet.backend,
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
          const first = sheet.languages[0];
          if (first) get().setSettings({ ocrLanguage: first, ocrBackend: sheet.backend });
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
  flatten: "Flattening transparency",
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
