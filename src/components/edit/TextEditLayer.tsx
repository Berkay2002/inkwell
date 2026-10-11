import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { TextBlock } from "../../lib/api";
import { redo, undo } from "../../lib/edit";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import { NEW_TEXT, type Rect4, caretFor, cssColor, cssFamily, isOffered, useTextEdit } from "../../lib/textEdit";

interface Props {
  docId: number;
  page: number;
  /** Document generation: the page's blocks are listed again when it changes. */
  rev: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
  /** Near the viewport: only these pages list their blocks. */
  visible: boolean;
}

const te = () => useTextEdit.getState();
const MIN_WIDTH = 24;

/** Edit mode's layer on one page: block outlines, the editor over the selected block, and the add-text box. */
export const TextEditLayer = memo(function TextEditLayer(p: Props) {
  const open = useTextEdit((s) => s.open);
  return open ? <Layer {...p} /> : null;
});

type Drag = { mode: "move" | "l" | "r"; start: [number, number]; d: [number, number] };

function Layer({ docId, page, rev, rot, rawSize, zoom, visible }: Props) {
  const tool = useTextEdit((s) => s.tool);
  const listed = useTextEdit((s) => s.docs[docId]?.pages[page]);
  const selected = useTextEdit((s) => s.docs[docId]?.selected ?? null);
  const draft = useTextEdit((s) => s.docs[docId]?.draft ?? null);
  const adding = useTextEdit((s) => s.docs[docId]?.adding ?? null);
  const anySelected = selected != null;
  const space = useRef<HTMLDivElement>(null);
  const caretAt = useRef<[number, number] | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);

  useEffect(() => {
    if (visible) void te().load(docId, page);
  }, [docId, page, rev, visible]);

  const toPage = (e: { clientX: number; clientY: number }): [number, number] => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = space.current!.parentElement!.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [Math.min(Math.max(q.x, 0), rawSize[0]), Math.min(Math.max(q.y, 0), rawSize[1])];
  };

  const mine = selected && selected.page === page ? (listed?.blocks[selected.index] ?? null) : null;
  const typed = mine && draft && draft.page === page && draft.index === selected!.index ? draft.text : (mine?.text ?? "");

  const down = (e: React.PointerEvent, b: TextBlock) => {
    e.stopPropagation();
    e.preventDefault();
    if (!b.editable) {
      void te().select(docId, null);
      te().setNotice(docId, b.reason ? `This text can't be edited here: ${b.reason}.` : "This text can't be edited here.");
      return;
    }
    caretAt.current = toPage(e);
    void te().select(docId, { page, index: b.index });
  };

  // ---- move and resize the selected block ----
  const startDrag = (e: React.PointerEvent, mode: Drag["mode"]) => {
    e.stopPropagation();
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    setDrag({ mode, start: toPage(e), d: [0, 0] });
  };
  const dragTo = (e: React.PointerEvent) => {
    if (!drag) return;
    const q = toPage(e);
    setDrag({ ...drag, d: [q[0] - drag.start[0], q[1] - drag.start[1]] });
  };
  const dragEnd = () => {
    const g = drag;
    setDrag(null);
    if (!g || !mine || Math.hypot(g.d[0], g.d[1]) * zoom < 3) return;
    const w = mine.rect[2] - mine.rect[0];
    if (g.mode === "move") void te().move(docId, g.d);
    else if (g.mode === "r") void te().apply(docId, { width: Math.max(MIN_WIDTH, w + g.d[0]) });
    else {
      const nw = Math.max(MIN_WIDTH, w - g.d[0]);
      void te().apply(docId, { width: nw, offset: [w - nw, 0] });
    }
  };
  // The frame as it is while dragging.
  const view = (r: Rect4): Rect4 => {
    if (!drag) return r;
    const [dx, dy] = drag.d;
    if (drag.mode === "move") return [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy];
    if (drag.mode === "r") return [r[0], r[1], Math.max(r[0] + MIN_WIDTH, r[2] + dx), r[3]];
    return [Math.min(r[2] - MIN_WIDTH, r[0] + dx), r[1], r[2], r[3]];
  };

  return (
    <div
      ref={space}
      className="te-space"
      style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
    >
      {(anySelected || adding) && tool === "select" && (
        <div
          className="te-bg"
          onPointerDown={(e) => {
            e.stopPropagation();
            if (adding) void te().finishAdding(docId);
            else void te().select(docId, null);
          }}
        />
      )}
      {tool === "text" && !adding && (
        <div
          className="te-capture"
          onPointerDown={(e) => {
            e.stopPropagation();
            e.preventDefault();
            te().startAdding(docId, page, toPage(e));
          }}
        />
      )}
      {listed?.areas.map((a, i) => (
        <div key={`a${i}`} className="te-block locked" style={boxStyle(a.rect)} title={`Can't be edited here: ${a.reason}`} />
      ))}
      {listed?.blocks.filter(isOffered).map((b) =>
        mine && b.index === mine.index ? null : (
          <div
            key={b.index}
            className={b.editable ? "te-block" : "te-block locked"}
            style={boxStyle(b.rect)}
            title={b.editable ? "Click to edit" : `Can't be edited here: ${b.reason ?? "this text"}`}
            onPointerDown={(e) => down(e, b)}
          />
        ),
      )}
      {mine && (
        <Editor
          key={`${page}:${mine.index}`}
          block={mine}
          value={typed}
          rect={view(mine.rect)}
          caret={caretAt.current ? caretFor(mine, typed, caretAt.current[0], caretAt.current[1]) : typed.length}
          onChange={(t) => te().setDraft(docId, t)}
          onDone={() => void te().select(docId, null)}
          onCommit={() => void te().commit(docId)}
          onHistory={(again) => void (again ? redo(docId) : undo(docId))}
          grips={{ down: startDrag, move: dragTo, up: dragEnd }}
        />
      )}
      {adding && adding.page === page && (
        <NewBoxEditor
          at={adding.at}
          value={adding.text}
          onChange={(t) => te().setAdding(docId, t)}
          onDone={() => void te().finishAdding(docId)}
        />
      )}
    </div>
  );
}

const boxStyle = (r: Rect4) => ({ left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] });

interface EditorProps {
  block: TextBlock;
  value: string;
  rect: Rect4;
  caret: number;
  onChange: (t: string) => void;
  /** Esc: commit and leave the block. */
  onDone: () => void;
  /** Ctrl+Enter or focus leaving: send what was typed. */
  onCommit: () => void;
  /** Ctrl+Z / Ctrl+Y with nothing typed since the last send: the document's undo, not the field's. */
  onHistory: (redo: boolean) => void;
  grips: {
    down: (e: React.PointerEvent, mode: "move" | "l" | "r") => void;
    move: (e: React.PointerEvent) => void;
    up: () => void;
  };
}

/** The block, drawn over the page in its own font, size and colour: white underneath to cover the original text. */
function Editor({ block, value, rect, caret, onChange, onDone, onCommit, onHistory, grips }: EditorProps) {
  const ta = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const el = ta.current;
    if (!el) return;
    el.focus({ preventScroll: true });
    el.setSelectionRange(caret, caret);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // Grow with the text (the PDF block grows the same way when it reflows).
  useLayoutEffect(() => {
    const el = ta.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.max(el.scrollHeight, block.rect[3] - block.rect[1])}px`;
  }, [value, block, rect[2] - rect[0]]);

  const lead = (block.lineSpacing ?? 1.2) * block.size;
  const style: React.CSSProperties = {
    left: rect[0],
    top: rect[1],
    // The browser's face is a little wider or narrower than the PDF's: slack so a line that fits there does not wrap here.
    width: (rect[2] - rect[0]) * 1.12,
    fontFamily: cssFamily(block.font),
    fontSize: block.size,
    lineHeight: `${lead}px`,
    fontWeight: block.bold ? 700 : 400,
    fontStyle: block.italic ? "italic" : "normal",
    color: cssColor(block.color),
    textAlign: block.align,
  };
  return (
    <>
      <textarea
        ref={ta}
        className="te-editor"
        style={style}
        value={value}
        spellCheck={false}
        aria-label="Edit text"
        onChange={(e) => onChange(e.target.value)}
        onBlur={(e) => {
          if (!(e.relatedTarget as HTMLElement | null)?.closest?.(".te-keep")) onCommit();
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            onDone();
          } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            onCommit();
          } else if ((e.ctrlKey || e.metaKey) && value === block.text && ["z", "y"].includes(e.key.toLowerCase())) {
            e.preventDefault();
            e.stopPropagation();
            onHistory(e.key.toLowerCase() === "y" || e.shiftKey);
          }
        }}
      />
      <div className="te-frame" style={{ left: rect[0], top: rect[1], width: rect[2] - rect[0], height: Math.max(rect[3] - rect[1], 1) }}>
        <span className="te-grip t" onPointerDown={(e) => grips.down(e, "move")} onPointerMove={grips.move} onPointerUp={grips.up} title="Drag to move" />
        <span className="te-h l" onPointerDown={(e) => grips.down(e, "l")} onPointerMove={grips.move} onPointerUp={grips.up} title="Drag to rewrap" />
        <span className="te-h r" onPointerDown={(e) => grips.down(e, "r")} onPointerMove={grips.move} onPointerUp={grips.up} title="Drag to rewrap" />
      </div>
    </>
  );
}

/** The box a new piece of text is typed into: a click on the page put its top left here. */
function NewBoxEditor({ at, value, onChange, onDone }: { at: [number, number]; value: string; onChange: (t: string) => void; onDone: () => void }) {
  const ta = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => ta.current?.focus({ preventScroll: true }), []);
  return (
    <textarea
      ref={ta}
      className="te-editor te-new"
      style={{ left: at[0], top: at[1], fontSize: NEW_TEXT.size, lineHeight: `${NEW_TEXT.size * 1.2}px`, color: cssColor(NEW_TEXT.color), fontFamily: cssFamily("Helvetica") }}
      value={value}
      rows={1}
      spellCheck={false}
      aria-label="New text"
      placeholder="Type here"
      onChange={(e) => onChange(e.target.value)}
      onBlur={(e) => {
        if (!(e.relatedTarget as HTMLElement | null)?.closest?.(".te-keep")) onDone();
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape" || (e.key === "Enter" && (e.ctrlKey || e.metaKey))) {
          e.preventDefault();
          e.stopPropagation();
          onDone();
        }
      }}
    />
  );
}
