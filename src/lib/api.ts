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
export const pickOpen = (multiple: boolean, title?: "locate" | "compare") => invoke<FileRef[]>("pick_open", { multiple, title: title ?? null });
/** What a save dialog filters for; PDF unless told otherwise. */
export type SaveKind = "pdf" | "text" | "html" | "xhtml" | "word";
/** Save dialog (run by Rust) in the folder of document `doc` or of open token `near`, with file name `name`. */
export const pickSave = (name: string, at: { doc?: number; near?: string } = {}, kind?: SaveKind) =>
  invoke<FileRef | null>("pick_save", { name, doc: at.doc ?? null, near: at.near ?? null, kind: kind ?? null });
/** Open dialog for what Create builds a PDF from: images (several) or one text file. */
export const pickSource = (kind: "images" | "text" | "digitalId", multiple: boolean) => invoke<FileRef[]>("pick_source", { kind, multiple });
/** Folder dialog (run by Rust); the token names the folder a multi-file export writes into. */
export const pickFolder = (doc?: number) => invoke<FileRef | null>("pick_folder", { doc: doc ?? null });
/** Open token for a path the user chose before (recent list); Rust refuses any other path. */
export const knownFile = (path: string) => invoke<FileRef>("known_file", { path });
/** `scratch`: a created file nobody saved yet (no version history until it is saved as). */
export const openDocument = (file: string, scratch = false) => invoke<OpenOutcome>("open_document", { file, scratch });
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

// ---- compress and OCR (src-tauri/src/tools/mod.rs) ----

export type CompressPreset = "smallest" | "balanced" | "highQuality";

export interface CompressAudit {
  total: number;
  categories: { category: "images" | "fonts" | "contentStreams" | "metadata" | "structure" | "other"; bytes: number; objects: number }[];
  images: number;
  fonts: number;
}

export interface CompressAuditReply {
  audit: CompressAudit;
  size: number;
  defaultOutput: string;
  outputExists: boolean;
}

/** Predicted size per preset, from recompressing a sample of the file's images. */
export interface CompressEstimate {
  preset: CompressPreset;
  bytes: number;
  /** The range it should fall in; equal when the sample covered every image. */
  low: number;
  high: number;
}

export interface CompressProgress {
  stage: "open" | "scan" | "images" | "fonts" | "write" | "verify";
  /** 0..1 over the whole job, never decreasing. */
  fraction: number;
  page: number;
  pages: number;
}

export interface CompressReport {
  input: string;
  output: string;
  sizeBefore: number;
  sizeAfter: number;
  savedBytes: number;
  savedPercent: number;
  keptOriginal: boolean;
  pages: number;
  /** Render comparison of before and after (8 pages at most); null when skipped. */
  visual: { maxDiff: number; meanDiff: number; psnr: number } | null;
  warnings: string[];
}

/** Documents are named by id; an output file by a save token from `pickSave`. */
export interface CompressRequest {
  doc: number;
  preset: CompressPreset;
  /** Full options from a Customize panel; wins over `preset`. */
  custom?: unknown;
  /** Save token for the output instead of `<name> (compressed).pdf` (used when that name is taken). */
  saveTo?: string;
}

export type OcrBackend = "windows" | "tesseract";

export interface OcrLanguages {
  windows: string[];
  tesseract: { path: string; languages: string[] } | null;
}

export interface OcrRequest {
  doc: number;
  /** One-based range like "1-3, 5"; omitted = every page. */
  pages?: string;
  languages: string[];
  force: boolean;
  backend: OcrBackend;
  /** Save token: write a new file instead of changing the document. */
  saveTo?: string;
}

export interface OcrProgress {
  done: number;
  total: number;
  page: number;
}

/** The engine's report as it serializes (snake_case: the AI tools read the same shape). */
export interface OcrReport {
  backend: string;
  pages_requested: number;
  pages_recognized: number;
  pages_skipped_text: number;
  pages_no_text: number;
  pages_failed: number;
  words: number;
  total_ms: number;
  warnings: string[];
}

export interface OcrOutcome {
  report: OcrReport;
  /** The document after an in-place run; null for `saveTo`. */
  state: EditState | null;
  output: string | null;
}

export const compressAudit = (doc: number) => invoke<CompressAuditReply>("compress_audit", { doc });
/** Runs on its own thread; `toolCancel(job)` stops it. */
export const compressEstimate = (job: number, doc: number) => invoke<CompressEstimate[]>("compress_estimate", { job, doc });

export function compressStart(job: number, req: CompressRequest, onProgress: (p: CompressProgress) => void) {
  const ch = new Channel<CompressProgress>();
  ch.onmessage = onProgress;
  return invoke<CompressReport>("compress_start", { job, req, onProgress: ch });
}

// ---- export and create (src-tauri/src/convert) ----

export type ImageFormat = "png" | "jpeg" | "tiff";

export type ExportKind =
  | { kind: "images"; format: ImageFormat; dpi: number; gray: boolean; quality: number }
  | { kind: "text"; layout: boolean }
  | { kind: "html"; xhtml: boolean }
  | { kind: "extractImages"; minSide: number }
  | { kind: "word" };

export interface ExportRequest {
  doc: number;
  kind: ExportKind;
  /** One-based range like "1-3, 5"; omitted = every page. */
  pages?: string;
  /** Save token: a file (`pickSave`) or, for several files, a folder (`pickFolder`). */
  saveTo: string;
}

export interface ExportOutput {
  files: string[];
  folder: string;
  bytes: number;
  pages: number;
  warnings: string[];
}

export interface ConvertProgress {
  done: number;
  total: number;
  fraction: number;
}

export type CreateSource =
  | { kind: "images"; files: string[] }
  | { kind: "text"; file: string }
  | { kind: "blank"; pages: number }
  | { kind: "clipboard" };

export type CreatePageSize = "fit" | "a4" | "letter";

export interface CreateRequest {
  source: CreateSource;
  pageSize: CreatePageSize;
  /** Points on every side. */
  margin: number;
  fontSize?: number;
}

export interface Created {
  file: FileRef;
  pages: number;
  warnings: string[];
}

export function exportStart(job: number, req: ExportRequest, onProgress: (p: ConvertProgress) => void) {
  const ch = new Channel<ConvertProgress>();
  ch.onmessage = onProgress;
  return invoke<ExportOutput>("export_start", { job, req, onProgress: ch });
}

export function createStart(job: number, req: CreateRequest, onProgress: (p: ConvertProgress) => void) {
  const ch = new Channel<ConvertProgress>();
  ch.onmessage = onProgress;
  return invoke<Created>("create_start", { job, req, onProgress: ch });
}

/** Deletes a created file that was never saved; `file` is its open token. */
export const discardScratch = (file: string) => invoke<boolean>("discard_scratch", { file });

export const toolCancel = (job: number) => invoke<void>("tool_cancel", { job });
export const ocrLanguages = () => invoke<OcrLanguages>("ocr_languages");
/** Opens a file picker (Rust side) and remembers tesseract.exe; resolves to the path or null. */
export const ocrChooseTesseract = () => invoke<string | null>("ocr_choose_tesseract");
export const ocrClearTesseract = () => invoke<void>("ocr_clear_tesseract");

export function ocrStart(job: number, req: OcrRequest, onProgress: (p: OcrProgress) => void) {
  const ch = new Channel<OcrProgress>();
  ch.onmessage = onProgress;
  return invoke<OcrOutcome>("ocr_start", { job, req, onProgress: ch });
}
