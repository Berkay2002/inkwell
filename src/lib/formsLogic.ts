/** Pure helpers for forms (src-tauri/src/forms): the field model, grouping, progress, geometry. No state here. */

export type Rect4 = [number, number, number, number];
export type RGB = [number, number, number];

export type FieldKind = "text" | "checkbox" | "radio" | "combo" | "list" | "button" | "signature";

export type Format =
  | { kind: "none" }
  | { kind: "number"; value: { decimals: number; sep: number; neg: number; currency: string; prepend: boolean } }
  | { kind: "percent"; value: { decimals: number; sep: number } }
  | { kind: "date"; value: string }
  | { kind: "time"; value: string }
  /** 0 zip, 1 zip + 4, 2 phone, 3 social security number. */
  | { kind: "special"; value: number }
  /** 9 digit, A letter, O letter or digit, X any. */
  | { kind: "mask"; value: string };
export type Validate = { kind: "none" } | { kind: "range"; min: number | null; max: number | null };
export type CalcOp = "sum" | "product" | "average" | "minimum" | "maximum";
export type Calculate =
  | { kind: "none" }
  | { kind: "simple"; value: { op: CalcOp; fields: string[] } }
  | { kind: "notation"; value: string };
export type ButtonAction =
  | { kind: "reset"; fields: string[]; exclude?: boolean }
  | { kind: "named"; name: string }
  | { kind: "uri"; uri: string }
  | { kind: "alert"; message: string }
  | { kind: "submit"; url: string }
  | { kind: "script"; source: string };

export interface Opt {
  export: string;
  label: string;
}

export interface Field {
  page: number;
  xref: number;
  node: number;
  name: string;
  kind: FieldKind;
  rect: Rect4;
  tab: number;
  value: string;
  values: string[];
  defaultValue: string;
  checked: boolean;
  export: string;
  options: Opt[];
  readOnly: boolean;
  required: boolean;
  hidden: boolean;
  multiline: boolean;
  password: boolean;
  comb: boolean;
  editable: boolean;
  multiSelect: boolean;
  noToggleOff: boolean;
  maxLen: number;
  fontSize: number;
  textColor: RGB;
  align: number;
  borderColor: RGB | null;
  fillColor: RGB | null;
  borderWidth: number;
  format: Format;
  validate: Validate;
  calculate: Calculate;
  shown: string;
  caption: string;
  action: ButtonAction | null;
  tooltip: string;
  script: boolean;
}

export interface Form {
  fields: Field[];
  xfa: boolean;
}

export interface Detected {
  kind: FieldKind;
  name: string;
  rect: Rect4;
}

export interface NewField {
  kind: FieldKind;
  rect: Rect4;
  name?: string;
  group?: string;
  export?: string;
  caption?: string;
  options?: Opt[];
}

export interface FieldPatch {
  name?: string;
  tooltip?: string;
  required?: boolean;
  readOnly?: boolean;
  hidden?: boolean;
  defaultValue?: string;
  multiline?: boolean;
  password?: boolean;
  comb?: boolean;
  editable?: boolean;
  multiSelect?: boolean;
  maxLen?: number;
  options?: Opt[];
  export?: string;
  fontSize?: number;
  textColor?: RGB;
  align?: number;
  borderColor?: RGB;
  noBorder?: boolean;
  fillColor?: RGB;
  noFill?: boolean;
  borderWidth?: number;
  format?: Format;
  validate?: Validate;
  calculate?: Calculate;
  caption?: string;
  action?: ButtonAction;
}

export type FieldValue = string | string[] | boolean;

export const KIND_LABEL: Record<FieldKind, string> = {
  text: "Text field",
  checkbox: "Check box",
  radio: "Radio button",
  combo: "Drop-down",
  list: "List box",
  button: "Button",
  signature: "Signature",
};

/** The plural used in sentences ("2 check boxes"). */
const KIND_PLURAL: Record<FieldKind, string> = {
  text: "text fields",
  checkbox: "check boxes",
  radio: "radio buttons",
  combo: "drop-downs",
  list: "list boxes",
  button: "buttons",
  signature: "signature fields",
};

/** A field the user fills in (buttons and signatures are not). */
export const isFillable = (f: Field): boolean => f.kind !== "button" && f.kind !== "signature";

/** The widgets of one page, in tab order. */
export const onPage = (fields: Field[], page: number): Field[] =>
  fields.filter((f) => f.page === page).sort((a, b) => a.tab - b.tab);

/** One row per field: the buttons of a radio group are one row (the first widget stands for it). */
export function rows(fields: Field[]): Field[] {
  const seen = new Set<number>();
  return fields.filter((f) => (seen.has(f.node) ? false : (seen.add(f.node), true)));
}

/** What the Fields list shows for a row: the chosen option of a group, "Yes"/"No", or the shown text. */
export function summary(f: Field, all: Field[]): string {
  if (f.kind === "radio") {
    const on = all.find((x) => x.node === f.node && x.checked);
    return on ? on.export : "";
  }
  if (f.kind === "checkbox") return f.checked ? "Yes" : "No";
  if (f.kind === "button" || f.kind === "signature") return "";
  if (f.password && f.value) return "•".repeat(Math.min(f.value.length, 12));
  if (f.kind === "combo" || f.kind === "list") {
    const label = (v: string) => f.options.find((o) => o.export === v)?.label ?? v;
    return f.values.map(label).join(", ");
  }
  return f.shown;
}

const filled = (f: Field, all: Field[]): boolean =>
  f.kind === "radio" ? all.some((x) => x.node === f.node && x.checked) : f.kind === "checkbox" ? f.checked : f.values.some((v) => v !== "");

/** `[filled, fillable]` over whole fields (a radio group counts once). */
export function progress(fields: Field[]): [number, number] {
  const r = rows(fields).filter(isFillable);
  return [r.filter((f) => filled(f, fields)).length, r.length];
}

/** Required fields still empty, in document order. */
export const missingRequired = (fields: Field[]): Field[] => rows(fields).filter((f) => isFillable(f) && f.required && !filled(f, fields));

export const fieldLabel = (f: Field): string => f.tooltip || f.name;

/** "Detect fields would add 6 text fields and 2 check boxes on this page." — said before it is done. */
export function detectSummary(found: Detected[]): string {
  if (found.length === 0) return "No more fields found on this page.";
  const n = (k: FieldKind) => found.filter((d) => d.kind === k).length;
  const one = (k: FieldKind) => (k === "signature" ? "signature field" : KIND_LABEL[k].toLowerCase());
  const parts = (["text", "checkbox", "signature"] as FieldKind[]).filter((k) => n(k) > 0).map((k) => `${n(k)} ${n(k) === 1 ? one(k) : KIND_PLURAL[k]}`);
  const list = parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}` : parts[0];
  return `Detect fields would add ${list} on this page.`;
}

/** Fields found by detection, as what to create. */
export const detectedToNew = (found: Detected[]): NewField[] => found.map((d) => ({ kind: d.kind, rect: d.rect, name: d.name }));

// ---- geometry (page space, y down) -------------------------------------------------------------

export const MIN_SIDE = 6;

export type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";

/** The rectangle after dragging `handle` by `(dx, dy)`, kept inside the page and at least `MIN_SIDE` wide and tall. */
export function resize(r: Rect4, handle: Handle, dx: number, dy: number, page: [number, number]): Rect4 {
  let [x0, y0, x1, y1] = r;
  if (handle.includes("w")) x0 += dx;
  if (handle.includes("e")) x1 += dx;
  if (handle.includes("n")) y0 += dy;
  if (handle.includes("s")) y1 += dy;
  x0 = Math.max(0, Math.min(x0, x1 - MIN_SIDE));
  y0 = Math.max(0, Math.min(y0, y1 - MIN_SIDE));
  x1 = Math.min(page[0], Math.max(x1, x0 + MIN_SIDE));
  y1 = Math.min(page[1], Math.max(y1, y0 + MIN_SIDE));
  return [x0, y0, x1, y1];
}

/** The rectangle moved by `(dx, dy)` and kept on the page. */
export function move(r: Rect4, dx: number, dy: number, page: [number, number]): Rect4 {
  const w = r[2] - r[0];
  const h = r[3] - r[1];
  const x = Math.min(Math.max(0, r[0] + dx), Math.max(0, page[0] - w));
  const y = Math.min(Math.max(0, r[1] + dy), Math.max(0, page[1] - h));
  return [x, y, x + w, y + h];
}

/** The rectangle of a field dragged out from `a` to `b`; a plain click makes the tool's default size. */
export function dragged(kind: FieldKind, a: [number, number], b: [number, number], page: [number, number]): Rect4 {
  const dragging = Math.abs(a[0] - b[0]) >= 4 || Math.abs(a[1] - b[1]) >= 4;
  const [w, h] = DEFAULT_SIZE[kind];
  const r: Rect4 = dragging
    ? [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])]
    : [a[0], a[1], a[0] + w, a[1] + h];
  return move(r, 0, 0, page);
}

export const DEFAULT_SIZE: Record<FieldKind, [number, number]> = {
  text: [150, 22],
  checkbox: [14, 14],
  radio: [14, 14],
  combo: [150, 22],
  list: [150, 64],
  button: [90, 26],
  signature: [190, 44],
};

/** A comb field needs a character limit to draw its cells. */
export const needsMaxLen = (f: Field): boolean => f.kind === "text" && f.comb && f.maxLen === 0;

export const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

/** Calculated fields are filled by the form, not by the person. */
export const isCalculated = (f: Field): boolean => f.calculate.kind !== "none";

/**
 * The widgets a reset button names (all of a field's widgets, and the fields below a parent name); null is the
 * whole form. With `exclude` the button resets everything except the named fields.
 */
export function resetTargets(all: Field[], names: string[], exclude = false): number[] | null {
  if (names.length === 0) return null;
  const named = (f: Field) => names.some((n) => f.name === n || f.name.startsWith(`${n}.`));
  return all.filter((f) => named(f) !== exclude).map((f) => f.xref);
}

/** The text size a field is drawn at on the page: its own, or about 70% of its height (a line of text) when automatic. */
export function fontSize(f: Field): number {
  if (f.fontSize > 0) return f.fontSize;
  const h = f.rect[3] - f.rect[1];
  return f.multiline || f.kind === "list" ? 12 : Math.max(6, Math.min(12, h * 0.7));
}

/** The css colour of an RGB triple in 0..1, or the fallback. */
export function cssColor(c: RGB | null, fallback: string): string {
  return c ? `rgb(${c.map((v) => Math.round(v * 255)).join(" ")})` : fallback;
}

export const FIELD_ICON: Record<FieldKind, "textField" | "checkSquare" | "radio" | "dropdown" | "listBox" | "pushButton" | "signature"> = {
  text: "textField",
  checkbox: "checkSquare",
  radio: "radio",
  combo: "dropdown",
  list: "listBox",
  button: "pushButton",
  signature: "signature",
};

/** A name for the field list: the label people gave it, or its name. */
export const SHORT = (s: string, n = 28): string => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

// ---- choices for the format, validate and calculate settings (src-tauri/src/forms/af.rs) ------------

export const DATE_FORMATS = ["m/d/yy", "mm/dd/yyyy", "mm/dd/yy", "yyyy-mm-dd", "yy-mm-dd", "dd-mmm-yyyy", "d-mmm-yy", "mmm d, yyyy", "mmmm d, yyyy", "mm/yy", "m/d", "m/d/yy h:MM tt", "m/d/yy HH:MM"];
export const TIME_FORMATS = ["HH:MM", "h:MM tt", "HH:MM:ss", "h:MM:ss tt"];
export const SPECIAL_FORMATS = ["Zip code", "Zip code + 4", "Phone number", "Social security number"];
export const SEPARATORS = ["1,234.56", "1234.56", "1.234,56", "1234,56", "1'234.56"];
export const NEGATIVES = ["-1234 (minus)", "-1234 (red minus)", "(1234)", "(1234) in red"];
export const CALC_OPS: { op: CalcOp; label: string }[] = [
  { op: "sum", label: "Sum of" },
  { op: "product", label: "Product of" },
  { op: "average", label: "Average of" },
  { op: "minimum", label: "Smallest of" },
  { op: "maximum", label: "Largest of" },
];
export const FONT_SIZES = [0, 8, 9, 10, 11, 12, 14, 16, 18, 24];

/** `#rrggbb` of an RGB triple in 0..1. */
export const toHex = (c: RGB | null, fallback = "#000000"): string =>
  c ? `#${c.map((v) => Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, "0")).join("")}` : fallback;

/** An RGB triple in 0..1 from `#rrggbb`. */
export const fromHex = (h: string): RGB => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255) as RGB;

/** The text fields a calculation can read (not itself), by name. */
export const calcSources = (all: Field[], self: Field): string[] => [
  ...new Set(all.filter((f) => f.kind === "text" && f.node !== self.node).map((f) => f.name)),
];
