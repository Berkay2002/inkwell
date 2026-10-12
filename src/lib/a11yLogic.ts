/**
 * The accessibility checker's data and the pure decisions the rail makes from it
 * (src-tauri/src/a11y). No state and no IPC here, so it can be tested alone.
 */

import type { A11yMark, A11yMarks } from "./recent";

export type Status = "passed" | "failed" | "manual" | "skipped";
export type { A11yMark, A11yMarks };
/** `[x0, y0, x1, y1]` in page points from the top left (the viewer's page space). */
export type Rect4 = [number, number, number, number];

export interface Finding {
  /** 0-based; null for the document as a whole. */
  page: number | null;
  object: string | null;
  xref: number | null;
  /** Null for text and for things without a place on the page. */
  rect: Rect4 | null;
  message: string;
}

export type FixKind = "title" | "displayDocTitle" | "language" | "tabOrder" | "fieldTooltips" | "figureAlt" | "autotag" | "metadata" | "parentTree";

export interface RuleResult {
  rule: string;
  name: string;
  group: string;
  status: Status;
  hint: string;
  fix: FixKind | null;
  findings: Finding[];
  /** Set by `applyOverrides` when the person decided this rule by hand; the checker's own result is in `auto`. */
  mark?: A11yMark;
  /** What the checker said before the mark. */
  auto?: Status;
}

export interface Report {
  pages: number;
  tagged: boolean;
  /** A certification (/DocMDP): the engine refuses every edit. */
  certified: boolean;
  /** Signed: edits are saved after the signed bytes. */
  signed: boolean;
  results: RuleResult[];
}

export const CERTIFIED_WHY = "This document is certified: changing its tags or reading order would void the certification, so Inkwell leaves them as they are.";
export const SIGNED_NOTE =
  "This document is signed. Edits are saved after the signed part, so the signature still covers the original; readers will show that the document changed after it was signed. Anything added and then removed in the same session stays in the saved file.";

export interface Progress {
  done: number;
  pages: number;
  /** `check`; for Autotag `read` and then `tag`. */
  stage: string;
}

export interface Preview {
  untagged: boolean;
  pages: number;
  figures: number;
  links: number;
  fields: number;
}

/** What `a11y_fix` takes (serde tag "fix"). */
export type Fix =
  | { fix: "title"; title: string | null }
  | { fix: "language"; lang: string }
  | { fix: "displayDocTitle" }
  | { fix: "tabOrder" }
  | { fix: "fieldTooltips" }
  | { fix: "figureAlt"; xref: number; alt: string }
  | { fix: "autotag" }
  | { fix: "metadata" }
  | { fix: "parentTree" }
  | { fix: "many"; fixes: Fix[] };

export const GROUPS = ["Document", "Page content", "Forms", "Alternate text", "Tables", "Lists", "Headings"] as const;

/** Fixes that are safe to run without asking what to write: everything except figure text and Autotag. */
const SAFE: FixKind[] = ["title", "displayDocTitle", "language", "tabOrder", "fieldTooltips"];

export const STATUS_WORD: Record<Status, string> = {
  passed: "Passed",
  failed: "Failed",
  manual: "Needs manual check",
  skipped: "Skipped",
};

/** "Passed manually" for a rule marked passed, "Skipped" for one marked skipped, else the status word. */
export const statusWord = (x: RuleResult) => (x.mark === "passed" ? "Passed manually" : STATUS_WORD[x.status]);

/**
 * The report with the person's marks applied: a marked rule reads passed or skipped whatever the checker found, so
 * the groups, counts and "Fix all" all follow. Pure: the document and the engine's report are not touched.
 */
export function applyOverrides(r: Report, marks: A11yMarks): Report {
  if (!Object.keys(marks).length) return r;
  return {
    ...r,
    results: r.results.map((x) => {
      const m = marks[x.rule];
      return m ? { ...x, status: m === "passed" ? "passed" : "skipped", mark: m, auto: x.status } : x;
    }),
  };
}

/** The marks that name a rule of this report, as the report export takes them. */
export const marksIn = (r: Report, marks: A11yMarks): A11yMarks =>
  Object.fromEntries(r.results.filter((x) => marks[x.rule]).map((x) => [x.rule, marks[x.rule]]));

export const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

export interface Counts {
  failed: number;
  manual: number;
  passed: number;
  /** Rules the person marked passed (not in `passed`). */
  passedManually: number;
  skipped: number;
}

export function counts(r: Report): Counts {
  const c: Counts = { failed: 0, manual: 0, passed: 0, passedManually: 0, skipped: 0 };
  for (const x of r.results) {
    if (x.mark === "passed") c.passedManually++;
    else c[x.status]++;
  }
  return c;
}

/** "5 failed · 1 needs a manual check · 9 passed · 2 passed manually" */
export function summaryLine(r: Report): string {
  const c = counts(r);
  const parts = [`${c.failed} failed`, `${c.manual} ${c.manual === 1 ? "needs" : "need"} a manual check`, `${c.passed} passed`];
  if (c.passedManually) parts.push(`${c.passedManually} passed manually`);
  if (c.skipped) parts.push(`${c.skipped} skipped`);
  return parts.join(" · ");
}

/** Failed first, then the ones to check by eye, passed, skipped; the engine's order within each. */
const ORDER: Record<Status, number> = { failed: 0, manual: 1, passed: 2, skipped: 3 };

export interface Group {
  name: string;
  rules: RuleResult[];
  failed: number;
  manual: number;
}

export function groupRules(r: Report): Group[] {
  return GROUPS.map((name) => {
    const rules = r.results
      .filter((x) => x.group === name)
      .map((x, i) => ({ x, i }))
      .sort((a, b) => ORDER[a.x.status] - ORDER[b.x.status] || a.i - b.i)
      .map((e) => e.x);
    const n = (s: Status) => rules.filter((x) => x.status === s).length;
    return { name, rules, failed: n("failed"), manual: n("manual") };
  }).filter((g) => g.rules.length > 0);
}

/** The base name of a file without its extension: what the Title fix uses. */
export const stem = (name: string) => name.replace(/\.pdf$/i, "");

export const LANGS = [
  "en-US", "en-GB", "sv-SE", "da-DK", "nb-NO", "fi-FI", "de-DE", "fr-FR", "es-ES", "it-IT",
  "nl-NL", "pt-PT", "pl-PL", "tr-TR", "ja-JP", "zh-CN",
];

/** "English (en-US)" */
export function langLabel(tag: string): string {
  try {
    const name = new Intl.DisplayNames(["en"], { type: "language" }).of(tag.split("-")[0]);
    return name ? `${name} (${tag})` : tag;
  } catch {
    return tag;
  }
}

/** The list entry that best matches the system language (en-US when nothing does). */
export function suggestLang(system: string | undefined): string {
  const s = (system ?? "").trim();
  if (!s) return "en-US";
  const hit = LANGS.find((l) => l.toLowerCase() === s.toLowerCase()) ?? LANGS.find((l) => l.split("-")[0].toLowerCase() === s.split("-")[0].toLowerCase());
  return hit ?? "en-US";
}

/** The fix a failed rule's one-click button runs; null for rules that need a person or have none. */
export function fixFor(x: RuleResult, lang: string): Fix | null {
  switch (x.fix) {
    case "title":
      return { fix: "title", title: null };
    case "displayDocTitle":
      return { fix: "displayDocTitle" };
    case "language":
      return { fix: "language", lang };
    case "tabOrder":
      return { fix: "tabOrder" };
    case "fieldTooltips":
      return { fix: "fieldTooltips" };
    case "autotag":
      return { fix: "autotag" };
    case "metadata":
      return { fix: "metadata" };
    case "parentTree":
      return { fix: "parentTree" };
    default:
      return null;
  }
}

/** One sentence on what a rule's Fix does, shown before it is pressed. */
export function fixEffect(x: RuleResult, file: string, lang: string): string | null {
  switch (x.fix) {
    case "title":
      return `Sets the title to “${stem(file)}”.`;
    case "displayDocTitle":
      return "Shows the title, not the file name, in the title bar.";
    case "language":
      return `Sets the language to ${langLabel(lang)}.`;
    case "tabOrder":
      return "Tabs through links and fields in structure order.";
    case "fieldTooltips":
      return "Gives each field a tooltip made from its name.";
    case "metadata":
      return "Adds the PDF/UA identification and the title to the XMP metadata.";
    case "parentTree":
      return "Rebuilds the links between the tags and the page content from the tag tree.";
    default:
      return null;
  }
}

/** The failed rules "Fix all safe issues" settles. */
export function safeRules(r: Report): RuleResult[] {
  return r.results.filter((x) => x.status === "failed" && x.fix !== null && SAFE.includes(x.fix));
}

export function safeFixes(r: Report, lang: string): Fix[] {
  return safeRules(r)
    .map((x) => fixFor(x, lang))
    .filter((f): f is Fix => f !== null);
}

/** After "Fix all safe issues": how many of the rules pass. */
export function afterFixAll(r: Report): { pass: number; of: number } {
  const c = counts(r);
  return { pass: c.passed + c.passedManually + safeRules(r).length, of: r.results.length };
}

/** The label of the footer's primary button and the line under it. */
export function fixAllText(r: Report): { label: string; outcome: string } | null {
  const n = safeRules(r).length;
  if (n === 0) return null;
  const a = afterFixAll(r);
  return { label: `Fix all safe issues (${n})`, outcome: `Fix ${plural(n, "issue")} · after this ${a.pass} of ${a.of} pass` };
}

/** The outcome of Autotag, said before it runs. */
export function autotagText(p: Preview): string {
  const parts = [`Adds tags to ${plural(p.pages, "page")}; reading order from layout.`];
  if (p.figures > 0) parts.push(`${plural(p.figures, "image")} get${p.figures === 1 ? "s" : ""} a Figure tag without alternate text.`);
  parts.push("One undo step.");
  return parts.join(" ");
}

/** "Checking page 5 of 12", "Tagging page 3 of 8" */
export function progressText(p: Progress | null, what: "check" | "autotag" | "export"): string {
  if (!p || p.pages === 0) return what === "check" ? "Checking…" : what === "autotag" ? "Tagging…" : "Writing the report…";
  if (what === "autotag" && p.stage === "read") return `Reading page ${Math.min(p.done + 1, p.pages)} of ${p.pages}`;
  const verb = what === "autotag" ? "Tagging" : "Checking";
  return `${verb} page ${Math.min(p.done + 1, p.pages)} of ${p.pages}`;
}

/** 0..1 across the stages of one run (Autotag reads every page, then tags every page). */
export function progressFraction(p: Progress | null, what: "check" | "autotag" | "export"): number {
  if (!p || p.pages === 0) return 0;
  const f = Math.min(1, p.done / p.pages);
  if (what === "autotag") return p.stage === "read" ? f / 2 : 0.5 + f / 2;
  return f;
}

/** The finding rows that make sense to show: those with their own place first, then the summary line. */
export function isOverflowNote(f: Finding): boolean {
  return f.page === null && f.xref === null && /^\.{3}\s*and\s+\d+\s+more/i.test(f.message);
}

/** A finding the alt-text field can answer: a figure with its own object. */
export function takesAlt(x: RuleResult, f: Finding): f is Finding & { xref: number } {
  return x.fix === "figureAlt" && x.status === "failed" && f.xref !== null;
}

/** "p. 5" or "Document". */
export const whereText = (f: Finding) => (f.page === null ? "Document" : `p. ${f.page + 1}`);

/** What the rail needs to decide whether the open document is to be read again by itself. */
export interface CheckState {
  phase: string;
  checkedRev: number | null;
  /** The revision whose check was cancelled. */
  skipRev: number | null;
  /** The revision a check failed on. */
  errorRev: number | null;
}

/**
 * True when the report is out of date for revision `rev` and nothing stands in the way: idle, and the
 * check for this very revision was not cancelled or did not fail (Check again, or an edit, lifts that).
 * An error from a fix or an export does not hold back a check after an edit.
 */
export function autoCheckDue(s: CheckState | undefined, rev: number): boolean {
  if (!s) return true;
  return s.phase === "idle" && s.checkedRev !== rev && s.skipRev !== rev && s.errorRev !== rev;
}

