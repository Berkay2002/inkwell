import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { DocJsCheck, DocJsList, DocJsOp } from "./api";
import { applyState } from "./edit";
import { useForms } from "./forms";

/**
 * The Document JavaScript sheet: the scripts a file carries (named scripts, document and page actions), one of them
 * in a plain editor at a time. Listing, reading and checking never run a script; Save is one engine edit (one undo
 * step). What the forms runtime itself runs, and when, is not changed here.
 */

export interface DocJsSheet {
  doc: number;
  list: DocJsList | null;
  error: string | null;
  /** The script in the editor: a named script's id, or `<slot>:<step>` (an empty slot's is `<slot>:0`). */
  selected: string | null;
  text: string;
  /** The text as it is in the file, to know what was changed. */
  saved: string;
  /** The script is over 1 MiB: not shown. */
  tooLarge: boolean;
  check: DocJsCheck | null;
  /** A Save of a script with a syntax error waits for a yes. */
  confirmSave: boolean;
  /** Something with unsaved changes is waiting for a yes to discard them: "close", "add" (a new script), or the id to switch to. */
  confirmDiscard: string | null;
  /** The name being typed for a new script / the new name of the selected one; null when not doing either. */
  adding: string | null;
  renaming: string | null;
  busy: boolean;
}

interface DocJsStore {
  sheet: DocJsSheet | null;
  open: (doc: number) => void;
  /** Closes (asks first when there are unsaved changes). */
  close: () => void;
  select: (id: string) => void;
  setText: (text: string) => void;
  /** Starts a script for a page's action (1-based page, `O` opens it, `C` closes it); saving it creates the action. */
  newPageAction: (page: number, event: "O" | "C") => void;
  save: (force?: boolean) => Promise<void>;
  /** Answers a pending question: the unsaved changes are dropped (`yes`) or kept. */
  answerDiscard: (yes: boolean) => void;
  cancelConfirmSave: () => void;
  startAdd: () => void;
  setAdding: (name: string | null) => void;
  commitAdd: () => Promise<void>;
  startRename: () => void;
  setRenaming: (name: string | null) => void;
  commitRename: () => Promise<void>;
  /** Deletes the selected named script, or clears the selected action (with its chain). */
  remove: () => Promise<void>;
  /** Closes the sheet and opens the forms rail, where field scripts are edited. */
  showInForms: () => void;
}

export const isNamed = (id: string) => id.startsWith("n:");
/** The id of the slot a step belongs to (`d:WC:0` is in `d:WC`; `p:2:O:1` in `p:2:O`). */
export const slotOf = (id: string) => id.replace(/:\d+$/, "");

export function dirty(s: DocJsSheet): boolean {
  return s.selected != null && !s.tooLarge && s.text !== s.saved;
}

/** One line for the status under the editor. */
export function statusLine(s: DocJsSheet): { kind: "ok" | "error" | "note"; text: string } {
  const c = s.check;
  if (s.tooLarge) return { kind: "note", text: "This script is over 1 MiB, too large to show here." };
  if (!c) return { kind: "note", text: "Checking..." };
  if (c.tooLarge) return { kind: "note", text: "Over 256 KiB: the engine does not run a script this long, so it is not checked." };
  if (c.ok) return { kind: "ok", text: "No syntax errors." };
  const at = c.line != null ? `Line ${c.line}${c.column != null ? `, column ${c.column}` : ""}: ` : "";
  return { kind: "error", text: `${at}${c.message ?? "syntax error"}` };
}

/** The first script of a list worth showing: the first named one, else the first action that has JavaScript. */
function firstScript(l: DocJsList): string | null {
  if (l.named[0]) return l.named[0].id;
  for (const slot of [...l.docActions, ...l.pageActions]) {
    const step = slot.steps.find((x) => x.javascript);
    if (step) return step.id;
  }
  return null;
}

/** Whether `id` names a script or an action slot's first step in `l` (an empty slot counts). */
function exists(l: DocJsList, id: string): boolean {
  if (isNamed(id)) return l.named.some((n) => n.id === id);
  if (l.docActions.some((s) => `${s.id}:0` === id)) return true;
  return l.docActions.concat(l.pageActions).some((s) => s.steps.some((x) => x.id === id));
}

let loads = 0;
let refreshes = 0;
let checks = 0;
let checkTimer: ReturnType<typeof setTimeout> | undefined;
let unsubscribe: (() => void) | undefined;

const CHECK_DELAY = 250;

export const useDocJs = create<DocJsStore>((set, get) => {
  const patch = (p: Partial<DocJsSheet>) => set((s) => (s.sheet ? { sheet: { ...s.sheet, ...p } } : s));
  const doc = () => get().sheet?.doc;

  const runCheck = (text: string) => {
    clearTimeout(checkTimer);
    const seq = ++checks;
    api
      .docJsCheck(text)
      .then((check) => seq === checks && patch({ check }))
      .catch(() => seq === checks && patch({ check: null }));
  };

  /** Reads the text of `id` into the editor (nothing is run: the engine only reads the object). */
  const load = async (id: string) => {
    const d = doc();
    if (d == null) return;
    const seq = ++loads;
    clearTimeout(checkTimer);
    ++checks;
    patch({ selected: id, text: "", saved: "", tooLarge: false, check: null, confirmSave: false, confirmDiscard: null, adding: null, renaming: null });
    // An empty slot has nothing to read.
    const l = get().sheet?.list;
    const empty = l && !isNamed(id) && !l.docActions.concat(l.pageActions).some((s) => s.steps.some((x) => x.id === id));
    if (empty) {
      patch({ check: { ok: true, message: null, line: null, column: null, tooLarge: false } });
      return;
    }
    try {
      const src = await api.docJsSource(d, id);
      if (seq !== loads || get().sheet?.selected !== id) return;
      patch({ text: src.text, saved: src.text, tooLarge: src.tooLarge });
      if (!src.tooLarge) runCheck(src.text);
    } catch (e) {
      if (seq === loads) patch({ error: String(e) });
    }
  };

  /** Reads the list again; keeps the selection when it is still there (and its unsaved text when `keepText`). */
  const refresh = async (keepText: boolean, select?: string | null) => {
    const d = doc();
    if (d == null) return;
    // Only the newest read counts: the file's own change event and an edit's can both ask.
    const seq = ++refreshes;
    try {
      const list = await api.docJsList(d);
      if (doc() !== d || seq !== refreshes) return;
      patch({ list, error: null });
      const cur = select ?? get().sheet?.selected ?? null;
      const next = cur && exists(list, cur) ? cur : firstScript(list);
      // Unsaved typing stays; otherwise the editor shows the file as it is now (an undo changes it under the sheet).
      const s = get().sheet;
      if (next === s?.selected && keepText && s && dirty(s)) return;
      if (next) await load(next);
      else patch({ selected: null, text: "", saved: "", check: null, tooLarge: false });
    } catch (e) {
      patch({ error: String(e) });
    }
  };

  /** One engine edit, then the list as it is now. */
  const apply = async (op: DocJsOp, select?: (applied: api.DocJsApplied) => string | null): Promise<boolean> => {
    const d = doc();
    if (d == null) return false;
    patch({ busy: true, error: null });
    try {
      const res = await api.docJsApply(d, op);
      applyState(d, res.state);
      patch({ busy: false });
      await refresh(false, select ? select(res) : undefined);
      return true;
    } catch (e) {
      patch({ busy: false, error: String(e) });
      return false;
    }
  };

  return {
    sheet: null,
    open: (d) => {
      unsubscribe?.();
      set({
        sheet: {
          doc: d,
          list: null,
          error: null,
          selected: null,
          text: "",
          saved: "",
          tooLarge: false,
          check: null,
          confirmSave: false,
          confirmDiscard: null,
          adding: null,
          renaming: null,
          busy: false,
        },
      });
      // Undo, redo and every other edit change the file under the sheet: read it again.
      let rev = useStore.getState().docs[d]?.rev;
      unsubscribe = useStore.subscribe((st) => {
        const r = st.docs[d]?.rev;
        if (r === rev) return;
        rev = r;
        if (get().sheet?.doc === d && r !== undefined) void refresh(true);
      });
      void refresh(false);
    },
    close: () => {
      const s = get().sheet;
      if (!s) return;
      if (dirty(s)) {
        patch({ confirmDiscard: "close" });
        return;
      }
      unsubscribe?.();
      unsubscribe = undefined;
      clearTimeout(checkTimer);
      loads++;
      checks++;
      set({ sheet: null });
    },
    select: (id) => {
      const s = get().sheet;
      if (!s || id === s.selected) return;
      if (dirty(s)) {
        patch({ confirmDiscard: id });
        return;
      }
      void load(id);
    },
    setText: (text) => {
      const s = get().sheet;
      if (!s || s.tooLarge || s.selected == null) return;
      patch({ text, confirmSave: false, check: null });
      clearTimeout(checkTimer);
      const seq = ++checks;
      checkTimer = setTimeout(() => {
        if (seq === checks) runCheck(text);
      }, CHECK_DELAY);
    },
    newPageAction: (page, event) => {
      const n = get().sheet?.doc != null ? useStore.getState().docs[get().sheet!.doc]?.pageCount ?? 0 : 0;
      if (!Number.isInteger(page) || page < 1 || page > n) {
        patch({ error: `The file has ${n} page${n === 1 ? "" : "s"}.` });
        return;
      }
      get().select(`p:${page - 1}:${event}:0`);
    },
    save: async (force = false) => {
      const s = get().sheet;
      if (!s || s.selected == null || s.busy || !dirty(s)) return;
      // A script with a syntax error may be saved (Acrobat allows it), after a yes.
      if (!force && s.check && !s.check.ok) {
        patch({ confirmSave: true });
        return;
      }
      const id = s.selected;
      const ok = await apply({ op: "setSource", id, source: s.text }, () => id);
      if (ok) patch({ confirmSave: false });
    },
    answerDiscard: (yes) => {
      const s = get().sheet;
      const to = s?.confirmDiscard;
      if (!s || to == null) return;
      if (!yes) {
        patch({ confirmDiscard: null });
        return;
      }
      if (to === "close") {
        patch({ text: s.saved });
        get().close();
      } else if (to === "add") {
        patch({ text: s.saved, confirmDiscard: null });
        void get().commitAdd();
      } else {
        void load(to);
      }
    },
    cancelConfirmSave: () => patch({ confirmSave: false }),
    startAdd: () => patch({ adding: "", renaming: null }),
    setAdding: (adding) => patch({ adding }),
    commitAdd: async () => {
      const s = get().sheet;
      if (!s || s.adding == null) return;
      // Adding selects the new script, which would drop unsaved typing: ask first.
      if (dirty(s)) {
        patch({ confirmDiscard: "add" });
        return;
      }
      const name = s.adding.trim();
      await apply({ op: "add", name: name === "" ? null : name, source: "" }, (r) => r.id);
    },
    startRename: () => {
      const s = get().sheet;
      if (!s?.selected || !isNamed(s.selected)) return;
      patch({ renaming: s.list?.named.find((n) => n.id === s.selected)?.name ?? "", adding: null });
    },
    setRenaming: (renaming) => patch({ renaming }),
    commitRename: async () => {
      const s = get().sheet;
      if (!s || s.renaming == null || !s.selected) return;
      const id = s.selected;
      const text = s.text;
      const was = s.saved;
      await apply({ op: "rename", id, name: s.renaming }, (r) => r.id ?? id);
      // The unsaved text stays with the script under its new name.
      if (text !== was && get().sheet?.selected && get().sheet?.saved === was) patch({ text });
    },
    remove: async () => {
      const s = get().sheet;
      if (!s || !s.selected) return;
      const id = s.selected;
      await apply({ op: "delete", id: isNamed(id) ? id : slotOf(id) }, () => null);
    },
    showInForms: () => {
      const s = get().sheet;
      if (!s || dirty(s)) return get().close();
      get().close();
      useForms.getState().setOpen(true);
    },
  };
});
