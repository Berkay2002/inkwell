import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));
const setUi = vi.hoisted(() => vi.fn());
vi.mock("../store", () => ({ useStore: { getState: () => ({ setUi }) } }));
vi.mock("./api", async (orig) => ({ ...(await orig<typeof import("./api")>()), pickSource: vi.fn() }));

import { invoke } from "@tauri-apps/api/core";
import { useAnnot } from "./annot";
import { pickSource } from "./api";
import { useStamps } from "./stamps";
import {
  type Stamp, SIGNED_NOTE, byCategory, categoriesOf, categoryProblem, customId, customValue, fitAspect, isPdfPath, nameFromPath, nameProblem,
} from "./stampsLogic";

const inv = vi.mocked(invoke);
const pick = vi.mocked(pickSource);
const st = (id: string, name: string, category = "Custom"): Stamp => ({ id, name, category, width: 200, height: 100 });
const reset = () => useStamps.setState({ stamps: [], loaded: false, busy: false, error: null, thumbs: {}, gates: {}, managerOpen: false, draft: null });

beforeEach(() => {
  vi.clearAllMocks();
  reset();
  useAnnot.setState({ stamp: "Approved" });
});

describe("stampsLogic", () => {
  it("groups by category, Custom first, names in natural order", () => {
    const g = byCategory([st("a", "b2", "Zed"), st("b", "b10", "Zed"), st("c", "x"), st("d", "a", "Alpha"), st("e", "b1", "Zed")]);
    expect(g.map((x) => x.category)).toEqual(["Custom", "Alpha", "Zed"]);
    expect(g[2].stamps.map((s) => s.name)).toEqual(["b1", "b2", "b10"]);
    expect(categoriesOf([])).toEqual([]);
  });

  it("round-trips the picker value and leaves standard names alone", () => {
    expect(customId(customValue("0123abcd"))).toBe("0123abcd");
    expect(customId("Approved")).toBeNull();
    expect(customId("Date")).toBeNull();
  });

  it("names a stamp from the file stem on either path style, capped, and spots a PDF", () => {
    expect(nameFromPath("C:\\Users\\a\\logo.final.png")).toBe("logo.final");
    expect(nameFromPath("/tmp/Paid stamp.PDF")).toBe("Paid stamp");
    expect(nameFromPath("/tmp/.hidden")).toBe(".hidden");
    expect([...nameFromPath(`/x/${"é".repeat(300)}.png`)]).toHaveLength(128);
    expect(isPdfPath("a.PDF")).toBe(true);
    expect(isPdfPath("a.png")).toBe(false);
  });

  it("checks the same limits as the engine, in characters", () => {
    expect(nameProblem("   ")).toMatch(/name/);
    expect(nameProblem("é".repeat(128))).toBeNull();
    expect(nameProblem("é".repeat(129))).toMatch(/128/);
    expect(categoryProblem("")).toBeNull();
    expect(categoryProblem("c".repeat(65))).toMatch(/64/);
  });

  it("keeps a picture's proportions on resize and anchors the sides that did not move", () => {
    const from: [number, number, number, number] = [100, 100, 300, 200]; // 2:1
    // Dragged the right edge to 500, bottom unchanged: height wins from the width, top-left stays.
    expect(fitAspect(from, [100, 100, 500, 200], 2)).toEqual([100, 100, 300, 200]);
    // Dragged the bottom edge down to 400: width follows, left stays.
    const r = fitAspect(from, [100, 100, 300, 400], 2);
    expect(r).toEqual([100, 100, 300, 200]);
    // Dragged the left edge to 0: anchored on the right.
    expect(fitAspect(from, [0, 100, 300, 200], 2)).toEqual([100, 100, 300, 200]);
    // A degenerate box or aspect is left as it is.
    expect(fitAspect(from, [10, 10, 10, 50], 2)).toEqual([10, 10, 10, 50]);
    expect(fitAspect(from, [0, 0, 10, 10], 0)).toEqual([0, 0, 10, 10]);
  });

  it("says what signed documents are told", () => {
    expect(SIGNED_NOTE).toMatch(/signed/);
  });
});

describe("stamps store", () => {
  it("loads the library and shows its warning", async () => {
    inv.mockResolvedValue({ stamps: [st("a", "A")], warning: "The list was damaged" });
    await useStamps.getState().load();
    expect(inv).toHaveBeenCalledWith("stamp_list");
    expect(useStamps.getState().stamps).toHaveLength(1);
    expect(useStamps.getState().loaded).toBe(true);
    expect(setUi).toHaveBeenCalledWith({ error: "The list was damaged" });
  });

  it("a failed load leaves an error and an empty, loaded library", async () => {
    inv.mockRejectedValue("disk gone");
    await useStamps.getState().load();
    expect(useStamps.getState()).toMatchObject({ loaded: true, stamps: [], error: "disk gone" });
  });

  it("makes a stamp from a picked picture: draft, then one create call, then listed", async () => {
    pick.mockResolvedValue([{ token: "tok", path: "C:\\x\\Logo.png" }] as never);
    await useStamps.getState().pick();
    expect(pick).toHaveBeenCalledWith("stamp", false);
    expect(useStamps.getState().draft).toMatchObject({ token: "tok", name: "Logo", category: "Custom", pdf: false });
    useStamps.getState().setDraft({ category: "Brand" });
    inv.mockResolvedValue(st("n1", "Logo", "Brand"));
    expect(await useStamps.getState().make()).toBe(true);
    expect(inv).toHaveBeenCalledWith("stamp_create", { file: "tok", name: "Logo", category: "Brand", page: null });
    expect(useStamps.getState().stamps).toEqual([st("n1", "Logo", "Brand")]);
    expect(useStamps.getState().draft).toBeNull();
  });

  it("sends the 0-based page for a PDF and clamps a bad page number", async () => {
    pick.mockResolvedValue([{ token: "t", path: "/a/Paid.pdf" }] as never);
    await useStamps.getState().pick();
    expect(useStamps.getState().draft?.pdf).toBe(true);
    inv.mockResolvedValue(st("n", "Paid"));
    useStamps.getState().setDraft({ page: "3" });
    await useStamps.getState().make();
    expect(inv).toHaveBeenLastCalledWith("stamp_create", { file: "t", name: "Paid", category: "Custom", page: 2 });
    pick.mockResolvedValue([{ token: "t2", path: "/a/Paid.pdf" }] as never);
    await useStamps.getState().pick();
    useStamps.getState().setDraft({ page: "abc" });
    await useStamps.getState().make();
    expect(inv).toHaveBeenLastCalledWith("stamp_create", { file: "t2", name: "Paid", category: "Custom", page: 0 });
  });

  it("keeps the draft and shows the engine's message when the create is refused", async () => {
    pick.mockResolvedValue([{ token: "t", path: "/a/big.png" }] as never);
    await useStamps.getState().pick();
    inv.mockRejectedValue("That picture is too large");
    expect(await useStamps.getState().make()).toBe(false);
    expect(useStamps.getState().error).toBe("That picture is too large");
    expect(useStamps.getState().draft).not.toBeNull();
    expect(useStamps.getState().busy).toBe(false);
  });

  it("refuses an empty name without calling the engine; a cancelled dialog makes no draft", async () => {
    pick.mockResolvedValue([{ token: "t", path: "/a/b.png" }] as never);
    await useStamps.getState().pick();
    useStamps.getState().setDraft({ name: "  " });
    expect(await useStamps.getState().make()).toBe(false);
    expect(inv).not.toHaveBeenCalled();
    useStamps.getState().cancelDraft();
    pick.mockResolvedValue(null as never);
    await useStamps.getState().pick();
    expect(useStamps.getState().draft).toBeNull();
  });

  it("renames and re-files through one update each, and does nothing when unchanged", async () => {
    useStamps.setState({ stamps: [st("a", "Old", "Custom")] });
    inv.mockResolvedValue(st("a", "New", "Custom"));
    await useStamps.getState().rename("a", "New");
    expect(inv).toHaveBeenCalledWith("stamp_update", { id: "a", name: "New", category: null });
    expect(useStamps.getState().stamps[0].name).toBe("New");
    inv.mockClear();
    await useStamps.getState().rename("a", " New ");
    await useStamps.getState().refile("a", "");
    expect(inv).not.toHaveBeenCalled();
    inv.mockResolvedValue(st("a", "New", "Work"));
    await useStamps.getState().refile("a", "Work");
    expect(inv).toHaveBeenCalledWith("stamp_update", { id: "a", name: null, category: "Work" });
    expect(useStamps.getState().stamps[0].category).toBe("Work");
  });

  it("removing the selected custom stamp puts the picker back on a standard one", async () => {
    useStamps.setState({ stamps: [st("a", "A"), st("b", "B")] });
    useAnnot.setState({ stamp: customValue("a") });
    inv.mockResolvedValue(undefined);
    await useStamps.getState().remove("b");
    expect(useAnnot.getState().stamp).toBe(customValue("a"));
    await useStamps.getState().remove("a");
    expect(useAnnot.getState().stamp).toBe("Approved");
    expect(useStamps.getState().stamps).toEqual([]);
  });

  it("a failed delete keeps the stamp", async () => {
    useStamps.setState({ stamps: [st("a", "A")] });
    inv.mockRejectedValue("locked");
    await useStamps.getState().remove("a");
    expect(useStamps.getState().stamps).toHaveLength(1);
    expect(useStamps.getState().error).toBe("locked");
  });

  it("keeps the newest gate read per document", async () => {
    let first!: (g: unknown) => void;
    inv.mockImplementationOnce(() => new Promise((r) => (first = r)));
    inv.mockResolvedValueOnce({ reason: null, signed: true });
    const slow = useStamps.getState().readGate(1);
    await useStamps.getState().readGate(1);
    first({ reason: "certified", signed: false });
    await slow;
    expect(useStamps.getState().gates[1]).toEqual({ reason: null, signed: true });
  });

  it("opening the manager reloads the library", async () => {
    inv.mockResolvedValue({ stamps: [st("a", "A")], warning: null });
    useStamps.getState().openManager();
    expect(useStamps.getState().managerOpen).toBe(true);
    await vi.waitFor(() => expect(useStamps.getState().stamps).toHaveLength(1));
  });
});
