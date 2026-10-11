import { useState } from "react";
import type { Change, FormatProp, MetadataField } from "../../lib/history";
import { GLYPH, TONE_WORD, categoryOf, changePage, inlineRuns, textRuns, toneOf } from "../../lib/historyLogic";

/** Past this many characters of word diff the row clamps to two lines and offers "Show more". */
const LONG = 110;

function MetaFields({ fields }: { fields: MetadataField[] }) {
  return (
    <span className="hx-inline hx-fields">
      {fields.map((f) => (
        <span className="hx-field-row" key={`${f.source}:${f.field}`}>
          <span className="dim">{f.field}</span> {f.old !== null && <del>{f.old}</del>}
          {f.old !== null && f.new !== null && " "}
          {f.new !== null && <ins>{f.new}</ins>}
        </span>
      ))}
    </span>
  );
}

function FormatProps({ props }: { props: FormatProp[] }) {
  return (
    <span className="hx-inline hx-fields">
      {props.map((p) => (
        <span className="hx-field-row" key={p.name}>
          <span className="dim">{p.name}</span> <del>{p.old}</del> <ins>{p.new}</ins>
        </span>
      ))}
    </span>
  );
}

/** One change as a flat revisions-pane row: tone bar and glyph, one-line label, page, then the word diff inline. */
export function RevRow({ change, on, onClick }: { change: Change; on: boolean; onClick: () => void }) {
  const [open, setOpen] = useState(false);
  const [more, setMore] = useState(false);
  const fields = change.detail.type === "metadata" ? (change.detail as { fields: MetadataField[] }).fields : null;
  const format = change.detail.type === "format" ? (change.detail as { text: string; props: FormatProp[] }) : null;
  const props = format?.props ?? null;
  const tone = toneOf(change);
  const p = changePage(change);
  const runs = textRuns(change);
  const isText = change.kind === "textChanged" && runs.length > 0;
  const inline = isText ? inlineRuns(runs) : [];
  const long = inline.reduce((n, r) => n + r.text.length + 1, 0) > LONG;
  return (
    <div className={`hx-rev ${tone}${on ? " on" : ""}`} data-cat={categoryOf(change)}>
      <button
        className="hx-rev-main"
        aria-pressed={on}
        aria-expanded={fields ? open : undefined}
        onClick={() => {
          onClick();
          if (fields) setOpen((o) => !o);
        }}
      >
        <span className={`hx-glyph ${tone}`} title={TONE_WORD[tone]} aria-label={TONE_WORD[tone]}>
          {GLYPH[tone]}
        </span>
        <span className="hx-rev-label">{isText ? `${TONE_WORD[tone]} text` : format ? "Formatting changed" : change.label}</span>
        {p !== null && <span className="hx-rev-page">p. {p + 1}</span>}
      </button>
      {fields && open && <MetaFields fields={fields} />}
      {format && format.text && <span className="hx-inline dim">“{format.text}”</span>}
      {props && <FormatProps props={props} />}
      {isText && (
        <>
          <span className={`hx-inline${long && !more ? " clamp" : ""}`}>
            {inline.map((r, i) =>
              r.op === "delete" ? (
                <del key={i}>{r.text}</del>
              ) : r.op === "insert" ? (
                <ins key={i}>{r.text}</ins>
              ) : (
                <span key={i}>{r.text}</span>
              ),
            ).flatMap((e) => [e, " "])}
          </span>
          {long && (
            <button className="hx-showmore" onClick={() => setMore((m) => !m)}>
              {more ? "Show less" : "Show more"}
            </button>
          )}
        </>
      )}
    </div>
  );
}
