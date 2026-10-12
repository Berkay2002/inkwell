import { Channel, invoke } from "@tauri-apps/api/core";
import { type FileRef, pickOpen, pickSave, toolCancel } from "./api";

export type Rect4 = [number, number, number, number];

export type Author = { type: "user" } | { type: "assistant"; turn_id: string } | { type: "external" } | { type: "autosave" };
export type Kind =
  | { type: "save" }
  | { type: "checkpoint" }
  | { type: "autosave" }
  | { type: "external" }
  | { type: "restore"; from: string }
  | { type: "import" };

export interface VersionInfo {
  id: string;
  parents: string[];
  /** Milliseconds since the epoch. */
  created: number;
  author: Author;
  kind: Kind;
  label: string;
  summary: string[];
  name: string | null;
  /** 0-based */
  pagesTouched: number[];
  size: number;
  broken: boolean;
}

export type HistoryStatus = "loading" | "ready" | "unavailable" | "off" | "ambiguous" | "none" | "error";

export interface Candidate {
  id: string;
  last: VersionInfo | null;
}

export interface HistoryState {
  status: HistoryStatus;
  message: string | null;
  recovery: VersionInfo | null;
  candidates: Candidate[];
}

/** What a command reads PDF bytes from. A picked file's `path` is for the window only (the engine reads the token). */
export type Source = { kind: "version"; id: string } | { kind: "current" } | { kind: "picked"; token: string; path?: string };

export type ChangeKind =
  | "pageAdded"
  | "pageRemoved"
  | "pageReplaced"
  | "pageMoved"
  | "pageRotated"
  | "pageBoxChanged"
  | "textChanged"
  | "formatChanged"
  | "imageReplaced"
  | "ocrLayerAdded"
  | "redactionApplied"
  | "pageContentChanged"
  | "annotationAdded"
  | "annotationRemoved"
  | "annotationChanged"
  | "formFieldChanged"
  | "metadataChanged"
  | "bookmarksChanged"
  | "attachmentsChanged";

export interface TextRun {
  op: "equal" | "insert" | "delete";
  text: string;
  boxesA: Rect4[];
  boxesB: Rect4[];
}

export interface FormatProp {
  name: "font" | "size" | "colour";
  old: string;
  new: string;
}

export interface MetadataField {
  source: "info" | "xmp";
  field: string;
  old: string | null;
  new: string | null;
}

export type Detail =
  | { type: "pageMoved"; from: number; to: number }
  | { type: "text"; runs: TextRun[] }
  | { type: "metadata"; fields: MetadataField[] }
  | { type: "format"; text: string; props: FormatProp[] }
  | { type: string };

export interface Change {
  kind: ChangeKind;
  /** 0-based */
  aPage: number | null;
  bPage: number | null;
  rectA: Rect4 | null;
  rectB: Rect4 | null;
  label: string;
  detail: Detail;
}

/** Every page number is the file's own (0-based): with a page range on a side they are the picked pages' real ones. */
export interface PageMap {
  /** How many pages of each file were compared. */
  aPages: number;
  bPages: number;
  /** The pages compared on each side, ascending. */
  aIds: number[];
  bIds: number[];
  /** For each of `bIds`, the matched page of A. */
  bToA: (number | null)[];
  removedA: number[];
}

/** The pages of each side a comparison covers (0-based; null = every page). */
export type ComparePages = [number[] | null, number[] | null];

export interface Comparison {
  pages: PageMap;
  changes: Change[];
  identical: boolean;
}

export interface VisualDiff {
  width: number;
  height: number;
  scale: number;
  boxes: Rect4[];
  boxesPx: [number, number, number, number][];
  changedPixels: number;
}

export interface RestoreOut {
  path: string;
  version: VersionInfo;
}

export const historyState = (doc: number) => invoke<HistoryState>("history_state", { doc });
export const historyVersions = (doc: number) => invoke<VersionInfo[]>("history_versions", { doc });
export const historyCheck = (doc: number) => invoke<VersionInfo | null>("history_check", { doc });
export const historyRestore = (doc: number, version: string) => invoke<RestoreOut>("history_restore", { doc, version });
export const historyRecover = (doc: number) => invoke<RestoreOut>("history_recover", { doc });
/** Save dialog (in Rust, next to the document), then writes the version there; null = cancelled. */
export async function historySaveCopy(doc: number, version: string, suggested: string): Promise<string | null> {
  const to = await pickSave(suggested, { doc });
  return to ? invoke<string>("history_save_copy", { doc, version, to: to.token }) : null;
}
/** After an assistant turn: a checkpoint version if the document ends with the turn's changes. */
export const historyCheckpoint = (doc: number) => invoke<VersionInfo | null>("history_checkpoint", { doc });
export const historyName = (doc: number, version: string, name: string | null) =>
  invoke<VersionInfo>("history_name", { doc, version, name });
export const historyConfirmSame = (doc: number, candidate: string | null) =>
  invoke<void>("history_confirm_same", { doc, candidate });
export const historyDelete = (doc: number) => invoke<void>("history_delete", { doc });
export const historyNeverKeep = (doc: number, never: boolean) => invoke<void>("history_never_keep", { doc, never });
export const historyCompare = (doc: number | null, a: Source, b: Source) => invoke<Comparison>("history_compare", { doc, a, b });
/** Runs the file dialog in Rust (`pick_open`); null = cancelled. Exactly two files. */
export async function pickCompareFiles(): Promise<FileRef[] | null> {
  const files = await pickOpen(true, "compare");
  if (!files.length) return null;
  if (files.length !== 2) throw new Error("Pick exactly 2 PDF files to compare.");
  return files;
}
export type CompareStage = "first" | "second" | "pages" | "document";
export interface CompareProgress {
  stage: CompareStage;
  done: number;
  total: number;
}

/** As `historyCompare`, on its own thread: progress through `onProgress`, `toolCancel(job)` stops it. A page past the
 *  end of its file is refused before any page is read. */
export function historyCompareRun(job: number, doc: number | null, a: Source, b: Source, pages: ComparePages, onProgress: (p: CompareProgress) => void) {
  const ch = new Channel<CompareProgress>();
  ch.onmessage = onProgress;
  return invoke<Comparison>("history_compare_run", { job, doc, a, b, pagesA: pages[0], pagesB: pages[1], onProgress: ch });
}

/** How many pages a source has; rejects for a file that cannot be read (a password, damage). */
export const historyPageCount = (doc: number | null, source: Source) => invoke<number>("history_page_count", { doc, source });

/** Save dialog (in Rust), then writes the report PDF there; null = cancelled. Returns the path. */
export async function historyCompareReport(
  job: number,
  doc: number | null,
  a: Source,
  b: Source,
  pages: ComparePages,
  names: [string, string],
  suggested: string,
  onProgress: (p: CompareProgress) => void,
): Promise<string | null> {
  const to = await pickSave(suggested, doc == null ? undefined : { doc });
  if (!to) return null;
  const ch = new Channel<CompareProgress>();
  ch.onmessage = onProgress;
  const date = new Date().toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
  return invoke<string>("history_compare_report", { job, doc, a, b, pagesA: pages[0], pagesB: pages[1], names, date, to: to.token, onProgress: ch });
}

export const compareCancel = (job: number) => toolCancel(job);

export const historyVisualDiff = (doc: number | null, a: Source, aPage: number, b: Source, bPage: number, scale: number, hints: Rect4[] = []) =>
  invoke<VisualDiff>("history_visual_diff", { doc, a, aPage, b, bPage, scale, hints });

/** A rendered page as an object URL (revoke it when done). Pixels per point = `scale`. */
export async function renderPage(doc: number | null, source: Source, page: number, scale: number): Promise<string> {
  const buf = await invoke<ArrayBuffer>("history_render", { doc, source, page, scale });
  return URL.createObjectURL(new Blob([buf], { type: "image/png" }));
}

export const sourceKey = (s: Source) => (s.kind === "version" ? `v:${s.id}` : s.kind === "picked" ? `p:${s.token}` : "current");
