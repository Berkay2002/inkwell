import { describe, expect, it } from "vitest";
import {
  PAD,
  PAGE_GAP,
  bucketScale,
  computeLayout,
  currentPage,
  pageAt,
  pageSpaceTransform,
  previewScaleKey,
  stepZoom,
  tilesForRect,
  typicalSize,
  visibleRange,
  type Size,
} from "./layout";

const letter: Size = [612, 792];

describe("layout", () => {
  const sizes: Size[] = Array.from({ length: 100 }, () => letter);
  const l = computeLayout(sizes, 0, 1, 800);

  it("stacks pages with gaps and padding", () => {
    expect(l.tops[0]).toBe(PAD);
    expect(l.tops[1]).toBe(PAD + 792 + PAGE_GAP);
    expect(l.total).toBe(PAD + 100 * 792 + 99 * PAGE_GAP + PAD);
    expect(l.contentWidth).toBe(800);
  });

  it("swaps width and height when rotated", () => {
    const r = computeLayout([letter], 1, 2, 100);
    expect(r.widths[0]).toBe(792 * 2);
    expect(r.heights[0]).toBe(612 * 2);
  });

  it("finds pages by y with binary search", () => {
    expect(pageAt(l.tops, 0)).toBe(0);
    expect(pageAt(l.tops, l.tops[37] + 5)).toBe(37);
    expect(pageAt(l.tops, l.total + 1000)).toBe(99);
  });

  it("returns the visible range", () => {
    const [a, b] = visibleRange(l.tops, l.heights, l.tops[10] + 100, l.tops[12] + 100);
    expect([a, b]).toEqual([10, 12]);
    // A viewport that starts in the gap below page 3 starts at page 4.
    const [c] = visibleRange(l.tops, l.heights, l.tops[3] + l.heights[3] + 5, l.tops[5]);
    expect(c).toBe(4);
  });

  it("picks the page under the viewport centre as current", () => {
    expect(currentPage(l.tops, l.heights, 0, 600)).toBe(0);
    expect(currentPage(l.tops, l.heights, l.tops[7], 400)).toBe(7);
  });
});

describe("zoom and scale", () => {
  it("snaps render scale to quarter octaves", () => {
    expect(bucketScale(1, 1)).toBe(1);
    expect(bucketScale(1.1, 1)).toBeCloseTo(Math.pow(2, 0.25), 6);
    expect(bucketScale(1.5, 1.5)).toBeCloseTo(Math.pow(2, 1.25), 6);
    expect(bucketScale(100, 2)).toBe(16);
  });

  it("steps through presets in both directions", () => {
    expect(stepZoom(1, 1)).toBe(1.25);
    expect(stepZoom(1, -1)).toBe(0.75);
    expect(stepZoom(0.1, -1)).toBe(0.1);
    expect(stepZoom(16, 1)).toBe(16);
  });

  it("previews fit one tile", () => {
    const k = previewScaleKey(letter);
    expect(Math.ceil(792 * (k / 1000))).toBeLessThanOrEqual(320);
  });

  it("uses the most common size as typical", () => {
    const s: Size[] = [letter, [595, 842], [595, 842], [595, 842], letter];
    expect(typicalSize(s, 0)).toEqual([595, 842]);
    expect(typicalSize(s, 1)).toEqual([842, 595]);
  });
});

describe("tiles", () => {
  it("covers only what is visible, nearest first", () => {
    // A 612x792 page at 1 css px per pt, scale 2: 1224x1584 device px = 3x4 tiles.
    const all = tilesForRect(0, 0, letter, 1, 2, { x0: 0, y0: 0, x1: 612, y1: 792 });
    expect(all).toHaveLength(12);
    const top = tilesForRect(0, 0, letter, 1, 2, { x0: 0, y0: 0, x1: 612, y1: 100 });
    expect(top.map((t) => t.ty)).toEqual([0, 0, 0]);
    // Centre of the view is x=306 -> tile 1 comes first.
    expect(top[0].tx).toBe(1);
  });

  it("clamps to the page", () => {
    const t = tilesForRect(3, 0, letter, 1, 1, { x0: -500, y0: -500, x1: 5000, y1: 5000 });
    expect(Math.max(...t.map((x) => x.tx))).toBe(1);
    expect(Math.max(...t.map((x) => x.ty))).toBe(1);
    expect(t.every((x) => x.page === 3)).toBe(true);
  });
});

describe("page space transform", () => {
  it("maps the four rotations", () => {
    expect(pageSpaceTransform(letter, 0, 2)).toContain("rotate(0deg) scale(2)");
    expect(pageSpaceTransform(letter, 1, 1)).toContain("translate(792px,0px) rotate(90deg)");
    expect(pageSpaceTransform(letter, 2, 1)).toContain("translate(612px,792px) rotate(180deg)");
    expect(pageSpaceTransform(letter, 3, 1)).toContain("translate(0px,612px) rotate(270deg)");
  });
});
