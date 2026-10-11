import { openA11y } from "./a11y";
import { openPreflight } from "./preflight";
import { openCompareSetup } from "../components/history/HistoryPanel";
import { type DocState, useStore } from "../store";
import { pickOpen } from "./api";
import { keyOf } from "./recent";
import { useConvert } from "./convertStore";
import { useMarks } from "./marksStore";
import { useProtect } from "./protectStore";
import { useSearch } from "./searchStore";
import { type ToolItem } from "./toolsCatalog";
import { useToolPrefs } from "./toolPrefs";
import { openEditSection } from "./textEdit";
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
      id: "links",
      group: "edit",
      icon: "link",
      label: "Links",
      description: "Add, change or remove links to pages and web addresses",
      detail: "Draw a link, choose where it goes (a page, a web address or a file), see which links are broken, and make links from web addresses in the text.",
      keywords: "link hyperlink url web address button jump broken",
      command: "edit-links",
      onSelect: () => id !== undefined && openEditSection(id, "links"),
    },
    {
      id: "bookmarks",
      group: "edit",
      icon: "book",
      label: "Bookmarks",
      description: "Add, rename, reorder or remove the outline of the file",
      detail: "Edit the outline in the side panel: add bookmarks, drag to reorder or nest, set where each one goes, or make them from headings.",
      keywords: "bookmark outline contents navigation headings table of contents",
      command: "edit-bookmarks",
      onSelect: () => id !== undefined && openEditSection(id, "bookmarks"),
    },
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
      id: "preflight",
      group: "optimize",
      icon: "check",
      label: "Preflight",
      description: "Check and convert for long-term archiving",
      detail: "Checks the file against PDF/A, PDF/UA or PDF/X, shows each rule that fails, and converts it into a new file or in place. A report you can export; it is not a full validation.",
      keywords: "PDF/A archive preflight PDF/X PDF/UA conform standard validate",
      command: "preflight",
      onSelect: openPreflight,
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
      description: "Compare this file with another file or an earlier version",
      detail: "Compare this file with another file or with one of its earlier versions, and see what was added, removed or changed.",
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
    {
      id: "search-advanced",
      group: "check",
      icon: "search",
      label: "Advanced search",
      description: "Find words across this file or a whole folder",
      detail: "Search with options in this file, or across a whole folder or PDFs you pick, and jump to each hit.",
      keywords: "find search options folder files history across",
      command: "search-advanced",
      onSelect: () => useSearch.getState().setOpen(true),
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
    const before = new Set(Object.keys(s.docs));
    await s.openPath(f.path, f.token);
    doc = await openedDoc(f.path, before);
    if (!doc) return; // cancelled, or the open error / password prompt is showing instead
  }
  const item = toolItems(doc ?? null).find((i) => i.id === toolId);
  if (item) runTool(item);
}

/**
 * The document `openPath` made for `path` (or already had open), not whatever happens to be active. A locked
 * file shows the password dialog: wait for it, and run on the file once it opens. Cancelling gives undefined.
 */
async function openedDoc(path: string, before: Set<string>): Promise<DocState | undefined> {
  const key = keyOf(path);
  const find = () => {
    const docs = Object.values(useStore.getState().docs);
    // Rust may spell the path differently: a document that was not there before counts as the one just opened
    return docs.find((d) => keyOf(d.path) === key) ?? docs.find((d) => !before.has(String(d.id)));
  };
  const now = find();
  if (now) return now;
  if (!useStore.getState().password) {
    // the open failed: say so rather than doing nothing (openPath sets its own error for a thrown one)
    if (!useStore.getState().error) useStore.setState({ error: `Could not open ${path.split(/[\\/]/).pop() ?? path}` });
    return undefined;
  }
  return new Promise((resolve) => {
    const off = useStore.subscribe((st) => {
      const d = find();
      if (d || !st.password) {
        off();
        resolve(d);
      }
    });
  });
}
