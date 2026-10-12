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
      /** Catalog searches: files whose stored text is of another version than the file on disk (left out). */
      stale: number;
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
export type SaveKind = "pdf" | "text" | "html" | "xhtml" | "word" | "csv" | "comments" | "formData" | "any";
/** Save dialog (run by Rust) in the folder of document `doc` or of open token `near`, with file name `name`. */
export const pickSave = (name: string, at: { doc?: number; near?: string } = {}, kind?: SaveKind) =>
  invoke<FileRef | null>("pick_save", { name, doc: at.doc ?? null, near: at.near ?? null, kind: kind ?? null });
/** Open dialog for what Create builds a PDF from: images (several) or one text file. */
export const pickSource = (kind: "images" | "text" | "json", multiple: boolean) => invoke<FileRef[]>("pick_source", { kind, multiple });
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
  tooLarge: number;
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
  tooLarge: number;
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
  /** Why the index cannot be written, updated or removed (certified, signed document); null when it can. */
  blocked: string | null;
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
  /** A registered print-production view: the tile comes from the `p` route in its plates. */
  view?: number,
): Promise<TileData | null> {
  const route = view ? "p" : "t";
  const url = `${TILE_ORIGIN}/${route}/${doc}/${page}/${rot}/${scale}/${tx}/${ty}?e=${epoch}&r=${rank}&s=${stream}&g=${rev}${view ? `&v=${view}` : ""}`;
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
/** Undo only while the document is still at generation `rev`; rejects with "stale" when a step landed since. */
export const editUndoIf = (doc: number, rev: number) => invoke<EditState>("edit_undo_if", { doc, rev });
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

/** How a link looks while it is pressed (`/H`): None, Invert (the default), Outline, Inset. */
export type LinkHighlight = "none" | "invert" | "outline" | "inset";

export interface PageLink {
  /** The annotation's object number; negative for a link written inline in `/Annots` (minus its position, minus one). */
  id: number;
  /** x0, y0, x1, y1 in page space. */
  rect: [number, number, number, number];
  target: LinkTarget;
  border: LinkBorder;
  highlight: LinkHighlight;
}

export interface PageLinks {
  rev: number;
  links: PageLink[];
  /** Why the links cannot be changed (a certified document); null when they can. */
  locked: string | null;
  /** Signed: a change is saved after the signature. */
  signed: boolean;
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
  | { op: "setHighlight"; page: number; id: number; highlight: LinkHighlight }
  | { op: "delete"; page: number; ids: number[] }
  | { op: "removeAll" }
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
  /** The document is signed: a change to its links is saved after the signature. */
  signed: boolean;
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

/** `auto`: the engine is picked from the languages (Windows reads one, several go to Tesseract). */
export type OcrBackend = "auto" | "windows" | "tesseract";
/** Searchable image (invisible layer), the same cleaned up, or real editable text. */
export type OcrStyle = "searchable" | "cleaned" | "editable";
/** The face editable text is set in: `auto` picks mono for typed pages. */
export type OcrFont = "auto" | "sans" | "mono";

/** One language as the list shows it: which engines have it. */
export interface OcrLang {
  /** Primary tag ("sv"). */
  tag: string;
  /** The installed Windows pack ("sv-SE"). */
  windows: string | null;
  tesseract: boolean;
  rtl: boolean;
  cjk: boolean;
  /** Read, but not as reliably as Latin text. */
  experimental: boolean;
}

export interface OcrCatalog {
  languages: OcrLang[];
  tesseractFound: boolean;
  /** Where Tesseract's `*.traineddata` files go. */
  tessdataDir: string | null;
}

export interface OcrLanguages {
  windows: string[];
  tesseract: { path: string; languages: string[] } | null;
  catalog: OcrCatalog;
  /** The user's Windows languages, display language first. */
  profile: string[];
  /** Display language plus English, as far as installed. */
  defaults: string[];
}

export interface OcrMissing {
  tag: string;
  /** `ms-settings:regionlanguage`; opened with `openLanguageSettings`. */
  windowsSettings: string;
  /** `swe.traineddata` */
  tessdataFile: string;
  tessdataDir: string | null;
}

/** Which engine reads the chosen languages, and what is missing for them. */
export interface OcrPlan {
  engine: "windows" | "tesseract" | null;
  languages: string[];
  dropped: string[];
  missing: OcrMissing[];
  note: string | null;
}

export interface OcrDetection {
  languages: string[];
  source: "text" | "sample" | "profile";
  guess: string | null;
  confidence: number;
}

export interface OcrRequest {
  doc: number;
  /** One-based range like "1-3, 5"; omitted = every page. */
  pages?: string;
  /** Primary tags ("sv"); the engine and its own tags are settled by Rust. */
  languages: string[];
  force: boolean;
  backend: OcrBackend;
  style?: OcrStyle;
  /** Render resolution: 150, 200 (default) or 300. */
  dpi?: number;
  /** Editable text only. */
  font?: OcrFont;
  /** Save token: write a new file instead of changing the document. */
  saveTo?: string;
}

export interface OcrProgress {
  done: number;
  total: number;
  page: number;
  /** After every page was read: the pages are being written into the document (the picture styles). */
  writing?: boolean;
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
  /** What the cleaned-up style did to the pictures; absent for the other styles. */
  clean?: OcrClean | null;
  /** What the editable-text style did; absent for the other styles. */
  editable?: OcrEditable | null;
}

/** A word that stayed in the picture (its text is still searchable). */
export interface OcrUnsure {
  /** 0-based page. */
  page: number;
  text: string;
  /** [x0, y0, x1, y1] on the displayed page, points. */
  bbox: [number, number, number, number];
  why: "low-confidence" | "unusual" | "no-font" | "picture";
}

export interface OcrEditable {
  pages_converted: number;
  /** Pages left as a picture with an invisible layer, with the first few reasons in `notes`. */
  pages_kept: number;
  words_converted: number;
  words_kept: number;
  unsure: OcrUnsure[];
  notes: string[];
}

export interface OcrClean {
  pages_cleaned: number;
  pages_deskewed: number;
  specks_removed: number;
  /** Pages that needed nothing. */
  pages_clean: number;
  /** Pages left as they were, with the first few reasons in `notes`. */
  pages_left: number;
  notes: string[];
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
/** Which engine reads `languages` and what is missing for them. */
export const ocrPlan = (languages: string[], backend: OcrBackend = "auto") => invoke<OcrPlan>("ocr_plan", { languages, backend });
/** The language the document is probably in: its own text, else a quick sample read of its first scan. */
export const ocrDetect = (job: number, doc: number) => invoke<OcrDetection>("ocr_detect", { job, doc });
/** Opens Windows' language settings, where an OCR language pack is added. A fixed address. */
export const openLanguageSettings = () => invoke<void>("open_language_settings");
export const ocrClearTesseract = () => invoke<void>("ocr_clear_tesseract");

/** One page before and after the style, as PNG data URLs. Changes nothing. */
export interface OcrPreview {
  before: string;
  after: string;
  applied: { pages_written: number; words: number; editable?: OcrEditable | null; clean?: OcrClean | null };
}

/** `page` is 0-based. Runs on its own thread; `toolCancel(job)` stops it. */
export const ocrPreview = (job: number, page: number, req: OcrRequest) => invoke<OcrPreview>("ocr_preview", { job, page, req });

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
  /** Pages Save as PDF writes, two-sided padding included: what its progress counts to. */
  pdfSides: number;
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

// ---- article threads ----

export interface ThreadInfo {
  title: string;
  subject: string;
  author: string;
  keywords: string;
}

export interface ThreadBead {
  /** The bead's object number: valid for the generation it was listed at. */
  id: number;
  page: number;
  /** x0, y0, x1, y1 in points on the page as shown (y down). */
  rect: [number, number, number, number];
}

export interface ArticleThread {
  id: number;
  title: string;
  info: ThreadInfo;
  beads: ThreadBead[];
  /** Why the article cannot be edited (only Delete and Properties are offered), or null. */
  damaged: string | null;
}

/** Whether the document is certified (no edit is made) or signed (an edit is saved as an update after the signature). */
export interface DocProtection {
  certified: boolean;
  signed: boolean;
}
export const docProtection = (doc: number) => invoke<DocProtection>("a11y_protection", { doc });
export const threadsList = (doc: number) => invoke<ArticleThread[]>("threads_list", { doc });
export const threadStart = (doc: number, title: string, page: number, rect: ThreadBead["rect"]) =>
  invoke<EditState>("thread_start", { doc, title, page, rect });
/** `after`: the bead to put the new one behind; null puts it at the end. */
export const threadBeadAdd = (doc: number, thread: number, after: number | null, page: number, rect: ThreadBead["rect"]) =>
  invoke<EditState>("thread_bead_add", { doc, thread, after, page, rect });
export const threadBeadMove = (doc: number, thread: number, bead: number, page: number, rect: ThreadBead["rect"]) =>
  invoke<EditState>("thread_bead_move", { doc, thread, bead, page, rect });
export const threadBeadDelete = (doc: number, thread: number, bead: number) =>
  invoke<EditState>("thread_bead_delete", { doc, thread, bead });
export const threadRemove = (doc: number, thread: number) => invoke<EditState>("thread_remove", { doc, thread });
export const threadDescribe = (doc: number, thread: number, info: ThreadInfo) =>
  invoke<EditState>("thread_describe", { doc, thread, info });
export const threadMove = (doc: number, thread: number, to: number) => invoke<EditState>("thread_move", { doc, thread, to });

export const transitionsList = (doc: number) => invoke<PageTrans[]>("transitions_list", { doc });
export const transitionsSet = (doc: number, pages: number[], change: PageTrans) =>
  invoke<EditState>("transitions_set", { doc, pages, change });

// ---- batch actions (Action Wizard; src-tauri/src/batch, docs/batch.md) ----

export type BatchPreset = "smallest" | "balanced" | "highQuality" | "custom";
export interface BatchSanitize {
  metadata: boolean;
  hiddenLayers: boolean;
  hiddenText: boolean;
  scriptsActions: boolean;
  embeddedFiles: boolean;
  annotations: boolean;
  formFieldsUnderMarks: boolean;
  bookmarks: boolean;
}
export type BatchPattern = "personnummer" | "ssn" | "iban" | "card" | "email" | "phone";
export type BatchQuery =
  | { type: "pattern"; pattern: BatchPattern }
  | { type: "text"; text: string; caseSensitive: boolean; wholeWord: boolean }
  | { type: "regex"; regex: string; caseSensitive: boolean };
export interface BatchHeaderFooter {
  text: SixTexts;
  style: MarkStyle;
  margins: [number, number, number, number];
  startNumber: number;
}
export interface BatchWatermark {
  text: string;
  /** Pictures are refused by a batch (their file token does not outlast a session). */
  picture: null;
  style: MarkStyle;
  scale: number;
  opacity: number;
  rotation: number;
  behind: boolean;
  offset: [number, number];
  showOnScreen: boolean;
  showInPrint: boolean;
}
export interface BatchBates {
  prefix: string;
  suffix: string;
  digits: number;
  start: number;
  slot: number;
  style: MarkStyle;
  margins: [number, number, number, number];
}

/** One step. The engine's set is closed; `invalid` is what a file's unknown step reads as (never sent back). */
export type BatchStep =
  | { kind: "compress"; preset: BatchPreset }
  | { kind: "ocr"; languages: string[]; force: boolean; backend: OcrBackend }
  | ({ kind: "headerFooter" } & BatchHeaderFooter)
  | ({ kind: "watermark" } & BatchWatermark)
  | { kind: "bates"; bates: BatchBates; continueAcrossFiles: boolean }
  /** The passwords exist only in a run's request: the engine never stores or returns them. */
  | { kind: "protect"; printing: Printing; changes: Changes; copy: boolean; accessibility: boolean; cipher: Cipher; openPassword?: string; permissionsPassword?: string }
  | ({ kind: "sanitize" } & BatchSanitize)
  | { kind: "redact"; queries: BatchQuery[]; overlayText: string | null; sanitize: BatchSanitize }
  | { kind: "flattenForms" }
  | { kind: "exportImages"; format: ImageFormat; dpi: number; gray: boolean; quality: number }
  | { kind: "exportText"; layout: boolean }
  | { kind: "setProperties"; title: string | null; author: string | null; subject: string | null; keywords: string | null; creator: string | null; producer: string | null }
  | { kind: "invalid"; name: string; why: string };

export type BatchStepKind = Exclude<BatchStep["kind"], "invalid">;

export type BatchOutput = { kind: "nextToSource"; suffix: string } | { kind: "folder"; suffix: string };

export interface BatchAction {
  /** Empty for a new action; the engine gives it one when it is saved. */
  id: string;
  name: string;
  description: string;
  steps: BatchStep[];
  output: BatchOutput;
  onError: "skip" | "stop";
  /** Read-only action that ships with the app: Duplicate it to change it. */
  builtIn: boolean;
}

export interface BatchIssue {
  /** Index of the step; null for the action itself. */
  step: number | null;
  code: "name" | "no-steps" | "output" | "order" | "unknown-step" | "forbidden-step" | "invalid-step" | "unavailable";
  message: string;
}

export type BatchStatus = "ok" | "skipped" | "error";

export interface BatchOutputFile {
  path: string;
  /** Open token of a file the run made. */
  token: string | null;
}

export interface BatchFileReport {
  index: number;
  input: string;
  name: string;
  status: BatchStatus;
  reason: string | null;
  sizeBefore: number;
  sizeAfter: number | null;
  outputs: BatchOutputFile[];
  notes: string[];
  warnings: string[];
  stepsDone: number;
  ms: number;
}

export interface BatchReport {
  action: string;
  files: BatchFileReport[];
  ok: number;
  skipped: number;
  errors: number;
  cancelled: boolean;
  stopped: boolean;
}

export type BatchEvent =
  | { event: "started"; files: number }
  | { event: "fileStarted"; index: number; total: number; name: string }
  | { event: "step"; index: number; step: number; steps: number; label: string }
  | { event: "fileDone"; index: number; report: BatchFileReport }
  | { event: "finished"; ok: number; skipped: number; errors: number; cancelled: boolean };

export interface BatchFilePreview {
  index: number;
  input: string;
  name: string;
  status: BatchStatus;
  reason: string | null;
  steps: string[];
  outputs: string[];
}

export interface BatchPreview {
  action: string;
  files: BatchFilePreview[];
  ok: number;
  skipped: number;
  errors: number;
}

export const batchList = () => invoke<BatchAction[]>("batch_list");
export const batchSave = (action: BatchAction) => invoke<BatchAction>("batch_save", { action });
export const batchDelete = (id: string) => invoke<void>("batch_delete", { id });
/** `file`: an open token of an exported action (`pickSource("json")`). Stored under a new id. */
export const batchImport = (file: string) => invoke<BatchAction>("batch_import", { file });
/** `saveTo`: a save token from `pickSave(name, {}, "any")`. Passwords are never written. */
export const batchExport = (action: BatchAction, saveTo: string) => invoke<void>("batch_export", { action, saveTo });
export const batchValidate = (action: BatchAction) => invoke<BatchIssue[]>("batch_validate", { action });
/** `files`: open tokens. A dry run: per file, would it run or be skipped, and under which names. Writes nothing. */
export const batchPreview = (action: BatchAction, files: string[]) => invoke<BatchPreview>("batch_preview", { action, files });
/** `folder`: an open token of a directory (`pickSearchFolder`). The PDFs in it come back as open tokens (at most 500). */
export const batchFolderFiles = (folder: string, recursive: boolean) =>
  invoke<{ files: FileRef[]; capped: boolean }>("batch_folder_files", { folder, recursive });
/** `folder`: a save token from `pickFolder`, needed when the action's output is a folder. `batchCancel(job)` stops it. */
export function batchRun(
  job: number,
  action: BatchAction,
  files: string[],
  folder: string | null,
  onEvent: (e: BatchEvent) => void,
) {
  const ch = new Channel<BatchEvent>();
  ch.onmessage = onEvent;
  return invoke<BatchReport>("batch_run", { job, action, files, folder, onEvent: ch });
}
export const batchCancel = (job: number) => invoke<void>("batch_cancel", { job });

/** Document JavaScript (src-tauri/src/forms/doc_js.rs). Nothing here runs a script. */
export interface DocJsMeta {
  /** Bytes of source; 0 when it was not read. */
  size: number;
  /** Over 1 MiB: not shown or scanned here. */
  tooLarge: boolean;
  /** Not read: the listing had read its share of script text. */
  unread: boolean;
  /** Keys of the capabilities the source mentions. */
  risks: string[];
}
export interface DocJsNamed extends DocJsMeta {
  /** `n:<name>`. */
  id: string;
  name: string;
}
export interface DocJsStep extends DocJsMeta {
  /** `d:<event>:<i>` or `p:<page>:<event>:<i>`; `d:open:<i>` is the open action. */
  id: string;
  javascript: boolean;
  /** What a step that is not JavaScript does. */
  what: string;
}
export interface DocJsSlot {
  /** `d:<event>` or `p:<page>:<event>`. */
  id: string;
  label: string;
  page: number | null;
  steps: DocJsStep[];
}
export interface DocJsCounts {
  fields: number;
  fieldScripts: number;
  links: number;
  bookmarks: number;
}
export interface DocJsCapability {
  key: string;
  label: string;
  count: number;
  places: string[];
}
export interface DocJsList {
  named: DocJsNamed[];
  docActions: DocJsSlot[];
  pageActions: DocJsSlot[];
  counts: DocJsCounts;
  capabilities: DocJsCapability[];
  incomplete: boolean;
  /** Signed: an edit is saved as an update and the signature will show the change. */
  signed: boolean;
  /** Certified (any /P): script edits are refused. */
  certified: boolean;
}
export interface DocJsSource {
  id: string;
  text: string;
  size: number;
  tooLarge: boolean;
}
export interface DocJsCheck {
  ok: boolean;
  message: string | null;
  /** 1-based. */
  line: number | null;
  column: number | null;
  /** Over what the engine runs (256 KiB): not compiled. */
  tooLarge: boolean;
}
export type DocJsOp =
  | { op: "add"; name: string | null; source: string }
  | { op: "rename"; id: string; name: string }
  | { op: "setSource"; id: string; source: string }
  | { op: "delete"; id: string };
export interface DocJsApplied {
  state: EditState;
  /** The id of the script an `add` made (and of a renamed one). */
  id: string | null;
  /** Set when the file is signed and the edit changed it. */
  warning: string | null;
}

export const docJsList = (doc: number) => invoke<DocJsList>("doc_js_list", { doc });
export const docJsSource = (doc: number, id: string) => invoke<DocJsSource>("doc_js_source", { doc, id });
export const docJsCheck = (source: string) => invoke<DocJsCheck>("doc_js_check", { source });
export const docJsApply = (doc: number, op: DocJsOp) => invoke<DocJsApplied>("doc_js_apply", { doc, op });
