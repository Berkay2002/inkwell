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

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./edit", () => ({ save: vi.fn(), undo: vi.fn(), applyState: vi.fn() }));
vi.mock("./api", () => ({
  pickSave: vi.fn(),
  pickFolder: vi.fn(),
  pickSource: vi.fn(),
  exportStart: vi.fn(),
  createStart: vi.fn(),
  toolCancel: vi.fn(() => Promise.resolve()),
  discardScratch: vi.fn(() => Promise.resolve(true)),
  closeDocument: vi.fn(() => Promise.resolve()),
}));

import { useStore } from "../store";
import * as api from "./api";
import { isFolderExport, pagesOf, readiness, useConvert } from "./convertStore";
import { useTools } from "./toolsStore";

const flush = () => new Promise((r) => setTimeout(r, 0));
const ref = (n: string) => ({ token: `tok-${n}`, path: `C:\\d\\${n}` });
const out = { files: ["C:\\o\\a-1.png"], folder: "C:\\o", bytes: 2048, pages: 1, warnings: [] };

beforeEach(() => {
  vi.clearAllMocks();
  useConvert.setState({ sheet: null });
  useTools.setState({ sheet: null });
  useStore.setState({
    docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty: false, currentPage: 2, pageCount: 12 } as never },
    order: [1],
    active: 1,
  });
});

describe("export sheet", () => {
  it("opens for the active document and reads the options into one request", async () => {
    useConvert.getState().openExport();
    expect(useConvert.getState().sheet).toMatchObject({ mode: "export", doc: 1, what: "images", dpi: 150 });
    useConvert.getState().set({ imageFormat: "jpeg", dpi: 300, gray: true, scope: "range", range: "2-4" });
    vi.mocked(api.pickFolder).mockResolvedValue(ref("out"));
    vi.mocked(api.exportStart).mockImplementation(async (_j, _r, onProgress) => {
      onProgress({ done: 1, total: 3, fraction: 0.3 });
      return out;
    });
    await useConvert.getState().start();
    const [, req] = vi.mocked(api.exportStart).mock.calls[0];
    expect(req).toEqual({
      doc: 1,
      kind: { kind: "images", format: "jpeg", dpi: 300, gray: true, quality: 85 },
      pages: "2-4",
      saveTo: "tok-out",
    });
    expect(useConvert.getState().sheet?.phase).toBe("done");
    expect(useConvert.getState().sheet?.exported).toEqual(out);
  });

  it("asks for a file, not a folder, for text and names it after the document", async () => {
    useConvert.getState().openExport(1, "text");
    useConvert.getState().set({ layout: true, scope: "page" });
    vi.mocked(api.pickSave).mockResolvedValue(ref("a.txt"));
    vi.mocked(api.exportStart).mockResolvedValue({ ...out, files: ["C:\\d\\a.txt"] });
    await useConvert.getState().start();
    expect(api.pickSave).toHaveBeenCalledWith("a.txt", { doc: 1 }, "text");
    expect(api.pickFolder).not.toHaveBeenCalled();
    const [, req] = vi.mocked(api.exportStart).mock.calls[0];
    expect(req).toMatchObject({ kind: { kind: "text", layout: true }, pages: "3", saveTo: "tok-a.txt" });
  });

  it("a dismissed dialog stays on the options", async () => {
    useConvert.getState().openExport();
    vi.mocked(api.pickFolder).mockResolvedValue(null);
    await useConvert.getState().start();
    expect(useConvert.getState().sheet?.phase).toBe("options");
    expect(api.exportStart).not.toHaveBeenCalled();
  });

  it("cancel returns to the options and tells the engine", async () => {
    useConvert.getState().openExport();
    vi.mocked(api.pickFolder).mockResolvedValue(ref("out"));
    let reject!: (e: unknown) => void;
    vi.mocked(api.exportStart).mockReturnValue(new Promise((_, r) => (reject = r)));
    const run = useConvert.getState().start();
    await flush();
    expect(useConvert.getState().sheet?.phase).toBe("running");
    useConvert.getState().cancel();
    expect(api.toolCancel).toHaveBeenCalled();
    reject("cancelled");
    await run;
    expect(useConvert.getState().sheet?.phase).toBe("options");
  });

  it("an engine error shows its message", async () => {
    useConvert.getState().openExport();
    vi.mocked(api.pickFolder).mockResolvedValue(ref("out"));
    vi.mocked(api.exportStart).mockRejectedValue("bad page range: 9");
    await useConvert.getState().start();
    expect(useConvert.getState().sheet).toMatchObject({ phase: "error", error: "bad page range: 9" });
  });

  it("helpers: folder exports, page scope, readiness", () => {
    expect(isFolderExport("images") && isFolderExport("extractImages") && !isFolderExport("word")).toBe(true);
    useConvert.getState().openExport();
    const s = useConvert.getState().sheet!;
    expect(pagesOf(s, 4)).toBeUndefined();
    expect(pagesOf({ ...s, scope: "page" }, 4)).toBe("5");
    expect(readiness({ ...s, scope: "range", range: "" }, 12)).toMatch(/pages/);
    expect(readiness({ ...s, scope: "range", range: "1-3" }, 12)).toBeNull();
    expect(readiness({ ...s, scope: "range", range: "20" }, 12)).not.toBeNull();
  });
});

describe("create sheet", () => {
  it("keeps images in the order the user arranges and opens the result unsaved", async () => {
    useConvert.getState().openCreate();
    vi.mocked(api.pickSource).mockResolvedValueOnce([ref("a.png"), ref("b.png"), ref("c.png")]);
    await useConvert.getState().addImages();
    useConvert.getState().moveImage(2, -1);
    useConvert.getState().moveImage(0, null);
    expect(useConvert.getState().sheet?.files.map((f) => f.token)).toEqual(["tok-c.png", "tok-b.png"]);
    expect(readiness(useConvert.getState().sheet!, 0)).toBeNull();

    useConvert.getState().set({ pageSize: "a4", margin: 36 });
    vi.mocked(api.createStart).mockResolvedValue({ file: ref("c.pdf"), pages: 2, warnings: ["x"] });
    const open = vi.fn(() => Promise.resolve());
    useStore.setState({ openPath: open } as never);
    await useConvert.getState().start();
    const [, req] = vi.mocked(api.createStart).mock.calls[0];
    expect(req).toEqual({ source: { kind: "images", files: ["tok-c.png", "tok-b.png"] }, pageSize: "a4", margin: 36 });
    expect(open).toHaveBeenCalledWith("C:\\d\\c.pdf", "tok-c.pdf", true);
    expect(useConvert.getState().sheet?.created).toEqual({ pages: 2, name: "c.pdf", warnings: ["x"] });
  });

  it("needs a source before it can start", () => {
    useConvert.getState().openCreate();
    const s = useConvert.getState().sheet!;
    expect(readiness(s, 0)).toMatch(/image/);
    expect(readiness({ ...s, from: "text" }, 0)).toMatch(/text/);
    expect(readiness({ ...s, from: "clipboard" }, 0)).toBeNull();
    expect(readiness({ ...s, from: "blank", blankPages: 0 }, 0)).not.toBeNull();
  });

  it("works from Home, with no document open", () => {
    useStore.setState({ active: null });
    useConvert.getState().openCreate("blank");
    expect(useConvert.getState().sheet).toMatchObject({ mode: "create", doc: null, from: "blank" });
    useConvert.getState().openExport();
    expect(useConvert.getState().sheet?.mode).toBe("create");
  });
});

describe("one rail at a time", () => {
  it("opening Compress closes an idle convert sheet; a running one is kept", () => {
    useConvert.getState().openCreate();
    useTools.setState({ sheet: { tool: "compress", doc: 1, phase: "options" } as never });
    expect(useConvert.getState().sheet).toBeNull();

    useConvert.getState().openCreate();
    useConvert.setState({ sheet: { ...useConvert.getState().sheet!, phase: "running" } });
    useTools.setState({ sheet: null });
    useTools.setState({ sheet: { tool: "ocr", doc: 1, phase: "options" } as never });
    expect(useConvert.getState().sheet?.phase).toBe("running");
  });

  it("opening a convert sheet closes an idle tools sheet", () => {
    useTools.setState({ sheet: { tool: "compress", doc: 1, phase: "options" } as never });
    useConvert.getState().openExport();
    expect(useTools.getState().sheet).toBeNull();
    expect(useConvert.getState().sheet?.mode).toBe("export");
  });
});
