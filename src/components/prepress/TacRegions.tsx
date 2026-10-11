import { useStore } from "../../store";
import { usePrepress } from "../../lib/prepressStore";

/** The areas above the ink limit on a page, in page points (the space `.pagespace` is in). */
export function TacRegions({ docId, page }: { docId: number; page: number }) {
  const rev = useStore((s) => s.docs[docId]?.rev);
  const report = usePrepress((s) => {
    const d = s.open ? s.docs[docId] : undefined;
    return d && d.showAreas && d.report && d.report.page === page && d.report.rev === rev ? d.report.data : null;
  });
  if (!report || report.regions.length === 0) return null;
  return (
    <>
      {report.regions.map((r, i) => (
        <div key={i} className="tac-region" style={{ left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] }} />
      ))}
    </>
  );
}
