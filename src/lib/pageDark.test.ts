import { describe, expect, it } from "vitest";
import { appIsDark, displayRect, naturalRects, pagesAreDark, parsePageDark, togglePageDark } from "./pageDark";

describe("dark pages setting", () => {
  it("reads what was stored, anything else is follow", () => {
    expect(parsePageDark("on")).toBe("on");
    expect(parsePageDark("off")).toBe("off");
    expect(parsePageDark("follow")).toBe("follow");
    expect(parsePageDark(null)).toBe("follow");
    expect(parsePageDark("dark")).toBe("follow");
  });

  it("knows when the app is dark", () => {
    expect(appIsDark("dark", false)).toBe(true);
    expect(appIsDark("light", true)).toBe(false);
    expect(appIsDark("system", true)).toBe(true);
    expect(appIsDark("system", false)).toBe(false);
  });

  it("resolves off, on and follow", () => {
    expect(pagesAreDark("off", "dark", true)).toBe(false);
    expect(pagesAreDark("on", "light", false)).toBe(true);
    expect(pagesAreDark("follow", "dark", false)).toBe(true);
    expect(pagesAreDark("follow", "light", true)).toBe(false);
    expect(pagesAreDark("follow", "system", true)).toBe(true);
    expect(pagesAreDark("follow", "system", false)).toBe(false);
  });

  it("a toggle shows the opposite and returns to follow when that is what follow shows", () => {
    // dark app, follow: pages dark -> normal pages are an explicit choice
    expect(togglePageDark("follow", "dark", false)).toBe("off");
    // ... and toggling again is what follow shows, so back to follow
    expect(togglePageDark("off", "dark", false)).toBe("follow");
    // light app, follow: pages normal -> dark pages are an explicit choice
    expect(togglePageDark("follow", "light", false)).toBe("on");
    expect(togglePageDark("on", "light", false)).toBe("follow");
    // an explicit setting that disagrees with the app flips to the other explicit value only when needed
    expect(togglePageDark("on", "dark", false)).toBe("off");
    expect(togglePageDark("off", "light", false)).toBe("on");
    // the system's theme counts when the app follows it
    expect(togglePageDark("follow", "system", true)).toBe("off");
    expect(togglePageDark("follow", "system", false)).toBe("on");
  });
});

describe("photographs stay natural", () => {
  const page: [number, number] = [612, 792];
  const img = (rect: [number, number, number, number], colorSpace = "DeviceRGB") => ({ rect, colorSpace });

  it("keeps photos and figures", () => {
    expect(naturalRects([img([72, 100, 300, 260]), img([320, 400, 540, 600], "ICCBased")], page)).toEqual([
      [72, 100, 300, 260],
      [320, 400, 540, 600],
    ]);
  });

  it("leaves out stencil masks, tiny marks and a scan that is the whole page", () => {
    const list = [img([72, 100, 300, 260], "ImageMask"), img([10, 10, 20, 20]), img([300, 10, 500, 14]), img([0, 0, 612, 792])];
    expect(naturalRects(list, page)).toEqual([]);
  });

  it("an image just under the page share still counts as a picture", () => {
    // about 55% of the page
    expect(naturalRects([img([20, 20, 592, 400])], page)).toEqual([[20, 20, 592, 400]]);
  });

  it("leaves out masked images: their shape is not their box", () => {
    expect(naturalRects([{ ...img([72, 100, 300, 260]), masked: true }, img([320, 400, 540, 600])], page)).toEqual([[320, 400, 540, 600]]);
  });

  const area = (r: [number, number, number, number]) => (r[2] - r[0]) * (r[3] - r[1]);
  const overlap = (a: [number, number, number, number], b: [number, number, number, number]) =>
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];

  it("a box inside another is dropped", () => {
    expect(naturalRects([img([72, 100, 400, 400]), img([100, 150, 200, 250])], page)).toEqual([[72, 100, 400, 400]]);
    // whichever comes first
    expect(naturalRects([img([100, 150, 200, 250]), img([72, 100, 400, 400])], page).reduce((s, r) => s + area(r), 0)).toBe(328 * 300);
  });

  it("overlapping boxes never cover the same spot twice and still cover all of it", () => {
    const a: [number, number, number, number] = [72, 100, 300, 300];
    const b: [number, number, number, number] = [200, 200, 450, 420];
    const rects = naturalRects([img(a), img(b)], page);
    for (let i = 0; i < rects.length; i++) for (let j = i + 1; j < rects.length; j++) expect(overlap(rects[i], rects[j])).toBe(false);
    const inter = (300 - 200) * (300 - 200);
    expect(rects.reduce((s, r) => s + area(r), 0)).toBe(area(a) + area(b) - inter);
    // every corner of both pictures is still inside some box
    const inside = (x: number, y: number) => rects.some((r) => x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]);
    for (const [x, y] of [[72, 100], [300, 300], [200, 200], [450, 420], [450, 200], [72, 300]]) expect(inside(x, y)).toBe(true);
  });

  it("strips or tiles that together make up a scan are left alone", () => {
    const strips = Array.from({ length: 8 }, (_, i) => img([0, i * 99, 612, (i + 1) * 99]));
    expect(naturalRects(strips, page)).toEqual([]);
    // tiled mosaic, 4 x 4 tiles each well under the share
    const tiles = [];
    for (let r = 0; r < 4; r++) for (let c = 0; c < 4; c++) tiles.push(img([c * 153, r * 198, (c + 1) * 153, (r + 1) * 198]));
    expect(naturalRects(tiles, page)).toEqual([]);
  });

  it("many small pictures keep only the largest ones", () => {
    const many = Array.from({ length: 40 }, (_, i) => img([10 + (i % 8) * 70, 10 + Math.floor(i / 8) * 40, 10 + (i % 8) * 70 + 30 + (i % 3), 10 + Math.floor(i / 8) * 40 + 30]));
    const rects = naturalRects(many, page);
    expect(rects).toHaveLength(24);
    expect(Math.min(...rects.map(area))).toBeGreaterThanOrEqual(900);
  });
});

describe("where a picture sits on the rotated page", () => {
  const raw: [number, number] = [600, 800];
  const r: [number, number, number, number] = [100, 50, 300, 200];
  it("is the same box when the page is not turned", () => {
    expect(displayRect(r, raw, 0)).toEqual(r);
  });
  it("follows the page's quarter turns (clockwise)", () => {
    // a quarter turn: the top edge becomes the right edge; the displayed page is 800 wide
    expect(displayRect(r, raw, 1)).toEqual([600, 100, 750, 300]);
    expect(displayRect(r, raw, 2)).toEqual([300, 600, 500, 750]);
    expect(displayRect(r, raw, 3)).toEqual([50, 300, 200, 500]);
  });
  it("keeps the box's area and stays inside the displayed page", () => {
    for (const rot of [1, 2, 3] as const) {
      const [x0, y0, x1, y1] = displayRect(r, raw, rot);
      expect((x1 - x0) * (y1 - y0)).toBe(200 * 150);
      const [dw, dh] = rot === 2 ? raw : [raw[1], raw[0]];
      expect(x0).toBeGreaterThanOrEqual(0);
      expect(y0).toBeGreaterThanOrEqual(0);
      expect(x1).toBeLessThanOrEqual(dw);
      expect(y1).toBeLessThanOrEqual(dh);
    }
  });
});
