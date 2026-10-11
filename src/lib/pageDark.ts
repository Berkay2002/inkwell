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
}

/** Smaller than this (points) is a rule, a bullet or an icon: it follows the page. */
const MIN_SIDE = 18;
/** An image covering this much of the page is the page itself (a scan): it follows the page too. */
const PAGE_SHARE = 0.6;

/**
 * The boxes to show in their own colours on a dark page: photographs and figures, not stencil
 * masks (those are ink), not tiny marks and not a scan that is the whole page.
 */
export function naturalRects(images: PageImage[], page: Size): Rect4[] {
  const pageArea = page[0] * page[1];
  const out: Rect4[] = [];
  for (const { rect, colorSpace } of images) {
    const w = rect[2] - rect[0];
    const h = rect[3] - rect[1];
    if (colorSpace === "ImageMask") continue;
    if (!(w >= MIN_SIDE && h >= MIN_SIDE)) continue;
    if (pageArea > 0 && w * h >= PAGE_SHARE * pageArea) continue;
    out.push(rect);
  }
  return out;
}
