import { useEffect, useMemo, useRef, useState } from "react";
import { TREE_PAGES, useA11y } from "../../lib/a11y";
import {
  type Row,
  type TagDraft,
  type TagMove,
  type TagNode,
  SCOPES,
  draftOf,
  flatten,
  headerCandidates,
  moveTarget,
  nodeLabel,
  problemsByXref,
  roleOptions,
  tagPatch,
} from "../../lib/a11yDepthLogic";
import { plural } from "../../lib/a11yLogic";
import type { DocState } from "../../store";
import { Icon } from "../Icons";

const st = () => useA11y.getState();
const ROW_H = 28;
/** Rows drawn above and below the visible ones. */
const OVERSCAN = 8;

/** The tag tree: collapsible and windowed, so a large tree draws only the rows in view. */
export function TagsTree({ doc }: { doc: DocState }) {
  const t = useA11y((s) => s.docs[doc.id]?.tags);
  const phase = useA11y((s) => s.docs[doc.id]?.phase ?? "idle");
  const id = doc.id;
  const busy = phase !== "idle";
  const tree = t?.tree ?? null;
  const rows = useMemo(() => (tree ? flatten(tree, t?.expanded ?? {}) : []), [tree, t?.expanded]);
  const selected = t?.selected ?? null;
  const problems = useMemo(() => problemsByXref(t?.problems ?? []), [t?.problems]);
  const scope = t?.scope ?? (doc.pageCount > TREE_PAGES ? "page" : "doc");

  const box = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, h: 320 });
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const read = () => setView({ top: el.scrollTop, h: el.clientHeight });
    read();
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, [tree]);

  const at = rows.findIndex((r) => r.key === selected);
  // Keep the selected row in view when the keyboard moves it.
  useEffect(() => {
    const el = box.current;
    if (!el || at < 0) return;
    const y = at * ROW_H;
    if (y < el.scrollTop) el.scrollTop = y;
    else if (y + ROW_H > el.scrollTop + el.clientHeight) el.scrollTop = y + ROW_H - el.clientHeight;
  }, [at]);

  if (t?.error && !tree) {
    return (
      <div className="a11y-card">
        <p className="a11y-error" role="alert">
          {t.error}
        </p>
      </div>
    );
  }
  if (!tree) return <p className="a11y-note">Reading the tags…</p>;
  if (tree.length === 0) {
    return (
      <div className="a11y-card">
        <b>{scope === "page" ? "No tags on this page" : "This document is not tagged"}</b>
        <p>{scope === "page" ? "Show the whole document, or go to a page that has tagged content." : "Autotag builds tags from the layout; you can then edit them here."}</p>
        <div className="a11y-actions">
          {scope === "page" ? (
            <button className="a11y-btn" onClick={() => st().setTagScope(id, "doc")}>
              Show the whole document
            </button>
          ) : (
            <button className="a11y-btn primary" onClick={() => st().setView("check")}>
              Go to the check
            </button>
          )}
        </div>
      </div>
    );
  }

  const first = Math.max(0, Math.floor(view.top / ROW_H) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((view.top + view.h) / ROW_H) + OVERSCAN);
  const node = selected ? rows.find((r) => r.key === selected)?.node ?? null : null;

  const select = (i: number) => {
    const r = rows[Math.min(Math.max(i, 0), rows.length - 1)];
    if (r) void st().selectTag(id, r.key);
  };
  const move = (r: Row, how: TagMove) => void st().moveTag(id, r.node.xref, how);
  const onKey = (e: React.KeyboardEvent) => {
    const r = rows[at];
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const dir = e.key === "ArrowDown" ? 1 : -1;
      if (e.altKey && r && !busy) return move(r, dir === 1 ? "down" : "up");
      select(at < 0 ? 0 : at + dir);
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      if (!r) return select(0);
      if (e.altKey && !busy) return move(r, "in");
      if (r.node.children.length === 0) return;
      if (!r.open) st().toggleTag(id, r.key);
      else select(at + 1);
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      if (!r) return;
      if (e.altKey && !busy) return move(r, "out");
      if (r.open) st().toggleTag(id, r.key);
      else if (r.parentKey) void st().selectTag(id, r.parentKey);
    } else if (e.key === "Home") {
      e.preventDefault();
      select(0);
    } else if (e.key === "End") {
      e.preventDefault();
      select(rows.length - 1);
    } else if ((e.key === "Enter" || e.key === " ") && r && r.node.children.length > 0) {
      e.preventDefault();
      st().toggleTag(id, r.key);
    }
  };

  const nProblems = t?.problems.length ?? 0;
  return (
    <div className="a11y-tags">
      <div className="a11y-actions">
        <label className="a11y-note" htmlFor="a11y-tag-scope">
          Show
        </label>
        <select id="a11y-tag-scope" className="a11y-select" value={scope} onChange={(e) => st().setTagScope(id, e.target.value as "doc" | "page")}>
          <option value="doc">Whole document</option>
          <option value="page">Page {doc.currentPage + 1} only</option>
        </select>
        <span className="grow" />
        <span className="a11y-note">{plural(rows.length, "row")} shown</span>
      </div>
      {nProblems > 0 && (
        <div className="a11y-card" role="status">
          <b>{plural(nProblems, "problem")} in the tag links</b>
          <p>{t!.problems[0].message}</p>
          <div className="a11y-actions">
            <button className="a11y-btn" disabled={busy} onClick={() => void st().fix(id, { fix: "parentTree" })}>
              Repair links
            </button>
          </div>
        </div>
      )}
      {t?.error && (
        <p className="a11y-error" role="alert">
          {t.error}
        </p>
      )}
      <div
        ref={box}
        className="a11y-tree"
        role="tree"
        aria-label="Tags"
        aria-activedescendant={selected ? `tag-${selected}` : undefined}
        tabIndex={0}
        onKeyDown={onKey}
        onScroll={(e) => setView({ top: e.currentTarget.scrollTop, h: e.currentTarget.clientHeight })}
      >
        <div style={{ height: rows.length * ROW_H, position: "relative" }}>
          {rows.slice(first, last).map((r, k) => {
            const i = first + k;
            const bad = problems.get(r.node.xref);
            const hasKids = r.node.children.length > 0;
            return (
              <div
                key={r.key}
                id={`tag-${r.key}`}
                role="treeitem"
                aria-level={r.depth + 1}
                aria-expanded={hasKids ? r.open : undefined}
                aria-selected={r.key === selected}
                aria-posinset={r.index + 1}
                aria-setsize={r.siblings}
                className={`a11y-tag${r.key === selected ? " selected" : ""}`}
                style={{ top: i * ROW_H, height: ROW_H, paddingLeft: 4 + r.depth * 14 }}
                onClick={() => void st().selectTag(id, r.key)}
              >
                <span
                  className={`a11y-tag-chev${hasKids ? "" : " none"}${r.open ? " open" : ""}`}
                  aria-hidden="true"
                  onClick={(e) => {
                    e.stopPropagation();
                    if (hasKids) st().toggleTag(id, r.key);
                  }}
                >
                  <Icon name="right" size={12} />
                </span>
                <span className="a11y-tag-role">{r.node.rawRole || r.node.role}</span>
                {r.node.rawRole && r.node.rawRole !== r.node.role && <span className="a11y-tag-map">as {r.node.role}</span>}
                {r.node.alt && <span className="a11y-tag-alt">{r.node.alt}</span>}
                {hasKids && !r.open && <span className="a11y-tag-n">{r.node.children.length}</span>}
                {bad && (
                  <span className="a11y-tag-bad" title={bad.join("\n")} aria-label={bad.join(". ")}>
                    !
                  </span>
                )}
              </div>
            );
          })}
        </div>
      </div>
      <p className="a11y-note">Arrow keys move through the tree. Alt+Up and Alt+Down move a tag, Alt+Right puts it into the one above, Alt+Left takes it out.</p>
      {node ? <TagEditor doc={doc} node={node} tree={tree} busy={busy} /> : <p className="a11y-note">Select a tag to edit it. Its content is outlined on the page.</p>}
    </div>
  );
}

function TagEditor({ doc, node, tree, busy }: { doc: DocState; node: TagNode; tree: TagNode[]; busy: boolean }) {
  const id = doc.id;
  const [d, setD] = useState<TagDraft>(() => draftOf(node));
  // A different tag, or the same one after an edit, starts from what the engine now says.
  useEffect(() => setD(draftOf(node)), [node]);
  const cells = useMemo(() => (node.role === "TD" || node.role === "TH" ? headerCandidates(tree, node.xref) : []), [tree, node]);
  // The type is its own step (Change type); Apply is for the properties.
  const was = draftOf(node);
  const patch = tagPatch(node, { ...d, role: was.role });
  const roleChanged = d.role.trim() !== was.role;
  const locked = !node.editable || node.xref <= 0;
  const set = (p: Partial<TagDraft>) => setD((x) => ({ ...x, ...p }));
  const moves: { how: TagMove; label: string }[] = [
    { how: "up", label: "Move up" },
    { how: "down", label: "Move down" },
    { how: "in", label: "Into the tag above" },
    { how: "out", label: "Out of its parent" },
  ];

  if (locked) {
    return <p className="a11y-note">This tag is written inside its parent, so it can be read but not edited.</p>;
  }
  return (
    <form
      className="a11y-card a11y-tag-edit"
      onSubmit={(e) => {
        e.preventDefault();
        if (!patch || busy) return;
        void st().editTag(id, node.xref, patch);
      }}
    >
      <b>{nodeLabel(node)}</b>
      <div className="a11y-tag-type">
        <label>
          <span>Type</span>
          <select className="a11y-select" value={d.role} onChange={(e) => set({ role: e.target.value })}>
            {roleOptions(was.role).map((r) => (
              <option key={r} value={r}>
                {r === "Artifact" ? "Artifact (take out of the tree)" : r}
              </option>
            ))}
          </select>
        </label>
        <button type="button" className="a11y-btn" disabled={busy || !roleChanged} title="One undo step" onClick={() => void st().setRole(id, node.xref, d.role.trim())}>
          Change type
        </button>
      </div>
      <label>
        <span>Alternate text</span>
        <input type="text" value={d.alt} onChange={(e) => set({ alt: e.target.value })} />
      </label>
      <label>
        <span>Actual text</span>
        <input type="text" value={d.actualText} onChange={(e) => set({ actualText: e.target.value })} />
      </label>
      <label>
        <span>Language</span>
        <input type="text" value={d.lang} placeholder="en-US" onChange={(e) => set({ lang: e.target.value })} />
      </label>
      {node.role === "TH" && (
        <label>
          <span>Scope</span>
          <select className="a11y-select" value={d.scope} onChange={(e) => set({ scope: e.target.value })}>
            <option value="">Not set</option>
            {SCOPES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </label>
      )}
      {cells.length > 0 && (
        <fieldset>
          <legend>Read with these header cells</legend>
          {cells.map((c) => (
            <label key={c.xref} className="a11y-check">
              <input
                type="checkbox"
                checked={d.headers.includes(c.xref)}
                onChange={(e) => set({ headers: e.target.checked ? [...d.headers, c.xref] : d.headers.filter((h) => h !== c.xref) })}
              />
              <span>{c.alt || `Header cell ${c.xref}`}</span>
            </label>
          ))}
        </fieldset>
      )}
      <div className="a11y-actions">
        <button type="submit" className="a11y-btn primary" disabled={busy || !patch} title="One undo step">
          Apply
        </button>
        <button type="button" className="a11y-quiet" disabled={!patch} onClick={() => setD(draftOf(node))}>
          Reset
        </button>
      </div>
      <div className="a11y-actions" role="group" aria-label="Move the tag">
        {moves.map((m) => (
          <button key={m.how} type="button" className="a11y-quiet" disabled={busy || !moveTarget(tree, node.xref, m.how)} onClick={() => void st().moveTag(id, node.xref, m.how)}>
            {m.label}
          </button>
        ))}
      </div>
    </form>
  );
}

