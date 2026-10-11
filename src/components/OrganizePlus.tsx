import { useEffect, useMemo, useRef, useState } from "react";
import * as api from "../lib/api";
import type { LabelStyle, PageTrans, TransStyle, Transition } from "../lib/api";
import * as edits from "../lib/edit";
import {
  DIRECTIONS,
  LABEL_STYLES,
  TRANS_STYLES,
  commonTrans,
  defaultTransition,
  describeTransition,
  sampleLabels,
  styleInfo,
  useSheets,
} from "../lib/organizePlus";
import { formatRange, parseRange } from "../lib/pageRange";
import { useStore } from "../store";
import { Icon } from "./Icons";
import "./organizePlus.css";

/** Escape closes the sheet (before anything underneath hears it). */
function useEscape(open: boolean, close: () => void) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if ((e.target as HTMLElement | null)?.closest("input, select")) (e.target as HTMLElement).blur();
      e.stopPropagation();
      close();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, close]);
}

function Shell({ title, label, onClose, children }: { title: string; label: string; onClose: () => void; children: React.ReactNode }) {
  return (
    <div className="scrim op-scrim" onMouseDown={onClose}>
      <section className="op-sheet" role="dialog" aria-label={label} onMouseDown={(e) => e.stopPropagation()}>
        <header className="op-head">
          <span className="op-title">{title}</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label={`Close ${label}`} onClick={onClose}>
            <Icon name="close" size={14} />
          </button>
        </header>
        {children}
      </section>
    </div>
  );
}

const toInt = (s: string) => (/^\s*\d+\s*$/.test(s) ? parseInt(s, 10) : null);

// ---- page labels ----------------------------------------------------------------------------

export function LabelsSheet() {
  const sheet = useSheets((s) => s.labels);
  const close = useSheets((s) => s.closeLabels);
  const doc = useStore((s) => (sheet ? s.docs[sheet.doc] : undefined));
  useEscape(sheet != null, close);
  if (!sheet || !doc) return null;
  return <LabelsBody key={`${sheet.doc}:${sheet.from}:${sheet.to}`} doc={doc.id} pageCount={doc.pageCount} labelled={doc.labels.length > 0} from={sheet.from} to={sheet.to} close={close} />;
}

function LabelsBody({ doc, pageCount, labelled, from, to, close }: { doc: number; pageCount: number; labelled: boolean; from: number; to: number; close: () => void }) {
  const [first, setFirst] = useState(String(from + 1));
  const [last, setLast] = useState(String(to + 1));
  const [style, setStyle] = useState<LabelStyle>("decimal");
  const [prefix, setPrefix] = useState("");
  const [start, setStart] = useState("1");
  const [extend, setExtend] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const touched = useRef(false);

  // Start from the section the first page is in: same style and prefix, numbered on from there.
  useEffect(() => {
    let live = true;
    api
      .labelsList(doc)
      .then((d) => {
        if (!live || touched.current || d.ranges.length === 0) return;
        const r = [...d.ranges].reverse().find((x) => x.page <= from);
        if (!r) return;
        setStyle(r.style);
        setPrefix(r.prefix);
        setStart(String(r.start + (from - r.page)));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [doc, from]);

  const a = toInt(first);
  const b = toInt(last);
  const s = toInt(start);
  const rangeOk = a != null && b != null && a >= 1 && b <= pageCount && a <= b;
  const startOk = extend || (s != null && s >= 1);
  const valid = rangeOk && startOk && !busy;
  const touch = <T,>(set: (v: T) => void) => (v: T) => {
    touched.current = true;
    setError(null);
    set(v);
  };

  const sample = sampleLabels({ style, prefix, start: extend ? 1 : (s ?? 1) }, rangeOk ? b! - a! + 1 : 3);
  const outcome = !rangeOk
    ? `Pages 1 to ${pageCount}`
    : `${a === b ? `Page ${a}` : `Pages ${a} to ${b}`} become ${extend ? "the section before, carried on" : sample || "unnumbered"}`;

  const apply = async () => {
    if (!valid) return;
    setBusy(true);
    try {
      await edits.run(doc, () => api.labelsSet(doc, a! - 1, b! - 1, style, prefix, extend ? 1 : s!, extend));
      close();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  const removeAll = async () => {
    setBusy(true);
    try {
      await edits.run(doc, () => api.labelsRemove(doc));
      close();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <Shell title="Page labels" label="Page labels" onClose={close}>
      <div className="op-body">
        <div className="op-pair">
          <label className="op-field">
            <span className="op-label">From page</span>
            <input className={`tl-field${a == null || a < 1 || a > pageCount ? " bad" : ""}`} value={first} inputMode="numeric" onChange={(e) => touch(setFirst)(e.target.value)} autoFocus />
          </label>
          <label className="op-field">
            <span className="op-label">To page</span>
            <input className={`tl-field${b == null || b > pageCount || (a != null && b < a) ? " bad" : ""}`} value={last} inputMode="numeric" onChange={(e) => touch(setLast)(e.target.value)} />
          </label>
          <span className="dim">of {pageCount}</span>
        </div>
        <label className="op-field">
          <span className="op-label">Numbering</span>
          <select className="tl-field" value={style} onChange={(e) => touch(setStyle)(e.target.value as LabelStyle)}>
            {LABEL_STYLES.map((o) => (
              <option key={o.id} value={o.id}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <div className="op-pair">
          <label className="op-field">
            <span className="op-label">Prefix</span>
            <input className="tl-field" value={prefix} placeholder="e.g. A-" spellCheck={false} onChange={(e) => touch(setPrefix)(e.target.value)} />
          </label>
          <label className="op-field">
            <span className="op-label">Start at</span>
            <input className={`tl-field${startOk ? "" : " bad"}`} value={start} inputMode="numeric" disabled={extend || style === "none"} onChange={(e) => touch(setStart)(e.target.value)} />
          </label>
        </div>
        <label className="op-check">
          <input type="checkbox" checked={extend} onChange={(e) => touch(setExtend)(e.target.checked)} />
          Carry on the numbering of the pages before
        </label>
        <div className="op-sample" aria-live="polite">
          {outcome}
        </div>
        {error && (
          <p className="op-error" role="alert">
            {error}
          </p>
        )}
      </div>
      <footer className="op-foot">
        {labelled && (
          <button className="btn ghost" disabled={busy} onClick={() => void removeAll()}>
            Remove all labels
          </button>
        )}
        <span className="grow" />
        <button className="btn ghost" onClick={close}>
          Cancel
        </button>
        <button className="btn primary" disabled={!valid} onClick={() => void apply()}>
          Apply
        </button>
      </footer>
    </Shell>
  );
}

// ---- page transitions -----------------------------------------------------------------------

export function TransitionsSheet() {
  const sheet = useSheets((s) => s.trans);
  const close = useSheets((s) => s.closeTrans);
  const doc = useStore((s) => (sheet ? s.docs[sheet.doc] : undefined));
  useEscape(sheet != null, close);
  if (!sheet || !doc) return null;
  return <TransBody key={`${sheet.doc}:${sheet.pages.join(",")}`} doc={doc.id} pageCount={doc.pageCount} pages={sheet.pages} close={close} />;
}

const num = (s: string) => (s.trim() !== "" && /^\d*\.?\d+$/.test(s.trim()) ? Number(s) : null);

function TransBody({ doc, pageCount, pages, close }: { doc: number; pageCount: number; pages: number[]; close: () => void }) {
  const [all, setAll] = useState<PageTrans[] | null>(null);
  const [text, setText] = useState(formatRange(pages));
  const [t, setT] = useState<Transition>(defaultTransition("r"));
  const [dur, setDur] = useState("1");
  const [auto, setAuto] = useState(false);
  const [secs, setSecs] = useState("5");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const touched = useRef(false);

  useEffect(() => {
    let live = true;
    api
      .transitionsList(doc)
      .then((l) => {
        if (!live) return;
        setAll(l);
        const common = commonTrans(l, pages);
        if (common && !touched.current) {
          setT(common.transition ?? defaultTransition("r"));
          setDur(String(common.transition?.duration ?? 1));
          setAuto(common.autoAdvance != null);
          if (common.autoAdvance != null) setSecs(String(common.autoAdvance));
        }
      })
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
    // the first pages decide the preset; later edits of the field do not reload it
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc]);

  const target = useMemo(() => parseRange(text, pageCount), [text, pageCount]);
  const info = styleInfo(t.style);
  const durN = num(dur);
  const secsN = num(secs);
  const durOk = t.style === "r" || (durN != null && durN > 0 && durN <= 60);
  const secsOk = !auto || (secsN != null && secsN > 0 && secsN <= 3600);
  const valid = target != null && target.length > 0 && durOk && secsOk && all != null && !busy;
  const mark = <A,>(f: (v: A) => void) => (v: A) => {
    touched.current = true;
    setError(null);
    f(v);
  };

  const change = (): PageTrans => ({
    transition: t.style === "r" ? null : { ...t, duration: durN ?? 1 },
    autoAdvance: auto ? (secsN ?? 5) : null,
  });
  const preview = describeTransition(t.style === "r" ? null : { ...t, duration: durN ?? 1 });

  const apply = async () => {
    if (!valid || !target) return;
    setBusy(true);
    try {
      await edits.run(doc, () => api.transitionsSet(doc, target, change()));
      close();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <Shell title="Page transitions" label="Page transitions" onClose={close}>
      <div className="op-body">
        <label className="op-field">
          <span className="op-label">Pages</span>
          <input
            className={`tl-field${target == null || target.length === 0 ? " bad" : ""}`}
            value={text}
            placeholder={`e.g. 1, 3-5 (of ${pageCount})`}
            spellCheck={false}
            onChange={(e) => mark(setText)(e.target.value)}
          />
        </label>
        <label className="op-field">
          <span className="op-label">Effect</span>
          <select
            className="tl-field"
            value={t.style}
            disabled={all == null}
            onChange={(e) => {
              const d = defaultTransition(e.target.value as TransStyle, durN ?? 1);
              mark(setT)(d);
            }}
          >
            {TRANS_STYLES.map((o) => (
              <option key={o.id} value={o.id}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        {info.dimension && (
          <Seg
            label="Direction of the bars"
            value={t.dimension ?? "H"}
            items={[
              { id: "H", label: "Horizontal" },
              { id: "V", label: "Vertical" },
            ]}
            onChange={(v) => mark(setT)({ ...t, dimension: v })}
          />
        )}
        {info.motion && (
          <Seg
            label="Motion"
            value={t.motion ?? "I"}
            items={[
              { id: "I", label: "Inward" },
              { id: "O", label: "Outward" },
            ]}
            onChange={(v) => mark(setT)({ ...t, motion: v })}
          />
        )}
        {info.direction && (
          <label className="op-field">
            <span className="op-label">Direction</span>
            <select className="tl-field" value={t.direction ?? 0} onChange={(e) => mark(setT)({ ...t, direction: Number(e.target.value) })}>
              {(info.direction === "glitter"
                ? [
                    { deg: 0, label: "Left to right" },
                    { deg: 270, label: "Top to bottom" },
                    { deg: 315, label: "Diagonal" },
                  ]
                : DIRECTIONS
              ).map((d) => (
                <option key={d.deg} value={d.deg}>
                  {d.label}
                </option>
              ))}
            </select>
          </label>
        )}
        {t.style !== "r" && (
          <label className="op-field">
            <span className="op-label">Takes (seconds)</span>
            <input className={`tl-field${durOk ? "" : " bad"}`} value={dur} inputMode="decimal" onChange={(e) => mark(setDur)(e.target.value)} />
          </label>
        )}
        <div className="op-pair">
          <label className="op-check">
            <input type="checkbox" checked={auto} onChange={(e) => mark(setAuto)(e.target.checked)} />
            Go on by itself after
          </label>
          <input
            className={`tl-field${secsOk ? "" : " bad"}`}
            style={{ maxWidth: 90 }}
            aria-label="Seconds before moving on"
            value={secs}
            disabled={!auto}
            inputMode="decimal"
            onChange={(e) => mark(setSecs)(e.target.value)}
          />
          <span className="dim">seconds</span>
        </div>
        <div className="op-sample" aria-live="polite">
          {all == null ? "Reading the pages…" : `${preview}${auto && secsOk ? `; moves on after ${secsN} s` : ""}`}
        </div>
        {error && (
          <p className="op-error" role="alert">
            {error}
          </p>
        )}
      </div>
      <footer className="op-foot">
        <span className="op-outcome dim">{target ? `${target.length} ${target.length === 1 ? "page" : "pages"}` : "Check the pages"}</span>
        <button className="btn ghost" onClick={close}>
          Cancel
        </button>
        <button className="btn primary" disabled={!valid} onClick={() => void apply()}>
          Apply
        </button>
      </footer>
    </Shell>
  );
}

function Seg<T extends string>({ label, value, items, onChange }: { label: string; value: T; items: { id: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <div className="op-field">
      <span className="op-label">{label}</span>
      <div className="ax-seg op-seg" role="radiogroup" aria-label={label}>
        {items.map((i) => (
          <button key={i.id} role="radio" aria-checked={value === i.id} className={value === i.id ? "on" : ""} onClick={() => onChange(i.id)}>
            {i.label}
          </button>
        ))}
      </div>
    </div>
  );
}
