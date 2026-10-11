import { beforeEach, describe, expect, it, vi } from "vitest";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
    clear: () => m.clear(),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./edit", () => ({ save: vi.fn(), undo: vi.fn(), applyState: vi.fn() }));
vi.mock("./api", () => ({
  printPrinters: vi.fn(),
  printCapabilities: vi.fn(),
  printPlan: vi.fn(),
  printPreview: vi.fn(),
  printStart: vi.fn(),
  pickSave: vi.fn(),
  toolCancel: vi.fn(() => Promise.resolve()),
}));

import { useStore } from "../store";
import * as api from "./api";
import type { PaperInfo, PrinterCaps } from "./api";
import { defaultOptions, loadRemembered, outcomeLine, pickPaper, toSettings, usePrint } from "./printStore";

const flush = () => new Promise((r) => setTimeout(r, 0));
/** The sheet has planned and drawn its first preview. */
const ready = () => vi.waitFor(() => expect(usePrint.getState().sheet?.previewUrl).toBeTruthy());
const A4: PaperInfo = { id: 9, name: "A4", width: 595.28, height: 841.89 };
const LETTER: PaperInfo = { id: 1, name: "Letter", width: 612, height: 792 };
const caps = (over: Partial<PrinterCaps> = {}): PrinterCaps => ({
  papers: [LETTER, A4],
  trays: [],
  duplex: true,
  color: true,
  collate: true,
  maxCopies: 99,
  ...over,
});
const plan = { pages: 12, sheets: 3, papers: 6, faces: [{ width: 595, height: 842, pages: [1, 2] }], editRev: 0 };
const png = new Uint8Array([137, 80, 78, 71]).buffer;

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  usePrint.setState({ sheet: null });
  URL.createObjectURL = vi.fn(() => "blob:x");
  URL.revokeObjectURL = vi.fn();
  useStore.setState({
    docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty: false, currentPage: 2, pageCount: 12, sizes: [[595, 842]] } as never },
    order: [1],
    active: 1,
  });
});

describe("toSettings", () => {
  it("maps the flat options onto the engine's tagged shapes", () => {
    const o = { ...defaultOptions(), printer: "P", paperId: 1, scope: "range" as const, range: "2-4", mode: "multiple" as const, perSheet: 4, copies: 3 };
    const s = toSettings(o, caps(), 5);
    expect(s.paper).toEqual({ id: 1, width: 612, height: 792 });
    expect(s.range).toEqual({ kind: "pages", text: "2-4" });
    expect(s.layout).toEqual({ kind: "multiple", perSheet: 4, order: "horizontal", border: false, autoRotate: true });
    expect(s.copies).toBe(3);
  });

  it("current page goes out 0-based as the viewer has it", () => {
    const s = toSettings({ ...defaultOptions(), scope: "current" }, caps(), 7);
    expect(s.range).toEqual({ kind: "current", page: 7 });
  });

  it("booklet and size layouts", () => {
    expect(toSettings({ ...defaultOptions(), mode: "booklet", sides: "frontOnly", binding: "right" }, caps(), 0).layout).toEqual({
      kind: "booklet",
      sides: "frontOnly",
      binding: "right",
    });
    expect(toSettings({ ...defaultOptions(), sizeMode: "custom", percent: 75 }, caps(), 0).layout).toEqual({ kind: "size", mode: "custom", percent: 75 });
  });

  it("drops two-sided when the printer cannot, and clamps copies to what it takes", () => {
    const s = toSettings({ ...defaultOptions(), duplex: "longEdge", copies: 500 }, caps({ duplex: false, maxCopies: 20 }), 0);
    expect(s.duplex).toBe("off");
    expect(s.copies).toBe(20);
  });

  it("falls back to A4 when the printer's capabilities are unknown", () => {
    const s = toSettings(defaultOptions(), null, 0);
    expect(s.paper.id).toBeNull();
    expect(s.paper.width).toBeCloseTo(595.28);
  });
});

describe("outcomeLine", () => {
  const a4caps = caps({ papers: [{ ...A4, name: "A4 (210 x 297 mm)" }] });
  const o = { ...defaultOptions(), printer: "P", paperId: 9 };
  it("says sheets of paper, sides and copies in words", () => {
    expect(outcomeLine({ ...plan, papers: 6 }, { ...o, duplex: "longEdge", copies: 2 }, a4caps)).toBe("6 sheets of A4, both sides, 2 copies");
  });
  it("is singular and quiet when there is nothing to add", () => {
    expect(outcomeLine({ ...plan, papers: 1 }, o, a4caps)).toBe("1 sheet of A4");
  });
  it("leaves out both sides when the printer cannot, and names grayscale", () => {
    expect(outcomeLine(plan, { ...o, duplex: "longEdge", grayscale: true }, { ...a4caps, duplex: false })).toBe("6 sheets of A4, grayscale");
  });
  it("is empty until there is a plan", () => {
    expect(outcomeLine(null, o, a4caps)).toBe("");
  });
});

describe("pickPaper", () => {
  it("takes the paper matching the first page, either orientation", () => {
    expect(pickPaper([A4, LETTER], { w: 612, h: 792 })?.id).toBe(1);
    expect(pickPaper([A4, LETTER], { w: 792, h: 612 })?.id).toBe(1);
  });
  it("else A4, else the first", () => {
    expect(pickPaper([LETTER, A4], { w: 300, h: 300 })?.id).toBe(9);
    expect(pickPaper([LETTER], null)?.id).toBe(1);
    expect(pickPaper([], null)).toBeNull();
  });
});

describe("remembered options", () => {
  it("round trips the sticky choices and not the page range or copies", async () => {
    vi.mocked(api.printPrinters).mockResolvedValue({ printers: [{ name: "P", isDefault: true, toFile: false }], fileDirSet: false });
    vi.mocked(api.printCapabilities).mockResolvedValue(caps());
    vi.mocked(api.printPlan).mockResolvedValue(plan);
    vi.mocked(api.printPreview).mockResolvedValue(png);
    usePrint.getState().open();
    await flush();
    usePrint.getState().set({ grayscale: true, content: "document", range: "1-2", copies: 4 });
    expect(loadRemembered()).toMatchObject({ grayscale: true, content: "document" });
    expect(loadRemembered()).not.toHaveProperty("range");
    expect(loadRemembered()).not.toHaveProperty("copies");
    usePrint.getState().close();
  });

  it("ignores a stored value of the wrong type", () => {
    localStorage.setItem("inkwell.print", JSON.stringify({ grayscale: "yes", duplex: "longEdge" }));
    expect(loadRemembered()).toEqual({ duplex: "longEdge" });
  });
});

describe("sheet", () => {
  it("opens for the active document, picks the default printer and asks for a plan and a preview", async () => {
    vi.mocked(api.printPrinters).mockResolvedValue({
      printers: [
        { name: "Other", isDefault: false, toFile: false },
        { name: "Microsoft Print to PDF", isDefault: true, toFile: true },
      ],
      fileDirSet: false,
    });
    vi.mocked(api.printCapabilities).mockResolvedValue(caps());
    vi.mocked(api.printPlan).mockResolvedValue(plan);
    vi.mocked(api.printPreview).mockResolvedValue(png);
    usePrint.getState().open();
    expect(usePrint.getState().sheet).toMatchObject({ doc: 1, name: "a.pdf", pageCount: 12, currentPage: 2, phase: "options" });
    await ready();
    const s = usePrint.getState().sheet!;
    expect(s.options.printer).toBe("Microsoft Print to PDF");
    expect(s.options.paperId).toBe(9);
    expect(s.plan).toEqual(plan);
    expect(s.previewUrl).toBe("blob:x");
    expect(api.printPreview).toHaveBeenCalledWith(1, expect.objectContaining({ printer: "Microsoft Print to PDF" }), 0, expect.any(Number));
    usePrint.getState().close();
    expect(usePrint.getState().sheet).toBeNull();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:x");
  });

  it("shows the engine's message when the options make no plan, and keeps Print off", async () => {
    vi.mocked(api.printPrinters).mockResolvedValue({ printers: [{ name: "P", isDefault: true, toFile: false }], fileDirSet: false });
    vi.mocked(api.printCapabilities).mockResolvedValue(caps());
    vi.mocked(api.printPlan).mockRejectedValue("no pages in that range");
    usePrint.getState().open();
    await vi.waitFor(() => expect(usePrint.getState().sheet?.planError).toBeTruthy());
    const s = usePrint.getState().sheet!;
    expect(s.plan).toBeNull();
    expect(s.planError).toBeTruthy();
    expect(s.previewUrl).toBeNull();
    await usePrint.getState().start();
    expect(api.printStart).not.toHaveBeenCalled();
    usePrint.getState().close();
  });

  it("asks where a print-to-file printer should write, then prints with that token", async () => {
    vi.mocked(api.printPrinters).mockResolvedValue({
      printers: [{ name: "Microsoft Print to PDF", isDefault: true, toFile: true }],
      fileDirSet: false,
    });
    vi.mocked(api.printCapabilities).mockResolvedValue(caps());
    vi.mocked(api.printPlan).mockResolvedValue(plan);
    vi.mocked(api.printPreview).mockResolvedValue(png);
    vi.mocked(api.pickSave).mockResolvedValue({ token: "tok-out", path: "C:\\o\\a.pdf" });
    vi.mocked(api.printStart).mockImplementation(async (_j, _d, _s, _t, onProgress) => {
      onProgress({ done: 1, total: 6 });
      return { printer: "Microsoft Print to PDF", pages: 12, sides: 6 };
    });
    usePrint.getState().open();
    await ready();
    await usePrint.getState().start();
    expect(api.pickSave).toHaveBeenCalledWith("a.pdf", { doc: 1 }, "pdf");
    expect(api.printStart).toHaveBeenCalledWith(expect.any(Number), 1, expect.anything(), "tok-out", expect.any(Function));
    expect(usePrint.getState().sheet).toMatchObject({ phase: "done", savedAs: "C:\\o\\a.pdf", report: { sides: 6 } });
    usePrint.getState().close();
  });

  it("goes back to the options when the job is cancelled, and to an error card on failure", async () => {
    vi.mocked(api.printPrinters).mockResolvedValue({ printers: [{ name: "P", isDefault: true, toFile: false }], fileDirSet: false });
    vi.mocked(api.printCapabilities).mockResolvedValue(caps());
    vi.mocked(api.printPlan).mockResolvedValue(plan);
    vi.mocked(api.printPreview).mockResolvedValue(png);
    usePrint.getState().open();
    await ready();
    vi.mocked(api.printStart).mockRejectedValueOnce("cancelled");
    await usePrint.getState().start();
    expect(usePrint.getState().sheet?.phase).toBe("options");
    vi.mocked(api.printStart).mockRejectedValueOnce("no-printer");
    await usePrint.getState().start();
    expect(usePrint.getState().sheet?.phase).toBe("error");
    usePrint.getState().back();
    expect(usePrint.getState().sheet?.phase).toBe("options");
    usePrint.getState().close();
  });
});
