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
vi.mock("./edit", () => ({ queued: vi.fn((_doc: number, f: () => unknown) => f()), applyState: vi.fn() }));
vi.mock("./api", () => ({
  pickSave: vi.fn(),
  saveDocument: vi.fn(),
  editState: vi.fn(),
  toolCancel: vi.fn(() => Promise.resolve()),
  discardScratch: vi.fn(() => Promise.resolve(true)),
}));
vi.mock("./prepressApi", () => ({
  prepressApi: {
    separations: vi.fn(),
    view: vi.fn(),
    ink: vi.fn(),
    inkAt: vi.fn(),
    convert: vi.fn(),
    boxes: vi.fn(),
    marks: vi.fn(),
    pageBoxes: vi.fn(),
    marksPreview: vi.fn(),
    pickProfile: vi.fn(),
  },
}));

import { useStore } from "../store";
import * as api from "./api";
import { applyState } from "./edit";
import { usePlates } from "./platesView";
import { prepressApi, type PageBoxes } from "./prepressApi";
import { toPt } from "./prepressLogic";
import { newBox, usePrepress } from "./prepressStore";

const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
};
const st = () => usePrepress.getState();
const state = (rev: number) => ({ rev, path: "C:\\d\\new.pdf" }) as never;
const seps = { process: ["Cyan", "Magenta", "Yellow", "Black"], spots: [{ name: "PANTONE 123", rgb: [255, 200, 0], cmyk: [0, 20, 100, 0] }], overprint: true, folded: 0 };
const inkReport = { page: 2, planes: [], maxTac: 280, maxAt: [0, 0], tacLimit: 300, overLimit: false, overLimitArea: 0, regions: [], regionsTruncated: false, spots: [] };
const boxes: PageBoxes = {
  rotate: 0,
  media: [0, 0, 612, 792],
  crop: [0, 0, 612, 792],
  trim: [20, 20, 592, 772],
  bleed: [11, 11, 601, 781],
  art: [0, 0, 612, 792],
  stated: { trim: true, bleed: true, art: false, crop: false },
};
const report = { pages: [0], remaining: [false, false, true], usedProfile: false, notes: [] } as never;

let views = 0;
beforeEach(() => {
  vi.clearAllMocks();
  views = 0;
  usePrepress.setState({ open: false, docs: {}, sheet: null });
  usePlates.setState({ doc: null, view: null });
  useStore.setState({
    docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty: false, currentPage: 2, pageCount: 12, rev: 3, sizes: Array.from({ length: 12 }, () => [612, 792]) } as never },
    order: [1],
    active: 1,
  });
  vi.mocked(prepressApi.view).mockImplementation(async () => ++views);
  vi.mocked(prepressApi.separations).mockResolvedValue(seps as never);
  vi.mocked(prepressApi.ink).mockResolvedValue([inkReport] as never);
  vi.mocked(prepressApi.pageBoxes).mockResolvedValue(boxes);
  vi.mocked(prepressApi.marksPreview).mockResolvedValue({ media: [-10, -10, 622, 802], trim: [20, 20, 592, 772], bleed: [11, 11, 601, 781], content: "0.25 w 0 0 m 5 0 l S", notes: [] });
  vi.mocked(prepressApi.marks).mockResolvedValue({ state: state(4), notes: [] });
  vi.mocked(prepressApi.boxes).mockResolvedValue(state(4));
  vi.mocked(prepressApi.convert).mockResolvedValue({ state: state(4), report });
  vi.mocked(api.saveDocument).mockResolvedValue(state(5));
});

describe("Output Preview", () => {
  it("registers the view for the page and holds its id for the tiles", async () => {
    st().setOpen(true);
    await flush();
    expect(prepressApi.separations).toHaveBeenCalledWith(1, 2);
    expect(prepressApi.view).toHaveBeenCalledWith(1, { mode: "plates", process: [true, true, true, true], hiddenSpots: [], overprint: true, tacLimit: 300 });
    expect(usePlates.getState()).toMatchObject({ doc: 1, view: 1 });
    st().setOpen(false);
    expect(usePlates.getState().view).toBeNull();
  });

  it("switches plates, shows one alone and brings all back", async () => {
    st().setOpen(true);
    await flush();
    st().toggle(1, "p3");
    await flush();
    expect(vi.mocked(prepressApi.view).mock.lastCall?.[1]).toMatchObject({ process: [true, true, true, false] });
    st().solo(1, "s:PANTONE 123");
    await flush();
    expect(vi.mocked(prepressApi.view).mock.lastCall?.[1]).toMatchObject({ mode: "plates", process: [false, false, false, false], hiddenSpots: [] });
    st().solo(1, "s:PANTONE 123");
    await flush();
    expect(vi.mocked(prepressApi.view).mock.lastCall?.[1]).toMatchObject({ process: [true, true, true, true], hiddenSpots: [] });
    st().toggle(1, "p0");
    st().showAll(1);
    await flush();
    expect(st().get(1).settings.process).toEqual([true, true, true, true]);
  });

  it("takes a limit as typed, measures again and refuses nonsense", async () => {
    st().setOpen(true);
    await flush();
    expect(vi.mocked(prepressApi.ink).mock.calls[0].slice(1, 5)).toEqual([1, [2], 300, true]);
    expect(st().setLimit(1, "250%")).toBe(true);
    await flush();
    expect(vi.mocked(prepressApi.ink).mock.lastCall?.[3]).toBe(250);
    expect(vi.mocked(prepressApi.view).mock.lastCall?.[1]).toMatchObject({ tacLimit: 250 });
    expect(st().setLimit(1, "lots")).toBe(false);
    expect(st().setLimit(1, "5000")).toBe(false);
    expect(st().get(1).settings.tacLimit).toBe(250);
    expect(st().get(1).report?.data.maxTac).toBe(280);
  });

  it("asks for the ink under the pointer one point at a time and keeps the newest", async () => {
    st().setOpen(true);
    await flush();
    const answers: ((v: unknown) => void)[] = [];
    vi.mocked(prepressApi.inkAt).mockImplementation(() => new Promise((r) => answers.push(r)) as never);
    st().probe(1, 2, 10, 10);
    st().probe(1, 2, 20, 20);
    st().probe(1, 2, 30, 30);
    expect(prepressApi.inkAt).toHaveBeenCalledTimes(1);
    answers[0]({ page: 2, planes: [], total: 100 });
    await flush();
    expect(prepressApi.inkAt).toHaveBeenCalledTimes(2);
    expect(vi.mocked(prepressApi.inkAt).mock.lastCall).toEqual([1, 2, 30, 30]);
    answers[1]({ page: 2, planes: [], total: 250 });
    await flush();
    expect(st().get(1).probe?.at.total).toBe(250);
    st().clearProbe(1);
    expect(st().get(1).probe).toBeNull();
  });
});

describe("Convert Colors", () => {
  it("shows what will change before anything is touched", () => {
    st().openConvert();
    st().setConvert({ rgb: false, gray: false });
    st().review();
    expect(st().sheet).toMatchObject({ kind: "convert", phase: "options", error: "Choose which colours to convert." });
    st().setConvert({ rgb: true });
    st().review();
    expect(st().sheet).toMatchObject({ phase: "confirm", error: null });
    expect(prepressApi.convert).not.toHaveBeenCalled();
    st().backToOptions();
    expect(st().sheet).toMatchObject({ phase: "options" });
  });

  it("saves a new file: asks where, converts, then saves the document there", async () => {
    st().openConvert();
    st().review();
    vi.mocked(api.pickSave).mockResolvedValue({ token: "tok-new", path: "C:\\d\\new.pdf" });
    await st().runConvert();
    expect(api.pickSave).toHaveBeenCalledWith("a (CMYK).pdf", { doc: 1 }, "pdf");
    expect(prepressApi.convert).toHaveBeenCalledTimes(1);
    expect(vi.mocked(prepressApi.convert).mock.calls[0].slice(1, 4)).toEqual([
      1,
      { gray: true, rgb: true, spot: false, grayToK: true, preserveBlack: true, intent: "device" },
      null,
    ]);
    expect(api.saveDocument).toHaveBeenCalledWith(1, "tok-new");
    expect(applyState).toHaveBeenCalledTimes(2);
    expect(st().sheet).toMatchObject({ phase: "done", result: { path: "C:\\d\\new.pdf" } });
  });

  it("never writes over the original file or one that is open in another tab", async () => {
    useStore.setState({
      docs: {
        ...useStore.getState().docs,
        2: { id: 2, path: "C:\\d\\other.pdf", name: "other.pdf", dirty: false, currentPage: 0, pageCount: 1, rev: 0, sizes: [[612, 792]] } as never,
      },
    });
    st().openConvert(1);
    st().review();
    vi.mocked(api.pickSave).mockResolvedValue({ token: "tok-same", path: "c:/D/A.pdf" });
    await st().runConvert();
    expect(prepressApi.convert).not.toHaveBeenCalled();
    expect(st().sheet).toMatchObject({ phase: "confirm", error: expect.stringMatching(/own file/) });
    vi.mocked(api.pickSave).mockResolvedValue({ token: "tok-other", path: "C:\\d\\other.pdf" });
    await st().runConvert();
    expect(prepressApi.convert).not.toHaveBeenCalled();
    expect(st().sheet).toMatchObject({ error: expect.stringMatching(/open in another tab/) });
    expect(api.saveDocument).not.toHaveBeenCalled();
  });

  it("a dismissed file dialog changes nothing", async () => {
    st().openConvert();
    st().review();
    vi.mocked(api.pickSave).mockResolvedValue(null);
    await st().runConvert();
    expect(prepressApi.convert).not.toHaveBeenCalled();
    expect(st().sheet).toMatchObject({ phase: "confirm" });
  });

  it("in place is one edit step and no save", async () => {
    st().openConvert();
    st().setConvert({ output: "inPlace", scope: "page" });
    st().review();
    await st().runConvert();
    expect(api.pickSave).not.toHaveBeenCalled();
    expect(api.saveDocument).not.toHaveBeenCalled();
    expect(vi.mocked(prepressApi.convert).mock.calls[0][2]).toMatchObject({ pages: [2] });
    expect(applyState).toHaveBeenCalledTimes(1);
    expect(st().sheet).toMatchObject({ phase: "done", result: { path: null } });
  });

  it("a cancelled run says nothing was changed and goes back to the confirmation", async () => {
    st().openConvert();
    st().setConvert({ output: "inPlace" });
    st().review();
    vi.mocked(prepressApi.convert).mockRejectedValue("cancelled");
    vi.mocked(api.editState).mockResolvedValue(state(3));
    await st().runConvert();
    expect(st().sheet).toMatchObject({ phase: "confirm", error: "Cancelled. Nothing was changed." });
  });

  it("uses the profile file that was picked", async () => {
    st().openConvert();
    vi.mocked(prepressApi.pickProfile).mockResolvedValue({ token: "tok-icc", path: "C:\\p\\FOGRA39.icc" });
    await st().pickProfile();
    expect(st().sheet).toMatchObject({ opts: { intent: "profile", profile: { token: "tok-icc", name: "FOGRA39.icc" } } });
    st().setConvert({ output: "inPlace" });
    st().review();
    await st().runConvert();
    expect(vi.mocked(prepressApi.convert).mock.calls[0][3]).toBe("tok-icc");
    expect(vi.mocked(prepressApi.convert).mock.calls[0][2]).toMatchObject({ intent: "profile", condition: "FOGRA39" });
  });
});

describe("Printer's marks", () => {
  it("previews the current page and asks again only when something changed", async () => {
    st().openMarks();
    await flush();
    expect(prepressApi.pageBoxes).toHaveBeenCalledWith(1, 2);
    expect(vi.mocked(prepressApi.marksPreview).mock.calls[0].slice(1, 2)).toEqual([2]);
    expect(vi.mocked(prepressApi.marksPreview).mock.calls[0][2]).toMatchObject({ pages: [2], marks: { crop: true, bleed: true, registration: true, colorBars: true } });
    expect(st().sheet).toMatchObject({ preview: { data: { notes: [] } } });
    st().setMarks({ units: "mm" });
    await flush();
    expect(prepressApi.marksPreview).toHaveBeenCalledTimes(1);
    st().setMarks({ weight: 0.5 });
    await flush();
    expect(prepressApi.marksPreview).toHaveBeenCalledTimes(2);
    for (const k of ["crop", "bleed", "registration"] as const) st().setMarkKind(k, false);
    await flush();
    expect(prepressApi.marksPreview).toHaveBeenCalledTimes(5);
    st().setMarkKind("colorBars", false);
    await flush();
    // nothing left to draw: no question, no stale picture
    expect(st().sheet).toMatchObject({ preview: null });
    expect(prepressApi.marksPreview).toHaveBeenCalledTimes(5);
  });

  it("adds the marks to the chosen pages as one step", async () => {
    st().openMarks();
    await flush();
    st().setMarks({ scope: "range", range: "1-2" });
    await st().applyMarks();
    expect(prepressApi.marks).toHaveBeenCalledTimes(1);
    expect(vi.mocked(prepressApi.marks).mock.calls[0][1]).toMatchObject({ pages: [0, 1], weight: 0.25, length: 18 });
    expect(applyState).toHaveBeenCalledWith(1, expect.objectContaining({ rev: 4 }));
    expect(st().sheet).toMatchObject({ busy: false, done: "Added printer's marks to 2 pages." });
  });

  it("does not run with no marks or a range that does not parse", async () => {
    st().openMarks();
    await flush();
    st().setMarks({ scope: "range", range: "banana" });
    await st().applyMarks();
    expect(st().sheet).toMatchObject({ error: "The page range is not valid." });
    expect(prepressApi.marks).not.toHaveBeenCalled();
  });
});

describe("Page boxes", () => {
  it("starts from what the page has, measured from the box around it", async () => {
    st().openBoxes();
    await flush();
    const s = st().sheet as never as { box: string; ref: string; margins: Record<string, number> };
    expect(s).toMatchObject({ kind: "boxes", box: "trim", ref: "bleed" });
    expect(s.margins).toEqual({ left: 9, bottom: 9, right: 9, top: 9 });
  });

  it("sets a 3 mm bleed from the trim on every page with one request", async () => {
    st().openBoxes();
    await flush();
    st().bleedPreset();
    const mm = toPt(3, "mm");
    expect(st().sheet).toMatchObject({ box: "bleed", ref: "trim", units: "mm", linked: true });
    const sheet = st().sheet as Parameters<typeof newBox>[0];
    expect(newBox(sheet)?.map((v) => +v.toFixed(3))).toEqual([20 - mm, 20 - mm, 592 + mm, 772 + mm].map((v) => +v.toFixed(3)));
    await st().applyBoxes();
    expect(prepressApi.boxes).toHaveBeenCalledTimes(1);
    expect(vi.mocked(prepressApi.boxes).mock.calls[0][1]).toEqual({ kind: "bleed", from: "trim", margin: [mm, mm, mm, mm] });
    expect(applyState).toHaveBeenCalledTimes(1);
    expect(st().sheet).toMatchObject({ busy: false, done: "Set the bleed box on 12 pages." });
  });

  it("keeps a box the page could not have from reaching the engine", async () => {
    st().openBoxes();
    await flush();
    st().setBoxes({ linked: true });
    st().setMargin("left", 500);
    await st().applyBoxes();
    expect(prepressApi.boxes).not.toHaveBeenCalled();
    expect(st().sheet).toMatchObject({ error: expect.stringMatching(/empty|inside|beyond/) });
  });

  it("limits the change to the current page or a range", async () => {
    st().openBoxes();
    await flush();
    st().bleedPreset();
    st().setBoxes({ scope: "page" });
    await st().applyBoxes();
    expect(vi.mocked(prepressApi.boxes).mock.calls[0][1]).toMatchObject({ pages: [2] });
    st().setBoxes({ scope: "range", range: "1, 3-4" });
    await st().applyBoxes();
    expect(vi.mocked(prepressApi.boxes).mock.calls[1][1]).toMatchObject({ pages: [0, 2, 3] });
  });
});

describe("closing a document", () => {
  it("takes its Output Preview state and open sheet with it", async () => {
    st().setOpen(true);
    await flush();
    st().openMarks();
    useStore.setState({ docs: {}, order: [], active: null });
    expect(st().docs[1]).toBeUndefined();
    expect(st().sheet).toBeNull();
    expect(usePlates.getState().doc).toBeNull();
  });
});
