import { describe, expect, it } from "vitest";
import type { PageTrans, Transition } from "./api";
import {
  alpha,
  commonTrans,
  defaultTransition,
  describeTransition,
  labelAt,
  pageOfLabel,
  roman,
  sampleLabels,
  styleInfo,
} from "./organizePlus";
import { directionVector, effectFor, transitionMs } from "./present";

describe("page label numbers", () => {
  it("writes roman numerals and falls back to digits past 3999", () => {
    expect([1, 4, 9, 14, 40, 1994].map(roman)).toEqual(["i", "iv", "ix", "xiv", "xl", "mcmxciv"]);
    expect(roman(4000)).toBe("4000");
  });

  it("repeats letters the PDF way", () => {
    expect([1, 26, 27, 52, 53].map(alpha)).toEqual(["a", "z", "aa", "zz", "aaa"]);
  });

  it("a section's labels: prefix, start and style", () => {
    expect(labelAt({ style: "lowerRoman", prefix: "", start: 1 }, 2)).toBe("iii");
    expect(labelAt({ style: "decimal", prefix: "A-", start: 7 }, 1)).toBe("A-8");
    expect(labelAt({ style: "upperAlpha", prefix: "", start: 1 }, 1)).toBe("B");
    expect(labelAt({ style: "none", prefix: "Cover", start: 1 }, 3)).toBe("Cover");
  });

  it("a sample line says how it goes on", () => {
    expect(sampleLabels({ style: "lowerRoman", prefix: "", start: 1 }, 10)).toBe("i, ii, iii, iv, …");
    expect(sampleLabels({ style: "decimal", prefix: "", start: 1 }, 2)).toBe("1, 2");
  });

  it("finds a page by its label, as typed first and then ignoring case", () => {
    const labels = ["i", "ii", "iii", "1", "2", "A-1"];
    expect(pageOfLabel(labels, "iii")).toBe(2);
    expect(pageOfLabel(labels, " a-1 ")).toBe(5);
    expect(pageOfLabel(labels, "2")).toBe(4);
    expect(pageOfLabel(labels, "9")).toBeNull();
    expect(pageOfLabel([], "1")).toBeNull();
  });
});

const wipe: Transition = { style: "wipe", duration: 2, dimension: null, motion: null, direction: 0 };

describe("page transitions", () => {
  it("a style's defaults fill only what it uses", () => {
    expect(defaultTransition("split")).toMatchObject({ dimension: "H", motion: "I", direction: null });
    expect(defaultTransition("wipe")).toMatchObject({ dimension: null, motion: null, direction: 0 });
    expect(defaultTransition("glitter").direction).toBe(315);
    expect(styleInfo("fade").direction).toBeUndefined();
  });

  it("says in words what a page does", () => {
    expect(describeTransition(null)).toBe("No transition");
    expect(describeTransition(wipe)).toBe("Wipe, left to right, 2 s");
    expect(describeTransition({ ...defaultTransition("split"), duration: 0.5 })).toBe("Split, horizontal, inward, 0.5 s");
  });

  it("starts from what every page of the range has in common", () => {
    const a: PageTrans = { transition: wipe, autoAdvance: 3 };
    const b: PageTrans = { transition: null, autoAdvance: null };
    expect(commonTrans([a, a, b], [0, 1])).toEqual(a);
    expect(commonTrans([a, a, b], [1, 2])).toBeNull();
    expect(commonTrans([a], [])).toBeNull();
  });
});

describe("full screen effects", () => {
  it("directions are screen vectors", () => {
    expect(directionVector(0)).toEqual([1, 0]);
    expect(directionVector(90)).toEqual([0, -1]);
    expect(directionVector(180)).toEqual([-1, 0]);
    expect(directionVector(270)).toEqual([0, 1]);
  });

  it("replace and no transition do nothing", () => {
    expect(effectFor(null)).toEqual({});
    expect(effectFor({ ...wipeOf("r") })).toEqual({});
  });

  it("a wipe left to right opens from the left edge", () => {
    expect(effectFor(wipeOf("wipe", 0)).incoming?.[0]).toEqual({ clipPath: "inset(0 100% 0 0)" });
    expect(effectFor(wipeOf("wipe", 90)).incoming?.[0]).toEqual({ clipPath: "inset(100% 0 0 0)" });
  });

  it("push moves both pages, uncover only the old one beneath the new", () => {
    const push = effectFor(wipeOf("push", 0));
    expect(push.incoming?.[0]).toEqual({ transform: "translate(-100%, 0%)" });
    expect(push.outgoing?.[1]).toEqual({ transform: "translate(100%, 0%)" });
    const un = effectFor(wipeOf("uncover", 180));
    expect(un.below).toBe(true);
    expect(un.incoming).toBeUndefined();
  });

  it("split goes inward from the old page, outward from the new one", () => {
    expect(effectFor({ ...defaultTransition("split"), motion: "I" }).below).toBe(true);
    const out = effectFor({ ...defaultTransition("split"), motion: "O", dimension: "V" });
    expect(out.incoming?.[0]).toEqual({ clipPath: "inset(0 50% 0 50%)" });
  });

  it("a transition takes what it says, within what a person can sit through", () => {
    expect(transitionMs({ ...wipe, duration: 2 })).toBe(2000);
    expect(transitionMs({ ...wipe, duration: 90 })).toBe(5000);
    expect(transitionMs(null)).toBe(1000);
  });
});

function wipeOf(style: Transition["style"], direction: number | null = 0): Transition {
  return { style, duration: 1, dimension: null, motion: null, direction };
}
