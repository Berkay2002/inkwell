/** The Tools menu in the toolbar: a plain list of items, grouped under headers, narrowed by a filter box. */

export interface ToolItem {
  id: string;
  /** What the menu shows; the filter matches on it. */
  label: string;
  /** One of `TOOL_GROUPS` ids. An id that is not listed there gets a header of its own at the end. */
  group: string;
  /** Extra words the filter also matches (so "ocr" finds "Recognize text…"). */
  keywords?: string;
  onSelect: () => void;
}

/** Header order and titles. Add a group here to give it a place; items may name any id. */
export const TOOL_GROUPS: { id: string; title: string }[] = [
  { id: "optimize", title: "Optimize" },
  { id: "marks", title: "Page marks" },
  { id: "protect", title: "Protect" },
  { id: "convert", title: "Convert & create" },
  { id: "check", title: "Check & compare" },
];

export interface ToolSection {
  id: string;
  title: string;
  items: ToolItem[];
}

const words = (s: string) =>
  s
    .toLowerCase()
    .replace(/…/g, " ")
    .split(/\s+/)
    .filter(Boolean);

/**
 * The items that match `query` (every word of it must appear in the label, the keywords or the group's
 * title), under their headers in `TOOL_GROUPS` order, items in the order given. A group with nothing left
 * has no header. An empty query keeps everything.
 */
export function filterTools(items: ToolItem[], query: string): ToolSection[] {
  const terms = words(query);
  const titleOf = new Map(TOOL_GROUPS.map((g) => [g.id, g.title]));
  const sections = new Map<string, ToolSection>(TOOL_GROUPS.map((g) => [g.id, { id: g.id, title: g.title, items: [] }]));
  for (const it of items) {
    const title = titleOf.get(it.group) ?? it.group;
    const hay = `${it.label} ${it.keywords ?? ""} ${title}`.toLowerCase().replace(/…/g, " ");
    if (!terms.every((t) => hay.includes(t))) continue;
    let s = sections.get(it.group);
    if (!s) {
      s = { id: it.group, title, items: [] };
      sections.set(it.group, s);
    }
    s.items.push(it);
  }
  return [...sections.values()].filter((s) => s.items.length > 0);
}
