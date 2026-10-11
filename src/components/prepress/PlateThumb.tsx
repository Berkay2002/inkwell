import { useEffect, useRef, useState } from "react";
import type { Size } from "../../lib/layout";
import { plateThumb } from "../../lib/plateThumbs";
import type { PageSeps } from "../../lib/prepressApi";
import type { Plate } from "../../lib/prepressLogic";

/** One plate of the current page as a small picture; drawn when it scrolls into view. Click shows it alone. */
export function PlateThumb(p: {
  doc: number;
  page: number;
  rev: number;
  size: Size;
  plate: Plate;
  seps: PageSeps | null;
  overprint: boolean;
  solo: boolean;
  onClick: () => void;
}) {
  const box = useRef<HTMLButtonElement>(null);
  const [seen, setSeen] = useState(false);
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    const el = box.current;
    if (!el || seen) return;
    const io = new IntersectionObserver((e) => e.some((x) => x.isIntersecting) && setSeen(true));
    io.observe(el);
    return () => io.disconnect();
  }, [seen]);
  useEffect(() => {
    if (!seen) return;
    let live = true;
    setUrl(null);
    void plateThumb(p.doc, p.page, p.size, p.rev, p.plate.key, p.seps, p.overprint).then((u) => live && setUrl(u));
    return () => {
      live = false;
    };
  }, [seen, p.doc, p.page, p.rev, p.plate.key, p.overprint, p.seps]);
  const [w, h] = p.size;
  return (
    <button ref={box} className={p.solo ? "pp-thumb on" : "pp-thumb"} aria-pressed={p.solo} title={p.solo ? "Show all plates again" : `Show ${p.plate.name} alone`} onClick={p.onClick}>
      <span className="pp-thumb-page" style={{ aspectRatio: `${w} / ${h}` }}>
        {url && <img src={url} alt="" draggable={false} />}
      </span>
      <span className="pp-thumb-name">
        <i className="pp-swatch" style={{ background: `rgb(${p.plate.rgb.join(",")})` }} />
        {p.plate.name}
      </span>
    </button>
  );
}
