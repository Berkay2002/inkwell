import type { Rect4 } from "../store";
import type { Size } from "./layout";

/** Dark pages: show the page image itself dark (a display filter, the file is never changed). */
export type PageDark = "follow" | "on" | "off";
type Theme = "system" | "light" | "dark";

export const PAGE_DARK_KEY = "inkwell.pageDark";

export const parsePageDark = (v: string | null | undefined): PageDark => (v === "on" || v === "off" || v === "follow" ? v : "follow");

/** Whether the app itself is dark: its own setting, or the system's when it follows the system. */
export const appIsDark = (theme: Theme, systemDark: boolean): boolean => (theme === "system" ? systemDark : theme === "dark");

/** Whether the pages show dark: "follow" is dark exactly when the app is. */
export const pagesAreDark = (setting: PageDark, theme: Theme, systemDark: boolean): boolean =>
  setting === "follow" ? appIsDark(theme, systemDark) : setting === "on";

/**
 * What a click on the toolbar toggle stores: the opposite of what shows now. When that is what
 * "follow" would show anyway, the setting goes back to "follow" so the pages keep tracking the app.
 */
export function togglePageDark(setting: PageDark, theme: Theme, systemDark: boolean): PageDark {
  const want = !pagesAreDark(setting, theme, systemDark);
  if (want === appIsDark(theme, systemDark)) return "follow";
  return want ? "on" : "off";
}

/** One image a page draws, in page space (`rect` is what shows of it). */
export interface PageImage {
  rect: Rect4;
  colorSpace: string;
  /** Has a soft mask or a stencil mask (a logo with a transparent background). */
  masked?: boolean;
}

/** Smaller than this (points) is a rule, a bullet or an icon: it follows the page. */
const MIN_SIDE = 18;
/** Images covering this much of the page are the page itself (a scan): they follow the page too. */
const PAGE_SHARE = 0.6;
/** Every box is one backdrop filter: past this many the largest are kept. */
const MAX_BOXES = 24;

const area = (r: Rect4) => (r[2] - r[0]) * (r[3] - r[1]);
const overlaps = (a: Rect4, b: Rect4) => a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];

/** `r` without the part `c` covers: up to four boxes that do not overlap each other. */
function minus(r: Rect4, c: Rect4): Rect4[] {
  if (!overlaps(r, c)) return [r];
  const out: Rect4[] = [];
  if (c[1] > r[1]) out.push([r[0], r[1], r[2], c[1]]);
  if (c[3] < r[3]) out.push([r[0], c[3], r[2], r[3]]);
  const top = Math.max(r[1], c[1]);
  const bottom = Math.min(r[3], c[3]);
  if (c[0] > r[0]) out.push([r[0], top, c[0], bottom]);
  if (c[2] < r[2]) out.push([c[2], top, r[2], bottom]);
  return out;
}

/**
 * The boxes to show in their own colours on a dark page: photographs and figures, not stencil
 * masks (those are ink), not masked images (their shape is not the box), not tiny marks, and not
 * a scan. The boxes never overlap, because a spot covered twice would be put back twice (dark
 * again): a box inside another vanishes, a partial overlap is cut off the later one. When the
 * pictures together make up most of the page it is a scan in strips or tiles and the page is left
 * alone.
 */
export function naturalRects(images: PageImage[], page: Size): Rect4[] {
  const pageArea = page[0] * page[1];
  let out: Rect4[] = [];
  for (const { rect, colorSpace, masked } of images) {
    const w = rect[2] - rect[0];
    const h = rect[3] - rect[1];
    if (colorSpace === "ImageMask" || masked) continue;
    if (!(w >= MIN_SIDE && h >= MIN_SIDE)) continue;
    if (pageArea > 0 && w * h >= PAGE_SHARE * pageArea) continue;
    let pieces: Rect4[] = [rect];
    for (const seen of out) pieces = pieces.flatMap((p) => minus(p, seen));
    out = out.concat(pieces);
  }
  const total = out.reduce((s, r) => s + area(r), 0);
  if (pageArea > 0 && total >= PAGE_SHARE * pageArea) return [];
  return out.length > MAX_BOXES ? out.sort((a, b) => area(b) - area(a)).slice(0, MAX_BOXES) : out;
}
