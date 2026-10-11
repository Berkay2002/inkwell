import { describe, expect, it, vi } from "vitest";
import tilesCss from "../components/allTools.css?raw";
import { drawingOf, gridFor, hasFilled, missingIcons, type IconName } from "../components/Icons";
import manifest from "../assets/icons/manifest.json";
import { TOOL_GROUPS, TOOL_HUES } from "./toolsCatalog";
import { quickTools } from "./tools";

// toolItems.ts pulls in the stores; none of them is touched until a tool runs
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));
const { toolItems } = await import("./toolItems");

const sources = import.meta.glob<string>("../**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true });
const fluentFiles = Object.keys(import.meta.glob("../assets/icons/fluent/*.svg", { query: "?raw", eager: true }));

describe("icon set", () => {
  it("every named icon has a Regular and a Filled drawing", () => {
    expect(missingIcons()).toEqual([]);
  });

  it("every tool in the catalog names a glyph that exists, and a Filled one for its tile", () => {
    const tools = toolItems(null);
    expect(tools.length).toBeGreaterThan(10);
    for (const t of tools) {
      expect(drawingOf(t.icon, "regular", 20), `${t.id} -> ${t.icon}`).toBeDefined();
      expect(hasFilled(t.icon), `${t.id} -> ${t.icon} has no Filled drawing`).toBe(true);
    }
  });

  it("the Home quick tools name glyphs that exist and a group that has a colour", () => {
    for (const q of quickTools) {
      expect(drawingOf(q.icon, "filled", 24), q.id).toBeDefined();
      expect(TOOL_HUES as readonly string[], `${q.id} group ${q.group}`).toContain(q.group);
    }
  });

  it("picks the grid for a rendered size", () => {
    expect([12, 14, 16].map(gridFor)).toEqual([16, 16, 16]);
    expect([18, 20, 22].map(gridFor)).toEqual([20, 20, 20]);
    expect([24, 30, 40].map(gridFor)).toEqual([24, 24, 24]);
  });

  it("falls back to the nearest grid when Fluent draws none at the wanted size", () => {
    // column_double_compare has a 20 px drawing only
    expect(drawingOf("compare", "regular", 16)?.grid).toBe(20);
    expect(drawingOf("close", "regular", 16)?.grid).toBe(16);
  });

  it("our own glyphs are filled outlines on the 20 px grid, like Fluent's, with one path each", () => {
    for (const name of Object.keys(manifest.inkwell) as IconName[]) {
      for (const variant of ["regular", "filled"] as const) {
        const d = drawingOf(name, variant, 20)!;
        expect(d.grid, `${name} ${variant}`).toBe(20);
        expect(d.shapes, `${name} ${variant}`).toHaveLength(1);
        expect(d.shapes[0].d.length, `${name} ${variant}`).toBeGreaterThan(40);
      }
    }
    expect(drawingOf("search", "regular", 20)!.grid).toBe(20);
  });

  it("no component carries its own table of icon paths (the app and provider marks aside)", () => {
    const hits = Object.entries(sources)
      .filter(([f, src]) => !f.endsWith(".test.ts") && /const PATHS/.test(src))
      .map(([f]) => f);
    expect(hits).toEqual([]);
  });

  it("every vendored Fluent file is one that the manifest asks for, and every manifest glyph is vendored", () => {
    const want = new Set(Object.values(manifest.fluent));
    const have = new Set(fluentFiles.map((f) => f.split("/").pop()!.replace(/_(16|20|24)_(regular|filled).svg$/, "")));
    expect([...have].filter((n) => !want.has(n))).toEqual([]);
    expect([...want].filter((n) => !have.has(n))).toEqual([]);
  });

  it("every icon name used in a component is in the manifest", () => {
    const known = new Set<string>([...Object.keys(manifest.fluent), ...Object.keys(manifest.inkwell)]);
    const used = new Set<string>();
    for (const [f, src] of Object.entries(sources)) {
      if (!f.endsWith(".tsx")) continue;
      for (const m of src.matchAll(/<(?:Icon|Glyph)s+name="([A-Za-z]+)"/g)) used.add(m[1]);
    }
    const glyphMapped = new Set(["doc", "chev", "docked", "floating", "bar", "brain"]); // assistant Glyph names
    expect([...used].filter((n) => !known.has(n) && !glyphMapped.has(n))).toEqual([]);
  });
});

// ---- tile colours ---------------------------------------------------------------------------------------
const lin = (c: number) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const luminance = (hex: string) => {
  const n = parseInt(hex.slice(1), 16);
  return 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
};
const ratio = (a: string, b: string) => {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

/** group -> { bg: [light, dark], fg: [light, dark] } read from allTools.css */
function tileTokens() {
  const css = tilesCss;
  const out: Record<string, { bg: string[]; fg: string[] }> = {};
  for (const m of css.matchAll(/\[data-group="([a-z]+)"\]\s*\{([^}]*)\}/g)) {
    const pick = (name: string) => new RegExp(`--tile-${name}:\s*light-dark\((#[0-9a-f]{6}),\s*(#[0-9a-f]{6})\)`).exec(m[2])?.slice(1, 3) ?? [];
    out[m[1]] = { bg: pick("bg"), fg: pick("fg") };
  }
  return out;
}

describe("tool tile colours", () => {
  const tokens = tileTokens();

  it("every tool group and every hue has a tile colour in both themes", () => {
    for (const id of new Set([...TOOL_GROUPS.map((g) => g.id), ...TOOL_HUES])) {
      expect(tokens[id]?.bg, `${id} --tile-bg`).toHaveLength(2);
      expect(tokens[id]?.fg, `${id} --tile-fg`).toHaveLength(2);
    }
  });

  it("the glyph reads on its tile: 4.5:1 or more, light and dark", () => {
    for (const id of TOOL_HUES) {
      for (const [i, theme] of ["light", "dark"].entries()) {
        const r = ratio(tokens[id].bg[i], tokens[id].fg[i]);
        expect(r, `${id} ${theme}: ${tokens[id].fg[i]} on ${tokens[id].bg[i]} is ${r.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });

  it("no two groups share a tile colour", () => {
    for (const i of [0, 1]) {
      const seen = new Map<string, string>();
      for (const id of TOOL_HUES) {
        const c = tokens[id].bg[i];
        expect(seen.get(c), `${id} and ${seen.get(c)} both use ${c}`).toBeUndefined();
        seen.set(c, id);
      }
    }
  });
});
