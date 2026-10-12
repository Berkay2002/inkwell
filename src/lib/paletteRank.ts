/** What the palette matches on: the visible title, and extra words (tool label, keywords, description) behind it. */
export interface Rankable {
  title: string;
  words?: string;
}

/** 0 the title starts with the word, 1 a word of the title does, 2 the title contains it, 3 only the extra words do. */
function tier(title: string, words: string, w: string): number | null {
  if (title.startsWith(w)) return 0;
  if (title.split(/[^\p{L}\p{N}]+/u).some((t) => t.startsWith(w))) return 1;
  if (title.includes(w)) return 2;
  return words.includes(w) ? 3 : null;
}

/**
 * Items that contain every word of the query, best first: a match in the title beats one in a description, whatever
 * the list's own order ("page": Organize pages before Edit links, whose description mentions pages). Ties keep the
 * list's order, and an empty query keeps all of it.
 */
export function rankPalette<T extends Rankable>(items: T[], query: string): T[] {
  const ws = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (ws.length === 0) return items;
  const scored: { item: T; score: number; at: number }[] = [];
  items.forEach((item, at) => {
    const title = item.title.toLowerCase();
    const words = (item.words ?? "").toLowerCase();
    let score = 0;
    for (const w of ws) {
      const t = tier(title, words, w);
      if (t === null) return;
      score += t;
    }
    scored.push({ item, score, at });
  });
  return scored.sort((a, b) => a.score - b.score || a.at - b.at).map((s) => s.item);
}
