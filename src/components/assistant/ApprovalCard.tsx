import type { Activity } from "../../lib/ai";
import { useAi } from "../../lib/aiStore";

/** A change the assistant wants to make and is waiting on: what and where, then Apply / Reject.
 *  The turn is paused until one is pressed (or the turn is stopped, which rejects it). */
export function ApprovalCard({ doc, a }: { doc: number; a: Activity }) {
  const decide = useAi.getState().decide;
  return (
    <div className={`ax-approve ${a.class}`} role="group" aria-label="Change waiting for your approval">
      <div className="ax-approve-what">{a.label}</div>
      {a.detail && <div className="ax-approve-where">{a.detail}</div>}
      <div className="ax-row">
        <button className="ax-btn primary" onClick={() => decide(doc, a.id, true)}>
          Apply
        </button>
        <button className="ax-btn" onClick={() => decide(doc, a.id, false)}>
          Reject
        </button>
      </div>
    </div>
  );
}
