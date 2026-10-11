import { useEffect, useRef, useState } from "react";
import type { TextAlign, TextBlock } from "../../lib/api";
import { displayFont, hexOf, isOffered, rgbOf, useTextEdit } from "../../lib/textEdit";
import { type DocState } from "../../store";
import { Icon } from "../Icons";
import "./edit.css";

const te = () => useTextEdit.getState();

const SWATCHES = ["#1a1c20", "#6b675f", "#2e4b8f", "#2f7d4f", "#a8321f"];
const ALIGNS: { id: TextAlign; label: string; path: string }[] = [
  { id: "left", label: "Align left", path: "M4 6h16M4 10h10M4 14h16M4 18h10" },
  { id: "center", label: "Centre", path: "M4 6h16M7 10h10M4 14h16M7 18h10" },
  { id: "right", label: "Align right", path: "M4 6h16M10 10h10M4 14h16M10 18h10" },
  { id: "justify", label: "Justify", path: "M4 6h16M4 10h16M4 14h16M4 18h16" },
];
const SPACINGS = [1, 1.15, 1.2, 1.5, 2];

/** The toolbar-less entry point is the Edit segment of the mode pill (Organize.tsx); this is its rail. */
export function EditPanel({ doc }: { doc: DocState }) {
  const open = useTextEdit((s) => s.open);
  const d = useTextEdit((s) => s.docs[doc.id]);
  const tool = useTextEdit((s) => s.tool);
  const block = d?.selected ? (d.pages[d.selected.page]?.blocks[d.selected.index] ?? null) : null;
  const id = doc.id;
  const here = d?.pages[doc.currentPage]?.blocks.filter(isOffered) ?? [];

  if (!open || doc.organize) return null;
  return (
    <aside className="ax-dock te-dock" style={{ width: "min(320px, 45vw)" }} aria-label="Edit">
      <section className="ax-panel docked te-panel">
        <div className="ax-head">
          <span className="ax-title">Edit</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Edit" onClick={() => te().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="te-body">
          <section className="te-sec">
            <h3 className="te-h3">Add content</h3>
            <div className="te-add">
              <button
                className={tool === "text" ? "te-btn on" : "te-btn"}
                aria-pressed={tool === "text"}
                title="Click the page, then type"
                onClick={() => te().setTool(tool === "text" ? "select" : "text")}
              >
                <Icon name="textT" size={16} />
                Text
              </button>
              <button className="te-btn" disabled title="Image editing is coming">
                <Icon name="area" size={16} />
                Image
                <span className="te-soon">soon</span>
              </button>
            </div>
          </section>

          <section className="te-sec">
            <h3 className="te-h3">Format text</h3>
            {block && (
              <p className="te-summary">
                <b>Editing “{excerpt(block.text, 36)}”</b>
                <br />
                <span className="dim">
                  {displayFont(block.font)} · {block.lines.length === 1 ? "1 line" : `${block.lines.length} lines`}
                </span>
              </p>
            )}
            {block ? <Format key={`${d?.selected?.page}:${block.index}`} id={id} block={block} /> : <Hint empty={!d?.notice} />}
            {d?.notice && (
              <p className="te-note" role="status">
                {d.notice}
              </p>
            )}
          </section>
          {here.length > 0 && (
            <section className="te-sec">
              <h3 className="te-h3">
                On page {doc.currentPage + 1} · {here.length} {here.length === 1 ? "block" : "blocks"}
              </h3>
              <ul className="te-list">
                {here.map((b) => (
                  <li key={b.index}>
                    <button
                      className={d?.selected?.page === doc.currentPage && d.selected.index === b.index ? "te-item on" : "te-item"}
                      onClick={() =>
                        b.editable
                          ? void te().select(id, { page: doc.currentPage, index: b.index })
                          : te().setNotice(id, b.reason ? `This text can't be edited here: ${b.reason}.` : "This text can't be edited here.")
                      }
                    >
                      <span className="te-excerpt">{excerpt(b.text, 44) || "Symbols"}</span>
                      {!b.editable && <span className="te-tag">locked</span>}
                    </button>
                  </li>
                ))}
              </ul>
            </section>
          )}
        </div>
        <div className="te-foot dim">Changes apply when you click away. Undo with Ctrl+Z.</div>
      </section>
    </aside>
  );
}

function Hint({ empty }: { empty: boolean }) {
  return empty ? <p className="dim te-hint">Click text on the page to edit it. Dotted outlines show what can be changed.</p> : null;
}

/** Keeps focus where it is (the text being typed) when a control is pressed. */
const keepFocus = { onMouseDown: (e: React.MouseEvent) => e.preventDefault() };

function Format({ id, block }: { id: number; block: TextBlock }) {
  const apply = (patch: Parameters<ReturnType<typeof te>["apply"]>[1]) => void te().apply(id, patch);
  const [size, setSize] = useState(String(round1(block.size)));
  useEffect(() => setSize(String(round1(block.size))), [block.size]);
  const sendSize = () => {
    const v = parseFloat(size);
    if (Number.isFinite(v) && v >= 1 && v <= 400 && Math.abs(v - block.size) > 0.05) apply({ size: v });
    else setSize(String(round1(block.size)));
  };
  const step = (by: number) => apply({ size: Math.min(400, Math.max(1, round1(block.size + by))) });

  const color = useRef<HTMLInputElement>(null);
  // The picker's `change` fires once, when it is closed; React's onChange fires on every drag.
  useEffect(() => {
    const el = color.current;
    if (!el) return;
    const on = () => {
      const c = rgbOf(el.value);
      if (c) void te().apply(id, { color: c });
    };
    el.addEventListener("change", on);
    return () => el.removeEventListener("change", on);
  }, [id]);

  const hex = hexOf(block.color);
  return (
    <div className="te-format">
      <div className="te-row">
        <span className="te-label">Font</span>
        <span className="te-font" title={block.font}>
          {displayFont(block.font)}
        </span>
        <span className="te-style">
          <button
            className={block.bold ? "te-tog on" : "te-tog"}
            aria-pressed={block.bold}
            aria-label="Bold"
            title="Bold: sets the block in the standard face"
            {...keepFocus}
            onClick={() => apply({ bold: !block.bold })}
          >
            <b>B</b>
          </button>
          <button
            className={block.italic ? "te-tog on" : "te-tog"}
            aria-pressed={block.italic}
            aria-label="Italic"
            title="Italic: sets the block in the standard face"
            {...keepFocus}
            onClick={() => apply({ italic: !block.italic })}
          >
            <i>I</i>
          </button>
        </span>
      </div>
      <div className="te-row">
        <span className="te-label">Size</span>
        <span className="te-stepper">
          <button aria-label="Smaller" {...keepFocus} onClick={() => step(-1)}>
            <Icon name="minus" size={14} />
          </button>
          <input
            className="te-keep"
            aria-label="Font size in points"
            inputMode="decimal"
            value={size}
            onChange={(e) => setSize(e.target.value)}
            onBlur={sendSize}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              else if (e.key === "Escape") setSize(String(round1(block.size)));
            }}
          />
          <button aria-label="Larger" {...keepFocus} onClick={() => step(1)}>
            <Icon name="plus" size={14} />
          </button>
        </span>
        <span className="dim">pt</span>
      </div>
      <div className="te-row">
        <span className="te-label">Colour</span>
        <span className="te-swatches">
          {SWATCHES.map((c) => (
            <button
              key={c}
              className={c === hex ? "te-sw on" : "te-sw"}
              style={{ background: c }}
              aria-label={`Colour ${c}`}
              {...keepFocus}
              onClick={() => rgbOf(c) && apply({ color: rgbOf(c)! })}
            />
          ))}
          <label className={SWATCHES.includes(hex) ? "te-sw custom" : "te-sw custom on"} title="Other colour…" style={{ background: hex }}>
            <input ref={color} type="color" className="te-keep" value={hex} onChange={() => {}} aria-label="Other colour" />
          </label>
        </span>
      </div>
      <div className="te-row">
        <span className="te-label">Align</span>
        <span className="te-seg" role="radiogroup" aria-label="Alignment">
          {ALIGNS.map((a) => (
            <button
              key={a.id}
              role="radio"
              aria-checked={block.align === a.id}
              className={block.align === a.id ? "on" : ""}
              aria-label={a.label}
              title={a.label}
              {...keepFocus}
              onClick={() => block.align !== a.id && apply({ align: a.id })}
            >
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden>
                <path d={a.path} />
              </svg>
            </button>
          ))}
        </span>
      </div>
      <div className="te-row">
        <span className="te-label">Spacing</span>
        <select
          className="te-select te-keep"
          aria-label="Line spacing"
          value={nearest(block.lineSpacing ?? 1.2)}
          disabled={block.lineSpacing == null && block.lines.length < 2 && false}
          onChange={(e) => apply({ lineSpacing: Number(e.target.value) })}
        >
          {SPACINGS.map((s) => (
            <option key={s} value={s}>
              {s.toFixed(s === 1 || s === 2 ? 1 : 2)}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}

const excerpt = (t: string, n: number) => {
  const s = t.replace(/\s+/g, " ").trim();
  return s.length > n ? `${s.slice(0, n - 1).trimEnd()}…` : s;
};
const round1 = (v: number) => Math.round(v * 10) / 10;
const nearest = (v: number) => SPACINGS.reduce((a, b) => (Math.abs(b - v) < Math.abs(a - v) ? b : a));
