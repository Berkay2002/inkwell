import { useEffect, useState } from "react";
import { scaleOf, type MeasureTool, useMeasure } from "../../lib/measure";
import { type Scale, type Snaps, type Unit, UNITS, inUnit, numText, parseScale, scaleLabel } from "../../lib/measureLogic";
import { type DocState, useStore } from "../../store";
import { Icon, type IconName } from "../Icons";
import "./measure.css";

const ms = () => useMeasure.getState();

const TOOLS: { tool: MeasureTool; icon: IconName; label: string }[] = [
  { tool: "select", icon: "cursor", label: "Select a measurement" },
  { tool: "distance", icon: "ruler", label: "Distance: click both ends" },
  { tool: "perimeter", icon: "polyline", label: "Perimeter: click each corner, double-click or Enter to finish" },
  { tool: "area", icon: "polygon", label: "Area: click each corner, double-click or Enter to finish" },
];

const SNAPS: { id: keyof Snaps; label: string; tip: string }[] = [
  { id: "endpoint", label: "Ends", tip: "Snap to the end points and corners of lines" },
  { id: "midpoint", label: "Mid", tip: "Snap to the middle of a line" },
  { id: "intersection", label: "Cross", tip: "Snap to where two lines cross" },
  { id: "line", label: "Path", tip: "Snap to the nearest point on a line" },
];

const num = (s: string) => (s.trim() === "" ? NaN : Number(s));

/** The floating bar over the page area: the three measurements, the scale, units, snapping and Done. */
export function MeasureBar({ doc }: { doc: DocState }) {
  const open = useMeasure((s) => s.open);
  const tool = useMeasure((s) => s.tool);
  const snaps = useMeasure((s) => s.snaps);
  const d = useMeasure((s) => s.docs[doc.id]);
  const locked = useMeasure((s) => s.locks[doc.id]) ?? null;
  // Off for the page scale alone on a certified file that allows annotations (/P 3): measuring still works.
  const scaleLocked = useMeasure((s) => s.scaleLocks[doc.id]) ?? null;
  const page = doc.currentPage;
  const ps = d?.scales[page];
  const scale = scaleOf(d, page);
  const [edit, setEdit] = useState(false);
  // The find bar (Ctrl+F) sits at the same top edge: the measure bar steps down under it while it is open.
  const finding = useStore((s) => s.searchOpen);

  useEffect(() => {
    if (open) {
      void ms().ensureLock(doc.id);
      void ms().ensureScale(doc.id, page);
    }
  }, [open, doc.id, page, doc.rev]);
  // A locked document is only looked at: the drawing tools give way to Select.
  useEffect(() => {
    if (locked && tool !== "select") ms().setTool("select");
  }, [locked, tool]);
  // A tool armed on another document must not draw a calibration line where the scale cannot be set.
  useEffect(() => {
    if (scaleLocked && tool === "calibrate") ms().setTool("distance");
  }, [scaleLocked, tool]);
  useEffect(() => setEdit(false), [tool, page, open]);

  if (!open) return null;
  const calib = d?.calib ?? null;
  return (
    <div className={finding ? "ms-bar-wrap below-find" : "ms-bar-wrap"} onMouseDown={(e) => e.target instanceof HTMLSelectElement || e.target instanceof HTMLInputElement || e.preventDefault()}>
      {calib ? (
        <CalibForm doc={doc.id} unit={scale.realUnit} lengthIn={Math.hypot(calib.b[0] - calib.a[0], calib.b[1] - calib.a[1]) / 72} />
      ) : (
        <div className="ms-bar" role="toolbar" aria-label="Measure tools">
          {TOOLS.map((t) => (
            <button
              key={t.tool}
              className={tool === t.tool ? "icon-btn on" : "icon-btn"}
              disabled={!!locked && t.tool !== "select"}
              title={locked && t.tool !== "select" ? locked : t.label}
              aria-label={t.label.split(":")[0]}
              aria-pressed={tool === t.tool}
              onClick={() => ms().setTool(t.tool)}
            >
              <Icon name={t.icon} />
            </button>
          ))}
          <span className="sep" />
          <button
            className={edit ? "ms-chip on" : "ms-chip"}
            disabled={!!scaleLocked}
            title={scaleLocked ?? (ps?.defined === false ? "This page has no scale yet: it measures in inches. Click to set one." : "The scale of this page. Click to change it.")}
            aria-expanded={edit}
            onClick={() => setEdit(!edit)}
          >
            {scaleLabel(scale)}
          </button>
          <select
            className="ms-units"
            value={scale.realUnit}
            disabled={!!scaleLocked}
            title={scaleLocked ?? "Show measurements in"}
            aria-label="Units"
            onChange={(e) => void ms().setPageScale(doc.id, page, inUnit(scale, e.target.value as Unit))}
          >
            {UNITS.map((u) => (
              <option key={u.id} value={u.id}>
                {u.label}
              </option>
            ))}
          </select>
          <span className="sep" />
          {SNAPS.map((s) => (
            <button key={s.id} className={snaps[s.id] ? "ms-chip on" : "ms-chip"} title={s.tip} aria-pressed={snaps[s.id]} onClick={() => ms().toggleSnap(s.id)}>
              {s.label}
            </button>
          ))}
          <span className="sep" />
          <button className="ms-done" title="Leave measuring" onClick={() => ms().setOpen(false)}>
            Done
          </button>
        </div>
      )}
      {scaleLocked && <p className="ms-error" role="status">{scaleLocked}</p>}
      {edit && !calib && !scaleLocked && <ScaleEditor doc={doc.id} page={page} scale={scale} onClose={() => setEdit(false)} />}
      {d?.error && (
        <p className="ms-error" role="alert">
          {d.error}
        </p>
      )}
    </div>
  );
}

function ScaleEditor({ doc, page, scale, onClose }: { doc: number; page: number; scale: Scale; onClose: () => void }) {
  const [text, setText] = useState(scaleLabel(scale));
  const parsed = parseScale(text);
  const apply = async () => {
    if (parsed && (await ms().setPageScale(doc, page, parsed))) onClose();
  };
  return (
    <div className="ms-pop" role="group" aria-label="Scale of this page">
      <label className="ms-field">
        <span className="dim">Scale of page {page + 1}</span>
        <input
          autoFocus
          className="cm-input"
          value={text}
          aria-invalid={!parsed}
          placeholder="1 in = 10 ft"
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void apply();
            else if (e.key === "Escape") onClose();
          }}
        />
      </label>
      <p className="dim ms-hint">{parsed ? "Anything measured on this page from now on uses it." : "Write it like 1 in = 10 ft or 1 cm = 2 m."}</p>
      <div className="ms-row">
        <button
          className="cm-link"
          title="Draw over a dimension printed on the drawing and type its real length"
          onClick={() => {
            onClose();
            ms().setTool("calibrate");
          }}
        >
          <Icon name="ruler" size={13} /> Calibrate from a known length
        </button>
        <span className="grow" />
        <button className="cm-btn primary" disabled={!parsed} onClick={() => void apply()}>
          Set scale
        </button>
      </div>
    </div>
  );
}

/** After the calibration line is drawn: its real length sets the page's scale. */
function CalibForm({ doc, unit, lengthIn }: { doc: number; unit: Unit; lengthIn: number }) {
  const [value, setValue] = useState("");
  const [u, setU] = useState<Unit>(unit);
  const n = num(value);
  const ok = Number.isFinite(n) && n > 0;
  const go = async () => {
    if (ok && (await ms().finishCalib(doc, n, u))) ms().setTool("distance");
  };
  const cancel = () => {
    ms().cancelCalib(doc);
    ms().setTool("distance");
  };
  return (
    <div className="ms-bar ms-calib-bar" role="group" aria-label="Calibrate the scale">
      <Icon name="ruler" size={16} />
      <span>
        The line is <b>{numText(lengthIn)} in</b> on the page. Its real length:
      </span>
      <input
        autoFocus
        className="ms-num"
        inputMode="decimal"
        aria-label="Real length"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void go();
          else if (e.key === "Escape") cancel();
        }}
      />
      <select className="ms-units" value={u} aria-label="Unit" onChange={(e) => setU(e.target.value as Unit)}>
        {UNITS.map((x) => (
          <option key={x.id} value={x.id}>
            {x.label}
          </option>
        ))}
      </select>
      <button className="cm-btn primary" disabled={!ok} onClick={() => void go()}>
        Set scale
      </button>
      <button className="cm-link" onClick={cancel}>
        Cancel
      </button>
    </div>
  );
}
