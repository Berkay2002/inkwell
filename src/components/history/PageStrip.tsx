import { memo, useEffect, useRef, useState } from "react";
import { type Source, renderPage, sourceKey } from "../../lib/history";
import type { Row } from "../../lib/historyLogic";
import { Icon } from "../Icons";

/** Pixels per point of a thumbnail; the pages are drawn small, so this is cheap. */
const THUMB_SCALE = 0.2;
const cache = new Map<string, Promise<string>>();

/** Thumbnails are object URLs; drop them when the view closes. */
export function clearThumbs() {
  for (const p of cache.values()) void p.then((u) => URL.revokeObjectURL(u), () => {});
  cache.clear();
}

function getThumb(doc: number | null, src: Source, page: number): Promise<string> {
  const key = `${doc}|${sourceKey(src)}|${page}`;
  let p = cache.get(key);
  if (!p) {
    p = renderPage(doc, src, page, THUMB_SCALE);
    p.catch(() => cache.delete(key));
    cache.set(key, p);
  }
  return p;
}

/** The first look at one page of one source; loads when it scrolls near. */
export const Thumb = memo(function Thumb({ doc, source, page, label }: { doc: number | null; source: Source; page: number; label: string }) {
  const el = useRef<HTMLSpanElement>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [near, setNear] = useState(false);
  const key = `${doc}|${sourceKey(source)}|${page}`;

  useEffect(() => {
    setUrl(null);
    setNear(false);
    const n = el.current;
    if (!n) return;
    const io = new IntersectionObserver(([e]) => e.isIntersecting && setNear(true), { rootMargin: "240px" });
    io.observe(n);
    return () => io.disconnect();
  }, [key]);

  useEffect(() => {
    if (!near) return;
    let live = true;
    getThumb(doc, source, page).then(
      (u) => live && setUrl(u),
      () => {},
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [near, key]);

  return (
    <span ref={el} className="hx-thumb">
      {url && <img src={url} alt={label} draggable={false} />}
    </span>
  );
});

const reduced = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

/** The pages of both sides along the bottom. They are linked: the active row lights its page on each side, and a click on
 *  any thumbnail takes both panes to that row. */
export function PageStrips({
  doc,
  a,
  b,
  names,
  pages,
  rows,
  active,
  changed,
  onGo,
}: {
  doc: number | null;
  a: Source;
  b: Source;
  names: [string, string];
  pages: [number, number];
  rows: Row[];
  /** The row the panes are showing. */
  active: number;
  /** Pages with differences, per side. */
  changed: [Set<number>, Set<number>];
  onGo: (row: number) => void;
}) {
  const rowOfPage = useRef<[Map<number, number>, Map<number, number>]>([new Map(), new Map()]);
  rowOfPage.current = [new Map(), new Map()];
  rows.forEach((r, i) => {
    if (r.a !== null) rowOfPage.current[0].set(r.a, i);
    if (r.b !== null) rowOfPage.current[1].set(r.b, i);
  });
  const here = rows[active];
  return (
    <div className="hx-pstrips" aria-label="Pages">
      <Strip side="a" doc={doc} source={a} name={names[0]} count={pages[0]} active={here?.a ?? null} changed={changed[0]} rowOf={rowOfPage.current[0]} onGo={onGo} />
      <span className="hx-plink" title="The pages are linked: they move together" aria-hidden>
        <Icon name="link" size={16} />
      </span>
      <Strip side="b" doc={doc} source={b} name={names[1]} count={pages[1]} active={here?.b ?? null} changed={changed[1]} rowOf={rowOfPage.current[1]} onGo={onGo} />
    </div>
  );
}

function Strip({
  side,
  doc,
  source,
  name,
  count,
  active,
  changed,
  rowOf,
  onGo,
}: {
  side: "a" | "b";
  doc: number | null;
  source: Source;
  name: string;
  count: number;
  active: number | null;
  changed: Set<number>;
  rowOf: Map<number, number>;
  onGo: (row: number) => void;
}) {
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const c = box.current;
    const n = active === null ? null : c?.querySelector<HTMLElement>(`[data-p="${active}"]`);
    if (!c || !n) return;
    const left = n.offsetLeft - c.clientWidth / 2 + n.clientWidth / 2;
    c.scrollTo({ left: Math.max(0, left), behavior: reduced() ? "auto" : "smooth" });
  }, [active]);

  const move = (e: React.KeyboardEvent) => {
    const dir = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!dir) return;
    const all = [...(box.current?.querySelectorAll<HTMLButtonElement>("button.hx-pthumb") ?? [])];
    const i = all.indexOf(document.activeElement as HTMLButtonElement);
    all[Math.min(all.length - 1, Math.max(0, i + dir))]?.focus();
    e.preventDefault();
  };

  return (
    <div className="hx-pstrip" data-side={side}>
      <div className="hx-pstrip-head">
        <strong title={name}>{name}</strong>
        <span className="dim">
          {count} {count === 1 ? "page" : "pages"}
        </span>
      </div>
      <div className="hx-pstrip-box" ref={box} onKeyDown={move}>
        {Array.from({ length: count }, (_, p) => {
          const row = rowOf.get(p);
          return (
            <button
              key={p}
              data-p={p}
              className={`hx-pthumb${active === p ? " on" : ""}${changed.has(p) ? " changed" : ""}`}
              aria-current={active === p}
              aria-label={`${side === "a" ? "Old" : "New"} page ${p + 1}${changed.has(p) ? ", has differences" : ""}`}
              disabled={row === undefined}
              onClick={() => row !== undefined && onGo(row)}
            >
              <Thumb doc={doc} source={source} page={p} label={`Page ${p + 1}`} />
              <span className="hx-pnum">{p + 1}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
