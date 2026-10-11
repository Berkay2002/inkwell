import { useEffect, useState } from "react";
import { formsApi, useForms } from "../../lib/forms";
import {
  type ButtonAction, type CalcOp, type Calculate, type Field, type FieldPatch, type Format, type Opt, type Validate,
  CALC_OPS, DATE_FORMATS, FIELD_ICON, FONT_SIZES, KIND_LABEL, NEGATIVES, SEPARATORS, SPECIAL_FORMATS, TIME_FORMATS,
  calcFields, calcSources, fromHex, moved, needsMaxLen, toHex,
} from "../../lib/formsLogic";
import { Icon } from "../Icons";

const fm = () => useForms.getState();

// ---- small controls --------------------------------------------------------------------------

function Text({ label, value, onCommit, placeholder, area }: { label: string; value: string; onCommit: (v: string) => void; placeholder?: string; area?: boolean }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  const done = () => v !== value && onCommit(v);
  const common = {
    className: "fm-in",
    value: v,
    placeholder,
    "aria-label": label,
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => setV(e.target.value),
    onBlur: done,
  };
  return (
    <label className="fm-field">
      <span>{label}</span>
      {area ? (
        <textarea {...common} rows={2} />
      ) : (
        <input
          {...common}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter") (e.target as HTMLElement).blur();
            else if (e.key === "Escape") setV(value);
          }}
        />
      )}
    </label>
  );
}

function Num({ label, value, onCommit, min = 0, max = 9999, placeholder }: { label: string; value: number | null; onCommit: (v: number | null) => void; min?: number; max?: number; placeholder?: string }) {
  const [v, setV] = useState(value === null ? "" : String(value));
  useEffect(() => setV(value === null ? "" : String(value)), [value]);
  const done = () => {
    const t = v.trim();
    const n = t === "" ? null : Math.min(max, Math.max(min, Number(t)));
    if (n !== null && !Number.isFinite(n)) return setV(value === null ? "" : String(value));
    if (n !== value) onCommit(n);
  };
  return (
    <label className="fm-field">
      <span>{label}</span>
      <input
        className="fm-in"
        inputMode="decimal"
        value={v}
        placeholder={placeholder}
        aria-label={label}
        onChange={(e) => setV(e.target.value)}
        onBlur={done}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") (e.target as HTMLElement).blur();
        }}
      />
    </label>
  );
}

function Check({ label, checked, onChange, disabled }: { label: string; checked: boolean; onChange: (v: boolean) => void; disabled?: boolean }) {
  return (
    <label className="fm-check">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
    </label>
  );
}

function Pick<T extends string | number>({ label, value, options, onChange }: { label: string; value: T; options: { value: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <label className="fm-field">
      <span>{label}</span>
      <select className="fm-in" value={value} aria-label={label} onChange={(e) => onChange((typeof value === "number" ? Number(e.target.value) : e.target.value) as T)}>
        {options.map((o) => (
          <option key={String(o.value)} value={o.value}>{o.label}</option>
        ))}
      </select>
    </label>
  );
}

function Colour({ label, value, onPick, onClear }: { label: string; value: [number, number, number] | null; onPick: (c: [number, number, number]) => void; onClear?: () => void }) {
  return (
    <div className="fm-field">
      <span>{label}</span>
      <div className="fm-color">
        <input type="color" value={toHex(value)} aria-label={label} onChange={(e) => onPick(fromHex(e.target.value))} />
        {onClear && (
          <button className="fm-btn" disabled={value === null} onClick={onClear}>
            None
          </button>
        )}
      </div>
    </div>
  );
}

// ---- the properties --------------------------------------------------------------------------

export function FieldProps({ doc, f, all }: { doc: number; f: Field; all: Field[] }) {
  const set = (p: FieldPatch) => void fm().edit(doc, f.page, f.xref, p);
  const text = f.kind === "text";
  const choice = f.kind === "combo" || f.kind === "list";
  const tick = f.kind === "checkbox" || f.kind === "radio";
  const typed = text || f.kind === "combo";
  const siblings = all.filter((x) => x.node === f.node).length;
  return (
    <section className="fm-props" aria-label="Field properties">
      <h3>
        <Icon name={FIELD_ICON[f.kind]} size={15} />
        <span>{KIND_LABEL[f.kind]}</span>
        <span className="grow" />
        <button className="fm-mini" title="Delete field" aria-label="Delete field" onClick={() => void fm().remove(doc, [{ page: f.page, xref: f.xref }])}>
          <Icon name="trash" size={14} />
        </button>
      </h3>

      {f.script && <p className="dim small">This field has a script Inkwell keeps but does not run.</p>}
      <Text label={f.kind === "radio" ? "Group name" : "Name"} value={f.name} onCommit={(name) => set({ name })} />
      <Text label="Tooltip" value={f.tooltip} onCommit={(tooltip) => set({ tooltip })} />
      {tick && <Text label={f.kind === "radio" ? "Choice value" : "Export value"} value={f.export} onCommit={(v) => set({ export: v })} />}
      {f.kind === "radio" && siblings > 1 && <p className="dim small">{siblings} buttons share this group.</p>}
      <Check label="Required" checked={f.required} onChange={(required) => set({ required })} />
      <Check label="Read-only" checked={f.readOnly} onChange={(readOnly) => set({ readOnly })} />
      <Check label="Hidden" checked={f.hidden} onChange={(hidden) => set({ hidden })} />

      {text && <Text label="Default value" value={f.defaultValue} onCommit={(defaultValue) => set({ defaultValue })} />}
      {text && (
        <>
          <Check label="Multiple lines" checked={f.multiline} onChange={(multiline) => set({ multiline })} />
          <Check label="Password (hide characters)" checked={f.password} onChange={(password) => set({ password })} />
          <Check label="Comb (one box per character)" checked={f.comb} onChange={(comb) => set({ comb })} />
          <Num label="Character limit (0 for none)" value={f.maxLen} onCommit={(v) => set({ maxLen: v ?? 0 })} max={500} />
          {needsMaxLen(f) && <p className="dim small">A comb field needs a character limit to draw its boxes.</p>}
        </>
      )}
      {f.kind === "combo" && <Check label="Let people type their own value" checked={f.editable} onChange={(editable) => set({ editable })} />}
      {f.kind === "list" && <Check label="Allow several choices" checked={f.multiSelect} onChange={(multiSelect) => set({ multiSelect })} />}
      {choice && <Options options={f.options} onChange={(options) => set({ options })} />}

      {f.kind === "button" && <ButtonProps f={f} all={all} set={set} />}

      {f.kind !== "signature" && f.kind !== "checkbox" && f.kind !== "radio" && f.kind !== "button" && (
        <>
          <div className="fm-sub">Appearance</div>
          <div className="fm-pair">
            <Pick
              label="Text size"
              value={f.fontSize}
              options={[...new Set([...FONT_SIZES, f.fontSize])].sort((a, b) => a - b).map((s) => ({ value: s, label: s === 0 ? "Auto" : `${s} pt` }))}
              onChange={(fontSize) => set({ fontSize })}
            />
            <Pick
              label="Align"
              value={f.align}
              options={[{ value: 0, label: "Left" }, { value: 1, label: "Centre" }, { value: 2, label: "Right" }]}
              onChange={(align) => set({ align })}
            />
          </div>
          <Colour label="Text colour" value={f.textColor} onPick={(textColor) => set({ textColor })} />
        </>
      )}
      <div className="fm-pair">
        <Colour label="Border colour" value={f.borderColor} onPick={(borderColor) => set({ borderColor })} onClear={() => set({ noBorder: true })} />
        <Colour label="Fill colour" value={f.fillColor} onPick={(fillColor) => set({ fillColor })} onClear={() => set({ noFill: true })} />
      </div>
      <Pick
        label="Border width"
        value={Math.round(f.borderWidth)}
        options={[0, 1, 2, 3, 4].map((w) => ({ value: w, label: w === 0 ? "None" : `${w} pt` }))}
        onChange={(borderWidth) => set({ borderWidth })}
      />

      {typed && <FormatProps f={f} set={set} />}
      {text && <ValidateProps f={f} set={set} />}
      {text && <CalcProps doc={doc} f={f} all={all} set={set} />}
    </section>
  );
}

function Options({ options, onChange }: { options: Opt[]; onChange: (o: Opt[]) => void }) {
  const [rows, setRows] = useState(options);
  useEffect(() => setRows(options), [options]);
  const commit = (next: Opt[]) => onChange(next);
  return (
    <div className="fm-field">
      <span>Choices (label, value)</span>
      <div className="fm-opts">
        {rows.map((o, i) => (
          <div key={i} className="fm-opt">
            <input
              className="fm-in"
              aria-label={`Choice ${i + 1} label`}
              value={o.label}
              onChange={(e) => setRows(rows.map((r, j) => (j === i ? { ...r, label: e.target.value } : r)))}
              onBlur={() => JSON.stringify(rows) !== JSON.stringify(options) && commit(rows)}
            />
            <input
              className="fm-in"
              aria-label={`Choice ${i + 1} value`}
              value={o.export}
              onChange={(e) => setRows(rows.map((r, j) => (j === i ? { ...r, export: e.target.value } : r)))}
              onBlur={() => JSON.stringify(rows) !== JSON.stringify(options) && commit(rows)}
            />
            <button className="fm-mini" title="Remove choice" aria-label={`Remove choice ${i + 1}`} onClick={() => commit(rows.filter((_, j) => j !== i))}>
              <Icon name="close" size={13} />
            </button>
          </div>
        ))}
        <button className="fm-btn" onClick={() => commit([...rows, { export: `Choice ${rows.length + 1}`, label: `Choice ${rows.length + 1}` }])}>
          Add a choice
        </button>
      </div>
    </div>
  );
}

const ACTIONS: { value: string; label: string }[] = [
  { value: "none", label: "Nothing" },
  { value: "reset", label: "Reset the form" },
  { value: "NextPage", label: "Go to the next page" },
  { value: "PrevPage", label: "Go to the previous page" },
  { value: "FirstPage", label: "Go to the first page" },
  { value: "LastPage", label: "Go to the last page" },
  { value: "alert", label: "Show a message" },
  { value: "submit", label: "Submit the form (does nothing here)" },
];

function actionKey(a: ButtonAction | null): string {
  if (!a) return "none";
  if (a.kind === "named") return a.name || "none";
  if (a.kind === "uri" || a.kind === "script") return "none";
  return a.kind;
}

function ButtonProps({ f, all, set }: { f: Field; all: Field[]; set: (p: FieldPatch) => void }) {
  const a = f.action;
  const key = actionKey(a);
  const names = [...new Set(all.filter((x) => x.kind !== "button" && x.kind !== "signature").map((x) => x.name))];
  const choose = (k: string) => {
    if (k === "reset") set({ action: { kind: "reset", fields: [] } });
    else if (k === "alert") set({ action: { kind: "alert", message: "Hello" } });
    else if (k === "submit") set({ action: { kind: "submit", url: "" } });
    else if (k === "none") set({ action: { kind: "named", name: "" } });
    else set({ action: { kind: "named", name: k } });
  };
  return (
    <>
      <Text label="Label on the button" value={f.caption} onCommit={(caption) => set({ caption })} />
      <Pick label="When clicked" value={key} options={ACTIONS} onChange={choose} />
      {a?.kind === "reset" && (
        <div className="fm-field">
          <span>Fields to reset (none ticked: the whole form)</span>
          {a.fields.length > 0 && (
            <Check label="Reset all but the ticked fields" checked={!!a.exclude} onChange={(exclude) => set({ action: { kind: "reset", fields: a.fields, exclude } })} />
          )}
          {names.map((n) => (
            <Check
              key={n}
              label={n}
              checked={a.fields.includes(n)}
              onChange={(on) => set({ action: { kind: "reset", fields: on ? [...a.fields, n] : a.fields.filter((x) => x !== n), exclude: a.exclude } })}
            />
          ))}
        </div>
      )}
      {a?.kind === "alert" && <Text label="Message" value={a.message} area onCommit={(message) => set({ action: { kind: "alert", message } })} />}
      {a?.kind === "submit" && (
        <>
          <Text label="Address (kept, never contacted)" value={a.url} onCommit={(url) => set({ action: { kind: "submit", url } })} />
          <p className="dim small">Inkwell shows the button but never sends anything.</p>
        </>
      )}
      {(a?.kind === "uri" || a?.kind === "script") && <p className="dim small">This button runs {a.kind === "uri" ? "a link" : "a script"} Inkwell keeps but does not run.</p>}
    </>
  );
}

function Script({ label, value, onCommit, hint }: { label: string; value: string; onCommit: (v: string) => void; hint?: string }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  return (
    <label className="fm-field">
      <span>{label}</span>
      <textarea
        className="fm-in fm-code"
        value={v}
        rows={5}
        spellCheck={false}
        aria-label={label}
        onChange={(e) => setV(e.target.value)}
        onBlur={() => v !== value && onCommit(v)}
        onKeyDown={(e) => e.stopPropagation()}
      />
      {hint && <small className="dim">{hint}</small>}
    </label>
  );
}

/** "1234.5 → $1,234.50": what the chosen format makes of a sample entry (a custom script is not run for a preview). */
function FormatPreview({ format }: { format: Format }) {
  const [shown, setShown] = useState<{ sample: string; shown: string } | null>(null);
  const key = JSON.stringify(format);
  useEffect(() => {
    let live = true;
    if (format.kind === "none" || format.kind === "custom") {
      setShown(null);
      return;
    }
    formsApi.formatPreview(format).then(
      (r) => live && setShown(r),
      () => live && setShown(null),
    );
    return () => {
      live = false;
    };
    // `key` stands for the whole format.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  if (format.kind === "custom") return <p className="fm-sample dim small">Custom script: what it shows is decided when the field is filled.</p>;
  if (!shown) return null;
  return (
    <p className="fm-sample" aria-label="Sample">
      <span className="dim">Sample</span> <code>{shown.sample}</code> <span className="dim">→</span> <b>{shown.shown}</b>
    </p>
  );
}

function FormatProps({ f, set }: { f: Field; set: (p: FieldPatch) => void }) {
  const fmt = f.format;
  // "Custom script" with both boxes empty is "no format" in the file, but the person has asked for the boxes.
  const [forced, setForced] = useState(false);
  const shownKind = fmt.kind === "custom" || (forced && fmt.kind === "none") ? "custom" : fmt.kind;
  const put = (format: Format) => set({ format });
  const kinds = [
    { value: "none", label: "None" },
    { value: "number", label: "Number" },
    { value: "percent", label: "Percentage" },
    { value: "date", label: "Date" },
    { value: "time", label: "Time" },
    { value: "special", label: "Zip, phone, social security" },
    { value: "mask", label: "Pattern (9 digit, A letter, X any)" },
    { value: "custom", label: "Custom script" },
  ];
  const choose = (k: string) => {
    setForced(k === "custom");
    if (k === shownKind) return;
    if (k === "number") put({ kind: "number", value: { decimals: 2, sep: 0, neg: 0, currency: "", prepend: true } });
    else if (k === "percent") put({ kind: "percent", value: { decimals: 0, sep: 0 } });
    else if (k === "date") put({ kind: "date", value: "mm/dd/yyyy" });
    else if (k === "time") put({ kind: "time", value: "HH:MM" });
    else if (k === "special") put({ kind: "special", value: 0 });
    else if (k === "mask") put({ kind: "mask", value: "999-999" });
    else if (k === "custom") put({ kind: "custom", value: { format: "", keystroke: "" } });
    else put({ kind: "none" });
  };
  const custom = fmt.kind === "custom" ? fmt.value : { format: "", keystroke: "" };
  const withCustom = (list: string[], cur: string) => (list.includes(cur) ? list : [cur, ...list]);
  return (
    <>
      <div className="fm-sub">Format</div>
      <Pick label="Show the value as" value={shownKind} options={kinds} onChange={choose} />
      {fmt.kind === "number" && (
        <>
          <div className="fm-pair">
            <Num label="Decimal places" value={fmt.value.decimals} max={12} onCommit={(v) => put({ kind: "number", value: { ...fmt.value, decimals: v ?? 0 } })} />
            <Text label="Currency symbol" value={fmt.value.currency} onCommit={(currency) => put({ kind: "number", value: { ...fmt.value, currency } })} />
          </div>
          <Pick label="Separators" value={fmt.value.sep} options={SEPARATORS.map((l, i) => ({ value: i, label: l }))} onChange={(sep) => put({ kind: "number", value: { ...fmt.value, sep } })} />
          <Pick label="Negative numbers" value={fmt.value.neg} options={NEGATIVES.map((l, i) => ({ value: i, label: l }))} onChange={(neg) => put({ kind: "number", value: { ...fmt.value, neg } })} />
          <Check label="Currency symbol before the number" checked={fmt.value.prepend} onChange={(prepend) => put({ kind: "number", value: { ...fmt.value, prepend } })} />
        </>
      )}
      {fmt.kind === "percent" && (
        <div className="fm-pair">
          <Num label="Decimal places" value={fmt.value.decimals} max={12} onCommit={(v) => put({ kind: "percent", value: { ...fmt.value, decimals: v ?? 0 } })} />
          <Pick label="Separators" value={fmt.value.sep} options={SEPARATORS.map((l, i) => ({ value: i, label: l }))} onChange={(sep) => put({ kind: "percent", value: { ...fmt.value, sep } })} />
        </div>
      )}
      {fmt.kind === "date" && (
        <Pick label="Date" value={fmt.value} options={withCustom(DATE_FORMATS, fmt.value).map((v) => ({ value: v, label: v }))} onChange={(value) => put({ kind: "date", value })} />
      )}
      {fmt.kind === "time" && (
        <Pick label="Time" value={fmt.value} options={withCustom(TIME_FORMATS, fmt.value).map((v) => ({ value: v, label: v }))} onChange={(value) => put({ kind: "time", value })} />
      )}
      {fmt.kind === "special" && (
        <Pick label="Kind" value={fmt.value} options={SPECIAL_FORMATS.map((l, i) => ({ value: i, label: l }))} onChange={(value) => put({ kind: "special", value })} />
      )}
      {fmt.kind === "mask" && <Text label="Pattern" value={fmt.value} onCommit={(value) => put({ kind: "mask", value })} />}
      {shownKind === "custom" && (
        <>
          <Script
            label="Custom format script"
            value={custom.format}
            hint="Runs to decide what the field shows. Set event.value to the text."
            onCommit={(format) => put({ kind: "custom", value: { ...custom, format } })}
          />
          <Script
            label="Custom keystroke script"
            value={custom.keystroke}
            hint="Runs on every key and on commit. Set event.rc = false to refuse."
            onCommit={(keystroke) => put({ kind: "custom", value: { ...custom, keystroke } })}
          />
        </>
      )}
      <FormatPreview format={fmt} />
    </>
  );
}

function ValidateProps({ f, set }: { f: Field; set: (p: FieldPatch) => void }) {
  const v: Validate = f.validate;
  const range = v.kind === "range" ? v : null;
  const [forced, setForced] = useState(false);
  const shownKind = v.kind === "custom" || (forced && v.kind === "none") ? "custom" : v.kind;
  const put = (validate: Validate) => set({ validate });
  const choose = (k: string) => {
    setForced(k === "custom");
    if (k === shownKind) return;
    if (k === "range") put({ kind: "range", min: 0, max: null });
    else if (k === "custom") put({ kind: "custom", source: "" });
    else put({ kind: "none" });
  };
  return (
    <>
      <div className="fm-sub">Validate</div>
      <Pick
        label="Check the value"
        value={shownKind}
        options={[
          { value: "none", label: "Not validated" },
          { value: "range", label: "Number in a range" },
          { value: "custom", label: "Custom validation script" },
        ]}
        onChange={choose}
      />
      {range && (
        <div className="fm-pair">
          <Num label="At least" value={range.min} min={-1e12} max={1e12} placeholder="no limit" onCommit={(min) => put({ kind: "range", min, max: range.max })} />
          <Num label="At most" value={range.max} min={-1e12} max={1e12} placeholder="no limit" onCommit={(max) => put({ kind: "range", min: range.min, max })} />
        </div>
      )}
      {shownKind === "custom" && (
        <Script
          label="Custom validation script"
          value={v.kind === "custom" ? v.source : ""}
          hint="Runs when the value is committed. Set event.rc = false to refuse it."
          onCommit={(source) => put({ kind: "custom", source })}
        />
      )}
    </>
  );
}

function CalcProps({ doc, f, all, set }: { doc: number; f: Field; all: Field[]; set: (p: FieldPatch) => void }) {
  const c: Calculate = f.calculate;
  const sources = calcSources(all, f);
  const order = useForms((s) => s.docs[doc]?.calcOrder);
  const [forced, setForced] = useState(false);
  const [ordering, setOrdering] = useState(false);
  const kind = c.kind === "simple" ? c.value.op : c.kind === "none" && forced ? "custom" : c.kind;
  const put = (calculate: Calculate) => set({ calculate });
  const choose = (k: string) => {
    setForced(k === "custom");
    if (k === kind) return;
    if (k === "none") put({ kind: "none" });
    else if (k === "notation") put({ kind: "notation", value: sources.slice(0, 2).join(" + ") || "0" });
    else if (k === "custom") put({ kind: "custom", value: "" });
    else put({ kind: "simple", value: { op: k as CalcOp, fields: c.kind === "simple" ? c.value.fields : sources.slice(0, 2) } });
  };
  return (
    <>
      <div className="fm-sub">Calculate</div>
      <Pick
        label="Value is"
        value={kind}
        options={[
          { value: "none", label: "Not calculated" },
          ...CALC_OPS.map((o) => ({ value: o.op as string, label: o.label })),
          { value: "notation", label: "Simplified field notation" },
          { value: "custom", label: "Custom calculation script" },
        ]}
        onChange={choose}
      />
      {c.kind === "simple" && (
        <div className="fm-field">
          <span>Fields</span>
          {sources.length === 0 && <p className="dim small">Add other text fields first.</p>}
          {sources.map((n) => (
            <Check
              key={n}
              label={n}
              checked={c.value.fields.includes(n)}
              onChange={(on) => put({ kind: "simple", value: { op: c.value.op, fields: on ? [...c.value.fields, n] : c.value.fields.filter((x) => x !== n) } })}
            />
          ))}
        </div>
      )}
      {c.kind === "notation" && (
        <>
          <Text label="Formula" value={c.value} onCommit={(value) => put({ kind: "notation", value })} placeholder="Price * Quantity" />
          <p className="dim small">Field names with + − × ÷ and brackets. Put a backslash before a space inside a name.</p>
        </>
      )}
      {kind === "custom" && (
        <Script
          label="Custom calculation script"
          value={c.kind === "custom" ? c.value : ""}
          hint='Runs after any field changes. Set event.value, e.g. event.value = this.getField("Price").value * 2;'
          onCommit={(value) => put({ kind: "custom", value })}
        />
      )}
      {(order?.length ?? 0) > 1 && (
        <div className="fm-actions">
          <button className="fm-btn" onClick={() => setOrdering(true)}>
            Calculation order…
          </button>
        </div>
      )}
      {ordering && <CalcOrder doc={doc} all={all} order={order ?? []} onClose={() => setOrdering(false)} />}
    </>
  );
}

/** The calculated fields in the order they run, to move up or down. OK is one undo step. */
function CalcOrder({ doc, all, order, onClose }: { doc: number; all: Field[]; order: number[]; onClose: () => void }) {
  const [nodes, setNodes] = useState(order);
  const fields = calcFields(all, nodes);
  const changed = nodes.join() !== order.join();
  return (
    <div className="scrim" role="presentation">
      <div
        className="dialog fm-order-dlg"
        role="dialog"
        aria-modal="true"
        aria-label="Calculation order"
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") onClose();
        }}
      >
        <h2>Calculation order</h2>
        <p className="dim small">Fields are calculated from the top down. Put a field after the ones it reads.</p>
        <ol className="fm-order-list">
          {fields.map((f, i) => (
            <li key={f.node}>
              <span className="n">{i + 1}</span>
              <span className="name" title={f.name}>{f.name}</span>
              <button className="fm-mini" disabled={i === 0} title="Earlier" aria-label={`Move ${f.name} earlier`} onClick={() => setNodes(moved(nodes, i, i - 1))}>
                <Icon name="up" size={13} />
              </button>
              <button className="fm-mini" disabled={i === fields.length - 1} title="Later" aria-label={`Move ${f.name} later`} onClick={() => setNodes(moved(nodes, i, i + 1))}>
                <Icon name="down" size={13} />
              </button>
            </li>
          ))}
        </ol>
        <div className="dialog-actions">
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            autoFocus
            className="btn primary"
            disabled={!changed}
            onClick={() => {
              void fm().calcOrder(doc, nodes);
              onClose();
            }}
          >
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
