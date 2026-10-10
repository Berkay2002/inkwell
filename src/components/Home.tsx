import { useState } from "react";
import { openDialog } from "../lib/commands";
import { type Recent, baseName, dirName, formatSize, greeting, keyOf, ordered, shortDir, whenLabel } from "../lib/recent";
import { quickTools, launchTool } from "../lib/tools";
import { useRecents } from "../recents";
import { type Theme, useStore } from "../store";
import { FileMenu, SearchPill, Thumb, useDropOver, useNow, useRecentsSync } from "./FileParts";
import { Icon } from "./Icons";
import { Library } from "./Library";
import { MenuButton, type MenuEntry } from "./Menu";

const open = (path: string) => void useStore.getState().openPath(path);
const CONTINUE = 4;
const RECENT_ROWS = 6;

/** Start screen: greeting, quick tools, continue reading, recent files. "See all" switches to the library. */
export function Home() {
  const [view, setView] = useState<"home" | "library">("home");
  const { over, handlers } = useDropOver();
  useRecentsSync();
  return (
    <div className={over ? "home over" : "home"} {...handlers}>
      {view === "home" ? <Landing over={over} onLibrary={() => setView("library")} /> : <Library over={over} onHome={() => setView("home")} />}
    </div>
  );
}

const themes: [Theme, string][] = [
  ["system", "Match system"],
  ["light", "Light"],
  ["dark", "Dark"],
];

function Landing({ over, onLibrary }: { over: boolean; onLibrary: () => void }) {
  const items = useRecents((s) => s.items);
  const exists = useRecents((s) => s.exists);
  const ready = useRecents((s) => s.ready);
  const theme = useStore((s) => s.theme);
  const now = useNow();

  const missing = (r: Recent) => exists[keyOf(r.path)] === false;
  const reading = items.filter((r) => !missing(r)).slice(0, CONTINUE);
  const taken = new Set(reading.map((r) => keyOf(r.path)));
  const recent = ordered(items.filter((r) => !taken.has(keyOf(r.path)))).slice(0, RECENT_ROWS);
  const empty = ready && items.length === 0;

  const settings: MenuEntry[] = [
    ...themes.map(([t, label]): MenuEntry => ({ label: `Theme: ${label}`, checked: theme === t, onSelect: () => useStore.getState().setTheme(t) })),
    "sep",
    { label: "Command palette", hint: "Ctrl+K", onSelect: () => useStore.getState().setUi({ paletteOpen: true }) },
    { label: "Clear recent files", onSelect: () => useRecents.getState().clear() },
  ];

  return (
    <div className="landing">
      <header className="land-top">
        <HomeSearch />
        <MenuButton className="icon-btn gear" label="Settings" items={settings}>
          <Icon name="settings" size={18} />
        </MenuButton>
      </header>

      <div className="land-body">
        <h1 className="greet">{greeting(new Date(now).getHours())}</h1>

        <ul className="tools" aria-label="Quick tools">
          {quickTools.map((t) => (
            <li key={t.id}>
              <button className="tool" onClick={() => void launchTool(t.id)}>
                <Icon name={t.icon} size={26} />
                {t.label}
              </button>
            </li>
          ))}
        </ul>

        {empty && (
          <section className="empty-panel" aria-label="No recent files">
            <span className="empty-glyph">
              <Icon name="file" size={30} />
            </span>
            <h2>No recent files yet</h2>
            <p className="dim">Files you open show up here, with a thumbnail and where you stopped reading.</p>
            <button className="btn primary" onClick={() => void openDialog()}>
              <Icon name="open" size={16} />
              Open file
              <span className="keys">Ctrl+O</span>
            </button>
          </section>
        )}

        {reading.length > 0 && (
          <section aria-label="Continue reading">
            <div className="sec-head">
              <h2>Continue reading</h2>
              <button className="see-all" onClick={onLibrary}>
                See all
              </button>
            </div>
            <ul className="reading">
              {reading.map((r) => (
                <ReadCard key={keyOf(r.path)} r={r} />
              ))}
            </ul>
          </section>
        )}

        {recent.length > 0 && (
          <section aria-label="Recent files">
            <div className="sec-head">
              <h2>Recent</h2>
              <button className="see-all" onClick={onLibrary}>
                See all
              </button>
            </div>
            <ul className="recent-list">
              {recent.map((r) => (
                <RecentRow key={keyOf(r.path)} r={r} missing={missing(r)} now={now} />
              ))}
            </ul>
          </section>
        )}

        <div className="drop-hint-wrap">
          <div className="drop-hint" role="note">
            <Icon name={over ? "open" : "file"} size={18} />
            {over ? "Release to open" : "Drop a PDF anywhere to open it"}
          </div>
        </div>
      </div>
    </div>
  );
}

/** Search over the recent list: a results popup under the field; Enter opens the highlighted match. */
function HomeSearch() {
  const items = useRecents((s) => s.items);
  const exists = useRecents((s) => s.exists);
  const [q, setQ] = useState("");
  const [hl, setHl] = useState(0);
  const needle = q.trim().toLowerCase();
  const hits = needle
    ? ordered(items)
        .filter((r) => exists[keyOf(r.path)] !== false && r.path.toLowerCase().includes(needle))
        .slice(0, 6)
    : [];
  const pick = (r: Recent) => {
    setQ("");
    open(r.path);
  };

  return (
    <SearchPill
      value={q}
      placeholder="Search your files"
      onChange={(v) => {
        setQ(v);
        setHl(0);
      }}
      onKeyDown={(e) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          setHl((h) => Math.max(0, Math.min(hits.length - 1, h + (e.key === "ArrowDown" ? 1 : -1))));
          e.preventDefault();
        } else if (e.key === "Enter" && hits[hl]) pick(hits[hl]);
      }}
    >
      {needle && (
        <ul className="search-results" role="listbox">
          {hits.length === 0 && <li className="dim none">No recent file matches “{q.trim()}”</li>}
          {hits.map((r, n) => (
            <li key={keyOf(r.path)}>
              <button className={n === hl ? "on" : undefined} role="option" aria-selected={n === hl} onMouseEnter={() => setHl(n)} onClick={() => pick(r)}>
                <Icon name="file" size={16} />
                <span className="name">{baseName(r.path)}</span>
                <span className="dim dir">{shortDir(dirName(r.path))}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </SearchPill>
  );
}

function ReadCard({ r }: { r: Recent }) {
  const total = r.pages ?? 0;
  const page = r.lastPage != null && total > 0 ? Math.min(r.lastPage, total - 1) : null;
  const pct = page != null ? ((page + 1) / total) * 100 : 0;
  const label = page != null ? `p. ${page + 1} of ${total}` : total > 0 ? `${total} ${total === 1 ? "page" : "pages"}` : "";
  return (
    <li className="read-card">
      <button className="read-open" title={r.path} onClick={() => open(r.path)}>
        <span className="read-page">
          <Thumb r={r} size={30} />
        </span>
        <span className="read-name">{baseName(r.path)}</span>
      </button>
      <div className="read-foot">
        <span className="bar" aria-hidden>
          <span style={{ width: `${pct}%` }} />
        </span>
        <span className="read-label dim">{label}</span>
        <FileMenu r={r} missing={false} />
      </div>
    </li>
  );
}

function RecentRow({ r, missing, now }: { r: Recent; missing: boolean; now: number }) {
  return (
    <li className={missing ? "recent-row missing" : "recent-row"}>
      <button className="recent-open" disabled={missing} title={r.path} onClick={() => open(r.path)}>
        <span className="file-glyph">
          <Icon name={missing ? "fileX" : "file"} size={18} />
        </span>
        <span className="recent-name">
          {baseName(r.path)}
          {r.pinned && <Icon name="pin" size={13} />}
        </span>
        <span className="recent-when dim">{missing ? "File not found" : whenLabel(r.opened, now)}</span>
        <span className="recent-size dim">{r.size != null ? formatSize(r.size) : ""}</span>
      </button>
      <FileMenu r={r} missing={missing} />
    </li>
  );
}
