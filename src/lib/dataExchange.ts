import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useStore } from "../store";
import { type DocProtection, type EditState, type FileRef, type SaveKind, docProtection, pickSave } from "./api";
import { useAnnot } from "./annot";
import { applyState } from "./edit";
import { useForms } from "./forms";

/**
 * Comments (XFDF, FDF) and form data (FDF, XFDF, JSON) to and from files: src-tauri/src/annot/xfdf.rs and
 * forms/data.rs. The panels' menus and the command palette call these; every result is one toast.
 */

export interface CommentsSummary {
  added: number;
  updated: number;
  skipped: { reason: string; count: number }[];
}
export interface CommentsImported {
  summary: CommentsSummary;
  state: EditState;
  signed: boolean;
}
export interface CommentsExported {
  count: number;
  leftOut: number;
}
export interface FormSummary {
  set: number;
  unchanged: number;
  skipped: { name: string; reason: string }[];
  skippedTotal: number;
}
export interface FormImported {
  summary: FormSummary;
  state: EditState;
  signed: boolean;
}
export interface FormExported {
  count: number;
}

export const CERTIFIED_REASON = "This document is certified. Importing would void the certification, so Inkwell leaves it as it is.";
export const SIGNED_NOTE = "This document is signed. The change is saved as an update after the signature, which will show that the file was changed.";

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const stem = (n: string) => n.replace(/\.pdf$/i, "");

/** "Imported comments: 3 added, 1 updated, 2 skipped (the page is not in this document: 2)." */
export function commentsImportText(s: CommentsSummary, signed: boolean): string {
  const skipped = s.skipped.reduce((n, r) => n + r.count, 0);
  const parts: string[] = [];
  if (s.added) parts.push(`${s.added} added`);
  if (s.updated) parts.push(`${s.updated} updated`);
  if (skipped) parts.push(`${skipped} skipped (${s.skipped.map((r) => `${r.reason}: ${r.count}`).join("; ")})`);
  const head = parts.length ? `Imported comments: ${parts.join(", ")}.` : "This file has no comments to import.";
  return signed && (s.added || s.updated) ? `${head} ${SIGNED_NOTE}` : head;
}

/** "Imported form data: 6 fields filled, 1 already had the value, 3 skipped: A (reason); B (reason) and 1 more." */
export function formImportText(s: FormSummary, signed: boolean): string {
  const parts: string[] = [];
  if (s.set) parts.push(`${plural(s.set, "field")} filled`);
  if (s.unchanged) parts.push(`${s.unchanged} already had the value`);
  let head = parts.length ? `Imported form data: ${parts.join(", ")}` : "Imported form data: nothing to fill";
  if (s.skippedTotal) {
    const shown = s.skipped.slice(0, 3).map((k) => `${k.name} (${k.reason})`);
    const more = s.skippedTotal - shown.length;
    head += `, ${s.skippedTotal} skipped: ${shown.join("; ")}${more > 0 ? ` and ${more} more` : ""}`;
  }
  head += ".";
  return signed && s.set ? `${head} ${SIGNED_NOTE}` : head;
}

export function commentsExportText(r: CommentsExported, file: string): string {
  const left = r.leftOut ? ` ${plural(r.leftOut, "comment")} left out: measurements, and kinds the file cannot carry.` : "";
  return `Exported ${plural(r.count, "comment")} to ${file}.${left}`;
}

export function formExportText(r: FormExported, file: string): string {
  return `Exported ${plural(r.count, "field")} to ${file}.`;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const toast = (text: string) => useStore.getState().setUi({ error: text });
const nameOf = (doc: number) => useStore.getState().docs[doc]?.name ?? "document.pdf";

interface Exchange {
  /** Certified or signed, per document, once read (not known yet reads as neither: the engine refuses anyway). */
  locks: Record<number, DocProtection>;
  /** `fresh`: ask the engine again (a file can be signed or certified while the panel is open). */
  readLocks: (doc: number, fresh?: boolean) => Promise<DocProtection | null>;
  exportComments: (doc: number) => Promise<void>;
  importComments: (doc: number) => Promise<void>;
  exportForm: (doc: number) => Promise<void>;
  importForm: (doc: number) => Promise<void>;
}

export const useExchange = create<Exchange>((set, get) => {
  /** Whether an import may go on; a certified document says why it cannot (nothing is picked or read). */
  const mayImport = async (doc: number): Promise<boolean> => {
    const p = await get().readLocks(doc, true);
    if (p?.certified) {
      toast(CERTIFIED_REASON);
      return false;
    }
    return true;
  };

  const exportTo = async <T>(doc: number, suffix: string, kind: SaveKind, cmd: string, text: (r: T, file: string) => string) => {
    try {
      const to = await pickSave(`${stem(nameOf(doc))} ${suffix}.xfdf`, { doc }, kind);
      if (!to) return;
      const r = await invoke<T>(cmd, { doc, to: to.token });
      toast(text(r, baseName(to.path)));
    } catch (e) {
      toast(msg(e));
    }
  };

  const importFrom = async <T extends { state: EditState }>(doc: number, cmd: string, done: (r: T) => string, reread: (doc: number) => Promise<void>) => {
    try {
      if (!(await mayImport(doc))) return;
      const file = await invoke<FileRef | null>("pick_data_file", { doc });
      if (!file) return;
      const r = await invoke<T>(cmd, { doc, file: file.token });
      applyState(doc, r.state);
      await reread(doc);
      toast(done(r));
    } catch (e) {
      toast(msg(e));
    }
  };

  return {
    locks: {},
    readLocks: async (doc, fresh = false) => {
      const known = get().locks[doc];
      if (known && !fresh) return known;
      try {
        const p = await docProtection(doc);
        set((s) => ({ locks: { ...s.locks, [doc]: p } }));
        return p;
      } catch {
        // unreadable: the engine refuses a certified document's import itself
        return null;
      }
    },
    exportComments: (doc) => exportTo<CommentsExported>(doc, "comments", "comments", "annot_export", commentsExportText),
    importComments: (doc) =>
      importFrom<CommentsImported>(doc, "annot_import", (r) => commentsImportText(r.summary, r.signed), (d) => useAnnot.getState().refresh(d)),
    exportForm: (doc) => exportTo<FormExported>(doc, "form data", "formData", "forms_export", formExportText),
    importForm: (doc) =>
      importFrom<FormImported>(doc, "forms_import", (r) => formImportText(r.summary, r.signed), (d) => useForms.getState().refresh(d)),
  };
});
