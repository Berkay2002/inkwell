import { describe, expect, it } from "vitest";
import type { BatchAction, BatchIssue } from "./api";
import { copyOf, emptyAction, exportName, keyMoveTarget, move, newStep, RANGE_KINDS, saveBlock, splitIssues, STEP_GROUPS, STEP_INFO, stepLabel, takesPages, withoutPasswords, withPasswords } from "./batchLogic";

const protect = (): BatchAction => ({ ...emptyAction(), name: "P", steps: [newStep("compress"), newStep("protect")] });

describe("the step library", () => {
  it("lists each of the engine's twelve steps once, in a group", () => {
    const listed = STEP_GROUPS.flatMap((g) => g.kinds);
    expect(new Set(listed).size).toBe(12);
    expect(listed.sort()).toEqual(Object.keys(STEP_INFO).sort());
    expect(STEP_GROUPS.map((g) => g.title)).toEqual(["Protect", "Document processing", "Pages", "Export", "Optimize"]);
  });

  it("starts every step with the kind it was made for and a usable label", () => {
    for (const k of Object.keys(STEP_INFO) as (keyof typeof STEP_INFO)[]) {
      const s = newStep(k);
      expect(s.kind).toBe(k);
      expect(stepLabel(s).length).toBeGreaterThan(3);
    }
    expect(stepLabel({ kind: "invalid", name: "decrypt", why: "x" })).toBe('Unknown step "decrypt"');
  });
});

describe("move and copy", () => {
  it("moves one item and leaves the array alone for a move that is none", () => {
    const a = ["a", "b", "c"];
    expect(move(a, 0, 2)).toEqual(["b", "c", "a"]);
    expect(move(a, 1, 1)).toBe(a);
    expect(move(a, 0, 3)).toBe(a);
  });

  it("a copy has no id and is not built in", () => {
    const c = copyOf({ ...emptyAction(), id: "builtin-x", name: "Prepare", builtIn: true });
    expect(c).toMatchObject({ id: "", builtIn: false, name: "Prepare copy" });
  });

  it("names an export after the action with the characters a file cannot hold taken out", () => {
    expect(exportName({ ...emptyAction(), name: ' A/B: "c"? ' })).toBe("A B c.json");
    expect(exportName(emptyAction())).toBe("action.json");
  });
});

describe("passwords", () => {
  it("go into Protect steps for a call and come out again for a save", () => {
    const a = protect();
    const w = withPasswords(a, "open-1", "owner-2");
    expect(w.steps[1]).toMatchObject({ openPassword: "open-1", permissionsPassword: "owner-2" });
    expect(w.steps[0]).not.toHaveProperty("openPassword");
    expect(a.steps[1]).not.toHaveProperty("openPassword");
    expect(JSON.stringify(withoutPasswords(w))).not.toMatch(/open-1|owner-2/);
    expect(withPasswords(a, "", "")).toBe(a);
  });

  it("a Protect step that only waits for its password does not block; any other issue does", () => {
    const a = protect();
    const issues: BatchIssue[] = [
      { step: 1, code: "invalid-step", message: "Type the password for this step" },
      { step: 0, code: "invalid-step", message: "Choose Smallest, Balanced or High quality." },
      { step: null, code: "output", message: "x" },
    ];
    const s = splitIssues(a, issues, false);
    expect(s.passwordFor).toEqual([1]);
    expect(s.blocking.map((i) => i.step)).toEqual([0, null]);
    // once something is typed, an issue on the Protect step is a real one
    expect(splitIssues(a, issues, true).blocking).toHaveLength(3);
  });
});

describe("saveBlock", () => {
  it("names what Save needs, in the order a person fixes it", () => {
    const a = emptyAction();
    expect(saveBlock(a)).toBe("Give the action a name.");
    expect(saveBlock({ ...a, name: "x" })).toBe("Add at least one step.");
    expect(saveBlock({ ...a, name: "x", steps: [newStep("compress")] })).toBeNull();
    expect(saveBlock({ ...a, name: "x", steps: [{ kind: "invalid", name: "n", why: "Not known." }] })).toBe("Not known.");
    expect(saveBlock({ ...a, name: "x", steps: [newStep("compress")], builtIn: true })).toContain("duplicate");
  });
});

describe("keyboard reorder", () => {
  it("moves a step one place with the arrows and stops at the ends; the grip then follows it", () => {
    expect(keyMoveTarget("ArrowUp", 2, 4)).toBe(1);
    expect(keyMoveTarget("ArrowDown", 2, 4)).toBe(3);
    expect(keyMoveTarget("ArrowUp", 0, 4)).toBeNull();
    expect(keyMoveTarget("ArrowDown", 3, 4)).toBeNull();
    expect(keyMoveTarget("Enter", 1, 4)).toBeNull();
    // two presses in a row carry one step two places: the second starts from where the first put it
    let steps = ["a", "b", "c", "d"];
    let at = 3;
    for (let n = 0; n < 2; n++) {
      const to = keyMoveTarget("ArrowUp", at, steps.length)!;
      steps = move(steps, at, to);
      at = to;
    }
    expect(steps).toEqual(["a", "d", "b", "c"]);
  });
});

describe("page ranges", () => {
  it("are offered to the page by page steps and to no other", () => {
    expect([...RANGE_KINDS].sort()).toEqual(["bates", "flattenForms", "headerFooter", "ocr", "redact", "watermark"]);
    for (const k of Object.keys(STEP_INFO) as (keyof typeof STEP_INFO)[]) expect(takesPages(newStep(k))).toBe(RANGE_KINDS.has(k));
    expect(takesPages({ kind: "invalid", name: "x", why: "y" })).toBe(false);
  });

  it("show in the step's label, and are kept when a step is copied or its passwords are handled", () => {
    const s = { ...newStep("watermark"), pages: "1-3, 5", prompt: true };
    expect(stepLabel(s)).toMatch(/\(pages 1-3, 5\)$/);
    expect(stepLabel({ ...s, pages: "  " })).toBe(stepLabel(newStep("watermark")));
    const a = { ...emptyAction(), steps: [s, newStep("protect")] };
    expect(withoutPasswords(withPasswords(a, "o", "p")).steps[0]).toEqual(s);
    expect(copyOf(a).steps[0]).toEqual(s);
  });
});
