import { useState } from "react";
import type { ArticleThread } from "../../lib/api";
import { useThreads } from "../../lib/threads";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import type { MenuEntry } from "../Menu";
import { ContextMenu } from "../PageMenu";
import "./threads.css";

const th = () => useThreads.getState();
const EMPTY: ArticleThread[] = [];

/**
 * The sidebar's Articles tab: the article threads of the document with their bead counts. Double click reads from the
 * first bead; right click offers Properties and Delete.
 */
export function ArticlesPanel({ doc }: { doc: DocState }) {
  const id = doc.id;
  const list = useThreads((s) => s.docs[id]);
  const show = useThreads((s) => s.show);
  const tool = useThreads((s) => s.tool);
  const reading = useThreads((s) => (s.follow?.doc === id ? s.follow.thread : null));
  const [menu, setMenu] = useState<{ x: number; y: number; thread: ArticleThread } | null>(null);
  const threads = list ?? EMPTY;

  const read = (t: ArticleThread) => th().startFollow(id, t.id);
  const goTo = (t: ArticleThread) => {
    const b = t.beads[0];
    if (b) useStore.getState().goTo(id, b.page, Math.max(0, b.rect[1] - 8));
  };
  const items = (t: ArticleThread, i: number): MenuEntry[] => [
    { label: "Read from the first bead", onSelect: () => read(t) },
    { label: "Properties…", onSelect: () => th().openProps(id, t.id) },
    "sep",
    ...(i > 0 ? [{ label: "Move up", onSelect: () => void th().reorder(id, t.id, i - 1) }] : []),
    ...(i < threads.length - 1 ? [{ label: "Move down", onSelect: () => void th().reorder(id, t.id, i + 1) }] : []),
    ...(threads.length > 1 ? (["sep"] as MenuEntry[]) : []),
    { label: "Delete article", danger: true, onSelect: () => void th().removeThread(id, t.id) },
  ];

  return (
    <div className="th-panel">
      <div className="th-bar">
        <button className={tool ? "btn primary" : "btn ghost"} aria-pressed={tool} onClick={() => th().setTool(!tool)}>
          <Icon name="article" size={15} />
          {tool ? "Article tool is on" : "New article"}
        </button>
        <label className="th-check">
          <input type="checkbox" checked={show} onChange={(e) => th().setShow(e.target.checked)} />
          Show beads
        </label>
      </div>
      {list == null ? (
        <div className="empty">Loading…</div>
      ) : threads.length === 0 ? (
        <div className="empty">This document has no articles. Use the Article tool to mark a column of text and its continuations.</div>
      ) : (
        <ul className="th-list">
          {threads.map((t) => (
            <li
              key={t.id}
              className={`th-row${reading === t.id ? " reading" : ""}`}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, thread: t });
              }}
            >
              <button
                className="th-open"
                title="Click to go there, double click to read"
                onClick={() => goTo(t)}
                onDoubleClick={() => read(t)}
                onKeyDown={(e) => e.key === "Enter" && (e.preventDefault(), read(t))}
              >
                <b>{t.title || "Untitled"}</b>
                <span className={t.damaged ? "dim bad" : "dim"}>
                  {t.damaged ? t.damaged : `${t.beads.length} ${t.beads.length === 1 ? "bead" : "beads"}${t.info.author ? ` · ${t.info.author}` : ""}`}
                </span>
              </button>
              <button className="ax-icon" title="Read from the first bead" aria-label={`Read ${t.title}`} disabled={t.beads.length === 0} onClick={() => read(t)}>
                <Icon name="right" size={15} />
              </button>
              <button className="ax-icon" title="Properties" aria-label={`Properties of ${t.title}`} onClick={() => th().openProps(id, t.id)}>
                <Icon name="edit" size={15} />
              </button>
              <button className="ax-icon" title="Delete article" aria-label={`Delete ${t.title}`} onClick={() => void th().removeThread(id, t.id)}>
                <Icon name="trash" size={15} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <p className="th-note dim">Enter or Down moves on while reading, Shift+Enter or Up goes back, Esc stops.</p>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={items(menu.thread, threads.indexOf(menu.thread))} onClose={() => setMenu(null)} />}
    </div>
  );
}
