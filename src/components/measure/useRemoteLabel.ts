import { useEffect, useRef, useState } from "react";
import { measureApi } from "../../lib/measure";
import type { MeasureKind, Pt } from "../../lib/measureLogic";

export interface ValueReq {
  doc: number;
  page: number;
  kind: MeasureKind;
  pts: Pt[];
  /** Value under this measurement's own /Measure; null: under the page's, at the first point. */
  xref: number | null;
}

/**
 * The engine's label for points that change while the pointer moves (a file's own /D and /A can't be worked out in the
 * UI). One request runs at a time and, when it lands, asks again if the points moved on; `null` for no request, and
 * until the first answer.
 */
export function useRemoteLabel(req: ValueReq | null): string | null {
  const [text, setText] = useState<string | null>(null);
  const key = req ? JSON.stringify([req.doc, req.page, req.kind, req.xref, req.pts]) : "";
  const want = useRef({ key, req });
  want.current = { key, req };
  const busy = useRef(false);

  useEffect(() => {
    if (!key) return void setText(null);
    if (busy.current) return;
    const ask = () => {
      const { key: k, req: r } = want.current;
      if (!r) return;
      busy.current = true;
      void measureApi
        .value(r.doc, r.page, r.kind, r.pts, r.xref)
        .then(
          (q) => want.current.key === k && setText(q.text),
          () => want.current.key === k && setText(null), // not countable (too few points): no label
        )
        .finally(() => {
          busy.current = false;
          if (want.current.key && want.current.key !== k) ask();
        });
    };
    ask();
  }, [key]);

  return key ? text : null;
}
