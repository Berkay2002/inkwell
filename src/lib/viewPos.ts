/**
 * Where each document's viewer is right now: the page at the top of the view and how far down it
 * the view starts (points). The viewer keeps it up to date while scrolling; "set to current view"
 * for a bookmark or a link reads it.
 */
export interface ViewPos {
  page: number;
  yPt: number;
}

const live = new Map<number, ViewPos>();

export const setViewPos = (doc: number, pos: ViewPos) => void live.set(doc, pos);
export const getViewPos = (doc: number): ViewPos | null => live.get(doc) ?? null;
export const dropViewPos = (doc: number) => void live.delete(doc);
