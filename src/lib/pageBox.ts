/**
 * Width of the toolbar's page-number box in `ch`: the digits of the page count plus about a character of
 * padding, never under 3 or over 6. A document with page labels ("iii", "A-12") can show longer text, so it
 * gets the widest box.
 */
export function pageBoxCh(pageCount: number, labelled: boolean): number {
  if (labelled) return 6;
  const digits = String(Math.max(1, Math.floor(pageCount))).length;
  return Math.min(6, Math.max(3, digits + 1));
}
