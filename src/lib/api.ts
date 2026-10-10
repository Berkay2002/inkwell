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

export const openDocument = (path: string) => invoke<OpenOutcome>("open_document", { path });
export const authenticate = (doc: number, password: string) => invoke<OpenInfo>("authenticate", { doc, password });
export const closeDocument = (doc: number) => invoke<void>("close_document", { doc });
export const outline = (doc: number) => invoke<OutlineItem[]>("outline", { doc });
export const pageText = (doc: number, page: number) => invoke<TextLine[]>("page_text", { doc, page });
export const cancelSearch = (doc: number) => invoke<void>("cancel_search", { doc });
export const engineStats = () => invoke<EngineStats>("engine_stats");
export const initialFiles = () => invoke<string[]>("initial_files");

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
