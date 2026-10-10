import { Channel, invoke } from "@tauri-apps/api/core";

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

export interface AskRequest {
  task: Task;
  name: string;
  model?: string | null;
  effort?: string | null;
  currentPage?: number | null;
}

export type ChatEvent =
  | { kind: "status"; phase: "reading" | "connecting" | "thinking" }
  | { kind: "delta"; text: string }
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

/** Starts a question; every event of the answer arrives through `onEvent`. */
export function aiAsk(doc: number, req: AskRequest, onEvent: (e: ChatEvent) => void) {
  const ch = new Channel<ChatEvent>();
  ch.onmessage = onEvent;
  return invoke<void>("ai_ask", { doc, req, onEvent: ch });
}

// ---- settings kept on this machine (never chats) ---------------------------------------------

export interface AiSettings {
  model: string | null;
  effort: string;
  consent: boolean;
  hideSources: boolean;
  width: number;
}

const KEY = "inkwell.ai";
const DEFAULTS: AiSettings = { model: null, effort: "low", consent: false, hideSources: false, width: 400 };

export function loadSettings(): AiSettings {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(KEY) ?? "{}") };
  } catch {
    return DEFAULTS;
  }
}

export function saveSettings(s: AiSettings) {
  localStorage.setItem(KEY, JSON.stringify(s));
}
