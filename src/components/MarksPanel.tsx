import { useEffect, useState } from "react";
import { useAi } from "../lib/aiStore";
import type { FontFamily, MarkStyle } from "../lib/api";
import { formatRange } from "../lib/pageRange";
import {
  batesText,
  DATE_FORMATS,
  expand,
  fromHex,
  KIND_NAME,
  KIND_TITLE,
  KINDS,
  type MarksSheet,
  outcome,
  pagesOf,
  type PagesMode,
  readiness,
  SLOTS,
  todayDate,
  toHex,
  useMarks,
} from "../lib/marksStore";
import type { DocState } from "../store";
import { Icon } from "./Icons";
import "./tools.css";
import "./marks.css";

const st = () => useMarks.getState();
const set = (p: Partial<MarksSheet>) => st().set(p);

/**
 * Header & footer, Watermark, Background and Bates numbering in the same right-hand rail as
 * Compress and Protect. The page shows the mark as it is typed (`MarkPreview`); Add, Update and
 * Remove are one undo step each.
 */
export function MarksPanel({ doc }: { doc: DocState }) {
  const sheet = useMarks((s) => s.sheet);
  const width = useAi((s) => s.settings.width);
  if (!sheet || sheet.doc !== doc.id) return null;
  return (
    <aside className="ax-dock" style={{ width: `min(${width}px, 45vw)` }} aria-label="Page marks">
      <section className="ax-panel docked tl mk">
        <div className="ax-head">
          <span className="ax-title">Page marks</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Page marks" onClick={() => st().close()}>
            <Icon name="close" size={14} />
          </button>
        </div>
        <Sheet sheet={sheet} doc={doc} />
      </section>
    </aside>
  );
}

function Sheet({ sheet, doc }: { sheet: MarksSheet; doc: DocState }) {
  const kinds = useMarks((s) => s.info[doc.id]);
  const here = kinds?.find((k) => k.kind === sheet.kind);
  const pages = pagesOf(sheet.pagesMode, sheet.range, doc.pageCount);
  const existing = !!here && !!pages && pages.some((p) => here.pages.includes(p));
  const why = readiness(sheet, doc.pageCount);
  const sentence = outcome(sheet, doc.pageCount, existing);
  const numbering = sheet.kind === "bates" && sheet.btFiles.length > 0;
  const verb = numbering ? "Number the files…" : existing ? "Update" : "Add to the document";
  return (
    <>
      <div className="ax-body tl-body">
        {kinds && kinds.length > 0 && (
          <div className="mk-has" aria-label="Marks in this document">
            <span className="tl-label">Has</span>
            {kinds.map((k) => (
              <div key={k.kind} className="mk-has-row">
                <span className="mk-has-name">{KIND_NAME[k.kind]}</span>
                <span className="dim small tl-mono">{k.pages.length === doc.pageCount ? "all pages" : `pp. ${formatRange(k.pages)}`}</span>
                <span className="grow" />
                <button className="tl-link" onClick={() => st().setKind(k.kind)}>
                  Update
                </button>
                <button className="tl-link" disabled={sheet.busy} onClick={() => void st().remove(k.kind)}>
                  Remove
                </button>
              </div>
            ))}
          </div>
        )}

        <div className="ax-seg tl-seg mk-tabs" role="tablist" aria-label="Kind of mark">
          {KINDS.map((k) => (
            <button key={k} role="tab" aria-selected={sheet.kind === k} className={sheet.kind === k ? "on" : ""} onClick={() => st().setKind(k)}>
              {KIND_TITLE[k]}
            </button>
          ))}
        </div>

        {sheet.kind === "headerFooter" && <HeaderFooterView sheet={sheet} />}
        {sheet.kind === "watermark" && <WatermarkView sheet={sheet} />}
        {sheet.kind === "background" && <BackgroundView sheet={sheet} />}
        {sheet.kind === "bates" && <BatesView sheet={sheet} />}

        {!numbering && <PagesRow sheet={sheet} count={doc.pageCount} />}
      </div>
      <div className="tl-foot">
        <p className="pr-outcome" aria-live="polite">
          {sentence}
        </p>
        {why && <p className="dim small">{why}</p>}
        {sheet.error && (
          <p className="tl-warn small" role="alert">
            {sheet.error}
          </p>
        )}
        {sheet.done && !sheet.error && (
          <p className="dim small" role="status">
            {sheet.done}
          </p>
        )}
        <button className="ax-btn primary tl-wide" disabled={sheet.busy || !!why} onClick={() => void st().apply()}>
          {sheet.busy ? "Working…" : verb}
        </button>
        {here && !numbering && (
          <button className="ax-btn tl-wide pr-quiet" disabled={sheet.busy} onClick={() => void st().remove(sheet.kind)}>
            Remove {KIND_NAME[sheet.kind]}
          </button>
        )}
      </div>
    </>
  );
}

// ---- small controls ----

function Num({ value, onChange, label, min, max, step = 1, className }: { value: number; onChange: (v: number) => void; label: string; min?: number; max?: number; step?: number; className?: string }) {
  const [text, setText] = useState(String(value));
  useEffect(() => {
    if (Number(text) !== value) setText(String(value));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value]);
  return (
    <input
      className={`tl-field mk-num ${className ?? ""}`}
      type="number"
      inputMode="decimal"
      aria-label={label}
      min={min}
      max={max}
      step={step}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        const v = Number(e.target.value);
        if (e.target.value.trim() !== "" && Number.isFinite(v)) onChange(v);
      }}
    />
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="mk-field">
      <span className="tl-label">{label}</span>
      {children}
    </label>
  );
}

function Seg<T extends string>({ label, value, items, onChange }: { label: string; value: T; items: { id: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <div className="ax-seg tl-seg" role="radiogroup" aria-label={label}>
      {items.map((i) => (
        <button key={i.id} role="radio" aria-checked={value === i.id} className={value === i.id ? "on" : ""} onClick={() => onChange(i.id)}>
          {i.label}
        </button>
      ))}
    </div>
  );
}

const FAMILIES: { id: FontFamily; label: string }[] = [
  { id: "helvetica", label: "Helvetica" },
  { id: "times", label: "Times" },
  { id: "courier", label: "Courier" },
];

function Colour({ value, onChange, label }: { value: [number, number, number]; onChange: (c: [number, number, number]) => void; label: string }) {
  return <input className="mk-swatch" type="color" aria-label={label} value={toHex(value)} onChange={(e) => onChange(fromHex(e.target.value))} />;
}

function StyleRow({ style, onChange }: { style: MarkStyle; onChange: (s: MarkStyle) => void }) {
  const patch = (p: Partial<MarkStyle>) => onChange({ ...style, ...p });
  return (
    <div className="tl-row mk-style">
      <select className="tl-field" aria-label="Font" value={style.family} onChange={(e) => patch({ family: e.target.value as FontFamily })}>
        {FAMILIES.map((f) => (
          <option key={f.id} value={f.id}>
            {f.label}
          </option>
        ))}
      </select>
      <Num label="Font size in points" value={style.size} min={1} max={600} step={1} onChange={(size) => patch({ size })} />
      <Colour label="Text colour" value={style.color} onChange={(color) => patch({ color })} />
      <button className={`ax-icon mk-tog ${style.bold ? "on" : ""}`} aria-pressed={style.bold} aria-label="Bold" title="Bold" onClick={() => patch({ bold: !style.bold })}>
        <b>B</b>
      </button>
      <button className={`ax-icon mk-tog ${style.italic ? "on" : ""}`} aria-pressed={style.italic} aria-label="Italic" title="Italic" onClick={() => patch({ italic: !style.italic })}>
        <i>I</i>
      </button>
      <button className={`ax-icon mk-tog ${style.underline ? "on" : ""}`} aria-pressed={style.underline} aria-label="Underline" title="Underline" onClick={() => patch({ underline: !style.underline })}>
        <u>U</u>
      </button>
    </div>
  );
}

function MarginsRow({ sheet }: { sheet: MarksSheet }) {
  const names = ["Top", "Bottom", "Left", "Right"] as const;
  return (
    <div className="mk-margins">
      <span className="tl-label">Margins (points)</span>
      <div className="tl-row">
        {names.map((n, i) => (
          <label key={n} className="mk-mini">
            <span>{n}</span>
            <Num
              label={`${n} margin in points`}
              value={sheet.margins[i]}
              min={0}
              onChange={(v) => {
                const m = [...sheet.margins] as MarksSheet["margins"];
                m[i] = v;
                set({ margins: m });
              }}
            />
          </label>
        ))}
      </div>
    </div>
  );
}

function PagesRow({ sheet, count }: { sheet: MarksSheet; count: number }) {
  const items: { id: PagesMode; label: string }[] = [
    { id: "all", label: "All" },
    { id: "odd", label: "Odd" },
    { id: "even", label: "Even" },
    { id: "range", label: "Range" },
  ];
  return (
    <div className="mk-pages">
      <span className="tl-label">Pages</span>
      <Seg<PagesMode> label="Pages" value={sheet.pagesMode} items={items} onChange={(pagesMode) => set({ pagesMode })} />
      {sheet.pagesMode === "range" && (
        <input
          className={pagesOf("range", sheet.range, count) ? "tl-field" : "tl-field bad"}
          aria-label="Pages to mark"
          placeholder={`1-${count}`}
          spellCheck={false}
          value={sheet.range}
          onChange={(e) => set({ range: e.target.value })}
        />
      )}
    </div>
  );
}

// ---- header and footer ----

const NUMBER_FORMATS: { token: string; label: string }[] = [
  { token: "<<1>>", label: "1" },
  { token: "<<Page 1>>", label: "Page 1" },
  { token: "<<1 of n>>", label: "1 of n" },
  { token: "<<Page 1 of n>>", label: "Page 1 of n" },
  { token: "<<1/n>>", label: "1/n" },
];

function HeaderFooterView({ sheet }: { sheet: MarksSheet }) {
  // The slot the next inserted token goes into: the last one typed in, else bottom right.
  const [slot, setSlot] = useState(5);
  const today = todayDate();
  const insert = (token: string) => {
    const text = [...sheet.text] as MarksSheet["text"];
    text[slot] = text[slot] + token;
    set({ text });
  };
  const slotInput = (i: number) => (
    <input
      key={i}
      className="tl-field"
      aria-label={`Text at the ${SLOTS[i]}`}
      placeholder={["Left", "Centre", "Right"][i % 3]}
      spellCheck={false}
      value={sheet.text[i]}
      onFocus={() => setSlot(i)}
      onChange={(e) => {
        const text = [...sheet.text] as MarksSheet["text"];
        text[i] = e.target.value;
        set({ text });
      }}
    />
  );
  return (
    <>
      <div className="mk-slots">
        <span className="tl-label">Top</span>
        <div className="mk-three">{[0, 1, 2].map(slotInput)}</div>
        <span className="tl-label">Bottom</span>
        <div className="mk-three">{[3, 4, 5].map(slotInput)}</div>
      </div>
      <div className="tl-row mk-insert">
        <select
          className="tl-field"
          aria-label="Insert page number"
          value=""
          onChange={(e) => e.target.value && insert(e.target.value)}
        >
          <option value="">Insert page number…</option>
          {NUMBER_FORMATS.map((f) => (
            <option key={f.token} value={f.token}>
              {expand(f.token, 1, 12, today)}
            </option>
          ))}
        </select>
        <select className="tl-field" aria-label="Insert date" value="" onChange={(e) => e.target.value && insert(`<<${e.target.value}>>`)}>
          <option value="">Insert date…</option>
          {DATE_FORMATS.map((f) => (
            <option key={f} value={f}>
              {expand(`<<${f}>>`, 1, 1, today)}
            </option>
          ))}
        </select>
      </div>
      <StyleRow style={sheet.style} onChange={(style) => set({ style })} />
      <MarginsRow sheet={sheet} />
      <Field label="Number the first page">
        <Num label="Start number" value={sheet.startNumber} min={0} onChange={(startNumber) => set({ startNumber })} />
      </Field>
    </>
  );
}

// ---- watermark and background ----

function PicturePick({ target, choice }: { target: "wm" | "bg"; choice: MarksSheet["wmPicture"] }) {
  return (
    <div className="mk-picture">
      <div className="tl-row">
        <button className="ax-btn" onClick={() => void st().pickPicture(target, "image")}>
          Choose image…
        </button>
        <button className="ax-btn" onClick={() => void st().pickPicture(target, "pdf")}>
          Choose PDF page…
        </button>
      </div>
      {choice ? (
        <div className="tl-row">
          <span className="tl-file tl-mono grow" title={choice.name}>
            {choice.name}
          </span>
          {choice.kind === "pdf" && (
            <Num
              label="Page of that PDF"
              value={choice.page}
              min={1}
              onChange={(page) => set(target === "wm" ? { wmPicture: { ...choice, page } } : { bgPicture: { ...choice, page } })}
            />
          )}
        </div>
      ) : (
        <p className="dim small">No picture chosen yet.</p>
      )}
    </div>
  );
}

function Slider({ label, value, onChange, min, max, unit = "%" }: { label: string; value: number; onChange: (v: number) => void; min: number; max: number; unit?: string }) {
  return (
    <label className="mk-slider">
      <span className="tl-label">{label}</span>
      <input type="range" min={min} max={max} value={value} onChange={(e) => onChange(Number(e.target.value))} />
      <span className="tl-mono mk-val">
        {value}
        {unit}
      </span>
    </label>
  );
}

function WatermarkView({ sheet }: { sheet: MarksSheet }) {
  const rot = sheet.wmRotation === 0 ? "0" : sheet.wmRotation === 45 ? "45" : sheet.wmRotation === -45 ? "-45" : "custom";
  const [custom, setCustom] = useState(rot === "custom");
  const shown = custom ? "custom" : rot;
  return (
    <>
      <Seg<"text" | "picture">
        label="Watermark source"
        value={sheet.wmSource}
        items={[
          { id: "text", label: "Text" },
          { id: "picture", label: "Picture" },
        ]}
        onChange={(wmSource) => set({ wmSource })}
      />
      {sheet.wmSource === "text" ? (
        <>
          <input className="tl-field" aria-label="Watermark text" placeholder="DRAFT" spellCheck={false} value={sheet.wmText} onChange={(e) => set({ wmText: e.target.value })} />
          <div className="tl-row mk-style">
            <select className="tl-field" aria-label="Font" value={sheet.wmFamily} onChange={(e) => set({ wmFamily: e.target.value as FontFamily })}>
              {FAMILIES.map((f) => (
                <option key={f.id} value={f.id}>
                  {f.label}
                </option>
              ))}
            </select>
            <Colour label="Text colour" value={sheet.wmColor} onChange={(wmColor) => set({ wmColor })} />
            <button className={`ax-icon mk-tog ${sheet.wmBold ? "on" : ""}`} aria-pressed={sheet.wmBold} aria-label="Bold" title="Bold" onClick={() => set({ wmBold: !sheet.wmBold })}>
              <b>B</b>
            </button>
          </div>
        </>
      ) : (
        <PicturePick target="wm" choice={sheet.wmPicture} />
      )}
      <Slider label="Opacity" value={sheet.wmOpacity} min={5} max={100} onChange={(wmOpacity) => set({ wmOpacity })} />
      <span className="tl-label">Rotation</span>
      <Seg<string>
        label="Rotation"
        value={shown}
        items={[
          { id: "0", label: "None" },
          { id: "45", label: "45°" },
          { id: "-45", label: "−45°" },
          { id: "custom", label: "Custom" },
        ]}
        onChange={(v) => {
          if (v === "custom") return setCustom(true);
          setCustom(false);
          set({ wmRotation: Number(v) });
        }}
      />
      {shown === "custom" && <Num label="Rotation in degrees, counter-clockwise" value={sheet.wmRotation} min={-360} max={360} onChange={(wmRotation) => set({ wmRotation })} />}
      <Slider label="Size, relative to the page" value={sheet.wmScale} min={10} max={150} onChange={(wmScale) => set({ wmScale })} />
      <span className="tl-label">Position</span>
      <Seg<"behind" | "front">
        label="Position in the stack"
        value={sheet.wmBehind ? "behind" : "front"}
        items={[
          { id: "behind", label: "Behind the text" },
          { id: "front", label: "In front" },
        ]}
        onChange={(v) => set({ wmBehind: v === "behind" })}
      />
      <div className="tl-row">
        <label className="mk-mini">
          <span>Right</span>
          <Num label="Distance to the right of the centre, in points" value={sheet.wmOffset[0]} onChange={(x) => set({ wmOffset: [x, sheet.wmOffset[1]] })} />
        </label>
        <label className="mk-mini">
          <span>Up</span>
          <Num label="Distance above the centre, in points" value={sheet.wmOffset[1]} onChange={(y) => set({ wmOffset: [sheet.wmOffset[0], y] })} />
        </label>
      </div>
      <label className="tl-switch">
        <input type="checkbox" checked={sheet.wmScreen} onChange={(e) => set({ wmScreen: e.target.checked })} />
        <span>Show on screen</span>
      </label>
      <label className="tl-switch">
        <input type="checkbox" checked={sheet.wmPrint} onChange={(e) => set({ wmPrint: e.target.checked })} />
        <span>Show when printing</span>
      </label>
    </>
  );
}

function BackgroundView({ sheet }: { sheet: MarksSheet }) {
  return (
    <>
      <Seg<"colour" | "picture">
        label="Background source"
        value={sheet.bgSource}
        items={[
          { id: "colour", label: "Colour" },
          { id: "picture", label: "Picture" },
        ]}
        onChange={(bgSource) => set({ bgSource })}
      />
      {sheet.bgSource === "colour" ? (
        <label className="tl-row mk-colour">
          <Colour label="Background colour" value={sheet.bgColor} onChange={(bgColor) => set({ bgColor })} />
          <span className="tl-mono">{toHex(sheet.bgColor)}</span>
        </label>
      ) : (
        <>
          <PicturePick target="bg" choice={sheet.bgPicture} />
          <Slider label="Size, relative to the page" value={sheet.bgScale} min={10} max={150} onChange={(bgScale) => set({ bgScale })} />
        </>
      )}
      <Slider label="Opacity" value={sheet.bgOpacity} min={5} max={100} onChange={(bgOpacity) => set({ bgOpacity })} />
    </>
  );
}

// ---- Bates ----

function BatesView({ sheet }: { sheet: MarksSheet }) {
  const example = batesText(sheet.btPrefix, sheet.btDigits, sheet.btStart, sheet.btSuffix);
  return (
    <>
      <div className="tl-row">
        <Field label="Prefix">
          <input className="tl-field" spellCheck={false} value={sheet.btPrefix} onChange={(e) => set({ btPrefix: e.target.value })} />
        </Field>
        <Field label="Suffix">
          <input className="tl-field" spellCheck={false} value={sheet.btSuffix} onChange={(e) => set({ btSuffix: e.target.value })} />
        </Field>
      </div>
      <div className="tl-row">
        <Field label="Digits (3 to 15)">
          <Num label="Number of digits" value={sheet.btDigits} min={3} max={15} onChange={(btDigits) => set({ btDigits })} />
        </Field>
        <Field label="Start number">
          <Num label="Start number" value={sheet.btStart} min={0} onChange={(btStart) => set({ btStart })} />
        </Field>
      </div>
      <p className="dim small">
        First number: <span className="tl-mono">{example}</span>
      </p>
      <span className="tl-label">Position</span>
      <div className="mk-places" role="radiogroup" aria-label="Where the number goes">
        {SLOTS.map((name, i) => (
          <button
            key={name}
            role="radio"
            aria-checked={sheet.btSlot === i}
            className={sheet.btSlot === i ? "on" : ""}
            onClick={() => set({ btSlot: i })}
          >
            {name}
          </button>
        ))}
      </div>
      <StyleRow style={sheet.style} onChange={(style) => set({ style })} />
      <MarginsRow sheet={sheet} />
      <span className="tl-label">Files</span>
      <p className="dim small">
        {sheet.btFiles.length === 0
          ? "Numbers this document. Add files to number several files in a row: copies are written to a folder and the numbers run on from file to file."
          : "Copies are written to a folder you choose. The numbers run on from file to file, in this order."}
      </p>
      {sheet.btFiles.length > 0 && (
        <ul className="tl-files">
          {sheet.btFiles.map((f, i) => (
            <li key={f.token}>
              <span className="tl-file tl-mono" title={f.path}>
                {f.name}
              </span>
              <button
                className="ax-icon"
                aria-label={`Remove ${f.name} from the list`}
                title="Remove from the list"
                onClick={() => set({ btFiles: sheet.btFiles.filter((_, j) => j !== i), btResults: null })}
              >
                <Icon name="close" size={12} />
              </button>
            </li>
          ))}
        </ul>
      )}
      <button className="ax-btn" onClick={() => void st().addBatesFiles()}>
        Add files…
      </button>
      {sheet.btResults && (
        <ul className="tl-files mk-results" aria-label="Files written">
          {sheet.btResults.map((r) => (
            <li key={r.output}>
              <span className="tl-file tl-mono" title={r.output}>
                {r.output.split(/[\\/]/).pop()}
              </span>
              <span className="dim small tl-mono">
                {batesText(sheet.btPrefix, sheet.btDigits, r.first, sheet.btSuffix)} – {batesText(sheet.btPrefix, sheet.btDigits, r.last, sheet.btSuffix)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}
