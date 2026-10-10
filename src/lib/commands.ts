import { open } from "@tauri-apps/plugin-dialog";
import { type DocState, useStore } from "../store";
import type { Task } from "./ai";
import { readViewerSelection, useAi } from "./aiStore";
import { stepZoom } from "./layout";

export interface Command {
  id: string;
  title: string;
  /** First entry is shown in the palette; all of them trigger the command. */
  keys?: string[];
  needsDoc?: boolean;
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
  const sel = await open({ multiple: true, filters: [{ name: "PDF documents", extensions: ["pdf"] }] });
  for (const p of sel ?? []) await st().openPath(p);
}

export const focusPageInput = () => {
  const i = document.getElementById("page-input") as HTMLInputElement | null;
  i?.focus();
  i?.select();
};

export const commands: Command[] = [
  { id: "open", title: "Open file…", keys: ["Ctrl+O"], run: () => void openDialog() },
  { id: "close", title: "Close tab", keys: ["Ctrl+W"], needsDoc: true, run: withDoc((d) => st().closeDoc(d.id)) },
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
  { id: "palette", title: "Command palette", keys: ["Ctrl+K"], run: () => st().setUi({ paletteOpen: true }) },
  { id: "ai-toggle", title: "AI Assistant: show or hide", keys: ["Ctrl+Shift+A"], needsDoc: true, run: () => useAi.getState().toggle() },
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
