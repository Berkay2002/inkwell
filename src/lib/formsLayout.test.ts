import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));
vi.mock("../store", () => ({ useStore: { getState: () => ({ goTo: vi.fn() }) } }));
vi.mock("./printStore", () => ({ usePrint: { getState: () => ({ open: vi.fn() }) } }));

import { invoke } from "@tauri-apps/api/core";
import { applyState } from "./edit";
import { useForms } from "./forms";
import {
  CERTIFIED_REASON, type Field, NudgeBatch, anchorOf, layoutReason, marqueeHits, nextPicks, normBox, nudgeDelta, renameCheck,
} from "./formsLogic";

const inv = vi.mocked(invoke);
const f = (p: Partial<Field>): Field => ({
  page: 0, xref: 1, node: 1, name: "A", kind: "text", rect: [0, 0, 10, 10], tab: 0, value: "", values: [], defaultValue: "",
  checked: false, export: "", options: [], readOnly: false, required: false, hidden: false, multiline: false, password: false,
  comb: false, editable: false, multiSelect: false, noToggleOff: false, maxLen: 0, fontSize: 0, textColor: [0, 0, 0], align: 0,
  borderColor: null, fillColor: null, borderWidth: 1, format: { kind: "none" }, validate: { kind: "none" },
  calculate: { kind: "none" }, shown: "", caption: "", action: null, tooltip: "", script: false, ...p,
});
const p = (xref: number, page = 0) => ({ page, xref });
const state = { rev: 2, pageCount: 1, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Align", redo: null };

describe("selection model", () => {
  it("a plain click picks one; an additive click adds it as the anchor, or takes it out", () => {
    expect(nextPicks([p(1), p(2)], p(3), false)).toEqual([p(3)]);
    const two = nextPicks([p(1)], p(2), true);
    expect(two).toEqual([p(1), p(2)]);
    expect(anchorOf(two)).toEqual(p(2));
    // picking the first again moves the anchor to it; picking the anchor takes it out and the one before becomes anchor
    expect(anchorOf(nextPicks(two, p(3), true))).toEqual(p(3));
    expect(nextPicks(two, p(2), true)).toEqual([p(1)]);
    expect(anchorOf([])).toBeNull();
  });

  it("the marquee takes the visible widgets of the page it touches, in reading order", () => {
    const fields = [
      f({ xref: 1, rect: [100, 100, 160, 120] }),
      f({ xref: 2, rect: [10, 10, 60, 30] }),
      f({ xref: 3, rect: [300, 300, 340, 320] }),
      f({ xref: 4, rect: [20, 12, 50, 28], hidden: true }),
      f({ xref: 5, page: 1, rect: [10, 10, 60, 30] }),
    ];
    expect(marqueeHits(fields, 0, normBox([0, 0], [200, 200]))).toEqual([p(2), p(1)]);
    // a box that only touches an edge hits nothing; a box drawn backwards is the same box
    expect(marqueeHits(fields, 0, [160, 100, 200, 120])).toEqual([]);
    expect(marqueeHits(fields, 0, normBox([340, 320], [250, 250]))).toEqual([p(3)]);
  });
});

describe("layout availability", () => {
  const fields = [
    f({ xref: 1, node: 1 }),
    f({ xref: 2, node: 2, rect: [20, 0, 30, 10] }),
    f({ xref: 3, node: 3, rect: [40, 0, 50, 10] }),
    f({ xref: 4, node: 4, page: 1 }),
    f({ xref: 5, node: 5, kind: "radio" }),
    f({ xref: 6, node: 6, calculate: { kind: "custom", value: "event.value = 1;" } }),
  ];
  it("says why a tool is off, and the certified reason comes first", () => {
    expect(layoutReason("align", [p(1)], fields, false)).toMatch(/two or more/);
    expect(layoutReason("align", [p(1), p(2)], fields, false)).toBeNull();
    expect(layoutReason("align", [p(1), p(4, 1)], fields, false)).toMatch(/one page/);
    expect(layoutReason("distribute", [p(1), p(2)], fields, false)).toMatch(/three or more/);
    expect(layoutReason("distribute", [p(1), p(2), p(3)], fields, false)).toBeNull();
    expect(layoutReason("align", [p(1), p(2)], fields, true)).toBe(CERTIFIED_REASON);
    expect(layoutReason("center", [], fields, false)).toMatch(/Select a field/);
  });
  it("duplicate and copies need one field that can be copied", () => {
    expect(layoutReason("duplicate", [p(1), p(2)], fields, false)).toMatch(/one field/);
    expect(layoutReason("duplicate", [p(1)], fields, false)).toBeNull();
    expect(layoutReason("copies", [p(5)], fields, false)).toMatch(/radio/i);
    expect(layoutReason("copies", [p(6)], fields, false)).toMatch(/calculat/i);
  });
});

describe("nudge batching", () => {
  it("arrow presses add up until the burst ends and go out as one move", () => {
    expect(nudgeDelta("ArrowRight", true)).toEqual([10, 0]);
    expect(nudgeDelta("a", false)).toBeNull();
    const b = new NudgeBatch();
    expect(b.push("ArrowRight", false)).toBe(true);
    b.push("ArrowRight", false);
    b.push("ArrowDown", true);
    expect(b.push("Enter", false)).toBe(false);
    expect(b.take()).toEqual([2, 10]);
    expect(b.take()).toBeNull();
  });
  it("a burst that cancels out sends nothing", () => {
    const b = new NudgeBatch();
    b.push("ArrowLeft", false);
    b.push("ArrowRight", false);
    expect(b.take()).toBeNull();
  });
});

describe("rename check", () => {
  const fields = [
    f({ xref: 1, node: 1, name: "Name" }),
    f({ xref: 2, node: 2, name: "Address.City" }),
    f({ xref: 3, node: 3, name: "Agree", kind: "checkbox" }),
    f({ xref: 4, node: 4, name: "Pick", kind: "radio" }),
    f({ xref: 5, node: 5, name: "Other", kind: "radio" }),
  ];
  it("same name, a free name, and bad names", () => {
    expect(renameCheck(fields, fields[0], " Name ").kind).toBe("same");
    expect(renameCheck(fields, fields[0], "Surname").kind).toBe("ok");
    expect(renameCheck(fields, fields[0], "A..B").kind).toBe("error");
    expect(renameCheck(fields, fields[0], "").kind).toBe("error");
  });
  it("a clash merges only fields of one kind, never radios, and a dotted name cannot sit under a field", () => {
    expect(renameCheck(fields, fields[0], "Address.City")).toMatchObject({ kind: "merge" });
    expect(renameCheck(fields, fields[0], "Agree").kind).toBe("error");
    expect(renameCheck(fields, fields[4], "Pick").kind).toBe("error");
    expect(renameCheck(fields, fields[0], "Address").kind).toBe("error");
    expect(renameCheck(fields, fields[1], "Name.First").kind).toBe("error");
    // a field may take a dotted name below its own old name: it is the one leaving
    expect(renameCheck(fields, fields[0], "Name.First").kind).toBe("ok");
  });
});

describe("layout actions in the store", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useForms.setState({ docs: {}, open: false, authoring: true, tool: "select" });
  });
  const list = { fields: [f({ xref: 1 }), f({ xref: 2, node: 2 }), f({ xref: 3, node: 3 })], xfa: false };
  const setup = async () => {
    inv.mockImplementation((cmd: string) => Promise.resolve(cmd.startsWith("forms_") && cmd !== "forms_list" ? state : list));
    await useForms.getState().refresh(1);
  };

  it("align sends the picked fields in order, the anchor last, as one command", async () => {
    await setup();
    useForms.getState().select(1, p(1));
    useForms.getState().toggle(1, p(3));
    useForms.getState().toggle(1, p(2));
    inv.mockClear();
    await useForms.getState().align(1, "left");
    expect(inv).toHaveBeenCalledWith("forms_align", { doc: 1, items: [[0, 1], [0, 3], [0, 2]], edge: "left" });
    expect(applyState).toHaveBeenCalledWith(1, state);
    expect(useForms.getState().get(1).selected).toEqual(p(2));
  });

  it("a marquee replaces the picks, or adds to them; a refresh drops fields that are gone", async () => {
    await setup();
    useForms.getState().select(1, p(1));
    useForms.getState().pickMany(1, [p(2), p(3)]);
    expect(useForms.getState().get(1).picked).toEqual([p(2), p(3)]);
    useForms.getState().pickMany(1, [p(1), p(3)], true);
    expect(useForms.getState().get(1).picked).toEqual([p(2), p(1), p(3)]);
    inv.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "forms_list" ? { fields: [f({ xref: 1 })], xfa: false } : { fields: [f({ xref: 1 })], xfa: false }),
    );
    await useForms.getState().refresh(1);
    expect(useForms.getState().get(1).picked).toEqual([p(1)]);
    expect(useForms.getState().get(1).selected).toEqual(p(1));
  });

  it("copies reports what was made and left out; a refusal comes back as null with the message in error", async () => {
    await setup();
    useForms.getState().select(1, p(1));
    inv.mockImplementation((cmd: string) => Promise.resolve(cmd === "forms_copies" ? { ...state, made: 5, skipped: 2 } : list));
    const note = await useForms.getState().copies(1, 2, 4, 12, 6);
    expect(inv).toHaveBeenCalledWith("forms_copies", { doc: 1, item: [0, 1], rows: 2, cols: 4, gapX: 12, gapY: 6 });
    expect(note).toBe("5 copies made, 2 left out because they would run off the page.");
    inv.mockRejectedValue("This document is certified.");
    expect(await useForms.getState().copies(1, 2, 2, 0, 0)).toBeNull();
    expect(useForms.getState().get(1).error).toBe("This document is certified.");
  });

  it("rename resolves to the refusal, or null", async () => {
    await setup();
    expect(await useForms.getState().rename(1, p(1), "B", false)).toBeNull();
    expect(inv).toHaveBeenCalledWith("forms_rename", { doc: 1, item: [0, 1], name: "B", merge: false });
    inv.mockRejectedValue("A check box named B already exists.");
    expect(await useForms.getState().rename(1, p(1), "B", false)).toBe("A check box named B already exists.");
  });

  it("the open dialog and the field being renamed belong to their document", () => {
    const st = useForms.getState();
    st.setDialog(1, "copies");
    st.setRenaming(1, p(3));
    expect(useForms.getState().dialog).toEqual({ doc: 1, kind: "copies" });
    expect(useForms.getState().renaming).toEqual({ doc: 1, page: 0, xref: 3 });
    // Another document's panel reads nothing from them.
    expect(useForms.getState().dialog?.doc === 2).toBe(false);
    expect(useForms.getState().renaming?.doc === 2).toBe(false);
    st.setDialog(2, null);
    st.setRenaming(2, null);
    expect(useForms.getState().dialog).toBeNull();
    expect(useForms.getState().renaming).toBeNull();
  });

  it("nudge goes out once with the summed distance", async () => {
    await setup();
    useForms.getState().select(1, p(2));
    inv.mockClear();
    await useForms.getState().nudge(1, 3, -10);
    expect(inv).toHaveBeenCalledWith("forms_nudge", { doc: 1, items: [[0, 2]], dx: 3, dy: -10 });
  });
});
