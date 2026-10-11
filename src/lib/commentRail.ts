import { useAi } from "./aiStore";
import { useAnnot } from "./annot";
import { useHistory } from "./historyStore";
import { useRedact } from "./redact";
import { useSign } from "./sign";
import { useTools } from "./toolsStore";

/**
 * One right-side rail at a time: opening Comments closes the others (a running tool keeps the
 * side to itself), and opening any of them closes Comments. The same holds for the Signatures
 * rail, and the Comment and Fill & Sign strips (same place on the page) exclude each other.
 * Called once by App.
 */
let wired = false;
export function wireCommentRail() {
  if (wired) return;
  wired = true;
  useAnnot.subscribe((s, prev) => {
    if (!s.open || prev.open) return;
    if (useTools.getState().sheet?.phase === "running") return void useAnnot.setState({ open: false });
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useTools.setState({ sheet: null });
    useSign.getState().setOpen(false);
    useSign.getState().setRail(null);
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  });
  useSign.subscribe((s, prev) => {
    if (s.open && !prev.open) close();
    if (!s.rail || prev.rail) return;
    if (useTools.getState().sheet?.phase === "running") return void useSign.setState({ rail: null });
    close();
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useTools.setState({ sheet: null });
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  });
  const close = () => {
    if (useAnnot.getState().open) useAnnot.getState().setOpen(false);
  };
  const closeSign = () => {
    if (useSign.getState().rail) useSign.getState().setRail(null);
  };
  const both = () => {
    close();
    closeSign();
  };
  useAi.subscribe((s, prev) => s.open && !prev.open && s.settings.mode === "docked" && both());
  useHistory.subscribe((s, prev) => s.open && !prev.open && both());
  useRedact.subscribe((s, prev) => s.open && !prev.open && both());
  useTools.subscribe((s, prev) => s.sheet && !prev.sheet && both());
}
