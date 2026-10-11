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

const dialog = vi.hoisted(() => ({ message: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: dialog.message }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onCloseRequested: () => Promise.resolve(() => {}) }) }));
vi.mock("./tiles", () => ({ dropCached: vi.fn() }));
vi.mock("./api", () => ({ textBlocks: vi.fn(), textEdit: vi.fn(), textAdd: vi.fn(), saveDocument: vi.fn(), cancelSearch: vi.fn(), closeDocument: vi.fn(() => Promise.resolve()) }));

import { useStore } from "../store";
import * as api from "./api";
import type { EditState, TextBlock } from "./api";
import { requestClose } from "./edit";
import { useTextEdit } from "./textEdit";

const block: TextBlock = {
  index: 0,
  text: "Rent is due on the first.",
  rect: [72, 100, 300, 114],
  lines: [[72, 100, 300, 114]],
  font: "Arial",
  size: 12,
  bold: false,
  italic: false,
  color: [0, 0, 0],
  align: "left",
  lineSpacing: null,
  editable: true,
  issue: null,
  reason: null,
};
const state = (over: Partial<EditState> = {}): EditState =>
  ({ rev: 2, pageCount: 1, sizes: [[595, 842]], path: "C:\\d\\a.pdf", dirty: true, undo: "Edit text on p. 1", redo: null, ...over }) as EditState;

beforeEach(async () => {
  vi.clearAllMocks();
  useStore.setState({
    docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty: false, scratch: null, currentPage: 0, pageCount: 1, rev: 1, sizes: [[595, 842]], search: { query: "" } } as never },
    order: [1],
    active: 1,
  });
  useTextEdit.setState({ open: true, tool: "select", docs: {} });
  vi.mocked(api.textBlocks).mockResolvedValue({ rev: 1, blocks: [block], areas: [] });
  const te = useTextEdit.getState();
  await te.load(1, 0);
  await te.select(1, { page: 0, index: 0 });
  te.setDraft(1, "Rent is due on the second.");
});

describe("leaving a document with typed text", () => {
  it("sends the typed text before the unsaved check, so closing asks about it", async () => {
    vi.mocked(api.textEdit).mockResolvedValue({ state: state(), fallback: null });
    dialog.message.mockResolvedValue("Cancel");
    await requestClose(1);
    expect(api.textEdit).toHaveBeenCalledWith(1, 1, 0, 0, { text: "Rent is due on the second." });
    // The document is dirty by then: the person is asked, and a Cancel keeps the tab.
    expect(dialog.message).toHaveBeenCalledTimes(1);
    expect(String(dialog.message.mock.calls[0][0])).toContain("Save the changes to a.pdf");
    expect(useStore.getState().docs[1]).toBeDefined();
  });

  it("asks before dropping text the engine refused, and keeps the tab on Keep editing", async () => {
    vi.mocked(api.textEdit).mockRejectedValue("\"✓\" can't be shown");
    dialog.message.mockResolvedValue("Keep editing");
    await requestClose(1);
    expect(dialog.message).toHaveBeenCalledTimes(1);
    expect(String(dialog.message.mock.calls[0][0])).toContain("Rent is due on the second.");
    expect(useStore.getState().docs[1]).toBeDefined();
    expect(useTextEdit.getState().docs[1].draft?.text).toBe("Rent is due on the second.");
  });

  it("closes when the person chooses to discard text the engine refused", async () => {
    vi.mocked(api.textEdit).mockRejectedValue("no");
    dialog.message.mockResolvedValue("Discard it");
    await requestClose(1);
    expect(useStore.getState().docs[1]).toBeUndefined();
  });

  it("closes without a question when nothing was typed", async () => {
    useTextEdit.setState((s) => ({ docs: { ...s.docs, 1: { ...s.docs[1], draft: null } } }));
    await requestClose(1);
    expect(dialog.message).not.toHaveBeenCalled();
    expect(useStore.getState().docs[1]).toBeUndefined();
  });
});
