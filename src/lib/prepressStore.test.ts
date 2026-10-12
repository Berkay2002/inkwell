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
  editUndo: vi.fn(),
  editUndoIf: vi.fn(),
  toolCancel: vi.fn(() => Promise.resolve()),
  discardScratch: vi.fn(() => Promise.resolve(true)),
}));
vi.mock("./prepressApi", () => ({
  prepressApi: {
    separations: vi.fn(),
    view: vi.fn(),
    ink: vi.fn(),
    inkAt: vi.fn(),
    inspect: vi.fn(),
    convert: vi.fn(),
    saveOther: vi.fn(),
    protection: vi.fn(),
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
import { usePlates, viewFailed } from "./platesView";
import { prepressApi, type PageBoxes } from "./prepressApi";
import { toPt } from "./prepressLogic";
import { currentSeps, inspectCapture, newBox, usePrepress } from "./prepressStore";

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
  vi.mocked(prepressApi.convert).mockResolvedValue({ state: state(4), report, saved: false, saveError: null });
  vi.mocked(prepressApi.protection).mockResolvedValue({ certified: false, signed: false });
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

  it("drops an answer that was asked before the pointer left or the document changed", async () => {
    st().setOpen(true);
    await flush();
    const answers: ((v: unknown) => void)[] = [];
    vi.mocked(prepressApi.inkAt).mockImplementation(() => new Promise((r) => answers.push(r)) as never);
    st().probe(1, 2, 10, 10);
    st().clearProbe(1);
    answers[0]({ page: 2, planes: [], total: 100 });
    await flush();
    expect(st().get(1).probe).toBeNull();
    st().probe(1, 2, 20, 20);
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 4 } } });
    answers[1]({ page: 2, planes: [], total: 120 });
    await flush();
    expect(st().get(1).probe).toBeNull();
    // the next answer is for the new generation and is kept with it
    st().probe(1, 2, 30, 30);
    answers[2]({ page: 2, planes: [], total: 130 });
    await flush();
    expect(st().get(1).probe).toMatchObject({ page: 2, rev: 4, at: { total: 130 } });
  });

  it("names plates from the inks of the page and generation on screen only", async () => {
    st().setOpen(true);
    await flush();
    const o = st().get(1);
    const d = useStore.getState().docs[1];
    expect(currentSeps(o, d)).toEqual(seps);
    expect(currentSeps(o, { ...d, currentPage: 3 })).toBeNull();
    expect(currentSeps(o, { ...d, rev: 9 })).toBeNull();
  });

  it("asks for the view again when its tiles fail", async () => {
    st().setOpen(true);
    await flush();
    expect(usePlates.getState().view).toBe(1);
    viewFailed(7); // a view that is no longer the one in use does not count
    await flush();
    expect(prepressApi.view).toHaveBeenCalledTimes(1);
    viewFailed(1);
    await flush();
    expect(prepressApi.view).toHaveBeenCalledTimes(2);
    expect(usePlates.getState().view).toBe(2);
  });
});

describe("object inspector", () => {
  const found = [{ kind: "fill", colorSpace: "DeviceCMYK", model: "cmyk", colorants: [], values: [0, 0, 0, 1], cmyk: [0, 0, 0, 100], ink: 100, overprint: true, overprintMode: false, alpha: 1, bounds: [0, 0, 5, 5], font: null, image: null }];
  const answers: ((v: unknown) => void)[] = [];
  const ask = async () => {
    st().setOpen(true);
    await flush();
    st().setInspecting(1, true);
    answers.length = 0;
    vi.mocked(prepressApi.inspect).mockImplementation(() => new Promise((r) => answers.push(r)) as never);
  };

  it("lists what a click finds with the point it was asked at, and keeps the document clean", async () => {
    await ask();
    const before = useStore.getState().docs[1];
    const done = st().inspectAt(1, 2, 40, 50);
    expect(st().get(1).inspect).toMatchObject({ on: true, at: { page: 2, rev: 3, x: 40, y: 50 }, found: null });
    expect(prepressApi.inspect).toHaveBeenCalledWith(1, 2, 40, 50);
    answers[0](found);
    await done;
    expect(st().get(1).inspect.found).toEqual(found);
    // read-only: no edit, no new generation, not dirty
    expect(applyState).not.toHaveBeenCalled();
    expect(useStore.getState().docs[1]).toBe(before);
    expect(useStore.getState().docs[1].dirty).toBe(false);
  });

  it("keeps an empty answer as an empty list", async () => {
    await ask();
    const done = st().inspectAt(1, 2, 1, 1);
    answers[0]([]);
    await done;
    expect(st().get(1).inspect.found).toEqual([]);
  });

  it("drops the answer of an older click", async () => {
    await ask();
    const a = st().inspectAt(1, 2, 10, 10);
    const b = st().inspectAt(1, 2, 20, 20);
    answers[1](found);
    await b;
    answers[0]([]);
    await a;
    expect(st().get(1).inspect).toMatchObject({ at: { x: 20, y: 20 }, found });
  });

  it("drops an answer when the generation moved, the page went, or the mode ended, while it was asked", async () => {
    await ask();
    const a = st().inspectAt(1, 2, 10, 10);
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 4 } } });
    answers[0](found);
    await a;
    expect(st().get(1).inspect).toMatchObject({ on: true, at: null, found: null });

    const b = st().inspectAt(1, 2, 10, 10);
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], pageCount: 2 } } });
    answers[1](found);
    await b;
    expect(st().get(1).inspect.at).toBeNull();

    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], pageCount: 12 } } });
    const c = st().inspectAt(1, 5, 10, 10);
    st().setInspecting(1, false);
    answers[2](found);
    await c;
    expect(st().get(1).inspect).toEqual({ on: false, at: null, found: null, error: null });
    // and a click outside the mode asks nothing
    await st().inspectAt(1, 5, 1, 1);
    expect(prepressApi.inspect).toHaveBeenCalledTimes(3);
  });

  it("keeps a result on another page than the current one while the view scrolls", async () => {
    await ask();
    // continuous scroll: page 5 is clicked while page 2 is the current one, then the current page moves
    const a = st().inspectAt(1, 5, 10, 10);
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], currentPage: 3 } } });
    answers[0](found);
    await a;
    expect(st().get(1).inspect).toMatchObject({ at: { page: 5 }, found });
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], currentPage: 6 } } });
    expect(st().get(1).inspect.found).toEqual(found);
    // a new generation takes it, and so does losing its page
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 4 } } });
    expect(st().get(1).inspect.at).toBeNull();
    const b = st().inspectAt(1, 5, 10, 10);
    answers[1](found);
    await b;
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], pageCount: 5 } } });
    expect(st().get(1).inspect.at).toBeNull();
  });

  it("clears the marker and keeps the mode; closing Output Preview leaves the mode", async () => {
    await ask();
    const a = st().inspectAt(1, 2, 10, 10);
    answers[0](found);
    await a;
    st().clearInspect(1);
    expect(st().get(1).inspect).toMatchObject({ on: true, at: null, found: null });
    const b = st().inspectAt(1, 2, 10, 10);
    answers[1](found);
    await b;
    st().setOpen(false);
    expect(st().get(1).inspect).toEqual({ on: false, at: null, found: null, error: null });
  });

  it("takes a press before anything on the page hears it, and a click asks the engine", async () => {
    await ask();
    const ev = () => ({ stopPropagation: vi.fn(), preventDefault: vi.fn() });
    const h = inspectCapture(1, 2, () => [7, 8]);
    for (const down of [h.onPointerDownCapture, h.onMouseDownCapture]) {
      const e = ev();
      down(e);
      expect(e.stopPropagation).toHaveBeenCalledOnce();
      expect(e.preventDefault).toHaveBeenCalledOnce();
    }
    expect(prepressApi.inspect).not.toHaveBeenCalled();
    const c = ev();
    h.onClickCapture(c);
    expect(c.stopPropagation).toHaveBeenCalledOnce();
    expect(c.preventDefault).toHaveBeenCalledOnce();
    expect(prepressApi.inspect).toHaveBeenCalledWith(1, 2, 7, 8);
  });

  it("shows the engine's refusal and still allows the next click", async () => {
    await ask();
    vi.mocked(prepressApi.inspect).mockRejectedValueOnce("the point is outside the page");
    await st().inspectAt(1, 2, 10, 10);
    expect(st().get(1).inspect).toMatchObject({ on: true, found: null, error: "the point is outside the page" });
    const c = st().inspectAt(1, 2, 20, 20);
    answers[0](found);
    await c;
    expect(st().get(1).inspect).toMatchObject({ error: null, found });
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
    vi.mocked(prepressApi.convert).mockResolvedValue({ state: state(5), report, saved: true, saveError: null });
    await st().runConvert();
    expect(api.pickSave).toHaveBeenCalledWith("a (CMYK).pdf", { doc: 1 }, "pdf");
    // one command does both, so nothing can land between the conversion and the save
    expect(prepressApi.convert).toHaveBeenCalledTimes(1);
    expect(vi.mocked(prepressApi.convert).mock.calls[0].slice(1, 5)).toEqual([
      1,
      { gray: true, rgb: true, spot: false, grayToK: true, preserveBlack: true, intent: "device" },
      null,
      "tok-new",
    ]);
    expect(api.saveDocument).not.toHaveBeenCalled();
    expect(applyState).toHaveBeenCalledTimes(1);
    expect(st().sheet).toMatchObject({ phase: "done", result: { path: "C:\\d\\new.pdf" } });
  });

  it("a failed write keeps the conversion and offers the save again, never a second conversion", async () => {
    st().openConvert();
    st().review();
    vi.mocked(api.pickSave).mockResolvedValue({ token: "tok-1", path: "C:\\d\\new.pdf" });
    vi.mocked(prepressApi.convert).mockResolvedValue({ state: state(4), report, saved: false, saveError: "Access is denied." });
    await st().runConvert();
    expect(applyState).toHaveBeenCalledTimes(1);
    expect(st().sheet).toMatchObject({ phase: "unsaved", error: expect.stringMatching(/converted in the open document.*Access is denied/) });
    vi.mocked(api.pickSave).mockResolvedValue({ token: "tok-2", path: "C:\\d\\other-name.pdf" });
    vi.mocked(prepressApi.saveOther).mockResolvedValue({ rev: 4, path: "C:\\d\\other-name.pdf" } as never);
    await st().saveConverted();
    expect(prepressApi.saveOther).toHaveBeenCalledWith(1, "tok-2");
    expect(prepressApi.convert).toHaveBeenCalledTimes(1);
    expect(st().sheet).toMatchObject({ phase: "done", error: null, result: { path: "C:\\d\\other-name.pdf" } });
  });

  describe("closing after a failed write", () => {
    const unsaved = async () => {
      st().openConvert();
      st().review();
      vi.mocked(api.pickSave).mockResolvedValue({ token: "tok-1", path: "C:\\d\\new.pdf" });
      vi.mocked(prepressApi.convert).mockResolvedValue({ state: state(4), report, saved: false, saveError: "Access is denied." });
      await st().runConvert();
      // applyState is mocked: the document is at the generation the conversion made
      useStore.setState((s) => ({ docs: { ...s.docs, 1: { ...s.docs[1], rev: 4 } } }));
    };

    it("asks first, and a second close goes back to the sheet", async () => {
      await unsaved();
      st().closeSheet();
      expect(st().sheet).toMatchObject({ phase: "unsaved", closing: true });
      st().closeSheet();
      expect(st().sheet).toMatchObject({ phase: "unsaved", closing: false });
      expect(api.editUndo).not.toHaveBeenCalled();
    });

    it("Undo takes the conversion back and closes", async () => {
      await unsaved();
      vi.mocked(api.editUndoIf).mockResolvedValue(state(3));
      st().closeSheet();
      await st().undoConversion();
      // asked for the generation the conversion made: the engine compares it under its edit lock
      expect(api.editUndoIf).toHaveBeenCalledExactlyOnceWith(1, 4);
      expect(api.editUndo).not.toHaveBeenCalled();
      expect(applyState).toHaveBeenLastCalledWith(1, state(3));
      expect(st().sheet).toBeNull();
    });

    it("closing the document (tab or window) goes back to the sheet's question instead of a Save prompt", async () => {
      await unsaved();
      expect(st().holdConversion(2)).toBe(false);
      expect(st().sheet).toMatchObject({ closing: false });
      expect(st().holdConversion(1)).toBe(true);
      expect(st().sheet).toMatchObject({ phase: "unsaved", closing: true });
      st().keepConversion();
      expect(st().holdConversion(1)).toBe(false);
    });

    it("Keep closes and leaves the document as it is", async () => {
      await unsaved();
      st().closeSheet();
      st().keepConversion();
      expect(api.editUndoIf).not.toHaveBeenCalled();
      expect(st().sheet).toBeNull();
    });

    it("does not undo a step that is not the conversion, and says so instead of closing", async () => {
      await unsaved();
      // an assistant step landed while the sheet was open: the engine refuses the undo
      vi.mocked(api.editUndoIf).mockRejectedValueOnce("stale");
      st().closeSheet();
      await st().undoConversion();
      expect(api.editUndo).not.toHaveBeenCalled();
      expect(applyState).not.toHaveBeenCalledWith(1, state(3));
      expect(st().sheet).toMatchObject({ phase: "unsaved", closing: true, error: expect.stringMatching(/changed since the conversion; nothing was undone/) });
      // Keep still closes it
      st().keepConversion();
      expect(st().sheet).toBeNull();
    });

    it("keeps the sheet and says so when the undo fails", async () => {
      await unsaved();
      vi.mocked(api.editUndoIf).mockRejectedValueOnce("Nothing to undo");
      st().closeSheet();
      await st().undoConversion();
      expect(st().sheet).toMatchObject({ phase: "unsaved", error: "Nothing to undo" });
    });
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
    expect(vi.mocked(prepressApi.convert).mock.calls[0][4]).toBeNull();
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

  it("an older preview that fails does not replace a newer one", async () => {
    const asked: { ok: (v: never) => void; fail: (e: unknown) => void }[] = [];
    vi.mocked(prepressApi.marksPreview).mockImplementation(() => new Promise((ok, fail) => asked.push({ ok: ok as never, fail })));
    st().openMarks();
    await flush();
    st().setMarks({ weight: 0.5 });
    await flush();
    expect(asked).toHaveLength(2);
    asked[1].ok({ media: [-10, -10, 622, 802], trim: [20, 20, 592, 772], bleed: [11, 11, 601, 781], content: "", notes: [] } as never);
    await flush();
    asked[0].fail("The first layout could not be drawn.");
    await flush();
    expect(st().sheet).toMatchObject({ error: null, preview: { data: { notes: [] } } });
    // the newest one failing is still shown
    st().setMarks({ weight: 1 });
    await flush();
    asked[2].fail("Could not draw the marks.");
    await flush();
    expect(st().sheet).toMatchObject({ preview: null, error: "Could not draw the marks." });
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
    // as shown: each page maps the margins by its own /Rotate in the engine
    expect(vi.mocked(prepressApi.boxes).mock.calls[0][1]).toEqual({ kind: "bleed", from: "trim", margin: [mm, mm, mm, mm], shown: true });
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

describe("certified and signed documents", () => {
  it("refuses all three edits on a certified document and says why", async () => {
    vi.mocked(prepressApi.protection).mockResolvedValue({ certified: true, signed: true });
    st().openMarks();
    await flush();
    expect(st().get(1).protection).toMatchObject({ certified: true, signed: true });
    await st().applyMarks();
    expect(st().sheet).toMatchObject({ error: expect.stringMatching(/certified/) });
    st().openBoxes();
    await flush();
    await st().applyBoxes();
    expect(st().sheet).toMatchObject({ error: expect.stringMatching(/certified/) });
    st().openConvert();
    st().review();
    await st().runConvert();
    expect(st().sheet).toMatchObject({ error: expect.stringMatching(/certified/) });
    expect(prepressApi.marks).not.toHaveBeenCalled();
    expect(prepressApi.boxes).not.toHaveBeenCalled();
    expect(prepressApi.convert).not.toHaveBeenCalled();
  });

  it("lets a signed document through: the engine keeps its save incremental", async () => {
    vi.mocked(prepressApi.protection).mockResolvedValue({ certified: false, signed: true });
    st().openMarks();
    await flush();
    await st().applyMarks();
    expect(prepressApi.marks).toHaveBeenCalledTimes(1);
    expect(st().get(1).protection).toMatchObject({ certified: false, signed: true });
  });

  it("reads the protection again when the document moves on, so a later signature shows", async () => {
    st().openMarks();
    await flush();
    expect(st().get(1).protection).toMatchObject({ certified: false, rev: 3 });
    vi.mocked(prepressApi.protection).mockResolvedValue({ certified: true, signed: true });
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 4 } as never } });
    await flush();
    expect(prepressApi.protection).toHaveBeenCalledTimes(2);
    expect(st().get(1).protection).toMatchObject({ certified: true, rev: 4 });
    // the same generation is not asked twice
    st().openMarks();
    await flush();
    expect(prepressApi.protection).toHaveBeenCalledTimes(2);
  });
});

describe("a sheet while the document changes under it", () => {
  const docWith = (p: object) => useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], ...p } as never } });

  it("is closed when an undo takes its page away", async () => {
    st().openBoxes();
    await flush();
    docWith({ pageCount: 2, rev: 4 });
    expect(st().sheet).toBeNull();
  });

  it("reads the page again, and the margins from what the page has, when the generation moves", async () => {
    st().openBoxes();
    await flush();
    st().setMargin("left", 50);
    vi.mocked(prepressApi.pageBoxes).mockResolvedValue({ ...boxes, trim: [30, 30, 582, 762], bleed: [21, 21, 591, 771] });
    docWith({ rev: 4 });
    await flush();
    expect(prepressApi.pageBoxes).toHaveBeenCalledTimes(2);
    expect(st().sheet).toMatchObject({ boxes: { trim: [30, 30, 582, 762] }, margins: { left: 9, bottom: 9, right: 9, top: 9 } });
  });

  it("draws the marks of the new generation", async () => {
    st().openMarks();
    await flush();
    expect(prepressApi.marksPreview).toHaveBeenCalledTimes(1);
    docWith({ rev: 4 });
    await flush();
    expect(prepressApi.marksPreview).toHaveBeenCalledTimes(2);
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
