import * as api from "./api";
import type { Size } from "./layout";
import { prepressApi, type PageSeps } from "./prepressApi";
import { DEFAULT_OUTPUT, plateOnly } from "./prepressLogic";

/**
 * Small pictures of a page for the Output Preview filmstrip and the dialogs' previews: one tile each, drawn once
 * and kept per document generation. They use tile streams of their own (2 and 3) so the page never waits for them.
 */

const PAGE_STREAM = 2;
const PLATE_STREAM = 3;
const EDGE = 160;
const KEEP = 48;

const done = new Map<string, string | null>();
const flying = new Map<string, Promise<string | null>>();

const scaleFor = (size: Size) => Math.max(1, Math.floor((EDGE / Math.max(size[0], size[1])) * 1000));

function remember(key: string, url: string | null) {
  done.delete(key);
  done.set(key, url);
  if (done.size > KEEP) done.delete(done.keys().next().value as string);
}

function once(key: string, make: () => Promise<string | null>): Promise<string | null> {
  if (done.has(key)) return Promise.resolve(done.get(key) ?? null);
  let p = flying.get(key);
  if (!p) {
    p = make()
      // a request that was superseded (null) is asked again next time
      .then((url) => (url && remember(key, url), url))
      .catch(() => null)
      .finally(() => flying.delete(key));
    flying.set(key, p);
  }
  return p;
}

async function draw(doc: number, page: number, size: Size, rev: number, stream: number, view?: number): Promise<string | null> {
  const t = await api.fetchTile(doc, page, 0, scaleFor(size), 0, 0, 1, 0, stream, rev, view);
  if (!t) return null;
  const c = document.createElement("canvas");
  c.width = t.w;
  c.height = t.h;
  c.getContext("2d")!.putImageData(new ImageData(t.rgba as Uint8ClampedArray<ArrayBuffer>, t.w, t.h), 0, 0);
  return c.toDataURL("image/jpeg", 0.8);
}

/** The page as printed, for the backdrop of a preview. */
export const pageThumb = (doc: number, page: number, size: Size, rev: number) =>
  once(`${doc}/${rev}/${page}/page`, () => draw(doc, page, size, rev, PAGE_STREAM));

/** One plate of the page on its own (the plate's `key` as in `plateList`). */
export const plateThumb = (doc: number, page: number, size: Size, rev: number, key: string, seps: PageSeps | null, overprint: boolean) =>
  once(`${doc}/${rev}/${page}/${key}/${overprint ? "o" : "k"}`, async () => {
    // always the same limit and no other hidden names: switching plates or the limit does not redraw the strip
    const view = plateOnly({ ...DEFAULT_OUTPUT, overprint }, key, seps);
    return draw(doc, page, size, rev, PLATE_STREAM, await prepressApi.view(doc, view));
  });
