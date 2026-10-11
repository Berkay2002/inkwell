import { describe, expect, it } from "vitest";
import { formatOrder, pagesIn, parseOrder, togglePage } from "./combineLogic";

describe("combine page ranges", () => {
  it("reads them in the order written, backwards ranges backwards", () => {
    expect(parseOrder("1-3, 6, 9-7", 10)).toEqual([0, 1, 2, 5, 8, 7, 6]);
    expect(parseOrder("2, 1", 3)).toEqual([1, 0]);
  });

  it("empty and 'all' are every page, an open end runs to the last", () => {
    expect(parseOrder("", 3)).toEqual([0, 1, 2]);
    expect(parseOrder(" All ", 2)).toEqual([0, 1]);
    expect(parseOrder("4-", 6)).toEqual([3, 4, 5]);
  });

  it("refuses what does not fit the file", () => {
    expect(parseOrder("0", 5)).toBeNull();
    expect(parseOrder("1-9", 5)).toBeNull();
    expect(parseOrder("a", 5)).toBeNull();
    expect(parseOrder(",", 5)).toBeNull();
  });

  it("writes runs as ranges and every page in order as nothing", () => {
    expect(formatOrder([0, 1, 2, 5, 8, 7, 6], 10)).toBe("1-3, 6, 9-7");
    expect(formatOrder([0, 1, 2], 3)).toBe("");
    expect(formatOrder([2, 0], 3)).toBe("3, 1");
  });

  it("a click takes a page out or puts it back at the end, and keeps the last one", () => {
    expect(togglePage("", 4, 1)).toBe("1, 3-4");
    expect(togglePage("1, 3-4", 4, 1)).toBe("1, 3-4, 2");
    expect(togglePage("2", 4, 1)).toBeNull();
    expect(togglePage("9", 4, 1)).toBeNull();
    expect(togglePage("1", 4, 9)).toBeNull();
  });

  it("the thumbnails read the text, not the other way round", () => {
    expect([...(pagesIn("2-3", 5) ?? [])]).toEqual([1, 2]);
    expect(pagesIn("nope", 5)).toBeNull();
  });
});
