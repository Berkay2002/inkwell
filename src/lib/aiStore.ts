import { create } from "zustand";
import * as ai from "./ai";
import { historyCheckpoint } from "./history";
import {
  MODES,
  type Activity,
  type AiMode,
  type AiSettings,
  type AiStatus,
  type Citation,
  type Prepared,
  type ProviderId,
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
  followups: string[];
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
  /** This document's provider; absent = the last one picked (`settings.provider`). */
  provider?: ProviderId;
  /** Brought back from the document's earlier chats: this many messages are from before, and the assistant
   *  itself no longer remembers them. */
  restored?: number;
}

/** A chat set aside by "New chat", kept for the life of the tab so the command bar can list it. */
export interface PastChat {
  id: number;
  messages: Msg[];
}

/** The provider that answers this document. */
export const providerOf = (s: { chats: Record<number, Chat>; settings: AiSettings }, doc: number): ProviderId =>
  s.chats[doc]?.provider ?? s.settings.provider;

/** Model and effort the user chose for a provider (null = the provider's own default). */
export const pickOf = (st: AiSettings, p: ProviderId): { model: string | null; effort: string | null } =>
  p === "claude" ? { model: st.claudeModel, effort: st.claudeEffort } : { model: st.model, effort: st.effort };

/** Whether the user already agreed to send text to this provider's company. */
export const consentOf = (st: AiSettings, p: ProviderId) => (p === "claude" ? st.claudeConsent : st.consent);

export type StatusEntry = AiStatus | "loading" | null;

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
  /** Per provider: not asked yet (absent), being asked, or what the CLI said. */
  statuses: Partial<Record<ProviderId, AiStatus | "loading">>;
  settings: AiSettings;
  chats: Record<number, Chat>;
  selection: Selection | null;
  mark: Mark | null;
  /** Command-bar layout: the answer sheet is showing. */
  sheet: boolean;
  /** Earlier chats of each document, newest first. */
  past: Record<number, PastChat[]>;
  /** A question asked before the user agreed to send text to the provider. */
  pending: { doc: number; task: Task; label: string } | null;
  /** Text on its way back into a document's composer ("Edit" on a question). */
  draft: { doc: number; text: string; nonce: number } | null;

  setOpen: (open: boolean) => void;
  toggle: () => void;
  setMode: (mode: AiMode) => void;
  cycleMode: () => void;
  setSheet: (sheet: boolean) => void;
  /** Asks the provider's CLI again (default: the active document's provider). */
  refreshStatus: (provider?: ProviderId) => void;
  /** The `claude` the user picked (kept by Rust, never sent from here); null while Inkwell finds it itself. */
  claudePath: string | null;
  /** Opens the file dialog for `claude`. Resolves to why a file was refused, or null (chosen or closed). */
  pickClaudePath: () => Promise<string | null>;
  clearClaudePath: () => void;
  /** Gives a document another provider: its chat starts afresh (a thread cannot move between CLIs). */
  setProvider: (doc: number, provider: ProviderId) => void;
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
function restart(doc: number, provider: ProviderId, prepared?: (prep: Prepared | null) => void) {
  const p: Promise<void> = ai
    .aiClose(doc)
    .then(() => ai.aiPrepare(doc, provider))
    .then(
      (prep) => prepared?.(prep),
      () => prepared?.(null),
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
    const user: Msg = { id: ++msgId, role: "user", kind: task.kind, text: label, citations: [], followups: [], activity: [], state: "done", at };
    const bot: Msg = { id: ++msgId, role: "assistant", text: "", citations: [], followups: [], activity: [], state: "streaming", phase: "reading", at };
    patchChat(doc, (c) => ({ ...c, messages: [...c.messages, user, bot] }), true);
    held = null;
    set({ mark: null, sheet: true });
    const provider = providerOf(st, doc);
    // Codex gets what was picked; Claude only what its own model list offers (a leftover id from
    // another provider, or an effort the model lacks, falls back to Claude's default).
    let { model, effort } = pickOf(st.settings, provider);
    if (provider === "claude") {
      const s = st.statuses.claude;
      const known = typeof s === "object" ? s.models.find((m) => m.id === model) : undefined;
      model = known ? known.id : null;
      if (!known || !effort || !known.efforts.includes(effort)) effort = null;
    }
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
          effort,
          currentPage: d.currentPage,
          selection: st.selection,
          permissions: st.settings.permissions,
        },
        provider,
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
            case "reset":
              // what streamed so far was a lead-in to a tool call
              patchMsg(doc, bot.id, (m) => ({ ...m, text: "", phase: "thinking" }));
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
              reveal(doc, a.page, a.type === "show" ? a.rects : [], 0);
              break;
            }
            case "done":
              patchMsg(doc, bot.id, (m) => ({
                ...m,
                text: e.answer,
                citations: e.citations,
                followups: e.followups,
                activity: settle(m.activity),
                state: "done",
                timing: { firstTokenMs: e.firstTokenMs, totalMs: e.totalMs, prepMs: e.prepMs },
              }));
              break;
            case "error":
              patchMsg(doc, bot.id, (m) => ({ ...m, activity: settle(m.activity), state: "error", error: e.message, hint: e.hint }));
              if (e.hint === "login" || e.hint === "install") get().refreshStatus(provider);
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

  /** The provider of the document in front (or the last one picked, with no document open). */
  const activeProvider = (): ProviderId => {
    const a = useStore.getState().active;
    return a != null ? providerOf(get(), a) : get().settings.provider;
  };

  /** Scrolls the document to `page` and outlines `rects` there (`n` is the source badge lit, 0 = none). */
  const reveal = (doc: number, page: number, rects: Rect4[], n: number, msg?: number) => {
    held = null; // a real jump replaces whatever a hover had lit
    set({ mark: rects.length || n ? { doc, page, rects, n, nonce: ++markNonce, msg } : null });
    useStore.getState().goTo(doc, page, rects.length ? Math.max(0, Math.min(...rects.map((r) => r[1])) - 90) : undefined);
  };

  return {
    open: false,
    statuses: {},
    claudePath: null,
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

    refreshStatus: (provider) => {
      const p = provider ?? activeProvider();
      if (get().statuses[p] === "loading") return;
      const put = (status: AiStatus | "loading") => set((s) => ({ statuses: { ...s.statuses, [p]: status } }));
      put("loading");
      ai.aiStatus(p).then(put, (e) =>
        put({ installed: true, loggedIn: false, account: null, plan: null, models: [], error: String(e) }),
      );
      // which file was picked (Rust restored it at startup)
      if (p === "claude") void ai.aiClaudePath().then((claudePath) => set({ claudePath }), () => {});
    },

    pickClaudePath: async () => {
      try {
        const picked = await ai.aiPickClaudePath();
        if (picked) {
          set({ claudePath: picked });
          get().refreshStatus("claude");
        }
        return null;
      } catch (e) {
        return String(e);
      }
    },
    clearClaudePath: () => {
      set({ claudePath: null });
      void ai.aiClearClaudePath().then(() => get().refreshStatus("claude"), () => {});
    },

    setProvider: (doc, provider) => {
      if (providerOf(get(), doc) === provider) return;
      if (get().chats[doc]?.messages.some((m) => m.state === "streaming")) return;
      get().setSettings({ provider }); // the next document starts with it too
      patchChat(doc, (c) => ({ ...c, provider, messages: [], prep: "loading" }), true);
      set({ mark: null, pending: get().pending?.doc === doc ? null : get().pending });
      if (!get().statuses[provider]) get().refreshStatus(provider);
      // the old provider's session for this document goes; the text is read again for the new one
      held = null;
      unsent.delete(doc);
      restart(doc, provider, (prep) => patchChat(doc, (c) => ({ ...c, prep: prep ?? "failed" })));
    },

    prepare: (doc) => {
      const c = get().chats[doc] ?? EMPTY_CHAT;
      if (c.prep && c.prep !== "failed") return; // a failed read is tried again on the next open
      patchChat(doc, (c) => ({ ...c, prep: "loading" }), true);
      ai.aiPrepare(doc, providerOf(get(), doc)).then(
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
      if (!consentOf(get().settings, providerOf(get(), doc))) {
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
      set((s) => ({
        chats: { ...s.chats, [doc]: { messages: [], prep: s.chats[doc]?.prep ?? null, provider: s.chats[doc]?.provider } },
        mark: null,
      }));
      restart(doc, providerOf(get(), doc));
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
        chats: {
          ...s.chats,
          [doc]: { messages: hit.messages, prep: s.chats[doc]?.prep ?? null, provider: s.chats[doc]?.provider, restored: hit.messages.length },
        },
        mark: null,
      });
      // the model thread starts over, as for "New chat"
      restart(doc, providerOf(get(), doc));
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
      const p = get().pending;
      const provider = p ? providerOf(get(), p.doc) : get().settings.provider;
      get().setSettings(provider === "claude" ? { claudeConsent: true } : { consent: true });
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
