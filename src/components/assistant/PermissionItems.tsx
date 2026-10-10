import type { ReactNode } from "react";
import { PERMISSIONS, type Permissions } from "../../lib/ai";

const NAMES: Record<Permissions, { name: string; hint: string }> = {
  ask: { name: "Ask before changes", hint: "Every change waits for your Apply." },
  auto: { name: "Apply edits, ask before deleting", hint: "Edits run; destructive changes still ask." },
  full: { name: "Apply everything", hint: "The assistant changes the document without asking." },
};

/** The "what may the assistant change" radio group, for the "..." menu. */
export function PermissionItems({
  value,
  onPick,
  tick,
}: {
  value: Permissions;
  onPick: (p: Permissions) => void;
  /** The check mark drawn beside the chosen item. */
  tick: ReactNode;
}) {
  return (
    <>
      <div className="ax-pop-label">Changes</div>
      {PERMISSIONS.map((p) => (
        <button key={p} role="menuitemradio" aria-checked={p === value} title={NAMES[p].hint} onClick={() => onPick(p)}>
          <span className="ax-tick">{p === value && tick}</span>
          {NAMES[p].name}
        </button>
      ))}
    </>
  );
}
