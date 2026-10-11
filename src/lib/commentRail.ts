import { useAi } from "./aiStore";
import { useAnnot } from "./annot";
import { useForms } from "./forms";
import { useHistory } from "./historyStore";
import { useRedact } from "./redact";
import { useTools } from "./toolsStore";

/**
 * One right-side rail at a time: opening Comments closes the others (a running tool keeps the
 * side to itself), and opening any of them closes Comments. Called once by App.
 */
let wired = false;
export function wireCommentRail() {
  if (wired) return;
  wired = true;
  useAnnot.subscribe((s, prev) => {
    if (!s.open || prev.open) return;
    if (useTools.getState().sheet?.phase === "running") return void useAnnot.setState({ open: false });
    useForms.getState().setOpen(false);
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useTools.setState({ sheet: null });
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  });
  // Forms is a rail of the same kind: the two close each other and every other rail.
  useForms.subscribe((s, prev) => {
    if (!s.open || prev.open) return;
    if (useTools.getState().sheet?.phase === "running") return void useForms.setState({ open: false });
    useAnnot.getState().setOpen(false);
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useTools.setState({ sheet: null });
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  });
  const close = () => {
    if (useAnnot.getState().open) useAnnot.getState().setOpen(false);
    if (useForms.getState().open) useForms.getState().setOpen(false);
  };
  useAi.subscribe((s, prev) => s.open && !prev.open && s.settings.mode === "docked" && close());
  useHistory.subscribe((s, prev) => s.open && !prev.open && close());
  useRedact.subscribe((s, prev) => s.open && !prev.open && close());
  useTools.subscribe((s, prev) => s.sheet && !prev.sheet && close());
}
