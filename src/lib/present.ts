import { fetchTile, type Transition } from "./api";
import { bucketScale, scaleKey, TILE, type Size } from "./layout";

/**
 * Full screen: a page drawn to fit the screen, and the page transitions as CSS animations.
 * The PDF names effects a presentation viewer is free to draw its own way; these are close
 * approximations built from clipping, sliding and fading.
 */

/** Spare tile stream of the engine: the page view and thumbnails are not delayed by a presentation. */
const STREAM = 2;

/** The direction (0 left to right, 90 bottom to top, 180, 270 top to bottom) as a screen vector. */
export function directionVector(deg: number | null): [number, number] {
  const r = ((deg ?? 0) * Math.PI) / 180;
  const v = (n: number) => (Math.abs(n) < 1e-9 ? 0 : n);
  return [v(Math.cos(r)), v(-Math.sin(r))];
}

export interface Effect {
  /** Keyframes of the page coming in; none: it just appears. */
  incoming?: Keyframe[];
  /** Keyframes of the page going out (it is removed when they end). */
  outgoing?: Keyframe[];
  /** The incoming page is drawn beneath the outgoing one. */
  below?: boolean;
  /** Class that gives the incoming page its mask (blinds). */
  className?: string;
}

const clip = (v: string): Keyframe => ({ clipPath: v });
const FULL = "inset(0 0 0 0)";

function wipeStart([x, y]: [number, number]): string {
  if (Math.abs(x) >= Math.abs(y)) return x >= 0 ? "inset(0 100% 0 0)" : "inset(0 0 0 100%)";
  return y < 0 ? "inset(100% 0 0 0)" : "inset(0 0 100% 0)";
}

/** What a transition does to the two pages. `r` (replace) and unknown styles have no effect. */
export function effectFor(t: Transition | null): Effect {
  if (!t || t.style === "r") return {};
  const v = directionVector(t.direction ?? 0);
  const move = (k: number) => `translate(${v[0] * 100 * k}%, ${v[1] * 100 * k}%)`;
  switch (t.style) {
    case "fade":
    case "dissolve":
    case "glitter":
      return { incoming: [{ opacity: 0 }, { opacity: 1 }] };
    case "wipe":
      return { incoming: [clip(wipeStart(v)), clip(FULL)] };
    case "push":
      return {
        incoming: [{ transform: move(-1) }, { transform: move(0) }],
        outgoing: [{ transform: move(0) }, { transform: move(1) }],
      };
    case "cover":
    case "fly":
      return { incoming: [{ transform: move(-1) }, { transform: move(0) }] };
    case "uncover":
      return { below: true, outgoing: [{ transform: move(0) }, { transform: move(1) }] };
    case "split": {
      const to = t.dimension === "H" ? "inset(50% 0 50% 0)" : "inset(0 50% 0 50%)";
      return t.motion === "I" ? { below: true, outgoing: [clip(FULL), clip(to)] } : { incoming: [clip(to), clip(FULL)] };
    }
    case "box":
      return t.motion === "I" ? { below: true, outgoing: [clip(FULL), clip("inset(50%)")] } : { incoming: [clip("inset(50%)"), clip(FULL)] };
    case "blinds":
      return {
        incoming: [{ "--op-p": "0%" }, { "--op-p": "100%" }],
        className: t.dimension === "V" ? "pres-blinds-v" : "pres-blinds-h",
      };
    default:
      return {};
  }
}

/** Seconds a transition takes, kept to what a person can sit through. */
export const transitionMs = (t: Transition | null) => Math.round(Math.min(5, Math.max(0.1, t?.duration ?? 1)) * 1000);

/**
 * Page `page` drawn to fit `vw` x `vh` CSS px, from the engine's tiles, onto one canvas.
 * `size` is the page size in points with the view rotation applied; `rev` the document generation.
 */
export async function renderPage(doc: number, page: number, rev: number, rot: number, size: Size, vw: number, vh: number, dpr: number): Promise<HTMLCanvasElement> {
  const zoom = Math.max(0.05, Math.min(vw / size[0], vh / size[1]));
  const key = scaleKey(bucketScale(zoom, dpr));
  const s = key / 1000;
  const w = Math.max(1, Math.ceil(size[0] * s));
  const h = Math.max(1, Math.ceil(size[1] * s));
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  canvas.style.width = `${Math.round(size[0] * zoom)}px`;
  canvas.style.height = `${Math.round(size[1] * zoom)}px`;
  const ctx = canvas.getContext("2d")!;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, w, h);
  const jobs: Promise<void>[] = [];
  for (let ty = 0; ty * TILE < h; ty++) {
    for (let tx = 0; tx * TILE < w; tx++) {
      jobs.push(
        fetchTile(doc, page, rot, key, tx, ty, 1, 0, STREAM, rev).then((t) => {
          if (t) ctx.putImageData(new ImageData(t.rgba as Uint8ClampedArray<ArrayBuffer>, t.w, t.h), tx * TILE, ty * TILE);
        }),
      );
    }
  }
  await Promise.all(jobs);
  return canvas;
}

/** The pages after `from` that go by on their own, for a quick look at what a deck does: none when `/Dur` is not set. */
export const autoAdvanceMs = (secs: number | null | undefined) => (secs != null && secs > 0 ? Math.round(secs * 1000) : null);
