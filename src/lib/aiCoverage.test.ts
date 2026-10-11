import { describe, expect, it } from "vitest";
import coverage from "./aiCoverage.json";
import toolItemsSource from "./toolItems.ts?raw";
import toolsSource from "./tools.ts?raw";

/** The tool ids the app has: the All tools list's `id:` lines and the quick tools' ToolId union. */
function appToolIds(): string[] {
  const items = [...toolItemsSource.matchAll(/^\s{6}id: "([a-z0-9-]+)",$/gm)].map((m) => m[1]);
  const union = /export type ToolId = ([^;]+);/.exec(toolsSource)?.[1] ?? "";
  const quick = [...union.matchAll(/"([a-z0-9-]+)"/g)].map((m) => m[1]);
  return [...new Set([...items, ...quick])];
}

describe("the assistant's coverage of the app's tools", () => {
  const ids = appToolIds();
  const tools = coverage.tools as Record<string, string>;
  const excluded = coverage.excluded as Record<string, string>;

  it("finds the app's tool ids", () => {
    expect(ids.length).toBeGreaterThanOrEqual(14);
    expect(ids).toEqual(expect.arrayContaining(["compress", "ocr", "redact", "organize", "a11y"]));
  });

  it("covers or excludes (with a reason) every tool of the app", () => {
    const missing = ids.filter((id) => !(id in tools) && !(id in excluded));
    expect(missing, `add these to src/lib/aiCoverage.json: tools (assistant tool) or excluded (reason)`).toEqual([]);
  });

  it("never lists a tool as both covered and excluded", () => {
    expect(Object.keys(tools).filter((id) => id in excluded)).toEqual([]);
  });

  it("gives every exclusion a reason", () => {
    for (const [id, why] of Object.entries(excluded)) expect(why.trim().length, id).toBeGreaterThan(10);
  });

  it("maps only to tools of the assistant's registry", () => {
    const registry = new Set(coverage.registry);
    for (const [id, name] of Object.entries(tools)) expect(registry.has(name), `${id} -> ${name}`).toBe(true);
    expect(registry.size).toBe(coverage.registry.length);
    expect(registry.size).toBeLessThanOrEqual(40);
  });

  it("names no tool the app does not have", () => {
    for (const id of Object.keys(tools)) expect(ids, id).toContain(id);
  });
});
