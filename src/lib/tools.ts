import { open } from "@tauri-apps/plugin-dialog";
import type { IconName } from "../components/Icons";
import { useStore } from "../store";
import { useTools } from "./toolsStore";

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
export const toolHandlers: Partial<Record<ToolId, (path: string) => void | Promise<void>>> = {};

/** Home's quick tools: open the picked file, then the rail for it (the sheet needs the open document). */
for (const id of ["ocr", "compress"] as const) {
  toolHandlers[id] = async (path) => {
    await useStore.getState().openPath(path);
    useTools.getState().open(id);
  };
}

// TODO(editing phases): set toolHandlers[id] when the tool exists (it receives the picked path).
export async function launchTool(id: ToolId) {
  const sel = await open({ multiple: false, filters: [{ name: "PDF documents", extensions: ["pdf"] }] });
  if (typeof sel !== "string") return;
  await (toolHandlers[id] ?? useStore.getState().openPath)(sel);
}

// TODO(assistant): open the assistant panel on the opened document with a summarize request.
/** "Summarize with Assistant": for now it opens the file; the panel hook lands with the assistant branch. */
export const openWithAssistant = (path: string) => useStore.getState().openPath(path);
