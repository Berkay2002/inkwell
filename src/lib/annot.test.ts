import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { useAnnot } from "./annot";
import { type Annot, groupByPage, hitTest, placePopover, quadsFromRects, resizeRect, timeLabel, toScreen } from "./annotLogic";
import { applyState } from "./edit";

const inv = vi.mocked(invoke);
const a = (p: Partial<Annot>): Annot => ({
  page: 0, xref: 1, kind: "note", author: "A", contents: "", created: "D:20260101120000Z", modified: "", color: null, fill: null,
  opacity: 1, width: 1, rect: [0, 0, 10, 10], quads: [], points: [], strokes: [], icon: null, fontSize: null, parent: null,
  state: null, resolved: false, ...p,
});
const state = { rev: 2, pageCount: 1, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Add note", redo: null };

beforeEach(() => {
  vi.clearAllMocks();
  useAnnot.setState({ docs: {}, tool: "select", open: false });
});

describe("annot store", () => {
  it("add runs one command, applies its state, re-reads the list and selects the new one", async () => {
    inv.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "annot_add" ? { state, xref: 7 } : [a({ xref: 7, contents: "hi" })]),
    );
    useAnnot.setState({ author: "Berkay" });
    const x = await useAnnot.getState().add(1, 0, { type: "note", at: [5, 5], text: "hi" });
    expect(x).toBe(7);
    expect(inv).toHaveBeenCalledWith("annot_add", { doc: 1, page: 0, what: { type: "note", at: [5, 5], text: "hi" }, style: null, author: "Berkay" });
    expect(applyState).toHaveBeenCalledWith(1, state);
    expect(useAnnot.getState().get(1).selected).toEqual({ page: 0, xref: 7 });
  });

  it("keeps the error and no selection when the command fails", async () => {
    inv.mockRejectedValue("boom");
    expect(await useAnnot.getState().add(1, 0, { type: "note", at: [0, 0], text: "" })).toBeNull();
    expect(useAnnot.getState().get(1).error).toBe("boom");
  });

  it("closing the rail returns to the select tool", () => {
    useAnnot.getState().setOpen(true);
    useAnnot.getState().setTool("pen");
    useAnnot.getState().setOpen(false);
    expect(useAnnot.getState().tool).toBe("select");
  });
});

describe("annot logic", () => {
  it("groups roots by page in reading order, replies stay under their parent, filter and search apply", () => {
    const items = [
      a({ xref: 1, page: 1, rect: [0, 50, 1, 60], contents: "later" }),
      a({ xref: 2, page: 0, rect: [0, 5, 1, 6], contents: "first" }),
      a({ xref: 3, page: 1, rect: [0, 10, 1, 20], kind: "highlight" }),
      a({ xref: 4, page: 1, parent: 1, contents: "reply" }),
      a({ xref: 5, page: 0, resolved: true, contents: "done", rect: [0, 9, 1, 10] }),
    ];
    const g = groupByPage(items, "all", "");
    expect(g.map((x) => [x.page, x.items.map((i) => i.xref)])).toEqual([[0, [2, 5]], [1, [3, 1]]]);
    expect(groupByPage(items, "resolved", "").flatMap((x) => x.items.map((i) => i.xref))).toEqual([5]);
    expect(groupByPage(items, "all", "reply").flatMap((x) => x.items.map((i) => i.xref))).toEqual([1]);
  });

  it("hitTest picks the smallest box; resizeRect never inverts", () => {
    const items = [a({ xref: 1, rect: [0, 0, 100, 100] }), a({ xref: 2, rect: [10, 10, 20, 20] })];
    expect(hitTest(items, 0, [15, 15])?.xref).toBe(2);
    expect(hitTest(items, 0, [50, 50])?.xref).toBe(1);
    expect(hitTest(items, 1, [15, 15])).toBeNull();
    expect(resizeRect([0, 0, 20, 20], "e", -50, 0)).toEqual([0, 0, 6, 20]);
  });

  it("merges a line's pieces into one quad and drops slivers", () => {
    const q = quadsFromRects([[10, 0, 20, 10], [20, 0, 30, 10], [0, 40, 5, 50], [0, 0, 0.1, 10]]);
    expect(q).toEqual([[10, 0, 30, 0, 10, 10, 30, 10], [0, 40, 5, 40, 0, 50, 5, 50]]);
  });

  it("maps page points to screen under rotation", () => {
    expect(toScreen([10, 20], [100, 200], 0, 2)).toEqual([20, 40]);
    expect(toScreen([10, 20], [100, 200], 1, 1)).toEqual([180, 10]);
  });

  it("labels today by time, other days by date, junk by nothing", () => {
    const now = new Date(2026, 9, 11, 15, 0);
    expect(timeLabel("nope", now)).toBe("");
    expect(timeLabel("D:20200102030405Z", now)).toMatch(/^\d{1,2} \w{3} \d\d:\d\d$/);
  });
});

describe("popover placement", () => {
  const bounds: [number, number, number, number] = [0, 100, 1000, 700];
  it("goes above the selection, centred, when it fits", () => {
    expect(placePopover([400, 300, 500, 320], [200, 100], bounds)).toEqual([350, 190]);
  });
  it("flips below at the top edge and clamps at the sides", () => {
    expect(placePopover([10, 110, 30, 130], [200, 100], bounds)).toEqual([4, 140]);
    expect(placePopover([960, 400, 990, 420], [200, 100], bounds)[0]).toBe(796);
  });
  it("pins inside the bounds when neither side fits", () => {
    expect(placePopover([400, 200, 500, 680], [200, 300], bounds)[1]).toBe(396);
  });
});
