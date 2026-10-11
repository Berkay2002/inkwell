import { describe, expect, it } from "vitest";
import { appIsDark, naturalRects, pagesAreDark, parsePageDark, togglePageDark } from "./pageDark";

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
});
