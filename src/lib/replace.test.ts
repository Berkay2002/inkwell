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
vi.mock("./edit", () => ({ replaceText: vi.fn(), editText: vi.fn(), addText: vi.fn(), setLeaveGuard: vi.fn(), save: vi.fn(), undo: vi.fn(), redo: vi.fn(), applyState: vi.fn() }));
vi.mock("./api", () => ({ textBlocks: vi.fn() }));

import { useStore } from "../store";
import type { ReplaceResult } from "./api";
import { replaceText } from "./edit";
import { lockedReason, useReplace } from "./replace";
import { reportOf, whyNotReplace } from "./replaceLogic";
import { useTextEdit } from "./textEdit";

const result = (over: Partial<ReplaceResult> = {}): ReplaceResult => ({
  state: { rev: 2, pageCount: 1, sizes: [], path: "a.pdf", dirty: true, undo: "Replace", redo: null, redacted: false },
  replaced: 3,
  skipped: [],
  skippedCount: 0,
  stopped: null,
  fallbacks: [],
  inserted: 0,
  ...over,
});

describe("what a run reports", () => {
  it("says how many were replaced and skipped, and lists the reasons with their page", () => {
    const r = reportOf(
      result({ replaced: 5, skippedCount: 2, skipped: [{ page: 2, text: "aaa", reason: "skipped: it is set in a Type 3 font, whose letters are drawings" }] }),
    );
    expect(r.summary).toBe("5 replaced, 2 skipped");
    expect(r.lines).toEqual(['p. 3, "aaa": skipped: it is set in a Type 3 font, whose letters are drawings']);
    // One listed of two counted: the other is "more".
    expect(r.more).toBe(1);
  });

  it("carries why it stopped and which font stood in", () => {
    const r = reportOf(result({ stopped: "Stopped after 2000 replacements.", fallbacks: [{ font: "Helvetica", embedded: false, reason: "Foo has no glyph for 'ž'" }] }));
    expect(r.notes).toEqual(["Stopped after 2000 replacements.", "Helvetica was used for some text: Foo has no glyph for 'ž'."]);
  });

  it("shortens a long match in the list", () => {
    const r = reportOf(result({ skippedCount: 1, skipped: [{ page: 0, text: "x".repeat(60), reason: "skipped: why" }] }));
    expect(r.lines[0]).toContain("…");
    expect(r.lines[0].length).toBeLessThan(80);
  });
});

describe("when Replace is available", () => {
  const base = { locked: null, query: "cat", hits: 3, searching: false, current: "text" as const };

  it("needs a query, matches and (for one) a text match", () => {
    expect(whyNotReplace(base, false)).toBeNull();
    expect(whyNotReplace(base, true)).toBeNull();
    expect(whyNotReplace({ ...base, query: " " }, true)).toMatch(/find first/);
    expect(whyNotReplace({ ...base, hits: 0 }, true)).toMatch(/No matches/);
    expect(whyNotReplace({ ...base, current: null }, false)).toMatch(/Pick a match/);
    expect(whyNotReplace({ ...base, current: "other" }, false)).toMatch(/comments/);
    expect(whyNotReplace({ ...base, searching: true }, true)).toMatch(/Wait/);
  });

  it("says why a certified document refuses", () => {
    expect(whyNotReplace({ ...base, locked: "This document is certified." }, true)).toBe("This document is certified.");
  });
});

describe("replacing through the store", () => {
  const hit = (page: number, x: number, y: number) => ({ page, rects: [[x, y, x + 20, y + 10]] as [number, number, number, number][], before: "", text: "cat", after: "", source: "text" as const });
  const runSearch = vi.fn();

  beforeEach(() => {
    vi.mocked(replaceText).mockReset();
    runSearch.mockReset();
    useReplace.setState({ text: "dog", busy: false, report: {} });
    useStore.setState({
      docs: {
        1: {
          ...(useStore.getState().docs[1] ?? {}),
          id: 1,
          rev: 1,
          search: { query: "cat", id: 5, hits: [hit(0, 10, 100), hit(0, 80, 100)], current: 1, done: true, scanned: 1, total: 1, capped: false },
        } as never,
      },
      runSearch,
    } as never);
    useTextEdit.setState({ docs: {} });
  });

  it("replaces the current match and goes on after it, past what the replacement holds", async () => {
    vi.mocked(replaceText).mockResolvedValue({ kind: "ok", value: result({ replaced: 1, inserted: 1 }) });
    await useReplace.getState().one(1);
    const [id, rev, req] = vi.mocked(replaceText).mock.calls[0];
    expect([id, rev]).toEqual([1, 1]);
    expect(req).toMatchObject({ find: "cat", with: "dog", at: { page: 0, rect: [80, 100, 100, 110] } });
    expect(runSearch).toHaveBeenCalledWith(1, "cat", { page: 0, x: 80, y: 100, skip: 1 });
    expect(useReplace.getState().report[1]?.summary).toBe("1 replaced, 0 skipped");
  });

  it("replaces every match without a position and searches by the engine's own re-run", async () => {
    vi.mocked(replaceText).mockResolvedValue({ kind: "ok", value: result({ replaced: 4, skippedCount: 1, skipped: [{ page: 0, text: "cat", reason: "skipped: x" }] }) });
    await useReplace.getState().all(1);
    expect(vi.mocked(replaceText).mock.calls[0][2].at).toBeUndefined();
    expect(runSearch).not.toHaveBeenCalled();
    expect(useReplace.getState().report[1]?.summary).toBe("4 replaced, 1 skipped");
  });

  it("searches again when the document moved on, and says nothing was replaced when it errs", async () => {
    vi.mocked(replaceText).mockResolvedValueOnce({ kind: "stale" });
    await useReplace.getState().all(1);
    expect(runSearch).toHaveBeenCalledWith(1, "cat");
    vi.mocked(replaceText).mockResolvedValueOnce({ kind: "error", message: "This document is certified." });
    await useReplace.getState().all(1);
    expect(useReplace.getState().busy).toBe(false);
  });

  it("does nothing for a match that is not page text, or without a query", async () => {
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], search: { ...useStore.getState().docs[1].search, hits: [{ ...hit(0, 1, 1), source: "comment" as const }], current: 0 } } } } as never);
    await useReplace.getState().one(1);
    expect(replaceText).not.toHaveBeenCalled();
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], search: { ...useStore.getState().docs[1].search, query: "  " } } } } as never);
    await useReplace.getState().all(1);
    expect(replaceText).not.toHaveBeenCalled();
  });

  it("reads the lock from any listed page", () => {
    expect(lockedReason(1)).toBeNull();
    useTextEdit.setState({ docs: { 1: { pages: { 0: { rev: 1, blocks: [], areas: [], locked: "certified", signed: false } } } } } as never);
    expect(lockedReason(1)).toBe("certified");
  });
});
