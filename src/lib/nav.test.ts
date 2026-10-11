import { beforeEach, describe, expect, it, vi } from "vitest";

// store.ts reads localStorage at import time; vitest runs in node.
vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
    clear: () => m.clear(),
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./edit", () => ({ editBookmarks: vi.fn(), editLinks: vi.fn(), applyState: vi.fn() }));
vi.mock("./api", () => ({
  bookmarksList: vi.fn(),
  bookmarksHeadings: vi.fn(),
  linksList: vi.fn(),
  linksUrls: vi.fn(),
  linkCheckUrl: vi.fn(),
  linkOpenUrl: vi.fn(),
}));

import { message } from "@tauri-apps/plugin-dialog";
import * as api from "./api";
import type { Bookmark, Bookmarks } from "./api";
import { editBookmarks, editLinks } from "./edit";
import { confirmOpen, dragRect, findBookmark, hexToRgb, isUnder, locate, rgbToHex, targetLabel, titleFromText, useNav, visibleRows } from "./nav";

const bm = (id: number, title: string, children: Bookmark[] = [], over: Partial<Bookmark> = {}): Bookmark => ({
  id,
  title,
  dest: { kind: "page", page: 0, at: null },
  open: true,
  bold: false,
  italic: false,
  color: null,
  children,
  ...over,
});

const tree = (): Bookmark[] => [
  bm(1, "Chapter 1", [bm(2, "Section 1.1", [bm(3, "Detail")]), bm(4, "Section 1.2")]),
  bm(5, "Chapter 2", [bm(6, "Section 2.1")], { open: false }),
  bm(7, "Chapter 3"),
];
const listed = (items: Bookmark[], rev = 0): Bookmarks => ({ rev, items, count: 7, truncated: false });

describe("tree helpers", () => {
  it("locates an item with its parent and index", () => {
    expect(locate(tree(), 4)).toMatchObject({ parent: 1, index: 1 });
    expect(locate(tree(), 7)).toMatchObject({ parent: null, index: 2 });
    expect(locate(tree(), 99)).toBeNull();
  });

  it("finds an item at any depth", () => {
    expect(findBookmark(tree(), 3)?.title).toBe("Detail");
    expect(findBookmark(tree(), 99)).toBeNull();
  });

  it("tells whether an item lies under another", () => {
    expect(isUnder(tree(), 3, 1)).toBe(true);
    expect(isUnder(tree(), 1, 1)).toBe(true);
    expect(isUnder(tree(), 1, 3)).toBe(false);
    expect(isUnder(tree(), 6, 1)).toBe(false);
  });

  it("lists only the rows a person can see: closed groups hide their members", () => {
    const rows = visibleRows(tree());
    expect(rows.map((r) => r.b.id)).toEqual([1, 2, 3, 4, 5, 7]);
    expect(rows.map((r) => r.depth)).toEqual([0, 1, 2, 1, 0, 0]);
  });
});

describe("small helpers", () => {
  it("makes a one-line name from selected text", () => {
    expect(titleFromText("  Two\n  lines   here ")).toBe("Two lines here");
    expect(titleFromText("x".repeat(300), 20)).toHaveLength(20);
    expect(titleFromText("x".repeat(300), 20).endsWith("…")).toBe(true);
    expect(titleFromText("   ")).toBe("");
  });

  it("round-trips colours and refuses malformed ones", () => {
    expect(rgbToHex([1, 0.5, 0])).toBe("#ff8000");
    expect(rgbToHex([2, -1, 0])).toBe("#ff0000");
    expect(hexToRgb("#2e4b8f")?.map((v) => Math.round(v * 255))).toEqual([0x2e, 0x4b, 0x8f]);
    expect(hexToRgb("blue")).toBeNull();
  });

  it("normalises a dragged rectangle and drops one too small to be a link", () => {
    expect(dragRect([50, 80], [10, 20], 4)).toEqual([10, 20, 50, 80]);
    expect(dragRect([10, 10], [12, 60], 4)).toBeNull();
    expect(dragRect([10, 10], [10, 10], 0)).toEqual([10, 10, 10, 10]);
  });

  it("labels a target in a few words", () => {
    expect(targetLabel({ kind: "page", page: 13, at: null })).toBe("Page 14");
    expect(targetLabel({ kind: "uri", uri: "https://example.com/" })).toBe("https://example.com/");
    expect(targetLabel({ kind: "file", name: "annex.pdf", page: 2 })).toBe("annex.pdf, page 3");
    expect(targetLabel({ kind: "file", name: "annex.pdf", page: null })).toBe("annex.pdf");
    expect(targetLabel({ kind: "other", what: "JavaScript" })).toBe("Action: JavaScript");
  });
});

const ok = <T,>(value: T) => ({ kind: "ok" as const, value });
const edited = (id: number | null, count = 1) => ({ state: {} as never, id, count });

describe("bookmark actions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNav.setState({ docs: {}, linkTool: "select" });
    vi.mocked(api.bookmarksList).mockResolvedValue(listed(tree()));
  });

  it("adds after the selected bookmark and starts a rename in place", async () => {
    vi.mocked(editBookmarks).mockResolvedValue(ok(edited(40)));
    await useNav.getState().loadBookmarks(1);
    useNav.getState().selectBookmark(1, 2);
    await useNav.getState().addBookmark(1);
    expect(editBookmarks).toHaveBeenCalledWith(1, 0, expect.objectContaining({ op: "add", parent: 1, index: 1, title: "Untitled" }));
    const d = useNav.getState().docs[1];
    expect(d.selectedBm).toBe(40);
    expect(d.renaming).toBe(40);
  });

  it("adds at the top level when nothing is selected", async () => {
    vi.mocked(editBookmarks).mockResolvedValue(ok(edited(41)));
    await useNav.getState().addBookmark(1);
    expect(editBookmarks).toHaveBeenCalledWith(1, 0, expect.objectContaining({ op: "add", parent: null, index: null }));
  });

  it("renames with a trimmed name and does nothing when it did not change", async () => {
    vi.mocked(editBookmarks).mockResolvedValue(ok(edited(7)));
    await useNav.getState().loadBookmarks(1);
    await useNav.getState().rename(1, 7, "  Chapter 3  ");
    expect(editBookmarks).not.toHaveBeenCalled();
    await useNav.getState().rename(1, 7, "  Outlook ");
    expect(editBookmarks).toHaveBeenCalledWith(1, 0, { op: "rename", id: 7, title: "Outlook" });
    await useNav.getState().rename(1, 7, "   ");
    expect(editBookmarks).toHaveBeenCalledTimes(1);
  });

  it("moves with the index the engine expects, and refuses a move into itself", async () => {
    vi.mocked(editBookmarks).mockResolvedValue(ok(edited(1)));
    await useNav.getState().loadBookmarks(1);
    // Chapter 1 (index 0) to after Chapter 2 (insertion index 2): the engine counts without Chapter 1.
    await useNav.getState().moveBookmark(1, 1, null, 2);
    expect(editBookmarks).toHaveBeenLastCalledWith(1, 0, { op: "move", id: 1, parent: null, index: 1 });
    // Moving up keeps the index.
    await useNav.getState().moveBookmark(1, 7, null, 0);
    expect(editBookmarks).toHaveBeenLastCalledWith(1, 0, { op: "move", id: 7, parent: null, index: 0 });
    // Into itself or its own child.
    vi.mocked(editBookmarks).mockClear();
    await useNav.getState().moveBookmark(1, 1, 1, 0);
    await useNav.getState().moveBookmark(1, 1, 3, 0);
    // To where it already is.
    await useNav.getState().moveBookmark(1, 5, null, 1);
    await useNav.getState().moveBookmark(1, 5, null, 2);
    expect(editBookmarks).not.toHaveBeenCalled();
  });

  it("says so, and lists again, when the document changed under the list", async () => {
    vi.mocked(editBookmarks).mockResolvedValue({ kind: "stale" });
    await useNav.getState().loadBookmarks(1);
    vi.mocked(api.bookmarksList).mockClear();
    await useNav.getState().rename(1, 7, "Outlook");
    expect(api.bookmarksList).toHaveBeenCalledTimes(1);
    expect(useNav.getState().docs[1].notice).toMatch(/changed/);
  });

  it("clears the selection of a bookmark that is gone after the list is read again", async () => {
    await useNav.getState().loadBookmarks(1);
    useNav.getState().selectBookmark(1, 6);
    vi.mocked(api.bookmarksList).mockResolvedValue(listed([bm(7, "Chapter 3")], 1));
    vi.mocked(editBookmarks).mockResolvedValue(ok(edited(null, 3)));
    await useNav.getState().remove(1, [5]);
    expect(useNav.getState().docs[1].selectedBm).toBeNull();
    expect(useNav.getState().docs[1].notice).toMatch(/3 bookmarks deleted/);
  });

  it("makes bookmarks from headings and says how many", async () => {
    vi.mocked(editBookmarks).mockResolvedValue(ok(edited(null, 9)));
    await useNav.getState().loadBookmarks(1);
    await useNav.getState().makeFromHeadings(1, true);
    expect(editBookmarks).toHaveBeenCalledWith(1, 0, { op: "fromHeadings", replace: true });
    expect(useNav.getState().docs[1].notice).toBe("9 bookmarks made from the headings.");
    expect(useNav.getState().docs[1].headings).toBeNull();
  });

  it("sends style changes as the engine takes them", async () => {
    vi.mocked(editBookmarks).mockResolvedValue(ok(edited(7)));
    await useNav.getState().loadBookmarks(1);
    await useNav.getState().setStyle(1, 7, { bold: true });
    expect(editBookmarks).toHaveBeenLastCalledWith(1, 0, { op: "setStyle", id: 7, bold: true });
    await useNav.getState().setStyle(1, 7, { color: null });
    expect(editBookmarks).toHaveBeenLastCalledWith(1, 0, { op: "setStyle", id: 7, resetColor: true });
    await useNav.getState().setStyle(1, 7, { color: [1, 0, 0] });
    expect(editBookmarks).toHaveBeenLastCalledWith(1, 0, { op: "setStyle", id: 7, color: [1, 0, 0] });
  });
});

describe("link actions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNav.setState({ docs: {}, linkTool: "select" });
    vi.mocked(api.linksList).mockResolvedValue({ rev: 0, links: [] });
  });

  it("adds a link, selects it and drops the pending rectangle", async () => {
    vi.mocked(editLinks).mockResolvedValue(ok(edited(88)));
    useNav.getState().setPending(1, { page: 2, rect: [10, 10, 100, 40] });
    const id = await useNav.getState().addLink(1, 2, [10, 10, 100, 40], { kind: "uri", uri: "https://example.com/" });
    expect(id).toBe(88);
    expect(editLinks).toHaveBeenCalledWith(1, 0, expect.objectContaining({ op: "add", page: 2, rect: [10, 10, 100, 40] }));
    const d = useNav.getState().docs[1];
    expect(d.selectedLink).toEqual({ page: 2, id: 88 });
    expect(d.pending).toBeNull();
  });

  it("keeps the rectangle when the engine refuses the target", async () => {
    vi.mocked(editLinks).mockResolvedValue({ kind: "error", message: "That address can't be linked." });
    useNav.getState().setPending(1, { page: 0, rect: [0, 0, 50, 20] });
    const id = await useNav.getState().addLink(1, 0, [0, 0, 50, 20], { kind: "uri", uri: "javascript:alert(1)" });
    expect(id).toBeNull();
    expect(useNav.getState().docs[1].pending).not.toBeNull();
  });

  it("selecting a link clears a pending rectangle and the other way round", () => {
    useNav.getState().setPending(1, { page: 0, rect: [0, 0, 50, 20] });
    useNav.getState().selectLink(1, { page: 0, id: 5 });
    expect(useNav.getState().docs[1].pending).toBeNull();
    useNav.getState().setPending(1, { page: 0, rect: [0, 0, 50, 20] });
    expect(useNav.getState().docs[1].selectedLink).toBeNull();
  });

  it("makes links from web addresses in one call", async () => {
    vi.mocked(editLinks).mockResolvedValue(ok(edited(null, 3)));
    await useNav.getState().makeFromUrls(1);
    expect(editLinks).toHaveBeenCalledWith(1, 0, { op: "fromUrls" });
    expect(useNav.getState().docs[1].notice).toBe("3 links made from web addresses.");
  });

  it("lists the page's links again when the document moved on", async () => {
    await useNav.getState().loadLinks(1, 3);
    vi.mocked(api.linksList).mockClear();
    useNav.getState().onRev(1);
    expect(api.linksList).toHaveBeenCalledWith(1, 3);
  });
});

describe("opening an address", () => {
  beforeEach(() => vi.clearAllMocks());

  it("opens only after the person says Open", async () => {
    vi.mocked(api.linkCheckUrl).mockResolvedValue("https://example.com/a");
    vi.mocked(message).mockResolvedValue("Cancel");
    expect(await confirmOpen("https://example.com/a")).toBe(false);
    expect(api.linkOpenUrl).not.toHaveBeenCalled();
    vi.mocked(message).mockResolvedValue("Ok");
    expect(await confirmOpen("https://example.com/a")).toBe(true);
    expect(api.linkOpenUrl).toHaveBeenCalledWith("https://example.com/a");
  });

  it("never opens an address the gate refused", async () => {
    vi.mocked(api.linkCheckUrl).mockRejectedValue("That address can't be opened.");
    expect(await confirmOpen("file:///C:/x")).toBe(false);
    expect(api.linkOpenUrl).not.toHaveBeenCalled();
    expect(message).toHaveBeenCalledTimes(1);
  });
});
