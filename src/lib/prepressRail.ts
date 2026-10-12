import { useA11y } from "./a11y";
import { useAi } from "./aiStore";
import { useAnnot } from "./annot";
import { useConvert } from "./convertStore";
import { useForms } from "./forms";
import { useHistory } from "./historyStore";
import { useMeasure } from "./measure";
import { useMarks } from "./marksStore";
import { usePreflight } from "./preflight";
import { usePrepress } from "./prepressStore";
import { useProtect } from "./protectStore";
import { useRedact } from "./redact";
import { useSearch } from "./searchStore";
import { useSign } from "./sign";
import { useTextEdit } from "./textEdit";
import { useTools } from "./toolsStore";

/**
 * One right-side rail at a time: opening Output Preview closes the others (a running tool keeps the side),
 * and opening any of them closes Output Preview. Called once by App, like wireCommentRail.
 */
let wired = false;
export function wirePrepressRail() {
  if (wired) return;
  wired = true;
  const closeOthers = () => {
    useTools.getState().close();
    useConvert.getState().close();
    useProtect.getState().close();
    useHistory.getState().setOpen(false);
    useRedact.getState().setOpen(false);
    useAnnot.getState().setOpen(false);
    useForms.getState().setOpen(false);
    useTextEdit.getState().setOpen(false);
    useA11y.getState().setOpen(false);
    usePreflight.getState().setOpen(false);
    useSearch.getState().setOpen(false);
    // their tools place points and signatures on a click: not while the page answers the inspector
    useMeasure.getState().setOpen(false);
    useSign.getState().setOpen(false);
    useSign.getState().setRail(null);
    if (useMarks.getState().sheet && !useMarks.getState().sheet?.busy) useMarks.setState({ sheet: null });
    const ai = useAi.getState();
    if (ai.open && ai.settings.mode === "docked") ai.setOpen(false);
  };
  usePrepress.subscribe((s, prev) => {
    if (!s.open || prev.open) return;
    // a signing sheet holds a draft, like a running sheet: it is answered first, not closed from under the person
    const running =
      useTools.getState().sheet?.phase === "running" ||
      useConvert.getState().sheet?.phase === "running" ||
      useMarks.getState().sheet?.busy ||
      useSign.getState().rail === "sheet";
    if (running) return void usePrepress.getState().setOpen(false);
    closeOthers();
  });
  const yield_ = () => usePrepress.getState().open && usePrepress.getState().setOpen(false);
  useTools.subscribe((s, prev) => s.sheet && !prev.sheet && yield_());
  useConvert.subscribe((s, prev) => s.sheet && !prev.sheet && yield_());
  useProtect.subscribe((s, prev) => s.sheet && !prev.sheet && yield_());
  useMarks.subscribe((s, prev) => s.sheet && !prev.sheet && yield_());
  useHistory.subscribe((s, prev) => s.open && !prev.open && yield_());
  useRedact.subscribe((s, prev) => s.open && !prev.open && yield_());
  useAnnot.subscribe((s, prev) => s.open && !prev.open && yield_());
  useForms.subscribe((s, prev) => s.open && !prev.open && yield_());
  useTextEdit.subscribe((s, prev) => s.open && !prev.open && yield_());
  useA11y.subscribe((s, prev) => s.open && !prev.open && yield_());
  usePreflight.subscribe((s, prev) => s.open && !prev.open && yield_());
  useSearch.subscribe((s, prev) => s.open && !prev.open && yield_());
  useMeasure.subscribe((s, prev) => s.open && !prev.open && yield_());
  useSign.subscribe((s, prev) => ((s.open && !prev.open) || (s.rail && !prev.rail)) && yield_());
  useAi.subscribe((s, prev) => s.open && !prev.open && s.settings.mode === "docked" && yield_());
}
