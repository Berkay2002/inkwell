import { useEffect, useState } from "react";
import type { DestEntry } from "../../lib/api";
import { destState, stateLabel, useNav, viewTarget } from "../../lib/nav";
import { useStore } from "../../store";
import { Icon } from "../Icons";

const nav = () => useNav.getState();

/** Where a destination goes, in the row: its page, or why it goes nowhere. */
function pageText(e: DestEntry): string {
  const d = e.dest;
  return d.kind === "page" ? `p. ${d.page + 1}` : d.kind === "broken" ? "Broken" : d.kind === "uri" ? "Web address" : "Other";
}

/** Which of the two places holds it; shown only when the file has both. */
function sourceText(e: DestEntry): string {
  return e.inDests && e.inTree ? "In both" : e.inDests ? "/Dests" : "Name tree";
}

/** The "Named destinations" section of the Bookmarks panel: the file's names, and what can be done to each. */
export function Destinations({ doc }: { doc: number }) {
  const rev = useStore((s) => s.docs[doc]?.rev);
  const list = useNav((s) => s.docs[doc]?.dests ?? null);
  const [sel, setSel] = useState<string | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [adding, setAdding] = useState("");

  useEffect(() => {
    void nav().loadDests(doc);
  }, [doc, rev]);

  if (!list) return <p className="dim te-hint">Looking for named destinations…</p>;
  const off = list.locked ?? list.frozen;
  const cur = list.items.find((e) => e.name === sel) ?? null;

  // The viewer's place now: the page, the point at its top left, and the zoom (1 is 100 %).
  const here = () => ({ ...viewTarget(doc), zoom: Number((useStore.getState().docs[doc]?.zoom ?? 1).toFixed(3)) });
  const go = (e: DestEntry) => {
    if (e.dest.kind === "page") useStore.getState().goTo(doc, e.dest.page, e.dest.at ? Math.max(0, e.dest.at[1] - 8) : undefined);
  };
  const commitRename = (e: DestEntry) => {
    setRenaming(null);
    const to = text.trim();
    if (to && to !== e.name) void nav().destOp(doc, { op: "rename", name: e.name, to }).then(() => setSel(to));
  };

  return (
    <div className="bm-dests">
      {list.locked && <p className="te-note">{list.locked}</p>}
      {!list.locked && list.frozen && <p className="te-note">{list.frozen}</p>}
      {list.signed && !list.locked && (
        <p className="dim te-hint">This document is signed. A change is saved after the signature, which will show that the file was changed.</p>
      )}
      {list.truncated && <p className="te-note">Only the first names are listed (at most 20,000 in each place).</p>}
      {list.items.length === 0 ? (
        <p className="dim te-hint">This document has no named destinations.</p>
      ) : (
        <ul className="bm-list" aria-label="Named destinations">
          {list.items.map((e) => (
            <li key={e.name}>
              <div className={`bm-row${e.name === sel ? " on" : ""}`} onDoubleClick={() => go(e)}>
                <span
                  className={`st-dot ${destState(e.dest)}`}
                  role="img"
                  aria-label={stateLabel(destState(e.dest), e.dest.kind === "broken" ? e.dest.what : undefined)}
                />
                {renaming === e.name ? (
                  <input
                    className="bm-edit te-keep"
                    aria-label="Destination name"
                    value={text}
                    autoFocus
                    onChange={(ev) => setText(ev.target.value)}
                    onBlur={() => commitRename(e)}
                    onKeyDown={(ev) => {
                      if (ev.key === "Enter") (ev.target as HTMLInputElement).blur();
                      else if (ev.key === "Escape") {
                        ev.preventDefault();
                        setRenaming(null);
                      }
                    }}
                  />
                ) : (
                  <button className="bm-title" title={e.name} onClick={() => setSel(e.name)}>
                    {e.name}
                  </button>
                )}
                <span className="dim">{pageText(e)}</span>
              </div>
              {(list.both || e.shadowed || e.refs > 0 || e.maybe || e.twice) && (
                <p className="dim te-hint">
                  {[
                    list.both ? sourceText(e) : null,
                    e.shadowed ? "A viewer that reads /Dests alone does not find it" : null,
                    e.twice ? "Defined more than once" : null,
                    e.refs > 0 ? `Used by ${e.refs}: ${e.usedBy.join(", ")}` : null,
                    e.maybe,
                  ]
                    .filter(Boolean)
                    .join(". ")}
                </p>
              )}
            </li>
          ))}
        </ul>
      )}
      <div className="te-row">
        <button className="te-btn" disabled={!cur || cur.dest.kind !== "page"} onClick={() => cur && go(cur)}>
          Go to
        </button>
        <button
          className="te-btn"
          disabled={!cur || !!off}
          title={off ?? undefined}
          onClick={() => {
            if (!cur) return;
            setText(cur.name);
            setRenaming(cur.name);
          }}
        >
          <Icon name="edit" size={16} />
          Rename
        </button>
        <button
          className="te-btn"
          disabled={!cur || !!off}
          title={off ?? "Make it go to what the viewer shows now"}
          onClick={() => cur && void nav().destOp(doc, { op: "setView", name: cur.name, ...here() })}
        >
          Set to current view
        </button>
        <button
          className="te-btn"
          disabled={!cur || !!off}
          title={off ?? undefined}
          onClick={() => {
            if (!cur) return;
            setSel(null);
            void nav().destOp(doc, { op: "delete", name: cur.name });
          }}
        >
          <Icon name="trash" size={16} />
          Delete
        </button>
      </div>
      <div className="te-row">
        <input
          className="bm-edit te-keep"
          aria-label="New destination name"
          placeholder="New name"
          value={adding}
          disabled={!!off}
          onChange={(e) => setAdding(e.target.value)}
        />
        <button
          className="te-btn"
          disabled={!!off || adding.trim() === ""}
          title={off ?? "Add a destination at what the viewer shows now"}
          onClick={() => void nav().destOp(doc, { op: "add", name: adding, ...here() }).then((n) => n?.startsWith("Added") && setAdding(""))}
        >
          <Icon name="plus" size={16} />
          Add here
        </button>
      </div>
      <button
        className="te-btn"
        disabled={!!off || list.unchecked || list.items.length === 0}
        title={off ?? (list.unchecked ? "What uses the names could not be fully found" : "Takes out every name no bookmark, link or action uses")}
        onClick={() => void nav().destOp(doc, { op: "removeUnused" })}
      >
        Remove unused
      </button>
    </div>
  );
}
