import { create } from "zustand";

/**
 * The print-production view the page tiles are drawn in: set by Output Preview (lib/prepressStore.ts) for the
 * document it is open on, empty otherwise. Kept apart from that store so the viewer and the dark-pages rule can
 * read it without pulling in everything Output Preview talks to.
 */
export const usePlates = create<{ doc: number | null; view: number | null }>(() => ({ doc: null, view: null }));
