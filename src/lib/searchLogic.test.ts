import { describe, expect, it } from "vitest";
import {
  DEFAULT_OPTS, type FileGroup, type Hit, approxTime, buildLine, byPosition, estimateLine, staleLine, staleResultsLine, changedOptions, filesLine, folderName, groupByPage, insertFile, optsKey, parseHistory,
  mergeHits, parseOpts, pickAfter, pickCurrent, pushHistory, resultsLine, shortDir, visibleRows,
} from "./searchLogic";

const hit = (page: number, y: number | null, text = "notice"): Hit => ({
  page,
  rects: y == null ? [] : [[10, y, 50, y + 10]],
  before: "a ",
  text,
  after: " b",
  source: y == null ? "bookmark" : "text",
});
const file = (index: number, hits = 1, items = 1): FileGroup => ({
  index, path: `C:\\Docs\\Housing\\f${index}.pdf`, name: `f${index}.pdf`, pageCount: 3, hits, pages: 1, truncated: false,
  items: Array.from({ length: items }, (_, i) => hit(i, 5)),
});

describe("find options", () => {
  it("fall back to the defaults for anything missing or of the wrong type", () => {
    expect(parseOpts(null)).toEqual(DEFAULT_OPTS);
    expect(parseOpts("not json")).toEqual(DEFAULT_OPTS);
    expect(parseOpts(JSON.stringify({ caseSensitive: true, wholeWords: "yes", extra: 1 }))).toEqual({ ...DEFAULT_OPTS, caseSensitive: true });
  });

  it("count what differs and make a key that changes with any option", () => {
    expect(changedOptions(DEFAULT_OPTS)).toBe(0);
    const o = { ...DEFAULT_OPTS, comments: true, ignoreAccents: true };
    expect(changedOptions(o)).toBe(2);
    expect(optsKey(o)).not.toBe(optsKey(DEFAULT_OPTS));
    expect(optsKey(o)).toBe(optsKey({ ...o }));
  });
});

describe("search history", () => {
  it("keeps the newest first, tidies white space and ignores blanks", () => {
    let h: string[] = [];
    h = pushHistory(h, "  notice   period ");
    h = pushHistory(h, "   ");
    h = pushHistory(h, "deposit");
    expect(h).toEqual(["deposit", "notice period"]);
  });

  it("moves a repeated query to the front instead of listing it twice", () => {
    expect(pushHistory(["b", "a", "c"], "a")).toEqual(["a", "b", "c"]);
  });

  it("holds the last ten", () => {
    let h: string[] = [];
    for (let i = 1; i <= 14; i++) h = pushHistory(h, `q${i}`);
    expect(h).toHaveLength(10);
    expect(h[0]).toBe("q14");
    expect(h[9]).toBe("q5");
  });

  it("reads a saved list defensively", () => {
    expect(parseHistory(null)).toEqual([]);
    expect(parseHistory("{}")).toEqual([]);
    expect(parseHistory(JSON.stringify(["a", 3, "a", " b ", "", null]))).toEqual(["a", "b"]);
    expect(parseHistory(JSON.stringify(Array.from({ length: 25 }, (_, i) => `q${i}`)))).toHaveLength(10);
  });
});

describe("results list", () => {
  it("groups sorted hits under their page and keeps each one's index", () => {
    const g = groupByPage([hit(1, 10), hit(1, 40), hit(4, 5)]);
    expect(g.map((x) => [x.page, x.items.map((i) => i.index)])).toEqual([[1, [0, 1]], [4, [2]]]);
  });

  it("orders by page, then top to bottom, with a bookmark ahead of the page text", () => {
    const list = [hit(2, 80), hit(1, 20), hit(2, null), hit(2, 30)].sort(byPosition);
    expect(list.map((h) => [h.page, h.rects[0]?.[1] ?? null])).toEqual([[1, 20], [2, null], [2, 30], [2, 80]]);
  });

  it("says what it has found, and whether it is still looking", () => {
    expect(resultsLine(0, 0, false)).toBe("Searching…");
    expect(resultsLine(0, 0, true)).toBe("No results");
    expect(resultsLine(1, 1, true)).toBe("1 result on 1 page");
    expect(resultsLine(12, 5, true)).toBe("12 results on 5 pages");
    expect(resultsLine(1200, 80, false)).toBe("1,200 results on 80 pages so far");
  });

  it("starts at the first hit from the page in view, or at the one asked for", () => {
    const hits = [hit(0, 10), hit(2, 100), hit(2, 300), hit(5, 10)];
    expect(pickCurrent([], 0)).toBe(-1);
    expect(pickCurrent(hits, 0)).toBe(0);
    expect(pickCurrent(hits, 1)).toBe(1);
    expect(pickCurrent(hits, 9)).toBe(0);
    expect(pickCurrent(hits, 0, { page: 2, y: 290 })).toBe(2);
    // the page asked for has not reported yet: fall back to the page in view
    expect(pickCurrent(hits, 1, { page: 7, y: 0 })).toBe(1);
  });
});

describe("across files", () => {
  it("keeps files in listing order whichever worker finishes first", () => {
    let list: FileGroup[] = [];
    for (const i of [3, 0, 2, 1]) list = insertFile(list, file(i));
    expect(list.map((f) => f.index)).toEqual([0, 1, 2, 3]);
  });

  it("shows the folder a file is in, short", () => {
    expect(shortDir("C:\\Docs\\Housing\\Lease.pdf")).toBe("Docs\\Housing");
    expect(shortDir("C:\\Lease.pdf")).toBe("C:");
    expect(shortDir("Lease.pdf")).toBe("");
    expect(folderName("C:\\Docs\\Housing\\")).toBe("Housing");
  });

  it("walks the rows that are showing: two per file until asked for more", () => {
    const files = [file(0, 5, 5), file(1, 1, 1)];
    expect(visibleRows(files, {})).toEqual([{ file: 0, item: 0 }, { file: 0, item: 1 }, { file: 1, item: 0 }]);
    expect(visibleRows(files, { 0: 4 }).filter((r) => r.file === 0)).toHaveLength(4);
    expect(visibleRows(files, { 0: 50 }).filter((r) => r.file === 0)).toHaveLength(5);
  });

  it("words the progress and the outcome", () => {
    expect(filesLine(0, 0, 38, 64, false, false)).toBe("Searching 38 of 64 files");
    expect(filesLine(3, 18, 64, 64, true, false)).toBe("18 results in 3 files");
    expect(filesLine(0, 0, 64, 64, true, false)).toBe("No results in 64 files");
    expect(filesLine(1, 2, 10, 64, true, true)).toBe("2 results in 1 file (stopped at 10 of 64)");
  });
});

describe("merging hits as pages report", () => {
  it("appends pages that come in order without touching the current hit", () => {
    const a = [hit(0, 10), hit(1, 20)];
    const r = mergeHits(a, [hit(2, 5), hit(2, 30)], a[1]);
    expect(r.hits.map((h) => [h.page, h.rects[0][1]])).toEqual([[0, 10], [1, 20], [2, 5], [2, 30]]);
    expect(r.keep).toBe(1);
  });

  it("merges a page that arrives late, sorted, and follows the current hit", () => {
    const a = [hit(0, 10), hit(4, 20), hit(4, 80)];
    const incoming = [hit(2, 50), hit(2, 5)];
    const r = mergeHits(a, incoming, a[1]);
    expect(r.hits.map((h) => [h.page, h.rects[0][1]])).toEqual([[0, 10], [2, 5], [2, 50], [4, 20], [4, 80]]);
    expect(r.keep).toBe(3);
    expect(r.hits[r.keep]).toBe(a[1]);
    // nothing is mutated
    expect(a).toHaveLength(3);
    expect(incoming.map((h) => h.rects[0][1])).toEqual([50, 5]);
  });

  it("handles an empty list, no current hit and nothing incoming", () => {
    expect(mergeHits([], [hit(1, 1)], null)).toEqual({ hits: [hit(1, 1)], keep: -1 });
    const a = [hit(0, 1)];
    expect(mergeHits(a, [], a[0])).toEqual({ hits: a, keep: 0 });
  });

  it("agrees with a full sort on shuffled input", () => {
    const all = Array.from({ length: 200 }, (_, i) => hit(Math.floor(i / 7), (i * 37) % 100));
    const sorted = [...all].sort(byPosition);
    let list: Hit[] = [];
    for (let i = 0; i < all.length; i += 13) list = mergeHits(list, all.slice(i, i + 13), null).hits;
    expect(list.map((h) => [h.page, h.rects[0][1]])).toEqual(sorted.map((h) => [h.page, h.rects[0][1]]));
  });
});

describe("catalog lines", () => {
  it("says what building would take", () => {
    expect(estimateLine({ files: 1800, secs: 120, indexBytes: 40 * 1024 * 1024 })).toBe("~1 800 files, about 2 minutes, ~40 MB");
    expect(estimateLine({ files: 1, secs: 3, indexBytes: 5000 })).toBe("~1 file, under a minute, ~5 KB");
    expect(estimateLine({ files: 0, secs: 0, indexBytes: 0 })).toBe("No PDFs in this folder");
    expect(approxTime(5400)).toBe("about 1.5 hours");
    expect(approxTime(3600)).toBe("about an hour");
  });

  it("badges only what a refresh would change", () => {
    expect(staleLine(undefined, 5)).toBe("");
    expect(staleLine({ added: 0, changed: 0, removed: 0, unusable: false }, 5)).toBe("");
    expect(staleLine({ added: 2, changed: 1, removed: 3, unusable: false }, 5)).toBe("1 changed, 2 new, 3 gone");
    expect(staleLine({ added: 0, changed: 0, removed: 0, unusable: true }, 0)).toBe("Not built yet");
    expect(staleLine({ added: 0, changed: 0, removed: 0, unusable: true }, 9)).toBe("Needs a rebuild");
  });

  it("says which results a catalog search left out because the files changed", () => {
    expect(staleResultsLine(1)).toBe("1 file changed since this catalog was built, so its results are left out. Refresh the catalog to search it again.");
    expect(staleResultsLine(3)).toMatch(/^3 files changed .* their results are left out/);
  });

  it("summarises a build, and a stopped one keeps the old index", () => {
    const b = { files: 30, added: 0, changed: 2, removed: 0, damaged: 1, password: 1, cancelled: false };
    expect(buildLine(b)).toBe("30 files, 2 changed; 2 could not be read");
    expect(buildLine({ ...b, tooLarge: 3 })).toBe("30 files, 2 changed; 2 could not be read, 3 too large to index");
    expect(buildLine({ ...b, damaged: 0, password: 0, tooLarge: 1 })).toBe("30 files, 2 changed; 1 too large to index");
    expect(buildLine({ ...b, cancelled: true })).toMatch(/previous index/);
    expect(buildLine({ ...b, cancelled: true }, false)).toMatch(/Build it again/);
  });
});

describe("landing after a replacement", () => {
  const hits = [hit(0, 100), hit(0, 100), hit(0, 300), hit(1, 40)];
  hits[1].rects = [[120, 100, 160, 110]];

  it("goes to the first match at or after the replaced one", () => {
    // The match at (10, 100) was replaced by text without the find text: the next one is the one beside it.
    expect(pickAfter([hits[1], hits[2], hits[3]], { page: 0, y: 100, x: 10 })).toBe(0);
    // A match later on the same line, on a later line, then on the next page.
    expect(pickAfter(hits, { page: 0, y: 100, x: 11 })).toBe(1);
    expect(pickAfter(hits, { page: 0, y: 150, x: 0 })).toBe(2);
    expect(pickAfter(hits, { page: 0, y: 301, x: 0 })).toBe(3);
  });

  it("steps over the matches the replacement holds, and wraps at the end", () => {
    // The replaced match is still there (the replacement holds the find text once): skip it.
    expect(pickAfter(hits, { page: 0, y: 100, x: 10 }, 1)).toBe(1);
    expect(pickAfter(hits, { page: 1, y: 40, x: 10 }, 1)).toBe(0);
    expect(pickAfter([], { page: 0, y: 0, x: 0 })).toBe(-1);
  });
});
