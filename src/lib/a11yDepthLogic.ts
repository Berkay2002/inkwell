/**
 * Pure decisions for the Reading order tool, the Tags tree and the Matterhorn view of the accessibility
 * rail (src-tauri/src/a11y/order.rs, tags.rs). No state and no IPC here, so it can be tested alone.
 */
import type { Rect4, Report, RuleResult, Status } from "./a11yLogic";
import { isOverflowNote } from "./a11yLogic";

// ---- reading order ---------------------------------------------------------------------------------

/** One region of a page's reading order (`a11y_reading_order`). */
export interface Region {
  /** The object number every edit takes; 0 for an element written directly inside its parent (not editable). */
  xref: number;
  role: string;
  rawRole: string;
  depth: number;
  parent: number;
  mcids: number[];
  annots: number[];
  bbox: Rect4 | null;
  /** The start of its text, up to 120 characters. */
  text: string;
  alt: boolean;
  editable: boolean;
}

/** Marked content no element points at: nothing reads it. */
export interface Orphan {
  mcid: number;
  bbox: Rect4 | null;
  text: string;
}

export interface ReadingOrder {
  page: number;
  tagged: boolean;
  regions: Region[];
  orphans: Orphan[];
}

/** What `a11y_reorder` takes: every region the engine can address, once, in the order to read them. */
export const orderOf = (regions: Region[]): number[] => regions.filter((r) => r.xref !== 0).map((r) => r.xref);

/** `order` with `xref` moved to index `to` (the others keep their order); null when nothing changes. */
export function moveTo(order: number[], xref: number, to: number): number[] | null {
  const from = order.indexOf(xref);
  if (from < 0 || to < 0 || to >= order.length || to === from) return null;
  const next = order.slice();
  next.splice(from, 1);
  next.splice(to, 0, xref);
  return next;
}

/** One step up (-1) or down (1); null at the ends. */
export function moveStep(order: number[], xref: number, dir: -1 | 1): number[] | null {
  const i = order.indexOf(xref);
  return i < 0 ? null : moveTo(order, xref, i + dir);
}

/** Dragging `xref` onto `target`: it takes the target's place (the target shifts one up or down). */
export function dropOnto(order: number[], xref: number, target: number): number[] | null {
  const to = order.indexOf(target);
  return to < 0 || xref === target ? null : moveTo(order, xref, to);
}

/** The selected regions in list order, when they are next to each other in the list and can all be edited. */
export function mergeRun(regions: Region[], selected: number[]): { into: number; from: number[] } | null {
  if (selected.length < 2) return null;
  const at = regions.map((r, i) => (selected.includes(r.xref) && r.xref !== 0 ? i : -1)).filter((i) => i >= 0);
  if (at.length !== selected.length) return null;
  if (at[at.length - 1] - at[0] !== at.length - 1) return null;
  const run = at.map((i) => regions[i]);
  if (run.some((r) => !r.editable)) return null;
  return { into: run[0].xref, from: run.slice(1).map((r) => r.xref) };
}

/** Why a region can't be split, or null when it can. */
export function splitBlocked(r: Region): string | null {
  if (!r.editable || r.xref === 0) return "This element is written inside its parent and can't be edited.";
  if (r.mcids.length < 2) return "This region holds one piece of content, so there is nothing to split.";
  return null;
}

/** Why a region can't be marked as an artifact, or null when it can. */
export function artifactBlocked(r: Region): string | null {
  return r.editable && r.xref !== 0 ? null : "This element is written inside its parent and can't be edited.";
}

/** The regions between two list positions, both included (Shift+click and Shift+arrow). */
export function rangeOf(regions: Region[], a: number, b: number): number[] {
  const ia = regions.findIndex((r) => r.xref === a);
  const ib = regions.findIndex((r) => r.xref === b);
  if (ia < 0 || ib < 0) return [a];
  const [from, to] = ia <= ib ? [ia, ib] : [ib, ia];
  return regions.slice(from, to + 1).map((r) => r.xref).filter((x) => x !== 0);
}

const ROLE_WORD: Record<string, string> = {
  P: "Paragraph",
  Figure: "Figure",
  Table: "Table",
  TR: "Table row",
  TH: "Table header",
  TD: "Table cell",
  L: "List",
  LI: "List item",
  Lbl: "List label",
  LBody: "List body",
  Link: "Link",
  Form: "Form field",
  Span: "Span",
  Div: "Division",
  Sect: "Section",
  Art: "Article",
  Caption: "Caption",
  Quote: "Quote",
  BlockQuote: "Block quote",
  Note: "Note",
  Code: "Code",
  Formula: "Formula",
  Annot: "Annotation",
  TOC: "Table of contents",
  TOCI: "Contents entry",
  Document: "Document",
  H: "Heading",
};

/** "Heading 2", "Paragraph", or the type as written. */
export function roleWord(role: string): string {
  const h = /^H([1-6])$/.exec(role);
  if (h) return `Heading ${h[1]}`;
  return ROLE_WORD[role] ?? role;
}

/** What a screen reader would say for each region, in order: the "read it to me" preview. */
export function readAloud(regions: Region[]): string[] {
  return regions.map((r) => {
    const t = r.text.trim();
    if (r.role === "Figure") return r.alt ? "Figure with a description." : "Figure, no description.";
    const say = r.role === "P" || r.role === "Span" || r.role === "Div" ? "" : `${roleWord(r.role)}: `;
    return t ? `${say}${t}` : `${roleWord(r.role)}, no text.`;
  });
}

/** The region whose box holds the point, the smallest one when several do; null on none. */
export function regionAt(regions: Region[], x: number, y: number, skip?: number): Region | null {
  let best: Region | null = null;
  let area = Infinity;
  for (const r of regions) {
    if (!r.bbox || r.xref === 0 || r.xref === skip) continue;
    const [x0, y0, x1, y1] = r.bbox;
    if (x < x0 || x > x1 || y < y0 || y > y1) continue;
    const a = (x1 - x0) * (y1 - y0);
    if (a < area) {
      best = r;
      area = a;
    }
  }
  return best;
}

// ---- the tag tree ----------------------------------------------------------------------------------

export interface TagNode {
  xref: number;
  role: string;
  rawRole: string;
  page: number | null;
  alt: string | null;
  actualText: string | null;
  lang: string | null;
  id: string | null;
  scope: string | null;
  headers: number[];
  mcids: number[];
  annots: number[];
  editable: boolean;
  children: TagNode[];
}

/** What `a11y_edit_tag` takes: a field left out stays, an empty text removes it. */
export interface TagPatch {
  role?: string;
  alt?: string;
  actualText?: string;
  lang?: string;
  scope?: string;
  headers?: number[];
}

/** Where the tag tree and the ParentTree disagree (`a11y_tag_problems`). */
export interface Problem {
  page: number | null;
  xref: number | null;
  message: string;
}

/** A node's identity in the tree: its object number, or its place when it is written inside its parent. */
export const nodeKey = (n: TagNode, path: string) => (n.xref > 0 ? `x${n.xref}` : `p${path}`);

export interface Row {
  node: TagNode;
  key: string;
  depth: number;
  /** The key and object number of the parent (0 at the top of the tree). */
  parentKey: string | null;
  parent: number;
  index: number;
  siblings: number;
  open: boolean;
}

/** The rows to show: only the children of expanded nodes are visited, so a closed branch costs nothing. */
export function flatten(tree: TagNode[], expanded: Record<string, boolean>): Row[] {
  const out: Row[] = [];
  const walk = (nodes: TagNode[], depth: number, parentKey: string | null, parent: number, path: string) => {
    nodes.forEach((node, index) => {
      const p = path ? `${path}.${index}` : `${index}`;
      const key = nodeKey(node, p);
      const open = expanded[key] === true && node.children.length > 0;
      out.push({ node, key, depth, parentKey, parent, index, siblings: nodes.length, open });
      if (open) walk(node.children, depth + 1, key, node.xref, p);
    });
  };
  walk(tree, 0, null, 0, "");
  return out;
}

export function findNode(tree: TagNode[], xref: number): TagNode | null {
  for (const n of tree) {
    if (n.xref === xref) return n;
    const k = findNode(n.children, xref);
    if (k) return k;
  }
  return null;
}

/** The parent (null at the top) and the index of a node, found by object number. */
export function locate(tree: TagNode[], xref: number, parent: TagNode | null = null): { parent: TagNode | null; index: number; siblings: TagNode[] } | null {
  for (let i = 0; i < tree.length; i++) {
    if (tree[i].xref === xref) return { parent, index: i, siblings: tree };
    const k = locate(tree[i].children, xref, tree[i]);
    if (k) return k;
  }
  return null;
}

export type TagMove = "up" | "down" | "in" | "out";

/** Where a tag goes: under `parent` (0: the top of the tree), beside its kid `anchor` (null: as the last kid). */
export interface MoveTo {
  parent: number;
  anchor: number | null;
  after: boolean;
}

/**
 * What `a11y_move_tag(xref, parent, anchor, after)` takes for a step, or null when the step goes nowhere.
 * The place is named by a sibling, not an index: the parent's /K also holds content items, which the tree
 * does not list, and a page-scoped tree lists only some of the kids. A step that would have to be placed
 * by a node with no object number is refused rather than turned into some other place.
 */
export function moveTarget(tree: TagNode[], xref: number, how: TagMove): MoveTo | null {
  const at = locate(tree, xref);
  if (!at) return null;
  const node = at.siblings[at.index];
  if (!node.editable || xref === 0) return null;
  const parent = at.parent?.xref ?? 0;
  if (at.parent && parent <= 0) return null;
  switch (how) {
    case "up": {
      // Before the nearest sibling above that has an object number.
      const above = at.siblings.slice(0, at.index).reverse().find((s) => s.xref > 0);
      return above ? { parent, anchor: above.xref, after: false } : null;
    }
    case "down": {
      const below = at.siblings.slice(at.index + 1).find((s) => s.xref > 0);
      return below ? { parent, anchor: below.xref, after: true } : null;
    }
    case "in": {
      // Into the sibling above, as its last kid.
      const above = at.siblings[at.index - 1];
      return above && above.editable && above.xref > 0 ? { parent: above.xref, anchor: null, after: false } : null;
    }
    case "out": {
      // Next to the parent, right after it.
      if (!at.parent || at.parent.xref <= 0) return null;
      const up = locate(tree, at.parent.xref);
      return up ? { parent: up.parent?.xref ?? 0, anchor: at.parent.xref, after: true } : null;
    }
  }
}

export function subtreeXrefs(n: TagNode, into: Set<number> = new Set()): Set<number> {
  if (n.xref > 0) into.add(n.xref);
  for (const c of n.children) subtreeXrefs(c, into);
  return into;
}

/** The first page a node or its descendants hold content on. */
export function tagPage(n: TagNode): number | null {
  if (n.page !== null && (n.mcids.length > 0 || n.annots.length > 0)) return n.page;
  for (const c of n.children) {
    const p = tagPage(c);
    if (p !== null) return p;
  }
  return n.page;
}

/** The box of a node's content on a page: the regions of that page that are the node or inside it. */
export function tagBounds(n: TagNode, regions: Region[]): Rect4 | null {
  const mine = subtreeXrefs(n);
  let out: Rect4 | null = null;
  for (const r of regions) {
    if (!r.bbox || !mine.has(r.xref)) continue;
    out = out ? [Math.min(out[0], r.bbox[0]), Math.min(out[1], r.bbox[1]), Math.max(out[2], r.bbox[2]), Math.max(out[3], r.bbox[3])] : [...r.bbox];
  }
  return out;
}

/** Messages by object number, for the badge on a row. */
export function problemsByXref(problems: Problem[]): Map<number, string[]> {
  const m = new Map<number, string[]>();
  for (const p of problems) {
    if (p.xref === null) continue;
    m.set(p.xref, [...(m.get(p.xref) ?? []), p.message]);
  }
  return m;
}

/** The standard structure types the role select offers (any type in the role map is accepted by the engine too). */
export const ROLES = [
  "P", "H1", "H2", "H3", "H4", "H5", "H6", "H", "Figure", "Caption", "Table", "THead", "TBody", "TFoot", "TR", "TH", "TD",
  "L", "LI", "Lbl", "LBody", "Link", "Form", "Formula", "Quote", "BlockQuote", "Note", "Code", "Span", "Div", "Sect", "Art",
  "Part", "Document", "TOC", "TOCI", "Artifact",
];

export function roleOptions(current: string): string[] {
  return ROLES.includes(current) ? ROLES : [current, ...ROLES];
}

export const SCOPES = ["Row", "Column", "Both"] as const;

/** What the tag editor has typed; text fields as strings, the scope as a name or "". */
export interface TagDraft {
  role: string;
  alt: string;
  actualText: string;
  lang: string;
  scope: string;
  headers: number[];
}

export function draftOf(n: TagNode): TagDraft {
  return {
    role: n.rawRole || n.role,
    alt: n.alt ?? "",
    actualText: n.actualText ?? "",
    lang: n.lang ?? "",
    scope: n.scope ?? "",
    headers: n.headers,
  };
}

/** Only what changed, or null when nothing did. An emptied text is sent as "", which removes it. */
export function tagPatch(n: TagNode, d: TagDraft): TagPatch | null {
  const was = draftOf(n);
  const p: TagPatch = {};
  if (d.role.trim() !== was.role) p.role = d.role.trim();
  if (d.alt.trim() !== was.alt) p.alt = d.alt.trim();
  if (d.actualText.trim() !== was.actualText) p.actualText = d.actualText.trim();
  if (d.lang.trim() !== was.lang) p.lang = d.lang.trim();
  if (n.role === "TH" && d.scope !== was.scope && d.scope !== "") p.scope = d.scope;
  const same = d.headers.length === was.headers.length && d.headers.every((h) => was.headers.includes(h));
  if (!same) p.headers = d.headers;
  return Object.keys(p).length ? p : null;
}

/** The header cells (TH) of the table a cell sits in: what its `headers` can name. */
export function headerCandidates(tree: TagNode[], xref: number): TagNode[] {
  const path: TagNode[] = [];
  const find = (nodes: TagNode[]): boolean => {
    for (const n of nodes) {
      path.push(n);
      if (n.xref === xref || find(n.children)) return true;
      path.pop();
    }
    return false;
  };
  if (!find(tree)) return [];
  const table = [...path].reverse().find((n) => n.role === "Table");
  if (!table) return [];
  const out: TagNode[] = [];
  const walk = (n: TagNode) => {
    if (n.role === "TH" && n.xref > 0 && n.xref !== xref) out.push(n);
    n.children.forEach(walk);
  };
  walk(table);
  return out.slice(0, 100);
}

/** The label of a row: the role, then what is in it. */
export function nodeLabel(n: TagNode): string {
  const extra = n.alt ? ` · ${n.alt}` : n.actualText ? ` · ${n.actualText}` : "";
  return `${n.rawRole || n.role}${extra}`;
}

// ---- Matterhorn checkpoints ------------------------------------------------------------------------

/**
 * The 31 checkpoints of the Matterhorn Protocol 1.1 (PDF/UA-1), as numbered in its table of contents.
 * The report has no checkpoint field, so each rule is placed under one by the table below.
 */
export const CHECKPOINTS: Record<string, string> = {
  "01": "Real content tagged",
  "02": "Role mapping",
  "03": "Flickering",
  "04": "Color and contrast",
  "05": "Sound",
  "06": "Metadata",
  "07": "Dictionary",
  "08": "OCR validation",
  "09": "Appropriate tags",
  "10": "Character mappings",
  "11": "Declared natural language",
  "12": "Stretchable characters",
  "13": "Graphics",
  "14": "Headings",
  "15": "Tables",
  "16": "Lists",
  "17": "Mathematical expressions",
  "18": "Page headers and footers",
  "19": "Notes and references",
  "20": "Optional content",
  "21": "Embedded files",
  "22": "Article threads",
  "23": "Digital signatures",
  "24": "Non-interactive forms",
  "25": "XFA",
  "26": "Security",
  "27": "Navigation",
  "28": "Annotations",
  "29": "Actions",
  "30": "XObjects",
  "31": "Fonts",
};

/** The bucket for checks that are not a numbered Matterhorn condition. */
export const OTHER = "other";

/** Rule id -> checkpoint number, and the failure conditions of that checkpoint the rule decides (docs/a11y.md). */
export const RULE_CHECKPOINT: Record<string, { cp: string; conditions: string[] }> = {
  "tagged-pdf": { cp: "01", conditions: ["01-007"] },
  "content-marking": { cp: "01", conditions: ["01-003", "01-004", "01-005"] },
  "role-map": { cp: "02", conditions: ["02-001", "02-003", "02-004"] },
  // Contrast is always a manual check and reading order is judged by a heuristic: neither decides a condition.
  "color-contrast": { cp: "04", conditions: [] },
  // The title rule reads /Title only, not dc:title (06-003, which `metadata` decides).
  title: { cp: "06", conditions: [] },
  metadata: { cp: "06", conditions: ["06-001", "06-002", "06-003"] },
  "display-doc-title": { cp: "07", conditions: ["07-001", "07-002"] },
  "reading-order": { cp: "09", conditions: [] },
  "list-structure": { cp: "09", conditions: ["09-005"] },
  "table-structure": { cp: "09", conditions: ["09-004"] },
  "fonts-unicode": { cp: "10", conditions: ["10-001", "31-027"] },
  "primary-language": { cp: "11", conditions: ["11-001"] },
  "language-values": { cp: "11", conditions: [] },
  "figure-alt-text": { cp: "13", conditions: ["13-004"] },
  "heading-nesting": { cp: "14", conditions: ["14-002", "14-003", "14-007"] },
  "table-headers": { cp: "15", conditions: [] },
  "table-scope": { cp: "15", conditions: ["15-003"] },
  "optional-content": { cp: "20", conditions: ["20-001", "20-002", "20-003"] },
  xfa: { cp: "25", conditions: ["25-001"] },
  "accessibility-permission": { cp: "26", conditions: ["26-001", "26-002"] },
  "tagged-links": { cp: "28", conditions: ["28-011"] },
  "annotations-tagged": { cp: "28", conditions: ["28-002", "28-004", "28-010"] },
  "tab-order": { cp: "28", conditions: ["28-008", "28-009"] },
  "field-tooltips": { cp: "28", conditions: ["28-005"] },
  "fonts-embedded": { cp: "31", conditions: ["31-009"] },
  // Acrobat's check and the ISO 32000-1 14.7 link check: no numbered Matterhorn condition.
  bookmarks: { cp: OTHER, conditions: [] },
  "parent-tree": { cp: OTHER, conditions: [] },
};

export interface MhGroup {
  /** "01".."31", or null for the checks that are not a numbered condition. */
  number: string | null;
  title: string;
  /** The worst of its rules: failed, then manual, then passed, then skipped. */
  status: Status;
  rules: RuleResult[];
  /** How many findings the failed rules report ("... and n more" counted). */
  count: number;
  /** The failure conditions its rules decide, ascending. */
  conditions: string[];
}

const WORST: Status[] = ["failed", "manual", "passed", "skipped"];

/** The findings a failed rule reports, with the engine's "... and n more" note counted in. */
export function ruleCount(x: RuleResult): number {
  if (x.status !== "failed") return 0;
  let n = 0;
  let more = 0;
  for (const f of x.findings) {
    if (isOverflowNote(f)) more += Number(/(\d+)/.exec(f.message)?.[1] ?? 0);
    else n++;
  }
  return Math.max(1, n + more);
}

/** The results grouped by Matterhorn checkpoint, in protocol order, the unnumbered checks last. */
export function matterhornGroups(r: Report): MhGroup[] {
  const by = new Map<string, RuleResult[]>();
  for (const x of r.results) {
    const cp = RULE_CHECKPOINT[x.rule]?.cp ?? OTHER;
    by.set(cp, [...(by.get(cp) ?? []), x]);
  }
  const keys = [...by.keys()].sort((a, b) => (a === OTHER ? 1 : b === OTHER ? -1 : a.localeCompare(b)));
  return keys.map((cp) => {
    const rules = (by.get(cp) ?? []).slice().sort((a, b) => WORST.indexOf(a.status) - WORST.indexOf(b.status));
    const conditions = [...new Set(rules.flatMap((x) => RULE_CHECKPOINT[x.rule]?.conditions ?? []))].sort();
    return {
      number: cp === OTHER ? null : cp,
      title: cp === OTHER ? "Other checks (no numbered Matterhorn condition)" : CHECKPOINTS[cp],
      status: rules.reduce<Status>((w, x) => (WORST.indexOf(x.status) < WORST.indexOf(w) ? x.status : w), "skipped"),
      rules,
      count: rules.reduce((n, x) => n + ruleCount(x), 0),
      conditions,
    };
  });
}

/** The checkpoints no rule of the checker covers: a person has to look (or Inkwell does not check them yet). */
export function uncheckedCheckpoints(groups: MhGroup[]): { number: string; title: string }[] {
  const have = new Set(groups.map((g) => g.number));
  return Object.entries(CHECKPOINTS)
    .filter(([n]) => !have.has(n))
    .map(([number, title]) => ({ number, title }));
}
