import { describe, expect, it, vi } from "vitest";
import { drawingOf, gridFor, hasFilled, missingIcons, type IconName } from "../components/Icons";
import manifest from "../assets/icons/manifest.json";
import { TOOL_GROUPS, TOOL_HUES } from "./toolsCatalog";
import { quickTools } from "./tools";

// toolItems.ts pulls in the stores; none of them is touched until a tool runs. store.ts reads localStorage at import
// time and vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
const { toolItems } = await import("./toolItems");

// vitest blanks every .css module (also with ?raw), so the tile tokens are read from the file itself
// @ts-expect-error no @types/node in this project
const { readFileSync } = await import("node:fs");
const tilesCss: string = readFileSync(new URL("../components/allTools.css", import.meta.url), "utf8");

const sources = import.meta.glob<string>("../**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true });
const fluentFiles = Object.keys(import.meta.glob("../assets/icons/fluent/*.svg", { query: "?raw", eager: true }));
const inkwellFiles = Object.keys(import.meta.glob("../assets/icons/inkwell/*.svg", { query: "?raw", eager: true }));

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
    const hits = Object.entries(sources).flatMap(([f, src]) => inlinePaths(src).map((h) => `${f}: ${h}`));
    // the ink-drop app mark (Chrome.tsx) and the assistant provider's mark (AssistantPanel.tsx) are brand marks
    expect(hits.filter((h) => !/\/(Chrome|AssistantPanel)\.tsx: /.test(h))).toEqual([]);
  });

  it("the inline-path check catches path constants and literal <path d>, and skips computed paths", () => {
    expect(inlinePaths('const GRIP = "M9.5 6h.01M14.5 6h.01";')).toHaveLength(1);
    expect(inlinePaths('const MINUS = "M6 12h12";')).toHaveLength(1);
    expect(inlinePaths('<path d="M1 2h3" />')).toHaveLength(1);
    expect(inlinePaths("<path d={`M${x} ${y}L${x2} ${y2}`} />")).toHaveLength(0);
    expect(inlinePaths('const msg = "Make the file smaller";')).toHaveLength(0);
  });

  it("every vendored Fluent file is one that the manifest asks for, and every manifest glyph is vendored", () => {
    const want = new Set(Object.values(manifest.fluent));
    const have = new Set(fluentFiles.map((f) => f.split("/").pop()!.replace(/_(16|20|24)_(regular|filled).svg$/, "")));
    expect([...have].filter((n) => !want.has(n))).toEqual([]);
    expect([...want].filter((n) => !have.has(n))).toEqual([]);
  });

  it("manifest keys are plain names, one per key, sorted, and every custom file is <name>.svg or <name>-filled.svg", () => {
    for (const set of [manifest.fluent, manifest.inkwell]) {
      const keys = Object.keys(set);
      expect(keys.filter((k) => !/^[A-Za-z0-9]+$/.test(k))).toEqual([]);
      expect(keys).toEqual([...keys].sort((a, b) => a.localeCompare(b, "en")));
    }
    const both = new Set([...Object.keys(manifest.fluent), ...Object.keys(manifest.inkwell)]);
    expect(both.size).toBe(Object.keys(manifest.fluent).length + Object.keys(manifest.inkwell).length);
    const stray = inkwellFiles.map((f) => f.split("/").pop()!).filter((f) => !/^[A-Za-z0-9]+(-filled)?\.svg$/.test(f));
    expect(stray).toEqual([]);
    const named = new Set(inkwellFiles.map((f) => f.split("/").pop()!.replace(/(-filled)?\.svg$/, "")));
    expect([...named].sort()).toEqual(Object.keys(manifest.inkwell).sort());
  });

  it("every icon name used in a component is in the manifest, and every Glyph name is one the assistant maps", () => {
    const known = new Set<string>([...Object.keys(manifest.fluent), ...Object.keys(manifest.inkwell)]);
    const parts = Object.entries(sources).find(([f]) => f.endsWith("/assistant/parts.tsx"))![1];
    const glyphKeys = new Set([...parts.matchAll(/^ {2}([a-z]+): "([A-Za-z]+)",$/gm)].map((m) => m[1]));
    const glyphValues = [...parts.matchAll(/^ {2}([a-z]+): "([A-Za-z]+)",$/gm)].map((m) => m[2]);
    expect(glyphValues.filter((n) => !known.has(n))).toEqual([]);
    const used = Object.entries(sources).flatMap(([f, src]) => (f.endsWith(".tsx") ? unknownNames(src, known, glyphKeys) : []));
    expect(used).toEqual([]);
  });

  it("the name check fails on a bogus name (it is not vacuous)", () => {
    const known = new Set(["search", "close"]);
    const glyphs = new Set(["doc"]);
    expect(unknownNames('<Icon name="search" size={16} /><Glyph name="doc" />', known, glyphs)).toEqual([]);
    expect(unknownNames('<Icon name="nope" />', known, glyphs)).toEqual(["Icon nope"]);
    expect(unknownNames('<Icon name={on ? "close" : "alsoNope"} />', known, glyphs)).toEqual(["Icon alsoNope"]);
    expect(unknownNames('<Glyph name="search" />', known, glyphs)).toEqual(["Glyph search"]);
  });
});

/** Path data written into a component: a string constant that is SVG path data, or a literal <path d="…">. */
function inlinePaths(src: string): string[] {
  const hits: string[] = [];
  for (const line of src.split(/\r?\n/)) {
    const literalPath = /<path\b[^>]*\sd="[^"{]/.test(line);
    const pathConstant = /["'`][Mm]\s?-?\d[\d.\s,-]*[A-Za-z][^"'`]*["'`]/.test(line) && /^\s*(export\s+)?(const|let)\s|:\s*["'`]/.test(line);
    if (literalPath || pathConstant) hits.push(line.trim().slice(0, 80));
  }
  return hits;
}

/** `<Icon name="x">` / `<Glyph name={a ? "x" : "y"}>` whose name is not known (Icon: the manifest; Glyph: the assistant's keys). */
function unknownNames(src: string, icons: Set<string>, glyphs: Set<string>): string[] {
  const out: string[] = [];
  for (const m of src.matchAll(/<(Icon|Glyph)\s+name=(?:"([A-Za-z0-9]+)"|\{([^}]*)\})/g)) {
    const names = m[2] ? [m[2]] : [...(m[3] ?? "").matchAll(/"([A-Za-z0-9]+)"/g)].map((x) => x[1]);
    for (const n of names) if (!(m[1] === "Icon" ? icons : glyphs).has(n)) out.push(`${m[1]} ${n}`);
  }
  return out;
}

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
    const pick = (name: string) => new RegExp(String.raw`--tile-${name}:\s*light-dark\((#[0-9a-f]{6}),\s*(#[0-9a-f]{6})\)`).exec(m[2])?.slice(1, 3) ?? [];
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
