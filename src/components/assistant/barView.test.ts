import { describe, expect, it } from "vitest";
import { type BarEvent, type BarView, barReduce, surfaceLeft, surfaceMaxH, surfaceWidth } from "./barView";

const run = (start: BarView, ...events: BarEvent[]) => events.reduce(barReduce, start);

describe("barReduce", () => {
  it("a send or a waiting approval opens the chat from any stage", () => {
    for (const v of ["open", "header", "bar"] as const) {
      expect(barReduce(v, "send")).toBe("open");
      expect(barReduce(v, "attention")).toBe("open");
    }
  });

  it("Esc and the minimize button step down: open, header, bar, then stay", () => {
    expect(run("open", "esc")).toBe("header");
    expect(run("open", "esc", "esc")).toBe("bar");
    expect(run("open", "esc", "esc", "esc")).toBe("bar");
    expect(run("open", "minimize", "minimize")).toBe("bar");
  });

  it("the header toggles between its two stages and a press on the page only closes the chat", () => {
    expect(run("header", "headerClick")).toBe("open");
    expect(run("open", "headerClick")).toBe("header");
    expect(run("header", "outside")).toBe("header");
    expect(run("bar", "outside")).toBe("bar");
    expect(run("open", "outside")).toBe("header");
  });

  it("entering the composer brings the header back, and nothing else", () => {
    expect(run("bar", "focus")).toBe("header");
    expect(run("header", "focus")).toBe("header");
    expect(run("open", "focus")).toBe("open");
  });

  it("a new send after collapsing reopens the card", () => {
    expect(run("open", "esc", "esc", "send")).toBe("open");
  });
});

describe("surface geometry", () => {
  it("is as wide as the bar, never wider than 640", () => {
    expect(surfaceWidth(700)).toBe(434);
    expect(surfaceWidth(1300)).toBe(640);
    expect(surfaceWidth(380)).toBe(332);
  });

  it("limits the open surface to 70 % of the area, with a floor, inside the area", () => {
    expect(surfaceMaxH(1000)).toBe(700);
    expect(surfaceMaxH(600)).toBe(420);
    expect(surfaceMaxH(300)).toBe(240); // the floor
    expect(surfaceMaxH(200)).toBe(152); // never taller than the area less its margins
    expect(surfaceMaxH(100)).toBe(120);
  });

  it("anchors left, centre and right inside the area", () => {
    expect(surfaceLeft("center", 200, 1200, 640)).toBe(380);
    expect(surfaceLeft("left", 200, 1200, 640)).toBe(224);
    expect(surfaceLeft("right", 200, 1200, 640)).toBe(536);
  });
});
