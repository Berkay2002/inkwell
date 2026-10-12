// Pure logic of the version history panel and Compare mode (grouping, filters, change lists, stepper).
import type { Change, ChangeKind, Comparison, PageMap, Rect4, TextRun, VersionInfo } from "./history";
import { parseRange } from "./pageRange";

// ---- versions ---------------------------------------------------------------------------------------

export type Filter = "all" | "you" | "assistant" | "outside";
export const FILTERS: { id: Filter; label: string }[] = [
  { id: "all", label: "All" },
  { id: "you", label: "You" },
  { id: "assistant", label: "Assistant" },
  { id: "outside", label: "Outside" },
];

export type Who = "you" | "assistant" | "outside" | "autosave" | "inkwell";

export function whoOf(v: VersionInfo): Who {
  if (v.kind.type === "import") return "inkwell";
  switch (v.author.type) {
    case "user":
      return "you";
    case "assistant":
      return "assistant";
    case "external":
      return "outside";
    case "autosave":
      return "autosave";
  }
}

export const WHO_WORD: Record<Who, string> = {
  you: "You",
  assistant: "Assistant",
  outside: "Outside Inkwell",
  autosave: "Autosave",
  inkwell: "Inkwell",
};

/** "You" also lists autosaves (they are your unsaved edits); the first version only shows under "All". */
export function matchesFilter(v: VersionInfo, f: Filter): boolean {
  const w = whoOf(v);
  return f === "all" || (f === "you" ? w === "you" || w === "autosave" : w === f);
}

export const versionTitle = (v: VersionInfo) => v.name || v.label;

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const sameDay = (a: Date, b: Date) => a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();

/** "Today", "Yesterday", "2 Oct" (with the year when it is not this year). */
export function dayLabel(ms: number, now: number): string {
  const d = new Date(ms);
  const n = new Date(now);
  if (sameDay(d, n)) return "Today";
  const y = new Date(n.getFullYear(), n.getMonth(), n.getDate() - 1);
  if (sameDay(d, y)) return "Yesterday";
  return `${d.getDate()} ${MONTHS[d.getMonth()]}${d.getFullYear() === n.getFullYear() ? "" : ` ${d.getFullYear()}`}`;
}

export const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/** "Today, 14:32" / "Yesterday, 16:48" / "2 Oct, 10:24". */
export const when = (ms: number, now: number) => `${dayLabel(ms, now)}, ${clock(ms)}`;

export interface DayGroup {
  key: string;
  label: string;
  items: VersionInfo[];
}

/** Newest first. Named versions are pinned (and only they show with `onlyNamed`); the rest group by day. */
export function groupVersions(all: VersionInfo[], filter: Filter, onlyNamed: boolean, now: number): { pinned: VersionInfo[]; days: DayGroup[] } {
  const sorted = [...all].sort((a, b) => b.created - a.created || (a.id < b.id ? 1 : -1));
  const shown = sorted.filter((v) => matchesFilter(v, filter));
  const pinned = shown.filter((v) => !!v.name);
  if (onlyNamed) return { pinned, days: [] };
  const days: DayGroup[] = [];
  for (const v of shown) {
    if (v.name) continue;
    const label = dayLabel(v.created, now);
    const last = days[days.length - 1];
    if (last && last.label === label) last.items.push(v);
    else days.push({ key: `${label}:${v.id}`, label, items: [v] });
  }
  return { pinned, days };
}

/** The version a version is compared with to list its changes: its parent, else the next older one. */
export function parentOf(all: VersionInfo[], v: VersionInfo): VersionInfo | null {
  const byId = all.find((x) => v.parents.includes(x.id));
  if (byId) return byId;
  const older = all.filter((x) => x.created < v.created || (x.created === v.created && x.id < v.id));
  return older.sort((a, b) => b.created - a.created)[0] ?? null;
}

/** "p. 1", "p. 3" ... at most `max`, then "+n". Pages are 0-based in, 1-based out. */
export function pageChips(pages: number[], max = 3): string[] {
  const p = [...new Set(pages)].sort((a, b) => a - b);
  const out = p.slice(0, max).map((n) => `p. ${n + 1}`);
  if (p.length > max) out.push(`+${p.length - max}`);
  return out;
}

// ---- change lists -----------------------------------------------------------------------------------

export type Category = "text" | "formatting" | "images" | "annotations" | "pages" | "forms" | "metadata";
export const CATEGORIES: { id: Category; label: string }[] = [
  { id: "text", label: "Text" },
  { id: "formatting", label: "Formatting" },
  { id: "images", label: "Images" },
  { id: "annotations", label: "Annotations" },
  { id: "pages", label: "Pages" },
  { id: "forms", label: "Form fields" },
  { id: "metadata", label: "Metadata" },
];

const CATEGORY: Record<ChangeKind, Category> = {
  pageAdded: "pages",
  pageRemoved: "pages",
  pageReplaced: "pages",
  pageMoved: "pages",
  pageRotated: "pages",
  pageBoxChanged: "pages",
  textChanged: "text",
  formatChanged: "formatting",
  ocrLayerAdded: "text",
  redactionApplied: "text",
  // painted content that no specific kind explains (vector art, colours) is listed with the images
  imageReplaced: "images",
  pageContentChanged: "images",
  annotationAdded: "annotations",
  annotationRemoved: "annotations",
  annotationChanged: "annotations",
  formFieldChanged: "forms",
  metadataChanged: "metadata",
  bookmarksChanged: "metadata",
  attachmentsChanged: "metadata",
};
export const categoryOf = (c: Change): Category => CATEGORY[c.kind];

export type Tone = "add" | "del" | "chg";
/** + added, − removed, ~ changed: shown next to the colour, never colour alone. */
export const GLYPH: Record<Tone, string> = { add: "+", del: "−", chg: "~" };
export const TONE_WORD: Record<Tone, string> = { add: "Added", del: "Removed", chg: "Changed" };

export function toneOf(c: Change): Tone {
  switch (c.kind) {
    case "pageAdded":
    case "annotationAdded":
    case "ocrLayerAdded":
      return "add";
    case "pageRemoved":
    case "annotationRemoved":
    case "redactionApplied":
      return "del";
    case "textChanged": {
      const runs = textRuns(c);
      const ins = runs.some((r) => r.op === "insert");
      const del = runs.some((r) => r.op === "delete");
      return ins && !del ? "add" : del && !ins ? "del" : "chg";
    }
    default:
      return "chg";
  }
}

export const textRuns = (c: Change): TextRun[] => (c.detail.type === "text" ? (c.detail as { runs: TextRun[] }).runs : []);

/** 0-based page a change belongs to (the newer side first), null for document-level changes. */
export const changePage = (c: Change): number | null => c.bPage ?? c.aPage;

export interface PageGroup {
  /** 0-based, null = the document as a whole */
  page: number | null;
  items: { change: Change; index: number }[];
}

/** Changes of the enabled categories, grouped by page (document-level last); `index` is the position in `changes`. */
export function groupChanges(changes: Change[], enabled: Set<Category> | null): PageGroup[] {
  const by = new Map<number | null, PageGroup>();
  changes.forEach((change, index) => {
    if (enabled && !enabled.has(categoryOf(change))) return;
    const page = changePage(change);
    let g = by.get(page);
    if (!g) by.set(page, (g = { page, items: [] }));
    g.items.push({ change, index });
  });
  return [...by.values()].sort((a, b) => (a.page ?? Infinity) - (b.page ?? Infinity));
}

/** The order the stepper walks (original indices, as listed). */
export const flatOrder = (groups: PageGroup[]): number[] => groups.flatMap((g) => g.items.map((i) => i.index));

/** Counts per category (for the filter chips). */
export function categoryCounts(changes: Change[]): Record<Category, number> {
  const n: Record<Category, number> = { text: 0, formatting: 0, images: 0, annotations: 0, pages: 0, forms: 0, metadata: 0 };
  for (const c of changes) n[categoryOf(c)]++;
  return n;
}

/** What the summary strip shows: one entry per category that has differences. */
export function summaryChips(counts: Record<Category, number>): { id: Category; label: string; n: number }[] {
  return CATEGORIES.filter((c) => counts[c.id] > 0).map((c) => ({ ...c, n: counts[c.id] }));
}

/** "12 differences" / "1 difference" / "No differences". */
export const differencesWord = (n: number) => (n === 0 ? "No differences" : n === 1 ? "1 difference" : `${n} differences`);

/** The words next to the progress bar. Stages come from the engine: first file, second file, pages, document. */
export function progressLabel(p: { stage: "first" | "second" | "pages" | "document"; done: number; total: number } | null): string {
  if (!p) return "Starting\u2026";
  const of = p.total > 0 ? ` ${p.done} of ${p.total}` : "";
  switch (p.stage) {
    case "first":
      return `Reading the first file, page${of || "s"}`;
    case "second":
      return `Reading the second file, page${of || "s"}`;
    case "pages":
      return p.total > 0 ? `Comparing page ${Math.min(p.done + 1, p.total)} of ${p.total}` : "Matching pages";
    default:
      return "Checking document properties";
  }
}

/** 0..1 for the progress bar: the two reads take 40 % each, the page comparison 20 %. */
export function progressFraction(p: { stage: "first" | "second" | "pages" | "document"; done: number; total: number } | null): number {
  if (!p) return 0;
  const f = p.total > 0 ? Math.min(1, p.done / p.total) : 0;
  switch (p.stage) {
    case "first":
      return 0.4 * f;
    case "second":
      return 0.4 + 0.4 * f;
    case "pages":
      return 0.8 + 0.2 * f;
    default:
      return 1;
  }
}

/** Previous / next with wrap-around; from "nothing selected" (-1) next goes to the first, previous to the last. */
export function stepIndex(cur: number, dir: 1 | -1, n: number): number {
  if (n <= 0) return -1;
  if (cur < 0) return dir === 1 ? 0 : n - 1;
  return (cur + dir + n) % n;
}

/** Consecutive runs of numbers: [1,2,3,5] -> [[1,3],[5,5]]. */
export function ranges(nums: number[]): [number, number][] {
  const s = [...new Set(nums)].sort((a, b) => a - b);
  const out: [number, number][] = [];
  for (const n of s) {
    const last = out[out.length - 1];
    if (last && n === last[1] + 1) last[1] = n;
    else out.push([n, n]);
  }
  return out;
}

const pagesWord = (a: number, b: number) => (a === b ? `Page ${a}` : `Pages ${a}-${b}`);

/** A page range field of the compare setup. `pages` is null for an empty field (every page) and while the text is not
 *  usable; `error` is what to tell the user then. Without `count` (the file's pages are not known yet) only the syntax
 *  is checked and the engine refuses a page past the end. */
export function rangeField(text: string, count: number | null): { pages: number[] | null; error: string | null } {
  if (!text.trim()) return { pages: null, error: null };
  const syntax = parseRange(text, Number.MAX_SAFE_INTEGER);
  if (!syntax?.length) return { pages: null, error: "Enter pages like 1-3, 5" };
  if (count !== null && syntax[syntax.length - 1] >= count) return { pages: null, error: `This file has ${count} ${count === 1 ? "page" : "pages"}` };
  return { pages: syntax, error: null };
}

/** Page-level summary lines for the top of the change list: "Page 3 inserted", "Pages 5-6 moved to 9". */
export function pageSummary(c: Comparison): string[] {
  const out: string[] = [];
  const label = (r: [number, number][], verb: string) => r.forEach(([a, b]) => out.push(`${pagesWord(a + 1, b + 1)} ${verb}`));
  const replaced = new Map(c.changes.flatMap((ch) => (ch.kind === "pageReplaced" && ch.bPage !== null ? [[ch.bPage, ch.aPage] as const] : [])));
  const replacedA = new Set(replaced.values());
  const added = c.pages.bToA.flatMap((a, k) => (a === null && !replaced.has(c.pages.bIds[k]) ? [c.pages.bIds[k]] : []));
  label(ranges(added), "inserted");
  label(ranges(c.pages.removedA.filter((a) => !replacedA.has(a))), "removed");
  label(ranges([...replaced.keys()]), "replaced");
  const moved = c.changes
    .flatMap((ch) => (ch.detail.type === "pageMoved" ? [ch.detail as { type: string; from: number; to: number }] : []))
    .sort((x, y) => x.to - y.to);
  for (let i = 0; i < moved.length; ) {
    let j = i;
    while (j + 1 < moved.length && moved[j + 1].from === moved[j].from + 1 && moved[j + 1].to === moved[j].to + 1) j++;
    out.push(`${pagesWord(moved[i].from + 1, moved[j].from + 1)} moved to ${moved[i].to + 1}`);
    i = j + 1;
  }
  label(
    ranges(c.changes.flatMap((ch) => (ch.kind === "pageRotated" && ch.bPage !== null ? [ch.bPage] : []))),
    "rotated",
  );
  return out;
}

// ---- compare: page rows and marks on pages -------------------------------------------------------------

export interface Row {
  a: number | null;
  b: number | null;
}

/** Pages of both sides in reading order, one row per page pair; removed pages get a row with an empty right side. */
export function alignRows(m: PageMap): Row[] {
  const rows: Row[] = [];
  const removed = [...m.removedA].sort((x, y) => x - y);
  let r = 0;
  m.bToA.forEach((a, k) => {
    const b = m.bIds[k];
    if (a !== null) while (r < removed.length && removed[r] < a) rows.push({ a: removed[r++], b: null });
    else {
      // an unmatched B page sharing its gap with a removed A page is a replaced page: one row
      const next = m.bToA.slice(k + 1).find((x) => x !== null) ?? Infinity;
      if (r < removed.length && removed[r] < next) return rows.push({ a: removed[r++], b });
    }
    rows.push({ a, b });
  });
  while (r < removed.length) rows.push({ a: removed[r++], b: null });
  return rows;
}

/** The row a change is on (-1 for document-level changes). */
export function rowOf(rows: Row[], c: Change): number {
  const both = rows.findIndex((r) => (c.bPage !== null && r.b === c.bPage) && (c.aPage === null || r.a === c.aPage));
  if (both >= 0) return both;
  return rows.findIndex((r) => (c.bPage !== null && r.b === c.bPage) || (c.aPage !== null && r.a === c.aPage));
}

/** Word boxes on the same line become one box. */
export function mergeLine(boxes: Rect4[]): Rect4[] {
  const out: Rect4[] = [];
  for (const b of boxes) {
    const p = out[out.length - 1];
    if (p && Math.abs(p[1] - b[1]) < 3 && Math.abs(p[3] - b[3]) < 3 && b[0] - p[2] < 8) {
      p[0] = Math.min(p[0], b[0]);
      p[2] = Math.max(p[2], b[2]);
      p[1] = Math.min(p[1], b[1]);
      p[3] = Math.max(p[3], b[3]);
    } else out.push([...b] as Rect4);
  }
  return out;
}

export interface Mark {
  /** null = the whole page */
  rect: Rect4 | null;
  tone: Tone;
  /** index into the comparison's changes */
  change: number;
  /** 0-based page on this side */
  page: number;
}

/** What to draw on the pages of one side: inserted words on B, deleted words on A, boxes of everything else. */
export function marksFor(changes: Change[], side: "a" | "b"): Mark[] {
  const out: Mark[] = [];
  changes.forEach((c, change) => {
    const page = side === "a" ? c.aPage : c.bPage;
    if (page === null) return;
    const tone = toneOf(c);
    const rect = side === "a" ? c.rectA : c.rectB;
    if (c.kind === "textChanged") {
      const runs = textRuns(c);
      let any = false;
      for (const r of runs) {
        if (r.op === (side === "a" ? "delete" : "insert")) {
          for (const box of mergeLine(side === "a" ? r.boxesA : r.boxesB)) {
            out.push({ rect: box, tone: side === "a" ? "del" : "add", change, page });
            any = true;
          }
        }
      }
      if (!any && rect) out.push({ rect, tone: "chg", change, page });
      return;
    }
    if (tone === "add" && side === "a") return;
    if (tone === "del" && side === "b") return;
    if (c.kind === "pageAdded" || c.kind === "pageRemoved" || c.kind === "pageReplaced") out.push({ rect: null, tone, change, page });
    else if (rect) out.push({ rect, tone, change, page });
  });
  return out;
}

const overlap = (a: Rect4, b: Rect4) => Math.max(0, Math.min(a[2], b[2]) - Math.max(a[0], b[0])) * Math.max(0, Math.min(a[3], b[3]) - Math.max(a[1], b[1]));

/** Pixel-diff boxes that no mark already explains (more than half of the box is inside a mark). */
export function uncovered(boxes: Rect4[], marks: Mark[]): Rect4[] {
  const rects = marks.flatMap((m) => (m.rect ? [m.rect] : []));
  return boxes.filter((b) => {
    const area = (b[2] - b[0]) * (b[3] - b[1]);
    return area > 0 && !rects.some((r) => overlap(b, r) > area * 0.5);
  });
}

// ---- inline word diff (GitHub style) ----------------------------------------------------------------

/** Runs for the change list: unchanged stretches shortened to a few words of context around the edits. */
export function inlineRuns(runs: TextRun[], context = 3): { op: TextRun["op"]; text: string }[] {
  return runs.map((r, i) => {
    if (r.op !== "equal") return { op: r.op, text: r.text };
    const w = r.text.split(/\s+/).filter(Boolean);
    if (w.length <= context * 2 + 2) return { op: r.op, text: r.text };
    const head = w.slice(0, context).join(" ");
    const tail = w.slice(-context).join(" ");
    if (i === 0) return { op: r.op, text: `… ${tail}` };
    if (i === runs.length - 1) return { op: r.op, text: `${head} …` };
    return { op: r.op, text: `${head} … ${tail}` };
  });
}

// ---- version track (Compare's bottom strip) ---------------------------------------------------------

export interface TrackDay {
  key: string;
  label: string;
  items: VersionInfo[];
}

/** Oldest first, grouped by day: the order the strip draws them in. */
export function trackDays(all: VersionInfo[], now: number): TrackDay[] {
  const sorted = [...all].sort((a, b) => a.created - b.created || (a.id < b.id ? -1 : 1));
  const days: TrackDay[] = [];
  for (const v of sorted) {
    const label = dayLabel(v.created, now);
    const last = days[days.length - 1];
    if (last && last.label === label) last.items.push(v);
    else days.push({ key: `${label}:${v.id}`, label, items: [v] });
  }
  return days;
}

/** The "Compare content" switches of the setup page. */
export interface SetupContent {
  text: boolean;
  formatting: boolean;
  images: boolean;
}

/** The categories the result starts with: everything, unless a switch is off (then undefined = all). */
export function setupShow(on: SetupContent): Category[] | undefined {
  if (on.text && on.formatting && on.images) return undefined;
  const off = new Set<Category>();
  if (!on.text) off.add("text");
  if (!on.formatting) off.add("formatting");
  if (!on.images) off.add("images");
  return CATEGORIES.map((c) => c.id).filter((id) => !off.has(id));
}
