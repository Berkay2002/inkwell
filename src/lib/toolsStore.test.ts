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
vi.mock("./edit", () => ({ save: vi.fn(() => Promise.resolve()), undo: vi.fn(() => Promise.resolve()), applyState: vi.fn() }));
vi.mock("./api", () => ({
  compressAudit: vi.fn(),
  compressStart: vi.fn(),
  toolCancel: vi.fn(() => Promise.resolve()),
  ocrLanguages: vi.fn(),
  ocrChooseTesseract: vi.fn(),
  ocrStart: vi.fn(),
}));

import { useStore } from "../store";
import * as api from "./api";
import * as edits from "./edit";
import { formatBytes, useTools } from "./toolsStore";

const audit = (outputExists = false) => ({
  audit: { total: 8_400_000, categories: [], images: 4, fonts: 2 },
  size: 8_400_000,
  defaultOutput: "C:\\d\\a (compressed).pdf",
  outputExists,
  predictions: [
    { preset: "smallest", bytes: 900_000 },
    { preset: "balanced", bytes: 2_100_000 },
    { preset: "highQuality", bytes: 4_000_000 },
  ],
});
const report = { output: "C:\\d\\a (compressed).pdf", sizeBefore: 8_400_000, sizeAfter: 2_000_000, savedPercent: 76 };
const flush = () => new Promise((r) => setTimeout(r, 0));

function doc(dirty = false) {
  useStore.setState({
    docs: { 1: { id: 1, path: "C:\\d\\a.pdf", name: "a.pdf", dirty, currentPage: 2, pageCount: 12 } as never },
    order: [1],
    active: 1,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  useTools.setState({ sheet: null, settings: { ocrLanguage: null, ocrBackend: "windows", compressPreset: "balanced" } });
  doc();
});

describe("compress sheet", () => {
  it("loads the audit with predictions, runs, shows progress and the result", async () => {
    vi.mocked(api.compressAudit).mockResolvedValue(audit() as never);
    useTools.getState().open("compress");
    await flush();
    expect(useTools.getState().sheet?.audit?.predictions).toHaveLength(3);

    let report$!: (v: unknown) => void;
    vi.mocked(api.compressStart).mockImplementation((_j, _r, onP) => {
      onP({ stage: "images", fraction: 0.62, page: 8, pages: 12 });
      return new Promise((r) => (report$ = r)) as never;
    });
    useTools.getState().set({ preset: "smallest" });
    const run = useTools.getState().start();
    await flush();
    const s = useTools.getState().sheet!;
    expect(s.phase).toBe("running");
    expect(s.progress).toBe(0.62);
    expect(s.status).toBe("Optimizing images, page 8 of 12");
    expect(vi.mocked(api.compressStart).mock.calls[0][1]).toEqual({ doc: 1, preset: "smallest", chooseOutput: false });

    report$(report);
    await run;
    expect(useTools.getState().sheet?.phase).toBe("done");
    expect(useTools.getState().sheet?.compressed).toEqual(report);
    expect(useTools.getState().settings.compressPreset).toBe("smallest");
  });

  it("saves pending edits first and stops if they could not be saved", async () => {
    doc(true);
    vi.mocked(api.compressAudit).mockResolvedValue(audit() as never);
    useTools.getState().open("compress");
    expect(useTools.getState().sheet?.needsSave).toBe(true);
    await flush();
    await useTools.getState().start();
    expect(edits.save).toHaveBeenCalledWith(1);
    expect(api.compressStart).not.toHaveBeenCalled();
    expect(useTools.getState().sheet?.phase).toBe("error");
  });

  it("has the Rust side ask for another name when the default is taken, and goes back when that is dismissed", async () => {
    vi.mocked(api.compressAudit).mockResolvedValue(audit(true) as never);
    useTools.getState().open("compress");
    await flush();
    vi.mocked(api.compressStart).mockRejectedValueOnce("cancelled");
    await useTools.getState().start();
    expect(vi.mocked(api.compressStart).mock.calls[0][1]).toMatchObject({ doc: 1, chooseOutput: true });
    expect(useTools.getState().sheet?.phase).toBe("options");

    vi.mocked(api.compressStart).mockResolvedValue(report as never);
    await useTools.getState().start();
    expect(useTools.getState().sheet?.phase).toBe("done");
  });

  it("returns to the options on cancel and shows a readable message on an error code", async () => {
    vi.mocked(api.compressAudit).mockResolvedValue(audit() as never);
    useTools.getState().open("compress");
    await flush();
    vi.mocked(api.compressStart).mockRejectedValueOnce("cancelled");
    await useTools.getState().start();
    expect(useTools.getState().sheet?.phase).toBe("options");

    vi.mocked(api.compressStart).mockRejectedValueOnce("output-exists");
    await useTools.getState().start();
    expect(useTools.getState().sheet?.phase).toBe("error");
    expect(useTools.getState().sheet?.error).toMatch(/already exists/);
  });

  it("cancel tells the engine which job to stop", async () => {
    vi.mocked(api.compressAudit).mockResolvedValue(audit() as never);
    useTools.getState().open("compress");
    await flush();
    vi.mocked(api.compressStart).mockReturnValue(new Promise(() => {}) as never);
    void useTools.getState().start();
    await flush();
    useTools.getState().cancel();
    const job = vi.mocked(api.compressStart).mock.calls[0][0];
    expect(api.toolCancel).toHaveBeenCalledWith(job);
  });
});

describe("OCR sheet", () => {
  const langs = { windows: ["en-US", "sv-SE"], tesseract: null };
  const outcome = {
    report: { pagesRecognized: 10, pagesSkippedText: 2, pagesFailed: 0 },
    state: { rev: 1, undo: "Recognize text (10 pages)" },
    output: null,
  };

  it("offers the installed languages, runs on the chosen pages and applies the new state", async () => {
    vi.mocked(api.ocrLanguages).mockResolvedValue(langs as never);
    useTools.getState().open("ocr");
    await flush();
    expect(useTools.getState().sheet?.languages).toEqual(["en-US"]);

    vi.mocked(api.ocrStart).mockImplementation((_j, _r, onP) => {
      onP({ done: 3, total: 12, page: 2 });
      return Promise.resolve(outcome as never);
    });
    useTools.getState().set({ scope: "range", range: "1-3, 5", languages: ["sv-SE"] });
    await useTools.getState().start();
    expect(vi.mocked(api.ocrStart).mock.calls[0][1]).toMatchObject({ doc: 1, pages: "1-3, 5", languages: ["sv-SE"], force: false });
    expect(edits.applyState).toHaveBeenCalledWith(1, outcome.state);
    expect(useTools.getState().sheet?.phase).toBe("done");
    expect(useTools.getState().settings.ocrLanguage).toBe("sv-SE");
  });

  it("reads this page when asked, and undo reverts the single step", async () => {
    vi.mocked(api.ocrLanguages).mockResolvedValue(langs as never);
    useTools.getState().open("ocr", 1, { scope: "page" });
    await flush();
    vi.mocked(api.ocrStart).mockResolvedValue(outcome as never);
    await useTools.getState().start();
    expect(vi.mocked(api.ocrStart).mock.calls[0][1].pages).toBe("3");
    await useTools.getState().undo();
    expect(edits.undo).toHaveBeenCalledWith(1);
    expect(useTools.getState().sheet).toBeNull();
  });

  it("save as new file asks for the name and leaves the document alone", async () => {
    vi.mocked(api.ocrLanguages).mockResolvedValue(langs as never);
    useTools.getState().open("ocr");
    await flush();
    vi.mocked(api.ocrStart).mockResolvedValue({ report: outcome.report, state: null, output: "C:\\d\\a (searchable).pdf" } as never);
    await useTools.getState().start({ saveAs: true });
    expect(vi.mocked(api.ocrStart).mock.calls[0][1].saveAs).toBe(true);
    expect(edits.applyState).not.toHaveBeenCalled();
  });
});

describe("formatBytes", () => {
  it("reads like a file manager", () => {
    expect(formatBytes(8_400_000)).toBe("8.4 MB");
    expect(formatBytes(2_000_000)).toBe("2.0 MB");
    expect(formatBytes(612_000)).toBe("612 KB");
    expect(formatBytes(900)).toBe("900 B");
  });
});
