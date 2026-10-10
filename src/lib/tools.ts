import type { IconName } from "../components/Icons";
import { useStore } from "../store";
import { type FileRef, pickOpen } from "./api";

export type ToolId = "organize" | "edit" | "annotate" | "fill" | "redact" | "ocr" | "compress" | "merge";

export const quickTools: { id: ToolId; label: string; icon: IconName }[] = [
  { id: "organize", label: "Organize", icon: "organize" },
  { id: "edit", label: "Edit", icon: "edit" },
  { id: "annotate", label: "Annotate", icon: "annotate" },
  { id: "fill", label: "Fill & sign", icon: "fill" },
  { id: "redact", label: "Redact", icon: "redact" },
  { id: "ocr", label: "OCR", icon: "ocr" },
  { id: "compress", label: "Compress", icon: "compress" },
  { id: "merge", label: "Merge", icon: "merge" },
];

/** Later phases register the real tool here; until one does, a tool just opens the picked file. */
export const toolHandlers: Partial<Record<ToolId, (file: FileRef) => void | Promise<void>>> = {};

// TODO(editing phases): set toolHandlers[id] when the tool exists (it receives the picked file's token and path).
export async function launchTool(id: ToolId) {
  const [f] = await pickOpen(false);
  if (!f) return;
  await (toolHandlers[id] ?? ((f: FileRef) => useStore.getState().openPath(f.path, f.token)))(f);
}

// TODO(assistant): open the assistant panel on the opened document with a summarize request.
/** "Summarize with Assistant": for now it opens the file; the panel hook lands with the assistant branch. */
export const openWithAssistant = (path: string) => useStore.getState().openPath(path);
