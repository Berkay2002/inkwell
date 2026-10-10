import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { Icon } from "./Icons";

/** The window has no native frame (decorations: false): minimize / maximize / close live in the tab strip. */
const win = () => {
  try {
    return getCurrentWindow();
  } catch {
    return null; // plain browser (dev preview)
  }
};

export function WindowControls() {
  const [max, setMax] = useState(false);

  useEffect(() => {
    const w = win();
    if (!w) return;
    let off: (() => void) | undefined;
    let dead = false;
    const sync = () => void w.isMaximized().then((m) => !dead && setMax(m)).catch(() => {});
    sync();
    w.onResized(sync)
      .then((u) => (dead ? u() : (off = u)))
      .catch(() => {});
    return () => {
      dead = true;
      off?.();
    };
  }, []);

  return (
    <div className="winctl">
      <button aria-label="Minimize" title="Minimize" onClick={() => void win()?.minimize()}>
        <Icon name="minus" size={14} />
      </button>
      <button aria-label={max ? "Restore" : "Maximize"} title={max ? "Restore" : "Maximize"} onClick={() => void win()?.toggleMaximize()}>
        <Icon name={max ? "restore" : "maximize"} size={13} />
      </button>
      <button className="close" aria-label="Close" title="Close" onClick={() => void win()?.close()}>
        <Icon name="close" size={14} />
      </button>
    </div>
  );
}
