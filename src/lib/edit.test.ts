import { beforeEach, describe, expect, it, vi } from "vitest";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});

vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ message: vi.fn(), open: vi.fn(), save: vi.fn() }));
vi.mock("./tiles", () => ({ dropCached: vi.fn() }));
vi.mock("../recents", () => ({ useRecents: { getState: () => ({ record: vi.fn() }) } }));
vi.mock("./api", () => ({
  search: vi.fn(() => Promise.resolve(7)),
  cancelSearch: vi.fn(() => Promise.resolve()),
  openDocument: vi.fn(),
  closeDocument: vi.fn(() => Promise.resolve()),
  pageSizes: vi.fn(() => Promise.resolve()),
}));

import * as api from "./api";
import { applyState } from "./edit";
import { useStore } from "../store";

const doc = (id: number, path: string, extra = {}) =>
  ({ id, path, name: path, pageCount: 3, sizes: [], rev: 0, currentPage: 2, search: { query: "", id: null, hits: [], current: -1, done: true }, ...extra }) as never;
const state = (rev: number) => ({ rev, pageCount: 2, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Delete p. 1", redo: null }) as never;

beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ docs: {}, order: [], active: null });
});

describe("applyState", () => {
  it("runs the search again when the generation changes, not otherwise", () => {
    useStore.setState({ docs: { 1: doc(1, "C:\\a.pdf", { search: { query: "rent", id: 3, hits: [{ page: 2, rect: [0, 0, 1, 1] }], current: 0, done: true } }) } });
    applyState(1, state(0));
    expect(api.search).not.toHaveBeenCalled();
    applyState(1, state(1));
    expect(api.search).toHaveBeenCalledWith(1, "rent", expect.any(Function));
    const d = useStore.getState().docs[1];
    expect(d.search.hits).toEqual([]);
    expect([d.rev, d.pageCount, d.currentPage, d.dirty]).toEqual([1, 2, 1, true]);
  });
});

describe("openPath", () => {
  it("activates the open tab for another spelling of the same file", async () => {
    useStore.setState({ docs: { 1: doc(1, "C:\\Docs\\a.pdf"), 2: doc(2, "C:\\b.pdf") }, order: [1, 2], active: 2 });
    await useStore.getState().openPath("c:/docs/A.PDF");
    expect(api.openDocument).not.toHaveBeenCalled();
    expect(useStore.getState().active).toBe(1);
  });

  it("closes the engine's second copy when the canonical path is already open", async () => {
    useStore.setState({ docs: { 1: doc(1, "C:\\Docs\\a.pdf") }, order: [1], active: null });
    vi.mocked(api.openDocument).mockResolvedValue({ status: "opened", doc: 9, path: "C:\\Docs\\a.pdf" } as never);
    await useStore.getState().openPath("D:\\link-to-a.pdf");
    expect(api.closeDocument).toHaveBeenCalledWith(9);
    expect(useStore.getState().order).toEqual([1]);
    expect(useStore.getState().active).toBe(1);
  });
});
