import { openA11y } from "./a11y";
import { openCompareSetup } from "../components/history/HistoryPanel";
import { type DocState, useStore } from "../store";
import { pickOpen } from "./api";
import { useConvert } from "./convertStore";
import { useMarks } from "./marksStore";
import { useProtect } from "./protectStore";
import { type ToolItem } from "./toolsCatalog";
import { useToolPrefs } from "./toolPrefs";
import { useTools } from "./toolsStore";

/**
 * Every tool, as a plain list: the single source for the All tools panel, the Home grid, the toolbar's pinned
 * buttons and the command palette's subtitles. To add a tool add one entry: its `group` is a `TOOL_GROUPS` id
 * (lib/toolsCatalog.ts), `description` one plain line, `keywords` the words people would search for.
 * Preflight and the other optimize tools go in "optimize", form tools in their own group.
 * `doc` is the document the tool will run on; with null (the Home grid) only the metadata is used.
 */
export function toolItems(doc: Pick<DocState, "id" | "name"> | null): ToolItem[] {
  const id = doc?.id;
  return [
    {
      id: "compress",
      group: "optimize",
      icon: "compress",
      label: "Compress file",
      description: "Make the file smaller to email or upload",
      detail: "Shrinks images and removes unused data. Choose Smallest, Balanced or High quality and save a smaller copy.",
      keywords: "shrink smaller reduce size optimize email upload",
      command: "compress",
      onSelect: () => useTools.getState().open("compress", id),
    },
    {
      id: "ocr",
      group: "optimize",
      icon: "ocr",
      label: "Recognize text",
      description: "Make scanned pages searchable and copyable",
      detail: "Reads the text in scanned pages and adds it behind the picture, so you can search, select and copy it.",
      keywords: "ocr scan scanned searchable read text image",
      command: "ocr",
      onSelect: () => useTools.getState().open("ocr", id),
    },
    {
      id: "header-footer",
      group: "marks",
      icon: "headerFooter",
      label: "Header & footer",
      description: "Add page numbers, dates or text to every page",
      detail: "Put text, a date or the page number at the top or bottom of each page, with a preview before you apply it.",
      keywords: "page numbers date running title",
      command: "header-footer",
      onSelect: () => useMarks.getState().open("headerFooter", id),
    },
    {
      id: "watermark",
      group: "marks",
      icon: "stamp",
      label: "Watermark",
      description: "Stamp text like DRAFT or CONFIDENTIAL across pages",
      detail: "Place a word or an image across the pages, with a preview before you apply it.",
      keywords: "stamp draft confidential text image overlay",
      command: "watermark",
      onSelect: () => useMarks.getState().open("watermark", id),
    },
    {
      id: "background",
      group: "marks",
      icon: "background",
      label: "Background",
      description: "Put a colour or image behind page content",
      detail: "Fill the pages with a colour or an image that sits behind the text and pictures.",
      keywords: "colour color image paper behind",
      command: "background",
      onSelect: () => useMarks.getState().open("background", id),
    },
    {
      id: "bates",
      group: "marks",
      icon: "hash",
      label: "Bates numbering",
      description: "Number pages for legal filings",
      detail: "Give every page a running number with an optional prefix, such as ABC-000123.",
      keywords: "legal numbers page numbers court prefix",
      command: "bates",
      onSelect: () => useMarks.getState().open("bates", id),
    },
    {
      id: "protect",
      group: "protect",
      icon: "lock",
      label: "Protect with a password",
      description: "Require a password to open the file",
      detail: "Encrypts the file so it asks for a password before it opens.",
      keywords: "encrypt lock secure password open",
      command: "protect",
      onSelect: () => useProtect.getState().open("protect", id),
    },
    {
      id: "security",
      group: "protect",
      icon: "shield",
      label: "Security",
      description: "Limit printing, copying and editing",
      detail: "See and change what a reader may do: print, copy text or edit, and how the file is encrypted.",
      keywords: "permissions restrictions encryption properties",
      command: "security",
      onSelect: () => useProtect.getState().open("security", id),
    },
    {
      id: "export",
      group: "convert",
      icon: "share",
      label: "Export",
      description: "Save as Word, images, text and more",
      detail: "Turn pages or the whole file into Word, images, plain text or HTML.",
      keywords: "save as word docx image jpg png text html convert",
      command: "export",
      onSelect: () => useConvert.getState().openExport(id),
    },
    {
      id: "create",
      group: "convert",
      icon: "filePlus",
      label: "Create PDF",
      description: "Make a PDF from images, text, the clipboard or a blank page",
      detail: "Start a new PDF from pictures, a text file, what is on the clipboard, or an empty page.",
      keywords: "new from images text clipboard blank convert",
      command: "create-pdf",
      onSelect: () => useConvert.getState().openCreate(),
    },
    {
      id: "compare",
      group: "check",
      icon: "swap",
      label: "Compare files",
      description: "Highlight the differences between two PDFs",
      detail: "Pick two files and see what was added, removed or changed between them.",
      keywords: "diff differences changes versions side by side",
      command: "compare-files",
      onSelect: () => void openCompareSetup(doc ?? undefined),
    },
    {
      id: "a11y",
      group: "check",
      icon: "access",
      label: "Check accessibility",
      description: "Find and fix problems for screen reader users",
      detail: "Checks tags, reading order, alt text and form labels, and fixes what it can.",
      keywords: "accessible tagged ada wcag screen reader pdf/ua alt text",
      command: "a11y",
      onSelect: openA11y,
    },
  ];
}

/** Runs a tool and remembers it as recent: every way to start a tool goes through here. */
export function runTool(item: ToolItem) {
  useToolPrefs.getState().used(item.id);
  item.onSelect();
}

/** Tools that need no open document (Create starts from nothing). */
const NO_DOC = new Set(["create"]);

/**
 * Home's grid: with a document open run the tool on it, otherwise ask for a file first (Create needs none),
 * open it and run the tool there.
 */
export async function launchToolFromHome(toolId: string) {
  const s = useStore.getState();
  let doc = s.active != null ? s.docs[s.active] : undefined;
  if (!doc && !NO_DOC.has(toolId)) {
    const [f] = await pickOpen(false);
    if (!f) return;
    await s.openPath(f.path, f.token);
    const now = useStore.getState();
    doc = now.active != null ? now.docs[now.active] : undefined;
    if (!doc) return; // a password prompt or an error is showing instead
  }
  const item = toolItems(doc ?? null).find((i) => i.id === toolId);
  if (item) runTool(item);
}
