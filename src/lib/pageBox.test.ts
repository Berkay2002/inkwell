import { describe, expect, it } from "vitest";
import { pageBoxCh } from "./pageBox";

describe("page number box width", () => {
  it("is the digits of the page count plus one, at least 3", () => {
    expect(pageBoxCh(1, false)).toBe(3);
    expect(pageBoxCh(9, false)).toBe(3);
    expect(pageBoxCh(91, false)).toBe(3);
    expect(pageBoxCh(100, false)).toBe(4);
    expect(pageBoxCh(1500, false)).toBe(5);
  });
  it("stops at 6 and copes with an empty count", () => {
    expect(pageBoxCh(123456, false)).toBe(6);
    expect(pageBoxCh(12345678, false)).toBe(6);
    expect(pageBoxCh(0, false)).toBe(3);
  });
  it("gives labelled documents the widest box", () => {
    expect(pageBoxCh(12, true)).toBe(6);
  });
});
