import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { TILE_ORIGIN } from "./api";

/**
 * Rich media (src-tauri/src/media, docs/media.md): the video, sound, 3D and Flash annotations of a
 * document. Nothing plays until a click; one player at a time; Esc or scrolling it away removes it.
 */

export type MediaKind = "richMedia" | "screen" | "movie" | "sound" | "threeD";

export interface MediaAsset {
  id: string;
  name: string;
  mime: string | null;
  size: number;
  embedded: boolean;
  playable: boolean;
  reason: string | null;
  element: "video" | "audio" | null;
}

export interface MediaItem {
  page: number;
  xref: number;
  kind: MediaKind;
  /** Page space, points, y down: the space comments use. */
  rect: [number, number, number, number];
  title: string;
  playable: boolean;
  /** The asset a click plays. */
  play: string | null;
  reason: string | null;
  scripts: boolean;
  assets: MediaAsset[];
}

/** The items, and the token every asset URL of the document carries (kept while the document is open). */
export interface MediaListing {
  token: string;
  items: MediaItem[];
}

export const mediaApi = {
  list: (doc: number) => invoke<MediaListing>("media_list", { doc }),
  /** The route serves this asset, and only it, until `release`. */
  arm: (doc: number, asset: string) => invoke<void>("media_play", { doc, asset }),
  release: () => invoke<void>("media_release"),
};

/** The only address a player is given: an asset the last listing of an open document returned, by id. */
export const mediaUrl = (doc: number, token: string, asset: string) => `${TILE_ORIGIN}/m/${doc}/${token}/${asset}`;

export const KIND_LABEL: Record<MediaKind, string> = {
  richMedia: "Rich media",
  screen: "Screen",
  movie: "Movie",
  sound: "Sound",
  threeD: "3D",
};

const FORMAT: Record<string, string> = {
  "video/mp4": "MP4",
  "audio/mp4": "M4A",
  "video/webm": "WebM",
  "audio/mpeg": "MP3",
  "audio/aac": "AAC",
  "audio/wav": "WAV",
  "audio/ogg": "Ogg",
};
export const formatLabel = (mime: string | null) => (mime ? (FORMAT[mime] ?? mime) : "");

/** 75 -> "1:15", 3725 -> "1:02:05". */
export function durationText(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "";
  const t = Math.round(s);
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const ss = String(t % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

/** What a badge tells without a click: the reason, or the format and, once known, the length. */
export function badgeText(item: MediaItem, info: { duration: number } | undefined): string {
  if (!item.playable) return item.reason ?? "Not supported";
  const a = item.assets.find((x) => x.id === item.play);
  return [formatLabel(a?.mime ?? null), info ? durationText(info.duration) : ""].filter(Boolean).join(" · ");
}

export interface Playing {
  doc: number;
  xref: number;
  asset: string;
}

interface DocMedia {
  rev: number;
  token: string;
  items: MediaItem[];
}

interface MediaState {
  docs: Record<number, DocMedia>;
  /** The revision a read is in flight for, per document. */
  loading: Record<number, number>;
  playing: Playing | null;
  /** Remembered for the session: the next player starts muted or not like the last one ended. */
  muted: boolean;
  /** Duration and format of what has been played, from the element's metadata. */
  info: Record<string, { duration: number }>;
  /** Reads the list for this generation of the document (once, however many pages ask). */
  ensure: (doc: number, rev: number) => void;
  /** A click on a playable item: it replaces whatever played. */
  play: (doc: number, item: MediaItem) => void;
  stop: () => void;
  /** Esc: true when it removed the player. */
  key: (key: string) => boolean;
  setMuted: (m: boolean) => void;
  setInfo: (doc: number, xref: number, duration: number) => void;
  forget: (doc: number) => void;
}

export const infoKey = (doc: number, xref: number) => `${doc}:${xref}`;

/** The latest click or stop: a click whose arming comes back after a newer one (or a stop) is dropped. */
let clicks = 0;

export const useMedia = create<MediaState>((set, get) => ({
  docs: {},
  loading: {},
  playing: null,
  muted: false,
  info: {},
  ensure: (doc, rev) => {
    const s = get();
    if (s.docs[doc]?.rev === rev || s.loading[doc] === rev) return;
    set({ loading: { ...s.loading, [doc]: rev } });
    mediaApi.list(doc).then(
      ({ token, items }) => {
        if (get().loading[doc] !== rev) return;
        const { [doc]: _, ...loading } = get().loading;
        set({ docs: { ...get().docs, [doc]: { rev, token, items } }, loading });
        // what is playing went away with an edit (a redaction, a removed page)
        const p = get().playing;
        if (p && p.doc === doc && !items.some((i) => i.xref === p.xref && i.assets.some((a) => a.id === p.asset && a.playable))) get().stop();
      },
      () => {
        if (get().loading[doc] !== rev) return;
        const { [doc]: _, ...loading } = get().loading;
        set({ loading, docs: { ...get().docs, [doc]: { rev, token: "", items: [] } } });
      },
    );
  },
  play: (doc, item) => {
    if (!item.play) return;
    const want = { doc, xref: item.xref, asset: item.play };
    const mine = ++clicks;
    // the element is mounted once the route will serve it
    mediaApi.arm(doc, want.asset).then(
      () => mine === clicks && set({ playing: want }),
      // not in the newest listing (an edit took it away): the list refresh removes the badge
      () => {},
    );
  },
  stop: () => {
    clicks++;
    if (!get().playing) return;
    set({ playing: null });
    void mediaApi.release().catch(() => {});
  },
  key: (key) => {
    if (key !== "Escape" || !get().playing) return false;
    get().stop();
    return true;
  },
  setMuted: (muted) => set({ muted }),
  setInfo: (doc, xref, duration) => set({ info: { ...get().info, [infoKey(doc, xref)]: { duration } } }),
  forget: (doc) => {
    const { [doc]: _a, ...docs } = get().docs;
    const { [doc]: _b, ...loading } = get().loading;
    set({ docs, loading });
    if (get().playing?.doc === doc) get().stop();
  },
}));
