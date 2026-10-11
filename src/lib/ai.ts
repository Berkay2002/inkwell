import { Channel, invoke } from "@tauri-apps/api/core";

export type Rect4 = [number, number, number, number];

export interface ModelInfo {
  id: string;
  name: string;
  efforts: string[];
  defaultEffort: string | null;
  isDefault: boolean;
}

/** Which CLI answers: Codex (ChatGPT subscription) or Claude Code (Claude subscription). */
export type ProviderId = "codex" | "claude";
export const PROVIDERS: ProviderId[] = ["codex", "claude"];
export const PROVIDER_NAMES: Record<ProviderId, string> = { codex: "Codex", claude: "Claude" };

export interface AiStatus {
  installed: boolean;
  loggedIn: boolean;
  account: string | null;
  plan: string | null;
  models: ModelInfo[];
  error: string | null;
  /** The CLI's own version, when it said. */
  version?: string | null;
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
  class: "read" | "edit" | "destructive";
  /** What and where, ready to show ("Searched “Ciri”", "Deleted page 4"). */
  label: string;
  state: "waiting" | "running" | "done" | "failed" | "rejected";
  detail: string | null;
}

/** The model moved the user's view. */
export type UiAction =
  | { type: "goTo"; page: number }
  | { type: "show"; page: number; rects: Rect4[]; quote: string };

export type ChatEvent =
  | { kind: "status"; phase: "reading" | "connecting" | "thinking" }
  | { kind: "delta"; text: string }
  /** What was streamed so far was a preamble before a tool call, not the answer: start the text again. */
  | { kind: "reset" }
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

export const aiStatus = (provider: ProviderId) => invoke<AiStatus>("ai_status", { provider });
export const aiPrepare = (doc: number, provider: ProviderId) => invoke<Prepared>("ai_prepare", { doc, provider });
/** The user's own path to `claude` (empty = look for it). */
export const aiSetClaudePath = (path: string | null) => invoke<void>("ai_set_claude_path", { path });
export const aiCancel = (doc: number) => invoke<void>("ai_cancel", { doc });
export const aiClose = (doc: number) => invoke<void>("ai_close", { doc });
/** The mode changed while a turn runs: its very next tool call is checked against it. */
export const aiSetPermissions = (doc: number, mode: Permissions) => invoke<void>("ai_set_permissions", { doc, mode });
/** The user's Apply (true) or Reject (false) on a waiting tool call. */
export const aiToolDecision = (doc: number, callId: string, allow: boolean) =>
  invoke<void>("ai_tool_decision", { doc, callId, allow });

/** Starts a question; every event of the answer arrives through `onEvent`. */
export function aiAsk(doc: number, req: AskRequest, provider: ProviderId, onEvent: (e: ChatEvent) => void) {
  const ch = new Channel<ChatEvent>();
  ch.onmessage = onEvent;
  return invoke<void>("ai_ask", { doc, req, provider, onEvent: ch });
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
  /** The provider a document without its own choice uses (the last one picked). */
  provider: ProviderId;
  /** Model and effort chosen for Codex; Claude's are `claudeModel` / `claudeEffort`. */
  model: string | null;
  effort: string;
  claudeModel: string | null;
  claudeEffort: string | null;
  /** The user's own path to claude(.exe), when it is not on PATH. */
  claudePath: string | null;
  /** Agreed to send document text to OpenAI (Codex) / to Anthropic (Claude): each is asked once. */
  consent: boolean;
  claudeConsent: boolean;
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
  provider: "codex",
  model: null,
  effort: "low",
  // Claude starts on the small, quick model; a choice made in the picker is kept
  claudeModel: "haiku",
  claudeEffort: "medium",
  claudePath: null,
  consent: false,
  claudeConsent: false,
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
      // null was "never chosen" before the defaults existed: the picker only ever stores an id
      claudeModel: s.claudeModel ?? DEFAULTS.claudeModel,
      claudeEffort: s.claudeEffort ?? DEFAULTS.claudeEffort,
      provider: PROVIDERS.includes(s.provider) ? s.provider : "codex",
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
