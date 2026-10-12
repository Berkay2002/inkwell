import { Fragment, useEffect, useId, useMemo, useRef, useState } from "react";
import { FORM_TOOLS, type FormTool, type ScriptsState, useForms } from "../../lib/forms";
import {
  type Field, FIELD_ICON, KIND_LABEL, NudgeBatch, SHORT, anchorOf, detectSummary, fieldLabel, isFillable, layoutReason, logCount, missingRequired,
  nudgeDelta, onPage, progress, renameCheck, rows, samePick, summary,
} from "../../lib/formsLogic";
import { SIGNED_NOTE } from "../../lib/a11yLogic";
import { parseRange } from "../../lib/pageRange";
import { FORMS_CERTIFIED, useExchange } from "../../lib/dataExchange";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import { MenuButton } from "../Menu";
import { FieldProps } from "./FieldProps";
import { LAYOUT_GROUPS, useCertified, useSigned } from "./layoutTools";
import "./forms.css";

const fm = () => useForms.getState();
const NO_FIELDS: Field[] = [];
const NO_PICKS: { page: number; xref: number }[] = [];

/** The toolbar's Forms button. */
export function FormsToggle() {
  const open = useForms((s) => s.open);
  return (
    <button className={open ? "icon-btn on" : "icon-btn"} title="Forms (Ctrl+Alt+F)" aria-label="Forms" aria-pressed={open} onClick={() => fm().setOpen(!open)}>
      <Icon name="fill" />
    </button>
  );
}

const TOOL_LABEL: Record<FormTool, string> = {
  select: "Select and edit fields",
  text: "Text field",
  checkbox: "Check box",
  radio: "Radio button",
  combo: "Drop-down list",
  list: "List box",
  button: "Button",
  signature: "Signature field",
};
const TOOL_ICON: Record<FormTool, "cursor" | "textField" | "checkSquare" | "radio" | "dropdown" | "listBox" | "pushButton" | "signature"> = {
  select: "cursor",
  ...FIELD_ICON,
};

/** The floating field tool strip over the page area while preparing a form. */
export function FormTools() {
  const authoring = useForms((s) => s.authoring);
  const tool = useForms((s) => s.tool);
  if (!authoring) return null;
  return (
    <div className="fm-strip" role="toolbar" aria-label="Field tools" onMouseDown={(e) => e.preventDefault()}>
      {FORM_TOOLS.map((t) => (
        <button
          key={t}
          className={tool === t ? "icon-btn on" : "icon-btn"}
          title={TOOL_LABEL[t]}
          aria-label={TOOL_LABEL[t]}
          aria-pressed={tool === t}
          onClick={() => fm().setTool(t)}
        >
          <Icon name={TOOL_ICON[t]} />
        </button>
      ))}
    </div>
  );
}

const NO_RESTART = "The script engine stopped answering. Scripts stay off until the app is restarted.";

/** What the banner says about the scripts of the file. */
function scriptsLine(s: ScriptsState): string {
  if (!s.enabled) return "Scripts are off for this file.";
  return s.usable ? "This form runs scripts." : "Scripts stopped in this file.";
}

/** Over the page: "This form runs scripts", with the per-file switch (Acrobat runs them; so does Inkwell, unless told not to). */
export function FormsBanner({ doc }: { doc: DocState }) {
  const scripts = useForms((s) => s.docs[doc.id]?.scripts);
  const gone = useForms((s) => s.docs[doc.id]?.bannerGone);
  if (!scripts?.hasScripts || (scripts.enabled && scripts.usable && gone)) return null;
  return (
    <div className="fm-banner" role="status">
      <Icon name="code" size={14} />
      <span>{scriptsLine(scripts)}</span>
      <label className="fm-check">
        <input type="checkbox" checked={scripts.enabled} disabled={!scripts.enabled && !scripts.canEnable} title={scripts.canEnable ? undefined : NO_RESTART} onChange={(e) => void fm().runScripts(doc.id, e.target.checked)} />
        <span>Run scripts</span>
      </label>
      {scripts.enabled && scripts.usable && (
        <button className="ax-icon" title="Dismiss" aria-label="Dismiss" onClick={() => fm().dismissBanner(doc.id)}>
          <Icon name="close" size={12} />
        </button>
      )}
    </div>
  );
}

/** The Scripts note of the rail: the switch and the log (warnings and errors, with the field), closed until asked for. */
function ScriptsNote({ doc, scripts }: { doc: number; scripts: ScriptsState }) {
  const n = logCount(scripts.log);
  return (
    <details className="fm-scripts">
      <summary>
        <span>Scripts</span>
        {n > 0 && <span className="fm-badge" aria-label={`${n} warnings or errors`}>{n}</span>}
        <span className="grow" />
        <span className="dim small">{!scripts.enabled ? "off" : scripts.usable ? "running" : "stopped"}</span>
      </summary>
      <label className="fm-check">
        <input type="checkbox" checked={scripts.enabled} disabled={!scripts.enabled && !scripts.canEnable} title={scripts.canEnable ? undefined : NO_RESTART} onChange={(e) => void fm().runScripts(doc, e.target.checked)} />
        <span>Run scripts in this file</span>
      </label>
      {scripts.log.length === 0 ? (
        <p className="dim small">Nothing to report.</p>
      ) : (
        <ul className="fm-log">
          {scripts.log.map((l, i) => (
            <li key={i} className={l.level}>
              {l.field && <b>{l.field}</b>}
              <span>{l.message}</span>
            </li>
          ))}
        </ul>
      )}
    </details>
  );
}

export function FormsPanel({ doc }: { doc: DocState }) {
  const open = useForms((s) => s.open);
  const authoring = useForms((s) => s.authoring);
  const highlight = useForms((s) => s.highlight);
  const d = useForms((s) => s.docs[doc.id]);
  const id = doc.id;
  const fields = d?.fields ?? NO_FIELDS;

  // The page layers fill from this list, so it is read whenever the document changes (rail open or not).
  useEffect(() => {
    void fm().refresh(id);
  }, [id, doc.rev]);
  const blocked = useExchange((s) => s.gates[id]?.forms === false);
  useEffect(() => {
    if (open) void useExchange.getState().readGates(id);
  }, [open, id]);

  // Arrow keys nudge the picked fields (1 pt, Shift 10 pt): the presses are collected and sent as one step at key up.
  // F2 renames the anchor. Not while typing in a control or with a menu open.
  useEffect(() => {
    if (!open || !authoring) return;
    const batch = new NudgeBatch();
    const flush = () => {
      const by = batch.take();
      if (by) void fm().nudge(id, by[0], by[1]);
    };
    const skip = (e: KeyboardEvent) =>
      e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey ||
      !!(e.target as HTMLElement | null)?.closest?.("input, textarea, select, [contenteditable]") ||
      !!document.querySelector(".ink-menu");
    const down = (e: KeyboardEvent) => {
      if (skip(e)) return;
      const cur = fm().get(id).picked;
      if (cur.length === 0) return;
      if (e.key === "F2") {
        if (cur.length === 1) {
          e.preventDefault();
          fm().setRenaming(id, cur[0]);
        }
      } else if (batch.push(e.key, e.shiftKey)) {
        e.preventDefault();
      }
    };
    const up = (e: KeyboardEvent) => nudgeDelta(e.key, false) && flush();
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", flush);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", flush);
    };
  }, [open, authoring, id]);

  const list = useMemo(() => rows(fields), [fields]);
  const [done, total] = useMemo(() => progress(fields), [fields]);
  const missing = useMemo(() => missingRequired(fields), [fields]);
  const picked = d?.picked ?? NO_PICKS;
  const certified = useCertified(id);
  const sel = d?.selected ? fields.find((f) => f.page === d.selected!.page && f.xref === d.selected!.xref) ?? null : null;
  const groups = useMemo(() => {
    const m = new Map<number, Field[]>();
    for (const f of list) m.set(f.page, [...(m.get(f.page) ?? []), f]);
    return [...m.entries()].sort((a, b) => a[0] - b[0]);
  }, [list]);

  if (!open) return null;
  const go = (f: Field) => {
    fm().select(id, { page: f.page, xref: f.xref });
    useStore.getState().goTo(id, f.page, Math.max(0, f.rect[1] - 80));
  };

  return (
    <aside className="ax-dock fm-dock" style={{ width: "min(360px, 45vw)" }} aria-label="Forms">
      <section className="ax-panel docked fm-panel">
        <div className="ax-head">
          <span className="ax-title">Forms</span>
          <span className="dim cm-count">{list.length}</span>
          <span className="grow" />
          <MenuButton
            className="ax-icon"
            label="Form data files"
            items={[
              { label: "Export form data…", onSelect: () => void useExchange.getState().exportForm(id) },
              {
                label: "Import form data…",
                disabled: blocked,
                title: blocked ? FORMS_CERTIFIED : undefined,
                onSelect: () => void useExchange.getState().importForm(id),
              },
            ]}
          >
            <Icon name="more" size={16} />
          </MenuButton>
          <button className="ax-icon" title="Close" aria-label="Close Forms" onClick={() => fm().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="fm-seg" role="tablist" aria-label="Forms mode">
          <button role="tab" aria-selected={!authoring} className={!authoring ? "on" : ""} onClick={() => fm().setAuthoring(false)}>
            Fill
          </button>
          <button role="tab" aria-selected={authoring} className={authoring ? "on" : ""} onClick={() => fm().setAuthoring(true)}>
            Prepare form
          </button>
        </div>
        <div className="fm-body">
          {d?.xfa && (
            <p className="fm-note warn">
              This form also carries XFA data, which Inkwell does not run. The fields below are its standard form fields.
            </p>
          )}

          {d?.scripts?.hasScripts && <ScriptsNote doc={id} scripts={d.scripts} />}

          {!authoring && (
            <>
              {total > 0 ? (
                <div className="fm-progress">
                  <span>
                    <b>
                      {done} of {total}
                    </b>{" "}
                    <span className="dim">fields filled</span>
                  </span>
                  <div className="fm-bar" aria-hidden>
                    <i style={{ width: `${(done / total) * 100}%` }} />
                  </div>
                  {missing.length > 0 && (
                    <span className="dim small">
                      Required and still empty:{" "}
                      {missing.slice(0, 3).map((f, i) => (
                        <span key={f.xref}>
                          {i > 0 && ", "}
                          <button className="cm-link" onClick={() => go(f)}>
                            {SHORT(fieldLabel(f), 22)}
                          </button>
                        </span>
                      ))}
                      {missing.length > 3 && ` and ${missing.length - 3} more`}
                    </span>
                  )}
                </div>
              ) : (
                <p className="dim cm-empty">This page has no form fields. Choose Prepare form to add some or find them automatically.</p>
              )}
              <div className="fm-actions">
                <button className="fm-btn" disabled={total === 0} onClick={() => void fm().clear(id)}>
                  Clear form
                </button>
                <label className="fm-check">
                  <input type="checkbox" checked={highlight} onChange={(e) => fm().setHighlight(e.target.checked)} />
                  <span>Highlight fields</span>
                </label>
              </div>
            </>
          )}

          {authoring && (
            <>
              <Detect doc={id} page={doc.currentPage} />
              {sel ? (
                <FieldProps key={`${sel.page}:${sel.xref}`} doc={id} f={sel} all={fields} />
              ) : (
                <p className="fm-note">Pick a field tool on the left and drag on the page, or select a field to change it.</p>
              )}
              <LayoutTools doc={id} pageCount={doc.pageCount} picked={picked} fields={fields} certified={certified} />
              <TabOrder doc={id} page={doc.currentPage} fields={fields} selected={sel} />
            </>
          )}

          {d?.error && (
            <p className="fm-error" role="alert">
              {d.error}
            </p>
          )}

          {groups.map(([page, items]) => (
            <section key={page}>
              <h3 className="fm-page">Page {page + 1}</h3>
              {items.map((f) => (
                <Row key={f.xref} doc={id} f={f} all={fields} follow={!authoring} active={picked.some((p) => samePick(p, f))} onOpen={() => go(f)} />
              ))}
            </section>
          ))}
        </div>
      </section>
    </aside>
  );
}

function Row({ doc, f, all, active, follow, onOpen }: { doc: number; f: Field; all: Field[]; active: boolean; follow: boolean; onOpen: () => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  const renaming = useForms((s) => s.renaming?.doc === doc && samePick(s.renaming, f));
  useEffect(() => {
    // While preparing, the properties above the list stay in view; only the fill list follows the page.
    if (active && follow) ref.current?.scrollIntoView({ block: "nearest" });
  }, [active, follow]);
  const authoring = useForms((s) => s.authoring);
  if (renaming) return <RenameRow doc={doc} f={f} all={all} />;
  const val = summary(f, all);
  return (
    <button
      ref={ref}
      className={`fm-row${active ? " on" : ""}`}
      onClick={(e) => (authoring && (e.metaKey || e.shiftKey || e.ctrlKey) ? fm().toggle(doc, { page: f.page, xref: f.xref }) : onOpen())}
      onDoubleClick={() => authoring && fm().setRenaming(doc, { page: f.page, xref: f.xref })}
      title={authoring ? `${KIND_LABEL[f.kind]}. Double-click or F2 to rename.` : KIND_LABEL[f.kind]}
    >
      <Icon name={FIELD_ICON[f.kind]} size={15} />
      <span className="name">
        {SHORT(fieldLabel(f), 30)}
        {f.required && isFillable(f) && <span className="req"> *</span>}
      </span>
      <span className="val">{val}</span>
    </button>
  );
}

function Detect({ doc, page }: { doc: number; page: number }) {
  const d = useForms((s) => s.docs[doc]);
  const found = d?.detected && d.detectPage === page ? d.detected : null;
  return (
    <div className="fm-props">
      <h3>
        <Icon name="wand" size={15} />
        <span>Find fields on this page</span>
      </h3>
      <p className="dim small" style={{ margin: 0 }}>
        Looks for underlines, empty boxes and check boxes next to labels, then offers them as fields.
      </p>
      {found && <p className="fm-note">{detectSummary(found)}</p>}
      <div className="fm-actions">
        <button className="fm-btn" onClick={() => void fm().detect(doc, page)}>
          {found ? "Look again" : "Detect fields"}
        </button>
        {found && found.length > 0 && (
          <button className="fm-btn primary" onClick={() => void fm().addDetected(doc)}>
            Add them
          </button>
        )}
      </div>
    </div>
  );
}

function TabOrder({ doc, page, fields, selected }: { doc: number; page: number; fields: Field[]; selected: Field | null }) {
  const order = useMemo(() => onPage(fields, page), [fields, page]);
  if (order.length < 2) return null;
  const swap = (i: number, to: number) => {
    const next = order.map((f) => f.xref);
    [next[i], next[to]] = [next[to], next[i]];
    void fm().tabOrder(doc, page, next);
  };
  return (
    <section className="fm-props" aria-label="Tab order">
      <h3>
        <span>Tab order, page {page + 1}</span>
      </h3>
      <div className="fm-actions">
        <button className="fm-btn" onClick={() => void fm().arrange(doc, page, "row")}>
          By row
        </button>
        <button className="fm-btn" onClick={() => void fm().arrange(doc, page, "column")}>
          By column
        </button>
      </div>
      <div className="fm-order">
        {order.map((f, i) => (
          <div key={f.xref} className={`fm-row${selected?.xref === f.xref ? " on" : ""}`}>
            <Icon name={FIELD_ICON[f.kind]} size={15} />
            <button className="name" style={{ textAlign: "left" }} onClick={() => fm().select(doc, { page, xref: f.xref })}>
              {i + 1}. {SHORT(f.kind === "radio" ? `${f.name} (${f.export})` : f.name, 26)}
            </button>
            <button className="fm-mini" disabled={i === 0} title="Earlier" aria-label={`Move ${f.name} earlier`} onClick={() => swap(i, i - 1)}>
              <Icon name="up" size={13} />
            </button>
            <button className="fm-mini" disabled={i === order.length - 1} title="Later" aria-label={`Move ${f.name} later`} onClick={() => swap(i, i + 1)}>
              <Icon name="down" size={13} />
            </button>
          </div>
        ))}
      </div>
    </section>
  );
}

/** The name of a field edited in place: its full dotted name; Enter saves, Esc cancels, a clash with a field of the same kind asks about merging. */
function RenameRow({ doc, f, all }: { doc: number; f: Field; all: Field[] }) {
  const [text, setText] = useState(f.name);
  const [merge, setMerge] = useState<Field | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  const done = () => fm().setRenaming(doc, null);
  const save = async (mergeInto: boolean) => {
    const c = renameCheck(all, f, text);
    if (c.kind === "same") return done();
    if (c.kind === "error") return setProblem(c.message);
    if (c.kind === "merge" && !mergeInto) return setMerge(c.target);
    const err = await fm().rename(doc, { page: f.page, xref: f.xref }, text.trim(), c.kind === "merge");
    if (err) setProblem(err);
    else done();
  };
  return (
    <div className="fm-rename-row">
      <input
        ref={input}
        className="fm-rename"
        value={text}
        aria-label={`Name of ${f.name}`}
        spellCheck={false}
        onChange={(e) => {
          setText(e.target.value);
          setMerge(null);
          setProblem(null);
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") void save(false);
          else if (e.key === "Escape") done();
        }}
        onBlur={() => !merge && !problem && done()}
      />
      {problem && <span className="fm-error small" role="alert">{problem}</span>}
      {merge && (
        <span className="small">
          A {KIND_LABEL[merge.kind].toLowerCase()} named {merge.name} exists. Merge into it? Both then share one value.{" "}
          <button className="fm-btn primary" onMouseDown={(e) => e.preventDefault()} onClick={() => void save(true)}>
            Merge
          </button>{" "}
          <button className="fm-btn" onMouseDown={(e) => e.preventDefault()} onClick={done}>
            Cancel
          </button>
        </span>
      )}
    </div>
  );
}

/** Align, centre, distribute, same size, duplicate and copies for the picked fields; the last one picked is the anchor. */
function LayoutTools({ doc, pageCount, picked, fields, certified }: {
  doc: number; pageCount: number; picked: { page: number; xref: number }[]; fields: Field[]; certified: boolean;
}) {
  const dialog = useForms((s) => (s.dialog?.doc === doc ? s.dialog.kind : null));
  const uid = useId();
  const signed = useSigned(doc);
  const [note, setNote] = useState<string | null>(null);
  const anchor = anchorOf(picked);
  const anchorField = anchor ? fields.find((f) => samePick(f, anchor)) : null;
  // A dialog belongs to the field it was opened for.
  useEffect(() => fm().setDialog(doc, null), [doc, anchor?.page, anchor?.xref]);
  return (
    <section className="fm-props" aria-label="Layout">
      <h3>
        <span>Layout</span>
        {picked.length > 1 && anchorField && <span className="dim small">{picked.length} fields, anchor {SHORT(anchorField.name, 20)}</span>}
      </h3>
      <div className="fm-tools" role="toolbar" aria-label="Layout tools" onMouseDown={(e) => e.preventDefault()}>
        {LAYOUT_GROUPS.map((g, i) => (
          <Fragment key={i}>
            {i > 0 && <span className="sep" />}
            {g.map((a) => {
              const why = layoutReason(a.op, picked, fields, certified);
              return (
                <button
                  key={a.id}
                  className="icon-btn"
                  aria-disabled={why ? true : undefined}
                  aria-describedby={why ? `${uid}-${a.id}` : undefined}
                  title={why ? `${a.label}. ${why}` : a.label}
                  aria-label={a.label}
                  onClick={() => {
                    if (why) return;
                    setNote(null);
                    a.run(doc);
                  }}
                >
                  <Icon name={a.icon} size={16} />
                  {why && (
                    <span id={`${uid}-${a.id}`} className="sr-only" aria-hidden="true">
                      {why}
                    </span>
                  )}
                </button>
              );
            })}
          </Fragment>
        ))}
      </div>
      {certified ? (
        <p className="fm-note warn">This document is certified, so its form fields cannot be changed.</p>
      ) : (
        <>
          {signed && <p className="fm-note">{SIGNED_NOTE}</p>}
          {picked.length < 2 && <p className="dim small" style={{ margin: 0 }}>Ctrl- or Shift-click more fields, or drag a box around them. The last one picked is the anchor. Arrow keys nudge (Shift: 10 pt).</p>}
        </>
      )}
      {dialog === "duplicate" && <DuplicateDialog doc={doc} pageCount={pageCount} onDone={(m) => setNote(m)} />}
      {dialog === "copies" && <CopiesDialog doc={doc} onDone={(m) => setNote(m)} />}
      {note && <p className="fm-note" role="status">{note}</p>}
    </section>
  );
}

function DuplicateDialog({ doc, pageCount, onDone }: { doc: number; pageCount: number; onDone: (note: string) => void }) {
  const [text, setText] = useState("");
  const pages = parseRange(text, pageCount);
  const go = async () => {
    if (!pages || pages.length === 0) return;
    if (await fm().duplicate(doc, pages)) {
      fm().setDialog(doc, null);
      onDone(`Added to ${pages.length} page${pages.length === 1 ? "" : "s"}. It is still one field: filling one fills them all.`);
    }
  };
  return (
    <div className="fm-dialog" role="group" aria-label="Duplicate on other pages">
      <label className="fm-field">
        <span>Pages (1 to {pageCount})</span>
        <input value={text} placeholder="2, 4-6" autoFocus onChange={(e) => setText(e.target.value)} onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") void go();
          else if (e.key === "Escape") fm().setDialog(doc, null);
        }} />
      </label>
      <div className="fm-actions">
        <button className="fm-btn primary" disabled={!pages || pages.length === 0} onClick={() => void go()}>Duplicate</button>
        <button className="fm-btn" onClick={() => fm().setDialog(doc, null)}>Cancel</button>
      </div>
    </div>
  );
}

const MAX_COPIES = 500;

function CopiesDialog({ doc, onDone }: { doc: number; onDone: (note: string) => void }) {
  const [rows, setRows] = useState("2");
  const [cols, setCols] = useState("2");
  const [gapX, setGapX] = useState("12");
  const [gapY, setGapY] = useState("12");
  const r = Number(rows);
  const c = Number(cols);
  const valid = Number.isInteger(r) && Number.isInteger(c) && r >= 1 && c >= 1 && r * c >= 2 && r * c <= MAX_COPIES && gapX.trim() !== "" && gapY.trim() !== "" && Number.isFinite(Number(gapX)) && Number.isFinite(Number(gapY));
  const go = async () => {
    if (!valid) return;
    const note = await fm().copies(doc, r, c, Number(gapX), Number(gapY));
    if (note) {
      fm().setDialog(doc, null);
      onDone(note);
    }
  };
  const num = (label: string, v: string, set: (s: string) => void) => (
    <label className="fm-field">
      <span>{label}</span>
      <input inputMode="decimal" value={v} onChange={(e) => set(e.target.value)} onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") void go();
        else if (e.key === "Escape") fm().setDialog(doc, null);
      }} />
    </label>
  );
  return (
    <div className="fm-dialog" role="group" aria-label="Create multiple copies">
      <div className="fm-grid">
        {num("Rows", rows, setRows)}
        {num("Columns", cols, setCols)}
        {num("Space between, across (pt)", gapX, setGapX)}
        {num("Space between, down (pt)", gapY, setGapY)}
      </div>
      <p className="dim small" style={{ margin: 0 }}>
        The field becomes name.0 and the copies name.1, name.2 and so on, to its right and below it. Copies that would run off the page are left out. Up to {MAX_COPIES} in all.
      </p>
      <div className="fm-actions">
        <button className="fm-btn primary" disabled={!valid} onClick={() => void go()}>Create</button>
        <button className="fm-btn" onClick={() => fm().setDialog(doc, null)}>Cancel</button>
      </div>
    </div>
  );
}
