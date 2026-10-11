import { create } from "zustand";
import type { LabelRange, LabelStyle, PageTrans, Transition, TransStyle } from "./api";

/**
 * Page labels and page transitions in the UI: the sheets that ask (open state) and the pure helpers
 * they and the full-screen view share. The engine owns the PDF side (src-tauri/src/organize).
 */

// ---- page labels ----------------------------------------------------------------------------

export const LABEL_STYLES: { id: LabelStyle; label: string; sample: string }[] = [
  { id: "decimal", label: "1, 2, 3", sample: "1, 2, 3" },
  { id: "lowerRoman", label: "i, ii, iii", sample: "i, ii, iii" },
  { id: "upperRoman", label: "I, II, III", sample: "I, II, III" },
  { id: "lowerAlpha", label: "a, b, c", sample: "a, b, c" },
  { id: "upperAlpha", label: "A, B, C", sample: "A, B, C" },
  { id: "none", label: "No number (prefix only)", sample: "" },
];

const ROMAN: [number, string][] = [
  [1000, "m"], [900, "cm"], [500, "d"], [400, "cd"], [100, "c"], [90, "xc"], [50, "l"], [40, "xl"], [10, "x"], [9, "ix"], [5, "v"], [4, "iv"], [1, "i"],
];

/** i, ii, iii ...; numbers over 3999 are written in digits (as the engine does). */
export function roman(n: number): string {
  if (n < 1 || n > 3999) return String(n);
  let out = "";
  for (const [v, r] of ROMAN) {
    while (n >= v) {
      out += r;
      n -= v;
    }
  }
  return out;
}

/** a ... z, aa ... zz, aaa ...: the letter repeats, it does not carry (the PDF way). */
export function alpha(n: number): string {
  if (n < 1) return "";
  return String.fromCharCode(97 + ((n - 1) % 26)).repeat(Math.floor((n - 1) / 26) + 1);
}

/** The label of the page `offset` pages into a section. */
export function labelAt(r: Pick<LabelRange, "style" | "prefix" | "start">, offset: number): string {
  const n = r.start + offset;
  const num =
    r.style === "none"
      ? ""
      : r.style === "decimal"
        ? String(n)
        : r.style === "lowerRoman"
          ? roman(n)
          : r.style === "upperRoman"
            ? roman(n).toUpperCase()
            : r.style === "lowerAlpha"
              ? alpha(n)
              : alpha(n).toUpperCase();
  return `${r.prefix}${num}`;
}

/** "i, ii, iii, iv": the first few labels a section would give, for the sheet's sample line. */
export function sampleLabels(r: Pick<LabelRange, "style" | "prefix" | "start">, count: number, shown = 4): string {
  const n = Math.min(count, shown);
  const parts = Array.from({ length: n }, (_, i) => labelAt(r, i));
  const more = count > n ? ", …" : "";
  return parts.join(", ") + more;
}

/** The page whose label is `text` (case as typed first, then ignoring case), else null. */
export function pageOfLabel(labels: string[], text: string): number | null {
  const t = text.trim();
  if (!t || labels.length === 0) return null;
  const exact = labels.indexOf(t);
  if (exact >= 0) return exact;
  const low = t.toLowerCase();
  const i = labels.findIndex((l) => l.toLowerCase() === low);
  return i >= 0 ? i : null;
}

// ---- page transitions -----------------------------------------------------------------------

export interface StyleInfo {
  id: TransStyle;
  label: string;
  /** Shows the direction control. */
  direction?: "four" | "glitter";
  /** Shows the horizontal/vertical control. */
  dimension?: boolean;
  /** Shows the inward/outward control. */
  motion?: boolean;
}

export const TRANS_STYLES: StyleInfo[] = [
  { id: "r", label: "None (replace)" },
  { id: "split", label: "Split", dimension: true, motion: true },
  { id: "blinds", label: "Blinds", dimension: true },
  { id: "box", label: "Box", motion: true },
  { id: "wipe", label: "Wipe", direction: "four" },
  { id: "dissolve", label: "Dissolve" },
  { id: "glitter", label: "Glitter", direction: "glitter" },
  { id: "fly", label: "Fly", direction: "four", motion: true },
  { id: "push", label: "Push", direction: "four" },
  { id: "cover", label: "Cover", direction: "four" },
  { id: "uncover", label: "Uncover", direction: "four" },
  { id: "fade", label: "Fade" },
];

export const styleInfo = (s: TransStyle): StyleInfo => TRANS_STYLES.find((x) => x.id === s) ?? TRANS_STYLES[0];

export const DIRECTIONS: { deg: number; label: string }[] = [
  { deg: 0, label: "Left to right" },
  { deg: 90, label: "Bottom to top" },
  { deg: 180, label: "Right to left" },
  { deg: 270, label: "Top to bottom" },
];

/** A transition with the defaults a style needs filled in (so the sheet always has a complete value). */
export function defaultTransition(style: TransStyle, duration = 1): Transition {
  const i = styleInfo(style);
  return {
    style,
    duration,
    dimension: i.dimension ? "H" : null,
    motion: i.motion ? "I" : null,
    direction: i.direction === "glitter" ? 315 : i.direction ? 0 : null,
  };
}

/** What the transition does, in words, for the sheet's pages summary. */
export function describeTransition(t: Transition | null): string {
  if (!t || t.style === "r") return "No transition";
  const i = styleInfo(t.style);
  const bits: string[] = [i.label];
  if (i.dimension && t.dimension) bits.push(t.dimension === "H" ? "horizontal" : "vertical");
  if (i.motion && t.motion) bits.push(t.motion === "I" ? "inward" : "outward");
  if (i.direction === "four" && t.direction != null) bits.push(DIRECTIONS.find((d) => d.deg === t.direction)?.label.toLowerCase() ?? `${t.direction}°`);
  bits.push(`${trimNum(t.duration)} s`);
  return bits.join(", ");
}

const trimNum = (n: number) => String(Math.round(n * 100) / 100);

/** True when two pages have the same transition and auto-advance. */
export function sameTrans(a: PageTrans, b: PageTrans): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/**
 * The pages' common setting, or null when they differ: the sheet starts from it when every page in
 * the range has the same transition.
 */
export function commonTrans(all: PageTrans[], pages: number[]): PageTrans | null {
  if (pages.length === 0) return null;
  const first = all[pages[0]];
  if (!first) return null;
  return pages.every((p) => all[p] && sameTrans(all[p], first)) ? first : null;
}

// ---- the sheets that ask --------------------------------------------------------------------

interface SheetsStore {
  /** Page labels sheet for document `doc`, preset to pages `from..=to` (0-based). */
  labels: { doc: number; from: number; to: number } | null;
  /** Page transitions sheet for `pages` of document `doc`. */
  trans: { doc: number; pages: number[] } | null;
  /** Full screen is showing document `doc`. */
  present: { doc: number; start: number } | null;
  openLabels: (doc: number, from: number, to: number) => void;
  closeLabels: () => void;
  openTrans: (doc: number, pages: number[]) => void;
  closeTrans: () => void;
  openPresent: (doc: number, start: number) => void;
  closePresent: () => void;
}

export const useSheets = create<SheetsStore>((set) => ({
  labels: null,
  trans: null,
  present: null,
  openLabels: (doc, from, to) => set({ labels: { doc, from, to } }),
  closeLabels: () => set({ labels: null }),
  openTrans: (doc, pages) => set({ trans: { doc, pages } }),
  closeTrans: () => set({ trans: null }),
  openPresent: (doc, start) => set({ present: { doc, start } }),
  closePresent: () => set({ present: null }),
}));
