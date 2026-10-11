import { beforeEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  const m = new Map<string, string>();
  (globalThis as { localStorage?: unknown }).localStorage = {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, String(v)),
    removeItem: (k: string) => void m.delete(k),
  };
  (globalThis as { requestAnimationFrame?: unknown }).requestAnimationFrame = (f: (t: number) => void) => {
    f(0);
    return 0;
  };
});

type Send = (e: unknown) => void;
let docSend: Send = () => {};
let filesSend: Send = () => {};
let next = 0;
const searchCall = vi.fn();
const searchFilesCall = vi.fn();
const cancelSearch = vi.fn(() => Promise.resolve());
const cancelSearchFiles = vi.fn(() => Promise.resolve());
const searchFileToken = vi.fn((_search: number, index: number) => Promise.resolve({ token: `tok-${index}`, path: "" }));
/** What `searchFiles` answers with: the id at once, or never (the folder is still being listed). */
let filesAnswer: () => Promise<number> = () => Promise.resolve(7);

vi.mock("./api", () => ({
  openDocument: vi.fn(() =>
    Promise.resolve({ status: "opened", doc: ++next, path: String.raw`C:\Docs\A.pdf`, title: null, pageCount: 30, firstPage: [600, 800] }),
  ),
  closeDocument: vi.fn(() => Promise.resolve()),
  knownFile: vi.fn((path: string) => Promise.resolve({ token: "t", path })),
  pageSizes: vi.fn(() => Promise.resolve()),
  cancelSearch,
  cancelSearchFiles,
  searchFileToken,
  search: (doc: number, q: string, opts: unknown, onEvent: Send) => {
    searchCall(doc, q, opts);
    docSend = onEvent;
    return Promise.resolve(41);
  },
  searchFiles: (scope: unknown, q: string, opts: unknown, onEvent: Send) => {
    searchFilesCall(scope, q, opts);
    filesSend = onEvent;
    // The engine says "started" before it lists anything.
    onEvent({ kind: "started", search: 7 });
    return filesAnswer();
  },
  pickOpen: vi.fn(() => Promise.resolve([{ token: "f1", path: String.raw`C:\Docs\A.pdf` }, { token: "f2", path: String.raw`C:\Docs\B.pdf` }])),
  pickSearchFolder: vi.fn(() => Promise.resolve({ token: "folder-token", path: String.raw`C:\Docs\Housing` })),
}));
vi.mock("../recents", () => ({ useRecents: { getState: () => ({ record: () => Promise.resolve() }) } }));

const found = (text: string, y: number) => ({ rects: [[10, y, 60, y + 10]], before: "the ", text, after: " here", source: "text" });
const tick = () => new Promise((r) => setTimeout(r, 0));

async function setup() {
  const { useStore } = await import("../store");
  const { useFind } = await import("./findStore");
  const { useSearch } = await import("./searchStore");
  return { useStore, useFind, useSearch };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("search in a document", () => {
  it("runs with the saved options and lists hits in reading order with the first one current", async () => {
    const { useStore, useFind } = await setup();
    await useStore.getState().openPath(String.raw`C:\Docs\A.pdf`);
    const id = useStore.getState().active!;
    useFind.getState().setOpt("caseSensitive", true);
    useStore.getState().runSearch(id, "notice");
    await tick();
    expect(searchCall).toHaveBeenCalledWith(id, "notice", expect.objectContaining({ caseSensitive: true, wholeWords: false }));

    docSend({ kind: "hits", search: 41, page: 1, hits: [found("notice", 20)] });
    docSend({ kind: "hits", search: 41, page: 3, hits: [found("notice", 200), found("notice", 50)] });
    docSend({ kind: "progress", search: 41, scanned: 16, total: 30 });
    let s = useStore.getState().docs[id].search;
    expect(s.hits.map((h) => [h.page, h.rects[0][1]])).toEqual([[1, 20], [3, 50], [3, 200]]);
    expect(s.current).toBe(0);
    expect([s.scanned, s.total, s.done]).toEqual([16, 30, false]);

    docSend({ kind: "done", search: 41, hits: 3, pages: 30 });
    s = useStore.getState().docs[id].search;
    expect(s.done).toBe(true);
    expect(s.scanned).toBe(30);
  });

  it("keeps the current hit while later pages report, and stepping wraps", async () => {
    const { useStore } = await setup();
    await useStore.getState().openPath(String.raw`C:\Docs\B.pdf`);
    const id = useStore.getState().active!;
    useStore.getState().runSearch(id, "x");
    await tick();
    docSend({ kind: "hits", search: 41, page: 5, hits: [found("x", 10)] });
    expect(useStore.getState().docs[id].search.current).toBe(0);
    docSend({ kind: "hits", search: 41, page: 2, hits: [found("x", 10)] });
    const s = useStore.getState().docs[id].search;
    // the page 5 hit moved to index 1 and is still the current one
    expect(s.hits[s.current].page).toBe(5);
    useStore.getState().stepHit(id, 1);
    expect(useStore.getState().docs[id].search.current).toBe(0);
  });

  it("keeps whether the engine stopped at its hit limit", async () => {
    const { useStore } = await setup();
    await useStore.getState().openPath(String.raw`C:\Docs\D.pdf`);
    const id = useStore.getState().active!;
    useStore.getState().runSearch(id, "x");
    await tick();
    docSend({ kind: "hits", search: 41, page: 0, hits: [found("x", 10)] });
    docSend({ kind: "done", search: 41, hits: 1, pages: 30, capped: true });
    expect(useStore.getState().docs[id].search).toMatchObject({ done: true, capped: true });
  });

  it("stop ends the search, keeps what was found and ignores late events", async () => {
    const { useStore } = await setup();
    await useStore.getState().openPath(String.raw`C:\Docs\C.pdf`);
    const id = useStore.getState().active!;
    useStore.getState().runSearch(id, "x");
    await tick();
    docSend({ kind: "hits", search: 41, page: 0, hits: [found("x", 10)] });
    useStore.getState().stopSearch(id);
    expect(cancelSearch).toHaveBeenCalledWith(id);
    const s = useStore.getState().docs[id].search;
    expect([s.done, s.hits.length]).toEqual([true, 1]);
  });
});

describe("search across files", () => {
  const fileEvent = (index: number, name: string, hits = 2) => ({
    kind: "file", search: 7, index, path: `C:\\Docs\\Housing\\${name}`, name, pageCount: 4, hits, pages: 1, truncated: false,
    items: [{ ...found("notice", 30), page: 2 }],
  });

  it("collects files in listing order, with progress, and remembers the query", async () => {
    const { useSearch, useFind } = await setup();
    useFind.getState().clearHistory();
    await useSearch.getState().pickFolder();
    expect(useSearch.getState().target).toEqual({ kind: "folder", token: "folder-token", path: String.raw`C:\Docs\Housing` });
    await useSearch.getState().startFolder("  notice ");
    expect(searchFilesCall).toHaveBeenCalledWith({ kind: "folder", token: "folder-token", recursive: true }, "notice", expect.anything());
    filesSend(fileEvent(2, "c.pdf"));
    filesSend(fileEvent(0, "a.pdf", 5));
    filesSend({ kind: "progress", search: 7, scanned: 3, total: 4 });
    expect(useSearch.getState().run!.files.map((f) => f.name)).toEqual(["a.pdf", "c.pdf"]);
    expect([useSearch.getState().run!.scanned, useSearch.getState().run!.total]).toEqual([3, 4]);
    filesSend({ kind: "done", search: 7, files: 4, matched: 2, hits: 7, skipped: 1, cloudOnly: 3, cancelled: false, capped: false });
    const r = useSearch.getState().run!;
    expect([r.running, r.matched, r.hits, r.skipped, r.cloudOnly, r.scanned]).toEqual([false, 2, 7, 1, 3, 4]);
    expect(useFind.getState().history[0]).toBe("notice");
  });

  it("searches a list of picked files by their tokens", async () => {
    const { useSearch } = await setup();
    expect(await useSearch.getState().pickFiles()).toBe(true);
    expect(useSearch.getState().target).toEqual({ kind: "files", tokens: ["f1", "f2"], paths: [String.raw`C:\Docs\A.pdf`, String.raw`C:\Docs\B.pdf`] });
    await useSearch.getState().startFolder("notice");
    expect(searchFilesCall).toHaveBeenCalledWith({ kind: "files", tokens: ["f1", "f2"] }, "notice", expect.anything());
  });

  it("ignores events of an older search once a new one has started", async () => {
    const { useSearch } = await setup();
    await useSearch.getState().pickFolder();
    await useSearch.getState().startFolder("one");
    const first = filesSend;
    await useSearch.getState().startFolder("two");
    expect(cancelSearchFiles).toHaveBeenCalledWith(7);
    first(fileEvent(0, "stale.pdf"));
    expect(useSearch.getState().run!.files).toHaveLength(0);
    expect(useSearch.getState().run!.query).toBe("two");
  });

  it("can be cancelled while the folder is still being listed", async () => {
    const { useSearch } = await setup();
    await useSearch.getState().pickFolder();
    filesAnswer = () => new Promise<number>(() => {});
    try {
      void useSearch.getState().startFolder("notice");
      await tick();
      // The command has not answered, but "started" gave the id.
      expect(useSearch.getState().run!.id).toBe(7);
      useSearch.getState().cancelFolder();
      expect(cancelSearchFiles).toHaveBeenCalledWith(7);
    } finally {
      filesAnswer = () => Promise.resolve(7);
    }
  });

  it("cancel asks the engine to stop and says so at once", async () => {
    const { useSearch } = await setup();
    await useSearch.getState().pickFolder();
    await useSearch.getState().startFolder("notice");
    useSearch.getState().cancelFolder();
    expect(cancelSearchFiles).toHaveBeenCalledWith(7);
    expect(useSearch.getState().run).toMatchObject({ running: false, cancelled: true });
  });

  it("opens a file at its hit: the tab, the query in the find box, that hit current", async () => {
    const { useSearch, useStore, useFind } = await setup();
    await useSearch.getState().pickFolder();
    await useSearch.getState().startFolder("notice");
    filesSend({ ...fileEvent(0, "A.pdf"), path: String.raw`C:\Docs\A.pdf` });
    const g = useSearch.getState().run!.files[0];
    await useSearch.getState().openAt(g, 0);
    // the open token is asked for now, for this one file
    expect(searchFileToken).toHaveBeenCalledWith(7, 0);
    const id = useStore.getState().active!;
    expect(useStore.getState().docs[id].path).toBe(String.raw`C:\Docs\A.pdf`);
    expect(useFind.getState().drafts[id]).toBe("notice");
    await tick();
    docSend({ kind: "hits", search: 41, page: 0, hits: [found("notice", 5)] });
    docSend({ kind: "hits", search: 41, page: 2, hits: [found("notice", 400), found("notice", 30)] });
    const s = useStore.getState().docs[id].search;
    // the hit on page 3 at y 30 was asked for, not the first one in the document
    expect(s.hits[s.current]).toMatchObject({ page: 2 });
    expect(s.hits[s.current].rects[0][1]).toBe(30);
  });
});
