import { type KeyboardEvent, useRef, useState } from "react";
import { openDialog } from "../lib/commands";
import { type Recent, baseName, dirName, formatSize, groupByDir, keyOf, ordered, relTime, shortDir } from "../lib/recent";
import { openWithAssistant } from "../lib/tools";
import { revealInFolder, useRecents } from "../recents";
import { useStore } from "../store";
import { FileMenu, SearchPill, Thumb, locateFile, useNow, usePreviewSrc } from "./FileParts";
import { Icon, type IconName } from "./Icons";

export type Scope = "recent" | "pinned" | "folders";

const open = (path: string) => void useStore.getState().openPath(path);
const pagesLabel = (n: number) => `${n} ${n === 1 ? "page" : "pages"}`;
const openedLabel = (r: Recent, now: number) => (r.opened > 0 ? `Opened ${relTime(r.opened, now)}` : "");

/** All recent files: drop panel and list; selecting a file turns it into list + preview. The sidebar belongs to Home. */
export function Library({ over, scope, onScope }: { over: boolean; scope: Scope; onScope: (s: Scope) => void }) {
  const items = useRecents((s) => s.items);
  const exists = useRecents((s) => s.exists);
  const now = useNow();
  const [pinnedFirst, setPinnedFirst] = useState(true);
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState<string | null>(null);

  const needle = query.trim().toLowerCase();
  let list = scope === "pinned" ? items.filter((r) => r.pinned) : pinnedFirst || scope === "folders" ? ordered(items) : items;
  if (needle) list = list.filter((r) => r.path.toLowerCase().includes(needle));
  const groups = scope === "folders" ? groupByDir(list) : [{ dir: "", items: list }];
  const selected = sel ? items.find((r) => keyOf(r.path) === sel) : undefined;
  const missing = (r: Recent) => exists[keyOf(r.path)] === false;

  const search = <SearchPill value={query} onChange={setQuery} placeholder={selected ? "Search files" : "Search PDFs"} />;
  const rows = <Rows groups={groups} split={!!selected} sel={sel} onSelect={setSel} now={now} missing={missing} empty={needle ? `No file matches “${query.trim()}”` : scope === "pinned" ? "Pin a file to keep it here." : "No recent files yet."} />;

  return (
    <div className="lib" onKeyDown={(e) => e.key === "Escape" && sel && setSel(null)}>
      {selected ? (
        <main className="lib-main split">
          <section className="panel lib-listcol">
            <div className="lib-search">{search}</div>
            <div className="pills wide" role="tablist">
              {(["recent", "pinned", "folders"] as Scope[]).map((s) => (
                <button key={s} role="tab" aria-selected={scope === s} className={scope === s ? "on" : undefined} onClick={() => onScope(s)}>
                  {s[0].toUpperCase() + s.slice(1)}
                </button>
              ))}
            </div>
            <div className="lib-scroll">{rows}</div>
          </section>
          <Preview r={selected} missing={missing(selected)} now={now} />
        </main>
      ) : (
        <main className="lib-main">
          <div className="lib-top">{search}</div>
          <section className={over ? "drop-panel over" : "drop-panel"} aria-label="Open a PDF">
            <span className="drop-glyph">
              <Icon name="file" size={34} />
            </span>
            <h1>{over ? "Release to open" : "Drop a PDF here"}</h1>
            <p className="dim">or open a file from your device</p>
            <button className="btn primary" onClick={() => void openDialog()}>
              <Icon name="open" size={16} />
              Open file
            </button>
            <span className="dim small">Press Ctrl + O to open a file</span>
          </section>
          <div className="sec-head">
            <h2>{scope === "pinned" ? "Pinned" : scope === "folders" ? "Folders" : "Recent"}</h2>
            {scope !== "pinned" && scope !== "folders" && (
              <div className="pills">
                <button className={pinnedFirst ? "on" : undefined} onClick={() => setPinnedFirst(true)}>
                  Pinned first
                </button>
                <button className={pinnedFirst ? undefined : "on"} onClick={() => setPinnedFirst(false)}>
                  All files
                </button>
              </div>
            )}
          </div>
          {rows}
        </main>
      )}
    </div>
  );
}

function Rows({
  groups,
  split,
  sel,
  onSelect,
  now,
  missing,
  empty,
}: {
  groups: { dir: string; items: Recent[] }[];
  split: boolean;
  sel: string | null;
  onSelect: (key: string) => void;
  now: number;
  missing: (r: Recent) => boolean;
  empty: string;
}) {
  const ul = useRef<HTMLDivElement>(null);
  const total = groups.reduce((n, g) => n + g.items.length, 0);

  // keyboard: arrows move the selection, Enter opens, P pins, Delete removes
  const onKey = (e: KeyboardEvent) => {
    const el = e.target as HTMLElement;
    if (!el.dataset.row || e.ctrlKey || e.metaKey || e.altKey) return;
    const all = [...ul.current!.querySelectorAll<HTMLElement>("[data-row]")];
    const i = all.indexOf(el);
    const go = (c?: HTMLElement) => {
      if (!c) return;
      c.focus();
      c.scrollIntoView({ block: "nearest" });
      onSelect(c.dataset.key!);
      e.preventDefault();
    };
    if (e.key === "ArrowDown") go(all[i + 1]);
    else if (e.key === "ArrowUp") go(all[i - 1]);
    else if (e.key === "Enter" && !missing({ path: el.dataset.path!, opened: 0 })) {
      open(el.dataset.path!);
      e.preventDefault();
    } else if (e.key.toLowerCase() === "p") {
      useRecents.getState().togglePin(el.dataset.path!);
      e.preventDefault();
    } else if (e.key === "Delete") {
      const next = all[i + 1] ?? all[i - 1];
      useRecents.getState().remove(el.dataset.path!);
      if (next) onSelect(next.dataset.key!);
      e.preventDefault();
    }
  };

  if (total === 0) return <p className="dim lib-none">{empty}</p>;
  return (
    <div ref={ul} onKeyDown={onKey}>
      {groups.map((g) => (
        <div key={g.dir}>
          {g.dir && (
            <h3 className="lib-group" title={g.dir}>
              {shortDir(g.dir)} <span className="dim">{g.items.length}</span>
            </h3>
          )}
          <ul className={split ? "lib-rows split" : "lib-rows"}>
            {g.items.map((r) => {
              const k = keyOf(r.path);
              const gone = missing(r);
              const dir = dirName(r.path);
              return (
                <li key={k} className={[k === sel ? "on" : "", gone ? "missing" : ""].filter(Boolean).join(" ")}>
                  <button
                    className="lib-row"
                    data-row
                    data-path={r.path}
                    data-key={k}
                    title={r.path}
                    aria-current={k === sel}
                    onClick={() => onSelect(k)}
                    onDoubleClick={() => !gone && open(r.path)}
                  >
                    <span className="lib-thumb">
                      <Thumb r={r} missing={gone} size={20} />
                    </span>
                    <span className="lib-text">
                      <span className="lib-name">{baseName(r.path)}</span>
                      <span className="lib-dir dim">{gone ? "File not found" : split ? shortDir(dir) : dir}</span>
                      {split && <span className="lib-dir dim">{openedLabel(r, now)}</span>}
                    </span>
                    <span className="lib-meta dim">
                      <span>{r.pages != null ? pagesLabel(r.pages) : ""}</span>
                      <span>{split ? (r.size != null ? formatSize(r.size) : "") : r.opened > 0 ? relTime(r.opened, now) : ""}</span>
                    </span>
                  </button>
                  {r.pinned && <Icon name="pin" size={15} />}
                  <FileMenu r={r} missing={gone} />
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </div>
  );
}

function Preview({ r, missing, now }: { r: Recent; missing: boolean; now: number }) {
  const src = usePreviewSrc(r, missing);
  const details: [IconName, string][] = [
    ["file", r.pages != null ? pagesLabel(r.pages) : ""],
    ["organize", r.size != null ? formatSize(r.size) : ""],
    ["open", shortDir(dirName(r.path))],
    ["clock", openedLabel(r, now)],
  ];

  return (
    <section className="panel lib-preview" aria-label="Preview">
      <div className="lib-preview-top">
        <button className="btn ghost" onClick={() => void openDialog()}>
          <Icon name="open" size={16} />
          Open file
          <span className="dim">Ctrl+O</span>
        </button>
      </div>
      <div className="preview-stage">
        {src ? (
          <img src={src} alt={`First page of ${baseName(r.path)}`} draggable={false} />
        ) : (
          <span className="preview-ph">
            <Icon name={missing ? "fileX" : "file"} size={44} />
          </span>
        )}
      </div>
      <h2 className="preview-name" title={r.path}>
        {baseName(r.path)}
      </h2>
      <ul className="details">
        {missing ? (
          <li className="danger">File not found at {shortDir(dirName(r.path))}</li>
        ) : (
          details.filter(([, t]) => t).map(([icon, t]) => (
            <li key={icon}>
              <Icon name={icon} size={16} />
              {t}
            </li>
          ))
        )}
      </ul>
      <div className="preview-actions">
        {missing ? (
          <>
            <button className="btn primary" onClick={() => void locateFile(r)}>
              Locate…
            </button>
            <button className="btn ghost" onClick={() => useRecents.getState().remove(r.path)}>
              Remove from list
            </button>
          </>
        ) : (
          <>
            <button className="btn primary" onClick={() => open(r.path)}>
              <Icon name="file" size={16} />
              Open
            </button>
            <button className="btn ghost" onClick={() => void openWithAssistant(r.path)}>
              <Icon name="chat" size={16} />
              Summarize with Assistant
            </button>
            <button className="btn ghost" onClick={() => void revealInFolder(r.path)}>
              <Icon name="open" size={16} />
              Show in folder
            </button>
          </>
        )}
      </div>
    </section>
  );
}
