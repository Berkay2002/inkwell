import { memo, useEffect, useId, useMemo, useRef, useState } from "react";
import { useForms } from "../../lib/forms";
import {
  type ButtonAction, type Field, type FieldValue, type Handle, type Rect4, HANDLES, KIND_LABEL, cssColor, dragged, fontSize,
  isCalculated, move, onPage, resetTargets, resize,
} from "../../lib/formsLogic";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import { useStore } from "../../store";
import "./forms.css";

interface Props {
  docId: number;
  page: number;
  rot: Rot;
  /** Unrotated page size in points: the layer lives in this space. */
  rawSize: Size;
  zoom: number;
}

const EMPTY: Field[] = [];
const fm = () => useForms.getState();
type Pt = [number, number];

/**
 * Forms on one page. Filling: each field is a control over its widget, see-through until it has focus
 * (the page tile already draws the field). Authoring ("Prepare form"): hit areas to select, move and
 * resize, and a capture layer to drag new fields out.
 */
export const FormLayer = memo(function FormLayer(p: Props) {
  const fields = useForms((s) => s.docs[p.docId]?.fields ?? EMPTY);
  const authoring = useForms((s) => s.authoring);
  const mine = useMemo(() => onPage(fields, p.page), [fields, p.page]);
  if (mine.length === 0 && !authoring) return null;
  return <Layer {...p} all={fields} mine={mine} authoring={authoring} />;
});

type Drag = { mode: "move" | { resize: Handle }; start: Pt; rect0: Rect4; rect: Rect4; moved: boolean; xref: number };

function Layer({ docId, page, rot, rawSize, zoom, all, mine, authoring }: Props & { all: Field[]; mine: Field[]; authoring: boolean }) {
  const highlight = useForms((s) => s.highlight);
  const tool = useForms((s) => s.tool);
  const selected = useForms((s) => s.docs[docId]?.selected ?? null);
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [draft, setDraft] = useState<{ a: Pt; b: Pt } | null>(null);

  // A tool change or Esc abandons what is half drawn.
  useEffect(() => setDraft(null), [tool]);
  useEffect(() => {
    if (!authoring) return;
    const k = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || (e.target as HTMLElement | null)?.closest("input, textarea, select")) return;
      setDraft(null);
      if (fm().tool !== "select") fm().setTool("select");
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [authoring]);

  const toPage = (e: { clientX: number; clientY: number }): Pt => {
    const m = new DOMMatrix(getComputedStyle(space.current!).transform).inverse();
    const r = space.current!.parentElement!.getBoundingClientRect();
    const q = m.transformPoint({ x: e.clientX - r.left, y: e.clientY - r.top });
    return [Math.min(Math.max(q.x, 0), rawSize[0]), Math.min(Math.max(q.y, 0), rawSize[1])];
  };
  const style = (r: Rect4) => ({ left: r[0], top: r[1], width: r[2] - r[0], height: r[3] - r[1] });

  const startDrag = (e: React.PointerEvent, f: Field, mode: Drag["mode"]) => {
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    if (!(selected && selected.page === f.page && selected.xref === f.xref)) fm().select(docId, { page: f.page, xref: f.xref });
    setDrag({ mode, start: toPage(e), rect0: f.rect, rect: f.rect, moved: false, xref: f.xref });
  };
  const onDragMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const q = toPage(e);
    const [dx, dy] = [q[0] - drag.start[0], q[1] - drag.start[1]];
    const rect = drag.mode === "move" ? move(drag.rect0, dx, dy, rawSize) : resize(drag.rect0, drag.mode.resize, dx, dy, rawSize);
    setDrag({ ...drag, rect, moved: drag.moved || Math.hypot(dx, dy) * zoom > 3 });
  };
  const onDragEnd = () => {
    const d = drag;
    setDrag(null);
    if (d?.moved) void fm().move(docId, page, d.xref, d.rect);
  };

  const create = () => {
    const d = draft;
    setDraft(null);
    if (!d || tool === "select") return;
    const sel = selected ? all.find((f) => f.page === selected.page && f.xref === selected.xref) : null;
    // A radio button drawn while another is selected joins its group.
    const group = tool === "radio" && sel?.kind === "radio" ? sel.name : undefined;
    void fm().add(docId, page, { kind: tool, rect: dragged(tool, d.a, d.b, rawSize), group });
    fm().setTool("select");
  };

  return (
    <div
      ref={space}
      className={`fm-space${highlight ? " hl" : ""}${authoring ? " authoring" : ""}`}
      style={{ width: rawSize[0], height: rawSize[1], transform: pageSpaceTransform(rawSize, rot, zoom), "--z": zoom } as React.CSSProperties}
    >
      {!authoring && mine.filter((f) => !f.hidden).map((f) => <Fill key={`${f.xref}`} docId={docId} f={f} all={all} />)}
      {authoring && (
        <>
          {tool === "select" && (
            <div className="fm-bg" style={{ width: rawSize[0], height: rawSize[1] }} onPointerDown={() => fm().select(docId, null)} />
          )}
          {mine.map((f) => {
            const on = selected?.page === f.page && selected.xref === f.xref;
            const r = drag && drag.xref === f.xref ? drag.rect : f.rect;
            return (
              <div
                key={f.xref}
                className={`fm-hit k-${f.kind}${on ? " on" : ""}${f.hidden ? " hidden" : ""}`}
                style={style(r)}
                title={`${f.name} · ${KIND_LABEL[f.kind]}`}
                onPointerDown={(e) => tool === "select" && startDrag(e, f, "move")}
                onPointerMove={onDragMove}
                onPointerUp={onDragEnd}
              >
                <span className="fm-name">{f.name}</span>
                {on && tool === "select" &&
                  HANDLES.map((h) => (
                    <span
                      key={h}
                      className={`fm-h ${h}`}
                      onPointerDown={(e) => startDrag(e, f, { resize: h })}
                      onPointerMove={onDragMove}
                      onPointerUp={onDragEnd}
                    />
                  ))}
              </div>
            );
          })}
          {tool !== "select" && (
            <div
              className="fm-capture"
              style={{ width: rawSize[0], height: rawSize[1] }}
              onPointerDown={(e) => {
                e.currentTarget.setPointerCapture(e.pointerId);
                const q = toPage(e);
                setDraft({ a: q, b: q });
              }}
              onPointerMove={(e) => draft && setDraft({ a: draft.a, b: toPage(e) })}
              onPointerUp={create}
              onPointerCancel={() => setDraft(null)}
            >
              {draft && <div className="fm-draft" style={style(dragged(tool, draft.a, draft.b, rawSize))} />}
            </div>
          )}
        </>
      )}
    </div>
  );
}

// ---- filling -----------------------------------------------------------------------------------

const ALIGN = ["left", "center", "right"] as const;

function Fill({ docId, f, all }: { docId: number; f: Field; all: Field[] }) {
  const [err, setErr] = useState<string | null>(null);
  const locked = f.readOnly || isCalculated(f);
  const empty =
    f.kind === "radio" ? !all.some((x) => x.node === f.node && x.checked) : f.kind === "checkbox" ? !f.checked : f.values.every((v) => v === "");
  const r = f.rect;
  const fs = fontSize(f);
  const css = {
    left: r[0],
    top: r[1],
    width: r[2] - r[0],
    height: r[3] - r[1],
    fontSize: fs,
    textAlign: ALIGN[Math.min(f.align, 2)],
    "--fm-fg": cssColor(f.textColor, "#000"),
    "--fm-bg": cssColor(f.fillColor, "#fff"),
  } as React.CSSProperties;
  const cls = `fm-w k-${f.kind}${f.required && empty ? " req" : ""}${locked ? " ro" : ""}`;

  const commit = async (v: FieldValue) => {
    const e = await fm().setValue(docId, f.page, f.xref, v);
    setErr(e);
    return e;
  };
  const label = f.tooltip || f.name;
  const bubble = err && <span className="fm-err" role="alert">{err}</span>;

  if (f.kind === "checkbox" || f.kind === "radio") {
    return (
      <div className="fm-cell" style={css}>
        <button
          className={cls}
          role={f.kind}
          aria-checked={f.checked}
          aria-label={label}
          title={label}
          disabled={f.readOnly}
          onClick={() => void commit(f.kind === "radio" ? (f.checked && !f.noToggleOff ? false : true) : !f.checked)}
        />
        {bubble}
      </div>
    );
  }
  if (f.kind === "button") {
    return (
      <div className="fm-cell" style={css}>
        <button className={cls} aria-label={label} title={buttonTitle(f)} onClick={() => run(docId, f.action, all)} />
      </div>
    );
  }
  if (f.kind === "signature") {
    return (
      <div className="fm-cell" style={css}>
        <div className={cls} role="img" aria-label={label} title={`${label} (signing is not available here)`} />
      </div>
    );
  }
  if (f.kind === "list") {
    return (
      <div className="fm-cell" style={css}>
        <select
          className={cls}
          multiple
          aria-label={label}
          title={label}
          disabled={f.readOnly}
          value={f.values}
          onChange={(e) => {
            const v = Array.from(e.target.selectedOptions, (o) => o.value);
            void commit(f.multiSelect ? v : (v[0] ?? ""));
          }}
        >
          {f.options.map((o) => (
            <option key={o.export} value={o.export}>{o.label}</option>
          ))}
        </select>
        {bubble}
      </div>
    );
  }
  if (f.kind === "combo" && !f.editable) {
    const cur = f.values[0] ?? "";
    return (
      <div className="fm-cell" style={css}>
        <select className={cls} aria-label={label} title={label} disabled={locked} value={cur} onChange={(e) => void commit(e.target.value)}>
          <option value="" />
          {cur && !f.options.some((o) => o.export === cur) && <option value={cur}>{cur}</option>}
          {f.options.map((o) => (
            <option key={o.export} value={o.export}>{o.label}</option>
          ))}
        </select>
        {bubble}
      </div>
    );
  }
  return (
    <div className="fm-cell" style={css}>
      <TextEditor f={f} className={cls} label={label} locked={locked} onCommit={commit} onChange={() => setErr(null)} />
      {bubble}
    </div>
  );
}

/** A text field (or an editable combo box): edits stay local until the control loses focus or Enter. */
function TextEditor({ f, className, label, locked, onCommit, onChange }: {
  f: Field; className: string; label: string; locked: boolean; onCommit: (v: string) => Promise<string | null>; onChange: () => void;
}) {
  const [v, setV] = useState(f.value);
  const list = useId();
  useEffect(() => setV(f.value), [f.value, f.xref]);
  const width = f.rect[2] - f.rect[0];
  const comb = f.comb && f.maxLen > 0 ? { letterSpacing: Math.max(0, width / f.maxLen - fontSize(f) * 0.55), paddingLeft: Math.max(0, width / f.maxLen / 2 - fontSize(f) * 0.27) } : undefined;
  const props = {
    className,
    "aria-label": label,
    title: label,
    value: v,
    readOnly: locked,
    maxLength: f.maxLen > 0 ? f.maxLen : undefined,
    style: comb,
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
      setV(e.target.value);
      onChange();
    },
    onBlur: () => {
      if (v !== f.value) void onCommit(v);
    },
  };
  const keys = (multi: boolean) => (e: React.KeyboardEvent) => {
    e.stopPropagation();
    if (e.key === "Escape") {
      setV(f.value);
      onChange();
      (e.target as HTMLElement).blur();
    } else if (e.key === "Enter" && (!multi || e.ctrlKey)) {
      e.preventDefault();
      (e.target as HTMLElement).blur();
    }
  };
  if (f.multiline) return <textarea {...props} onKeyDown={keys(true)} />;
  return (
    <>
      <input {...props} type={f.password ? "password" : "text"} list={f.kind === "combo" ? list : undefined} onKeyDown={keys(false)} autoComplete="off" spellCheck={false} />
      {f.kind === "combo" && (
        <datalist id={list}>
          {f.options.map((o) => (
            <option key={o.export} value={o.export}>{o.label}</option>
          ))}
        </datalist>
      )}
    </>
  );
}

function buttonTitle(f: Field): string {
  const a = f.action;
  const base = f.tooltip || f.caption || f.name;
  if (!a) return base;
  if (a.kind === "submit") return `${base} (nothing is sent from Inkwell)`;
  if (a.kind === "reset") return `${base} (resets ${a.fields.length ? "these fields" : "the form"})`;
  return base;
}

/** What a push button does. Scripts, links and submit are shown but do nothing (no network, no scripts). */
function run(docId: number, a: ButtonAction | null, all: Field[]) {
  if (!a) return;
  const s = useStore.getState();
  const d = s.docs[docId];
  switch (a.kind) {
    case "reset": {
      const t = resetTargets(all, a.fields);
      void fm().clear(docId, t ?? undefined);
      break;
    }
    case "named": {
      if (!d) break;
      const moves: Record<string, number> = { NextPage: d.currentPage + 1, PrevPage: d.currentPage - 1, FirstPage: 0, LastPage: d.pageCount - 1 };
      const to = moves[a.name];
      if (to !== undefined) s.goTo(docId, to);
      break;
    }
    case "alert":
      s.setUi({ error: a.message });
      break;
    default:
      break;
  }
}
