import { describe, expect, it } from "vitest";
import { formatRange, movedIndices, parseRange } from "./pageRange";

describe("page ranges", () => {
  it("parses what people type", () => {
    expect(parseRange("1, 3-5", 12)).toEqual([0, 2, 3, 4]);
    expect(parseRange(" 4 - 3 ,3", 12)).toEqual([2, 3]);
    expect(parseRange("", 12)).toEqual([]);
    expect(parseRange("1,,2,", 12)).toEqual([0, 1]);
  });
  it("rejects what it cannot use", () => {
    expect(parseRange("0", 12)).toBeNull();
    expect(parseRange("13", 12)).toBeNull();
    expect(parseRange("2-x", 12)).toBeNull();
    expect(parseRange("1.5", 12)).toBeNull();
  });
  it("formats and round-trips", () => {
    expect(formatRange([2, 3])).toBe("3-4");
    expect(formatRange([0, 2, 3, 4, 9])).toBe("1, 3-5, 10");
    expect(formatRange([])).toBe("");
    expect(parseRange(formatRange([5, 1, 2]), 12)).toEqual([1, 2, 5]);
  });
  it("knows where moved pages land", () => {
    expect(movedIndices([0], 3)).toEqual([2]);
    expect(movedIndices([4, 5], 1)).toEqual([1, 2]);
    expect(movedIndices([1, 6], 4)).toEqual([3, 4]);
  });
});
