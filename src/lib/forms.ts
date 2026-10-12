import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { EditState } from "./api";
import { applyState } from "./edit";
import { useStore } from "../store";
import type { Detected, Field, FieldKind, FieldPatch, FieldValue, Form, Format, NewField, Rect4, ScriptAction, ScriptPrompt } from "./formsLogic";
import { anchorOf, capPrompts, detectedToNew, effectsOf, nextPicks, resetTargets, samePick } from "./formsLogic";
import type { Pick } from "./formsLogic";
import { usePrint } from "./printStore";

/**
 * Forms for the UI (src-tauri/src/forms). Filling happens on the page; authoring ("Prepare form") is a
 * mode with a field tool strip and the selected field's properties. Every change is one engine edit
 * (one undo step) that Save keeps; the field list is re-read after each one, so undo and redo stay in
 * step with the panel.
 */

/** `app.alert` from a field script: shown by the UI (never a native dialog). */
export interface ScriptAlert {
  message: string;
  title?: string;
  field?: string;
}

/** Something a script asked for. The engine never does any of it; the UI asks first (links: http, https, mailto only). */
export type ScriptRequest =
  | { kind: "reset"; fields: string[] }
  | { kind: "print" }
  | { kind: "goToPage"; page: number }
  | { kind: "launchUrl"; url: string }
  | { kind: "submit"; url: string }
  | { kind: "focus"; field: string }
  | { kind: "beep" };

/** The edit state after a fill or reset, with what the field scripts raised. */
export type Stepped = EditState & { alerts: ScriptAlert[]; requests: ScriptRequest[] };

export interface ScriptLogLine {
  level: "info" | "warn" | "error";
  field?: string;
  message: string;
}

export interface ScriptsState {
  hasScripts: boolean;
  enabled: boolean;
  /** They can run now (not taken down by timeouts or a script engine that stopped answering). */
  usable: boolean;
  /** They may be turned on again (not after the script engine stopped answering: it is left abandoned until the app exits). */
  canEnable: boolean;
  log: ScriptLogLine[];
}

export const formsApi = {
  list: (doc: number) => invoke<Form>("forms_list", { doc }),
  setValue: (doc: number, page: number, xref: number, value: FieldValue) =>
    invoke<Stepped>("forms_set_value", { doc, page, xref, value }),
  reset: (doc: number, fields: number[] | null) => invoke<Stepped>("forms_reset", { doc, fields }),
  scripts: (doc: number) => invoke<ScriptsState>("forms_scripts", { doc }),
  scriptsEnable: (doc: number, on: boolean) => invoke<ScriptsState>("forms_scripts_enable", { doc, on }),
  add: (doc: number, page: number, what: NewField) => invoke<{ state: EditState; xref: number }>("forms_add", { doc, page, what }),
  addMany: (doc: number, page: number, items: NewField[]) => invoke<EditState>("forms_add_many", { doc, page, items }),
  move: (doc: number, page: number, xref: number, rect: Rect4) => invoke<EditState>("forms_move", { doc, page, xref, rect }),
  edit: (doc: number, page: number, xref: number, patch: FieldPatch) => invoke<EditState>("forms_edit", { doc, page, xref, patch }),
  remove: (doc: number, items: [number, number][]) => invoke<EditState>("forms_delete", { doc, items }),
  tabOrder: (doc: number, page: number, order: number[]) => invoke<EditState>("forms_tab_order", { doc, page, order }),
  arrange: (doc: number, page: number, by: "row" | "column") => invoke<EditState>("forms_arrange", { doc, page, by }),
  detect: (doc: number, page: number) => invoke<Detected[]>("forms_detect", { doc, page }),
  calcOrder: (doc: number, nodes: number[]) => invoke<EditState>("forms_calc_order", { doc, nodes }),
  align: (doc: number, items: [number, number][], edge: AlignEdge) => invoke<EditState>("forms_align", { doc, items, edge }),
  center: (doc: number, items: [number, number][], axis: LayoutAxis) => invoke<EditState>("forms_center", { doc, items, axis }),
  distribute: (doc: number, items: [number, number][], axis: LayoutAxis) => invoke<EditState>("forms_distribute", { doc, items, axis }),
  matchSize: (doc: number, items: [number, number][], dim: SizeDim) => invoke<EditState>("forms_match_size", { doc, items, dim }),
  nudge: (doc: number, items: [number, number][], dx: number, dy: number) => invoke<EditState>("forms_nudge", { doc, items, dx, dy }),
  duplicate: (doc: number, item: [number, number], pages: number[]) => invoke<EditState>("forms_duplicate", { doc, item, pages }),
  copies: (doc: number, item: [number, number], rows: number, cols: number, gapX: number, gapY: number) =>
    invoke<EditState & { made: number; skipped: number }>("forms_copies", { doc, item, rows, cols, gapX, gapY }).then((r) => r),
  rename: (doc: number, item: [number, number], name: string, merge: boolean) => invoke<EditState>("forms_rename", { doc, item, name, merge }),
  /** What a format makes of a sample entry (empty: a default one); null for no format and for a custom script. */
  formatPreview: (format: Format, sample = "") => invoke<{ sample: string; shown: string } | null>("forms_format_preview", { format, sample }),
};

export type AlignEdge = "left" | "right" | "top" | "bottom" | "hcenter" | "vcenter";
export type LayoutAxis = "horizontal" | "vertical" | "both";
export type SizeDim = "width" | "height" | "both";

/** The field tools of the authoring strip. */
export type FormTool = "select" | FieldKind;
export const FORM_TOOLS: FormTool[] = ["select", "text", "checkbox", "radio", "combo", "list", "button", "signature"];

export interface Ref {
  page: number;
  xref: number;
}

export interface DocForm {
  fields: Field[];
  /** The `node` of each calculated field, in running order. */
  calcOrder: number[];
  xfa: boolean;
  /** Whether the file has scripts, whether they run, and their log (null until read). */
  scripts: ScriptsState | null;
  /** The banner over the page was dismissed for this session. */
  bannerGone: boolean;
  /** Field chosen on the page or in the list: the anchor, the last of `picked`. */
  selected: Ref | null;
  /** Every field picked (Ctrl or Shift click, marquee), in the order picked; the last is the anchor. */
  picked: Ref[];
  /** What detection found on `detectPage`, shown as a prediction before it is added. */
  detected: Detected[] | null;
  detectPage: number | null;
  /** The message of the last rejected value or failed change. */
  error: string | null;
}

const fresh = (): DocForm => ({ fields: [], calcOrder: [], xfa: false, scripts: null, bannerGone: false, selected: null, picked: [], detected: null, detectPage: null, error: null });

const HIGHLIGHT_KEY = "inkwell.forms.highlight";
const loadHighlight = () => {
  try {
    return localStorage.getItem(HIGHLIGHT_KEY) !== "0";
  } catch {
    return true;
  }
};

interface FormsStore {
  /** The Forms rail is showing. */
  open: boolean;
  setOpen: (open: boolean) => void;
  /** Authoring ("Prepare form") is on: the page edits fields instead of filling them. */
  authoring: boolean;
  setAuthoring: (on: boolean) => void;
  tool: FormTool;
  setTool: (t: FormTool) => void;
  /** Fields are tinted on the page (Acrobat's "Highlight existing fields"). */
  highlight: boolean;
  setHighlight: (on: boolean) => void;

  /** Messages, links and submits that scripts raised, waiting for the person (the first is showing). */
  prompts: (ScriptPrompt & { id: number; doc: number })[];
  dismissPrompt: (id: number) => void;
  /** Drops every waiting prompt (of one document, or of all). */
  dismissAll: (doc?: number) => void;
  /** "Stop scripts in this file": scripts off for the document, and its waiting prompts dropped. */
  stopScripts: (doc: number) => Promise<void>;
  /** Turns the file's scripts on or off. */
  runScripts: (doc: number, on: boolean) => Promise<void>;
  dismissBanner: (doc: number) => void;
  calcOrder: (doc: number, nodes: number[]) => Promise<void>;

  docs: Record<number, DocForm>;
  get: (doc: number) => DocForm;
  refresh: (doc: number) => Promise<void>;
  select: (doc: number, ref: Ref | null) => void;
  /** Click with Ctrl, Cmd or Shift: adds the field to the picked ones (or takes it out). */
  toggle: (doc: number, ref: Ref) => void;
  /** Replaces the picked fields (a marquee); `add` keeps the ones already picked. */
  pickMany: (doc: number, refs: Ref[], add?: boolean) => void;
  /** Which layout dialog the rail shows. */
  dialog: "duplicate" | "copies" | null;
  setDialog: (d: "duplicate" | "copies" | null) => void;
  align: (doc: number, edge: AlignEdge) => Promise<void>;
  center: (doc: number, axis: LayoutAxis) => Promise<void>;
  distribute: (doc: number, axis: LayoutAxis) => Promise<void>;
  matchSize: (doc: number, dim: SizeDim) => Promise<void>;
  nudge: (doc: number, dx: number, dy: number) => Promise<void>;
  /** The picked field again on these pages (0-based). */
  duplicate: (doc: number, pages: number[]) => Promise<boolean>;
  /** A grid of copies of the picked field; resolves to what to tell the person, or null on failure (`error` is set). */
  copies: (doc: number, rows: number, cols: number, gapX: number, gapY: number) => Promise<string | null>;
  /** The field's full name; `merge` joins it to the field that has the name. Resolves to the message of a refusal, or null. */
  rename: (doc: number, ref: Ref, name: string, merge: boolean) => Promise<string | null>;
  clearError: (doc: number) => void;
  /** Fill a field; resolves to the rejection message (the field is left as it was), or null when accepted. */
  setValue: (doc: number, page: number, xref: number, value: FieldValue) => Promise<string | null>;
  clear: (doc: number, fields?: number[]) => Promise<void>;
  add: (doc: number, page: number, what: NewField) => Promise<number | null>;
  move: (doc: number, page: number, xref: number, rect: Rect4) => Promise<void>;
  edit: (doc: number, page: number, xref: number, patch: FieldPatch) => Promise<string | null>;
  remove: (doc: number, refs: Ref[]) => Promise<void>;
  tabOrder: (doc: number, page: number, order: number[]) => Promise<void>;
  arrange: (doc: number, page: number, by: "row" | "column") => Promise<void>;
  detect: (doc: number, page: number) => Promise<void>;
  addDetected: (doc: number) => Promise<void>;
}

let promptId = 0;

/** Moves the page view to a field and focuses its control (the page may still be drawing, so it tries for a moment). */
function focusField(doc: number, name: string) {
  const f = useForms.getState().get(doc).fields.find((x) => x.name === name);
  if (!f) return;
  useStore.getState().goTo(doc, f.page, Math.max(0, f.rect[1] - 80));
  let tries = 0;
  const find = () => {
    const cell = document.querySelector(`[data-fm-xref="${f.xref}"]`);
    const el = cell?.querySelector<HTMLElement>("input, textarea, select, button");
    if (el) el.focus();
    else if (++tries < 20) setTimeout(find, 60);
  };
  setTimeout(find, 60);
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));

export const useForms = create<FormsStore>((set, get) => {
  const patch = (doc: number, p: Partial<DocForm>) =>
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? fresh()), ...p } } }));
  /** Picks fields; the anchor (the last) is the selected one the properties follow. */
  const setPicked = (doc: number, picked: Pick[]) => patch(doc, { picked, selected: anchorOf(picked) });
  const pairs = (doc: number): [number, number][] => get().get(doc).picked.map((r) => [r.page, r.xref]);
  /** Alerts show in a dialog; resets, page changes, focus and print happen now; links and submits wait for the person. */
  const react = (doc: number, r: Stepped, quiet = false) => {
    const { prompts, actions } = effectsOf(r.alerts ?? [], r.requests ?? []);
    if (prompts.length) {
      set((s) => ({ prompts: capPrompts(s.prompts, prompts.map((p) => ({ ...p, id: ++promptId, doc }))).queue }));
    }
    // A reset a script asked for does not act on what that reset's own scripts ask for (no loop).
    if (!quiet) for (const a of actions) void act(doc, a);
  };
  const act = async (doc: number, a: ScriptAction) => {
    switch (a.kind) {
      case "reset": {
        const targets = resetTargets(get().get(doc).fields, a.fields);
        const r = await run(doc, () => formsApi.reset(doc, targets), (s) => s);
        if (r) react(doc, r, true);
        break;
      }
      case "goToPage":
        useStore.getState().goTo(doc, a.page);
        break;
      case "focus":
        focusField(doc, a.field);
        break;
      case "print":
        usePrint.getState().open(doc);
        break;
    }
  };
  /** Runs one engine call, shows its new state, and re-reads the list. Null (and `error`) on failure. */
  const run = async <T>(doc: number, f: () => Promise<T>, state: (r: T) => EditState): Promise<T | null> => {
    try {
      const r = await f();
      applyState(doc, state(r));
      await get().refresh(doc);
      patch(doc, { error: null });
      return r;
    } catch (e) {
      patch(doc, { error: msg(e) });
      return null;
    }
  };

  return {
    open: false,
    setOpen: (open) => set(open ? { open } : { open, authoring: false, tool: "select" }),
    authoring: false,
    setAuthoring: (authoring) => set({ authoring, tool: "select", ...(authoring ? { open: true } : {}) }),
    tool: "select",
    setTool: (tool) => set({ tool }),
    highlight: loadHighlight(),
    setHighlight: (highlight) => {
      try {
        localStorage.setItem(HIGHLIGHT_KEY, highlight ? "1" : "0");
      } catch {
        /* kept for the session */
      }
      set({ highlight });
    },

    prompts: [],
    dismissPrompt: (id) => set((s) => ({ prompts: s.prompts.filter((p) => p.id !== id) })),
    dismissAll: (doc) => set((s) => ({ prompts: doc === undefined ? [] : s.prompts.filter((p) => p.doc !== doc) })),
    stopScripts: async (doc) => {
      get().dismissAll(doc);
      await get().runScripts(doc, false);
    },
    runScripts: async (doc, on) => {
      try {
        patch(doc, { scripts: await formsApi.scriptsEnable(doc, on), error: null });
        await get().refresh(doc);
      } catch (e) {
        patch(doc, { error: msg(e) });
      }
    },
    dismissBanner: (doc) => patch(doc, { bannerGone: true }),
    calcOrder: async (doc, nodes) => {
      await run(doc, () => formsApi.calcOrder(doc, nodes), (s) => s);
    },

    docs: {},
    get: (doc) => get().docs[doc] ?? fresh(),
    refresh: async (doc) => {
      // The scripts state does not wait for the field list (listing runs the Format scripts), so the banner and the
      // switch show at once. It is read again after the list, which may have logged something.
      const readScripts = () =>
        formsApi.scripts(doc).then(
          (scripts) => patch(doc, { scripts }),
          () => undefined,
        );
      const early = readScripts();
      try {
        const form = await formsApi.list(doc);
        const d = get().get(doc);
        const alive = (r: Ref) => form.fields.some((f) => f.page === r.page && f.xref === r.xref);
        const picked = d.picked.filter(alive);
        patch(doc, { fields: form.fields, calcOrder: form.calcOrder ?? [], xfa: form.xfa, picked, selected: anchorOf(picked) });
        await early;
        await readScripts();
      } catch (e) {
        await early;
        patch(doc, { error: msg(e) });
      }
    },
    select: (doc, ref) => setPicked(doc, ref ? [ref] : []),
    toggle: (doc, ref) => setPicked(doc, nextPicks(get().get(doc).picked, ref, true)),
    pickMany: (doc, refs, add = false) => {
      const had = add ? get().get(doc).picked : [];
      setPicked(doc, [...had.filter((h) => !refs.some((r) => samePick(r, h))), ...refs]);
    },
    dialog: null,
    setDialog: (dialog) => set({ dialog }),
    align: async (doc, edge) => {
      await run(doc, () => formsApi.align(doc, pairs(doc), edge), (s) => s);
    },
    center: async (doc, axis) => {
      await run(doc, () => formsApi.center(doc, pairs(doc), axis), (s) => s);
    },
    distribute: async (doc, axis) => {
      await run(doc, () => formsApi.distribute(doc, pairs(doc), axis), (s) => s);
    },
    matchSize: async (doc, dim) => {
      await run(doc, () => formsApi.matchSize(doc, pairs(doc), dim), (s) => s);
    },
    nudge: async (doc, dx, dy) => {
      await run(doc, () => formsApi.nudge(doc, pairs(doc), dx, dy), (s) => s);
    },
    duplicate: async (doc, pages) => {
      const a = anchorOf(get().get(doc).picked);
      if (!a) return false;
      return (await run(doc, () => formsApi.duplicate(doc, [a.page, a.xref], pages), (s) => s)) !== null;
    },
    copies: async (doc, rows, cols, gapX, gapY) => {
      const a = anchorOf(get().get(doc).picked);
      if (!a) return null;
      const r = await run(doc, () => formsApi.copies(doc, [a.page, a.xref], rows, cols, gapX, gapY), (s) => s);
      if (!r) return null;
      // The original is name.0 now: keep it picked, as the same widget.
      return `${r.made} cop${r.made === 1 ? "y" : "ies"} made${r.skipped ? `, ${r.skipped} left out because they would run off the page` : ""}.`;
    },
    rename: async (doc, ref, name, merge) => {
      const r = await run(doc, () => formsApi.rename(doc, [ref.page, ref.xref], name, merge), (s) => s);
      return r ? null : get().get(doc).error;
    },
    clearError: (doc) => patch(doc, { error: null }),

    setValue: async (doc, page, xref, value) => {
      const r = await run(doc, () => formsApi.setValue(doc, page, xref, value), (s) => s);
      if (r) react(doc, r);
      return r ? null : get().get(doc).error;
    },
    clear: async (doc, fields) => {
      const r = await run(doc, () => formsApi.reset(doc, fields ?? null), (s) => s);
      if (r) react(doc, r);
    },
    add: async (doc, page, what) => {
      const r = await run(doc, () => formsApi.add(doc, page, what), (x) => x.state);
      if (!r) return null;
      setPicked(doc, [{ page, xref: r.xref }]);
      return r.xref;
    },
    move: async (doc, page, xref, rect) => {
      await run(doc, () => formsApi.move(doc, page, xref, rect), (s) => s);
    },
    edit: async (doc, page, xref, p) => {
      const r = await run(doc, () => formsApi.edit(doc, page, xref, p), (s) => s);
      return r ? null : get().get(doc).error;
    },
    remove: async (doc, refs) => {
      if (refs.length === 0) return;
      await run(doc, () => formsApi.remove(doc, refs.map((r) => [r.page, r.xref])), (s) => s);
      setPicked(doc, []);
    },
    tabOrder: async (doc, page, order) => {
      await run(doc, () => formsApi.tabOrder(doc, page, order), (s) => s);
    },
    arrange: async (doc, page, by) => {
      await run(doc, () => formsApi.arrange(doc, page, by), (s) => s);
    },
    detect: async (doc, page) => {
      try {
        patch(doc, { detected: await formsApi.detect(doc, page), detectPage: page, error: null });
      } catch (e) {
        patch(doc, { detected: null, detectPage: null, error: msg(e) });
      }
    },
    addDetected: async (doc) => {
      const d = get().get(doc);
      if (!d.detected || d.detectPage === null || d.detected.length === 0) return;
      const page = d.detectPage;
      const ok = await run(doc, () => formsApi.addMany(doc, page, detectedToNew(d.detected!)), (s) => s);
      if (ok) patch(doc, { detected: [], detectPage: page });
    },
  };
});
