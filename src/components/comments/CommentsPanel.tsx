import { useEffect, useMemo, useRef, useState } from "react";
import { type Tool, useAnnot } from "../../lib/annot";
import {
  type Annot, FILTERS, KIND_LABEL, STAMPS, groupByPage, quadsFromRects, repliesOf, rgbHex, roots, timeLabel,
} from "../../lib/annotLogic";
import { type DocState, useStore } from "../../store";
import { Icon, type IconName } from "../Icons";
import "./comments.css";

const st = () => useAnnot.getState();

/** The toolbar's Comments button. */
export function CommentsToggle() {
  const open = useAnnot((s) => s.open);
  return (
    <button className={open ? "icon-btn on" : "icon-btn"} title="Comments (Ctrl+Alt+M)" aria-label="Comments" aria-pressed={open} onClick={() => st().setOpen(!open)}>
      <Icon name="chat" />
    </button>
  );
}

const TOOLS: { tool: Tool; icon: IconName; label: string; key: string }[] = [
  { tool: "select", icon: "cursor", label: "Select", key: "V" },
  { tool: "highlight", icon: "highlight", label: "Highlight", key: "H" },
  { tool: "underline", icon: "underline", label: "Underline", key: "U" },
  { tool: "strikeOut", icon: "strike", label: "Strikethrough", key: "S" },
  { tool: "squiggly", icon: "squiggle", label: "Squiggly underline", key: "" },
  { tool: "note", icon: "note", label: "Note", key: "N" },
  { tool: "freeText", icon: "textT", label: "Text box", key: "T" },
  { tool: "pen", icon: "pen", label: "Pen", key: "P" },
  { tool: "rectangle", icon: "rect", label: "Rectangle", key: "R" },
  { tool: "ellipse", icon: "ellipse", label: "Ellipse", key: "O" },
  { tool: "line", icon: "line", label: "Line", key: "L" },
  { tool: "arrow", icon: "arrow", label: "Arrow", key: "A" },
  { tool: "polygon", icon: "polygon", label: "Polygon", key: "" },
  { tool: "stamp", icon: "stamp", label: "Stamp", key: "M" },
];

const MARKUP: Record<string, "highlight" | "underline" | "strikeOut" | "squiggly"> = {
  highlight: "highlight", underline: "underline", strikeOut: "strikeOut", squiggly: "squiggly",
};

/** Page-space quads of the current text selection, per page. */
function selectionQuads(): Map<number, ReturnType<typeof quadsFromRects>> {
  const out = new Map<number, ReturnType<typeof quadsFromRects>>();
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return out;
  const rects = Array.from(sel.getRangeAt(0).getClientRects()).filter((r) => r.width > 0.5 && r.height > 0.5);
  for (const el of Array.from(document.querySelectorAll<HTMLElement>(".page[data-page]"))) {
    const box = el.getBoundingClientRect();
    const space = el.querySelector<HTMLElement>(".pagespace");
    if (!space) continue;
    const m = new DOMMatrix(getComputedStyle(space).transform).inverse();
    const mine = rects.filter((r) => r.left < box.right && r.right > box.left && r.top < box.bottom && r.bottom > box.top);
    const list = mine.map((r) => {
      const c = [[r.left, r.top], [r.right, r.top], [r.left, r.bottom], [r.right, r.bottom]].map(([x, y]) => m.transformPoint({ x: x - box.left, y: y - box.top }));
      const xs = c.map((p) => p.x);
      const ys = c.map((p) => p.y);
      return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)] as [number, number, number, number];
    });
    const quads = quadsFromRects(list);
    if (quads.length) out.set(Number(el.dataset.page), quads);
  }
  return out;
}

/** The floating tool strip over the page area; text markup applies to the selection it finds. */
export function CommentTools({ doc }: { doc: DocState }) {
  const open = useAnnot((s) => s.open);
  const tool = useAnnot((s) => s.tool);
  const stamp = useAnnot((s) => s.stamp);

  useEffect(() => {
    if (!open || !MARKUP[tool]) return;
    const apply = () => {
      const quads = selectionQuads();
      if (quads.size === 0) return;
      window.getSelection()?.removeAllRanges();
      for (const [page, q] of quads) void st().add(doc.id, page, { type: "markup", markup: MARKUP[tool], quads: q });
    };
    // After the browser has settled the selection.
    const up = () => setTimeout(apply, 0);
    window.addEventListener("pointerup", up);
    return () => window.removeEventListener("pointerup", up);
  }, [open, tool, doc.id]);

  useEffect(() => {
    if (!open) return;
    const k = (e: KeyboardEvent) => e.key === "Escape" && !(e.target as HTMLElement)?.closest("input, textarea") && st().setTool("select");
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [open]);

  if (!open) return null;
  return (
    <div className="an-strip" role="toolbar" aria-label="Comment tools" onMouseDown={(e) => e.target instanceof HTMLSelectElement || e.preventDefault()}>
      {TOOLS.map((t) => (
        <button
          key={t.tool}
          className={tool === t.tool ? "icon-btn on" : "icon-btn"}
          title={t.key ? `${t.label} (${t.key})` : t.label}
          aria-label={t.label}
          aria-pressed={tool === t.tool}
          onClick={() => st().setTool(t.tool)}
        >
          <Icon name={t.icon} />
        </button>
      ))}
      {tool === "stamp" && (
        <select className="an-stamp" value={stamp} aria-label="Stamp" onChange={(e) => st().setStamp(e.target.value)}>
          {[...STAMPS, "Date"].map((s) => (
            <option key={s}>{s}</option>
          ))}
        </select>
      )}
    </div>
  );
}

const NO_DOC = { items: [] as Annot[], selected: null, editing: null, filter: "all" as const, query: "", error: null };

export function CommentsPanel({ doc }: { doc: DocState }) {
  const open = useAnnot((s) => s.open);
  const id = doc.id;
  const d = useAnnot((s) => s.docs[id]) ?? NO_DOC;
  const author = useAnnot((s) => s.author);
  const groups = useMemo(() => groupByPage(d.items, d.filter, d.query), [d.items, d.filter, d.query]);
  const total = roots(d.items).length;

  useEffect(() => {
    if (open) void st().refresh(id);
  }, [open, id, doc.rev]);

  if (!open) return null;
  const go = (a: Annot) => {
    st().select(id, { page: a.page, xref: a.xref });
    useStore.getState().goTo(id, a.page, Math.max(0, a.rect[1] - 80));
  };
  return (
    <aside className="ax-dock cm-dock" style={{ width: "min(360px, 45vw)" }} aria-label="Comments">
      <section className="ax-panel docked cm-panel">
        <div className="ax-head">
          <span className="ax-title">Comments</span>
          <span className="dim cm-count">{total}</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Comments" onClick={() => st().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="cm-find">
          <input
            className="cm-input"
            placeholder="Search comments"
            aria-label="Search comments"
            value={d.query}
            onChange={(e) => st().setQuery(id, e.target.value)}
          />
          <div className="cm-chips">
            {FILTERS.map((f) => (
              <button key={f.id} className={d.filter === f.id ? "cm-chip on" : "cm-chip"} onClick={() => st().setFilter(id, f.id)}>
                {f.label}
              </button>
            ))}
          </div>
        </div>
        <div className="cm-body">
          {groups.length === 0 && (
            <p className="dim cm-empty">{total === 0 ? "No comments yet. Pick a tool and mark up the page." : "Nothing matches."}</p>
          )}
          {groups.map((g) => (
            <section key={g.page} className="cm-group">
              <h3 className="cm-page">Page {g.page + 1}</h3>
              {g.items.map((a) => (
                <Card key={a.xref} doc={id} a={a} replies={repliesOf(d.items, a.xref)} active={d.selected?.xref === a.xref} onOpen={() => go(a)} />
              ))}
            </section>
          ))}
          {d.error && (
            <p className="cm-error" role="alert">
              {d.error}
            </p>
          )}
        </div>
        <div className="cm-foot">
          <label className="dim" htmlFor="cm-author">
            Your name
          </label>
          <input id="cm-author" className="cm-input" value={author} placeholder="Shown on your comments" onChange={(e) => st().setAuthor(e.target.value)} />
        </div>
      </section>
    </aside>
  );
}

function Card({ doc, a, replies, active, onOpen }: { doc: number; a: Annot; replies: Annot[]; active: boolean; onOpen: () => void }) {
  const [reply, setReply] = useState("");
  const [replying, setReplying] = useState(false);
  const ref = useRef<HTMLElement>(null);
  useEffect(() => {
    if (active) ref.current?.scrollIntoView({ block: "nearest" });
  }, [active]);
  const send = () => {
    void st().reply(doc, a.page, a.xref, reply);
    setReply("");
    setReplying(false);
  };
  return (
    <article ref={ref} className={`cm-card${active ? " on" : ""}${a.resolved ? " resolved" : ""}`}>
      <button className="cm-main" onClick={onOpen}>
        <span className="cm-swatch" style={{ background: rgbHex(a.color, "var(--text-2)") }} />
        <span className="cm-meta">
          <b>{a.author || "Unknown"}</b>
          <span className="dim"> · {KIND_LABEL[a.kind]}</span>
          <span className="dim cm-time">{timeLabel(a.modified || a.created)}</span>
        </span>
        {a.contents && <span className="cm-text">{a.contents}</span>}
      </button>
      {replies.map((r) => (
        <div key={r.xref} className="cm-reply">
          <b>{r.author || "Unknown"}</b> <span className="dim">{timeLabel(r.created)}</span>
          <div>{r.contents}</div>
        </div>
      ))}
      {replying ? (
        <div className="cm-compose">
          <textarea
            autoFocus
            rows={2}
            className="cm-input"
            placeholder="Reply"
            value={reply}
            onChange={(e) => setReply(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) send();
              else if (e.key === "Escape") setReplying(false);
            }}
          />
          <button className="cm-btn primary" disabled={!reply.trim()} onClick={send}>
            Reply
          </button>
        </div>
      ) : (
        <div className="cm-actions">
          <button className="cm-link" onClick={() => setReplying(true)}>
            <Icon name="reply" size={13} /> Reply
          </button>
          <button className="cm-link" onClick={() => void st().setResolved(doc, a.page, a.xref, !a.resolved)}>
            <Icon name="check" size={13} /> {a.resolved ? "Reopen" : "Resolve"}
          </button>
          <span className="grow" />
          <button className="cm-link" aria-label="Delete comment" title="Delete" onClick={() => void st().remove(doc, [{ page: a.page, xref: a.xref }])}>
            <Icon name="trash" size={13} />
          </button>
        </div>
      )}
    </article>
  );
}
