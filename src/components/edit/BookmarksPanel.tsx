import { useEffect, useRef, useState } from "react";
import type { Bookmark } from "../../lib/api";
import { findBookmark, hexToRgb, locate, rgbToHex, useNav, visibleRows } from "../../lib/nav";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";

const nav = () => useNav.getState();

const SWATCHES = ["#1a1c20", "#2e4b8f", "#2f7d4f", "#a8321f", "#7a4a9e"];

type Drop = { id: number; at: "before" | "after" | "into" };

/** The Bookmarks section of the Edit rail: the tree, in-place rename, drag to reorder or nest, style, destination. */
export function BookmarksPanel({ doc }: { doc: DocState }) {
  const id = doc.id;
  const d = useNav((s) => s.docs[id]);
  const tree = d?.bookmarks ?? null;
  const [drag, setDrag] = useState<number | null>(null);
  const [drop, setDrop] = useState<Drop | null>(null);
  const [heads, setHeads] = useState(false);

  useEffect(() => {
    void nav().loadBookmarks(id);
  }, [id, doc.rev]);

  const items = tree?.items ?? [];
  const rows = visibleRows(items);
  const sel = d?.selectedBm != null ? findBookmark(items, d.selectedBm) : null;
  const where = sel ? locate(items, sel.id) : null;

  const go = (b: Bookmark) => {
    if (b.dest.kind === "page") useStore.getState().goTo(id, b.dest.page, b.dest.at ? Math.max(0, b.dest.at[1] - 8) : undefined);
  };

  const indent = () => {
    if (!sel || !where || where.index === 0) return;
    const prev = where.siblings[where.index - 1];
    void nav().moveBookmark(id, sel.id, prev.id, prev.children.length);
  };
  const outdent = () => {
    if (!sel || !where || where.parent == null) return;
    const up = locate(items, where.parent);
    if (up) void nav().moveBookmark(id, sel.id, up.parent, up.index + 1);
  };

  const dropOn = (target: Bookmark, at: Drop["at"]) => {
    const moving = drag;
    setDrag(null);
    setDrop(null);
    if (moving == null || moving === target.id) return;
    const w = locate(items, target.id);
    if (!w) return;
    if (at === "into") void nav().moveBookmark(id, moving, target.id, target.children.length);
    else void nav().moveBookmark(id, moving, w.parent, at === "before" ? w.index : w.index + 1);
  };

  return (
    <div className="bm">
      <div className="bm-bar" role="toolbar" aria-label="Bookmarks">
        <button className="te-btn" title="Add a bookmark at this place, or from the selected text" onClick={() => void nav().addBookmark(id)}>
          <Icon name="plus" size={16} />
          New
        </button>
        <button className="te-icon" aria-label="Indent" title="Make it a part of the bookmark above" disabled={!sel || !where || where.index === 0} onClick={indent}>
          <Icon name="right" size={16} />
        </button>
        <button className="te-icon" aria-label="Outdent" title="Move it out of its group" disabled={!sel || !where || where.parent == null} onClick={outdent}>
          <Icon name="up" size={16} />
        </button>
        <span className="grow" />
        <button className="te-icon" aria-label="Delete bookmark" title="Delete the selected bookmark and what is under it" disabled={!sel} onClick={() => sel && void nav().remove(id, [sel.id])}>
          <Icon name="trash" size={16} />
        </button>
      </div>

      {tree && rows.length === 0 && <p className="dim te-hint">This document has no bookmarks. Select text and press New to make one from it, or make them from the headings.</p>}
      {!tree && <p className="dim te-hint">Loading…</p>}
      {rows.length > 0 && (
        <ul className="bm-list" role="tree" aria-label="Bookmarks" onDragLeave={() => setDrop(null)}>
          {rows.map(({ b, depth }) => (
            <li key={b.id} role="treeitem" aria-selected={d?.selectedBm === b.id} aria-expanded={b.children.length > 0 ? b.open : undefined}>
              <Row
                doc={id}
                b={b}
                depth={depth}
                selected={d?.selectedBm === b.id}
                renaming={d?.renaming === b.id}
                drop={drop?.id === b.id ? drop.at : null}
                onGo={() => go(b)}
                onDragStart={() => setDrag(b.id)}
                onDragEnd={() => {
                  setDrag(null);
                  setDrop(null);
                }}
                onDragOver={(at) => drag != null && drag !== b.id && setDrop({ id: b.id, at })}
                onDrop={(at) => dropOn(b, at)}
              />
            </li>
          ))}
        </ul>
      )}
      {tree?.truncated && <p className="te-note">This document has more bookmarks than are shown here.</p>}

      {sel && <Selected key={sel.id} doc={id} b={sel} />}

      <section className="te-sec">
        <h3 className="te-h3">From the document</h3>
        <button
          className={heads ? "te-btn on" : "te-btn"}
          aria-expanded={heads}
          onClick={() => {
            const next = !heads;
            setHeads(next);
            if (next) void nav().askHeadings(id);
          }}
        >
          <Icon name="outline" size={16} />
          Bookmarks from headings
        </button>
        {heads && <Headings doc={id} existing={items.length > 0} />}
      </section>
      {d?.notice && (
        <p className="te-note" role="status">
          {d.notice}
        </p>
      )}
    </div>
  );
}

function Row(p: {
  doc: number;
  b: Bookmark;
  depth: number;
  selected: boolean;
  renaming: boolean;
  drop: Drop["at"] | null;
  onGo: () => void;
  onDragStart: () => void;
  onDragEnd: () => void;
  onDragOver: (at: Drop["at"]) => void;
  onDrop: (at: Drop["at"]) => void;
}) {
  const { b, doc } = p;
  const [text, setText] = useState(b.title);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (p.renaming) {
      setText(b.title);
      requestAnimationFrame(() => input.current?.select());
    }
  }, [p.renaming, b.title]);

  const style: React.CSSProperties = {
    fontWeight: b.bold ? 700 : undefined,
    fontStyle: b.italic ? "italic" : undefined,
    color: b.color ? rgbToHex(b.color) : undefined,
  };
  const zone = (e: React.DragEvent): Drop["at"] => {
    const r = e.currentTarget.getBoundingClientRect();
    const f = (e.clientY - r.top) / r.height;
    return f < 0.28 ? "before" : f > 0.72 ? "after" : "into";
  };
  return (
    <div
      className={`bm-row${p.selected ? " on" : ""}${p.drop ? ` drop-${p.drop}` : ""}`}
      style={{ paddingLeft: 6 + p.depth * 16 }}
      draggable={!p.renaming}
      onDragStart={(e) => {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", b.title);
        p.onDragStart();
      }}
      onDragEnd={p.onDragEnd}
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        p.onDragOver(zone(e));
      }}
      onDrop={(e) => {
        e.preventDefault();
        p.onDrop(zone(e));
      }}
    >
      <button
        className={`twisty${b.open ? " open" : ""}`}
        style={{ visibility: b.children.length > 0 ? "visible" : "hidden" }}
        aria-label={b.open ? "Collapse" : "Expand"}
        onClick={() => void nav().setOpen(doc, b.id, !b.open)}
      >
        <Icon name="right" size={12} />
      </button>
      {p.renaming ? (
        <input
          ref={input}
          className="bm-edit te-keep"
          aria-label="Bookmark name"
          value={text}
          autoFocus
          onChange={(e) => setText(e.target.value)}
          onBlur={() => void nav().rename(doc, b.id, text)}
          onKeyDown={(e) => {
            if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            else if (e.key === "Escape") {
              e.preventDefault();
              nav().setRenaming(doc, null);
            }
          }}
        />
      ) : (
        <button
          className="bm-title"
          style={style}
          title={b.title}
          onClick={() => {
            nav().selectBookmark(doc, b.id);
            p.onGo();
          }}
          onDoubleClick={() => nav().setRenaming(doc, b.id)}
          onKeyDown={(e) => {
            if (e.key === "F2") nav().setRenaming(doc, b.id);
            else if (e.key === "Delete") void nav().remove(doc, [b.id]);
          }}
        >
          {b.title || "(untitled)"}
        </button>
      )}
    </div>
  );
}

/** What the selected bookmark does, and its style. */
function Selected({ doc, b }: { doc: number; b: Bookmark }) {
  const where =
    b.dest.kind === "page"
      ? `Goes to page ${b.dest.page + 1}`
      : b.dest.kind === "uri"
        ? "Opens a web address"
        : b.dest.kind === "other"
          ? "Runs an action this editor keeps as it is"
          : "Goes nowhere";
  const hex = b.color ? rgbToHex(b.color) : null;
  return (
    <section className="te-sec">
      <h3 className="te-h3">Selected bookmark</h3>
      <p className="te-summary">
        <b>{b.title || "(untitled)"}</b>
        <br />
        <span className="dim">{where}</span>
      </p>
      <div className="te-row">
        <button className="te-btn" title="Make it go to what the viewer shows now" onClick={() => void nav().setDestHere(doc, b.id)}>
          Set to current view
        </button>
        <button className="te-btn" onClick={() => nav().setRenaming(doc, b.id)}>
          Rename
        </button>
      </div>
      <div className="te-row">
        <span className="te-label">Style</span>
        <span className="te-style">
          <button className={b.bold ? "te-tog on" : "te-tog"} aria-pressed={b.bold} aria-label="Bold" onClick={() => void nav().setStyle(doc, b.id, { bold: !b.bold })}>
            <b>B</b>
          </button>
          <button className={b.italic ? "te-tog on" : "te-tog"} aria-pressed={b.italic} aria-label="Italic" onClick={() => void nav().setStyle(doc, b.id, { italic: !b.italic })}>
            <i>I</i>
          </button>
        </span>
        <span className="te-swatches">
          {SWATCHES.map((c) => (
            <button
              key={c}
              className={c === hex ? "te-sw on" : "te-sw"}
              style={{ background: c }}
              aria-label={`Colour ${c}`}
              onClick={() => {
                const rgb = hexToRgb(c);
                if (rgb) void nav().setStyle(doc, b.id, { color: rgb });
              }}
            />
          ))}
          <button className="te-tog" aria-label="Default colour" title="Default colour" disabled={!b.color} onClick={() => void nav().setStyle(doc, b.id, { color: null })}>
            <Icon name="close" size={12} />
          </button>
        </span>
      </div>
    </section>
  );
}

/** The prediction for "from headings", then the button that makes them. */
function Headings({ doc, existing }: { doc: number; existing: boolean }) {
  const h = useNav((s) => s.docs[doc]?.headings ?? null);
  const [replace, setReplace] = useState(false);
  if (!h) return <p className="dim te-hint">Looking for headings…</p>;
  if (h.count === 0) return <p className="dim te-hint">No headings were found: no line is set clearly larger than the body text.</p>;
  return (
    <div className="bm-heads">
      <p className="te-summary">
        <b>
          {h.count} {h.count === 1 ? "bookmark" : "bookmarks"}
        </b>{" "}
        in {h.levels} {h.levels === 1 ? "level" : "levels"}, from the text set larger than the rest.
      </p>
      <ul className="bm-preview">
        {h.rows.slice(0, 6).map((r, i) => (
          <li key={i} style={{ paddingLeft: (r.level - 1) * 12 }}>
            <span className="bm-pt">{r.title}</span>
            <span className="dim"> p. {r.page + 1}</span>
          </li>
        ))}
        {h.count > 6 && <li className="dim">and {h.count - 6} more</li>}
      </ul>
      {existing && (
        <label className="te-check">
          <input type="checkbox" checked={replace} onChange={(e) => setReplace(e.target.checked)} />
          Replace the bookmarks there are
        </label>
      )}
      <button className="te-btn primary" onClick={() => void nav().makeFromHeadings(doc, replace)}>
        Make {h.count} {h.count === 1 ? "bookmark" : "bookmarks"}
      </button>
    </div>
  );
}
