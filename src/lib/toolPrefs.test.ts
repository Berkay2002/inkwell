import { describe, expect, it, vi } from "vitest";

// the module reads localStorage at import time; vitest runs in node. A tool that no longer exists is left behind.
vi.hoisted(() => {
  const m = new Map<string, string>([["inkwell.toolPrefs", JSON.stringify({ recent: ["gone", "a"], pinned: ["gone", "x", "y"] })]]);
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});

import { useToolPrefs } from "./toolPrefs";

describe("toolPrefs", () => {
  it("a renamed tool leaves no ghost pin: prune drops it and frees the slot", () => {
    const s = useToolPrefs.getState();
    expect(s.pinned).toEqual(["gone", "x", "y"]);
    // without the catalog the ghost fills a slot
    expect(s.togglePin("z")).toBe(false);
    // with it the ghost is dropped first, so the third real pin fits
    expect(useToolPrefs.getState().togglePin("z", ["x", "y", "z", "a"])).toBe(true);
    expect(useToolPrefs.getState().pinned).toEqual(["x", "y", "z"]);
    expect(useToolPrefs.getState().recent).toEqual(["a"]);
    expect(JSON.parse(localStorage.getItem("inkwell.toolPrefs")!)).toEqual({ recent: ["a"], pinned: ["x", "y", "z"] });
  });
  it("a fourth real pin is still refused", () => {
    expect(useToolPrefs.getState().togglePin("w", ["x", "y", "z", "w"])).toBe(false);
    expect(useToolPrefs.getState().pinned).toEqual(["x", "y", "z"]);
  });
  it("the panel is per session: never written to storage", () => {
    useToolPrefs.getState().setPanel(true);
    expect(JSON.parse(localStorage.getItem("inkwell.toolPrefs")!)).not.toHaveProperty("panelOpen");
    useToolPrefs.getState().setPanel(false);
  });
});
