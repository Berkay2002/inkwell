import { beforeEach, describe, expect, it, vi } from "vitest";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage: ((m: unknown) => void) | null = null;
  },
}));
vi.mock("./edit", () => ({ applyState: vi.fn() }));
vi.mock("./api", () => ({ pickSave: vi.fn(), toolCancel: vi.fn(() => Promise.resolve()) }));

import { invoke } from "@tauri-apps/api/core";
import { useStore } from "../store";
import { usePreflight } from "./preflight";
import { applyState } from "./edit";
import { pickSave } from "./api";
import {
  type Finding,
  type Plan,
  type PlanGroup,
  type Report,
  type RuleResult,
  a11yFixOf,
  autoCheckDue,
  claimText,
  convertedName,
  counts,
  groupRules,
  groupsFor,
  isTicked,
  outcome,
  progressFraction,
  progressText,
  ruleFix,
  summaryLine,
  ticked,
  verdict,
  verifiedText,
  willIdentify,
} from "./preflightLogic";

const inv = vi.mocked(invoke);
const fnd = (over: Partial<Finding> = {}): Finding => ({
  page: 0,
  object: null,
  xref: null,
  rect: null,
  message: "m",
  fixable: true,
  newFile: false,
  note: null,
  ...over,
});
const rule = (over: Partial<RuleResult>): RuleResult => ({
  rule: "x",
  name: "X",
  group: "Fonts",
  clause: "6.2.11.4",
  status: "passed",
  hint: "",
  fix: null,
  a11yFix: null,
  count: 0,
  findings: [],
  ...over,
});
const grp = (id: PlanGroup["id"], issues: number, over: Partial<PlanGroup> = {}): PlanGroup => ({
  id,
  label: id,
  effect: "",
  issues,
  newFileIssues: 0,
  defaultOn: true,
  disabled: null,
  ...over,
});
const PLAN: Plan = {
  total: 16,
  groups: [
    grp("actions", 2),
    grp("files", 1, { newFileIssues: 1 }),
    grp("colour", 3),
    grp("fonts", 8),
    grp("appearance", 1, { defaultOn: false }),
    grp("metadata", 1),
  ],
  remaining: [{ count: 2, text: "fonts that aren't installed" }],
  needsLang: false,
};
const report = (results: RuleResult[], plan: Plan = PLAN, conforms = false): Report => ({
  profile: "pdfa-2b",
  pages: 12,
  claimed: null,
  conforms,
  results,
  plan,
});

describe("what the new file is named by", () => {
  const clean: Plan = { total: 4, groups: [grp("colour", 3), grp("metadata", 1)], remaining: [], needsLang: false };
  it("expects identification only when nothing is left, skipped or waiting for a person", () => {
    expect(willIdentify(report([rule({ status: "failed" })], clean), {}, "newFile")).toBe(true);
    expect(willIdentify(report([rule({ status: "failed" })], PLAN), {}, "newFile")).toBe(false);
    expect(willIdentify(report([rule({ status: "failed" })], clean), { colour: false }, "newFile")).toBe(false);
    expect(willIdentify(report([rule({ status: "failed" }), rule({ rule: "m", status: "manual" })], clean), {}, "newFile")).toBe(false);
    // The other profiles are not held back by a manual check.
    const ua = { ...report([rule({ status: "failed" }), rule({ rule: "m", status: "manual" })], clean), profile: "pdfua-1" as const };
    expect(willIdentify(ua, {}, "newFile")).toBe(true);
  });
});

describe("report logic", () => {
  const r = report([
    rule({ rule: "a", name: "A", group: "Fonts", status: "passed" }),
    rule({ rule: "b", name: "B", group: "Fonts", status: "failed", count: 1, findings: [fnd()] }),
    rule({ rule: "c", name: "C", group: "Colour", status: "manual" }),
    rule({ rule: "d", name: "D", group: "Colour", status: "info" }),
  ]);

  it("counts, words and groups with failed rules first, in the engine's group order", () => {
    expect(counts(r)).toEqual({ failed: 1, manual: 1, passed: 1, info: 1 });
    expect(summaryLine(r)).toBe("1 failed · 1 needs a manual check · 1 passed · 1 for information");
    const g = groupRules(r);
    expect(g.map((x) => x.name)).toEqual(["Fonts", "Colour"]);
    expect(g[0].rules.map((x) => x.rule)).toEqual(["b", "a"]);
    expect(g[1].rules.map((x) => x.rule)).toEqual(["c", "d"]);
  });

  it("says whether any rule fails, naming the profile", () => {
    expect(verdict(r)).toEqual({ ok: false, title: "Doesn't meet PDF/A-2b", sub: "1 rule failed." });
    const ok = verdict(report([rule({}), rule({ rule: "m", status: "manual" })], PLAN, true));
    expect(ok.ok).toBe(true);
    expect(ok.title).toBe("No rule fails for PDF/A-2b");
    expect(ok.sub).toBe("1 check left for a person to look at.");
  });
});

describe("the outcome line", () => {
  it("predicts the new-file outcome from the default ticks", () => {
    // Appearance is not ticked by default, so its one issue stays.
    const o = outcome(PLAN, {}, "newFile");
    expect(o.fixed).toBe(15);
    expect(o.skipped).toBe(1);
    expect(o.line).toBe("Fixes 15 of 16 issues; 2 need fonts that aren't installed; 1 left as it is");
  });

  it("updates live when a group is unticked", () => {
    const o = outcome(PLAN, { fonts: false }, "newFile");
    expect(o.fixed).toBe(7);
    expect(o.skipped).toBe(9);
    expect(o.line).toBe("Fixes 7 of 16 issues; 2 need fonts that aren't installed; 9 left as they are");
    // switching Appearance on moves its issue from "left" to "fixed"
    expect(outcome(PLAN, { appearance: true }, "newFile").fixed).toBe(16);
  });

  it("in place, what only a new file can settle stays and says so", () => {
    const o = outcome(PLAN, {}, "inPlace");
    expect(o.fixed).toBe(14);
    expect(o.left).toEqual([
      { count: 1, text: "a new file" },
      { count: 2, text: "fonts that aren't installed" },
    ]);
    expect(o.line).toBe("Fixes 14 of 16 issues; 1 needs a new file; 2 need fonts that aren't installed; 1 left as it is");
  });

  it("a disabled group is never ticked and its issues are not counted as fixed", () => {
    const g = grp("fonts", 4, { disabled: "Encrypted" });
    expect(isTicked(g, { fonts: true })).toBe(false);
    const plan: Plan = { ...PLAN, total: 4, groups: [g], remaining: [] };
    expect(outcome(plan, {}, "newFile").line).toBe("Fixes nothing; 4 left as they are");
    expect(ticked(PLAN, { fonts: false, appearance: true })).toEqual(["actions", "files", "colour", "appearance", "metadata"]);
  });

  it("has nothing to say when nothing is wrong", () => {
    expect(outcome({ total: 0, groups: [], remaining: [], needsLang: false }, {}, "newFile").line).toBe("Nothing to fix");
  });
});

describe("rule fixes and wording", () => {
  it("offers a fix for a failed rule with fixable findings, noting new-file-only ones", () => {
    const x = rule({ status: "failed", fix: "actions", findings: [fnd()] });
    expect(ruleFix(x, PLAN)?.needsNewFile).toBe(false);
    const nf = rule({ status: "failed", fix: "files", findings: [fnd({ newFile: true })] });
    expect(ruleFix(nf, PLAN)?.needsNewFile).toBe(true);
    expect(ruleFix(rule({ status: "failed", fix: "fonts", findings: [fnd({ fixable: false })] }), PLAN)).toBeNull();
    expect(ruleFix(rule({ status: "passed", fix: "fonts" }), PLAN)).toBeNull();
  });

  it("maps a PDF/UA rule to the accessibility checker's fix", () => {
    expect(a11yFixOf("language", "sv-SE")).toEqual({ fix: "language", lang: "sv-SE" });
    expect(a11yFixOf("title", "en-US")).toEqual({ fix: "title", title: null });
    expect(a11yFixOf("autotag", "en-US")).toBeNull();
    expect(a11yFixOf(null, "en-US")).toBeNull();
  });

  it("names the new file and reads the progress", () => {
    expect(convertedName("Annual report", "pdfa-2b")).toBe("Annual report (PDF-A-2b).pdf");
    // A file that will not say it conforms is not named as if it did.
    expect(convertedName("Annual report", "pdfa-2b", false)).toBe("Annual report (converted).pdf");
    expect(progressText({ done: 4, pages: 8, stage: "check" }, "check")).toBe("Checking page 5 of 8");
    expect(progressText({ done: 0, pages: 8, stage: "survey" }, "check")).toBe("Reading page 1 of 8");
    expect(progressText({ done: 1, pages: 6, stage: "convert" }, "convert")).toBe("Applying step 2 of 6");
    expect(progressText(null, "export")).toBe("Writing the report…");
    expect(progressFraction({ done: 2, pages: 8, stage: "check" })).toBe(0.25);
  });

  it("re-checks for a new revision or profile, not after a cancel or an error at this revision", () => {
    const s = { phase: "idle", checkedRev: 3, checkedProfile: "pdfa-2b" as const, skipRev: null, errorRev: null };
    expect(autoCheckDue(undefined, 3, "pdfa-2b")).toBe(true);
    expect(autoCheckDue(s, 3, "pdfa-2b")).toBe(false);
    expect(autoCheckDue(s, 4, "pdfa-2b")).toBe(true);
    expect(autoCheckDue(s, 3, "pdfua-1")).toBe(true);
    expect(autoCheckDue({ ...s, skipRev: 4 }, 4, "pdfa-2b")).toBe(false);
    expect(autoCheckDue({ ...s, errorRev: 4 }, 4, "pdfa-2b")).toBe(false);
    expect(autoCheckDue({ ...s, phase: "checking" }, 4, "pdfa-2b")).toBe(false);
  });
});

const state = (rev: number) => ({ rev, pageCount: 2, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Convert to PDF/A-2b", redo: null });
const p = () => usePreflight.getState();

function reply(map: Record<string, unknown | (() => unknown)>) {
  inv.mockImplementation((cmd: string) => {
    if (!(cmd in map)) return Promise.reject(`unexpected ${cmd}`);
    const v = map[cmd];
    return typeof v === "function" ? (v as () => Promise<unknown>)() : Promise.resolve(v);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  usePreflight.setState({ docs: {}, open: false, profile: "pdfa-2b" });
  useStore.setState({
    docs: { 1: { id: 1, name: "Annual report 2026.pdf", rev: 3, pageCount: 12, currentPage: 0, sizes: [], nav: null } as never },
    order: [1],
    active: 1,
  });
});

const failing = report([rule({ rule: "fonts-embedded", status: "failed", fix: "fonts", count: 1, findings: [fnd()] })]);

describe("what the file says and what Inkwell checked", () => {
  it("words the file's claim as a claim, and Verify's answer as no failures in these checks", () => {
    expect(claimText({ label: "PDF/A-2b", profile: "pdfa-2b" })).toBe("Says it is PDF/A-2b");
    expect(claimText({ label: "PDF/A-2b", profile: "pdfa-2b" })).not.toMatch(/conform/i);
    expect(verifiedText("PDF/A-2b")).toBe("No failures in Inkwell's checks for PDF/A-2b (not a full validation).");
  });

  it("the identification fix runs the ticked groups, with Metadata among them; other fixes run their own group", () => {
    const id = rule({ rule: "xmp-id", status: "failed", fix: "metadata", findings: [fnd()] });
    expect(groupsFor(id, PLAN, {})).toEqual(["actions", "files", "colour", "fonts", "metadata"]);
    // Metadata unticked: still part of the identification fix, once.
    expect(groupsFor(id, PLAN, { metadata: false })).toEqual(["actions", "files", "colour", "fonts", "metadata"]);
    expect(groupsFor(rule({ rule: "actions-forbidden", fix: "actions" }), PLAN, {})).toEqual(["actions"]);
    expect(groupsFor(rule({ rule: "x", fix: null }), PLAN, {})).toEqual([]);
  });
});

describe("preflight store", () => {
  it("checks against the chosen profile and keeps the revision it read", async () => {
    reply({ preflight_check: failing });
    p().setProfile("pdfa-3b");
    await p().check(1);
    expect(inv).toHaveBeenCalledWith("preflight_check", expect.objectContaining({ doc: 1, profile: "pdfa-3b" }));
    expect(p().get(1).report).toEqual(failing);
    expect(p().get(1).checkedRev).toBe(3);
    expect(p().get(1).checkedProfile).toBe("pdfa-3b");
    expect(p().get(1).phase).toBe("idle");
  });

  it("converts into a new file named by the person and leaves the document alone", async () => {
    reply({ preflight_check: failing });
    await p().check(1);
    vi.mocked(pickSave).mockResolvedValue({ token: "tok-1", path: "C:\\out.pdf", name: "out.pdf" } as never);
    reply({ preflight_convert: { path: "C:\\out.pdf", report: report([], PLAN, true), fixed: ["Embedded Arial"] } });
    p().setTick(1, "fonts", false);
    await p().convert(1);
    // Fonts are switched off and two findings need fonts that are not installed: not named as PDF/A.
    expect(pickSave).toHaveBeenCalledWith("Annual report 2026 (converted).pdf", { doc: 1 }, "pdf");
    const call = inv.mock.calls.find((c) => c[0] === "preflight_convert")!;
    expect(call[1]).toMatchObject({ doc: 1, profile: "pdfa-2b", to: "tok-1", convert: { groups: ["actions", "files", "colour", "metadata"] } });
    expect(applyState).not.toHaveBeenCalled();
    expect(p().get(1).converted?.path).toBe("C:\\out.pdf");
  });

  it("converts in place as one undo step", async () => {
    reply({ preflight_check: failing });
    await p().check(1);
    p().setOutput(1, "inPlace");
    reply({ preflight_fix: state(4) });
    await p().convert(1);
    expect(pickSave).not.toHaveBeenCalled();
    expect(applyState).toHaveBeenCalledWith(1, state(4));
    expect(p().get(1).phase).toBe("idle");
  });

  it("a cancelled save dialog does nothing", async () => {
    reply({ preflight_check: failing });
    await p().check(1);
    vi.mocked(pickSave).mockResolvedValue(null);
    await p().convert(1);
    expect(p().get(1).phase).toBe("idle");
    expect(inv.mock.calls.some((c) => c[0] === "preflight_convert")).toBe(false);
  });

  it("a PDF/UA rule runs the accessibility checker's fix, one undo step", async () => {
    const ua = report([rule({ rule: "title", status: "failed", fix: "accessibility", a11yFix: "language", findings: [fnd({ page: null })] })]);
    reply({ preflight_check: { ...ua, profile: "pdfua-1" } });
    await p().check(1);
    reply({ a11y_fix: state(4) });
    expect(await p().fixRule(1, ua.results[0])).toBe(true);
    expect(inv).toHaveBeenCalledWith("a11y_fix", expect.objectContaining({ doc: 1, fix: { fix: "language", lang: p().lang } }));
    expect(applyState).toHaveBeenCalledWith(1, state(4));
  });

  it("an error is shown and a cancel is not an error", async () => {
    reply({ preflight_check: () => Promise.reject("The document is encrypted") });
    await p().check(1);
    expect(p().get(1).error).toBe("The document is encrypted");
    expect(p().get(1).errorRev).toBe(3);
    reply({ preflight_check: () => Promise.reject("cancelled") });
    await p().check(1);
    expect(p().get(1).error).toBeNull();
  });

  it("reads the file's claim and Verify opens the rail on the claimed profile", async () => {
    reply({ preflight_claim: { label: "PDF/A-1b", profile: "pdfa-1b" }, preflight_check: report([], PLAN, true) });
    await p().refreshClaim(1);
    expect(p().get(1).claim).toEqual({ label: "PDF/A-1b", profile: "pdfa-1b" });
    expect(p().get(1).claimRev).toBe(3);
    p().verify(1);
    expect(p().open).toBe(true);
    expect(p().profile).toBe("pdfa-1b");
  });

  it("shows a finding by jumping above it", () => {
    p().show(1, fnd({ page: 4, rect: [10, 200, 50, 260] }));
    expect(useStore.getState().docs[1].nav).toMatchObject({ page: 4, y: 152 });
    expect(p().get(1).shown).toMatchObject({ page: 4, rect: [10, 200, 50, 260] });
    p().clearShown(1);
    p().show(1, fnd({ page: null }));
    expect(p().get(1).shown).toBeNull();
  });

  it("a closed document takes its report with it", async () => {
    reply({ preflight_check: failing });
    await p().check(1);
    useStore.setState({ docs: {}, order: [], active: null as never });
    expect(p().docs[1]).toBeUndefined();
  });
});
