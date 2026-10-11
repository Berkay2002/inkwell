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
  aiToolDecision: vi.fn(() => Promise.resolve()),
  aiSetPermissions: vi.fn(() => Promise.resolve()),
  loadSettings: () => ({
    model: null,
    effort: "low",
    consent: false,
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
import { useAi } from "./aiStore";
import { useStore } from "../store";

const goTo = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ docs: { 1: { id: 1, name: "a.pdf", currentPage: 0 } as never }, order: [1], active: 1, goTo });
  useAi.setState({ chats: {}, past: {}, pending: null, mark: null, settings: { ...useAi.getState().settings, consent: true } });
});

const ask = (q: string) => useAi.getState().ask(1, { kind: "ask", question: q });
const finish = () => emit({ kind: "done", answer: "a", citations: [], followups: [], firstTokenMs: 1, totalMs: 2, prepMs: 0, chunksSent: 0, chunksTotal: 0 });

describe("a document's chats (command bar)", () => {
  it("stamps each message with the time it was asked", () => {
    ask("first");
    const [q, a] = useAi.getState().chats[1].messages;
    expect(typeof q.at).toBe("number");
    expect(a.at).toBe(q.at);
  });

  it("New chat sets the current chat aside and starts an empty one", () => {
    ask("first");
    finish();
    useAi.getState().newChat(1);
    expect(useAi.getState().chats[1].messages).toEqual([]);
    expect(useAi.getState().past[1]).toHaveLength(1);
    expect(useAi.getState().past[1][0].messages[0].text).toBe("first");
    expect(ai.aiClose).toHaveBeenCalledWith(1);
  });

  it("New chat on an empty chat lists nothing", () => {
    useAi.getState().newChat(1);
    expect(useAi.getState().past[1] ?? []).toHaveLength(0);
  });

  it("a chat set aside in mid-turn is not left streaming", () => {
    ask("first");
    useAi.getState().newChat(1);
    expect(useAi.getState().past[1][0].messages.every((m) => m.state !== "streaming")).toBe(true);
  });

  it("opening an earlier chat swaps it with the current one and marks it restored", () => {
    ask("first");
    finish();
    useAi.getState().newChat(1);
    ask("second");
    finish();
    const id = useAi.getState().past[1][0].id;
    useAi.getState().openChat(1, id);
    const c = useAi.getState().chats[1];
    expect(c.messages[0].text).toBe("first");
    expect(c.restored).toBe(true);
    expect(useAi.getState().past[1].map((p) => p.messages[0].text)).toEqual(["second"]);
  });

  it("closing the tab drops its earlier chats", () => {
    ask("first");
    finish();
    useAi.getState().newChat(1);
    useAi.getState().closeChat(1);
    expect(useAi.getState().past[1]).toBeUndefined();
  });
});
