import { useEffect, useMemo, useRef, useState } from "react";
import { type Comparison } from "../../lib/history";
import { marksFor, stepIndex, versionTitle, when, GLYPH, toneOf } from "../../lib/historyLogic";
import { useHistory } from "../../lib/historyStore";
import type { DocState } from "../../store";
import { Icon } from "../Icons";
import { PageImg, SCALE, clearPageCache } from "./PageImg";

const EMPTY: never[] = [];

/** A past version in the main view: read-only, its changes (against its parent) marked on the pages. */
export function VersionView({ doc, versionId }: { doc: DocState; versionId: string }) {
  const h = useHistory.getState();
  const v = useHistory((s) => s.docs[doc.id]?.versions.find((x) => x.id === versionId));
  const ch = useHistory((s) => s.changes[`${doc.id}:${versionId}`]);
  const cur = useHistory((s) => s.current[doc.id] ?? null);
  const scroller = useRef<HTMLDivElement>(null);
  const [ratios, setRatios] = useState<Record<number, number>>({});
  const [flash, setFlash] = useState(0);
  const cmp: Comparison | null = ch && ch !== "loading" && !("error" in ch) ? ch : null;
  const pageCount = cmp && cmp.pages.bPages > 0 ? cmp.pages.bPages : doc.pageCount;
  const marks = useMemo(() => (cmp ? marksFor(cmp.changes, "b") : EMPTY), [cmp]);
  const byPage = useMemo(() => {
    const m = new Map<number, typeof marks>();
    for (const k of marks) m.set(k.page, [...(m.get(k.page) ?? []), k]);
    return m;
  }, [marks]);
  const source = useMemo(() => ({ kind: "version" as const, id: versionId }), [versionId]);

  useEffect(() => () => clearPageCache(), []);

  // First change selected once the list is in.
  useEffect(() => {
    if (cmp && cmp.changes.length > 0 && cur === null) h.setCurrent(doc.id, 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cmp]);

  // Bring the current change into view.
  useEffect(() => {
    if (cur === null || !cmp) return;
    setFlash((n) => n + 1);
    const c = cmp.changes[cur];
    const page = c.bPage;
    const s = scroller.current;
    if (page === null || !s) return;
    const el = s.querySelector<HTMLElement>(`[data-page="${page}"]`);
    if (!el) return;
    const top = el.offsetTop + (c.rectB ? scrollOffset(el, c.rectB[1]) : 0) - 140;
    s.scrollTo({ top: Math.max(0, top), behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cur, cmp]);

  if (!v) return null;
  const n = cmp?.changes.length ?? 0;
  const step = (d: 1 | -1) => h.setCurrent(doc.id, stepIndex(cur ?? -1, d, n));
  const tone = cur !== null && cmp ? toneOf(cmp.changes[cur]) : null;
  return (
    <div className="hx-version">
      <div className="hx-vbar" role="region" aria-label="Viewing a past version">
        <div className="hx-vbar-text">
          <strong>Viewing version from {when(v.created, Date.now())}</strong>
          <span className="dim">{versionTitle(v)}</span>
        </div>
        <span className="grow" />
        {n > 0 && (
          <div className="hx-stepper" aria-label="Changes">
            <button className="icon-btn" title="Previous change" aria-label="Previous change" onClick={() => step(-1)}>
              <Icon name="up" size={14} />
            </button>
            <span className="hx-step-label">
              {tone && <span className={`hx-glyph ${tone}`}>{GLYPH[tone]}</span>}
              {cur !== null ? `${cur + 1} of ${n}` : `${n} changes`}
            </span>
            <button className="icon-btn" title="Next change" aria-label="Next change" onClick={() => step(1)}>
              <Icon name="down" size={14} />
            </button>
          </div>
        )}
        <button className="hx-btn primary" onClick={() => void h.restore(doc.id, versionId)}>
          Restore this version
        </button>
        <button className="hx-btn" onClick={() => void h.saveCopy(doc.id, versionId)}>
          Save a copy
        </button>
        <button className="hx-btn" onClick={() => h.select(doc.id, null)}>
          Back to current
        </button>
      </div>
      <div className="hx-scroll" ref={scroller}>
        <div className="hx-pages">
          {Array.from({ length: pageCount }, (_, p) => {
            const sz = doc.sizes[p];
            const ratio = ratios[p] ?? (sz && doc.pageCount === pageCount ? sz[1] / sz[0] : 1.294);
            return (
              <PageImg
                key={p}
                doc={doc.id}
                source={source}
                page={p}
                ratio={ratio}
                marks={byPage.get(p)}
                current={cur}
                flash={flash}
                onRatio={(r) => setRatios((o) => (Math.abs((o[p] ?? 0) - r) < 1e-3 ? o : { ...o, [p]: r }))}
                onMark={(i) => h.setCurrent(doc.id, i)}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}

/** Pixels from the top of the page element for a y in page points (the page element's height spans the page). */
function scrollOffset(el: HTMLElement, yPt: number): number {
  const img = el.querySelector("img");
  const hPt = img ? img.naturalHeight / SCALE : 0;
  return hPt ? (yPt / hPt) * el.clientHeight : 0;
}
