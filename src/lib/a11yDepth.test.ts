import { beforeEach, describe, expect, it, vi } from "vitest";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage: ((m: unknown) => void) | null = null;
  },
}));
// `run` is the edit lane: it applies the state the edit returned, like lib/edit.ts.
vi.mock("./edit", () => ({
  applyState: vi.fn(),
  run: vi.fn(async (_id: number, f: () => Promise<unknown>) => f()),
}));
vi.mock("./api", () => ({ pickSave: vi.fn(), toolCancel: vi.fn(() => Promise.resolve()) }));

import { invoke } from "@tauri-apps/api/core";
import { useStore } from "../store";
import { useA11y } from "./a11y";
import {
  type Region,
  type TagNode,
  CHECKPOINTS,
  artifactBlocked,
  dropOnto,
  flatten,
  headerCandidates,
  matterhornGroups,
  mergeRun,
  moveStep,
  moveTarget,
  moveTo,
  orderOf,
  rangeOf,
  readAloud,
  regionAt,
  ruleCount,
  splitBlocked,
  tagBounds,
  tagPage,
  tagPatch,
  draftOf,
  uncheckedCheckpoints,
} from "./a11yDepthLogic";
import type { Report, RuleResult } from "./a11yLogic";

const inv = vi.mocked(invoke);
const a = () => useA11y.getState();

const region = (xref: number, over: Partial<Region> = {}): Region => ({
  xref,
  role: "P",
  rawRole: "P",
  depth: 0,
  parent: 1,
  mcids: [xref],
  annots: [],
  bbox: [0, xref, 100, xref + 10],
  text: `text ${xref}`,
  alt: false,
  editable: true,
  ...over,
});
const node = (xref: number, role: string, children: TagNode[] = [], over: Partial<TagNode> = {}): TagNode => ({
  xref,
  role,
  rawRole: role,
  page: 0,
  alt: null,
  actualText: null,
  lang: null,
  id: null,
  scope: null,
  headers: [],
  mcids: [],
  annots: [],
  editable: true,
  children,
  ...over,
});

describe("reading order moves", () => {
  const order = [10, 11, 12, 13];
  it("moves one region and keeps the others in their order", () => {
    expect(moveTo(order, 12, 0)).toEqual([12, 10, 11, 13]);
    expect(moveStep(order, 11, 1)).toEqual([10, 12, 11, 13]);
    expect(moveStep(order, 10, -1)).toBeNull();
    expect(moveStep(order, 13, 1)).toBeNull();
    expect(moveTo(order, 99, 0)).toBeNull();
  });
  it("a drop puts the dragged region where the target was", () => {
    expect(dropOnto(order, 10, 12)).toEqual([11, 12, 10, 13]);
    expect(dropOnto(order, 13, 11)).toEqual([10, 13, 11, 12]);
    expect(dropOnto(order, 10, 10)).toBeNull();
  });
  it("sends only the regions the engine can address, in list order", () => {
    expect(orderOf([region(10), region(0), region(12)])).toEqual([10, 12]);
  });
});

describe("selection, merge and split rules", () => {
  const regions = [region(10), region(11), region(0), region(13), region(14, { mcids: [1, 2, 3] })];
  it("merges a run of regions next to each other, into the first", () => {
    expect(mergeRun(regions, [11, 10])).toEqual({ into: 10, from: [11] });
    expect(mergeRun(regions, [13, 14])).toEqual({ into: 13, from: [14] });
  });
  it("refuses one region, a gap, or one that can't be edited", () => {
    expect(mergeRun(regions, [10])).toBeNull();
    expect(mergeRun(regions, [10, 13])).toBeNull();
    expect(mergeRun([region(10), region(11, { editable: false })], [10, 11])).toBeNull();
  });
  it("selects a range in list order", () => {
    expect(rangeOf(regions, 14, 11)).toEqual([11, 13, 14]);
    expect(rangeOf(regions, 10, 99)).toEqual([10]);
  });
  it("says why a region can't be split or made an artifact", () => {
    expect(splitBlocked(regions[0])).toMatch(/one piece/);
    expect(splitBlocked(regions[4])).toBeNull();
    expect(splitBlocked(region(0, { editable: false, mcids: [1, 2] }))).toMatch(/inside its parent/);
    expect(artifactBlocked(regions[0])).toBeNull();
    expect(artifactBlocked(regions[2])).toMatch(/inside its parent/);
  });
  it("finds the smallest region under a point", () => {
    const big = region(1, { bbox: [0, 0, 200, 200] });
    const small = region(2, { bbox: [10, 10, 50, 50] });
    expect(regionAt([big, small], 20, 20)?.xref).toBe(2);
    expect(regionAt([big, small], 20, 20, 2)?.xref).toBe(1);
    expect(regionAt([big, small], 300, 20)).toBeNull();
  });
});

describe("read it to me", () => {
  it("says headings, figures and paragraphs the way a reader would", () => {
    const said = readAloud([
      region(1, { role: "H2", text: "Results" }),
      region(2, { text: "Sales went up." }),
      region(3, { role: "Figure", text: "", alt: false }),
      region(4, { role: "Figure", text: "", alt: true }),
      region(5, { role: "L", text: "" }),
    ]);
    expect(said).toEqual(["Heading 2: Results", "Sales went up.", "Figure, no description.", "Figure with a description.", "List, no text."]);
  });
});

describe("tag tree", () => {
  const tree = [
    node(100, "Document", [
      node(101, "H1"),
      node(102, "Table", [node(103, "TR", [node(104, "TH"), node(105, "TD")])]),
      node(0, "P", [], { editable: false }),
    ]),
  ];
  it("draws only the children of expanded nodes", () => {
    expect(flatten(tree, {}).map((r) => r.node.xref)).toEqual([100]);
    const rows = flatten(tree, { x100: true, x102: true });
    expect(rows.map((r) => [r.node.xref, r.depth])).toEqual([[100, 0], [101, 1], [102, 1], [103, 2], [0, 1]]);
    expect(rows[1]).toMatchObject({ parent: 100, index: 0, siblings: 3 });
    // an unaddressable node is keyed by where it is
    expect(rows[4].key).toBe("p0.2");
  });
  it("a large tree costs nothing for what is closed", () => {
    const wide = [node(1, "Document", Array.from({ length: 50_000 }, (_, i) => node(1000 + i, "P")))];
    expect(flatten(wide, {})).toHaveLength(1);
    expect(flatten(wide, { x1: true })).toHaveLength(50_001);
  });
  it("names the place by a sibling, not an index", () => {
    expect(moveTarget(tree, 102, "up")).toEqual({ parent: 100, anchor: 101, after: false });
    expect(moveTarget(tree, 101, "up")).toBeNull();
    expect(moveTarget(tree, 101, "down")).toEqual({ parent: 100, anchor: 102, after: true });
    // into the one above, as its last kid
    expect(moveTarget(tree, 102, "in")).toEqual({ parent: 101, anchor: null, after: false });
    // out: right after the parent; at the top of the tree it is parent 0
    expect(moveTarget(tree, 103, "out")).toEqual({ parent: 100, anchor: 102, after: true });
    expect(moveTarget(tree, 100, "out")).toBeNull();
    expect(moveTarget(tree, 100, "down")).toBeNull();
  });
  it("steps over siblings that have no object number, and moves a top-level tag beside its sibling", () => {
    const t = [node(1, "Document", [node(2, "P"), node(0, "Span", [], { editable: false }), node(3, "P")]), node(4, "Document")];
    expect(moveTarget(t, 3, "up")).toEqual({ parent: 1, anchor: 2, after: false });
    expect(moveTarget(t, 2, "down")).toEqual({ parent: 1, anchor: 3, after: true });
    expect(moveTarget(t, 4, "up")).toEqual({ parent: 0, anchor: 1, after: false });
    // only an unaddressable sibling below: there is nothing to place it by
    expect(moveTarget([node(1, "Document", [node(2, "P"), node(0, "Span", [], { editable: false })])], 2, "down")).toBeNull();
  });
  it("never moves into, or out of, a node that has no object number", () => {
    const t = [node(1, "Document", [node(0, "L", [], { editable: false }), node(2, "P")])];
    expect(moveTarget(t, 2, "in")).toBeNull();
    expect(moveTarget(t, 0, "up")).toBeNull();
  });
  it("lists the header cells of the table a cell sits in", () => {
    expect(headerCandidates(tree, 105).map((n) => n.xref)).toEqual([104]);
    expect(headerCandidates(tree, 101)).toEqual([]);
  });
  it("sends only what the editor changed", () => {
    const n = node(5, "TH", [], { alt: "Name", scope: null, headers: [] });
    const d = draftOf(n);
    expect(tagPatch(n, d)).toBeNull();
    expect(tagPatch(n, { ...d, role: "TD", alt: "" })).toEqual({ role: "TD", alt: "" });
    expect(tagPatch(n, { ...d, scope: "Column", headers: [7] })).toEqual({ scope: "Column", headers: [7] });
    // scope is for header cells only
    expect(tagPatch(node(6, "TD"), { ...draftOf(node(6, "TD")), scope: "Row" })).toBeNull();
  });
  it("finds a node's content on the page from the regions inside it", () => {
    const regions = [region(101, { bbox: [10, 10, 50, 30] }), region(104, { bbox: [60, 40, 90, 60] }), region(999, { bbox: [0, 0, 5, 5] })];
    expect(tagBounds(tree[0], regions)).toEqual([10, 10, 90, 60]);
    expect(tagBounds(tree[0].children[1], regions)).toEqual([60, 40, 90, 60]);
    expect(tagBounds(node(7, "P"), regions)).toBeNull();
    expect(tagPage(node(1, "Document", [node(2, "P", [], { page: 3, mcids: [0] })], { page: null }))).toBe(3);
  });
});

const rule = (over: Partial<RuleResult>): RuleResult => ({ rule: "title", name: "Title", group: "Document", status: "passed", hint: "", fix: null, findings: [], ...over });
const find = (message: string, page: number | null = 0) => ({ page, object: null, xref: null, rect: null, message });

describe("Matterhorn checkpoints", () => {
  const report: Report = {
    pages: 3,
    tagged: true,
    certified: false,
    signed: false,
    results: [
      rule({ rule: "figure-alt-text", status: "failed", findings: [find("a"), find("b"), find("... and 5 more", null)] }),
      rule({ rule: "tab-order", status: "failed", fix: "tabOrder", findings: [find("Page 1")] }),
      rule({ rule: "tagged-links", status: "passed" }),
      rule({ rule: "field-tooltips", status: "manual" }),
      rule({ rule: "reading-order", status: "passed" }),
      rule({ rule: "bookmarks", status: "failed", findings: [] }),
      rule({ rule: "some-future-rule", status: "passed" }),
    ],
  };
  const g = matterhornGroups(report);

  it("groups by checkpoint in protocol order, the unnumbered checks last", () => {
    expect(g.map((x) => x.number)).toEqual(["09", "13", "28", null]);
    expect(g[1]).toMatchObject({ title: "Graphics", status: "failed", count: 7, conditions: ["13-004"] });
    // the engine's "and 5 more" note is a count, not a finding
    expect(ruleCount(report.results[0])).toBe(7);
  });
  it("a checkpoint takes the worst state of its rules and counts only failed ones", () => {
    const annots = g.find((x) => x.number === "28")!;
    expect(annots.status).toBe("failed");
    expect(annots.count).toBe(1);
    expect(annots.rules.map((x) => x.rule)).toEqual(["tab-order", "field-tooltips", "tagged-links"]);
    expect(annots.conditions).toEqual(["28-005", "28-008", "28-009", "28-011"]);
    expect(g.find((x) => x.number === "09")!.status).toBe("passed");
  });
  it("puts a failed rule with no findings at one, and unknown rules in the unnumbered bucket", () => {
    const other = g.find((x) => x.number === null)!;
    expect(other.rules.map((x) => x.rule)).toEqual(["bookmarks", "some-future-rule"]);
    expect(other.count).toBe(1);
  });
  it("quotes no condition for the rules that only guess or always ask for a manual check", () => {
    const r: Report = { pages: 1, tagged: true, certified: false, signed: false, results: [rule({ rule: "color-contrast", status: "manual" }), rule({ rule: "reading-order", status: "passed" })] };
    const groups = matterhornGroups(r);
    expect(groups.map((x) => [x.number, x.status, x.conditions])).toEqual([
      ["04", "manual", []],
      ["09", "passed", []],
    ]);
  });
  it("lists the checkpoints no rule decides", () => {
    const un = uncheckedCheckpoints(g);
    expect(un).toHaveLength(Object.keys(CHECKPOINTS).length - 3);
    expect(un.find((c) => c.number === "03")?.title).toBe("Flickering");
    expect(un.find((c) => c.number === "13")).toBeUndefined();
  });
  it("numbers the 31 checkpoints of the protocol", () => {
    expect(Object.keys(CHECKPOINTS)).toHaveLength(31);
    expect(CHECKPOINTS["01"]).toBe("Real content tagged");
    expect(CHECKPOINTS["31"]).toBe("Fonts");
  });
});

// ---- the store -----------------------------------------------------------------------------------

const state = (rev: number) => ({ rev, pageCount: 12, sizes: [], path: "C:\\a.pdf", dirty: true, undo: "Change reading order", redo: null });

function reply(map: Record<string, unknown | ((args: unknown) => unknown)>) {
  inv.mockImplementation((cmd: string, args?: unknown) => {
    if (!(cmd in map)) return Promise.reject(`unexpected ${cmd}`);
    const v = map[cmd];
    return typeof v === "function" ? (v as (a: unknown) => Promise<unknown>)(args) : Promise.resolve(v);
  });
}
const ro = (regions: Region[], page = 0) => ({ page, tagged: true, regions, orphans: [] });

beforeEach(() => {
  vi.clearAllMocks();
  useA11y.setState({ docs: {}, open: false, view: "check" });
  useStore.setState({
    docs: { 1: { id: 1, name: "Report.pdf", rev: 3, pageCount: 12, currentPage: 0, sizes: [], nav: null } as never },
    order: [1],
    active: 1,
  });
});

describe("Reading order store", () => {
  const three = [region(10), region(11), region(12)];

  it("reads the page once per generation and keeps a selection that is still there", async () => {
    reply({ a11y_reading_order: ro(three) });
    await a().loadOrder(1, 0);
    await a().loadOrder(1, 0);
    expect(inv.mock.calls.filter((c) => c[0] === "a11y_reading_order")).toHaveLength(1);
    a().selectRegion(1, 11, "one");
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 4 } } });
    reply({ a11y_reading_order: ro([region(11), region(12)]) });
    await a().loadOrder(1, 0);
    expect(a().get(1).order).toMatchObject({ rev: 4, selected: [11] });
  });

  it("an older answer does not replace a newer one", async () => {
    let first: (v: unknown) => void = () => {};
    inv.mockImplementationOnce(() => new Promise((res) => (first = res)));
    const p = a().loadOrder(1, 0);
    reply({ a11y_reading_order: ro(three, 1) });
    await a().loadOrder(1, 1);
    first(ro(three, 0));
    await p;
    expect(a().get(1).order.page).toBe(1);
  });

  it("a reorder is one command with every addressable region", async () => {
    reply({ a11y_reading_order: ro([region(10), region(0), region(12)]), a11y_reorder: state(4) });
    await a().loadOrder(1, 0);
    expect(await a().dropRegion(1, 12, 10)).toBe(true);
    expect(inv).toHaveBeenCalledWith("a11y_reorder", { doc: 1, page: 0, order: [12, 10] });
    expect(a().get(1).phase).toBe("idle");
  });

  it("merges the selected run in one command", async () => {
    reply({ a11y_reading_order: ro(three), a11y_merge_many: state(4) });
    await a().loadOrder(1, 0);
    a().selectRegion(1, 10, "one");
    a().selectRegion(1, 12, "range");
    expect(a().get(1).order.selected).toEqual([10, 11, 12]);
    expect(await a().mergeSelected(1)).toBe(true);
    expect(inv).toHaveBeenCalledWith("a11y_merge_many", { doc: 1, into: 10, from: [11, 12] });
    expect(a().get(1).order.selected).toEqual([10]);
  });

  it("a certified document takes no edit: nothing is sent", async () => {
    const certified: Report = { pages: 1, tagged: true, certified: true, signed: true, results: [] };
    reply({ a11y_check: certified, a11y_reading_order: ro(three), a11y_reorder: state(4), a11y_merge_many: state(4), a11y_fix: state(4) });
    await a().check(1);
    await a().loadOrder(1, 0);
    a().selectRegion(1, 10, "one");
    a().selectRegion(1, 12, "range");
    expect(await a().dropRegion(1, 12, 10)).toBe(false);
    expect(await a().moveRegion(1, 11, -1)).toBe(false);
    expect(await a().mergeSelected(1)).toBe(false);
    expect(await a().fix(1, { fix: "tabOrder" })).toBe(false);
    const sent = inv.mock.calls.map((c) => c[0]);
    expect(sent.filter((c) => c === "a11y_reorder" || c === "a11y_merge_many" || c === "a11y_fix")).toEqual([]);
  });

  it("split and artifact are one command each", async () => {
    reply({ a11y_reading_order: ro(three), a11y_split_region: state(4), a11y_artifact: state(5) });
    await a().loadOrder(1, 0);
    await a().splitRegion(1, 10, 7);
    expect(inv).toHaveBeenCalledWith("a11y_split_region", { doc: 1, xref: 10, atMcid: 7, role: null });
    a().selectRegion(1, 11, "one");
    await a().artifactRegion(1, 11);
    expect(inv).toHaveBeenCalledWith("a11y_artifact", { doc: 1, xref: 11 });
    expect(a().get(1).order.selected).toEqual([]);
  });

  it("a refusal is shown and the page is read again", async () => {
    reply({ a11y_reading_order: ro(three), a11y_reorder: () => Promise.reject("a region also holds content on page 2") });
    await a().loadOrder(1, 0);
    expect(await a().moveRegion(1, 10, 1)).toBe(false);
    expect(a().get(1).order.error).toBe("a region also holds content on page 2");
    expect(a().get(1).phase).toBe("idle");
    expect(inv.mock.calls.filter((c) => c[0] === "a11y_reading_order")).toHaveLength(2);
  });

  it("reads the page again before the document is given back, so key repeat plans from the new order", async () => {
    let order = three;
    reply({
      a11y_reading_order: () => Promise.resolve(ro(order)),
      a11y_reorder: (args: unknown) => {
        order = (args as { order: number[] }).order.map((x) => three.find((r) => r.xref === x)!);
        // what applyState does: the revision moves
        useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: useStore.getState().docs[1].rev + 1 } } });
        return Promise.resolve(state(4));
      },
    });
    await a().loadOrder(1, 0);
    expect(await a().moveRegion(1, 10, 1)).toBe(true);
    expect(a().get(1).order.rev).toBe(useStore.getState().docs[1].rev);
    expect(a().get(1).order.data?.regions.map((r) => r.xref)).toEqual([11, 10, 12]);
    // the second step moves 10 after 12 (it is now at index 1), not after 11 again
    expect(await a().moveRegion(1, 10, 1)).toBe(true);
    expect(inv.mock.calls.filter((c) => c[0] === "a11y_reorder").map((c) => (c[1] as { order: number[] }).order)).toEqual([[11, 10, 12], [11, 12, 10]]);
  });

  it("refuses to plan from regions older than the document (undo came first) and reads the page again", async () => {
    reply({ a11y_reading_order: ro(three), a11y_reorder: state(4) });
    await a().loadOrder(1, 0);
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 4 } } });
    // all asked before the page is read again
    const asked = await Promise.all([a().moveRegion(1, 10, 1), a().dropRegion(1, 12, 10), a().splitRegion(1, 10, 1)]);
    expect(asked).toEqual([false, false, false]);
    expect(inv.mock.calls.some((c) => c[0] === "a11y_reorder" || c[0] === "a11y_split_region")).toBe(false);
    await vi.waitFor(() => expect(a().get(1).order.rev).toBe(4));
    expect(await a().moveRegion(1, 10, 1)).toBe(true);
  });

  it("does not start an edit while a fix holds the document", async () => {
    reply({ a11y_reading_order: ro(three), a11y_reorder: state(4) });
    await a().loadOrder(1, 0);
    useA11y.setState({ docs: { 1: { ...a().get(1), phase: "fixing" } } });
    expect(await a().moveRegion(1, 10, 1)).toBe(false);
    expect(inv.mock.calls.some((c) => c[0] === "a11y_reorder")).toBe(false);
  });
});

describe("Tags store", () => {
  const tree = [node(100, "Document", [node(101, "H1", [], { page: 0, mcids: [0] }), node(102, "P", [], { page: 0, mcids: [1] })])];

  it("reads the tree and the problems, and opens only the top of it", async () => {
    reply({ a11y_tags: tree, a11y_tag_problems: [{ page: 0, xref: 102, message: "MCID 1 is not in the ParentTree" }] });
    await a().loadTags(1);
    const t = a().get(1).tags;
    expect(t.tree).toEqual(tree);
    expect(t.problems).toHaveLength(1);
    expect(t.expanded).toEqual({ x100: true });
    // the whole document: no page argument
    expect(inv).toHaveBeenCalledWith("a11y_tags", { doc: 1, page: null });
  });

  it("a long document opens its tree for the current page", async () => {
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], pageCount: 400, currentPage: 7 } } });
    reply({ a11y_tags: tree, a11y_tag_problems: [] });
    await a().loadTags(1);
    expect(inv).toHaveBeenCalledWith("a11y_tags", { doc: 1, page: 7 });
  });

  it("selecting a tag jumps to its content and outlines it", async () => {
    reply({
      a11y_tags: tree,
      a11y_tag_problems: [],
      a11y_reading_order: ro([region(101, { bbox: [10, 100, 50, 130] }), region(102, { bbox: [10, 150, 90, 170] })]),
    });
    await a().loadTags(1);
    await a().selectTag(1, "x102");
    expect(a().get(1).tags.selected).toBe("x102");
    expect(a().get(1).shown).toMatchObject({ page: 0, rect: [10, 150, 90, 170] });
  });

  it("role, properties and moves are one command each", async () => {
    reply({ a11y_tags: tree, a11y_tag_problems: [], a11y_set_role: state(4), a11y_edit_tag: state(5), a11y_move_tag: state(6) });
    await a().loadTags(1);
    await a().setRole(1, 102, "H2");
    expect(inv).toHaveBeenCalledWith("a11y_set_role", { doc: 1, xref: 102, role: "H2" });
    await a().editTag(1, 101, { alt: "Cover", lang: "en-US" });
    expect(inv).toHaveBeenCalledWith("a11y_edit_tag", { doc: 1, xref: 101, patch: { alt: "Cover", lang: "en-US" } });
    expect(await a().moveTag(1, 102, "up")).toBe(true);
    expect(inv).toHaveBeenCalledWith("a11y_move_tag", { doc: 1, xref: 102, parent: 100, anchor: 101, after: false });
    // the first kid has nowhere to go up to
    expect(await a().moveTag(1, 101, "up")).toBe(false);
  });

  it("a move from a tree older than the document is refused, and the tree is read again", async () => {
    reply({ a11y_tags: tree, a11y_tag_problems: [], a11y_move_tag: state(6) });
    await a().loadTags(1);
    useStore.setState({ docs: { 1: { ...useStore.getState().docs[1], rev: 9 } } });
    expect(await a().moveTag(1, 102, "up")).toBe(false);
    expect(inv.mock.calls.some((c) => c[0] === "a11y_move_tag")).toBe(false);
    await vi.waitFor(() => expect(a().get(1).tags.rev).toBe(9));
    expect(await a().moveTag(1, 102, "up")).toBe(true);
  });

  it("a refused edit says why and reads the tree again", async () => {
    reply({ a11y_tags: tree, a11y_tag_problems: [], a11y_set_role: () => Promise.reject("Table can't go inside P") });
    await a().loadTags(1);
    expect(await a().setRole(1, 102, "Table")).toBe(false);
    expect(a().get(1).tags.error).toBe("Table can't go inside P");
    expect(inv.mock.calls.filter((c) => c[0] === "a11y_tags")).toHaveLength(2);
  });
});

describe("views", () => {
  it("opens the rail on the view asked for", async () => {
    const { openA11y } = await import("./a11y");
    openA11y("order");
    expect(a()).toMatchObject({ open: true, view: "order" });
    openA11y();
    expect(a().view).toBe("order");
  });
});
