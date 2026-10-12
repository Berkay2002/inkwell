import { useEffect, useRef } from "react";
import type { PrintDuplex, PrintOrientation, PrintPageOrder } from "../lib/api";
import { outcomeLine, paperList, PER_SHEET, type PrintOptions, type PrintSheetState, usePrint } from "../lib/printStore";
import { Icon } from "./Icons";
import "./print.css";

const act = () => usePrint.getState();

function Seg<T extends string>({
  label,
  value,
  items,
  onChange,
  disabled,
}: {
  label: string;
  value: T;
  items: { id: T; label: string }[];
  onChange: (v: T) => void;
  disabled?: boolean;
}) {
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

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="pr-field">
      <span className="pr-label">{label}</span>
      {children}
    </div>
  );
}

const pagesLabel = (n: number[]) =>
  n.length === 0 ? "blank" : n.length > 4 ? `pages ${n[0]} to ${n[n.length - 1]}` : `${n.length === 1 ? "page" : "pages"} ${n.join(", ")}`;

/** The Print sheet: the page as it will come out on the left, the options on the right. */
export function PrintSheet() {
  const sheet = usePrint((s) => s.sheet);
  const open = sheet != null;
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        act().close();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open]);
  if (!sheet) return null;
  return (
    <div className="scrim pr-scrim" onMouseDown={() => sheet.phase !== "running" && act().close()}>
      <section className="pr-sheet" role="dialog" aria-label="Print" onMouseDown={(e) => e.stopPropagation()}>
        <header className="pr-head">
          <Icon name="print" size={16} />
          <span className="pr-title">Print</span>
          <span className="pr-doc dim">{sheet.name}</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Print" onClick={() => act().close()}>
            <Icon name="close" size={14} />
          </button>
        </header>
        <div className="pr-main">
          <Preview sheet={sheet} />
          <div className="pr-side">
            {sheet.phase === "options" && <Options sheet={sheet} />}
            {sheet.phase === "running" && <Running sheet={sheet} />}
            {sheet.phase === "done" && <Done sheet={sheet} />}
            {sheet.phase === "error" && <Failed sheet={sheet} />}
          </div>
        </div>
      </section>
    </div>
  );
}

function Preview({ sheet }: { sheet: PrintSheetState }) {
  const box = useRef<HTMLDivElement>(null);
  const { plan, face } = sheet;
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => act().setPreviewPx(Math.max(240, Math.round(Math.max(el.clientWidth, el.clientHeight)))));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const f = plan?.faces[face];
  const n = plan?.sheets ?? 0;
  return (
    <div className="pr-preview">
      <div className="pr-stage" ref={box}>
        {sheet.previewUrl && f && (
          <img
            className={sheet.previewing ? "pr-paper busy" : "pr-paper"}
            src={sheet.previewUrl}
            alt={`Preview of side ${face + 1}`}
            style={{ aspectRatio: `${f.width} / ${f.height}` }}
          />
        )}
        {sheet.planError && <p className="pr-note bad">{sheet.planError}</p>}
        {!sheet.previewUrl && !sheet.planError && <p className="pr-note dim">Preparing the preview…</p>}
      </div>
      <div className="pr-pager">
        <button className="ax-icon" aria-label="Previous side" disabled={face <= 0} onClick={() => act().setFace(face - 1)}>
          <Icon name="up" size={14} />
        </button>
        <span className="pr-pager-text">{plan ? `${face + 1} of ${n}` : ""}</span>
        <button className="ax-icon" aria-label="Next side" disabled={face >= n - 1} onClick={() => act().setFace(face + 1)}>
          <Icon name="down" size={14} />
        </button>
        <span className="dim pr-pager-pages">{f ? pagesLabel(f.pages) : ""}</span>
      </div>
    </div>
  );
}

const ORIENTATIONS: { id: PrintOrientation; label: string }[] = [
  { id: "auto", label: "Automatic" },
  { id: "portrait", label: "Portrait" },
  { id: "landscape", label: "Landscape" },
];

const ORDERS: { id: PrintPageOrder; label: string }[] = [
  { id: "horizontal", label: "Across, then down" },
  { id: "horizontalReversed", label: "Across, right to left" },
  { id: "vertical", label: "Down, then across" },
  { id: "verticalReversed", label: "Down, right to left" },
];

function Options({ sheet }: { sheet: PrintSheetState }) {
  const o = sheet.options;
  const set = (p: Partial<PrintOptions>) => act().set(p);
  const caps = sheet.caps;
  const perSheetPreset = (PER_SHEET as readonly number[]).includes(o.perSheet);
  const outcome = outcomeLine(sheet.plan, o, caps);
  const papers = paperList(caps, o.printer);
  const canRun = !!sheet.plan && !sheet.planError;
  return (
    <>
      <div className="pr-body">
        <Field label="Printer">
          <select className="tl-field" aria-label="Printer" value={o.printer ?? ""} onChange={(e) => set({ printer: e.target.value })}>
            {sheet.printers.length === 0 && <option value="">No printer installed</option>}
            {sheet.printers.map((p) => (
              <option key={p.name} value={p.name}>
                {p.name}
                {p.isDefault ? " (default)" : ""}
              </option>
            ))}
          </select>
        </Field>

        <div className="pr-pair">
          <Field label="Copies">
            <input
              className="tl-field"
              type="number"
              min={1}
              max={caps && caps.maxCopies > 1 ? caps.maxCopies : 999}
              aria-label="Copies"
              value={o.copies}
              onChange={(e) => set({ copies: Math.max(1, Number(e.target.value) || 1) })}
            />
          </Field>
          <label className="pr-check">
            <input type="checkbox" checked={o.collate} disabled={o.copies < 2} onChange={(e) => set({ collate: e.target.checked })} />
            <span>Collate</span>
          </label>
        </div>

        <Field label="Pages">
          <Seg
            label="Pages to print"
            value={o.scope}
            items={[
              { id: "all", label: `All ${sheet.pageCount}` },
              { id: "current", label: `Page ${sheet.currentPage + 1}` },
              { id: "range", label: "Range" },
            ]}
            onChange={(scope) => set({ scope })}
          />
        </Field>
        {o.scope === "range" && (
          <input
            className={sheet.planError ? "tl-field bad" : "tl-field"}
            aria-label="Page range"
            placeholder="1-3, 5, 8-"
            spellCheck={false}
            autoFocus
            value={o.range}
            onChange={(e) => set({ range: e.target.value })}
          />
        )}
        <div className="pr-pair">
          <select className="tl-field" aria-label="Odd or even pages" value={o.subset} onChange={(e) => set({ subset: e.target.value as PrintOptions["subset"] })}>
            <option value="all">All pages in range</option>
            <option value="odd">Odd pages only</option>
            <option value="even">Even pages only</option>
          </select>
          <label className="pr-check">
            <input type="checkbox" checked={o.reverse} onChange={(e) => set({ reverse: e.target.checked })} />
            <span>Reverse order</span>
          </label>
        </div>

        <Field label="Pages per sheet">
          <Seg
            label="Layout"
            value={o.mode}
            items={[
              { id: "size", label: "One" },
              { id: "multiple", label: "Several" },
              { id: "booklet", label: "Booklet" },
              { id: "poster", label: "Poster" },
            ]}
            onChange={(mode) => set({ mode })}
          />
        </Field>
        {o.mode === "size" && (
          <>
            <Seg
              label="Scaling"
              value={o.sizeMode}
              items={[
                { id: "fit", label: "Fit" },
                { id: "shrink", label: "Shrink" },
                { id: "actual", label: "Actual" },
                { id: "custom", label: "Custom" },
              ]}
              onChange={(sizeMode) => set({ sizeMode })}
            />
            {o.sizeMode === "custom" && (
              <div className="pr-pair">
                <input
                  className="tl-field"
                  type="number"
                  min={1}
                  max={1000}
                  aria-label="Scale in percent"
                  value={o.percent}
                  onChange={(e) => set({ percent: Math.max(1, Math.min(1000, Number(e.target.value) || 100)) })}
                />
                <span className="dim">% of actual size</span>
              </div>
            )}
          </>
        )}
        {o.mode === "multiple" && (
          <>
            <div className="pr-pair">
              <select
                className="tl-field"
                aria-label="Pages per sheet"
                value={perSheetPreset ? o.perSheet : "custom"}
                onChange={(e) => e.target.value !== "custom" && set({ perSheet: Number(e.target.value) })}
              >
                {PER_SHEET.map((n) => (
                  <option key={n} value={n}>
                    {n} per sheet
                  </option>
                ))}
                {!perSheetPreset && <option value="custom">{o.perSheet} per sheet</option>}
              </select>
              <input
                className="tl-field"
                type="number"
                min={2}
                max={100}
                aria-label="Custom pages per sheet"
                value={o.perSheet}
                onChange={(e) => set({ perSheet: Math.max(2, Math.min(100, Number(e.target.value) || 2)) })}
              />
            </div>
            <select className="tl-field" aria-label="Page order" value={o.order} onChange={(e) => set({ order: e.target.value as PrintPageOrder })}>
              {ORDERS.map((x) => (
                <option key={x.id} value={x.id}>
                  {x.label}
                </option>
              ))}
            </select>
            <label className="pr-check">
              <input type="checkbox" checked={o.border} onChange={(e) => set({ border: e.target.checked })} />
              <span>Draw a border around each page</span>
            </label>
            <label className="pr-check">
              <input type="checkbox" checked={o.autoRotate} onChange={(e) => set({ autoRotate: e.target.checked })} />
              <span>Turn pages to fit</span>
            </label>
          </>
        )}
        {o.mode === "booklet" && (
          <>
            <Seg
              label="Booklet sides"
              value={o.sides}
              items={[
                { id: "both", label: "Both sides" },
                { id: "frontOnly", label: "Front only" },
                { id: "backOnly", label: "Back only" },
              ]}
              onChange={(sides) => set({ sides })}
            />
            <Seg
              label="Binding"
              value={o.binding}
              items={[
                { id: "left", label: "Left" },
                { id: "right", label: "Right" },
              ]}
              onChange={(binding) => set({ binding })}
            />
            <div className="pr-pair">
              <input
                className="tl-field"
                type="number"
                min={0}
                max={3}
                step={0.1}
                aria-label="Creep in millimetres"
                value={o.creepMm}
                onChange={(e) => set({ creepMm: Math.max(0, Math.min(3, Number(e.target.value) || 0)) })}
              />
              <span className="dim">mm creep, for thick paper</span>
            </div>
          </>
        )}
        {o.mode === "poster" && (
          <>
            <div className="pr-pair">
              <input
                className="tl-field"
                type="number"
                min={1}
                max={1000}
                aria-label="Tile scale in percent"
                value={o.tileScale}
                onChange={(e) => set({ tileScale: Math.max(1, Math.min(1000, Number(e.target.value) || 100)) })}
              />
              <span className="dim">% tile scale</span>
            </div>
            <div className="pr-pair">
              <input
                className="tl-field"
                type="number"
                min={0}
                max={50}
                aria-label="Overlap in millimetres"
                value={o.overlapMm}
                onChange={(e) => set({ overlapMm: Math.max(0, Math.min(50, Number(e.target.value) || 0)) })}
              />
              <span className="dim">mm overlap</span>
            </div>
            <label className="pr-check">
              <input type="checkbox" checked={o.cutMarks} onChange={(e) => set({ cutMarks: e.target.checked })} />
              <span>Cut marks at the overlap</span>
            </label>
            <label className="pr-check">
              <input type="checkbox" checked={o.labels} onChange={(e) => set({ labels: e.target.checked })} />
              <span>Label each tile with its row, column and page</span>
            </label>
          </>
        )}

        <Field label="Paper">
          <select
            className="tl-field"
            aria-label="Paper size"
            value={o.paperId ?? ""}
            disabled={papers.length === 0}
            onChange={(e) => set({ paperId: Number(e.target.value) })}
          >
            {papers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
            {papers.length === 0 && <option value="">Printer default</option>}
          </select>
        </Field>
        {caps && caps.trays.length > 1 && (
          <select
            className="tl-field"
            aria-label="Paper tray"
            value={o.tray ?? ""}
            onChange={(e) => set({ tray: e.target.value === "" ? null : Number(e.target.value) })}
          >
            <option value="">Select the tray automatically</option>
            {caps.trays.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
              </option>
            ))}
          </select>
        )}
        <Seg label="Orientation" value={o.orientation} items={ORIENTATIONS} onChange={(orientation) => set({ orientation })} />

        <Field label="Colour">
          <Seg
            label="Colour"
            value={o.grayscale ? "gray" : "color"}
            items={[
              { id: "color", label: "Colour" },
              { id: "gray", label: "Grayscale" },
            ]}
            onChange={(v) => set({ grayscale: v === "gray" })}
            disabled={!o.printer}
          />
          <span className="dim">Save as PDF always keeps colour.</span>
        </Field>
        {(!caps || caps.duplex) && (
          <Field label="Two-sided">
            <select
              className="tl-field"
              aria-label="Two-sided printing"
              value={o.duplex}
              onChange={(e) => set({ duplex: e.target.value as PrintDuplex })}
            >
              <option value="off">One side only</option>
              <option value="longEdge">Both sides, flip on long edge</option>
              <option value="shortEdge">Both sides, flip on short edge</option>
            </select>
          </Field>
        )}

        <Field label="Comments and forms">
          <select
            className="tl-field"
            aria-label="Comments and forms"
            value={o.content}
            onChange={(e) => set({ content: e.target.value as PrintOptions["content"] })}
          >
            <option value="document">Document</option>
            <option value="documentAndMarkups">Document and markups</option>
            <option value="formFieldsOnly">Form fields only</option>
          </select>
        </Field>
      </div>
      <footer className="pr-foot">
        <span className="pr-outcome" aria-live="polite">
          {outcome}
        </span>
        <span className="grow" />
        <button className="btn ghost" onClick={() => act().close()}>
          Cancel
        </button>
        <button className="btn ghost" disabled={!canRun} onClick={() => void act().savePdf()}>
          Save as PDF…
        </button>
        <button className="btn primary" disabled={!canRun || !o.printer} onClick={() => void act().start()}>
          Print
        </button>
      </footer>
    </>
  );
}

function Running({ sheet }: { sheet: PrintSheetState }) {
  const { done, total } = sheet.progress;
  return (
    <div className="pr-state">
      <p className="tl-big">{sheet.job === "pdf" ? "Writing the PDF…" : "Printing…"}</p>
      <div className="tl-track" role="progressbar" aria-valuemin={0} aria-valuemax={total} aria-valuenow={done}>
        <span style={{ width: `${total ? (100 * done) / total : 0}%` }} />
      </div>
      <p className="dim">
        {sheet.job === "pdf" ? "Page" : "Side"} {Math.min(done + 1, total)} of {total}
      </p>
      <div className="pr-actions">
        <button className="btn ghost" onClick={() => act().cancel()}>
          {sheet.job === "pdf" ? "Cancel" : "Cancel printing"}
        </button>
      </div>
    </div>
  );
}

function Done({ sheet }: { sheet: PrintSheetState }) {
  const r = sheet.report;
  const saved = sheet.saved;
  if (saved) {
    return (
      <div className="pr-state">
        <p className="tl-big">Saved {saved.file.path.split(/[\\/]/).pop()}</p>
        <p className="dim">
          {saved.sheets} {saved.sheets === 1 ? "page" : "pages"} from {saved.pages} {saved.pages === 1 ? "page" : "pages"} of {sheet.name}. The original is untouched.
        </p>
        <div className="pr-actions">
          <button className="btn ghost" onClick={() => act().close()}>
            Done
          </button>
          <button className="btn primary" onClick={() => void act().openSaved()}>
            Open it
          </button>
        </div>
      </div>
    );
  }
  return (
    <div className="pr-state">
      <p className="tl-big">Sent to {r?.printer}</p>
      <p className="dim">
        {r?.pages} {r?.pages === 1 ? "page" : "pages"}, {r?.sides} {r?.sides === 1 ? "side" : "sides"}
        {sheet.savedAs ? `, saved as ${sheet.savedAs.split(/[\\/]/).pop()}` : ""}
      </p>
      <div className="pr-actions">
        <button className="btn primary" onClick={() => act().close()}>
          Done
        </button>
      </div>
    </div>
  );
}

function Failed({ sheet }: { sheet: PrintSheetState }) {
  return (
    <div className="pr-state">
      <p className="tl-big">{sheet.job === "pdf" ? "Could not save the PDF" : "Could not print"}</p>
      <p className="pr-note bad">{sheet.error}</p>
      <div className="pr-actions">
        <button className="btn ghost" onClick={() => act().back()}>
          Back to options
        </button>
      </div>
    </div>
  );
}
