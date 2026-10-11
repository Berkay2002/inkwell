import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { BatesFileResult, FontFamily, MarkDate, MarkKind, MarkKindInfo, MarkPicture, MarkStyle, PageMark, Rgb, SixTexts } from "./api";
import { useAi } from "./aiStore";
import { useAnnot } from "./annot";
import { useConvert } from "./convertStore";
import { applyState, queued } from "./edit";
import { useHistory } from "./historyStore";
import { formatRange, parseRange } from "./pageRange";
import { useProtect } from "./protectStore";
import { useRedact } from "./redact";
import { useTools } from "./toolsStore";

/**
 * Header & footer, watermark, background and Bates numbering: one rail sheet per window plus what
 * each open document already has (`info`). UI-agnostic; `MarksPanel` renders it and `MarkPreview`
 * draws the same settings on the page before anything is written.
 *
 * Add, Update and Remove are one edit step each in the engine (src-tauri/src/marks): undoable,
 * written by Save or Save as. Bates numbering across several files writes numbered copies into a
 * folder and leaves the originals alone.
 */

export type PagesMode = "all" | "odd" | "even" | "range";

/** A watermark or background picture as the sheet holds it: `file` is an open token. */
export interface PictureChoice {
  file: string;
  name: string;
  /** An image file, or a page of a PDF. */
  kind: "image" | "pdf";
  /** 1-based, for a PDF. */
  page: number;
}

export interface BatesInput {
  token: string;
  path: string;
  name: string;
}

export interface MarksSheet {
  doc: number;
  kind: MarkKind;
  busy: boolean;
  /** Nothing has been typed or chosen yet: the first read of the file may still fill the sheet in. */
  fresh: boolean;
  error: string | null;
  /** What the last Add, Update or Remove did, in one sentence. */
  done: string | null;
  pagesMode: PagesMode;
  /** Typed pages, 1-based ("1, 3-5"), when `pagesMode` is "range". */
  range: string;
  // header and footer; the look is shared with Bates numbering
  text: SixTexts;
  style: MarkStyle;
  /** Top, bottom, left, right in points. */
  margins: [number, number, number, number];
  startNumber: number;
  // watermark
  wmSource: "text" | "picture";
  wmText: string;
  wmPicture: PictureChoice | null;
  wmColor: Rgb;
  wmBold: boolean;
  wmFamily: FontFamily;
  /** Percent of the page: 100 spans the page diagonal (text) or the page (picture). */
  wmScale: number;
  /** Percent. */
  wmOpacity: number;
  /** Degrees counter-clockwise. */
  wmRotation: number;
  wmBehind: boolean;
  /** Points right and up from the page centre. */
  wmOffset: [number, number];
  wmScreen: boolean;
  wmPrint: boolean;
  // background
  bgSource: "colour" | "picture";
  bgColor: Rgb;
  bgOpacity: number;
  bgPicture: PictureChoice | null;
  bgScale: number;
  // Bates numbering
  btPrefix: string;
  btSuffix: string;
  btDigits: number;
  btStart: number;
  /** 0..5, the order of `text`. */
  btSlot: number;
  btFiles: BatesInput[];
  btResults: BatesFileResult[] | null;
}

interface MarksStore {
  sheet: MarksSheet | null;
  /** The marks each open document has, as the engine last said. */
  info: Record<number, MarkKindInfo[]>;
  open: (kind?: MarkKind, doc?: number) => void;
  close: () => void;
  setKind: (kind: MarkKind) => void;
  set: (p: Partial<MarksSheet>) => void;
  refresh: (doc: number) => Promise<void>;
  /** Adds the mark of the open tab, or updates it where the document has one on those pages. */
  apply: () => Promise<void>;
  remove: (kind: MarkKind) => Promise<void>;
  pickPicture: (target: "wm" | "bg", kind: "image" | "pdf") => Promise<void>;
  addBatesFiles: () => Promise<void>;
  numberFiles: () => Promise<void>;
}

const st = () => useStore.getState();
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

export const KIND_NAME: Record<MarkKind, string> = {
  headerFooter: "header and footer",
  watermark: "watermark",
  background: "background",
  bates: "Bates numbering",
};
export const KIND_TITLE: Record<MarkKind, string> = {
  headerFooter: "Header & footer",
  watermark: "Watermark",
  background: "Background",
  bates: "Bates",
};
export const KINDS: MarkKind[] = ["headerFooter", "watermark", "background", "bates"];

/** The six places, in the order of `text`. */
export const SLOTS = ["top left", "top centre", "top right", "bottom left", "bottom centre", "bottom right"] as const;

const baseStyle = (): MarkStyle => ({ family: "helvetica", bold: false, italic: false, size: 12, color: [0, 0, 0], underline: false });

const blank = (doc: number, kind: MarkKind): MarksSheet => ({
  doc,
  kind,
  busy: false,
  fresh: true,
  error: null,
  done: null,
  pagesMode: "all",
  range: "",
  text: ["", "", "", "", "", ""],
  style: baseStyle(),
  margins: [36, 36, 72, 72],
  startNumber: 1,
  wmSource: "text",
  wmText: "",
  wmPicture: null,
  wmColor: [0.6, 0.6, 0.6],
  wmBold: true,
  wmFamily: "helvetica",
  wmScale: 50,
  wmOpacity: 50,
  wmRotation: 45,
  wmBehind: false,
  wmOffset: [0, 0],
  wmScreen: true,
  wmPrint: true,
  bgSource: "colour",
  bgColor: [1, 1, 0.85],
  bgOpacity: 100,
  bgPicture: null,
  bgScale: 100,
  btPrefix: "",
  btSuffix: "",
  btDigits: 6,
  btStart: 1,
  btSlot: 5,
  btFiles: [],
  btResults: null,
});

// ---- pure helpers (tested in marksStore.test.ts) ----

const MONTHS = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/** The `<<…>>` tokens of the engine (src-tauri/src/marks/mod.rs `expand`), the same words for the same input. */
export function expand(template: string, page: number, count: number, date: MarkDate): string {
  const { year: y, month: m, day: d } = date;
  const yy = String(((y % 100) + 100) % 100).padStart(2, "0");
  const p2 = (v: number) => String(v).padStart(2, "0");
  return template.replace(/<<([^>]*?)>>/g, (all, token: string) => {
    switch (token) {
      case "1":
        return String(page);
      case "n":
        return String(count);
      case "1 of n":
        return `${page} of ${count}`;
      case "1/n":
        return `${page}/${count}`;
      case "Page 1":
        return `Page ${page}`;
      case "Page 1 of n":
        return `Page ${page} of ${count}`;
      case "m/d":
        return `${m}/${d}`;
      case "m/d/yy":
        return `${m}/${d}/${yy}`;
      case "m/d/yyyy":
        return `${m}/${d}/${y}`;
      case "mm/dd/yy":
        return `${p2(m)}/${p2(d)}/${yy}`;
      case "mm/dd/yyyy":
        return `${p2(m)}/${p2(d)}/${y}`;
      case "d/m/yy":
        return `${d}/${m}/${yy}`;
      case "d/m/yyyy":
        return `${d}/${m}/${y}`;
      case "dd/mm/yyyy":
        return `${p2(d)}/${p2(m)}/${y}`;
      case "yyyy-mm-dd":
        return `${y}-${p2(m)}-${p2(d)}`;
      case "mmmm d, yyyy":
        return `${MONTHS[Math.min(Math.max(m, 1), 12) - 1]} ${d}, ${y}`;
      default:
        return all;
    }
  });
}

/** The date formats the Insert date menu offers, as tokens. */
export const DATE_FORMATS = ["m/d/yyyy", "mm/dd/yyyy", "d/m/yyyy", "dd/mm/yyyy", "yyyy-mm-dd", "mmmm d, yyyy", "m/d/yy"] as const;

export const todayDate = (): MarkDate => {
  const n = new Date();
  return { year: n.getFullYear(), month: n.getMonth() + 1, day: n.getDate() };
};

export function batesText(prefix: string, digits: number, number: number, suffix: string): string {
  return `${prefix}${String(number).padStart(Math.min(Math.max(digits, 3), 15), "0")}${suffix}`;
}

/** 0-based pages of the choice, or null when the typed range does not parse. */
export function pagesOf(mode: PagesMode, range: string, count: number): number[] | null {
  const all = Array.from({ length: count }, (_, i) => i);
  if (mode === "all") return all;
  // Page 1 is odd.
  if (mode === "odd") return all.filter((i) => i % 2 === 0);
  if (mode === "even") return all.filter((i) => i % 2 === 1);
  const r = parseRange(range, count);
  return r && r.length > 0 ? r : null;
}

const pagesPhrase = (pages: number[], count: number) =>
  pages.length === count ? `${count} ${count === 1 ? "page" : "pages"}` : `${pages.length} ${pages.length === 1 ? "page" : "pages"} (${formatRange(pages)})`;

const hex2 = (v: number) =>
  Math.round(Math.min(Math.max(v, 0), 1) * 255)
    .toString(16)
    .padStart(2, "0");
export const toHex = (c: Rgb) => `#${hex2(c[0])}${hex2(c[1])}${hex2(c[2])}`;
export const fromHex = (h: string): Rgb => {
  const m = /^#?([0-9a-f]{6})$/i.exec(h.trim());
  if (!m) return [0, 0, 0];
  const n = parseInt(m[1], 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
};

const num = (v: number, lo: number, hi: number) => (Number.isFinite(v) ? Math.min(Math.max(v, lo), hi) : lo);

/** The picture as the command takes it: `file` is the open token. */
const pictureOf = (p: PictureChoice | null): MarkPicture | null => (p ? { file: p.file, page: p.kind === "pdf" ? Math.max(0, p.page - 1) : 0 } : null);

/** The mark the sheet describes, for the tab that is open. */
export function markOf(s: MarksSheet): PageMark {
  switch (s.kind) {
    case "headerFooter":
      return { kind: "headerFooter", text: s.text, style: s.style, margins: s.margins, startNumber: Math.max(0, Math.floor(s.startNumber)) };
    case "bates":
      return {
        kind: "bates",
        prefix: s.btPrefix,
        suffix: s.btSuffix,
        digits: s.btDigits,
        start: Math.max(0, Math.floor(s.btStart)),
        slot: s.btSlot,
        style: s.style,
        margins: s.margins,
      };
    case "watermark":
      return {
        kind: "watermark",
        text: s.wmSource === "text" ? s.wmText : "",
        picture: s.wmSource === "picture" ? pictureOf(s.wmPicture) : null,
        // Size 0: the text is fitted to the page by `scale`.
        style: { family: s.wmFamily, bold: s.wmBold, italic: false, size: 0, color: s.wmColor, underline: false },
        scale: num(s.wmScale, 5, 300) / 100,
        opacity: num(s.wmOpacity, 0, 100) / 100,
        rotation: s.wmRotation,
        behind: s.wmBehind,
        offset: s.wmOffset,
        showOnScreen: s.wmScreen,
        showInPrint: s.wmPrint,
      };
    case "background":
      return {
        kind: "background",
        color: s.bgColor,
        opacity: num(s.bgOpacity, 0, 100) / 100,
        picture: s.bgSource === "picture" ? pictureOf(s.bgPicture) : null,
        scale: num(s.bgScale, 5, 300) / 100,
      };
  }
}

/** Why the sheet can't be applied yet, or null. */
export function readiness(s: MarksSheet, count: number): string | null {
  if (pagesOf(s.pagesMode, s.range, count) == null) return s.range.trim() === "" ? "Type the pages, like 1-3, 7." : `Pages go from 1 to ${count}.`;
  switch (s.kind) {
    case "headerFooter":
      return s.text.every((t) => t.trim() === "") ? "Type the text of a header or footer." : sizeProblem(s);
    case "watermark":
      if (s.wmSource === "text" && s.wmText.trim() === "") return "Type the watermark text.";
      if (s.wmSource === "picture" && !s.wmPicture) return "Choose the picture for the watermark.";
      if (!s.wmScreen && !s.wmPrint) return "Show it on screen, in print, or both.";
      return null;
    case "background":
      return s.bgSource === "picture" && !s.bgPicture ? "Choose the picture for the background." : null;
    case "bates": {
      if (s.btFiles.length > 0) return s.btDigits < 3 || s.btDigits > 15 ? "A Bates number has 3 to 15 digits." : sizeProblem(s);
      const pages = pagesOf(s.pagesMode, s.range, count) ?? [];
      if (s.btDigits < 3 || s.btDigits > 15) return "A Bates number has 3 to 15 digits.";
      if (s.btStart + pages.length - 1 >= 10 ** s.btDigits) return `${pages.length} pages from ${s.btStart} need more than ${s.btDigits} digits.`;
      return sizeProblem(s);
    }
  }
}

function sizeProblem(s: MarksSheet): string | null {
  return s.style.size > 0 && s.style.size <= 600 ? null : "The font size has to be between 1 and 600 points.";
}

const quote = (t: string) => `'${t.replace(/\s+/g, " ").trim()}'`;
const list = (items: string[]) => (items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`);

/**
 * What the button will do, in a sentence: "Adds 'Page 1 of 12' to the bottom right of 12 pages."
 * `existing`: the document already has this kind on the chosen pages, so the button updates it.
 */
export function outcome(s: MarksSheet, count: number, existing: boolean, date: MarkDate = todayDate()): string {
  const pages = pagesOf(s.pagesMode, s.range, count);
  if (!pages) return "Choose the pages first.";
  const on = pagesPhrase(pages, count);
  const verb = existing ? "Updates" : "Adds";
  switch (s.kind) {
    case "headerFooter": {
      const first = Math.max(0, Math.floor(s.startNumber));
      const used = s.text.map((t, i) => ({ t, i })).filter((x) => x.t.trim() !== "");
      if (used.length === 0) return "Nothing to add yet: type the text of a header or footer.";
      if (used.length > 3) return `${verb} ${used.length} texts to the margins of ${on}.`;
      const parts = used.map((x) => `${quote(expand(x.t, first, count, date))} to the ${SLOTS[x.i]}`);
      return `${verb} ${list(parts)} of ${on}.`;
    }
    case "watermark": {
      const what = s.wmSource === "text" ? quote(s.wmText || "…") : s.wmPicture ? `the picture ${s.wmPicture.name}` : "a picture";
      const where = s.wmBehind ? "behind the text" : "in front of the text";
      const where2 = !s.wmScreen && s.wmPrint ? ", for printing only" : s.wmScreen && !s.wmPrint ? ", for the screen only" : "";
      return `${verb} ${what} ${where} on ${on}${where2}.`;
    }
    case "background":
      return s.bgSource === "picture"
        ? `${verb} ${s.bgPicture ? `the picture ${s.bgPicture.name}` : "a picture"} behind ${on}.`
        : `${verb} a background colour behind ${on}.`;
    case "bates": {
      const place = SLOTS[s.btSlot] ?? "bottom right";
      if (s.btFiles.length > 0) {
        const first = batesText(s.btPrefix, s.btDigits, s.btStart, s.btSuffix);
        return `Writes a numbered copy of ${s.btFiles.length} ${s.btFiles.length === 1 ? "file" : "files"} to a folder, from ${first}, at the ${place} of each page. The files stay as they are.`;
      }
      const a = batesText(s.btPrefix, s.btDigits, s.btStart, s.btSuffix);
      const b = batesText(s.btPrefix, s.btDigits, s.btStart + pages.length - 1, s.btSuffix);
      return pages.length === 1 ? `${verb} ${a} at the ${place} of ${on}.` : `${verb} ${a} to ${b} at the ${place} of ${on}.`;
    }
  }
}

/** The settings of `mark`, as the sheet holds them (Update starts from what is in the file). */
export function fromMark(base: MarksSheet, mark: PageMark): MarksSheet {
  switch (mark.kind) {
    case "headerFooter":
      return { ...base, kind: "headerFooter", text: mark.text, style: mark.style, margins: mark.margins, startNumber: mark.startNumber };
    case "bates":
      return {
        ...base,
        kind: "bates",
        btPrefix: mark.prefix,
        btSuffix: mark.suffix,
        btDigits: mark.digits,
        btStart: mark.start,
        btSlot: mark.slot,
        style: mark.style,
        margins: mark.margins,
      };
    case "watermark":
      return {
        ...base,
        kind: "watermark",
        wmSource: mark.picture ? "picture" : "text",
        wmText: mark.text,
        wmFamily: mark.style.family,
        wmBold: mark.style.bold,
        wmColor: mark.style.color,
        wmScale: Math.round(mark.scale * 100),
        wmOpacity: Math.round(mark.opacity * 100),
        wmRotation: mark.rotation,
        wmBehind: mark.behind,
        wmOffset: mark.offset,
        wmScreen: mark.showOnScreen,
        wmPrint: mark.showInPrint,
      };
    case "background":
      return {
        ...base,
        kind: "background",
        bgSource: mark.picture ? "picture" : "colour",
        bgColor: mark.color,
        bgOpacity: Math.round(mark.opacity * 100),
        bgScale: Math.round(mark.scale * 100),
      };
  }
}

/** The engine's messages are sentences already; the codes it uses for other things are not shown raw. */
export function marksError(e: unknown): string {
  const s = String(e);
  if (s === "stale") return "The document changed while this was being added. Try again.";
  if (s === "not a PDF file") return "That file is not a PDF.";
  return s;
}

// ---- the store ----

export const useMarks = create<MarksStore>((set, get) => {
  const patch = (p: Partial<MarksSheet>) => set((s) => (s.sheet ? { sheet: { ...s.sheet, ...p } } : s));

  /** One right-side rail at a time; a running Compress / OCR keeps the right side. */
  function claim(): boolean {
    if (useTools.getState().sheet?.phase === "running" || useConvert.getState().sheet?.phase === "running") return false;
    useTools.getState().close();
    useConvert.getState().close();
    useProtect.getState().close();
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useAnnot.getState().setOpen(false);
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
    return true;
  }

  /** The sheet for `kind`, starting from what the document's own mark of that kind says. */
  function filled(sheet: MarksSheet, kind: MarkKind): MarksSheet {
    const found = get().info[sheet.doc]?.find((k) => k.kind === kind);
    let next: MarksSheet = { ...sheet, kind, error: null, done: null };
    if (found?.mark) {
      next = fromMark(next, found.mark);
      const count = st().docs[sheet.doc]?.pageCount ?? 0;
      if (found.pages.length > 0 && found.pages.length < count) next = { ...next, pagesMode: "range", range: formatRange(found.pages) };
      else next = { ...next, pagesMode: "all", range: "" };
      // A picture the file remembers by path needs a new token to be sent again.
      const pic = found.mark.kind === "watermark" || found.mark.kind === "background" ? found.mark.picture : null;
      if (pic) void restorePicture(sheet.doc, kind, pic);
    }
    return next;
  }

  async function restorePicture(doc: number, kind: MarkKind, pic: MarkPicture) {
    try {
      const ref = await api.knownFile(pic.file);
      const isPdf = /\.pdf$/i.test(ref.path);
      const choice: PictureChoice = { file: ref.token, name: baseName(ref.path), kind: isPdf ? "pdf" : "image", page: pic.page + 1 };
      const s = get().sheet;
      if (!s || s.doc !== doc || s.kind !== kind) return;
      patch(kind === "watermark" ? { wmPicture: choice } : { bgPicture: choice });
    } catch {
      // The file moved or was never chosen here: the sheet asks for the picture again.
    }
  }

  async function run(step: () => Promise<api.EditState>, doneText: (r: api.EditState) => string) {
    const sheet = get().sheet;
    if (!sheet || sheet.busy) return;
    const d = st().docs[sheet.doc];
    if (!d) return;
    patch({ busy: true, error: null, done: null });
    try {
      const after = await queued(d.id, step);
      applyState(d.id, after);
      void get().refresh(d.id);
      patch({ busy: false, done: doneText(after) });
    } catch (e) {
      void get().refresh(d.id);
      // The step was not taken; the undo and redo labels are asked for again all the same.
      void api.editState(d.id).then((s) => applyState(d.id, s), () => {});
      patch({ busy: false, error: marksError(e) });
    }
  }

  return {
    sheet: null,
    info: {},

    open: (kind, docId) => {
      const doc = docId ?? st().active;
      const d = doc != null ? st().docs[doc] : undefined;
      if (!d || get().sheet?.busy || !claim()) return;
      if (d.organize) st().patch(d.id, { organize: false });
      const k = kind ?? get().sheet?.kind ?? "headerFooter";
      set({ sheet: filled(blank(d.id, k), k) });
      void get().refresh(d.id).then(() => {
        // The first read can land after the sheet opened: start the tab from what the file has.
        const s = get().sheet;
        if (s && s.doc === d.id && s.fresh && !s.busy) set({ sheet: { ...filled(s, s.kind), fresh: true } });
      });
    },

    close: () => {
      if (get().sheet?.busy) return;
      set({ sheet: null });
    },

    setKind: (kind) => {
      const s = get().sheet;
      if (!s || s.busy || s.kind === kind) return;
      set({ sheet: { ...filled(s, kind), fresh: false } });
    },

    set: (p) => patch({ ...p, fresh: false, error: null, done: null }),

    refresh: async (doc) => {
      try {
        const { kinds } = await api.marksInfo(doc);
        if (!st().docs[doc]) return;
        set((s) => ({ info: { ...s.info, [doc]: kinds } }));
      } catch {
        // A document that is still locked or being replaced has nothing to show yet.
      }
    },

    apply: async () => {
      const s = get().sheet;
      if (!s || s.busy) return;
      const d = st().docs[s.doc];
      if (!d) return;
      const why = readiness(s, d.pageCount);
      if (why) return patch({ error: why });
      if (s.kind === "bates" && s.btFiles.length > 0) return get().numberFiles();
      const pages = pagesOf(s.pagesMode, s.range, d.pageCount) ?? [];
      const has = get().info[s.doc]?.find((k) => k.kind === s.kind);
      const existing = !!has && pages.some((p) => has.pages.includes(p));
      const text = outcome(s, d.pageCount, existing);
      await run(
        () => api.marksAdd(d.id, { mark: markOf(s), pages, replace: existing, date: todayDate() }),
        () => `${text.replace(/^Updates/, "Updated").replace(/^Adds/, "Added").replace(/\.$/, "")}. You can undo it.`,
      );
    },

    remove: async (kind) => {
      const s = get().sheet;
      if (!s) return;
      await run(
        () => api.marksRemove(s.doc, kind),
        () => `Removed the ${KIND_NAME[kind]}. You can undo it.`,
      );
    },

    pickPicture: async (target, kind) => {
      const s = get().sheet;
      if (!s || s.busy) return;
      try {
        const picked = kind === "pdf" ? await api.pickOpen(false) : await api.pickSource("images", false);
        const f = picked[0];
        if (!f) return;
        const choice: PictureChoice = { file: f.token, name: baseName(f.path), kind, page: 1 };
        patch(target === "wm" ? { wmPicture: choice, error: null } : { bgPicture: choice, error: null });
      } catch (e) {
        patch({ error: marksError(e) });
      }
    },

    addBatesFiles: async () => {
      const s = get().sheet;
      if (!s || s.busy) return;
      try {
        const picked = await api.pickOpen(true);
        if (picked.length === 0) return;
        const have = new Set(s.btFiles.map((f) => f.path));
        const add = picked.filter((f) => !have.has(f.path)).map((f) => ({ token: f.token, path: f.path, name: baseName(f.path) }));
        patch({ btFiles: [...s.btFiles, ...add], btResults: null });
      } catch (e) {
        patch({ error: marksError(e) });
      }
    },

    numberFiles: async () => {
      const s = get().sheet;
      if (!s || s.busy || s.btFiles.length === 0) return;
      const bates = markOf({ ...s, kind: "bates" });
      if (bates.kind !== "bates") return;
      patch({ busy: true, error: null, done: null, btResults: null });
      try {
        const folder = await api.pickFolder(s.doc);
        if (!folder) return void patch({ busy: false });
        const results = await api.marksBatesFiles({ files: s.btFiles.map((f) => f.token), bates, date: todayDate(), folder: folder.token });
        const pages = results.reduce((n, r) => n + r.pages, 0);
        patch({ busy: false, btResults: results, done: `Wrote ${results.length} numbered ${results.length === 1 ? "copy" : "copies"}, ${pages} pages.` });
      } catch (e) {
        patch({ busy: false, error: marksError(e) });
      }
    },
  };
});

// A new tab (or a reopened file) is read for its marks; an edit state landing (an Add, an Undo, a
// Save) reads them again; a closed document is forgotten.
useStore.subscribe((s, prev) => {
  const sheetDoc = useMarks.getState().sheet?.doc;
  for (const k of Object.keys(s.docs)) {
    const id = Number(k);
    const d = s.docs[id];
    const was = prev.docs[id];
    const moved = was && (d.rev !== was.rev || d.undoLabel !== was.undoLabel || d.redoLabel !== was.redoLabel);
    // Only the document with its sheet open is read each time; the others when they get one.
    if ((!was || moved) && (id === sheetDoc || useMarks.getState().info[id])) void useMarks.getState().refresh(id);
  }
  for (const k of Object.keys(prev.docs)) {
    const id = Number(k);
    if (s.docs[id]) continue;
    useMarks.setState((p) => {
      if (!(id in p.info) && p.sheet?.doc !== id) return p;
      const info = { ...p.info };
      delete info[id];
      return { info, sheet: p.sheet?.doc === id ? null : p.sheet };
    });
  }
});

// Another rail opening closes an idle Marks rail; a running one keeps the right side.
const yield_ = (opening: () => void) => {
  if (useMarks.getState().sheet?.busy) return opening();
  if (useMarks.getState().sheet) useMarks.setState({ sheet: null });
};
useTools.subscribe((s, prev) => s.sheet && !prev.sheet && yield_(() => useTools.getState().close()));
useConvert.subscribe((s, prev) => s.sheet && !prev.sheet && yield_(() => useConvert.setState({ sheet: null })));
useProtect.subscribe((s, prev) => s.sheet && !prev.sheet && yield_(() => useProtect.getState().close()));
useHistory.subscribe((s, prev) => s.open && !prev.open && yield_(() => useHistory.getState().setOpen(false)));
useRedact.subscribe((s, prev) => s.open && !prev.open && yield_(() => useRedact.getState().setOpen(false)));
useAnnot.subscribe((s, prev) => s.open && !prev.open && yield_(() => useAnnot.getState().setOpen(false)));
useAi.subscribe((s, prev) => s.open && !prev.open && s.settings.mode === "docked" && yield_(() => useAi.getState().setOpen(false)));

// ---- the preview on the page ----

/** What `MarkPreview` draws, in page points with y down (the page as it is displayed, before the view rotation). */
export type PreviewShape =
  | {
      kind: "text";
      x: number;
      y: number;
      text: string;
      family: FontFamily;
      bold: boolean;
      italic: boolean;
      size: number;
      color: Rgb;
      underline: boolean;
      width: number;
      /** Where the text is anchored: "start" for left, "middle" for centre and watermarks. */
      anchor: "start" | "middle";
      opacity: number;
    }
  | { kind: "group"; cx: number; cy: number; rotation: number; opacity: number; shapes: PreviewShape[] }
  | { kind: "rect"; x: number; y: number; w: number; h: number; color: Rgb; opacity: number; blend: boolean }
  | { kind: "picture"; cx: number; cy: number; w: number; h: number; rotation: number; opacity: number; name: string };

export type Measure = (style: Pick<MarkStyle, "family" | "bold" | "italic">, size: number, text: string) => number;

export const CSS_FONT: Record<FontFamily, string> = {
  helvetica: 'Arial, "Liberation Sans", Helvetica, sans-serif',
  times: '"Times New Roman", "Liberation Serif", Times, serif',
  courier: '"Courier New", "Liberation Mono", Courier, monospace',
};

let ctx: CanvasRenderingContext2D | null = null;
/** Width in points of `text`, in the face the engine draws it in (the browser's Arial, Times New Roman and Courier New match the standard 14). */
export const canvasMeasure: Measure = (style, size, text) => {
  ctx ??= document.createElement("canvas").getContext("2d");
  if (!ctx) return text.length * size * 0.5;
  ctx.font = `${style.italic ? "italic " : ""}${style.bold ? "bold " : ""}${size}px ${CSS_FONT[style.family]}`;
  return ctx.measureText(text).width;
};

/**
 * The mark of the open tab on page `page` (0-based) of `count`, laid out as the engine lays it out
 * (src-tauri/src/marks/apply.rs): the same margins, baselines and fitted watermark size. A picture
 * is a box, as the file's own pixels are not known here.
 */
export function previewShapes(s: MarksSheet, page: number, count: number, size: [number, number], measure: Measure, date: MarkDate): PreviewShape[] {
  const pages = pagesOf(s.pagesMode, s.range, count);
  const k = pages ? pages.indexOf(page) : -1;
  if (k < 0) return [];
  const [w, h] = size;
  switch (s.kind) {
    case "headerFooter":
    case "bates": {
      const texts: string[] =
        s.kind === "headerFooter"
          ? s.text.map((t) => expand(t, Math.max(0, Math.floor(s.startNumber)) + k, count, date))
          : s.text.map((_, i) => (i === s.btSlot ? batesText(s.btPrefix, s.btDigits, Math.max(0, Math.floor(s.btStart)) + k, s.btSuffix) : ""));
      return textShapes(texts, s.style, s.margins, w, h, measure);
    }
    case "watermark": {
      if (s.wmSource === "picture") {
        const fit = Math.min(w, h) * (num(s.wmScale, 5, 300) / 100);
        return [
          {
            kind: "picture",
            cx: w / 2 + s.wmOffset[0],
            cy: h / 2 - s.wmOffset[1],
            w: fit,
            h: fit * 0.6,
            rotation: s.wmRotation,
            opacity: num(s.wmOpacity, 0, 100) / 100,
            name: s.wmPicture?.name ?? "Picture",
          },
        ];
      }
      const lines = s.wmText.trim() === "" ? [] : s.wmText.trim().split(/\r?\n/);
      if (lines.length === 0) return [];
      const face = { family: s.wmFamily, bold: s.wmBold, italic: false };
      const widest = Math.max(0.01, ...lines.map((l) => measure(face, 1, l)));
      const diag = Math.hypot(w, h);
      const fitted = Math.min(400, Math.max(6, (diag * Math.max(num(s.wmScale, 5, 300) / 100, 0.05)) / widest));
      const lineH = fitted * 1.15;
      const total = lineH * (lines.length - 1);
      const shapes: PreviewShape[] = lines.map((l, i) => ({
        kind: "text",
        x: 0,
        y: -total / 2 + i * lineH + fitted * 0.35,
        text: l,
        ...face,
        size: fitted,
        color: s.wmColor,
        underline: false,
        width: measure(face, fitted, l),
        anchor: "middle",
        opacity: 1,
      }));
      return [
        { kind: "group", cx: w / 2 + s.wmOffset[0], cy: h / 2 - s.wmOffset[1], rotation: s.wmRotation, opacity: num(s.wmOpacity, 0, 100) / 100, shapes },
      ];
    }
    case "background": {
      if (s.bgSource === "picture") {
        return [
          {
            kind: "picture",
            cx: w / 2,
            cy: h / 2,
            w: w * (num(s.bgScale, 5, 300) / 100),
            h: h * (num(s.bgScale, 5, 300) / 100),
            rotation: 0,
            opacity: num(s.bgOpacity, 0, 100) / 100,
            name: s.bgPicture?.name ?? "Picture",
          },
        ];
      }
      return [{ kind: "rect", x: 0, y: 0, w, h, color: s.bgColor, opacity: num(s.bgOpacity, 0, 100) / 100, blend: true }];
    }
  }
}

/** Six slots at the page edges: the engine's `slots_bytes`, with y turned to count from the top. */
export function textShapes(texts: string[], style: MarkStyle, margins: [number, number, number, number], w: number, h: number, measure: Measure): PreviewShape[] {
  const [top, bottom, left, right] = margins;
  const size = style.size;
  const lineH = size * 1.2;
  const out: PreviewShape[] = [];
  texts.forEach((text, slot) => {
    if (text.trim() === "") return;
    const lines = text.split(/\r?\n/);
    lines.forEach((line, li) => {
      const width = measure(style, size, line);
      const x = slot % 3 === 0 ? left : slot % 3 === 1 ? (w - width) / 2 : w - right - width;
      // Baseline from the bottom of the page, as in the file.
      const up = slot < 3 ? h - top - size * 0.8 - li * lineH : bottom + size * 0.2 + (lines.length - 1 - li) * lineH;
      out.push({
        kind: "text",
        x,
        y: h - up,
        text: line,
        family: style.family,
        bold: style.bold,
        italic: style.italic,
        size,
        color: style.color,
        underline: style.underline,
        width,
        anchor: "start",
        opacity: 1,
      });
    });
  });
  return out;
}
