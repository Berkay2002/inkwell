import { ask } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { PortfolioFile, PortfolioInfo, PortfolioSortKey } from "./api";
import { defaultLayout, type Layout, nextSort, openAction, typeLabel } from "./portfolioLogic";
import { dropThumbs } from "./portfolioThumb";

const st = () => useStore.getState();

/** One document's portfolio view state. */
export interface PortfolioDoc {
  /** None: the document is not a portfolio (or its collection was refused). */
  info: PortfolioInfo | null;
  /** The reader's own sort; null shows the collection's. */
  sort: PortfolioSortKey[] | null;
  open: number[];
  /** A file's id, or `d:<id>` for a folder row (`rowKey`). */
  selected: string | null;
  /** The cover page (the normal page view) is showing instead of the file list. */
  cover: boolean;
  /** Details or Tiles: the collection's /View until the person picks one. For this session only, never written. */
  layout: Layout;
  /** The preview pane beside the list is showing. */
  pane: boolean;
  /** The folder the tiles are in (null: the top level). */
  folder: number | null;
}

interface Store {
  docs: Record<number, PortfolioDoc>;
  /** Reads the collection again (after a change); the first read decides the view. */
  load: (doc: number) => Promise<void>;
  toggleSort: (doc: number, field: number) => void;
  resetSort: (doc: number) => void;
  toggleFolder: (doc: number, folder: number) => void;
  select: (doc: number, key: string | null) => void;
  showCover: (doc: number, on: boolean) => void;
  setLayout: (doc: number, layout: Layout) => void;
  togglePane: (doc: number) => void;
  /** Tiles: go into a folder (null: the top). */
  enterFolder: (doc: number, folder: number | null) => void;
}

/** The newest read asked for of each document: an older answer that arrives late is dropped. */
const asked = new Map<number, number>();

const patch = (set: (f: (s: Store) => Partial<Store>) => void, doc: number, p: Partial<PortfolioDoc>) =>
  set((s) => (s.docs[doc] ? { docs: { ...s.docs, [doc]: { ...s.docs[doc], ...p } } } : {}));

export const usePortfolio = create<Store>((set, get) => ({
  docs: {},
  load: async (doc) => {
    const seq = (asked.get(doc) ?? 0) + 1;
    asked.set(doc, seq);
    let info: PortfolioInfo | null = null;
    let error: string | null = null;
    try {
      info = await api.portfolioInfo(doc);
    } catch (e) {
      error = String(e);
    }
    if (asked.get(doc) !== seq) return;
    if (!st().docs[doc]) {
      asked.delete(doc);
      return;
    }
    const had = get().docs[doc];
    // The files may have changed: the pictures are drawn again when a tile or the pane asks.
    dropThumbs(doc);
    set((s) => {
      // Documents that are gone take their state with them.
      const docs: Record<number, PortfolioDoc> = {};
      for (const [k, v] of Object.entries(s.docs)) {
        if (st().docs[Number(k)]) docs[Number(k)] = v;
        else dropThumbs(Number(k));
      }
      const keep = had?.info && info ? had : null;
      const has = (key: string | null) => !!key && !!info && (info.files.some((f) => f.id === key) || info.folders.some((f) => `d:${f.id}` === key));
      // The first look selects the author's initial document, with the folders around it open.
      const first = !keep && info?.initial ? info.files.find((f) => f.id === info.initial) : undefined;
      const open = [...(keep?.open ?? [])];
      for (let p = first?.folder ?? null, n = 0; p != null && n < 33; n++) {
        if (!open.includes(p)) open.push(p);
        p = info?.folders.find((f) => f.id === p)?.parent ?? null;
      }
      docs[doc] = {
        info,
        sort: keep?.sort ?? null,
        open,
        selected: keep ? (has(keep.selected) ? keep.selected : null) : (first?.id ?? null),
        // The first look shows the files, unless the author asked for no portfolio view.
        cover: keep ? keep.cover : info?.view === "hidden",
        // What the person chose stays through every re-read (an attachment added, a file removed).
        layout: keep?.layout ?? defaultLayout(info?.view ?? "details"),
        pane: keep?.pane ?? false,
        folder: keep ? (keep.folder != null && info?.folders.some((f) => f.id === keep.folder) ? keep.folder : null) : (first?.folder ?? null),
      };
      return { docs };
    });
    // A refused collection is said once, then the document is a plain one.
    if (error && !had) st().setUi({ error });
  },
  toggleSort: (doc, field) => {
    const d = get().docs[doc];
    if (!d?.info) return;
    patch(set, doc, { sort: nextSort(d.sort ?? d.info.sort, field) });
  },
  resetSort: (doc) => patch(set, doc, { sort: null }),
  toggleFolder: (doc, folder) => {
    const d = get().docs[doc];
    if (d) patch(set, doc, { open: d.open.includes(folder) ? d.open.filter((f) => f !== folder) : [...d.open, folder] });
  },
  select: (doc, key) => patch(set, doc, { selected: key }),
  showCover: (doc, on) => patch(set, doc, { cover: on }),
  setLayout: (doc, layout) => {
    const d = get().docs[doc];
    if (!d) return;
    // Tiles open in the folder of what is selected, so the selection is still in view.
    let folder = d.folder;
    if (layout === "tiles" && d.info && d.selected) {
      const file = d.info.files.find((f) => f.id === d.selected);
      const dir = d.selected.startsWith("d:") ? d.info.folders.find((f) => `d:${f.id}` === d.selected) : undefined;
      if (file) folder = file.folder;
      else if (dir) folder = dir.parent;
    }
    patch(set, doc, { layout, folder });
  },
  togglePane: (doc) => {
    const d = get().docs[doc];
    if (d) patch(set, doc, { pane: !d.pane });
  },
  enterFolder: (doc, folder) => patch(set, doc, { folder, selected: null }),
}));

const say = (m: string) => st().setUi({ error: m });
const fail = (e: unknown) => say(String(e));
const folderOf = (path: string) => path.split(/[\\/]/).slice(-2, -1)[0] ?? "the folder";

/** A PDF opens in a tab of its own (as an unsaved copy); nothing else is ever opened. */
export async function openFile(doc: number, file: PortfolioFile) {
  try {
    const f = await api.attachmentOpen(doc, file.id);
    await st().openPath(f.path, f.token, true);
  } catch (e) {
    fail(e);
  }
}

/** Saves one file where the person says. Anything but a PDF asks first, naming the file and its type. */
export async function extractFile(doc: number, file: PortfolioFile) {
  try {
    if (!file.isPdf) {
      const ok = await ask(`Save ${file.name} to a folder?\n\nType: ${typeLabel(file)}\n\nInkwell never opens or runs it; it is only written to the file you choose.`, {
        title: "Extract file",
        kind: "warning",
        okLabel: "Choose where…",
        cancelLabel: "Cancel",
      });
      if (!ok) return;
    }
    const out = await api.pickSave(file.name, { doc }, "any");
    if (!out) return;
    const path = await api.attachmentSave(doc, file.id, out.token);
    say(`Saved ${file.name} to ${folderOf(path)}`);
  } catch (e) {
    fail(e);
  }
}

/** The action of Open / Enter / double-click on a file. */
export const activate = (doc: number, file: PortfolioFile) => (openAction(file) === "open" ? openFile(doc, file) : extractFile(doc, file));

/** Writes every file into a folder the person picks; a taken name gets " (2)". */
export async function extractAll(doc: number) {
  try {
    const to = await api.pickFolder(doc);
    if (!to) return;
    const r = await api.portfolioExtractAll(doc, to.token);
    const n = r.written.length;
    const left = r.skipped.length ? `, ${r.skipped.length} left out (${r.skipped[0]})` : "";
    say(`Extracted ${n} ${n === 1 ? "file" : "files"} to ${r.folder.split(/[\\/]/).filter(Boolean).pop() ?? "the folder"}${left}`);
  } catch (e) {
    fail(e);
  }
}

/** Create PDF Portfolio…: the files, then where to save it; the result opens in a tab. */
export async function createPortfolio() {
  try {
    const files = await api.pickPortfolioFiles();
    if (files.length === 0) return;
    const out = await api.pickSave("Portfolio.pdf", { near: files[0].token });
    if (!out) return;
    const made = await api.portfolioCreate(
      files.map((f) => f.token),
      out.token,
    );
    await st().openPath(made.path, made.token);
  } catch (e) {
    fail(e);
  }
}
