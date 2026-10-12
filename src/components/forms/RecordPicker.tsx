import { useState } from "react";
import { useExchange } from "../../lib/dataExchange";

/**
 * Which record of a CSV or tab text file to import: the file has a row of values per record, and a form takes one.
 * Each row is its first cell that has text, numbered as a spreadsheet numbers it (the header is row 1).
 */
export function RecordPicker() {
  const picker = useExchange((s) => s.picker);
  if (!picker) return null;
  return <Picker name={picker.name} labels={picker.labels} />;
}

/** The text a row shows: its place in the file, then its label. */
export const rowText = (i: number, label: string) => `${i + 2}. ${label}`;

export function Picker({ name, labels }: { name: string; labels: string[] }) {
  const [at, setAt] = useState(0);
  const choose = useExchange.getState().chooseRecord;
  const title = "Choose a record to import";
  return (
    <div className="scrim" role="presentation">
      <div
        className="dialog fm-prompt fm-records"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            choose(null);
          }
        }}
      >
        <h2>{title}</h2>
        <p className="dim small">
          {name} has {labels.length} records. The form takes one of them.
        </p>
        <ul className="fm-record-list" role="listbox" aria-label="Records">
          {labels.map((label, i) => (
            <li key={i} role="option" aria-selected={i === at}>
              <button
                autoFocus={i === 0}
                className={i === at ? "on" : ""}
                onClick={() => setAt(i)}
                onDoubleClick={() => choose(i)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    choose(i);
                  }
                }}
              >
                {rowText(i, label)}
              </button>
            </li>
          ))}
        </ul>
        <div className="dialog-actions">
          <button className="btn" onClick={() => choose(null)}>
            Cancel
          </button>
          <button className="btn primary" onClick={() => choose(at)}>
            Import record
          </button>
        </div>
      </div>
    </div>
  );
}
