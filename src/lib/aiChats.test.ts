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

  it("a chat set aside in mid-turn settles its cards: reopening it does not ask again for a dead thread", () => {
    ask("first");
    emit({ kind: "activity", id: "c1", tool: "rotate_page", class: "edit", label: "Rotate page 4", state: "waiting", detail: null } as ChatEvent);
    emit({ kind: "activity", id: "c2", tool: "search", class: "read", label: "Searched", state: "running", detail: null } as ChatEvent);
    useAi.getState().newChat(1);
    ask("second");
    finish();
    const id = useAi.getState().past[1][0].id;
    useAi.getState().openChat(1, id);
    const bot = useAi.getState().chats[1].messages[1];
    expect(bot.state).toBe("cancelled");
    expect(bot.activity.map((x) => [x.id, x.state])).toEqual([
      ["c1", "rejected"],
      ["c2", "failed"],
    ]);
  });

  it("a question asked right after New chat waits until the old thread is closed and prepared again", async () => {
    let release: () => void = () => {};
    vi.mocked(ai.aiClose).mockImplementationOnce(() => new Promise<void>((r) => (release = r)));
    ask("first");
    finish();
    useAi.getState().newChat(1);
    vi.mocked(ai.aiAsk).mockClear();
    ask("second");
    expect(useAi.getState().chats[1].messages[0].text).toBe("second"); // on screen at once
    await Promise.resolve();
    expect(ai.aiAsk).not.toHaveBeenCalled();
    release();
    await vi.waitFor(() => expect(ai.aiAsk).toHaveBeenCalledTimes(1));
    expect(ai.aiPrepare).toHaveBeenCalled();
  });

  it("Stop on an answer that waits for the thread reset cancels it here and it is never sent", async () => {
    let release: () => void = () => {};
    vi.mocked(ai.aiClose).mockImplementationOnce(() => new Promise<void>((r) => (release = r)));
    ask("first");
    finish();
    useAi.getState().newChat(1);
    vi.mocked(ai.aiAsk).mockClear();
    vi.mocked(ai.aiCancel).mockClear();
    ask("second");
    expect(useAi.getState().chats[1].messages[1].state).toBe("streaming");
    useAi.getState().cancel(1);
    const [, bot] = useAi.getState().chats[1].messages;
    expect(bot.state).toBe("cancelled");
    expect(ai.aiCancel).not.toHaveBeenCalled(); // nothing is running on the other side
    release();
    await new Promise((r) => setTimeout(r, 0));
    expect(ai.aiAsk).not.toHaveBeenCalled();
    // and the chat takes the next question
    ask("third");
    await vi.waitFor(() => expect(ai.aiAsk).toHaveBeenCalledTimes(1));
  });

  it("Stop on a running answer still reaches the engine", () => {
    ask("first");
    useAi.getState().cancel(1);
    expect(ai.aiCancel).toHaveBeenCalledWith(1);
  });

  it("a question that was cleared away while it waited is never asked", async () => {
    let release: () => void = () => {};
    vi.mocked(ai.aiClose).mockImplementationOnce(() => new Promise<void>((r) => (release = r)));
    useAi.getState().newChat(1);
    vi.mocked(ai.aiAsk).mockClear();
    ask("late");
    useAi.getState().clear(1);
    release();
    await new Promise((r) => setTimeout(r, 0));
    expect(ai.aiAsk).not.toHaveBeenCalled();
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
    expect(c.restored).toBe(2); // the two messages brought back; the note sits after them
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

describe("source marks of the command bar", () => {
  const cite = (n: number) => ({ n, page: 3, rects: [[0, 0, 1, 1]], quote: "q", doc: 1 }) as never;

  it("a hover lights the badge and leaving puts back what was lit", () => {
    const before = { doc: 1, page: 0, rects: [], n: 9, nonce: 1 };
    useAi.setState({ mark: before });
    useAi.getState().liftMark(1, cite(1), 7);
    expect(useAi.getState().mark).toMatchObject({ page: 3, n: 1, msg: 7 });
    expect(goTo).not.toHaveBeenCalled();
    useAi.getState().dropMark();
    expect(useAi.getState().mark).toBe(before);
  });

  it("a hover that outlives its chip is forgotten when the chat is cleared", () => {
    useAi.setState({ mark: null });
    useAi.getState().liftMark(1, cite(1), 7);
    useAi.getState().clear(1);
    expect(useAi.getState().mark).toBeNull();
    useAi.getState().liftMark(1, cite(2), 8);
    useAi.getState().dropMark();
    expect(useAi.getState().mark).toBeNull(); // not the stale "prev" of the first hover
  });

  it("a click while hovering is a jump that keeps the mark, and the next click turns it off", () => {
    const c = cite(2);
    useAi.getState().liftMark(1, c, 7);
    useAi.getState().showCitation(1, c, 7);
    expect(goTo).toHaveBeenCalled();
    expect(useAi.getState().mark).toMatchObject({ n: 2, msg: 7 });
    useAi.getState().dropMark(); // the pointer leaves
    expect(useAi.getState().mark).toMatchObject({ n: 2, msg: 7 });
    useAi.getState().liftMark(1, c, 7);
    useAi.getState().showCitation(1, c, 7);
    expect(useAi.getState().mark).toBeNull();
  });
});

describe("Edit on a question", () => {
  it("hands the words back to that document's composer", () => {
    useAi.getState().setDraft(1, "what is the cash runway?");
    expect(useAi.getState().draft).toMatchObject({ doc: 1, text: "what is the cash runway?" });
    useAi.getState().clearDraft();
    expect(useAi.getState().draft).toBeNull();
  });
});
