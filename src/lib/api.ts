import { Channel, invoke } from "@tauri-apps/api/core";
import type { Size } from "./layout";

export interface OpenInfo {
  doc: number;
  path: string;
  pageCount: number;
  title: string | null;
  firstPage: Size;
}

export type OpenOutcome =
  | ({ status: "opened" } & OpenInfo)
  | { status: "needsPassword"; doc: number; path: string };

export interface OutlineItem {
  title: string;
  page: number | null;
  uri: string | null;
  children: OutlineItem[];
}

export interface TextLine {
  /** x0, y0, x1, y1 in unrotated page points. */
  b: [number, number, number, number];
  /** Font size in points. */
  s: number;
  t: string;
}

export type SearchEvent =
  | { kind: "hits"; search: number; page: number; rects: [number, number, number, number][] }
  | { kind: "done"; search: number; hits: number; pages: number };

export interface EngineStats {
  cacheBytes: number;
  cacheTiles: number;
  lists: number;
  queued: number;
  rendered: number;
  renderMsTotal: number;
  hits: number;
  misses: number;
}

/**
 * A file the user chose. Commands take the token only (src-tauri/src/paths.rs); `path` is for display
 * and for telling open tabs apart. Open tokens can be reused, save tokens work once.
 */
export interface FileRef {
  token: string;
  path: string;
}

/** Open dialog (run by Rust). */
export const pickOpen = (multiple: boolean, title?: string) => invoke<FileRef[]>("pick_open", { multiple, title: title ?? null });
/** Save dialog (run by Rust) in the folder of document `doc` or of open token `near`, with file name `name`. */
export const pickSave = (name: string, at: { doc?: number; near?: string } = {}) =>
  invoke<FileRef | null>("pick_save", { name, doc: at.doc ?? null, near: at.near ?? null });
/** Open token for a path the user chose before (recent list); Rust refuses any other path. */
export const knownFile = (path: string) => invoke<FileRef>("known_file", { path });
export const openDocument = (file: string) => invoke<OpenOutcome>("open_document", { file });
export const authenticate = (doc: number, password: string) => invoke<OpenInfo>("authenticate", { doc, password });
export const closeDocument = (doc: number) => invoke<void>("close_document", { doc });
export const outline = (doc: number) => invoke<OutlineItem[]>("outline", { doc });
export const pageText = (doc: number, page: number) => invoke<TextLine[]>("page_text", { doc, page });
export const cancelSearch = (doc: number) => invoke<void>("cancel_search", { doc });
export const engineStats = () => invoke<EngineStats>("engine_stats");
export const initialFiles = () => invoke<FileRef[]>("initial_files");

export function pageSizes(doc: number, onChunk: (start: number, sizes: Size[]) => void) {
  const ch = new Channel<{ start: number; sizes: Size[] }>();
  ch.onmessage = (m) => onChunk(m.start, m.sizes);
  return invoke<void>("page_sizes", { doc, onChunk: ch });
}

export function search(doc: number, query: string, onEvent: (e: SearchEvent) => void) {
  const ch = new Channel<SearchEvent>();
  ch.onmessage = onEvent;
  return invoke<number>("search", { doc, query, onEvent: ch });
}

/** Origin of the tile protocol: WebView2 serves custom schemes as http://<scheme>.localhost. */
export const TILE_ORIGIN = navigator.userAgent.includes("Windows") ? "http://inkwell.localhost" : "inkwell://localhost";

export interface TileData {
  w: number;
  h: number;
  rgba: Uint8ClampedArray;
}

/** Resolves to null when the engine reports the request as superseded (HTTP 410). */
export async function fetchTile(
  doc: number,
  page: number,
  rot: number,
  scale: number,
  tx: number,
  ty: number,
  epoch: number,
  rank: number,
  stream: number,
  rev: number,
): Promise<TileData | null> {
  const url = `${TILE_ORIGIN}/t/${doc}/${page}/${rot}/${scale}/${tx}/${ty}?e=${epoch}&r=${rank}&s=${stream}&g=${rev}`;
  const res = await fetch(url);
  if (res.status === 410) return null;
  if (!res.ok) throw new Error(`tile ${res.status}`);
  const w = Number(res.headers.get("x-tile-w"));
  const h = Number(res.headers.get("x-tile-h"));
  const buf = await res.arrayBuffer();
  return { w, h, rgba: new Uint8ClampedArray(buf) };
}

// ---- editing (src-tauri/src/engine/edit.rs) ----

/** Page numbers are 0-based indices into the current generation. */
export type EditOp =
  | { op: "rotate"; pages: number[]; quarterTurns: number }
  | { op: "delete"; pages: number[] }
  /** `to`: the gap before page `to`, counted before the move (`pageCount` = the end). */
  | { op: "move"; pages: number[]; to: number }
  | { op: "insertBlank"; at: number; like: number }
  /** `file`: an open token. */
  | { op: "insertFile"; at: number; file: string; pages?: number[] | null; password?: string | null };

export interface EditState {
  rev: number;
  pageCount: number;
  sizes: Size[];
  path: string;
  dirty: boolean;
  /** Labels of what Undo / Redo would do. */
  undo: string | null;
  redo: string | null;
  redacted: boolean;
}

export const editApply = (doc: number, op: EditOp) => invoke<EditState>("edit_apply", { doc, op });
export const editUndo = (doc: number) => invoke<EditState>("edit_undo", { doc });
export const editRedo = (doc: number) => invoke<EditState>("edit_redo", { doc });
/** `to`: a save token (Save as). */
export const saveDocument = (doc: number, to?: string) => invoke<EditState>("save_document", { doc, to: to ?? null });
/** `out`: a save token. */
export const extractPages = (doc: number, pages: number[], out: string, deleteAfter: boolean) =>
  invoke<EditState>("extract_pages", { doc, pages, out, deleteAfter });
export const splitDocument = (doc: number, every: number) => invoke<{ files: string[] }>("split_document", { doc, every });
/** `files`: open tokens; `out`: a save token. Returns the result to open. */
export const mergeFiles = (files: string[], out: string) => invoke<FileRef>("merge_files", { files, out });
