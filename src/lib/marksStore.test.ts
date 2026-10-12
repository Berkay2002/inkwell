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
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve()), Channel: class {} }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./edit", () => ({ setLeaveGuard: vi.fn(), save: vi.fn(), undo: vi.fn(), applyState: vi.fn(), queued: vi.fn((_id: number, f: () => Promise<unknown>) => f()) }));
vi.mock("./api", () => ({
  marksInfo: vi.fn(),
  marksAdd: vi.fn(),
  marksRemove: vi.fn(),
  marksBatesFiles: vi.fn(),
  pickOpen: vi.fn(),
  pickSource: vi.fn(),
  pickFolder: vi.fn(),
  knownFile: vi.fn(),
  editState: vi.fn(() => Promise.resolve({})),
  securityInfo: vi.fn(() => Promise.resolve({})),
  closeDocument: vi.fn(() => Promise.resolve()),
  cancelSearch: vi.fn(() => Promise.resolve()),
}));

import { useStore } from "../store";
import * as api from "./api";
import type { MarkKindInfo } from "./api";
import { useA11y } from "./a11y";
import { usePreflight } from "./preflight";
import { useConvert } from "./convertStore";
import * as edits from "./edit";
import { useForms } from "./forms";
import {
  batesText,
  expand,
  footerLine,
  fromHex,
  fromMark,
  type Measure,
  markOf,
  type MarksSheet,
  marksError,
  outcome,
  pagesOf,
  previewShapes,
  readiness,
  textShapes,
  toHex,
  useMarks,
} from "./marksStore";
import { useProtect } from "./protectStore";
import { useSearch } from "./searchStore";
import { useTextEdit } from "./textEdit";
import { useTools } from "./toolsStore";

const DATE = { year: 2026, month: 10, day: 11 };
const NO_MORE = { foreign: [], annotated: [], signed: false, certified: false };
const flush = () => new Promise((r) => setTimeout(r, 0));
const state = { rev: 2, pageCount: 12, sizes: [], path: "C:\\d\\Lease agreement.pdf", dirty: true, undo: "x", redo: null, redacted: false };

/** Every character is 5 pt at size 10, so layout numbers are easy to check by hand. */
const measure: Measure = (_f, size, text) => text.length * size * 0.5;

function open(kind: MarksSheet["kind"] = "headerFooter", p: Partial<MarksSheet> = {}): MarksSheet {
  useMarks.getState().open(kind, 1);
  useMarks.getState().set(p);
  return useMarks.getState().sheet!;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.marksInfo).mockResolvedValue({ ...NO_MORE, kinds: [] });
  useMarks.setState({ sheet: null, info: {}, more: {} });
  useProtect.setState({ sheet: null, security: {}, dismissed: {} });
  useConvert.setState({ sheet: null });
  useTools.setState({ sheet: null });
  useStore.setState({
    docs: { 1: { id: 1, path: "C:\\d\\Lease agreement.pdf", name: "Lease agreement.pdf", dirty: false, currentPage: 0, pageCount: 12, organize: false } as never },
    order: [1],
    active: 1,
  });
});

describe("expand", () => {
  it("writes the same words as the engine for the page tokens", () => {
    expect(expand("<<1>>", 3, 12, DATE)).toBe("3");
    expect(expand("Page <<1>> of <<n>>", 3, 12, DATE)).toBe("Page 3 of 12");
    expect(expand("<<Page 1 of n>> · <<1/n>>", 3, 12, DATE)).toBe("Page 3 of 12 · 3/12");
    expect(expand("<<1 of n>>", 3, 12, DATE)).toBe("3 of 12");
  });
  it("writes the dates", () => {
    expect(expand("<<mm/dd/yyyy>>", 1, 1, DATE)).toBe("10/11/2026");
    expect(expand("<<d/m/yy>>", 1, 1, DATE)).toBe("11/10/26");
    expect(expand("<<yyyy-mm-dd>>", 1, 1, DATE)).toBe("2026-10-11");
    expect(expand("<<mmmm d, yyyy>>", 1, 1, DATE)).toBe("October 11, 2026");
    expect(expand("<<m/d/yyyy>>", 1, 1, { year: 2026, month: 3, day: 4 })).toBe("3/4/2026");
  });
  it("leaves what it does not know as typed", () => {
    expect(expand("<<what>> <<1", 1, 1, DATE)).toBe("<<what>> <<1");
  });
});

describe("batesText and pagesOf", () => {
  it("pads to the digits", () => {
    expect(batesText("ACME-", 6, 41, "-X")).toBe("ACME-000041-X");
    expect(batesText("", 3, 1234, "")).toBe("1234");
  });
  it("takes page 1 as odd, and ranges as typed (1-based)", () => {
    expect(pagesOf("all", "", 4)).toEqual([0, 1, 2, 3]);
    expect(pagesOf("odd", "", 5)).toEqual([0, 2, 4]);
    expect(pagesOf("even", "", 5)).toEqual([1, 3]);
    expect(pagesOf("range", "2-3, 5", 5)).toEqual([1, 2, 4]);
    expect(pagesOf("range", "9", 5)).toBeNull();
    expect(pagesOf("range", "", 5)).toBeNull();
  });
});

describe("colours", () => {
  it("round-trips hex", () => {
    expect(toHex([1, 1, 0.85])).toBe("#ffffd9");
    expect(fromHex("#ff0000")).toEqual([1, 0, 0]);
    expect(fromHex("nonsense")).toEqual([0, 0, 0]);
  });
});

describe("footerLine", () => {
  it("says what is missing, once, instead of an outcome sentence about a mark that cannot be added", () => {
    const empty = open("headerFooter");
    const why = readiness(empty, 12);
    expect(why).toMatch(/Type the text/);
    expect(footerLine(why, outcome(empty, 12, false, DATE))).toBe(why);
    const w = open("watermark", { wmText: "DRAFT" });
    expect(footerLine(readiness(w, 12), outcome(w, 12, false, DATE))).toBe("Adds 'DRAFT' in front of the text on 12 pages.");
  });
});

describe("readiness", () => {
  it("asks for the text of a header and footer", () => {
    const s = open("headerFooter");
    expect(readiness(s, 12)).toMatch(/Type the text/);
    expect(readiness({ ...s, text: ["", "", "", "", "", "Page <<1>>"] }, 12)).toBeNull();
  });
  it("asks for pages that exist", () => {
    const s = open("headerFooter", { text: ["", "", "", "", "", "x"], pagesMode: "range", range: "20" });
    expect(readiness(s, 12)).toMatch(/1 to 12/);
  });
  it("needs a watermark's text or picture and somewhere to show", () => {
    const s = open("watermark");
    expect(readiness(s, 12)).toMatch(/watermark text/);
    expect(readiness({ ...s, wmText: "DRAFT" }, 12)).toBeNull();
    expect(readiness({ ...s, wmText: "DRAFT", wmScreen: false, wmPrint: false }, 12)).toMatch(/screen/);
    expect(readiness({ ...s, wmSource: "picture" }, 12)).toMatch(/picture/);
  });
  it("tells a Bates number that runs out of digits", () => {
    const s = open("bates", { btDigits: 3, btStart: 995 });
    expect(readiness(s, 12)).toMatch(/12 pages from 995 need more than 3 digits/);
    expect(readiness({ ...s, btStart: 1 }, 12)).toBeNull();
  });
});

describe("outcome", () => {
  it("names what is added and where", () => {
    const s = open("headerFooter", { text: ["", "", "Lease agreement", "", "", "Page <<1>> of <<n>>"] });
    expect(outcome(s, 12, false, DATE)).toBe("Adds 'Lease agreement' to the top right and 'Page 1 of 12' to the bottom right of 12 pages.");
    expect(outcome({ ...s, text: ["", "", "", "", "", "Page <<1>> of <<n>>"] }, 12, true, DATE)).toBe("Updates 'Page 1 of 12' to the bottom right of 12 pages.");
  });
  it("counts a selection of pages and uses the start number", () => {
    const s = open("headerFooter", { text: ["", "", "", "", "<<1>>", ""], pagesMode: "odd", startNumber: 5 });
    expect(outcome(s, 12, false, DATE)).toBe("Adds '5' to the bottom centre of 6 pages (1, 3, 5, 7, 9, 11).");
  });
  it("describes a watermark, a background and a Bates number", () => {
    const w = open("watermark", { wmText: "DRAFT" });
    expect(outcome(w, 12, false, DATE)).toBe("Adds 'DRAFT' in front of the text on 12 pages.");
    expect(outcome({ ...w, wmBehind: true, wmScreen: false }, 12, false, DATE)).toBe("Adds 'DRAFT' behind the text on 12 pages, for printing only.");
    const b = open("background");
    expect(outcome(b, 12, false, DATE)).toBe("Adds a background colour behind 12 pages.");
    const t = open("bates", { btPrefix: "ACME-", btStart: 41 });
    expect(outcome(t, 12, false, DATE)).toBe("Adds ACME-000041 to ACME-000052 at the bottom right of 12 pages.");
  });
});

describe("markOf", () => {
  it("sends percentages as fractions and a fitted watermark as size 0", () => {
    const s = open("watermark", { wmText: "DRAFT", wmOpacity: 30, wmScale: 80, wmRotation: -45, wmBehind: true });
    const m = markOf(s);
    expect(m).toMatchObject({ kind: "watermark", text: "DRAFT", scale: 0.8, opacity: 0.3, rotation: -45, behind: true, showOnScreen: true, showInPrint: true });
    expect(m.kind === "watermark" && m.style.size).toBe(0);
  });
  it("round-trips through fromMark", () => {
    const s = open("headerFooter", { text: ["a", "", "", "", "", "b"], startNumber: 7, margins: [10, 20, 30, 40] });
    const back = fromMark(open("headerFooter"), markOf(s));
    expect(back.text).toEqual(s.text);
    expect(back.startNumber).toBe(7);
    expect(back.margins).toEqual([10, 20, 30, 40]);
    const w = open("watermark", { wmText: "X", wmOpacity: 35, wmScale: 60, wmRotation: 30 });
    const wb = fromMark(open("watermark"), markOf(w));
    expect([wb.wmText, wb.wmOpacity, wb.wmScale, wb.wmRotation]).toEqual(["X", 35, 60, 30]);
  });
});

describe("shrink", () => {
  it("is off until it is ticked and goes out with header/footer and Bates", () => {
    expect(open("headerFooter").shrink).toBe(false);
    const m = markOf(open("headerFooter", { shrink: true }));
    expect(m.kind === "headerFooter" && m.shrink).toBe(true);
    const b = markOf(open("bates", { shrink: true }));
    expect(b.kind === "bates" && b.shrink).toBe(true);
  });
  it("comes back from the file, and a file without it reads as off", () => {
    const on = fromMark(open("headerFooter"), markOf(open("headerFooter", { shrink: true })));
    expect(on.shrink).toBe(true);
    const old = markOf(open("headerFooter"));
    if (old.kind === "headerFooter") delete old.shrink;
    expect(fromMark(open("headerFooter", { shrink: true }), old).shrink).toBe(false);
  });
  it("is said in the outcome", () => {
    const s = open("headerFooter", { text: ["", "", "", "", "", "x"], shrink: true });
    expect(outcome(s, 12, false, DATE)).toMatch(/The page content is scaled down to fit between the margins\.$/);
    expect(outcome({ ...s, shrink: false }, 12, false, DATE)).not.toMatch(/scaled down/);
  });
});

describe("preview layout", () => {
  const size: [number, number] = [612, 792];
  const style = { family: "helvetica" as const, bold: false, italic: false, size: 10, color: [0, 0, 0] as [number, number, number], underline: false };

  it("puts a header under the top margin and a footer above the bottom one, like the engine", () => {
    const shapes = textShapes(["", "", "Head", "Foot", "", ""], style, [36, 36, 72, 72], 612, 792, measure);
    const head = shapes[0];
    const foot = shapes[1];
    if (head.kind !== "text" || foot.kind !== "text") throw new Error("text");
    // right aligned: 612 - 72 - 4 chars x 5 pt
    expect(head.x).toBe(612 - 72 - 20);
    // baseline from the bottom: 792 - 36 - 8 = 748, so 44 from the top
    expect(head.y).toBeCloseTo(44);
    expect(foot.x).toBe(72);
    // baseline 36 + 2 = 38 from the bottom
    expect(foot.y).toBeCloseTo(792 - 38);
  });

  it("stacks lines of a multi-line footer upwards from the bottom margin", () => {
    const shapes = textShapes(["", "", "", "one\ntwo", "", ""], style, [36, 36, 72, 72], 612, 792, measure);
    const ys = shapes.map((s) => (s.kind === "text" ? s.y : 0));
    expect(ys[1]).toBeCloseTo(792 - 38);
    expect(ys[0]).toBeCloseTo(792 - 38 - 12);
  });

  it("counts page numbers over the selected pages only", () => {
    const s = open("headerFooter", { text: ["", "", "", "", "", "<<1>>"], pagesMode: "even" });
    const on = previewShapes(s, 3, 12, size, measure, DATE);
    const off = previewShapes(s, 2, 12, size, measure, DATE);
    expect(off).toEqual([]);
    // pages 2, 4, 6 ...: page 4 is the second of them
    expect(on[0]).toMatchObject({ kind: "text", text: "2" });
  });

  it("fits a watermark to the page diagonal and rotates it about the centre", () => {
    const s = open("watermark", { wmText: "DRAFT", wmScale: 50, wmRotation: 45 });
    const [g] = previewShapes(s, 0, 12, size, measure, DATE);
    if (g.kind !== "group") throw new Error("group");
    expect(g).toMatchObject({ cx: 306, cy: 396, rotation: 45, opacity: 0.5 });
    const t = g.shapes[0];
    if (t.kind !== "text") throw new Error("text");
    const diag = Math.hypot(612, 792);
    // 5 chars at 0.5 em: widest = 2.5 per point of size
    expect(t.size).toBeCloseTo((diag * 0.5) / 2.5, 5);
  });

  it("draws a background as a full-page rectangle", () => {
    const s = open("background", { bgOpacity: 80 });
    expect(previewShapes(s, 0, 12, size, measure, DATE)[0]).toMatchObject({ kind: "rect", w: 612, h: 792, opacity: 0.8 });
  });
});

describe("the sheet", () => {
  it("opens one rail at a time and closes Protect and Compress", () => {
    useProtect.setState({ sheet: { doc: 1 } as never });
    useMarks.getState().open("watermark", 1);
    expect(useMarks.getState().sheet?.kind).toBe("watermark");
    expect(useProtect.getState().sheet).toBeNull();
  });

  it("takes the side from Forms, Edit, Accessibility, Preflight and Search, and gives it up to them", () => {
    const rails = [
      { set: (o: boolean) => useForms.setState({ open: o }), open: () => useForms.getState().open, toggle: () => useForms.getState().setOpen(true) },
      { set: (o: boolean) => useTextEdit.setState({ open: o }), open: () => useTextEdit.getState().open, toggle: () => useTextEdit.getState().setOpen(true) },
      { set: (o: boolean) => useA11y.setState({ open: o }), open: () => useA11y.getState().open, toggle: () => useA11y.getState().setOpen(true) },
      { set: (o: boolean) => usePreflight.setState({ open: o }), open: () => usePreflight.getState().open, toggle: () => usePreflight.getState().setOpen(true) },
      { set: (o: boolean) => useSearch.setState({ open: o }), open: () => useSearch.getState().open, toggle: () => useSearch.getState().setOpen(true) },
    ];
    for (const rail of rails) {
      rail.set(true);
      useMarks.getState().open("watermark", 1);
      expect(rail.open()).toBe(false);
      expect(useMarks.getState().sheet?.kind).toBe("watermark");
      rail.toggle();
      expect(useMarks.getState().sheet).toBeNull();
      rail.set(false);
    }
  });

  it("starts an Update from the settings the file was made with", async () => {
    const mark = markOf(open("watermark", { wmText: "SECRET", wmOpacity: 25 }));
    const kinds: MarkKindInfo[] = [{ kind: "watermark", pages: [0, 1, 2], mark }];
    vi.mocked(api.marksInfo).mockResolvedValue({ ...NO_MORE, kinds });
    useMarks.setState({ sheet: null });
    useMarks.getState().open("watermark", 1);
    await flush();
    const s = useMarks.getState().sheet!;
    expect([s.wmText, s.wmOpacity, s.pagesMode, s.range]).toEqual(["SECRET", 25, "range", "1-3"]);
  });

  it("adds, and says so", async () => {
    vi.mocked(api.marksAdd).mockResolvedValue(state as never);
    open("headerFooter", { text: ["", "", "", "", "", "Page <<1>> of <<n>>"] });
    await useMarks.getState().apply();
    const [doc, req] = vi.mocked(api.marksAdd).mock.calls[0];
    expect(doc).toBe(1);
    expect(req.pages).toHaveLength(12);
    expect(req.replace).toBe(false);
    expect(req.mark.kind).toBe("headerFooter");
    expect(edits.applyState).toHaveBeenCalledWith(1, state);
    expect(useMarks.getState().sheet?.done).toMatch(/^Added 'Page 1 of 12' to the bottom right of 12 pages\. You can undo it\.$/);
    expect(useMarks.getState().sheet?.busy).toBe(false);
  });

  it("updates where the document already has the kind on those pages", async () => {
    vi.mocked(api.marksAdd).mockResolvedValue(state as never);
    const s = open("headerFooter", { text: ["", "", "", "", "", "x"] });
    useMarks.setState({ info: { 1: [{ kind: "headerFooter", pages: [0, 1], mark: markOf(s) }] } });
    await useMarks.getState().apply();
    expect(vi.mocked(api.marksAdd).mock.calls[0][1].replace).toBe(true);
  });

  it("does not call the engine for a sheet that is not ready", async () => {
    open("headerFooter");
    await useMarks.getState().apply();
    expect(api.marksAdd).not.toHaveBeenCalled();
    expect(useMarks.getState().sheet?.error).toMatch(/Type the text/);
  });

  it("keeps the sheet and shows the engine's sentence when the step fails", async () => {
    vi.mocked(api.marksAdd).mockRejectedValue("The text has a character the standard fonts can't draw.");
    open("headerFooter", { text: ["", "", "", "", "", "x"] });
    await useMarks.getState().apply();
    const s = useMarks.getState().sheet!;
    expect(s.error).toMatch(/can't draw/);
    expect(s.busy).toBe(false);
    expect(api.editState).toHaveBeenCalled();
  });

  it("removes a kind from all pages", async () => {
    vi.mocked(api.marksRemove).mockResolvedValue(state as never);
    open("watermark");
    await useMarks.getState().remove("watermark");
    expect(api.marksRemove).toHaveBeenCalledWith(1, "watermark");
    expect(useMarks.getState().sheet?.done).toBe("Removed the watermark. You can undo it.");
  });

  it("removes another app's marks with the foreign flag, and keeps what the engine said", async () => {
    vi.mocked(api.marksRemove).mockResolvedValue(state as never);
    const foreign = [{ kind: "headerFooter" as const, pages: [0, 1], stuck: [1] }];
    vi.mocked(api.marksInfo).mockResolvedValue({ ...NO_MORE, kinds: [], foreign, annotated: [2], signed: true });
    open("headerFooter");
    await useMarks.getState().refresh(1);
    expect(useMarks.getState().more[1]).toEqual({ foreign, annotated: [2], signed: true, certified: false });
    await useMarks.getState().remove("headerFooter", true);
    expect(api.marksRemove).toHaveBeenCalledWith(1, "headerFooter", undefined, true);
    expect(useMarks.getState().sheet?.done).toBe("Removed the header and footer from another app. On 1 page (2) it can't be taken out without changing the rest of the page, so it stays. You can undo it.");
  });

  it("numbers files into a picked folder and leaves this document alone", async () => {
    vi.mocked(api.pickFolder).mockResolvedValue({ token: "f1", path: "C:\\out" });
    vi.mocked(api.marksBatesFiles).mockResolvedValue([
      { input: "C:\\a.pdf", output: "C:\\out\\a - Bates.pdf", pages: 3, first: 1, last: 3 },
      { input: "C:\\b.pdf", output: "C:\\out\\b - Bates.pdf", pages: 2, first: 4, last: 5 },
    ]);
    open("bates", { btFiles: [{ token: "t1", path: "C:\\a.pdf", name: "a.pdf" }, { token: "t2", path: "C:\\b.pdf", name: "b.pdf" }] });
    await useMarks.getState().apply();
    const req = vi.mocked(api.marksBatesFiles).mock.calls[0][0];
    expect(req.files).toEqual(["t1", "t2"]);
    expect(req.folder).toBe("f1");
    expect(req.bates).toMatchObject({ digits: 6, start: 1, slot: 5 });
    expect(api.marksAdd).not.toHaveBeenCalled();
    expect(useMarks.getState().sheet?.btResults).toHaveLength(2);
    expect(useMarks.getState().sheet?.done).toBe("Wrote 2 numbered copies, 5 pages.");
  });

  it("does nothing when the folder dialog is dismissed", async () => {
    vi.mocked(api.pickFolder).mockResolvedValue(null);
    open("bates", { btFiles: [{ token: "t1", path: "C:\\a.pdf", name: "a.pdf" }] });
    await useMarks.getState().apply();
    expect(api.marksBatesFiles).not.toHaveBeenCalled();
    expect(useMarks.getState().sheet?.busy).toBe(false);
  });
});

describe("marksError", () => {
  it("passes the engine's sentences through and words the codes", () => {
    expect(marksError("Type the header or footer text first.")).toBe("Type the header or footer text first.");
    expect(marksError("stale")).toMatch(/Try again/);
  });
});
