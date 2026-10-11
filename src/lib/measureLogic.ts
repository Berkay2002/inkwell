/**
 * Pure helpers for measuring (src-tauri/src/measure, docs/measure.md): units, scales, the live label, the
 * 45 degree constraint, snapping and hit tests. Points are page space (points, y down, rotation applied).
 * No state here.
 */

export type Pt = [number, number];
export type MeasureKind = "distance" | "perimeter" | "area";
export type Unit = "pt" | "in" | "mm" | "cm" | "m" | "ft";

/** "paper" of `paperUnit` on the page stand for `real` of `realUnit` ("1 in = 10 ft"). */
export interface Scale {
  paper: number;
  paperUnit: Unit;
  real: number;
  realUnit: Unit;
}

export interface Quantity {
  value: number;
  unit: string;
  text: string;
}

export interface Measure {
  page: number;
  xref: number;
  kind: MeasureKind;
  points: Pt[];
  contents: string;
  author: string;
  quantity: Quantity;
  /** null: the file's measure uses a unit we have no name for (the quantity is still right). */
  scale: Scale | null;
}

export interface PageScale {
  scale: Scale;
  defined: boolean;
  label: string;
}

export type SnapKind = "endpoint" | "midpoint" | "intersection" | "line";
export interface SnapHit {
  kind: SnapKind;
  x: number;
  y: number;
  distance: number;
}

export interface Snaps {
  endpoint: boolean;
  midpoint: boolean;
  intersection: boolean;
  line: boolean;
}

export const UNITS: { id: Unit; label: string; area: string; ptPer: number }[] = [
  { id: "pt", label: "pt", area: "sq pt", ptPer: 1 },
  { id: "in", label: "in", area: "sq in", ptPer: 72 },
  { id: "mm", label: "mm", area: "sq mm", ptPer: 72 / 25.4 },
  { id: "cm", label: "cm", area: "sq cm", ptPer: 72 / 2.54 },
  { id: "m", label: "m", area: "sq m", ptPer: 72 / 0.0254 },
  { id: "ft", label: "ft", area: "sq ft", ptPer: 864 },
];
const U = (u: Unit) => UNITS.find((x) => x.id === u)!;

export const KIND_NAME: Record<MeasureKind, string> = { distance: "Distance", perimeter: "Perimeter", area: "Area" };
export const MIN_POINTS: Record<MeasureKind, number> = { distance: 2, perimeter: 2, area: 3 };

/** The spellings Rust's `Unit::parse` knows (and the same answers). */
export function parseUnit(s: string): Unit | null {
  switch (s.trim().toLowerCase()) {
    case "pt": case "pts": case "point": case "points": return "pt";
    case "in": case "inch": case "inches": case "\"": return "in";
    case "mm": case "millimeter": case "millimeters": case "millimetre": case "millimetres": return "mm";
    case "cm": case "centimeter": case "centimeters": case "centimetre": case "centimetres": return "cm";
    case "m": case "meter": case "meters": case "metre": case "metres": return "m";
    case "ft": case "foot": case "feet": case "'": return "ft";
    default: return null;
  }
}

/** `10`, `0.25`, `2.5`: up to six decimals, no trailing zeros (Rust `trim_num`). */
export function trimNum(v: number): string {
  return v.toFixed(6).replace(/0+$/, "").replace(/\.$/, "");
}

export const scaleLabel = (s: Scale) => `${trimNum(s.paper)} ${s.paperUnit} = ${trimNum(s.real)} ${s.realUnit}`;

/** A side of a scale: "1 in", "10ft", "0.25 in"; null for anything else. */
function side(t: string): { n: number; unit: Unit } | null {
  const m = /^\s*(\d+(?:\.\d*)?|\.\d+)\s*([^\d.\s].*?)\s*$/.exec(t);
  if (!m) return null;
  const unit = parseUnit(m[2]);
  const n = Number(m[1]);
  return unit && Number.isFinite(n) ? { n, unit } : null;
}

/** "1 in = 10 ft", "1in=10ft", "0.25 in = 1 ft"; null for anything else, such as "1:100", or a side that is not above zero. */
export function parseScale(s: string): Scale | null {
  const at = s.indexOf("=");
  if (at < 0) return null;
  const [l, r] = [side(s.slice(0, at)), side(s.slice(at + 1))];
  if (!l || !r || l.n <= 0 || r.n <= 0) return null;
  return { paper: l.n, paperUnit: l.unit, real: r.n, realUnit: r.unit };
}

export const validScale = (s: Scale) => [s.paper, s.real].every((v) => Number.isFinite(v) && v > 0);

export const UNSCALED: Scale = { paper: 1, paperUnit: "in", real: 1, realUnit: "in" };

/** Units of `realUnit` for one point on the page. */
export const realPerPt = (s: Scale) => s.real / (s.paper * U(s.paperUnit).ptPer);

/** The same ratio spelt in another real unit: 1 in = 10 ft is 1 in = 3.048 m. */
export function inUnit(s: Scale, unit: Unit): Scale {
  return { ...s, real: s.real * (U(s.realUnit).ptPer / U(unit).ptPer), realUnit: unit };
}

export function pathLength(p: Pt[]): number {
  let n = 0;
  for (let i = 1; i < p.length; i++) n += Math.hypot(p[i][0] - p[i - 1][0], p[i][1] - p[i - 1][1]);
  return n;
}

/** Shoelace; the polygon closes itself. */
export function polygonArea(p: Pt[]): number {
  let a = 0;
  for (let i = 0; i < p.length; i++) {
    const [x0, y0] = p[i];
    const [x1, y1] = p[(i + 1) % p.length];
    a += x0 * y1 - x1 * y0;
  }
  return Math.abs(a) / 2;
}

/** "1,234.50": two decimals and a comma every three digits, as the engine writes its own labels. */
export function numText(v: number): string {
  const [int, frac] = Math.abs(v).toFixed(2).split(".");
  return `${v < 0 ? "-" : ""}${int.replace(/\B(?=(\d{3})+(?!\d))/g, ",")}.${frac}`;
}

/**
 * The value `pts` would measure at `scale`, as the live label shows it before the engine writes the real one.
 * shortcut: the engine also applies a file's own /D and /A multipliers and per-viewport scales; this does not, so a
 * file from Acrobat with those can differ until the measurement is made (then the engine's label is shown).
 */
export function quantityOf(kind: MeasureKind, pts: Pt[], scale: Scale): Quantity {
  const k = realPerPt(scale);
  const u = U(scale.realUnit);
  if (kind === "area") {
    const value = polygonArea(pts) * k * k;
    return { value, unit: u.area, text: `${numText(value)} ${u.area}` };
  }
  const value = pathLength(kind === "distance" ? pts.slice(0, 2) : pts) * k;
  return { value, unit: u.label, text: `${numText(value)} ${u.label}` };
}

/** `to` moved onto the nearest 45 degree ray from `from` (Shift while drawing). */
export function constrain45(from: Pt, to: Pt): Pt {
  const [dx, dy] = [to[0] - from[0], to[1] - from[1]];
  const r = Math.hypot(dx, dy);
  if (r === 0) return to;
  const a = (Math.round(Math.atan2(dy, dx) / (Math.PI / 4)) * Math.PI) / 4;
  return [from[0] + r * Math.cos(a), from[1] + r * Math.sin(a)];
}

/**
 * Where a point lands: the first point of a path being closed when within `radius`, else the engine's snap `hit`
 * (already filtered), else the pointer itself. Shift constrains to 45 degrees from the last point and turns snapping
 * off (a snapped point would pull the line off its ray).
 */
export function place(raw: Pt, last: Pt | null, shift: boolean, hit: SnapHit | null, close: Pt | null, radius: number): { at: Pt; snap: SnapKind | "close" | null } {
  if (shift && last) return { at: constrain45(last, raw), snap: null };
  if (close && Math.hypot(raw[0] - close[0], raw[1] - close[1]) <= radius) return { at: close, snap: "close" };
  if (hit) return { at: [hit.x, hit.y], snap: hit.kind };
  return { at: raw, snap: null };
}

/** The scale that makes the line from `a` to `b` on the page `real` of `unit` long (calibrating from a printed dimension). */
export function calibrate(a: Pt, b: Pt, real: number, unit: Unit): Scale | null {
  const pt = Math.hypot(b[0] - a[0], b[1] - a[1]);
  if (!(pt > 0) || !Number.isFinite(real) || real <= 0) return null;
  return { paper: pt / 72, paperUnit: "in", real, realUnit: unit };
}

export interface Total {
  kind: MeasureKind;
  unit: string;
  count: number;
  value: number;
  text: string;
}

/** Cumulative values: one total per kind and unit (never a sum across units), in the order kinds are listed. */
export function totals(items: Measure[]): Total[] {
  const by = new Map<string, Total>();
  for (const m of items) {
    const key = `${m.kind}|${m.quantity.unit}`;
    const t = by.get(key) ?? { kind: m.kind, unit: m.quantity.unit, count: 0, value: 0, text: "" };
    t.count += 1;
    t.value += m.quantity.value;
    by.set(key, t);
  }
  const order: MeasureKind[] = ["distance", "perimeter", "area"];
  return [...by.values()]
    .map((t) => ({ ...t, text: `${numText(t.value)} ${t.unit}` }))
    .sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind) || a.unit.localeCompare(b.unit));
}
