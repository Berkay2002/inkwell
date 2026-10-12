import { describe, expect, it } from "vitest";
import { formatOrder, pagesIn, parseOrder, planFiles, securityDropped, togglePage, badRangeMessage } from "./combineLogic";

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

describe("the no-password footer", () => {
  const plain = { password: "", locked: null };
  it("shows for an unlocked row and for an owner-only source that has no password to enter", () => {
    expect(securityDropped([plain], [{ encrypted: false }])).toBe(false);
    expect(securityDropped([{ password: "pw", locked: null }], [{ encrypted: true }])).toBe(true);
    expect(securityDropped([plain, plain], [{ encrypted: false }, { encrypted: true }])).toBe(true);
    // a typed password counts before the plan has caught up
    expect(securityDropped([{ password: "pw", locked: null }], [])).toBe(true);
  });
  it("does not show for a locked row that was never unlocked: it is left out", () => {
    expect(securityDropped([{ password: "", locked: 3 }], [{ encrypted: false }])).toBe(false);
    expect(securityDropped([{ password: "", locked: 3 }], [])).toBe(false);
    expect(securityDropped([plain, { password: "", locked: 3 }], [{ encrypted: false }, { encrypted: false }])).toBe(false);
    // the file next to it that does go in still says so
    expect(securityDropped([{ password: "", locked: 3 }, plain], [{ encrypted: false }, { encrypted: true }])).toBe(true);
    // unlocked, but its owner allows no copying: left out too, the plan says so
    expect(securityDropped([{ password: "pw", locked: null }], [{ encrypted: false }])).toBe(false);
  });
});

describe("a plan made before the list changed", () => {
  const row = (key: number, locked: number | null = null) => ({ key, password: "", locked });
  const files = (...encrypted: boolean[]) => encrypted.map((e) => ({ encrypted: e }));
  const dropped = (rows: ReturnType<typeof row>[], plan: { encrypted: boolean }[], planKeys: number[]) =>
    securityDropped(rows, planFiles(rows, plan, planKeys));
  it("follows a row that moved", () => {
    // planned as [1 locked, 2 encrypted]; the rows were then swapped
    expect(dropped([row(2), row(1, 3)], files(false, true), [1, 2])).toBe(true);
    // planned as [1 encrypted, 2 plain]; the rows were then swapped
    expect(dropped([row(2), row(1)], files(true, false), [1, 2])).toBe(true);
  });
  it("does not give a remaining row the security of a removed one", () => {
    expect(dropped([row(2)], files(true, false), [1, 2])).toBe(false);
  });
  it("says nothing about a bad range of a row that was removed", () => {
    const bad = [
      { state: { state: "badRange" }, message: "Page 9 is not in a.pdf" },
      { state: { state: "ready" }, message: null },
    ];
    expect(badRangeMessage([{ key: 1 }, { key: 2 }], bad, [1, 2])).toBe("Page 9 is not in a.pdf");
    expect(badRangeMessage([{ key: 2 }], bad, [1, 2])).toBeUndefined();
  });
  it("lets a row the plan has not seen stand on its password", () => {
    expect(planFiles([{ key: 5 }], files(true), [1])).toEqual([undefined]);
    expect(dropped([{ key: 5, password: "pw", locked: null }], files(false), [1])).toBe(true);
  });
});
