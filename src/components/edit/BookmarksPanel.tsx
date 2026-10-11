import { useState } from "react";
import type { Bookmark } from "../../lib/api";
import { findBookmark, hexToRgb, rgbToHex, stateLabel, bookmarkState, useNav } from "../../lib/nav";
import { type DocState } from "../../store";
import { Icon } from "../Icons";
import { BookmarkBar, BookmarkTree } from "./BookmarkTree";

const nav = () => useNav.getState();

const SWATCHES = ["#1a1c20", "#2e4b8f", "#2f7d4f", "#a8321f", "#7a4a9e"];

/** The Bookmarks section of the Edit rail: the tree, style, destination and bookmarks from headings. */
export function BookmarksPanel({ doc }: { doc: DocState }) {
  const id = doc.id;
  const d = useNav((s) => s.docs[id]);
  const items = d?.bookmarks?.items ?? [];
  const sel = d?.selectedBm != null ? findBookmark(items, d.selectedBm) : null;
  const [heads, setHeads] = useState(false);

  return (
    <div className="bm">
      <BookmarkBar doc={id} />
      <BookmarkTree doc={id} />
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

/** What the selected bookmark does, and its style. */
function Selected({ doc, b }: { doc: number; b: Bookmark }) {
  const state = bookmarkState(b);
  const where =
    b.dest.kind === "page"
      ? `Goes to page ${b.dest.page + 1}`
      : b.dest.kind === "uri"
        ? "Opens a web address"
        : b.dest.kind === "other"
          ? "Runs an action this editor keeps as it is"
          : b.dest.kind === "broken"
            ? stateLabel(state, b.dest.what)
            : "Goes nowhere";
  const hex = b.color ? rgbToHex(b.color) : null;
  return (
    <section className="te-sec">
      <h3 className="te-h3">Selected bookmark</h3>
      <p className="te-summary">
        <b>{b.title || "(untitled)"}</b>
        <br />
        <span className="dim">
          <span className={`st-dot ${state}`} aria-hidden /> {where}
        </span>
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
  if (h.count === 0) return <p className="dim te-hint">No headings were found: the file has no heading tags and no line is set clearly larger than the body text.</p>;
  return (
    <div className="bm-heads">
      <p className="te-summary">
        <b>
          {h.count} {h.count === 1 ? "bookmark" : "bookmarks"}
        </b>{" "}
        in {h.levels} {h.levels === 1 ? "level" : "levels"},{" "}
        {h.source === "tags" ? "from the heading tags of this file." : "guessed from the text set larger than the rest."}
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
