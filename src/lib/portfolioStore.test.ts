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

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn(), open: vi.fn(), message: vi.fn(), ask: vi.fn() }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({}) }));
vi.mock("./api", () => ({
  portfolioInfo: vi.fn(),
  portfolioThumb: vi.fn(),
  portfolioExtractAll: vi.fn(),
  portfolioCreate: vi.fn(),
  pickPortfolioFiles: vi.fn(),
  pickFolder: vi.fn(),
  pickSave: vi.fn(),
  attachmentOpen: vi.fn(),
  attachmentSave: vi.fn(),
}));

import { ask } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import * as api from "./api";
import type { PortfolioFile, PortfolioInfo } from "./api";
import {
  crumbs,
  defaultLayout,
  extBadge,
  fileIcon,
  gridMove,
  nextSort,
  openAction,
  paneFields,
  rowKey,
  rows,
  sortFiles,
  tileField,
  tilesAt,
  typeLabel,
} from "./portfolioLogic";
import { activate, createPortfolio, extractAll, usePortfolio } from "./portfolioStore";
import { dropThumbs, requestThumb, THUMB_MAX_BYTES, type ThumbResult, thumbable, thumbNote } from "./portfolioThumb";

const file = (name: string, o: Partial<PortfolioFile> = {}): PortfolioFile => ({
  id: `n:${name}`,
  name,
  description: "",
  size: 10,
  mime: null,
  isPdf: name.endsWith(".pdf"),
  hasData: true,
  folder: null,
  cells: [name, ""],
  ranks: [0, null],
  ...o,
});

/** Name, Pages; the author's order is Pages descending. */
const info = (o: Partial<PortfolioInfo> = {}): PortfolioInfo => ({
  fields: [
    { key: "Name", name: "Name", subtype: "F", kind: "text", visible: true },
    { key: "Pages", name: "Pages", subtype: "N", kind: "number", visible: true },
  ],
  sort: [{ field: 1, ascending: false }],
  view: "details",
  initial: null,
  folders: [
    { id: 1, name: "Reports", parent: null },
    { id: 2, name: "2026", parent: 1 },
  ],
  files: [
    file("c.pdf", { ranks: [2, 2], folder: 2 }),
    file("a.pdf", { ranks: [0, 1], folder: 1 }),
    file("b.txt", { ranks: [1, 0] }),
    file("z.pdf", { ranks: [3, null] }),
  ],
  ...o,
});

const DOC = 7;
beforeEach(() => {
  vi.clearAllMocks();
  useStore.setState({ docs: { [DOC]: { id: DOC } as never }, error: null } as never);
  usePortfolio.setState({ docs: {} });
});

describe("portfolio order", () => {
  it("a click on a header sorts by it, a second click flips it", () => {
    expect(nextSort([{ field: 1, ascending: false }], 0)).toEqual([{ field: 0, ascending: true }]);
    expect(nextSort([{ field: 0, ascending: true }], 0)).toEqual([{ field: 0, ascending: false }]);
    expect(nextSort([], 1)).toEqual([{ field: 1, ascending: true }]);
  });

  it("sorts by the engine's ranks, a missing value last whichever way, ties in the engine's order", () => {
    const f = info().files;
    expect(sortFiles(f, [{ field: 1, ascending: true }]).map((x) => x.name)).toEqual(["b.txt", "a.pdf", "c.pdf", "z.pdf"]);
    expect(sortFiles(f, [{ field: 1, ascending: false }]).map((x) => x.name)).toEqual(["c.pdf", "a.pdf", "b.txt", "z.pdf"]);
    const tied = [file("x", { ranks: [1, 1] }), file("y", { ranks: [0, 1] })];
    expect(sortFiles(tied, [{ field: 1, ascending: true }]).map((x) => x.name)).toEqual(["x", "y"]);
    expect(sortFiles(f, [])).toBe(f);
  });

  it("the store keeps the reader's sort apart from the collection's", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info());
    await usePortfolio.getState().load(DOC);
    const s = () => usePortfolio.getState().docs[DOC];
    expect(s().sort).toBeNull();
    usePortfolio.getState().toggleSort(DOC, 0);
    expect(s().sort).toEqual([{ field: 0, ascending: true }]);
    usePortfolio.getState().toggleSort(DOC, 0);
    expect(s().sort).toEqual([{ field: 0, ascending: false }]);
    usePortfolio.getState().resetSort(DOC);
    expect(s().sort).toBeNull();
    // the first click on the collection's own column flips that one
    usePortfolio.getState().toggleSort(DOC, 1);
    expect(s().sort).toEqual([{ field: 1, ascending: true }]);
  });
});

describe("reads that finish out of order", () => {
  it("an older answer arriving after a newer one is dropped", async () => {
    const answers: Array<(i: PortfolioInfo) => void> = [];
    vi.mocked(api.portfolioInfo).mockImplementation(() => new Promise<PortfolioInfo>((r) => answers.push(r)));
    const first = usePortfolio.getState().load(DOC);
    const second = usePortfolio.getState().load(DOC);
    // the newer read finishes first, the older one afterwards with what was true before the edit
    answers[1](info({ files: [file("new.pdf")] }));
    await second;
    answers[0](info({ files: [file("old.pdf")] }));
    await first;
    expect(usePortfolio.getState().docs[DOC].info?.files.map((f) => f.name)).toEqual(["new.pdf"]);
  });
});

describe("folders", () => {
  it("expand and collapse show the contents under the folder", () => {
    const i = info();
    const names = (open: number[]) => rows(i, i.sort, new Set(open)).map((r) => (r.kind === "folder" ? `[${r.folder.name}]` : r.file.name));
    expect(names([])).toEqual(["[Reports]", "b.txt", "z.pdf"]);
    expect(names([1])).toEqual(["[Reports]", "[2026]", "a.pdf", "b.txt", "z.pdf"]);
    expect(names([1, 2])).toEqual(["[Reports]", "[2026]", "c.pdf", "a.pdf", "b.txt", "z.pdf"]);
    const r = rows(i, i.sort, new Set([1]));
    expect(r[0]).toMatchObject({ kind: "folder", open: true, count: 2, depth: 0 });
    expect(r[1]).toMatchObject({ kind: "folder", open: false, count: 1, depth: 1 });
  });

  it("the store toggles a folder and keeps it across a reload", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info());
    await usePortfolio.getState().load(DOC);
    usePortfolio.getState().toggleFolder(DOC, 1);
    expect(usePortfolio.getState().docs[DOC].open).toEqual([1]);
    usePortfolio.getState().select(DOC, "n:a.pdf");
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC]).toMatchObject({ open: [1], selected: "n:a.pdf" });
    usePortfolio.getState().toggleFolder(DOC, 1);
    expect(usePortfolio.getState().docs[DOC].open).toEqual([]);
  });

  it("the first look selects the initial document with its folders open; a folder row can be selected too", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info({ initial: "n:c.pdf" }));
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC]).toMatchObject({ selected: "n:c.pdf", open: [2, 1] });
    usePortfolio.getState().select(DOC, rowKey({ kind: "folder", folder: info().folders[0], depth: 0, open: false, count: 0 }));
    expect(usePortfolio.getState().docs[DOC].selected).toBe("d:1");
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].selected).toBe("d:1");
  });
});

describe("open or extract", () => {
  it("a PDF opens, everything else is extracted", () => {
    expect(openAction(file("a.pdf"))).toBe("open");
    expect(openAction(file("a.exe"))).toBe("extract");
    expect(openAction(file("a.pdf", { hasData: false }))).toBe("extract");
    expect(typeLabel(file("run.EXE", { mime: "application/octet-stream" }))).toBe("EXE");
    expect(typeLabel(file("note", { mime: "text/plain" }))).toBe("text/plain");
    expect(typeLabel(file("a.csv", { mime: "text/csv" }))).toBe("CSV (text/csv)");
  });

  it("a PDF goes to a tab; another type asks first, naming it, and is saved only to a chosen file", async () => {
    const openPath = vi.fn(() => Promise.resolve());
    useStore.setState({ openPath } as never);
    vi.mocked(api.attachmentOpen).mockResolvedValue({ token: "t", path: "C:\\tmp\\a.pdf" });
    await activate(DOC, file("a.pdf"));
    expect(api.attachmentOpen).toHaveBeenCalledWith(DOC, "n:a.pdf");
    expect(openPath).toHaveBeenCalledWith("C:\\tmp\\a.pdf", "t", true);

    vi.mocked(ask).mockResolvedValue(false);
    await activate(DOC, file("setup.exe", { mime: "application/octet-stream" }));
    expect(vi.mocked(ask).mock.calls[0][0]).toContain("setup.exe");
    expect(vi.mocked(ask).mock.calls[0][0]).toContain("Type: EXE");
    expect(api.attachmentOpen).toHaveBeenCalledTimes(1);
    expect(api.pickSave).not.toHaveBeenCalled();

    vi.mocked(ask).mockResolvedValue(true);
    vi.mocked(api.pickSave).mockResolvedValue({ token: "s", path: "C:\\out\\setup.exe" });
    vi.mocked(api.attachmentSave).mockResolvedValue("C:\\out\\setup.exe");
    await activate(DOC, file("setup.exe"));
    expect(api.attachmentSave).toHaveBeenCalledWith(DOC, "n:setup.exe", "s");
    expect(api.attachmentOpen).toHaveBeenCalledTimes(1);
    expect(openPath).toHaveBeenCalledTimes(1);
  });

  it("Extract all asks for a folder and says what was written", async () => {
    vi.mocked(api.pickFolder).mockResolvedValue({ token: "f", path: "C:\\out" });
    vi.mocked(api.portfolioExtractAll).mockResolvedValue({ folder: "C:\\out\\files", written: ["a.pdf", "b (2).txt"], skipped: [] });
    await extractAll(DOC);
    expect(api.portfolioExtractAll).toHaveBeenCalledWith(DOC, "f");
    expect(useStore.getState().error).toBe("Extracted 2 files to files");
    vi.mocked(api.pickFolder).mockResolvedValue(null);
    vi.mocked(api.portfolioExtractAll).mockClear();
    await extractAll(DOC);
    expect(api.portfolioExtractAll).not.toHaveBeenCalled();
  });
});

describe("loading and creating", () => {
  it("a plain PDF has no portfolio; a refused collection is said once and stays plain", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(null);
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].info).toBeNull();
    expect(useStore.getState().error).toBeNull();

    usePortfolio.setState({ docs: {} });
    vi.mocked(api.portfolioInfo).mockRejectedValue("the portfolio's folders loop back on themselves");
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].info).toBeNull();
    expect(useStore.getState().error).toContain("loop");
    useStore.setState({ error: null } as never);
    await usePortfolio.getState().load(DOC);
    expect(useStore.getState().error).toBeNull();
  });

  it("the file list shows first, unless the author asked for no portfolio view", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info());
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].cover).toBe(false);
    usePortfolio.getState().showCover(DOC, true);
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].cover).toBe(true);
    usePortfolio.setState({ docs: {} });
    vi.mocked(api.portfolioInfo).mockResolvedValue(info({ view: "hidden" }));
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].cover).toBe(true);
  });

  it("Create PDF Portfolio picks files, then where to save, then opens the result", async () => {
    const openPath = vi.fn(() => Promise.resolve());
    useStore.setState({ openPath } as never);
    vi.mocked(api.pickPortfolioFiles).mockResolvedValue([
      { token: "a", path: "C:\\in\\a.pdf" },
      { token: "b", path: "C:\\in\\b.docx" },
    ]);
    vi.mocked(api.pickSave).mockResolvedValue({ token: "out", path: "C:\\in\\Portfolio.pdf" });
    vi.mocked(api.portfolioCreate).mockResolvedValue({ token: "new", path: "C:\\in\\Portfolio.pdf" });
    await createPortfolio();
    expect(api.pickSave).toHaveBeenCalledWith("Portfolio.pdf", { near: "a" });
    expect(api.portfolioCreate).toHaveBeenCalledWith(["a", "b"], "out");
    expect(openPath).toHaveBeenCalledWith("C:\\in\\Portfolio.pdf", "new");

    vi.mocked(api.pickPortfolioFiles).mockResolvedValue([]);
    vi.mocked(api.pickSave).mockClear();
    await createPortfolio();
    expect(api.pickSave).not.toHaveBeenCalled();
  });
});

describe("layout", () => {
  it("opens in the layout the collection asks for; hidden and custom views are Details", () => {
    expect(defaultLayout("tile")).toBe("tiles");
    expect(defaultLayout("details")).toBe("details");
    expect(defaultLayout("hidden")).toBe("details");
    expect(defaultLayout("custom")).toBe("details");
  });

  it("the store starts in that layout and keeps the person's choice through a re-read", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info({ view: "tile" }));
    await usePortfolio.getState().load(DOC);
    const s = () => usePortfolio.getState().docs[DOC];
    expect(s()).toMatchObject({ layout: "tiles", pane: false, folder: null });
    usePortfolio.getState().setLayout(DOC, "details");
    usePortfolio.getState().togglePane(DOC);
    await usePortfolio.getState().load(DOC);
    expect(s()).toMatchObject({ layout: "details", pane: true });
    // not written to the file: nothing else is asked of the engine
    expect(api.portfolioInfo).toHaveBeenCalledTimes(2);
    usePortfolio.getState().togglePane(DOC);
    expect(s().pane).toBe(false);
  });

  it("switching to Details opens the folders around the selection so it is in view", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info({ view: "tile" }));
    await usePortfolio.getState().load(DOC);
    usePortfolio.getState().select(DOC, "n:c.pdf");
    usePortfolio.getState().setLayout(DOC, "details");
    expect(usePortfolio.getState().docs[DOC].open.slice().sort()).toEqual([1, 2]);
  });

  it("tiles open in the folder of the initial document, and in the folder of the selection when switched to", async () => {
    vi.mocked(api.portfolioInfo).mockResolvedValue(info({ initial: "n:c.pdf", view: "tile" }));
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].folder).toBe(2);
    usePortfolio.getState().setLayout(DOC, "details");
    usePortfolio.getState().select(DOC, "n:b.txt");
    usePortfolio.getState().setLayout(DOC, "tiles");
    expect(usePortfolio.getState().docs[DOC].folder).toBeNull();
    usePortfolio.getState().select(DOC, "d:2");
    usePortfolio.getState().setLayout(DOC, "tiles");
    expect(usePortfolio.getState().docs[DOC].folder).toBe(1);
    // a folder that is gone after a re-read puts the tiles back at the top
    usePortfolio.getState().enterFolder(DOC, 2);
    vi.mocked(api.portfolioInfo).mockResolvedValue(info({ folders: [{ id: 1, name: "Reports", parent: null }], view: "tile" }));
    await usePortfolio.getState().load(DOC);
    expect(usePortfolio.getState().docs[DOC].folder).toBeNull();
  });

  it("a level of tiles is its folders (with the files under them counted) then its files in the sort", () => {
    const i = info();
    const label = (r: ReturnType<typeof tilesAt>[number]) => (r.kind === "folder" ? `[${r.folder.name}:${r.count}]` : r.file.name);
    expect(tilesAt(i, i.sort, null).map(label)).toEqual(["[Reports:2]", "b.txt", "z.pdf"]);
    expect(tilesAt(i, i.sort, 1).map(label)).toEqual(["[2026:1]", "a.pdf"]);
    expect(tilesAt(i, i.sort, 2).map(label)).toEqual(["c.pdf"]);
    // the reader's sort applies to the files of a level
    expect(tilesAt(i, [{ field: 1, ascending: true }], null).map(label)).toEqual(["[Reports:2]", "b.txt", "z.pdf"]);
    expect(crumbs(i, 2).map((f) => f.name)).toEqual(["Reports", "2026"]);
    expect(crumbs(i, null)).toEqual([]);
  });
});

describe("keyboard movement in the grid", () => {
  // 7 items, 3 across:  0 1 2 / 3 4 5 / 6
  it("moves by one and by a row, stops at the edges", () => {
    expect(gridMove(4, "ArrowRight", 3, 7)).toBe(5);
    expect(gridMove(4, "ArrowLeft", 3, 7)).toBe(3);
    expect(gridMove(4, "ArrowDown", 3, 7)).toBe(6);
    expect(gridMove(4, "ArrowUp", 3, 7)).toBe(1);
    expect(gridMove(1, "ArrowUp", 3, 7)).toBe(1);
    expect(gridMove(0, "ArrowLeft", 3, 7)).toBe(0);
    expect(gridMove(6, "ArrowRight", 3, 7)).toBe(6);
    expect(gridMove(6, "ArrowDown", 3, 7)).toBe(6);
  });

  it("a short last row is reached from the row above, at its end", () => {
    expect(gridMove(5, "ArrowDown", 3, 7)).toBe(6);
    expect(gridMove(3, "ArrowDown", 3, 7)).toBe(6);
    expect(gridMove(2, "ArrowDown", 3, 7)).toBe(5);
  });

  it("starts at the first item with no selection, and Home and End go to the ends", () => {
    expect(gridMove(-1, "ArrowDown", 3, 7)).toBe(0);
    expect(gridMove(-1, "ArrowRight", 3, 7)).toBe(0);
    expect(gridMove(3, "Home", 3, 7)).toBe(0);
    expect(gridMove(3, "End", 3, 7)).toBe(6);
    expect(gridMove(0, "ArrowDown", 3, 0)).toBe(-1);
    expect(gridMove(0, "ArrowDown", 0, 1)).toBe(0);
    expect(gridMove(2, "x", 3, 7)).toBe(2);
  });
});

describe("preview pane", () => {
  // the name is not the first column: the schema starts with an author
  const schema = (): PortfolioInfo =>
    info({
      fields: [
        { key: "Author", name: "Author", subtype: "S", kind: "text", visible: true },
        { key: "Name", name: "Name", subtype: "F", kind: "text", visible: true },
        { key: "Notes", name: "Notes", subtype: "S", kind: "text", visible: false },
        { key: "Pages", name: "Pages", subtype: "N", kind: "number", visible: true },
      ],
    });
  const f = file("a.pdf", { cells: ["Zed", "a.pdf", "secret", "10"] });

  it("lists every visible field with its value, in the author's order, and none that is hidden", () => {
    expect(paneFields(schema(), f)).toEqual([
      { key: "Author", label: "Author", value: "Zed", number: false },
      { key: "Name", label: "Name", value: "a.pdf", number: false },
      { key: "Pages", label: "Pages", value: "10", number: true },
    ]);
  });

  it("a tile says the first visible field that is not the file name", () => {
    expect(tileField(schema(), f)).toEqual({ label: "Author", value: "Zed" });
    const named = info(); // Name, Pages
    expect(tileField(named, file("a.pdf", { cells: ["a.pdf", "9"] }))).toEqual({ label: "Pages", value: "9" });
    expect(tileField(info({ fields: [named.fields[0]] }), file("a.pdf"))).toBeNull();
  });

  it("icons are from the manifest by type, with the extension as a badge", () => {
    expect(fileIcon({ name: "a.pdf" })).toBe("file");
    expect(fileIcon({ name: "notes.TXT" })).toBe("article");
    expect(fileIcon({ name: "t.csv" })).toBe("grid");
    expect(fileIcon({ name: "x.json" })).toBe("code");
    expect(fileIcon({ name: "noext" })).toBe("file");
    expect(extBadge({ name: "report.final.pdf" })).toBe("PDF");
    expect(extBadge({ name: "noext" })).toBe("");
  });
});

describe("thumbnails", () => {
  const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47]).buffer;
  const got = (): { r?: ThumbResult } => ({});

  it("only an embedded PDF with its bytes and under the cap is drawn; the others say why or are plain icons", () => {
    expect(thumbable(file("a.pdf"))).toBe(true);
    expect(thumbable(file("a.txt"))).toBe(false);
    expect(thumbable(file("a.pdf", { hasData: false }))).toBe(false);
    expect(thumbable(file("big.pdf", { size: THUMB_MAX_BYTES + 1 }))).toBe(false);
    expect(thumbNote(file("big.pdf", { size: THUMB_MAX_BYTES + 1 }))).toContain("Too large");
    expect(thumbNote(file("a.txt"))).toBeNull();
  });

  it("asks for at most two renders at a time, drops those no tile waits for, and remembers the answers", async () => {
    const asked: Array<{ id: string; done: (b: ArrayBuffer | Error) => void }> = [];
    vi.mocked(api.portfolioThumb).mockImplementation(
      (_d, id) =>
        new Promise<ArrayBuffer>((res, rej) => {
          asked.push({ id, done: (b) => (b instanceof Error ? rej(b) : res(b)) });
        }),
    );
    const out = ["a", "b", "c", "d"].map(() => got());
    const cancels = ["a", "b", "c", "d"].map((n, k) => requestThumb(900, `n:${n}`, 100, (r) => (out[k].r = r)));
    expect(asked.map((x) => x.id)).toEqual(["n:a", "n:b"]);
    // the tile for d scrolled away before its turn
    cancels[3]();
    asked[0].done(png);
    await vi.waitFor(() => expect(asked.map((x) => x.id)).toEqual(["n:a", "n:b", "n:c"]));
    expect(out[0].r?.url).toMatch(/^data:image\/png;base64,/);
    asked[1].done(new Error("This PDF is password protected"));
    asked[2].done(png);
    await vi.waitFor(() => expect(out[2].r?.url).toBeDefined());
    // a failed one is a note, and nothing was asked for d
    expect(out[1].r).toEqual({ note: "This PDF is password protected" });
    expect(asked).toHaveLength(3);
    // asked again, it is answered from memory; a re-read of the document draws it afresh
    const again = got();
    requestThumb(900, "n:a", 100, (r) => (again.r = r));
    expect(again.r?.url).toBeDefined();
    expect(asked).toHaveLength(3);
    dropThumbs(900);
    requestThumb(900, "n:a", 100, () => {});
    expect(asked).toHaveLength(4);
    asked[3].done(png);
    await vi.waitFor(() => expect(api.portfolioThumb).toHaveBeenCalledTimes(4));
  });

  it("a picture that was in flight when the document was read again goes nowhere", async () => {
    const asked: Array<(b: ArrayBuffer) => void> = [];
    vi.mocked(api.portfolioThumb).mockImplementation(() => new Promise<ArrayBuffer>((res) => asked.push(res)));
    const old = got();
    requestThumb(901, "n:a.pdf", 100, (r) => (old.r = r));
    expect(asked).toHaveLength(1);
    // the file is replaced by another with the same name: the document is read again
    dropThumbs(901);
    const now = got();
    requestThumb(901, "n:a.pdf", 100, (r) => (now.r = r));
    expect(asked).toHaveLength(2);
    const stale = new Uint8Array([1, 2, 3]).buffer;
    const fresh = new Uint8Array([4, 5, 6]).buffer;
    asked[0](stale);
    await Promise.resolve();
    await vi.waitFor(() => expect(old.r).toBeUndefined());
    expect(now.r).toBeUndefined();
    asked[1](fresh);
    await vi.waitFor(() => expect(now.r?.url).toBe(`data:image/png;base64,${btoa("\x04\x05\x06")}`));
    // and the remembered one is the new picture
    const again = got();
    requestThumb(901, "n:a.pdf", 100, (r) => (again.r = r));
    expect(again.r?.url).toBe(now.r?.url);
  });
});
