import manifest from "../assets/icons/manifest.json";

/**
 * Every icon in the app comes from two folders of SVG files (docs/icons.md):
 * - src/assets/icons/fluent/    Fluent UI System Icons (MIT), vendored by scripts/vendor-fluent-icons.mjs. Filled
 *                               shapes on a 16, 20 or 24 px grid, a Regular and a Filled drawing for each.
 * - src/assets/icons/inkwell/   our own, for the PDF-specific tools Fluent has no good glyph for: generated in the
 *                               Fluent style (scripts/icons-gen) and traced to one filled path on the 20 px grid.
 * The app mark (ink drop, Chrome.tsx) and the assistant's provider mark are brand marks and stay outside this set.
 * `name` is ours; src/assets/icons/manifest.json says which glyph it is.
 */
export type IconName = keyof typeof manifest.fluent | keyof typeof manifest.inkwell;

/** A glyph: filled paths on a square grid (Fluent's and the traced Inkwell ones alike). */
interface Drawing {
  grid: number;
  shapes: { d: string; evenodd: boolean }[];
}

type Variant = "regular" | "filled";

function parse(svg: string, grid: number): Drawing {
  const shapes = [...svg.matchAll(/<path\b[^>]*>/g)].map(([tag]) => ({
    d: /\sd="([^"]*)"/.exec(tag)?.[1] ?? "",
    evenodd: /\sfill-rule="evenodd"/.test(tag),
  }));
  return { grid, shapes };
}

const fluentFiles = import.meta.glob<string>("../assets/icons/fluent/*.svg", { query: "?raw", import: "default", eager: true });
const inkwellFiles = import.meta.glob<string>("../assets/icons/inkwell/*.svg", { query: "?raw", import: "default", eager: true });

/** glyph -> variant -> grid -> drawing. */
type Drawn = Map<string, Record<Variant, Map<number, Drawing>>>;
const fluent: Drawn = new Map();
const inkwell: Drawn = new Map();

function slot(into: Drawn, id: string) {
  let s = into.get(id);
  if (!s) into.set(id, (s = { regular: new Map(), filled: new Map() }));
  return s;
}
for (const [file, svg] of Object.entries(fluentFiles)) {
  const m = /([a-z0-9_]+)_(16|20|24)_(regular|filled)\.svg$/.exec(file);
  if (m) slot(fluent, m[1])[m[3] as Variant].set(Number(m[2]), parse(svg, Number(m[2])));
}
for (const [file, svg] of Object.entries(inkwellFiles)) {
  // <name>.svg and <name>-filled.svg with an alphanumeric name; anything else (a hyphenated name would have
  // registered under its last segment) is left out, and the icon test fails on such a file
  const m = /^([A-Za-z0-9]+)(-filled)?\.svg$/.exec(file.split("/").pop() ?? "");
  if (m) slot(inkwell, m[1])[m[2] ? "filled" : "regular"].set(20, parse(svg, 20));
}

/** The Fluent grid for a rendered size: 16 for dense menus, 20 for the toolbar, 24 for tile glyphs and big marks. */
export function gridFor(size: number): 16 | 20 | 24 {
  return size <= 17 ? 16 : size <= 22 ? 20 : 24;
}

/** The drawing for `name`: the wanted variant and grid, else the nearest grid, else the Regular drawing. */
export function drawingOf(name: IconName, variant: Variant, size: number): Drawing | undefined {
  const fluentId = (manifest.fluent as Record<string, string>)[name];
  const sets = fluentId ? fluent.get(fluentId) : inkwell.get(name);
  if (!sets) return undefined;
  const want = gridFor(size);
  for (const v of variant === "filled" ? (["filled", "regular"] as const) : (["regular"] as const)) {
    const grids = [...sets[v].keys()].sort((a, b) => Math.abs(a - want) - Math.abs(b - want));
    if (grids.length) return sets[v].get(grids[0]);
  }
  return undefined;
}

/** Names that have no drawing in the folders (the vitest checks this is empty). */
export function missingIcons(): string[] {
  const all = [...Object.keys(manifest.fluent), ...Object.keys(manifest.inkwell)] as IconName[];
  return all.filter((n) => !drawingOf(n, "regular", 20) || !drawingOf(n, "filled", 20));
}

/** Whether `name` has its own Filled drawing (not the Regular one standing in). */
export function hasFilled(name: IconName): boolean {
  const fluentId = (manifest.fluent as Record<string, string>)[name];
  const sets = fluentId ? fluent.get(fluentId) : inkwell.get(name);
  return !!sets && sets.filled.size > 0;
}

/**
 * An icon. `size` is the rendered size in px: 16 in dense menus, 20 on the toolbar, 24 for a tile glyph (the grid
 * follows it, so strokes land on whole pixels). `filled` is the Filled drawing: the pressed / selected state of a
 * toolbar button, and the glyph on a tool tile. Always aria-hidden: the control carries the label.
 */
export function Icon({ name, size = 16, filled = false }: { name: IconName; size?: number; filled?: boolean }) {
  const d = drawingOf(name, filled ? "filled" : "regular", size);
  if (!d) return null;
  return (
    <svg width={size} height={size} viewBox={`0 0 ${d.grid} ${d.grid}`} fill="currentColor" aria-hidden focusable="false">
      {d.shapes.map((s, i) => (
        <path key={i} d={s.d} fillRule={s.evenodd ? "evenodd" : undefined} />
      ))}
    </svg>
  );
}
