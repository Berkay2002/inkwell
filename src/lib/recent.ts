// Pure logic of the start screen's recent list. No DOM, no Tauri: unit-tested in recent.test.ts.

export interface Recent {
  /** Path to open and show: canonical with on-disk casing once the file was seen by the shell. */
  path: string;
  /** Last opened, ms since epoch. */
  opened: number;
  pinned?: boolean;
  pages?: number;
  size?: number;
  /** File modified time when `thumb` was made; a different value means the thumbnail is stale. */
  mtime?: number;
  /** First page as a small WebP data URL. */
  thumb?: string;
}

/** Unpinned entries kept; pinned ones never expire. */
export const MAX_UNPINNED = 24;

/** Identity of a file on Windows: slash style, verbatim prefix, repeated or trailing separators and case don't matter. */
export function keyOf(path: string): string {
  let p = path.replace(/\//g, "\\");
  if (p.startsWith("\\\\?\\UNC\\")) p = "\\\\" + p.slice(8);
  else if (p.startsWith("\\\\?\\")) p = p.slice(4);
  const unc = p.startsWith("\\\\");
  p = p.replace(/\\{2,}/g, "\\").replace(/\\+$/, "");
  return (unc ? "\\" + p : p).toLowerCase();
}

/** Fixtures of the test suite and the benchmarks: never worth listing. */
export const isJunk = (path: string) => /[\\/]inkwell-(tests|bench)[\\/]/i.test(path);

export const baseName = (p: string) => p.split(/[\\/]/).pop() || p;

export function dirName(p: string): string {
  const i = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
  return i > 0 ? p.slice(0, i) : "";
}

/** Last two folders, "…\Documents\Taxes": enough to recognise a file without the whole path. */
export function shortDir(dir: string): string {
  const parts = dir.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return dir;
  return `…\\${parts.slice(-2).join("\\")}`;
}

export function formatSize(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  if (bytes < 1e6) return `${Math.round(bytes / 1e3)} KB`;
  if (bytes < 1e9) return `${(bytes / 1e6).toFixed(bytes < 1e7 ? 1 : 0)} MB`;
  return `${(bytes / 1e9).toFixed(1)} GB`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

export function relTime(ts: number, now: number): string {
  const s = Math.max(0, (now - ts) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  const d = new Date(ts);
  const today = new Date(now);
  const days = Math.round((Date.UTC(today.getFullYear(), today.getMonth(), today.getDate()) - Date.UTC(d.getFullYear(), d.getMonth(), d.getDate())) / 86400000);
  if (days <= 1) return "Yesterday";
  if (days < 7) return `${days} days ago`;
  const date = `${d.getDate()} ${MONTHS[d.getMonth()]}`;
  return d.getFullYear() === today.getFullYear() ? date : `${date} ${d.getFullYear()}`;
}

/** Pinned first, then most recently opened. The input is already newest first, so the sort is stable. */
export const ordered = (list: Recent[]): Recent[] => [...list.filter((r) => r.pinned), ...list.filter((r) => !r.pinned)];

const keepPinnedAndNewest = (list: Recent[]) => {
  let unpinned = 0;
  return list.filter((r) => r.pinned || ++unpinned <= MAX_UNPINNED);
};

/** Puts `entry` first, dropping any older entry for the same file (and keeping its pin). */
export function upsert(list: Recent[], entry: Recent): Recent[] {
  const k = keyOf(entry.path);
  const prev = list.find((r) => keyOf(r.path) === k);
  const merged: Recent = { ...entry, pinned: entry.pinned ?? prev?.pinned };
  if (!merged.pinned) delete merged.pinned;
  return keepPinnedAndNewest([merged, ...list.filter((r) => keyOf(r.path) !== k)]);
}

/** Drops fixtures and duplicates (first, i.e. newest, wins). */
export function clean(list: Recent[]): Recent[] {
  const seen = new Set<string>();
  return keepPinnedAndNewest(
    list.filter((r) => {
      const k = keyOf(r.path);
      if (isJunk(r.path) || seen.has(k)) return false;
      seen.add(k);
      return true;
    }),
  );
}

/** Reads what `JSON.stringify(list)` wrote; anything malformed is dropped rather than trusted. */
export function parse(json: string | null): Recent[] | null {
  if (json == null) return null;
  try {
    const v: unknown = JSON.parse(json);
    if (!Array.isArray(v)) return [];
    return v.filter((r): r is Recent => typeof r?.path === "string" && typeof r?.opened === "number");
  } catch {
    return [];
  }
}

/** The pre-recents-store list was bare paths, newest first. */
export function fromLegacy(paths: unknown, now: number): Recent[] {
  if (!Array.isArray(paths)) return [];
  return paths.filter((p): p is string => typeof p === "string").map((path, i) => ({ path, opened: now - i }));
}
