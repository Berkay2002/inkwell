import { memo, useEffect, useRef, useState } from "react";
import type { Rect4, Source } from "../../lib/history";
import { renderPage, sourceKey } from "../../lib/history";
import { GLYPH, type Mark, TONE_WORD } from "../../lib/historyLogic";

/** Pixels per point of the rendered pages. */
export const SCALE = 1.5;

const cache = new Map<string, Promise<string>>();

/** Rendered pages are object URLs; drop them when a history view closes. */
export function clearPageCache() {
  for (const p of cache.values()) void p.then((u) => URL.revokeObjectURL(u), () => {});
  cache.clear();
}

function getPage(doc: number | null, src: Source, page: number): Promise<string> {
  const key = `${doc}|${sourceKey(src)}|${page}`;
  let p = cache.get(key);
  if (!p) {
    p = renderPage(doc, src, page, SCALE);
    p.catch(() => cache.delete(key));
    cache.set(key, p);
  }
  return p;
}

export interface PageImgProps {
  doc: number | null;
  source: Source;
  /** 0-based */
  page: number;
  /** height / width until the page itself says */
  ratio: number;
  marks?: Mark[];
  /** Pixel-diff regions nothing else explains (page points). */
  vboxes?: Rect4[];
  /** The change the stepper is on, and a counter that restarts its flash. */
  current?: number | null;
  flash?: number;
  onRatio?: (r: number) => void;
  onMark?: (change: number) => void;
  /** Called with the element once, for scroll-to. */
  label?: string;
}

const pct = (v: number, of: number) => `${(v / of) * 100}%`;

/** One page of one source: loads when near the viewport, draws marks over it in page space. */
export const PageImg = memo(function PageImg({ doc, source, page, ratio, marks, vboxes, current, flash, onRatio, onMark, label }: PageImgProps) {
  const el = useRef<HTMLDivElement>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [pt, setPt] = useState<[number, number] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [near, setNear] = useState(false);
  const key = `${doc}|${sourceKey(source)}|${page}`;

  useEffect(() => {
    setUrl(null);
    setPt(null);
    setError(null);
    const node = el.current;
    if (!node) return;
    const io = new IntersectionObserver(([e]) => e.isIntersecting && setNear(true), { rootMargin: "900px 0px" });
    io.observe(node);
    return () => io.disconnect();
  }, [key]);

  useEffect(() => {
    if (!near) return;
    let live = true;
    getPage(doc, source, page).then(
      (u) => live && setUrl(u),
      (e) => live && setError(String(e)),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [near, key]);

  return (
    <div ref={el} className="hx-page" data-page={page} style={{ aspectRatio: `1 / ${ratio}` }} aria-label={label}>
      {url && (
        <img
          src={url}
          alt={label ?? `Page ${page + 1}`}
          draggable={false}
          onLoad={(e) => {
            const i = e.currentTarget;
            setPt([i.naturalWidth / SCALE, i.naturalHeight / SCALE]);
            onRatio?.(i.naturalHeight / i.naturalWidth);
          }}
        />
      )}
      {error && <div className="hx-page-msg">{error}</div>}
      {pt &&
        marks?.map((m, i) => {
          const cur = current === m.change;
          const r = m.rect;
          return (
            <span
              key={`${i}:${cur ? flash : 0}`}
              className={`hx-mark ${m.tone}${cur ? " cur" : ""}${r ? "" : " whole"}`}
              data-change={m.change}
              style={r ? { left: pct(r[0], pt[0]), top: pct(r[1], pt[1]), width: pct(r[2] - r[0], pt[0]), height: pct(r[3] - r[1], pt[1]) } : undefined}
              title={TONE_WORD[m.tone]}
              onClick={() => onMark?.(m.change)}
            >
              <i aria-label={TONE_WORD[m.tone]}>{GLYPH[m.tone]}</i>
            </span>
          );
        })}
      {pt &&
        vboxes?.map((r, i) => (
          <span
            key={`v${i}`}
            className="hx-vbox"
            style={{ left: pct(r[0], pt[0]), top: pct(r[1], pt[1]), width: pct(r[2] - r[0], pt[0]), height: pct(r[3] - r[1], pt[1]) }}
          />
        ))}
    </div>
  );
});
