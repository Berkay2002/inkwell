import { create } from "zustand";

/**
 * The print-production view the page tiles are drawn in: set by Output Preview (lib/prepressStore.ts) for the
 * document it is open on, empty otherwise. Kept apart from that store so the viewer and the dark-pages rule can
 * read it without pulling in everything Output Preview talks to.
 * `lost` counts the times a tile of the current view failed: the engine may have dropped the view, so Output
 * Preview asks for it again.
 */
export const usePlates = create<{ doc: number | null; view: number | null; lost: number }>(() => ({ doc: null, view: null, lost: 0 }));

/** A tile of `view` failed. Only the view in use counts; late failures of an older one do not. */
export const viewFailed = (view: number) => usePlates.setState((s) => (s.view === view ? { lost: s.lost + 1 } : s));
