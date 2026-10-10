import { useEffect, useMemo, useRef, useState } from "react";
import { type Comparison, type Rect4, type Source, type VersionInfo, historyCompare, historyVisualDiff, sourceKey } from "../../lib/history";
import {
  CATEGORIES,
  type Category,
  type Mark,
  alignRows,
  categoryCounts,
  clock,
  flatOrder,
  groupChanges,
  marksFor,
  pageSummary,
  rowOf,
  stepIndex,
  trackDays,
  uncovered,
  versionTitle,
  when,
} from "../../lib/historyLogic";
import { type CompareReq, useHistory } from "../../lib/historyStore";
import { Icon } from "../Icons";
import { PageImg, SCALE, clearPageCache } from "./PageImg";
import { RevRow } from "./RevRow";

type Mode = "twoup" | "swipe" | "onion";
const MODES: { id: Mode; label: string }[] = [
  { id: "twoup", label: "2-up" },
  { id: "swipe", label: "Swipe" },
  { id: "onion", label: "Onion skin" },
];

const EMPTY_VERSIONS: VersionInfo[] = [];
const reduced = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Compare mode: the whole window, two sources, their differences on the pages and in a list. */
export function CompareView() {
  const req = useHistory((s) => s.compare)!;
  const versions = useHistory((s) => (req.doc != null ? (s.docs[req.doc]?.versions ?? EMPTY_VERSIONS) : EMPTY_VERSIONS));
  const h = useHistory.getState();
  const [mode, setMode] = useState<Mode>("twoup");
  const [diffs, setDiffs] = useState(true);
  const [cmp, setCmp] = useState<Comparison | "loading" | { error: string }>("loading");
  const [cur, setCur] = useState<number | null>(null);
  const [flash, setFlash] = useState(0);
  const [cats, setCats] = useState<Set<Category> | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const aKey = sourceKey(req.a);
  const bKey = sourceKey(req.b);

  useEffect(() => () => clearPageCache(), []);

  useEffect(() => {
    let live = true;
    setCmp("loading");
    setCur(null);
    historyCompare(req.doc, req.a, req.b).then(
      (c) => {
        if (!live) return;
        setCmp(c);
        setCur(c.changes.length ? 0 : null);
      },
      (e) => live && setCmp({ error: String(e) }),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [req.doc, aKey, bKey]);

  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !(e.target instanceof HTMLInputElement) && !(e.target instanceof HTMLSelectElement)) h.closeCompare();
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const done = typeof cmp === "object" && "pages" in cmp ? cmp : null;
  const rows = useMemo(() => (done ? alignRows(done.pages) : []), [done]);
  const groups = useMemo(() => (done ? groupChanges(done.changes, cats) : []), [done, cats]);
  const order = useMemo(() => flatOrder(groups), [groups]);
  const counts = useMemo(() => (done ? categoryCounts(done.changes) : null), [done]);
  const visible = useMemo(() => new Set(order), [order]);
  const marksA = useMemo(() => (done ? marksFor(done.changes, "a").filter((m) => visible.has(m.change)) : []), [done, visible]);
  const marksB = useMemo(() => (done ? marksFor(done.changes, "b").filter((m) => visible.has(m.change)) : []), [done, visible]);

  const select = (i: number | null) => {
    setCur(i);
    setFlash((n) => n + 1);
  };

  // Scroll both sides to the change (one scroller holds both columns, so one scroll does it).
  useEffect(() => {
    if (cur === null || !done || !scroller.current) return;
    const c = done.changes[cur];
    const r = rowOf(rows, c);
    const row = scroller.current.querySelector<HTMLElement>(`[data-row="${r}"]`);
    if (!row) return;
    const page = row.querySelector<HTMLElement>(".hx-page");
    const img = page?.querySelector("img");
    const rect = c.rectB ?? c.rectA;
    const frac = rect && img ? rect[1] / (img.naturalHeight / SCALE) : 0;
    const top = row.offsetTop + (page ? frac * page.clientHeight : 0) - 150;
    scroller.current.scrollTo({ top: Math.max(0, top), behavior: reduced() ? "auto" : "smooth" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cur, flash, done]);

  const step = (d: 1 | -1) => {
    const pos = cur === null ? -1 : order.indexOf(cur);
    const n = stepIndex(pos, d, order.length);
    if (n >= 0) select(order[n]);
  };
  const pos = cur === null ? -1 : order.indexOf(cur);

  // Swipe and onion skin are driven by CSS variables, so dragging never re-renders the pages.
  const setVar = (name: string, v: number) => scroller.current?.style.setProperty(name, String(v));
  const [slider, setSlider] = useState(0.5);
  useEffect(() => {
    setVar("--swipe", slider);
    setVar("--onion", slider);
  }, [slider, mode, done]);

  const pick = (side: "a" | "b", value: string) => {
    const src: Source = value === "current" ? { kind: "current" } : { kind: "version", id: value };
    h.setCompare(side === "a" ? { a: src } : { b: src });
  };
  const names = labelsOf(req, versions);

  return (
    <div className="hx-compare">
      <div className="hx-cbar">
        <strong className="hx-ctitle">Compare</strong>
        <div className="hx-seg inline" role="tablist" aria-label="View">
          {MODES.map((m) => (
            <button key={m.id} role="tab" aria-selected={mode === m.id} className={mode === m.id ? "on" : ""} onClick={() => setMode(m.id)}>
              {m.label}
            </button>
          ))}
        </div>
        <label className="hx-switch-row compact">
          <input type="checkbox" role="switch" checked={diffs} onChange={(e) => setDiffs(e.target.checked)} />
          <span className="hx-switch" aria-hidden />
          Show differences
        </label>
        {mode !== "twoup" && (
          <label className="hx-slider">
            <span className="dim">{mode === "swipe" ? "Position" : "Before"}</span>
            <input
              type="range"
              min={0}
              max={1}
              step={0.01}
              value={slider}
              aria-label={mode === "swipe" ? "Swipe position" : "Onion skin opacity"}
              onChange={(e) => setSlider(Number(e.target.value))}
            />
            {mode === "onion" && <span className="dim">After</span>}
          </label>
        )}
        <span className="grow" />
        <div className="hx-stepper" aria-label="Differences">
          <button className="icon-btn" title="Previous change" aria-label="Previous change" disabled={order.length === 0} onClick={() => step(-1)}>
            <Icon name="up" size={14} />
          </button>
          <span className="hx-step-label">{order.length ? (pos >= 0 ? `${pos + 1} of ${order.length}` : `${order.length} changes`) : "0 changes"}</span>
          <button className="icon-btn" title="Next change" aria-label="Next change" disabled={order.length === 0} onClick={() => step(1)}>
            <Icon name="down" size={14} />
          </button>
        </div>
        <button className="hx-btn" onClick={() => h.closeCompare()}>
          Done
        </button>
      </div>
      <div className="hx-cbody">
        <div className="hx-cview">
          <div className={`hx-tags${mode === "twoup" ? "" : " one"}`}>
            <Picker side="a" label="Before" req={req} versions={versions} name={names[0]} onPick={pick} />
            <Picker side="b" label="After" req={req} versions={versions} name={names[1]} onPick={pick} />
          </div>
          <div className="hx-cscroll" ref={scroller} data-mode={mode} onPointerMove={(e) => mode === "swipe" && swipeTo(e, scroller.current!)}>
            {cmp === "loading" && <p className="hx-cmsg dim">Comparing{"…"}</p>}
            {typeof cmp === "object" && "error" in cmp && <p className="hx-cmsg">{cmp.error}</p>}
            {done &&
              rows.map((row, i) => (
                <CompareRow
                  key={`${row.a}:${row.b}`}
                  index={i}
                  req={req}
                  row={row}
                  mode={mode}
                  marksA={marksA}
                  marksB={marksB}
                  diffs={diffs}
                  cur={cur}
                  flash={flash}
                  onMark={select}
                />
              ))}
          </div>
        </div>
        <aside className="hx-cpanel" aria-label="Changes">
          <header>
            <Icon name="file" size={18} />
            <h3>Changes</h3>
            {done && <span className="hx-chip">{done.changes.length}</span>}
          </header>
          {done && (
            <>
              {pageSummary(done).length > 0 && (
                <ul className="hx-summary">
                  {pageSummary(done).map((s) => (
                    <li key={s}>{s}</li>
                  ))}
                </ul>
              )}
              {counts && done.changes.length > 0 && (
                <div className="hx-seg hx-cats" role="group" aria-label="Filter by type">
                  <button className={!cats ? "on" : ""} aria-pressed={!cats} onClick={() => setCats(null)}>
                    All
                  </button>
                  {CATEGORIES.filter((c) => counts[c.id] > 0).map((c) => {
                    const on = !!cats && cats.size === 1 && cats.has(c.id);
                    return (
                      <button key={c.id} className={on ? "on" : ""} aria-pressed={on} onClick={() => setCats(on ? null : new Set([c.id]))}>
                        {c.label} <span className="n">{counts[c.id]}</span>
                      </button>
                    );
                  })}
                </div>
              )}
              <div className="hx-clist">
                {done.changes.length === 0 && (
                  <p className="hx-note dim">{done.identical ? "These two are the same: no page, text, annotation or form differences." : "No differences found."}</p>
                )}
                {groups.map((g) => (
                  <section key={g.page ?? "doc"}>
                    <h4 className="hx-day">
                      {g.page === null ? "Document" : `Page ${g.page + 1}`} <span className="n">{g.items.length}</span>
                    </h4>
                    {g.items.map(({ change, index }) => (
                      <RevRow key={index} change={change} on={cur === index} onClick={() => select(index)} />
                    ))}
                  </section>
                ))}
              </div>
            </>
          )}
        </aside>
      </div>
      {req.doc !== null && versions.length > 0 && <VersionTrack req={req} versions={versions} onSet={pick} />}
    </div>
  );
}

/** The versions along the bottom, oldest first by day. Click sets After; Alt/Shift-click or the toggle sets Before. */
function VersionTrack({ req, versions, onSet }: { req: CompareReq; versions: VersionInfo[]; onSet: (side: "a" | "b", value: string) => void }) {
  const [target, setTarget] = useState<"a" | "b">("b");
  const days = useMemo(() => trackDays(versions, Date.now()), [versions]);
  const strip = useRef<HTMLDivElement>(null);
  const is = (src: Source, id: string) => (id === "current" ? src.kind === "current" : src.kind === "version" && src.id === id);
  const move = (e: React.KeyboardEvent) => {
    const dir = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (!dir) return;
    const all = [...(strip.current?.querySelectorAll<HTMLButtonElement>("button.hx-node") ?? [])];
    const i = all.indexOf(document.activeElement as HTMLButtonElement);
    all[Math.min(all.length - 1, Math.max(0, i + dir))]?.focus();
    e.preventDefault();
  };
  const node = (id: string, title: string, time: string) => {
    const a = is(req.a, id);
    const b = is(req.b, id);
    return (
      <button
        key={id}
        className={`hx-node${a ? " is-a" : ""}${b ? " is-b" : ""}`}
        aria-pressed={a || b}
        aria-label={`${title}, ${time}${a ? ", Before" : ""}${b ? ", After" : ""}`}
        title={`${title} · click: After, Alt-click: Before`}
        onClick={(e) => onSet(e.altKey || e.shiftKey || target === "a" ? "a" : "b", id)}
      >
        <span className="hx-dot" aria-hidden>
          {a && <i className="hx-handle-a">Before</i>}
          {b && <i className="hx-handle-b">After</i>}
        </span>
        <span className="hx-node-time">{time}</span>
      </button>
    );
  };
  return (
    <div className="hx-track" aria-label="Versions">
      <strong className="hx-track-title">Versions</strong>
      <div className="hx-seg inline" role="group" aria-label="A click sets">
        <button className={target === "a" ? "on" : ""} aria-pressed={target === "a"} onClick={() => setTarget("a")}>
          Before
        </button>
        <button className={target === "b" ? "on" : ""} aria-pressed={target === "b"} onClick={() => setTarget("b")}>
          After
        </button>
      </div>
      <div className="hx-strip" ref={strip} onKeyDown={move}>
        {days.map((d) => (
          <div key={d.key} className="hx-track-day">
            <span className="hx-track-label">{d.label}</span>
            <div className="hx-track-items">{d.items.map((v) => node(v.id, versionTitle(v), clock(v.created)))}</div>
          </div>
        ))}
        <div className="hx-track-day">
          <span className="hx-track-label">Now</span>
          <div className="hx-track-items">{node("current", "Current file", "file")}</div>
        </div>
      </div>
    </div>
  );
}

/** The divider follows the pointer 1:1: a CSS variable, no re-render. */
function swipeTo(e: React.PointerEvent, scroller: HTMLElement) {
  const r = scroller.querySelector(".hx-swipe")?.getBoundingClientRect();
  if (!r) return;
  scroller.style.setProperty("--swipe", String(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width))));
}

function labelsOf(req: CompareReq, versions: VersionInfo[]): [string, string] {
  if (req.names) return req.names;
  const one = (s: Source) => {
    if (s.kind === "current") return "Current file";
    const v = versions.find((x) => x.id === (s.kind === "version" ? s.id : ""));
    return v ? `${versionTitle(v)} · ${when(v.created, Date.now())}` : "Version";
  };
  return [one(req.a), one(req.b)];
}

function Picker({
  side,
  label,
  req,
  versions,
  name,
  onPick,
}: {
  side: "a" | "b";
  label: string;
  req: CompareReq;
  versions: VersionInfo[];
  name: string;
  onPick: (side: "a" | "b", v: string) => void;
}) {
  const src = side === "a" ? req.a : req.b;
  const value = src.kind === "version" ? src.id : "current";
  return (
    <div className="hx-tag">
      <span className="hx-tag-label">{label}</span>
      {req.doc !== null && versions.length > 0 ? (
        <select value={value} aria-label={`${label} version`} onChange={(e) => onPick(side, e.target.value)}>
          <option value="current">Current file</option>
          {versions.map((v) => (
            <option key={v.id} value={v.id}>
              {versionTitle(v)} {"·"} {when(v.created, Date.now())}
            </option>
          ))}
        </select>
      ) : (
        <span className="hx-tag-name" title={name}>
          {name}
        </span>
      )}
    </div>
  );
}

let lastRatio = 1.294;

function CompareRow({
  index,
  req,
  row,
  mode,
  marksA,
  marksB,
  diffs,
  cur,
  flash,
  onMark,
}: {
  index: number;
  req: CompareReq;
  row: { a: number | null; b: number | null };
  mode: Mode;
  marksA: Mark[];
  marksB: Mark[];
  diffs: boolean;
  cur: number | null;
  flash: number;
  onMark: (i: number) => void;
}) {
  const el = useRef<HTMLDivElement>(null);
  const [ratio, setRatio] = useState(lastRatio);
  const [seen, setSeen] = useState(false);
  const [vb, setVb] = useState<Rect4[]>([]);
  const onRatio = (r: number) => {
    lastRatio = r;
    setRatio(r);
  };
  const ma = useMemo(() => (diffs ? marksA.filter((m) => m.page === row.a) : undefined), [diffs, marksA, row.a]);
  const mb = useMemo(() => (diffs ? marksB.filter((m) => m.page === row.b) : undefined), [diffs, marksB, row.b]);

  useEffect(() => {
    const n = el.current;
    if (!n) return;
    const io = new IntersectionObserver(([e]) => e.isIntersecting && setSeen(true), { rootMargin: "600px 0px" });
    io.observe(n);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    if (!diffs || !seen || row.a === null || row.b === null) return;
    let live = true;
    // regions a semantic change explains are kept however faint
    const hints = [...(ma ?? []), ...(mb ?? [])].flatMap((m) => (m.rect ? [m.rect] : []));
    historyVisualDiff(req.doc, req.a, row.a, req.b, row.b, 1, hints).then(
      (d) => live && setVb(d.boxes),
      () => {},
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [diffs, seen, row.a, row.b, sourceKey(req.a), sourceKey(req.b), ma, mb]);

  const vboxes = useMemo(() => (diffs ? uncovered(vb, [...(ma ?? []), ...(mb ?? [])]) : undefined), [diffs, vb, ma, mb]);
  const cell = (side: "a" | "b") => {
    const page = side === "a" ? row.a : row.b;
    if (page === null)
      return (
        <div className="hx-page blank" style={{ aspectRatio: `1 / ${ratio}` }}>
          <span>{side === "a" ? "Page inserted" : "Page removed"}</span>
        </div>
      );
    return (
      <PageImg
        doc={req.doc}
        source={side === "a" ? req.a : req.b}
        page={page}
        ratio={ratio}
        marks={side === "a" ? ma : mb}
        vboxes={vboxes}
        current={cur}
        flash={flash}
        onRatio={onRatio}
        onMark={onMark}
        label={`${side === "a" ? "Before" : "After"}, page ${page + 1}`}
      />
    );
  };

  return (
    <div ref={el} className="hx-crow" data-row={index}>
      {mode === "twoup" ? (
        <>
          <div className="hx-cell">{cell("a")}</div>
          <div className="hx-cell">{cell("b")}</div>
        </>
      ) : (
        <div className={`hx-cell single ${mode === "swipe" ? "hx-swipe" : "hx-onion"}`}>
          {mode === "swipe" ? (
            <>
              {cell("b")}
              <div className="hx-over hx-swipe-top">{cell("a")}</div>
              <div className="hx-handle" aria-hidden>
                <i />
              </div>
            </>
          ) : (
            <>
              {cell("a")}
              <div className="hx-over hx-onion-top">{cell("b")}</div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
