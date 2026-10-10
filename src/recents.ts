import { invoke } from "@tauri-apps/api/core";
import { type StoreApi, create } from "zustand";
import * as api from "./lib/api";
import { previewScaleKey, PREVIEW_EDGE, TILE, type Size } from "./lib/layout";
import { type Recent, clean, fromLegacy, isJunk, keyOf, parse, upsert } from "./lib/recent";

interface FileStat {
  path: string;
  exists: boolean;
  size: number;
  modified: number;
}

const statFiles = (paths: string[]) => invoke<FileStat[]>("stat_files", { paths });

/** Spare tile stream of the engine: a thumbnail never delays the page view or the sidebar. */
const THUMB_STREAM = 2;
const LEGACY_KEY = "inkwell.recent";

interface RecentsStore {
  items: Recent[];
  /** Existence by `keyOf(path)`, filled by `refresh`; absent means not checked yet. */
  exists: Record<string, boolean>;
  ready: boolean;
  /** `locked`: the document needed a password, so no first-page image of it is kept on disk. */
  record: (info: api.OpenInfo, locked?: boolean) => Promise<void>;
  refresh: () => Promise<void>;
  backfillThumbs: () => Promise<void>;
  togglePin: (path: string) => void;
  remove: (path: string) => void;
  /** Forgets every unpinned entry. */
  clear: () => void;
  /** Remembers the page (0-based) the viewer was on, for "Continue reading". */
  setPage: (path: string, page: number) => void;
  /** Where reading stopped, once the stored list is loaded. */
  lastPageOf: (path: string) => Promise<number | undefined>;
  relocate: (oldPath: string, newPath: string) => Promise<void>;
}

/** WebP of the first page from a tile of the preview scale: small enough to live inside recents.json. */
function encodeThumb(t: api.TileData): string {
  const c = document.createElement("canvas");
  c.width = t.w;
  c.height = t.h;
  c.getContext("2d")!.putImageData(new ImageData(t.rgba as Uint8ClampedArray<ArrayBuffer>, t.w, t.h), 0, 0);
  return c.toDataURL("image/webp", 0.82);
}

/** First page of an already open document, or null when the engine declines (closed, superseded). */
async function thumbOf(doc: number, firstPage: Size): Promise<string | null> {
  const t = await api.fetchTile(doc, 0, 0, previewScaleKey(firstPage), 0, 0, 1, 0, THUMB_STREAM, 0);
  return t && t.w <= PREVIEW_EDGE + 1 && t.h <= PREVIEW_EDGE + 1 ? encodeThumb(t) : null;
}

let saving: Promise<unknown> = Promise.resolve();
/** Paths whose thumbnail could not be made this session (encrypted, damaged): not retried until restart. */
const noThumb = new Set<string>();
let backfilling = false;

export const useRecents = create<RecentsStore>((set, get) => {
  /** Applies `f` to the list once loaded, then saves. Saves are chained so the last one wins on disk. */
  const mutate = async (f: (l: Recent[]) => Recent[]) => {
    await loaded;
    const items = f(get().items);
    if (items === get().items) return;
    set({ items });
    saving = saving.then(() => invoke("recents_save", { json: JSON.stringify(items) })).catch(() => {});
  };

  const patchItem = (path: string, p: Partial<Recent>) =>
    mutate((l) => l.map((r) => (keyOf(r.path) === keyOf(path) ? { ...r, ...p } : r)));

  const loaded = (async () => {
    let items: Recent[] = [];
    let migrated = false;
    try {
      const r = await invoke<{ json: string | null; isolated: boolean }>("recents_load");
      const stored = parse(r.json);
      if (stored) items = stored;
      else if (!r.isolated) {
        // first run of the stored list: take over the old localStorage one
        try {
          items = fromLegacy(JSON.parse(localStorage.getItem(LEGACY_KEY) ?? "[]"));
        } catch {
          /* unreadable legacy list: start empty */
        }
        migrated = true;
      }
      const cleaned = clean(items);
      if (migrated || cleaned.length !== items.length) {
        items = cleaned;
        await invoke("recents_save", { json: JSON.stringify(items) });
      }
      // The old list stays in localStorage: Rust drops its paths (it was never told about them), and
      // deleting it would lose them for good. See docs/engine-rules.md, "File paths".
    } catch {
      // no shell (plain browser) or unreadable store: an empty list is the safe fallback
    }
    set({ items, ready: true });
  })();

  return {
    items: [],
    exists: {},
    ready: false,

    record: async (info, locked = false) => {
      await loaded;
      let stat: FileStat | undefined;
      try {
        [stat] = await statFiles([info.path]);
      } catch {
        /* keep the path as given */
      }
      const path = stat?.exists ? stat.path : info.path;
      if (isJunk(path)) return;
      const prev = get().items.find((r) => keyOf(r.path) === keyOf(path));
      const fresh = !locked && prev && stat?.exists && prev.mtime === stat.modified && prev.size === stat.size;
      const entry: Recent = {
        path,
        opened: Date.now(),
        pages: info.pageCount,
        size: stat?.exists ? stat.size : prev?.size,
        mtime: stat?.exists ? stat.modified : prev?.mtime,
        thumb: fresh ? prev.thumb : undefined,
        lastPage: prev?.pages === info.pageCount ? prev.lastPage : undefined,
      };
      await mutate((l) => upsert(l, entry));
      if (entry.thumb || locked) return;
      // the document is open already, so its first page costs one tile, usually a cache hit
      try {
        const thumb = await thumbOf(info.doc, info.firstPage);
        if (thumb) await patchItem(path, { thumb });
      } catch {
        /* closed in the meantime: the start screen backfills it */
      }
    },

    refresh: async () => {
      await loaded;
      const items = get().items;
      if (items.length === 0) return;
      let stats: FileStat[];
      try {
        stats = await statFiles(items.map((r) => r.path));
      } catch {
        return;
      }
      const exists: Record<string, boolean> = {};
      let changed = false;
      const next = items.map((r, i) => {
        const s = stats[i];
        exists[keyOf(r.path)] = s.exists;
        if (!s.exists) return r;
        const stale = r.mtime !== undefined && (r.mtime !== s.modified || r.size !== s.size);
        if (s.path === r.path && !stale && r.size === s.size && r.mtime === s.modified) return r;
        changed = true;
        return { ...r, path: s.path, size: s.size, mtime: s.modified, thumb: stale ? undefined : r.thumb };
      });
      set({ exists });
      if (changed) await mutate(() => clean(next));
    },

    /** Makes missing thumbnails one file at a time; password-protected or damaged files keep their placeholder. */
    backfillThumbs: async () => {
      if (backfilling) return;
      backfilling = true;
      try {
        for (const r of get().items) {
          const k = keyOf(r.path);
          if (r.thumb || noThumb.has(k) || get().exists[k] === false) continue;
          let doc: number | null = null;
          try {
            const o = await api.openDocument(r.path);
            doc = o.doc;
            if (o.status !== "opened") throw new Error("password");
            const thumb = await thumbOf(o.doc, o.firstPage);
            if (!thumb) throw new Error("no tile");
            await patchItem(r.path, { thumb, pages: o.pageCount });
          } catch {
            noThumb.add(k);
          } finally {
            if (doc != null) void api.closeDocument(doc);
          }
        }
      } finally {
        backfilling = false;
      }
    },

    togglePin: (path) => void mutate((l) => l.map((r) => (keyOf(r.path) === keyOf(path) ? { ...r, pinned: !r.pinned || undefined } : r))),
    remove: (path) => void mutate((l) => l.filter((r) => keyOf(r.path) !== keyOf(path))),
    clear: () => void mutate((l) => l.filter((r) => r.pinned)),

    setPage: (path, page) =>
      void mutate((l) => {
        const i = l.findIndex((r) => keyOf(r.path) === keyOf(path));
        if (i < 0 || l[i].lastPage === page) return l;
        return l.map((r, n) => (n === i ? { ...r, lastPage: page } : r));
      }),

    lastPageOf: async (path) => {
      await loaded;
      return get().items.find((r) => keyOf(r.path) === keyOf(path))?.lastPage;
    },

    relocate: async (oldPath, newPath) => {
      let s: FileStat | undefined;
      try {
        [s] = await statFiles([newPath]);
      } catch {
        /* fall through with the path as given */
      }
      const path = s?.exists ? s.path : newPath;
      await mutate((l) => {
        const old = l.find((r) => keyOf(r.path) === keyOf(oldPath));
        const rest = l.filter((r) => keyOf(r.path) !== keyOf(oldPath));
        return old ? upsert(rest, { ...old, path, mtime: s?.modified, size: s?.size ?? old.size, thumb: undefined }) : rest;
      });
    },
  };
});

/** Larger first page for the library preview: one tile of TILE px, cached for the last few selections. */
const previews = new Map<string, string>();
const PREVIEWS_KEPT = 12;

/** Data URL of the first page at preview size; null for protected or unreadable files (the caller falls back to the thumbnail). */
export async function previewOf(r: Recent): Promise<string | null> {
  const key = `${keyOf(r.path)}|${r.mtime ?? 0}`;
  const hit = previews.get(key);
  if (hit) return hit;
  let doc: number | null = null;
  try {
    const o = await api.openDocument(r.path);
    doc = o.doc;
    if (o.status !== "opened") return null;
    const edge = Math.max(o.firstPage[0], o.firstPage[1]);
    const t = await api.fetchTile(doc, 0, 0, Math.max(1, Math.floor((TILE / edge) * 1000)), 0, 0, 1, 0, THUMB_STREAM, 0);
    if (!t) return null;
    const url = encodeThumb(t);
    previews.set(key, url);
    if (previews.size > PREVIEWS_KEPT) previews.delete(previews.keys().next().value!);
    return url;
  } catch {
    return null;
  } finally {
    if (doc != null) void api.closeDocument(doc);
  }
}

/** "Show in folder": Explorer with the file selected. */
export const revealInFolder = (path: string) => invoke("reveal_in_folder", { path }).catch(() => {});

interface Tracked {
  docs: Record<number, { id: number; path: string; currentPage: number }>;
  goTo: (id: number, page: number) => void;
}

/**
 * Keeps each open document's page in its recents entry (debounced) and puts a newly opened one back where
 * it was. Takes the main store as an argument: store.ts imports this module, so importing it back would be a cycle.
 */
export function trackReadingPosition(store: StoreApi<Tracked>) {
  return store.subscribe((s, prev) => {
    for (const d of Object.values(s.docs)) {
      const before = prev.docs[d.id];
      if (!before) {
        void useRecents
          .getState()
          .lastPageOf(d.path)
          .then((page) => {
            const now = store.getState().docs[d.id];
            if (page && now && now.currentPage === 0) store.getState().goTo(d.id, page);
          });
      } else if (before.currentPage !== d.currentPage) {
        clearTimeout(timers.get(d.id));
        timers.set(
          d.id,
          setTimeout(() => {
            timers.delete(d.id);
            useRecents.getState().setPage(d.path, d.currentPage);
          }, 700),
        );
      }
    }
  });
}
const timers = new Map<number, ReturnType<typeof setTimeout>>();
