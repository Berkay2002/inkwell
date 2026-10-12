import type { IconName } from "../components/Icons";
import { useStore } from "../store";
import { type FileRef, pickOpen } from "./api";
import { useCombine } from "./combineStore";
import { useConvert } from "./convertStore";
import { useProtect } from "./protectStore";
import { useTools } from "./toolsStore";

export type ToolId = "organize" | "edit" | "annotate" | "fill" | "redact" | "ocr" | "compress" | "protect" | "merge" | "export" | "create";

/** `group` is the colour the tile takes (a `TOOL_GROUPS` id, or one of the hues in toolsCatalog.ts). */
export const quickTools: { id: ToolId; label: string; icon: IconName; group: string }[] = [
  { id: "organize", label: "Organize", icon: "organize", group: "organize" },
  { id: "edit", label: "Edit", icon: "edit", group: "edit" },
  { id: "annotate", label: "Annotate", icon: "annotate", group: "comment" },
  { id: "fill", label: "Fill & sign", icon: "fill", group: "sign" },
  { id: "redact", label: "Redact", icon: "redact", group: "protect" },
  { id: "ocr", label: "OCR", icon: "ocr", group: "optimize" },
  { id: "compress", label: "Compress", icon: "compress", group: "optimize" },
  { id: "protect", label: "Protect", icon: "lock", group: "protect" },
  { id: "merge", label: "Combine", icon: "merge", group: "organize" },
  { id: "export", label: "Export", icon: "share", group: "convert" },
  { id: "create", label: "Create PDF", icon: "filePlus", group: "convert" },
];

/** Later phases register the real tool here; until one does, a tool just opens the picked file. */
export const toolHandlers: Partial<Record<ToolId, (file: FileRef) => void | Promise<void>>> = {};

/** Home's quick tools: open the picked file, then the rail for it (the sheet needs the open document). */
for (const id of ["ocr", "compress"] as const) {
  toolHandlers[id] = async (f) => {
    await useStore.getState().openPath(f.path, f.token);
    useTools.getState().open(id);
  };
}

toolHandlers.protect = async (f) => {
  await useStore.getState().openPath(f.path, f.token);
  useProtect.getState().open("protect");
};

toolHandlers.export = async (f) => {
  await useStore.getState().openPath(f.path, f.token);
  useConvert.getState().openExport();
};

// TODO(editing phases): set toolHandlers[id] when the tool exists (it receives the picked file's token and path).
export async function launchTool(id: ToolId) {
  // Create needs no document: its rail opens on Home.
  if (id === "create") return useConvert.getState().openCreate();
  // Combine picks its own files, in its own sheet.
  if (id === "merge") return useCombine.getState().open();
  const [f] = await pickOpen(false);
  if (!f) return;
  await (toolHandlers[id] ?? ((f: FileRef) => useStore.getState().openPath(f.path, f.token)))(f);
}

// TODO(assistant): open the assistant panel on the opened document with a summarize request.
/** "Summarize with Assistant": for now it opens the file; the panel hook lands with the assistant branch. */
export const openWithAssistant = (path: string) => useStore.getState().openPath(path);
