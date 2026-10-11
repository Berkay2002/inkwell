import { useEffect, useState } from "react";
import { useForms } from "../../lib/forms";
import {
  type ButtonAction, type CalcOp, type Calculate, type Field, type FieldPatch, type Format, type Opt, type Validate,
  CALC_OPS, DATE_FORMATS, FIELD_ICON, FONT_SIZES, KIND_LABEL, NEGATIVES, SEPARATORS, SPECIAL_FORMATS, TIME_FORMATS,
  calcSources, fromHex, needsMaxLen, toHex,
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
      {text && <CalcProps f={f} all={all} set={set} />}
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
          {names.map((n) => (
            <Check
              key={n}
              label={n}
              checked={a.fields.includes(n)}
              onChange={(on) => set({ action: { kind: "reset", fields: on ? [...a.fields, n] : a.fields.filter((x) => x !== n) } })}
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

function FormatProps({ f, set }: { f: Field; set: (p: FieldPatch) => void }) {
  const fmt = f.format;
  const put = (format: Format) => set({ format });
  const kinds = [
    { value: "none", label: "None" },
    { value: "number", label: "Number" },
    { value: "percent", label: "Percentage" },
    { value: "date", label: "Date" },
    { value: "time", label: "Time" },
    { value: "special", label: "Zip, phone, social security" },
    { value: "mask", label: "Pattern (9 digit, A letter, X any)" },
  ];
  const choose = (k: string) => {
    if (k === "number") put({ kind: "number", value: { decimals: 2, sep: 0, neg: 0, currency: "", prepend: true } });
    else if (k === "percent") put({ kind: "percent", value: { decimals: 0, sep: 0 } });
    else if (k === "date") put({ kind: "date", value: "mm/dd/yyyy" });
    else if (k === "time") put({ kind: "time", value: "HH:MM" });
    else if (k === "special") put({ kind: "special", value: 0 });
    else if (k === "mask") put({ kind: "mask", value: "999-999" });
    else put({ kind: "none" });
  };
  const withCustom = (list: string[], cur: string) => (list.includes(cur) ? list : [cur, ...list]);
  return (
    <>
      <div className="fm-sub">Format</div>
      <Pick label="Show the value as" value={fmt.kind} options={kinds} onChange={choose} />
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
    </>
  );
}

function ValidateProps({ f, set }: { f: Field; set: (p: FieldPatch) => void }) {
  const v: Validate = f.validate;
  const range = v.kind === "range" ? v : null;
  const put = (validate: Validate) => set({ validate });
  return (
    <>
      <div className="fm-sub">Validate</div>
      <Check
        label="Value must be a number in a range"
        checked={range !== null}
        onChange={(on) => put(on ? { kind: "range", min: 0, max: null } : { kind: "none" })}
      />
      {range && (
        <div className="fm-pair">
          <Num label="At least" value={range.min} min={-1e12} max={1e12} placeholder="no limit" onCommit={(min) => put({ kind: "range", min, max: range.max })} />
          <Num label="At most" value={range.max} min={-1e12} max={1e12} placeholder="no limit" onCommit={(max) => put({ kind: "range", min: range.min, max })} />
        </div>
      )}
    </>
  );
}

function CalcProps({ f, all, set }: { f: Field; all: Field[]; set: (p: FieldPatch) => void }) {
  const c: Calculate = f.calculate;
  const sources = calcSources(all, f);
  const kind = c.kind === "simple" ? c.value.op : c.kind;
  const put = (calculate: Calculate) => set({ calculate });
  const choose = (k: string) => {
    if (k === "none") put({ kind: "none" });
    else if (k === "notation") put({ kind: "notation", value: sources.slice(0, 2).join(" + ") || "0" });
    else put({ kind: "simple", value: { op: k as CalcOp, fields: c.kind === "simple" ? c.value.fields : sources.slice(0, 2) } });
  };
  return (
    <>
      <div className="fm-sub">Calculate</div>
      <Pick
        label="Value is"
        value={kind}
        options={[{ value: "none", label: "Typed in" }, ...CALC_OPS.map((o) => ({ value: o.op as string, label: o.label })), { value: "notation", label: "A formula" }]}
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
    </>
  );
}
