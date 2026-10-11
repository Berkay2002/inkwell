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

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn(), ask: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./api", () => ({
  portfolioInfo: vi.fn(),
  portfolioExtractAll: vi.fn(),
  portfolioCreate: vi.fn(),
  pickPortfolioFiles: vi.fn(),
  pickFolder: vi.fn(),
  pickSave: vi.fn(),
  attachmentOpen: vi.fn(),
  attachmentSave: vi.fn(),
}));

import { ask } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import * as api from "./api";
import type { PortfolioFile, PortfolioInfo } from "./api";
import { nextSort, openAction, rows, sortFiles, typeLabel } from "./portfolioLogic";
import { activate, createPortfolio, extractAll, usePortfolio } from "./portfolioStore";

const file = (name: string, o: Partial<PortfolioFile> = {}): PortfolioFile => ({
  id: `n:${name}`,
  name,
  description: "",
  size: 10,
  mime: null,
  isPdf: name.endsWith(".pdf"),
  hasData: true,
  folder: null,
  cells: [name, ""],
  ranks: [0, null],
  ...o,
});

/** Name, Pages; the author's order is Pages descending. */
const info = (o: Partial<PortfolioInfo> = {}): PortfolioInfo => ({
  fields: [
    { key: "Name", name: "Name", subtype: "F", kind: "text", visible: true },
    { key: "Pages", name: "Pages", subtype: "N", kind: "number", visible: true },
  ],
  sort: [{ field: 1, ascending: false }],
  view: "details",
  initial: null,
  folders: [
    { id: 1, name: "Reports", parent: null },
    { id: 2, name: "2026", parent: 1 },
  ],
  files: [
    file("c.pdf", { ranks: [2, 2], folder: 2 }),
    file("a.pdf", { ranks: [0, 1], folder: 1 }),
    file("b.txt", { ranks: [1, 0] }),
    file("z.pdf", { ranks: [3, null] }),
  ],
  ...o,
});

const DOC = 7;
beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ docs: { [DOC]: { id: DOC } as never }, error: null } as never);
  usePortfolio.setState({ docs: {} });
});

describe("portfolio order", () => {
  it("a click on a header sorts by it, a second click flips it", () => {
    expect(nextSort([{ field: 1, ascending: false }], 0)).toEqual([{ field: 0, ascending: true }]);
    expect(nextSort([{ field: 0, ascending: true }], 0)).toEqual([{ field: 0, ascending: false }]);
    expect(nextSort([], 1)).toEqual([{ field: 1, ascending: true }]);
  });

  it("sorts by the engine's ranks, a missing value last whichever way, ties in the engine's order", () => {
    const f = info().files;
    expect(sortFiles(f, [{ field: 1, ascending: true }]).map((x) => x.name)).toEqual(["b.txt", "a.pdf", "c.pdf", "z.pdf"]);
    expect(sortFiles(f, [{ field: 1, ascending: false }]).map((x) => x.name)).toEqual(["c.pdf", "a.pdf", "b.txt", "z.pdf"]);
    const tied = [file("x", { ranks: [1, 1] }), file("y", { ranks: [0, 1] })];
    expect(sortFiles(tied, [{ field: 1, ascending: true }]).map((x) => x.name)).toEqual(["x", "y"]);
    expect(sortFiles(f, [])).toBe(f);
  });

  it("the store keeps the reader's sort apart from the collection's", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info());
    await usePortfolio.getState().load(DOC);
    const s = () => usePortfolio.getState().docs[DOC];
    expect(s().sort).toBeNull();
    usePortfolio.getState().toggleSort(DOC, 0);
    expect(s().sort).toEqual([{ field: 0, ascending: true }]);
    usePortfolio.getState().toggleSort(DOC, 0);
    expect(s().sort).toEqual([{ field: 0, ascending: false }]);
    usePortfolio.getState().resetSort(DOC);
    expect(s().sort).toBeNull();
    // the first click on the collection's own column flips that one
    usePortfolio.getState().toggleSort(DOC, 1);
    expect(s().sort).toEqual([{ field: 1, ascending: true }]);
  });
});

describe("folders", () => {
  it("expand and collapse show the contents under the folder", () => {
    const i = info();
    const names = (open: number[]) => rows(i, i.sort, new Set(open)).map((r) => (r.kind === "folder" ? `[${r.folder.name}]` : r.file.name));
    expect(names([])).toEqual(["[Reports]", "b.txt", "z.pdf"]);
    expect(names([1])).toEqual(["[Reports]", "[2026]", "a.pdf", "b.txt", "z.pdf"]);
    expect(names([1, 2])).toEqual(["[Reports]", "[2026]", "c.pdf", "a.pdf", "b.txt", "z.pdf"]);
    const r = rows(i, i.sort, new Set([1]));
    expect(r[0]).toMatchObject({ kind: "folder", open: true, count: 2, depth: 0 });
    expect(r[1]).toMatchObject({ kind: "folder", open: false, count: 1, depth: 1 });
  });

  it("the store toggles a folder and keeps it across a reload", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info());
    await usePortfolio.getState().load(DOC);
    usePortfolio.getState().toggleFolder(DOC, 1);
    expect(usePortfolio.getState().docs[DOC].open).toEqual([1]);
    usePortfolio.getState().select(DOC, "n:a.pdf");
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC]).toMatchObject({ open: [1], selected: "n:a.pdf" });
    usePortfolio.getState().toggleFolder(DOC, 1);
    expect(usePortfolio.getState().docs[DOC].open).toEqual([]);
  });
});

describe("open or extract", () => {
  it("a PDF opens, everything else is extracted", () => {
    expect(openAction(file("a.pdf"))).toBe("open");
    expect(openAction(file("a.exe"))).toBe("extract");
    expect(openAction(file("a.pdf", { hasData: false }))).toBe("extract");
    expect(typeLabel(file("run.EXE", { mime: "application/octet-stream" }))).toBe("EXE");
    expect(typeLabel(file("note", { mime: "text/plain" }))).toBe("text/plain");
    expect(typeLabel(file("a.csv", { mime: "text/csv" }))).toBe("CSV (text/csv)");
  });

  it("a PDF goes to a tab; another type asks first, naming it, and is saved only to a chosen file", async () => {
    const openPath = vi.fn(() => Promise.resolve());
    useStore.setState({ openPath } as never);
    vi.mocked(api.attachmentOpen).mockResolvedValue({ token: "t", path: "C:\\tmp\\a.pdf" });
    await activate(DOC, file("a.pdf"));
    expect(api.attachmentOpen).toHaveBeenCalledWith(DOC, "n:a.pdf");
    expect(openPath).toHaveBeenCalledWith("C:\\tmp\\a.pdf", "t", true);

    vi.mocked(ask).mockResolvedValue(false);
    await activate(DOC, file("setup.exe", { mime: "application/octet-stream" }));
    expect(vi.mocked(ask).mock.calls[0][0]).toContain("setup.exe");
    expect(vi.mocked(ask).mock.calls[0][0]).toContain("Type: EXE");
    expect(api.attachmentOpen).toHaveBeenCalledTimes(1);
    expect(api.pickSave).not.toHaveBeenCalled();

    vi.mocked(ask).mockResolvedValue(true);
    vi.mocked(api.pickSave).mockResolvedValue({ token: "s", path: "C:\\out\\setup.exe" });
    vi.mocked(api.attachmentSave).mockResolvedValue("C:\\out\\setup.exe");
    await activate(DOC, file("setup.exe"));
    expect(api.attachmentSave).toHaveBeenCalledWith(DOC, "n:setup.exe", "s");
    expect(api.attachmentOpen).toHaveBeenCalledTimes(1);
    expect(openPath).toHaveBeenCalledTimes(1);
  });

  it("Extract all asks for a folder and says what was written", async () => {
    vi.mocked(api.pickFolder).mockResolvedValue({ token: "f", path: "C:\\out" });
    vi.mocked(api.portfolioExtractAll).mockResolvedValue({ folder: "C:\\out\\files", written: ["a.pdf", "b (2).txt"], skipped: [] });
    await extractAll(DOC);
    expect(api.portfolioExtractAll).toHaveBeenCalledWith(DOC, "f");
    expect(useStore.getState().error).toBe("Extracted 2 files to files");
    vi.mocked(api.pickFolder).mockResolvedValue(null);
    vi.mocked(api.portfolioExtractAll).mockClear();
    await extractAll(DOC);
    expect(api.portfolioExtractAll).not.toHaveBeenCalled();
  });
});

describe("loading and creating", () => {
  it("a plain PDF has no portfolio; a refused collection is said once and stays plain", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(null);
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].info).toBeNull();
    expect(useStore.getState().error).toBeNull();

    usePortfolio.setState({ docs: {} });
    vi.mocked(api.portfolioInfo).mockRejectedValue("the portfolio's folders loop back on themselves");
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].info).toBeNull();
    expect(useStore.getState().error).toContain("loop");
    useStore.setState({ error: null } as never);
    await usePortfolio.getState().load(DOC);
    expect(useStore.getState().error).toBeNull();
  });

  it("the file list shows first, unless the author asked for no portfolio view", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info());
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].cover).toBe(false);
    usePortfolio.getState().showCover(DOC, true);
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].cover).toBe(true);
    usePortfolio.setState({ docs: {} });
    vi.mocked(api.portfolioInfo).mockResolvedValue(info({ view: "hidden" }));
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].cover).toBe(true);
  });

  it("Create PDF Portfolio picks files, then where to save, then opens the result", async () => {
    const openPath = vi.fn(() => Promise.resolve());
    useStore.setState({ openPath } as never);
    vi.mocked(api.pickPortfolioFiles).mockResolvedValue([
      { token: "a", path: "C:\\in\\a.pdf" },
      { token: "b", path: "C:\\in\\b.docx" },
    ]);
    vi.mocked(api.pickSave).mockResolvedValue({ token: "out", path: "C:\\in\\Portfolio.pdf" });
    vi.mocked(api.portfolioCreate).mockResolvedValue({ token: "new", path: "C:\\in\\Portfolio.pdf" });
    await createPortfolio();
    expect(api.pickSave).toHaveBeenCalledWith("Portfolio.pdf", { near: "a" });
    expect(api.portfolioCreate).toHaveBeenCalledWith(["a", "b"], "out");
    expect(openPath).toHaveBeenCalledWith("C:\\in\\Portfolio.pdf", "new");

    vi.mocked(api.pickPortfolioFiles).mockResolvedValue([]);
    vi.mocked(api.pickSave).mockClear();
    await createPortfolio();
    expect(api.pickSave).not.toHaveBeenCalled();
  });
});
