import { useEffect, useMemo, useState } from "react";
import { useStore } from "../../store";
import type { Size } from "../../lib/layout";
import { pageThumb } from "../../lib/plateThumbs";
import { newBox, usePrepress, type BoxesSheet, type ConvertSheet, type MarksSheet } from "../../lib/prepressStore";
import type { BoxKind, Rect4 } from "../../lib/prepressApi";
import {
  type MarkPath,
  type Units,
  BOX_KINDS,
  BOX_LABEL,
  CERTIFIED_REASON,
  INTENT_LABEL,
  UNITS,
  WEIGHTS,
  boxOf,
  boxProblem,
  cmykCss,
  convertProblem,
  convertSummary,
  marksCaption,
  parseLength,
  parseMarks,
  rectShown,
  refsFor,
  remainingText,
  SIGNED_WARNING,
  showLength,
  svgPath,
  visibleBox,
} from "../../lib/prepressLogic";
import { Icon } from "../Icons";
import "../print.css";
import "./prepress.css";

const act = () => usePrepress.getState();

/** The three print production sheets, over the viewer. */
export function PrepressSheets() {
  const sheet = usePrepress((s) => s.sheet);
  const busy = sheet != null && (sheet.kind === "convert" ? sheet.phase === "running" : sheet.kind === "marks" || sheet.kind === "boxes" ? sheet.busy : false);
  useEffect(() => {
    if (!sheet) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        act().closeSheet();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [sheet != null]);
  if (!sheet) return null;
  return (
    <div className="scrim pr-scrim" onMouseDown={() => !busy && act().closeSheet()}>
      {sheet.kind === "convert" ? <ConvertView sheet={sheet} /> : sheet.kind === "marks" ? <MarksView sheet={sheet} /> : <BoxesView sheet={sheet} />}
    </div>
  );
}

// ---- shared bits ----

function Head({ icon, title, doc }: { icon: "swatches" | "crop" | "pagebox"; title: string; doc: string }) {
  return (
    <header className="pr-head">
      <Icon name={icon} size={16} />
      <span className="pr-title">{title}</span>
      <span className="pr-doc dim">{doc}</span>
      <span className="grow" />
      <button className="ax-icon" title="Close" aria-label={`Close ${title}`} onClick={() => act().closeSheet()}>
        <Icon name="close" size={14} />
      </button>
    </header>
  );
}

function Seg<T extends string>({ label, value, items, onChange, disabled }: { label: string; value: T; items: { id: T; label: string }[]; onChange: (v: T) => void; disabled?: boolean }) {
  return (
    <div className="ax-seg pr-seg" role="radiogroup" aria-label={label}>
      {items.map((i) => (
        <button key={i.id} role="radio" aria-checked={value === i.id} disabled={disabled} className={value === i.id ? "on" : ""} onClick={() => onChange(i.id)}>
          {i.label}
        </button>
      ))}
    </div>
  );
}

/** What the document's signatures mean for these sheets: certified is refused (the controls are off), signed is a warning. */
function useLocked(doc: number) {
  return usePrepress((s) => s.docs[doc]?.protection ?? null);
}

function Protect({ doc }: { doc: number }) {
  const p = useLocked(doc);
  if (p?.certified) return <p className="pr-note bad">{CERTIFIED_REASON}</p>;
  if (p?.signed) return <p className="pr-note dim">{SIGNED_WARNING}</p>;
  return null;
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="pr-field">
      <span className="pr-label">{label}</span>
      {children}
    </div>
  );
}

function Check({ label, checked, onChange, disabled }: { label: string; checked: boolean; onChange: (v: boolean) => void; disabled?: boolean }) {
  return (
    <label className="pr-check">
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
    </label>
  );
}

/** A length in the sheet's units; the points value is the source of truth, the text only follows it. */
function Length({ label, pt, units, onPt, disabled }: { label: string; pt: number; units: Units; onPt: (pt: number) => void; disabled?: boolean }) {
  const [text, setText] = useState(showLength(pt, units));
  const [bad, setBad] = useState(false);
  useEffect(() => {
    const p = parseLength(text, units);
    if (p === null || Math.abs(p - pt) > 1e-6) setText(showLength(pt, units));
    setBad(false);
    // the text is only rewritten when the value or the units moved
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pt, units]);
  return (
    <label className="pr-field">
      <span className="pr-label">{label}</span>
      <input
        className={bad ? "tl-field bad" : "tl-field"}
        inputMode="decimal"
        spellCheck={false}
        disabled={disabled}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          const p = parseLength(e.target.value, units);
          setBad(p === null);
          if (p !== null) onPt(p);
        }}
      />
    </label>
  );
}

function UnitSelect({ units, onChange }: { units: Units; onChange: (u: Units) => void }) {
  return (
    <select className="tl-field" aria-label="Units" value={units} onChange={(e) => onChange(e.target.value as Units)}>
      {UNITS.map((u) => (
        <option key={u} value={u}>
          {u === "pt" ? "Points" : u === "mm" ? "Millimetres" : "Inches"}
        </option>
      ))}
    </select>
  );
}

function Scope(p: { scope: "all" | "page" | "range"; range: string; count: number; page: number; disabled?: boolean; set: (v: { scope?: "all" | "page" | "range"; range?: string }) => void }) {
  return (
    <>
      <Field label="Pages">
        <Seg
          label="Pages"
          value={p.scope}
          disabled={p.disabled}
          items={[
            { id: "all", label: `All ${p.count}` },
            { id: "page", label: `Page ${p.page + 1}` },
            { id: "range", label: "Range" },
          ]}
          onChange={(scope) => p.set({ scope })}
        />
      </Field>
      {p.scope === "range" && (
        <input className="tl-field" aria-label="Page range" placeholder="1-3, 5, 8-" spellCheck={false} disabled={p.disabled} value={p.range} onChange={(e) => p.set({ range: e.target.value })} />
      )}
    </>
  );
}

/** The page as printed, with `children` drawn over it in page points from its top-left (SVG, may reach past the page). */
function PagePreview({ doc, page, size, rev, extent, children, caption }: { doc: number; page: number; size: Size; rev: number; extent: Rect4; children: React.ReactNode; caption?: string }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    void pageThumb(doc, page, size, rev).then((u) => live && setUrl(u));
    return () => {
      live = false;
    };
  }, [doc, page, rev, size[0], size[1]]);
  const pad = Math.max(size[0], size[1]) * 0.04;
  const x0 = Math.min(0, extent[0]) - pad;
  const y0 = Math.min(0, extent[1]) - pad;
  const x1 = Math.max(size[0], extent[2]) + pad;
  const y1 = Math.max(size[1], extent[3]) + pad;
  const k = Math.max(size[0], size[1]) / 400;
  return (
    <div className="pr-preview">
      <div className="pr-stage">
        <svg className="pp-svg" viewBox={`${x0} ${y0} ${x1 - x0} ${y1 - y0}`} role="img" aria-label="Preview of the page" style={{ "--k": k } as React.CSSProperties}>
          <rect x={0} y={0} width={size[0]} height={size[1]} fill="#fff" />
          {url && <image href={url} x={0} y={0} width={size[0]} height={size[1]} />}
          {children}
        </svg>
      </div>
      <div className="pr-pager">
        <span className="dim pp-caption">{caption ?? ""}</span>
      </div>
    </div>
  );
}

// ---- Convert Colors ----

function ConvertView({ sheet }: { sheet: ConvertSheet }) {
  const d = useStore((s) => s.docs[sheet.doc]);
  if (!d) return null;
  const o = sheet.opts;
  const set = act().setConvert;
  const running = sheet.phase === "running";
  const locked = useLocked(sheet.doc)?.certified === true;
  const problem = convertProblem(o, d.pageCount, d.currentPage);
  return (
    <section className="pr-sheet pp-sheet" role="dialog" aria-label="Convert Colors" onMouseDown={(e) => e.stopPropagation()}>
      <Head icon="swatches" title="Convert Colors" doc={d.name} />
      <div className="pr-body pp-sheet-body">
        {sheet.phase === "options" && (
          <fieldset className="pp-lock" disabled={locked}>
            <Field label="Convert to CMYK">
              <Check label="RGB colours and pictures" checked={o.rgb} onChange={(rgb) => set({ rgb })} />
              <Check label="Gray" checked={o.gray} onChange={(gray) => set({ gray })} />
              <Check label="Spot colours (their tint becomes process ink)" checked={o.spot} onChange={(spot) => set({ spot })} />
            </Field>
            <Check label="Gray becomes black ink only" checked={o.grayToK} disabled={!o.gray} onChange={(grayToK) => set({ grayToK })} />
            <Check label="Keep pure black as black ink only" checked={o.preserveBlack} onChange={(preserveBlack) => set({ preserveBlack })} />
            <Field label="Conversion">
              <select className="tl-field" aria-label="Conversion" value={o.intent} onChange={(e) => set({ intent: e.target.value as typeof o.intent })}>
                {(Object.keys(INTENT_LABEL) as (keyof typeof INTENT_LABEL)[]).map((k) => (
                  <option key={k} value={k}>
                    {INTENT_LABEL[k]}
                  </option>
                ))}
              </select>
            </Field>
            {o.intent === "profile" && (
              <div className="pr-pair">
                <button className="btn ghost" onClick={() => void act().pickProfile()}>
                  {o.profile ? "Choose another profile…" : "Choose a profile…"}
                </button>
                <span className="dim">{o.profile?.name ?? "No file chosen"}</span>
              </div>
            )}
            {o.intent === "profile" && (
              <Field label="Output condition">
                <input className="tl-field" aria-label="Output condition" placeholder={o.profile ? o.profile.name.replace(/\.(icc|icm)$/i, "") : "Name of the printing condition"} value={o.condition} onChange={(e) => set({ condition: e.target.value })} />
              </Field>
            )}
            <Scope scope={o.scope} range={o.range} count={d.pageCount} page={d.currentPage} set={(v) => set(v)} />
            <Field label="Result">
              <Seg
                label="Result"
                value={o.output}
                items={[
                  { id: "newFile", label: "New file" },
                  { id: "inPlace", label: "This document" },
                ]}
                onChange={(output) => set({ output })}
              />
            </Field>
          </fieldset>
        )}
        {sheet.phase === "confirm" && (
          <>
            <p className="pp-lead">This will change the document:</p>
            <ul className="pp-summary">
              {convertSummary(o, d.pageCount, d.currentPage, d.dirty).map((t, i) => (
                <li key={i}>{t}</li>
              ))}
            </ul>
          </>
        )}
        {running && (
          <div className="pr-state">
            <progress className="pp-progress" value={sheet.progress} max={1} aria-label="Converting" />
            <p className="dim">Converting colours… {Math.round(sheet.progress * 100)}%</p>
          </div>
        )}
        {sheet.phase === "done" && sheet.result && (
          <div className="pr-state">
            <p className="pp-lead">
              Converted {sheet.result.report.pages.length === 1 ? "1 page" : `${sheet.result.report.pages.length} pages`}.
              {sheet.result.path ? ` Saved as ${sheet.result.path.split(/[\\/]/).pop()}.` : " One undo step; Save writes it."}
            </p>
            <p>{remainingText(sheet.result.report)}</p>
            {sheet.result.report.usedProfile && <p>The profile is now the file's output intent.</p>}
            {sheet.result.report.notes.map((n, i) => (
              <p key={i} className="dim">
                {n}
              </p>
            ))}
          </div>
        )}
        {sheet.phase === "unsaved" && sheet.result && (
          <div className="pr-state">
            <p className="pp-lead">Converted, but not saved as a new file.</p>
            <p>{remainingText(sheet.result.report)}</p>
            {sheet.closing && <p className="pr-note bad">The open document holds the converted colours, and Save would write them over the original file. Undo the conversion, or keep it?</p>}
          </div>
        )}
        {sheet.phase !== "running" && <Protect doc={sheet.doc} />}
        {sheet.error && <p className="pr-note bad">{sheet.error}</p>}
      </div>
      <footer className="pr-foot">
        <span className="grow" />
        {sheet.phase === "options" && (
          <>
            <button className="btn ghost" onClick={() => act().closeSheet()}>
              Cancel
            </button>
            <button className="btn primary" disabled={!!problem || locked} title={locked ? CERTIFIED_REASON : undefined} onClick={() => act().review()}>
              Review…
            </button>
          </>
        )}
        {sheet.phase === "confirm" && (
          <>
            <button className="btn ghost" onClick={() => act().backToOptions()}>
              Back
            </button>
            <button className="btn primary" disabled={locked} onClick={() => void act().runConvert()}>
              {o.output === "newFile" ? "Convert and save as…" : "Convert"}
            </button>
          </>
        )}
        {running && (
          <button className="btn ghost" onClick={() => act().cancelConvert()}>
            Cancel
          </button>
        )}
        {sheet.phase === "unsaved" && !sheet.closing && (
          <>
            <button className="btn ghost" onClick={() => act().closeSheet()}>
              Close
            </button>
            <button className="btn primary" onClick={() => void act().saveConverted()}>
              Save as…
            </button>
          </>
        )}
        {sheet.phase === "unsaved" && sheet.closing && (
          <>
            <button className="btn ghost" onClick={() => act().keepConversion()}>
              Keep
            </button>
            <button className="btn primary" onClick={() => void act().undoConversion()}>
              Undo conversion
            </button>
          </>
        )}
        {sheet.phase === "done" && (
          <button className="btn primary" onClick={() => act().closeSheet()}>
            Close
          </button>
        )}
      </footer>
    </section>
  );
}

// ---- Printer's marks ----

function MarksView({ sheet }: { sheet: MarksSheet }) {
  const d = useStore((s) => s.docs[sheet.doc]);
  const f = sheet.form;
  const paths = useMemo<MarkPath[]>(() => (sheet.preview ? parseMarks(sheet.preview.data.content) : []), [sheet.preview]);
  const locked = useLocked(sheet.doc)?.certified === true;
  const size = d?.sizes[sheet.page];
  // an undo took the page away; the sheet is closed in a moment
  if (!d || !size) return null;
  const b = sheet.boxes;
  const crop = b ? visibleBox(b) : null;
  const rot = b?.rotate ?? 0;
  const p = sheet.preview?.data ?? null;
  const media = p && crop ? rectShown(p.media, crop, rot) : ([0, 0, size[0], size[1]] as Rect4);
  const kinds: { id: keyof typeof f.marks; label: string }[] = [
    { id: "crop", label: "Crop marks" },
    { id: "bleed", label: "Bleed marks" },
    { id: "registration", label: "Registration marks" },
    { id: "colorBars", label: "Colour bars" },
  ];
  const nothing = !Object.values(f.marks).some(Boolean);
  const offsetPt = f.offset ?? 9;
  return (
    <section className="pr-sheet" role="dialog" aria-label="Printer's Marks" onMouseDown={(e) => e.stopPropagation()}>
      <Head icon="crop" title="Printer's Marks" doc={d.name} />
      <div className="pr-main">
        <PagePreview doc={sheet.doc} page={sheet.page} size={size} rev={d.rev} extent={media} caption={p && b ? marksCaption(p, b.media, f.units) : nothing ? "No marks chosen." : "Drawing the marks…"}>
          {crop && p && (
            <>
              <rect className="pp-media" x={media[0]} y={media[1]} width={media[2] - media[0]} height={media[3] - media[1]} />
              <Outline r={rectShown(p.bleed, crop, rot)} cls="pp-bleed" />
              <Outline r={rectShown(p.trim, crop, rot)} cls="pp-trim" />
              {paths.map((m, i) =>
                m.kind === "stroke" ? (
                  <path key={i} d={svgPath(m.points, crop, rot, false)} fill="none" stroke="#000" strokeWidth={Math.max(m.width, 0.5)} />
                ) : (
                  <path key={i} d={svgPath(m.points, crop, rot, true)} fill={m.cmyk ? cmykCss(m.cmyk) : "#000"} />
                ),
              )}
            </>
          )}
        </PagePreview>
        <div className="pr-side">
          <div className="pr-body">
            <fieldset className="pp-lock" disabled={locked}>
            <Field label="Marks">
              {kinds.map((k) => (
                <Check key={k.id} label={k.label} checked={f.marks[k.id]} disabled={sheet.busy} onChange={(on) => act().setMarkKind(k.id, on)} />
              ))}
            </Field>
            <Field label="Line weight">
              <select className="tl-field" aria-label="Line weight" value={f.weight} disabled={sheet.busy} onChange={(e) => act().setMarks({ weight: Number(e.target.value) })}>
                {[...new Set([...WEIGHTS, f.weight])].sort((a, c) => a - c).map((w) => (
                  <option key={w} value={w}>
                    {w} pt
                  </option>
                ))}
              </select>
            </Field>
            <div className="pr-pair">
              <Length label="Mark length" pt={f.length} units={f.units} disabled={sheet.busy} onPt={(length) => length > 0 && act().setMarks({ length })} />
              <Length label="Distance from trim" pt={offsetPt} units={f.units} disabled={sheet.busy} onPt={(offset) => offset >= 0 && act().setMarks({ offset })} />
            </div>
            <div className="pr-pair">
              <UnitSelect units={f.units} onChange={(units) => act().setMarks({ units })} />
              {f.offset !== null && (
                <button className="a11y-quiet" onClick={() => act().setMarks({ offset: null })}>
                  Distance: automatic
                </button>
              )}
            </div>
            <Scope scope={f.scope} range={f.range} count={d.pageCount} page={sheet.page} disabled={sheet.busy} set={(v) => act().setMarks(v)} />
            </fieldset>
            <Protect doc={sheet.doc} />
            {sheet.preview === null && !nothing && !sheet.error && <p className="pr-note dim">Reading the page…</p>}
            {sheet.error && <p className="pr-note bad">{sheet.error}</p>}
            {sheet.done && <p className="pr-note">{sheet.done}</p>}
          </div>
          <footer className="pr-foot">
            <span className="grow" />
            <button className="btn ghost" disabled={sheet.busy} onClick={() => act().closeSheet()}>
              {sheet.done ? "Close" : "Cancel"}
            </button>
            <button className="btn primary" disabled={sheet.busy || nothing || locked} title={locked ? CERTIFIED_REASON : undefined} onClick={() => void act().applyMarks()}>
              Add marks
            </button>
          </footer>
        </div>
      </div>
    </section>
  );
}

const Outline = ({ r, cls }: { r: Rect4; cls: string }) => <rect className={cls} x={r[0]} y={r[1]} width={r[2] - r[0]} height={r[3] - r[1]} />;

// ---- Page boxes ----

function BoxesView({ sheet }: { sheet: BoxesSheet }) {
  const d = useStore((s) => s.docs[sheet.doc]);
  const locked = useLocked(sheet.doc)?.certified === true;
  const size = d?.sizes[sheet.page];
  // an undo took the page away; the sheet is closed in a moment
  if (!d || !size) return null;
  const b = sheet.boxes;
  const crop = b ? visibleBox(b) : null;
  const rot = b?.rotate ?? 0;
  const box = newBox(sheet);
  const problem = b && box ? boxProblem(sheet.box, box, b) : null;
  const shown = (r: Rect4) => (crop ? rectShown(r, crop, rot) : r);
  const u = sheet.units;
  const sides = [
    ["left", "Left"],
    ["top", "Top"],
    ["right", "Right"],
    ["bottom", "Bottom"],
  ] as const;
  const touched = !!b && sheet.margins && (["left", "bottom", "right", "top"] as const).some((s) => Math.abs(sheet.margins[s]) > 1e-6);
  const extent = box && crop ? shown(box) : ([0, 0, size[0], size[1]] as Rect4);
  return (
    <section className="pr-sheet" role="dialog" aria-label="Page Boxes" onMouseDown={(e) => e.stopPropagation()}>
      <Head icon="pagebox" title="Page Boxes" doc={d.name} />
      <div className="pr-main">
        <PagePreview
          doc={sheet.doc}
          page={sheet.page}
          size={size}
          rev={d.rev}
          extent={extent}
          caption={box ? `${BOX_LABEL[sheet.box]}: ${showLength(box[2] - box[0], u)} x ${showLength(box[3] - box[1], u)} ${u}` : "Reading the page…"}
        >
          {b && crop && (
            <>
              {(["media", "crop", "art", "trim", "bleed"] as const)
                .filter((k) => k !== sheet.box && (k === "media" || b.stated[k as BoxKind]))
                .map((k) => (
                  <Outline key={k} r={shown(boxOf(b, k))} cls={`pp-box pp-${k}`} />
                ))}
              {box && <Outline r={shown(box)} cls="pp-box pp-new" />}
            </>
          )}
        </PagePreview>
        <div className="pr-side">
          <div className="pr-body">
            <fieldset className="pp-lock" disabled={locked}>
            <Field label="Set the">
              <select className="tl-field" aria-label="Box to set" value={sheet.box} disabled={sheet.busy} onChange={(e) => act().setBox(e.target.value as BoxKind)}>
                {BOX_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {BOX_LABEL[k]}
                    {b && !b.stated[k] ? " (not set)" : ""}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Measured from">
              <select className="tl-field" aria-label="Measured from" value={sheet.ref} disabled={sheet.busy} onChange={(e) => act().setRef(e.target.value as typeof sheet.ref)}>
                {refsFor(sheet.box).map((k) => (
                  <option key={k} value={k}>
                    {BOX_LABEL[k]}
                  </option>
                ))}
              </select>
            </Field>
            <div className="pr-pair">
              <UnitSelect units={u} onChange={(units) => act().setBoxes({ units })} />
              <Check label="Same on all sides" checked={sheet.linked} disabled={sheet.busy} onChange={(linked) => act().setBoxes({ linked })} />
            </div>
            <span className="pr-label">Margin from {BOX_LABEL[sheet.ref].toLowerCase()}</span>
            <div className="pp-margins">
              {sides.map(([side, label]) => (
                <Length key={side} label={label} pt={sheet.margins[side]} units={u} disabled={sheet.busy} onPt={(pt) => act().setMargin(side, pt)} />
              ))}
            </div>
            <div className="pr-pair">
              <button className="btn ghost" disabled={sheet.busy} onClick={() => act().bleedPreset()}>
                Bleed 3 mm
              </button>
              <button className="a11y-quiet" disabled={sheet.busy || !touched} onClick={() => act().revertMargins()}>
                Back to what the page has
              </button>
            </div>
            <Scope scope={sheet.scope} range={sheet.range} count={d.pageCount} page={sheet.page} disabled={sheet.busy} set={(v) => act().setBoxes(v)} />
            </fieldset>
            <Protect doc={sheet.doc} />
            {problem && <p className="pr-note bad">{problem}</p>}
            {sheet.error && <p className="pr-note bad">{sheet.error}</p>}
            {sheet.done && <p className="pr-note">{sheet.done}</p>}
            {sheet.scope !== "page" && d.pageCount > 1 && <p className="pr-note dim">Each page measures from its own {BOX_LABEL[sheet.ref].toLowerCase()}.</p>}
          </div>
          <footer className="pr-foot">
            <span className="grow" />
            <button className="btn ghost" disabled={sheet.busy} onClick={() => act().closeSheet()}>
              {sheet.done ? "Close" : "Cancel"}
            </button>
            <button className="btn primary" disabled={sheet.busy || !b || !!problem || locked} title={locked ? CERTIFIED_REASON : undefined} onClick={() => void act().applyBoxes()}>
              Set {BOX_LABEL[sheet.box].toLowerCase()}
            </button>
          </footer>
        </div>
      </div>
    </section>
  );
}
