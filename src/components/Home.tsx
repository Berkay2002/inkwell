import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { openDialog } from "../lib/commands";
import { type Recent, baseName, dirName, formatSize, keyOf, ordered, relTime, shortDir } from "../lib/recent";
import { useRecents } from "../recents";
import { type Theme, useStore } from "../store";
import { Icon, type IconName } from "./Icons";

const isPdf = (p: string) => /\.pdf$/i.test(p);
type View = "grid" | "list";

/** Ink nib on an accent tile. The gradient comes from the --brand-* tokens, so it follows the theme. */
export function BrandMark({ size = 28 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden>
      <defs>
        <linearGradient id="brand-fill" x1="4" y1="2" x2="28" y2="30" gradientUnits="userSpaceOnUse">
          <stop offset="0" style={{ stopColor: "var(--brand-a)" }} />
          <stop offset="1" style={{ stopColor: "var(--brand-b)" }} />
        </linearGradient>
      </defs>
      <rect width="32" height="32" rx="9" fill="url(#brand-fill)" />
      <path
        d="M16 26.5 9.6 16.4C8.7 11.8 11.4 8 16 5.5c4.6 2.5 7.3 6.3 6.4 10.9Z"
        fill="#fff"
        fillOpacity="0.95"
      />
      <path d="M16 26.5v-9.4" stroke="var(--brand-b)" strokeWidth="1.5" strokeLinecap="round" />
      <circle cx="16" cy="14.2" r="1.7" fill="var(--brand-b)" />
    </svg>
  );
}

const Kbd = ({ children }: { children: string }) => <kbd className="kbd">{children}</kbd>;

const themeIcon: Record<Theme, IconName> = { system: "system", light: "sun", dark: "moon" };
const nextTheme: Record<Theme, Theme> = { system: "light", light: "dark", dark: "system" };
const themeName: Record<Theme, string> = { system: "Theme: match system", light: "Theme: light", dark: "Theme: dark" };

export function Home() {
  const items = useRecents((s) => s.items);
  const exists = useRecents((s) => s.exists);
  const ready = useRecents((s) => s.ready);
  const theme = useStore((s) => s.theme);
  const [over, setOver] = useState(false);
  const [view, setView] = useState<View>(() => (localStorage.getItem("inkwell.homeView") === "list" ? "list" : "grid"));
  const [confirmClear, setConfirmClear] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const grid = useRef<HTMLUListElement>(null);

  // Files dragged from Explorer arrive as Tauri events (the webview never sees DOM drag events for them).
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        const p = e.payload;
        if (p.type === "enter") setOver(p.paths.some(isPdf));
        else if (p.type !== "over") setOver(false);
      })
      .then((u) => (cancelled ? u() : (off = u)))
      .catch(() => {});
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  // Existence and freshness of the listed files; again whenever the window regains focus.
  useEffect(() => {
    const check = () => void useRecents.getState().refresh().then(() => useRecents.getState().backfillThumbs());
    check();
    window.addEventListener("focus", check);
    const tick = setInterval(() => setNow(Date.now()), 60_000);
    return () => {
      window.removeEventListener("focus", check);
      clearInterval(tick);
    };
  }, [ready]);

  useEffect(() => {
    if (!confirmClear) return;
    const t = setTimeout(() => setConfirmClear(false), 3500);
    return () => clearTimeout(t);
  }, [confirmClear]);

  const setViewMode = (v: View) => {
    localStorage.setItem("inkwell.homeView", v);
    setView(v);
  };

  const list = ordered(items);
  const hasPinned = list.some((r) => r.pinned);

  const onGridKey = (e: KeyboardEvent) => {
    const el = e.target as HTMLElement;
    if (!el.dataset.card || e.ctrlKey || e.metaKey || e.altKey) return;
    const cards = [...grid.current!.querySelectorAll<HTMLElement>("[data-card]:not(:disabled)")];
    const i = cards.indexOf(el);
    const go = (c?: HTMLElement) => {
      if (!c) return;
      c.focus();
      c.scrollIntoView({ block: "nearest" });
      e.preventDefault();
    };
    const r = el.getBoundingClientRect();
    // nearest card in the next row up/down: the row first, then the closest column
    const vertical = (dir: 1 | -1) => {
      const cand = cards.filter((c) => (dir === 1 ? c.getBoundingClientRect().top > r.top + 4 : c.getBoundingClientRect().top < r.top - 4));
      if (cand.length === 0) return undefined;
      const rowTop = dir === 1 ? Math.min(...cand.map((c) => c.getBoundingClientRect().top)) : Math.max(...cand.map((c) => c.getBoundingClientRect().top));
      return cand
        .filter((c) => Math.abs(c.getBoundingClientRect().top - rowTop) < 4)
        .sort((a, b) => Math.abs(a.getBoundingClientRect().left - r.left) - Math.abs(b.getBoundingClientRect().left - r.left))[0];
    };
    if (e.key === "ArrowRight") go(cards[i + 1]);
    else if (e.key === "ArrowLeft") go(cards[i - 1]);
    else if (e.key === "ArrowDown") go(vertical(1));
    else if (e.key === "ArrowUp") go(vertical(-1));
    else if (e.key.toLowerCase() === "p") {
      useRecents.getState().togglePin(el.dataset.path!);
      requestAnimationFrame(() => grid.current?.querySelector<HTMLElement>(`[data-key="${CSS.escape(keyOf(el.dataset.path!))}"]`)?.focus());
      e.preventDefault();
    } else if (e.key === "Delete" || e.key === "Backspace") {
      const next = cards[i + 1] ?? cards[i - 1];
      const nextKey = next?.dataset.key;
      useRecents.getState().remove(el.dataset.path!);
      requestAnimationFrame(() => nextKey && grid.current?.querySelector<HTMLElement>(`[data-key="${CSS.escape(nextKey)}"]`)?.focus());
      e.preventDefault();
    }
  };

  return (
    <div
      className={over ? "home over" : "home"}
      onDragEnter={(e) => {
        e.preventDefault();
        setOver(true);
      }}
      onDragOver={(e) => e.preventDefault()}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
      }}
      onDrop={() => setOver(false)}
    >
      <div className="home-bg" aria-hidden />
      <div className="home-scroll">
        <header className="home-top">
          <div className="brand">
            <BrandMark size={26} />
            <span className="wordmark">Inkwell</span>
          </div>
          <div className="home-tools">
            <button className="chip" onClick={() => useStore.getState().setUi({ paletteOpen: true })}>
              <Icon name="command" size={14} />
              Commands
              <span className="keys">
                <Kbd>Ctrl</Kbd>
                <Kbd>K</Kbd>
              </span>
            </button>
            <button
              className="icon-btn"
              title={themeName[theme]}
              aria-label={themeName[theme]}
              onClick={() => useStore.getState().setTheme(nextTheme[theme])}
            >
              <Icon name={themeIcon[theme]} />
            </button>
          </div>
        </header>

        <main className={ready && list.length === 0 ? "home-main first-run" : "home-main"}>
          <section className="drop rise" aria-label="Open a PDF" style={{ "--i": 0 } as React.CSSProperties}>
            <div className="drop-glyph">
              <Icon name={over ? "open" : "upload"} size={26} />
            </div>
            <div className="drop-text">
              <h1>{over ? "Drop to open" : "Open a PDF"}</h1>
              <p className="dim">Drag a file here, or choose one from your computer.</p>
            </div>
            <button className="btn primary big" onClick={() => void openDialog()}>
              <Icon name="open" />
              Open file
              <span className="keys">
                <Kbd>Ctrl</Kbd>
                <Kbd>O</Kbd>
              </span>
            </button>
          </section>

          {list.length > 0 && (
            <section className="recents rise" aria-label="Recent files" style={{ "--i": 1 } as React.CSSProperties}>
              <div className="recents-head">
                <h2>
                  Recent <span className="dim count">{list.length}</span>
                </h2>
                <div className="recents-tools">
                  <div className="seg-group" role="group" aria-label="Layout">
                    <button className={view === "grid" ? "icon-btn on" : "icon-btn"} title="Thumbnails" aria-label="Thumbnails" onClick={() => setViewMode("grid")}>
                      <Icon name="grid" size={15} />
                    </button>
                    <button className={view === "list" ? "icon-btn on" : "icon-btn"} title="List" aria-label="List" onClick={() => setViewMode("list")}>
                      <Icon name="outline" size={15} />
                    </button>
                  </div>
                  {confirmClear ? (
                    <button
                      className="link danger"
                      onClick={() => {
                        useRecents.getState().clear();
                        setConfirmClear(false);
                      }}
                    >
                      {hasPinned ? "Clear unpinned?" : "Clear all?"} Click to confirm
                    </button>
                  ) : (
                    <button className="link" onClick={() => setConfirmClear(true)}>
                      Clear
                    </button>
                  )}
                </div>
              </div>
              <ul className={view === "list" ? "cards list" : "cards"} ref={grid} onKeyDown={onGridKey}>
                {list.map((r, n) => (
                  <Card key={keyOf(r.path)} r={r} missing={exists[keyOf(r.path)] === false} now={now} index={n} />
                ))}
              </ul>
            </section>
          )}
        </main>

        <footer className="home-foot">
          <Icon name="lock" size={13} />
          Files are opened and processed on this PC. Nothing is uploaded.
        </footer>
      </div>
    </div>
  );
}

function Card({ r, missing, now, index }: { r: Recent; missing: boolean; now: number; index: number }) {
  const name = baseName(r.path);
  const dir = dirName(r.path);
  const meta = [
    r.pages != null ? `${r.pages} ${r.pages === 1 ? "page" : "pages"}` : null,
    r.size != null ? formatSize(r.size) : null,
    r.opened > 0 ? relTime(r.opened, now) : null,
  ].filter(Boolean);
  const st = useRecents.getState;
  const locate = async () => {
    const sel = await open({ multiple: false, filters: [{ name: "PDF documents", extensions: ["pdf"] }], title: `Locate ${name}` });
    if (typeof sel !== "string") return;
    await st().relocate(r.path, sel);
    void useStore.getState().openPath(sel);
  };
  const cls = ["card", r.pinned ? "pinned" : "", missing ? "missing" : ""].filter(Boolean).join(" ");

  return (
    <li className={cls} style={{ "--i": Math.min(index, 12) } as React.CSSProperties}>
      <button
        className="card-open"
        data-card
        data-path={r.path}
        data-key={keyOf(r.path)}
        disabled={missing}
        title={r.path}
        onClick={() => void useStore.getState().openPath(r.path)}
      >
        <span className="thumb-box">
          {r.thumb ? (
            <img src={r.thumb} alt="" draggable={false} />
          ) : (
            <span className="thumb-ph">
              <Icon name={missing ? "fileX" : "file"} size={22} />
            </span>
          )}
        </span>
        <span className="card-text">
          <span className="card-name">{name}</span>
          <span className="card-dir dim">{shortDir(dir)}</span>
          <span className="card-meta dim">{missing ? "File not found" : meta.join(" · ")}</span>
        </span>
      </button>
      <div className="card-actions">
        {missing ? (
          <button className="mini" onClick={() => void locate()}>
            Locate…
          </button>
        ) : (
          <button
            className={r.pinned ? "icon-btn sm on" : "icon-btn sm"}
            title={r.pinned ? "Unpin (P)" : "Pin to top (P)"}
            aria-label={r.pinned ? "Unpin" : "Pin to top"}
            aria-pressed={!!r.pinned}
            onClick={() => st().togglePin(r.path)}
          >
            <Icon name="pin" size={14} />
          </button>
        )}
        <button className="icon-btn sm" title="Remove from list (Del)" aria-label="Remove from list" onClick={() => st().remove(r.path)}>
          <Icon name="close" size={14} />
        </button>
      </div>
    </li>
  );
}
