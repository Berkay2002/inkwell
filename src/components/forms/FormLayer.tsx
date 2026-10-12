import { memo, useEffect, useId, useMemo, useRef, useState } from "react";
import { commandFor } from "../../lib/commands";
import { useForms } from "../../lib/forms";
import {
  type ButtonAction, type Field, type FieldValue, type Handle, type Pick, type Rect4, HANDLES, KIND_LABEL, anchorOf, commitThenRun, cssColor, dragged,
  fontSize, groupDelta, groupDragEnd, groupPicks, isCalculated, marqueeHits, move, normBox, onPage, resetTargets, resize, samePick,
} from "../../lib/formsLogic";
import { type Rot, type Size, pageSpaceTransform } from "../../lib/layout";
import { type SignatureInfo, sigFieldView, useSign } from "../../lib/sign";
import { useStore } from "../../store";
import { ContextMenu } from "../PageMenu";
import { layoutMenu, useCertified } from "./layoutTools";
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
const NO_PICKS: Pick[] = [];
const EMPTY_SIGS: SignatureInfo[] = [];
const fm = () => useForms.getState();
/** Ctrl-click is the context menu on a Mac: there only Cmd and Shift add to the picked fields. */
const MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);
const additive = (e: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) => e.metaKey || e.shiftKey || (e.ctrlKey && !MAC);
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

/** `group` is set when the press was on a picked field of a selection of two or more: they all move by `delta`. */
type Drag = {
  mode: "move" | { resize: Handle }; start: Pt; rect0: Rect4; rect: Rect4; moved: boolean; xref: number;
  group: Pick[] | null; delta: Pt;
};

function Layer({ docId, page, rot, rawSize, zoom, all, mine, authoring }: Props & { all: Field[]; mine: Field[]; authoring: boolean }) {
  const highlight = useForms((s) => s.highlight);
  const tool = useForms((s) => s.tool);
  const picked = useForms((s) => s.docs[docId]?.picked ?? NO_PICKS);
  const anchor = anchorOf(picked);
  const certified = useCertified(docId);
  const predicted = useForms((s) => (s.docs[docId]?.detectPage === page ? s.docs[docId]?.detected : null));
  const space = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const [draft, setDraft] = useState<{ a: Pt; b: Pt } | null>(null);
  const [marquee, setMarquee] = useState<{ a: Pt; b: Pt; add: boolean } | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  // A tool change or Esc abandons what is half drawn.
  useEffect(() => setDraft(null), [tool]);
  useEffect(() => {
    if (!authoring) return;
    const k = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || (e.target as HTMLElement | null)?.closest("input, textarea, select")) return;
      setDraft(null);
      setDrag(null); // a drag in progress is cancelled: nothing is sent
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
    if (MAC && e.ctrlKey) return; // the context menu follows
    e.currentTarget.setPointerCapture(e.pointerId);
    // Ctrl, Cmd or Shift adds to the picked fields and does not drag. A plain press on a picked field of a selection of
    // two or more drags them all; on any other field it picks just that one and drags it.
    if (additive(e)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
      fm().toggle(docId, { page: f.page, xref: f.xref });
      return;
    }
    const group = mode === "move" ? groupPicks(picked, f) : null;
    if (!group && !(picked.length === 1 && samePick(picked[0], f))) fm().select(docId, { page: f.page, xref: f.xref });
    setDrag({ mode, start: toPage(e), rect0: f.rect, rect: f.rect, moved: false, xref: f.xref, group, delta: [0, 0] });
  };
  const onDragMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const q = toPage(e);
    const [dx, dy] = [q[0] - drag.start[0], q[1] - drag.start[1]];
    const moved = drag.moved || Math.hypot(dx, dy) * zoom > 3;
    if (drag.group) {
      const rects = mine.filter((m) => drag.group!.some((p) => samePick(p, m))).map((m) => m.rect);
      return setDrag({ ...drag, delta: groupDelta(rects, dx, dy, rawSize), moved });
    }
    const rect = drag.mode === "move" ? move(drag.rect0, dx, dy, rawSize) : resize(drag.rect0, drag.mode.resize, dx, dy, rawSize);
    setDrag({ ...drag, rect, moved });
  };
  const onDragEnd = () => {
    const d = drag;
    setDrag(null);
    if (!d) return;
    if (d.group) {
      // The group goes as one nudge of the picked fields of this page; a click without a drag narrows the selection to the field.
      const end = groupDragEnd(d);
      if (end.kind === "select") fm().select(docId, { page, xref: d.xref });
      else if (end.kind === "nudge") void fm().nudge(docId, end.by[0], end.by[1], page);
    } else if (d.moved) void fm().move(docId, page, d.xref, d.rect);
  };

  const create = () => {
    const d = draft;
    setDraft(null);
    if (!d || tool === "select") return;
    const sel = anchor ? all.find((f) => samePick(f, anchor)) : null;
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
            <div
              className="fm-bg"
              style={{ width: rawSize[0], height: rawSize[1] }}
              onPointerDown={(e) => {
                e.currentTarget.setPointerCapture(e.pointerId);
                const q = toPage(e);
                setMarquee({ a: q, b: q, add: additive(e) });
              }}
              onPointerMove={(e) => marquee && setMarquee({ ...marquee, b: toPage(e) })}
              onPointerUp={() => {
                const m = marquee;
                setMarquee(null);
                if (!m) return;
                // A click (no real box) on the page clears the selection, unless adding.
                if (Math.hypot(m.b[0] - m.a[0], m.b[1] - m.a[1]) * zoom < 4) return m.add ? undefined : fm().select(docId, null);
                fm().pickMany(docId, marqueeHits(mine, page, normBox(m.a, m.b)), m.add);
              }}
              onPointerCancel={() => setMarquee(null)}
            >
              {marquee && <div className="fm-marquee" style={style(normBox(marquee.a, marquee.b))} />}
            </div>
          )}
          {predicted?.map((d, i) => (
            <div key={`p${i}`} className={`fm-pred k-${d.kind}`} style={style(d.rect)} title={`Would add: ${d.name}`} />
          ))}
          {mine.map((f) => {
            const on = picked.some((p) => samePick(p, f));
            const isAnchor = on && picked.length > 1 && !!anchor && samePick(anchor, f);
            const along = drag?.group?.some((p) => samePick(p, f)) ? (drag.moved ? drag.delta : ([0, 0] as Pt)) : null;
            const r: Rect4 = along
              ? [f.rect[0] + along[0], f.rect[1] + along[1], f.rect[2] + along[0], f.rect[3] + along[1]]
              : drag && !drag.group && drag.xref === f.xref ? drag.rect : f.rect;
            return (
              <div
                key={f.xref}
                className={`fm-hit k-${f.kind}${on ? " on" : ""}${isAnchor ? " anchor" : ""}${f.hidden ? " hidden" : ""}`}
                style={style(r)}
                title={`${f.name} · ${KIND_LABEL[f.kind]}`}
                onPointerDown={(e) => tool === "select" && e.button === 0 && startDrag(e, f, "move")}
                onPointerMove={onDragMove}
                onPointerUp={onDragEnd}
                onPointerCancel={() => setDrag(null)}
                onContextMenu={(e) => {
                  if (tool !== "select") return;
                  e.preventDefault();
                  // A right click on a field that is not picked picks it, as in Acrobat.
                  if (!on) fm().select(docId, { page: f.page, xref: f.xref });
                  setMenu({ x: e.clientX, y: e.clientY });
                }}
              >
                <span className="fm-name">{f.name}</span>
                {on && picked.length === 1 && tool === "select" &&
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
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={layoutMenu(docId, fm().get(docId).picked, all, certified)}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

// ---- filling -----------------------------------------------------------------------------------

const ALIGN = ["left", "center", "right"] as const;

/** An empty signature field starts signing into it, a signed one shows its signature; the sheet still asks before anything is signed. */
function SigField({ docId, f, cls, css, label }: { docId: number; f: Field; cls: string; css: React.CSSProperties; label: string }) {
  const list = useSign((s) => s.docs[docId]?.sigs) ?? EMPTY_SIGS;
  const sig = list.find((x) => x.field === f.name);
  const v = sigFieldView(label, sig, list);
  const act = () => {
    // The engine finds the field by the signature list's name and box, so those are sent, not the form's.
    if (sig && v.state === "sign") useSign.getState().signField(docId, sig);
    else if (sig && v.state === "signed") useSign.getState().showSignature(docId, sig.field);
  };
  return (
    <div className="fm-cell" style={css} data-fm-xref={f.xref}>
      {v.state === "unknown" ? (
        <div className={cls} role="img" aria-label={v.title} title={v.title} />
      ) : (
        <button className={cls} aria-label={v.title} title={v.title} aria-disabled={v.state === "locked" || undefined} onClick={act} />
      )}
    </div>
  );
}

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
      <div className="fm-cell" style={css} data-fm-xref={f.xref}>
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
      <div className="fm-cell" style={css} data-fm-xref={f.xref}>
        <button className={cls} aria-label={label} title={buttonTitle(f)} onClick={() => run(docId, f.action, all)} />
      </div>
    );
  }
  if (f.kind === "signature") return <SigField docId={docId} f={f} cls={cls} css={css} label={label} />;
  if (f.kind === "list") {
    return (
      <div className="fm-cell" style={css} data-fm-xref={f.xref}>
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
      <div className="fm-cell" style={css} data-fm-xref={f.xref}>
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
    <div className="fm-cell" style={css} data-fm-xref={f.xref}>
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
  // What is typed, what is saved and how to save it, for the commit when the control goes away (closing the
  // document, switching to Prepare form) without having lost focus first.
  const live = useRef({ v: f.value, saved: f.value, commit: onCommit });
  live.current.saved = f.value;
  live.current.commit = onCommit;
  const show = (s: string) => {
    live.current.v = s;
    setV(s);
  };
  useEffect(() => show(f.value), [f.value, f.xref]);
  useEffect(
    () => () => {
      const l = live.current;
      if (l.v !== l.saved) void l.commit(l.v);
    },
    [],
  );
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
      const typed = e.target.value;
      show(typed);
      onChange();
      // Picking an entry from the list commits at once; typing waits for the field to lose focus.
      const pick = f.kind === "combo" ? f.options.find((o) => o.export === typed || o.label === typed) : undefined;
      if (pick && pick.export !== f.value) void onCommit(pick.export);
    },
    onBlur: () => {
      if (live.current.v !== f.value) void onCommit(live.current.v);
    },
  };
  const keys = (multi: boolean) => (e: React.KeyboardEvent) => {
    e.stopPropagation();
    // Save while typing: the typed value is committed first (it would only commit on blur), then the save runs.
    const cmd = commandFor(e.nativeEvent);
    if (cmd?.id === "save" || cmd?.id === "save-as") {
      e.preventDefault();
      void commitThenRun(live.current.v, f.value, onCommit, () => cmd.run());
    } else if (e.key === "Escape") {
      show(f.value);
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
  if (a.kind === "reset") return `${base} (resets ${a.fields.length ? (a.exclude ? "all but these fields" : "these fields") : "the form"})`;
  return base;
}

/** What a push button does. Scripts, links and submit are shown but do nothing (no network, no scripts). */
function run(docId: number, a: ButtonAction | null, all: Field[]) {
  if (!a) return;
  const s = useStore.getState();
  const d = s.docs[docId];
  switch (a.kind) {
    case "reset": {
      const t = resetTargets(all, a.fields, a.exclude);
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
