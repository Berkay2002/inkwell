import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatEvent } from "./ai";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});

let emit: (e: ChatEvent) => void = () => {};
vi.mock("./ai", () => ({
  aiAsk: vi.fn((_doc: number, _req: unknown, _provider: string, cb: (e: ChatEvent) => void) => {
    emit = cb;
    return Promise.resolve();
  }),
  aiStatus: vi.fn(() => Promise.resolve({ installed: true, loggedIn: true, models: [] })),
  aiPrepare: vi.fn(() => Promise.resolve({ pages: 1, chars: 500, enough: true })),
  aiCancel: vi.fn(() => Promise.resolve()),
  aiClose: vi.fn(() => Promise.resolve()),
  aiToolDecision: vi.fn(() => Promise.resolve()),
  aiSetPermissions: vi.fn(() => Promise.resolve()),
  aiPickClaudePath: vi.fn((): Promise<string | null> => Promise.resolve("C:\\tools\\claude.exe")),
  aiClaudePath: vi.fn((): Promise<string | null> => Promise.resolve(null)),
  aiClearClaudePath: vi.fn(() => Promise.resolve()),
  loadSettings: () => ({
    provider: "codex",
    model: null,
    effort: "low",
    claudeModel: null,
    claudeEffort: null,
    consent: false,
    claudeConsent: false,
    hideSources: false,
    width: 400,
    mode: "docked",
    float: null,
    permissions: "ask",
  }),
  saveSettings: vi.fn(),
  MODES: ["docked", "floating", "bar"],
  MODE_NAMES: { docked: "Docked", floating: "Floating", bar: "Command bar" },
}));

vi.mock("./history", () => ({ historyCheckpoint: vi.fn(() => Promise.resolve(null)) }));

import * as ai from "./ai";
import { historyCheckpoint } from "./history";
import { providerOf, useAi } from "./aiStore";
import { useStore } from "../store";

const goTo = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({
    docs: { 1: { id: 1, name: "a.pdf", currentPage: 0 } as never },
    order: [1],
    active: 1,
    goTo,
  });
  useAi.setState({
    chats: {},
    pending: null,
    mark: null,
    statuses: {},
    claudePath: null,
    settings: { ...useAi.getState().settings, consent: false, claudeConsent: false, provider: "codex" },
  });
});

describe("assistant store", () => {
  it("holds a question until the user consents, then streams the answer", () => {
    useAi.getState().ask(1, { kind: "ask", question: "why?" });
    expect(ai.aiAsk).not.toHaveBeenCalled();
    expect(useAi.getState().pending?.label).toBe("why?");

    useAi.getState().grantConsent();
    expect(ai.aiAsk).toHaveBeenCalledTimes(1);
    expect(useAi.getState().pending).toBeNull();

    emit({ kind: "delta", text: "Hel" });
    emit({ kind: "delta", text: "lo [1]" });
    const [, bot] = useAi.getState().chats[1].messages;
    expect(bot.text).toBe("Hello [1]");
    expect(bot.state).toBe("streaming");

    const citations = [{ n: 1, page: 4, rects: [[1, 2, 3, 4]], quote: "q", chunk: "p5.1", exact: true }] as never;
    emit({ kind: "done", answer: "Hello [1]", citations, followups: ["more?"], firstTokenMs: 10, totalMs: 20, prepMs: 1, chunksSent: 1, chunksTotal: 1 });
    const done = useAi.getState().chats[1].messages[1];
    expect(done.state).toBe("done");
    expect(done.followups).toEqual(["more?"]);
  });

  it("jumps to a citation, marks it, and toggles the mark off on a second click", () => {
    const c = { n: 2, page: 6, rects: [[10, 300, 50, 320]], quote: "q", chunk: null, exact: true } as never;
    useAi.getState().showCitation(1, c);
    expect(useAi.getState().mark).toMatchObject({ doc: 1, page: 6, n: 2 });
    expect(goTo).toHaveBeenCalledWith(1, 6, 210);
    useAi.getState().showCitation(1, c);
    expect(useAi.getState().mark).toBeNull();
  });

  it("does not bring a closed tab's chat back when a late event arrives", () => {
    useAi.getState().setSettings({ consent: true });
    useAi.getState().ask(1, { kind: "ask", question: "q" });
    useStore.setState({ docs: {}, order: [], active: null });
    expect(useAi.getState().chats[1]).toBeUndefined();
    emit({ kind: "delta", text: "late" });
    emit({ kind: "cancelled" });
    expect(useAi.getState().chats[1]).toBeUndefined();
  });

  it("tries the document read again after a failure", async () => {
    vi.mocked(ai.aiPrepare).mockRejectedValueOnce(new Error("boom"));
    useAi.getState().prepare(1);
    await new Promise((r) => setTimeout(r, 0));
    expect(useAi.getState().chats[1].prep).toBe("failed");
    useAi.getState().prepare(1);
    await new Promise((r) => setTimeout(r, 0));
    expect(useAi.getState().chats[1].prep).toMatchObject({ enough: true });
    expect(ai.aiPrepare).toHaveBeenCalledTimes(2);
  });

  it("cycles the layout docked, floating, bar, docked and keeps it in the saved settings", () => {
    useAi.setState({ open: false, settings: { ...useAi.getState().settings, mode: "docked" } });
    useAi.getState().cycleMode();
    expect(useAi.getState().settings.mode).toBe("floating");
    expect(useAi.getState().open).toBe(true);
    useAi.getState().cycleMode();
    expect(useAi.getState().settings.mode).toBe("bar");
    expect(ai.saveSettings).toHaveBeenLastCalledWith(expect.objectContaining({ mode: "bar" }));
    useAi.getState().cycleMode();
    expect(useAi.getState().settings.mode).toBe("docked");
    useAi.getState().setMode("bar");
    expect(useAi.getState().settings.mode).toBe("bar");
  });

  it("opens the bar's answer sheet when a question is asked and clears the mark when it is closed", () => {
    useAi.getState().setSettings({ consent: true });
    useAi.setState({ sheet: false });
    useAi.getState().ask(1, { kind: "ask", question: "q" });
    expect(useAi.getState().sheet).toBe(true);
    expect(useAi.getState().chats[1].messages[0].kind).toBe("ask");
    useAi.setState({ mark: { doc: 1, page: 0, rects: [], n: 1, nonce: 1 } });
    useAi.getState().setSheet(false);
    expect(useAi.getState().mark).toBeNull();
  });

  it("a finished, cancelled or failed turn is no longer streaming (nothing keeps the cursor)", () => {
    useAi.getState().setSettings({ consent: true });
    useAi.getState().ask(1, { kind: "ask", question: "q" });
    emit({ kind: "delta", text: "partial" });
    expect(useAi.getState().chats[1].messages[1].state).toBe("streaming");
    emit({ kind: "cancelled" });
    expect(useAi.getState().chats[1].messages[1].state).toBe("cancelled");
  });

  it("asks for a history checkpoint when a turn ends, however it ends", () => {
    useAi.getState().setSettings({ consent: true });
    useAi.getState().ask(1, { kind: "ask", question: "q" });
    emit({ kind: "delta", text: "partial" });
    expect(historyCheckpoint).not.toHaveBeenCalled();
    emit({ kind: "error", message: "x", hint: null } as ChatEvent);
    expect(historyCheckpoint).toHaveBeenCalledWith(1);
  });

  it("sends the permission mode and the selection with the question", () => {
    useAi.getState().setSettings({ consent: true, permissions: "auto" });
    useAi.setState({ selection: { text: "thirty days", page: 2 } });
    useAi.getState().ask(1, { kind: "ask", question: "q" });
    expect(vi.mocked(ai.aiAsk).mock.calls[0][1]).toMatchObject({
      permissions: "auto",
      selection: { text: "thirty days", page: 2 },
    });
    useAi.getState().setSettings({ permissions: "ask" });
  });

  it("tells a turn in flight when the permission mode is tightened", () => {
    useAi.getState().setSettings({ consent: true, permissions: "full" });
    useAi.getState().ask(1, { kind: "ask", question: "q" });
    useAi.getState().setSettings({ permissions: "ask" });
    expect(ai.aiSetPermissions).toHaveBeenCalledWith(1, "ask");
    vi.mocked(ai.aiSetPermissions).mockClear();
    useAi.getState().setSettings({ hideSources: true }); // unrelated settings do not ping the backend
    expect(ai.aiSetPermissions).not.toHaveBeenCalled();
  });

  describe("tool activity", () => {
    const act = (state: string, id = "c1") =>
      ({ kind: "activity", id, tool: "rotate_page", class: "edit", label: "Rotate page 4", state, detail: null }) as ChatEvent;
    const start = () => {
      useAi.getState().setSettings({ consent: true });
      useAi.getState().ask(1, { kind: "ask", question: "q" });
    };
    const bot = () => useAi.getState().chats[1].messages[1];

    it("keeps one entry per call and follows it through its states", () => {
      start();
      emit(act("waiting"));
      emit(act("running"));
      emit({ ...act("done", "c2"), tool: "search", class: "read", label: 'Searched "Ciri"' } as ChatEvent);
      emit(act("done"));
      expect(bot().activity.map((a) => [a.id, a.state])).toEqual([
        ["c1", "done"],
        ["c2", "done"],
      ]);
    });

    it("Apply and Reject answer at once and tell the backend", () => {
      start();
      emit(act("waiting"));
      useAi.getState().decide(1, "c1", true);
      expect(bot().activity[0].state).toBe("running");
      expect(ai.aiToolDecision).toHaveBeenCalledWith(1, "c1", true);
      emit(act("waiting", "c3"));
      useAi.getState().decide(1, "c3", false);
      expect(bot().activity[1].state).toBe("rejected");
      expect(ai.aiToolDecision).toHaveBeenLastCalledWith(1, "c3", false);
    });

    it("an unanswered card does not outlive the turn", () => {
      start();
      emit(act("waiting"));
      emit({ kind: "cancelled" });
      expect(bot().activity[0].state).toBe("rejected");
    });

    it("moves the view when the assistant goes to a page or shows a source", () => {
      start();
      emit({ kind: "ui", action: { type: "goTo", page: 8 } });
      expect(goTo).toHaveBeenLastCalledWith(1, 8, undefined);
      expect(useAi.getState().mark).toBeNull();
      emit({ kind: "ui", action: { type: "show", page: 3, rects: [[10, 300, 50, 320]], quote: "q" } });
      expect(goTo).toHaveBeenLastCalledWith(1, 3, 210);
      expect(useAi.getState().mark).toMatchObject({ doc: 1, page: 3, n: 0 });
    });
  });

  describe("providers", () => {
    const CLAUDE_MODELS = [
      { id: "default", name: "Default", efforts: ["low", "medium", "high"], defaultEffort: null, isDefault: true },
      { id: "haiku", name: "Haiku", efforts: [], defaultEffort: null, isDefault: false },
    ];
    const claudeStatus = { installed: true, loggedIn: true, account: null, plan: "max", models: CLAUDE_MODELS, error: null };

    it("asks Codex by default and sends the provider with the question", () => {
      useAi.getState().setSettings({ consent: true });
      useAi.getState().ask(1, { kind: "ask", question: "q" });
      expect(vi.mocked(ai.aiAsk).mock.calls[0][2]).toBe("codex");
    });

    it("keeps one choice per document and remembers the last one for the next", async () => {
      useStore.setState({
        docs: { 1: { id: 1, name: "a.pdf", currentPage: 0 } as never, 2: { id: 2, name: "b.pdf", currentPage: 0 } as never },
        order: [1, 2],
      });
      useAi.getState().setProvider(1, "claude");
      expect(useAi.getState().chats[1].provider).toBe("claude");
      expect(useAi.getState().settings.provider).toBe("claude");
      expect(ai.saveSettings).toHaveBeenCalledWith(expect.objectContaining({ provider: "claude" }));
      useAi.getState().setProvider(2, "codex");
      expect(providerOf(useAi.getState(), 1)).toBe("claude");
      expect(providerOf(useAi.getState(), 2)).toBe("codex");
      await new Promise((r) => setTimeout(r, 0));
      expect(ai.aiClose).toHaveBeenCalledWith(1);
      expect(ai.aiPrepare).toHaveBeenCalledWith(1, "claude");
    });

    it("switching starts a new chat and is refused while an answer streams", () => {
      useAi.getState().setSettings({ consent: true });
      useAi.getState().ask(1, { kind: "ask", question: "q" });
      useAi.getState().setProvider(1, "claude"); // streaming: ignored
      expect(providerOf(useAi.getState(), 1)).toBe("codex");
      emit({ kind: "cancelled" });
      useAi.getState().setProvider(1, "claude");
      expect(useAi.getState().chats[1].messages).toEqual([]);
    });

    // switching renews the model thread first: a question is sent once that is done
    const settled = () => new Promise((r) => setTimeout(r, 0));

    it("asks for consent for each provider separately", async () => {
      useAi.getState().setSettings({ consent: true });
      useAi.getState().setProvider(1, "claude");
      useAi.getState().ask(1, { kind: "ask", question: "q" });
      expect(ai.aiAsk).not.toHaveBeenCalled();
      expect(useAi.getState().pending?.doc).toBe(1);
      useAi.getState().grantConsent();
      expect(useAi.getState().settings.claudeConsent).toBe(true);
      await settled();
      expect(vi.mocked(ai.aiAsk).mock.calls[0][2]).toBe("claude");
    });

    it("sends Claude only a model and effort its own list offers", async () => {
      useAi.getState().setSettings({ consent: true, claudeConsent: true, model: "gpt-x", effort: "xhigh", claudeModel: "haiku", claudeEffort: "high" });
      useAi.setState({ statuses: { claude: claudeStatus } });
      useAi.getState().setProvider(1, "claude");
      useAi.getState().ask(1, { kind: "ask", question: "q" });
      await settled();
      // haiku has no efforts: the effort is dropped, never the Codex one
      expect(vi.mocked(ai.aiAsk).mock.calls[0][1]).toMatchObject({ model: "haiku", effort: null });
      emit({ kind: "cancelled" });
      useAi.getState().setSettings({ claudeModel: "gpt-x" }); // an id from another provider
      useAi.getState().ask(1, { kind: "ask", question: "q2" });
      await settled();
      expect(vi.mocked(ai.aiAsk).mock.calls[1][1]).toMatchObject({ model: null, effort: null });
    });

    it("keeps a status per provider and looks again for the one asked", async () => {
      useAi.getState().refreshStatus("codex");
      useAi.getState().refreshStatus("claude");
      await new Promise((r) => setTimeout(r, 0));
      expect(ai.aiStatus).toHaveBeenCalledWith("codex");
      expect(ai.aiStatus).toHaveBeenCalledWith("claude");
      expect(Object.keys(useAi.getState().statuses).sort()).toEqual(["claude", "codex"]);
    });

    it("a picked claude is kept by Rust; the store only shows it and looks again", async () => {
      // Rust answers the re-check with the file it kept
      vi.mocked(ai.aiClaudePath).mockResolvedValueOnce("C:\\tools\\claude.exe");
      const err = await useAi.getState().pickClaudePath();
      await new Promise((r) => setTimeout(r, 0));
      expect(err).toBeNull();
      expect(ai.aiPickClaudePath).toHaveBeenCalledWith();
      expect(useAi.getState().claudePath).toBe("C:\\tools\\claude.exe");
      expect(ai.aiStatus).toHaveBeenCalledWith("claude");
      // nothing about the path goes to localStorage
      expect(JSON.stringify(vi.mocked(ai.saveSettings).mock.calls)).not.toContain("claude.exe");
    });

    it("a refused file comes back as the reason, and nothing is chosen", async () => {
      vi.mocked(ai.aiPickClaudePath).mockRejectedValueOnce("That file is not Claude Code.");
      const err = await useAi.getState().pickClaudePath();
      expect(err).toContain("not Claude Code");
      expect(useAi.getState().claudePath).toBeNull();
    });

    it("forgetting the file asks Rust to clear it and checks again", async () => {
      useAi.setState({ claudePath: "C:\\tools\\claude.exe" });
      useAi.getState().clearClaudePath();
      await new Promise((r) => setTimeout(r, 0));
      expect(ai.aiClearClaudePath).toHaveBeenCalledTimes(1);
      expect(useAi.getState().claudePath).toBeNull();
      expect(ai.aiStatus).toHaveBeenCalledWith("claude");
    });

    it("a reset event empties the lead-in text before a tool call", () => {
      useAi.getState().setSettings({ consent: true });
      useAi.getState().ask(1, { kind: "ask", question: "q" });
      emit({ kind: "delta", text: "Let me look. " });
      emit({ kind: "reset" });
      emit({ kind: "delta", text: "Answer" });
      expect(useAi.getState().chats[1].messages[1].text).toBe("Answer");
    });
  });

  it("drops the chat when its tab closes", () => {
    useAi.setState({ chats: { 1: { messages: [], prep: null } } });
    useStore.setState({ docs: {}, order: [], active: null });
    expect(useAi.getState().chats[1]).toBeUndefined();
    expect(ai.aiClose).toHaveBeenCalledWith(1);
  });
});
