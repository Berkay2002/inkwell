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
vi.mock("./edit", () => ({ applyState: vi.fn() }));
vi.mock("./api", () => ({
  docJsList: vi.fn(),
  docJsSource: vi.fn(),
  docJsCheck: vi.fn(),
  docJsApply: vi.fn(),
}));

import { useStore } from "../store";
import * as api from "./api";
import type { DocJsList } from "./api";
import { dirty, statusLine, useDocJs } from "./docJsStore";

const meta = { size: 10, tooLarge: false, unread: false, risks: [] as string[] };
const list = (names: string[] = ["Init"], extra: Partial<DocJsList> = {}): DocJsList => ({
  named: names.map((n) => ({ ...meta, id: `n:${n}`, name: n })),
  docActions: [
    { id: "d:open", label: "Document opens", page: null, steps: [] },
    { id: "d:WC", label: "Before closing", page: null, steps: [] },
  ],
  pageActions: [],
  counts: { fields: 2, fieldScripts: 3, links: 0, bookmarks: 0 },
  capabilities: [],
  incomplete: false,
  ...extra,
});
const ok = { ok: true, message: null, line: null, column: null, tooLarge: false };
const bad = { ok: false, message: "unexpected token", line: 2, column: 5, tooLarge: false };
const src = (id: string, text: string) => ({ id, text, size: text.length, tooLarge: false });
const sheet = () => useDocJs.getState().sheet!;

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  useDocJs.getState().close();
  useDocJs.setState({ sheet: null });
  useStore.setState({ docs: { 7: { rev: 0, pageCount: 4 } as never } });
  vi.mocked(api.docJsList).mockResolvedValue(list());
  vi.mocked(api.docJsSource).mockImplementation(async (_d, id) => src(id, id === "n:Init" ? "var a = 1;" : "var b = 2;"));
  vi.mocked(api.docJsCheck).mockResolvedValue(ok);
  vi.mocked(api.docJsApply).mockResolvedValue({ state: {} as never, id: null });
});

afterEach(() => {
  useDocJs.getState().close();
  useDocJs.setState({ sheet: null });
  vi.useRealTimers();
});

const open = async () => {
  useDocJs.getState().open(7);
  await vi.advanceTimersByTimeAsync(0);
};

describe("document JavaScript sheet", () => {
  it("opens on the first named script with its text and a syntax check", async () => {
    await open();
    expect(api.docJsList).toHaveBeenCalledWith(7);
    expect(sheet().selected).toBe("n:Init");
    expect(sheet().text).toBe("var a = 1;");
    expect(api.docJsCheck).toHaveBeenCalledWith("var a = 1;");
    expect(statusLine(sheet()).kind).toBe("ok");
  });

  it("checks typed text once, a moment after the last keystroke", async () => {
    await open();
    vi.mocked(api.docJsCheck).mockClear();
    useDocJs.getState().setText("var a = ");
    useDocJs.getState().setText("var a = (");
    await vi.advanceTimersByTimeAsync(100);
    expect(api.docJsCheck).not.toHaveBeenCalled();
    vi.mocked(api.docJsCheck).mockResolvedValue(bad);
    await vi.advanceTimersByTimeAsync(300);
    expect(api.docJsCheck).toHaveBeenCalledTimes(1);
    expect(api.docJsCheck).toHaveBeenCalledWith("var a = (");
    expect(statusLine(sheet())).toEqual({ kind: "error", text: "Line 2, column 5: unexpected token" });
  });

  it("saves a script with an error only after a yes, as one engine edit", async () => {
    await open();
    vi.mocked(api.docJsCheck).mockResolvedValue(bad);
    useDocJs.getState().setText("var a = (");
    await vi.advanceTimersByTimeAsync(300);
    await useDocJs.getState().save();
    expect(api.docJsApply).not.toHaveBeenCalled();
    expect(sheet().confirmSave).toBe(true);
    await useDocJs.getState().save(true);
    expect(api.docJsApply).toHaveBeenCalledTimes(1);
    expect(api.docJsApply).toHaveBeenCalledWith(7, { op: "setSource", id: "n:Init", source: "var a = (" });
    expect(sheet().confirmSave).toBe(false);
  });

  it("saves a clean script straight away and does nothing when unchanged", async () => {
    await open();
    await useDocJs.getState().save();
    expect(api.docJsApply).not.toHaveBeenCalled();
    useDocJs.getState().setText("var a = 2;");
    await vi.advanceTimersByTimeAsync(300);
    expect(dirty(sheet())).toBe(true);
    await useDocJs.getState().save();
    expect(api.docJsApply).toHaveBeenCalledWith(7, { op: "setSource", id: "n:Init", source: "var a = 2;" });
  });

  it("asks before dropping unsaved changes on select and on close", async () => {
    vi.mocked(api.docJsList).mockResolvedValue(list(["Init", "Two"]));
    await open();
    useDocJs.getState().setText("changed");
    useDocJs.getState().select("n:Two");
    expect(sheet().selected).toBe("n:Init");
    expect(sheet().confirmDiscard).toBe("n:Two");
    useDocJs.getState().answerDiscard(false);
    expect(sheet().text).toBe("changed");
    useDocJs.getState().close();
    expect(sheet().confirmDiscard).toBe("close");
    // a second Esc is not a yes
    useDocJs.getState().close();
    expect(useDocJs.getState().sheet).not.toBeNull();
    useDocJs.getState().answerDiscard(true);
    expect(useDocJs.getState().sheet).toBeNull();
  });

  it("switches script when nothing is unsaved", async () => {
    vi.mocked(api.docJsList).mockResolvedValue(list(["Init", "Two"]));
    await open();
    useDocJs.getState().select("n:Two");
    await vi.advanceTimersByTimeAsync(0);
    expect(sheet().selected).toBe("n:Two");
    expect(sheet().text).toBe("var b = 2;");
  });

  it("ignores a source that arrives after another script was chosen", async () => {
    vi.mocked(api.docJsList).mockResolvedValue(list(["Init", "Two"]));
    await open();
    let late!: (v: ReturnType<typeof src>) => void;
    vi.mocked(api.docJsSource).mockImplementationOnce(() => new Promise((r) => (late = r)));
    useDocJs.getState().select("n:Two");
    vi.mocked(api.docJsSource).mockImplementationOnce(async (_d, id) => src(id, "second"));
    useDocJs.setState((s) => ({ sheet: { ...s.sheet!, selected: "n:Init", text: sheet().saved } }));
    useDocJs.getState().select("n:Two");
    await vi.advanceTimersByTimeAsync(0);
    late(src("n:Two", "first"));
    await vi.advanceTimersByTimeAsync(0);
    expect(sheet().text).toBe("second");
  });

  it("adds, renames and deletes through the engine and selects what it made", async () => {
    await open();
    vi.mocked(api.docJsApply).mockResolvedValueOnce({ state: {} as never, id: "n:Script 1" });
    vi.mocked(api.docJsList).mockResolvedValue(list(["Init", "Script 1"]));
    useDocJs.getState().startAdd();
    await useDocJs.getState().commitAdd();
    expect(api.docJsApply).toHaveBeenLastCalledWith(7, { op: "add", name: null, source: "" });
    expect(sheet().selected).toBe("n:Script 1");

    useDocJs.getState().startRename();
    expect(sheet().renaming).toBe("Script 1");
    useDocJs.getState().setRenaming("Later");
    vi.mocked(api.docJsApply).mockResolvedValueOnce({ state: {} as never, id: "n:Later" });
    vi.mocked(api.docJsList).mockResolvedValue(list(["Init", "Later"]));
    await useDocJs.getState().commitRename();
    expect(api.docJsApply).toHaveBeenLastCalledWith(7, { op: "rename", id: "n:Script 1", name: "Later" });
    expect(sheet().selected).toBe("n:Later");

    vi.mocked(api.docJsList).mockResolvedValue(list(["Init"]));
    await useDocJs.getState().remove();
    expect(api.docJsApply).toHaveBeenLastCalledWith(7, { op: "delete", id: "n:Later" });
    expect(sheet().selected).toBe("n:Init");
  });

  it("clears a whole action by its slot id", async () => {
    const l = list([], {
      docActions: [
        { id: "d:WC", label: "Before closing", page: null, steps: [{ ...meta, id: "d:WC:0", javascript: true, what: "" }] },
      ],
    });
    vi.mocked(api.docJsList).mockResolvedValue(l);
    await open();
    expect(sheet().selected).toBe("d:WC:0");
    vi.mocked(api.docJsList).mockResolvedValue(list([]));
    await useDocJs.getState().remove();
    expect(api.docJsApply).toHaveBeenCalledWith(7, { op: "delete", id: "d:WC" });
  });

  it("starts a page action on an existing page only, reading nothing for it", async () => {
    await open();
    useDocJs.getState().newPageAction(9, "O");
    expect(sheet().error).toMatch(/4 pages/);
    useDocJs.getState().newPageAction(2, "C");
    await vi.advanceTimersByTimeAsync(0);
    expect(sheet().selected).toBe("p:1:C:0");
    expect(sheet().text).toBe("");
    expect(api.docJsSource).toHaveBeenCalledTimes(1);
  });

  it("reads the list again when the document changes underneath (undo)", async () => {
    await open();
    vi.mocked(api.docJsList).mockClear();
    useStore.setState({ docs: { 7: { rev: 1, pageCount: 4 } as never } });
    await vi.advanceTimersByTimeAsync(0);
    expect(api.docJsList).toHaveBeenCalledTimes(1);
  });

  it("shows the file's text again after an undo, and keeps unsaved typing", async () => {
    await open();
    useDocJs.getState().setText("var a = 2;");
    await vi.advanceTimersByTimeAsync(300);
    vi.mocked(api.docJsSource).mockImplementation(async (_d, id) => src(id, "var a = 2;"));
    await useDocJs.getState().save();
    expect(sheet().text).toBe("var a = 2;");
    expect(dirty(sheet())).toBe(false);
    // Ctrl+Z elsewhere: the file goes back, the editor follows
    vi.mocked(api.docJsSource).mockImplementation(async (_d, id) => src(id, "var a = 1;"));
    useStore.setState({ docs: { 7: { rev: 5, pageCount: 4 } as never } });
    await vi.advanceTimersByTimeAsync(0);
    expect(sheet().text).toBe("var a = 1;");
    expect(dirty(sheet())).toBe(false);
    // typing that is not saved survives another edit's refresh
    useDocJs.getState().setText("typing");
    useStore.setState({ docs: { 7: { rev: 6, pageCount: 4 } as never } });
    await vi.advanceTimersByTimeAsync(0);
    expect(sheet().text).toBe("typing");
  });

  it("asks before a new script drops unsaved typing", async () => {
    await open();
    useDocJs.getState().setText("typing");
    useDocJs.getState().startAdd();
    vi.mocked(api.docJsApply).mockResolvedValueOnce({ state: {} as never, id: "n:Script 1" });
    await useDocJs.getState().commitAdd();
    expect(api.docJsApply).not.toHaveBeenCalled();
    expect(sheet().confirmDiscard).toBe("add");
    useDocJs.getState().answerDiscard(false);
    expect(sheet().text).toBe("typing");
    await useDocJs.getState().commitAdd();
    useDocJs.getState().answerDiscard(true);
    await vi.advanceTimersByTimeAsync(0);
    expect(api.docJsApply).toHaveBeenCalledWith(7, { op: "add", name: null, source: "" });
  });

  it("shows a script over 1 MiB as too large and never edits it", async () => {
    vi.mocked(api.docJsSource).mockResolvedValue({ id: "n:Init", text: "", size: 5_000_000, tooLarge: true });
    await open();
    expect(statusLine(sheet()).text).toMatch(/too large/);
    useDocJs.getState().setText("x");
    expect(sheet().text).toBe("");
    expect(dirty(sheet())).toBe(false);
    expect(api.docJsCheck).not.toHaveBeenCalled();
  });
});
