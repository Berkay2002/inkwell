import { useA11y } from "./a11y";
import { useAi } from "./aiStore";
import { useAnnot } from "./annot";
import { useConvert } from "./convertStore";
import { useForms } from "./forms";
import { useHistory } from "./historyStore";
import { useProtect } from "./protectStore";
import { useRedact } from "./redact";
import { useSearch } from "./searchStore";
import { useTextEdit } from "./textEdit";
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
    useTextEdit.getState().setOpen(false);
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useSearch.getState().setOpen(false);
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
    useTextEdit.getState().setOpen(false);
    useSearch.getState().setOpen(false);
    useTools.setState({ sheet: null });
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  });
  const close = () => {
    if (useAnnot.getState().open) useAnnot.getState().setOpen(false);
    if (useTextEdit.getState().open) useTextEdit.getState().setOpen(false);
    if (useForms.getState().open) useForms.getState().setOpen(false);
    if (useSearch.getState().open) useSearch.getState().setOpen(false);
  };
  // Edit takes the rail like Comments does.
  useTextEdit.subscribe((s, prev) => {
    if (!s.open || prev.open) return;
    if (useTools.getState().sheet?.phase === "running") return void useTextEdit.setState({ open: false });
    useAnnot.getState().setOpen(false);
    useForms.getState().setOpen(false);
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useSearch.getState().setOpen(false);
    useTools.setState({ sheet: null });
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  });
  // Search is a rail of the same kind: it takes the side, and any other rail opening closes it.
  useSearch.subscribe((s, prev) => {
    if (!s.open || prev.open) return;
    if (useTools.getState().sheet?.phase === "running") return void useSearch.setState({ open: false });
    useAnnot.getState().setOpen(false);
    useForms.getState().setOpen(false);
    useTextEdit.getState().setOpen(false);
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useA11y.getState().setOpen(false);
    useTools.setState({ sheet: null });
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  });
  useAi.subscribe((s, prev) => s.open && !prev.open && s.settings.mode === "docked" && close());
  useHistory.subscribe((s, prev) => s.open && !prev.open && close());
  useRedact.subscribe((s, prev) => s.open && !prev.open && close());
  useTools.subscribe((s, prev) => s.sheet && !prev.sheet && close());

  // The accessibility rail docks like the others: opening it closes the rest (a running tool keeps the
  // side to itself), and any of them opening closes it.
  useA11y.subscribe((s, prev) => {
    if (!s.open || prev.open) return;
    if (useTools.getState().sheet?.phase === "running" || useConvert.getState().sheet?.phase === "running") {
      return void useA11y.setState({ open: false });
    }
    // Protect keeps the side while it works; an idle Protect rail gives it up.
    const protect = useProtect.getState();
    if (protect.sheet?.phase === "working") return void useA11y.setState({ open: false });
    if (protect.sheet) useProtect.setState({ sheet: null });
    useAnnot.getState().setOpen(false);
    useForms.getState().setOpen(false);
    useTextEdit.getState().setOpen(false);
    useSearch.getState().setOpen(false);
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useTools.setState({ sheet: null });
    useConvert.setState({ sheet: null });
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  });
  const closeA11y = () => {
    if (useA11y.getState().open) useA11y.getState().setOpen(false);
  };
  useAi.subscribe((s, prev) => s.open && !prev.open && s.settings.mode === "docked" && closeA11y());
  useHistory.subscribe((s, prev) => s.open && !prev.open && closeA11y());
  useRedact.subscribe((s, prev) => s.open && !prev.open && closeA11y());
  useAnnot.subscribe((s, prev) => s.open && !prev.open && closeA11y());
  useForms.subscribe((s, prev) => s.open && !prev.open && closeA11y());
  useTextEdit.subscribe((s, prev) => s.open && !prev.open && closeA11y());
  useSearch.subscribe((s, prev) => s.open && !prev.open && closeA11y());
  useTools.subscribe((s, prev) => s.sheet && !prev.sheet && closeA11y());
  useConvert.subscribe((s, prev) => s.sheet && !prev.sheet && closeA11y());
  useProtect.subscribe((s, prev) => s.sheet && !prev.sheet && closeA11y());
}
