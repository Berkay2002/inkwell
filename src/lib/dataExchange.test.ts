import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));
const setUi = vi.fn();
vi.mock("../store", () => ({ useStore: { getState: () => ({ setUi, docs: { 1: { name: "Contract.pdf" } } }) } }));
vi.mock("./api", () => ({ docProtection: vi.fn(), pickSave: vi.fn() }));
vi.mock("./annot", () => ({ useAnnot: { getState: () => ({ refresh: vi.fn() }) } }));
vi.mock("./forms", () => ({ useForms: { getState: () => ({ refresh: vi.fn() }) } }));

import { invoke } from "@tauri-apps/api/core";
import { docProtection, pickSave } from "./api";
import { applyState } from "./edit";
import {
  CERTIFIED_REASON, SIGNED_NOTE, commentsExportText, commentsImportText, formExportText, formImportText, useExchange,
} from "./dataExchange";

const inv = vi.mocked(invoke);
const state = { rev: 3, pageCount: 1, sizes: [], path: "C:\\Contract.pdf", dirty: true, undo: "Import comments", redo: null };

beforeEach(() => {
  vi.clearAllMocks();
  useExchange.setState({ locks: {} });
  vi.mocked(docProtection).mockResolvedValue({ certified: false, signed: false });
});

describe("summary texts", () => {
  it("comments: added, updated and each skip reason with its count", () => {
    expect(commentsImportText({ added: 2, updated: 1, skipped: [{ reason: "the page is not in this document", count: 3 }, { reason: "no position", count: 1 }] }, false)).toBe(
      "Imported comments: 2 added, 1 updated, 4 skipped (the page is not in this document: 3; no position: 1).",
    );
    expect(commentsImportText({ added: 0, updated: 0, skipped: [] }, true)).toBe("This file has no comments to import.");
  });

  it("a signed document's import says it is saved after the signature, only when something changed", () => {
    expect(commentsImportText({ added: 1, updated: 0, skipped: [] }, true)).toContain(SIGNED_NOTE);
    expect(commentsImportText({ added: 0, updated: 0, skipped: [{ reason: "x", count: 1 }] }, true)).not.toContain(SIGNED_NOTE);
    expect(formImportText({ set: 1, unchanged: 0, skipped: [], skippedTotal: 0 }, true)).toContain(SIGNED_NOTE);
    expect(formImportText({ set: 0, unchanged: 2, skipped: [], skippedTotal: 0 }, true)).not.toContain(SIGNED_NOTE);
  });

  it("form data: lists the first three skipped fields and counts the rest", () => {
    const skipped = ["A", "B", "C"].map((name) => ({ name, reason: "there is no field with this name" }));
    expect(formImportText({ set: 1, unchanged: 2, skipped, skippedTotal: 5 }, false)).toBe(
      "Imported form data: 1 field filled, 2 already had the value, 5 skipped: A (there is no field with this name); B (there is no field with this name); C (there is no field with this name) and 2 more.",
    );
    expect(formImportText({ set: 0, unchanged: 0, skipped: [], skippedTotal: 0 }, false)).toBe("Imported form data: nothing to fill.");
  });

  it("export texts name the file and what was left out", () => {
    expect(commentsExportText({ count: 1, leftOut: 2 }, "a.xfdf")).toBe("Exported 1 comment to a.xfdf. 2 comments left out: measurements, and kinds the file cannot carry.");
    expect(formExportText({ count: 4 }, "a.fdf")).toBe("Exported 4 fields to a.fdf.");
  });
});

describe("export and import", () => {
  it("export asks for a save token of its kind, runs one command and reports it", async () => {
    vi.mocked(pickSave).mockResolvedValue({ token: "t1", path: "C:\\Docs\\Contract comments.xfdf" });
    inv.mockResolvedValue({ count: 3, leftOut: 0 });
    await useExchange.getState().exportComments(1);
    expect(pickSave).toHaveBeenCalledWith("Contract comments.xfdf", { doc: 1 }, "comments");
    expect(inv).toHaveBeenCalledWith("annot_export", { doc: 1, to: "t1" });
    expect(setUi).toHaveBeenCalledWith({ error: "Exported 3 comments to Contract comments.xfdf." });
  });

  it("a cancelled save dialog runs nothing", async () => {
    vi.mocked(pickSave).mockResolvedValue(null);
    await useExchange.getState().exportForm(1);
    expect(inv).not.toHaveBeenCalled();
    expect(setUi).not.toHaveBeenCalled();
  });

  it("import picks a file, applies the returned state once and shows the summary", async () => {
    inv.mockImplementation((cmd: string) =>
      Promise.resolve(cmd === "pick_data_file" ? { token: "f1", path: "C:\\in.xfdf" } : { summary: { added: 1, updated: 0, skipped: [] }, state, signed: false }),
    );
    await useExchange.getState().importComments(1);
    expect(inv).toHaveBeenCalledWith("annot_import", { doc: 1, file: "f1" });
    expect(applyState).toHaveBeenCalledTimes(1);
    expect(applyState).toHaveBeenCalledWith(1, state);
    expect(setUi).toHaveBeenCalledWith({ error: "Imported comments: 1 added." });
  });

  it("a cancelled file dialog changes nothing", async () => {
    inv.mockResolvedValue(null);
    await useExchange.getState().importForm(1);
    expect(inv).toHaveBeenCalledTimes(1);
    expect(applyState).not.toHaveBeenCalled();
  });

  it("a certified document refuses before any dialog opens, with the reason", async () => {
    vi.mocked(docProtection).mockResolvedValue({ certified: true, signed: true });
    await useExchange.getState().importForm(1);
    expect(inv).not.toHaveBeenCalled();
    expect(setUi).toHaveBeenCalledWith({ error: CERTIFIED_REASON });
  });

  it("an engine error becomes the toast", async () => {
    inv.mockImplementation((cmd: string) => (cmd === "pick_data_file" ? Promise.resolve({ token: "f", path: "x" }) : Promise.reject("This FDF file is damaged, so Inkwell cannot read it.")));
    await useExchange.getState().importForm(1);
    expect(applyState).not.toHaveBeenCalled();
    expect(setUi).toHaveBeenCalledWith({ error: "This FDF file is damaged, so Inkwell cannot read it." });
  });
});
