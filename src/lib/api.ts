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
/** An image a page draws: `rect` is what shows of it, in page space (points, origin top left). */
export interface PageImageBlock {
  rect: [number, number, number, number];
  colorSpace: string;
  /** Has a soft mask or a stencil mask: its shape is not the box. */
  masked: boolean;
}
/** The images page `page` draws (dark pages keep the photographs in their own colours). */
export const pageImages = (doc: number, page: number) =>
  invoke<{ rev: number; images: PageImageBlock[] }>("image_blocks", { doc, page }).then((b) => b.images);

// ---- editing images in place (src-tauri/src/imageedit, docs/image-edit.md) ----

/** One image a page draws, with what the editor needs: all rectangles are page space (points, y down). */
export interface ImageBlock extends PageImageBlock {
  /** Pass to `imageEdit` with the `rev` of the list it came from. */
  index: number;
  kind: "xObject" | "inline";
  /** The whole image's box: larger than `rect` when a clip crops it. */
  fullRect: [number, number, number, number];
  /** The placement: unit square to user space. */
  matrix: [number, number, number, number, number, number];
  /** Pixel size. */
  width: number;
  height: number;
  bitsPerComponent: number;
  /** The stream's filters, outermost first. */
  filter: string[];
  name?: string | null;
  xref?: number | null;
  editable: boolean;
  reason?: string | null;
}

export interface ImageBlocks {
  rev: number;
  images: ImageBlock[];
}

/** `file`: an open token from `pickSource("images", …)`. `rect`s are the box to fit, in page space. */
export type ImageChange =
  | { op: "move"; dx: number; dy: number }
  | { op: "resize"; rect: [number, number, number, number]; keepAspect: boolean }
  | { op: "rotate"; quarterTurns: number }
  | { op: "flip"; horizontal: boolean }
  | { op: "delete" }
  | { op: "replace"; file: string }
  | { op: "crop"; rect: [number, number, number, number]; pixels: boolean }
  | { op: "add"; file: string; rect: [number, number, number, number]; keepAspect?: boolean };

export const imageBlocks = (doc: number, page: number) => invoke<ImageBlocks>("image_blocks", { doc, page });
/** `index` is null for `add`. A `rev` older than the document's is refused as "stale". */
export const imageEdit = (doc: number, rev: number, page: number, index: number | null, change: ImageChange) =>
  invoke<{ state: EditState }>("image_edit", { doc, rev, page, index, change });
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

// ---- catalogs: a saved full-text index over a folder (src-tauri/src/search/catalog) ----------------

export interface CatalogMeta {
  id: string;
  name: string;
  folder: string;
  recursive: boolean;
  created: number;
  /** Unix milliseconds the live index was built; 0 when it never was. */
  built: number;
  generation: number;
  files: number;
  indexed: number;
  pages: number;
  damaged: number;
  password: number;
  cloudOnly: number;
  cut: number;
  capped: boolean;
  indexLen: number;
}

export interface CatalogSummary {
  files: number;
  kept: number;
  extracted: number;
  fromEmbedded: number;
  added: number;
  changed: number;
  removed: number;
  damaged: number;
  password: number;
  cloudOnly: number;
  pages: number;
  cut: number;
  capped: boolean;
  cancelled: boolean;
}

/** What a refresh would change; `unusable`: never built, or the index must be built again. */
export interface CatalogStale {
  added: number;
  changed: number;
  removed: number;
  unusable: boolean;
}

export interface CatalogEstimate {
  files: number;
  bytes: number;
  secs: number;
  indexBytes: number;
  capped: boolean;
}

export type CatalogBuildEvent =
  | { kind: "started"; job: number }
  | { kind: "progress"; job: number; done: number; total: number }
  | { kind: "done"; job: number; summary: CatalogSummary; catalog: CatalogMeta };

/** What a document carries of an embedded search index. */
export interface Embedded {
  present: boolean;
  /** Same /ID, page count and page content as the file now: a build reads it. */
  current: boolean;
  pages: number;
  bytes: number;
}

export const catalogEstimate = (folder: string, recursive: boolean) => invoke<CatalogEstimate>("catalog_estimate", { folder, recursive });
export const catalogCreate = (folder: string, name: string, recursive: boolean) => invoke<CatalogMeta>("catalog_create", { folder, name, recursive });
export const catalogList = () => invoke<CatalogMeta[]>("catalog_list");
export const catalogDelete = (id: string) => invoke<void>("catalog_delete", { id });
export const catalogStale = (id: string) => invoke<CatalogStale>("catalog_stale", { id });
/** Builds (`full`) or refreshes a catalog; resolves with what it did. Cancel with `catalogCancel(job)` from the `started` event. */
export function catalogBuild(id: string, full: boolean, onEvent: (e: CatalogBuildEvent) => void) {
  const ch = new Channel<CatalogBuildEvent>();
  ch.onmessage = onEvent;
  return invoke<CatalogSummary>("catalog_build", { id, full, onEvent: ch });
}
export const catalogCancel = (job: number) => invoke<void>("catalog_cancel", { job });
/** Searches a catalog; same events and ids as `searchFiles`, so `searchFileToken` and `cancelSearchFiles` work on it. */
export function catalogSearch(id: string, query: string, opts: FindOpts, onEvent: (e: FilesEvent) => void) {
  const ch = new Channel<FilesEvent>();
  ch.onmessage = onEvent;
  return invoke<number>("catalog_search", { id, query, opts, onEvent: ch });
}
export const indexEmbedded = (doc: number) => invoke<Embedded>("index_embedded", { doc });
export const indexEmbed = (doc: number) => invoke<EditState>("index_embed", { doc });
export const indexUnembed = (doc: number) => invoke<EditState>("index_unembed", { doc });

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

// ---- bookmarks and links (src-tauri/src/outline, src-tauri/src/links) ----

export type BookmarkDest =
  | { kind: "none" }
  /** `at`: the point shown at the top left of the view, page space. */
  | { kind: "page"; page: number; at: [number, number] | null }
  | { kind: "uri"; uri: string }
  | { kind: "other"; what: string }
  /** A destination that can't be followed (a missing named destination or page); `what` says which. */
  | { kind: "broken"; what: string };

export interface Bookmark {
  /** The item's object number: stable across undo. */
  id: number;
  title: string;
  dest: BookmarkDest;
  open: boolean;
  bold: boolean;
  italic: boolean;
  color: [number, number, number] | null;
  children: Bookmark[];
}

export interface Bookmarks {
  rev: number;
  items: Bookmark[];
  count: number;
  /** More items than are read: the rest is left as it is and can't be edited. */
  truncated: boolean;
}

export type BookmarkOp =
  | { op: "add"; parent: number | null; index: number | null; title: string; page: number; at: [number, number] | null }
  | { op: "rename"; id: number; title: string }
  | { op: "delete"; ids: number[] }
  | { op: "move"; id: number; parent: number | null; index: number }
  | { op: "setDest"; id: number; page: number; at: [number, number] | null }
  | { op: "setOpen"; id: number; open: boolean }
  | { op: "setStyle"; id: number; bold?: boolean; italic?: boolean; color?: [number, number, number]; resetColor?: boolean }
  | { op: "fromHeadings"; replace: boolean };

export interface BookmarkEdited {
  state: EditState;
  id: number | null;
  count: number;
}

export interface HeadingPreview {
  count: number;
  levels: number;
  /** `tags`: the file's own heading tags; `size`: guessed from the size of the text. */
  source: "tags" | "size";
  rows: { title: string; page: number; level: number }[];
}

export type LinkTarget =
  | { kind: "page"; page: number; at: [number, number] | null }
  | { kind: "uri"; uri: string }
  | { kind: "file"; name: string; page: number | null }
  | { kind: "other"; what: string }
  | { kind: "broken"; what: string };

export interface LinkBorder {
  visible: boolean;
  width: number;
  color: [number, number, number];
}

export interface PageLink {
  /** The annotation's object number. */
  id: number;
  /** x0, y0, x1, y1 in page space. */
  rect: [number, number, number, number];
  target: LinkTarget;
  border: LinkBorder;
}

export interface PageLinks {
  rev: number;
  links: PageLink[];
}

export type NewLinkTarget =
  | { kind: "page"; page: number; at: [number, number] | null }
  | { kind: "uri"; uri: string }
  /** `file`: an open token from `linkPickFile`. */
  | { kind: "file"; file: string; page: number | null };

export interface BorderSpec {
  visible: boolean;
  width?: number;
  color?: [number, number, number];
}

export type LinkOp =
  | { op: "add"; page: number; rect: [number, number, number, number]; target: NewLinkTarget; border?: BorderSpec }
  | { op: "setTarget"; page: number; id: number; target: NewLinkTarget }
  | { op: "setRect"; page: number; id: number; rect: [number, number, number, number] }
  | { op: "setBorder"; page: number; id: number; border: BorderSpec }
  | { op: "delete"; page: number; ids: number[] }
  | { op: "fromUrls" };

export interface LinkEdited {
  state: EditState;
  id: number | null;
  count: number;
  /** A link to a file: the name the link now has for it, found from the folder of the document. */
  name: string | null;
}

export interface LinkAudit {
  total: number;
  external: number;
  broken: number;
  /** The first broken ones. */
  rows: { page: number; id: number; what: string }[];
}

export interface UrlPreview {
  count: number;
  pages: number;
  rows: { page: number; url: string }[];
}

export const bookmarksList = (doc: number) => invoke<Bookmarks>("bookmarks_list", { doc });
export const bookmarksApply = (doc: number, rev: number, op: BookmarkOp) => invoke<BookmarkEdited>("bookmarks_apply", { doc, rev, op });
export const bookmarksHeadings = (doc: number) => invoke<HeadingPreview>("bookmarks_headings", { doc });
export const linksList = (doc: number, page: number) => invoke<PageLinks>("links_list", { doc, page });
export const linksApply = (doc: number, rev: number, op: LinkOp) => invoke<LinkEdited>("links_apply", { doc, rev, op });
export const linksUrls = (doc: number) => invoke<UrlPreview>("links_urls", { doc });
export const linksAudit = (doc: number) => invoke<LinkAudit>("links_audit", { doc });
/** An address as it would be linked (gate: https, http, mailto), or an error saying why not. */
export const linkCheckUrl = (url: string) => invoke<string>("link_check_url", { url });
/** Any file, as an open token. */
export const linkPickFile = () => invoke<FileRef | null>("link_pick_file");
/** The PDF a file link names (next to the document), as an open token; refused with the reason otherwise. */
/** Where a file link looks, in full, for the confirmation: no token is issued. */
export const linkFilePath = (doc: number, name: string) => invoke<string>("link_file_path", { doc, name });
/** The open token for a file link's PDF: ask only after the person confirmed `linkFilePath`. */
export const linkFileRef = (doc: number, name: string) => invoke<FileRef>("link_file_ref", { doc, name });
/** Only after the person confirmed. */
export const linkOpenUrl = (url: string) => invoke<void>("link_open_url", { url });

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

export type AuditCategory =
  | "images"
  | "fonts"
  | "contentStreams"
  | "forms"
  | "comments"
  | "links"
  | "bookmarks"
  | "thumbnails"
  | "embeddedFiles"
  | "metadata"
  | "structure"
  | "other";

/** One font by name, as the audit lists it (what "Unembed" works on). */
export interface FontUsage {
  /** `/BaseFont` as written, subset tag included. */
  name: string;
  kind: string;
  embedded: boolean;
  subset: boolean;
  /** Stored size of the font file(s). */
  bytes: number;
  canUnembed: boolean;
  whyNot: string | null;
}

export interface CompressAudit {
  total: number;
  /** The categories add up to `total`; `percent` is the share of the file, 0..100. */
  categories: { category: AuditCategory; bytes: number; objects: number; percent: number }[];
  images: number;
  fonts: number;
  fontList: FontUsage[];
}

export type ImageCodec = "retain" | "jpeg" | "jpx" | "flate" | "fax";

export interface ImageClassSettings {
  /** Downsample images shown above this many ppi (0 = never). */
  abovePpi: number;
  toPpi: number;
  codec: ImageCodec;
  quality: number;
}

export interface DiscardSettings {
  metadata: boolean;
  thumbnails: boolean;
  embeddedFiles: boolean;
  javascript: boolean;
  bookmarks: boolean;
  formActions: boolean;
  alternateImages: boolean;
  privateData: boolean;
  links: boolean;
  invalidLinks: boolean;
}

/** The engine's `Options` as it serializes: the three presets and the Advanced settings are the same thing. */
export interface CompressOptions {
  preset: CompressPreset | "custom";
  images: { color: ImageClassSettings; gray: ImageClassSettings; mono: ImageClassSettings; onlyIfSmaller: boolean };
  subsetFonts: boolean;
  unembedFonts: string[];
  unembedAll: boolean;
  transparency: { flatten: boolean; ppi: number };
  compressStreams: boolean;
  objectStreams: boolean;
  /** 0 none, 1 unused, 2 + renumber, 3 + identical objects, 4 + identical streams. */
  garbage: number;
  strip: DiscardSettings;
  visualPages: number;
}

export interface NamedCompressPreset {
  name: string;
  options: CompressOptions;
}

export interface CompressAuditReply {
  audit: CompressAudit;
  size: number;
  defaultOutput: string;
  outputExists: boolean;
  /** The document carries signatures; the compressed copy invalidates them. */
  signed: boolean;
}

/** Predicted size per preset, from recompressing a sample of the file's images. */
export interface CompressEstimate {
  preset: CompressPreset | "custom";
  bytes: number;
  /** The range it should fall in; equal when the sample covered every image. */
  low: number;
  high: number;
}

export interface CompressProgress {
  stage: "open" | "scan" | "flatten" | "images" | "fonts" | "write" | "verify";
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
  /** Render comparison of before and after (the sampled pages, plus every page the run flattened or gave
   *  another font); null when skipped. `worstPage` is zero-based. */
  visual: {
    maxDiff: number;
    meanDiff: number;
    psnr: number;
    /** The difference 99.9 % of the compared samples stay under. */
    p999Diff: number;
    worstPage: number;
    pagesChecked: number;
    pagesTotal: number;
  } | null;
  warnings: string[];
  before: CompressAudit;
  after: CompressAudit;
  /** What the discard options took out, by option. */
  stripped: {
    metadata: boolean;
    bookmarks: boolean;
    thumbnails: number;
    embeddedFiles: number;
    javascript: number;
    formActions: number;
    alternateImages: number;
    privateData: number;
    links: number;
    invalidLinks: number;
  };
  unembedded: { files: number; bytes: number; skipped: [string, string][] };
  /** 1-based pages drawn as a picture, and the ones that use transparency and could not be. */
  flattened: { pages: number[]; skipped: [number, string][] };
}

/** Documents are named by id; an output file by a save token from `pickSave`. */
export interface CompressRequest {
  doc: number;
  preset: CompressPreset;
  /** The Advanced settings; wins over `preset`. */
  custom?: CompressOptions;
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

/** The three presets as the engine defines them: the starting point of the Advanced settings. */
export const compressPresetOptions = () => invoke<Record<CompressPreset, CompressOptions>>("compress_preset_options");
/** One estimate for a set of Advanced settings (on the same sample). `toolCancel(job)` stops it. */
export const compressEstimateCustom = (job: number, doc: number, options: CompressOptions) =>
  invoke<CompressEstimate>("compress_estimate_custom", { job, doc, options });
export const compressPresets = () => invoke<NamedCompressPreset[]>("compress_presets");
export const compressPresetSave = (name: string, options: CompressOptions) =>
  invoke<NamedCompressPreset[]>("compress_preset_save", { name, options });
export const compressPresetDelete = (name: string) => invoke<NamedCompressPreset[]>("compress_preset_delete", { name });

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
  /** `creep` in points: how much closer to the spine the innermost sheet's pages sit than the outermost's. */
  | { kind: "booklet"; sides: "both" | "frontOnly" | "backOnly"; binding: "left" | "right"; creep: number }
  /** `scale` in percent, `overlap` in points. */
  | { kind: "poster"; scale: number; overlap: number; cutMarks: boolean; labels: boolean };

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
  /** Poster: which tile of its page this side is (0-based) in a `cols` x `rows` grid. */
  tile: { col: number; row: number; cols: number; rows: number } | null;
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

export interface PrintSavedPdf {
  /** The new file, as an open token for a new tab. */
  file: FileRef;
  /** Source pages chosen. */
  pages: number;
  /** Pages of the new file. */
  sheets: number;
}

/** Writes the plan as a new vector PDF at `saveTo` (a save token from `pickSave`). Needs no printer. */
export function printSavePdf(job: number, doc: number, settings: PrintSettings, saveTo: string, onProgress: (p: PrintProgress) => void) {
  const ch = new Channel<PrintProgress>();
  ch.onmessage = onProgress;
  return invoke<PrintSavedPdf>("print_save_pdf", { job, doc, settings, saveTo, onProgress: ch });
}

// ---- combine, attachments, page labels, transitions (src-tauri/src/organize) ----

export interface CombineFile {
  /** Open token of the PDF. */
  file: string;
  /** "1-3, 6, 9-7": in the order written; empty = every page. */
  range: string;
  /** Bookmark title; null = the file's name. */
  title: string | null;
}

export interface CombineOptions {
  /** One bookmark per file. */
  bookmarks: boolean;
  /** Each file's own bookmarks, kept under its bookmark. */
  keepOutlines: boolean;
  /** A contents page in front. */
  contentsPage: boolean;
}

export type CombineFileState = "ready" | "encrypted" | "noCopy" | "unreadable" | "badRange";

export interface CombineFilePlan {
  name: string;
  title: string;
  state: { state: CombineFileState };
  message: string | null;
  pages: number;
  used: number;
  hasOutline: boolean;
  hasFields: boolean;
}

export interface CombinePlan {
  files: CombineFilePlan[];
  pages: number;
  contentsPages: number;
  bookmarks: number;
  leftOut: number;
  canRun: boolean;
  /** "Combines 4 files, 37 pages, 4 bookmarks". */
  summary: string;
}

export interface Combined {
  file: FileRef;
  plan: CombinePlan;
}

export const combineInspect = (files: CombineFile[], options: CombineOptions) =>
  invoke<CombinePlan>("combine_inspect", { files, options });
/** `out`: a save token from `pickSave`. */
export const combineRun = (files: CombineFile[], options: CombineOptions, out: string) =>
  invoke<Combined>("combine_run", { files, options, out });

export interface Attachment {
  /** `n:<name>` for a document-level file, `a:<object>` for a page annotation. */
  id: string;
  name: string;
  description: string;
  size: number;
  mime: string | null;
  kind: "file" | "annotation";
  /** 0-based page of an annotation. */
  page: number | null;
  hasData: boolean;
  isPdf: boolean;
}

/** Open dialog for any files to attach. */
export const pickAttachments = () => invoke<FileRef[]>("pick_attachments");
export const attachmentsList = (doc: number) => invoke<Attachment[]>("attachments_list", { doc });
export const attachmentsAdd = (doc: number, files: string[], description?: string) =>
  invoke<EditState>("attachments_add", { doc, files, description: description ?? null });
/** `to`: a save token (`pickSave` with kind "any"). Writes the bytes, never runs the file. Resolves to the path. */
export const attachmentSave = (doc: number, id: string, to: string) => invoke<string>("attachment_save", { doc, id, to });
/** A PDF attachment as an open token (scratch copy); open it with `store.openPath(path, token, true)`. */
export const attachmentOpen = (doc: number, id: string) => invoke<FileRef>("attachment_open", { doc, id });
export const attachmentsRemove = (doc: number, ids: string[]) => invoke<EditState>("attachments_remove", { doc, ids });
export const attachmentDescribe = (doc: number, id: string, text: string) =>
  invoke<EditState>("attachment_describe", { doc, id, text });

// ---- PDF Portfolios (src-tauri/src/organize/portfolio) ----

export interface PortfolioField {
  key: string;
  name: string;
  /** The schema's subtype: S, D, N, F, Desc, Size, ModDate, CreationDate, CompressedSize. */
  subtype: string;
  kind: "text" | "number" | "date";
  visible: boolean;
}

export interface PortfolioSortKey {
  /** Index into `fields`. */
  field: number;
  ascending: boolean;
}

export interface PortfolioFolder {
  id: number;
  name: string;
  parent: number | null;
}

export interface PortfolioFile {
  /** The attachment id: what `attachmentOpen`, `attachmentSave` and the other attachment calls take. */
  id: string;
  name: string;
  description: string;
  size: number;
  mime: string | null;
  isPdf: boolean;
  hasData: boolean;
  folder: number | null;
  /** What each field shows, in `fields` order. */
  cells: string[];
  /** The value's place among all files per field (equal values share one; null: none). Sorting is a sort of these. */
  ranks: (number | null)[];
}

export interface PortfolioInfo {
  /** The schema's fields in the author's column order. */
  fields: PortfolioField[];
  /** The collection's own sort: `files` come in this order. */
  sort: PortfolioSortKey[];
  view: "details" | "tile" | "hidden" | "custom";
  /** The file that opens first (an attachment id). */
  initial: string | null;
  /** Parents before children, siblings by name. */
  folders: PortfolioFolder[];
  files: PortfolioFile[];
}

export interface PortfolioExtracted {
  folder: string;
  written: string[];
  skipped: string[];
}

/** The portfolio view of a document; null for a plain PDF. Rejects when the collection is unusable (folders that loop). */
export const portfolioInfo = (doc: number) => invoke<PortfolioInfo | null>("portfolio_info", { doc });
/** Open dialog for the files of a new portfolio (any type). */
export const pickPortfolioFiles = () => invoke<FileRef[]>("pick_portfolio_files");
/** `out`: a save token from `pickSave`. Resolves to an open token for the new file. The sources are only read. */
export const portfolioCreate = (files: string[], out: string) => invoke<FileRef>("portfolio_create", { files, out });
/** `to`: a folder token from `pickFolder`. Never replaces a file there. */
export const portfolioExtractAll = (doc: number, to: string) => invoke<PortfolioExtracted>("portfolio_extract_all", { doc, to });

export type LabelStyle = "none" | "decimal" | "upperRoman" | "lowerRoman" | "upperAlpha" | "lowerAlpha";

export interface LabelRange {
  /** 0-based first page of the section. */
  page: number;
  style: LabelStyle;
  prefix: string;
  start: number;
}

export interface LabelsDto {
  ranges: LabelRange[];
  /** One label per page; empty when the document has no page labels. */
  labels: string[];
}

export const labelsList = (doc: number) => invoke<LabelsDto>("labels_list", { doc });
/** Labels pages `from..=to` (0-based). `extend`: carry on the numbering of the section before. */
export const labelsSet = (doc: number, from: number, to: number, style: LabelStyle, prefix: string, start: number, extend: boolean) =>
  invoke<EditState>("labels_set", { doc, from, to, style, prefix, start, extend });
export const labelsRemove = (doc: number) => invoke<EditState>("labels_remove", { doc });

export type TransStyle = "split" | "blinds" | "box" | "wipe" | "dissolve" | "glitter" | "r" | "fly" | "push" | "cover" | "uncover" | "fade";

export interface Transition {
  style: TransStyle;
  /** Seconds the effect takes. */
  duration: number;
  dimension: "H" | "V" | null;
  motion: "I" | "O" | null;
  /** 0, 90, 180, 270 or 315. */
  direction: number | null;
}

export interface PageTrans {
  transition: Transition | null;
  /** Seconds before full screen moves on by itself. */
  autoAdvance: number | null;
}

export const transitionsList = (doc: number) => invoke<PageTrans[]>("transitions_list", { doc });
export const transitionsSet = (doc: number, pages: number[], change: PageTrans) =>
  invoke<EditState>("transitions_set", { doc, pages, change });
