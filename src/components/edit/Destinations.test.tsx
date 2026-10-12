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
vi.mock("../../lib/api");

import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import type { DestEntry, DestList } from "../../lib/api";
import { useNav } from "../../lib/nav";
import { Destinations } from "./Destinations";

// zustand reads its initial state when rendering on the server: read the live one instead
vi.spyOn(React, "useSyncExternalStore").mockImplementation((_sub, snap) => snap());

const entry = (name: string, over: Partial<DestEntry> = {}): DestEntry => ({
  name,
  dest: { kind: "page", page: 2, at: null },
  inDests: false,
  inTree: true,
  shadowed: false,
  twice: false,
  refs: 0,
  usedBy: [],
  maybe: null,
  ...over,
});

function html(items: DestEntry[], over: Partial<DestList> = {}) {
  const dests: DestList = { rev: 0, items, both: false, truncated: false, frozen: null, locked: null, signed: false, unchecked: false, ...over };
  useNav.setState({ docs: { 1: { dests } as never } });
  return renderToStaticMarkup(<Destinations doc={1} />);
}

describe("Destinations", () => {
  beforeEach(() => useNav.setState({ docs: {} }));

  it("lists the page, or Broken, and shows the source only when the file has both", () => {
    const one = html([entry("alpha"), entry("lost", { dest: { kind: "broken", what: "no such page" } })]);
    expect(one).toContain("p. 3");
    expect(one).toContain("Broken");
    expect(one).not.toContain("Name tree");
    const both = html([entry("alpha", { inDests: true, inTree: false }), entry("beta", { shadowed: true })], { both: true });
    expect(both).toContain("/Dests");
    expect(both).toContain("Name tree");
    expect(both).toContain("does not find it");
  });

  it("says who uses a name and why a script may", () => {
    const out = html([entry("alpha", { refs: 2, usedBy: ['bookmark "One"', "a link on page 1"], maybe: "JavaScript in the document mentions it" })]);
    expect(out).toContain('Used by 2: bookmark &quot;One&quot;, a link on page 1');
    expect(out).toContain("JavaScript in the document mentions it");
  });

  it("turns every change off, with the reason, in a certified document", () => {
    const why = "This document is certified, so its named destinations are left as they are.";
    const out = html([entry("alpha")], { locked: why });
    expect(out).toContain(why);
    for (const label of ["Rename", "Set to current view", "Delete", "Add here", "Remove unused"]) {
      expect(out).toMatch(new RegExp(`<button[^>]*\\sdisabled[^>]*>(<svg.*?</svg>)?${label}`));
    }
  });

  it("warns on a signed document and says when only the first names are listed", () => {
    const out = html([entry("alpha")], { signed: true, truncated: true });
    expect(out).toContain("This document is signed");
    expect(out).toContain("at most 20,000");
  });

  it("keeps Remove unused off when what uses the names could not be fully found", () => {
    expect(html([entry("alpha")], { unchecked: true })).toMatch(/<button[^>]*\sdisabled[^>]*>Remove unused/);
    expect(html([entry("alpha")])).not.toMatch(/<button[^>]*\sdisabled[^>]*>Remove unused/);
  });
});
