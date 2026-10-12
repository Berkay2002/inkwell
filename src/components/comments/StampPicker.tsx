import { useEffect, useMemo } from "react";
import { useAnnot } from "../../lib/annot";
import { STAMPS } from "../../lib/annotLogic";
import { useStamps } from "../../lib/stamps";
import { SIGNED_NOTE, byCategory, customId, customValue } from "../../lib/stampsLogic";
import type { DocState } from "../../store";
import { Icon } from "../Icons";

/**
 * The stamp tool's picker: Standard, Dynamic (the date) and one group per category of custom stamps, and the button
 * that opens the manager. A document that takes no annotations (certified) turns the picker off and says why; a signed
 * one is told what a stamp does to it.
 */
export function StampPicker({ doc }: { doc: DocState }) {
  const stamp = useAnnot((s) => s.stamp);
  const stamps = useStamps((s) => s.stamps);
  const loaded = useStamps((s) => s.loaded);
  const gate = useStamps((s) => s.gates[doc.id]);
  const groups = useMemo(() => byCategory(stamps), [stamps]);

  useEffect(() => {
    if (!loaded) void useStamps.getState().load();
  }, [loaded]);
  useEffect(() => {
    void useStamps.getState().readGate(doc.id);
  }, [doc.id, doc.rev]);

  const id = customId(stamp);
  const value = id && !stamps.some((s) => s.id === id) ? "Approved" : stamp;
  const reason = gate?.reason ?? null;
  return (
    <>
      <select
        className="an-stamp"
        value={value}
        aria-label="Stamp"
        title={reason ?? "Stamp"}
        disabled={!!reason}
        onChange={(e) => useAnnot.getState().setStamp(e.target.value)}
      >
        <optgroup label="Standard">
          {STAMPS.map((s) => (
            <option key={s}>{s}</option>
          ))}
        </optgroup>
        <optgroup label="Dynamic">
          <option value="Date">Date</option>
        </optgroup>
        {groups.map((g) => (
          <optgroup key={g.category} label={g.category}>
            {g.stamps.map((s) => (
              <option key={s.id} value={customValue(s.id)}>
                {s.name}
              </option>
            ))}
          </optgroup>
        ))}
      </select>
      <button className="icon-btn" title="Custom stamps…" aria-label="Custom stamps" onClick={() => useStamps.getState().openManager()}>
        <Icon name="settings" />
      </button>
      {(reason || gate?.signed) && (
        <p className={`an-note${reason ? " off" : ""}`} role="status">
          {reason ?? SIGNED_NOTE}
        </p>
      )}
    </>
  );
}
