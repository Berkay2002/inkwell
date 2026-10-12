import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { pickOpen } from "../../lib/api";
import {
  type CompareProgress,
  type ComparePages,
  type Comparison,
  type Rect4,
  type Source,
  type VersionInfo,
  compareCancel,
  historyCompareReport,
  historyCompareRun,
  historyVisualDiff,
  sourceKey,
} from "../../lib/history";
import {
  CATEGORIES,
  type Category,
  type Mark,
  alignRows,
  categoryCounts,
  differencesWord,
  flatOrder,
  groupChanges,
  marksFor,
  pageSummary,
  progressFraction,
  progressLabel,
  rangeField,
  rowOf,
  stepIndex,
  summaryChips,
  uncovered,
  versionTitle,
  when,
} from "../../lib/historyLogic";
import { type CompareReq, useHistory } from "../../lib/historyStore";
import { useStore } from "../../store";
import { Icon } from "../Icons";
import { CompareSetup } from "./CompareSetup";
import { PageImg, SCALE, clearPageCache } from "./PageImg";
import { PageStrips, clearThumbs } from "./PageStrip";
import { RevRow } from "./RevRow";

type Mode = "twoup" | "swipe" | "onion";
const MODES: { id: Mode; label: string }[] = [
  { id: "twoup", label: "Side by side" },
  { id: "swipe", label: "Slider" },
  { id: "onion", label: "Overlay" },
];

const EMPTY_VERSIONS: VersionInfo[] = [];
/** Job ids of this view; far from the tools' counters so `tool_cancel` never meets another job. */
let jobSeq = 2_000_000_000;
const FILE = "__file";
const toast = (error: string) => useStore.getState().setUi({ error });
const baseName = (p: string) => p.split(/[\/]/).pop() || "file";
const reduced = () => matchMedia("(prefers-reduced-motion: reduce)").matches;
/** A side whose source changed starts with all of its pages again. */
const withoutRange = (ranges: [string, string], side: "a" | "b"): [string, string] => (side === "a" ? ["", ranges[1]] : [ranges[0], ""]);

/** Compare mode: the whole window. First the setup page when asked for, then the comparison. */
export function CompareView() {
  const setup = useHistory((s) => s.compare?.stage === "setup");
  return setup ? <CompareSetup /> : <CompareResult />;
}

/** Two sources, their differences on the pages and in a list. */
function CompareResult() {
  const req = useHistory((s) => s.compare)!;
  const versions = useHistory((s) => (req.doc != null ? (s.docs[req.doc]?.versions ?? EMPTY_VERSIONS) : EMPTY_VERSIONS));
  const h = useHistory.getState();
  const [mode, setMode] = useState<Mode>("twoup");
  const [diffs, setDiffs] = useState(true);
  const [cmp, setCmp] = useState<Comparison | "loading" | { error: string }>("loading");
  const [progress, setProgress] = useState<CompareProgress | null>(null);
  const [job, setJob] = useState(0);
  const [report, setReport] = useState<{ job: number; progress: CompareProgress | null } | null>(null);
  const [run, setRun] = useState(0);
  const [cur, setCur] = useState<number | null>(null);
  const [flash, setFlash] = useState(0);
  const [cats, setCats] = useState<Set<Category> | null>(() => (req.show ? new Set(req.show) : null));
  const [active, setActive] = useState(0);
  const scroller = useRef<HTMLDivElement>(null);
  const aKey = sourceKey(req.a);
  const bKey = sourceKey(req.b);
  // The fields were checked on the setup page; a page past the end of its file is refused by the engine.
  const ranges = req.ranges ?? ["", ""];
  const pages: ComparePages = [rangeField(ranges[0], null).pages, rangeField(ranges[1], null).pages];
  const pagesKey = JSON.stringify(pages);

  useEffect(
    () => () => {
      clearPageCache();
      clearThumbs();
    },
    [],
  );

  useEffect(() => {
    let live = true;
    let finished = false;
    const id = ++jobSeq;
    setCmp("loading");
    setCur(null);
    setProgress(null);
    setJob(id);
    historyCompareRun(id, req.doc, req.a, req.b, pages, (p) => live && setProgress(p)).then(
      (c) => {
        finished = true;
        if (!live) return;
        setCmp(c);
        setCur(c.changes.length ? 0 : null);
      },
      (e) => {
        finished = true;
        if (live) setCmp({ error: String(e) });
      },
    );
    return () => {
      live = false;
      // a job that already finished has nothing to stop
      if (!finished) void compareCancel(id).catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [req.doc, aKey, bKey, pagesKey, run]);

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
    // flash is 0 until the user steps or clicks: the first change is selected silently, the pages stay at scroll 0
    if (cur === null || !flash || !done || !scroller.current) return;
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

  const changedPages = useMemo<[Set<number>, Set<number>]>(() => {
    const a = new Set<number>();
    const b = new Set<number>();
    for (const c of done?.changes ?? []) {
      if (c.aPage !== null) a.add(c.aPage);
      if (c.bPage !== null) b.add(c.bPage);
    }
    return [a, b];
  }, [done]);

  const scrollToRow = (r: number) => {
    const row = scroller.current?.querySelector<HTMLElement>(`[data-row="${r}"]`);
    if (!row || !scroller.current) return;
    setActive(r);
    scroller.current.scrollTo({ top: Math.max(0, row.offsetTop - 12), behavior: reduced() ? "auto" : "smooth" });
  };

  // The row at the top of the panes lights its pages in the strips; one frame at most per scroll burst.
  const frame = useRef(0);
  const onScroll = () => {
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      const s = scroller.current;
      if (!s) return;
      const rowsEl = s.querySelectorAll<HTMLElement>(".hx-crow");
      let lo = 0;
      let hi = rowsEl.length - 1;
      const line = s.scrollTop + s.clientHeight * 0.5;
      while (lo < hi) {
        const mid = (lo + hi + 1) >> 1;
        if (rowsEl[mid].offsetTop <= line) lo = mid;
        else hi = mid - 1;
      }
      setActive(lo);
    });
  };
  useEffect(() => () => cancelAnimationFrame(frame.current), []);

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

  const names = labelsOf(req, versions);
  const chooseFile = async (side: "a" | "b") => {
    try {
      const f = (await pickOpen(false, "compare"))[0];
      if (!f) return;
      const next: [string, string] = side === "a" ? [baseName(f.path), names[1]] : [names[0], baseName(f.path)];
      h.setCompare({ [side]: { kind: "picked", token: f.token, path: f.path }, names: next, ranges: withoutRange(ranges, side) });
    } catch (e) {
      toast(`Could not pick the file: ${String(e)}`);
    }
  };
  const pick = (side: "a" | "b", value: string) => {
    if (value === FILE) return void chooseFile(side);
    const src: Source = value === "current" ? { kind: "current" } : { kind: "version", id: value };
    // a picked file's name does not describe the document or version that replaces it
    const next: [string, string] = side === "a" ? ["", names[1]] : [names[0], ""];
    h.setCompare(side === "a" ? { a: src, names: next, ranges: withoutRange(ranges, side) } : { b: src, names: next, ranges: withoutRange(ranges, side) });
  };
  const swap = () => h.setCompare({ a: req.b, b: req.a, names: [names[1], names[0]], ranges: [ranges[1], ranges[0]] });
  const exportReport = async () => {
    const id = ++jobSeq;
    const stem = (names[1] || "comparison").replace(/\.pdf$/i, "");
    setReport({ job: id, progress: null });
    try {
      // the report's header names the pages a ranged side covers
      const shown = names.map((n, i) => (pages[i] ? `${n} (pages ${ranges[i].trim()})` : n)) as [string, string];
      const dest = await historyCompareReport(id, req.doc, req.a, req.b, pages, shown, `${stem} comparison report.pdf`, (progress) =>
        setReport((r) => (r && r.job === id ? { job: id, progress } : r)),
      );
      if (dest) toast(`Saved the comparison report as ${baseName(dest)}`);
    } catch (e) {
      if (!/cancelled/.test(String(e))) toast(`Could not write the report: ${String(e)}`);
    } finally {
      setReport(null);
    }
  };

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
            <span className="dim">{mode === "swipe" ? "Position" : "Old"}</span>
            <input
              type="range"
              min={0}
              max={1}
              step={0.01}
              value={slider}
              aria-label={mode === "swipe" ? "Slider position" : "Overlay: old to new"}
              onChange={(e) => setSlider(Number(e.target.value))}
            />
            {mode === "onion" && <span className="dim">New</span>}
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
      {done && (
        <div className="hx-csum" aria-label="Summary">
          <strong>{differencesWord(done.changes.length)}</strong>
          {counts &&
            summaryChips(counts).map((c) => {
              const on = !!cats && cats.size === 1 && cats.has(c.id);
              return (
                <button key={c.id} className={`hx-chip btn${on ? " on" : ""}`} aria-pressed={on} onClick={() => setCats(on ? null : new Set([c.id]))}>
                  {c.n} {c.label}
                </button>
              );
            })}
          <span className="grow" />
          {report ? (
            <>
              <span className="dim">{progressLabel(report.progress)}</span>
              <button className="hx-btn" onClick={() => void compareCancel(report.job)}>
                Cancel
              </button>
            </>
          ) : (
            <button className="hx-btn" disabled={done.changes.length === 0 && done.identical} onClick={() => void exportReport()}>
              Export report
            </button>
          )}
        </div>
      )}
      <div className="hx-cbody">
        <div className="hx-cview">
          <div className={`hx-tags${mode === "twoup" ? "" : " one"}`}>
            <Picker side="a" label="Old" req={req} versions={versions} name={names[0]} onPick={pick} />
            <Picker side="b" label="New" req={req} versions={versions} name={names[1]} onPick={pick} swap={swap} />
          </div>
          <div
            className="hx-cscroll"
            ref={scroller}
            data-mode={mode}
            onScroll={onScroll}
            onPointerMove={(e) => mode === "swipe" && swipeTo(e, scroller.current!)}
          >
            {cmp === "loading" && (
              <div className="hx-cprog" role="status">
                <progress max={1} value={progressFraction(progress)} aria-label="Comparison progress" />
                <span className="dim">{progressLabel(progress)}</span>
                <button className="hx-btn" onClick={() => void compareCancel(job)}>
                  Cancel
                </button>
              </div>
            )}
            {typeof cmp === "object" && "error" in cmp && (
              <div className="hx-cmsg">
                <p>{/cancelled/.test(cmp.error) ? "The comparison was cancelled." : cmp.error}</p>
                {/cancelled/.test(cmp.error) && (
                  <button className="hx-btn" onClick={() => setRun((n) => n + 1)}>
                    Compare again
                  </button>
                )}
              </div>
            )}
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
      {done && (
        <PageStrips
          doc={req.doc}
          a={req.a}
          b={req.b}
          names={names}
          ids={[done.pages.aIds, done.pages.bIds]}
          rows={rows}
          active={active}
          changed={changedPages}
          onGo={scrollToRow}
        />
      )}
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
  const one = (s: Source, i: number) => {
    if (s.kind === "picked") return req.names?.[i] || "File";
    if (s.kind === "current") return req.names?.[i] || "Current file";
    const v = versions.find((x) => x.id === (s.kind === "version" ? s.id : ""));
    return v ? `${versionTitle(v)} · ${when(v.created, Date.now())}` : "Version";
  };
  return [one(req.a, 0), one(req.b, 1)];
}

function Picker({
  side,
  label,
  req,
  versions,
  name,
  onPick,
  swap,
}: {
  side: "a" | "b";
  label: string;
  req: CompareReq;
  versions: VersionInfo[];
  name: string;
  onPick: (side: "a" | "b", v: string) => void;
  swap?: () => void;
}) {
  const src = side === "a" ? req.a : req.b;
  const value = src.kind === "version" ? src.id : src.kind === "picked" ? FILE : "current";
  return (
    <div className="hx-tag">
      <span className="hx-tag-label">{label}</span>
      {req.doc !== null ? (
        <select value={value} aria-label={`${label} file or version`} onChange={(e) => onPick(side, e.target.value)}>
          <option value="current">Current file</option>
          {versions.map((v) => (
            <option key={v.id} value={v.id}>
              {versionTitle(v)} {"·"} {when(v.created, Date.now())}
            </option>
          ))}
          <option value={FILE}>{src.kind === "picked" ? name : "Another file\u2026"}</option>
        </select>
      ) : (
        <>
          <span className="hx-tag-name" title={name}>
            {name}
          </span>
          <button className="hx-btn" onClick={() => onPick(side, FILE)}>
            Change{"…"}
          </button>
        </>
      )}
      {swap && (
        <button className="icon-btn hx-swap" title="Swap before and after" aria-label="Swap before and after" onClick={swap}>
          <Icon name="swap" size={14} />
        </button>
      )}
    </div>
  );
}

let lastRatio = 1.294;

interface Link {
  change: number;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}
const sameLinks = (a: Link[], b: Link[]) =>
  a.length === b.length && a.every((l, i) => l.change === b[i].change && l.x1 === b[i].x1 && l.y1 === b[i].y1 && l.x2 === b[i].x2 && l.y2 === b[i].y2);

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

  // Dashed connectors: a change that has a box on both pages is tied across the gap, along the foot of its boxes (so the line
  // never reads as a strike-through), measured from the boxes themselves.
  const [links, setLinks] = useState<Link[]>([]);
  useLayoutEffect(() => {
    const row = el.current;
    if (!row || mode !== "twoup" || !diffs) return setLinks((l) => (l.length ? [] : l));
    let raf = 0;
    const measure = () => {
      raf = 0;
      const cells = row.querySelectorAll<HTMLElement>(":scope > .hx-cell");
      if (cells.length !== 2) return;
      const at = row.getBoundingClientRect();
      const boxes = (cell: HTMLElement) => {
        const m = new Map<number, DOMRect[]>();
        for (const n of cell.querySelectorAll<HTMLElement>(".hx-mark:not(.whole)")) {
          const k = Number(n.dataset.change);
          const l = m.get(k) ?? [];
          l.push(n.getBoundingClientRect());
          m.set(k, l);
        }
        return m;
      };
      const A = boxes(cells[0]);
      const B = boxes(cells[1]);
      const out: Link[] = [];
      for (const [change, ra] of A) {
        const rb = B.get(change);
        if (!rb) continue;
        for (let i = 0; i < Math.min(ra.length, rb.length); i++) {
          out.push({
            change,
            x1: Math.round(ra[i].right - at.left),
            y1: Math.round(ra[i].bottom - at.top),
            x2: Math.round(rb[i].left - at.left),
            y2: Math.round(rb[i].bottom - at.top),
          });
        }
      }
      setLinks((prev) => (sameLinks(prev, out) ? prev : out));
    };
    const sched = () => {
      if (!raf) raf = requestAnimationFrame(measure);
    };
    const mo = new MutationObserver(sched);
    mo.observe(row, { childList: true, subtree: true });
    const ro = new ResizeObserver(sched);
    ro.observe(row);
    sched();
    return () => {
      mo.disconnect();
      ro.disconnect();
      cancelAnimationFrame(raf);
    };
  }, [mode, diffs]);
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
      {mode === "twoup" && links.length > 0 && (
        <svg className="hx-links" aria-hidden>
          {links.map((l, i) => (
            <line key={i} className={l.change === cur ? "cur" : undefined} x1={l.x1} y1={l.y1} x2={l.x2} y2={l.y2} />
          ))}
        </svg>
      )}
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
