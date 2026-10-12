import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./edit", () => ({ applyState: vi.fn() }));
const setUi = vi.fn();
vi.mock("../store", () => ({ useStore: { getState: () => ({ setUi, docs: { 1: { name: "Contract.pdf" } } }) } }));
vi.mock("./api", () => ({ pickSave: vi.fn() }));
vi.mock("./annot", () => ({ useAnnot: { getState: () => ({ refresh: vi.fn() }) } }));
vi.mock("./forms", () => ({ useForms: { getState: () => ({ refresh: vi.fn() }) } }));

import { invoke } from "@tauri-apps/api/core";
import { pickSave } from "./api";
import { applyState } from "./edit";
import {
  COMMENTS_CERTIFIED, FORMS_CERTIFIED, SIGNED_NOTE, commentsExportText, commentsImportText, formExportText, formImportText, useExchange,
} from "./dataExchange";

const inv = vi.mocked(invoke);
const state = { rev: 3, pageCount: 1, sizes: [], path: "C:\\Contract.pdf", dirty: true, undo: "Import comments", redo: null };

beforeEach(() => {
  vi.clearAllMocks();
  useExchange.setState({ gates: {} });
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
    // A total with nothing listed (never a dangling "skipped:  and 1 more").
    expect(formImportText({ set: 1, unchanged: 0, skipped: [], skippedTotal: 1 }, false)).toBe("Imported form data: 1 field filled, 1 skipped.");
  });

  it("export texts name the file and what was left out", () => {
    expect(commentsExportText({ count: 1, leftOut: 2 }, "a.xfdf")).toBe("Exported 1 comment to a.xfdf. 2 comments left out: measurements, and kinds the file cannot carry.");
    expect(commentsExportText({ count: 3, leftOut: 0, longText: 2 }, "a.xfdf")).toBe("Exported 3 comments to a.xfdf. 2 texts longer than 1 MB left empty.");
    expect(formExportText({ count: 4 }, "a.fdf")).toBe("Exported 4 fields to a.fdf.");
    expect(formExportText({ count: 4, leftOut: 1 }, "a.fdf")).toBe("Exported 4 fields to a.fdf. 1 password field left out.");
    expect(formExportText({ count: 4, leftOut: 0, longText: 2 }, "a.fdf")).toBe("Exported 4 fields to a.fdf. 2 fields with a value over 1 MB left out.");
    expect(formExportText({ count: 4, flattened: 1 }, "a.txt")).toBe("Exported 4 fields to a.txt. 1 value had a tab or line break written as a space: tab text has no quoting.");
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

  const open = { comments: true, forms: true, signed: false };
  /** The engine's answers by command; `data_gates` defaults to everything allowed. */
  const engine = (answers: Record<string, unknown>) =>
    inv.mockImplementation((cmd: string) => (cmd in answers ? Promise.resolve(answers[cmd]) : Promise.resolve(cmd === "data_gates" ? open : null)));
  const ran = () => inv.mock.calls.map((c) => c[0]);

  it("import picks a file, applies the returned state once and shows the summary", async () => {
    engine({ pick_data_file: { token: "f1", path: "C:\\in.xfdf" }, annot_import: { summary: { added: 1, updated: 0, skipped: [] }, state, signed: false } });
    await useExchange.getState().importComments(1);
    expect(inv).toHaveBeenCalledWith("annot_import", { doc: 1, file: "f1" });
    expect(applyState).toHaveBeenCalledTimes(1);
    expect(applyState).toHaveBeenCalledWith(1, state);
    expect(setUi).toHaveBeenCalledWith({ error: "Imported comments: 1 added." });
  });

  it("a cancelled file dialog changes nothing", async () => {
    engine({});
    await useExchange.getState().importForm(1);
    expect(ran()).toEqual(["data_gates", "pick_data_file"]);
    expect(applyState).not.toHaveBeenCalled();
  });

  it.each([
    ["P 1 (nothing allowed)", { comments: false, forms: false, signed: true }, "importComments", COMMENTS_CERTIFIED],
    ["P 1 (nothing allowed)", { comments: false, forms: false, signed: true }, "importForm", FORMS_CERTIFIED],
    ["P 2 (form filling only)", { comments: false, forms: true, signed: true }, "importComments", COMMENTS_CERTIFIED],
  ] as const)("a certification at %s refuses before any dialog opens, with the reason", async (_, gates, act, reason) => {
    engine({ data_gates: gates });
    await useExchange.getState()[act](1);
    expect(ran()).toEqual(["data_gates"]);
    expect(setUi).toHaveBeenCalledWith({ error: reason });
  });

  it("a certification at P 2 takes form data, one at P 3 takes both", async () => {
    engine({ data_gates: { comments: false, forms: true, signed: true } });
    await useExchange.getState().importForm(1);
    expect(ran()).toEqual(["data_gates", "pick_data_file"]);
    inv.mockClear();
    engine({ data_gates: { comments: true, forms: true, signed: true } });
    await useExchange.getState().importComments(1);
    expect(ran()).toEqual(["data_gates", "pick_data_file"]);
  });

  it("the gates are read again for each import and cached for the panels", async () => {
    engine({ data_gates: { comments: true, forms: false, signed: false } });
    await useExchange.getState().readGates(1);
    expect(useExchange.getState().gates[1]).toEqual({ comments: true, forms: false, signed: false });
    engine({ data_gates: { comments: false, forms: false, signed: true } });
    await useExchange.getState().importComments(1);
    expect(useExchange.getState().gates[1].comments).toBe(false);
    expect(setUi).toHaveBeenCalledWith({ error: COMMENTS_CERTIFIED });
  });

  it("an engine error becomes the toast", async () => {
    inv.mockImplementation((cmd: string) =>
      cmd === "data_gates" ? Promise.resolve(open) : cmd === "pick_data_file" ? Promise.resolve({ token: "f", path: "x" }) : Promise.reject("This FDF file is damaged, so Inkwell cannot read it."),
    );
    await useExchange.getState().importForm(1);
    expect(applyState).not.toHaveBeenCalled();
    expect(setUi).toHaveBeenCalledWith({ error: "This FDF file is damaged, so Inkwell cannot read it." });
  });
});

describe("a table file with several records", () => {
  const formState = { rev: 4, pageCount: 1, sizes: [], path: "C:\\Contract.pdf", dirty: true, undo: "Import form data", redo: null };
  const summary = { set: 2, unchanged: 0, skipped: [], skippedTotal: 0 };
  const answers = (records: { count: number; labels: string[]; rows: number[] }) =>
    inv.mockImplementation((cmd: string) =>
      Promise.resolve(
        cmd === "data_gates" ? { comments: true, forms: true, signed: false }
        : cmd === "pick_data_file" ? { token: "f1", path: "C:\\Data\\people.csv" }
        : cmd === "forms_import_records" ? records
        : cmd === "forms_import" ? { summary, state: formState, signed: false }
        : null,
      ),
    );
  const importCall = () => inv.mock.calls.find((c) => c[0] === "forms_import");

  it("asks which record and imports the picked one", async () => {
    answers({ count: 3, labels: ["Asa Berg", "Side St 2", "Ole Dahl"], rows: [2, 3, 6] });
    const done = useExchange.getState().importForm(1);
    await vi.waitFor(() => expect(useExchange.getState().picker).not.toBeNull());
    expect(useExchange.getState().picker).toEqual({ name: "people.csv", labels: ["Asa Berg", "Side St 2", "Ole Dahl"], rows: [2, 3, 6] });
    expect(importCall()).toBeUndefined();
    useExchange.getState().chooseRecord(2);
    await done;
    expect(importCall()).toEqual(["forms_import", { doc: 1, file: "f1", row: 2 }]);
    expect(useExchange.getState().picker).toBeNull();
    expect(applyState).toHaveBeenCalledWith(1, formState);
    expect(setUi).toHaveBeenCalledWith({ error: "Imported form data: 2 fields filled." });
  });

  it("cancelling the question imports nothing", async () => {
    answers({ count: 3, labels: ["a", "b", "c"], rows: [2, 3, 4] });
    const done = useExchange.getState().importForm(1);
    await vi.waitFor(() => expect(useExchange.getState().picker).not.toBeNull());
    useExchange.getState().chooseRecord(null);
    await done;
    expect(importCall()).toBeUndefined();
    expect(applyState).not.toHaveBeenCalled();
    expect(useExchange.getState().picker).toBeNull();
  });

  it("a file with one record, or another format, asks nothing", async () => {
    answers({ count: 1, labels: [], rows: [] });
    await useExchange.getState().importForm(1);
    expect(importCall()).toEqual(["forms_import", { doc: 1, file: "f1", row: null }]);
    expect(useExchange.getState().picker).toBeNull();
  });
});
