import { useAi } from "../lib/aiStore";
import type { CompressEstimate, CompressPreset } from "../lib/api";
import * as edits from "../lib/edit";
import { parseRange } from "../lib/pageRange";
import { validateOptions, visualVerdict } from "../lib/optimizer";
import { fileName, formatBytes, type OcrScope, type Sheet, useTools } from "../lib/toolsStore";
import type { DocState } from "../store";
import { Icon } from "./Icons";
import { Advanced, AdvancedEstimate, UsageChange } from "./OptimizerPanel";
import "./tools.css";

/**
 * Compress and Recognize text live in one right-hand rail (approved: design/approved/tools-*.png).
 * It takes the assistant's dock width and chrome, and only one of the two is open at a time
 * (the stores see to that). The state itself is `useTools`: options, running, done or error.
 */
export function ToolsPanel({ doc }: { doc: DocState }) {
  const sheet = useTools((s) => s.sheet);
  const width = useAi((s) => s.settings.width);
  if (!sheet || sheet.doc !== doc.id) return null;
  const title = sheet.tool === "compress" ? "Compress file" : "Recognize text";
  return (
    <aside className="ax-dock" style={{ width: `min(${width}px, 45vw)` }} aria-label={title}>
      <section className="ax-panel docked tl">
        <div className="ax-head">
          <span className="ax-title">{title}</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label={`Close ${title}`} onClick={() => useTools.getState().close()}>
            <Icon name="close" size={14} />
          </button>
        </div>
        {sheet.phase === "options" && (sheet.tool === "compress" ? <CompressOptions sheet={sheet} /> : <OcrOptions sheet={sheet} doc={doc} />)}
        {sheet.phase === "running" && <Running sheet={sheet} />}
        {sheet.phase === "done" && (sheet.tool === "compress" ? <CompressDone sheet={sheet} /> : <OcrDone sheet={sheet} />)}
        {sheet.phase === "error" && <Failed sheet={sheet} />}
      </section>
    </aside>
  );
}

const st = () => useTools.getState();

// ---- compress ----------------------------------------------------------------------------------

const PRESETS: { id: CompressPreset; name: string; desc: string }[] = [
  { id: "smallest", name: "Smallest", desc: "Smaller files, softer images" },
  { id: "balanced", name: "Balanced", desc: "Good for email and sharing" },
  { id: "highQuality", name: "High quality", desc: "Keeps images sharp" },
];

/** "~2.1 MB", or "~1.8–2.6 MB" when the sample could not see every image. */
function sizeText(e: CompressEstimate) {
  if (e.high - e.low < e.bytes * 0.1) return `~${formatBytes(e.bytes)}`;
  const [lo, hi] = [formatBytes(e.low), formatBytes(e.high)];
  const unit = (t: string) => t.split(" ")[1];
  return `~${unit(lo) === unit(hi) ? lo.split(" ")[0] : lo}–${hi}`;
}

function CompressOptions({ sheet }: { sheet: Sheet }) {
  const a = sheet.audit;
  const bytes = (c: string) => a?.audit.categories.find((x) => x.category === c)?.bytes ?? 0;
  const advanced = sheet.mode === "advanced";
  const invalid = advanced && (sheet.advanced == null || validateOptions(sheet.advanced) != null);
  return (
    <>
      <div className="ax-body tl-body">
        <div className="ax-seg tl-seg" role="radiogroup" aria-label="Compress mode">
          {(["presets", "advanced"] as const).map((m) => (
            <button key={m} role="radio" aria-checked={sheet.mode === m} className={sheet.mode === m ? "on" : ""} onClick={() => st().setMode(m)}>
              {m === "presets" ? "Presets" : "Advanced"}
            </button>
          ))}
        </div>
        {advanced && <Advanced sheet={sheet} />}
        {!advanced && <div className="tl-presets" role="radiogroup" aria-label="Preset">
          {PRESETS.map((p) => {
            const est = sheet.estimates?.find((x) => x.preset === p.id);
            return (
              <button
                key={p.id}
                role="radio"
                aria-checked={sheet.preset === p.id}
                className={sheet.preset === p.id ? "tl-preset on" : "tl-preset"}
                onClick={() => st().set({ preset: p.id })}
              >
                <span className="tl-radio" />
                <span className="tl-preset-text">
                  <span className="tl-preset-name">{p.name}</span>
                  <span className="dim small">{p.desc}</span>
                </span>
                <span className="tl-mono">{est ? sizeText(est) : "…"}</span>
              </button>
            );
          })}
        </div>}
        {!advanced && (
          <p className="dim tl-now">
            {a ? (
              <>
                Now <b className="tl-mono">{formatBytes(a.size)}</b>: images {formatBytes(bytes("images"))}, fonts {formatBytes(bytes("fonts"))}
              </>
            ) : (
              "Looking at what is in the file…"
            )}
          </p>
        )}
        <p className="dim small">
          Sizes are estimates; the result shows the real size. The original is never changed
          {advanced ? ", and the copy is checked page by page against it." : "."}
        </p>
        {sheet.needsSave && <p className="dim small">This document has unsaved changes. They are saved first.</p>}
        {a?.outputExists && <p className="dim small">The usual name is taken; you will be asked for another one.</p>}
        {a?.signed && (
          <p className="tl-warn small">This document is signed. The compressed copy is a new file: its signatures will not be valid.</p>
        )}
      </div>
      <div className="tl-foot">
        {advanced && <AdvancedEstimate sheet={sheet} />}
        <button className="ax-btn primary tl-wide" disabled={!a || invalid} onClick={() => void st().start()}>
          Compress
        </button>
      </div>
    </>
  );
}

/** What the run took out, in words: only the options that were on and found something. */
function changes(r: NonNullable<Sheet["compressed"]>): string[] {
  const s = r.stripped;
  const out: string[] = [];
  if (!s) return out;
  const n = (v: number, one: string, many: string) => v > 0 && out.push(`${v} ${v === 1 ? one : many}`);
  if (s.metadata) out.push("Document information and metadata");
  if (s.bookmarks) out.push("Bookmarks");
  n(s.thumbnails, "page thumbnail", "page thumbnails");
  n(s.embeddedFiles, "attachment", "attachments");
  n(s.javascript, "JavaScript action", "JavaScript actions");
  n(s.formActions, "form action", "form actions");
  n(s.alternateImages, "alternate image list", "alternate image lists");
  n(s.privateData, "private data entry", "private data entries");
  n(s.links, "link", "links");
  n(s.invalidLinks, "link that led nowhere", "links that led nowhere");
  n(r.unembedded?.files ?? 0, "font file unembedded", "font files unembedded");
  n(r.flattened?.pages.length ?? 0, "page flattened", "pages flattened");
  return out;
}

function CompressDone({ sheet }: { sheet: Sheet }) {
  const r = sheet.compressed;
  if (!r) return null;
  const after = Math.max(2, Math.min(100, (r.sizeAfter / Math.max(1, r.sizeBefore)) * 100));
  const v = r.visual;
  return (
    <>
      <div className="ax-body tl-body">
        <div className="tl-bars">
          <div>
            <span className="dim small">Before</span>
            <span className="tl-bar" style={{ width: "100%" }} />
            <span className="tl-mono">{formatBytes(r.sizeBefore)}</span>
          </div>
          <div>
            <span className="dim small">After</span>
            <span className="tl-bar after" style={{ width: `${after}%` }} />
            <span className="tl-mono">{formatBytes(r.sizeAfter)}</span>
          </div>
        </div>
        <p className="tl-big">{r.keptOriginal ? "Could not be made smaller" : `Saved ${Math.round(r.savedPercent)}%`}</p>
        {r.keptOriginal && <p className="dim small">Every setting made this file bigger, so the copy is identical to the original.</p>}
        {changes(r).length > 0 && (
          <p className="dim small" aria-label="Removed">
            Removed or changed: {changes(r).join(", ")}.
          </p>
        )}
        {r.before && r.after && <UsageChange before={r.before} after={r.after} />}
        {v == null ? (
          <p className="dim">The visual check was skipped.</p>
        ) : (
          (() => {
            const verdict = visualVerdict(v);
            return <p className={verdict.level === "different" ? "tl-bad" : "dim"}>{verdict.text}</p>;
          })()
        )}
        <p className="dim small tl-file" title={r.output}>
          {fileName(r.output)}
        </p>
        {r.warnings.map((w, i) => (
          <p key={i} className="tl-warn small">
            {w}
          </p>
        ))}
      </div>
      <div className="tl-foot">
        <button className="ax-btn primary tl-wide" onClick={() => void st().openOutput()}>
          Open compressed file
        </button>
        <button className="ax-btn tl-wide" onClick={() => st().close()}>
          Done
        </button>
      </div>
    </>
  );
}

// ---- OCR ---------------------------------------------------------------------------------------

const SCOPES: { id: OcrScope; label: string }[] = [
  { id: "all", label: "All pages" },
  { id: "page", label: "This page" },
  { id: "range", label: "Range" },
];

const languageName = (tag: string) => {
  try {
    return new Intl.DisplayNames(["en"], { type: "language" }).of(tag) ?? tag;
  } catch {
    return tag;
  }
};

function OcrOptions({ sheet, doc }: { sheet: Sheet; doc: DocState }) {
  const av = sheet.available;
  const have = av == null ? [] : sheet.backend === "tesseract" ? (av.tesseract?.languages ?? []) : av.windows;
  const rangeOk = sheet.scope !== "range" || (sheet.range.trim() !== "" && parseRange(sheet.range, doc.pageCount) != null);
  const ready = av != null && sheet.languages.length > 0 && rangeOk;
  return (
    <>
      <div className="ax-body tl-body">
        <label className="tl-label">Pages</label>
        <div className="ax-seg tl-seg" role="radiogroup" aria-label="Pages">
          {SCOPES.map((s) => (
            <button key={s.id} role="radio" aria-checked={sheet.scope === s.id} className={sheet.scope === s.id ? "on" : ""} onClick={() => st().set({ scope: s.id })}>
              {s.label}
            </button>
          ))}
        </div>
        {sheet.scope === "range" && (
          <input
            className={rangeOk ? "tl-field" : "tl-field bad"}
            value={sheet.range}
            placeholder={`1-${doc.pageCount}, e.g. 1-3, 5`}
            aria-label="Page range"
            spellCheck={false}
            onChange={(e) => st().set({ range: e.target.value })}
          />
        )}
        {sheet.scope === "page" && <p className="dim small">Page {doc.currentPage + 1}</p>}

        <label className="tl-label">Language</label>
        <select
          className="tl-field"
          value={sheet.languages[0] ?? ""}
          aria-label="Language"
          disabled={have.length === 0}
          onChange={(e) => st().set({ languages: [e.target.value] })}
        >
          {have.length === 0 && <option value="">{av ? "No language installed" : "Looking…"}</option>}
          {have.map((t) => (
            <option key={t} value={t}>
              {languageName(t)}
            </option>
          ))}
        </select>

        <label className="tl-label">Engine</label>
        <div className="ax-seg tl-seg" role="radiogroup" aria-label="Engine">
          {(["windows", "tesseract"] as const).map((b) => {
            const list = av == null ? [] : b === "tesseract" ? (av.tesseract?.languages ?? []) : av.windows;
            return (
              <button
                key={b}
                role="radio"
                aria-checked={sheet.backend === b}
                className={sheet.backend === b ? "on" : ""}
                disabled={b === "tesseract" && !av?.tesseract}
                onClick={() => st().set({ backend: b, languages: list[0] ? [list[0]] : [] })}
              >
                {b === "windows" ? "Windows" : "Tesseract"}
              </button>
            );
          })}
        </div>
        <button className="tl-link small" onClick={() => void st().chooseTesseract()}>
          {av?.tesseract ? "Choose another tesseract.exe…" : "Use Tesseract: choose tesseract.exe…"}
        </button>

        <label className="tl-switch">
          <input type="checkbox" checked={sheet.force} onChange={(e) => st().set({ force: e.target.checked })} />
          <span>Redo pages that already have text</span>
        </label>
        <p className="dim small">The text is added to this document as one step you can undo.</p>
      </div>
      <div className="tl-foot">
        <button className="ax-btn primary tl-wide" disabled={!ready} onClick={() => void st().start()}>
          Recognize text
        </button>
        <button className="ax-btn tl-wide" disabled={!ready} onClick={() => void st().start({ saveAs: true })}>
          Save as new file…
        </button>
      </div>
    </>
  );
}

function OcrDone({ sheet }: { sheet: Sheet }) {
  const o = sheet.recognized;
  if (!o) return null;
  const r = o.report;
  const undoable = o.state != null && r.pages_recognized > 0;
  return (
    <>
      <div className="ax-body tl-body">
        <p className="tl-big">
          {o.output ? "Saved a searchable copy" : r.pages_recognized === 1 ? "1 page recognized" : `${r.pages_recognized} pages recognized`}
        </p>
        <dl className="tl-counts">
          <dt>Recognized</dt>
          <dd className="tl-mono">{r.pages_recognized}</dd>
          <dt>Already had text</dt>
          <dd className="tl-mono">{r.pages_skipped_text}</dd>
          {r.pages_no_text > 0 && (
            <>
              <dt>No text found</dt>
              <dd className="tl-mono">{r.pages_no_text}</dd>
            </>
          )}
          <dt>Failed</dt>
          <dd className="tl-mono">{r.pages_failed}</dd>
        </dl>
        {r.words > 0 && <p className="dim small">{r.words} words added.</p>}
        {o.output && (
          <p className="dim small tl-file" title={o.output}>
            {fileName(o.output)}
          </p>
        )}
        {r.warnings.map((w, i) => (
          <p key={i} className="tl-warn small">
            {w}
          </p>
        ))}
      </div>
      <div className="tl-foot">
        {o.output ? (
          <button className="ax-btn primary tl-wide" onClick={() => void st().openOutput()}>
            Open new file
          </button>
        ) : (
          undoable && (
            <button className="ax-btn tl-wide" onClick={() => void st().undo()}>
              Undo
            </button>
          )
        )}
        {!o.output && r.pages_recognized > 0 && (
          <button className="ax-btn tl-wide" onClick={() => void edits.saveAs(sheet.doc)}>
            Save as new file…
          </button>
        )}
        <button className={o.output || !undoable ? "ax-btn primary tl-wide" : "ax-btn tl-wide"} onClick={() => st().close()}>
          Done
        </button>
      </div>
    </>
  );
}

// ---- shared states -----------------------------------------------------------------------------

function Running({ sheet }: { sheet: Sheet }) {
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
        <p className="dim small">
          {sheet.tool === "ocr" ? `${sheet.backend === "tesseract" ? "Tesseract" : "Windows OCR"}, ${sheet.languages.join(", ")}` : "Your original stays as it is."}
        </p>
      </div>
      <div className="tl-foot">
        <button className="ax-btn tl-wide" onClick={() => st().cancel()}>
          Cancel
        </button>
      </div>
    </>
  );
}

function Failed({ sheet }: { sheet: Sheet }) {
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
