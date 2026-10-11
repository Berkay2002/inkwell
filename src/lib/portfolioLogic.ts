import type { PortfolioFile, PortfolioFolder, PortfolioInfo, PortfolioSortKey } from "./api";

/**
 * Portfolio view logic: what is shown in which order. The engine owns the values and the collation: every
 * file carries its rank per field, so sorting here is a numeric sort of those ranks and never compares text.
 */

/** The next sort after a click on a column header: the same column flips, another starts ascending. */
export function nextSort(current: PortfolioSortKey[], field: number): PortfolioSortKey[] {
  const first = current[0];
  return [{ field, ascending: first?.field === field ? !first.ascending : true }];
}

/** `files` in the order of `sort`; a missing value goes last either way, and ties keep the engine's order. */
export function sortFiles(files: PortfolioFile[], sort: PortfolioSortKey[]): PortfolioFile[] {
  if (sort.length === 0) return files;
  const at = new Map(files.map((f, i) => [f, i]));
  return [...files].sort((a, b) => {
    for (const k of sort) {
      const x = a.ranks[k.field];
      const y = b.ranks[k.field];
      if (x == null && y == null) continue;
      if (x == null) return 1;
      if (y == null) return -1;
      if (x !== y) return k.ascending ? x - y : y - x;
    }
    return (at.get(a) ?? 0) - (at.get(b) ?? 0);
  });
}

export type Row =
  | { kind: "folder"; folder: PortfolioFolder; depth: number; open: boolean; count: number }
  | { kind: "file"; file: PortfolioFile; depth: number };

/** The rows of the list: folders (open ones followed by their contents), then the files at that level. */
export function rows(info: PortfolioInfo, sort: PortfolioSortKey[], open: ReadonlySet<number>): Row[] {
  const files = sortFiles(info.files, sort);
  const inFolder = new Map<number | null, PortfolioFile[]>();
  for (const f of files) {
    const l = inFolder.get(f.folder);
    if (l) l.push(f);
    else inFolder.set(f.folder, [f]);
  }
  const kids = new Map<number | null, PortfolioFolder[]>();
  for (const f of info.folders) {
    const l = kids.get(f.parent);
    if (l) l.push(f);
    else kids.set(f.parent, [f]);
  }
  const count = (id: number): number =>
    (inFolder.get(id)?.length ?? 0) + (kids.get(id) ?? []).reduce((n, k) => n + count(k.id), 0);
  const out: Row[] = [];
  const walk = (parent: number | null, depth: number) => {
    for (const folder of kids.get(parent) ?? []) {
      const isOpen = open.has(folder.id);
      out.push({ kind: "folder", folder, depth, open: isOpen, count: count(folder.id) });
      if (isOpen) walk(folder.id, depth + 1);
    }
    for (const file of inFolder.get(parent) ?? []) out.push({ kind: "file", file, depth });
  };
  walk(null, 0);
  return out;
}

/** What "Open" does for a file: a PDF opens as a document; anything else is only ever extracted. */
export function openAction(file: Pick<PortfolioFile, "isPdf" | "hasData">): "open" | "extract" {
  return file.isPdf && file.hasData ? "open" : "extract";
}

/** The type of a file for the confirmation that names it: its extension, or its media type. */
export function typeLabel(file: Pick<PortfolioFile, "name" | "mime">): string {
  const m = /\.([A-Za-z0-9]{1,8})$/.exec(file.name);
  const ext = m ? m[1].toUpperCase() : null;
  const mime = file.mime && file.mime !== "application/octet-stream" ? file.mime : null;
  if (ext && mime) return `${ext} (${mime})`;
  return ext ?? mime ?? "unknown type";
}

/** What the store's `selected` holds for a row: a file's id, or `d:<id>` for a folder. */
export const rowKey = (r: Row) => (r.kind === "file" ? r.file.id : `d:${r.folder.id}`);
