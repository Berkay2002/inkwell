import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { CreatePageSize, CreateRequest, ExportKind, ExportOutput, FileRef, ImageFormat, SaveKind } from "./api";
import { useAi } from "./aiStore";
import { parseRange } from "./pageRange";
import { friendly, useTools } from "./toolsStore";

/**
 * Export (pages to images, text, HTML, embedded images, Word) and Create PDF (from images, text, blank,
 * the clipboard) share one rail sheet, like Compress and OCR do in `useTools`. UI-agnostic state;
 * `ConvertPanel` renders it.
 */

export type ConvertMode = "export" | "create";
export type ExportWhat = "images" | "text" | "html" | "extractImages" | "word";
export type CreateFrom = "images" | "text" | "blank" | "clipboard";
export type Scope = "all" | "page" | "range";
export type Phase = "options" | "running" | "done" | "error";

export interface ConvertSheet {
  mode: ConvertMode;
  /** The document an export reads; null for Create. */
  doc: number | null;
  phase: Phase;
  progress: number;
  status: string;
  error: string | null;
  // export
  what: ExportWhat;
  imageFormat: ImageFormat;
  dpi: number;
  gray: boolean;
  quality: number;
  layout: boolean;
  xhtml: boolean;
  scope: Scope;
  range: string;
  // create
  from: CreateFrom;
  files: FileRef[];
  textFile: FileRef | null;
  blankPages: number;
  pageSize: CreatePageSize;
  margin: number;
  // result
  exported: ExportOutput | null;
  created: { pages: number; name: string; warnings: string[] } | null;
}

interface ConvertStore {
  sheet: ConvertSheet | null;
  openExport: (doc?: number, what?: ExportWhat) => void;
  openCreate: (from?: CreateFrom) => void;
  close: () => void;
  set: (p: Partial<ConvertSheet>) => void;
  addImages: () => Promise<void>;
  chooseText: () => Promise<void>;
  /** Moves image `i` by `d` places, or drops it when `d` is null. */
  moveImage: (i: number, d: -1 | 1 | null) => void;
  start: () => Promise<void>;
  cancel: () => void;
}

const st = () => useStore.getState();
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const stem = (p: string) => baseName(p).replace(/\.[^.]+$/, "");

const blank = (mode: ConvertMode, doc: number | null): ConvertSheet => ({
  mode,
  doc,
  phase: "options",
  progress: 0,
  status: "",
  error: null,
  what: "images",
  imageFormat: "png",
  dpi: 150,
  gray: false,
  quality: 85,
  layout: false,
  xhtml: false,
  scope: "all",
  range: "",
  from: "images",
  files: [],
  textFile: null,
  blankPages: 1,
  pageSize: "fit",
  margin: 0,
  exported: null,
  created: null,
});

/** The pages option as the engine reads it: undefined means every page. */
export function pagesOf(s: ConvertSheet, current: number): string | undefined {
  return s.scope === "page" ? String(current + 1) : s.scope === "range" ? s.range : undefined;
}

/** One file per page: the destination is a folder, not a file. */
export const isFolderExport = (w: ExportWhat) => w === "images" || w === "extractImages";

let jobSeq = 0;

export const useConvert = create<ConvertStore>((set, get) => {
  const patch = (p: Partial<ConvertSheet>) => set((s) => (s.sheet ? { sheet: { ...s.sheet, ...p } } : s));
  let current = 0;
  const live = (job: number) => (p: Partial<ConvertSheet>) => job === current && patch(p);

  /** One right-side panel at a time; a running Compress / OCR keeps the right side. */
  function claim(): boolean {
    if (get().sheet?.phase === "running" || useTools.getState().sheet?.phase === "running") return false;
    useAi.getState().setOpen(false);
    useTools.getState().close();
    return true;
  }

  return {
    sheet: null,

    openExport: (docId, what) => {
      const doc = docId ?? st().active;
      const d = doc != null ? st().docs[doc] : undefined;
      if (!d || !claim()) return;
      if (d.organize) st().patch(d.id, { organize: false });
      set({ sheet: { ...blank("export", d.id), what: what ?? "images" } });
    },

    openCreate: (from) => {
      if (!claim()) return;
      set({ sheet: { ...blank("create", null), from: from ?? "images" } });
    },

    close: () => {
      if (get().sheet?.phase === "running") get().cancel();
      set({ sheet: null });
    },

    set: (p) => patch(p),

    addImages: async () => {
      const picked = await api.pickSource("images", true);
      if (picked.length) patch({ files: [...(get().sheet?.files ?? []), ...picked] });
    },

    chooseText: async () => {
      const [f] = await api.pickSource("text", false);
      if (f) patch({ textFile: f });
    },

    moveImage: (i, d) => {
      const files = [...(get().sheet?.files ?? [])];
      if (d === null) files.splice(i, 1);
      else if (i + d >= 0 && i + d < files.length) [files[i], files[i + d]] = [files[i + d], files[i]];
      patch({ files });
    },

    start: async () => {
      const sheet = get().sheet;
      if (!sheet || sheet.phase === "running") return;
      const job = ++jobSeq;
      current = job;
      const upd = live(job);
      try {
        if (sheet.mode === "export") {
          const d = sheet.doc != null ? st().docs[sheet.doc] : undefined;
          if (!d) return;
          const kind: ExportKind =
            sheet.what === "images"
              ? { kind: "images", format: sheet.imageFormat, dpi: sheet.dpi, gray: sheet.gray, quality: sheet.quality }
              : sheet.what === "text"
                ? { kind: "text", layout: sheet.layout }
                : sheet.what === "html"
                  ? { kind: "html", xhtml: sheet.xhtml }
                  : sheet.what === "extractImages"
                    ? { kind: "extractImages", minSide: 32 }
                    : { kind: "word" };
          // Rust shows the dialog; a dismissed one stays on the options.
          const name = stem(d.path);
          let to: FileRef | null;
          if (isFolderExport(sheet.what)) to = await api.pickFolder(d.id);
          else {
            const ext = sheet.what === "text" ? "txt" : sheet.what === "html" ? (sheet.xhtml ? "xhtml" : "html") : "docx";
            const k: SaveKind = sheet.what === "text" ? "text" : sheet.what === "html" ? (sheet.xhtml ? "xhtml" : "html") : "word";
            to = await api.pickSave(`${name}.${ext}`, { doc: d.id }, k);
          }
          if (!to) return;
          patch({ phase: "running", progress: 0, status: "Starting", error: null });
          const out = await api.exportStart(job, { doc: d.id, kind, pages: pagesOf(sheet, d.currentPage), saveTo: to.token }, (p) =>
            upd({ progress: p.fraction, status: p.total > 0 ? `Page ${Math.min(p.done + 1, p.total)} of ${p.total}` : "Working" }),
          );
          upd({ phase: "done", progress: 1, exported: out });
        } else {
          const source: CreateRequest["source"] =
            sheet.from === "images"
              ? { kind: "images", files: sheet.files.map((f) => f.token) }
              : sheet.from === "text"
                ? { kind: "text", file: sheet.textFile!.token }
                : sheet.from === "blank"
                  ? { kind: "blank", pages: sheet.blankPages }
                  : { kind: "clipboard" };
          patch({ phase: "running", progress: 0, status: "Starting", error: null });
          const made = await api.createStart(
            job,
            { source, pageSize: sheet.pageSize, margin: sheet.margin },
            (p) => upd({ progress: p.fraction, status: p.total > 1 ? `Image ${Math.min(p.done + 1, p.total)} of ${p.total}` : "Working" }),
          );
          // An unsaved tab: Save asks where it goes.
          await st().openPath(made.file.path, made.file.token, true);
          upd({ phase: "done", progress: 1, created: { pages: made.pages, name: baseName(made.file.path), warnings: made.warnings } });
        }
      } catch (e) {
        if (String(e) === "cancelled") upd({ phase: "options", progress: 0, status: "" });
        else upd({ phase: "error", error: friendly(e) });
      }
    },

    cancel: () => {
      if (get().sheet?.phase !== "running") return;
      patch({ status: "Cancelling" });
      void api.toolCancel(current).catch(() => {});
    },
  };
});

/** Whether the sheet's options are complete enough to start, with the reason when not. */
export function readiness(s: ConvertSheet, pageCount: number): string | null {
  if (s.mode === "export") {
    if (s.scope === "range" && (s.range.trim() === "" || parseRange(s.range, pageCount) == null)) return "Enter pages like 1-3, 5";
    if (s.what === "images" && (s.dpi < 36 || s.dpi > 1200)) return "Resolution is 36 to 1200 dpi";
    return null;
  }
  if (s.from === "images" && s.files.length === 0) return "Add at least one image";
  if (s.from === "text" && !s.textFile) return "Choose a text file";
  if (s.from === "blank" && (s.blankPages < 1 || s.blankPages > 500)) return "1 to 500 pages";
  return null;
}

// Compress / OCR taking the rail, or the assistant opening, closes an idle convert rail.
useTools.subscribe((s, prev) => {
  if (s.sheet && !prev.sheet && useConvert.getState().sheet?.phase !== "running") useConvert.setState({ sheet: null });
});
useAi.subscribe((s, prev) => {
  if (s.open && !prev.open) {
    const c = useConvert.getState();
    if (c.sheet?.phase === "running") useAi.setState({ open: false });
    else if (c.sheet) useConvert.setState({ sheet: null });
  }
});
