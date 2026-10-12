import { useEffect, useRef, useState } from "react";
import type { Bookmark } from "../../lib/api";
import { bookmarkState, countProblems, findBookmark, locate, onlyProblems, rgbToHex, stateLabel, useNav, visibleRows } from "../../lib/nav";
import { useStore } from "../../store";
import { Icon } from "../Icons";
import type { MenuEntry } from "../Menu";
import { ContextMenu } from "../PageMenu";
import "./edit.css";

const nav = () => useNav.getState();

type Drop = { id: number; at: "before" | "after" | "into" };

/** The buttons above a bookmark tree: new, indent, outdent, delete and the problems filter. */
export function BookmarkBar({ doc }: { doc: number }) {
  const d = useNav((s) => s.docs[doc]);
  const showProblems = useNav((s) => s.showProblems);
  const items = d?.bookmarks?.items ?? [];
  const sel = d?.selectedBm != null ? findBookmark(items, d.selectedBm) : null;
  const where = sel ? locate(items, sel.id) : null;
  const problems = countProblems(items);

  const indent = () => {
    if (!sel || !where || where.index === 0) return;
    const prev = where.siblings[where.index - 1];
    void nav().moveBookmark(doc, sel.id, prev.id, prev.children.length);
  };
  const outdent = () => {
    if (!sel || !where || where.parent == null) return;
    const up = locate(items, where.parent);
    if (up) void nav().moveBookmark(doc, sel.id, up.parent, up.index + 1);
  };

  return (
    <div className="bm-bar" role="toolbar" aria-label="Bookmarks">
      <button className="te-btn" title="Add a bookmark at this place, or from the selected text" onClick={() => void nav().addBookmark(doc)}>
        <Icon name="plus" size={16} />
        New
      </button>
      <button
        className={showProblems ? "te-btn on bm-problems" : "te-btn bm-problems"}
        aria-pressed={showProblems}
        title="Show only the bookmarks that go nowhere or to a destination that is missing"
        onClick={() => nav().setShowProblems(!showProblems)}
      >
        Problems{problems > 0 ? ` ${problems}` : ""}
      </button>
      <span className="grow" />
      <button className="te-icon" aria-label="Delete bookmark" title="Delete the selected bookmark and what is under it" disabled={!sel} onClick={() => sel && void nav().remove(doc, [sel.id])}>
        <Icon name="trash" size={16} />
      </button>
      {/* a second row on purpose: words, not bare arrows, say which way each one moves the bookmark */}
      <span className="bm-break" />
      <button className="te-btn bm-move" title="Make it a part of the bookmark above" disabled={!sel || !where || where.index === 0} onClick={indent}>
        <Icon name="right" size={16} />
        Indent
      </button>
      <button className="te-btn bm-move" title="Move it out of its group" disabled={!sel || !where || where.parent == null} onClick={outdent}>
        <Icon name="left" size={16} />
        Outdent
      </button>
    </div>
  );
}

/** The editable bookmark tree, in the Edit rail and in the sidebar: one store, one undo step per change. */
export function BookmarkTree({ doc, className }: { doc: number; className?: string }) {
  const d = useNav((s) => s.docs[doc]);
  const rev = useStore((s) => s.docs[doc]?.rev);
  const showProblems = useNav((s) => s.showProblems);
  const tree = d?.bookmarks ?? null;
  const all = tree?.items ?? [];
  const items = showProblems ? onlyProblems(all) : all;
  const rows = visibleRows(items);
  const [drag, setDrag] = useState<number | null>(null);
  const [drop, setDrop] = useState<Drop | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; b: Bookmark } | null>(null);

  useEffect(() => {
    void nav().loadBookmarks(doc);
  }, [doc, rev]);

  const go = (b: Bookmark) => {
    if (b.dest.kind === "page") useStore.getState().goTo(doc, b.dest.page, b.dest.at ? Math.max(0, b.dest.at[1] - 8) : undefined);
  };

  const dropOn = (target: Bookmark, at: Drop["at"]) => {
    const moving = drag;
    setDrag(null);
    setDrop(null);
    if (moving == null || moving === target.id) return;
    const w = locate(all, target.id);
    if (!w) return;
    if (at === "into") void nav().moveBookmark(doc, moving, target.id, findBookmark(all, target.id)?.children.length ?? 0);
    else void nav().moveBookmark(doc, moving, w.parent, at === "before" ? w.index : w.index + 1);
  };

  const entries = (b: Bookmark): MenuEntry[] => [
    {
      label: "Add bookmark",
      onSelect: () => {
        nav().selectBookmark(doc, b.id);
        void nav().addBookmark(doc);
      },
    },
    {
      label: "Add bookmark inside",
      onSelect: () => {
        nav().selectBookmark(doc, b.id);
        void nav().addBookmark(doc, { inside: true });
      },
    },
    { label: "Rename", hint: "F2", onSelect: () => nav().setRenaming(doc, b.id) },
    { label: "Set destination to current view", onSelect: () => void nav().setDestHere(doc, b.id) },
    "sep",
    { label: "Delete", hint: "Del", danger: true, onSelect: () => void nav().remove(doc, [b.id]) },
  ];

  if (!tree) return <p className="dim te-hint">Loading…</p>;
  if (all.length === 0) {
    return <p className="dim te-hint">This document has no bookmarks. Select text and press New to make one from it, or make them from the headings.</p>;
  }
  if (rows.length === 0) return <p className="dim te-hint">No problems: every bookmark goes somewhere.</p>;
  return (
    <>
      <ul className={className ? `bm-list ${className}` : "bm-list"} role="tree" aria-label="Bookmarks" onDragLeave={() => setDrop(null)}>
        {rows.map(({ b, depth }) => (
          <li key={b.id} role="treeitem" aria-selected={d?.selectedBm === b.id} aria-expanded={b.children.length > 0 ? b.open : undefined}>
            <Row
              doc={doc}
              b={b}
              depth={depth}
              selected={d?.selectedBm === b.id}
              renaming={d?.renaming === b.id}
              drop={drop?.id === b.id ? drop.at : null}
              onGo={() => go(b)}
              onMenu={(x, y) => {
                nav().selectBookmark(doc, b.id);
                setMenu({ x, y, b });
              }}
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
      {tree.truncated && <p className="te-note">This document has more bookmarks than are shown here.</p>}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={entries(menu.b)} onClose={() => setMenu(null)} />}
    </>
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
  onMenu: (x: number, y: number) => void;
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

  const state = bookmarkState(b);
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
  const what = b.dest.kind === "broken" ? b.dest.what : undefined;
  return (
    <div
      className={`bm-row${p.selected ? " on" : ""}${p.drop ? ` drop-${p.drop}` : ""}`}
      style={{ paddingLeft: 6 + p.depth * 16 }}
      draggable={!p.renaming}
      onContextMenu={(e) => {
        e.preventDefault();
        p.onMenu(e.clientX, e.clientY);
      }}
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
      <span className={`st-dot ${state}`} role="img" aria-label={stateLabel(state, what)} title={stateLabel(state, what)} />
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
