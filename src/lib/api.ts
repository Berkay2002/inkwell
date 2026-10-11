import { Channel, invoke } from "@tauri-apps/api/core";
import type { Size } from "./layout";
import type { FindOpts, Found } from "./searchLogic";

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
  | { kind: "hits"; search: number; page: number; hits: Found[] }
  | { kind: "progress"; search: number; scanned: number; total: number }
  /** `capped`: the search stopped at 5000 hits; the ones listed are the first. */
  | { kind: "done"; search: number; hits: number; pages: number; capped: boolean };

/** What a search across files reports (src-tauri/src/search/files.rs). */
export type FilesEvent =
  /** The search exists: this id cancels it, even while the folder is still being listed. */
  | { kind: "started"; search: number }
  | {
      kind: "file";
      search: number;
      index: number;
      path: string;
      name: string;
      pageCount: number;
      hits: number;
      pages: number;
      truncated: boolean;
      items: (Found & { page: number })[];
    }
  | { kind: "progress"; search: number; scanned: number; total: number }
  | {
      kind: "done";
      search: number;
      files: number;
      matched: number;
      hits: number;
      skipped: number;
      /** PDFs left unread because they are only in the cloud (OneDrive placeholders). */
      cloudOnly: number;
      cancelled: boolean;
      capped: boolean;
    };

export type SearchScope = { kind: "folder"; token: string; recursive: boolean } | { kind: "files"; tokens: string[] };

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
export const pickOpen = (multiple: boolean, title?: "locate" | "compare" | "search") => invoke<FileRef[]>("pick_open", { multiple, title: title ?? null });
/** What a save dialog filters for; PDF unless told otherwise. */
export type SaveKind = "pdf" | "text" | "html" | "xhtml" | "word" | "any";
/** Save dialog (run by Rust) in the folder of document `doc` or of open token `near`, with file name `name`. */
export const pickSave = (name: string, at: { doc?: number; near?: string } = {}, kind?: SaveKind) =>
  invoke<FileRef | null>("pick_save", { name, doc: at.doc ?? null, near: at.near ?? null, kind: kind ?? null });
/** Open dialog for what Create builds a PDF from: images (several) or one text file. */
export const pickSource = (kind: "images" | "text", multiple: boolean) => invoke<FileRef[]>("pick_source", { kind, multiple });
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

export function search(doc: number, query: string, opts: FindOpts, onEvent: (e: SearchEvent) => void) {
  const ch = new Channel<SearchEvent>();
  ch.onmessage = onEvent;
  return invoke<number>("search", { doc, query, opts, onEvent: ch });
}

/** Folder dialog (run by Rust) for search across files; the token names the folder to read. */
export const pickSearchFolder = () => invoke<FileRef | null>("pick_search_folder");

/** Starts a search of several files; resolves with the search id once it is running. */
export function searchFiles(scope: SearchScope, query: string, opts: FindOpts, onEvent: (e: FilesEvent) => void) {
  const ch = new Channel<FilesEvent>();
  ch.onmessage = onEvent;
  return invoke<number>("search_files", { scope, query, opts, onEvent: ch });
}
export const cancelSearchFiles = (search: number) => invoke<void>("cancel_search_files", { search });
/** An open token for file `index` of search `search` (a result being opened); the path never reaches the page. */
export const searchFileToken = (search: number, index: number) => invoke<FileRef>("search_file_token", { search, index });

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
export const editState = (doc: number) => invoke<EditState>("edit_state", { doc });

// ---- editing existing text (src-tauri/src/textedit, docs/text-edit.md) ----

export type TextAlign = "left" | "center" | "right" | "justify";
/** Why a block can't be edited. `hidden` text is not offered at all. */
export type TextIssue = "type3" | "noUnicode" | "vertical" | "encoding" | "clip" | "invisible" | "hidden";

export interface TextBlock {
  /** Pass to `textEdit` with the `rev` of the list it came from. */
  index: number;
  text: string;
  /** x0, y0, x1, y1 in page space (the page as shown, y down). */
  rect: [number, number, number, number];
  lines: [number, number, number, number][];
  /** `/BaseFont`, the subset prefix kept. */
  font: string;
  size: number;
  bold: boolean;
  italic: boolean;
  /** RGB 0..1. */
  color: [number, number, number];
  align: TextAlign;
  /** Distance between lines over the font size; null for a single line. */
  lineSpacing: number | null;
  editable: boolean;
  issue: TextIssue | null;
  reason: string | null;
}

export interface TextArea {
  rect: [number, number, number, number];
  issue: "inForm" | "outlines";
  reason: string;
}

export interface TextBlocks {
  rev: number;
  blocks: TextBlock[];
  areas: TextArea[];
}

/** Unset fields keep the block's own. `offset` and `width` are in page space. */
export interface BlockChange {
  text?: string;
  size?: number;
  color?: [number, number, number];
  align?: TextAlign;
  offset?: [number, number];
  width?: number;
  lineSpacing?: number;
  bold?: boolean;
  italic?: boolean;
}

/** Set when the block's font could not show the text and another one was used. */
export interface TextFallback {
  font: string;
  embedded: boolean;
  reason: string;
}

export interface TextEdited {
  state: EditState;
  fallback: TextFallback | null;
}

export interface NewTextBox {
  text: string;
  size: number;
  color: [number, number, number];
  /** Top left of the box in page space. */
  at: [number, number];
  /** Wrap to this width; unset keeps the lines as typed. */
  width?: number;
}

export const textBlocks = (doc: number, page: number) => invoke<TextBlocks>("text_blocks", { doc, page });
export const textEdit = (doc: number, rev: number, page: number, index: number, change: BlockChange) =>
  invoke<TextEdited>("text_edit", { doc, rev, page, index, change });
export const textAdd = (doc: number, rev: number, page: number, newText: NewTextBox) =>
  invoke<EditState>("text_add", { doc, rev, page, new: newText });
/** `to`: a save token (Save as). */
export const saveDocument = (doc: number, to?: string) => invoke<EditState>("save_document", { doc, to: to ?? null });
/** `out`: a save token. */
export const extractPages = (doc: number, pages: number[], out: string, deleteAfter: boolean) =>
  invoke<EditState>("extract_pages", { doc, pages, out, deleteAfter });
export const splitDocument = (doc: number, every: number) => invoke<{ files: string[] }>("split_document", { doc, every });
/** `files`: open tokens; `out`: a save token. Returns the result to open. */
export const mergeFiles = (files: string[], out: string) => invoke<FileRef>("merge_files", { files, out });

// ---- password security (src-tauri/src/protect) ----

export type Printing = "none" | "low" | "high";
export type Changes = "none" | "pages" | "forms" | "comments" | "allButExtract";
export type Cipher = "aes256" | "aes128";

/** What to set. Passwords stay in this object only until the command returns. */
export interface ProtectSpec {
  openPassword: string | null;
  permissionsPassword: string | null;
  printing: Printing;
  changes: Changes;
  copy: boolean;
  accessibility: boolean;
  cipher: Cipher;
}

export interface Rights {
  printing: Printing;
  modify: boolean;
  annotate: boolean;
  forms: boolean;
  assemble: boolean;
  copy: boolean;
  accessibility: boolean;
}

export interface SecurityInfo {
  encrypted: boolean;
  /** "AES-256", "AES-128", "RC4 128-bit" ... */
  method: string | null;
  revision: number | null;
  openPassword: boolean;
  rights: Rights;
  restricted: boolean;
  /** This session may change or remove the security without a permissions password. */
  unlocked: boolean;
  /** Where it comes from: the file as opened, set in this session, or removed in this session. */
  origin: "file" | "set" | "removed";
  /** Set or removed here and not saved yet. */
  unsaved: boolean;
}

export const securityInfo = (doc: number) => invoke<SecurityInfo>("security_info", { doc });
/** One undo step; Save or Save as writes it. `permissionsPassword`: only when the file already has restrictions. */
export const protectApply = (doc: number, spec: ProtectSpec, permissionsPassword?: string | null) =>
  invoke<EditState>("protect_apply", { doc, spec, permissionsPassword: permissionsPassword ?? null });
/** Protects and saves in one step (over the file, or to a `pick_save` token); a failed save takes the protection back. */
export const protectAndSave = (doc: number, spec: ProtectSpec, permissionsPassword?: string | null, to?: string) =>
  invoke<EditState>("protect_and_save", { doc, spec, permissionsPassword: permissionsPassword ?? null, to: to ?? null });
export const removeSecurity = (doc: number, permissionsPassword?: string | null) =>
  invoke<EditState>("remove_security", { doc, permissionsPassword: permissionsPassword ?? null });

// ---- page marks (src-tauri/src/marks) ----

export type MarkKind = "headerFooter" | "watermark" | "background" | "bates";
export type FontFamily = "helvetica" | "times" | "courier";
export type Rgb = [number, number, number];

export interface MarkStyle {
  family: FontFamily;
  bold: boolean;
  italic: boolean;
  size: number;
  color: Rgb;
  underline: boolean;
}

/** A watermark or background picture: `file` is an open token (`pickOpen`); `page` is 0-based, for a PDF. */
export interface MarkPicture {
  file: string;
  page: number;
  password?: string | null;
}

/** Top left, top centre, top right, bottom left, bottom centre, bottom right. */
export type SixTexts = [string, string, string, string, string, string];

export type PageMark =
  | { kind: "headerFooter"; text: SixTexts; style: MarkStyle; margins: [number, number, number, number]; startNumber: number }
  | {
      kind: "watermark";
      text: string;
      picture: MarkPicture | null;
      style: MarkStyle;
      scale: number;
      opacity: number;
      rotation: number;
      behind: boolean;
      offset: [number, number];
      showOnScreen: boolean;
      showInPrint: boolean;
    }
  | { kind: "background"; color: Rgb; opacity: number; picture: MarkPicture | null; scale: number }
  | {
      kind: "bates";
      prefix: string;
      suffix: string;
      digits: number;
      start: number;
      slot: number;
      style: MarkStyle;
      margins: [number, number, number, number];
    };

export interface MarkDate {
  year: number;
  month: number;
  day: number;
}

export interface MarkRequest {
  mark: PageMark;
  /** 0-based. */
  pages: number[];
  replace: boolean;
  date: MarkDate;
}

export interface MarkKindInfo {
  kind: MarkKind;
  pages: number[];
  /** The settings it was made with, when this app made it. */
  mark: PageMark | null;
}

export interface BatesFileResult {
  input: string;
  output: string;
  pages: number;
  first: number;
  last: number;
}

export const marksInfo = (doc: number) => invoke<{ kinds: MarkKindInfo[] }>("marks_info", { doc });
/** One undo step: Add, or Update when `replace`. */
export const marksAdd = (doc: number, request: MarkRequest) => invoke<EditState>("marks_add", { doc, request });
/** One undo step; all pages when `pages` is omitted. */
export const marksRemove = (doc: number, kind: MarkKind, pages?: number[]) =>
  invoke<EditState>("marks_remove", { doc, kind, pages: pages ?? null });
/** Writes numbered copies of the files (open tokens) into a folder (`pickFolder` token); the originals stay as they are. */
export const marksBatesFiles = (request: { files: string[]; bates: Extract<PageMark, { kind: "bates" }>; date: MarkDate; folder: string }) =>
  invoke<BatesFileResult[]>("marks_bates_files", { request: { ...request, bates: stripKind(request.bates) } });

function stripKind<T extends { kind: string }>(m: T): Omit<T, "kind"> {
  const { kind: _kind, ...rest } = m;
  return rest;
}

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

// ---- print (src-tauri/src/print) ----

export interface PrinterInfo {
  name: string;
  isDefault: boolean;
  /** Prints to a file (Microsoft Print to PDF, the XPS writer): the user is asked where. */
  toFile: boolean;
}

export interface PrinterList {
  printers: PrinterInfo[];
  /** INKWELL_PRINT_DIR is set: print-to-file printers write there without asking (automation). */
  fileDirSet: boolean;
}

export interface PaperInfo {
  /** The driver's DMPAPER number. */
  id: number;
  name: string;
  /** Points, portrait. */
  width: number;
  height: number;
}

export interface PrinterCaps {
  papers: PaperInfo[];
  trays: { id: number; name: string }[];
  duplex: boolean;
  color: boolean;
  collate: boolean;
  maxCopies: number;
}

export type PrintOrientation = "auto" | "portrait" | "landscape";
export type PrintSubset = "all" | "odd" | "even";
export type PrintDuplex = "off" | "longEdge" | "shortEdge";
export type PrintContent = "document" | "documentAndMarkups" | "formFieldsOnly";
export type PrintSizeMode = "fit" | "actual" | "shrink" | "custom";
export type PrintPageOrder = "horizontal" | "horizontalReversed" | "vertical" | "verticalReversed";

export type PrintRange = { kind: "all" } | { kind: "current"; page: number } | { kind: "pages"; text: string };

export type PrintLayout =
  | { kind: "size"; mode: PrintSizeMode; percent: number }
  | { kind: "multiple"; perSheet: number; order: PrintPageOrder; border: boolean; autoRotate: boolean }
  | { kind: "booklet"; sides: "both" | "frontOnly" | "backOnly"; binding: "left" | "right" };

export interface PrintSettings {
  printer: string | null;
  paper: { id: number | null; width: number; height: number };
  tray: number | null;
  orientation: PrintOrientation;
  range: PrintRange;
  subset: PrintSubset;
  reverse: boolean;
  copies: number;
  collate: boolean;
  duplex: PrintDuplex;
  grayscale: boolean;
  layout: PrintLayout;
  content: PrintContent;
}

export interface PrintFace {
  width: number;
  height: number;
  /** Source pages on this side (1-based), in the order drawn. */
  pages: number[];
}

export interface PrintPlan {
  pages: number;
  /** Sides of paper per copy. */
  sheets: number;
  /** Physical sheets for all copies. */
  papers: number;
  /** Sides sent to the printer for all copies, blank backs included: what a job's progress counts to. */
  sides: number;
  faces: PrintFace[];
  editRev: number;
}

export interface PrintProgress {
  done: number;
  total: number;
}

export interface PrintReport {
  printer: string;
  pages: number;
  sides: number;
}

export const printPrinters = () => invoke<PrinterList>("print_printers");
export const printCapabilities = (printer: string) => invoke<PrinterCaps | null>("print_capabilities", { printer });
export const printPlan = (doc: number, settings: PrintSettings) => invoke<PrintPlan>("print_plan", { doc, settings });
/** PNG bytes of side `sheet` (0-based), `maxPx` on its longer edge: the printer path's own drawing. */
export const printPreview = (doc: number, settings: PrintSettings, sheet: number, maxPx: number) =>
  invoke<ArrayBuffer>("print_preview", { doc, settings, sheet, maxPx });

/** `saveTo`: a save token from `pickSave`, for printers that print to a file. `toolCancel(job)` stops it. */
export function printStart(job: number, doc: number, settings: PrintSettings, saveTo: string | null, onProgress: (p: PrintProgress) => void) {
  const ch = new Channel<PrintProgress>();
  ch.onmessage = onProgress;
  return invoke<PrintReport>("print_start", { job, doc, settings, saveTo, onProgress: ch });
}
