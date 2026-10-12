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
vi.mock("./prepressApi", () => ({
  prepressApi: { separations: vi.fn(() => Promise.resolve({})), view: vi.fn(() => Promise.resolve(1)), ink: vi.fn(() => Promise.resolve([])), protection: vi.fn(() => Promise.resolve({})) },
}));

import { useStore } from "../store";
import { useMeasure } from "./measure";
import { usePrepress } from "./prepressStore";
import { wirePrepressRail } from "./prepressRail";
import { useSign } from "./sign";

describe("Output Preview and the drawing tools", () => {
  beforeEach(() => {
    wirePrepressRail();
    usePrepress.setState({ open: false, docs: {}, sheet: null });
    useSign.setState({ open: false, tool: "select", rail: null });
    useMeasure.setState({ open: false, tool: "select" });
    useStore.setState({ docs: { 1: { id: 1, path: "a.pdf", name: "a.pdf", dirty: false, currentPage: 0, pageCount: 1, rev: 0, sizes: [[612, 792]] } as never }, order: [1], active: 1 });
  });

  it("opening Output Preview disarms an armed Sign and Measure, so the inspector's click places nothing", () => {
    useSign.getState().setOpen(true);
    useSign.getState().setTool("text");
    useMeasure.getState().setOpen(true);
    expect(useMeasure.getState().tool).toBe("distance");
    usePrepress.getState().setOpen(true);
    expect(useSign.getState()).toMatchObject({ open: false, tool: "select" });
    expect(useMeasure.getState()).toMatchObject({ open: false, tool: "select" });
  });

  it("opening Sign or Measure while Output Preview is open closes it", () => {
    usePrepress.getState().setOpen(true);
    useSign.getState().setOpen(true);
    expect(usePrepress.getState().open).toBe(false);
    usePrepress.getState().setOpen(true);
    useMeasure.getState().setOpen(true);
    expect(usePrepress.getState().open).toBe(false);
  });
});
