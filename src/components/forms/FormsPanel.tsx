import { useEffect, useMemo, useRef } from "react";
import { FORM_TOOLS, type FormTool, useForms } from "../../lib/forms";
import {
  type Field, FIELD_ICON, KIND_LABEL, SHORT, detectSummary, fieldLabel, isFillable, missingRequired, onPage, progress, rows, summary,
} from "../../lib/formsLogic";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import { FieldProps } from "./FieldProps";
import "./forms.css";

const fm = () => useForms.getState();
const NO_FIELDS: Field[] = [];

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

  const list = useMemo(() => rows(fields), [fields]);
  const [done, total] = useMemo(() => progress(fields), [fields]);
  const missing = useMemo(() => missingRequired(fields), [fields]);
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
                <Row key={f.xref} f={f} all={fields} follow={!authoring} active={d?.selected?.page === f.page && d.selected.xref === f.xref} onOpen={() => go(f)} />
              ))}
            </section>
          ))}
        </div>
      </section>
    </aside>
  );
}

function Row({ f, all, active, follow, onOpen }: { f: Field; all: Field[]; active: boolean; follow: boolean; onOpen: () => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    // While preparing, the properties above the list stay in view; only the fill list follows the page.
    if (active && follow) ref.current?.scrollIntoView({ block: "nearest" });
  }, [active, follow]);
  const val = summary(f, all);
  return (
    <button ref={ref} className={`fm-row${active ? " on" : ""}`} onClick={onOpen} title={KIND_LABEL[f.kind]}>
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
