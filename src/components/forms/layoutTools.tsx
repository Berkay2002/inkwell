import { useEffect } from "react";
import { useA11y } from "../../lib/a11y";
import { type AlignEdge, type LayoutAxis, type SizeDim, useForms } from "../../lib/forms";
import { CERTIFIED_REASON, type Field, type LayoutOp, type Pick, layoutReason } from "../../lib/formsLogic";
import type { IconName } from "../Icons";
import type { MenuEntry } from "../Menu";

const fm = () => useForms.getState();

export interface LayoutAction {
  id: string;
  label: string;
  icon: IconName;
  op: LayoutOp;
  run: (doc: number) => void;
}

const align = (edge: AlignEdge, label: string, icon: IconName): LayoutAction => ({ id: `align-${edge}`, label, icon, op: "align", run: (d) => void fm().align(d, edge) });
const center = (axis: LayoutAxis, label: string, icon: IconName): LayoutAction => ({ id: `center-${axis}`, label, icon, op: "center", run: (d) => void fm().center(d, axis) });
const distribute = (axis: LayoutAxis, label: string, icon: IconName): LayoutAction => ({ id: `distribute-${axis}`, label, icon, op: "distribute", run: (d) => void fm().distribute(d, axis) });
const size = (dim: SizeDim, label: string, icon: IconName): LayoutAction => ({ id: `size-${dim}`, label, icon, op: "size", run: (d) => void fm().matchSize(d, dim) });

/** The layout tools in toolbar order; the groups are the separators of the context menu. */
export const LAYOUT_GROUPS: LayoutAction[][] = [
  [
    align("left", "Align left", "alignLeft"),
    align("right", "Align right", "alignRight"),
    align("top", "Align top", "alignTop"),
    align("bottom", "Align bottom", "alignBottom"),
    align("hCenter", "Align horizontal centres", "alignHCenter"),
    align("vCenter", "Align vertical centres", "alignVCenter"),
  ],
  [
    center("horizontal", "Centre horizontally on the page", "centerH"),
    center("vertical", "Centre vertically on the page", "centerV"),
    center("both", "Centre on the page", "centerBoth"),
  ],
  [distribute("horizontal", "Distribute horizontally", "distributeH"), distribute("vertical", "Distribute vertically", "distributeV")],
  [size("width", "Same width", "sameWidth"), size("height", "Same height", "sameHeight"), size("both", "Same size", "sameSize")],
  [
    { id: "duplicate", label: "Duplicate on other pages…", icon: "duplicate", op: "duplicate", run: (d) => openDialog(d, "duplicate") },
    { id: "copies", label: "Create multiple copies…", icon: "copies", op: "copies", run: (d) => openDialog(d, "copies") },
  ],
];

/** The dialogs live in the rail. */
function openDialog(doc: number, d: "duplicate" | "copies") {
  fm().setOpen(true);
  fm().setDialog(doc, d);
}

/** Whether the document is certified: no layout tool is offered then, and the rail says why. */
export function useCertified(doc: number): boolean {
  const certified = useA11y((s) => s.docs[doc]?.protection?.certified ?? false);
  useEffect(() => {
    void useA11y.getState().protection(doc);
  }, [doc]);
  return certified;
}

/** The right-click menu of a picked field: the layout tools (disabled with their reason), rename and delete. */
export function layoutMenu(doc: number, picked: Pick[], fields: Field[], certified: boolean): MenuEntry[] {
  const out: MenuEntry[] = [];
  for (const g of LAYOUT_GROUPS) {
    for (const a of g) out.push({ label: a.label, disabled: layoutReason(a.op, picked, fields, certified) ?? undefined, onSelect: () => a.run(doc) });
    out.push("sep");
  }
  const anchor = picked[picked.length - 1];
  out.push({
    label: "Rename…",
    hint: "F2",
    disabled: layoutReason("rename", picked, fields, certified) ?? undefined,
    onSelect: () => {
      fm().setOpen(true);
      fm().setRenaming(doc, anchor ?? null);
    },
  });
  out.push({
    label: picked.length > 1 ? `Delete ${picked.length} fields` : "Delete field",
    hint: "Del",
    danger: true,
    disabled: certified ? CERTIFIED_REASON : undefined,
    onSelect: () => void fm().remove(doc, picked),
  });
  return out;
}
