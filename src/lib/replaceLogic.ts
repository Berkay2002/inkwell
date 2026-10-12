import type { ReplaceFrom, ReplaceResult, ReplaceSkipped } from "./api";

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
  /** Where Replace all goes on from when the run stopped early. */
  resume?: ReplaceFrom | null;
  /** The replacement holds the find text, so a second run would grow what the first one wrote. */
  grows?: boolean;
  /** The request it answers (find, replacement, options): a go-on is only for the same one. */
  ask?: string;
}

const quote = (t: string) => `"${t.length > 40 ? `${t.slice(0, 40)}…` : t}"`;

/** The hit with a few words of what surrounds it, so rows for the same word can be told apart. */
const inContext = (s: ReplaceSkipped) => `${s.before ? `…${s.before.trimStart()}` : ""}${quote(s.text)}${s.after ? `${s.after.trimEnd()}…` : ""}`;

export function reportOf(r: ReplaceResult): ReplaceReport {
  const listed = r.skipped.slice(0, LISTED);
  const notes: string[] = [];
  if (r.stopped) notes.push(r.stopped);
  // Edit text's own yellow "!": the font that stood in, once each.
  for (const f of r.fallbacks) notes.push(`${f.font} was used for some text: ${f.reason}.`);
  return {
    summary: `${r.replaced} replaced, ${r.skippedCount} skipped${r.stopped ? ", stopped early" : ""}`,
    lines: listed.map((s) => `p. ${s.page + 1}, ${inContext(s)}: ${s.reason}`),
    more: Math.max(0, r.skippedCount - listed.length),
    notes,
    resume: r.resume,
    grows: r.inserted > 0,
  };
}

/** The report while the document is still the one the run left (an undo or another edit makes it history). */
export function reportAt(report: ReplaceReport | null | undefined, rev: number): ReplaceReport | null {
  return report && report.rev === rev ? report : null;
}

/** Where Replace all goes on from: the stopped run of this same request, on the document it left. */
export function resumeOf(report: ReplaceReport | null | undefined, rev: number, ask: string): ReplaceFrom | undefined {
  return report && report.rev === rev && report.ask === ask ? (report.resume ?? undefined) : undefined;
}

/** The signature banner's height when it has not been measured yet (one line of text). */
export const BANNER_HEIGHT = 40;

/**
 * Top offsets (px) of the Find bar, the Replace row and the report under them. The banner sits at 12 px in the
 * same strip; when it shows (`banner` is its height) they go 6 px below its bottom edge, the row and the report at
 * their usual distance from the bar.
 */
export function barTops(banner: number | null): { find: number; replace: number; report: number } {
  const find = banner == null ? 12 : 12 + Math.ceil(banner) + 6;
  return { find, replace: find + 46, report: find + 94 };
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
