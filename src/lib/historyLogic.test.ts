import { describe, expect, it } from "vitest";
import type { Change, Comparison, Source, VersionInfo } from "./history";
import {
  alignRows,
  categoryCounts,
  categoryOf,
  dayLabel,
  flatOrder,
  groupChanges,
  groupVersions,
  inlineRuns,
  marksFor,
  matchesFilter,
  differencesWord,
  mergeLine,
  pageChips,
  pageSummary,
  progressFraction,
  progressLabel,
  parentOf,
  rangeField,
  rowOf,
  sameSides,
  stepIndex,
  setupShow,
  summaryChips,
  toneOf,
  trackDays,
  uncovered,
} from "./historyLogic";

const at = (y: number, mo: number, d: number, h = 12, mi = 0) => new Date(y, mo - 1, d, h, mi).getTime();
const NOW = at(2026, 10, 10, 15);

const ver = (id: string, created: number, o: Partial<VersionInfo> = {}): VersionInfo => ({
  id,
  parents: [],
  created,
  author: { type: "user" },
  kind: { type: "save" },
  label: "Saved",
  summary: [],
  name: null,
  pagesTouched: [],
  size: 1,
  broken: false,
  ...o,
});

describe("day grouping", () => {
  it("labels today, yesterday and older days", () => {
    expect(dayLabel(at(2026, 10, 10, 1), NOW)).toBe("Today");
    expect(dayLabel(at(2026, 10, 9, 23), NOW)).toBe("Yesterday");
    expect(dayLabel(at(2026, 10, 2), NOW)).toBe("2 Oct");
    expect(dayLabel(at(2025, 12, 31), NOW)).toBe("31 Dec 2025");
  });

  it("yesterday works across a month boundary", () => {
    expect(dayLabel(at(2026, 9, 30), at(2026, 10, 1, 8))).toBe("Yesterday");
  });

  it("groups newest first, pins named versions and keeps them out of the days", () => {
    const vs = [
      ver("a", at(2026, 10, 9, 16)),
      ver("b", at(2026, 10, 10, 9)),
      ver("c", at(2026, 10, 10, 14), { author: { type: "assistant", turn_id: "t" } }),
      ver("d", at(2026, 10, 2, 10), { name: "Signed by tenant" }),
    ];
    const g = groupVersions(vs, "all", false, NOW);
    expect(g.pinned.map((v) => v.id)).toEqual(["d"]);
    expect(g.days.map((d) => [d.label, d.items.map((v) => v.id)])).toEqual([
      ["Today", ["c", "b"]],
      ["Yesterday", ["a"]],
    ]);
  });

  it("only named versions hides the days", () => {
    const vs = [ver("a", at(2026, 10, 9)), ver("n", at(2026, 10, 8), { name: "Final" })];
    const g = groupVersions(vs, "all", true, NOW);
    expect(g.pinned.map((v) => v.id)).toEqual(["n"]);
    expect(g.days).toEqual([]);
  });
});

describe("filters", () => {
  const you = ver("y", 1);
  const asst = ver("a", 2, { author: { type: "assistant", turn_id: "t" } });
  const out = ver("o", 3, { author: { type: "external" }, kind: { type: "external" } });
  const auto = ver("s", 4, { author: { type: "autosave" }, kind: { type: "autosave" } });
  const first = ver("i", 0, { author: { type: "external" }, kind: { type: "import" } });
  it("splits by author", () => {
    const names = (f: Parameters<typeof matchesFilter>[1]) => [you, asst, out, auto, first].filter((v) => matchesFilter(v, f)).map((v) => v.id);
    expect(names("all")).toEqual(["y", "a", "o", "s", "i"]);
    expect(names("you")).toEqual(["y", "s"]);
    expect(names("assistant")).toEqual(["a"]);
    expect(names("outside")).toEqual(["o"]);
  });

  it("combines with the named toggle", () => {
    const vs = [ver("n", 5, { name: "Mine" }), ver("m", 6, { name: "Theirs", author: { type: "assistant", turn_id: "t" } })];
    expect(groupVersions(vs, "assistant", true, NOW).pinned.map((v) => v.id)).toEqual(["m"]);
  });
});

describe("version helpers", () => {
  it("finds the parent, else the next older version", () => {
    const a = ver("a", 1);
    const b = ver("b", 2, { parents: ["a"] });
    const c = ver("c", 3, { parents: ["gone"] });
    expect(parentOf([c, b, a], b)?.id).toBe("a");
    expect(parentOf([c, b, a], c)?.id).toBe("b");
    expect(parentOf([c, b, a], a)).toBeNull();
  });

  it("page chips are 1-based and capped", () => {
    expect(pageChips([2, 0])).toEqual(["p. 1", "p. 3"]);
    expect(pageChips([0, 1, 2, 3, 4])).toEqual(["p. 1", "p. 2", "p. 3", "+2"]);
    expect(pageChips([])).toEqual([]);
  });
});

const chg = (kind: Change["kind"], o: Partial<Change> = {}): Change => ({
  kind,
  aPage: null,
  bPage: null,
  rectA: null,
  rectB: null,
  label: kind,
  detail: { type: "none" },
  ...o,
});

describe("change lists", () => {
  const changes: Change[] = [
    chg("metadataChanged"),
    chg("textChanged", { aPage: 2, bPage: 2 }),
    chg("annotationAdded", { bPage: 0 }),
    chg("pageAdded", { bPage: 2 }),
    chg("formFieldChanged", { aPage: 0, bPage: 0 }),
  ];

  it("groups by page with the document last and honours the category filter", () => {
    const g = groupChanges(changes, null);
    expect(g.map((x) => [x.page, x.items.map((i) => i.index)])).toEqual([
      [0, [2, 4]],
      [2, [1, 3]],
      [null, [0]],
    ]);
    expect(flatOrder(g)).toEqual([2, 4, 1, 3, 0]);
    const only = groupChanges(changes, new Set(["text", "forms"]));
    expect(flatOrder(only)).toEqual([4, 1]);
  });

  it("counts by category", () => {
    expect(categoryCounts(changes)).toEqual({ text: 1, formatting: 0, images: 0, annotations: 1, pages: 1, forms: 1, metadata: 1 });
  });

  it("stepper wraps and starts from either end", () => {
    expect(stepIndex(-1, 1, 4)).toBe(0);
    expect(stepIndex(-1, -1, 4)).toBe(3);
    expect(stepIndex(3, 1, 4)).toBe(0);
    expect(stepIndex(0, -1, 4)).toBe(3);
    expect(stepIndex(1, 1, 4)).toBe(2);
    expect(stepIndex(0, 1, 0)).toBe(-1);
  });

  it("tone follows the direction of a text change", () => {
    const run = (op: "equal" | "insert" | "delete") => ({ op, text: "x", boxesA: [], boxesB: [] });
    const t = (ops: ("equal" | "insert" | "delete")[]) => chg("textChanged", { detail: { type: "text", runs: ops.map(run) } });
    expect(toneOf(t(["equal", "insert"]))).toBe("add");
    expect(toneOf(t(["delete"]))).toBe("del");
    expect(toneOf(t(["delete", "insert"]))).toBe("chg");
    expect(toneOf(chg("annotationRemoved"))).toBe("del");
  });
});

describe("page summary and rows", () => {
  const cmp = (o: Partial<Comparison["pages"]>, changes: Change[] = []): Comparison => ({
    pages: { aPages: 6, bPages: 6, aIds: [0, 1, 2, 3, 4, 5], bIds: [0, 1, 2, 3, 4, 5], bToA: [0, 1, null, 2, 3, 4], removedA: [], ...o },
    changes,
    identical: false,
  });

  const four = { aPages: 4, bPages: 4, aIds: [0, 1, 2, 3], bIds: [0, 1, 2, 3] };

  it("names inserted, removed, moved and rotated pages", () => {
    const moved = [
      chg("pageMoved", { detail: { type: "pageMoved", from: 4, to: 8 } }),
      chg("pageMoved", { detail: { type: "pageMoved", from: 5, to: 9 } }),
      chg("pageMoved", { detail: { type: "pageMoved", from: 0, to: 2 } }),
      chg("pageRotated", { bPage: 3 }),
    ];
    expect(pageSummary(cmp({ removedA: [1] }, moved))).toEqual([
      "Page 3 inserted",
      "Page 2 removed",
      "Page 1 moved to 3",
      "Pages 5-6 moved to 9",
      "Page 4 rotated",
    ]);
    expect(pageSummary(cmp({ bIds: [0, 1, 2, 3], bToA: [0, null, null, 1] }))).toEqual(["Pages 2-3 inserted"]);
  });

  it("aligns pages into rows, removed pages keep their place", () => {
    const rows = alignRows({ ...four, bToA: [0, null, 1, 3], removedA: [2] });
    expect(rows).toEqual([
      { a: 0, b: 0 },
      { a: null, b: 1 },
      { a: 1, b: 2 },
      { a: 2, b: null },
      { a: 3, b: 3 },
    ]);
    expect(rowOf(rows, chg("textChanged", { aPage: 1, bPage: 2 }))).toBe(2);
    expect(rowOf(rows, chg("pageRemoved", { aPage: 2 }))).toBe(3);
    expect(rowOf(rows, chg("metadataChanged"))).toBe(-1);
  });

  it("puts a replaced page on one row, both sides", () => {
    // A pages 1 and 3 were swapped for B pages 1 and 3 (each pair shares a gap); only the first has a pageReplaced change
    const pages = { ...four, bToA: [0, null, 2, null], removedA: [1, 3] };
    const rows = alignRows(pages);
    expect(rows).toEqual([
      { a: 0, b: 0 },
      { a: 1, b: 1 },
      { a: 2, b: 2 },
      { a: 3, b: 3 },
    ]);
    expect(rowOf(rows, chg("pageReplaced", { aPage: 1, bPage: 1 }))).toBe(1);
    const replaced = chg("pageReplaced", { aPage: 1, bPage: 1 });
    expect(pageSummary(cmp(pages, [replaced]))).toEqual(["Page 4 inserted", "Page 4 removed", "Page 2 replaced"]);
  });

  it("rows and the summary use the pages' own numbers when a side has a range", () => {
    // A pages 4-7 against B pages 2-4: A's 4 pairs B's 2, B's 3 is new, A's 7 has no partner
    const pages = { aPages: 4, bPages: 3, aIds: [3, 4, 5, 6], bIds: [1, 2, 3], bToA: [3, null, 5], removedA: [4, 6] };
    expect(alignRows(pages)).toEqual([
      { a: 3, b: 1 },
      { a: 4, b: 2 },
      { a: 5, b: 3 },
      { a: 6, b: null },
    ]);
    expect(pageSummary(cmp(pages, [chg("pageReplaced", { aPage: 4, bPage: 2 })]))).toEqual(["Page 7 removed", "Page 3 replaced"]);
  });

  it("an inserted page next to a removed one in another gap is not paired", () => {
    expect(alignRows({ aPages: 2, bPages: 2, aIds: [0, 1], bIds: [0, 1], bToA: [null, 0], removedA: [1] })).toEqual([
      { a: null, b: 0 },
      { a: 0, b: 1 },
      { a: 1, b: null },
    ]);
  });
});

describe("inline word diff", () => {
  it("keeps a little context around the edit", () => {
    const eq = (text: string) => ({ op: "equal" as const, text, boxesA: [], boxesB: [] });
    const out = inlineRuns([
      eq("one two three four five six seven eight nine ten"),
      { op: "delete", text: "old", boxesA: [], boxesB: [] },
      { op: "insert", text: "new", boxesA: [], boxesB: [] },
      eq("a b c d e f g h i j"),
    ]);
    expect(out.map((r) => r.text)).toEqual(["… eight nine ten", "old", "new", "a b c …"]);
    expect(inlineRuns([eq("short bit")])[0].text).toBe("short bit");
  });
});

describe("marks", () => {
  it("merges word boxes on one line", () => {
    expect(mergeLine([[0, 0, 10, 10], [12, 0, 20, 10], [0, 20, 10, 30]])).toEqual([[0, 0, 20, 10], [0, 20, 10, 30]]);
  });

  it("draws inserted words on B, deleted on A, nothing for additions on A", () => {
    const text = chg("textChanged", {
      aPage: 0,
      bPage: 0,
      detail: {
        type: "text",
        runs: [
          { op: "delete", text: "blank", boxesA: [[1, 1, 5, 5]], boxesB: [] },
          { op: "insert", text: "Novak", boxesA: [], boxesB: [[2, 2, 9, 9]] },
        ],
      },
    });
    const added = chg("annotationAdded", { bPage: 1, rectB: [0, 0, 4, 4] });
    const a = marksFor([text, added], "a");
    const b = marksFor([text, added], "b");
    expect(a.map((m) => [m.tone, m.change])).toEqual([["del", 0]]);
    expect(b.map((m) => [m.tone, m.change, m.page])).toEqual([["add", 0, 0], ["add", 1, 1]]);
  });

  it("drops pixel boxes already explained by a mark", () => {
    const marks = [{ rect: [0, 0, 100, 20] as [number, number, number, number], tone: "add" as const, change: 0, page: 0 }];
    expect(uncovered([[5, 2, 50, 18], [200, 200, 220, 220]], marks)).toEqual([[200, 200, 220, 220]]);
  });
});

describe("version track", () => {
  it("groups oldest first by day", () => {
    const vs = [ver("c", at(2026, 10, 10, 9)), ver("a", at(2026, 10, 9, 8)), ver("b", at(2026, 10, 9, 16)), ver("d", at(2026, 10, 10, 14))];
    const days = trackDays(vs, NOW);
    expect(days.map((d) => [d.label, d.items.map((v) => v.id)])).toEqual([["Yesterday", ["a", "b"]], ["Today", ["c", "d"]]]);
  });
});

describe("compare summary and progress", () => {
  it("formatting is its own category", () => {
    const f = chg("formatChanged", { aPage: 0, bPage: 0, detail: { type: "format", text: "x", props: [{ name: "font", old: "Helvetica", new: "Times-Roman" }] } });
    expect(categoryOf(f)).toBe("formatting");
    expect(toneOf(f)).toBe("chg");
    expect(categoryCounts([f, f]).formatting).toBe(2);
  });

  it("summary chips list only the categories with differences", () => {
    const counts = categoryCounts([chg("textChanged"), chg("textChanged"), chg("pageAdded", { bPage: 3 })]);
    expect(summaryChips(counts).map((c) => [c.label, c.n])).toEqual([["Text", 2], ["Pages", 1]]);
    expect(differencesWord(0)).toBe("No differences");
    expect(differencesWord(1)).toBe("1 difference");
    expect(differencesWord(12)).toBe("12 differences");
  });

  it("progress words and fraction follow the engine's stages", () => {
    expect(progressLabel(null)).toBe("Starting\u2026");
    expect(progressLabel({ stage: "second", done: 3, total: 10 })).toBe("Reading the second file, page 3 of 10");
    expect(progressLabel({ stage: "pages", done: 0, total: 500 })).toBe("Comparing page 1 of 500");
    expect(progressFraction(null)).toBe(0);
    expect(progressFraction({ stage: "first", done: 5, total: 10 })).toBeCloseTo(0.2);
    expect(progressFraction({ stage: "pages", done: 250, total: 500 })).toBeCloseTo(0.9);
    expect(progressFraction({ stage: "document", done: 0, total: 0 })).toBe(1);
  });

  it("the setup switches say which categories the result starts with", () => {
    expect(setupShow({ text: true, formatting: true, images: true })).toBeUndefined();
    expect(setupShow({ text: true, formatting: false, images: true })).toEqual(["text", "images", "annotations", "pages", "forms", "metadata"]);
    expect(setupShow({ text: false, formatting: false, images: false })).toEqual(["annotations", "pages", "forms", "metadata"]);
  });
});

describe("compare page range field", () => {
  it("an empty field is every page", () => {
    expect(rangeField("", 12)).toEqual({ pages: null, error: null });
    expect(rangeField("  ", null)).toEqual({ pages: null, error: null });
  });

  it("reads the organize syntax into 0-based pages", () => {
    expect(rangeField("3-10", 12)).toEqual({ pages: [2, 3, 4, 5, 6, 7, 8, 9], error: null });
    expect(rangeField("1, 3-4, 4", null)).toEqual({ pages: [0, 2, 3], error: null });
  });

  it("says what is wrong", () => {
    expect(rangeField("abc", 12).error).toBe("Enter pages like 1-3, 5");
    expect(rangeField("0-2", 12).error).toBe("Enter pages like 1-3, 5");
    expect(rangeField(",", null).error).toBe("Enter pages like 1-3, 5");
    expect(rangeField("5-20", 12)).toEqual({ pages: null, error: "This file has 12 pages" });
    expect(rangeField("2", 1).error).toBe("This file has 1 page");
    // the page count is not known yet: only the syntax is judged, the engine refuses a page past the end
    expect(rangeField("5-20", null).pages).toHaveLength(16);
  });

  it("turns a huge range away without expanding it", () => {
    expect(rangeField("1-20000000", 12)).toEqual({ pages: null, error: "This file has 12 pages" });
    expect(rangeField("1-20000000", null)).toEqual({ pages: null, error: "Enter pages like 1-3, 5" });
    expect(rangeField("1-99999999999999999999", null).pages).toBeNull();
  });
});

describe("comparing a file with itself", () => {
  const doc = "C:\\Docs\\Contract.pdf";
  const pick = (token: string, path?: string): Source => ({ kind: "picked", token, path });
  const all: [null, null] = [null, null];
  const unknown: [null, null] = [null, null];

  it("the same path picked twice, or the open document picked again, is the same file", () => {
    expect(sameSides(pick("t1", doc), pick("t2", "c:/docs/contract.PDF"), doc, all, [5, 5])).toBe(true);
    expect(sameSides({ kind: "current" }, pick("t1", doc), doc, all, [5, 5])).toBe(true);
    expect(sameSides({ kind: "current" }, { kind: "current" }, doc, all, unknown)).toBe(true);
  });

  it("another path or name is another file", () => {
    expect(sameSides(pick("t1", doc), pick("t2", "C:\\Other\\Contract.pdf"), doc, all, [5, 5])).toBe(false);
    expect(sameSides({ kind: "current" }, pick("t1", "C:\\Other\\Contract.pdf"), doc, all, [5, 5])).toBe(false);
  });

  it("a past version against the current file, or against another version, is a comparison", () => {
    expect(sameSides({ kind: "version", id: "v1" }, { kind: "current" }, doc, all, [5, 5])).toBe(false);
    expect(sameSides({ kind: "version", id: "v1" }, { kind: "version", id: "v2" }, doc, all, [5, 5])).toBe(false);
    expect(sameSides({ kind: "version", id: "v1" }, { kind: "version", id: "v1" }, doc, all, [5, 5])).toBe(true);
  });

  it("different pages of one file are allowed, the same pages written two ways are not", () => {
    expect(sameSides({ kind: "current" }, { kind: "current" }, doc, [[0, 1], [2, 3]], [5, 5])).toBe(false);
    expect(sameSides({ kind: "current" }, { kind: "current" }, doc, [[0, 1], [0, 1]], [5, 5])).toBe(true);
    expect(sameSides({ kind: "current" }, { kind: "current" }, doc, [null, [0, 1, 2, 3, 4]], [5, 5])).toBe(true);
    expect(sameSides({ kind: "current" }, { kind: "current" }, doc, [null, [0, 1]], [5, 5])).toBe(false);
    // every page against some, with the count not known yet: not decided as the same
    expect(sameSides({ kind: "current" }, { kind: "current" }, doc, [null, [0, 1]], [null, null])).toBe(false);
  });

  it("a picked file without a known path is told apart by its token", () => {
    expect(sameSides(pick("t1"), pick("t1"), null, all, [5, 5])).toBe(true);
    expect(sameSides(pick("t1"), pick("t2"), null, all, [5, 5])).toBe(false);
  });
});
