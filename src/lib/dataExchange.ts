import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useStore } from "../store";
import { type EditState, type FileRef, type SaveKind, pickSave } from "./api";
import { useAnnot } from "./annot";
import { applyState } from "./edit";
import { useForms } from "./forms";

/**
 * Comments (XFDF, FDF) and form data (FDF, XFDF, JSON, XML, CSV, tab text) to and from files: src-tauri/src/annot/xfdf.rs
 * and forms/data.rs. The panels' menus and the command palette call these; every result is one toast. A CSV or tab
 * text file with several records asks which one first (`picker`, shown by components/forms/RecordPicker.tsx).
 */

export interface CommentsSummary {
  added: number;
  updated: number;
  skipped: { reason: string; count: number }[];
  /** Comments made without a part of the file (its appearance, rich text or attached file), with how many each. */
  notes?: { reason: string; count: number }[];
  /** Comments that now show the appearance the file gave, and attached files that were added. */
  appearances?: number;
  files?: number;
}
export interface CommentsImported {
  summary: CommentsSummary;
  state: EditState;
  signed: boolean;
}
export interface CommentsExported {
  count: number;
  leftOut: number;
  /** Text entries left empty for being over 1 MB, which a file Inkwell reads cannot hold. */
  longText?: number;
  /** Appearances (of picture stamps) and attached files written. */
  appearances?: number;
  files?: number;
  /** Parts that could not be written, with how many each. */
  notes?: { reason: string; count: number }[];
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
/** What a data file holds: more than one record asks the person which to import (labels are empty for one). */
export interface FormRecords {
  count: number;
  labels: string[];
  /** The line of each record in the file, as a spreadsheet numbers rows (the header is 1). */
  rows: number[];
}
export interface FormExported {
  count: number;
  /** Password fields: their values are never written to a file. */
  leftOut?: number;
  /** Fields left out for a value over 1 MB, which a file Inkwell reads cannot hold. */
  longText?: number;
}

/** What each import may do to a document (engine `data_gates`): a certification's `/P` 1 allows neither, 2 form data only, 3 both. */
export interface Gates {
  comments: boolean;
  forms: boolean;
  signed: boolean;
}

export const COMMENTS_CERTIFIED =
  "This document is certified, and its certification does not allow comments. Importing them would void it, so Inkwell leaves the document as it is.";
export const FORMS_CERTIFIED =
  "This document is certified, and its certification does not allow filling in form fields. Importing form data would void it, so Inkwell leaves the document as it is.";
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
  if (s.appearances) parts.push(`${plural(s.appearances, "appearance")} from the file`);
  if (s.files) parts.push(`${plural(s.files, "attached file")}`);
  const kept = (s.notes ?? []).reduce((n, r) => n + r.count, 0);
  if (kept) parts.push(`${kept} made without part of the file (${s.notes?.map((r) => `${r.reason}: ${r.count}`).join("; ")})`);
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
    head += `, ${s.skippedTotal} skipped`;
    if (shown.length) head += `: ${shown.join("; ")}${more > 0 ? ` and ${more} more` : ""}`;
  }
  head += ".";
  return signed && s.set ? `${head} ${SIGNED_NOTE}` : head;
}

export function commentsExportText(r: CommentsExported, file: string): string {
  const left = r.leftOut ? ` ${plural(r.leftOut, "comment")} left out: measurements, attachments over their limit, and kinds the file cannot carry.` : "";
  const long = r.longText ? ` ${plural(r.longText, "text")} longer than 1 MB left empty.` : "";
  const carried = [r.appearances ? plural(r.appearances, "appearance") : "", r.files ? plural(r.files, "attached file") : ""].filter(Boolean);
  const wrote = carried.length ? ` Includes ${carried.join(" and ")}.` : "";
  const notes = r.notes?.length ? ` Not written: ${r.notes.map((n) => `${n.reason}: ${n.count}`).join("; ")}.` : "";
  return `Exported ${plural(r.count, "comment")} to ${file}.${wrote}${left}${long}${notes}`;
}

export function formExportText(r: FormExported, file: string): string {
  const left = r.leftOut ? ` ${plural(r.leftOut, "password field")} left out.` : "";
  const long = r.longText ? ` ${plural(r.longText, "field")} with a value over 1 MB left out.` : "";
  return `Exported ${plural(r.count, "field")} to ${file}.${left}${long}`;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const toast = (text: string) => useStore.getState().setUi({ error: text });
const nameOf = (doc: number) => useStore.getState().docs[doc]?.name ?? "document.pdf";

interface Exchange {
  /** What each import may do, per document, once read (not known yet reads as allowed: the engine refuses anyway). */
  gates: Record<number, Gates>;
  /** `fresh`: ask the engine again (a file can be signed or certified while the panel is open). */
  readGates: (doc: number, fresh?: boolean) => Promise<Gates | null>;
  exportComments: (doc: number) => Promise<void>;
  importComments: (doc: number) => Promise<void>;
  exportForm: (doc: number) => Promise<void>;
  importForm: (doc: number) => Promise<void>;
  /** The data file is a table with several records: the person picks one (null while nothing is asked). */
  picker: { name: string; labels: string[]; rows: number[] } | null;
  /** The person's answer to `picker`: a record (0 is the first under the header), or null to cancel. */
  chooseRecord: (row: number | null) => void;
}

/** Answers the question `picker` shows; one at a time. */
let answer: ((row: number | null) => void) | null = null;

export const useExchange = create<Exchange>((set, get) => {
  /** Whether an import may go on; a certification that does not allow it says why (nothing is picked or read). */
  const mayImport = async (doc: number, kind: "comments" | "forms"): Promise<boolean> => {
    const g = await get().readGates(doc, true);
    if (g && !g[kind]) {
      toast(kind === "comments" ? COMMENTS_CERTIFIED : FORMS_CERTIFIED);
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

  /** The record of a table file to import: asks when there are several. `undefined` is "cancelled". */
  const recordOf = async (file: FileRef): Promise<{ row: number | null } | undefined> => {
    const recs = await invoke<FormRecords>("forms_import_records", { file: file.token });
    if (recs.count <= 1) return { row: null };
    const row = await new Promise<number | null>((resolve) => {
      answer?.(null);
      answer = resolve;
      set({ picker: { name: baseName(file.path), labels: recs.labels, rows: recs.rows } });
    });
    return row === null ? undefined : { row };
  };

  const importFrom = async <T extends { state: EditState }>(
    doc: number,
    kind: "comments" | "forms",
    cmd: string,
    done: (r: T) => string,
    reread: (doc: number) => Promise<void>,
    extra?: (file: FileRef) => Promise<Record<string, unknown> | undefined>,
  ) => {
    try {
      if (!(await mayImport(doc, kind))) return;
      const file = await invoke<FileRef | null>("pick_data_file", { doc });
      if (!file) return;
      const more = extra ? await extra(file) : {};
      if (!more) return;
      const r = await invoke<T>(cmd, { doc, file: file.token, ...more });
      applyState(doc, r.state);
      await reread(doc);
      toast(done(r));
    } catch (e) {
      toast(msg(e));
    }
  };

  return {
    gates: {},
    picker: null,
    chooseRecord: (row) => {
      const resolve = answer;
      answer = null;
      set({ picker: null });
      resolve?.(row);
    },
    readGates: async (doc, fresh = false) => {
      const known = get().gates[doc];
      if (known && !fresh) return known;
      try {
        const g = await invoke<Gates>("data_gates", { doc });
        set((s) => ({ gates: { ...s.gates, [doc]: g } }));
        return g;
      } catch {
        // unreadable: the engine refuses the import itself
        return null;
      }
    },
    exportComments: (doc) => exportTo<CommentsExported>(doc, "comments", "comments", "annot_export", commentsExportText),
    importComments: (doc) =>
      importFrom<CommentsImported>(doc, "comments", "annot_import", (r) => commentsImportText(r.summary, r.signed), (d) => useAnnot.getState().refresh(d)),
    exportForm: (doc) => exportTo<FormExported>(doc, "form data", "formData", "forms_export", formExportText),
    importForm: (doc) =>
      importFrom<FormImported>(doc, "forms", "forms_import", (r) => formImportText(r.summary, r.signed), (d) => useForms.getState().refresh(d), recordOf),
  };
});
