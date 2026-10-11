/**
 * The page range of a file in Combine: "1-3, 6, 9-7" (1-based, in the order written, a backwards range
 * reads backwards, "5-" runs to the end, empty is every page). The same grammar as the engine's
 * `parse_order` (src-tauri/src/organize/combine.rs); the text stays the source of truth and the
 * page thumbnails only rewrite it.
 */

/** 0-based pages in the order written; null when the text does not fit a file of `count` pages. */
export function parseOrder(text: string, count: number): number[] | null {
  const t = text.trim();
  if (t === "" || t.toLowerCase() === "all") return Array.from({ length: count }, (_, i) => i);
  const out: number[] = [];
  for (const raw of t.split(/[,;]/)) {
    const part = raw.trim();
    if (!part) continue;
    const m = /^(\d+)\s*(?:(-)\s*(\d*))?$/.exec(part);
    if (!m) return null;
    const a = Number(m[1]);
    const b = m[2] ? (m[3] === "" ? count : Number(m[3])) : a;
    if (a < 1 || a > count || b < 1 || b > count) return null;
    if (a <= b) for (let p = a; p <= b; p++) out.push(p - 1);
    else for (let p = a; p >= b; p--) out.push(p - 1);
  }
  return out.length > 0 ? out : null;
}

/** 0-based pages in order -> text: runs that go up or down by one are written as ranges. Every page in order is "". */
export function formatOrder(pages: number[], count: number): string {
  if (pages.length === count && pages.every((p, i) => p === i)) return "";
  const parts: string[] = [];
  for (let i = 0; i < pages.length; ) {
    let j = i;
    const step = pages[i + 1] === pages[i] + 1 ? 1 : pages[i + 1] === pages[i] - 1 ? -1 : 0;
    if (step !== 0) while (j + 1 < pages.length && pages[j + 1] === pages[j] + step) j++;
    parts.push(i === j ? `${pages[i] + 1}` : `${pages[i] + 1}-${pages[j] + 1}`);
    i = j + 1;
  }
  return parts.join(", ");
}

/**
 * The range text after a click on page `page` (0-based) of a file of `count` pages: a page that is in
 * goes out, one that is not comes in at the end. The last page cannot be taken out (a file with no
 * pages is removed with its row instead). Null when the text does not parse or nothing changes.
 */
export function togglePage(text: string, count: number, page: number): string | null {
  const have = parseOrder(text, count);
  if (!have || page < 0 || page >= count) return null;
  if (have.includes(page)) {
    const rest = have.filter((p) => p !== page);
    return rest.length === 0 ? null : formatOrder(rest, count);
  }
  return formatOrder([...have, page], count);
}

/** The pages `text` takes, as a set for drawing the toggles; null while the text does not parse. */
export function pagesIn(text: string, count: number): Set<number> | null {
  const p = parseOrder(text, count);
  return p ? new Set(p) : null;
}
