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
  | { kind: "mask"; value: string }
  /** "Custom": the format script (`/AA /F`) and the keystroke script (`/AA /K`); either may be empty. */
  | { kind: "custom"; value: { format: string; keystroke: string } };
export type Validate =
  | { kind: "none" }
  | { kind: "range"; min: number | null; max: number | null }
  | { kind: "custom"; source: string };
export type CalcOp = "sum" | "product" | "average" | "minimum" | "maximum";
export type Calculate =
  | { kind: "none" }
  | { kind: "simple"; value: { op: CalcOp; fields: string[] } }
  | { kind: "notation"; value: string }
  | { kind: "custom"; value: string };
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
  /** The `node` of each calculated field, in the order the calculations run. */
  calcOrder: number[];
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

// ---- scripts: links, prompts, calculation order -----------------------------------------------------

/** Bidirectional controls (embeddings, overrides, isolates): text that uses them can show a link in an order it is not in. */
const BIDI = /[\u202A-\u202E\u2066-\u2069]/g;

/** `text` without bidirectional controls (shown in dialogs where a script's text must not reorder what follows). */
export const stripBidi = (text: string): string => text.replace(BIDI, "");

/**
 * A link a script asked to open, or null when it may not be offered: only http, https and mailto, with no
 * credentials in the address. Returned in its normalised form.
 */
export function safeLink(url: string): string | null {
  const t = stripBidi(url).trim();
  if (t === "" || t.length > 4096) return null;
  let u: URL;
  try {
    u = new URL(t);
  } catch {
    return null;
  }
  if (!["http:", "https:", "mailto:"].includes(u.protocol)) return null;
  if (u.username || u.password) return null;
  return u.href;
}

/** The most prompts that wait for the person at once; a script that raises more than this is flooding. */
export const MAX_PROMPTS = 20;

/** `queue` with `incoming` after it, without going past [`MAX_PROMPTS`]; `dropped` is how many did not fit. */
export function capPrompts<T>(queue: T[], incoming: T[], max = MAX_PROMPTS): { queue: T[]; dropped: number } {
  const room = Math.max(0, max - queue.length);
  return { queue: [...queue, ...incoming.slice(0, room)], dropped: Math.max(0, incoming.length - room) };
}

/**
 * Save from inside a field: the typed value is committed first (it would only commit when the field loses focus),
 * then `run` starts (the save). A value that is unchanged is not committed again; a refused one still lets the save run.
 */
export async function commitThenRun(typed: string, saved: string, commit: (v: string) => Promise<unknown>, run: () => void): Promise<void> {
  if (typed !== saved) await commit(typed);
  run();
}

/** What the UI asks the person about: a script's message, or a link or a submit it wants. Never acted on without them. */
export type ScriptPrompt =
  | { kind: "alert"; title: string; message: string; field?: string }
  | { kind: "link"; url: string }
  | { kind: "submit"; url: string };

/** What a script asked that needs no question: the UI does these straight away. */
export type ScriptAction =
  | { kind: "reset"; fields: string[] }
  | { kind: "goToPage"; page: number }
  | { kind: "focus"; field: string }
  | { kind: "print" };

export interface ScriptEffects {
  prompts: ScriptPrompt[];
  actions: ScriptAction[];
}

interface RawAlert {
  message: string;
  title?: string;
  field?: string;
}
type RawRequest =
  | { kind: "reset"; fields: string[] }
  | { kind: "print" }
  | { kind: "goToPage"; page: number }
  | { kind: "launchUrl"; url: string }
  | { kind: "submit"; url: string }
  | { kind: "focus"; field: string }
  | { kind: "beep" };

/** Sorts the alerts and requests of one fill: alerts first (in order), then links and submits to ask about, and the rest to do. */
export function effectsOf(alerts: RawAlert[], requests: RawRequest[]): ScriptEffects {
  const prompts: ScriptPrompt[] = alerts.map((a) => ({ kind: "alert", title: stripBidi(a.title ?? "").trim() || "Inkwell", message: stripBidi(a.message), field: a.field }));
  const actions: ScriptAction[] = [];
  for (const r of requests) {
    switch (r.kind) {
      case "launchUrl": {
        const url = safeLink(r.url);
        if (url) prompts.push({ kind: "link", url });
        break;
      }
      case "submit": {
        const url = safeLink(r.url);
        if (url) prompts.push({ kind: "submit", url });
        break;
      }
      case "reset":
        actions.push({ kind: "reset", fields: r.fields });
        break;
      case "goToPage":
        actions.push({ kind: "goToPage", page: r.page });
        break;
      case "focus":
        actions.push({ kind: "focus", field: r.field });
        break;
      case "print":
        actions.push({ kind: "print" });
        break;
      case "beep":
        // Nothing to show: a beep has no place in a quiet window.
        break;
    }
  }
  return { prompts, actions };
}

/** The first widget of each field in `order` (the calculation order), skipping nodes that are gone. */
export function calcFields(all: Field[], order: number[]): Field[] {
  return order.flatMap((n) => {
    const f = all.find((x) => x.node === n);
    return f ? [f] : [];
  });
}

/** `list` with the item at `from` moved to `to` (clamped to the list). */
export function moved<T>(list: T[], from: number, to: number): T[] {
  if (from < 0 || from >= list.length) return list;
  const t = Math.max(0, Math.min(list.length - 1, to));
  const next = list.slice();
  const [x] = next.splice(from, 1);
  next.splice(t, 0, x);
  return next;
}

/** How many lines of the script log are worth a badge: warnings and errors. */
export const logCount = (log: { level: string }[]): number => log.filter((l) => l.level !== "info").length;

// ---- layout tools: selection, availability, nudge, rename --------------------------------------

/** A picked widget. The last one picked is the anchor the align and size tools follow, as in Acrobat. */
export interface Pick {
  page: number;
  xref: number;
}

export const samePick = (a: Pick, b: Pick): boolean => a.page === b.page && a.xref === b.xref;
export const anchorOf = (picked: Pick[]): Pick | null => picked[picked.length - 1] ?? null;

/**
 * The picks a press on `pressed` drags along: the picked fields of its page when it is one of two or more picked fields
 * (a selection across pages moves one page at a time), else null (a press on a field that is not picked drags just it).
 */
export function groupPicks(picked: Pick[], pressed: Pick): Pick[] | null {
  if (picked.length < 2 || !picked.some((p) => samePick(p, pressed))) return null;
  return picked.filter((p) => p.page === pressed.page);
}

/**
 * How far a group drag moves, for a pointer that went `dx`, `dy` (page space): the engine's nudge rule, so the preview
 * is what lands. The group moves as one block that stays inside the page; a block already past an edge may not go
 * further past it.
 */
export function groupDelta(rects: Rect4[], dx: number, dy: number, page: [number, number]): [number, number] {
  if (rects.length === 0) return [0, 0];
  const g: Rect4 = [
    Math.min(...rects.map((r) => r[0])),
    Math.min(...rects.map((r) => r[1])),
    Math.max(...rects.map((r) => r[2])),
    Math.max(...rects.map((r) => r[3])),
  ];
  const ddx = dx < 0 ? Math.max(dx, Math.min(0 - g[0], 0)) : Math.min(dx, Math.max(page[0] - g[2], 0));
  const ddy = dy < 0 ? Math.max(dy, Math.min(0 - g[1], 0)) : Math.min(dy, Math.max(page[1] - g[3], 0));
  return [ddx, ddy];
}

/**
 * What releasing a group drag does. `d` is the drag as it stood (null when it was cancelled with Esc or by the system:
 * nothing is sent). A click without a drag narrows the selection to the pressed field; a drag sends one nudge of the
 * clamped distance (none when the block could not move at all).
 */
export type DragEnd = { kind: "none" } | { kind: "select" } | { kind: "nudge"; by: [number, number] };
export function groupDragEnd(d: { moved: boolean; delta: [number, number] } | null): DragEnd {
  if (!d) return { kind: "none" };
  if (!d.moved) return { kind: "select" };
  return d.delta[0] === 0 && d.delta[1] === 0 ? { kind: "none" } : { kind: "nudge", by: d.delta };
}

/** A click picks one; Ctrl, Cmd or Shift adds the clicked field (it becomes the anchor), or takes it out if it was picked. */
export function nextPicks(picked: Pick[], ref: Pick, additive: boolean): Pick[] {
  if (!additive) return [ref];
  return picked.some((p) => samePick(p, ref)) ? picked.filter((p) => !samePick(p, ref)) : [...picked, ref];
}

export function normBox(a: [number, number], b: [number, number]): Rect4 {
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
}

/** The widgets of `page` the box touches, in reading order (rows top to bottom), so the anchor is the last one. Hidden ones are not hit. */
export function marqueeHits(fields: Field[], page: number, box: Rect4): Pick[] {
  const [x0, y0, x1, y1] = box;
  return fields
    .filter((f) => f.page === page && !f.hidden && f.rect[0] < x1 && f.rect[2] > x0 && f.rect[1] < y1 && f.rect[3] > y0)
    .sort((a, b) => Math.round(a.rect[1] / 8) - Math.round(b.rect[1] / 8) || a.rect[0] - b.rect[0])
    .map((f) => ({ page: f.page, xref: f.xref }));
}

export type LayoutOp = "align" | "center" | "distribute" | "size" | "nudge" | "duplicate" | "copies" | "rename";

export const CERTIFIED_REASON = "This document is certified, and certification does not allow changes to its form fields.";

/** Why a layout tool is off for this selection (shown on the disabled control), or null when it can run. */
export function layoutReason(op: LayoutOp, picked: Pick[], fields: Field[], certified: boolean): string | null {
  if (certified) return CERTIFIED_REASON;
  const of = picked.map((p) => fields.find((f) => samePick(f, p))).filter((f): f is Field => !!f);
  const n = of.length;
  const onePage = of.every((f) => f.page === of[0].page);
  switch (op) {
    case "align":
    case "size":
      if (n < 2) return "Select two or more fields. The last one picked is the anchor.";
      return onePage ? null : "Select fields on one page.";
    case "distribute":
      if (n < 3) return "Select three or more fields.";
      return onePage ? null : "Select fields on one page.";
    case "center":
    case "nudge":
      return n < 1 ? "Select a field." : null;
    case "rename":
      return n === 1 ? null : "Select one field.";
    case "duplicate":
    case "copies": {
      if (n !== 1) return "Select one field.";
      const f = of[0];
      if (f.kind === "radio") return "A radio button belongs to its group: draw another one with the radio tool.";
      if (f.kind === "signature") return "A signature field is not copied.";
      if (op === "duplicate") return null;
      if (fields.filter((x) => x.node === f.node).length > 1) return "This field has several widgets. Copy a field that has one.";
      return isCalculated(f) ? "A calculated field is not copied: its calculation would not follow." : null;
    }
  }
}

/** The arrow keys' step on the page: 1 point, 10 with Shift. */
export function nudgeDelta(key: string, shift: boolean): [number, number] | null {
  const s = shift ? 10 : 1;
  switch (key) {
    case "ArrowLeft":
      return [-s, 0];
    case "ArrowRight":
      return [s, 0];
    case "ArrowUp":
      return [0, -s];
    case "ArrowDown":
      return [0, s];
    default:
      return null;
  }
}

/** Arrow-key presses collected until the burst ends (key up), then sent as one move: one undo step. */
export class NudgeBatch {
  private dx = 0;
  private dy = 0;
  /** Adds a key press; false when it is not an arrow key. */
  push(key: string, shift: boolean): boolean {
    const d = nudgeDelta(key, shift);
    if (!d) return false;
    this.dx += d[0];
    this.dy += d[1];
    return true;
  }
  /** What was collected (null when nothing, or when it cancelled out); starts over. */
  take(): [number, number] | null {
    const r: [number, number] = [this.dx, this.dy];
    this.dx = 0;
    this.dy = 0;
    return r[0] === 0 && r[1] === 0 ? null : r;
  }
}

export type RenameCheck = { kind: "same" } | { kind: "ok" } | { kind: "merge"; target: Field } | { kind: "error"; message: string };

/** What renaming `f` to `to` (a full, dotted name) would do, decided the way the engine decides it, so a merge is asked about first. */
export function renameCheck(fields: Field[], f: Field, to: string): RenameCheck {
  const name = to.trim();
  if (name === f.name) return { kind: "same" };
  if (name === "" || name.split(".").some((p) => p.trim() === "")) {
    return { kind: "error", message: "A field needs a name, and no part of a dotted name may be empty." };
  }
  for (const o of fields) {
    if (o.node === f.node) continue;
    if (o.name === name) {
      if (o.kind !== f.kind) return { kind: "error", message: `A ${KIND_LABEL[o.kind].toLowerCase()} named ${name} already exists, so this ${KIND_LABEL[f.kind].toLowerCase()} cannot take that name or merge into it.` };
      if (f.kind === "radio" || f.kind === "signature") return { kind: "error", message: `A field named ${name} already exists. Radio groups and signature fields are not merged.` };
      return { kind: "merge", target: o };
    }
    if (o.name.startsWith(`${name}.`)) return { kind: "error", message: `${name} is already the name of a group of fields.` };
    if (name.startsWith(`${o.name}.`)) return { kind: "error", message: `${o.name} is a field, so no field can be named below it.` };
  }
  return { kind: "ok" };
}
