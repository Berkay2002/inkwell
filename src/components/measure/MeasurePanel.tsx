import { useEffect, useMemo, useState } from "react";
import { type LastMeasure, scaleOf, useMeasure } from "../../lib/measure";
import { type Measure, KIND_NAME, UNITS, type Unit, inUnit, parseScale, scaleLabel, totals } from "../../lib/measureLogic";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import "./measure.css";

const ms = () => useMeasure.getState();
const NO_ITEMS: Measure[] = [];

/** The Measure rail: the selected measurement, the last value, cumulative values and the CSV export. */
export function MeasurePanel({ doc }: { doc: DocState }) {
  const open = useMeasure((s) => s.open);
  const id = doc.id;
  const d = useMeasure((s) => s.docs[id]);
  const items = d?.items ?? NO_ITEMS;
  const sel = d?.selected ? (items.find((m) => m.page === d.selected!.page && m.xref === d.selected!.xref) ?? null) : null;
  const sums = useMemo(() => totals(items), [items]);

  useEffect(() => {
    if (open) void ms().refresh(id);
  }, [open, id, doc.rev]);

  if (!open) return null;
  const go = (m: Measure) => {
    ms().select(id, { page: m.page, xref: m.xref });
    ms().setTool("select");
    const top = Math.min(...m.points.map((p) => p[1]));
    useStore.getState().goTo(id, m.page, Math.max(0, top - 80));
  };
  const pages = [...new Set(items.map((m) => m.page))].sort((a, b) => a - b);
  return (
    <aside className="ax-dock cm-dock" style={{ width: "min(340px, 45vw)" }} aria-label="Measure">
      <section className="ax-panel docked cm-panel">
        <div className="ax-head">
          <span className="ax-title">Measure</span>
          <span className="dim cm-count">{items.length}</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Measure" onClick={() => ms().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="cm-body ms-body">
          {sel ? <Selected doc={id} m={sel} /> : d?.last ? <Last last={d.last} /> : <p className="dim cm-empty">Pick Distance, Perimeter or Area and click on the page. Select shows a measurement here.</p>}

          {sums.length > 0 && (
            <section className="ms-sec">
              <h3 className="cm-page">All measurements</h3>
              {sums.map((t) => (
                <div key={`${t.kind}${t.unit}`} className="ms-total">
                  <span>
                    {KIND_NAME[t.kind]} <span className="dim">× {t.count}</span>
                  </span>
                  <b>{t.text}</b>
                </div>
              ))}
              <button className="cm-btn ms-export" title="Save the list as a spreadsheet file" onClick={() => void ms().exportCsv(id)}>
                Export to CSV
              </button>
              {d?.exported && <p className="dim ms-hint">Saved to {d.exported}</p>}
            </section>
          )}

          {pages.map((p) => (
            <section key={p} className="cm-group">
              <h3 className="cm-page">Page {p + 1}</h3>
              {items
                .filter((m) => m.page === p)
                .map((m) => (
                  <button key={m.xref} className={`ms-item${sel?.xref === m.xref ? " on" : ""}`} onClick={() => go(m)}>
                    <span>{KIND_NAME[m.kind]}</span>
                    <b>{m.quantity.text}</b>
                  </button>
                ))}
            </section>
          ))}
          {d?.error && (
            <p className="cm-error" role="alert">
              {d.error}
            </p>
          )}
        </div>
      </section>
    </aside>
  );
}

function Last({ last }: { last: LastMeasure }) {
  return (
    <section className="ms-sec">
      <h3 className="cm-page">Last measurement</h3>
      <div className="ms-big">{last.quantity.text}</div>
      <div className="dim">
        {KIND_NAME[last.kind]} · page {last.page + 1}
      </div>
    </section>
  );
}

/** One measurement: its value, scale and units; the scale can be changed (it is measured again) and it can be deleted. */
function Selected({ doc, m }: { doc: number; m: Measure }) {
  const live = useMeasure((s) => scaleOf(s.docs[doc], m.page));
  const own = m.scale ?? live;
  const [text, setText] = useState(scaleLabel(own));
  useEffect(() => setText(scaleLabel(own)), [m.xref, m.scale?.paper, m.scale?.real, m.scale?.paperUnit, m.scale?.realUnit]);
  const parsed = parseScale(text);
  const changed = parsed && scaleLabel(parsed) !== (m.scale ? scaleLabel(m.scale) : "");
  const ref = { page: m.page, xref: m.xref };
  return (
    <section className="ms-sec">
      <h3 className="cm-page">
        {KIND_NAME[m.kind]} · page {m.page + 1}
      </h3>
      <div className="ms-big">{m.quantity.text}</div>
      <dl className="ms-facts">
        <dt>Units</dt>
        <dd>{m.quantity.unit}</dd>
        <dt>Points</dt>
        <dd>{m.points.length}</dd>
        {m.author && (
          <>
            <dt>By</dt>
            <dd>{m.author}</dd>
          </>
        )}
      </dl>
      <label className="ms-field">
        <span className="dim">Scale of this measurement</span>
        <input
          className="cm-input"
          value={text}
          disabled={m.scale === null}
          aria-invalid={!parsed}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && parsed && changed && void ms().rescale(doc, ref, parsed)}
        />
      </label>
      <div className="ms-row">
        <select
          className="ms-units"
          aria-label="Show in"
          title="Show this measurement in another unit"
          value={m.scale?.realUnit ?? ""}
          disabled={m.scale === null}
          onChange={(e) => m.scale && void ms().rescale(doc, ref, inUnit(m.scale, e.target.value as Unit))}
        >
          {m.scale === null && <option value="">?</option>}
          {UNITS.map((u) => (
            <option key={u.id} value={u.id}>
              {u.label}
            </option>
          ))}
        </select>
        <button className="cm-btn primary" disabled={!changed} onClick={() => parsed && void ms().rescale(doc, ref, parsed)}>
          Apply scale
        </button>
        <span className="grow" />
        <button className="cm-link" title="Delete (Delete key)" aria-label="Delete measurement" onClick={() => void ms().remove(doc, ref)}>
          <Icon name="trash" size={14} />
        </button>
      </div>
    </section>
  );
}
