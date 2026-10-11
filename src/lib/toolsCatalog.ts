import type { IconName } from "../components/Icons";

/** One tool, as the All tools panel, the Home grid, the toolbar's pins and the command palette show it. */
export interface ToolItem {
  id: string;
  /** The name: short, no trailing ellipsis. */
  label: string;
  /** One of `TOOL_GROUPS` ids. An id that is not listed there gets a header of its own at the end. */
  group: string;
  icon: IconName;
  /** One plain line saying what it is for ("Make the file smaller to email or upload"). */
  description: string;
  /** A fuller sentence for the hover card; falls back to `description`. */
  detail?: string;
  /** Synonyms the search also matches, as plain words ("shrink smaller reduce size"). */
  keywords?: string;
  /** Id of the command (lib/commands.ts) that does the same: its shortcut shows on the card and its palette entry gets `description`. */
  command?: string;
  /** Runs the tool on the document it was built for. */
  onSelect: () => void;
}

/** Header order and titles. Add a group here to give it a place; items may name any id. */
export const TOOL_GROUPS: { id: string; title: string }[] = [
  { id: "optimize", title: "Optimize" },
  { id: "marks", title: "Page marks" },
  { id: "protect", title: "Protect" },
  { id: "sign", title: "Fill & sign" },
  { id: "convert", title: "Convert & create" },
  { id: "check", title: "Check & compare" },
];

export interface ToolSection {
  id: string;
  title: string;
  items: ToolItem[];
}

/** Section id of the ranked list a search returns. */
export const RESULTS = "results";

const groupTitle = (id: string) => TOOL_GROUPS.find((g) => g.id === id)?.title ?? id;

/** Lower-case words, with a plain plural / -ing / -ed taken off so "numbers" finds "numbering". */
export function tokens(s: string): string[] {
  return s
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean)
    .map((w) => {
      if (w.length > 5 && w.endsWith("ing")) return w.slice(0, -3);
      if (w.length > 4 && w.endsWith("ed")) return w.slice(0, -2);
      if (w.length > 3 && w.endsWith("s") && !w.endsWith("ss")) return w.slice(0, -1);
      return w;
    });
}

/** How well `term` matches an item: 0 when it does not match at all. Name beats synonym beats description. */
function termScore(it: ToolItem, term: string): number {
  const name = tokens(it.label);
  if (name.some((w) => w.startsWith(term))) return name[0]?.startsWith(term) ? 12 : 10;
  if (tokens(it.keywords ?? "").some((w) => w.startsWith(term))) return 6;
  if (tokens(groupTitle(it.group)).some((w) => w.startsWith(term))) return 3;
  if (tokens(it.description).some((w) => w.startsWith(term))) return 2;
  return 0;
}

/** Items matching every word of `query` (name, synonyms, group title or description), best first, catalog order for ties. */
export function searchTools(items: ToolItem[], query: string): ToolItem[] {
  const terms = tokens(query);
  if (terms.length === 0) return items;
  const scored: { it: ToolItem; score: number; at: number }[] = [];
  items.forEach((it, at) => {
    let score = 0;
    for (const t of terms) {
      const s = termScore(it, t);
      if (s === 0) return;
      score += s;
    }
    scored.push({ it, score, at });
  });
  return scored.sort((a, b) => b.score - a.score || a.at - b.at).map((s) => s.it);
}

/** Items under their headers in `TOOL_GROUPS` order (items in the order given); a group with nothing in it has no header. */
export function groupTools(items: ToolItem[]): ToolSection[] {
  const sections = new Map<string, ToolSection>(TOOL_GROUPS.map((g) => [g.id, { id: g.id, title: g.title, items: [] }]));
  for (const it of items) {
    let s = sections.get(it.group);
    if (!s) {
      s = { id: it.group, title: groupTitle(it.group), items: [] };
      sections.set(it.group, s);
    }
    s.items.push(it);
  }
  return [...sections.values()].filter((s) => s.items.length > 0);
}

/** What the panel lists: the groups when the box is empty, otherwise one ranked "Results" section. */
export function filterTools(items: ToolItem[], query: string): ToolSection[] {
  if (tokens(query).length === 0) return groupTools(items);
  const hits = searchTools(items, query);
  return hits.length === 0 ? [] : [{ id: RESULTS, title: "Results", items: hits }];
}

export const MAX_RECENT = 4;
export const MAX_PINNED = 3;

/** `id` moved to the front of the recent list, without a duplicate, at most `max` long. */
export function pushRecent(list: string[], id: string, max = MAX_RECENT): string[] {
  return [id, ...list.filter((x) => x !== id)].slice(0, max);
}

/** Pins or unpins `id`. Pinning a fourth tool is refused (`full`) rather than dropping an older pin. */
export function togglePin(list: string[], id: string, max = MAX_PINNED): { pinned: string[]; full: boolean } {
  if (list.includes(id)) return { pinned: list.filter((x) => x !== id), full: false };
  if (list.length >= max) return { pinned: list, full: true };
  return { pinned: [...list, id], full: false };
}

/** A list read back from storage: strings only, no duplicates, at most `max`. */
export function cleanIds(v: unknown, max: number): string[] {
  if (!Array.isArray(v)) return [];
  return [...new Set(v.filter((x): x is string => typeof x === "string"))].slice(0, max);
}

/** `ids` without the ones that are not in `known` (a tool another build renamed or removed): no ghost pins or chips. */
export function pruneIds(ids: string[], known: Iterable<string>): string[] {
  const ok = new Set(known);
  return ids.filter((id) => ok.has(id));
}

/** The items for ids, in that order; ids that are not tools (a tool another branch removed) are skipped. */
export function pick(items: ToolItem[], ids: string[]): ToolItem[] {
  return ids.flatMap((id) => items.find((i) => i.id === id) ?? []);
}
