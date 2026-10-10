import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import { useAi } from "./aiStore";
import type { CompressAuditReply, CompressPreset, CompressReport, OcrBackend, OcrLanguages, OcrOutcome } from "./api";
import * as edits from "./edit";

/**
 * Compress and OCR share one sheet that changes state in place: options, running, done (or error).
 * The sheet is UI-agnostic state; components render it.
 */

export type ToolKind = "compress" | "ocr";
export type Phase = "options" | "running" | "done" | "error";
export type OcrScope = "all" | "page" | "range";

export interface ToolSettings {
  /** BCP-47 tag of the language OCR starts with. */
  ocrLanguage: string | null;
  ocrBackend: OcrBackend;
  compressPreset: CompressPreset;
}

const KEY = "inkwell.tools";
const DEFAULTS: ToolSettings = { ocrLanguage: null, ocrBackend: "windows", compressPreset: "balanced" };

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
  compressed: CompressReport | null;
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
  compressed: null,
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
      if (get().sheet?.doc === sheet.doc && get().sheet?.tool === "compress") patch({ audit });
    } catch (e) {
      patch({ phase: "error", error: friendly(e) });
    }
  }

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
        backend: s.ocrBackend,
        needsSave: d.dirty,
        scope: opts?.scope ?? "all",
        range: opts?.range ?? "",
      };
      // One right-side panel at a time: the rail replaces the assistant; Organize has no rail.
      useAi.getState().setOpen(false);
      if (d.organize) st().patch(d.id, { organize: false });
      set({ sheet });
      void (tool === "compress" ? loadAudit(sheet) : loadLanguages(sheet));
    },

    close: () => {
      if (get().sheet?.phase === "running") get().cancel();
      set({ sheet: null });
    },

    set: (p) => patch(p),

    setSettings: (p) => {
      const settings = { ...get().settings, ...p };
      localStorage.setItem(KEY, JSON.stringify(settings));
      set({ settings });
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
          // A taken default name is asked for in a dialog (the Rust side shows it).
          const chooseOutput = sheet.audit?.outputExists ?? false;
          const report = await api.compressStart(job, { doc: d.id, preset: sheet.preset, chooseOutput }, (p) =>
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
          const out = await api.ocrStart(
            job,
            {
              doc: d.id,
              pages,
              languages: sheet.languages,
              force: sheet.force,
              backend: sheet.backend,
              saveAs: opts?.saveAs,
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
  else if (t.sheet) useTools.setState({ sheet: null });
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
