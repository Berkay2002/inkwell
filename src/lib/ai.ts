import { Channel, invoke } from "@tauri-apps/api/core";
import type { EditState } from "./api";

export type Rect4 = [number, number, number, number];

export interface ModelInfo {
  id: string;
  name: string;
  efforts: string[];
  defaultEffort: string | null;
  isDefault: boolean;
}

export interface AiStatus {
  installed: boolean;
  loggedIn: boolean;
  account: string | null;
  plan: string | null;
  models: ModelInfo[];
  error: string | null;
}

export interface Prepared {
  pages: number;
  chars: number;
  enough: boolean;
}

export interface Citation {
  /** The number in the answer's [n] marker. */
  n: number;
  /** 0-based; null when the model gave nothing usable. */
  page: number | null;
  /** Line boxes in unrotated page points; empty = jump to the page only. */
  rects: Rect4[];
  quote: string;
  chunk: string | null;
  exact: boolean;
}

export type Task =
  | { kind: "ask"; question: string }
  | { kind: "summarizeDocument" }
  | { kind: "summarizePage"; page: number }
  | { kind: "selection"; action: "explain" | "rewrite" | "summarize"; text: string; page: number; style?: string };

/** What the assistant may change without asking (see `src-tauri/src/ai/tools.rs`). */
export type Permissions = "ask" | "auto" | "full";
export const PERMISSIONS: Permissions[] = ["ask", "auto", "full"];

export interface AskRequest {
  task: Task;
  name: string;
  model?: string | null;
  effort?: string | null;
  currentPage?: number | null;
  /** The text selected in the viewer, 0-based page. */
  selection?: { text: string; page: number } | null;
  permissions?: Permissions;
}

/** One tool call of a turn, at its latest step. `waiting` is a card needing Apply / Reject. */
export interface Activity {
  id: string;
  tool: string;
  /** `permanent` cannot be taken back from the saved file (a redaction) and asks in every mode. */
  class: "read" | "edit" | "destructive" | "permanent";
  /** What and where, ready to show ("Searched “Ciri”", "Deleted page 4"). */
  label: string;
  state: "waiting" | "running" | "done" | "failed" | "rejected";
  detail: string | null;
}

/** The report of a redaction the assistant applied (the Redact rail opens on it). */
export interface RedactReport {
  marks: number;
  sanitize: Record<string, unknown>;
  verify: { leaks: { needle: string; place: string; critical: boolean }[] };
}

/** The model moved the user's view, or changed the document. */
export type UiAction =
  | { type: "goTo"; page: number }
  | { type: "show"; page: number; rects: Rect4[]; quote: string }
  /** The assistant changed the document: this is its new generation (pages, undo label, marks). */
  | { type: "edited"; state: EditState; redacted: RedactReport | null };

export type ChatEvent =
  | { kind: "status"; phase: "reading" | "connecting" | "thinking" }
  | { kind: "delta"; text: string }
  | ({ kind: "activity" } & Activity)
  | { kind: "ui"; action: UiAction }
  | {
      kind: "done";
      answer: string;
      citations: Citation[];
      followups: string[];
      firstTokenMs: number | null;
      totalMs: number;
      prepMs: number;
      chunksSent: number;
      chunksTotal: number;
    }
  | { kind: "error"; message: string; hint: string | null }
  | { kind: "cancelled" };

export const aiStatus = () => invoke<AiStatus>("ai_status");
export const aiPrepare = (doc: number) => invoke<Prepared>("ai_prepare", { doc });
export const aiCancel = (doc: number) => invoke<void>("ai_cancel", { doc });
export const aiClose = (doc: number) => invoke<void>("ai_close", { doc });
/** The mode changed while a turn runs: its very next tool call is checked against it. */
export const aiSetPermissions = (doc: number, mode: Permissions) => invoke<void>("ai_set_permissions", { doc, mode });
/** The user's Apply (true) or Reject (false) on a waiting tool call. */
export const aiToolDecision = (doc: number, callId: string, allow: boolean) =>
  invoke<void>("ai_tool_decision", { doc, callId, allow });

/** Starts a question; every event of the answer arrives through `onEvent`. */
export function aiAsk(doc: number, req: AskRequest, onEvent: (e: ChatEvent) => void) {
  const ch = new Channel<ChatEvent>();
  ch.onmessage = onEvent;
  return invoke<void>("ai_ask", { doc, req, onEvent: ch });
}

// ---- settings kept on this machine (never chats) ---------------------------------------------

export type AiMode = "docked" | "floating" | "bar";
export const MODES: AiMode[] = ["docked", "floating", "bar"];
export const MODE_NAMES: Record<AiMode, string> = { docked: "Docked", floating: "Floating", bar: "Command bar" };

export interface Geo {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface AiSettings {
  model: string | null;
  effort: string;
  consent: boolean;
  hideSources: boolean;
  /** Docked width in px. */
  width: number;
  mode: AiMode;
  /** Floating sheet position and size; null until the user moves it. */
  float: Geo | null;
  /** ask: every change waits for Apply; auto: edits run, destructive ones ask; full: all run. */
  permissions: Permissions;
}

const KEY = "inkwell.ai";
const DEFAULTS: AiSettings = {
  model: null,
  effort: "low",
  consent: false,
  hideSources: false,
  width: 400,
  mode: "docked",
  float: null,
  permissions: "ask",
};

export function loadSettings(): AiSettings {
  try {
    const s: AiSettings = { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
    return {
      ...s,
      mode: MODES.includes(s.mode) ? s.mode : "docked",
      permissions: PERMISSIONS.includes(s.permissions) ? s.permissions : "ask",
    };
  } catch {
    return DEFAULTS;
  }
}

export function saveSettings(s: AiSettings) {
  localStorage.setItem(KEY, JSON.stringify(s));
}
