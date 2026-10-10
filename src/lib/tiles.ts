import { fetchTile, type TileData } from "./api";
import { type TileRef, tileKeyStr } from "./layout";

/** Streams keep independent staleness counters in the engine: 0 = main view, 1 = thumbnails. */
export type Stream = 0 | 1;

interface Sub {
  draw: (d: TileData) => void;
  drawn: boolean;
}

export interface Want {
  doc: number;
  tile: TileRef;
}

/**
 * Fetches tiles for whatever the view currently wants and hands pixels to the canvases that asked
 * for them. Whenever the wanted set changes the epoch advances; the engine drops queued jobs from
 * older epochs, so tiles that are still wanted are simply requested again with the new epoch.
 */
class TileClient {
  private subs = new Map<string, Sub>();
  private epoch: [number, number] = [0, 0];
  private signature: [string, string] = ["", ""];
  private wanted: [Want[], Want[]] = [[], []];
  /** Keys with a request on the wire: at most one per tile, or duplicate responses pile up in memory. */
  private pending = new Set<string>();

  register(stream: Stream, doc: number, tile: TileRef, sub: Sub) {
    this.subs.set(`${stream}:${tileKeyStr(doc, tile)}`, sub);
  }

  unregister(stream: Stream, doc: number, tile: TileRef, sub: Sub) {
    const k = `${stream}:${tileKeyStr(doc, tile)}`;
    if (this.subs.get(k) === sub) this.subs.delete(k);
  }

  /** Declares the full ordered set of tiles the stream wants right now (most important first). */
  want(stream: Stream, wanted: Want[]) {
    const sig = wanted.map((w) => tileKeyStr(w.doc, w.tile)).join("|");
    if (sig === this.signature[stream]) return;
    this.signature[stream] = sig;
    this.wanted[stream] = wanted;
    this.epoch[stream]++;
    this.issue(stream);
  }

  /**
   * Requests every wanted, undrawn tile that has no request in flight, tagged with the current
   * epoch. A request overtaken by a newer epoch comes back stale and is re-issued from here if
   * the tile is still wanted.
   */
  private issue(stream: Stream) {
    const epoch = this.epoch[stream];
    this.wanted[stream].forEach((w, rank) => {
      const key = `${stream}:${tileKeyStr(w.doc, w.tile)}`;
      if (this.pending.has(key) || this.subs.get(key)?.drawn) return;
      this.pending.add(key);
      const t = w.tile;
      fetchTile(w.doc, t.page, t.rot, t.scale, t.tx, t.ty, epoch, rank, stream).then(
        (data) => {
          this.pending.delete(key);
          if (!data) return this.issue(stream);
          const sub = this.subs.get(key);
          if (sub && !sub.drawn) sub.draw(data);
        },
        () => {
          // a failed tile stays blank; the preview underneath remains
          this.pending.delete(key);
        },
      );
    });
  }

  /** Forget the wanted set, e.g. when the viewer unmounts. */
  clear(stream: Stream) {
    this.signature[stream] = "";
    this.wanted[stream] = [];
  }
}

export const tileClient = new TileClient();

const textCache = new Map<string, Promise<unknown>>();
/** Tiny LRU for promise-valued lookups (page text). */
export function cached<T>(key: string, make: () => Promise<T>, limit = 96): Promise<T> {
  const hit = textCache.get(key);
  if (hit) {
    textCache.delete(key);
    textCache.set(key, hit);
    return hit as Promise<T>;
  }
  const p = make();
  textCache.set(key, p);
  if (textCache.size > limit) textCache.delete(textCache.keys().next().value!);
  p.catch(() => textCache.delete(key));
  return p;
}

export function dropCached(prefix: string) {
  for (const k of [...textCache.keys()]) if (k.startsWith(prefix)) textCache.delete(k);
}
