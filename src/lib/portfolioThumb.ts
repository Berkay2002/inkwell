import * as api from "./api";
import type { PortfolioFile } from "./api";

/** An embedded PDF past this size (decoded) gets its file-type icon; the engine refuses it too. */
export const THUMB_MAX_BYTES = 32 << 20;
/** At most this many renders are asked of the engine at once (it draws one at a time anyway). */
const MAX_RUNNING = 2;
const CACHE_MAX = 300;

/** What a thumbnail came to: a picture, or the short reason there is none. */
export type ThumbResult = { url: string; note?: undefined } | { url?: undefined; note: string };

/** Whether a file may be drawn at all: an embedded PDF with its bytes in the document, under the size cap. */
export function thumbable(f: Pick<PortfolioFile, "isPdf" | "hasData" | "size">): boolean {
  return f.isPdf && f.hasData && f.size <= THUMB_MAX_BYTES;
}

/** The note for a file that has no picture and is not simply another type: why the icon stands in. */
export function thumbNote(f: Pick<PortfolioFile, "isPdf" | "hasData" | "size">): string | null {
  if (!f.isPdf) return null;
  if (!f.hasData) return "This file is not stored in the portfolio";
  if (f.size > THUMB_MAX_BYTES) return `Too large to preview (over ${THUMB_MAX_BYTES >> 20} MB)`;
  return null;
}

interface Job {
  key: string;
  doc: number;
  id: string;
  side: number;
  subs: Set<(r: ThumbResult) => void>;
  started: boolean;
}

const cache = new Map<string, ThumbResult>();
const jobs = new Map<string, Job>();
let queue: Job[] = [];
let running = 0;

const keyOf = (doc: number, id: string, side: number) => `${doc}\u0000${id}\u0000${side}`;

/** PNG bytes as a data: URL, so a picture needs no object URL to revoke when the cache lets it go. */
function toDataUrl(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return `data:image/png;base64,${btoa(bin)}`;
}

function remember(key: string, r: ThumbResult) {
  cache.delete(key);
  cache.set(key, r);
  while (cache.size > CACHE_MAX) cache.delete(cache.keys().next().value as string);
}

function pump() {
  while (running < MAX_RUNNING && queue.length) {
    const job = queue.shift() as Job;
    job.started = true;
    running++;
    void api
      .portfolioThumb(job.doc, job.id, job.side)
      .then((buf): ThumbResult => ({ url: toDataUrl(buf) }))
      .catch((e): ThumbResult => ({ note: String(e).replace(/^Error:\s*/, "") || "No preview" }))
      .then((r) => {
        running--;
        jobs.delete(job.key);
        remember(job.key, r);
        for (const cb of job.subs) cb(r);
        pump();
      });
  }
}

/**
 * Asks for the picture of page 1 of an embedded PDF. `cb` gets the result (at once when it is known). A request
 * that has not started when its last asker goes away (a tile scrolled off) is dropped, so a fast scroll over
 * thousands of files costs only the tiles that were seen. Returns the cancel.
 */
export function requestThumb(doc: number, id: string, side: number, cb: (r: ThumbResult) => void): () => void {
  const key = keyOf(doc, id, side);
  const hit = cache.get(key);
  if (hit) {
    cb(hit);
    return () => {};
  }
  let job = jobs.get(key);
  if (!job) {
    job = { key, doc, id, side, subs: new Set(), started: false };
    jobs.set(key, job);
    queue.push(job);
  }
  const j = job;
  j.subs.add(cb);
  pump();
  return () => {
    j.subs.delete(cb);
    if (!j.started && j.subs.size === 0) {
      queue = queue.filter((q) => q !== j);
      jobs.delete(j.key);
    }
  };
}

/** The remembered pictures of a document (its files changed, or it closed). */
export function dropThumbs(doc: number) {
  const prefix = `${doc}\u0000`;
  for (const k of [...cache.keys()]) if (k.startsWith(prefix)) cache.delete(k);
}
