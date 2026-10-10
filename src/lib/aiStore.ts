import { create } from "zustand";
import * as ai from "./ai";
import { MODES, type AiMode, type AiSettings, type AiStatus, type Citation, type Prepared, type Rect4, type Task } from "./ai";
import { useStore } from "../store";

export interface Msg {
  id: number;
  role: "user" | "assistant";
  /** Of a user message: what was asked (the floating layout's Summary tab finds the summary by it). */
  kind?: Task["kind"];
  text: string;
  citations: Citation[];
  followups: string[];
  state: "streaming" | "done" | "error" | "cancelled";
  phase?: string;
  error?: string;
  hint?: string | null;
  timing?: { firstTokenMs: number | null; totalMs: number; prepMs: number };
}

export interface Chat {
  messages: Msg[];
  prep: Prepared | "loading" | "failed" | null;
}

/** The outlined source of the citation the user clicked last. */
export interface Mark {
  doc: number;
  page: number;
  rects: Rect4[];
  n: number;
  nonce: number;
}

export interface Selection {
  text: string;
  /** 0-based */
  page: number;
}

interface AiStore {
  open: boolean;
  status: AiStatus | "loading" | null;
  settings: AiSettings;
  chats: Record<number, Chat>;
  selection: Selection | null;
  mark: Mark | null;
  /** Command-bar layout: the answer sheet is showing. */
  sheet: boolean;
  /** A question asked before the user agreed to send text to OpenAI. */
  pending: { doc: number; task: Task; label: string } | null;

  setOpen: (open: boolean) => void;
  toggle: () => void;
  setMode: (mode: AiMode) => void;
  cycleMode: () => void;
  setSheet: (sheet: boolean) => void;
  refreshStatus: () => void;
  prepare: (doc: number) => void;
  ask: (doc: number, task: Task, label?: string) => void;
  cancel: (doc: number) => void;
  clear: (doc: number) => void;
  closeChat: (doc: number) => void;
  grantConsent: () => void;
  declineConsent: () => void;
  setSettings: (p: Partial<AiSettings>) => void;
  setSelection: (s: Selection | null) => void;
  showCitation: (doc: number, c: Citation) => void;
  clearMark: () => void;
  transcript: (doc: number) => string;
}

let msgId = 0;
let markNonce = 0;
const EMPTY_CHAT: Chat = { messages: [], prep: null };

export const useAi = create<AiStore>((set, get) => {
  // Late events of a closed tab must not bring its chat back: only `create` starts a chat.
  const patchChat = (doc: number, f: (c: Chat) => Chat, create = false) =>
    set((s) => {
      const c = s.chats[doc] ?? (create ? EMPTY_CHAT : null);
      return c ? { chats: { ...s.chats, [doc]: f(c) } } : s;
    });
  const patchMsg = (doc: number, id: number, f: (m: Msg) => Msg) =>
    patchChat(doc, (c) => ({ ...c, messages: c.messages.map((m) => (m.id === id ? f(m) : m)) }));

  const run = (doc: number, task: Task, label: string) => {
    const d = useStore.getState().docs[doc];
    if (!d) return;
    const st = get();
    const chat = st.chats[doc] ?? EMPTY_CHAT;
    if (chat.messages.some((m) => m.state === "streaming")) return;
    const user: Msg = { id: ++msgId, role: "user", kind: task.kind, text: label, citations: [], followups: [], state: "done" };
    const bot: Msg = { id: ++msgId, role: "assistant", text: "", citations: [], followups: [], state: "streaming", phase: "reading" };
    patchChat(doc, (c) => ({ ...c, messages: [...c.messages, user, bot] }), true);
    set({ mark: null, sheet: true });
    const model = st.settings.model;
    ai.aiAsk(
      doc,
      { task, name: d.name, model, effort: st.settings.effort, currentPage: d.currentPage },
      (e) => {
        switch (e.kind) {
          case "status":
            patchMsg(doc, bot.id, (m) => ({ ...m, phase: e.phase }));
            break;
          case "delta":
            patchMsg(doc, bot.id, (m) => ({ ...m, text: m.text + e.text, phase: "answering" }));
            break;
          case "done":
            patchMsg(doc, bot.id, (m) => ({
              ...m,
              text: e.answer,
              citations: e.citations,
              followups: e.followups,
              state: "done",
              timing: { firstTokenMs: e.firstTokenMs, totalMs: e.totalMs, prepMs: e.prepMs },
            }));
            break;
          case "error":
            patchMsg(doc, bot.id, (m) => ({ ...m, state: "error", error: e.message, hint: e.hint }));
            if (e.hint === "login" || e.hint === "install") get().refreshStatus();
            break;
          case "cancelled":
            patchMsg(doc, bot.id, (m) => ({ ...m, state: "cancelled" }));
            break;
        }
      },
    ).catch((err) => patchMsg(doc, bot.id, (m) => ({ ...m, state: "error", error: String(err), hint: null })));
  };

  return {
    open: false,
    status: null,
    settings: ai.loadSettings(),
    chats: {},
    selection: null,
    mark: null,
    sheet: false,
    pending: null,

    setOpen: (open) => {
      set({ open });
      if (open) get().refreshStatus();
      else set({ mark: null });
    },
    toggle: () => get().setOpen(!get().open),
    setMode: (mode) => {
      get().setSettings({ mode });
      get().setOpen(true);
    },
    cycleMode: () => get().setMode(MODES[(MODES.indexOf(get().settings.mode) + 1) % MODES.length]),
    setSheet: (sheet) => set(sheet ? { sheet } : { sheet, mark: null }),

    refreshStatus: () => {
      if (get().status === "loading") return;
      set({ status: "loading" });
      ai.aiStatus().then(
        (status) => set({ status }),
        (e) =>
          set({ status: { installed: true, loggedIn: false, account: null, plan: null, models: [], error: String(e) } }),
      );
    },

    prepare: (doc) => {
      const c = get().chats[doc] ?? EMPTY_CHAT;
      if (c.prep && c.prep !== "failed") return; // a failed read is tried again on the next open
      patchChat(doc, (c) => ({ ...c, prep: "loading" }), true);
      ai.aiPrepare(doc).then(
        (prep) => patchChat(doc, (c) => ({ ...c, prep })),
        () => patchChat(doc, (c) => ({ ...c, prep: "failed" })),
      );
    },

    ask: (doc, task, label) => {
      const text =
        label ??
        (task.kind === "ask"
          ? task.question
          : task.kind === "summarizeDocument"
            ? "Summarize this document"
            : task.kind === "summarizePage"
              ? `Summarize page ${task.page + 1}`
              : `${task.action[0].toUpperCase()}${task.action.slice(1)} the selected text`);
      if (!get().settings.consent) {
        set({ pending: { doc, task, label: text } });
        return;
      }
      run(doc, task, text);
    },

    cancel: (doc) => void ai.aiCancel(doc),

    clear: (doc) => {
      // a fresh thread: the model forgets the conversation (a turn in flight is interrupted)
      set((s) => ({ chats: { ...s.chats, [doc]: { messages: [], prep: s.chats[doc]?.prep ?? null } }, mark: null }));
      void ai
        .aiClose(doc)
        .then(() => ai.aiPrepare(doc))
        .catch(() => {});
    },

    closeChat: (doc) => {
      void ai.aiClose(doc);
      set((s) => {
        const chats = { ...s.chats };
        delete chats[doc];
        return { chats, mark: s.mark?.doc === doc ? null : s.mark };
      });
    },

    grantConsent: () => {
      get().setSettings({ consent: true });
      const p = get().pending;
      set({ pending: null });
      if (p) run(p.doc, p.task, p.label);
    },
    declineConsent: () => set({ pending: null }),

    setSettings: (p) => {
      const settings = { ...get().settings, ...p };
      ai.saveSettings(settings);
      set({ settings });
    },

    setSelection: (selection) => {
      const a = get().selection;
      if (a?.text === selection?.text && a?.page === selection?.page) return;
      set({ selection });
    },

    showCitation: (doc, c) => {
      const st = useStore.getState();
      if (c.page == null) return;
      const m = get().mark;
      if (m && m.doc === doc && m.n === c.n && m.page === c.page && m.rects === c.rects) return set({ mark: null });
      set({ mark: { doc, page: c.page, rects: c.rects, n: c.n, nonce: ++markNonce } });
      st.goTo(doc, c.page, c.rects.length ? Math.max(0, Math.min(...c.rects.map((r) => r[1])) - 90) : undefined);
    },
    clearMark: () => set({ mark: null }),

    transcript: (doc) =>
      (get().chats[doc]?.messages ?? [])
        .map((m) => `${m.role === "user" ? "You" : "Assistant"}: ${m.text}`)
        .join("\n\n"),
  };
});

// A closed tab ends its chat: unsubscribes the thread, drops the extracted text.
useStore.subscribe((s, prev) => {
  for (const id of Object.keys(prev.docs)) {
    if (!s.docs[Number(id)]) useAi.getState().closeChat(Number(id));
  }
});

/** The selected text in the viewer and the page it is on, or null. */
export function readViewerSelection(): Selection | null {
  const sel = window.getSelection();
  const text = sel?.toString().trim() ?? "";
  if (!sel || sel.isCollapsed || text === "") return null;
  const node = sel.anchorNode instanceof Element ? sel.anchorNode : sel.anchorNode?.parentElement;
  const page = node?.closest<HTMLElement>(".viewer .page[data-page]");
  if (!page) return null;
  return { text: text.slice(0, 6000), page: Number(page.dataset.page) };
}
