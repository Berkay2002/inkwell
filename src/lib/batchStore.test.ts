import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BatchAction, BatchEvent, BatchFileReport } from "./api";

vi.mock("./api", () => ({
  batchList: vi.fn(),
  batchSave: vi.fn(),
  batchDelete: vi.fn(),
  batchImport: vi.fn(),
  batchExport: vi.fn(),
  batchValidate: vi.fn(),
  batchPreview: vi.fn(),
  batchFolderFiles: vi.fn(),
  batchRun: vi.fn(),
  batchCancel: vi.fn(() => Promise.resolve()),
  pickOpen: vi.fn(),
  pickSave: vi.fn(),
  pickSource: vi.fn(),
  pickFolder: vi.fn(),
  pickSearchFolder: vi.fn(),
}));

import * as api from "./api";
import { applyEvent, useBatch } from "./batchStore";

/** A promise the test settles by hand, to hold an await open while something else happens. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

const ref = (n: string) => ({ token: `tok-${n}`, path: `C:\\d\\${n}.pdf` });
const action = (over: Partial<BatchAction> = {}): BatchAction => ({
  id: "mine",
  name: "Mine",
  description: "",
  steps: [{ kind: "compress", preset: "balanced" }],
  output: { kind: "nextToSource", suffix: " (batch)" },
  onError: "skip",
  builtIn: false,
  ...over,
});
const report = (i: number, status: BatchFileReport["status"], reason: string | null = null): BatchFileReport => ({
  index: i,
  input: `C:\\d\\${i}.pdf`,
  name: `${i}.pdf`,
  status,
  reason,
  sizeBefore: 10,
  sizeAfter: status === "ok" ? 5 : null,
  outputs: [],
  notes: [],
  warnings: [],
  stepsDone: status === "ok" ? 1 : 0,
  ms: 1,
});

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  useBatch.setState({ open: true, view: "list", actions: [], editor: null, run: null, listError: null, confirmDelete: null });
  vi.mocked(api.batchList).mockResolvedValue([]);
  vi.mocked(api.batchValidate).mockResolvedValue([]);
});
afterEach(() => vi.useRealTimers());

const st = () => useBatch.getState();
const flush = () => vi.advanceTimersByTimeAsync(300);

describe("editor state", () => {
  it("builds a draft: steps are added at the end and selected, reordered, and removed", async () => {
    st().edit(null);
    st().addStep("compress");
    st().addStep("watermark");
    st().addStep("bates");
    expect(st().editor!.action.steps.map((s) => s.kind)).toEqual(["compress", "watermark", "bates"]);
    expect(st().editor!.selected).toBe(2);
    st().moveStep(2, 0);
    expect(st().editor!.action.steps.map((s) => s.kind)).toEqual(["bates", "compress", "watermark"]);
    expect(st().editor!.selected).toBe(0);
    // a step moved past the selected one shifts the selection with it
    st().moveStep(2, 0);
    expect(st().editor!.selected).toBe(1);
    st().removeStep(0);
    expect(st().editor!.action.steps.map((s) => s.kind)).toEqual(["bates", "compress"]);
    expect(st().editor!.dirty).toBe(true);
    await flush();
  });

  it("moves nothing for an index outside the list", async () => {
    st().edit(action({ steps: [{ kind: "flattenForms" }, { kind: "compress", preset: "smallest" }] }));
    const before = st().editor!.action.steps;
    st().moveStep(0, 5);
    st().moveStep(-1, 0);
    expect(st().editor!.action.steps).toBe(before);
    await flush();
  });

  it("asks the engine once, a moment after the last change, and keeps the issues it names", async () => {
    vi.mocked(api.batchValidate).mockResolvedValue([{ step: 0, code: "invalid-step", message: "Choose Smallest, Balanced or High quality." }]);
    st().edit(null);
    st().setName("A");
    st().addStep("compress");
    st().patchStep(0, { preset: "custom" } as never);
    await flush();
    expect(api.batchValidate).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.batchValidate).mock.calls[0][0].steps[0]).toEqual({ kind: "compress", preset: "custom" });
    expect(st().editor!.issues[0].message).toContain("Smallest");
  });

  it("opens a built-in as a copy that Save stores under a new id", async () => {
    const b = action({ id: "builtin-make-searchable", name: "Make searchable", builtIn: true });
    st().edit(b);
    expect(st().editor!.action).toMatchObject({ id: "", builtIn: false, name: "Make searchable copy" });
    await flush();
  });
});

describe("save", () => {
  it("refuses an unnamed or empty action without calling the engine, and says why", async () => {
    st().edit(null);
    await st().save();
    expect(st().editor!.error).toBe("Give the action a name.");
    st().setName("Named");
    await st().save();
    expect(st().editor!.error).toBe("Add at least one step.");
    expect(api.batchSave).not.toHaveBeenCalled();
    await flush();
  });

  it("refuses a step the engine does not know, with the engine's own reason", async () => {
    st().edit(action({ steps: [{ kind: "invalid", name: "decrypt", why: "Removing security is not allowed in a batch action." }] }));
    await st().save();
    expect(st().editor!.error).toContain("Removing security");
    expect(api.batchSave).not.toHaveBeenCalled();
    await flush();
  });

  it("never sends a password, and returns to the list", async () => {
    vi.mocked(api.batchSave).mockResolvedValue(action());
    st().edit(action({ steps: [{ kind: "protect", printing: "high", changes: "allButExtract", copy: true, accessibility: true, cipher: "aes256", openPassword: "secret-pw-1" }] }));
    await st().save();
    const sent = vi.mocked(api.batchSave).mock.calls[0][0];
    expect(JSON.stringify(sent)).not.toContain("secret-pw-1");
    expect(st().view).toBe("list");
    expect(st().editor).toBeNull();
  });

  it("shows a refused save in the editor and keeps the draft", async () => {
    vi.mocked(api.batchSave).mockRejectedValue("Give the action a name of 1 to 80 characters.");
    st().edit(action());
    await st().save();
    expect(st().editor!.error).toBe("Give the action a name of 1 to 80 characters.");
    expect(st().editor!.saving).toBe(false);
    expect(st().view).toBe("edit");
  });
});

describe("list: import, export, duplicate, delete", () => {
  it("shows the engine's validation error for an imported file instead of opening it", async () => {
    vi.mocked(api.pickSource).mockResolvedValue([ref("act")]);
    vi.mocked(api.batchImport).mockRejectedValue("\"decrypt\" is not a step this version of Inkwell knows.");
    await st().importFile();
    expect(api.pickSource).toHaveBeenCalledWith("json", false);
    expect(api.batchImport).toHaveBeenCalledWith("tok-act");
    expect(st().listError).toContain("not a step this version");
    expect(st().view).toBe("list");
  });

  it("opens an imported action for a look, and does nothing when the dialog is cancelled", async () => {
    vi.mocked(api.pickSource).mockResolvedValueOnce([]);
    await st().importFile();
    expect(api.batchImport).not.toHaveBeenCalled();
    vi.mocked(api.pickSource).mockResolvedValueOnce([ref("a")]);
    vi.mocked(api.batchImport).mockResolvedValue(action({ id: "new-id", name: "Imported" }));
    await st().importFile();
    expect(st().view).toBe("edit");
    expect(st().editor!.action.name).toBe("Imported");
    await flush();
  });

  it("exports under a .json name through a save token", async () => {
    vi.mocked(api.pickSave).mockResolvedValue({ token: "save-1", path: "C:\\x.json" });
    await st().exportAction(action({ name: "Mail: weekly?" }));
    expect(vi.mocked(api.pickSave).mock.calls[0]).toEqual(["Mail weekly.json", {}, "any"]);
    expect(api.batchExport).toHaveBeenCalledWith(expect.objectContaining({ id: "mine" }), "save-1");
  });

  it("duplicates without an id and deletes only after a second click, never a built-in", async () => {
    vi.mocked(api.batchSave).mockResolvedValue(action());
    await st().duplicate(action({ id: "builtin-x", builtIn: true, name: "Prepare" }));
    expect(vi.mocked(api.batchSave).mock.calls[0][0]).toMatchObject({ id: "", builtIn: false, name: "Prepare copy" });
    st().askDelete("mine");
    expect(st().confirmDelete).toBe("mine");
    await st().remove(action({ id: "builtin-x", builtIn: true }));
    expect(api.batchDelete).not.toHaveBeenCalled();
    await st().remove(action());
    expect(api.batchDelete).toHaveBeenCalledWith("mine");
    expect(st().confirmDelete).toBeNull();
  });
});

describe("run state machine", () => {
  const protectAction = () =>
    action({ steps: [{ kind: "protect", printing: "high", changes: "allButExtract", copy: true, accessibility: true, cipher: "aes256" }] });

  it("applyEvent follows one file at a time and keeps each report", () => {
    let rows = applyEvent([], { event: "started", files: 2 });
    expect(rows.map((r) => r.state)).toEqual(["waiting", "waiting"]);
    rows = applyEvent(rows, { event: "fileStarted", index: 0, total: 2, name: "a.pdf" });
    rows = applyEvent(rows, { event: "step", index: 0, step: 1, steps: 3, label: "Watermark" });
    expect(rows[0]).toMatchObject({ state: "running", step: 1, steps: 3, label: "Watermark" });
    rows = applyEvent(rows, { event: "fileDone", index: 0, report: report(0, "ok") });
    rows = applyEvent(rows, { event: "fileDone", index: 1, report: report(1, "skipped", "signed") });
    expect(rows.map((r) => r.state)).toEqual(["ok", "skipped"]);
    expect(rows[1].report?.reason).toBe("signed");
  });

  it("lists files once each and takes a folder's PDFs, saying when the list was cut", async () => {
    st().startRun(action());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a"), ref("b")]);
    await st().addFiles();
    await st().addFiles();
    expect(st().run!.files).toHaveLength(2);
    vi.mocked(api.pickSearchFolder).mockResolvedValue({ token: "dir", path: "C:\\d" });
    vi.mocked(api.batchFolderFiles).mockResolvedValue({ files: [ref("b"), ref("c")], capped: true });
    st().setRecursive(true);
    await st().addFolder();
    expect(api.batchFolderFiles).toHaveBeenCalledWith("dir", true);
    expect(st().run!.files.map((f) => f.token)).toEqual(["tok-a", "tok-b", "tok-c"]);
    expect(st().run!.cappedFolders).toEqual(["d"]);
  });

  it("previews before anything is written: validate, then the dry run, files as tokens", async () => {
    st().startRun(action());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    const p = { action: "Mine", files: [], ok: 1, skipped: 0, errors: 0 };
    vi.mocked(api.batchPreview).mockResolvedValue(p);
    await st().preview();
    expect(api.batchPreview).toHaveBeenCalledWith(expect.objectContaining({ id: "mine" }), ["tok-a"]);
    expect(st().run!.preview).toBe(p);
    expect(st().run!.stale).toBe(false);
    expect(api.batchRun).not.toHaveBeenCalled();
    // a change to the files makes the preview out of date
    vi.mocked(api.pickOpen).mockResolvedValue([ref("z")]);
    await st().addFiles();
    expect(st().run!.stale).toBe(true);
  });

  it("previews a Protect action without a typed password, with stand-ins that are not the typed ones", async () => {
    st().startRun(protectAction());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    vi.mocked(api.batchPreview).mockResolvedValue({ action: "Mine", files: [], ok: 1, skipped: 0, errors: 0 });
    await st().preview();
    const sent = vi.mocked(api.batchPreview).mock.calls[0][0].steps[0] as { openPassword?: string; permissionsPassword?: string };
    expect(sent.openPassword).toBeTruthy();
    expect(sent.permissionsPassword).toBeTruthy();
    expect(sent.openPassword).not.toBe(sent.permissionsPassword);
    expect(st().run!.action.steps[0]).not.toHaveProperty("openPassword");
  });

  it("does not start while the engine names an issue, and does not spend a folder pick", async () => {
    st().startRun(action({ output: { kind: "folder", suffix: "" } }));
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    vi.mocked(api.batchValidate).mockResolvedValue([{ step: 0, code: "unavailable", message: "Tesseract was not found." }]);
    await st().runNow();
    expect(api.pickFolder).not.toHaveBeenCalled();
    expect(api.batchRun).not.toHaveBeenCalled();
    expect(st().run!.issues[0].code).toBe("unavailable");
    expect(st().run!.phase).toBe("setup");
  });

  it("asks for the Protect password before running, then merges it into the one run call only", async () => {
    st().startRun(protectAction());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    vi.mocked(api.batchValidate).mockResolvedValue([{ step: 0, code: "invalid-step", message: "Type the password for this step" }]);
    await st().runNow();
    expect(api.batchRun).not.toHaveBeenCalled();
    expect(st().run!.error).toContain("password");

    vi.mocked(api.batchValidate).mockResolvedValue([]);
    vi.mocked(api.batchRun).mockImplementation(async (_job, _a, _files, _folder, onEvent) => {
      const evs: BatchEvent[] = [
        { event: "started", files: 1 },
        { event: "fileStarted", index: 0, total: 1, name: "a.pdf" },
        { event: "fileDone", index: 0, report: report(0, "ok") },
        { event: "finished", ok: 1, skipped: 0, errors: 0, cancelled: false },
      ];
      evs.forEach(onEvent);
      return { action: "Mine", files: [report(0, "ok")], ok: 1, skipped: 0, errors: 0, cancelled: false, stopped: false };
    });
    st().setPassword("open", "open-pw-123");
    await st().runNow();
    const [, sent, files, folder] = vi.mocked(api.batchRun).mock.calls[0];
    expect((sent.steps[0] as { openPassword?: string }).openPassword).toBe("open-pw-123");
    expect(files).toEqual(["tok-a"]);
    expect(folder).toBeNull();
    const r = st().run!;
    expect(r.phase).toBe("done");
    expect(r.report?.ok).toBe(1);
    expect(r.rows[0].state).toBe("ok");
    // nothing keeps the typed password past the run, and the action in the store never held it
    expect(r.openPassword).toBe("");
    expect(JSON.stringify(r.action)).not.toContain("open-pw-123");
  });

  it("takes the output folder only when Run is pressed, and a cancelled dialog runs nothing", async () => {
    st().startRun(action({ output: { kind: "folder", suffix: "" } }));
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    vi.mocked(api.pickFolder).mockResolvedValueOnce(null);
    await st().runNow();
    expect(api.batchRun).not.toHaveBeenCalled();
    expect(st().run!.phase).toBe("setup");
    vi.mocked(api.pickFolder).mockResolvedValueOnce({ token: "out", path: "C:\\out" });
    vi.mocked(api.batchRun).mockResolvedValue({ action: "Mine", files: [], ok: 0, skipped: 0, errors: 0, cancelled: false, stopped: false });
    await st().runNow();
    expect(vi.mocked(api.batchRun).mock.calls[0][3]).toBe("out");
  });

  it("cancel reaches the running job; a failed run ends in done with the message", async () => {
    st().startRun(action());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    let job = 0;
    let fail!: (e: unknown) => void;
    vi.mocked(api.batchRun).mockImplementation((j) => {
      job = j;
      return new Promise((_, rej) => (fail = rej));
    });
    const running = st().runNow();
    await Promise.resolve();
    await Promise.resolve();
    expect(st().run!.phase).toBe("running");
    st().cancel();
    expect(api.batchCancel).toHaveBeenCalledWith(job);
    // the sheet cannot be closed under a running job
    st().close();
    expect(st().open).toBe(true);
    fail("A batch with that job id is already running.");
    await running;
    expect(st().run!.phase).toBe("done");
    expect(st().run!.error).toContain("already running");
  });

  it("locks the run from the moment Run is pressed: a second press starts nothing, Back and Close do nothing", async () => {
    st().startRun(action());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    const check = deferred<[]>();
    vi.mocked(api.batchValidate).mockReturnValue(check.promise);
    vi.mocked(api.batchRun).mockResolvedValue({ action: "Mine", files: [], ok: 0, skipped: 0, errors: 0, cancelled: false, stopped: false });
    const first = st().runNow();
    expect(st().run!.phase).toBe("starting");
    void st().runNow();
    st().backToList();
    st().close();
    expect(st().view).toBe("run");
    expect(st().open).toBe(true);
    check.resolve([]);
    await first;
    expect(api.batchValidate).toHaveBeenCalledTimes(1);
    expect(api.batchRun).toHaveBeenCalledTimes(1);
    expect(st().run!.phase).toBe("done");
  });

  it("drops the answer of a run that was left: another action's run is not started by it", async () => {
    st().startRun(protectAction());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    st().setPassword("open", "open-pw-123");
    const check = deferred<[]>();
    vi.mocked(api.batchValidate).mockReturnValue(check.promise);
    const old = st().runNow();
    // however it got there: the run on screen is another one now
    st().startRun(action({ id: "other", name: "Other" }));
    check.resolve([]);
    await old;
    expect(api.batchRun).not.toHaveBeenCalled();
    expect(st().run!.action.id).toBe("other");
    expect(st().run!.phase).toBe("setup");
  });

  it("holds Run, Preview, Back and Close while a folder is being listed, and drops a listing for a run that was left", async () => {
    st().startRun(action());
    vi.mocked(api.pickSearchFolder).mockResolvedValue({ token: "dir", path: "C:\\d\\Inbox" });
    const listing = deferred<{ files: ReturnType<typeof ref>[]; capped: boolean }>();
    vi.mocked(api.batchFolderFiles).mockReturnValue(listing.promise);
    const scan = st().addFolder();
    await Promise.resolve();
    await Promise.resolve();
    expect(st().run!.scanning).toBe(true);
    st().backToList();
    st().close();
    await st().runNow();
    await st().preview();
    expect(st().view).toBe("run");
    expect(st().open).toBe(true);
    expect(api.batchValidate).not.toHaveBeenCalled();

    st().startRun(action({ id: "other" }));
    listing.resolve({ files: [ref("x")], capped: true });
    await scan;
    expect(st().run!.files).toEqual([]);
    expect(st().run!.cappedFolders).toEqual([]);
    expect(st().run!.scanning).toBe(false);
  });

  it("names each folder that was cut, not a count that mixed picks would make wrong", async () => {
    st().startRun(action());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    vi.mocked(api.pickSearchFolder).mockResolvedValueOnce({ token: "d1", path: "C:\\d\\Big" });
    vi.mocked(api.batchFolderFiles).mockResolvedValueOnce({ files: [ref("b")], capped: true });
    await st().addFolder();
    vi.mocked(api.pickSearchFolder).mockResolvedValueOnce({ token: "d2", path: "C:\\d\\Small" });
    vi.mocked(api.batchFolderFiles).mockResolvedValueOnce({ files: [ref("c")], capped: false });
    await st().addFolder();
    expect(st().run!.cappedFolders).toEqual(["Big"]);
    expect(st().run!.files).toHaveLength(3);
  });

  it("opening the wizard again under a running job keeps the run and its Cancel", async () => {
    st().startRun(action());
    vi.mocked(api.pickOpen).mockResolvedValue([ref("a")]);
    await st().addFiles();
    let job = 0;
    vi.mocked(api.batchRun).mockImplementation((j) => {
      job = j;
      return new Promise(() => {});
    });
    void st().runNow();
    await Promise.resolve();
    await Promise.resolve();
    expect(st().run!.phase).toBe("running");
    useBatch.setState({ open: false });
    await st().show();
    expect(st().open).toBe(true);
    expect(st().view).toBe("run");
    expect(st().run!.phase).toBe("running");
    st().cancel();
    expect(api.batchCancel).toHaveBeenCalledWith(job);
  });
});
