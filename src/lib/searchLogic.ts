// Find: the options, the query history, and the pure helpers the find box and the Search rail share.
// No store imports here: store.ts and the rail both build on it.

export type Rect4 = [number, number, number, number];
export type HitSource = "text" | "comment" | "field" | "bookmark";

/** One match as the engine reports it, with the words around it. */
export interface Found {
  /** One rectangle per line the match covers, in unrotated page points; none for a bookmark. */
  rects: Rect4[];
  before: string;
  text: string;
  after: string;
  source: HitSource;
}

/** A match with its page (0-based). */
export interface Hit extends Found {
  page: number;
}

export interface FindOpts {
  caseSensitive: boolean;
  wholeWords: boolean;
  ignoreAccents: boolean;
  comments: boolean;
  bookmarks: boolean;
  fields: boolean;
}

export const DEFAULT_OPTS: FindOpts = {
  caseSensitive: false,
  wholeWords: false,
  ignoreAccents: false,
  comments: false,
  bookmarks: false,
  fields: false,
};

export const HISTORY_MAX = 10;
export const OPTS_KEY = "inkwell.find.opts";
export const HISTORY_KEY = "inkwell.find.history";

const KEYS = Object.keys(DEFAULT_OPTS) as (keyof FindOpts)[];

/** The saved options; anything missing or of the wrong type falls back to the default. */
export function parseOpts(raw: string | null): FindOpts {
  const out = { ...DEFAULT_OPTS };
  if (!raw) return out;
  try {
    const v = JSON.parse(raw) as Record<string, unknown>;
    for (const k of KEYS) if (typeof v[k] === "boolean") out[k] = v[k] as boolean;
  } catch {
    /* defaults */
  }
  return out;
}

/** The saved history (newest first): strings only, tidied, no repeats, at most `HISTORY_MAX`. */
export function parseHistory(raw: string | null): string[] {
  if (!raw) return [];
  try {
    const v = JSON.parse(raw);
    if (!Array.isArray(v)) return [];
    const out: string[] = [];
    for (const q of v) {
      const t = typeof q === "string" ? tidy(q) : "";
      if (t && !out.includes(t)) out.push(t);
    }
    return out.slice(0, HISTORY_MAX);
  } catch {
    return [];
  }
}

const tidy = (q: string) => q.replace(/\s+/g, " ").trim();

/** `list` (newest first) with `query` put in front. Blank queries are ignored and a repeat moves up. */
export function pushHistory(list: string[], query: string): string[] {
  const q = tidy(query);
  if (!q) return list;
  return [q, ...list.filter((x) => x !== q)].slice(0, HISTORY_MAX);
}

/** How many options differ from the defaults (the badge on the "…" button). */
export const changedOptions = (o: FindOpts) => KEYS.filter((k) => o[k] !== DEFAULT_OPTS[k]).length;

/** Part of the key a search was run with: a changed option runs the search again. */
export const optsKey = (o: FindOpts) => KEYS.map((k) => (o[k] ? "1" : "0")).join("");

/** What the engine accepts: only the options, camelCase as stored. */
export const engineOpts = (o: FindOpts) => ({ ...o });

export interface PageGroup {
  page: number;
  /** Items with their index in the full, sorted hit list. */
  items: { index: number; hit: Hit }[];
}

/** Hits (sorted by page) grouped under their page. */
export function groupByPage(hits: Hit[]): PageGroup[] {
  const out: PageGroup[] = [];
  hits.forEach((hit, index) => {
    const last = out[out.length - 1];
    if (last && last.page === hit.page) last.items.push({ index, hit });
    else out.push({ page: hit.page, items: [{ index, hit }] });
  });
  return out;
}

/**
 * `hits` (sorted) with `incoming` merged in, and where `keep` (one of `hits`) ended up (-1 for none). Pages report
 * in order, so the common case is a plain append; otherwise it is one linear merge, never a full sort.
 */
export function mergeHits(hits: Hit[], incoming: Hit[], keep: Hit | null): { hits: Hit[]; keep: number } {
  if (incoming.length === 0) return { hits, keep: keep ? hits.indexOf(keep) : -1 };
  const add = incoming.length > 1 ? [...incoming].sort(byPosition) : incoming;
  const last = hits[hits.length - 1];
  if (!last || byPosition(last, add[0]) <= 0) {
    return { hits: hits.concat(add), keep: keep ? hits.indexOf(keep) : -1 };
  }
  const out: Hit[] = new Array(hits.length + add.length);
  let i = 0;
  let j = 0;
  let k = 0;
  let at = -1;
  while (i < hits.length || j < add.length) {
    const takeOld = j >= add.length || (i < hits.length && byPosition(hits[i], add[j]) <= 0);
    const h = takeOld ? hits[i++] : add[j++];
    if (takeOld && h === keep) at = k;
    out[k++] = h;
  }
  return { hits: out, keep: at };
}

/** Reading order: page, then top to bottom, then left to right; a bookmark comes before the text of its page. */
export function byPosition(a: Hit, b: Hit): number {
  const ay = a.rects[0]?.[1] ?? -1;
  const by = b.rects[0]?.[1] ?? -1;
  return a.page - b.page || ay - by || (a.rects[0]?.[0] ?? 0) - (b.rects[0]?.[0] ?? 0);
}

const plural = (n: number, one: string, many: string) => `${n.toLocaleString("en-US")} ${n === 1 ? one : many}`;

/** "12 results on 5 pages", "No results", "Searching…". */
export function resultsLine(hits: number, pages: number, done: boolean): string {
  if (hits === 0) return done ? "No results" : "Searching…";
  return `${plural(hits, "result", "results")} on ${plural(pages, "page", "pages")}${done ? "" : " so far"}`;
}

/** The hit to start on: the one at `focus` (a page and a y) when given, else the first at or after the page in view. */
export function pickCurrent(hits: Hit[], currentPage: number, focus?: { page: number; y: number }): number {
  if (hits.length === 0) return -1;
  if (focus) {
    let best = -1;
    let dist = Infinity;
    hits.forEach((h, i) => {
      if (h.page !== focus.page) return;
      const d = Math.abs((h.rects[0]?.[1] ?? 0) - focus.y);
      if (d < dist) {
        dist = d;
        best = i;
      }
    });
    if (best !== -1) return best;
  }
  const i = hits.findIndex((h) => h.page >= currentPage);
  return i === -1 ? 0 : i;
}

/** Where the viewer scrolls to for a hit: a little above it. */
export const hitY = (h: Hit) => Math.max(0, (h.rects[0]?.[1] ?? 0) - 80);

export const SOURCE_LABEL: Record<HitSource, string> = {
  text: "",
  comment: "Comment",
  field: "Form field",
  bookmark: "Bookmark",
};

// ---- across files -----------------------------------------------------------------------------------

/** A file of the folder search that has hits. `items` lists the first of them; `hits` counts all. */
export interface FileGroup {
  index: number;
  path: string;
  name: string;
  pageCount: number;
  hits: number;
  pages: number;
  truncated: boolean;
  items: Hit[];
}

/** Puts `g` in `list` by its place in the folder listing (workers finish out of order). */
export function insertFile(list: FileGroup[], g: FileGroup): FileGroup[] {
  const i = list.findIndex((x) => x.index > g.index);
  return i === -1 ? [...list, g] : [...list.slice(0, i), g, ...list.slice(i)];
}

/** The last `n` folders of the directory a file is in, for the quiet line under its name. */
export function shortDir(path: string, n = 2): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  parts.pop();
  return parts.slice(-n).join("\\");
}

/** The folder's own name for the rail ("Housing"), with the full path left to the tooltip. */
export const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

/** Items shown per file at first, and how many each "Show more" adds. */
export const FIRST_ITEMS = 2;
export const MORE_ITEMS = 10;

/** The rows of the folder results, in order, as (file, item) pairs: what arrow keys walk. */
export function visibleRows(files: FileGroup[], shown: Record<number, number>): { file: number; item: number }[] {
  const rows: { file: number; item: number }[] = [];
  files.forEach((g, file) => {
    const n = Math.min(g.items.length, shown[g.index] ?? FIRST_ITEMS);
    for (let item = 0; item < n; item++) rows.push({ file, item });
  });
  return rows;
}

export function filesLine(matched: number, hits: number, scanned: number, total: number, done: boolean, cancelled: boolean): string {
  if (!done) return `Searching ${scanned.toLocaleString("en-US")} of ${total.toLocaleString("en-US")} files`;
  const where = `${plural(total, "file", "files")}`;
  if (matched === 0) return cancelled ? `Stopped after ${scanned.toLocaleString("en-US")} of ${where}` : `No results in ${where}`;
  const head = `${plural(hits, "result", "results")} in ${plural(matched, "file", "files")}`;
  return cancelled ? `${head} (stopped at ${scanned.toLocaleString("en-US")} of ${total.toLocaleString("en-US")})` : head;
}

// ---- catalogs ---------------------------------------------------------------------------------------

const MB = 1024 * 1024;

/** "~40 MB", "~3 KB": the rough size the build will write. */
export const approxSize = (bytes: number) => (bytes >= MB ? `~${Math.max(1, Math.round(bytes / MB))} MB` : `~${Math.max(1, Math.round(bytes / 1024))} KB`);

/** "about 2 minutes", "under a minute": a time as people say it, rounded up. */
export function approxTime(secs: number): string {
  if (secs < 45) return "under a minute";
  const m = Math.round(secs / 60);
  if (m < 60) return `about ${plural(m, "minute", "minutes")}`;
  const h = Math.round((secs / 3600) * 2) / 2;
  return `about ${h === 1 ? "an hour" : `${h} hours`}`;
}

/** The sheet's line before a build: "~1 800 files, about 2 minutes, ~40 MB". */
export function estimateLine(e: { files: number; secs: number; indexBytes: number }): string {
  if (e.files === 0) return "No PDFs in this folder";
  return `~${e.files.toLocaleString("en-US").replace(/,/g, " ")} ${e.files === 1 ? "file" : "files"}, ${approxTime(e.secs)}, ${approxSize(e.indexBytes)}`;
}

/** The badge beside a catalog: what a refresh would change, or "" when nothing. */
export function staleLine(s: { added: number; changed: number; removed: number; unusable: boolean } | undefined, built: number): string {
  if (!s) return "";
  if (s.unusable) return built === 0 ? "Not built yet" : "Needs a rebuild";
  const parts: string[] = [];
  if (s.changed) parts.push(`${s.changed} changed`);
  if (s.added) parts.push(`${s.added} new`);
  if (s.removed) parts.push(`${s.removed} gone`);
  return parts.join(", ");
}

/** What a finished build says: "1 804 files, 12 new, 3 changed; 2 could not be read". */
export function buildLine(b: { files: number; added: number; changed: number; removed: number; damaged: number; password: number; cancelled: boolean }): string {
  if (b.cancelled) return "Stopped. The previous index is still in use.";
  const head = [`${plural(b.files, "file", "files")}`];
  if (b.added) head.push(`${b.added} new`);
  if (b.changed) head.push(`${b.changed} changed`);
  if (b.removed) head.push(`${b.removed} removed`);
  const bad = b.damaged + b.password;
  return head.join(", ") + (bad ? `; ${bad} could not be read` : "");
}
