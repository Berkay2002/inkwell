import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./api", () => ({
  pickOpen: vi.fn(),
  pickSave: vi.fn(),
  combineInspect: vi.fn(),
  combineRun: vi.fn(),
  openDocument: vi.fn(),
  closeDocument: vi.fn(() => Promise.resolve()),
  pageSizes: vi.fn(() => Promise.resolve()),
  fetchTile: vi.fn(() => Promise.resolve(null)),
}));

import { useStore } from "../store";
import * as api from "./api";
import { useCombine } from "./combineStore";

const flush = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};
const ref = (n: string) => ({ token: `tok-${n}`, path: `C:\\d\\${n}.pdf` });
const plan = { files: [], pages: 5, contentsPages: 0, bookmarks: 2, leftOut: 0, canRun: true, summary: "Combines 2 files, 5 pages, 2 bookmarks" };

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  useCombine.setState({ sheet: null });
  vi.mocked(api.combineInspect).mockResolvedValue(plan as never);
  // a file that cannot be drawn: the sheet still lists and plans it
  vi.mocked(api.openDocument).mockResolvedValue({ doc: 0, status: "needsPassword" } as never);
});

afterEach(() => vi.useRealTimers());

describe("combine sheet", () => {
  it("asks the engine what the list gives, once, a moment after the last change", async () => {
    const c = useCombine.getState();
    c.open([ref("a"), ref("b")]);
    useCombine.getState().setRange(useCombine.getState().sheet!.entries[0].key, "1-2");
    await vi.advanceTimersByTimeAsync(300);
    expect(api.combineInspect).toHaveBeenCalledTimes(1);
    const [files, options] = vi.mocked(api.combineInspect).mock.calls[0];
    expect(files).toEqual([
      { file: "tok-a", range: "1-2", title: null },
      { file: "tok-b", range: "", title: null },
    ]);
    expect(options).toEqual({ bookmarks: true, keepOutlines: true, contentsPage: false });
    expect(useCombine.getState().sheet?.plan?.summary).toBe("Combines 2 files, 5 pages, 2 bookmarks");
  });

  it("reorders and removes rows", async () => {
    useCombine.getState().open([ref("a"), ref("b"), ref("c")]);
    useCombine.getState().move(2, 0);
    expect(useCombine.getState().sheet!.entries.map((e) => e.name)).toEqual(["c.pdf", "a.pdf", "b.pdf"]);
    useCombine.getState().remove(useCombine.getState().sheet!.entries[1].key);
    expect(useCombine.getState().sheet!.entries.map((e) => e.name)).toEqual(["c.pdf", "b.pdf"]);
    await vi.advanceTimersByTimeAsync(300);
  });

  it("a page click rewrites the range text", async () => {
    useCombine.getState().open([ref("a")]);
    const e = useCombine.getState().sheet!.entries[0];
    // the file reports its page count when it is open
    useCombine.setState((s) => ({ sheet: { ...s.sheet!, entries: [{ ...e, pages: 4 }] } }));
    useCombine.getState().togglePage(e.key, 1);
    expect(useCombine.getState().sheet!.entries[0].range).toBe("1, 3-4");
    useCombine.getState().togglePage(e.key, 1);
    expect(useCombine.getState().sheet!.entries[0].range).toBe("1, 3-4, 2");
    await vi.advanceTimersByTimeAsync(300);
  });

  it("saves through a token, combines, and opens the result", async () => {
    useCombine.getState().open([ref("a"), ref("b")]);
    await vi.advanceTimersByTimeAsync(300);
    vi.mocked(api.pickSave).mockResolvedValue({ token: "save-1", path: "C:\\o\\a (combined).pdf" });
    vi.mocked(api.combineRun).mockResolvedValue({ file: { token: "open-9", path: "C:\\o\\a (combined).pdf" }, plan } as never);
    const open = vi.fn(() => Promise.resolve());
    useStore.setState({ openPath: open } as never);
    await useCombine.getState().run();
    await flush();
    expect(api.pickSave).toHaveBeenCalledWith("a (combined).pdf", { near: "tok-a" });
    expect(vi.mocked(api.combineRun).mock.calls[0][2]).toBe("save-1");
    expect(open).toHaveBeenCalledWith("C:\\o\\a (combined).pdf", "open-9");
    expect(useCombine.getState().sheet).toBeNull();
  });

  it("a file the dialog does not return is a run that does not start", async () => {
    useCombine.getState().open([ref("a")]);
    await vi.advanceTimersByTimeAsync(300);
    vi.mocked(api.pickSave).mockResolvedValue(null);
    await useCombine.getState().run();
    expect(api.combineRun).not.toHaveBeenCalled();
    expect(useCombine.getState().sheet?.phase).toBe("setup");
  });
});
