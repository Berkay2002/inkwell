import { useEffect, useState } from "react";
import { type DocState } from "../../store";
import { usePrepress } from "../../lib/prepressStore";
import { type Plate, coverageText, isSolo, plateList, readout, showInk } from "../../lib/prepressLogic";
import { Icon } from "../Icons";
import { PlateThumb } from "./PlateThumb";
import "../a11y/a11y.css";
import "./prepress.css";

const st = () => usePrepress.getState();

/** Output Preview: the page's plates, the ink under the pointer, the total-ink limit and a strip of the plates. */
export function PrepressPanel({ doc }: { doc: DocState }) {
  const open = usePrepress((s) => s.open);
  const d = usePrepress((s) => s.docs[doc.id]);
  const id = doc.id;

  // Read again whenever the page or the document generation changes.
  useEffect(() => {
    if (open) void st().refresh(id);
  }, [open, id, doc.currentPage, doc.rev]);

  if (!open) return null;
  const s = d?.settings;
  if (!d || !s) return null;
  const plates = plateList(d.seps?.data ?? null, s);
  const seps = d.seps?.data ?? null;
  const report = d.report && d.report.page === doc.currentPage && d.report.rev === doc.rev ? d.report.data : null;
  const at = d.probe && d.probe.page === doc.currentPage ? d.probe.at : null;
  const rows = readout(at, plates);
  const allOn = plates.every((p) => p.on);
  const size = doc.sizes[doc.currentPage];
  return (
    <aside className="ax-dock a11y-dock" style={{ width: "min(360px, 45vw)" }} aria-label="Output Preview">
      <section className="ax-panel docked a11y-panel pp-panel">
        <div className="ax-head">
          <span className="ax-title">Output Preview</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Output Preview" onClick={() => st().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="a11y-body">
          <div className="ax-seg pp-seg" role="radiogroup" aria-label="Preview">
            <button role="radio" aria-checked={s.mode === "plates"} className={s.mode === "plates" ? "on" : ""} onClick={() => st().setMode(id, "plates")}>
              Plates
            </button>
            <button role="radio" aria-checked={s.mode === "coverage"} className={s.mode === "coverage" ? "on" : ""} onClick={() => st().setMode(id, "coverage")}>
              Ink coverage
            </button>
          </div>
          <label className="pp-check">
            <input type="checkbox" checked={s.overprint} onChange={(e) => st().setOverprint(id, e.target.checked)} />
            <span>Simulate overprint</span>
          </label>

          <h3 className="pp-h">Plates{doc.pageCount > 1 ? ` on page ${doc.currentPage + 1}` : ""}</h3>
          <ul className="pp-plates">
            {rows.map(({ plate, value }) => (
              <PlateRow key={plate.key} plate={plate} value={at ? value : null} solo={isSolo(s, plate.key, seps)} doc={id} />
            ))}
          </ul>
          {d.seps && d.seps.data.folded > 0 && (
            <p className="pp-note dim">
              {d.seps.data.folded} more spot {d.seps.data.folded === 1 ? "colour is" : "colours are"} painted into CMYK and cannot be switched.
            </p>
          )}
          {!s.overprint && plates.some((p) => !p.process) && <p className="pp-note dim">Spot colours are mixed into CMYK. Simulate overprint to switch them.</p>}
          <div className="pp-total" aria-live="polite">
            <span>Total ink</span>
            <b className={at && at.total > s.tacLimit ? "bad" : ""}>{at ? showInk(at.total) : "–"}</b>
            {!at && <span className="dim pp-hint">Move the pointer over the page.</span>}
          </div>
          <div className="pp-actions">
            <button className="a11y-quiet" disabled={allOn} onClick={() => st().showAll(id)}>
              Show all plates
            </button>
          </div>

          <h3 className="pp-h">Ink limit</h3>
          <Limit doc={id} value={s.tacLimit} />
          <p className={report?.overLimit ? "pp-note bad" : "pp-note"} aria-live="polite">
            {d.reporting && !report
              ? "Measuring the ink on this page…"
              : report
                ? coverageText(report.maxTac, s.tacLimit, report.regions.length, report.regionsTruncated)
                : ""}
          </p>
          <label className="pp-check">
            <input type="checkbox" checked={d.showAreas} onChange={(e) => st().setShowAreas(id, e.target.checked)} />
            <span>Mark the areas above the limit</span>
          </label>
          {d.error && <p className="pp-note bad">{d.error}</p>}

          {size && plates.length > 1 && (
            <>
              <h3 className="pp-h">Plates of this page</h3>
              <div className="pp-strip">
                {plates.map((p) => (
                  <PlateThumb
                    key={p.key}
                    doc={id}
                    page={doc.currentPage}
                    rev={doc.rev}
                    size={size}
                    plate={p}
                    seps={seps}
                    overprint={s.overprint}
                    solo={isSolo(s, p.key, seps)}
                    onClick={() => st().solo(id, p.key)}
                  />
                ))}
              </div>
            </>
          )}
        </div>
        <footer className="a11y-foot pp-foot">
          <button className="btn ghost" onClick={() => st().openConvert(id)}>
            Convert Colors…
          </button>
          <button className="btn ghost" onClick={() => st().openMarks(id)}>
            Printer's Marks…
          </button>
          <button className="btn ghost" onClick={() => st().openBoxes(id)}>
            Page Boxes…
          </button>
        </footer>
      </section>
    </aside>
  );
}

function PlateRow({ plate, value, solo, doc }: { plate: Plate; value: number | null; solo: boolean; doc: number }) {
  return (
    <li className={plate.on ? "pp-plate" : "pp-plate off"}>
      <label className="pp-plate-main" title={plate.locked ? "Simulate overprint to switch spot colours" : undefined}>
        <input type="checkbox" checked={plate.on} disabled={plate.locked} onChange={() => st().toggle(doc, plate.key)} />
        <i className="pp-swatch" style={{ background: `rgb(${plate.rgb.join(",")})` }} />
        <span className="pp-plate-name">{plate.name}</span>
      </label>
      <span className="pp-plate-ink">{value === null ? "" : showInk(value)}</span>
      <button className={solo ? "a11y-quiet on" : "a11y-quiet"} disabled={plate.locked} onClick={() => st().solo(doc, plate.key)}>
        {solo ? "All" : "Only"}
      </button>
    </li>
  );
}

function Limit({ doc, value }: { doc: number; value: number }) {
  const [text, setText] = useState(String(value));
  const [bad, setBad] = useState(false);
  useEffect(() => {
    setText(String(value));
    setBad(false);
  }, [value]);
  const commit = () => {
    const ok = st().setLimit(doc, text);
    setBad(!ok);
    if (ok) setText(String(usePrepress.getState().docs[doc]?.settings.tacLimit ?? value));
  };
  return (
    <div className="pp-limit">
      <input
        className={bad ? "tl-field bad" : "tl-field"}
        aria-label="Total ink limit in per cent"
        inputMode="decimal"
        spellCheck={false}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setBad(false);
        }}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && commit()}
      />
      <span className="dim">% of ink at most</span>
      <button className="a11y-quiet" disabled={value === 300 && text === "300"} onClick={() => st().setLimit(doc, "300")}>
        300%
      </button>
    </div>
  );
}
