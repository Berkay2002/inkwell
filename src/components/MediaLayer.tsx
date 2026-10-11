import { memo, useEffect, useMemo, useRef } from "react";
import { rectToScreen } from "../lib/annotLogic";
import type { Rot, Size } from "../lib/layout";
import { type MediaItem, badgeText, clickPending, infoKey, mediaUrl, useMedia } from "../lib/media";
import { useStore } from "../store";
import { Icon } from "./Icons";
import "./media.css";

interface Props {
  docId: number;
  page: number;
  rev: number;
  rot: Rot;
  /** Unrotated page size in points. */
  rawSize: Size;
  zoom: number;
}

const EMPTY: MediaItem[] = [];
/** The controls need room even when the annotation is a small icon. */
const MIN = { audio: [240, 40], video: [160, 90] };
/** A player that was never seen on screen (it was started from the panel) is removed after this. */
const UNSEEN_MS = 2000;

/**
 * The media of one page: a badge on each annotation's area, and the player that replaces the badge
 * of the one item the person clicked. Everything is placed in the page's CSS px, so the native
 * controls stay upright and the same size at any zoom or rotation.
 */
export const MediaLayer = memo(function MediaLayer({ docId, page, rev, rot, rawSize, zoom }: Props) {
  useEffect(() => useMedia.getState().ensure(docId, rev), [docId, rev]);
  const items = useMedia((s) => s.docs[docId]?.items ?? EMPTY);
  const token = useMedia((s) => s.docs[docId]?.token ?? "");
  // an annotation with nothing in it (a LaTeX animation frame, a bare Screen) is listed in the panel only
  const mine = useMemo(() => items.filter((i) => i.page === page && i.assets.length > 0), [items, page]);
  const playing = useMedia((s) => s.playing);
  if (mine.length === 0) return null;
  return (
    <div className="media-layer">
      {mine.map((item) => {
        const [x0, y0, x1, y1] = rectToScreen(item.rect, rawSize, rot, zoom);
        const live = playing?.doc === docId && playing.xref === item.xref;
        return live ? (
          <Player key={item.xref} docId={docId} token={token} item={item} box={[x0, y0, x1, y1]} asset={playing.asset} />
        ) : (
          <Badge key={item.xref} docId={docId} item={item} box={[x0, y0, x1, y1]} />
        );
      })}
    </div>
  );
});

type Box = [number, number, number, number];

function Badge({ docId, item, box }: { docId: number; item: MediaItem; box: Box }) {
  const info = useMedia((s) => s.info[infoKey(docId, item.xref)]);
  const text = badgeText(item, info);
  const label = `${item.title || "Media"}: ${item.playable ? "play" : text}`;
  return (
    <div className="media-area" onPointerDown={(e) => e.stopPropagation()} style={{ left: box[0], top: box[1], width: box[2] - box[0], height: box[3] - box[1] }}>
      <button
        className={item.playable ? "media-badge" : "media-badge off"}
        title={item.playable ? `Play ${item.title || "media"}${text ? ` (${text})` : ""}` : text}
        aria-label={label}
        disabled={!item.playable}
        onClick={() => useMedia.getState().play(docId, item)}
      >
        <Icon name={item.playable ? "play" : "playOff"} size={18} />
      </button>
      {!item.playable && <span className="media-note">{text}</span>}
      {item.playable && text && <span className="media-tip">{text}</span>}
    </div>
  );
}

/** The element over the annotation's rect: native controls, no autoplay of its own, no loop. */
function Player({ docId, token, item, box, asset }: { docId: number; token: string; item: MediaItem; box: Box; asset: string }) {
  const a = item.assets.find((x) => x.id === asset);
  const el = useRef<HTMLVideoElement & HTMLAudioElement>(null);
  const audio = a?.element === "audio";
  const min = audio ? MIN.audio : MIN.video;
  const w = Math.max(box[2] - box[0], min[0]);
  const h = Math.max(box[3] - box[1], min[1]);

  useEffect(() => {
    const m = el.current;
    if (!m) return;
    m.muted = useMedia.getState().muted;
    // the click that made the player is the one asking for sound
    void m.play().catch(() => {});
    // out of view: removed (but a player started from the panel gets time to scroll in)
    let seen = false;
    const unseen = window.setTimeout(() => !seen && useMedia.getState().stop(), UNSEEN_MS);
    const io = new IntersectionObserver((e) => {
      const on = e.some((x) => x.isIntersecting);
      if (on) seen = true;
      else if (seen) useMedia.getState().stop();
    });
    io.observe(m);
    const key = (e: KeyboardEvent) => {
      // Esc leaves full screen first
      if (document.fullscreenElement) return;
      if (useMedia.getState().key(e.key)) {
        e.stopPropagation();
        e.preventDefault();
      }
    };
    window.addEventListener("keydown", key, true);
    return () => {
      window.clearTimeout(unseen);
      io.disconnect();
      window.removeEventListener("keydown", key, true);
      // The layer went away (page scrolled off, tab closed): the player goes with it. A re-run of this
      // effect keeps the same element (React's dev double mount), so look once it has settled.
      window.setTimeout(() => {
        const p = useMedia.getState().playing;
        if (!m.isConnected && p && p.doc === docId && p.xref === item.xref) useMedia.getState().stop();
      }, 0);
    };
  }, [docId, item.xref, asset]);

  if (!a) return null;
  const common = {
    ref: el,
    src: mediaUrl(docId, token, a.id),
    controls: true,
    preload: "none" as const,
    controlsList: "nodownload",
    className: "media-player",
    onPointerDown: (e: React.PointerEvent) => e.stopPropagation(),
    style: { left: box[0], top: box[1], width: w, height: h },
    onLoadedMetadata: () => el.current && useMedia.getState().setInfo(docId, item.xref, el.current.duration),
    onVolumeChange: () => el.current && useMedia.getState().setMuted(el.current.muted),
    onError: () => {
      // a click on another item has armed the route for it: this player's requests are refused now
      if (clickPending()) return;
      useMedia.getState().stop();
      useStore.getState().setUi({ error: `${item.title || a.name} could not be played` });
    },
  };
  return audio ? <audio {...common} /> : <video {...common} playsInline disablePictureInPicture />;
}
