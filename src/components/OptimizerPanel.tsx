import { useState } from "react";
import type { CompressAudit, CompressOptions, CompressPreset, ImageClassSettings, ImageCodec } from "../lib/api";
import {
  CATEGORY_LABEL,
  CODEC_LABEL,
  CODECS,
  DISCARD,
  FLATTEN_PPI,
  GARBAGE_LABEL,
  sameOptions,
  summarize,
  usageRows,
  validateOptions,
  forPreset,
} from "../lib/optimizer";
import { formatBytes, type Sheet, useTools } from "../lib/toolsStore";
import "./tools.css";

/**
 * The Advanced settings of the Compress rail: Acrobat's Optimized PDF in one column. Saved settings on
 * top, where the bytes are now, then one section per kind of change (images, fonts, transparency,
 * discard objects, clean up). Everything here edits `sheet.advanced`; the engine's own presets are the
 * starting points and the same options go to the same compress engine.
 */
const st = () => useTools.getState();

export function Advanced({ sheet }: { sheet: Sheet }) {
  const o = sheet.advanced;
  if (!o) return <p className="dim">Loading the settings…</p>;
  const set = (fn: (o: CompressOptions) => CompressOptions) => st().setAdvanced(fn);
  const sum = summarize(o);
  return (
    <>
      <SavedSettings sheet={sheet} />
      {sheet.audit && <SpaceUsage audit={sheet.audit.audit} />}

      <Section title="Images" summary={sum.images} open>
        <ImageClass title="Colour" id="color" value={o.images.color} onChange={(c) => set((x) => ({ ...x, images: { ...x.images, color: c } }))} />
        <ImageClass title="Grayscale" id="gray" value={o.images.gray} onChange={(c) => set((x) => ({ ...x, images: { ...x.images, gray: c } }))} />
        <ImageClass title="Monochrome" id="mono" value={o.images.mono} onChange={(c) => set((x) => ({ ...x, images: { ...x.images, mono: c } }))} />
        <Check
          checked={o.images.onlyIfSmaller}
          onChange={(v) => set((x) => ({ ...x, images: { ...x.images, onlyIfSmaller: v } }))}
          label="Keep an image when re-encoding would make it bigger"
        />
      </Section>

      <Section title="Fonts" summary={sum.fonts}>
        <Check checked={o.subsetFonts} onChange={(v) => set((x) => ({ ...x, subsetFonts: v }))} label="Keep only the characters that are used (subset)" />
        <Check
          checked={o.unembedAll}
          onChange={(v) => set((x) => ({ ...x, unembedAll: v }))}
          label="Unembed every font that can be"
          hint="Viewers draw such text with a similar font they have"
        />
        <FontList sheet={sheet} o={o} />
      </Section>

      <Section title="Transparency" summary={sum.transparency}>
        <Check
          checked={o.transparency.flatten}
          onChange={(v) => set((x) => ({ ...x, transparency: { ...x.transparency, flatten: v } }))}
          label="Flatten pages that use transparency"
          hint="Each such page becomes one picture: its text can no longer be selected"
        />
        {o.transparency.flatten && (
          <label className="tl-inline">
            <span className="dim small">Resolution</span>
            <select
              className="tl-field tl-short"
              value={o.transparency.ppi}
              aria-label="Flatten resolution"
              onChange={(e) => set((x) => ({ ...x, transparency: { ...x.transparency, ppi: Number(e.target.value) } }))}
            >
              {FLATTEN_PPI.map((p) => (
                <option key={p} value={p}>
                  {p} ppi
                </option>
              ))}
            </select>
          </label>
        )}
      </Section>

      <Section title="Discard objects" summary={sum.discard}>
        {DISCARD.map((d) => (
          <Check
            key={d.key}
            checked={o.strip[d.key]}
            onChange={(v) => set((x) => ({ ...x, strip: { ...x.strip, [d.key]: v } }))}
            label={d.label}
            hint={d.hint}
          />
        ))}
      </Section>

      <Section title="Clean up" summary={sum.cleanup}>
        <Check
          checked={o.strip.invalidLinks}
          onChange={(v) => set((x) => ({ ...x, strip: { ...x.strip, invalidLinks: v } }))}
          label="Links that lead nowhere"
          hint="No action, a missing page or an unknown named destination"
        />
        <Check checked={o.objectStreams} onChange={(v) => set((x) => ({ ...x, objectStreams: v }))} label="Pack the document structure (object streams)" />
        <Check checked={o.compressStreams} onChange={(v) => set((x) => ({ ...x, compressStreams: v }))} label="Compress streams that are stored raw (Flate)" />
        <label className="tl-inline">
          <span className="dim small">Objects</span>
          <select className="tl-field" value={o.garbage} aria-label="Unused and duplicate objects" onChange={(e) => set((x) => ({ ...x, garbage: Number(e.target.value) }))}>
            {GARBAGE_LABEL.map((l, i) => (
              <option key={i} value={i}>
                {l}
              </option>
            ))}
          </select>
        </label>
      </Section>
    </>
  );
}

/** The size the settings give, and the check that stays on: shown at the foot of the options. */
export function AdvancedEstimate({ sheet }: { sheet: Sheet }) {
  const o = sheet.advanced;
  const e = sheet.customEstimate;
  const bad = o ? validateOptions(o) : null;
  const total = sheet.audit?.size ?? 0;
  if (bad) {
    return (
      <p className="tl-warn small" role="alert">
        {bad}
      </p>
    );
  }
  const text = (() => {
    if (!e) return "…";
    const unit = (t: string) => t.split(" ")[1];
    const spread = e.high - e.low >= e.bytes * 0.1;
    if (!spread) return `~${formatBytes(e.bytes)}`;
    const [lo, hi] = [formatBytes(e.low), formatBytes(e.high)];
    return `~${unit(lo) === unit(hi) ? lo.split(" ")[0] : lo}–${hi}`;
  })();
  const pct = e && total > 0 ? Math.round((1 - e.bytes / total) * 100) : null;
  return (
    <div className="tl-est" aria-live="polite">
      <span className="dim">Estimated size</span>
      <span className="tl-mono">
        {text}
        {pct != null && pct > 0 && <span className="dim"> · −{pct}%</span>}
      </span>
    </div>
  );
}

// ---- saved settings --------------------------------------------------------------------------

function SavedSettings({ sheet }: { sheet: Sheet }) {
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState("");
  const o = sheet.advanced!;
  const current = sheet.saved.find((p) => p.name === sheet.savedName);
  const changed = current != null && !sameOptions(forPreset(o), { ...current.options, unembedFonts: [] });
  const startFrom = (p: CompressPreset) => st().startFrom(p);
  const commit = () => {
    const n = name.trim();
    if (!n) return;
    void st().savePreset(n);
    setNaming(false);
  };
  return (
    <div className="tl-saved">
      <label className="tl-label">Settings</label>
      <div className="tl-row">
        <select
          className="tl-field"
          aria-label="Saved settings"
          value={sheet.savedName ?? ""}
          onChange={(e) => (e.target.value ? st().loadPreset(e.target.value) : st().set({ savedName: null }))}
        >
          <option value="">{sheet.saved.length === 0 ? "No saved settings yet" : "Saved settings…"}</option>
          {sheet.saved.map((p) => (
            <option key={p.name} value={p.name}>
              {p.name}
            </option>
          ))}
        </select>
        <button
          className="ax-btn"
          onClick={() => {
            setName(sheet.savedName ?? "");
            setNaming(true);
          }}
        >
          Save…
        </button>
      </div>
      {naming && (
        <div className="tl-row">
          <input
            className="tl-field"
            autoFocus
            value={name}
            maxLength={60}
            placeholder="Name"
            aria-label="Name for these settings"
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              if (e.key === "Escape") setNaming(false);
            }}
          />
          <button className="ax-btn primary" disabled={!name.trim()} onClick={commit}>
            Save
          </button>
        </div>
      )}
      {sheet.savedName && !naming && (
        <p className="dim small tl-saved-line">
          {changed ? `${sheet.savedName}, changed` : sheet.savedName}
          {" · "}
          <button className="tl-link small" onClick={() => void st().deletePreset(sheet.savedName!)}>
            Delete
          </button>
        </p>
      )}
      {sheet.savedError && (
        <p className="tl-warn small" role="alert">
          {sheet.savedError}
        </p>
      )}
      <p className="dim small tl-startfrom">
        Start from{" "}
        {(["smallest", "balanced", "highQuality"] as const).map((p, i) => (
          <span key={p}>
            {i > 0 && ", "}
            <button className="tl-link small" onClick={() => startFrom(p)}>
              {p === "highQuality" ? "High quality" : p === "smallest" ? "Smallest" : "Balanced"}
            </button>
          </span>
        ))}
      </p>
    </div>
  );
}

// ---- space usage -----------------------------------------------------------------------------

export function SpaceUsage({ audit }: { audit: CompressAudit }) {
  const rows = usageRows(audit);
  return (
    <section className="tl-usage" aria-label="Space usage">
      <div className="tl-usage-head">
        <span className="tl-label">Space usage</span>
        <span className="tl-mono dim">{formatBytes(audit.total)}</span>
      </div>
      <table>
        <tbody>
          {rows.map((r) => (
            <tr key={r.category}>
              <th scope="row">{r.label}</th>
              <td className="tl-mono">{formatBytes(r.bytes)}</td>
              <td className="tl-mono tl-pct">{r.percent >= 0.1 ? `${r.percent.toFixed(1)}%` : "<0.1%"}</td>
              <td className="tl-usage-bar" aria-hidden>
                <span style={{ width: `${Math.max(1.5, Math.min(100, r.percent))}%` }} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/** Before and after, by category, once a run is finished. */
export function UsageChange({ before, after }: { before: CompressAudit; after: CompressAudit }) {
  const cats = [...new Set([...before.categories, ...after.categories].map((c) => c.category))];
  const get = (a: CompressAudit, c: string) => a.categories.find((x) => x.category === c)?.bytes ?? 0;
  const rows = cats
    .map((c) => ({ c, b: get(before, c), a: get(after, c) }))
    .filter((r) => r.b > 0 || r.a > 0)
    .sort((x, y) => y.b - x.b);
  return (
    <table className="tl-change">
      <thead>
        <tr>
          <th />
          <th className="dim small">Before</th>
          <th className="dim small">After</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r.c}>
            <th scope="row">{CATEGORY_LABEL[r.c as keyof typeof CATEGORY_LABEL] ?? r.c}</th>
            <td className="tl-mono">{formatBytes(r.b)}</td>
            <td className={r.a < r.b ? "tl-mono tl-less" : "tl-mono"}>{formatBytes(r.a)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

// ---- pieces ----------------------------------------------------------------------------------

function Section({ title, summary, open, children }: { title: string; summary: string; open?: boolean; children: React.ReactNode }) {
  return (
    <details className="tl-sec" open={open}>
      <summary>
        <span className="tl-sec-title">{title}</span>
        <span className="dim small tl-sec-sum">{summary}</span>
      </summary>
      <div className="tl-sec-body">{children}</div>
    </details>
  );
}

function Check({ checked, onChange, label, hint, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string; disabled?: boolean }) {
  return (
    <label className={disabled ? "tl-check off" : "tl-check"} title={hint}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>
        {label}
        {hint && <span className="dim small tl-check-hint">{hint}</span>}
      </span>
    </label>
  );
}

function ImageClass({ title, id, value, onChange }: { title: string; id: "color" | "gray" | "mono"; value: ImageClassSettings; onChange: (c: ImageClassSettings) => void }) {
  const down = value.abovePpi > 0;
  const lossy = value.codec === "jpeg" || value.codec === "jpx";
  const num = (v: string) => Math.max(0, Math.round(Number(v) || 0));
  const bad = down && (value.abovePpi < 36 || value.toPpi < 36 || value.abovePpi > 2400 || value.toPpi > 2400 || value.toPpi > value.abovePpi);
  return (
    <fieldset className="tl-class">
      <legend>{title}</legend>
      <label className="tl-check">
        <input
          type="checkbox"
          checked={down}
          onChange={(e) => onChange(e.target.checked ? { ...value, abovePpi: id === "mono" ? 450 : 225, toPpi: id === "mono" ? 300 : 150 } : { ...value, abovePpi: 0 })}
        />
        <span>Downsample</span>
      </label>
      {down && (
        <div className="tl-row tl-ppi">
          <label>
            <span className="dim small">above</span>
            <input
              className={bad ? "tl-field tl-num bad" : "tl-field tl-num"}
              type="number"
              min={36}
              max={2400}
              value={value.abovePpi}
              aria-label={`${title}: downsample images above this many ppi`}
              onChange={(e) => onChange({ ...value, abovePpi: num(e.target.value) })}
            />
          </label>
          <label>
            <span className="dim small">to</span>
            <input
              className={bad ? "tl-field tl-num bad" : "tl-field tl-num"}
              type="number"
              min={36}
              max={2400}
              value={value.toPpi}
              aria-label={`${title}: downsample to this many ppi`}
              onChange={(e) => onChange({ ...value, toPpi: num(e.target.value) })}
            />
          </label>
          <span className="dim small">ppi</span>
        </div>
      )}
      <div className="tl-row">
        <select
          className="tl-field"
          value={value.codec}
          aria-label={`${title}: compression`}
          onChange={(e) => onChange({ ...value, codec: e.target.value as ImageCodec })}
        >
          {CODECS[id].map((c) => (
            <option key={c} value={c}>
              {CODEC_LABEL[c]}
            </option>
          ))}
        </select>
        {lossy && (
          <label className="tl-quality">
            <span className="dim small">Quality</span>
            <input
              className="tl-field tl-num"
              type="number"
              min={1}
              max={100}
              value={value.quality}
              aria-label={`${title}: quality`}
              onChange={(e) => onChange({ ...value, quality: Math.round(Number(e.target.value) || 0) })}
            />
          </label>
        )}
      </div>
    </fieldset>
  );
}

function FontList({ sheet, o }: { sheet: Sheet; o: CompressOptions }) {
  const fonts = (sheet.audit?.audit.fontList ?? []).filter((f) => f.embedded);
  if (fonts.length === 0) return <p className="dim small">This file has no embedded fonts.</p>;
  const toggle = (name: string, on: boolean) =>
    st().setAdvanced((x) => ({ ...x, unembedFonts: on ? [...new Set([...x.unembedFonts, name])] : x.unembedFonts.filter((n) => n !== name) }));
  return (
    <ul className="tl-fonts" aria-label="Embedded fonts">
      {fonts.map((f) => {
        const checked = f.canUnembed && (o.unembedAll || o.unembedFonts.includes(f.name));
        return (
          <li key={f.name} title={f.whyNot ?? undefined}>
            <label className={f.canUnembed && !o.unembedAll ? "tl-check" : "tl-check off"}>
              <input type="checkbox" checked={checked} disabled={!f.canUnembed || o.unembedAll} onChange={(e) => toggle(f.name, e.target.checked)} />
              <span className="tl-font-name">{f.name}</span>
              <span className="tl-mono dim small">{formatBytes(f.bytes)}</span>
            </label>
            {!f.canUnembed && f.whyNot && <span className="dim small tl-font-why">{f.whyNot}</span>}
          </li>
        );
      })}
    </ul>
  );
}
