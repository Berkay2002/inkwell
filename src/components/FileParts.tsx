import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { type Recent, baseName } from "../lib/recent";
import { revealInFolder, useRecents } from "../recents";
import { useStore } from "../store";
import { Icon } from "./Icons";
import { MenuButton, type MenuEntry } from "./Menu";

const isPdf = (p: string) => /\.pdf$/i.test(p);

/** Clock for relative times and the greeting; ticks once a minute. */
export function useNow() {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(t);
  }, []);
  return now;
}

/** Existence and freshness of the listed files, again whenever the window regains focus; then missing thumbnails. */
export function useRecentsSync() {
  const ready = useRecents((s) => s.ready);
  useEffect(() => {
    const check = () => void useRecents.getState().refresh().then(() => useRecents.getState().backfillThumbs());
    check();
    window.addEventListener("focus", check);
    return () => window.removeEventListener("focus", check);
  }, [ready]);
}

/**
 * True while a PDF is dragged over the window. Files from Explorer arrive as Tauri events (the webview
 * never sees DOM drag events for them); the DOM handlers cover in-page drags.
 */
export function useDropOver() {
  const [over, setOver] = useState(false);
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    try {
      getCurrentWebview()
        .onDragDropEvent((e) => {
          const p = e.payload;
          if (p.type === "enter") setOver(p.paths.some(isPdf));
          else if (p.type !== "over") setOver(false);
        })
        .then((u) => (cancelled ? u() : (off = u)))
        .catch(() => {});
    } catch {
      /* plain browser */
    }
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);
  const handlers = {
    onDragEnter: (e: React.DragEvent) => {
      e.preventDefault();
      setOver(true);
    },
    onDragOver: (e: React.DragEvent) => e.preventDefault(),
    onDragLeave: (e: React.DragEvent) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
    },
    onDrop: () => setOver(false),
  };
  return { over, handlers };
}

/** First page, or a plain grey document glyph (never a branded PDF icon) while there is none. */
export function Thumb({ r, missing, size = 22 }: { r: Recent; missing?: boolean; size?: number }) {
  return r.thumb && !missing ? (
    <img src={r.thumb} alt="" draggable={false} />
  ) : (
    <span className="thumb-ph">
      <Icon name={missing ? "fileX" : "file"} size={size} />
    </span>
  );
}

/** Asks for the file's new place, repairs the entry and opens it. */
export async function locateFile(r: Recent) {
  const name = baseName(r.path);
  const sel = await open({ multiple: false, filters: [{ name: "PDF documents", extensions: ["pdf"] }], title: `Locate ${name}` });
  if (typeof sel !== "string") return;
  await useRecents.getState().relocate(r.path, sel);
  void useStore.getState().openPath(sel);
}

export function fileMenu(r: Recent, missing: boolean): MenuEntry[] {
  const st = useRecents.getState();
  const remove = { label: "Remove from list", onSelect: () => st.remove(r.path) };
  if (missing) return [{ label: "Locate…", onSelect: () => void locateFile(r) }, "sep", remove];
  return [
    { label: r.pinned ? "Unpin" : "Pin to top", onSelect: () => st.togglePin(r.path) },
    { label: "Show in folder", onSelect: () => void revealInFolder(r.path) },
    "sep",
    remove,
  ];
}

/** The "…" button of a file row or card. */
export function FileMenu({ r, missing }: { r: Recent; missing: boolean }) {
  return (
    <MenuButton className="icon-btn more" label={`Actions for ${baseName(r.path)}`} items={fileMenu(r, missing)}>
      <Icon name="more" size={18} />
    </MenuButton>
  );
}

/** Rounded search field; `children` are rendered below it (a results popup). */
export function SearchPill({
  value,
  onChange,
  onKeyDown,
  placeholder,
  children,
}: {
  value: string;
  onChange: (v: string) => void;
  onKeyDown?: (e: React.KeyboardEvent<HTMLInputElement>) => void;
  placeholder: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="pill-search">
      <Icon name="search" size={16} />
      <input
        value={value}
        placeholder={placeholder}
        aria-label={placeholder}
        spellCheck={false}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && value) {
            onChange("");
            e.stopPropagation();
          } else onKeyDown?.(e);
        }}
      />
      {children}
    </div>
  );
}
