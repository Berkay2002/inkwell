import { invoke } from "@tauri-apps/api/core";

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

/** What a command reads PDF bytes from. */
export type Source = { kind: "version"; id: string } | { kind: "current" } | { kind: "picked"; token: string };

/** A file chosen in the native dialog run by Rust; the UI only ever holds its token. */
export interface Picked {
  token: string;
  name: string;
}

export type ChangeKind =
  | "pageAdded"
  | "pageRemoved"
  | "pageReplaced"
  | "pageMoved"
  | "pageRotated"
  | "pageBoxChanged"
  | "textChanged"
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

export interface PageMap {
  aPages: number;
  bPages: number;
  bToA: (number | null)[];
  removedA: number[];
}

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
export const historySaveCopy = (doc: number, version: string, suggested: string) =>
  invoke<string | null>("history_save_copy", { doc, version, suggested });
export const historyName = (doc: number, version: string, name: string | null) =>
  invoke<VersionInfo>("history_name", { doc, version, name });
export const historyConfirmSame = (doc: number, candidate: string | null) =>
  invoke<void>("history_confirm_same", { doc, candidate });
export const historyDelete = (doc: number) => invoke<void>("history_delete", { doc });
export const historyNeverKeep = (doc: number, never: boolean) => invoke<void>("history_never_keep", { doc, never });
export const historyCompare = (doc: number | null, a: Source, b: Source) => invoke<Comparison>("history_compare", { doc, a, b });
/** Runs the file dialog in Rust; null = cancelled. Exactly two files. */
export const pickCompareFiles = () => invoke<Picked[] | null>("history_pick_compare");
export const historyVisualDiff = (doc: number | null, a: Source, aPage: number, b: Source, bPage: number, scale: number, hints: Rect4[] = []) =>
  invoke<VisualDiff>("history_visual_diff", { doc, a, aPage, b, bPage, scale, hints });

/** A rendered page as an object URL (revoke it when done). Pixels per point = `scale`. */
export async function renderPage(doc: number | null, source: Source, page: number, scale: number): Promise<string> {
  const buf = await invoke<ArrayBuffer>("history_render", { doc, source, page, scale });
  return URL.createObjectURL(new Blob([buf], { type: "image/png" }));
}

export const sourceKey = (s: Source) => (s.kind === "version" ? `v:${s.id}` : s.kind === "picked" ? `p:${s.token}` : "current");
