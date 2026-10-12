import { describe, expect, it } from "vitest";
import type { PageBoxes, PageSeps } from "./prepressApi";
import {
  DEFAULT_CONVERT,
  DEFAULT_MARKS,
  DEFAULT_OUTPUT,
  allPlates,
  applyMargins,
  boxProblem,
  clickSolo,
  convertProblem,
  convertSpec,
  convertSummary,
  convertedFileName,
  coverageText,
  isSolo,
  marginsBetween,
  marksKey,
  onlyPlate,
  parseLength,
  parseLimit,
  parseMarks,
  plateList,
  rectShown,
  holdsSave,
  scopePages,
  tabTarget,
  showInk,
  showLength,
  toPt,
  togglePlate,
  shownMargins,
} from "./prepressLogic";

const seps: PageSeps = {
  process: ["Cyan", "Magenta", "Yellow", "Black"],
  spots: [
    { name: "PANTONE 123", rgb: [255, 200, 0], cmyk: [0, 20, 100, 0] },
    { name: "Varnish", rgb: [200, 200, 200], cmyk: [0, 0, 0, 10] },
  ],
  overprint: true,
  folded: 0,
};

const on = (o: Parameters<typeof plateList>[1], s: PageSeps | null = seps) => plateList(s, o).filter((p) => p.on).map((p) => p.name);

describe("plates", () => {
  it("lists the four inks then the page's spots", () => {
    expect(plateList(seps, DEFAULT_OUTPUT).map((p) => p.key)).toEqual(["p0", "p1", "p2", "p3", "s:PANTONE 123", "s:Varnish"]);
    expect(plateList(null, DEFAULT_OUTPUT)).toHaveLength(4);
  });

  it("toggles one plate and leaves a spot alone while overprint is off", () => {
    const o = togglePlate(DEFAULT_OUTPUT, "p3");
    expect(on(o)).toEqual(["Cyan", "Magenta", "Yellow", "PANTONE 123", "Varnish"]);
    expect(on(togglePlate(o, "s:Varnish"))).toEqual(["Cyan", "Magenta", "Yellow", "PANTONE 123"]);
    const flat = { ...DEFAULT_OUTPUT, overprint: false };
    expect(togglePlate(flat, "s:Varnish")).toBe(flat);
    expect(plateList(seps, flat).every((p) => p.process || p.locked)).toBe(true);
  });

  it("shows one plate alone, and everything again on a second click", () => {
    const solo = clickSolo(DEFAULT_OUTPUT, "s:PANTONE 123", seps);
    expect(on(solo)).toEqual(["PANTONE 123"]);
    expect(isSolo(solo, "s:PANTONE 123", seps)).toBe(true);
    expect(isSolo(DEFAULT_OUTPUT, "p0", seps)).toBe(false);
    expect(on(clickSolo(solo, "s:PANTONE 123", seps))).toHaveLength(6);
    expect(on(clickSolo(solo, "p1", seps))).toEqual(["Magenta"]);
  });

  it("a solo click leaves the hidden spots of other pages as they were", () => {
    const o = { ...DEFAULT_OUTPUT, hiddenSpots: ["Gold"] };
    expect(onlyPlate(o, "p0", seps).hiddenSpots.sort()).toEqual(["Gold", "PANTONE 123", "Varnish"]);
    expect(allPlates(onlyPlate(o, "p0", seps))).toMatchObject({ process: [true, true, true, true], hiddenSpots: [] });
  });

  it("a solo click on a plate goes to the coverage view's plates", () => {
    expect(clickSolo({ ...DEFAULT_OUTPUT, mode: "coverage" }, "p2", seps).mode).toBe("plates");
  });
});

describe("ink limit", () => {
  it("reads a limit as typed", () => {
    expect(parseLimit("300")).toBe(300);
    expect(parseLimit(" 287,5 % ")).toBe(287.5);
    expect(parseLimit("0")).toBe(0);
    expect(parseLimit("1000")).toBe(1000);
    expect(parseLimit("1001")).toBeNull();
    expect(parseLimit("-5")).toBeNull();
    expect(parseLimit("many")).toBeNull();
    expect(parseLimit("")).toBeNull();
  });

  it("says in one sentence whether the page is within the limit", () => {
    expect(coverageText(280, 300, 0, false)).toBe("Highest ink on this page is 280%, within 300%.");
    expect(coverageText(340, 300, 1, false)).toBe("Ink goes up to 340% in 1 area, above 300%.");
    expect(coverageText(400, 300, 64, true)).toBe("Ink goes up to 400% in more than 64 areas, above 300%.");
  });

  it("shows small ink values without false zeros", () => {
    expect([0, 0.4, 1, 37.6, 400].map(showInk)).toEqual(["0%", "<1%", "1%", "38%", "400%"]);
  });
});

describe("pages and units", () => {
  it("turns a scope into 0-based pages", () => {
    expect(scopePages("all", "", 10, 4)).toBeUndefined();
    expect(scopePages("page", "", 10, 4)).toEqual([4]);
    expect(scopePages("range", "2-3, 5", 10, 4)).toEqual([1, 2, 4]);
    expect(scopePages("range", "nope", 10, 4)).toBeNull();
    expect(scopePages("range", "", 10, 4)).toBeNull();
  });

  it("reads lengths in the chosen unit and shows them without noise", () => {
    expect(parseLength("3", "mm")).toBeCloseTo(8.5039, 3);
    expect(parseLength("0,125", "in")).toBeCloseTo(9, 6);
    expect(parseLength("-2", "pt")).toBe(-2);
    expect(parseLength("1/8", "in")).toBeNull();
    expect(showLength(toPt(3, "mm"), "mm")).toBe("3");
    expect(showLength(9, "in")).toBe("0.125");
  });
});

describe("margins", () => {
  const ref: [number, number, number, number] = [20, 30, 592, 762];
  const m = { left: 1, bottom: 2, right: 3, top: 4 };

  it("moves each edge as the page is shown", () => {
    expect(applyMargins(ref, 0, m, true)).toEqual([19, 28, 595, 766]);
    expect(applyMargins(ref, 0, m, false)).toEqual([21, 32, 589, 758]);
    // turned 90 degrees, the left edge of the page is the bottom of the user space
    expect(applyMargins([0, 0, 100, 200], 90, { left: 10, bottom: 0, right: 0, top: 0 }, true)).toEqual([0, -10, 100, 200]);
  });

  it("goes there and back for every rotation, outward and inward", () => {
    for (const rot of [0, 90, 180, 270]) {
      for (const out of [true, false]) {
        const box = applyMargins(ref, rot, m, out);
        expect(marginsBetween(ref, box, rot, out)).toEqual(m);
      }
    }
  });

  it("sends the engine the margins as shown, outward positive; each page maps them by its own turn", () => {
    expect(shownMargins(m, true)).toEqual([1, 2, 3, 4]);
    expect(shownMargins(m, false)).toEqual([-1, -2, -3, -4]);
  });

  it("puts a user-space box on the page as shown", () => {
    expect(rectShown([10, 20, 110, 220], [0, 0, 612, 792], 0)).toEqual([10, 572, 110, 772]);
    expect(rectShown([10, 20, 110, 220], [0, 0, 612, 792], 90)).toEqual([20, 10, 220, 110]);
  });
});

describe("page box rules", () => {
  const page: PageBoxes = {
    rotate: 0,
    media: [0, 0, 612, 792],
    crop: [0, 0, 612, 792],
    trim: [20, 20, 592, 772],
    bleed: [11, 11, 601, 781],
    art: [0, 0, 612, 792],
    stated: { trim: true, bleed: true, art: false, crop: false },
  };

  it("refuses what the engine would refuse", () => {
    expect(boxProblem("bleed", [11, 11, 601, 781], page)).toBeNull();
    expect(boxProblem("bleed", [-5, 0, 612, 792], page)).toMatch(/beyond the media box/);
    expect(boxProblem("bleed", [30, 30, 580, 760], page)).toMatch(/trim box has to stay inside/);
    expect(boxProblem("trim", [5, 5, 600, 780], page)).toMatch(/inside the bleed box/);
    expect(boxProblem("trim", [100, 100, 100, 700], page)).toMatch(/empty/);
    expect(boxProblem("art", [0, 0, 700, 792], page)).toMatch(/beyond the media box/);
  });
});

describe("printer's marks", () => {
  it("reads the engine's strokes and fills", () => {
    const paths = parseMarks("q 0.25 w 10 10 m 20 10 l S Q 0 0 0 1 k 5 5 10 10 re f");
    expect(paths).toHaveLength(2);
    expect(paths[0]).toMatchObject({ kind: "stroke", width: 0.25, points: [[[10, 10], [20, 10]]] });
    expect(paths[1]).toMatchObject({ kind: "fill", cmyk: [0, 0, 0, 1] });
    expect(paths[1].points[0]).toHaveLength(4);
  });

  it("asks the engine again only when the marks change", () => {
    const f = DEFAULT_MARKS;
    expect(marksKey(1, 5, f)).toBe(marksKey(1, 5, { ...f, units: "mm", scope: "page", range: "1" }));
    expect(marksKey(1, 5, f)).not.toBe(marksKey(1, 5, { ...f, weight: 0.5 }));
    expect(marksKey(1, 5, f)).not.toBe(marksKey(1, 6, f));
    expect(marksKey(1, 5, f)).not.toBe(marksKey(2, 5, f));
  });
});

describe("convert colours", () => {
  it("needs a colour kind, a profile for a profile and a valid range", () => {
    expect(convertProblem(DEFAULT_CONVERT, 10, 0)).toBeNull();
    expect(convertProblem({ ...DEFAULT_CONVERT, rgb: false, gray: false }, 10, 0)).toMatch(/Choose which colours/);
    expect(convertProblem({ ...DEFAULT_CONVERT, intent: "profile" }, 10, 0)).toMatch(/profile/);
    expect(convertProblem({ ...DEFAULT_CONVERT, scope: "range", range: "x" }, 10, 0)).toMatch(/range/);
  });

  it("makes the engine's request, naming the condition after the profile", () => {
    expect(convertSpec(DEFAULT_CONVERT, 10, 3)).toEqual({ gray: true, rgb: true, spot: false, grayToK: true, preserveBlack: true, intent: "device" });
    const o = { ...DEFAULT_CONVERT, scope: "page" as const, intent: "profile" as const, profile: { token: "t", name: "Coated FOGRA39.icc" } };
    expect(convertSpec(o, 10, 3)).toMatchObject({ pages: [3], intent: "profile", condition: "Coated FOGRA39" });
    expect(convertSpec({ ...o, condition: " My press " }, 10, 3)).toMatchObject({ condition: "My press" });
  });

  it("summarises what will change before it does", () => {
    const text = convertSummary(DEFAULT_CONVERT, 10, 0, false).join("\n");
    expect(text).toMatch(/RGB and gray become CMYK on all 10 pages/);
    expect(text).toMatch(/new file/);
    expect(text).not.toMatch(/unsaved/);
    expect(convertSummary(DEFAULT_CONVERT, 10, 0, true).join("\n")).toMatch(/unsaved changes/);
    const inPlace = convertSummary({ ...DEFAULT_CONVERT, output: "inPlace", scope: "page" }, 10, 4, true).join("\n");
    expect(inPlace).toMatch(/on 1 page/);
    expect(inPlace).toMatch(/undo/);
    expect(inPlace).not.toMatch(/unsaved/);
  });

  it("names the new file after the document and the profile", () => {
    expect(convertedFileName("Brochure", DEFAULT_CONVERT)).toBe("Brochure (CMYK).pdf");
    expect(convertedFileName("Brochure", { ...DEFAULT_CONVERT, intent: "profile", profile: { token: "t", name: "ISOcoated.icc" } })).toBe("Brochure (CMYK ISOcoated).pdf");
  });
});

describe("tab order inside a sheet", () => {
  it("wraps at both ends and starts at an end when focus is on the sheet", () => {
    expect(tabTarget(4, 3, false)).toBe(0);
    expect(tabTarget(4, 0, true)).toBe(3);
    expect(tabTarget(4, 1, false)).toBe(2);
    expect(tabTarget(4, 1, true)).toBe(0);
    expect(tabTarget(4, -1, false)).toBe(0);
    expect(tabTarget(4, -1, true)).toBe(3);
    expect(tabTarget(1, 0, false)).toBe(0);
    expect(tabTarget(0, -1, false)).toBeNull();
  });
});

describe("Save while a conversion is not saved", () => {
  const key = (k: string, m: Partial<{ ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean }> = {}) => ({ key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...m });
  it("holds back Ctrl+S and Cmd+S only in that phase", () => {
    expect(holdsSave(key("s", { ctrlKey: true }), true)).toBe(true);
    expect(holdsSave(key("S", { metaKey: true }), true)).toBe(true);
    expect(holdsSave(key("s", { ctrlKey: true }), false)).toBe(false);
    expect(holdsSave(key("s"), true)).toBe(false);
    expect(holdsSave(key("s", { ctrlKey: true, shiftKey: true }), true)).toBe(false);
    expect(holdsSave(key("z", { ctrlKey: true }), true)).toBe(false);
  });
});
