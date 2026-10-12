import { BANNER_HEIGHT, barTops } from "./replaceLogic";
import { bannerShown, useSign } from "./sign";

/**
 * Where the Find bar and what hangs from it sit for a document: the signature banner takes the strip at the top, so
 * with it showing they go below it, by the banner's own height (`SignBanner` measures it, its text wraps in a narrow
 * window).
 */
export function useBarTops(doc: number) {
  const banner = useSign((s) => bannerShown(s.docs[doc]));
  const height = useSign((s) => s.docs[doc]?.bannerHeight ?? null);
  return barTops(banner ? (height ?? BANNER_HEIGHT) : null);
}
