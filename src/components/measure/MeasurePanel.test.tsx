import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("../../lib/edit", () => ({ applyState: vi.fn(), setLeaveGuard: vi.fn(), queued: vi.fn() }));

import { useMeasure } from "../../lib/measure";
import type { Measure } from "../../lib/measureLogic";
import type { DocState } from "../../store";
import { MeasurePanel } from "./MeasurePanel";

const M: Measure = {
  page: 0,
  xref: 5,
  kind: "distance",
  points: [[0, 0], [100, 0]],
  contents: "",
  author: "",
  quantity: { value: 10, unit: "ft", text: "10.00 ft" },
  scale: null,
  plain: true,
};
// A server render reads the store's initial state object, not the current one: the test writes to both.
const set = (p: Partial<ReturnType<typeof useMeasure.getState>>) => {
  useMeasure.setState(p);
  Object.assign(useMeasure.getInitialState(), p);
};
const doc = { id: 1 } as DocState;
const render = () => renderToStaticMarkup(<MeasurePanel doc={doc} />);
const docs = { 1: { items: [M], selected: { page: 0, xref: 5 }, scales: {}, last: null, calib: null, exported: null, error: null } };

describe("the measure rail", () => {
  beforeEach(() => set({ open: true, docs, locks: {}, live: null }));

  it("shows the value being dragged in place of the stored one, for that measurement only", () => {
    expect(render()).toContain('<div class="ms-big">10.00 ft</div>');
    set({ live: { doc: 1, xref: 5, text: "31.25 ft" } });
    const out = render();
    expect(out).toContain('<div class="ms-big">31.25 ft</div>');
    expect(out).toContain("<b>31.25 ft</b>");
    // The cumulative totals are of what is stored.
    expect(out).toContain('<div class="ms-total"><span>Distance <span class="dim">× 1</span></span><b>10.00 ft</b></div>');
    set({ live: { doc: 1, xref: 6, text: "99 ft" } });
    expect(render()).toContain('<div class="ms-big">10.00 ft</div>');
  });

  it("says why nothing can be changed on a certified file", () => {
    const why = "This document is certified and does not allow annotations.";
    expect(render()).not.toContain(why);
    set({ locks: { 1: why } });
    const out = render();
    expect(out).toContain(why);
    expect(out).not.toContain("Drag a point on the page");
  });
});
