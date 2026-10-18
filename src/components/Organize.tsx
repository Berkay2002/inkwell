import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { pickOpen } from "../lib/api";
import * as edits from "../lib/edit";
import { clamp, previewScaleKey, type Size, type TileRef } from "../lib/layout";
import { formatRange, movedIndices, parseRange } from "../lib/pageRange";
import { useTextEdit } from "../lib/textEdit";
import { tileClient, type Want } from "../lib/tiles";
import { useSheets } from "../lib/organizePlus";
import { type DocState, pageLabel, useStore } from "../store";
import { Icon, type IconName } from "./Icons";
import { MenuButton } from "./Menu";
import { ContextMenu, pageActions } from "./PageMenu";
import { TileCanvas } from "./TileCanvas";
import "./organize.css";

/** Organize pages: a panel of page actions and a grid of every page. */

const st = () => useStore.getState();
const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

const PAD = 24;
const GAP_X = 22;
const GAP_Y = 18;
/** Narrowest page cell: a 1440 px window at 100% fits 6, wider windows more. */
const MIN_CELL = 160;
/** Page box height per unit of cell width (US Letter portrait fits with a little air). */
const BOX_ASPECT = 1.36;
/** Below the page box: gap + number badge. */
const LABEL_H = 40;
const SPIN_MS = 200;
const LEAVE_MS = 180;

export function ModeSwitch({ doc }: { doc: DocState }) {
  const editing = useTextEdit((s) => s.open) && !doc.organize;
  const set = (organize: boolean, edit = false) => {
    useTextEdit.getState().setOpen(edit);
    st().patch(doc.id, { organize });
  };
  return (
    <div className="mode-pill" role="tablist" aria-label="View">
      <button role="tab" aria-selected={!doc.organize && !editing} className={doc.organize || editing ? "" : "on"} onClick={() => set(false)}>
        <Icon name="book" size={16} filled={!doc.organize && !editing} />
        Read
      </button>
      <button role="tab" aria-selected={editing} className={editing ? "on" : ""} onClick={() => set(false, true)}>
        <Icon name="edit" size={16} filled={editing} />
        Edit
      </button>
      <button role="tab" aria-selected={doc.organize} className={doc.organize ? "on" : ""} onClick={() => set(true)}>
        <Icon name="grid" size={16} filled={doc.organize} />
        Organize
      </button>
    </div>
  );
}

interface Grid {
  cols: number;
  cell: number;
  box: number;
  rowH: number;
}

function gridFor(width: number): Grid {
  const inner = Math.max(0, width - 2 * PAD);
  const cols = Math.max(1, Math.floor((inner + GAP_X) / (MIN_CELL + GAP_X)));
  const cell = (inner - (cols - 1) * GAP_X) / cols;
  const box = Math.round(cell * BOX_ASPECT);
  return { cols, cell, box, rowH: box + LABEL_H + GAP_Y };
}

const cellX = (g: Grid, col: number) => PAD + col * (g.cell + GAP_X);
const cellY = (g: Grid, row: number) => PAD + row * g.rowH;

/** Displayed page size inside the cell's page box. */
function fit(g: Grid, s: Size): Size {
  const k = Math.min(g.cell / s[0], g.box / s[1]);
  return [s[0] * k, s[1] * k];
}

interface Drag {
  /** Gap the pages would be moved to (engine `move` semantics). */
  gap: number;
  /** Where the insertion bar is: before column `col` of `row` (`col == cols`: after the row). */
  row: number;
  col: number;
  x: number;
  y: number;
}

interface Band {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  base: Set<number>;
}

export function Organize({ doc }: { doc: DocState }) {
  const { id, pageCount, sizes, rev } = doc;
  const [sel, setSel] = useState<Set<number>>(() => new Set());
  const anchor = useRef<number | null>(null);
  const [spin, setSpin] = useState<{ pages: Set<number>; deg: number; at: number; rev: number | null } | null>(null);
  const [leaving, setLeaving] = useState<Set<number>>(() => new Set());
  const [toast, setToast] = useState<{ text: string; n: number } | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  // Pages that no longer exist leave the selection.
  useEffect(() => {
    setSel((s) => (s.size && [...s].some((p) => p >= pageCount) ? new Set([...s].filter((p) => p < pageCount)) : s));
  }, [pageCount]);

  const selected = useMemo(() => [...sel].sort((a, b) => a - b), [sel]);

  const rotate = useCallback(
    (pages: number[], quarterTurns: number) => {
      if (pages.length === 0) return;
      const run = edits.edit(id, { op: "rotate", pages, quarterTurns });
      if (reducedMotion()) return;
      setSpin({ pages: new Set(pages), deg: quarterTurns * 90, at: performance.now(), rev: null });
      void run.then((s) => setSpin((cur) => (cur && s ? { ...cur, rev: s.rev } : null)));
    },
    [id],
  );

  const remove = useCallback(
    (pages: number[]) => {
      if (pages.length === 0) return;
      if (pages.length >= pageCount) {
        st().setUi({ error: "A document needs at least one page." });
        return;
      }
      const go = () =>
        void edits.edit(id, { op: "delete", pages }).then((s) => {
          setLeaving(new Set());
          if (!s) return;
          setSel(new Set());
          setToast({ text: pages.length === 1 ? `Deleted page ${pages[0] + 1}` : `Deleted ${pages.length} pages`, n: Date.now() });
        });
      if (reducedMotion()) return go();
      setLeaving(new Set(pages));
      setTimeout(go, LEAVE_MS);
    },
    [id, pageCount],
  );

  const insertAt = selected.length ? selected[selected.length - 1] + 1 : pageCount;

  const replace = async () => {
    if (selected.length === 0) return;
    const before = st().docs[id]?.pageCount ?? pageCount;
    const s = await edits.insertFile(id, selected[0]);
    if (!s) return;
    const added = s.pageCount - before;
    // The selected pages moved down by what was inserted in front of them.
    await edits.edit(id, { op: "delete", pages: selected.map((p) => p + added) });
    setSel(new Set(Array.from({ length: added }, (_, i) => selected[0] + i)));
  };

  return (
    <div className="org">
        <aside className="org-panel">
          <SelectionField count={pageCount} selected={selected} onChange={(p) => setSel(new Set(p))} />
          <div className="org-actions">
            <RoundBtn icon="rotateLeft" label="Rotate left" disabled={!selected.length} onClick={() => rotate(selected, -1)} />
            <RoundBtn icon="rotate" label="Rotate right" disabled={!selected.length} onClick={() => rotate(selected, 1)} />
            <RoundBtn icon="trash" label="Delete" disabled={!selected.length} onClick={() => remove(selected)} />
            <RoundBtn icon="share" label="Extract" disabled={!selected.length} onClick={() => void edits.extract(id, selected)} />
          </div>
          <hr />
          <div className="org-list">
            <MenuButton
              label="Insert pages"
              className="org-row"
              items={[
                { label: "Blank page", onSelect: () => void edits.edit(id, { op: "insertBlank", at: insertAt, like: Math.max(0, insertAt - 1) }) },
                { label: "From file…", onSelect: () => void edits.insertFile(id, insertAt) },
              ]}
            >
              <Icon name="filePlus" size={20} />
              Insert pages
            </MenuButton>
            <MenuButton
              label="Split"
              className="org-row"
              items={[1, 2, 5, 10].map((n) => ({
                label: n === 1 ? "Into single pages" : `Every ${n} pages`,
                onSelect: () => void edits.split(id, n),
              }))}
            >
              <Icon name="scissors" size={20} />
              Split
            </MenuButton>
            <button className="org-row" title="Add the pages of other PDFs at the end" onClick={() => void mergeInto(id)}>
              <Icon name="filePlus" size={20} />
              Append files
            </button>
            <button className="org-row" title="Choose, order and trim several PDFs into a new file" onClick={() => edits.combineFiles()}>
              <Icon name="merge" size={20} />
              Combine files…
            </button>
            <button className="org-row" title="Replace the selected pages with the pages of another PDF" disabled={!selected.length} onClick={() => void replace()}>
              <Icon name="swap" size={20} />
              Replace
            </button>
            <button
              className="org-row"
              title="Number the pages i, ii, iii or A-1, A-2 (shown in the page box and under every thumbnail)"
              onClick={() => useSheets.getState().openLabels(id, selected[0] ?? doc.currentPage, selected[selected.length - 1] ?? doc.currentPage)}
            >
              <Icon name="hash" size={20} />
              Page labels…
            </button>
            <button
              className="org-row"
              title="How full screen moves to these pages, and how long it stays"
              onClick={() => useSheets.getState().openTrans(id, selected.length ? selected : [doc.currentPage])}
            >
              <Icon name="present" size={20} />
              Transitions…
            </button>
          </div>
          <span className="grow" />
          <p className="org-count">
            {selected.length} of {pageCount} pages selected
          </p>
        </aside>
        <PageGrid
          doc={doc}
          sizes={sizes}
          rev={rev}
          sel={sel}
          setSel={setSel}
          anchor={anchor}
          spin={spin}
          clearSpin={() => setSpin(null)}
          leaving={leaving}
          onRotate={rotate}
          onDelete={remove}
          onMenu={(x, y) => setMenu({ x, y })}
        />
        {menu && selected.length > 0 && (
          <ContextMenu
            x={menu.x}
            y={menu.y}
            items={pageActions(doc, selected, { rotate: (q) => rotate(selected, q), remove: () => remove(selected) })}
            onClose={() => setMenu(null)}
          />
        )}
        {toast && <UndoToast key={toast.n} text={toast.text} onUndo={() => void edits.undo(id)} onDone={() => setToast(null)} />}
    </div>
  );
}

/** Merge files: other PDFs' pages added at the end, one undo step each. */
async function mergeInto(id: number) {
  for (const f of await pickOpen(true)) {
    const n = st().docs[id]?.pageCount;
    if (n == null || !(await edits.edit(id, { op: "insertFile", at: n, file: f.token }))) return;
  }
}

function RoundBtn({ icon, label, onClick, disabled }: { icon: IconName; label: string; onClick: () => void; disabled?: boolean }) {
  return (
    <button className="org-round" onClick={onClick} disabled={disabled}>
      <span className="org-round-icon">
        <Icon name={icon} size={20} />
      </span>
      {label}
    </button>
  );
}

/** "Pages selected": typed ranges select pages, and selecting pages rewrites the text. */
function SelectionField({ count, selected, onChange }: { count: number; selected: number[]; onChange: (p: number[]) => void }) {
  const [text, setText] = useState<string | null>(null);
  const shown = text ?? formatRange(selected).replace(/-/g, " - ");
  const bad = text != null && parseRange(text, count) == null;
  return (
    <label className="org-field">
      <span>Pages</span>
      <input
        value={shown}
        placeholder="e.g. 1, 3-5"
        spellCheck={false}
        className={bad ? "bad" : undefined}
        aria-invalid={bad}
        onFocus={() => setText(shown)}
        onBlur={() => setText(null)}
        onChange={(e) => {
          setText(e.target.value);
          const p = parseRange(e.target.value, count);
          if (p) onChange(p);
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === "Escape") (e.target as HTMLInputElement).blur();
        }}
      />
    </label>
  );
}

function UndoToast({ text, onUndo, onDone }: { text: string; onUndo: () => void; onDone: () => void }) {
  useEffect(() => {
    const t = setTimeout(onDone, 6000);
    return () => clearTimeout(t);
  }, [onDone]);
  return (
    <div className="org-toast" role="status">
      {text}
      <button
        onClick={() => {
          onUndo();
          onDone();
        }}
      >
        Undo
      </button>
    </div>
  );
}

interface GridProps {
  doc: DocState;
  sizes: Size[];
  rev: number;
  sel: Set<number>;
  setSel: (s: Set<number> | ((s: Set<number>) => Set<number>)) => void;
  anchor: React.MutableRefObject<number | null>;
  spin: { pages: Set<number>; deg: number; at: number; rev: number | null } | null;
  clearSpin: () => void;
  leaving: Set<number>;
  onRotate: (pages: number[], quarterTurns: number) => void;
  onDelete: (pages: number[]) => void;
  onMenu: (x: number, y: number) => void;
}

function PageGrid({ doc, sizes, rev, sel, setSel, anchor, spin, clearSpin, leaving, onRotate, onDelete, onMenu }: GridProps) {
  const { id, pageCount } = doc;
  const el = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, w: 0, h: 0 });
  const [drag, setDrag] = useState<Drag | null>(null);
  const [band, setBand] = useState<Band | null>(null);
  const ghost = useRef<HTMLCanvasElement>(null);

  useLayoutEffect(() => {
    const s = el.current!;
    const sync = () => setView({ top: s.scrollTop, w: s.clientWidth, h: s.clientHeight });
    sync();
    const ro = new ResizeObserver(sync);
    ro.observe(s);
    s.focus();
    return () => {
      ro.disconnect();
      tileClient.clear(1);
    };
  }, []);

  const g = gridFor(view.w);
  const rows = Math.ceil(pageCount / g.cols);
  const total = PAD * 2 + rows * g.rowH - GAP_Y;
  const sorted = () => [...sel].sort((a, b) => a - b);

  // Show the current page when the view opens.
  useLayoutEffect(() => {
    if (view.w === 0) return;
    const row = Math.floor(doc.currentPage / g.cols);
    el.current!.scrollTop = Math.max(0, cellY(g, row) - PAD);
    // once, when the width is first known
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view.w > 0]);

  /** Content coordinates of a pointer event. */
  const at = (e: { clientX: number; clientY: number }) => {
    const r = el.current!.getBoundingClientRect();
    return { x: e.clientX - r.left + el.current!.scrollLeft, y: e.clientY - r.top + el.current!.scrollTop };
  };

  const gapAt = (x: number, y: number) => {
    const row = clamp(Math.floor((y - PAD + GAP_Y / 2) / g.rowH), 0, rows - 1);
    // gap k of a row sits at PAD + k * (cell + GAP_X) - GAP_X / 2
    const inRow = Math.min(g.cols, pageCount - row * g.cols);
    const col = clamp(Math.round((x - PAD + GAP_X / 2) / (g.cell + GAP_X)), 0, inRow);
    return { gap: row * g.cols + col, row, col };
  };

  const click = (i: number, e: React.MouseEvent) => {
    if (e.shiftKey && anchor.current != null) {
      const [a, b] = [Math.min(anchor.current, i), Math.max(anchor.current, i)];
      const range = Array.from({ length: b - a + 1 }, (_, k) => a + k);
      setSel((s) => new Set(e.ctrlKey ? [...s, ...range] : range));
      return;
    }
    anchor.current = i;
    if (e.ctrlKey || e.metaKey) {
      setSel((s) => {
        const n = new Set(s);
        if (!n.delete(i)) n.add(i);
        return n;
      });
    } else setSel(new Set([i]));
  };

  /** Press on a page: click, or (after a few px) drag every selected page. */
  const pressPage = (i: number, e: React.MouseEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    el.current!.focus();
    const start = { x: e.clientX, y: e.clientY };
    let moving = false;
    let pages: number[] = [];
    let gap: number | null = null;
    let lastEvent = e.nativeEvent;
    const scroller = setInterval(() => {
      // Near the top or bottom edge the grid scrolls by itself.
      if (!moving) return;
      const r = el.current!.getBoundingClientRect();
      const dy = lastEvent.clientY < r.top + 48 ? -14 : lastEvent.clientY > r.bottom - 48 ? 14 : 0;
      if (dy) {
        el.current!.scrollTop += dy;
        update(lastEvent);
      }
    }, 16);
    const update = (m: MouseEvent) => {
      const p = at(m);
      const where = gapAt(p.x, p.y);
      gap = where.gap;
      setDrag({ ...where, x: m.clientX, y: m.clientY });
    };
    const move = (m: MouseEvent) => {
      lastEvent = m;
      if (!moving && Math.hypot(m.clientX - start.x, m.clientY - start.y) < 5) return;
      if (!moving) {
        moving = true;
        pages = sel.has(i) ? sorted() : [i];
        if (!sel.has(i)) {
          anchor.current = i;
          setSel(new Set([i]));
        }
        paintGhost(i, pages.length);
      }
      update(m);
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      clearInterval(scroller);
      if (!moving) return click(i, e);
      setDrag(null);
      if (gap == null) return;
      const to = gap;
      void edits.edit(id, { op: "move", pages, to }).then((s) => s && setSel(new Set(movedIndices(pages, to))));
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  /** The dragged page's pixels, lifted under the pointer. */
  const paintGhost = (i: number, n: number) => {
    requestAnimationFrame(() => {
      const src = el.current?.querySelector<HTMLCanvasElement>(`[data-index="${i}"] canvas`);
      const dst = ghost.current;
      if (!src || !dst) return;
      dst.width = src.width;
      dst.height = src.height;
      dst.getContext("2d")!.drawImage(src, 0, 0);
      dst.parentElement!.dataset.count = n > 1 ? String(n) : "";
    });
  };

  /** Press on empty space: rubber band selection. */
  const pressBackground = (e: React.MouseEvent) => {
    if (e.button !== 0 || (e.target !== e.currentTarget.firstChild && e.target !== e.currentTarget)) return;
    e.preventDefault();
    el.current!.focus();
    const p0 = at(e);
    const base = e.ctrlKey || e.shiftKey ? new Set(sel) : new Set<number>();
    if (!e.ctrlKey && !e.shiftKey) setSel(new Set());
    const move = (m: MouseEvent) => {
      const p = at(m);
      const b = { x0: Math.min(p0.x, p.x), y0: Math.min(p0.y, p.y), x1: Math.max(p0.x, p.x), y1: Math.max(p0.y, p.y), base };
      setBand(b);
      const hit = new Set(base);
      for (let i = 0; i < pageCount; i++) {
        const [r, c] = [Math.floor(i / g.cols), i % g.cols];
        const x = cellX(g, c);
        const y = cellY(g, r);
        if (x < b.x1 && x + g.cell > b.x0 && y < b.y1 && y + g.box > b.y0) hit.add(i);
      }
      setSel(hit);
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      setBand(null);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const onKey = (e: React.KeyboardEvent) => {
    const ctrl = e.ctrlKey || e.metaKey;
    const k = e.key.toLowerCase();
    const take = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    if (ctrl && k === "a") {
      take();
      setSel(new Set(Array.from({ length: pageCount }, (_, i) => i)));
    } else if (e.key === "Delete" || e.key === "Backspace") {
      take();
      onDelete(sorted());
    } else if (ctrl && k === "r") {
      take();
      onRotate(sorted(), e.shiftKey ? -1 : 1);
    } else if (e.key === "Escape" && sel.size) {
      take();
      setSel(new Set());
    } else if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key) && !ctrl) {
      take();
      const cur = anchor.current ?? -1;
      const step = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -g.cols, ArrowDown: g.cols }[e.key]!;
      const next = clamp(cur < 0 ? 0 : cur + step, 0, pageCount - 1);
      if (e.shiftKey) setSel((s) => new Set([...s, next]));
      else setSel(new Set([next]));
      anchor.current = next;
      const y = cellY(g, Math.floor(next / g.cols));
      const s = el.current!;
      if (y < s.scrollTop) s.scrollTop = y - PAD;
      else if (y + g.rowH > s.scrollTop + s.clientHeight) s.scrollTop = y + g.rowH - s.clientHeight;
    } else if (e.key === "Enter" && anchor.current != null) {
      take();
      open(anchor.current);
    }
  };

  const open = (page: number) => {
    st().patch(id, { organize: false });
    st().goTo(id, page);
  };

  // Visible rows, with one row of slack either way.
  const first = Math.max(0, Math.floor((view.top - PAD) / g.rowH) - 1);
  const last = Math.min(rows - 1, Math.floor((view.top + view.h) / g.rowH) + 1);
  const items: React.ReactNode[] = [];
  const wanted: Want[] = [];
  const dragRow = drag?.row ?? -1;
  const dragCol = drag?.col ?? -1;
  for (let r = first; r <= last && view.w > 0; r++) {
    for (let c = 0; c < g.cols; c++) {
      const i = r * g.cols + c;
      if (i >= pageCount) break;
      const size = sizes[i];
      const [w, h] = fit(g, size);
      const tile: TileRef = { page: i, rot: 0, scale: previewScaleKey(size), tx: 0, ty: 0, ...(rev ? { rev } : {}) };
      wanted.push({ doc: id, tile });
      const spinning = spin?.pages.has(i) ?? false;
      // Rotating the box: scaled so the turned page fits the cell.
      const turned = spinning && Math.abs(spin!.deg) % 180 === 90 ? fit(g, [size[1], size[0]]) : null;
      const k = turned ? turned[0] / h : 1;
      const shift = r === dragRow ? (c < dragCol ? -10 : 10) : 0;
      items.push(
        <div
          key={i}
          data-index={i}
          className={[
            "org-cell",
            sel.has(i) && "on",
            leaving.has(i) && "leaving",
            drag && sel.has(i) && "dragged",
          ]
            .filter(Boolean)
            .join(" ")}
          style={{ left: cellX(g, c), top: cellY(g, r), width: g.cell, transform: shift ? `translateX(${shift}px)` : undefined }}
        >
          <div className="org-box" style={{ height: g.box }}>
            <div
              className={spinning ? "org-page spin" : "org-page"}
              style={{ width: w, height: h, transform: spinning ? `rotate(${spin!.deg}deg) scale(${k})` : undefined }}
              onMouseDown={(e) => pressPage(i, e)}
              onDoubleClick={() => open(i)}
              onContextMenu={(e) => {
                e.preventDefault();
                if (!sel.has(i)) {
                  anchor.current = i;
                  setSel(new Set([i]));
                }
                onMenu(e.clientX, e.clientY);
              }}
              title={`Page ${i + 1}`}
            >
              <TileCanvas
                docId={id}
                tile={tile}
                size={size}
                zoom={w / size[0]}
                stream={1}
                cover
                onDrawn={(key) => {
                  // The turned page is on screen: drop the stand-in rotation (after it played).
                  if (spinning && spin!.rev != null && key.endsWith(`/${spin!.rev}`)) {
                    setTimeout(clearSpin, Math.max(0, SPIN_MS - (performance.now() - spin!.at)));
                  }
                }}
              />
              {sel.has(i) && (
                <span className="org-check" aria-hidden>
                  <Icon name="check" size={14} />
                </span>
              )}
            </div>
          </div>
          <span className="org-num" title={doc.labels[i] ? `Page ${i + 1}` : undefined}>
            {pageLabel(doc, i)}
          </span>
        </div>,
      );
    }
  }
  useEffect(() => tileClient.want(1, wanted));

  // A spin whose new tiles never arrive (or none were visible) ends anyway.
  useEffect(() => {
    if (!spin) return;
    const t = setTimeout(clearSpin, 1500);
    return () => clearTimeout(t);
  }, [spin, clearSpin]);

  const bar = drag && <div className="org-bar" style={{ left: cellX(g, dragCol) - GAP_X / 2 - 2, top: cellY(g, dragRow), height: g.box }} />;

  return (
    <div
      ref={el}
      className={drag ? "org-grid dragging" : "org-grid"}
      tabIndex={0}
      onKeyDown={onKey}
      onMouseDown={pressBackground}
      onScroll={() => setView((v) => ({ ...v, top: el.current!.scrollTop }))}
      aria-label="Pages"
    >
      <div className="org-content" style={{ height: total }}>
        {items}
        {bar}
        {band && <div className="org-band" style={{ left: band.x0, top: band.y0, width: band.x1 - band.x0, height: band.y1 - band.y0 }} />}
      </div>
      <div className="org-ghost" style={drag ? { left: drag.x + 12, top: drag.y + 12 } : { display: "none" }}>
        <canvas ref={ghost} />
      </div>
    </div>
  );
}
