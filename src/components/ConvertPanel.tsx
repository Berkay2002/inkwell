import { useAi } from "../lib/aiStore";
import { type ConvertSheet, type CreateFrom, type ExportWhat, isFolderExport, readiness, type Scope, useConvert } from "../lib/convertStore";
import type { CreatePageSize, ImageFormat } from "../lib/api";
import { formatBytes } from "../lib/toolsStore";
import type { DocState } from "../store";
import { Icon } from "./Icons";
import "./tools.css";

const st = () => useConvert.getState();
const base = (p: string) => p.split(/[\\/]/).pop() ?? p;

/**
 * Export and Create PDF in the same right-hand rail as Compress and Recognize text. `doc` is the active
 * document (null on Home, where only Create is offered).
 */
export function ConvertPanel({ doc }: { doc: DocState | null }) {
  const sheet = useConvert((s) => s.sheet);
  const width = useAi((s) => s.settings.width);
  if (!sheet || (sheet.mode === "export" && sheet.doc !== doc?.id)) return null;
  const title = sheet.mode === "export" ? "Export" : "Create PDF";
  return (
    <aside className="ax-dock" style={{ width: `min(${width}px, 45vw)` }} aria-label={title}>
      <section className="ax-panel docked tl">
        <div className="ax-head">
          <span className="ax-title">{title}</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label={`Close ${title}`} onClick={() => st().close()}>
            <Icon name="close" size={14} />
          </button>
        </div>
        {sheet.phase === "options" && (sheet.mode === "export" ? <ExportOptions sheet={sheet} doc={doc!} /> : <CreateOptions sheet={sheet} />)}
        {sheet.phase === "running" && <Running sheet={sheet} />}
        {sheet.phase === "done" && <Done sheet={sheet} />}
        {sheet.phase === "error" && <Failed sheet={sheet} />}
      </section>
    </aside>
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

const WHAT: { id: ExportWhat; label: string; desc: string }[] = [
  { id: "images", label: "Page images", desc: "PNG, JPEG or TIFF, one file per page" },
  { id: "text", label: "Text", desc: "Plain or layout-preserved .txt" },
  { id: "html", label: "Web page", desc: "HTML or XHTML" },
  { id: "extractImages", label: "Embedded images", desc: "The pictures inside the PDF" },
  { id: "word", label: "Word", desc: "A flowing .docx, not layout-exact" },
  { id: "rtf", label: "Rich Text", desc: "A flowing .rtf, not layout-exact" },
];

const SCOPES: { id: Scope; label: string }[] = [
  { id: "all", label: "All pages" },
  { id: "page", label: "This page" },
  { id: "range", label: "Range" },
];

const DPIS = [72, 150, 300, 600];

function ExportOptions({ sheet, doc }: { sheet: ConvertSheet; doc: DocState }) {
  const why = readiness(sheet, doc.pageCount);
  const set = (p: Partial<ConvertSheet>) => st().set(p);
  return (
    <>
      <div className="ax-body tl-body">
        <label className="tl-label">Export as</label>
        <div className="tl-presets" role="radiogroup" aria-label="Export as">
          {WHAT.map((w) => (
            <button key={w.id} role="radio" aria-checked={sheet.what === w.id} className={sheet.what === w.id ? "tl-preset on" : "tl-preset"} onClick={() => set({ what: w.id })}>
              <span className="tl-radio" />
              <span className="tl-preset-text">
                <span className="tl-preset-name">{w.label}</span>
                <span className="dim small">{w.desc}</span>
              </span>
            </button>
          ))}
        </div>

        {sheet.what === "images" && (
          <>
            <label className="tl-label">Format</label>
            <Seg<ImageFormat>
              label="Image format"
              value={sheet.imageFormat}
              items={[
                { id: "png", label: "PNG" },
                { id: "jpeg", label: "JPEG" },
                { id: "tiff", label: "TIFF" },
              ]}
              onChange={(imageFormat) => set({ imageFormat })}
            />
            <label className="tl-label">Resolution</label>
            <div className="tl-row">
              <select className="tl-field" aria-label="Resolution" value={DPIS.includes(sheet.dpi) ? sheet.dpi : "custom"} onChange={(e) => e.target.value !== "custom" && set({ dpi: Number(e.target.value) })}>
                {DPIS.map((d) => (
                  <option key={d} value={d}>
                    {d} dpi
                  </option>
                ))}
                {!DPIS.includes(sheet.dpi) && <option value="custom">{sheet.dpi} dpi</option>}
              </select>
              <input
                className={sheet.dpi >= 36 && sheet.dpi <= 1200 ? "tl-field tl-num" : "tl-field tl-num bad"}
                type="number"
                min={36}
                max={1200}
                aria-label="Custom resolution in dpi"
                value={sheet.dpi}
                onChange={(e) => set({ dpi: Number(e.target.value) })}
              />
            </div>
            {sheet.imageFormat === "jpeg" && (
              <>
                <label className="tl-label">Quality {sheet.quality}</label>
                <input type="range" min={30} max={100} value={sheet.quality} aria-label="JPEG quality" onChange={(e) => set({ quality: Number(e.target.value) })} />
              </>
            )}
            <label className="tl-switch">
              <input type="checkbox" checked={sheet.gray} onChange={(e) => set({ gray: e.target.checked })} />
              <span>Grayscale</span>
            </label>
          </>
        )}
        {sheet.what === "text" && (
          <label className="tl-switch">
            <input type="checkbox" checked={sheet.layout} onChange={(e) => set({ layout: e.target.checked })} />
            <span>Keep the page layout (columns, spacing)</span>
          </label>
        )}
        {sheet.what === "html" && (
          <label className="tl-switch">
            <input type="checkbox" checked={sheet.xhtml} onChange={(e) => set({ xhtml: e.target.checked })} />
            <span>XHTML instead of HTML</span>
          </label>
        )}

        <label className="tl-label">Pages</label>
        <Seg<Scope> label="Pages" value={sheet.scope} items={SCOPES} onChange={(scope) => set({ scope })} />
        {sheet.scope === "range" && (
          <input
            className={why ? "tl-field bad" : "tl-field"}
            value={sheet.range}
            placeholder={`1-${doc.pageCount}, e.g. 1-3, 5`}
            aria-label="Page range"
            spellCheck={false}
            onChange={(e) => set({ range: e.target.value })}
          />
        )}
        {sheet.scope === "page" && <p className="dim small">Page {doc.currentPage + 1}</p>}
        <p className="dim small">
          {isFolderExport(sheet.what) ? "You choose a folder; files are named after the document and never overwrite others." : "You choose where the file goes."} Unsaved edits are included.
        </p>
      </div>
      <div className="tl-foot">
        {why && sheet.scope === "range" && <p className="dim small">{why}</p>}
        <button className="ax-btn primary tl-wide" disabled={why != null} onClick={() => void st().start()}>
          {isFolderExport(sheet.what) ? "Choose folder and export…" : "Choose file and export…"}
        </button>
      </div>
    </>
  );
}

const FROM: { id: CreateFrom; label: string; desc: string }[] = [
  { id: "images", label: "Images", desc: "JPEG, PNG, TIFF (every page), BMP, GIF" },
  { id: "text", label: "Text file", desc: "A plain .txt, wrapped onto pages" },
  { id: "clipboard", label: "Clipboard image", desc: "What you last copied" },
  { id: "blank", label: "Blank pages", desc: "Empty pages to fill in later" },
];

const SIZES: { id: CreatePageSize; label: string }[] = [
  { id: "fit", label: "Fit image" },
  { id: "a4", label: "A4" },
  { id: "letter", label: "Letter" },
];

function CreateOptions({ sheet }: { sheet: ConvertSheet }) {
  const set = (p: Partial<ConvertSheet>) => st().set(p);
  const why = readiness(sheet, 0);
  return (
    <>
      <div className="ax-body tl-body">
        <label className="tl-label">Create from</label>
        <div className="tl-presets" role="radiogroup" aria-label="Create from">
          {FROM.map((w) => (
            <button key={w.id} role="radio" aria-checked={sheet.from === w.id} className={sheet.from === w.id ? "tl-preset on" : "tl-preset"} onClick={() => set({ from: w.id })}>
              <span className="tl-radio" />
              <span className="tl-preset-text">
                <span className="tl-preset-name">{w.label}</span>
                <span className="dim small">{w.desc}</span>
              </span>
            </button>
          ))}
        </div>

        {sheet.from === "images" && (
          <>
            <ol className="tl-files" aria-label="Images, in page order">
              {sheet.files.map((f, i) => (
                <li key={`${f.token}-${i}`}>
                  <span className="tl-file" title={f.path}>
                    {i + 1}. {base(f.path)}
                  </span>
                  <button className="ax-icon" aria-label={`Move ${base(f.path)} up`} disabled={i === 0} onClick={() => st().moveImage(i, -1)}>
                    <Icon name="up" size={12} />
                  </button>
                  <button className="ax-icon" aria-label={`Move ${base(f.path)} down`} disabled={i === sheet.files.length - 1} onClick={() => st().moveImage(i, 1)}>
                    <Icon name="down" size={12} />
                  </button>
                  <button className="ax-icon" aria-label={`Remove ${base(f.path)}`} onClick={() => st().moveImage(i, null)}>
                    <Icon name="close" size={12} />
                  </button>
                </li>
              ))}
            </ol>
            <button className="ax-btn tl-wide" onClick={() => void st().addImages()}>
              {sheet.files.length ? "Add more images…" : "Choose images…"}
            </button>
          </>
        )}
        {sheet.from === "text" && (
          <button className="ax-btn tl-wide" onClick={() => void st().chooseText()}>
            {sheet.textFile ? base(sheet.textFile.path) : "Choose a text file…"}
          </button>
        )}
        {sheet.from === "blank" && (
          <>
            <label className="tl-label">Pages</label>
            <input className="tl-field" type="number" min={1} max={500} aria-label="Number of pages" value={sheet.blankPages} onChange={(e) => set({ blankPages: Number(e.target.value) })} />
          </>
        )}

        <label className="tl-label">Page size</label>
        <Seg<CreatePageSize>
          label="Page size"
          value={sheet.pageSize}
          items={sheet.from === "images" || sheet.from === "clipboard" ? SIZES : SIZES.filter((s) => s.id !== "fit")}
          onChange={(pageSize) => set({ pageSize })}
        />
        <label className="tl-label">Margins {sheet.margin} pt</label>
        <input type="range" min={0} max={72} step={6} value={sheet.margin} aria-label="Margins in points" onChange={(e) => set({ margin: Number(e.target.value) })} />
        <p className="dim small">The new document opens in a tab. Nothing is written next to your files until you save it.</p>
      </div>
      <div className="tl-foot">
        {why && <p className="dim small">{why}</p>}
        <button className="ax-btn primary tl-wide" disabled={why != null} onClick={() => void st().start()}>
          Create PDF
        </button>
      </div>
    </>
  );
}

function Running({ sheet }: { sheet: ConvertSheet }) {
  const pct = Math.round(sheet.progress * 100);
  return (
    <>
      <div className="ax-body tl-body">
        <div className="tl-line">
          <span>{sheet.status || "Working"}</span>
          <span className="tl-mono">{pct}%</span>
        </div>
        <div className="tl-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct}>
          <span style={{ width: `${pct}%` }} />
        </div>
        <p className="dim small">{sheet.mode === "export" ? "Cancel leaves no partial files." : "Your files are not changed."}</p>
      </div>
      <div className="tl-foot">
        <button className="ax-btn tl-wide" onClick={() => st().cancel()}>
          Cancel
        </button>
      </div>
    </>
  );
}

function Done({ sheet }: { sheet: ConvertSheet }) {
  const e = sheet.exported;
  const c = sheet.created;
  const warnings = e?.warnings ?? c?.warnings ?? [];
  return (
    <>
      <div className="ax-body tl-body">
        {e && (
          <>
            <p className="tl-big">{e.files.length === 1 ? "1 file written" : `${e.files.length} files written`}</p>
            <dl className="tl-counts">
              <dt>Pages</dt>
              <dd className="tl-mono">{e.pages}</dd>
              <dt>Size</dt>
              <dd className="tl-mono">{formatBytes(e.bytes)}</dd>
            </dl>
            <p className="dim small tl-file" title={e.folder}>
              {e.files.length === 1 ? base(e.files[0]) : base(e.folder)}
            </p>
            {e.files.length === 0 && <p className="dim small">That document has no embedded images of a useful size.</p>}
          </>
        )}
        {c && (
          <>
            <p className="tl-big">{c.pages === 1 ? "1 page created" : `${c.pages} pages created`}</p>
            <p className="dim">Opened in a new tab as {c.name}. Save it to keep it.</p>
          </>
        )}
        {warnings.map((w, i) => (
          <p key={i} className="tl-warn small">
            {w}
          </p>
        ))}
      </div>
      <div className="tl-foot">
        <button className="ax-btn primary tl-wide" onClick={() => st().close()}>
          Done
        </button>
      </div>
    </>
  );
}

function Failed({ sheet }: { sheet: ConvertSheet }) {
  return (
    <>
      <div className="ax-body tl-body">
        <p className="tl-big">That did not work</p>
        <p className="dim" role="alert">
          {sheet.error}
        </p>
      </div>
      <div className="tl-foot">
        <button className="ax-btn primary tl-wide" onClick={() => st().set({ phase: "options", error: null })}>
          Back to options
        </button>
        <button className="ax-btn tl-wide" onClick={() => st().close()}>
          Close
        </button>
      </div>
    </>
  );
}
