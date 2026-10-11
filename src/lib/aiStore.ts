import { create } from "zustand";
import * as ai from "./ai";
import { applyState } from "./edit";
import { useForms } from "./forms";
import { historyCheckpoint } from "./history";
import { useRedact } from "./redact";
import {
  MODES,
  type Activity,
  type AiMode,
  type AiSettings,
  type AiStatus,
  type Citation,
  type Prepared,
  type Rect4,
  type Task,
} from "./ai";
import { useStore } from "../store";

export interface Msg {
  id: number;
  role: "user" | "assistant";
  /** Of a user message: what was asked (the floating layout's Summary tab finds the summary by it). */
  kind?: Task["kind"];
  text: string;
  citations: Citation[];
  /** The tool calls of this turn (searches, page reads, changes), in order. */
  activity: Activity[];
  state: "streaming" | "done" | "error" | "cancelled";
  phase?: string;
  error?: string;
  hint?: string | null;
  timing?: { firstTokenMs: number | null; totalMs: number; prepMs: number };
  /** When the message was made (ms since the epoch). */
  at?: number;
}

export interface Chat {
  messages: Msg[];
  prep: Prepared | "loading" | "failed" | null;
  /** Brought back from the document's earlier chats: this many messages are from before, and the assistant
   *  itself no longer remembers them. */
  restored?: number;
}

/** A chat set aside by "New chat", kept for the life of the tab so the command bar can list it. */
export interface PastChat {
  id: number;
  messages: Msg[];
}

/** The outlined source of the citation the user clicked last. */
export interface Mark {
  doc: number;
  page: number;
  rects: Rect4[];
  n: number;
  nonce: number;
  /** The answer whose chip or citation this is (source numbers repeat from answer to answer). */
  msg?: number;
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
  /** Earlier chats of each document, newest first. */
  past: Record<number, PastChat[]>;
  /** A question asked before the user agreed to send text to OpenAI. */
  pending: { doc: number; task: Task; label: string } | null;
  /** Text on its way back into a document's composer ("Edit" on a question). */
  draft: { doc: number; text: string; nonce: number } | null;

  setOpen: (open: boolean) => void;
  toggle: () => void;
  setMode: (mode: AiMode) => void;
  cycleMode: () => void;
  setSheet: (sheet: boolean) => void;
  refreshStatus: () => void;
  prepare: (doc: number) => void;
  ask: (doc: number, task: Task, label?: string) => void;
  cancel: (doc: number) => void;
  /** Apply (true) or Reject (false) a change the assistant is waiting to make. */
  decide: (doc: number, callId: string, allow: boolean) => void;
  clear: (doc: number) => void;
  /** Set the current chat aside (it stays listed in `past`) and start a fresh one. */
  newChat: (doc: number) => void;
  /** Bring an earlier chat back as the current one; the current chat is set aside. */
  openChat: (doc: number, id: number) => void;
  closeChat: (doc: number) => void;
  grantConsent: () => void;
  declineConsent: () => void;
  setSettings: (p: Partial<AiSettings>) => void;
  setSelection: (s: Selection | null) => void;
  /** `msg`: the answer the citation sits in. */
  showCitation: (doc: number, c: Citation, msg?: number) => void;
  /** Light a source badge on the page without scrolling there; `dropMark` puts back what was lit. */
  liftMark: (doc: number, c: Citation, msg: number) => void;
  dropMark: () => void;
  setDraft: (doc: number, text: string) => void;
  clearDraft: () => void;
  clearMark: () => void;
  transcript: (doc: number) => string;
}

let msgId = 0;
let markNonce = 0;
const EMPTY_CHAT: Chat = { messages: [], prep: null };
let pastId = 0;
let draftNonce = 0;
/** The mark the user had before a hover lit another (see `liftMark`). */
let held: { prev: Mark | null } | null = null;
/** Documents whose model thread is being closed and prepared again: a question waits for it. */
const resets = new Map<number, Promise<void>>();
/** Documents whose newest answer is waiting for `resets`: it has not been sent yet (doc -> answer id). */
const unsent = new Map<number, number>();

/** The turn is over: a card nobody answered did not run. */
const settle = (acts: Activity[]): Activity[] =>
  acts.map((a) => (a.state === "waiting" ? { ...a, state: "rejected" } : a.state === "running" ? { ...a, state: "failed" } : a));

/** A chat for the list of earlier ones: a turn that was still running is over, and so are its cards. */
const pastOf = (messages: Msg[]): PastChat => ({
  id: ++pastId,
  messages: messages.map((m) => (m.state === "streaming" ? { ...m, state: "cancelled" as const, activity: settle(m.activity) } : m)),
});

/** A fresh model thread for the document (the old one is closed, the text read again). */
function restart(doc: number) {
  const p: Promise<void> = ai
    .aiClose(doc)
    .then(() => ai.aiPrepare(doc))
    .then(
      () => {},
      () => {},
    );
  resets.set(doc, p);
  void p.then(() => {
    if (resets.get(doc) === p) resets.delete(doc);
  });
}

export const useAi = create<AiStore>((set, get) => {
  // Late events of a closed tab must not bring its chat back: only `create` starts a chat.
  const patchChat = (doc: number, f: (c: Chat) => Chat, create = false) =>
    set((s) => {
      const c = s.chats[doc] ?? (create ? EMPTY_CHAT : null);
      return c ? { chats: { ...s.chats, [doc]: f(c) } } : s;
    });
  const patchMsg = (doc: number, id: number, f: (m: Msg) => Msg) =>
    patchChat(doc, (c) => ({ ...c, messages: c.messages.map((m) => (m.id === id ? f(m) : m)) }));

  const run =(doc: number, task: Task, label: string) => {
    const d = useStore.getState().docs[doc];
    if (!d) return;
    const st = get();
    const chat = st.chats[doc] ?? EMPTY_CHAT;
    if (chat.messages.some((m) => m.state === "streaming")) return;
    const at = Date.now();
    const user: Msg = { id: ++msgId, role: "user", kind: task.kind, text: label, citations: [], activity: [], state: "done", at };
    const bot: Msg = { id: ++msgId, role: "assistant", text: "", citations: [], activity: [], state: "streaming", phase: "reading", at };
    patchChat(doc, (c) => ({ ...c, messages: [...c.messages, user, bot] }), true);
    held = null;
    set({ mark: null, sheet: true });
    const model = st.settings.model;
    const start = () => {
      if (unsent.get(doc) === bot.id) unsent.delete(doc);
      // the chat was cleared, switched or stopped while the model thread was being renewed: nothing to ask
      if (!get().chats[doc]?.messages.some((m) => m.id === bot.id && m.state === "streaming")) return Promise.resolve();
      return ai.aiAsk(
        doc,
        {
          task,
          name: d.name,
          model,
          effort: st.settings.effort,
          currentPage: d.currentPage,
          selection: st.selection,
          permissions: st.settings.permissions,
        },
        (e) => {
          // the turn is over: what it changed (if anything) becomes a version history checkpoint
          if (e.kind === "done" || e.kind === "error" || e.kind === "cancelled") void historyCheckpoint(doc).catch(() => {});
          switch (e.kind) {
            case "status":
              patchMsg(doc, bot.id, (m) => ({ ...m, phase: e.phase }));
              break;
            case "delta":
              patchMsg(doc, bot.id, (m) => ({ ...m, text: m.text + e.text, phase: "answering" }));
              break;
            case "activity": {
              const { kind: _kind, ...a } = e;
              patchMsg(doc, bot.id, (m) => ({
                ...m,
                activity: m.activity.some((x) => x.id === a.id)
                  ? m.activity.map((x) => (x.id === a.id ? a : x))
                  : [...m.activity, a],
              }));
              break;
            }
            case "ui": {
              const a = e.action;
              if (a.type === "edited") adopt(doc, a);
              else reveal(doc, a.page, a.type === "show" ? a.rects : [], 0);
              break;
            }
            case "done":
              patchMsg(doc, bot.id, (m) => ({
                ...m,
                text: e.answer,
                citations: e.citations,
                activity: settle(m.activity),
                state: "done",
                timing: { firstTokenMs: e.firstTokenMs, totalMs: e.totalMs, prepMs: e.prepMs },
              }));
              break;
            case "error":
              patchMsg(doc, bot.id, (m) => ({ ...m, activity: settle(m.activity), state: "error", error: e.message, hint: e.hint }));
              if (e.hint === "login" || e.hint === "install") get().refreshStatus();
              break;
            case "cancelled":
              patchMsg(doc, bot.id, (m) => ({ ...m, activity: settle(m.activity), state: "cancelled" }));
              break;
          }
        },
      );
    };
    // after "New chat" or a switch the old thread is still being closed: asking now would be closed with it
    const gate = resets.get(doc);
    if (gate) unsent.set(doc, bot.id);
    (gate ? gate.then(start) : start()).catch((err) =>
      patchMsg(doc, bot.id, (m) => ({ ...m, activity: settle(m.activity), state: "error", error: String(err), hint: null })),
    );
  };

  /** The assistant changed the document: its new generation becomes the document's, as after any edit,
   *  and a redaction it applied is shown in the Redact rail (a docked assistant keeps its place: the
   *  message has a Review in Redact button instead). */
  const adopt = (doc: number, a: Extract<ai.UiAction, { type: "edited" }>) => {
    applyState(doc, a.state);
    if (useForms.getState().docs[doc]) void useForms.getState().refresh(doc);
    const r = a.redacted;
    if (!r) return;
    useRedact.getState().showApplied(doc, {
      status: "applied",
      state: a.state,
      marks: r.marks,
      sanitize: r.sanitize,
      verify: r.verify,
      savedTo: null,
    });
    if (get().settings.mode !== "docked") useRedact.getState().setOpen(true);
  };

  /** Scrolls the document to `page` and outlines `rects` there (`n` is the source badge lit, 0 = none). */
  const reveal = (doc: number, page: number, rects: Rect4[], n: number, msg?: number) => {
    held = null; // a real jump replaces whatever a hover had lit
    set({ mark: rects.length || n ? { doc, page, rects, n, nonce: ++markNonce, msg } : null });
    useStore.getState().goTo(doc, page, rects.length ? Math.max(0, Math.min(...rects.map((r) => r[1])) - 90) : undefined);
  };

  return {
    open: false,
    status: null,
    settings: ai.loadSettings(),
    chats: {},
    selection: null,
    mark: null,
    sheet: false,
    past: {},
    pending: null,
    draft: null,

    setOpen: (open) => {
      set({ open });
      if (open) get().refreshStatus();
      else {
        held = null;
        set({ mark: null });
      }
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

    cancel: (doc) => {
      // Stop on an answer that is still waiting for the thread reset: nothing is running to stop, so the
      // answer is closed here and `start` finds it so and never sends it
      const waiting = unsent.get(doc);
      if (waiting == null) return void ai.aiCancel(doc);
      unsent.delete(doc);
      patchMsg(doc, waiting, (m) => ({ ...m, activity: settle(m.activity), state: "cancelled" }));
    },

    decide: (doc, callId, allow) => {
      // the card answers at once; the activity events that follow confirm or correct it
      const step = (state: Activity["state"], from: Activity["state"]) =>
        patchChat(doc, (c) => ({
          ...c,
          messages: c.messages.map((m) => ({
            ...m,
            activity: m.activity.map((a) => (a.id === callId && a.state === from ? { ...a, state } : a)),
          })),
        }));
      const next = allow ? "running" : "rejected";
      step(next, "waiting");
      ai.aiToolDecision(doc, callId, allow).catch(() => step("failed", next));
    },

    clear: (doc) => {
      // a fresh thread: the model forgets the conversation (a turn in flight is interrupted)
      held = null;
      unsent.delete(doc);
      set((s) => ({ chats: { ...s.chats, [doc]: { messages: [], prep: s.chats[doc]?.prep ?? null } }, mark: null }));
      restart(doc);
    },

    newChat: (doc) => {
      const cur = get().chats[doc]?.messages ?? [];
      if (cur.length > 0) set((s) => ({ past: { ...s.past, [doc]: [pastOf(cur), ...(s.past[doc] ?? [])] } }));
      get().clear(doc);
    },

    openChat: (doc, id) => {
      const s = get();
      const hit = s.past[doc]?.find((p) => p.id === id);
      if (!hit) return;
      const cur = s.chats[doc]?.messages ?? [];
      const rest = (s.past[doc] ?? []).filter((p) => p.id !== id);
      held = null;
      unsent.delete(doc);
      set({
        past: { ...s.past, [doc]: cur.length > 0 ? [pastOf(cur), ...rest] : rest },
        chats: { ...s.chats, [doc]: { messages: hit.messages, prep: s.chats[doc]?.prep ?? null, restored: hit.messages.length } },
        mark: null,
      });
      // the model thread starts over, as for "New chat"
      restart(doc);
    },

    closeChat: (doc) => {
      held = null;
      resets.delete(doc);
      unsent.delete(doc);
      void ai.aiClose(doc);
      set((s) => {
        const chats = { ...s.chats };
        delete chats[doc];
        const past = { ...s.past };
        delete past[doc];
        return { chats, past, mark: s.mark?.doc === doc ? null : s.mark };
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
      // a turn in flight must not keep the looser mode it started with
      if (p.permissions) for (const doc of Object.keys(get().chats)) void ai.aiSetPermissions(Number(doc), p.permissions);
    },

    setSelection: (selection) => {
      const a = get().selection;
      if (a?.text === selection?.text && a?.page === selection?.page) return;
      set({ selection });
    },

    showCitation: (doc, c, msg) => {
      if (c.page == null) return;
      // a hover may have lit this very citation: the click is what turns it off again
      const m = held ? held.prev : get().mark;
      if (m && m.doc === doc && m.n === c.n && m.page === c.page && m.rects === c.rects) {
        held = null;
        return set({ mark: null });
      }
      reveal(doc, c.page, c.rects, c.n, msg);
    },
    liftMark: (doc, c, msg) => {
      if (c.page == null) return;
      held ??= { prev: get().mark };
      set({ mark: { doc, page: c.page, rects: c.rects, n: c.n, nonce: ++markNonce, msg } });
    },
    dropMark: () => {
      if (!held) return;
      const { prev } = held;
      held = null;
      set({ mark: prev });
    },
    setDraft: (doc, text) => set({ draft: { doc, text, nonce: ++draftNonce } }),
    clearDraft: () => set({ draft: null }),
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
