import { memo, useEffect, useMemo, useRef, useState } from "react";
import { type Pt, type Rect4, rectToScreen } from "../../lib/annotLogic";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import {
  DEFAULT_WIDTH, type Draft, type Item, type MarkKind, type Saved, canSign, detailLines, idKey, imageRect, picture, sigBox, today, useSign,
} from "../../lib/sign";
import { Icon } from "../Icons";

interface Props {
  docId: number;
  page: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
}

const EMPTY: Item[] = [];
const MARKS: MarkKind[] = ["check", "cross", "dot", "circle", "line"];
const sg = () => useSign.getState();

/** Fill & Sign's layer on one page: placing items, moving them, and the digital signature box. */
export const SignLayer = memo(function SignLayer(p: Props) {
  const open = useSign((s) => s.open);
  const draft = useSign((s) => s.docs[p.docId]?.draft ?? null);
  // The signing box stays on the page while the sheet is open, even after the strip closes.
  return open ? <Layer {...p} /> : draft && draft.page === p.page ? <DraftBox {...p} draft={draft} /> : null;
});

type Drag = { item: Item; start: Pt; rect: Rect4; resize: boolean; moved: boolean };

function Layer({ docId, page, rot, rawSize, zoom }: Props) {
  const tool = useSign((s) => s.tool);
  const items = useSign((s) => s.docs[docId]?.items ?? EMPTY);
  const selected = useSign((s) => s.docs[docId]?.selected ?? null);
  const draft = useSign((s) => s.docs[docId]?.draft ?? null);
  const fields = useSign((s) => s.docs[docId]?.sigs)?.filter((f) => f.status === "unsigned" && f.page === page);
  const saved = useSign((s) => s.saved);
  const pick = useSign((s) => s.pick);
  const mine = useMemo(() => items.filter((a) => a.page === page), [items, page]);
  const sel = mine.find((a) => a.xref === selected) ?? null;
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [text, setText] = useState<Pt | null>(null);
  const [box, setBox] = useState<{ a: Pt; b: Pt } | null>(null);

  useEffect(() => {
    setText(null);
    setBox(null);
  }, [tool]);

  const toPage = (e: { clientX: number; clientY: number }): Pt => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = space.current!.parentElement!.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [Math.min(Math.max(q.x, 0), rawSize[0]), Math.min(Math.max(q.y, 0), rawSize[1])];
  };

  // ---- placing ----
  const placeImage = (kind: "signature" | "initials", at: Pt) => {
    const s = saved.find((x) => x.id === pick[kind]);
    if (!s) return sg().setCreator(kind);
    void sg().place(docId, { page, type: "image", kind, rect: imageRect(at, s, DEFAULT_WIDTH[kind], rawSize), savedId: s.id });
    sg().setTool("select");
  };
  const down = (e: React.PointerEvent) => {
    const q = toPage(e);
    if (tool === "text") {
      // Keep the pointer from taking focus back from the editor that mounts now.
      e.preventDefault();
      return setText(q);
    }
    if (tool === "date") return void sg().place(docId, { page, type: "text", at: [q[0], q[1] - 8], text: today() });
    if ((MARKS as string[]).includes(tool)) return void sg().place(docId, { page, type: "mark", mark: tool as MarkKind, at: q });
    if (tool === "signature" || tool === "initials") return placeImage(tool, q);
    if (tool === "digital") {
      e.currentTarget.setPointerCapture(e.pointerId);
      setBox({ a: q, b: q });
    }
  };
  const boxUp = () => {
    if (!box) return;
    setBox(null);
    sg().setDraft(docId, { page, rect: sigBox(box.a, box.b, rawSize) });
    sg().setRail("sheet");
  };

  // ---- select, move, resize ----
  const startDrag = (e: React.PointerEvent, item: Item, resize: boolean) => {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    sg().select(docId, item.xref);
    setDrag({ item, start: toPage(e), rect: item.rect, resize, moved: false });
  };
  const dragMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const q = toPage(e);
    const [dx, dy] = [q[0] - drag.start[0], q[1] - drag.start[1]];
    const r0 = drag.item.rect;
    let rect: Rect4;
    if (drag.resize) {
      // Corner resize keeps the picture's shape.
      const w = Math.max(12, r0[2] - r0[0] + dx);
      rect = [r0[0], r0[1], r0[0] + w, r0[1] + (w * (r0[3] - r0[1])) / (r0[2] - r0[0])];
    } else rect = [r0[0] + dx, r0[1] + dy, r0[2] + dx, r0[3] + dy];
    setDrag({ ...drag, rect, moved: drag.moved || Math.hypot(dx, dy) * zoom > 3 });
  };
  const dragEnd = () => {
    const d = drag;
    setDrag(null);
    if (d?.moved) void sg().move(docId, page, d.item.xref, d.rect);
  };

  useEffect(() => {
    if (!sel) return;
    const k = (e: KeyboardEvent) => {
      if ((e.key === "Delete" || e.key === "Backspace") && !(e.target as HTMLElement)?.closest("input, textarea")) {
        e.preventDefault();
        void sg().remove(docId, page, sel.xref);
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [sel, docId, page]);

  const style = (r: Rect4) => ({ left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] });
  const frame = drag && sel && drag.item.xref === sel.xref ? drag.rect : sel?.rect;
  const isImage = (a: Item) => a.item === "signature" || a.item === "initials";

  return (
    <>
      <div
        ref={space}
        className="an-space sg-space"
        style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
      >
        {tool === "select" &&
          mine.map((a) => (
            <div
              key={a.xref}
              className="an-hit sg-hit"
              style={style(a.rect)}
              onPointerDown={(e) => startDrag(e, a, false)}
              onPointerMove={dragMove}
              onPointerUp={dragEnd}
            />
          ))}
        {tool === "select" && sel && frame && (
          <div className="an-frame" style={style(frame)}>
            {isImage(sel) && <span className="an-h se" onPointerDown={(e) => startDrag(e, sel, true)} onPointerMove={dragMove} onPointerUp={dragEnd} />}
          </div>
        )}
        {tool === "digital" &&
          fields?.filter((f) => f.field !== draft?.field).map((f) => (
            <button
              key={f.field}
              className="sg-field"
              style={style(f.rect)}
              title={canSign(f) ? `Sign in "${f.field}"` : (f.refused ?? "")}
              aria-disabled={canSign(f) ? undefined : true}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={() => canSign(f) && sg().signField(docId, f)}
            >
              <span>{canSign(f) ? "Sign here" : "Can't sign"}</span>
            </button>
          ))}
        {box && <div className="sg-drag" style={style(sigBox(box.a, box.b, [Infinity, Infinity]))} />}
        {draft && draft.page === page && !box && <Preview draft={draft} />}
        {text && (
          <TextEditor
            at={text}
            onDone={(t) => {
              setText(null);
              if (t.trim()) void sg().place(docId, { page, type: "text", at: text, text: t.trim(), fontSize: 12 });
            }}
          />
        )}
      </div>
      {tool !== "select" && !text && (
        <div className={`an-capture sg-capture ${tool}`} onPointerDown={down} onPointerMove={(e) => box && setBox({ a: box.a, b: toPage(e) })} onPointerUp={boxUp} />
      )}
      {tool === "select" && sel && !drag && <ItemBar docId={docId} item={sel} anchor={rectToScreen(sel.rect, rawSize, rot, zoom)} />}
    </>
  );
}

/** The signing box with its live preview, drawn while the strip is closed. */
function DraftBox({ rawSize, rot, zoom, draft }: Props & { draft: Draft }) {
  return (
    <div className="an-space sg-space" style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom) }}>
      <Preview draft={draft} />
    </div>
  );
}

/** What the signature will look like, inside its box (same layout as sign/pdf.rs `appearance`). */
function Preview({ draft }: { draft: Draft }) {
  const who = useSign((s) => s.ids.find((i) => idKey(i) === s.idChoice));
  const reason = useSign((s) => s.reason);
  const pic = useSign((s) => (s.picture ? picture(s) : null));
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(t);
  }, []);
  const [x0, y0, x1, y1] = draft.rect;
  const [w, h] = [x1 - x0, y1 - y0];
  const name = who?.name ?? "Your name";
  const lines = detailLines(name, now, reason);
  const pad = Math.min(Math.max(h * 0.06, 1), 6);
  const wide = w >= h * 2.2;
  const L = wide ? [pad, pad, w * 0.45, h - pad] : [pad, pad, w - pad, h * 0.55];
  const R = wide ? [w * 0.47, pad, w - pad, h - pad] : [pad, h * 0.57, w - pad, h - pad];
  // Helvetica's average advance is about half an em.
  const nameFs = Math.max(1, Math.min((L[2] - L[0]) / (name.length * 0.55), (L[3] - L[1]) * 0.6));
  const widest = Math.max(...lines.map((l) => l.length));
  const fs = Math.max(1, Math.min((R[2] - R[0]) / (widest * 0.5), (R[3] - R[1]) / (lines.length * 1.2)));
  return (
    <div className="sg-preview" style={{ left: x0, top: y0, width: w, height: h }} aria-label="Signature preview">
      <svg width={w} height={h} viewBox={`0 0 ${w} ${h}`}>
        {pic ? (
          <image href={pic.dataUrl} x={L[0]} y={L[1]} width={L[2] - L[0]} height={L[3] - L[1]} preserveAspectRatio="xMidYMid meet" />
        ) : (
          <text x={L[0]} y={(L[1] + L[3]) / 2 + nameFs * 0.36} fontSize={nameFs} className="sg-pv-name">
            {name}
          </text>
        )}
        {lines.map((l, i) => (
          <text key={i} x={R[0]} y={(R[1] + R[3]) / 2 - (lines.length * fs * 1.2) / 2 + fs * (i * 1.2 + 1)} fontSize={fs} className="sg-pv-line">
            {l}
          </text>
        ))}
      </svg>
    </div>
  );
}

function TextEditor({ at, onDone }: { at: Pt; onDone: (t: string) => void }) {
  const [t, setT] = useState("");
  const done = useRef(false);
  const end = (v: string) => {
    if (done.current) return;
    done.current = true;
    onDone(v);
  };
  return (
    <input
      className="sg-text-edit"
      style={{ left: at[0], top: at[1] - 8, width: Math.max(80, t.length * 7 + 20) }}
      autoFocus
      value={t}
      placeholder="Type"
      aria-label="Text to place"
      onChange={(e) => setT(e.target.value)}
      onBlur={() => end(t)}
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") end(t);
        else if (e.key === "Escape") end("");
      }}
    />
  );
}

/** The small bar over a selected item: flatten its page, delete it. */
function ItemBar({ docId, item, anchor }: { docId: number; item: Item; anchor: Rect4 }) {
  return (
    <div className="sg-itembar" style={{ left: anchor[0], top: Math.max(0, anchor[1] - 40) }} onPointerDown={(e) => e.stopPropagation()}>
      <button className="sg-ib" title="Bake the items on this page into it" onClick={() => void sg().flatten(docId, [item.page])}>
        Flatten
      </button>
      <button className="sg-ib icon" title="Delete (Del)" aria-label="Delete item" onClick={() => void sg().remove(docId, item.page, item.xref)}>
        <Icon name="trash" size={15} />
      </button>
    </div>
  );
}

/** For the strip's tiles. */
export const savedOf = (saved: Saved[], id: string | undefined) => saved.find((s) => s.id === id) ?? null;

