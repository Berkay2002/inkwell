import { describe, expect, it } from "vitest";
import {
  type Measure, type Scale, UNSCALED, allowSnap, calibrate, constrain45, inUnit, numText, parseScale, parseUnit, pathLength,
  place, polygonArea, quantityOf, realPerPt, scaleLabel, totals, trimNum,
} from "./measureLogic";

const FT10: Scale = { paper: 1, paperUnit: "in", real: 10, realUnit: "ft" };
const close = (a: number, b: number) => expect(a).toBeCloseTo(b, 6);

describe("units and scales", () => {
  it("reads unit spellings like the engine", () => {
    expect(parseUnit(" Inches ")).toBe("in");
    expect(parseUnit("feet")).toBe("ft");
    expect(parseUnit("'")).toBe("ft");
    expect(parseUnit("metres")).toBe("m");
    expect(parseUnit("km")).toBeNull();
  });
  it("parses and prints a scale", () => {
    expect(parseScale("1 in = 10 ft")).toEqual(FT10);
    expect(parseScale("1in=10ft")).toEqual(FT10);
    expect(parseScale("0.25 in = 1 ft")).toEqual({ paper: 0.25, paperUnit: "in", real: 1, realUnit: "ft" });
    expect(parseScale("1:100")).toBeNull();
    expect(parseScale("0 in = 1 ft")).toBeNull();
    expect(parseScale("1 in = 10 km")).toBeNull();
    expect(parseScale("= 10 ft")).toBeNull();
    expect(scaleLabel(FT10)).toBe("1 in = 10 ft");
    expect(scaleLabel(parseScale("0.25 in = 1 ft")!)).toBe("0.25 in = 1 ft");
    expect(trimNum(2.5)).toBe("2.5");
    expect(trimNum(10)).toBe("10");
  });
  it("converts the same ratio to another unit", () => {
    close(realPerPt(FT10), 10 / 72);
    const m = inUnit(FT10, "m");
    expect(m.realUnit).toBe("m");
    close(m.real, 3.048);
    close(realPerPt(m) * 1, realPerPt(FT10) * 0.3048);
    expect(inUnit(inUnit(FT10, "mm"), "ft").real).toBeCloseTo(10, 9);
  });
});

describe("label maths", () => {
  it("measures a distance, a perimeter and an area at scale", () => {
    // 3-4-5 of 36 pt: 180 pt = 2.5 in = 25 ft
    const tri: [number, number][] = [[0, 0], [108, 0], [108, 144]];
    expect(quantityOf("distance", [[0, 0], [108, 144]], FT10).text).toBe("25.00 ft");
    expect(quantityOf("perimeter", tri, FT10).value).toBeCloseTo(35, 6); // 108 + 144 = 252 pt = 3.5 in = 35 ft
    expect(polygonArea(tri)).toBe(7776);
    // a 2 sq in rectangle at 1 in = 10 ft is 200 sq ft
    const q = quantityOf("area", [[0, 0], [144, 0], [144, 72], [0, 72]], FT10);
    expect(q.unit).toBe("sq ft");
    expect(q.text).toBe("200.00 sq ft");
  });
  it("uses the first two points of a distance, the open path of a perimeter", () => {
    const p: [number, number][] = [[0, 0], [72, 0], [72, 72]];
    expect(quantityOf("distance", p, UNSCALED).value).toBe(1);
    expect(quantityOf("perimeter", p, UNSCALED).value).toBe(2);
    expect(pathLength(p)).toBe(144);
  });
  it("groups thousands and keeps two decimals", () => {
    expect(numText(1234.5)).toBe("1,234.50");
    expect(numText(0)).toBe("0.00");
    expect(numText(1234567.891)).toBe("1,234,567.89");
    expect(numText(-12)).toBe("-12.00");
  });
});

describe("placing a point", () => {
  it("constrains to 45 degrees", () => {
    const [x, y] = constrain45([0, 0], [100, 10]);
    close(x, Math.hypot(100, 10));
    close(y, 0);
    const d = constrain45([10, 10], [50, 60]); // nearest ray is 45 degrees
    close(d[0] - 10, d[1] - 10);
    expect(constrain45([5, 5], [5, 5])).toEqual([5, 5]);
  });
  it("snaps only the kinds that are switched on", () => {
    const hit = { kind: "midpoint" as const, x: 10, y: 20, distance: 2 };
    expect(allowSnap(hit, { endpoint: true, midpoint: false, line: true })).toBeNull();
    expect(allowSnap(hit, { endpoint: false, midpoint: true, line: false })).toBe(hit);
    expect(allowSnap(null, { endpoint: true, midpoint: true, line: true })).toBeNull();
  });
  it("prefers closing a path, then the snap, and lets Shift beat both", () => {
    const hit = { kind: "endpoint" as const, x: 50, y: 50, distance: 1 };
    expect(place([51, 51], null, false, hit, null, 8)).toEqual({ at: [50, 50], snap: "endpoint" });
    expect(place([51, 51], null, false, null, null, 8)).toEqual({ at: [51, 51], snap: null });
    expect(place([1, 1], [100, 100], false, hit, [0, 0], 8)).toEqual({ at: [0, 0], snap: "close" });
    // away from the first point the snap hit is taken
    expect(place([99, 20], [0, 0], false, hit, [0, 0], 8)).toEqual({ at: [50, 50], snap: "endpoint" });
  });
  it("Shift ignores snapping and constrains from the last point", () => {
    const hit = { kind: "endpoint" as const, x: 50, y: 50, distance: 1 };
    const r = place([100, 8], [0, 0], true, hit, null, 8);
    expect(r.snap).toBeNull();
    close(r.at[1], 0);
    // with no last point there is nothing to constrain from: Shift is not special
    expect(place([100, 8], null, true, hit, null, 8).snap).toBe("endpoint");
  });
});

describe("calibrating and totals", () => {
  it("makes the scale from a printed dimension", () => {
    // 144 pt (2 in) drawn over a dimension that reads 20 ft
    const s = calibrate([0, 0], [144, 0], 20, "ft")!;
    expect(s).toEqual({ paper: 2, paperUnit: "in", real: 20, realUnit: "ft" });
    expect(quantityOf("distance", [[0, 0], [72, 0]], s).text).toBe("10.00 ft");
    expect(calibrate([5, 5], [5, 5], 20, "ft")).toBeNull();
    expect(calibrate([0, 0], [10, 0], 0, "ft")).toBeNull();
    expect(calibrate([0, 0], [10, 0], NaN, "ft")).toBeNull();
  });
  it("sums per kind and unit, never across units", () => {
    const m = (kind: Measure["kind"], value: number, unit: string): Measure => ({
      page: 0, xref: Math.random(), kind, points: [], contents: "", author: "", scale: null, quantity: { value, unit, text: "" },
    });
    const t = totals([m("area", 100, "sq ft"), m("distance", 10, "ft"), m("distance", 5, "m"), m("distance", 2.5, "ft")]);
    expect(t.map((x) => [x.kind, x.unit, x.count, x.text])).toEqual([
      ["distance", "ft", 2, "12.50 ft"],
      ["distance", "m", 1, "5.00 m"],
      ["area", "sq ft", 1, "100.00 sq ft"],
    ]);
  });
});
