import type { IconName } from "../components/Icons";
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

// ---- layouts: tiles and the preview pane ----------------------------------------------------------------------

export type Layout = "details" | "tiles";

/** The layout a collection opens in: /View T is tiles; D, H and C (a custom one is not drawn) are Details. */
export const defaultLayout = (view: PortfolioInfo["view"]): Layout => (view === "tile" ? "tiles" : "details");

/** One level of the tiles: the folders directly in `folder` (name order), then its files in the sort. */
export function tilesAt(info: PortfolioInfo, sort: PortfolioSortKey[], folder: number | null): Row[] {
  // files per folder, counted once, then rolled up the folder tree (parents come before children)
  const total = new Map<number, number>();
  for (const f of info.files) if (f.folder != null) total.set(f.folder, (total.get(f.folder) ?? 0) + 1);
  for (const k of [...info.folders].reverse()) if (k.parent != null) total.set(k.parent, (total.get(k.parent) ?? 0) + (total.get(k.id) ?? 0));
  const out: Row[] = [];
  for (const f of info.folders) if (f.parent === folder) out.push({ kind: "folder", folder: f, depth: 0, open: false, count: total.get(f.id) ?? 0 });
  for (const f of sortFiles(info.files.filter((f) => f.folder === folder), sort)) out.push({ kind: "file", file: f, depth: 0 });
  return out;
}

/** The folders from the top down to `folder` (empty at the top level): the breadcrumb of the tiles. */
export function crumbs(info: PortfolioInfo, folder: number | null): PortfolioFolder[] {
  const out: PortfolioFolder[] = [];
  for (let at = folder, n = 0; at != null && n < 33; n++) {
    const f = info.folders.find((k) => k.id === at);
    if (!f) break;
    out.unshift(f);
    at = f.parent;
  }
  return out;
}

/** Where an arrow key (or Home/End) takes the selection in a grid `cols` wide; the same index when it cannot move. */
export function gridMove(at: number, key: string, cols: number, count: number): number {
  if (count <= 0) return -1;
  const last = count - 1;
  if (at < 0) return key === "End" ? last : 0;
  const c = Math.max(1, cols);
  switch (key) {
    case "ArrowRight":
      return Math.min(last, at + 1);
    case "ArrowLeft":
      return Math.max(0, at - 1);
    case "ArrowDown":
      // a short last row is reached from the row above it, at its end
      return at + c <= last ? at + c : Math.floor(at / c) < Math.floor(last / c) ? last : at;
    case "ArrowUp":
      return at - c >= 0 ? at - c : at;
    case "Home":
      return 0;
    case "End":
      return last;
    default:
      return at;
  }
}

/** The visible fields of the collection with their column index, in the author's order. */
export const visibleFields = (info: PortfolioInfo) => info.fields.map((f, i) => ({ f, i })).filter((c) => c.f.visible);

/** What a tile says under the file's name: the first visible field that is not the file name itself. */
export function tileField(info: PortfolioInfo, file: PortfolioFile): { label: string; value: string } | null {
  const c = visibleFields(info).find(({ f }) => f.subtype !== "F");
  return c ? { label: c.f.name, value: file.cells[c.i] ?? "" } : null;
}

/** Every visible field of a file with its value: the preview pane's list. */
export const paneFields = (info: PortfolioInfo, file: PortfolioFile) =>
  visibleFields(info).map(({ f, i }) => ({ key: f.key, label: f.name, value: file.cells[i] ?? "", number: f.kind === "number" }));

const ICONS: [RegExp, IconName][] = [
  [/^(txt|md|rtf|doc|docx|odt|pages|log)$/, "article"],
  [/^(csv|tsv|xls|xlsx|ods|numbers)$/, "grid"],
  [/^(json|xml|html?|js|ts|css|py|rs|yml|yaml|toml|ini)$/, "code"],
];

/** The glyph for a file in a tile or the pane; every name is one of src/assets/icons/manifest.json. */
export function fileIcon(file: Pick<PortfolioFile, "name">): IconName {
  const ext = /\.([A-Za-z0-9]{1,8})$/.exec(file.name)?.[1].toLowerCase() ?? "";
  return ICONS.find(([re]) => re.test(ext))?.[1] ?? "file";
}

/** The extension as a small badge on the icon ("" when there is none). */
export const extBadge = (file: Pick<PortfolioFile, "name">) => /\.([A-Za-z0-9]{1,5})$/.exec(file.name)?.[1].toUpperCase() ?? "";
