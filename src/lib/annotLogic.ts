/** Pure helpers for comments (src-tauri/src/annot): threads, grouping, filter, geometry. No state here. */

export type Kind =
  | "highlight" | "underline" | "strikeOut" | "squiggly" | "note" | "freeText" | "ink"
  | "rectangle" | "ellipse" | "line" | "arrow" | "polygon" | "stamp" | "other";
export type Rect4 = [number, number, number, number];
export type Pt = [number, number];
/** `[ulx, uly, urx, ury, llx, lly, lrx, lry]`, page space, y down. */
export type Quad8 = [number, number, number, number, number, number, number, number];
export type RGB = [number, number, number];

export interface Annot {
  page: number;
  xref: number;
  kind: Kind;
  author: string;
  contents: string;
  /** PDF date strings (`D:20261011143200Z`). */
  created: string;
  modified: string;
  color: RGB | null;
  fill: RGB | null;
  opacity: number;
  width: number;
  rect: Rect4;
  quads: Quad8[];
  points: Pt[];
  strokes: Pt[][];
  icon: string | null;
  fontSize: number | null;
  parent: number | null;
  state: string | null;
  resolved: boolean;
}

export type Filter = "all" | "notes" | "markup" | "resolved";
export const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "notes", label: "Notes" },
  { id: "markup", label: "Markup" },
  { id: "resolved", label: "Resolved" },
];

export const KIND_LABEL: Record<Kind, string> = {
  highlight: "Highlight",
  underline: "Underline",
  strikeOut: "Strikethrough",
  squiggly: "Squiggly underline",
  note: "Note",
  freeText: "Text box",
  ink: "Drawing",
  rectangle: "Rectangle",
  ellipse: "Ellipse",
  line: "Line",
  arrow: "Arrow",
  polygon: "Polygon",
  stamp: "Stamp",
  other: "Annotation",
};

const TEXT_MARKUP: Kind[] = ["highlight", "underline", "strikeOut", "squiggly"];
export const isTextMarkup = (k: Kind) => TEXT_MARKUP.includes(k);
/** Kinds whose own box can be dragged and resized on the page. */
export const isBoxed = (k: Kind) => k !== "other";

/** The comments that are not replies. */
export const roots = (items: Annot[]) => items.filter((a) => a.parent == null || !items.some((p) => p.xref === a.parent));

export function repliesOf(items: Annot[], xref: number): Annot[] {
  return items.filter((a) => a.parent === xref).sort((a, b) => a.created.localeCompare(b.created) || a.xref - b.xref);
}

export function matches(a: Annot, filter: Filter, query: string): boolean {
  if (filter === "notes" && !(a.kind === "note" || a.kind === "freeText" || a.contents !== "")) return false;
  if (filter === "markup" && !(isTextMarkup(a.kind) || ["ink", "rectangle", "ellipse", "line", "arrow", "polygon", "stamp"].includes(a.kind))) return false;
  if (filter === "resolved" && !a.resolved) return false;
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return `${a.author} ${a.contents} ${KIND_LABEL[a.kind]}`.toLowerCase().includes(q);
}

export interface PageGroup {
  page: number;
  items: Annot[];
}

/**
 * Root comments grouped by page, in reading order (top to bottom, then left). A root stays when it
 * or one of its replies matches the filter and query.
 */
export function groupByPage(items: Annot[], filter: Filter, query: string): PageGroup[] {
  const out = new Map<number, Annot[]>();
  for (const a of roots(items)) {
    const keep = matches(a, filter, query) || (filter !== "resolved" && repliesOf(items, a.xref).some((r) => matches(r, "all", query) && query.trim() !== ""));
    if (!keep) continue;
    const l = out.get(a.page);
    if (l) l.push(a);
    else out.set(a.page, [a]);
  }
  return [...out.entries()]
    .sort((x, y) => x[0] - y[0])
    .map(([page, list]) => ({ page, items: list.sort((p, q) => p.rect[1] - q.rect[1] || p.rect[0] - q.rect[0]) }));
}

/** "D:20261011143200Z" -> Date, or null. */
export function parsePdfDate(s: string): Date | null {
  const m = /^D:(\d{4})(\d\d)?(\d\d)?(\d\d)?(\d\d)?(\d\d)?/.exec(s);
  if (!m) return null;
  const [y, mo = "01", d = "01", h = "00", mi = "00", se = "00"] = m.slice(1);
  const t = Date.UTC(+y, +mo - 1, +d, +h, +mi, +se);
  return Number.isNaN(t) ? null : new Date(t);
}

/** "14:32" today, "11 Oct 14:32" otherwise; "" when the file has no date. */
export function timeLabel(s: string, now = new Date()): string {
  const d = parsePdfDate(s);
  if (!d) return "";
  const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  if (d.toDateString() === now.toDateString()) return hm;
  return `${d.getDate()} ${d.toLocaleString("en", { month: "short" })} ${hm}`;
}

export const rectToQuad = (r: Rect4): Quad8 => [r[0], r[1], r[2], r[1], r[0], r[3], r[2], r[3]];

export function normRect(a: Pt, b: Pt): Rect4 {
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
}

/** The box a drag from `a` to `b` makes, clamped to the page. */
export function clampRect(r: Rect4, size: [number, number]): Rect4 {
  return [Math.max(0, r[0]), Math.max(0, r[1]), Math.min(size[0], r[2]), Math.min(size[1], r[3])];
}

export const HANDLES = ["nw", "n", "ne", "e", "se", "s", "sw", "w"] as const;
export type Handle = (typeof HANDLES)[number];

/** `r` after dragging `handle` by (dx, dy) page points; never inverted below `min`. */
export function resizeRect(r: Rect4, handle: Handle, dx: number, dy: number, min = 6): Rect4 {
  let [x0, y0, x1, y1] = r;
  if (handle.includes("w")) x0 = Math.min(x0 + dx, x1 - min);
  if (handle.includes("e")) x1 = Math.max(x1 + dx, x0 + min);
  if (handle.includes("n")) y0 = Math.min(y0 + dy, y1 - min);
  if (handle.includes("s")) y1 = Math.max(y1 + dy, y0 + min);
  return [x0, y0, x1, y1];
}

export const moveRect = (r: Rect4, dx: number, dy: number): Rect4 => [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy];

/** Top-most (smallest) comment under the point, replies excluded; `slop` points of leeway. */
export function hitTest(items: Annot[], page: number, p: Pt, slop = 3): Annot | null {
  let best: Annot | null = null;
  let bestArea = Infinity;
  for (const a of roots(items)) {
    if (a.page !== page || a.kind === "other") continue;
    const [x0, y0, x1, y1] = a.rect;
    if (p[0] < x0 - slop || p[0] > x1 + slop || p[1] < y0 - slop || p[1] > y1 + slop) continue;
    const area = Math.max(1, (x1 - x0) * (y1 - y0));
    if (area < bestArea) {
      best = a;
      bestArea = area;
    }
  }
  return best;
}

/** Per-line boxes of a text selection (already in page space) merged into one quad per line. */
export function quadsFromRects(rects: Rect4[]): Quad8[] {
  const lines: Rect4[] = [];
  for (const r of [...rects].filter((r) => r[2] - r[0] > 0.5 && r[3] - r[1] > 0.5).sort((a, b) => a[1] - b[1] || a[0] - b[0])) {
    const last = lines[lines.length - 1];
    const sameLine = last && Math.min(last[3], r[3]) - Math.max(last[1], r[1]) > 0.5 * Math.min(last[3] - last[1], r[3] - r[1]);
    if (sameLine) {
      last[0] = Math.min(last[0], r[0]);
      last[1] = Math.min(last[1], r[1]);
      last[2] = Math.max(last[2], r[2]);
      last[3] = Math.max(last[3], r[3]);
    } else lines.push([...r]);
  }
  return lines.map(rectToQuad);
}

/** The date stamp's text ("11 Oct 2026"). */
export const dateStampName = (d = new Date()) => `${d.getDate()} ${d.toLocaleString("en", { month: "short" })} ${d.getFullYear()}`;

export const STAMPS = ["Approved", "Draft", "Confidential"] as const;
/** Rough size of a stamp for its name, in points. */
export const stampSize = (name: string): [number, number] => [Math.max(110, 22 + name.length * 13), 40];

export const rgbHex = (c: RGB | null, fallback = "#1A1C20") =>
  c ? `#${c.map((v) => Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, "0")).join("")}` : fallback;

export const COLORS: { name: string; rgb: RGB }[] = [
  { name: "Yellow", rgb: [0.965, 0.847, 0.376] },
  { name: "Green", rgb: [0.608, 0.827, 0.643] },
  { name: "Blue", rgb: [0.18, 0.294, 0.561] },
  { name: "Orange", rgb: [0.878, 0.663, 0.231] },
  { name: "Grey", rgb: [0.42, 0.404, 0.373] },
  { name: "Ink", rgb: [0.102, 0.11, 0.125] },
];
