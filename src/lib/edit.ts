import { getCurrentWindow } from "@tauri-apps/api/window";
import { message } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import * as api from "./api";
import type { EditOp, EditState } from "./api";
import { formatRange } from "./pageRange";
import { useAnnot } from "./annot";
import { useRedact } from "./redact";
import { useSign } from "./sign";
import { dropCached } from "./tiles";

/**
 * Page edits for the UI: every call goes through the engine's edit lane (src-tauri/src/engine/edit.rs)
 * and the state it returns becomes the document's new generation here.
 */

const st = () => useStore.getState();
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const stem = (p: string) => baseName(p).replace(/\.pdf$/i, "");

/** Makes the engine's state the document's: new generation, pages, sizes, labels. */
export function applyState(id: number, s: EditState) {
  const d = st().docs[id];
  if (!d) return;
  const revChanged = s.rev !== d.rev;
  if (revChanged) dropCached(`${id}:`);
  st().patch(id, {
    rev: s.rev,
    pageCount: s.pageCount,
    sizes: s.sizes,
    path: s.path,
    name: baseName(s.path),
    dirty: s.dirty,
    undoLabel: s.undo,
    redoLabel: s.redo,
    currentPage: Math.min(d.currentPage, s.pageCount - 1),
    ...(revChanged ? { outline: null } : {}),
  });
  // Undo and redo add and remove redaction marks.
  if (revChanged && useRedact.getState().docs[id]) void useRedact.getState().refreshMarks(id);
  // ... and comments: undo, redo and restoring a version change them too.
  if (revChanged && useAnnot.getState().docs[id]) void useAnnot.getState().refresh(id);
  // ... and Fill & Sign items.
  if (revChanged && useSign.getState().docs[id]) void useSign.getState().refresh(id);
  // Hits point at the old pages.
  if (revChanged && d.search.query) st().runSearch(id, d.search.query);
}

/** Edits of one document run in the order they were asked for. */
const queues = new Map<number, Promise<unknown>>();
function serial<T>(id: number, f: () => Promise<T>): Promise<T | undefined> {
  const run = (queues.get(id) ?? Promise.resolve()).then(f).catch((e) => {
    if (String(e) === "needs-permissions-password") {
      // Pages of a file that restricts copying or page changes leave only with its permissions
      // password: the Security view is where it is typed (and the restrictions removed).
      st().setUi({ error: "This file doesn't allow copying or page changes. Type its permissions password in Security to continue." });
      void import("./protectStore").then((m) => m.useProtect.getState().open("security", id));
    } else {
      st().setUi({ error: String(e) });
    }
    return undefined;
  });
  queues.set(id, run);
  return run;
}

/**
 * Runs `f` in the document's edit order like every edit, but hands its failure back to the caller
 * (a sheet that shows its own errors) instead of the shared error line.
 */
export function queued<T>(id: number, f: () => Promise<T>): Promise<T> {
  const run = (queues.get(id) ?? Promise.resolve()).then(f);
  queues.set(id, run.catch(() => undefined));
  return run;
}

function withState(id: number, f: () => Promise<EditState>) {
  return serial(id, async () => {
    const s = await f();
    applyState(id, s);
    return s;
  });
}

export const edit = (id: number, op: EditOp) => withState(id, () => api.editApply(id, op));
export const undo = (id: number) => withState(id, () => api.editUndo(id));
export const redo = (id: number) => withState(id, () => api.editRedo(id));

/** A created file that was never saved has no place yet: Save asks where it goes. */
export const save = (id: number) => (st().docs[id]?.scratch ? saveAs(id) : withState(id, () => api.saveDocument(id)));

export async function saveAs(id: number) {
  const d = st().docs[id];
  if (!d) return;
  const to = await api.pickSave(baseName(d.path), { doc: id });
  if (!to) return;
  const s = await withState(id, () => api.saveDocument(id, to.token));
  // Saved: the scratch copy is not needed any more.
  const scratch = st().docs[id]?.scratch;
  if (s && scratch) {
    st().patch(id, { scratch: null });
    void api.discardScratch(scratch).catch(() => {});
  }
  return s;
}

/** Writes `pages` to a new file (asked for), optionally deleting them here as one undo step. */
export async function extract(id: number, pages: number[], deleteAfter = false) {
  const d = st().docs[id];
  if (!d || pages.length === 0) return;
  const label = formatRange(pages);
  const out = await api.pickSave(`${stem(d.path)} (pages ${label}).pdf`, { doc: id });
  if (!out) return;
  const s = await withState(id, () => api.extractPages(id, pages, out.token, deleteAfter));
  if (s) st().setUi({ error: `Saved ${pages.length === 1 ? "page" : "pages"} ${label} to ${baseName(out.path)}` });
}

/** `<name>_Part<n>.pdf` files of `every` pages next to the document. */
export async function split(id: number, every: number) {
  const r = await serial(id, () => api.splitDocument(id, every));
  if (r) st().setUi({ error: `Split into ${r.files.length} files next to ${st().docs[id]?.name ?? "the document"}` });
}

/** Inserts another PDF's pages at `at`. */
export async function insertFile(id: number, at: number) {
  const [f] = await api.pickOpen(false);
  if (f) return edit(id, { op: "insertFile", at, file: f.token });
}

/** Asks for PDFs and an output file, merges them in the order picked and opens the result. */
export async function mergeFiles() {
  try {
    const files = await api.pickOpen(true);
    if (files.length < 2) return;
    const out = await api.pickSave(`${stem(files[0].path)} (merged).pdf`, { near: files[0].token });
    if (!out) return;
    const merged = await api.mergeFiles(
      files.map((f) => f.token),
      out.token,
    );
    await st().openPath(merged.path, merged.token);
  } catch (e) {
    st().setUi({ error: String(e) });
  }
}

/** Save / Don't save / Cancel for a document with unsaved edits. True when it may close. */
async function settle(id: number): Promise<boolean> {
  const d = st().docs[id];
  if (!d || (!d.dirty && !d.scratch)) return true;
  const r = await message(d.scratch ? `Save ${d.name} before closing? It has not been saved yet.` : `Save the changes to ${d.name} before closing?`, {
    title: "Inkwell",
    kind: "warning",
    buttons: { yes: "Save", no: "Don't save", cancel: "Cancel" },
  });
  if (r === "Yes" || r === "Save") return (await save(id)) != null && !st().docs[id]?.dirty;
  return r === "No" || r === "Don't save";
}

/** Tab closes and the window close button treat an unsaved created file like a document with edits. */
export const unsaved = (id: number) => st().docs[id]?.dirty || st().docs[id]?.scratch != null;

/** Closes the tab, asking first when it has unsaved edits. */
export async function requestClose(id: number) {
  if (await settle(id)) st().closeDoc(id);
}

/** The window's close button asks about every document with unsaved edits. */
export function guardWindowClose(): () => void {
  let off: (() => void) | undefined;
  let dead = false;
  getCurrentWindow()
    .onCloseRequested(async (e) => {
      const dirty = st().order.filter(unsaved);
      if (dirty.length === 0) return;
      e.preventDefault();
      for (const id of dirty) {
        st().activate(id);
        if (!(await settle(id))) return;
      }
      await getCurrentWindow().destroy();
    })
    .then((u) => (dead ? u() : (off = u)))
    .catch(() => {});
  return () => {
    dead = true;
    off?.();
  };
}
