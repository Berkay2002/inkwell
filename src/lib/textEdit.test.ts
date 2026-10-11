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
vi.mock("./edit", () => ({ editText: vi.fn(), addText: vi.fn(), save: vi.fn(), undo: vi.fn(), redo: vi.fn(), applyState: vi.fn() }));
vi.mock("./api", () => ({ textBlocks: vi.fn() }));

import { useStore } from "../store";
import * as api from "./api";
import type { TextBlock } from "./api";
import { addText, editText } from "./edit";
import { caretFor, changeFor, closestBlock, cssColor, cssFamily, displayFont, fallbackNote, hexOf, isOffered, rgbOf, useTextEdit } from "./textEdit";

const block = (over: Partial<TextBlock> = {}): TextBlock => ({
  index: 0,
  text: "Rent is due on the first.",
  rect: [72, 100, 300, 114],
  lines: [[72, 100, 300, 114]],
  font: "ABCDEF+Arial-BoldMT",
  size: 12,
  bold: false,
  italic: false,
  color: [0, 0, 0],
  align: "left",
  lineSpacing: null,
  editable: true,
  issue: null,
  reason: null,
  ...over,
});

describe("helpers", () => {
  it("names fonts the way a person would", () => {
    expect(displayFont("ABCDEF+Arial-BoldMT")).toBe("Arial");
    expect(displayFont("TimesNewRomanPSMT")).toBe("Times New Roman");
    expect(displayFont("Helvetica")).toBe("Helvetica");
  });

  it("picks a CSS family for the editor", () => {
    expect(cssFamily("Courier-Bold")).toContain("Courier");
    expect(cssFamily("TimesNewRoman")).toContain("Times");
    expect(cssFamily("ArialMT")).toContain("Arial");
    expect(cssFamily("DejaVuSans")).toContain("Arial");
  });

  it("round-trips colours", () => {
    expect(hexOf([1, 0.5, 0])).toBe("#ff8000");
    expect(rgbOf("#2e4b8f")?.map((v) => Math.round(v * 255))).toEqual([0x2e, 0x4b, 0x8f]);
    expect(rgbOf("blue")).toBeNull();
    expect(cssColor([0, 0, 1])).toBe("rgb(0 0 255)");
  });

  it("does not offer hidden-layer text", () => {
    expect(isOffered(block({ issue: "hidden", editable: false }))).toBe(false);
    expect(isOffered(block({ issue: "type3", editable: false }))).toBe(true);
    expect(isOffered(block())).toBe(true);
  });

  it("finds the block an edit produced, and not an unrelated one", () => {
    const prev = block({ rect: [72, 100, 300, 114] });
    const moved = block({ index: 3, rect: [92, 130, 320, 144] });
    const far = block({ index: 4, rect: [72, 600, 300, 614] });
    expect(closestBlock(prev, [far, moved], [20, 30])?.index).toBe(3);
    expect(closestBlock(prev, [far])).toBeNull();
    expect(closestBlock(prev, [block({ index: 5, editable: false })])).toBeNull();
  });

  it("sends only what differs", () => {
    const b = block();
    expect(changeFor(b, b.text)).toBeNull();
    expect(changeFor(b, null)).toBeNull();
    expect(changeFor(b, "Rent is due on the second.")).toEqual({ text: "Rent is due on the second." });
    expect(changeFor(b, b.text, { size: 14 })).toEqual({ size: 14 });
    expect(changeFor(b, "x", { size: 14 })).toEqual({ size: 14, text: "x" });
  });

  it("says what a replaced font became", () => {
    const s = fallbackNote({ font: "Helvetica-Oblique", embedded: false, reason: "the embedded font has no glyph for “ž”" });
    expect(s).toContain("The embedded font has no glyph");
    expect(s).toContain("Helvetica Italic");
  });

  it("puts the caret by where the click landed", () => {
    const b = block({ text: "abcdefghij", rect: [0, 0, 100, 10], lines: [[0, 0, 100, 10]] });
    expect(caretFor(b, b.text, 0, 5)).toBe(0);
    expect(caretFor(b, b.text, 100, 5)).toBe(10);
    expect(caretFor(b, b.text, 50, 5)).toBe(5);
    const two = block({ text: "abcdefghij", rect: [0, 0, 100, 20], lines: [[0, 0, 100, 10], [0, 10, 100, 20]] });
    expect(caretFor(two, two.text, 0, 15)).toBe(5);
  });
});

describe("store", () => {
  const list = (blocks: TextBlock[], rev = 1) => ({ rev, blocks, areas: [] });

  beforeEach(() => {
    vi.clearAllMocks();
    useTextEdit.setState({ open: true, tool: "select", docs: {} });
    useStore.setState({
      docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty: false, currentPage: 0, pageCount: 2, rev: 1, sizes: [[595, 842]] } as never },
      order: [1],
      active: 1,
    });
  });

  it("lists a page once per document generation", async () => {
    vi.mocked(api.textBlocks).mockResolvedValue(list([block()]));
    await useTextEdit.getState().load(1, 0);
    await useTextEdit.getState().load(1, 0);
    expect(api.textBlocks).toHaveBeenCalledTimes(1);
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 2 } } });
    vi.mocked(api.textBlocks).mockResolvedValue(list([block()], 2));
    await useTextEdit.getState().load(1, 0);
    expect(api.textBlocks).toHaveBeenCalledTimes(2);
  });

  it("sends typed text as one change when the block is left", async () => {
    vi.mocked(api.textBlocks).mockResolvedValue(list([block(), block({ index: 1, text: "Other", rect: [72, 200, 200, 214] })]));
    vi.mocked(editText).mockResolvedValue({ state: {} as never, fallback: null });
    const te = useTextEdit.getState();
    await te.load(1, 0);
    await te.select(1, { page: 0, index: 0 });
    te.setDraft(1, "Rent is due on the second.");
    await te.select(1, { page: 0, index: 1 });
    expect(editText).toHaveBeenCalledTimes(1);
    expect(editText).toHaveBeenCalledWith(1, 1, 0, 0, { text: "Rent is due on the second." });
  });

  it("sends nothing when the text is as it was", async () => {
    vi.mocked(api.textBlocks).mockResolvedValue(list([block()]));
    const te = useTextEdit.getState();
    await te.load(1, 0);
    await te.select(1, { page: 0, index: 0 });
    te.setDraft(1, block().text);
    await te.commit(1);
    expect(editText).not.toHaveBeenCalled();
  });

  it("applies a format change with the typed text and keeps the block selected", async () => {
    vi.mocked(api.textBlocks).mockResolvedValue(list([block()]));
    vi.mocked(editText).mockResolvedValue({
      state: {} as never,
      fallback: { font: "Helvetica-Bold", embedded: false, reason: "bold is not in the embedded font" },
    });
    const te = useTextEdit.getState();
    await te.load(1, 0);
    await te.select(1, { page: 0, index: 0 });
    te.setDraft(1, "New words");
    await te.apply(1, { bold: true });
    expect(editText).toHaveBeenCalledWith(1, 1, 0, 0, { bold: true, text: "New words" });
    const d = useTextEdit.getState().docs[1];
    expect(d.selected).toEqual({ page: 0, index: 0 });
    expect(d.draft).toBeNull();
    expect(d.notice).toContain("Helvetica Bold");
  });

  it("lists the page again when the document moved on (stale)", async () => {
    vi.mocked(api.textBlocks).mockResolvedValue(list([block()]));
    vi.mocked(editText).mockResolvedValue(null as never);
    const te = useTextEdit.getState();
    await te.load(1, 0);
    await te.select(1, { page: 0, index: 0 });
    vi.mocked(api.textBlocks).mockResolvedValue(list([block({ index: 2 })], 2));
    await te.apply(1, { size: 14 });
    expect(api.textBlocks).toHaveBeenCalledTimes(2);
    expect(useTextEdit.getState().docs[1].selected).toEqual({ page: 0, index: 2 });
  });

  it("adds a text box and selects the block it became", async () => {
    vi.mocked(api.textBlocks).mockResolvedValue(list([block({ index: 7, text: "Hello", rect: [100, 200, 160, 214] })], 2));
    vi.mocked(addText).mockResolvedValue({} as never);
    const te = useTextEdit.getState();
    te.setTool("text");
    te.startAdding(1, 0, [100, 200]);
    te.setAdding(1, "Hello");
    await te.finishAdding(1);
    expect(addText).toHaveBeenCalledWith(1, 1, 0, { text: "Hello", size: 12, color: [0, 0, 0], at: [100, 200] });
    expect(useTextEdit.getState().tool).toBe("select");
    expect(useTextEdit.getState().docs[1].selected).toEqual({ page: 0, index: 7 });
  });

  it("drops an empty new box", async () => {
    const te = useTextEdit.getState();
    te.startAdding(1, 0, [10, 10]);
    te.setAdding(1, "   ");
    await te.finishAdding(1);
    expect(addText).not.toHaveBeenCalled();
    expect(useTextEdit.getState().docs[1].adding).toBeNull();
  });
});
