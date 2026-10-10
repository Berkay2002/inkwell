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
  aiAsk: vi.fn((_doc: number, _req: unknown, cb: (e: ChatEvent) => void) => {
    emit = cb;
    return Promise.resolve();
  }),
  aiStatus: vi.fn(() => Promise.resolve({ installed: true, loggedIn: true, models: [] })),
  aiPrepare: vi.fn(() => Promise.resolve({ pages: 1, chars: 500, enough: true })),
  aiCancel: vi.fn(() => Promise.resolve()),
  aiClose: vi.fn(() => Promise.resolve()),
  loadSettings: () => ({ model: null, effort: "low", consent: false, hideSources: false, width: 400, mode: "docked", float: null }),
  saveSettings: vi.fn(),
  MODES: ["docked", "floating", "bar"],
  MODE_NAMES: { docked: "Docked", floating: "Floating", bar: "Command bar" },
}));

import * as ai from "./ai";
import { useAi } from "./aiStore";
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
  useAi.setState({ chats: {}, pending: null, mark: null, settings: { ...useAi.getState().settings, consent: false } });
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

  it("drops the chat when its tab closes", () => {
    useAi.setState({ chats: { 1: { messages: [], prep: null } } });
    useStore.setState({ docs: {}, order: [], active: null });
    expect(useAi.getState().chats[1]).toBeUndefined();
    expect(ai.aiClose).toHaveBeenCalledWith(1);
  });
});
