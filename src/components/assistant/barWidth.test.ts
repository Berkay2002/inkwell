import { describe, expect, it } from "vitest";
import { barWidth } from "./barWidth";

describe("barWidth", () => {
  it("grows with the document area and stays inside it", () => {
    expect(barWidth(700)).toBe(434);
    expect(barWidth(1300)).toBe(806);
    expect(barWidth(2400)).toBe(960);
    expect(barWidth(380)).toBe(332);
    expect(barWidth(1300)).toBeGreaterThan(barWidth(900));
  });
});
