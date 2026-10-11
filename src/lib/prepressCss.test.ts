import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const read = (p: string) => readFileSync(new URL(p, import.meta.url), "utf8");
const css = read("../components/prepress/prepress.css");
const tokens = read("../styles.css");

describe("prepress.css", () => {
  it("uses only tokens that exist and no colour of its own", () => {
    const defined = new Set([...tokens.matchAll(/(--[\w-]+)\s*:/g)].map((m) => m[1]));
    const used = [...css.matchAll(/var\((--[\w-]+)/g)].map((m) => m[1]);
    // --k is set inline by the preview, per page
    expect(used.filter((v) => v !== "--k" && !defined.has(v))).toEqual([]);
    // paper is white; every other colour is a token
    expect([...css.matchAll(/#[0-9a-f]{3,8}\b/gi)].map((m) => m[0].toLowerCase()).filter((c) => c !== "#fff")).toEqual([]);
  });

  it("draws the trim box green and the bleed box blue, as Acrobat does", () => {
    expect(tokens).toMatch(/--pp-trim:\s*#1f9d55/);
    expect(tokens).toMatch(/--pp-bleed:\s*#2563eb/);
    expect(css).toMatch(/\.pp-trim\s*\{[^}]*stroke:\s*var\(--pp-trim\)/);
    expect(css).toMatch(/\.pp-bleed\s*\{[^}]*stroke:\s*var\(--pp-bleed\)/);
  });
});
