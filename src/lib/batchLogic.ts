import type { BatchAction, BatchIssue, BatchSanitize, BatchStep, BatchStepKind, MarkStyle } from "./api";
import type { IconName } from "../components/Icons";

/**
 * The Action Wizard without the store: the step library (grouped like Acrobat's), the settings a new step
 * starts with, and the pure helpers the editor and the run sheet share. Mirrors src-tauri/src/batch/action.rs.
 */

export interface StepInfo {
  kind: BatchStepKind;
  label: string;
  description: string;
  icon: IconName;
}

export const STEP_INFO: Record<BatchStepKind, StepInfo> = {
  protect: { kind: "protect", label: "Protect with a password", description: "Require a password to open, or limit printing and editing", icon: "lock" },
  sanitize: { kind: "sanitize", label: "Remove hidden information", description: "Metadata, scripts, hidden layers, attachments and more", icon: "shield" },
  redact: { kind: "redact", label: "Redact", description: "Find text or patterns, remove them for good and check the result", icon: "redact" },
  ocr: { kind: "ocr", label: "Recognize text", description: "Make scanned pages searchable and copyable", icon: "ocr" },
  flattenForms: { kind: "flattenForms", label: "Flatten form fields", description: "Turn fields into plain page content", icon: "checkSquare" },
  setProperties: { kind: "setProperties", label: "Set document properties", description: "Title, author, subject and keywords", icon: "file" },
  headerFooter: { kind: "headerFooter", label: "Header & footer", description: "Page numbers, dates or text at the top or bottom", icon: "headerFooter" },
  watermark: { kind: "watermark", label: "Watermark", description: "Stamp text like DRAFT or CONFIDENTIAL across pages", icon: "stamp" },
  bates: { kind: "bates", label: "Bates numbering", description: "A running number on every page, across files", icon: "hash" },
  exportText: { kind: "exportText", label: "Export text", description: "Write the text of each file to a .txt", icon: "textT" },
  exportImages: { kind: "exportImages", label: "Export page images", description: "One picture per page, in a folder", icon: "share" },
  compress: { kind: "compress", label: "Compress", description: "Make the file smaller to email or upload", icon: "compress" },
};

/** The library's groups, in Acrobat's order. */
export const STEP_GROUPS: { id: string; title: string; kinds: BatchStepKind[] }[] = [
  { id: "protect", title: "Protect", kinds: ["protect", "sanitize", "redact"] },
  { id: "document", title: "Document processing", kinds: ["ocr", "flattenForms", "setProperties"] },
  { id: "pages", title: "Pages", kinds: ["headerFooter", "watermark", "bates"] },
  { id: "export", title: "Export", kinds: ["exportText", "exportImages"] },
  { id: "optimize", title: "Optimize", kinds: ["compress"] },
];

export const MAX_STEPS = 24;

const STYLE: MarkStyle = { family: "helvetica", bold: false, italic: false, size: 12, color: [0, 0, 0], underline: false };

const ALL_ON: BatchSanitize = {
  metadata: true,
  hiddenLayers: true,
  hiddenText: true,
  scriptsActions: true,
  embeddedFiles: true,
  annotations: true,
  formFieldsUnderMarks: true,
  bookmarks: true,
};

/** What a redact step removes besides the text: what could quote it, not the person's own comments and bookmarks. */
export const SANITIZE_QUOTING: BatchSanitize = {
  metadata: true,
  hiddenLayers: true,
  hiddenText: false,
  scriptsActions: true,
  embeddedFiles: true,
  annotations: false,
  formFieldsUnderMarks: true,
  bookmarks: false,
};

/** A new step with the settings the engine would give it (the Rust `Default`s), filled in so the form can show them. */
export function newStep(kind: BatchStepKind): BatchStep {
  switch (kind) {
    case "compress":
      return { kind, preset: "balanced" };
    case "ocr":
      return { kind, languages: [], force: false, backend: "windows" };
    case "headerFooter":
      // Page number at the bottom centre: a header with no text at all is refused.
      return { kind, text: ["", "", "", "", "Page <<1>> of <<n>>", ""], style: { ...STYLE }, margins: [36, 36, 72, 72], startNumber: 1 };
    case "watermark":
      return {
        kind,
        text: "DRAFT",
        picture: null,
        style: { ...STYLE, size: 0, color: [0.6, 0.6, 0.6], bold: true },
        scale: 0.5,
        opacity: 0.5,
        rotation: 45,
        behind: false,
        offset: [0, 0],
        showOnScreen: true,
        showInPrint: true,
      };
    case "bates":
      return {
        kind,
        bates: { prefix: "", suffix: "", digits: 6, start: 1, slot: 5, style: { ...STYLE }, margins: [36, 36, 72, 72] },
        continueAcrossFiles: true,
      };
    case "protect":
      return { kind, printing: "high", changes: "allButExtract", copy: true, accessibility: true, cipher: "aes256" };
    case "sanitize":
      return { kind, ...ALL_ON };
    case "redact":
      return { kind, queries: [{ type: "pattern", pattern: "email" }], overlayText: null, sanitize: { ...SANITIZE_QUOTING } };
    case "flattenForms":
      return { kind };
    case "exportImages":
      return { kind, format: "png", dpi: 150, gray: false, quality: 90 };
    case "exportText":
      return { kind, layout: false };
    case "setProperties":
      return { kind, title: null, author: null, subject: null, keywords: null, creator: null, producer: null };
  }
}

export const emptyAction = (): BatchAction => ({
  id: "",
  name: "",
  description: "",
  steps: [],
  output: { kind: "nextToSource", suffix: " (batch)" },
  onError: "skip",
  builtIn: false,
});

/** A copy to edit: no id (it is stored under a new one), not built in. Used for Duplicate and for editing a built-in. */
export const copyOf = (a: BatchAction, name = `${a.name} copy`): BatchAction => ({
  ...structuredClone(a),
  id: "",
  name: name.slice(0, 80),
  builtIn: false,
});

/** `arr` with the item at `from` moved to index `to`; the same array when nothing moves. */
export function move<T>(arr: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= arr.length || to >= arr.length) return arr;
  const next = arr.slice();
  const [x] = next.splice(from, 1);
  next.splice(to, 0, x);
  return next;
}

/** One line for a card or a row: the step's name and its main setting. */
export function stepLabel(s: BatchStep): string {
  switch (s.kind) {
    case "invalid":
      return `Unknown step "${s.name}"`;
    case "compress":
      return `Compress (${{ smallest: "smallest", balanced: "balanced", highQuality: "high quality", custom: "custom" }[s.preset]})`;
    case "watermark":
      return s.text.trim() ? `Watermark "${s.text.trim()}"` : "Watermark";
    case "redact": {
      const n = s.queries.length;
      return `Redact (${n} ${n === 1 ? "search" : "searches"})`;
    }
    case "exportImages":
      return `Export page images (${s.format.toUpperCase()}, ${s.dpi} dpi)`;
    case "ocr":
      return s.languages.length ? `Recognize text (${s.languages.join(", ")})` : "Recognize text";
    default:
      return STEP_INFO[s.kind].label;
  }
}

/** The action as a run or a validation takes it: the typed passwords go into the Protect steps, nowhere else. */
export function withPasswords(a: BatchAction, open: string, permissions: string): BatchAction {
  if (!open && !permissions) return a;
  return {
    ...a,
    steps: a.steps.map((s) =>
      s.kind === "protect" ? { ...s, ...(open ? { openPassword: open } : {}), ...(permissions ? { permissionsPassword: permissions } : {}) } : s,
    ),
  };
}

/** The action as it is saved or exported: the engine never writes passwords, and neither does a copy of ours. */
export function withoutPasswords(a: BatchAction): BatchAction {
  return {
    ...a,
    steps: a.steps.map((s) => {
      if (s.kind !== "protect") return s;
      const { openPassword: _o, permissionsPassword: _p, ...rest } = s;
      return rest;
    }),
  };
}

export const hasProtect = (a: BatchAction) => a.steps.some((s) => s.kind === "protect");

/**
 * Issues split into those that stop the action and the Protect steps that only wait for a password (typed when
 * the run starts: a stored action has none, so the engine always asks). Told by step and code, not by wording:
 * with no password a Protect step can only fail for want of one.
 */
export function splitIssues(a: BatchAction, issues: BatchIssue[], passwordTyped: boolean): { blocking: BatchIssue[]; passwordFor: number[] } {
  const blocking: BatchIssue[] = [];
  const passwordFor: number[] = [];
  for (const i of issues) {
    if (!passwordTyped && i.code === "invalid-step" && i.step != null && a.steps[i.step]?.kind === "protect") passwordFor.push(i.step);
    else blocking.push(i);
  }
  return { blocking, passwordFor };
}

/** Why Save is not offered, or null: it needs a name and only steps the engine knows; other issues still let a draft be kept. */
export function saveBlock(a: BatchAction): string | null {
  if (a.builtIn) return "A built-in action cannot be changed: duplicate it first.";
  if (!a.name.trim()) return "Give the action a name.";
  if (a.name.trim().length > 80) return "The name is too long (80 characters at most).";
  if (a.steps.length === 0) return "Add at least one step.";
  const bad = a.steps.find((s) => s.kind === "invalid");
  if (bad && bad.kind === "invalid") return bad.why;
  return null;
}

/** The file name an exported action is offered under. */
export const exportName = (a: BatchAction) => `${a.name.trim().replace(/[<>:"/\\|?*\u0000-\u001f]+/g, " ").replace(/\s+/g, " ").trim() || "action"}.json`;

/** "3 files" / "1 file". */
export const filesWord = (n: number) => `${n} ${n === 1 ? "file" : "files"}`;
