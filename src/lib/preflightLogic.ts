import { type Progress, type Rect4, plural } from "./a11yLogic";

/**
 * Preflight's data and the pure decisions the rail makes from it (src-tauri/src/preflight). No
 * state and no IPC here, so it can be tested alone.
 */

export type { Progress, Rect4 };

export type ProfileId = "pdfa-1b" | "pdfa-2b" | "pdfa-3b" | "pdfa-2u" | "pdfua-1" | "pdfx-4";
export type Status = "passed" | "failed" | "manual" | "info";
export type FixGroup =
  | "structure"
  | "actions"
  | "annotations"
  | "forms"
  | "images"
  | "files"
  | "colour"
  | "fonts"
  | "appearance"
  | "accessibility"
  | "metadata";

export interface Finding {
  page: number | null;
  object: string | null;
  xref: number | null;
  rect: Rect4 | null;
  message: string;
  /** A fix group repairs it. */
  fixable: boolean;
  /** Only a new file can settle it. */
  newFile: boolean;
  /** What it needs when it cannot be fixed: "fonts that aren't installed". */
  note: string | null;
}

export interface RuleResult {
  rule: string;
  name: string;
  group: string;
  clause: string;
  status: Status;
  hint: string;
  fix: FixGroup | null;
  /** PDF/UA: the accessibility checker's own fix for the rule. */
  a11yFix: string | null;
  count: number;
  findings: Finding[];
}

export interface PlanGroup {
  id: FixGroup;
  label: string;
  effect: string;
  issues: number;
  newFileIssues: number;
  defaultOn: boolean;
  disabled: string | null;
}

export interface Plan {
  total: number;
  groups: PlanGroup[];
  remaining: { count: number; text: string }[];
  needsLang: boolean;
}

export interface Report {
  profile: ProfileId;
  pages: number;
  claimed: string | null;
  conforms: boolean;
  results: RuleResult[];
  plan: Plan;
}

export interface Claim {
  label: string;
  profile: ProfileId | null;
}

export interface Converted {
  path: string;
  report: Report;
  fixed: string[];
}

export interface ProfileInfo {
  id: ProfileId;
  label: string;
  /** One line under the select. */
  about: string;
  /** The conversion's name: "Convert to PDF/A-2b". */
  convert: string | null;
}

export const PROFILES: ProfileInfo[] = [
  { id: "pdfa-1b", label: "PDF/A-1b", about: "Long-term archiving on PDF 1.4: no transparency, no layers, no attachments.", convert: "Convert to PDF/A-1b" },
  { id: "pdfa-2b", label: "PDF/A-2b", about: "Long-term archiving on PDF 1.7: transparency and layers are allowed.", convert: "Convert to PDF/A-2b" },
  { id: "pdfa-3b", label: "PDF/A-3b", about: "Like PDF/A-2b, and any file may be attached.", convert: "Convert to PDF/A-3b" },
  { id: "pdfa-2u", label: "PDF/A-2u", about: "Like PDF/A-2b, and every character maps to Unicode.", convert: "Convert to PDF/A-2u" },
  { id: "pdfua-1", label: "PDF/UA-1", about: "Accessible PDF: the accessibility checker's rules and the metadata that names them.", convert: null },
  { id: "pdfx-4", label: "PDF/X-4", about: "Print exchange, basic: output intent, trim box, trapping, metadata.", convert: null },
];

export const profileInfo = (id: ProfileId): ProfileInfo => PROFILES.find((p) => p.id === id) ?? PROFILES[1];
export const isPdfa = (id: ProfileId) => id.startsWith("pdfa-");

export const STATUS_WORD: Record<Status, string> = {
  passed: "Passed",
  failed: "Failed",
  manual: "Needs manual check",
  info: "For information",
};

export interface Counts {
  failed: number;
  manual: number;
  passed: number;
  info: number;
}

export function counts(r: Report): Counts {
  const c: Counts = { failed: 0, manual: 0, passed: 0, info: 0 };
  for (const x of r.results) c[x.status]++;
  return c;
}

/** "5 failed · 1 needs a manual check · 9 passed" */
export function summaryLine(r: Report): string {
  const c = counts(r);
  const parts = [`${c.failed} failed`, `${c.manual} ${c.manual === 1 ? "needs" : "need"} a manual check`, `${c.passed} passed`];
  if (c.info) parts.push(`${c.info} for information`);
  return parts.join(" · ");
}

/** The verdict above the groups. */
export function verdict(r: Report): { ok: boolean; title: string; sub: string } {
  const label = profileInfo(r.profile).label;
  const c = counts(r);
  if (r.conforms) {
    return {
      ok: true,
      title: `No rule fails for ${label}`,
      sub: c.manual > 0 ? `${plural(c.manual, "check")} left for a person to look at.` : "Every rule that Inkwell checks passes.",
    };
  }
  return { ok: false, title: `Doesn't meet ${label}`, sub: `${plural(c.failed, "rule")} failed.` };
}

/** Failed first, then the ones to look at, for information, passed; the engine's order within each. */
const ORDER: Record<Status, number> = { failed: 0, manual: 1, info: 2, passed: 3 };

export interface Group {
  name: string;
  rules: RuleResult[];
  failed: number;
  manual: number;
}

/** Groups in the order the engine lists them. */
export function groupRules(r: Report): Group[] {
  const names: string[] = [];
  for (const x of r.results) if (!names.includes(x.group)) names.push(x.group);
  return names.map((name) => {
    const rules = r.results
      .filter((x) => x.group === name)
      .map((x, i) => ({ x, i }))
      .sort((a, b) => ORDER[a.x.status] - ORDER[b.x.status] || a.i - b.i)
      .map((e) => e.x);
    const n = (s: Status) => rules.filter((x) => x.status === s).length;
    return { name, rules, failed: n("failed"), manual: n("manual") };
  });
}

// ---- the conversion's outcome card -----------------------------------------------------------

/** What the person unticked or ticked, by group; a group not in it keeps its default. */
export type Ticks = Partial<Record<FixGroup, boolean>>;
export type Output = "newFile" | "inPlace";

export const isTicked = (g: PlanGroup, ticks: Ticks): boolean => !g.disabled && (ticks[g.id] ?? g.defaultOn);

export interface Outcome {
  /** Findings the ticked groups settle. */
  fixed: number;
  total: number;
  /** Findings that stay, by what they need ("fonts that aren't installed"). */
  left: { count: number; text: string }[];
  /** Findings in groups the person switched off. */
  skipped: number;
  /** "Fixes 14 of 16 issues; 2 need fonts that aren't installed" */
  line: string;
}

export function outcome(plan: Plan, ticks: Ticks, output: Output): Outcome {
  let fixed = 0;
  let skipped = 0;
  const left: { count: number; text: string }[] = [];
  const add = (count: number, text: string) => {
    if (count <= 0) return;
    const hit = left.find((l) => l.text === text);
    if (hit) hit.count += count;
    else left.push({ count, text });
  };
  for (const g of plan.groups) {
    if (!isTicked(g, ticks)) {
      skipped += g.issues;
      continue;
    }
    if (output === "inPlace") {
      fixed += g.issues - g.newFileIssues;
      add(g.newFileIssues, "a new file");
    } else {
      fixed += g.issues;
    }
  }
  for (const r of plan.remaining) add(r.count, r.text);
  const total = plan.total;
  let line: string;
  if (total === 0) {
    line = "Nothing to fix";
  } else {
    line = fixed === 0 ? "Fixes nothing" : `Fixes ${fixed} of ${plural(total, "issue")}`;
    for (const l of left) line += `; ${l.count} ${l.count === 1 ? "needs" : "need"} ${l.text}`;
    if (skipped > 0) line += `; ${skipped} left as ${skipped === 1 ? "it is" : "they are"}`;
  }
  return { fixed, total, left, skipped, line };
}

/** The groups a conversion runs, in the engine's own order. */
export function ticked(plan: Plan, ticks: Ticks): FixGroup[] {
  return plan.groups.filter((g) => isTicked(g, ticks)).map((g) => g.id);
}

/** "Quarterly report (PDF-A-2b).pdf" */
export function convertedName(stem: string, p: ProfileId): string {
  return `${stem} (${profileInfo(p).label.replace("/", "-")}).pdf`;
}

/** What "Show" and the per-rule Fix button need to decide. */
export function ruleFix(x: RuleResult, plan: Plan): { group: PlanGroup | null; needsNewFile: boolean } | null {
  if (x.status !== "failed" || !x.fix) return null;
  const group = plan.groups.find((g) => g.id === x.fix) ?? null;
  if (!group) return null;
  const fixable = x.findings.filter((f) => f.fixable);
  if (fixable.length === 0) return null;
  return { group, needsNewFile: fixable.every((f) => f.newFile) };
}

/** The accessibility-checker fix a PDF/UA rule's button runs (the rail's `Fix` of the a11y rail). */
export function a11yFixOf(kind: string | null, lang: string): { fix: string; [k: string]: unknown } | null {
  switch (kind) {
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
    default:
      return null;
  }
}

/** "Checking page 5 of 12", "Applying step 2 of 6" */
export function progressText(p: Progress | null, what: "check" | "convert" | "export"): string {
  if (!p) return what === "check" ? "Checking…" : what === "convert" ? "Converting…" : "Writing the report…";
  if (p.stage === "objects") return "Reading the file's objects…";
  if (p.stage === "convert") return `Applying step ${Math.min(p.done + 1, Math.max(1, p.pages))} of ${Math.max(1, p.pages)}`;
  if (p.pages === 0) return "Checking…";
  const verb = p.stage === "survey" ? "Reading" : "Checking";
  return `${verb} page ${Math.min(p.done + 1, p.pages)} of ${p.pages}`;
}

/** 0..1 across one run. */
export function progressFraction(p: Progress | null): number {
  if (!p || p.pages === 0) return 0;
  return Math.min(1, p.done / p.pages);
}

/** The report is out of date for revision `rev` and nothing stands in the way. */
export interface CheckState {
  phase: string;
  checkedRev: number | null;
  checkedProfile: ProfileId | null;
  skipRev: number | null;
  errorRev: number | null;
}

export function autoCheckDue(s: CheckState | undefined, rev: number, profile: ProfileId): boolean {
  if (!s) return true;
  if (s.phase !== "idle") return false;
  if (s.checkedRev === rev && s.checkedProfile === profile) return false;
  return s.skipRev !== rev && s.errorRev !== rev;
}

/** The Document properties line for a file that says it conforms. */
export function claimText(c: Claim): string {
  return `Conforms to ${c.label}`;
}
