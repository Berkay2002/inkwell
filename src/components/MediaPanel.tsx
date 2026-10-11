import { useEffect } from "react";
import { KIND_LABEL, type MediaItem, durationText, formatLabel, infoKey, useMedia } from "../lib/media";
import { type DocState, useStore } from "../store";
import { Icon } from "./Icons";
import "./media.css";

const EMPTY: MediaItem[] = [];
const sizeText = (n: number) => (n < 1024 * 1024 ? `${Math.max(1, Math.round(n / 1024))} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);

/**
 * The sidebar's Media tab: every video, sound and 3D item of the document and whether it can play
 * here. Playing is always a click; Flash, scripts, 3D and outside files are only listed.
 */
export function MediaPanel({ doc }: { doc: DocState }) {
  const { id, rev } = doc;
  useEffect(() => useMedia.getState().ensure(id, rev), [id, rev]);
  const loaded = useMedia((s) => s.docs[id]);
  const items = loaded?.items ?? EMPTY;
  const playing = useMedia((s) => s.playing);
  const info = useMedia((s) => s.info);

  const play = (item: MediaItem) => {
    useStore.getState().goTo(id, item.page);
    useMedia.getState().play(id, item);
  };

  return (
    <div className="media-panel">
      {!loaded ? (
        <div className="empty">Loading…</div>
      ) : items.length === 0 ? (
        <div className="empty">This document has no video, sound or 3D content.</div>
      ) : (
        <ul className="media-list">
          {items.map((item) => {
            const live = playing?.doc === id && playing.xref === item.xref;
            const a = item.assets.find((x) => x.id === item.play);
            const known = info[infoKey(id, item.xref)];
            const status = item.playable
              ? ["Plays here", formatLabel(a?.mime ?? null), known ? durationText(known.duration) : ""].filter(Boolean).join(" · ")
              : (item.reason ?? "Not supported");
            return (
              <li key={item.xref} className={live ? "media-row playing" : "media-row"}>
                <div className="media-title" title={item.title}>
                  <Icon name={item.playable ? "play" : "playOff"} size={14} />
                  <span>{item.title || KIND_LABEL[item.kind]}</span>
                </div>
                <div className="media-sub dim">
                  {KIND_LABEL[item.kind]} · page {item.page + 1} · {status}
                </div>
                {item.scripts && <div className="media-sub dim">Scripts in this item are never run.</div>}
                {item.assets.length > 1 && (
                  <ul className="media-assets dim">
                    {item.assets.map((x) => (
                      <li key={x.id}>
                        {x.name}
                        {x.embedded && x.size > 0 ? ` (${sizeText(x.size)})` : ""}: {x.playable ? "plays" : (x.reason ?? "not supported")}
                      </li>
                    ))}
                  </ul>
                )}
                <div className="media-acts">
                  {item.playable &&
                    (live ? (
                      <button className="btn ghost" onClick={() => useMedia.getState().stop()}>
                        Stop
                      </button>
                    ) : (
                      <button className="btn ghost" onClick={() => play(item)}>
                        <Icon name="play" size={14} />
                        Play
                      </button>
                    ))}
                  <button className="btn ghost" onClick={() => useStore.getState().goTo(id, item.page)}>
                    Go to
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
      <p className="media-note-foot dim">Nothing plays until you click it. Flash, scripts and files outside the document are never run or loaded.</p>
    </div>
  );
}
