import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { EditState } from "./api";
import { applyState } from "./edit";

/**
 * Redaction for the UI (src-tauri/src/redact/app.rs). Marks are ordinary undo steps; Apply asks
 * where to save in a native dialog on the Rust side (no path ever goes from here to the engine).
 */

export type Pattern = "personnummer" | "ssn" | "iban" | "card" | "email" | "phone";
export type FindQuery =
  | { kind: "text"; needle: string; caseSensitive: boolean; wholeWord: boolean }
  | { kind: "pattern"; pattern: Pattern };
/** `[ulx, uly, urx, ury, llx, lly, lrx, lry]`, page space, y down. */
export type Quad = [number, number, number, number, number, number, number, number];
export interface Hit { page: number; text: string; quads: Quad[]; before: string; after: string }
export interface Mark { page: number; xref: number; quads: Quad[]; wholePage: boolean; label: string | null }
export interface Sanitize {
  metadata: boolean; hiddenLayers: boolean; hiddenText: boolean; scriptsActions: boolean;
  embeddedFiles: boolean; annotations: boolean; formFieldsUnderMarks: boolean; bookmarks: boolean;
}
export type SanitizeReport = Record<string, unknown>;
export interface Leak { needle: string; place: string; critical: boolean }
export interface RedactOptions { sanitize: Sanitize; preview: SanitizeReport; saveName: string }
export type ApplyOutcome =
  | { status: "applied"; state: EditState; marks: number; sanitize: SanitizeReport; verify: { leaks: Leak[] }; savedTo: string | null }
  | { status: "refused"; leaks: Leak[] }
  | { status: "cancelled" };
export type MarkWhat =
  | { kind: "hits"; hits: Hit[] }
  | { kind: "area"; page: number; area: [number, number, number, number] }
  | { kind: "pages"; pages: number[] };

export const redactApi = {
  find: (doc: number, query: FindQuery) => invoke<Hit[]>("redact_find", { doc, query }),
  mark: (doc: number, what: MarkWhat) => invoke<EditState>("redact_mark", { doc, what }),
  unmark: (doc: number, marks: [number, number][]) => invoke<EditState>("redact_unmark", { doc, marks }),
  listMarks: (doc: number) => invoke<Mark[]>("redact_list_marks", { doc }),
  options: (doc: number, sanitize?: Sanitize) => invoke<RedactOptions>("redact_options", { doc, sanitize }),
  apply: (doc: number, sanitize: Sanitize, save: boolean) =>
    invoke<ApplyOutcome>("redact_apply", { doc, options: { sanitize, save } }),
};

export const PATTERNS: { id: Pattern; label: string }[] = [
  { id: "personnummer", label: "Personnummer" },
  { id: "ssn", label: "SSN" },
  { id: "card", label: "Card number" },
  { id: "iban", label: "IBAN" },
  { id: "email", label: "E-mail" },
  { id: "phone", label: "Phone" },
];

export const SANITIZE_OFF: Sanitize = {
  metadata: false, hiddenLayers: false, hiddenText: false, scriptsActions: false,
  embeddedFiles: false, annotations: false, formFieldsUnderMarks: false, bookmarks: false,
};

/** The marks list's text for a mark: the marked text (kept in the file), "Area on page N" or "Page N". */
export function markLabel(m: Mark): string {
  if (m.wholePage) return `Page ${m.page + 1}`;
  return m.label || `Area on page ${m.page + 1}`;
}

export type Phase = "marking" | "confirm" | "applying" | "report" | "refused";

export interface DocRedact {
  /** What `hits` came from (the active pill or the typed text). */
  query: FindQuery | null;
  /** Hits per pattern, filled when the rail opens. */
  counts: Partial<Record<Pattern, number>>;
  hits: Hit[];
  /** Indexes into `hits` that "Mark n" will mark. */
  checked: number[];
  marks: Mark[];
  phase: Phase;
  options: RedactOptions | null;
  outcome: ApplyOutcome | null;
  error: string | null;
}

const fresh = (): DocRedact => ({ query: null, counts: {}, hits: [], checked: [], marks: [], phase: "marking", options: null, outcome: null, error: null });

interface RedactStore {
  /** The right rail is showing, and the area tool (drag on a page) is armed. */
  open: boolean;
  tool: boolean;
  setOpen: (open: boolean) => void;
  setTool: (on: boolean) => void;
  docs: Record<number, DocRedact>;
  loadCounts: (doc: number) => Promise<void>;
  /** "Also remove hidden information": all sanitize options off, or back to the defaults. */
  removeHidden: (doc: number, on: boolean) => Promise<void>;
  get: (doc: number) => DocRedact;
  find: (doc: number, q: FindQuery) => Promise<void>;
  clearFind: (doc: number) => void;
  toggleHit: (doc: number, i: number) => void;
  markChecked: (doc: number) => Promise<void>;
  mark: (doc: number, what: MarkWhat) => Promise<void>;
  unmark: (doc: number, marks: Mark[]) => Promise<void>;
  /** After any change to the document (undo, redo, marking elsewhere). */
  refreshMarks: (doc: number) => Promise<void>;
  confirm: (doc: number) => Promise<void>;
  setSanitize: (doc: number, s: Sanitize) => Promise<void>;
  cancel: (doc: number) => void;
  apply: (doc: number) => Promise<void>;
  done: (doc: number) => void;
}

const msg = (e: unknown) => (typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e));

export const useRedact = create<RedactStore>((set, get) => {
  const patch = (doc: number, p: Partial<DocRedact>) =>
    set((s) => ({ docs: { ...s.docs, [doc]: { ...(s.docs[doc] ?? fresh()), ...p } } }));
  const run = async (doc: number, f: () => Promise<void>) => {
    try {
      await f();
    } catch (e) {
      patch(doc, { error: msg(e) });
    }
  };
  const edited = async (doc: number, s: EditState) => {
    applyState(doc, s);
    await get().refreshMarks(doc);
  };

  return {
    open: false,
    tool: false,
    setOpen: (open) => set({ open, ...(open ? {} : { tool: false }) }),
    setTool: (tool) => set({ tool }),
    docs: {},
    loadCounts: (doc) =>
      run(doc, async () => {
        const counts: DocRedact["counts"] = {};
        await Promise.all(
          PATTERNS.map(async ({ id }) => {
            counts[id] = (await redactApi.find(doc, { kind: "pattern", pattern: id })).length;
          }),
        );
        patch(doc, { counts });
      }),
    removeHidden: (doc, on) =>
      run(doc, async () => patch(doc, { options: await redactApi.options(doc, on ? undefined : SANITIZE_OFF) })),
    get: (doc) => get().docs[doc] ?? fresh(),
    find: (doc, q) =>
      run(doc, async () => {
        const hits = await redactApi.find(doc, q);
        patch(doc, { query: q, hits, checked: hits.map((_, i) => i), error: null });
      }),
    clearFind: (doc) => patch(doc, { query: null, hits: [], checked: [] }),
    toggleHit: (doc, i) => {
      const c = get().get(doc).checked;
      patch(doc, { checked: c.includes(i) ? c.filter((x) => x !== i) : [...c, i].sort((a, b) => a - b) });
    },
    markChecked: (doc) =>
      run(doc, async () => {
        const d = get().get(doc);
        const hits = d.checked.map((i) => d.hits[i]);
        if (!hits.length) return;
        const what: MarkWhat = { kind: "hits", hits };
        await edited(doc, await redactApi.mark(doc, what));
        patch(doc, { hits: [], checked: [], query: null });
      }),
    mark: (doc, what) => run(doc, async () => edited(doc, await redactApi.mark(doc, what))),
    unmark: (doc, marks) =>
      run(doc, async () => edited(doc, await redactApi.unmark(doc, marks.map((m) => [m.page, m.xref])))),
    refreshMarks: (doc) => run(doc, async () => patch(doc, { marks: await redactApi.listMarks(doc) })),
    confirm: (doc) =>
      run(doc, async () => {
        if (!get().get(doc).marks.length) return;
        patch(doc, { options: await redactApi.options(doc), phase: "confirm", error: null });
      }),
    setSanitize: (doc, s) => run(doc, async () => patch(doc, { options: await redactApi.options(doc, s) })),
    cancel: (doc) => patch(doc, { phase: "marking", options: null }),
    apply: (doc) =>
      run(doc, async () => {
        const o = get().get(doc).options;
        if (!o) return;
        patch(doc, { phase: "applying" });
        try {
          const out = await redactApi.apply(doc, o.sanitize, true);
          if (out.status === "applied") {
            applyState(doc, out.state);
            patch(doc, { outcome: out, phase: "report", marks: [] });
          } else if (out.status === "refused") {
            patch(doc, { outcome: out, phase: "refused" });
          } else {
            patch(doc, { phase: "confirm" });
          }
        } catch (e) {
          patch(doc, { phase: "confirm" });
          throw e;
        }
      }),
    done: (doc) => patch(doc, { ...fresh() }),
  };
});
