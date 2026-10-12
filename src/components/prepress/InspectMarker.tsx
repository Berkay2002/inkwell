import { usePrepress } from "../../lib/prepressStore";

/** The point the object inspector was asked about, in page points (the space `.pagespace` is in), the same size at any zoom. */
export function InspectMarker({ docId, page, zoom }: { docId: number; page: number; zoom: number }) {
  const at = usePrepress((s) => (s.open ? s.docs[docId]?.inspect.at : null));
  if (!at || at.page !== page) return null;
  return <div className="pp-mark" style={{ left: at.x, top: at.y, transform: `translate(-50%, -50%) scale(${1 / zoom})` }} />;
}
