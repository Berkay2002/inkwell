import { describe, expect, it } from "vitest";

const sources = import.meta.glob<string>("/src/**/*.tsx", { query: "?raw", import: "default", eager: true });

/**
 * Hooks called after an early return at the top level of a component: the hook count changes between renders and
 * React throws ("Rendered more hooks than during the previous render"), which blanked the window for Advanced search.
 * Reads the source: a top-level `if (...) return ...;` (or a block that returns) at the component's own indent,
 * then a `use...(` call at that same indent before the component's closing brace.
 */
function hooksAfterEarlyReturn(src: string): { line: number; text: string }[] {
  const lines = src.split(/\r?\n/);
  const found: { line: number; text: string }[] = [];
  let returned = false;
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    if (/^\}/.test(l)) {
      returned = false;
      continue;
    }
    if (/^ {2}if \(.*\) return\b.*;\s*$/.test(l) || (/^ {2}if \(.*\) \{\s*$/.test(l) && /^ {4}return\b/.test(lines[i + 1] ?? "") && /^ {2}\}\s*$/.test(lines[i + 2] ?? ""))) {
      returned = true;
      continue;
    }
    if (returned && /^ {2}(?:(?:const|let)\b[^=]*=\s*)?use[A-Z]\w*\(/.test(l)) found.push({ line: i + 1, text: l.trim() });
  }
  return found;
}

describe("hook order", () => {
  it("the scan itself sees a hook after an early return", () => {
    const bad = ["export function P() {", "  const a = useThing((s) => s.a);", "  if (!a) return null;", "  const b = useThing((s) => s.b);", "  return null;", "}"].join("\n");
    expect(hooksAfterEarlyReturn(bad)).toEqual([{ line: 4, text: "const b = useThing((s) => s.b);" }]);
    const ok = ["export function P() {", "  const a = useThing((s) => s.a);", "  const b = useThing((s) => s.b);", "  if (!a) return null;", "  return null;", "}"].join("\n");
    expect(hooksAfterEarlyReturn(ok)).toEqual([]);
  });

  it("no component under src calls a hook after an early return", () => {
    const files = Object.entries(sources);
    expect(files.length).toBeGreaterThan(50);
    const bad = files.flatMap(([f, src]) => hooksAfterEarlyReturn(src).map((h) => `${f}:${h.line}  ${h.text}`));
    expect(bad).toEqual([]);
  });
});
