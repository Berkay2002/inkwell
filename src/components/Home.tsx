import { type KeyboardEvent, useMemo, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { join, resourceDir } from "@tauri-apps/api/path";
import { message } from "@tauri-apps/plugin-dialog";
import { ocrChooseTesseract, ocrClearTesseract } from "../lib/api";
import { useTools } from "../lib/toolsStore";
import { openDialog } from "../lib/commands";
import { type Recent, baseName, dirName, formatSize, greeting, keyOf, ordered, relTime, shortDir, whenLabel } from "../lib/recent";
import { launchTool, quickTools } from "../lib/tools";
import { launchToolFromHome, toolItems } from "../lib/toolItems";
import { groupTools } from "../lib/toolsCatalog";
import { revealInFolder, useRecents } from "../recents";
import type { CompressPreset } from "../lib/api";
import type { PageDark } from "../lib/pageDark";
import { type Theme, useStore } from "../store";
import { FileMenu, NavItem, SearchPill, Thumb, useDropOver, useNow, usePreviewSrc, useRecentsSync } from "./FileParts";
import { Icon } from "./Icons";
import { ToolTile } from "./ToolTile";
import { Library, type Scope } from "./Library";
import { MenuButton, type MenuEntry } from "./Menu";
import "./allTools.css";

const open = (path: string) => void useStore.getState().openPath(path);

/** The AGPL's "Appropriate Legal Notices" (section 5d): copyright, no warranty, the licence and where the source is. */
async function showAbout() {
  const [version, licenses] = await Promise.all([
    getVersion().catch(() => ""),
    resourceDir().then((d) => join(d, "licenses")).catch(() => "the licenses folder where Inkwell is installed"),
  ]);
  await message(
    `Inkwell ${version}\nCopyright (C) 2026 Berkay\n\n` +
      "This program comes with ABSOLUTELY NO WARRANTY. It is free software: you can redistribute it and/or modify it " +
      "under the terms of the GNU Affero General Public License, version 3 or (at your option) any later version.\n\n" +
      `Source code: https://github.com/Berkay2002/inkwell\nLicence and third-party notices: ${licenses}`,
    { title: "About Inkwell", kind: "info" },
  );
}
const CONTINUE = 3;
const RECENT_ROWS = 8;

/** Start screen: sidebar always visible; Home shows tools, continue reading and recent files with a preview. */
export function Home() {
  const [view, setView] = useState<"home" | Scope>("home");
  const { over, handlers } = useDropOver();
  useRecentsSync();
  return (
    <div className={over ? "home over" : "home"} {...handlers}>
      <nav className="lib-nav" aria-label="Start">
        <NavItem icon="home" label="Home" on={view === "home"} onClick={() => setView("home")} />
        <NavItem icon="clock" label="Recent" on={view === "recent" || view === "folders"} onClick={() => setView("recent")} />
        <NavItem icon="pin" label="Pinned" on={view === "pinned"} onClick={() => setView("pinned")} />
        <NavItem icon="system" label="This PC" onClick={() => void openDialog()} />
      </nav>
      {view === "home" ? <Landing over={over} onSeeAll={() => setView("recent")} /> : <Library over={over} scope={view} onScope={setView} />}
    </div>
  );
}

const themes: [Theme, string][] = [
  ["system", "Match system"],
  ["light", "Light"],
  ["dark", "Dark"],
];

const pageModes: [PageDark, string][] = [
  ["follow", "follow app theme"],
  ["on", "always dark"],
  ["off", "always normal"],
];

function Landing({ over, onSeeAll }: { over: boolean; onSeeAll: () => void }) {
  const items = useRecents((s) => s.items);
  const exists = useRecents((s) => s.exists);
  const ready = useRecents((s) => s.ready);
  const theme = useStore((s) => s.theme);
  const pageDark = useStore((s) => s.pageDark);
  const now = useNow();
  const toolGroups = useMemo(() => groupTools(toolItems(null)), []);
  const [sel, setSel] = useState<string | null>(null);
  const table = useRef<HTMLUListElement>(null);

  const missing = (r: Recent) => exists[keyOf(r.path)] === false;
  const reading = items.filter((r) => !missing(r)).slice(0, CONTINUE);
  const rows = ordered(items).slice(0, RECENT_ROWS);
  const selected = rows.find((r) => keyOf(r.path) === sel) ?? rows[0];
  const empty = ready && items.length === 0;

  const tools = useTools((s) => s.settings);
  const presets: [CompressPreset, string][] = [
    ["smallest", "Smallest"],
    ["balanced", "Balanced"],
    ["highQuality", "High quality"],
  ];
  const set = useTools.getState().setSettings;

  const settings: MenuEntry[] = [
    ...themes.map(([t, label]): MenuEntry => ({ label: `Theme: ${label}`, checked: theme === t, onSelect: () => useStore.getState().setTheme(t) })),
    "sep",
    ...pageModes.map(([m, label]): MenuEntry => ({ label: `Pages: ${label}`, checked: pageDark === m, onSelect: () => useStore.getState().setPageDark(m) })),
    "sep",
    ...presets.map(([p, label]): MenuEntry => ({ label: `Compress preset: ${label}`, checked: tools.compressPreset === p, onSelect: () => set({ compressPreset: p }) })),
    // OCR languages are chosen in the Recognize text sheet; only where Tesseract lives is a setting.
    { label: "Tesseract location…", onSelect: () => void ocrChooseTesseract() },
    { label: "Forget Tesseract location", onSelect: () => void ocrClearTesseract() },
    "sep",
    { label: "Command palette", hint: "Ctrl+K", onSelect: () => useStore.getState().setUi({ paletteOpen: true }) },
    { label: "Clear recent files", onSelect: () => useRecents.getState().clear() },
    "sep",
    { label: "About Inkwell", onSelect: () => void showAbout() },
  ];

  // arrows move the selection, Enter opens it, P pins, Delete removes
  const onRowKey = (e: KeyboardEvent) => {
    const el = e.target as HTMLElement;
    if (!el.dataset.row || e.ctrlKey || e.metaKey || e.altKey) return;
    const all = [...table.current!.querySelectorAll<HTMLElement>("[data-row]")];
    const i = all.indexOf(el);
    const go = (c?: HTMLElement) => {
      if (!c) return;
      c.focus();
      setSel(c.dataset.key!);
      e.preventDefault();
    };
    if (e.key === "ArrowDown") go(all[i + 1]);
    else if (e.key === "ArrowUp") go(all[i - 1]);
    else if (e.key === "p" || e.key === "P") {
      useRecents.getState().togglePin(el.dataset.path!);
      e.preventDefault();
    } else if (e.key === "Delete") {
      const next = all[i + 1] ?? all[i - 1];
      useRecents.getState().remove(el.dataset.path!);
      if (next) setSel(next.dataset.key!);
      e.preventDefault();
    }
  };

  return (
    <div className="landing">
      <header className="land-top">
        <HomeSearch />
        <button className="btn ghost open-file" onClick={() => void openDialog()}>
          <Icon name="open" size={16} />
          Open file
          <span className="keys">Ctrl+O</span>
        </button>
        <MenuButton className="icon-btn gear" label="Settings" items={settings}>
          <Icon name="settings" size={18} />
        </MenuButton>
      </header>

      <h1 className="greet">{greeting(new Date(now).getHours())}</h1>

      <ul className="tools" aria-label="Quick tools">
        {quickTools.map((t) => (
          <li key={t.id}>
            <button className="quick-tool" onClick={() => void launchTool(t.id)}>
              <ToolTile icon={t.icon} group={t.group} size={28} />
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
          </div>
          <ul className="reading">
            {reading.map((r) => (
              <ReadCard key={keyOf(r.path)} r={r} now={now} />
            ))}
          </ul>
        </section>
      )}

      {rows.length > 0 && selected && (
        <div className="recent-grid">
          <section aria-label="Recent files">
            <div className="sec-head">
              <h2>Recent</h2>
              <button className="see-all" onClick={onSeeAll}>
                See all
              </button>
            </div>
            <div className="trow thead" aria-hidden>
              <span>Name</span>
              <span>Modified</span>
              <span className="r">Size</span>
              <span />
            </div>
            <ul className="recent-table" ref={table} onKeyDown={onRowKey}>
              {rows.map((r) => {
                const k = keyOf(r.path);
                const gone = missing(r);
                const cls = ["trow", k === keyOf(selected.path) ? "on" : "", gone ? "missing" : ""].filter(Boolean).join(" ");
                return (
                  <li key={k} className={cls}>
                    <button
                      className="trow-open"
                      data-row
                      data-path={r.path}
                      data-key={k}
                      title={r.path}
                      aria-current={k === keyOf(selected.path)}
                      onClick={() => setSel(k)}
                      onDoubleClick={() => !gone && open(r.path)}
                    >
                      <span className="trow-name">
                        <span className="mini-thumb">
                          <Thumb r={r} missing={gone} size={16} />
                        </span>
                        <span className="trow-text">
                          <span className="lib-name">
                            {baseName(r.path)}
                            {r.pinned && <Icon name="pin" size={13} />}
                          </span>
                          <span className="dim small">{gone ? "File not found" : r.pages != null ? `${r.pages} ${r.pages === 1 ? "page" : "pages"}` : ""}</span>
                        </span>
                      </span>
                      <span className="dim">{gone ? "" : whenLabel(r.mtime ?? r.opened, now)}</span>
                      <span className="dim r">{r.size != null && !gone ? formatSize(r.size) : ""}</span>
                    </button>
                    <FileMenu r={r} missing={gone} />
                  </li>
                );
              })}
            </ul>
          </section>
          <HomePreview r={selected} missing={missing(selected)} now={now} />
        </div>
      )}

      <section aria-label="All tools">
        <div className="sec-head">
          <h2>All tools</h2>
        </div>
        {toolGroups.map((g) => (
          <div key={g.id}>
            <h3 className="tools-group">{g.title}</h3>
            <ul className="tools-grid">
              {g.items.map((it) => (
                <li key={it.id}>
                  <button className="tool-card-btn" data-tool={it.id} onClick={() => void launchToolFromHome(it.id)}>
                    <ToolTile icon={it.icon} group={it.group} size={40} />
                    <span className="tool-text">
                      <span className="tool-name">{it.label}</span>
                      <span className="tool-desc">{it.description}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </section>

      <div className="drop-hint-wrap">
        <div className="drop-hint" role="note">
          <Icon name={over ? "open" : "file"} size={18} />
          {over ? "Release to open" : "Drop a PDF anywhere to open it"}
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

function ReadCard({ r, now }: { r: Recent; now: number }) {
  const total = r.pages ?? 0;
  const page = r.lastPage != null && total > 0 ? Math.min(r.lastPage, total - 1) : null;
  const pages = total > 0 ? `${total} ${total === 1 ? "page" : "pages"}` : "";
  return (
    <li className="read-card">
      <button className="read-open" title={r.path} onClick={() => open(r.path)}>
        <span className="read-page">
          <Thumb r={r} size={26} />
        </span>
        <span className="read-text">
          <span className="read-name">{baseName(r.path)}</span>
          {r.opened > 0 && <span className="dim">Last opened {relTime(r.opened, now)}</span>}
          <span className="dim">{page != null && page > 0 ? `p. ${page + 1} of ${total}` : pages}</span>
          {page != null && page > 0 && (
            <span className="read-bar" aria-hidden>
              <span style={{ width: `${((page + 1) / total) * 100}%` }} />
            </span>
          )}
        </span>
      </button>
    </li>
  );
}

/** Right-hand pane of Home: large first page, a few facts, Open and Show in folder. */
function HomePreview({ r, missing, now }: { r: Recent; missing: boolean; now: number }) {
  const src = usePreviewSrc(r, missing);
  const facts: [string, string][] = [
    ["Type", "PDF document"],
    ["Size", r.size != null ? formatSize(r.size) : "—"],
    ["Modified", whenLabel(r.mtime ?? r.opened, now) || "—"],
    ["Pages", r.pages != null ? String(r.pages) : "—"],
  ];
  return (
    <aside className="home-preview" aria-label="Preview">
      <div className="hp-stage">
        {src ? (
          <img src={src} alt={`First page of ${baseName(r.path)}`} draggable={false} />
        ) : (
          <span className="preview-ph">
            <Icon name={missing ? "fileX" : "file"} size={40} />
          </span>
        )}
      </div>
      <h3 className="hp-name" title={r.path}>
        {baseName(r.path)}
      </h3>
      {missing ? (
        <p className="danger">File not found at {shortDir(dirName(r.path))}</p>
      ) : (
        <dl className="facts">
          {facts.map(([k, v]) => (
            <div key={k}>
              <dt className="dim">{k}</dt>
              <dd>{v}</dd>
            </div>
          ))}
        </dl>
      )}
      <div className="hp-actions">
        <button className="btn primary" disabled={missing} onClick={() => open(r.path)}>
          Open
        </button>
        <button className="btn ghost" disabled={missing} onClick={() => void revealInFolder(r.path)}>
          Show in folder
        </button>
      </div>
    </aside>
  );
}
