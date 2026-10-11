import { useEffect } from "react";
import { usePreflight } from "../../lib/preflight";
import { counts, claimText, profileInfo, verifiedText } from "../../lib/preflightLogic";
import type { DocState } from "../../store";
import "./preflight.css";

/**
 * "Says it is PDF/A-2b" in Document properties, when the file's XMP says so. It is the file's own
 * claim, not a check: Verify runs the profile in the Preflight rail and reports what it finds.
 */
export function ClaimBadge({ doc }: { doc: DocState }) {
  const d = usePreflight((s) => s.docs[doc.id]);
  const id = doc.id;
  useEffect(() => {
    void usePreflight.getState().refreshClaim(id);
  }, [id, doc.rev]);
  const claim = d?.claim;
  if (!claim || d?.claimRev !== doc.rev) return null;
  // A report of this very revision against the claimed profile answers the claim.
  const r = d.report;
  const verified = r && claim.profile && r.profile === claim.profile && d.checkedRev === doc.rev && d.phase === "idle" ? r : null;
  const failed = verified ? counts(verified).failed : 0;
  return (
    <div className="pf-claim" role="group" aria-label="Conformance claim">
      <span className="pf-claim-text">
        {claimText(claim)}
      </span>
      <span className="pf-claim-note">
        {verified
          ? verified.conforms
            ? verifiedText(profileInfo(verified.profile).label)
            : `Not verified: ${failed} ${failed === 1 ? "rule fails" : "rules fail"}.`
          : "This is what the file says about itself."}
      </span>
      {claim.profile && (
        <button className="a11y-link pf-claim-verify" onClick={() => usePreflight.getState().verify(id)}>
          Verify
        </button>
      )}
    </div>
  );
}
