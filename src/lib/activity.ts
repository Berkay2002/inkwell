import type { Activity, Citation } from "./ai";

/** A source badge or row is clickable only when the quote was really found on its page. */
export const isVerified = (c: Citation) => c.exact && c.page != null;

const SHOWN = 6;

/** The quiet one-liner of a turn's tool calls: `Searched "Ciri" · Read pp. 40-52 · Went to p. 41`.
 *  Same-label repeats collapse; a rejected or failed call says so. */
export function trailSummary(acts: Activity[]): string {
  const parts: string[] = [];
  for (const a of acts) {
    const s = a.state === "rejected" ? `${a.label} (rejected)` : a.state === "failed" ? `${a.label} (failed)` : a.label;
    if (parts[parts.length - 1] !== s) parts.push(s);
  }
  return parts.length > SHOWN ? `${parts.slice(0, SHOWN).join(" · ")} · +${parts.length - SHOWN} more` : parts.join(" · ");
}

/** The assistant changed redaction marks or applied a redaction this turn: the message offers
 *  "Review in Redact". Listing marks, a rejected call and a failed one do not count. */
export const touchedRedaction = (acts: Activity[] | undefined) =>
  (acts ?? []).some((a) => a.tool.startsWith("redact_") && a.class !== "read" && a.state === "done");

/** Calls the user must still decide on, oldest first. */
export const waiting = (acts: Activity[] | undefined) => (acts ?? []).filter((a) => a.state === "waiting");
