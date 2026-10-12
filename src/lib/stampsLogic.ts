import type { Rect4 } from "./annotLogic";

/** Pure helpers for custom stamps (src-tauri/src/annot/stamps.rs): grouping, names, the picker's values. No state here. */

export interface Stamp {
  id: string;
  name: string;
  category: string;
  /** Pixels of the stored picture. */
  width: number;
  height: number;
}

export interface Library {
  stamps: Stamp[];
  /** The list was damaged or had entries that could not be used. */
  warning: string | null;
}

/** Why a stamp cannot be placed (a certification that allows no annotations), and whether the document is signed. */
export interface Gate {
  reason: string | null;
  signed: boolean;
}

export const MAX_STAMPS = 200;
export const MAX_NAME = 128;
export const MAX_CATEGORY = 64;
export const DEFAULT_CATEGORY = "Custom";

/** What a signed document is told when a stamp is placed (the same words as its other write paths). */
export const SIGNED_NOTE =
  "This document is signed. A stamp is saved as an update after the signature, which will show that the file was changed. Anything added and then removed in the same session stays in the saved file.";

const PREFIX = "custom:";
/** The picker's value for library stamp `id` (the standard ones are their own names). */
export const customValue = (id: string) => PREFIX + id;
export const customId = (value: string): string | null => (value.startsWith(PREFIX) ? value.slice(PREFIX.length) : null);

const collator = new Intl.Collator("en", { sensitivity: "base", numeric: true });

/** Stamps grouped by category: categories in name order, "Custom" first, stamps in name order. */
export function byCategory(stamps: Stamp[]): { category: string; stamps: Stamp[] }[] {
  const groups = new Map<string, Stamp[]>();
  for (const s of stamps) groups.set(s.category, [...(groups.get(s.category) ?? []), s]);
  return [...groups.entries()]
    .sort(([a], [b]) => (a === DEFAULT_CATEGORY ? -1 : b === DEFAULT_CATEGORY ? 1 : collator.compare(a, b)))
    .map(([category, list]) => ({ category, stamps: [...list].sort((a, b) => collator.compare(a.name, b.name)) }));
}

export const categoriesOf = (stamps: Stamp[]): string[] => byCategory(stamps).map((g) => g.category);

/** A first name for a stamp made from `path`: the file's name without its extension. */
export function nameFromPath(path: string): string {
  const file = path.split(/[\\/]/).pop() ?? "";
  const stem = file.replace(/\.[^.]*$/, "");
  return [...(stem || file)].slice(0, MAX_NAME).join("").trim();
}

export const isPdfPath = (path: string) => /\.pdf$/i.test(path);

/** What is wrong with a stamp's name or category as typed, or null (the engine checks the same limits). */
export function nameProblem(name: string): string | null {
  const t = name.trim();
  if (!t) return "Give the stamp a name";
  if ([...t].length > MAX_NAME) return `A stamp's name is at most ${MAX_NAME} characters`;
  return null;
}
export const categoryProblem = (c: string): string | null =>
  [...c.trim()].length > MAX_CATEGORY ? `A category is at most ${MAX_CATEGORY} characters` : null;

/**
 * `to` (the box a drag asked for) cut to the proportion `aspect` (width / height) of a picture: the largest such
 * box inside it, kept on the side or sides of `from` that did not move, else centred. The engine does the same
 * (annot/stamps.rs `fit_aspect`) when the change is saved; this is for the frame while it is dragged.
 */
export function fitAspect(from: Rect4, to: Rect4, aspect: number): Rect4 {
  const tw = to[2] - to[0];
  const th = to[3] - to[1];
  if (!(Number.isFinite(aspect) && aspect > 1e-3) || tw <= 1e-3 || th <= 1e-3) return to;
  const [w, h] = tw / th > aspect ? [th * aspect, th] : [tw, tw / aspect];
  const near = (a: number, b: number) => Math.abs(a - b) < 0.01;
  const place = (lo: number, hi: number, fromLo: number, fromHi: number, size: number) =>
    near(lo, fromLo) ? lo : near(hi, fromHi) ? hi - size : (lo + hi) / 2 - size / 2;
  const x0 = place(to[0], to[2], from[0], from[2], w);
  const y0 = place(to[1], to[3], from[1], from[3], h);
  return [x0, y0, x0 + w, y0 + h];
}
