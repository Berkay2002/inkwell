import { useEffect, useId, useMemo, useRef, useState } from "react";
import type { PortfolioFile, PortfolioInfo, PortfolioSortKey } from "../lib/api";
import { crumbs, extBadge, fileIcon, gridMove, rowKey, tileField, tilesAt } from "../lib/portfolioLogic";
import { activate, usePortfolio } from "../lib/portfolioStore";
import { requestThumb, type ThumbResult, thumbable, thumbNote } from "../lib/portfolioThumb";
import { Icon } from "./Icons";

const TILE_W = 168;
const TILE_H = 204;
const GAP = 12;
const PAD = 16;
/** Pixels of the longest side asked for a tile's picture (a little over its box, for sharp edges on HiDPI). */
export const TILE_SIDE = 220;

/**
 * A file's picture: page 1 of an embedded PDF, drawn lazily (only a mounted tile asks) and only under the size
 * cap; any other file, a PDF over the cap and one that cannot be drawn (locked, damaged) show the file-type icon,
 * with a short note when there is a reason. `version` is the portfolio read the picture belongs to.
 */
export function FileThumb({ doc, file, side, version, glyph }: { doc: number; file: PortfolioFile; side: number; version: unknown; glyph: number }) {
  const want = thumbable(file);
  const key = `${doc}|${file.id}|${side}`;
  const [got, setGot] = useState<{ key: string; r: ThumbResult } | null>(null);
  useEffect(() => {
    if (!want) return;
    return requestThumb(doc, file.id, side, (r) => setGot({ key, r }));
  }, [doc, file.id, side, want, key, version]);
  const r = got?.key === key ? got.r : null;
  if (want && r?.url) return <img className="pf-pic" src={r.url} alt="" draggable={false} />;
  const note = thumbNote(file) ?? r?.note ?? null;
  const badge = extBadge(file);
  return (
    <div className="pf-glyph" data-loading={want && !r ? "" : undefined}>
      <Icon name={fileIcon(file)} size={glyph} />
      {badge && <span className="pf-ext">{badge}</span>}
      {note && <span className="pf-note-line">{note}</span>}
    </div>
  );
}

interface Props {
  id: number;
  info: PortfolioInfo;
  sort: PortfolioSortKey[];
  selected: string | null;
  folder: number | null;
}

/**
 * The tiles layout: one tile per folder and file at the current level (folders first, then the files in the
 * sort the Details layout uses), windowed by row so a level of 50 000 files stays light. A folder opens on
 * double click or Enter; the breadcrumb goes back up.
 */
export function PortfolioTiles({ id, info, sort, selected, folder }: Props) {
  const uid = useId();
  const store = usePortfolio.getState;
  const el = useRef<HTMLDivElement>(null);
  const [top, setTop] = useState(0);
  const [box, setBox] = useState({ w: 800, h: 600 });

  useEffect(() => {
    const s = el.current;
    if (!s) return;
    const ro = new ResizeObserver(() => setBox({ w: s.clientWidth, h: s.clientHeight }));
    ro.observe(s);
    return () => ro.disconnect();
  }, []);

  const items = useMemo(() => tilesAt(info, sort, folder), [info, sort, folder]);
  const path = useMemo(() => crumbs(info, folder), [info, folder]);
  const cols = Math.max(1, Math.floor((box.w - 2 * PAD + GAP) / (TILE_W + GAP)));
  const rowH = TILE_H + GAP;
  const rowCount = Math.ceil(items.length / cols);
  const firstRow = Math.max(0, Math.floor((top - PAD) / rowH) - 1);
  const lastRow = Math.min(rowCount, Math.ceil((top + box.h) / rowH) + 1);
  const sel = selected == null ? -1 : items.findIndex((r) => rowKey(r) === selected);
  const tileId = (at: number) => `${uid}-t${at}`;

  const reveal = (at: number) => {
    const s = el.current;
    if (!s || at < 0) return;
    const y = PAD + Math.floor(at / cols) * rowH;
    if (y - PAD < s.scrollTop) s.scrollTop = Math.max(0, y - PAD);
    else if (y + TILE_H + PAD > s.scrollTop + s.clientHeight) s.scrollTop = y + TILE_H + PAD - s.clientHeight;
  };
  // A selection made elsewhere (the first look, a layout switch) is brought into view.
  useEffect(() => {
    if (sel >= 0) reveal(sel);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, cols, folder]);

  const up = () => {
    if (folder == null) return;
    store().enterFolder(id, path.length > 1 ? path[path.length - 2].id : null);
    store().select(id, `d:${folder}`);
  };

  const onKey = (e: React.KeyboardEvent) => {
    const r = items[sel];
    if (e.key === "Enter") {
      if (r?.kind === "folder") store().enterFolder(id, r.folder.id);
      else if (r?.kind === "file") void activate(id, r.file);
      else return;
    } else if (e.key === "Backspace" || (e.altKey && e.key === "ArrowUp")) {
      if (folder == null) return;
      up();
    } else if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(e.key)) {
      const to = gridMove(sel, e.key, cols, items.length);
      if (to < 0) return;
      store().select(id, rowKey(items[to]));
      reveal(to);
    } else return;
    e.preventDefault();
  };

  const hasFiles = info.files.length > 0;
  return (
    <div className="pf-tiles-wrap">
      <nav className="pf-crumbs" aria-label="Folder">
        <button className="pf-crumb" aria-current={path.length === 0 ? "page" : undefined} onClick={() => store().enterFolder(id, null)}>
          Portfolio
        </button>
        {path.map((f, n) => (
          <span key={f.id} className="pf-crumb-item">
            <Icon name="right" size={12} />
            <button className="pf-crumb" aria-current={n === path.length - 1 ? "page" : undefined} onClick={() => store().enterFolder(id, f.id)}>
              {f.name}
            </button>
          </span>
        ))}
      </nav>
      <div
        ref={el}
        className="pf-tiles"
        role="listbox"
        aria-label="Files in this portfolio"
        aria-activedescendant={sel >= 0 ? tileId(sel) : undefined}
        tabIndex={0}
        onKeyDown={onKey}
        onScroll={(e) => setTop(e.currentTarget.scrollTop)}
      >
        {items.length === 0 ? (
          <div className="pf-empty">{hasFiles ? "This folder is empty." : "This portfolio has no files."}</div>
        ) : (
          <div className="pf-tiles-body" style={{ height: PAD * 2 + rowCount * rowH }}>
            {items.slice(firstRow * cols, lastRow * cols).map((r, n) => {
              const at = firstRow * cols + n;
              const style = { left: PAD + (at % cols) * (TILE_W + GAP), top: PAD + Math.floor(at / cols) * rowH, width: TILE_W, height: TILE_H };
              const on = rowKey(r) === selected;
              if (r.kind === "folder") {
                const label = `${r.folder.name}, folder, ${r.count} ${r.count === 1 ? "file" : "files"}`;
                return (
                  <div
                    key={rowKey(r)}
                    id={tileId(at)}
                    className="pf-tile pf-tile-folder"
                    role="option"
                    aria-selected={on}
                    aria-label={label}
                    aria-posinset={at + 1}
                    aria-setsize={items.length}
                    style={style}
                    onClick={() => store().select(id, rowKey(r))}
                    onDoubleClick={() => store().enterFolder(id, r.folder.id)}
                  >
                    <div className="pf-thumb">
                      <div className="pf-glyph">
                        <Icon name="open" size={48} />
                      </div>
                    </div>
                    <div className="pf-tile-name" title={r.folder.name}>
                      {r.folder.name}
                    </div>
                    <div className="pf-tile-sub">
                      {r.count} {r.count === 1 ? "file" : "files"}
                    </div>
                  </div>
                );
              }
              const f = r.file;
              const extra = tileField(info, f);
              const label = extra?.value ? `${f.name}, ${extra.label}: ${extra.value}` : f.name;
              return (
                <div
                  key={f.id}
                  id={tileId(at)}
                  className="pf-tile"
                  role="option"
                  aria-selected={on}
                  aria-label={label}
                  aria-posinset={at + 1}
                  aria-setsize={items.length}
                  style={style}
                  onClick={() => store().select(id, f.id)}
                  onDoubleClick={() => void activate(id, f)}
                >
                  <div className="pf-thumb">
                    <FileThumb doc={id} file={f} side={TILE_SIDE} version={info} glyph={48} />
                  </div>
                  <div className="pf-tile-name" title={f.name}>
                    {f.name}
                  </div>
                  <div className="pf-tile-sub" title={extra ? `${extra.label}: ${extra.value}` : undefined}>
                    {extra?.value ?? ""}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
