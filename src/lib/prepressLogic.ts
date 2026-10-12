import type { BoxKind, Cmyk, ConvertReport, InkAt, Inspected, Intent, ObjectKind, MarkSet, MarksPreview, MarksSpec, PageBoxes, PageSeps, PrepressView, Rect4, ViewMode } from "./prepressApi";
import { type Rot, type Size, unrotatePoint } from "./layout";
import { parseRange } from "./pageRange";

/** Pure rules of the print-production tools (lib/prepressStore.ts holds the state, components/prepress the screens). */

// ---- units ----

export type Units = "pt" | "mm" | "in";
export const UNITS: Units[] = ["pt", "mm", "in"];
const PT_PER: Record<Units, number> = { pt: 1, mm: 72 / 25.4, in: 72 };
const DECIMALS: Record<Units, number> = { pt: 2, mm: 2, in: 3 };

export const toPt = (v: number, u: Units) => v * PT_PER[u];
export const fromPt = (pt: number, u: Units) => pt / PT_PER[u];

/** A number as typed, in `u`, as points: "3,5" and "3.5" both work; null when it is not a number. */
export function parseLength(text: string, u: Units): number | null {
  const t = text.trim().replace(",", ".");
  if (!/^-?\d*\.?\d+$/.test(t) && !/^-?\d+\.$/.test(t)) return null;
  const v = Number(t);
  return Number.isFinite(v) ? toPt(v, u) : null;
}

/** Points as the number shown in `u` (no unit), rounded to what the unit needs. */
export function showLength(pt: number, u: Units): string {
  const f = 10 ** DECIMALS[u];
  return String(Math.round(fromPt(pt, u) * f) / f);
}

// ---- plates ----

export interface OutputSettings {
  mode: ViewMode;
  /** Cyan, Magenta, Yellow, Black. */
  process: [boolean, boolean, boolean, boolean];
  /** Names of the spot colours that are off. */
  hiddenSpots: string[];
  /** Simulate overprint; without it the spots fold into CMYK and only the process plates can be switched. */
  overprint: boolean;
  /** Per cent: coverage above it is flagged. */
  tacLimit: number;
}

export const DEFAULT_TAC = 300;
export const MAX_TAC = 1000;

export const DEFAULT_OUTPUT: OutputSettings = {
  mode: "plates",
  process: [true, true, true, true],
  hiddenSpots: [],
  overprint: true,
  tacLimit: DEFAULT_TAC,
};

export const PROCESS_NAMES = ["Cyan", "Magenta", "Yellow", "Black"];
const PROCESS_RGB: [number, number, number][] = [
  [0, 174, 239],
  [236, 0, 140],
  [255, 221, 0],
  [35, 31, 32],
];
/** Spots that get a plane of their own (src-tauri/src/prepress/seps.rs); the page's others are folded. */
export const MAX_SPOT_PLANES = 27;

export interface Plate {
  /** "p0".."p3" for the process inks, "s:<name>" for a spot. */
  key: string;
  name: string;
  process: boolean;
  rgb: [number, number, number];
  on: boolean;
  /** Cannot be switched: spots with overprint simulation off. */
  locked: boolean;
}

/** The plates of a page in plane order: the four process inks, then its spots. */
export function plateList(seps: PageSeps | null, o: OutputSettings): Plate[] {
  const out: Plate[] = PROCESS_NAMES.map((name, i) => ({ key: `p${i}`, name, process: true, rgb: PROCESS_RGB[i], on: o.process[i], locked: false }));
  for (const s of (seps?.spots ?? []).slice(0, MAX_SPOT_PLANES)) {
    out.push({ key: `s:${s.name}`, name: s.name, process: false, rgb: s.rgb, on: !o.hiddenSpots.includes(s.name), locked: !o.overprint });
  }
  return out;
}

const isProcess = (key: string) => /^p[0-3]$/.test(key);
const spotName = (key: string) => key.slice(2);

/** Switches one plate. A locked plate does not move. */
export function togglePlate(o: OutputSettings, key: string): OutputSettings {
  if (isProcess(key)) {
    const process = [...o.process] as OutputSettings["process"];
    const i = Number(key.slice(1));
    process[i] = !process[i];
    return { ...o, process };
  }
  if (!o.overprint) return o;
  const name = spotName(key);
  return { ...o, hiddenSpots: o.hiddenSpots.includes(name) ? o.hiddenSpots.filter((n) => n !== name) : [...o.hiddenSpots, name] };
}

/** Only `key` on. The names of spots this page does not have stay as they were: they belong to other pages. */
export function onlyPlate(o: OutputSettings, key: string, seps: PageSeps | null): OutputSettings {
  const here = (seps?.spots ?? []).slice(0, MAX_SPOT_PLANES).map((s) => s.name);
  const elsewhere = o.hiddenSpots.filter((n) => !here.includes(n));
  const process = PROCESS_NAMES.map((_, i) => key === `p${i}`) as OutputSettings["process"];
  const hidden = here.filter((n) => key !== `s:${n}`);
  return { ...o, process, hiddenSpots: [...elsewhere, ...(o.overprint ? hidden : [])] };
}

/** Every plate on. */
export function allPlates(o: OutputSettings): OutputSettings {
  return { ...o, process: [true, true, true, true], hiddenSpots: [] };
}

/** Whether `key` is the only plate of the page that is on. */
export function isSolo(o: OutputSettings, key: string, seps: PageSeps | null): boolean {
  const on = plateList(seps, o).filter((p) => p.on);
  return on.length === 1 && on[0].key === key;
}

/** Click on a plate in the filmstrip: it alone, or everything again when it already is alone. */
export function clickSolo(o: OutputSettings, key: string, seps: PageSeps | null): OutputSettings {
  return isSolo(o, key, seps) ? allPlates(o) : onlyPlate({ ...o, mode: "plates" }, key, seps);
}

/** The view of one plate on its own, for its thumbnail. */
export function plateOnly(o: OutputSettings, key: string, seps: PageSeps | null): PrepressView {
  return wireView(onlyPlate({ ...o, mode: "plates" }, key, seps));
}

export const wireView = (o: OutputSettings): PrepressView => ({
  mode: o.mode,
  process: o.process,
  hiddenSpots: o.hiddenSpots,
  overprint: o.overprint,
  tacLimit: o.tacLimit,
});

/** A limit as typed ("300", "300%", "287,5"): 0 to 1000, or null. */
export function parseLimit(text: string): number | null {
  const t = text.trim().replace(/%$/, "").replace(",", ".").trim();
  if (!/^\d+(\.\d+)?$/.test(t)) return null;
  const v = Number(t);
  return v >= 0 && v <= MAX_TAC ? v : null;
}

/** "42%" for an ink value, no decimals below 1 ("<1%"). */
export function showInk(v: number): string {
  if (v <= 0.05) return "0%";
  if (v < 1) return "<1%";
  return `${Math.round(v)}%`;
}

/** The readout under the cursor, in the order of the plate list; plates the point has no value for are left out. */
export function readout(at: InkAt | null, plates: Plate[]): { plate: Plate; value: number | null }[] {
  return plates.map((plate) => {
    const v = at?.planes.find((p) => p.name === plate.name);
    return { plate, value: v ? v.value : null };
  });
}

/** The ink limit the engine measured at, as one sentence. */
export function coverageText(max: number, limit: number, regions: number, truncated: boolean): string {
  if (max <= limit) return `Highest ink on this page is ${Math.round(max)}%, within ${limit}%.`;
  const where = regions === 0 ? "" : ` in ${truncated ? "more than " : ""}${regions} ${regions === 1 ? "area" : "areas"}`;
  return `Ink goes up to ${Math.round(max)}%${where}, above ${limit}%.`;
}

// ---- object inspector ----

export const NOTHING_PAINTED = "Nothing is painted here";

/**
 * A point of the displayed page, `dx`, `dy` CSS px from its top-left corner at `zoom` and view rotation `rot`, as page
 * points from the top-left of the page as shown (`raw` is its size), which is what the engine's inspector and ink
 * probe take. Held to the page.
 */
export function pointOnPage(dx: number, dy: number, raw: Size, rot: Rot, zoom: number): [number, number] {
  const [x, y] = unrotatePoint(raw, rot, dx / zoom, dy / zoom);
  return [Math.min(Math.max(x, 0), raw[0]), Math.min(Math.max(y, 0), raw[1])];
}

const KIND_NAME: Record<ObjectKind, string> = {
  fill: "Fill",
  stroke: "Stroke",
  text: "Text",
  strokedText: "Stroked text",
  image: "Image",
  imageMask: "Image mask",
  shading: "Shading",
};

const round1 = (v: number) => String(Math.round(v * 10) / 10);
const unit = (v: number) => `${round1(v * 100)}%`;

/** One object as lines of text: what it is, its colour space, the values as written, the CMYK equivalent, overprint. */
export function objectLines(o: Inspected) {
  const v = o.values;
  let values: string;
  if (o.model === "shading") values = "Colours are not read";
  else if (v.length === 0) values = "";
  else if (o.model === "gray") values = `Gray ${unit(v[0])}`;
  else if (o.model === "rgb" && v.length === 3) values = `R ${Math.round(v[0] * 255)} G ${Math.round(v[1] * 255)} B ${Math.round(v[2] * 255)}`;
  else if (o.model === "cmyk" && v.length === 4) values = `C ${unit(v[0])} M ${unit(v[1])} Y ${unit(v[2])} K ${unit(v[3])}`;
  else if (o.model === "spot" && o.colorants.length === v.length) values = o.colorants.map((n, i) => `${n} ${unit(v[i])}`).join(", ");
  else values = v.map((x) => String(Math.round(x * 100) / 100)).join(" ");
  const cmyk = o.cmyk
    ? `C ${round1(o.cmyk[0])}% M ${round1(o.cmyk[1])}% Y ${round1(o.cmyk[2])}% K ${round1(o.cmyk[3])}%${o.ink == null ? "" : `, ink ${showInk(o.ink)}`}`
    : "Not available";
  const detail = o.font ? ` in ${o.font}` : o.image ? `, ${o.image[0]} x ${o.image[1]} pixels` : "";
  return {
    kind: `${KIND_NAME[o.kind]}${detail}`,
    space: o.colorSpace,
    values,
    cmyk,
    overprint: o.overprint ? (o.overprintMode ? "Yes, mode 1" : "Yes") : "No",
  };
}

/** Escape clears the inspector's marker unless a field, menu, dialog, the find bar or the assistant already used it. */
export function escapeClearsMarker(e: { key: string; defaultPrevented: boolean }, inField: boolean): boolean {
  return e.key === "Escape" && !e.defaultPrevented && !inField;
}

// ---- pages ----

export type Scope = "all" | "page" | "range";

/** 0-based pages for a scope; undefined means every page; null when the range does not parse. */
export function scopePages(scope: Scope, range: string, count: number, current: number): number[] | undefined | null {
  if (scope === "all") return undefined;
  if (scope === "page") return [current];
  const p = parseRange(range, count);
  return p && p.length > 0 ? p : null;
}

// ---- page boxes ----

export const BOX_KINDS: BoxKind[] = ["trim", "bleed", "art", "crop"];
export type RefKind = BoxKind | "media";
export const BOX_LABEL: Record<RefKind, string> = { trim: "Trim box", bleed: "Bleed box", art: "Art box", crop: "Crop box", media: "Media box" };

export const boxOf = (b: PageBoxes, k: RefKind): Rect4 => b[k];

/** Boxes the typed margins can be measured from: every other box of the page. */
export const refsFor = (kind: BoxKind): RefKind[] => (["media", "crop", "trim", "bleed", "art"] as RefKind[]).filter((r) => r !== kind);

const RANK: Record<RefKind, number> = { trim: 1, art: 1, bleed: 2, crop: 3, media: 4 };

/** Margins measured from a smaller box grow outward (a bleed from its trim); from a larger one they cut inward (a trim from the media). */
export const isOutward = (kind: BoxKind, ref: RefKind) => RANK[ref] < RANK[kind];

/** Where the box starts when it is opened: the usual reference for each kind. */
export const defaultRef = (kind: BoxKind): RefKind => (kind === "bleed" ? "trim" : kind === "trim" ? "bleed" : kind === "art" ? "trim" : "media");

export interface Margins {
  left: number;
  bottom: number;
  right: number;
  top: number;
}

export const ZERO: Margins = { left: 0, bottom: 0, right: 0, top: 0 };

/**
 * Which coordinate of a user-space box (`[x0, y0, x1, y1]`) is the left, bottom, right and top edge of the page
 * as it is shown, for /Rotate `rot`. A page turned 90 degrees shows its left edge where the box has its bottom.
 */
const EDGE_OF: Record<number, Record<keyof Margins, number>> = {
  0: { left: 0, bottom: 1, right: 2, top: 3 },
  90: { left: 1, top: 0, right: 3, bottom: 2 },
  180: { left: 2, top: 1, right: 0, bottom: 3 },
  270: { left: 3, bottom: 0, top: 2, right: 1 },
};

const SIDES: (keyof Margins)[] = ["left", "bottom", "right", "top"];

/** `ref` moved by `m` on each side as the page is shown: outward when `outward`, else inward. */
export function applyMargins(ref: Rect4, rot: number, m: Margins, outward: boolean): Rect4 {
  const out: Rect4 = [...ref];
  const edges = EDGE_OF[rot] ?? EDGE_OF[0];
  for (const side of SIDES) {
    const i = edges[side];
    // the low coordinates (x0, y0) go down to grow the box; the high ones go up
    const grow = i < 2 ? -m[side] : m[side];
    out[i] = ref[i] + (outward ? grow : -grow);
  }
  return out;
}

/**
 * The margins as the engine reads them with `shown: true`: `[left, bottom, right, top]` of the page as it is
 * shown, outward positive. Each page maps them through its own /Rotate and measures them from its own box.
 */
export function shownMargins(m: Margins, outward: boolean): Rect4 {
  return SIDES.map((side) => (outward ? m[side] : -m[side])) as Rect4;
}

/** Why the sheets do nothing on a certified document, and what a signed one gets instead of a rewrite. */
export const CERTIFIED_REASON = "This document is certified. Any change would void the certification, so Inkwell leaves it as it is.";
export const SIGNED_WARNING =
  "This document is signed. Changes are added to the end of the file, so the signatures keep verifying the original, but the file shows as changed after signing. Anything added and then removed in the same session stays in the saved file.";

/** The margins that take `ref` to `box`: `applyMargins` backwards. */
export function marginsBetween(ref: Rect4, box: Rect4, rot: number, outward: boolean): Margins {
  const edges = EDGE_OF[rot] ?? EDGE_OF[0];
  const m = { ...ZERO };
  for (const side of SIDES) {
    const i = edges[side];
    const grow = i < 2 ? ref[i] - box[i] : box[i] - ref[i];
    m[side] = outward ? grow : -grow;
  }
  return m;
}

const TOL = 0.01;
const within = (outer: Rect4, inner: Rect4) =>
  inner[0] >= outer[0] - TOL && inner[1] >= outer[1] - TOL && inner[2] <= outer[2] + TOL && inner[3] <= outer[3] + TOL;

/** Why `rect` cannot be the `kind` box of the page, the engine's rules (src-tauri/src/prepress/boxes.rs) said before asking it. */
export function boxProblem(kind: BoxKind, rect: Rect4, b: PageBoxes): string | null {
  if (!rect.every(Number.isFinite) || rect[2] - rect[0] < 1 || rect[3] - rect[1] < 1) return `The ${BOX_LABEL[kind].toLowerCase()} would be empty.`;
  if (kind === "bleed") {
    if (!within(b.media, rect)) return "The bleed box would go beyond the media box.";
    if (!within(rect, b.trim)) return "The trim box has to stay inside the bleed box.";
  } else if (kind === "trim") {
    if (!within(b.media, rect)) return "The trim box would go beyond the media box.";
    if (!within(b.bleed, rect)) return "The trim box has to stay inside the bleed box.";
  } else if (!within(b.media, rect)) {
    return `The ${BOX_LABEL[kind].toLowerCase()} would go beyond the media box.`;
  }
  return null;
}

// ---- the page as shown ----

export type Pt = [number, number];

/**
 * A point of the page's user space as a point of the page as it is shown (origin top-left), where `crop` is the
 * visible area and `rot` the page's /Rotate. Points outside the crop box come out negative or past the size.
 */
export function toShown(p: Pt, crop: Rect4, rot: number): Pt {
  const [x, y] = p;
  switch (rot) {
    case 90:
      return [y - crop[1], x - crop[0]];
    case 180:
      return [crop[2] - x, y - crop[1]];
    case 270:
      return [crop[3] - y, crop[2] - x];
    default:
      return [x - crop[0], crop[3] - y];
  }
}

export function rectShown(r: Rect4, crop: Rect4, rot: number): Rect4 {
  const a = toShown([r[0], r[1]], crop, rot);
  const b = toShown([r[2], r[3]], crop, rot);
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
}

/** The part of the page that is shown: the crop box inside the media box. */
export function visibleBox(b: PageBoxes): Rect4 {
  const [m, c] = [b.media, b.crop];
  const r: Rect4 = [Math.max(m[0], c[0]), Math.max(m[1], c[1]), Math.min(m[2], c[2]), Math.min(m[3], c[3])];
  return r[2] > r[0] && r[3] > r[1] ? r : m;
}

// ---- printer's marks ----

export interface MarkPath {
  /** `stroke` draws the registration colour (every plate); `fill` a colour bar patch in `cmyk`. */
  kind: "stroke" | "fill";
  width: number;
  cmyk: Cmyk | null;
  /** Subpaths of the path, in user space; curves are cut into short lines. */
  points: Pt[][];
}

/**
 * Reads the marks' content stream (the operators `marks.rs` writes: `w m l c re S f k`; the rest only change
 * state we do not draw) into paths. A line or a circle strokes; a rectangle fills.
 */
export function parseMarks(content: string): MarkPath[] {
  const out: MarkPath[] = [];
  let nums: number[] = [];
  let sub: Pt[][] = [];
  let width = 1;
  let fill: Cmyk = [0, 0, 0, 1];
  const last = (): Pt | undefined => sub[sub.length - 1]?.[sub[sub.length - 1].length - 1];
  for (const tok of content.split(/\s+/)) {
    if (tok === "") continue;
    const n = Number(tok);
    if (!Number.isNaN(n) && /^[-+.\d]/.test(tok)) {
      nums.push(n);
      continue;
    }
    const a = nums;
    switch (tok) {
      case "w":
        if (a.length >= 1) width = a[a.length - 1];
        break;
      case "m":
        if (a.length >= 2) sub.push([[a[a.length - 2], a[a.length - 1]]]);
        break;
      case "l":
        if (a.length >= 2 && sub.length > 0) sub[sub.length - 1].push([a[a.length - 2], a[a.length - 1]]);
        break;
      case "c": {
        const p0 = last();
        if (a.length >= 6 && p0) {
          const [x1, y1, x2, y2, x3, y3] = a.slice(-6);
          const cur = sub[sub.length - 1];
          for (let i = 1; i <= 8; i++) {
            const t = i / 8;
            const u = 1 - t;
            cur.push([
              u * u * u * p0[0] + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t * t * t * x3,
              u * u * u * p0[1] + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t * t * t * y3,
            ]);
          }
        }
        break;
      }
      case "re":
        if (a.length >= 4) {
          const [x, y, w, h] = a.slice(-4);
          sub.push([[x, y], [x + w, y], [x + w, y + h], [x, y + h]]);
        }
        break;
      case "k":
        if (a.length >= 4) fill = a.slice(-4) as Cmyk;
        break;
      case "S":
        if (sub.length > 0) out.push({ kind: "stroke", width, cmyk: null, points: sub });
        sub = [];
        break;
      case "f":
        if (sub.length > 0) out.push({ kind: "fill", width, cmyk: fill, points: sub });
        sub = [];
        break;
      default:
        break;
    }
    nums = [];
  }
  return out;
}

/** An SVG path of `paths` in the page as it is shown. */
export function svgPath(points: Pt[][], crop: Rect4, rot: number, close: boolean): string {
  return points
    .map((sp) =>
      sp
        .map((p, i) => {
          const [x, y] = toShown(p, crop, rot);
          return `${i === 0 ? "M" : "L"}${+x.toFixed(2)} ${+y.toFixed(2)}`;
        })
        .join("") + (close ? "Z" : ""),
    )
    .join("");
}

/** A CMYK patch (0 to 1 each) as a CSS colour. */
export function cmykCss([c, m, y, k]: Cmyk): string {
  const v = (a: number) => Math.round(255 * (1 - Math.min(1, a)) * (1 - Math.min(1, k)));
  return `rgb(${v(c)} ${v(m)} ${v(y)})`;
}

export interface MarksForm {
  marks: MarkSet;
  /** Points. */
  weight: number;
  /** Points from the trim edge to where the crop marks start; null lets the engine take the bleed and a little more. */
  offset: number | null;
  /** Points. */
  length: number;
  units: Units;
  scope: Scope;
  range: string;
}

export const WEIGHTS = [0.125, 0.25, 0.35, 0.5, 1];

export const DEFAULT_MARKS: MarksForm = {
  marks: { crop: true, bleed: true, registration: true, colorBars: true },
  weight: 0.25,
  offset: null,
  length: 18,
  units: "pt",
  scope: "all",
  range: "",
};

export const anyMark = (m: MarkSet) => m.crop || m.bleed || m.registration || m.colorBars;

/** The request for the pages of the scope (the preview asks for the current page alone). */
export function marksSpec(f: MarksForm, pages: number[] | undefined): MarksSpec {
  return {
    ...(pages ? { pages } : {}),
    marks: f.marks,
    weight: f.weight,
    ...(f.offset !== null ? { offset: f.offset } : {}),
    length: f.length,
  };
}

/** What a marks preview must be redrawn for: the same key means the same answer. */
export const marksKey = (page: number, rev: number, f: MarksForm) =>
  [page, rev, f.marks.crop, f.marks.bleed, f.marks.registration, f.marks.colorBars, f.weight, f.offset, f.length].join("|");

/** Lines of the preview's caption: how far the marks reach. */
export function marksCaption(p: MarksPreview, original: Rect4, u: Units): string {
  const grow = Math.max(original[0] - p.media[0], original[1] - p.media[1], p.media[2] - original[2], p.media[3] - original[3], 0);
  const w = p.media[2] - p.media[0];
  const h = p.media[3] - p.media[1];
  const size = `${showLength(w, u)} x ${showLength(h, u)} ${u}`;
  return grow < 0.01 ? `The page already has room for the marks (${size}).` : `The page grows to ${size}, ${showLength(grow, u)} ${u} on the widest side.`;
}

// ---- convert colours ----

export interface ConvertOptions {
  rgb: boolean;
  gray: boolean;
  spot: boolean;
  grayToK: boolean;
  preserveBlack: boolean;
  intent: Intent;
  /** The CMYK ICC profile picked for `intent: "profile"`: an open token and its file name. */
  profile: { token: string; name: string } | null;
  /** The output condition written with the profile; empty takes the profile's file name. */
  condition: string;
  scope: Scope;
  range: string;
  /** A new file the converted document is saved as, or this document (an undo step, saved by Save). */
  output: "newFile" | "inPlace";
}

export const DEFAULT_CONVERT: ConvertOptions = {
  rgb: true,
  gray: true,
  spot: false,
  grayToK: true,
  preserveBlack: true,
  intent: "device",
  profile: null,
  condition: "",
  scope: "all",
  range: "",
  output: "newFile",
};

export const INTENT_LABEL: Record<Intent, string> = {
  device: "Standard conversion",
  document: "The file's output intent",
  profile: "A CMYK profile file",
};

/** Why the options cannot be run, or null. */
export function convertProblem(o: ConvertOptions, count: number, current: number): string | null {
  if (!o.rgb && !o.gray && !o.spot) return "Choose which colours to convert.";
  if (o.intent === "profile" && !o.profile) return "Choose a CMYK profile file.";
  if (scopePages(o.scope, o.range, count, current) === null) return "The page range is not valid.";
  return null;
}

/** The engine's request for the options. */
export function convertSpec(o: ConvertOptions, count: number, current: number) {
  const pages = scopePages(o.scope, o.range, count, current);
  const condition = o.condition.trim() || (o.profile ? o.profile.name.replace(/\.(icc|icm)$/i, "") : "");
  return {
    ...(pages ? { pages } : {}),
    gray: o.gray,
    rgb: o.rgb,
    spot: o.spot,
    grayToK: o.grayToK,
    preserveBlack: o.preserveBlack,
    intent: o.intent,
    ...(o.intent === "profile" ? { condition } : {}),
  };
}

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** What the confirmation says will change: one plain sentence per effect, before anything is touched. */
export function convertSummary(o: ConvertOptions, count: number, current: number, dirty: boolean): string[] {
  const kinds = [o.rgb && "RGB", o.gray && "gray", o.spot && "spot colours"].filter(Boolean) as string[];
  const pages = scopePages(o.scope, o.range, count, current);
  const where = pages === undefined ? `all ${plural(count, "page", "pages")}` : plural(pages?.length ?? 0, "page", "pages");
  const out: string[] = [`${kinds.join(", ").replace(/, ([^,]*)$/, " and $1")} become CMYK on ${where}: text, lines, fills, shadings and pictures.`];
  if (o.rgb || o.gray) out.push("Pictures are rewritten as CMYK without compression; Compress makes them small again.");
  if (o.gray && o.grayToK) out.push("Gray becomes black ink alone.");
  if (o.preserveBlack && (o.rgb || o.gray)) out.push("Pure black stays black ink alone, whatever the profile would mix.");
  out.push(
    o.intent === "device"
      ? "The values come from the standard conversion; no profile is attached."
      : o.intent === "document"
        ? "The values are computed through the CMYK profile of the file's own output intent."
        : `The values are computed through ${o.profile?.name ?? "the profile"}, which also becomes the file's PDF/X output intent.`,
  );
  out.push("CMYK colours stay as they are.");
  out.push(
    o.output === "newFile"
      ? "The result is saved as a new file and this tab moves to it. The original file on disk is not touched."
      : "The change goes into this document as one step you can undo; Save writes it.",
  );
  if (o.output === "newFile" && dirty) out.push("This document has unsaved changes. They are saved into the new file too.");
  return out;
}

/** What is left after a conversion, as plain words. */
export function remainingText(r: ConvertReport): string {
  const left = [r.remaining[1] && "RGB", r.remaining[0] && "gray"].filter(Boolean) as string[];
  return left.length === 0 ? "No RGB or gray colours are left in the file." : `${left.join(" and ")} colours are still in use (outside the pages converted, or in a place the conversion does not reach).`;
}

/** The default name of the converted file. */
export function convertedFileName(stem: string, o: ConvertOptions): string {
  return `${stem} (CMYK${o.intent === "profile" && o.profile ? ` ${o.profile.name.replace(/\.(icc|icm)$/i, "")}` : ""}).pdf`;
}

/**
 * Where Tab or Shift+Tab goes inside a sheet that holds focus: the next of `count` controls, wrapping at the ends.
 * `at` is the focused control's index, or -1 when focus is on the sheet itself (or outside it): then the first
 * (the last backwards). `null` when there is nothing to focus.
 */
export function tabTarget(count: number, at: number, back: boolean): number | null {
  if (count <= 0) return null;
  if (at < 0) return back ? count - 1 : 0;
  return (at + (back ? -1 : 1) + count) % count;
}

/**
 * Ctrl/Cmd+S (not Save as), Ctrl/Cmd+W and Ctrl/Cmd+K are held back while Convert Colors waits to save: a Save, the
 * save prompt of a tab close and the palette's Save would each write the conversion over the original.
 */
export function holdsSave(e: { key: string; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean }, unsaved: boolean): boolean {
  return unsaved && (e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && ["s", "w", "k"].includes(e.key.toLowerCase());
}
