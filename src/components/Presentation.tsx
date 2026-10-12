import { useCallback, useEffect, useRef, useState } from "react";
import * as api from "../lib/api";
import type { PageTrans } from "../lib/api";
import { Icon } from "./Icons";
import { rotated } from "../lib/layout";
import { useSheets } from "../lib/organizePlus";
import { autoAdvanceMs, effectFor, renderPage, transitionMs } from "../lib/present";
import { pageLabel, useStore } from "../store";
import "./organizePlus.css";

const st = () => useStore.getState();
const reduced = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Full screen (Ctrl+L): one page at a time, with the page's transition and automatic advance. */
export function Presentation() {
  const present = useSheets((s) => s.present);
  const doc = useStore((s) => (present ? s.docs[present.doc] : undefined));
  if (!present || !doc) return null;
  return <Show key={present.doc} doc={doc.id} start={present.start} />;
}

function Show({ doc, start }: { doc: number; start: number }) {
  const stage = useRef<HTMLDivElement>(null);
  const [page, setPage] = useState(() => Math.min(Math.max(start, 0), Math.max((st().docs[doc]?.pageCount ?? 1) - 1, 0)));
  const [ready, setReady] = useState(false);
  const [hud, setHud] = useState(true);
  const [total, setTotal] = useState(() => st().docs[doc]?.pageCount ?? 1);
  const trans = useRef<PageTrans[]>([]);
  /** Rendered canvases by page; a window resize or a new generation of the document empties it. */
  const cache = useRef(new Map<number, Promise<HTMLCanvasElement>>());
  const shown = useRef<number | null>(null);
  const layers = useRef<{ el: HTMLElement; anim: Animation[] }[]>([]);
  const hudTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const exiting = useRef(false);

  const close = useCallback(() => {
    if (exiting.current) return;
    exiting.current = true;
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    useSheets.getState().closePresent();
  }, []);

  // The window goes full screen (the HTML API: no capability to grant); its end ends the presentation.
  useEffect(() => {
    let live = true;
    const el = document.documentElement;
    const done = () => live && setReady(true);
    if (el.requestFullscreen) el.requestFullscreen().then(done, done);
    else done();
    const onChange = () => {
      if (!document.fullscreenElement) close();
    };
    document.addEventListener("fullscreenchange", onChange);
    // read the page transitions once: they are fixed while the presentation shows this generation
    api
      .transitionsList(doc)
      .then((l) => live && (trans.current = l))
      .catch(() => {});
    return () => {
      live = false;
      document.removeEventListener("fullscreenchange", onChange);
      if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
    };
  }, [doc, close]);

  const canvasOf = useCallback(
    (p: number) => {
      const d = st().docs[doc];
      const box = stage.current;
      if (!d || !box || p < 0 || p >= d.pageCount || !d.sizes[p]) return null;
      let c = cache.current.get(p);
      if (!c) {
        c = renderPage(doc, p, d.rev, d.rot, rotated(d.sizes[p], d.rot), box.clientWidth, box.clientHeight, window.devicePixelRatio || 1);
        cache.current.set(p, c);
        // keep this page and its neighbours only
        for (const k of [...cache.current.keys()]) if (Math.abs(k - p) > 2) cache.current.delete(k);
      }
      return c;
    },
    [doc],
  );

  /** Puts page `p` on screen, with its transition when it comes forward and the page asks for one. */
  const display = useCallback(
    async (p: number, animate: boolean) => {
      const box = stage.current;
      const c = canvasOf(p);
      if (!box || !c) return;
      const canvas = await c;
      // a later request got there first
      if (!stage.current || (shown.current === p && animate)) return;
      // finish what was still moving
      for (const l of layers.current) {
        l.anim.forEach((a) => a.cancel());
      }
      const old = layers.current;
      layers.current = [];
      const wrap = document.createElement("div");
      wrap.className = "pres-page";
      const copy = document.createElement("canvas");
      copy.width = canvas.width;
      copy.height = canvas.height;
      copy.style.cssText = canvas.style.cssText;
      copy.getContext("2d")!.drawImage(canvas, 0, 0);
      wrap.appendChild(copy);
      const t = animate && !reduced() ? (trans.current[p]?.transition ?? null) : null;
      const fx = effectFor(t);
      const prev = old[old.length - 1]?.el;
      if (fx.below && prev) box.insertBefore(wrap, prev);
      else box.insertBefore(wrap, box.querySelector(".pres-hud"));
      if (fx.className) wrap.classList.add(fx.className);
      const layer = { el: wrap, anim: [] as Animation[] };
      layers.current = [layer];
      const gone = () => {
        for (const o of old) o.el.remove();
      };
      shown.current = p;
      if (!prev || (!fx.incoming && !fx.outgoing)) {
        gone();
      } else {
        const opts: KeyframeAnimationOptions = { duration: transitionMs(t), easing: "ease-in-out", fill: "both" };
        const anims: Animation[] = [];
        if (fx.incoming) anims.push(wrap.animate(fx.incoming, opts));
        if (fx.outgoing) anims.push(prev.animate(fx.outgoing, opts));
        layer.anim = anims;
        // the blinds mask is a registered property: once the page is in, it must not stay masked
        Promise.all(anims.map((a) => a.finished.catch(() => undefined))).then(() => {
          gone();
          anims.forEach((a) => a.cancel());
          wrap.classList.remove("pres-blinds-h", "pres-blinds-v");
        });
      }
      // draw ahead: the next page is ready before it is asked for
      void canvasOf(p + 1);
    },
    [canvasOf],
  );

  // the page changed (or the screen is ready)
  const first = useRef(true);
  useEffect(() => {
    if (!ready) return;
    const forward = shown.current != null && page > shown.current;
    void display(page, forward && !first.current);
    first.current = false;
    // the page can say it moves on by itself
    const ms = autoAdvanceMs(trans.current[page]?.autoAdvance);
    if (ms == null || page >= total - 1) return;
    const t = setTimeout(() => setPage((p) => Math.min(p + 1, total - 1)), ms);
    return () => clearTimeout(t);
  }, [page, ready, total, display]);

  // a different size (the screen came up, the window moved monitors): draw again at that size
  useEffect(() => {
    const box = stage.current;
    if (!box || !ready) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let w = box.clientWidth;
    let h = box.clientHeight;
    const ro = new ResizeObserver(() => {
      if (box.clientWidth === w && box.clientHeight === h) return;
      w = box.clientWidth;
      h = box.clientHeight;
      clearTimeout(timer);
      timer = setTimeout(() => {
        cache.current.clear();
        shown.current = null;
        void display(page, false);
      }, 150);
    });
    ro.observe(box);
    return () => {
      clearTimeout(timer);
      ro.disconnect();
    };
  }, [ready, page, display]);

  // the document changed under us (undo from a shortcut ...): its pages are not the ones we drew
  useEffect(
    () =>
      useStore.subscribe((s, p) => {
        const d = s.docs[doc];
        const o = p.docs[doc];
        if (!d) return close();
        if (o && d.rev !== o.rev) {
          cache.current.clear();
          setTotal(d.pageCount);
          setPage((x) => Math.min(x, d.pageCount - 1));
          shown.current = null;
        }
      }),
    [doc, close],
  );

  const go = useCallback(
    (to: number) => {
      setPage((p) => Math.min(Math.max(to, 0), total - 1) === p ? p : Math.min(Math.max(to, 0), total - 1));
    },
    [total],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const next = [" ", "ArrowRight", "ArrowDown", "PageDown", "Enter"];
      const prev = ["ArrowLeft", "ArrowUp", "PageUp", "Backspace"];
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "Escape") close();
      else if (next.includes(e.key)) setPage((p) => Math.min(p + 1, total - 1));
      else if (prev.includes(e.key)) setPage((p) => Math.max(p - 1, 0));
      else if (e.key === "Home") setPage(0);
      else if (e.key === "End") setPage(total - 1);
      else return;
      // the page view underneath does not hear it
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [total, close]);

  // follow the current page back to the reader when the presentation ends
  useEffect(() => () => st().goTo(doc, shown.current ?? start), [doc, start]);

  const wake = () => {
    setHud(true);
    clearTimeout(hudTimer.current);
    hudTimer.current = setTimeout(() => setHud(false), 2200);
  };
  useEffect(() => {
    wake();
    return () => clearTimeout(hudTimer.current);
  }, []);

  const d = st().docs[doc];
  const label = d ? pageLabel(d, page) : String(page + 1);
  return (
    <div
      ref={stage}
      className={hud ? "pres show-cursor" : "pres"}
      role="dialog"
      aria-label="Full screen"
      onMouseMove={wake}
      onClick={() => setPage((p) => Math.min(p + 1, total - 1))}
    >
      <div className="pres-hud" onClick={(e) => e.stopPropagation()}>
        <button aria-label="Previous page" disabled={page === 0} onClick={() => go(page - 1)}>
          <Icon name="left" size={16} />
        </button>
        <span>
          {d && d.labels.length > 0 ? `${label} (${page + 1} of ${total})` : `${page + 1} of ${total}`}
        </span>
        <button aria-label="Next page" disabled={page >= total - 1} onClick={() => go(page + 1)}>
          <Icon name="right" size={16} />
        </button>
        <button aria-label="Exit full screen" onClick={close}>
          Esc
        </button>
      </div>
      {!ready && <span className="pres-wait">Opening full screen…</span>}
    </div>
  );
}
