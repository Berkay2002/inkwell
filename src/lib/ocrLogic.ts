import type { OcrCatalog, OcrDetection, OcrEditable, OcrFont, OcrLang, OcrPlan, OcrStyle, OcrUnsure } from "./api";

/**
 * The logic behind the Recognize text sheet's language list and output styles, kept apart from the
 * components so it can be tested. A language is a primary tag ("sv"); which engine reads it is the
 * engine's business (`ocr_plan`).
 */

/** Primary tags the sheet can explain how to add, whether or not they are installed. */
export const KNOWN_LANGUAGES = [
  "en", "sv", "de", "fr", "es", "it", "nl", "pt", "da", "nb", "fi", "pl", "cs", "ru", "tr", "ja", "zh", "ko", "ar", "he",
  "fa", "hi", "el", "uk", "hu", "ro", "bg", "th", "vi", "id", "ca", "hr", "sk", "sl", "lt", "lv", "et", "is",
];

export const primary = (tag: string) => tag.split(/[-_]/)[0].toLowerCase();

export function languageName(tag: string): string {
  try {
    const n = new Intl.DisplayNames(["en"], { type: "language" }).of(primary(tag));
    return n && n !== tag ? n : tag;
  } catch {
    return tag;
  }
}

/** "Windows", "Tesseract" or "Windows, Tesseract": who can read it. */
export function engineLabel(l: Pick<OcrLang, "windows" | "tesseract">): string {
  if (l.windows && l.tesseract) return "Windows, Tesseract";
  return l.windows ? "Windows" : "Tesseract";
}

/** The languages whose name or tag contains the query (every word of it), in list order. */
export function matchLanguages(langs: OcrLang[], query: string): OcrLang[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return langs;
  return langs.filter((l) => {
    const hay = `${languageName(l.tag)} ${l.tag}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

/** A typed name that matches no installed language but a known one: its tag, for the how-to-add help. */
export function missingMatch(catalog: OcrCatalog, query: string): string | null {
  const q = query.trim().toLowerCase();
  if (q.length < 2) return null;
  const installed = new Set(catalog.languages.map((l) => l.tag));
  const hit = KNOWN_LANGUAGES.find((t) => !installed.has(t) && (languageName(t).toLowerCase().startsWith(q) || t === q));
  return hit ?? null;
}

/**
 * The selection the sheet starts on. A confident detection wins (the file's own text, or a sample
 * read), then what the user chose last time (if still installed), then the engine's default (the
 * display language plus English).
 */
export function initialLanguages(o: {
  detection: OcrDetection | null;
  saved: string[] | null;
  defaults: string[];
  catalog: OcrCatalog;
}): string[] {
  const d = o.detection;
  if (d && d.source !== "profile" && d.guess) return [d.guess];
  const installed = new Set(o.catalog.languages.map((l) => l.tag));
  const saved = (o.saved ?? []).map(primary).filter((t) => installed.has(t));
  if (saved.length > 0) return saved;
  return o.defaults.length > 0 ? o.defaults : d?.languages ?? [];
}

/** Adds or removes `tag`, keeping the others in the order they were chosen (that is the priority). */
export function toggleLanguage(selected: string[], tag: string): string[] {
  return selected.includes(tag) ? selected.filter((t) => t !== tag) : [...selected, tag];
}

/** One line for the sheet: where the detected language came from. */
export function detectedLine(d: OcrDetection | null, detecting: boolean): string {
  if (detecting) return "Looking at the document to choose a language…";
  if (!d) return "";
  const name = d.guess ? languageName(d.guess) : null;
  if (d.source === "text" && name) return `Looks like ${name}, from the text already in this file.`;
  if (d.source === "sample" && name) return `Looks like ${name}, from a sample of the first page.`;
  return "Chosen from your Windows languages.";
}

/** The engine line under the list. */
export function engineLine(plan: OcrPlan | null): string {
  if (!plan) return "";
  if (plan.engine === "windows") return "Read with Windows OCR.";
  if (plan.engine === "tesseract") return "Read with Tesseract.";
  return "No installed engine reads this language.";
}

export interface StyleInfo {
  id: OcrStyle;
  label: string;
  /** One sentence under the label. */
  text: string;
  /** Shown when the style is chosen: what it changes. */
  warning: string | null;
}

export const STYLES: StyleInfo[] = [
  {
    id: "searchable",
    label: "Searchable image",
    text: "Adds an invisible text layer. The page looks exactly as it does now.",
    warning: null,
  },
  {
    id: "cleaned",
    label: "Searchable image, cleaned up",
    text: "Straightens a tilted scan and removes specks, then adds the text layer.",
    warning: "This changes the page picture on the pages it improves. Undo brings the original back.",
  },
  {
    id: "editable",
    label: "Editable text",
    text: "Turns the recognized lines into real text you can edit, and replaces the scan under them.",
    warning:
      "The original scan pixels under the recognized text are replaced. Undo, or the version history, brings them back. Words it is unsure of stay as the image.",
  },
];

export const DPI_CHOICES = [150, 200, 300] as const;

export const FONTS: { id: OcrFont; label: string; text: string }[] = [
  { id: "auto", label: "Match", text: "Monospaced when the page looks typed, sans-serif otherwise." },
  { id: "sans", label: "Sans", text: "Always sans-serif (Noto Sans)." },
  { id: "mono", label: "Mono", text: "Always monospaced (Noto Sans Mono)." },
];

/** Whether the style changes the page picture, so a before/after preview is worth showing. */
export const changesPicture = (style: OcrStyle) => style !== "searchable";

/** The page to preview (0-based): the one in view when it is in the chosen pages, else the first of them. */
export function previewPage(pages: number[] | null, current: number): number {
  if (!pages || pages.length === 0) return current;
  return pages.includes(current) ? current : pages[0];
}

const UNSURE_WHY: Record<OcrUnsure["why"], string> = {
  "low-confidence": "The engine was not sure of it",
  unusual: "Looks like a misread",
  "no-font": "Needs a letter the bundled fonts lack",
  picture: "Sits on a picture",
};
export const unsureReason = (why: OcrUnsure["why"]) => UNSURE_WHY[why] ?? why;

/** "12 words kept as image (unsure)"; empty when none were. */
export function keptLine(e: Pick<OcrEditable, "words_kept">): string {
  const n = e.words_kept;
  return n > 0 ? `${n} word${n === 1 ? "" : "s"} kept as image (unsure)` : "";
}

/** The page's own unsure words first, then the rest in page order: the list a review walks through. */
export function reviewOrder(unsure: OcrUnsure[], current: number): OcrUnsure[] {
  return [...unsure].sort((a, b) => {
    const ka = a.page === current ? -1 : a.page;
    const kb = b.page === current ? -1 : b.page;
    return ka - kb || a.bbox[1] - b.bbox[1] || a.bbox[0] - b.bbox[0];
  });
}

/** A run's request, from the sheet's choices. Editable text never redoes pages that have text. */
export function requestOptions(o: { style: OcrStyle; dpi: number; skipText: boolean }): { style: OcrStyle; dpi: number; force: boolean } {
  return { style: o.style, dpi: o.dpi, force: o.style === "editable" ? false : !o.skipText };
}

/** Whether Start is possible. */
export function canStart(o: { languages: string[]; plan: OcrPlan | null; rangeOk: boolean }): boolean {
  return o.rangeOk && o.languages.length > 0 && o.plan != null && o.plan.engine != null;
}
