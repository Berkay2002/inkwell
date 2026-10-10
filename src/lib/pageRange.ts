/** "1, 3-5" (1-based, as typed) -> sorted unique 0-based pages below `count`; null when it does not parse. */
export function parseRange(text: string, count: number): number[] | null {
  const out = new Set<number>();
  for (const part of text.split(",")) {
    const p = part.trim();
    if (!p) continue;
    const m = /^(\d+)\s*(?:-\s*(\d+))?$/.exec(p);
    if (!m) return null;
    let a = Number(m[1]);
    let b = m[2] ? Number(m[2]) : a;
    if (a > b) [a, b] = [b, a];
    if (a < 1 || b > count) return null;
    for (let i = a; i <= b; i++) out.add(i - 1);
  }
  return [...out].sort((x, y) => x - y);
}

/** 0-based pages -> "1, 3-5". Runs of two are written as a range too ("3-4"). */
export function formatRange(pages: Iterable<number>): string {
  const s = [...new Set(pages)].sort((x, y) => x - y);
  const runs: string[] = [];
  for (let i = 0; i < s.length; ) {
    let j = i;
    while (j + 1 < s.length && s[j + 1] === s[j] + 1) j++;
    runs.push(i === j ? `${s[i] + 1}` : `${s[i] + 1}-${s[j] + 1}`);
    i = j + 1;
  }
  return runs.join(", ");
}

/** Where the pages end up after moving them to gap `to` (engine `move` semantics). */
export function movedIndices(pages: number[], to: number): number[] {
  const first = to - pages.filter((p) => p < to).length;
  return pages.map((_, i) => first + i);
}
