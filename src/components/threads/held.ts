import { useAnnot } from "../../lib/annot";
import { useForms } from "../../lib/forms";
import { useNav } from "../../lib/nav";
import { useRedact } from "../../lib/redact";
import { useTextEdit } from "../../lib/textEdit";
import { otherToolHeld } from "../../lib/threads";

/** Is a page tool other than the Article tool in the hand (mark text, comments, forms, text edit, links)? */
export function useOtherToolHeld(): boolean {
  const redact = useRedact((s) => s.tool);
  const annot = useAnnot((s) => s.tool);
  const forms = useForms((s) => s.tool);
  const text = useTextEdit((s) => s.tool);
  const link = useNav((s) => s.linkTool);
  return otherToolHeld({ redact, annot, forms, text, link });
}
