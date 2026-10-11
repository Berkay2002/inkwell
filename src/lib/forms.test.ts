import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { applyState } from "./edit";
import { useForms } from "./forms";
import {
  type Field,
  detectSummary,
  detectedToNew,
  dragged,
  missingRequired,
  move,
  onPage,
  progress,
  resetTargets,
  resize,
  rows,
  summary,
} from "./formsLogic";

const inv = vi.mocked(invoke);
const f = (p: Partial<Field>): Field => ({
  page: 0, xref: 1, node: 1, name: "A", kind: "text", rect: [0, 0, 10, 10], tab: 0, value: "", values: [], defaultValue: "",
  checked: false, export: "", options: [], readOnly: false, required: false, hidden: false, multiline: false, password: false,
  comb: false, editable: false, multiSelect: false, noToggleOff: false, maxLen: 0, fontSize: 0, textColor: [0, 0, 0], align: 0,
  borderColor: null, fillColor: null, borderWidth: 1, format: { kind: "none" }, validate: { kind: "none" },
  calculate: { kind: "none" }, shown: "", caption: "", action: null, tooltip: "", script: false, ...p,
});
const state = { rev: 2, pageCount: 1, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Fill field on p. 1", redo: null };

beforeEach(() => {
  vi.clearAllMocks();
  useForms.setState({ docs: {}, open: false, authoring: false, tool: "select" });
});

describe("forms store", () => {
  it("fill runs one command, applies its state and re-reads the list", async () => {
    inv.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "forms_set_value" ? state : { fields: [f({ xref: 3, value: "Alice", values: ["Alice"] })], xfa: false }),
    );
    const err = await useForms.getState().setValue(1, 0, 3, "Alice");
    expect(err).toBeNull();
    expect(inv).toHaveBeenCalledWith("forms_set_value", { doc: 1, page: 0, xref: 3, value: "Alice" });
    expect(applyState).toHaveBeenCalledWith(1, state);
    expect(useForms.getState().get(1).fields[0].value).toBe("Alice");
  });

  it("a rejected value comes back as its message and changes nothing", async () => {
    inv.mockRejectedValue("Invalid value: must be greater than or equal to 0.");
    const err = await useForms.getState().setValue(1, 0, 3, "-5");
    expect(err).toBe("Invalid value: must be greater than or equal to 0.");
    expect(applyState).not.toHaveBeenCalled();
    expect(useForms.getState().get(1).error).toBe(err);
  });

  it("add selects the new field; clear form sends no field list", async () => {
    inv.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "forms_add" ? { state, xref: 9 } : cmd === "forms_reset" ? state : { fields: [f({ xref: 9 })], xfa: false }),
    );
    expect(await useForms.getState().add(1, 0, { kind: "text", rect: [0, 0, 100, 20] })).toBe(9);
    expect(useForms.getState().get(1).selected).toEqual({ page: 0, xref: 9 });
    await useForms.getState().clear(1);
    expect(inv).toHaveBeenCalledWith("forms_reset", { doc: 1, fields: null });
  });

  it("detect keeps the prediction and adding it makes one batch", async () => {
    const found = [
      { kind: "text" as const, name: "Full name", rect: [10, 10, 100, 30] as [number, number, number, number] },
      { kind: "checkbox" as const, name: "Agree", rect: [10, 40, 24, 54] as [number, number, number, number] },
    ];
    inv.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "forms_detect" ? found : cmd === "forms_add_many" ? state : { fields: [], xfa: false }),
    );
    await useForms.getState().detect(1, 2);
    expect(useForms.getState().get(1).detected).toEqual(found);
    await useForms.getState().addDetected(1);
    expect(inv).toHaveBeenCalledWith("forms_add_many", {
      doc: 1,
      page: 2,
      items: [
        { kind: "text", rect: [10, 10, 100, 30], name: "Full name" },
        { kind: "checkbox", rect: [10, 40, 24, 54], name: "Agree" },
      ],
    });
    expect(useForms.getState().get(1).detected).toEqual([]);
  });

  it("leaving authoring (closing the rail) returns to the select tool", () => {
    useForms.getState().setAuthoring(true);
    useForms.getState().setTool("checkbox");
    expect(useForms.getState().open).toBe(true);
    useForms.getState().setOpen(false);
    expect(useForms.getState().authoring).toBe(false);
    expect(useForms.getState().tool).toBe("select");
  });
});

describe("forms logic", () => {
  const radios = [
    f({ xref: 10, node: 5, name: "Contact", kind: "radio", export: "Email", checked: true, tab: 2 }),
    f({ xref: 11, node: 5, name: "Contact", kind: "radio", export: "Phone", tab: 3 }),
  ];

  it("lists a radio group as one row and says what is chosen", () => {
    const all = [f({ xref: 1, node: 1, tab: 0 }), ...radios, f({ xref: 12, node: 12, kind: "checkbox", checked: true, tab: 4 })];
    expect(rows(all).map((r) => r.xref)).toEqual([1, 10, 12]);
    expect(summary(radios[0], all)).toBe("Email");
    expect(summary(all[3], all)).toBe("Yes");
  });

  it("counts whole fields filled, and names the required ones still empty", () => {
    const all = [
      f({ xref: 1, node: 1, values: ["Alex"], value: "Alex" }),
      f({ xref: 2, node: 2, required: true }),
      f({ xref: 3, node: 3, kind: "button" }),
      ...radios,
    ];
    expect(progress(all)).toEqual([2, 3]);
    expect(missingRequired(all).map((x) => x.xref)).toEqual([2]);
  });

  it("resets the named fields, or with exclude everything but them", () => {
    const all = [f({ xref: 1, name: "A" }), f({ xref: 2, name: "B" }), f({ xref: 3, name: "G.x" }), f({ xref: 4, name: "C" })];
    expect(resetTargets(all, [])).toBeNull();
    expect(resetTargets(all, ["A", "G"])).toEqual([1, 3]);
    expect(resetTargets(all, ["A", "G"], true)).toEqual([2, 4]);
  });

  it("orders a page's widgets by tab order", () => {
    const all = [f({ xref: 1, tab: 2 }), f({ xref: 2, tab: 0, page: 1 }), f({ xref: 3, tab: 1 })];
    expect(onPage(all, 0).map((x) => x.xref)).toEqual([3, 1]);
  });

  it("predicts what detection would add, before it does", () => {
    const d = (kind: "text" | "checkbox" | "signature") => ({ kind, name: "x", rect: [0, 0, 1, 1] as [number, number, number, number] });
    expect(detectSummary([])).toBe("No more fields found on this page.");
    expect(detectSummary([...Array(6).fill(0).map(() => d("text")), d("checkbox"), d("checkbox")])).toBe(
      "Detect fields would add 6 text fields and 2 check boxes on this page.",
    );
    expect(detectSummary([d("checkbox")])).toBe("Detect fields would add 1 check box on this page.");
    expect(detectSummary([d("text"), d("text"), d("signature")])).toBe(
      "Detect fields would add 2 text fields and 1 signature field on this page.",
    );
    expect(detectedToNew([d("text")])).toEqual([{ kind: "text", rect: [0, 0, 1, 1], name: "x" }]);
  });

  it("keeps moved and resized rectangles on the page and above the minimum", () => {
    expect(move([10, 10, 60, 30], -50, 500, [200, 100])).toEqual([0, 80, 50, 100]);
    expect(resize([10, 10, 60, 30], "se", 500, 500, [200, 100])).toEqual([10, 10, 200, 100]);
    expect(resize([10, 10, 60, 30], "nw", 500, 500, [200, 100])).toEqual([54, 24, 60, 30]);
    expect(dragged("text", [20, 20], [21, 21], [300, 300])).toEqual([20, 20, 170, 42]);
    expect(dragged("text", [100, 50], [20, 30], [300, 300])).toEqual([20, 30, 100, 50]);
    expect(dragged("checkbox", [295, 295], [295, 295], [300, 300])).toEqual([286, 286, 300, 300]);
  });
});
