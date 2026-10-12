import { beforeEach, describe, expect, it, vi } from "vitest";

// vitest runs in node: the stores read localStorage at import
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()), Channel: class {} }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("../lib/api");

import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { DocState } from "../store";
import { useMarks } from "../lib/marksStore";
import { useStore } from "../store";
import { MarksPanel } from "./MarksPanel";

const doc = { id: 1, pageCount: 3 } as DocState;
const more = { foreign: [], annotated: [], signed: false, certified: true };

// zustand reads its initial state when rendering on the server: read the live one instead
vi.spyOn(React, "useSyncExternalStore").mockImplementation((_sub, snap) => snap());

function html(kind: "headerFooter" | "bates") {
  useMarks.getState().open(kind, 1);
  useMarks.setState({ info: { 1: [] }, more: { 1: more } });
  return renderToStaticMarkup(<MarksPanel doc={doc} />);
}

describe("MarksPanel on a certified document", () => {
  beforeEach(() => {
    useMarks.setState({ sheet: null, info: {}, more: {} });
    useStore.setState({ docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty: false, currentPage: 0, pageCount: 3, organize: false } as never }, order: [1], active: 1 });
  });

  it("turns the style controls off, and leaves Add files… on so other files can still be numbered", () => {
    const out = html("bates");
    expect(out).toMatch(/<fieldset[^>]*\sdisabled/);
    const form = out.slice(out.indexOf("<fieldset"), out.indexOf("</fieldset>"));
    expect(form).not.toContain("Add files");
    const files = out.slice(out.indexOf("</fieldset>"));
    expect(files).toContain("Add files…");
    expect(files).not.toMatch(/<button[^>]*\sdisabled[^>]*>Add files/);
  });

  it("locks every control of the other kinds", () => {
    expect(html("headerFooter")).toMatch(/<fieldset[^>]*\sdisabled/);
  });
});
