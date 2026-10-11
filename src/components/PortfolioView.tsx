import { useEffect, useMemo, useRef, useState } from "react";
import type { PortfolioInfo } from "../lib/api";
import { openAction, rowKey, rows } from "../lib/portfolioLogic";
import { activate, extractAll, extractFile, usePortfolio } from "../lib/portfolioStore";
import type { DocState } from "../store";
import { Icon } from "./Icons";
import "./portfolio.css";

const ROW = 32;
const EMPTY_OPEN: number[] = [];

/**
 * The portfolio's files as a list, with the collection's visible columns in its order, sorted the way
 * the author set (a click on a header sorts by it). A PDF opens in a tab of its own; any other file is
 * only ever extracted to a folder the person picks, never opened or run.
 */
export function PortfolioView({ doc, info }: { doc: DocState; info: PortfolioInfo }) {
  const id = doc.id;
  const state = usePortfolio((s) => s.docs[id]);
  const sort = state?.sort ?? info.sort;
  const open = state?.open ?? EMPTY_OPEN;
  const selected = state?.selected ?? null;
  const [top, setTop] = useState(0);
  const [height, setHeight] = useState(600);
  const el = useRef<HTMLDivElement>(null);
  const store = usePortfolio.getState;

  useEffect(() => {
    const s = el.current;
    if (!s) return;
    const ro = new ResizeObserver(() => setHeight(s.clientHeight));
    ro.observe(s);
    return () => ro.disconnect();
  }, []);

  const cols = useMemo(() => info.fields.map((f, i) => ({ f, i })).filter((c) => c.f.visible), [info.fields]);
  const list = useMemo(() => rows(info, sort, new Set(open)), [info, sort, open]);
  const template = cols.length === 0 ? "1fr" : cols.map((_, n) => (n === 0 ? "minmax(200px, 2fr)" : "minmax(80px, 1fr)")).join(" ");
  const sel = selected == null ? -1 : list.findIndex((r) => rowKey(r) === selected);
  const row = sel >= 0 ? list[sel] : undefined;
  const selFile = row?.kind === "file" ? row.file : null;
  const custom = state?.sort != null;
  const first = Math.max(0, Math.floor(top / ROW) - 6);
  const last = Math.min(list.length, Math.ceil((top + height) / ROW) + 6);

  const move = (to: number) => {
    const i = Math.max(0, Math.min(list.length - 1, to));
    const r = list[i];
    if (!r) return;
    store().select(id, rowKey(r));
    const s = el.current;
    if (s) {
      if (i * ROW < s.scrollTop) s.scrollTop = i * ROW;
      else if ((i + 1) * ROW + ROW > s.scrollTop + s.clientHeight) s.scrollTop = (i + 2) * ROW - s.clientHeight;
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    const r = row;
    if (e.key === "ArrowDown") move(sel + 1);
    else if (e.key === "ArrowUp") move(sel - 1);
    else if (e.key === "Enter" && selFile) void activate(id, selFile);
    else if (e.key === "Enter" && r?.kind === "folder") store().toggleFolder(id, r.folder.id);
    else if (e.key === "ArrowRight" && r?.kind === "folder" && !r.open) store().toggleFolder(id, r.folder.id);
    else if (e.key === "ArrowLeft" && r?.kind === "folder" && r.open) store().toggleFolder(id, r.folder.id);
    else return;
    e.preventDefault();
  };

  const files = info.files.length;
  return (
    <div className="pf" role="region" aria-label="PDF Portfolio">
      <div className="pf-bar">
        <button className="btn ghost" disabled={!selFile} onClick={() => selFile && void activate(id, selFile)}>
          <Icon name={selFile && openAction(selFile) === "open" ? "filePlus" : "download"} size={15} />
          {selFile && openAction(selFile) === "open" ? "Open" : "Extract…"}
        </button>
        {selFile && openAction(selFile) === "open" && (
          <button className="btn ghost" onClick={() => void extractFile(id, selFile)}>
            <Icon name="download" size={15} />
            Extract…
          </button>
        )}
        <button className="btn ghost" disabled={files === 0} onClick={() => void extractAll(id)}>
          <Icon name="download" size={15} />
          Extract all…
        </button>
        <button className="btn ghost" onClick={() => store().showCover(id, true)}>
          <Icon name="file" size={15} />
          Show cover page
        </button>
        {custom && (
          <button className="btn ghost" onClick={() => store().resetSort(id)}>
            Portfolio order
          </button>
        )}
        <span className="grow" />
        <span className="pf-count">
          {files} {files === 1 ? "file" : "files"}
        </span>
      </div>
      <div
        ref={el}
        className="pf-grid"
        role="treegrid"
        aria-label="Files in this portfolio"
        aria-rowcount={list.length + 1}
        tabIndex={0}
        onKeyDown={onKey}
        onScroll={(e) => setTop(e.currentTarget.scrollTop)}
      >
        <div className="pf-row pf-head" role="row" style={{ gridTemplateColumns: template }}>
          {cols.map(({ f, i }, n) => {
            const k = sort.findIndex((s) => s.field === i);
            const asc = k >= 0 ? sort[k].ascending : null;
            return (
              <div key={f.key} role="columnheader" aria-sort={asc == null ? "none" : asc ? "ascending" : "descending"} style={{ minWidth: 0 }}>
                <button className="pf-th" style={{ paddingLeft: n === 0 ? 0 : undefined }} onClick={() => store().toggleSort(id, i)}>
                  <span>{f.name}</span>
                  {asc != null && <Icon name={asc ? "up" : "down"} size={12} />}
                  {asc != null && k === 0 && !custom && <span className="pf-default">portfolio order</span>}
                </button>
              </div>
            );
          })}
        </div>
        {list.length === 0 ? (
          <div className="pf-empty">This portfolio has no files.</div>
        ) : (
          <div className="pf-body" style={{ height: list.length * ROW }}>
            {list.slice(first, last).map((r, n) => {
              const at = first + n;
              const style = { top: at * ROW, gridTemplateColumns: template };
              const pad = { paddingLeft: r.depth * 18 };
              if (r.kind === "folder") {
                return (
                  <div
                    key={`f${r.folder.id}`}
                    className="pf-row pf-folder"
                    role="row"
                    aria-level={r.depth + 1}
                    aria-expanded={r.open}
                    aria-selected={rowKey(r) === selected}
                    style={style}
                    onClick={() => {
                      store().select(id, rowKey(r));
                      store().toggleFolder(id, r.folder.id);
                    }}
                  >
                    <div className="pf-name" role="gridcell" style={pad}>
                      <span className={`pf-twist${r.open ? " open" : ""}`}>
                        <Icon name="right" size={12} />
                      </span>
                      <Icon name="open" size={15} />
                      <span>{r.folder.name}</span>
                      <span className="pf-count">{r.count}</span>
                    </div>
                  </div>
                );
              }
              const f = r.file;
              return (
                <div
                  key={f.id}
                  className="pf-row"
                  role="row"
                  aria-level={r.depth + 1}
                  aria-selected={f.id === selected}
                  style={style}
                  onClick={() => store().select(id, f.id)}
                  onDoubleClick={() => void activate(id, f)}
                >
                  {cols.map(({ f: field, i }, c) => {
                    const text = f.cells[i] ?? "";
                    if (c === 0) {
                      return (
                        <div key={field.key} className="pf-name" role="gridcell" style={{ paddingLeft: r.depth * 18 + 16 }} title={text}>
                          <Icon name="file" size={15} />
                          <span>{text}</span>
                        </div>
                      );
                    }
                    return (
                      <div key={field.key} className={`pf-cell${field.kind === "number" ? " num" : ""}`} role="gridcell" title={text}>
                        {text}
                      </div>
                    );
                  })}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

/** Over the page view while the cover is shown: the way back to the file list. */
export function PortfolioCoverNote({ doc }: { doc: DocState }) {
  return (
    <div className="pf-note" role="status">
      <span>This is a PDF Portfolio. This is its cover page.</span>
      <button className="btn" onClick={() => usePortfolio.getState().showCover(doc.id, false)}>
        Show files
      </button>
    </div>
  );
}
