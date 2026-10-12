import type { DocState } from "../store";

/**
 * The assistant's `open_tool`: opens one of the app's own tools for the user, for what the assistant may not
 * do itself (a password, permissions, combining files, searching a folder). `id` is a `toolItems` id, or
 * "merge". The tool modules are loaded on demand: they pull in the stores and panels of the whole app.
 * Returns false for an id the app does not have.
 */
export async function openAppTool(id: string, doc: Pick<DocState, "id" | "name"> | null): Promise<boolean> {
  const { toolItems, runTool } = await import("./toolItems");
  const item = toolItems(doc).find((i) => i.id === id);
  if (item) {
    runTool(item);
    return true;
  }
  if (id === "merge") {
    const { useCombine } = await import("./combineStore");
    useCombine.getState().open();
    return true;
  }
  return false;
}
