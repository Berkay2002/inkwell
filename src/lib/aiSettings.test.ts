import { beforeEach, describe, expect, it, vi } from "vitest";

// ai.ts is imported for its settings only; the store is not involved. vitest runs in node.
const store = vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
  return m;
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));

import { loadSettings } from "./ai";

describe("assistant settings", () => {
  beforeEach(() => store.clear());

  it("Claude starts on Haiku with medium effort", () => {
    const s = loadSettings();
    expect([s.claudeModel, s.claudeEffort]).toEqual(["haiku", "medium"]);
    expect(s.provider).toBe("codex");
  });

  it("settings saved before the defaults existed (null) get them", () => {
    store.set("inkwell.ai", JSON.stringify({ claudeModel: null, claudeEffort: null, consent: true }));
    const s = loadSettings();
    expect([s.claudeModel, s.claudeEffort, s.consent]).toEqual(["haiku", "medium", true]);
  });

  it("a choice the user made is kept", () => {
    store.set("inkwell.ai", JSON.stringify({ claudeModel: "claude-opus-5", claudeEffort: "high" }));
    const s = loadSettings();
    expect([s.claudeModel, s.claudeEffort]).toEqual(["claude-opus-5", "high"]);
  });
});
