import { Channel, invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useStore } from "../store";
import { type EditState, pickSave, toolCancel } from "./api";
import { applyState, run } from "./edit";
import { type Finding, type Fix, type Preview, type Progress, type Rect4, type Report, safeFixes, stem, suggestLang } from "./a11yLogic";
import {
  type Problem,
  type ReadingOrder,
  type TagMove,
  type TagNode,
  type TagPatch,
  findNode,
  mergeRun,
  moveStep,
  moveTarget,
  orderOf,
  rangeOf,
  tagBounds,
  tagPage,
  dropOnto,
} from "./a11yDepthLogic";

/**
 * The accessibility checker for the UI (src-tauri/src/a11y/app.rs). The check and every fix run
 * on their own threads with progress and cancel; a fix is one undo step, and the report is read
 * again after any change of the document (a fix, an undo, an edit).
 */

export const a11yApi = {
  check: (job: number, doc: number, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<Report>("a11y_check", { job, doc, onProgress: ch });
  },
  fix: (job: number, doc: number, fix: Fix, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<EditState>("a11y_fix", { job, doc, fix, onProgress: ch });
  },
  preview: (doc: number) => invoke<Preview>("a11y_preview", { doc }),
  readingOrder: (doc: number, page: number) => invoke<ReadingOrder>("a11y_reading_order", { doc, page }),
  tags: (doc: number, page: number | null) => invoke<TagNode[]>("a11y_tags", { doc, page }),
  tagProblems: (doc: number) => invoke<Problem[]>("a11y_tag_problems", { doc }),
  reorder: (doc: number, page: number, order: number[]) => invoke<EditState>("a11y_reorder", { doc, page, order }),
  mergeMany: (doc: number, into: number, from: number[]) => invoke<EditState>("a11y_merge_many", { doc, into, from }),
  split: (doc: number, xref: number, atMcid: number) => invoke<EditState>("a11y_split_region", { doc, xref, atMcid, role: null }),
  artifact: (doc: number, xref: number) => invoke<EditState>("a11y_artifact", { doc, xref }),
  setRole: (doc: number, xref: number, role: string) => invoke<EditState>("a11y_set_role", { doc, xref, role }),
  editTag: (doc: number, xref: number, patch: TagPatch) => invoke<EditState>("a11y_edit_tag", { doc, xref, patch }),
  moveTag: (doc: number, xref: number, parent: number, anchor: number | null, after: boolean) =>
    invoke<EditState>("a11y_move_tag", { doc, xref, parent, anchor, after }),
  exportReport: (job: number, doc: number, format: "html" | "pdf", date: string, to: string, onProgress: (p: Progress) => void) => {
    const ch = new Channel<Progress>();
    ch.onmessage = onProgress;
    return invoke<string>("a11y_export_report", { job, doc, format, date, to, onProgress: ch });
  },
};

export type Phase = "idle" | "checking" | "fixing" | "autotagging" | "exporting";

export interface Shown {
  page: number;
  rect: Rect4 | null;
  /** Restarts the highlight's flash. */
  nonce: number;
}

/** The rail's views: the checker by Acrobat's groups, by Matterhorn checkpoint, the Reading order tool and the Tags tree. */
export type View = "check" | "matterhorn" | "order" | "tags";

/** The Reading order tool's state for one document. */
export interface OrderState {
  /** The page and generation `data` was read for. */
  page: number | null;
  rev: number | null;
  data: ReadingOrder | null;
  /** Object numbers of the selected regions. */
  selected: number[];
  /** Where Shift+click and Shift+arrow ranges start. */
  anchor: number | null;
  loading: boolean;
  error: string | null;
}

/** The Tags tree's state for one document. */
export interface TagsState {
  rev: number | null;
  /** The page the tree was read for; null for the whole document. */
  page: number | null;
  /** null: chosen by the size of the document. */
  scope: "doc" | "page" | null;
  tree: TagNode[] | null;
  problems: Problem[];
  /** Expanded nodes by key; a closed branch is never rendered. */
  expanded: Record<string, boolean>;
  /** The selected node's key. */
  selected: string | null;
  loading: boolean;
  error: string | null;
}

const freshOrder = (): OrderState => ({ page: null, rev: null, data: null, selected: [], anchor: null, loading: false, error: null });
const freshTags = (): TagsState => ({ rev: null, page: null, scope: null, tree: null, problems: [], expanded: {}, selected: null, loading: false, error: null });

/** A document with more pages than this opens its tag tree for the current page only (the whole tree is one IPC reply). */
export const TREE_PAGES = 100;

export interface DocA11y {
  report: Report | null;
  /** The document generation the report was read from. */
  checkedRev: number | null;
  phase: Phase;
  progress: Progress | null;
  job: number;
  /** The revision whose check was cancelled: it is not started again by itself (Check again does). */
  skipRev: number | null;
  /** The revision a check failed on, so a failing check is not retried in a loop; an edit moves on. */
  errorRev: number | null;
  /** What Autotag would do; read when the report says the document is untagged. */
  preview: Preview | null;
  /** The person dismissed the Autotag offer for now. */
  notNow: boolean;
  error: string | null;
  /** Where the last report export went. */
  exported: string | null;
  shown: Shown | null;
  /** Groups collapsed by name. */
  collapsed: Record<string, boolean>;
  order: OrderState;
  tags: TagsState;
}

const fresh = (): DocA11y => ({
  report: null,
  checkedRev: null,
  phase: "idle",
  progress: null,
  job: 0,
  skipRev: null,
  errorRev: null,
  preview: null,
  notNow: false,
  error: null,
  exported: null,
  shown: null,
  collapsed: {},
  order: freshOrder(),
  tags: freshTags(),
});

interface A11yStore {
  open: boolean;
  setOpen: (open: boolean) => void;
  view: View;
  setView: (view: View) => void;
  /** Language the Primary language fix and "Fix all" set. */
  lang: string;
  setLang: (lang: string) => void;
  docs: Record<number, DocA11y>;
  get: (doc: number) => DocA11y;
  check: (doc: number) => Promise<void>;
  /** One fix, one undo step. */
  fix: (doc: number, fix: Fix) => Promise<boolean>;
  fixAll: (doc: number) => Promise<void>;
  autotag: (doc: number) => Promise<void>;
  cancel: (doc: number) => void;
  exportReport: (doc: number, format: "html" | "pdf") => Promise<void>;
  /** Jumps to the finding's page and highlights it. */
  show: (doc: number, f: Finding) => void;
  clearShown: (doc: number) => void;
  toggleGroup: (doc: number, name: string) => void;
  /** Opens or closes one group by name (the Matterhorn view opens the failed ones by itself). */
  setGroupClosed: (doc: number, name: string, closed: boolean) => void;
  dismissAutotag: (doc: number) => void;
  forget: (doc: number) => void;

  // ---- Reading order tool
  /** Reads the page's regions again unless they are already read for this page and generation. */
  loadOrder: (doc: number, page: number, force?: boolean) => Promise<void>;
  selectRegion: (doc: number, xref: number, how: "one" | "toggle" | "range") => void;
  /** Applies a new order of the page's regions: one undo step. */
  reorder: (doc: number, next: number[]) => Promise<boolean>;
  moveRegion: (doc: number, xref: number, dir: -1 | 1) => Promise<boolean>;
  dropRegion: (doc: number, xref: number, target: number) => Promise<boolean>;
  mergeSelected: (doc: number) => Promise<boolean>;
  splitRegion: (doc: number, xref: number, atMcid: number) => Promise<boolean>;
  artifactRegion: (doc: number, xref: number) => Promise<boolean>;

  // ---- Tags tree
  loadTags: (doc: number, force?: boolean) => Promise<void>;
  setTagScope: (doc: number, scope: "doc" | "page") => void;
  toggleTag: (doc: number, key: string) => void;
  /** Selects a node and shows its content on the page. */
  selectTag: (doc: number, key: string | null) => Promise<void>;
  setRole: (doc: number, xref: number, role: string) => Promise<boolean>;
  editTag: (doc: number, xref: number, patch: TagPatch) => Promise<boolean>;
  moveTag: (doc: number, xref: number, how: TagMove) => Promise<boolean>;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));
const CANCELLED = "cancelled";

let jobSeq = 1_900_000_000;
let nonce = 0;
const st = () => useStore.getState();

export const useA11y = create<A11yStore>((set, get) => {
  // A progress or completion that arrives after the tab closed must not bring its entry back.
  const patch = (doc: number, p: Partial<DocA11y>) => {
    if (!st().docs[doc]) return;
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? fresh()), ...p } } }));
  };
  /** The check each document is waiting for; an older answer is dropped. */
  const latest = new Map<number, number>();
  /** The same for the page's regions and the tag tree. */
  const latestOrder = new Map<number, number>();
  const latestTags = new Map<number, number>();
  let seq = 0;
  const patchOrder = (doc: number, p: Partial<OrderState>) => patch(doc, { order: { ...get().get(doc).order, ...p } });
  const patchTags = (doc: number, p: Partial<TagsState>) => patch(doc, { tags: { ...get().get(doc).tags, ...p } });
  const busyPhase = (doc: number) => {
    const ph = get().get(doc).phase;
    return ph === "fixing" || ph === "autotagging" || ph === "exporting";
  };

  /**
   * One edit of the tag structure, queued with the document's other edits. It holds the document like a
   * fix does (a check still running is stopped; the report is read again when the revision changes). A
   * refusal is shown where the person is and the view is read again, so the page snaps back.
   */
  async function edit(doc: number, f: () => Promise<EditState>, fail: (m: string) => void, reload: () => Promise<void>): Promise<boolean> {
    if (!st().docs[doc] || busyPhase(doc)) return false;
    const cur = get().get(doc);
    if (cur.phase === "checking") {
      latest.delete(doc);
      void toolCancel(cur.job).catch(() => {});
    }
    patch(doc, { phase: "fixing", progress: null, job: ++jobSeq });
    try {
      await run(doc, f);
      // Read the view again before the document is given back, so the next edit is never planned from
      // what this one changed.
      await reload();
      patch(doc, { phase: "idle" });
      return true;
    } catch (e) {
      patch(doc, { phase: "idle" });
      // Read it again first (that clears the last message), then say why.
      await reload();
      if (msg(e) !== CANCELLED) fail(msg(e));
      return false;
    }
  }
  /**
   * Whether the cached regions are those of the document as it is now. An undo or a fix moves the
   * revision before the view has read it again; a plan made from older regions would be stored as a step.
   * When they are stale the page is read again and the caller does nothing.
   */
  const orderReady = (doc: number) => {
    const d = st().docs[doc];
    const o = get().get(doc).order;
    if (!d || !o.data || o.page === null) return false;
    if (o.loading || o.rev !== d.rev) {
      if (!o.loading) void get().loadOrder(doc, o.page, true);
      return false;
    }
    return true;
  };
  const tagsReady = (doc: number) => {
    const d = st().docs[doc];
    const t = get().get(doc).tags;
    if (!d || !t.tree) return false;
    if (t.loading || t.rev !== d.rev) {
      if (!t.loading) void get().loadTags(doc, true);
      return false;
    }
    return true;
  };
  const editOrder = (doc: number, f: () => Promise<EditState>) =>
    edit(doc, f, (m) => patchOrder(doc, { error: m }), async () => {
      const p = get().get(doc).order.page;
      if (p !== null) await get().loadOrder(doc, p, true);
    });
  const editTags = (doc: number, f: () => Promise<EditState>) =>
    edit(doc, f, (m) => patchTags(doc, { error: m }), () => get().loadTags(doc, true));

  async function loadPreview(doc: number, r: Report) {
    if (r.tagged) return patch(doc, { preview: null });
    try {
      const p = await a11yApi.preview(doc);
      if (get().docs[doc]) patch(doc, { preview: p });
    } catch {
      patch(doc, { preview: null });
    }
  }

  return {
    open: false,
    setOpen: (open) => {
      // Opening the rail is asking for a check again, so a cancel from before is forgotten.
      if (open) {
        set((s) => ({
          open,
          docs: Object.fromEntries(Object.entries(s.docs).map(([id, d]) => [id, d.skipRev === null ? d : { ...d, skipRev: null }])),
        }));
      } else {
        set({ open });
        for (const [id, d] of Object.entries(get().docs)) if (d.phase === "checking") get().cancel(Number(id));
      }
    },
    view: "check",
    setView: (view) => set({ view }),
    lang: suggestLang(typeof navigator === "undefined" ? undefined : navigator.language),
    setLang: (lang) => set({ lang }),
    docs: {},
    get: (doc) => get().docs[doc] ?? fresh(),

    check: async (doc) => {
      const d = st().docs[doc];
      if (!d) return;
      const busy = get().get(doc);
      // Fixes and exports hold the document; the report is read again when they are done.
      if (busy.phase === "fixing" || busy.phase === "autotagging" || busy.phase === "exporting") return;
      if (busy.phase === "checking") void toolCancel(busy.job).catch(() => {});
      const job = ++jobSeq;
      latest.set(doc, job);
      const rev = d.rev;
      patch(doc, { phase: "checking", progress: null, job, error: null, skipRev: null, errorRev: null });
      // Only a check that still owns the phase gives it back (an export may have taken it meanwhile).
      const done = () => (get().get(doc).phase === "checking" ? ({ phase: "idle", progress: null } as const) : {});
      try {
        const report = await a11yApi.check(job, doc, (p) => latest.get(doc) === job && patch(doc, { progress: p }));
        if (latest.get(doc) !== job) return;
        patch(doc, { report, checkedRev: rev, shown: null, ...done() });
        await loadPreview(doc, report);
      } catch (e) {
        if (latest.get(doc) !== job) return;
        patch(doc, { ...done(), ...(msg(e) === CANCELLED ? {} : { error: msg(e), errorRev: rev }) });
      }
    },

    fix: async (doc, fix) => {
      const cur = get().get(doc);
      if (cur.phase === "fixing" || cur.phase === "autotagging" || cur.phase === "exporting") return false;
      // A check that is still running reads the document the fix is about to change.
      if (cur.phase === "checking") {
        latest.delete(doc);
        void toolCancel(cur.job).catch(() => {});
      }
      const job = ++jobSeq;
      const auto = fix.fix === "autotag";
      patch(doc, { phase: auto ? "autotagging" : "fixing", progress: null, job, error: null });
      try {
        const state = await a11yApi.fix(job, doc, fix, (p) => patch(doc, { progress: p }));
        applyState(doc, state);
        patch(doc, { phase: "idle", progress: null });
        return true;
      } catch (e) {
        patch(doc, { phase: "idle", progress: null, ...(msg(e) === CANCELLED ? {} : { error: msg(e) }) });
        // The report on screen is still right unless a check was stopped to make room for this fix.
        const now = get().get(doc);
        if (!now.report || now.checkedRev !== st().docs[doc]?.rev) void get().check(doc);
        return false;
      }
    },

    fixAll: async (doc) => {
      const r = get().get(doc).report;
      if (!r) return;
      const fixes = safeFixes(r, get().lang);
      if (!fixes.length) return;
      await get().fix(doc, fixes.length === 1 ? fixes[0] : { fix: "many", fixes });
    },

    autotag: async (doc) => {
      await get().fix(doc, { fix: "autotag" });
    },

    cancel: (doc) => {
      const d = get().get(doc);
      if (d.phase === "idle") return;
      void toolCancel(d.job).catch(() => {});
      // A cancelled check says so itself; the others come back with "cancelled".
      if (d.phase === "checking") {
        latest.delete(doc);
        patch(doc, { phase: "idle", progress: null, skipRev: st().docs[doc]?.rev ?? null });
      }
    },

    exportReport: async (doc, format) => {
      const d = st().docs[doc];
      if (!d) return;
      const cur = get().get(doc);
      if (cur.phase !== "idle") return;
      try {
        const to = await pickSave(`${stem(d.name)} Accessibility Report.${format}`, { doc }, format === "pdf" ? "pdf" : "html");
        if (!to) return;
        // The dialog was open a while: a check may have started, or a fix. The export reads the document
        // itself, so a check gives way; a fix does not.
        const now = get().get(doc);
        if (!st().docs[doc]) return;
        if (now.phase === "fixing" || now.phase === "autotagging" || now.phase === "exporting") {
          return patch(doc, { error: "Wait for the current step to finish, then export the report again." });
        }
        if (now.phase === "checking") {
          latest.delete(doc);
          void toolCancel(now.job).catch(() => {});
        }
        const job = ++jobSeq;
        patch(doc, { phase: "exporting", progress: null, job, error: null, exported: null });
        const date = new Date().toLocaleDateString("en-GB", { day: "numeric", month: "long", year: "numeric" });
        const path = await a11yApi.exportReport(job, doc, format, date, to.token, (p) => patch(doc, { progress: p }));
        patch(doc, { phase: "idle", progress: null, exported: path });
      } catch (e) {
        patch(doc, { phase: "idle", progress: null, ...(msg(e) === CANCELLED ? {} : { error: msg(e) }) });
      }
    },

    show: (doc, f) => {
      if (f.page === null) return;
      // A little above the element, so it is not pressed against the top edge.
      st().goTo(doc, f.page, f.rect ? Math.max(0, f.rect[1] - 48) : undefined);
      patch(doc, { shown: { page: f.page, rect: f.rect, nonce: ++nonce } });
    },
    clearShown: (doc) => patch(doc, { shown: null }),
    toggleGroup: (doc, name) => {
      const c = get().get(doc).collapsed;
      patch(doc, { collapsed: { ...c, [name]: !c[name] } });
    },
    setGroupClosed: (doc, name, closed) => patch(doc, { collapsed: { ...get().get(doc).collapsed, [name]: closed } }),
    dismissAutotag: (doc) => patch(doc, { notNow: true }),
    forget: (doc) =>
      set((s) => {
        const { [doc]: _gone, ...rest } = s.docs;
        void _gone;
        latest.delete(doc);
        latestOrder.delete(doc);
        latestTags.delete(doc);
        return { docs: rest };
      }),

    // ---- Reading order tool ----------------------------------------------------------------------
    loadOrder: async (doc, page, force = false) => {
      const d = st().docs[doc];
      if (!d) return;
      const o = get().get(doc).order;
      if (!force && o.data && o.page === page && o.rev === d.rev) return;
      const mine = ++seq;
      latestOrder.set(doc, mine);
      patchOrder(doc, { loading: true, error: null });
      try {
        const data = await a11yApi.readingOrder(doc, page);
        if (latestOrder.get(doc) !== mine) return;
        // A selection survives a reload of the same page when its regions are still there.
        const keep = o.page === page ? o.selected.filter((x) => data.regions.some((r) => r.xref === x)) : [];
        patchOrder(doc, { data, page, rev: d.rev, loading: false, selected: keep, anchor: keep.includes(o.anchor ?? -1) ? o.anchor : (keep[0] ?? null) });
      } catch (e) {
        if (latestOrder.get(doc) !== mine) return;
        patchOrder(doc, { loading: false, data: null, page, rev: d.rev, error: msg(e) });
      }
    },
    selectRegion: (doc, xref, how) => {
      const o = get().get(doc).order;
      const regions = o.data?.regions ?? [];
      if (how === "one") return patchOrder(doc, { selected: [xref], anchor: xref });
      if (how === "toggle") {
        const on = o.selected.includes(xref);
        const sel = on ? o.selected.filter((x) => x !== xref) : [...o.selected, xref];
        return patchOrder(doc, { selected: sel, anchor: xref });
      }
      patchOrder(doc, { selected: rangeOf(regions, o.anchor ?? xref, xref) });
    },
    reorder: async (doc, next) => {
      const page = get().get(doc).order.page;
      if (page === null) return false;
      return editOrder(doc, () => a11yApi.reorder(doc, page, next));
    },
    moveRegion: async (doc, xref, dir) => {
      if (!orderReady(doc)) return false;
      const regions = get().get(doc).order.data?.regions ?? [];
      const next = moveStep(orderOf(regions), xref, dir);
      return next ? get().reorder(doc, next) : false;
    },
    dropRegion: async (doc, xref, target) => {
      if (!orderReady(doc)) return false;
      const regions = get().get(doc).order.data?.regions ?? [];
      const next = dropOnto(orderOf(regions), xref, target);
      return next ? get().reorder(doc, next) : false;
    },
    mergeSelected: async (doc) => {
      if (!orderReady(doc)) return false;
      const o = get().get(doc).order;
      const plan = mergeRun(o.data?.regions ?? [], o.selected);
      if (!plan) return false;
      const ok = await editOrder(doc, () => a11yApi.mergeMany(doc, plan.into, plan.from));
      if (ok) patchOrder(doc, { selected: [plan.into], anchor: plan.into });
      return ok;
    },
    splitRegion: async (doc, xref, atMcid) => orderReady(doc) && editOrder(doc, () => a11yApi.split(doc, xref, atMcid)),
    artifactRegion: async (doc, xref) => {
      if (!orderReady(doc)) return false;
      const ok = await editOrder(doc, () => a11yApi.artifact(doc, xref));
      if (ok) patchOrder(doc, { selected: get().get(doc).order.selected.filter((x) => x !== xref) });
      return ok;
    },

    // ---- Tags tree -------------------------------------------------------------------------------
    loadTags: async (doc, force = false) => {
      const d = st().docs[doc];
      if (!d) return;
      const t = get().get(doc).tags;
      const scope = t.scope ?? (d.pageCount > TREE_PAGES ? "page" : "doc");
      const page = scope === "page" ? d.currentPage : null;
      if (!force && t.tree && t.rev === d.rev && t.page === page) return;
      const mine = ++seq;
      latestTags.set(doc, mine);
      patchTags(doc, { loading: true, error: null });
      try {
        const [tree, problems] = await Promise.all([a11yApi.tags(doc, page), a11yApi.tagProblems(doc).catch(() => [] as Problem[])]);
        if (latestTags.get(doc) !== mine) return;
        // The top of the tree opens by itself; the rest opens on demand.
        const expanded = { ...t.expanded };
        if (!t.tree) for (const n of tree) if (n.xref > 0) expanded[`x${n.xref}`] = true;
        patchTags(doc, { tree, problems, rev: d.rev, page, expanded, loading: false });
      } catch (e) {
        if (latestTags.get(doc) !== mine) return;
        patchTags(doc, { loading: false, tree: null, rev: d.rev, page, error: msg(e) });
      }
    },
    setTagScope: (doc, scope) => {
      patchTags(doc, { scope, tree: null });
    },
    toggleTag: (doc, key) => {
      const e = get().get(doc).tags.expanded;
      patchTags(doc, { expanded: { ...e, [key]: !e[key] } });
    },
    selectTag: async (doc, key) => {
      patchTags(doc, { selected: key });
      const tree = get().get(doc).tags.tree;
      if (!key || !tree || !key.startsWith("x")) {
        // A node written inside its parent has no object number; there is no content to look up by it.
        return;
      }
      const node = findNode(tree, Number(key.slice(1)));
      if (!node) return;
      const page = tagPage(node);
      if (page === null) return;
      let rect: Rect4 | null = null;
      try {
        rect = tagBounds(node, (await a11yApi.readingOrder(doc, page)).regions);
      } catch {
        // The page jump is still useful.
      }
      if (get().get(doc).tags.selected !== key) return;
      get().show(doc, { page, rect, xref: node.xref, object: null, message: "" });
    },
    setRole: (doc, xref, role) => editTags(doc, () => a11yApi.setRole(doc, xref, role)),
    editTag: (doc, xref, p) => editTags(doc, () => a11yApi.editTag(doc, xref, p)),
    moveTag: async (doc, xref, how) => {
      if (!tagsReady(doc)) return false;
      const tree = get().get(doc).tags.tree;
      const to = tree ? moveTarget(tree, xref, how) : null;
      if (!to) return false;
      const ok = await editTags(doc, () => a11yApi.moveTag(doc, xref, to.parent, to.anchor, to.after));
      // Into a closed tag: open it, so the moved tag is still in view.
      if (ok && to.parent > 0) patchTags(doc, { expanded: { ...get().get(doc).tags.expanded, [`x${to.parent}`]: true } });
      return ok;
    },
  };
});

/** The All tools panel and palette entry: the rail opens and reads the active document. */
export function openA11y(view?: View) {
  if (st().active == null) return;
  if (view) useA11y.getState().setView(view);
  useA11y.getState().setOpen(true);
}

// A closed document takes its report with it.
useStore.subscribe((s, prev) => {
  if (s.docs === prev.docs) return;
  for (const id of Object.keys(useA11y.getState().docs)) if (!s.docs[Number(id)]) useA11y.getState().forget(Number(id));
});
