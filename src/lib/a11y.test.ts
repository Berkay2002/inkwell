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
vi.mock("./edit", () => ({ applyState: vi.fn(), run: vi.fn() }));
vi.mock("./api", () => ({ pickSave: vi.fn(), toolCancel: vi.fn(() => Promise.resolve()) }));

import { invoke } from "@tauri-apps/api/core";
import { useStore } from "../store";
import { useA11y } from "./a11y";
import {
  type Finding,
  type Report,
  type RuleResult,
  afterFixAll,
  autoCheckDue,
  autotagText,
  counts,
  fixAllText,
  groupRules,
  isOverflowNote,
  progressFraction,
  progressText,
  safeFixes,
  summaryLine,
  suggestLang,
} from "./a11yLogic";
import { applyState } from "./edit";
import { pickSave, toolCancel } from "./api";

const inv = vi.mocked(invoke);
const f = (over: Partial<Finding> = {}): Finding => ({ page: 0, object: null, xref: null, rect: null, message: "m", ...over });
const rule = (over: Partial<RuleResult>): RuleResult => ({
  rule: "title",
  name: "Title",
  group: "Document",
  status: "passed",
  hint: "",
  fix: null,
  findings: [],
  ...over,
});
const report = (results: RuleResult[], tagged = true): Report => ({ pages: 12, tagged, certified: false, signed: false, results });
const sample = report([
  rule({ rule: "title", name: "Title", status: "failed", fix: "title", findings: [f({ page: null })] }),
  rule({ rule: "display-doc-title", name: "Title in title bar", status: "passed", fix: "displayDocTitle" }),
  rule({ rule: "primary-language", name: "Primary language", status: "failed", fix: "language" }),
  rule({ rule: "tab-order", name: "Tab order", group: "Page content", status: "failed", fix: "tabOrder" }),
  rule({ rule: "color-contrast", name: "Color contrast", status: "manual" }),
  rule({
    rule: "figure-alt-text",
    name: "Figures alternate text",
    group: "Alternate text",
    status: "failed",
    fix: "figureAlt",
    findings: [f({ page: 1, xref: 40, rect: [10, 10, 100, 80] })],
  }),
  rule({ rule: "heading-nesting", name: "Heading nesting", group: "Headings", status: "failed" }),
  rule({ rule: "table-headers", name: "Table headers", group: "Tables", status: "skipped" }),
]);

describe("report logic", () => {
  it("counts and words the result", () => {
    expect(counts(sample)).toEqual({ failed: 5, manual: 1, passed: 1, skipped: 1 });
    expect(summaryLine(sample)).toBe("5 failed · 1 needs a manual check · 1 passed · 1 skipped");
  });

  it("groups in Acrobat's order with failed rules first", () => {
    const g = groupRules(sample);
    expect(g.map((x) => x.name)).toEqual(["Document", "Page content", "Alternate text", "Tables", "Headings"]);
    expect(g[0].rules.map((x) => x.rule)).toEqual(["title", "primary-language", "color-contrast", "display-doc-title"]);
    expect(g[0].failed).toBe(2);
  });

  it("picks the safe fixes only and predicts the outcome", () => {
    const fixes = safeFixes(sample, "sv-SE");
    expect(fixes).toEqual([{ fix: "title", title: null }, { fix: "language", lang: "sv-SE" }, { fix: "tabOrder" }]);
    // the figure rule needs text from a person, so it is not one of them
    expect(afterFixAll(sample)).toEqual({ pass: 4, of: 8 });
    expect(fixAllText(sample)).toEqual({ label: "Fix all safe issues (3)", outcome: "Fix 3 issues · after this 4 of 8 pass" });
  });

  it("offers no fix-all when nothing is safe", () => {
    expect(fixAllText(report([rule({ status: "passed" })]))).toBeNull();
  });

  it("says what Autotag does before it runs", () => {
    expect(autotagText({ untagged: true, pages: 12, figures: 1, links: 0, fields: 0 })).toBe(
      "Adds tags to 12 pages; reading order from layout. 1 image gets a Figure tag without alternate text. One undo step.",
    );
    expect(autotagText({ untagged: true, pages: 1, figures: 0, links: 0, fields: 0 })).toBe(
      "Adds tags to 1 page; reading order from layout. One undo step.",
    );
  });

  it("reports progress across Autotag's two stages", () => {
    expect(progressText({ done: 4, pages: 8, stage: "tag" }, "autotag")).toBe("Tagging page 5 of 8");
    expect(progressText({ done: 0, pages: 8, stage: "read" }, "autotag")).toBe("Reading page 1 of 8");
    expect(progressFraction({ done: 4, pages: 8, stage: "read" }, "autotag")).toBe(0.25);
    expect(progressFraction({ done: 4, pages: 8, stage: "tag" }, "autotag")).toBe(0.75);
    expect(progressText(null, "check")).toBe("Checking…");
  });

  it("tells the truncation note from a finding and matches a language", () => {
    expect(isOverflowNote(f({ page: null, message: "... and 12 more" }))).toBe(true);
    expect(isOverflowNote(f({ page: null, message: "The document has no title." }))).toBe(false);
    expect(suggestLang("sv")).toBe("sv-SE");
    expect(suggestLang("en-GB")).toBe("en-GB");
    expect(suggestLang("xx")).toBe("en-US");
  });
});

const state = (rev: number) => ({ rev, pageCount: 2, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Set document title", redo: null });
const a = () => useA11y.getState();

function reply(map: Record<string, unknown | (() => unknown)>) {
  inv.mockImplementation((cmd: string) => {
    if (!(cmd in map)) return Promise.reject(`unexpected ${cmd}`);
    const v = map[cmd];
    return typeof v === "function" ? (v as () => Promise<unknown>)() : Promise.resolve(v);
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  useA11y.setState({ docs: {}, open: false });
  useStore.setState({
    docs: { 1: { id: 1, name: "Annual report 2026.pdf", rev: 3, pageCount: 12, currentPage: 0, sizes: [], nav: null } as never },
    order: [1],
    active: 1,
  });
});

describe("a11y store", () => {
  it("reads the report and, for an untagged document, what Autotag would do", async () => {
    const untagged = report([rule({ rule: "tagged-pdf", status: "failed", fix: "autotag" })], false);
    reply({ a11y_check: untagged, a11y_preview: { untagged: true, pages: 12, figures: 2, links: 0, fields: 0 } });
    await a().check(1);
    expect(a().get(1).report).toEqual(untagged);
    expect(a().get(1).checkedRev).toBe(3);
    expect(a().get(1).preview?.figures).toBe(2);
    expect(a().get(1).phase).toBe("idle");
  });

  it("a fix is one command and one new state", async () => {
    reply({ a11y_check: sample, a11y_preview: { untagged: false } });
    await a().check(1);
    reply({ a11y_fix: state(4) });
    expect(await a().fix(1, { fix: "title", title: null })).toBe(true);
    expect(inv).toHaveBeenCalledWith("a11y_fix", expect.objectContaining({ doc: 1, fix: { fix: "title", title: null } }));
    expect(applyState).toHaveBeenCalledWith(1, state(4));
    expect(a().get(1).phase).toBe("idle");
  });

  it("Fix all sends the safe fixes as one group", async () => {
    reply({ a11y_check: sample, a11y_preview: { untagged: false } });
    await a().check(1);
    reply({ a11y_fix: state(4) });
    await a().fixAll(1);
    const call = inv.mock.calls.find((c) => c[0] === "a11y_fix")!;
    expect((call[1] as { fix: unknown }).fix).toEqual({
      fix: "many",
      fixes: [{ fix: "title", title: null }, { fix: "language", lang: a().lang }, { fix: "tabOrder" }],
    });
  });

  it("a cancelled Autotag is not an error and changes nothing", async () => {
    reply({ a11y_check: report([], false), a11y_preview: { untagged: true }, a11y_fix: () => Promise.reject("cancelled") });
    await a().check(1);
    vi.mocked(applyState).mockClear();
    expect(await a().fix(1, { fix: "autotag" })).toBe(false);
    expect(a().get(1).error).toBeNull();
    expect(a().get(1).phase).toBe("idle");
    expect(applyState).not.toHaveBeenCalled();
  });

  it("shows a finding by jumping above it and keeping its box", () => {
    a().show(1, f({ page: 4, rect: [10, 200, 50, 260] }));
    expect(useStore.getState().docs[1].nav).toMatchObject({ page: 4, y: 152 });
    expect(a().get(1).shown).toMatchObject({ page: 4, rect: [10, 200, 50, 260] });
    // a document-level finding has nowhere to go
    a().clearShown(1);
    a().show(1, f({ page: null }));
    expect(a().get(1).shown).toBeNull();
  });

  it("exports through a save token and says where the file went", async () => {
    vi.mocked(pickSave).mockResolvedValue({ token: "tok", path: "C:\\r.html" } as never);
    reply({ a11y_export_report: "C:\\r.html" });
    await a().exportReport(1, "html");
    expect(pickSave).toHaveBeenCalledWith("Annual report 2026 Accessibility Report.html", { doc: 1 }, "html");
    expect(inv).toHaveBeenCalledWith("a11y_export_report", expect.objectContaining({ doc: 1, format: "html", to: "tok" }));
    expect(a().get(1).exported).toBe("C:\\r.html");
  });

  it("a new check stops the one still running", async () => {
    let release: (r: Report) => void = () => {};
    inv.mockImplementationOnce(() => new Promise((res) => (release = res as never)));
    const first = a().check(1);
    reply({ a11y_check: sample, a11y_preview: { untagged: false } });
    await a().check(1);
    expect(toolCancel).toHaveBeenCalledTimes(1);
    release(report([]));
    await first;
    // the stale answer did not replace the newer report
    expect(a().get(1).report).toEqual(sample);
  });
});

describe("when the report is read again by itself", () => {
  const idle = { phase: "idle", checkedRev: 2, skipRev: null, errorRev: null };

  it("is due after an edit, not when the report is current", () => {
    expect(autoCheckDue(idle, 2)).toBe(false);
    expect(autoCheckDue(idle, 3)).toBe(true);
    expect(autoCheckDue(undefined, 0)).toBe(true);
    expect(autoCheckDue({ ...idle, phase: "checking" }, 3)).toBe(false);
  });

  it("a cancelled check stays cancelled until Check again, the rail opening, or an edit", async () => {
    inv.mockImplementationOnce(() => new Promise(() => {}));
    void a().check(1);
    expect(a().get(1).phase).toBe("checking");
    a().cancel(1);
    expect(a().get(1)).toMatchObject({ phase: "idle", skipRev: 3 });
    // the effect would run now: the revision is the cancelled one, so nothing restarts
    expect(autoCheckDue(a().get(1), 3)).toBe(false);
    // an edit moves on
    expect(autoCheckDue(a().get(1), 4)).toBe(true);
    // opening the rail asks again
    a().setOpen(true);
    expect(a().get(1).skipRev).toBeNull();
    expect(autoCheckDue(a().get(1), 3)).toBe(true);
    // and so does Check again
    a().cancel(1);
    reply({ a11y_check: sample, a11y_preview: { untagged: false } });
    await a().check(1);
    expect(a().get(1)).toMatchObject({ phase: "idle", skipRev: null, checkedRev: 3 });
  });

  it("closing the rail cancels the check and does not start it again at once", () => {
    inv.mockImplementationOnce(() => new Promise(() => {}));
    void a().check(1);
    a().setOpen(false);
    expect(toolCancel).toHaveBeenCalled();
    expect(a().get(1).skipRev).toBe(3);
  });

  it("an error from a fix or an export does not stop the check after the next edit", async () => {
    reply({ a11y_check: sample, a11y_preview: { untagged: false }, a11y_fix: () => Promise.reject("the file is read-only") });
    await a().check(1);
    await a().fix(1, { fix: "tabOrder" });
    expect(a().get(1).error).toBe("the file is read-only");
    expect(a().get(1).errorRev).toBeNull();
    expect(autoCheckDue(a().get(1), 4)).toBe(true);
  });

  it("a check that failed holds back only its own revision", async () => {
    reply({ a11y_check: () => Promise.reject("could not read the file") });
    await a().check(1);
    expect(a().get(1)).toMatchObject({ error: "could not read the file", errorRev: 3 });
    expect(autoCheckDue(a().get(1), 3)).toBe(false);
    expect(autoCheckDue(a().get(1), 4)).toBe(true);
  });
});

describe("a document's protection", () => {
  const prot = (certified: boolean, signed = false) => ({ certified, signed });

  it("is read again when the document is edited or saved, and the controls follow", async () => {
    reply({ a11y_protection: prot(false) });
    await a().protection(1);
    expect(a().get(1).protection).toEqual(prot(false));
    reply({ a11y_protection: prot(true, true) });
    // a save certified it: the revision and the dirty flag change
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], dirty: true } } });
    await vi.waitFor(() => expect(a().get(1).protection).toEqual(prot(true, true)));
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 4 } } });
    await vi.waitFor(() => expect(inv.mock.calls.filter((c) => c[0] === "a11y_protection")).toHaveLength(3));
    // the edit guards go by the new value
    reply({ a11y_check: sample, a11y_preview: { untagged: false } });
    expect(await a().fix(1, { fix: "tabOrder" })).toBe(false);
  });

  it("keeps what it knew when a read fails, and the newest read wins", async () => {
    reply({ a11y_protection: prot(true) });
    await a().protection(1);
    reply({ a11y_protection: () => Promise.reject("no") });
    expect(await a().refreshProtection(1)).toEqual(prot(true));
    expect(a().get(1).protection).toEqual(prot(true));
    let first!: (v: unknown) => void;
    inv.mockReturnValueOnce(new Promise((r) => (first = r)));
    const slow = a().refreshProtection(1);
    reply({ a11y_protection: prot(false) });
    await a().refreshProtection(1);
    first(prot(true));
    await slow;
    expect(a().get(1).protection).toEqual(prot(false));
  });
});

describe("a closed document", () => {
  it("is not brought back by a late progress or completion", async () => {
    let finish: (r: Report) => void = () => {};
    let progress: ((p: unknown) => void) | undefined;
    inv.mockImplementationOnce((_cmd: string, args?: unknown) => {
      progress = (args as { onProgress: { onmessage: (p: unknown) => void } }).onProgress.onmessage;
      return new Promise((res) => (finish = res as never));
    });
    const run = a().check(1);
    expect(a().docs[1]).toBeDefined();
    // the tab closes: its entry goes with it
    useStore.setState({ docs: {}, order: [], active: null as never });
    a().forget(1);
    progress?.({ done: 3, pages: 12, stage: "check" });
    finish(sample);
    await run;
    expect(a().docs[1]).toBeUndefined();
  });

  it("a fix finishing after the tab closed leaves no entry either", async () => {
    reply({ a11y_check: sample, a11y_preview: { untagged: false } });
    await a().check(1);
    // The first edit of a document reads its protection first; read it here so the next call is the fix.
    await a().protection(1);
    let finish: (s: unknown) => void = () => {};
    inv.mockImplementationOnce(() => new Promise((res) => (finish = res)));
    const run = a().fix(1, { fix: "tabOrder" });
    useStore.setState({ docs: {}, order: [], active: null as never });
    a().forget(1);
    finish(state(4));
    await run;
    expect(a().docs[1]).toBeUndefined();
  });
});

describe("a report export", () => {
  it("takes the phase from a check that started while the Save dialog was open", async () => {
    reply({ a11y_check: sample, a11y_preview: { untagged: false } });
    await a().check(1);
    let pick: (v: unknown) => void = () => {};
    vi.mocked(pickSave).mockImplementationOnce(() => new Promise((res) => (pick = res)) as never);
    const exporting = a().exportReport(1, "html");
    // an edit starts a check behind the dialog
    inv.mockImplementationOnce(() => new Promise(() => {}));
    void a().check(1);
    expect(a().get(1).phase).toBe("checking");
    reply({ a11y_export_report: "C:\\r.html" });
    pick({ token: "tok", path: "C:\\r.html" });
    await exporting;
    expect(toolCancel).toHaveBeenCalled();
    expect(a().get(1)).toMatchObject({ phase: "idle", exported: "C:\\r.html" });
  });
});
