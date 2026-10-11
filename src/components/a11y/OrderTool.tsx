import { useRef, useState } from "react";
import { useA11y } from "../../lib/a11y";
import { artifactBlocked, mergeRun, orderOf, readAloud, roleWord, splitBlocked } from "../../lib/a11yDepthLogic";
import { plural } from "../../lib/a11yLogic";
import type { DocState } from "../../store";
import { Icon } from "../Icons";

const st = () => useA11y.getState();

/** The Reading order tool: the page's regions in order, with the edits and the text as it would be read. */
export function OrderTool({ doc }: { doc: DocState }) {
  const o = useA11y((s) => s.docs[doc.id]?.order);
  const phase = useA11y((s) => s.docs[doc.id]?.phase ?? "idle");
  const [at, setAt] = useState(1);
  const list = useRef<HTMLUListElement>(null);
  const data = o?.data ?? null;
  const regions = data?.regions ?? [];
  const selected = o?.selected ?? [];
  const busy = phase !== "idle";
  const id = doc.id;

  if (o?.error && !data) {
    return (
      <div className="a11y-card">
        <p className="a11y-error" role="alert">
          {o.error}
        </p>
      </div>
    );
  }
  if (!data) return <p className="a11y-note">Reading page {doc.currentPage + 1}…</p>;
  if (!data.tagged) {
    return (
      <div className="a11y-card">
        <b>This document is not tagged</b>
        <p>The reading order is the order of the tags, so there is none yet. Autotag builds tags from the layout; you can then order them here.</p>
        <div className="a11y-actions">
          <button className="a11y-btn primary" onClick={() => st().setView("check")}>
            Go to the check
          </button>
        </div>
      </div>
    );
  }

  const one = selected.length === 1 ? (regions.find((r) => r.xref === selected[0]) ?? null) : null;
  const merge = mergeRun(regions, selected);
  const order = orderOf(regions);
  const pos = one ? order.indexOf(one.xref) : -1;
  const splitWhy = one ? splitBlocked(one) : "Select one region to split it.";
  const mergeWhy = merge ? null : selected.length < 2 ? "Select two or more regions next to each other in the list." : "The selected regions are not next to each other, or one can't be edited.";
  const artifactWhy = one ? artifactBlocked(one) : "Select one region.";
  const said = readAloud(regions);
  const splitAt = one && !splitWhy ? Math.min(Math.max(at, 1), one.mcids.length - 1) : 1;

  const focusRow = (i: number) => list.current?.querySelectorAll<HTMLElement>("[data-row]")[i]?.focus();
  const onKey = (e: React.KeyboardEvent, i: number) => {
    const r = regions[i];
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const dir = e.key === "ArrowDown" ? 1 : -1;
      if (e.altKey) {
        if (r.xref !== 0 && !busy) void st().moveRegion(id, r.xref, dir).then(() => requestAnimationFrame(() => focusRow(i + dir)));
        return;
      }
      const j = Math.min(Math.max(i + dir, 0), regions.length - 1);
      if (e.shiftKey && regions[j].xref !== 0) st().selectRegion(id, regions[j].xref, "range");
      focusRow(j);
    } else if ((e.key === " " || e.key === "Enter") && r.xref !== 0) {
      e.preventDefault();
      st().selectRegion(id, r.xref, e.ctrlKey || e.metaKey ? "toggle" : "one");
    } else if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      focusRow(e.key === "Home" ? 0 : regions.length - 1);
    }
  };

  return (
    <div className="a11y-order">
      <p className="a11y-note">
        Page {data.page + 1} · {plural(regions.length, "region")}. Drag a number on the page, or select a row and press Alt+Up or Alt+Down.
      </p>
      <div className="a11y-actions" role="toolbar" aria-label="Reading order actions">
        <button className="a11y-btn" disabled={busy || !one || pos <= 0} onClick={() => one && void st().moveRegion(id, one.xref, -1)} title="Alt+Up">
          <Icon name="up" size={12} /> Earlier
        </button>
        <button className="a11y-btn" disabled={busy || !one || pos < 0 || pos >= order.length - 1} onClick={() => one && void st().moveRegion(id, one.xref, 1)} title="Alt+Down">
          <Icon name="down" size={12} /> Later
        </button>
        <button className="a11y-btn" disabled={busy || !merge} title={mergeWhy ?? "One undo step"} onClick={() => void st().mergeSelected(id)}>
          Merge{merge ? ` (${merge.from.length + 1})` : ""}
        </button>
        <button className="a11y-btn" disabled={busy || !one || !!artifactWhy} title={artifactWhy ?? "Takes the content out of the reading order"} onClick={() => one && void st().artifactRegion(id, one.xref)}>
          Artifact
        </button>
      </div>
      <div className="a11y-actions">
        <label className="a11y-note" htmlFor="a11y-split-at">
          Split before item
        </label>
        <select
          id="a11y-split-at"
          className="a11y-select"
          disabled={busy || !one || !!splitWhy}
          value={splitAt}
          title={splitWhy ?? undefined}
          onChange={(e) => setAt(Number(e.target.value))}
        >
          {one && !splitWhy ? (
            one.mcids.slice(1).map((_, i) => (
              <option key={i} value={i + 1}>
                {i + 2} of {one.mcids.length}
              </option>
            ))
          ) : (
            <option value={1}>–</option>
          )}
        </select>
        <button className="a11y-btn" disabled={busy || !one || !!splitWhy} title={splitWhy ?? "One undo step"} onClick={() => one && void st().splitRegion(id, one.xref, one.mcids[splitAt])}>
          Split
        </button>
      </div>
      {o?.error && (
        <p className="a11y-error" role="alert">
          {o.error}
        </p>
      )}
      {regions.length === 0 ? (
        <p className="a11y-note">No tagged content on this page.</p>
      ) : (
        <ul ref={list} className="a11y-ro-list" role="listbox" aria-multiselectable="true" aria-label="Regions in reading order">
          {regions.map((r, i) => {
            const sel = selected.includes(r.xref) && r.xref !== 0;
            return (
              <li key={`${r.xref}.${i}`} role="option" aria-selected={sel}>
                <button
                  data-row
                  className={`a11y-ro-row${sel ? " selected" : ""}`}
                  tabIndex={i === 0 || sel ? 0 : -1}
                  style={{ paddingLeft: 6 + Math.min(r.depth, 6) * 8 }}
                  onClick={(e) => r.xref !== 0 && st().selectRegion(id, r.xref, e.shiftKey ? "range" : e.metaKey || e.ctrlKey ? "toggle" : "one")}
                  onKeyDown={(e) => onKey(e, i)}
                >
                  <span className="a11y-ro-n">{i + 1}</span>
                  <span className="a11y-ro-role">{roleWord(r.role)}</span>
                  <span className="a11y-ro-text">{r.text || (r.role === "Figure" ? (r.alt ? "Described" : "No description") : "")}</span>
                  {r.xref === 0 && <span className="a11y-ro-lock" title="Written inside its parent: it can be read but not edited" />}
                </button>
              </li>
            );
          })}
        </ul>
      )}
      {data.orphans.length > 0 && (
        <p className="a11y-note">
          {plural(data.orphans.length, "piece")} of content on this page {data.orphans.length === 1 ? "is" : "are"} in no tag, so a screen reader skips {data.orphans.length === 1 ? "it" : "them"}: “
          {data.orphans[0].text.trim() || "no text"}”{data.orphans.length > 1 ? " and others" : ""}. The Content marking check lists them.
        </p>
      )}
      <details className="a11y-read" open>
        <summary>Reads as</summary>
        <ol className="a11y-read-list">
          {said.map((t, i) => (
            <li key={i} className={selected.includes(regions[i].xref) && regions[i].xref !== 0 ? "selected" : ""}>
              {t}
              {regions[i].text.length >= 120 && regions[i].role !== "Figure" ? "…" : ""}
            </li>
          ))}
        </ol>
      </details>
    </div>
  );
}
