import { type DocState, useStore } from "../store";
import { MODES, MODE_NAMES, type Task } from "./ai";
import { readViewerSelection, useAi } from "./aiStore";
import { useRedact } from "./redact";
import { useAnnot } from "./annot";
import { useHistory } from "./historyStore";
import { openCompareSetup } from "../components/history/HistoryPanel";
import { pickOpen } from "./api";
import * as edits from "./edit";
import { stepZoom } from "./layout";
import { useConvert } from "./convertStore";
import { useMarks } from "./marksStore";
import { useProtect } from "./protectStore";
import { usePrint } from "./printStore";
import { useTools } from "./toolsStore";

export interface Command {
  id: string;
  title: string;
  /** First entry is shown in the palette; all of them trigger the command. */
  keys?: string[];
  needsDoc?: boolean;
  /** Left to the text field when one has focus (its own undo, select all ...). */
  notInText?: boolean;
  run: () => void;
}

const st = () => useStore.getState();
const activeDoc = (): DocState | undefined => {
  const s = st();
  return s.active != null ? s.docs[s.active] : undefined;
};
const withDoc = (f: (d: DocState) => void) => () => {
  const d = activeDoc();
  if (d) f(d);
};

export async function openDialog() {
  for (const f of await pickOpen(true)) await st().openPath(f.path, f.token);
}

export const focusPageInput = () => {
  const i = document.getElementById("page-input") as HTMLInputElement | null;
  i?.focus();
  i?.select();
};

export const commands: Command[] = [
  { id: "open", title: "Open file…", keys: ["Ctrl+O"], run: () => void openDialog() },
  { id: "close", title: "Close tab", keys: ["Ctrl+W"], needsDoc: true, run: withDoc((d) => void edits.requestClose(d.id)) },
  { id: "save", title: "Save", keys: ["Ctrl+S"], needsDoc: true, run: withDoc((d) => void edits.save(d.id)) },
  { id: "save-as", title: "Save as…", keys: ["Ctrl+Shift+S"], needsDoc: true, run: withDoc((d) => void edits.saveAs(d.id)) },
  { id: "undo", title: "Undo", keys: ["Ctrl+Z"], needsDoc: true, notInText: true, run: withDoc((d) => void edits.undo(d.id)) },
  { id: "redo", title: "Redo", keys: ["Ctrl+Y", "Ctrl+Shift+Z"], needsDoc: true, notInText: true, run: withDoc((d) => void edits.redo(d.id)) },
  {
    id: "organize",
    title: "Organize pages",
    keys: ["Ctrl+Shift+O"],
    needsDoc: true,
    run: withDoc((d) => st().patch(d.id, { organize: !d.organize })),
  },
  {
    id: "insert-blank",
    title: "Insert blank page after this one",
    needsDoc: true,
    run: withDoc((d) => void edits.edit(d.id, { op: "insertBlank", at: d.currentPage + 1, like: d.currentPage })),
  },
  {
    id: "insert-file",
    title: "Insert pages from file after this page…",
    needsDoc: true,
    run: withDoc((d) => void edits.insertFile(d.id, d.currentPage + 1)),
  },
  {
    id: "rotate-page-cw",
    title: "Rotate this page clockwise (saved in the file)",
    needsDoc: true,
    run: withDoc((d) => void edits.edit(d.id, { op: "rotate", pages: [d.currentPage], quarterTurns: 1 })),
  },
  {
    id: "delete-page",
    title: "Delete this page",
    needsDoc: true,
    run: withDoc((d) => d.pageCount > 1 && void edits.edit(d.id, { op: "delete", pages: [d.currentPage] })),
  },
  { id: "extract-page", title: "Extract this page…", needsDoc: true, run: withDoc((d) => void edits.extract(d.id, [d.currentPage])) },
  { id: "split", title: "Split into single pages", needsDoc: true, run: withDoc((d) => void edits.split(d.id, 1)) },
  { id: "merge", title: "Merge files…", run: () => void edits.mergeFiles() },
  { id: "compress", title: "Compress file…", needsDoc: true, run: () => useTools.getState().open("compress") },
  { id: "ocr", title: "Recognize text (OCR)…", needsDoc: true, run: () => useTools.getState().open("ocr") },
  {
    id: "ocr-page",
    title: "Recognize text on this page",
    needsDoc: true,
    run: () => useTools.getState().open("ocr", undefined, { scope: "page" }),
  },
  { id: "header-footer", title: "Add header and footer…", needsDoc: true, run: () => useMarks.getState().open("headerFooter") },
  { id: "page-numbers", title: "Add page numbers…", needsDoc: true, run: () => useMarks.getState().open("headerFooter") },
  { id: "watermark", title: "Add watermark…", needsDoc: true, run: () => useMarks.getState().open("watermark") },
  { id: "background", title: "Add background…", needsDoc: true, run: () => useMarks.getState().open("background") },
  { id: "bates", title: "Add Bates numbering…", needsDoc: true, run: () => useMarks.getState().open("bates") },
  { id: "protect", title: "Protect with a password…", needsDoc: true, run: () => useProtect.getState().open("protect") },
  { id: "security", title: "Document properties and security…", needsDoc: true, run: () => useProtect.getState().open("security") },
  { id: "export", title: "Export pages, text or images…", needsDoc: true, run: () => useConvert.getState().openExport() },
  { id: "export-word", title: "Export to Word…", needsDoc: true, run: () => useConvert.getState().openExport(undefined, "word") },
  { id: "print", title: "Print…", keys: ["Ctrl+P"], needsDoc: true, run: () => usePrint.getState().open() },
  { id: "create-pdf", title: "Create PDF…", run: () => useConvert.getState().openCreate() },
  { id: "create-clipboard", title: "Create PDF from the clipboard image", run: () => useConvert.getState().openCreate("clipboard") },
  { id: "create-blank", title: "Create a blank PDF…", run: () => useConvert.getState().openCreate("blank") },
  { id: "next-tab", title: "Next tab", keys: ["Ctrl+Tab", "Ctrl+PageDown"], run: () => st().cycleTab(1) },
  { id: "prev-tab", title: "Previous tab", keys: ["Ctrl+Shift+Tab", "Ctrl+PageUp"], run: () => st().cycleTab(-1) },
  { id: "find", title: "Find in document", keys: ["Ctrl+F"], needsDoc: true, run: () => st().setUi({ searchOpen: true }) },
  {
    id: "find-next",
    title: "Find next",
    keys: ["F3"],
    needsDoc: true,
    run: withDoc((d) => st().stepHit(d.id, 1)),
  },
  {
    id: "find-prev",
    title: "Find previous",
    keys: ["Shift+F3"],
    needsDoc: true,
    run: withDoc((d) => st().stepHit(d.id, -1)),
  },
  { id: "goto", title: "Go to page…", keys: ["Ctrl+G"], needsDoc: true, run: focusPageInput },
  {
    id: "next-page",
    title: "Next page",
    keys: ["Ctrl+ArrowDown"],
    needsDoc: true,
    run: withDoc((d) => st().goTo(d.id, d.currentPage + 1)),
  },
  {
    id: "prev-page",
    title: "Previous page",
    keys: ["Ctrl+ArrowUp"],
    needsDoc: true,
    run: withDoc((d) => st().goTo(d.id, d.currentPage - 1)),
  },
  { id: "first-page", title: "First page", needsDoc: true, run: withDoc((d) => st().goTo(d.id, 0)) },
  { id: "last-page", title: "Last page", needsDoc: true, run: withDoc((d) => st().goTo(d.id, d.pageCount - 1)) },
  {
    id: "zoom-in",
    title: "Zoom in",
    keys: ["Ctrl+=", "Ctrl++"],
    needsDoc: true,
    run: withDoc((d) => st().setZoom(d.id, stepZoom(d.zoom, 1))),
  },
  {
    id: "zoom-out",
    title: "Zoom out",
    keys: ["Ctrl+-"],
    needsDoc: true,
    run: withDoc((d) => st().setZoom(d.id, stepZoom(d.zoom, -1))),
  },
  { id: "actual", title: "Actual size (100%)", keys: ["Ctrl+1"], needsDoc: true, run: withDoc((d) => st().setZoom(d.id, 1)) },
  { id: "fit-width", title: "Fit width", keys: ["Ctrl+2"], needsDoc: true, run: withDoc((d) => st().setZoomMode(d.id, "fitWidth")) },
  { id: "fit-page", title: "Fit page", keys: ["Ctrl+0"], needsDoc: true, run: withDoc((d) => st().setZoomMode(d.id, "fitPage")) },
  { id: "rotate-cw", title: "Rotate clockwise", keys: ["Ctrl+R"], needsDoc: true, run: withDoc((d) => st().rotate(d.id, 1)) },
  {
    id: "rotate-ccw",
    title: "Rotate counter-clockwise",
    keys: ["Ctrl+Shift+R"],
    needsDoc: true,
    run: withDoc((d) => st().rotate(d.id, -1)),
  },
  { id: "sidebar", title: "Toggle sidebar", keys: ["Ctrl+B"], run: () => st().setSidebar(st().sidebar === "none" ? "thumbs" : "none") },
  { id: "thumbs", title: "Show thumbnails", run: () => st().setSidebar("thumbs") },
  { id: "outline", title: "Show outline", run: () => st().setSidebar("outline") },
  { id: "theme-system", title: "Theme: match system", run: () => st().setTheme("system") },
  { id: "theme-light", title: "Theme: light", run: () => st().setTheme("light") },
  { id: "theme-dark", title: "Theme: dark", run: () => st().setTheme("dark") },
  {
    id: "history",
    title: "Version history",
    keys: ["Ctrl+Alt+H"],
    needsDoc: true,
    run: () => {
      useHistory.getState().closeCompare();
      useHistory.getState().toggle();
    },
  },
  { id: "redact", title: "Redact", needsDoc: true, run: () => useRedact.getState().setOpen(!useRedact.getState().open) },
  { id: "comments", title: "Comments", keys: ["Ctrl+Alt+M"], needsDoc: true, run: () => useAnnot.getState().setOpen(!useAnnot.getState().open) },
  ...(
    [
      ["v", "select", "Select"], ["h", "highlight", "Highlight"], ["u", "underline", "Underline"], ["s", "strikeOut", "Strikethrough"],
      ["n", "note", "Note"], ["t", "freeText", "Text box"], ["p", "pen", "Pen"], ["r", "rectangle", "Rectangle"],
      ["o", "ellipse", "Ellipse"], ["l", "line", "Line"], ["a", "arrow", "Arrow"], ["m", "stamp", "Stamp"],
    ] as const
  ).map(([key, tool, name]) => ({
    id: `comment-tool-${tool}`,
    title: `Comments: ${name} tool`,
    keys: [key.toUpperCase()],
    needsDoc: true,
    notInText: true,
    run: () => {
      const a = useAnnot.getState();
      if (a.open) a.setTool(tool);
    },
  })),
  {
    id: "comment-delete",
    title: "Comments: delete selected",
    keys: ["Delete"],
    needsDoc: true,
    notInText: true,
    run: withDoc((d) => {
      const a = useAnnot.getState();
      const sel = a.open ? a.get(d.id).selected : null;
      if (sel) void a.remove(d.id, [sel]);
    }),
  },
  {
    id: "history-compare",
    title: "Compare with\u2026",
    needsDoc: true,
    run: withDoc((d) => {
      const h = useHistory.getState();
      const vs = h.docs[d.id]?.versions ?? [];
      // the selected version, else the one before the newest; against the file as it is now
      const a = h.selected[d.id] ?? (vs[1] ?? vs[0])?.id;
      if (!a) return st().setUi({ error: "This document has no earlier version to compare with yet." });
      h.openCompare({ doc: d.id, a: { kind: "version", id: a }, b: { kind: "current" } });
    }),
  },
  {
    id: "compare-files",
    title: "Compare files\u2026",
    run: () => {
      const s = st();
      const d = s.active != null ? s.docs[s.active] : null;
      void openCompareSetup(d ? { id: d.id, name: d.name } : undefined);
    },
  },
  { id: "palette", title: "Command palette", keys: ["Ctrl+K"], run: () => st().setUi({ paletteOpen: true }) },
  { id: "ai-toggle", title: "AI Assistant: show or hide", keys: ["Ctrl+Shift+A"], needsDoc: true, run: () => useAi.getState().toggle() },
  { id: "ai-layout-cycle", title: "Assistant: next layout", keys: ["Ctrl+Shift+L"], needsDoc: true, run: () => useAi.getState().cycleMode() },
  ...MODES.map((m) => ({
    id: `ai-layout-${m}`,
    title: `Assistant: ${MODE_NAMES[m].toLowerCase()} layout`,
    needsDoc: true,
    run: () => useAi.getState().setMode(m),
  })),
  {
    id: "ai-summarize",
    title: "AI Assistant: summarize this document",
    needsDoc: true,
    run: withDoc((d) => askAi(d.id, { kind: "summarizeDocument" })),
  },
  {
    id: "ai-summarize-page",
    title: "AI Assistant: summarize this page",
    needsDoc: true,
    run: withDoc((d) => askAi(d.id, { kind: "summarizePage", page: d.currentPage })),
  },
  ...(["explain", "rewrite", "summarize"] as const).map((action) => ({
    id: `ai-${action}-selection`,
    title: `AI Assistant: ${action} selected text`,
    needsDoc: true,
    run: withDoc((d) => {
      const sel = readViewerSelection() ?? useAi.getState().selection;
      if (sel) askAi(d.id, { kind: "selection", action, text: sel.text, page: sel.page });
    }),
  })),
  { id: "ai-clear", title: "AI Assistant: clear chat", needsDoc: true, run: withDoc((d) => useAi.getState().clear(d.id)) },
];

function askAi(doc: number, task: Task) {
  const ai = useAi.getState();
  ai.setOpen(true);
  ai.ask(doc, task);
}

/** "Ctrl+Shift+Tab" style name of a key event; shift is only part of it for letters and named keys. */
export function comboOf(e: KeyboardEvent): string {
  const named = e.key.length > 1 || /^[a-z]$/i.test(e.key);
  const key = e.key.length === 1 ? e.key.toUpperCase() : e.key;
  return [e.ctrlKey || e.metaKey ? "Ctrl" : "", e.altKey ? "Alt" : "", e.shiftKey && named ? "Shift" : "", key]
    .filter(Boolean)
    .join("+");
}

const byCombo = new Map<string, Command>();
for (const c of commands) for (const k of c.keys ?? []) byCombo.set(k.toUpperCase(), c);

export const commandFor = (e: KeyboardEvent): Command | undefined => byCombo.get(comboOf(e).toUpperCase());

export const prettyKeys = (k: string) => k.replace("ArrowDown", "↓").replace("ArrowUp", "↑");
