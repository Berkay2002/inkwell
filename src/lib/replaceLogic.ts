import type { ReplaceResult } from "./api";

// What Find & Replace tells the person after a run. Pure: the store (replace.ts) runs the engine.

/** Skipped matches shown in the list before "and N more". */
export const LISTED = 50;

export interface ReplaceReport {
  /** "5 replaced, 2 skipped", with ", stopped early" when the run did not reach the end. */
  summary: string;
  /** One line per skipped match: "p. 3, "aaa": skipped: it is set in a Type 3 font ..." */
  lines: string[];
  /** Skipped matches the engine counted but did not list, plus those past `LISTED`. */
  more: number;
  /** Why it stopped early, or the font that stood in for a missing glyph. */
  notes: string[];
  /** The document generation the run left behind; the report is about that one only. */
  rev?: number;
}

const quote = (t: string) => `"${t.length > 40 ? `${t.slice(0, 40)}…` : t}"`;

export function reportOf(r: ReplaceResult): ReplaceReport {
  const listed = r.skipped.slice(0, LISTED);
  const notes: string[] = [];
  if (r.stopped) notes.push(r.stopped);
  // Edit text's own yellow "!": the font that stood in, once each.
  for (const f of r.fallbacks) notes.push(`${f.font} was used for some text: ${f.reason}.`);
  return {
    summary: `${r.replaced} replaced, ${r.skippedCount} skipped${r.stopped ? ", stopped early" : ""}`,
    lines: listed.map((s) => `p. ${s.page + 1}, ${quote(s.text)}: ${s.reason}`),
    more: Math.max(0, r.skippedCount - listed.length),
    notes,
  };
}

/** The report while the document is still the one the run left (an undo or another edit makes it history). */
export function reportAt(report: ReplaceReport | null | undefined, rev: number): ReplaceReport | null {
  return report && report.rev === rev ? report : null;
}

/** Why Replace is unavailable, or null when it can run. */
export function whyNotReplace(opts: { locked: string | null; query: string; hits: number; searching: boolean; current: "text" | "other" | null }, all: boolean): string | null {
  if (opts.locked) return opts.locked;
  if (!opts.query.trim()) return "Type what to find first";
  if (opts.hits === 0) return opts.searching ? "Still searching" : "No matches to replace";
  if (all) return opts.searching ? "Wait for the search to finish" : null;
  if (opts.current === null) return "Pick a match first";
  if (opts.current === "other") return "Only text on the page can be replaced, not comments, form fields or bookmarks";
  return null;
}
