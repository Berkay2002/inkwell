import { useState } from "react";
import { trailSummary, waiting } from "../../lib/activity";
import type { Msg } from "../../lib/aiStore";
import { ApprovalCard } from "./ApprovalCard";
import "./activity.css";

const STATE_WORD: Record<string, string> = { running: "running", failed: "failed", rejected: "rejected" };

/** What the assistant did to answer, as one quiet line that opens into the full list, plus a card
 *  for every change that waits for the user. Nothing is drawn for a turn with no tool calls. */
export function ActivityTrail({ doc, m, worked = false }: { doc: number; m: Msg; worked?: boolean }) {
  const [open, setOpen] = useState(false);
  const cards = waiting(m.activity);
  const rest = m.activity.filter((a) => a.state !== "waiting");
  if (m.activity.length === 0) return null;
  return (
    <div className="ax-activity">
      {rest.length > 0 && (
        <div className={open ? "ax-trail open" : "ax-trail"}>
          <button className="ax-trail-line" aria-expanded={open} onClick={() => setOpen(!open)} title={open ? "Hide" : "Show what was done"}>
            <span className="ax-trail-text">
              {worked && m.timing ? `Worked for ${Math.max(1, Math.round(m.timing.totalMs / 1000))}s` : trailSummary(rest)}
              {worked && <span className="ax-trail-chev" aria-hidden />}
            </span>
          </button>
          {open && (
            <ul className="ax-trail-list">
              {rest.map((a) => (
                <li key={a.id} className={`${a.class} ${a.state}`}>
                  <span>{a.label}</span>
                  {STATE_WORD[a.state] && <span className="ax-trail-state">{STATE_WORD[a.state]}</span>}
                  {a.state === "failed" && a.detail && <span className="ax-trail-why">{a.detail}</span>}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {cards.map((a) => (
        <ApprovalCard key={a.id} doc={doc} a={a} />
      ))}
    </div>
  );
}
