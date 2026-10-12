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
  knownFile: vi.fn((path: string) => Promise.resolve({ token: `known:${path}`, path })),
  pickOpen: vi.fn(),
  pickSave: vi.fn(),
  editApply: vi.fn(),
  saveDocument: vi.fn(),
  closeDocument: vi.fn(() => Promise.resolve()),
  pageSizes: vi.fn(() => Promise.resolve()),
}));

import * as api from "./api";
import * as edits from "./edit";
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
    expect(api.search).toHaveBeenCalledWith(1, "rent", expect.objectContaining({ caseSensitive: false }), expect.any(Function));
    const d = useStore.getState().docs[1];
    expect(d.search.hits).toEqual([]);
    expect([d.rev, d.pageCount, d.currentPage, d.dirty]).toEqual([1, 2, 1, true]);
  });
});

describe("applyState and Edit text", () => {
  it("tells the registered guard when the generation changes, not otherwise", () => {
    useStore.setState({ docs: { 1: doc(1, "C:\\a.pdf") } });
    const rev = vi.fn();
    edits.setLeaveGuard({ flush: () => Promise.resolve(), unsent: () => null, rev });
    applyState(1, state(0));
    expect(rev).not.toHaveBeenCalled();
    applyState(1, state(1));
    expect(rev).toHaveBeenCalledWith(1);
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

  it("asks Rust for a token unless the file came with one", async () => {
    useStore.setState({ docs: {}, order: [], active: null });
    vi.mocked(api.openDocument).mockResolvedValue({ status: "opened", doc: 3, path: "C:/x.pdf" } as never);
    await useStore.getState().openPath("C:/Docs/recent.pdf");
    expect(api.openDocument).toHaveBeenLastCalledWith("known:C:/Docs/recent.pdf");
    useStore.setState({ docs: {}, order: [], active: null });
    await useStore.getState().openPath("C:/Docs/picked.pdf", "tok");
    expect(api.openDocument).toHaveBeenLastCalledWith("tok");
  });
});

describe("file edits send tokens, never paths", () => {
  it("insert, Save as", async () => {
    useStore.setState({ docs: { 1: doc(1, "C:/Docs/a.pdf") }, order: [1], active: 1 });
    vi.mocked(api.pickOpen).mockResolvedValue([{ token: "in", path: "C:/Docs/b.pdf" }]);
    vi.mocked(api.editApply).mockResolvedValue({ rev: 1, pageCount: 2, sizes: [], path: "C:/Docs/a.pdf", dirty: true, undo: null, redo: null, redacted: false });
    await edits.insertFile(1, 0);
    expect(api.editApply).toHaveBeenCalledWith(1, { op: "insertFile", at: 0, file: "in" });

    vi.mocked(api.pickSave).mockResolvedValue({ token: "out", path: "C:/Docs/c.pdf" });
    vi.mocked(api.saveDocument).mockResolvedValue({ rev: 1, pageCount: 2, sizes: [], path: "C:/Docs/c.pdf", dirty: false, undo: null, redo: null, redacted: false });
    await edits.saveAs(1);
    expect(api.pickSave).toHaveBeenCalledWith("a.pdf", { doc: 1 });
    expect(api.saveDocument).toHaveBeenCalledWith(1, "out");
  });
});
