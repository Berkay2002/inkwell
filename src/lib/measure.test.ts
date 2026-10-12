import { beforeEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./edit", () => ({ applyState: vi.fn(), setLeaveGuard: vi.fn(), queued: vi.fn() }));
vi.mock("./api", () => ({ pickSave: vi.fn() }));

import { useStore } from "../store";
import * as api from "./api";
import { applyState } from "./edit";
import { useMeasure } from "./measure";
import type { Measure } from "./measureLogic";

const state = { rev: 2, pageCount: 1, sizes: [], path: "a.pdf", dirty: true, undo: "Add", redo: null };
const m = (xref: number, extra: Partial<Measure> = {}): Measure => ({
  page: 0, xref, kind: "distance", points: [[0, 0], [72, 0]], contents: "10.00 ft", author: "", scale: null,
  quantity: { value: 10, unit: "ft", text: "10.00 ft" }, ...extra,
});
const calls = (name: string) => invoke.mock.calls.filter((c) => c[0] === name);

beforeEach(() => {
  invoke.mockReset();
  vi.mocked(applyState).mockReset();
  useMeasure.setState({ open: false, tool: "distance", docs: {}, locks: {} });
  useStore.setState({ docs: { 1: { id: 1, name: "plan.pdf" } as never } });
});

describe("measure store", () => {
  it("opens on Distance and goes back to Select when it closes", () => {
    useMeasure.getState().setOpen(true);
    expect(useMeasure.getState().tool).toBe("distance");
    useMeasure.getState().setTool("area");
    useMeasure.getState().setOpen(false);
    expect(useMeasure.getState().tool).toBe("select");
    useMeasure.getState().setOpen(true);
    expect(useMeasure.getState().tool).toBe("distance");
  });

  it("adds through the engine, selects the result and remembers it as the last", async () => {
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "measure_add") return { state, xref: 7, quantity: { value: 10, unit: "ft", text: "10.00 ft" } };
      if (cmd === "measure_list") return [m(7)];
      return null;
    });
    const x = await useMeasure.getState().add(1, 0, "distance", [[0, 0], [72, 0]]);
    expect(x).toBe(7);
    expect(applyState).toHaveBeenCalledWith(1, state);
    const d = useMeasure.getState().get(1);
    expect(d.items).toHaveLength(1);
    expect(d.selected).toEqual({ page: 0, xref: 7 });
    expect(d.last?.quantity.text).toBe("10.00 ft");
    expect(calls("measure_add")[0][1]).toMatchObject({ doc: 1, page: 0, kind: "distance", scale: null });
  });

  it("shows an engine refusal and keeps the list", async () => {
    invoke.mockRejectedValue("a distance needs exactly two points");
    expect(await useMeasure.getState().add(1, 0, "distance", [[0, 0]])).toBeNull();
    expect(useMeasure.getState().get(1).error).toBe("a distance needs exactly two points");
    expect(applyState).not.toHaveBeenCalled();
  });

  it("refuses a scale with a zero side without asking the engine", async () => {
    const ok = await useMeasure.getState().setPageScale(1, 0, { paper: 0, paperUnit: "in", real: 5, realUnit: "ft" });
    expect(ok).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
    expect(useMeasure.getState().get(1).error).toMatch(/above zero/);
  });

  it("calibrates: the drawn line and the typed length become the page scale", async () => {
    invoke.mockImplementation(async (cmd: string) =>
      cmd === "measure_scale_set" ? state : cmd === "measure_scale_get" ? { scale: { paper: 2, paperUnit: "in", real: 20, realUnit: "ft" }, defined: true, label: "2 in = 20 ft" } : [],
    );
    useMeasure.getState().startCalib(1, { page: 0, a: [0, 0], b: [144, 0] });
    expect(await useMeasure.getState().finishCalib(1, 20, "ft")).toBe(true);
    expect(calls("measure_scale_set")[0][1]).toEqual({ doc: 1, page: 0, scale: { paper: 1, paperUnit: "in", real: 10, realUnit: "ft" } });
    expect(useMeasure.getState().get(1).calib).toBeNull();
    expect(useMeasure.getState().get(1).scales[0].label).toBe("2 in = 20 ft");
    // a bad length leaves the line waiting
    useMeasure.getState().startCalib(1, { page: 0, a: [0, 0], b: [144, 0] });
    expect(await useMeasure.getState().finishCalib(1, 0, "ft")).toBe(false);
    expect(useMeasure.getState().get(1).calib).not.toBeNull();
  });

  it("rescale selects the replacement; a stale selection is dropped by the next list", async () => {
    invoke.mockImplementation(async (cmd: string) =>
      cmd === "measure_rescale" ? { state, xref: 9, quantity: { value: 3, unit: "m", text: "3.00 m" } } : cmd === "measure_list" ? [m(9)] : null,
    );
    useMeasure.setState({ docs: { 1: { items: [m(7)], selected: { page: 0, xref: 7 }, scales: {}, last: null, calib: null, exported: null, error: null } } });
    await useMeasure.getState().rescale(1, { page: 0, xref: 7 }, { paper: 1, paperUnit: "in", real: 1, realUnit: "m" });
    expect(useMeasure.getState().get(1).selected).toEqual({ page: 0, xref: 9 });
    invoke.mockResolvedValue([]);
    await useMeasure.getState().refresh(1);
    expect(useMeasure.getState().get(1).selected).toBeNull();
  });

  it("a certified document is read once for its reason, and nothing is written on it", async () => {
    const why = "This document is certified; its certification doesn't allow comments or measurements.";
    invoke.mockResolvedValue(why);
    await useMeasure.getState().ensureLock(1);
    await useMeasure.getState().ensureLock(1);
    expect(calls("measure_lock")).toHaveLength(1);
    expect(useMeasure.getState().locks[1]).toBe(why);
    invoke.mockClear();
    expect(await useMeasure.getState().add(1, 0, "distance", [[0, 0], [72, 0]])).toBeNull();
    await useMeasure.getState().setPageScale(1, 0, { paper: 1, paperUnit: "in", real: 5, realUnit: "ft" });
    await useMeasure.getState().remove(1, { page: 0, xref: 7 });
    expect(invoke).not.toHaveBeenCalled();
    expect(useMeasure.getState().get(1).error).toBe(why);
  });

  it("measures freely when nothing certifies the file, and reads the lock again when the file changes under it", async () => {
    invoke.mockResolvedValue(null);
    await useMeasure.getState().ensureLock(1);
    expect(useMeasure.getState().locks[1]).toBeNull();
    useMeasure.getState().onRev(1);
    expect(useMeasure.getState().locks[1]).toBeUndefined();
    await vi.waitFor(() => expect(calls("measure_lock")).toHaveLength(2));
  });

  it("an answer it cannot read counts as a refusal", async () => {
    invoke.mockRejectedValue("engine gone");
    await useMeasure.getState().ensureLock(1);
    expect(useMeasure.getState().locks[1]).toBe("engine gone");
  });

  it("exports through a save token, and does nothing when the dialog is cancelled", async () => {
    vi.mocked(api.pickSave).mockResolvedValueOnce(null);
    await useMeasure.getState().exportCsv(1);
    expect(invoke).not.toHaveBeenCalled();
    vi.mocked(api.pickSave).mockResolvedValueOnce({ token: "tok", path: "C:\\out\\plan measurements.csv" });
    invoke.mockResolvedValue("C:\\out\\plan measurements.csv");
    await useMeasure.getState().exportCsv(1);
    expect(api.pickSave).toHaveBeenLastCalledWith("plan measurements.csv", { doc: 1 }, "csv");
    expect(calls("measure_export_csv")[0][1]).toEqual({ doc: 1, to: "tok" });
    expect(useMeasure.getState().get(1).exported).toBe("C:\\out\\plan measurements.csv");
  });

  it("asks the engine with the kinds that are on, and not at all when none is", async () => {
    invoke.mockResolvedValue({ kind: "line", x: 5, y: 6, distance: 1 });
    expect(await useMeasure.getState().snap(1, 0, [5, 5], 8)).toMatchObject({ kind: "line" });
    expect(calls("measure_snap")[0][1]).toEqual({
      doc: 1, page: 0, x: 5, y: 5, radius: 8, kinds: { endpoint: true, midpoint: true, intersection: true, line: true },
    });
    useMeasure.getState().toggleSnap("midpoint");
    await useMeasure.getState().snap(1, 0, [5, 5], 8);
    expect(calls("measure_snap")[1][1].kinds).toMatchObject({ midpoint: false, line: true });
    invoke.mockClear();
    for (const k of ["endpoint", "midpoint", "intersection", "line"] as const) {
      if (useMeasure.getState().snaps[k]) useMeasure.getState().toggleSnap(k);
    }
    expect(await useMeasure.getState().snap(1, 0, [5, 5], 8)).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });
});
