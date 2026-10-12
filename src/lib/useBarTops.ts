import { useEffect, useState } from "react";
import { BANNER_HEIGHT, barTops } from "./replaceLogic";
import { bannerShown, useSign } from "./sign";

/**
 * Where the Find bar and what hangs from it sit for a document: the signature banner takes the strip at the top, so
 * with it showing they go below it, by the banner's own height (its text wraps in a narrow window).
 */
export function useBarTops(doc: number) {
  const banner = useSign((s) => bannerShown(s.docs[doc]));
  const [height, setHeight] = useState<number | null>(null);
  useEffect(() => {
    const el = banner ? document.querySelector<HTMLElement>(".sg-banner") : null;
    if (!el) return setHeight(null);
    const measure = () => setHeight(el.offsetHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [banner]);
  return barTops(banner ? (height ?? BANNER_HEIGHT) : null);
}
