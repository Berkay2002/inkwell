import { create } from "zustand";
import * as api from "./api";
import type { BatchAction, BatchAnswer, BatchEvent, BatchFileReport, BatchIssue, BatchOutput, BatchPreview, BatchPromptEvent, BatchReport, BatchStep, BatchStepKind, FileRef } from "./api";
import { copyOf, emptyAction, exportName, hasProtect, MAX_STEPS, move, newStep, saveBlock, splitIssues, withoutPasswords, withPasswords } from "./batchLogic";

/**
 * The Action Wizard: one sheet with three views. `list` shows the built-in and saved actions, `edit` is the
 * step library beside the step sequence, `run` picks files, predicts what would happen (a dry run) and runs.
 * Everything the engine decides (validation, skips, names) comes from `batch_*` commands; this store holds the
 * draft, the files and the progress. Passwords typed for a Protect step live in `run` only: they are merged into
 * the action for one call and are never put in a saved action, an export or any persisted state.
 */

export type BatchView = "list" | "edit" | "run";
/** `starting`: Run was pressed and the engine is being asked (validation, folder pick) before the job exists. */
export type RunPhase = "setup" | "previewing" | "starting" | "running" | "done";
export type RowState = "waiting" | "running" | "ok" | "skipped" | "error";

export interface RunRow {
  state: RowState;
  /** The step the file is on (0-based) and how many there are. */
  step: number;
  steps: number;
  label: string;
  report: BatchFileReport | null;
}

export interface Editor {
  action: BatchAction;
  /** The draft differs from what is stored (a new action, a copy of a built-in, or any edit). */
  dirty: boolean;
  issues: BatchIssue[];
  selected: number | null;
  saving: boolean;
  error: string | null;
}

/** A step the run is waiting on: the person changes its options for this file, then continues, skips it or stops. */
export interface RunPrompt {
  /** The engine's id for this question: an answer for another one is dropped. */
  request: number;
  /** The file (position in the run) and its name, and the step's place. */
  file: number;
  name: string;
  step: number;
  steps: number;
  label: string;
  /** The step as the person has it now: the saved options, with their changes. */
  entry: BatchStep;
  /** Why the last answer was not accepted. */
  problem: string | null;
  /** An answer is on its way. */
  sending: boolean;
}

export interface Run {
  /** Which run this is: an answer that arrives after the person left it is dropped. */
  rid: number;
  action: BatchAction;
  files: FileRef[];
  /** Names of the folders whose listing was cut at the engine's limit. */
  cappedFolders: string[];
  /** A file or folder pick, or a folder listing, is in progress: nothing else may change the run meanwhile. */
  scanning: boolean;
  recursive: boolean;
  openPassword: string;
  permissionsPassword: string;
  phase: RunPhase;
  preview: BatchPreview | null;
  /** The files or the action changed since the preview was made. */
  stale: boolean;
  issues: BatchIssue[];
  job: number | null;
  rows: RunRow[];
  /** The step the run is waiting on, if any. */
  prompt: RunPrompt | null;
  report: BatchReport | null;
  error: string | null;
}

interface BatchStore {
  open: boolean;
  view: BatchView;
  actions: BatchAction[];
  loading: boolean;
  /** Shown above the list: a failed import, delete or export. */
  listError: string | null;
  /** Id of the action whose Delete is waiting for a second click. */
  confirmDelete: string | null;
  editor: Editor | null;
  run: Run | null;

  show: () => Promise<void>;
  close: () => void;
  refresh: () => Promise<void>;
  importFile: () => Promise<void>;
  exportAction: (a: BatchAction) => Promise<void>;
  duplicate: (a: BatchAction) => Promise<void>;
  askDelete: (id: string | null) => void;
  remove: (a: BatchAction) => Promise<void>;

  edit: (a: BatchAction | null) => void;
  backToList: () => void;
  setName: (v: string) => void;
  setDescription: (v: string) => void;
  setOutput: (o: BatchOutput) => void;
  setOnError: (v: "skip" | "stop") => void;
  addStep: (kind: BatchStepKind) => void;
  removeStep: (i: number) => void;
  moveStep: (from: number, to: number) => void;
  patchStep: (i: number, patch: Partial<BatchStep>) => void;
  selectStep: (i: number | null) => void;
  save: () => Promise<void>;

  startRun: (a: BatchAction) => void;
  addFiles: () => Promise<void>;
  addFolder: () => Promise<void>;
  setRecursive: (v: boolean) => void;
  removeFile: (i: number) => void;
  setPassword: (which: "open" | "permissions", v: string) => void;
  preview: () => Promise<void>;
  runNow: () => Promise<void>;
  cancel: () => void;
  again: () => void;
  /** Changes the options (or page range) of the step the run is waiting on, for the file it is on. */
  patchPrompt: (patch: Partial<BatchStep>) => void;
  /** Answers the prompt with that `request`: one that is no longer the current is dropped. */
  answerPrompt: (request: number, answer: "continue" | "skip" | "stop") => Promise<void>;
}

let jobSeq = 1_970_000_000;
let runSeq = 0;
let validateSeq = 0;
let validateTimer: ReturnType<typeof setTimeout> | undefined;

/** Passwords a preview uses when none are typed yet: it writes nothing, so they only let the action validate. */
const PREVIEW_PASSWORDS = ["preview-only-open-1", "preview-only-owner-2"] as const;

const errText = (e: unknown) => String(e).replace(/^Error: /, "");

/** Nothing may leave the run (Back, Close) or start another while this is true. */
export const runLocked = (r: Run | null) => !!r && (r.scanning || r.phase === "running" || r.phase === "previewing" || r.phase === "starting");

const blankRow = (): RunRow => ({ state: "waiting", step: 0, steps: 0, label: "", report: null });

/** The prompt an event opens (a `prompt` event), or null. */
export const promptOf = (ev: BatchPromptEvent): RunPrompt => ({
  request: ev.request,
  file: ev.index,
  name: ev.name,
  step: ev.step,
  steps: ev.steps,
  label: ev.label,
  entry: structuredClone(ev.entry),
  problem: ev.problem,
  sending: false,
});

/** One engine event applied to the rows: the run's state machine, kept pure for the tests. */
export function applyEvent(rows: RunRow[], ev: BatchEvent): RunRow[] {
  switch (ev.event) {
    case "prompt":
      return rows;
    case "started":
      return Array.from({ length: ev.files }, blankRow);
    case "fileStarted":
      return rows.map((r, i) => (i === ev.index ? { ...r, state: "running", step: 0, steps: 0, label: "" } : r));
    case "step":
      return rows.map((r, i) => (i === ev.index ? { ...r, state: "running", step: ev.step, steps: ev.steps, label: ev.label } : r));
    case "fileDone":
      return rows.map((r, i) => (i === ev.index ? { ...r, state: ev.report.status, report: ev.report } : r));
    case "finished":
      return rows;
  }
}

export const useBatch = create<BatchStore>((set, get) => {
  const patchEditor = (p: Partial<Editor>) => set((s) => (s.editor ? { editor: { ...s.editor, ...p } } : s));
  const patchRun = (p: Partial<Run>) => set((s) => (s.run ? { run: { ...s.run, ...p } } : s));
  const draft = (f: (a: BatchAction) => BatchAction) => {
    const e = get().editor;
    if (!e || e.saving) return;
    patchEditor({ action: f(e.action), dirty: true, error: null });
    scheduleValidate();
  };

  /** Asks the engine what is wrong with the draft, a moment after the last change. */
  function scheduleValidate() {
    clearTimeout(validateTimer);
    const seq = ++validateSeq;
    validateTimer = setTimeout(() => {
      const e = get().editor;
      if (!e) return;
      api
        .batchValidate(withoutPasswords(e.action))
        .then((issues) => seq === validateSeq && patchEditor({ issues }))
        .catch(() => {});
    }, 250);
  }

  const listFailed = (e: unknown) => set({ listError: errText(e), confirmDelete: null });

  /** Marks the preview out of date: the files or the action it was made for changed. */
  const stale = () => patchRun({ stale: true });

  return {
    open: false,
    view: "list",
    actions: [],
    loading: false,
    listError: null,
    confirmDelete: null,
    editor: null,
    run: null,

    show: async () => {
      // Opened again (the palette, the Tools panel) under a live run: the sheet stays as it is, with its Cancel.
      if (runLocked(get().run)) {
        set({ open: true });
        return;
      }
      set({ open: true, view: "list", editor: null, run: null, listError: null, confirmDelete: null });
      await get().refresh();
    },

    close: () => {
      if (runLocked(get().run)) return;
      clearTimeout(validateTimer);
      validateSeq++;
      set({ open: false, editor: null, run: null, listError: null, confirmDelete: null });
    },

    refresh: async () => {
      set({ loading: true });
      try {
        set({ actions: await api.batchList(), loading: false });
      } catch (e) {
        set({ loading: false, listError: errText(e) });
      }
    },

    importFile: async () => {
      try {
        const [f] = await api.pickSource("json", false);
        if (!f) return;
        const a = await api.batchImport(f.token);
        await get().refresh();
        set({ listError: null, confirmDelete: null });
        // The imported action is in the list under a new id; open it for a look before anything runs.
        get().edit(a);
      } catch (e) {
        // The engine validated the file: what is wrong with it is the message.
        listFailed(e);
      }
    },

    exportAction: async (a) => {
      try {
        const to = await api.pickSave(exportName(a), {}, "any");
        if (!to) return;
        await api.batchExport(withoutPasswords(a), to.token);
        set({ listError: null });
      } catch (e) {
        listFailed(e);
      }
    },

    duplicate: async (a) => {
      try {
        await api.batchSave(copyOf(a));
        await get().refresh();
        set({ listError: null });
      } catch (e) {
        listFailed(e);
      }
    },

    askDelete: (id) => set({ confirmDelete: id }),

    remove: async (a) => {
      if (a.builtIn) return;
      try {
        await api.batchDelete(a.id);
        set({ confirmDelete: null, listError: null });
        await get().refresh();
      } catch (e) {
        listFailed(e);
      }
    },

    // ---- editor

    edit: (a) => {
      // A built-in is read-only: its editor opens on a copy that Save stores as an action of the person's own.
      const action = a ? (a.builtIn ? copyOf(a) : structuredClone(a)) : emptyAction();
      set({
        view: "edit",
        run: null,
        editor: { action, dirty: a?.builtIn === true || a == null, issues: [], selected: action.steps.length ? 0 : null, saving: false, error: null },
      });
      scheduleValidate();
    },

    backToList: () => {
      const r = get().run;
      if (runLocked(r)) return;
      clearTimeout(validateTimer);
      validateSeq++;
      set({ view: "list", editor: null, run: null, confirmDelete: null });
      void get().refresh();
    },

    setName: (v) => draft((a) => ({ ...a, name: v })),
    setDescription: (v) => draft((a) => ({ ...a, description: v })),
    setOutput: (o) => draft((a) => ({ ...a, output: o })),
    setOnError: (v) => draft((a) => ({ ...a, onError: v })),

    addStep: (kind) => {
      const e = get().editor;
      if (!e || e.action.steps.length >= MAX_STEPS) return;
      draft((a) => ({ ...a, steps: [...a.steps, newStep(kind)] }));
      patchEditor({ selected: e.action.steps.length });
    },

    removeStep: (i) => {
      const e = get().editor;
      if (!e) return;
      draft((a) => ({ ...a, steps: a.steps.filter((_, n) => n !== i) }));
      const n = e.action.steps.length - 1;
      patchEditor({ selected: n === 0 ? null : Math.min(i, n - 1) });
    },

    moveStep: (from, to) => {
      const e = get().editor;
      if (!e) return;
      const next = move(e.action.steps, from, to);
      if (next === e.action.steps) return;
      draft((a) => ({ ...a, steps: next }));
      // The selection follows the step that was moved, or the one it was swapped past.
      const sel = e.selected;
      if (sel === from) patchEditor({ selected: to });
      else if (sel != null && from < sel && to >= sel) patchEditor({ selected: sel - 1 });
      else if (sel != null && from > sel && to <= sel) patchEditor({ selected: sel + 1 });
    },

    patchStep: (i, patch) => draft((a) => ({ ...a, steps: a.steps.map((s, n) => (n === i ? ({ ...s, ...patch } as BatchStep) : s)) })),

    selectStep: (i) => patchEditor({ selected: i }),

    save: async () => {
      const e = get().editor;
      if (!e || e.saving) return;
      const block = saveBlock(e.action);
      if (block) {
        patchEditor({ error: block });
        return;
      }
      patchEditor({ saving: true, error: null });
      try {
        // Never a password: the editor holds none, and this strips any that a step was given.
        await api.batchSave(withoutPasswords(e.action));
        get().backToList();
      } catch (err) {
        patchEditor({ saving: false, error: errText(err) });
      }
    },

    // ---- run

    startRun: (a) => {
      set({
        view: "run",
        editor: null,
        run: {
          rid: ++runSeq,
          action: structuredClone(a),
          files: [],
          cappedFolders: [],
          scanning: false,
          recursive: false,
          openPassword: "",
          permissionsPassword: "",
          phase: "setup",
          preview: null,
          stale: false,
          issues: [],
          job: null,
          rows: [],
          prompt: null,
          report: null,
          error: null,
        },
      });
    },

    addFiles: async () => {
      const r = get().run;
      if (!r || r.phase !== "setup" || r.scanning) return;
      const mine = () => get().run?.rid === r.rid;
      patchRun({ scanning: true });
      try {
        const picked = await api.pickOpen(true);
        if (!mine()) return;
        const have = new Set(get().run?.files.map((f) => f.token));
        const fresh = picked.filter((f) => !have.has(f.token));
        patchRun({ scanning: false, ...(fresh.length ? { files: [...(get().run?.files ?? []), ...fresh], stale: true, error: null } : {}) });
      } catch (e) {
        if (mine()) patchRun({ scanning: false, error: errText(e) });
      }
    },

    addFolder: async () => {
      const r = get().run;
      if (!r || r.phase !== "setup" || r.scanning) return;
      const mine = () => get().run?.rid === r.rid;
      patchRun({ scanning: true });
      try {
        const dir = await api.pickSearchFolder();
        if (!mine()) return;
        if (!dir) {
          patchRun({ scanning: false });
          return;
        }
        const listed = await api.batchFolderFiles(dir.token, get().run?.recursive ?? false);
        if (!mine()) return;
        const cur = get().run;
        const have = new Set(cur?.files.map((f) => f.token));
        const fresh = listed.files.filter((f) => !have.has(f.token));
        const folderName = dir.path.split(/[\\/]/).filter(Boolean).pop() ?? dir.path;
        patchRun({
          scanning: false,
          files: [...(cur?.files ?? []), ...fresh],
          cappedFolders: listed.capped ? [...(cur?.cappedFolders ?? []), folderName] : cur?.cappedFolders ?? [],
          stale: true,
          error: listed.files.length === 0 ? "That folder has no PDF files in it." : null,
        });
      } catch (e) {
        if (mine()) patchRun({ scanning: false, error: errText(e) });
      }
    },

    setRecursive: (v) => patchRun({ recursive: v }),

    removeFile: (i) => {
      const r = get().run;
      if (!r || r.phase !== "setup") return;
      patchRun({ files: r.files.filter((_, n) => n !== i), stale: true });
    },

    setPassword: (which, v) => {
      const r = get().run;
      if (!r || r.phase !== "setup") return;
      patchRun(which === "open" ? { openPassword: v, issues: [] } : { permissionsPassword: v, issues: [] });
      stale();
    },

    preview: async () => {
      const r = get().run;
      if (!r || r.phase !== "setup" || r.scanning || r.files.length === 0) return;
      const mine = () => get().run?.rid === r.rid;
      patchRun({ phase: "previewing", error: null, issues: [] });
      try {
        const typed = !!(r.openPassword || r.permissionsPassword);
        const [o, p] = typed ? [r.openPassword, r.permissionsPassword] : PREVIEW_PASSWORDS;
        const action = withPasswords(r.action, hasProtect(r.action) ? o : "", hasProtect(r.action) ? p : "");
        const issues = await api.batchValidate(action);
        if (!mine()) return;
        if (issues.length) {
          patchRun({ phase: "setup", issues });
          return;
        }
        const preview = await api.batchPreview(action, r.files.map((f) => f.token));
        if (mine()) patchRun({ phase: "setup", preview, stale: false });
      } catch (e) {
        if (mine()) patchRun({ phase: "setup", error: errText(e) });
      }
    },

    runNow: async () => {
      const r = get().run;
      if (!r || r.phase !== "setup" || r.scanning || r.files.length === 0) return;
      const mine = () => get().run?.rid === r.rid;
      const typed = !!(r.openPassword || r.permissionsPassword);
      const action = withPasswords(r.action, r.openPassword, r.permissionsPassword);
      // Before the first await: a second press, Back or Close while the engine is asked finds the run locked.
      patchRun({ phase: "starting", error: null });
      try {
        // What would stop the run is said before anything is asked of the person, and before a folder token is spent.
        const issues = await api.batchValidate(action);
        if (!mine()) return;
        const { blocking, passwordFor } = splitIssues(r.action, issues, typed);
        if (blocking.length || passwordFor.length) {
          patchRun({
            phase: "setup",
            issues,
            error: passwordFor.length && !blocking.length ? "Type the password for the Protect step first." : null,
          });
          return;
        }
        let folder: string | null = null;
        if (r.action.output.kind === "folder") {
          const f = await api.pickFolder();
          if (!mine()) return;
          if (!f) {
            patchRun({ phase: "setup" });
            return;
          }
          folder = f.token;
        }
        const job = ++jobSeq;
        patchRun({ phase: "running", job, issues: [], error: null, rows: r.files.map(blankRow), report: null });
        const report = await api.batchRun(
          job,
          action,
          r.files.map((f) => f.token),
          folder,
          (ev) =>
            set((s) => {
              if (!s.run || s.run.job !== job) return s;
              // A question is open until the engine moves on: a step, a file or the end of the run closes it.
              const prompt = ev.event === "prompt" ? promptOf(ev) : s.run.prompt && ev.event !== "started" ? null : s.run.prompt;
              return { run: { ...s.run, rows: applyEvent(s.run.rows, ev), prompt } };
            }),
        );
        // Passwords are done with: nothing keeps them past the run that used them.
        if (mine()) patchRun({ phase: "done", report, openPassword: "", permissionsPassword: "", job: null, prompt: null });
      } catch (e) {
        if (mine()) patchRun({ phase: get().run?.phase === "running" ? "done" : "setup", error: errText(e), openPassword: "", permissionsPassword: "", job: null, prompt: null });
      }
    },

    cancel: () => {
      const j = get().run?.job;
      if (j != null && get().run?.phase === "running") void api.batchCancel(j).catch(() => {});
    },

    patchPrompt: (patch) =>
      set((s) => {
        const p = s.run?.prompt;
        if (!s.run || !p || p.sending) return s;
        return { run: { ...s.run, prompt: { ...p, entry: { ...p.entry, ...patch } as BatchStep } } };
      }),

    answerPrompt: async (request, answer) => {
      const r = get().run;
      const p = r?.prompt;
      // An answer to a question that is no longer the open one (a second press, a slow double click) is not sent.
      if (!r || !p || p.request !== request || p.sending || r.job == null || r.phase !== "running") return;
      const body: BatchAnswer = answer === "continue" ? { kind: "continue", step: p.entry } : { kind: answer };
      patchRun({ prompt: { ...p, sending: true } });
      try {
        await api.batchPromptAnswer(r.job, request, body);
      } catch {
        // The run decides what happens next; a failed call leaves the question as it was, to try again.
        if (get().run?.prompt?.request === request) patchRun({ prompt: { ...p, sending: false } });
      }
    },

    again: () => {
      const r = get().run;
      if (!r || r.phase !== "done") return;
      patchRun({ phase: "setup", preview: null, stale: false, rows: [], report: null, error: null, issues: [] });
    },
  };
});
