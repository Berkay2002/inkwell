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
let buildSend: Send = () => {};
const catalogSearchCall = vi.fn();
const catalogCancel = vi.fn(() => Promise.resolve());
const catalogDelete = vi.fn(() => Promise.resolve());
let buildDone: () => Promise<unknown> = () => Promise.resolve({});
const meta = (id: string, name: string, built = 1) => ({ id, name, folder: `C:/Docs/${name}`, recursive: true, built, files: 3 });
let catalogs = [meta("c1", "Housing"), meta("c2", "Fresh", 0)];

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
  catalogList: vi.fn(() => Promise.resolve(catalogs)),
  catalogStale: vi.fn((id: string) => Promise.resolve({ added: id === "c1" ? 2 : 0, changed: id === "c1" ? 1 : 0, removed: 0, unusable: id === "c2" })),
  catalogEstimate: vi.fn(() => Promise.resolve({ files: 1800, bytes: 1e9, secs: 120, indexBytes: 40 * 1024 * 1024, capped: false })),
  catalogCreate: vi.fn((_f: string, name: string) => Promise.resolve(meta("c3", name, 0))),
  catalogDelete,
  catalogCancel,
  catalogBuild: (id: string, full: boolean, onEvent: Send) => {
    buildSend = onEvent;
    onEvent({ kind: "started", job: 9 });
    return buildDone().then(() => ({ files: 3, added: 3, changed: 0, removed: 0, damaged: 0, password: 0, cancelled: false, id, full }));
  },
  catalogSearch: (id: string, q: string, opts: unknown, onEvent: Send) => {
    catalogSearchCall(id, q, opts);
    filesSend = onEvent;
    onEvent({ kind: "started", search: 7 });
    return Promise.resolve(7);
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

describe("search in a catalog", () => {
  const reset = async () => {
    const { useSearch } = await setup();
    useSearch.setState({ target: null, run: null, build: null, draft: null, catalogs: [], stale: {} });
    return useSearch;
  };

  it("lists the catalogs with what a refresh would change", async () => {
    const useSearch = await reset();
    await useSearch.getState().loadCatalogs();
    expect(useSearch.getState().catalogs.map((c) => c.name)).toEqual(["Housing", "Fresh"]);
    expect(useSearch.getState().stale.c1).toMatchObject({ added: 2, changed: 1 });
    expect(useSearch.getState().stale.c2.unusable).toBe(true);
  });

  it("searches the selected catalog with the folder search's events and opens a hit by its search id", async () => {
    const useSearch = await reset();
    await useSearch.getState().loadCatalogs();
    useSearch.getState().selectCatalog("c1");
    expect(useSearch.getState().target).toEqual({ kind: "catalog", id: "c1", name: "Housing" });
    await useSearch.getState().startFolder(" notice ");
    expect(catalogSearchCall).toHaveBeenCalledWith("c1", "notice", expect.anything());
    expect(searchFilesCall).not.toHaveBeenCalled();
    filesSend({ kind: "file", search: 7, index: 0, path: String.raw`C:\Docs\Housing\A.pdf`, name: "A.pdf", pageCount: 4, hits: 1, pages: 1, truncated: false, items: [{ ...found("notice", 30), page: 2 }] });
    filesSend({ kind: "done", search: 7, files: 3, matched: 1, hits: 1, skipped: 0, cloudOnly: 0, cancelled: false, capped: false });
    expect(useSearch.getState().run).toMatchObject({ running: false, matched: 1, scanned: 3 });
    await useSearch.getState().openAt(useSearch.getState().run!.files[0], 0);
    expect(searchFileToken).toHaveBeenCalledWith(7, 0);
  });

  it("shows the estimate for a picked folder, and a new catalog is created, selected and built with progress", async () => {
    const useSearch = await reset();
    await useSearch.getState().newCatalog();
    expect(useSearch.getState().draft).toMatchObject({ token: "folder-token", name: "Housing", recursive: true });
    expect(useSearch.getState().draft!.estimate!.files).toBe(1800);
    useSearch.getState().patchDraft({ name: "Rentals" });
    let finish = () => {};
    buildDone = () => new Promise<void>((r) => (finish = r));
    try {
      const made = useSearch.getState().createCatalog();
      await tick();
      expect(useSearch.getState().draft).toBeNull();
      expect(useSearch.getState().target).toEqual({ kind: "catalog", id: "c3", name: "Rentals" });
      buildSend({ kind: "progress", job: 9, done: 2, total: 3 });
      expect(useSearch.getState().build).toMatchObject({ id: "c3", job: 9, done: 2, total: 3, running: true });
      useSearch.getState().cancelBuild();
      expect(catalogCancel).toHaveBeenCalledWith(9);
      finish();
      await made;
    } finally {
      buildDone = () => Promise.resolve({});
    }
    expect(useSearch.getState().build).toMatchObject({ running: false, line: "3 files, 3 new" });
  });

  it("re-reads the estimate when sub-folders are switched, and a cancelled sheet leaves nothing behind", async () => {
    const useSearch = await reset();
    await useSearch.getState().newCatalog();
    useSearch.getState().patchDraft({ recursive: false });
    expect(useSearch.getState().draft).toMatchObject({ recursive: false, estimate: null });
    await tick();
    expect(useSearch.getState().draft!.estimate).not.toBeNull();
    useSearch.getState().dropDraft();
    expect(useSearch.getState().draft).toBeNull();
  });

  it("deleting the selected catalog clears the target and its results", async () => {
    const useSearch = await reset();
    await useSearch.getState().loadCatalogs();
    useSearch.getState().selectCatalog("c1");
    await useSearch.getState().removeCatalog("c1");
    expect(catalogDelete).toHaveBeenCalledWith("c1");
    expect(useSearch.getState()).toMatchObject({ target: null, run: null });
    expect(useSearch.getState().catalogs.map((c) => c.id)).toEqual(["c2"]);
  });
});
