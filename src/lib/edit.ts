import { getCurrentWindow } from "@tauri-apps/api/window";
import { message, open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useStore } from "../store";
import * as api from "./api";
import type { EditOp, EditState } from "./api";
import { formatRange } from "./pageRange";
import { dropCached } from "./tiles";

/**
 * Page edits for the UI: every call goes through the engine's edit lane (src-tauri/src/engine/edit.rs)
 * and the state it returns becomes the document's new generation here.
 */

const st = () => useStore.getState();
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const stem = (p: string) => baseName(p).replace(/\.pdf$/i, "");
const dirOf = (p: string) => p.slice(0, Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/")) + 1);
const PDF = [{ name: "PDF documents", extensions: ["pdf"] }];

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
  // Hits point at the old pages.
  if (revChanged && d.search.query) st().runSearch(id, d.search.query);
}

/** Edits of one document run in the order they were asked for. */
const queues = new Map<number, Promise<unknown>>();
function serial<T>(id: number, f: () => Promise<T>): Promise<T | undefined> {
  const run = (queues.get(id) ?? Promise.resolve()).then(f).catch((e) => {
    st().setUi({ error: String(e) });
    return undefined;
  });
  queues.set(id, run);
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

export const save = (id: number) => withState(id, () => api.saveDocument(id));

export async function saveAs(id: number) {
  const d = st().docs[id];
  if (!d) return;
  const path = await saveDialog({ defaultPath: d.path, filters: PDF });
  if (path) return withState(id, () => api.saveDocument(id, path));
}

/** Writes `pages` to a new file (asked for), optionally deleting them here as one undo step. */
export async function extract(id: number, pages: number[], deleteAfter = false) {
  const d = st().docs[id];
  if (!d || pages.length === 0) return;
  const label = formatRange(pages);
  const path = await saveDialog({ defaultPath: `${dirOf(d.path)}${stem(d.path)} (pages ${label}).pdf`, filters: PDF });
  if (!path) return;
  const s = await withState(id, () => api.extractPages(id, pages, path, deleteAfter));
  if (s) st().setUi({ error: `Saved ${pages.length === 1 ? "page" : "pages"} ${label} to ${baseName(path)}` });
}

/** `<name>_Part<n>.pdf` files of `every` pages next to the document. */
export async function split(id: number, every: number) {
  const r = await serial(id, () => api.splitDocument(id, every));
  if (r) st().setUi({ error: `Split into ${r.files.length} files next to ${st().docs[id]?.name ?? "the document"}` });
}

/** Inserts another PDF's pages at `at`. */
export async function insertFile(id: number, at: number) {
  const path = await open({ multiple: false, filters: PDF });
  if (typeof path === "string") return edit(id, { op: "insertFile", at, path });
}

/** Asks for PDFs and an output file, merges them in the order picked and opens the result. */
export async function mergeFiles(first?: string) {
  const picked = await open({ multiple: true, filters: PDF });
  const paths = [...(first ? [first] : []), ...(picked ?? [])];
  if (paths.length < 2) return;
  const out = await saveDialog({ defaultPath: `${dirOf(paths[0])}${stem(paths[0])} (merged).pdf`, filters: PDF });
  if (!out) return;
  try {
    await api.mergeFiles(paths, out);
    await st().openPath(out);
  } catch (e) {
    st().setUi({ error: String(e) });
  }
}

/** Save / Don't save / Cancel for a document with unsaved edits. True when it may close. */
async function settle(id: number): Promise<boolean> {
  const d = st().docs[id];
  if (!d?.dirty) return true;
  const r = await message(`Save the changes to ${d.name} before closing?`, {
    title: "Inkwell",
    kind: "warning",
    buttons: { yes: "Save", no: "Don't save", cancel: "Cancel" },
  });
  if (r === "Yes" || r === "Save") return (await save(id)) != null && !st().docs[id]?.dirty;
  return r === "No" || r === "Don't save";
}

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
      const dirty = st().order.filter((id) => st().docs[id]?.dirty);
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
