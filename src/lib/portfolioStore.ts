import { ask } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { useStore } from "../store";
import * as api from "./api";
import type { PortfolioFile, PortfolioInfo, PortfolioSortKey } from "./api";
import { nextSort, openAction, typeLabel } from "./portfolioLogic";

const st = () => useStore.getState();

/** One document's portfolio view state. */
export interface PortfolioDoc {
  /** None: the document is not a portfolio (or its collection was refused). */
  info: PortfolioInfo | null;
  /** The reader's own sort; null shows the collection's. */
  sort: PortfolioSortKey[] | null;
  open: number[];
  selected: string | null;
  /** The cover page (the normal page view) is showing instead of the file list. */
  cover: boolean;
}

interface Store {
  docs: Record<number, PortfolioDoc>;
  /** Reads the collection again (after a change); the first read decides the view. */
  load: (doc: number) => Promise<void>;
  toggleSort: (doc: number, field: number) => void;
  resetSort: (doc: number) => void;
  toggleFolder: (doc: number, folder: number) => void;
  select: (doc: number, file: string | null) => void;
  showCover: (doc: number, on: boolean) => void;
}

const patch = (set: (f: (s: Store) => Partial<Store>) => void, doc: number, p: Partial<PortfolioDoc>) =>
  set((s) => (s.docs[doc] ? { docs: { ...s.docs, [doc]: { ...s.docs[doc], ...p } } } : {}));

export const usePortfolio = create<Store>((set, get) => ({
  docs: {},
  load: async (doc) => {
    let info: PortfolioInfo | null = null;
    let error: string | null = null;
    try {
      info = await api.portfolioInfo(doc);
    } catch (e) {
      error = String(e);
    }
    if (!st().docs[doc]) return;
    const had = get().docs[doc];
    set((s) => {
      // Documents that are gone take their state with them.
      const docs: Record<number, PortfolioDoc> = {};
      for (const [k, v] of Object.entries(s.docs)) if (st().docs[Number(k)]) docs[Number(k)] = v;
      const keep = had?.info && info ? had : null;
      docs[doc] = {
        info,
        sort: keep?.sort ?? null,
        open: keep?.open ?? [],
        selected: keep && info?.files.some((f) => f.id === keep.selected) ? keep.selected : null,
        // The first look shows the files, unless the author asked for no portfolio view.
        cover: keep ? keep.cover : info?.view === "hidden",
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
  select: (doc, file) => patch(set, doc, { selected: file }),
  showCover: (doc, on) => patch(set, doc, { cover: on }),
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
