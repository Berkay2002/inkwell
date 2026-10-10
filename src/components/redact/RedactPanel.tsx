import { useEffect, useMemo, useRef, useState } from "react";
import { parseRange } from "../../lib/pageRange";
import { type ApplyOutcome, type Hit, type Mark, type SanitizeReport, PATTERNS, markLabel, useRedact } from "../../lib/redact";
import { revealInFolder } from "../../recents";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import "./redact.css";

const st = () => useRedact.getState();

/** The toolbar's Redact button. */
export function RedactToggle() {
  const open = useRedact((s) => s.open);
  return (
    <button className={open ? "icon-btn on" : "icon-btn"} title="Redact" aria-label="Redact" aria-pressed={open} onClick={() => st().setOpen(!open)}>
      <Icon name="redact" />
    </button>
  );
}

const baseName = (p: string) => p.split(/[\\/]/).pop() || p;
const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

export function RedactPanel({ doc }: { doc: DocState }) {
  const open = useRedact((s) => s.open);
  const phase = useRedact((s) => s.docs[doc.id]?.phase ?? "marking");
  const id = doc.id;

  // What is marked already, and how many of each pattern there are.
  useEffect(() => {
    if (!open) return;
    void st().refreshMarks(id);
    if (phase === "marking") void st().loadCounts(id);
  }, [open, id, phase]);

  if (!open) return null;
  return (
    <aside className="ax-dock rd-dock" style={{ width: "min(400px, 45vw)" }} aria-label="Redact">
      <section className="ax-panel docked rd-panel">
        <div className="ax-head">
          <span className="ax-title">Redact</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Redact" onClick={() => st().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>
        {phase === "report" ? <Result doc={doc} /> : phase === "refused" ? <Refused doc={doc} /> : <Marking doc={doc} />}
      </section>
    </aside>
  );
}

function Marking({ doc }: { doc: DocState }) {
  const id = doc.id;
  const d = useRedact((s) => s.docs[id]);
  const marks = d?.marks ?? [];
  const phase = d?.phase ?? "marking";
  const applying = phase === "confirm" || phase === "applying";
  return (
    <>
      <div className="rd-body">
        <Tools doc={doc} />
        <Find doc={doc} />
        <Marks doc={doc} />
        {applying && <ApplySection doc={doc} />}
        {d?.error && (
          <p className="rd-error" role="alert">
            {d.error}
          </p>
        )}
      </div>
      {!applying && (
        <div className="rd-foot">
          <span className="dim">{plural(marks.length, "mark")}</span>
          <span className="grow" />
          <button className="rd-btn primary" disabled={marks.length === 0} onClick={() => void st().confirm(id)}>
            Apply {plural(marks.length, "mark")}
          </button>
        </div>
      )}
    </>
  );
}

function Tools({ doc }: { doc: DocState }) {
  const tool = useRedact((s) => s.tool);
  const [pagesOpen, setPagesOpen] = useState(false);
  const [text, setText] = useState("");
  const [bad, setBad] = useState(false);
  const markPages = () => {
    const pages = text.trim() ? parseRange(text, doc.pageCount) : [doc.currentPage];
    if (!pages || pages.length === 0) return setBad(true);
    setBad(false);
    setText("");
    void st().mark(doc.id, { kind: "pages", pages });
  };
  return (
    <div className="rd-tools">
      <div className="rd-tool-row">
        <button className={tool ? "rd-tool on" : "rd-tool"} aria-pressed={tool} onClick={() => st().setTool(!tool)}>
          <span className="rd-round">
            <Icon name="area" size={18} />
          </span>
          Mark text or area
        </button>
        <button className={pagesOpen ? "rd-tool on" : "rd-tool"} aria-pressed={pagesOpen} onClick={() => setPagesOpen(!pagesOpen)}>
          <span className="rd-round">
            <Icon name="file" size={18} />
          </span>
          Mark pages
        </button>
      </div>
      {tool && <p className="rd-hint dim">Drag over the page to mark an area.</p>}
      {pagesOpen && (
        <div className="rd-pages">
          <input
            className={bad ? "rd-input bad" : "rd-input"}
            value={text}
            placeholder={`Page ${doc.currentPage + 1}, or e.g. 2, 5-7`}
            aria-label="Pages to mark"
            aria-invalid={bad}
            onChange={(e) => {
              setText(e.target.value);
              setBad(false);
            }}
            onKeyDown={(e) => e.key === "Enter" && markPages()}
          />
          <button className="rd-btn" onClick={markPages}>
            Mark
          </button>
        </div>
      )}
    </div>
  );
}

function Find({ doc }: { doc: DocState }) {
  const id = doc.id;
  const d = useRedact((s) => s.docs[id]);
  const [q, setQ] = useState("");
  const query = d?.query ?? null;
  const hits = d?.hits ?? [];
  const checked = d?.checked ?? [];
  const run = useRef(0);

  // Typing searches the text after a short pause; an empty box clears the list.
  useEffect(() => {
    const n = ++run.current;
    const t = setTimeout(() => {
      if (n !== run.current) return;
      if (q.trim()) void st().find(id, { kind: "text", needle: q.trim(), caseSensitive: false, wholeWord: false });
      else if (st().get(id).query?.kind === "text") st().clearFind(id);
    }, 280);
    return () => clearTimeout(t);
  }, [q, id]);

  const pages = useMemo(() => new Set(hits.map((h) => h.page)).size, [hits]);
  const groups = useMemo(() => {
    const g: { page: number; items: { hit: Hit; i: number }[] }[] = [];
    hits.forEach((hit, i) => {
      const last = g[g.length - 1];
      if (last && last.page === hit.page) last.items.push({ hit, i });
      else g.push({ page: hit.page, items: [{ hit, i }] });
    });
    return g;
  }, [hits]);

  const pill = (id2: (typeof PATTERNS)[number]["id"]) => {
    if (query?.kind === "pattern" && query.pattern === id2) return st().clearFind(id);
    setQ("");
    void st().find(id, { kind: "pattern", pattern: id2 });
  };

  return (
    <div className="rd-find">
      <label className="rd-search">
        <Icon name="search" size={15} />
        <input value={q} placeholder="Find text or a pattern" spellCheck={false} onChange={(e) => setQ(e.target.value)} aria-label="Find text to redact" />
      </label>
      <div className="rd-pills">
        {PATTERNS.map((p) => {
          const n = d?.counts[p.id];
          const on = query?.kind === "pattern" && query.pattern === p.id;
          return (
            <button key={p.id} className={on ? "rd-pill on" : "rd-pill"} aria-pressed={on} onClick={() => pill(p.id)}>
              {p.label}
              {n !== undefined && <span className={n === 0 ? "rd-count zero" : "rd-count"}>{n}</span>}
            </button>
          );
        })}
      </div>
      {query && (
        <>
          <div className="rd-found">
            <span>{hits.length === 0 ? "Nothing found" : `${hits.length} found on ${plural(pages, "page")}`}</span>
            <span className="grow" />
            <button className="rd-btn primary" disabled={checked.length === 0} onClick={() => void st().markChecked(id)}>
              Mark {checked.length}
            </button>
          </div>
          <div className="rd-hits">
            {groups.map((g) => (
              <div key={g.items[0].i} className="rd-group">
                {g.items.map(({ hit, i }) => (
                  <div key={i} className="rd-hit">
                    <input type="checkbox" checked={checked.includes(i)} aria-label={`Mark ${hit.text}`} onChange={() => st().toggleHit(id, i)} />
                    <button
                      className="rd-snippet"
                      title="Show on the page"
                      onClick={() => useStore.getState().goTo(id, hit.page, Math.max(0, Math.min(...hit.quads.map((x) => x[1])) - 80))}
                    >
                      {hit.text}
                    </button>
                    <span className="rd-chip">p. {hit.page + 1}</span>
                  </div>
                ))}
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}

function Marks({ doc }: { doc: DocState }) {
  const d = useRedact((s) => s.docs[doc.id]);
  const marks = d?.marks ?? [];
  const labels = d?.labels ?? {};
  if (marks.length === 0) return <p className="rd-none dim">Nothing is marked yet. Marked text stays in the file until you apply the marks.</p>;
  return (
    <div className="rd-marks">
      <h4>{plural(marks.length, "mark")}</h4>
      <ul>
        {marks.map((m) => (
          <MarkRow key={`${m.page}:${m.xref}`} doc={doc.id} m={m} label={markLabel(m, labels)} />
        ))}
      </ul>
    </div>
  );
}

function MarkRow({ doc, m, label }: { doc: number; m: Mark; label: string }) {
  const kind = m.wholePage ? "file" : label.startsWith("Area") ? "area" : "textT";
  return (
    <li className="rd-mark-row">
      <span className="rd-kind">
        <Icon name={kind} size={15} />
      </span>
      <button className="rd-mark-text" title="Show on the page" onClick={() => useStore.getState().goTo(doc, m.page)}>
        {label}
      </button>
      <button className="ax-icon small" title="Unmark" aria-label={`Unmark ${label}`} onClick={() => void st().unmark(doc, [m])}>
        <Icon name="close" size={13} />
      </button>
    </li>
  );
}

// What Apply takes out besides the marked content, in the order shown.
const ROWS: [string, string][] = [
  ["metadata", "Metadata"],
  ["embeddedFiles", "Attachments"],
  ["hiddenLayerBlocks", "Hidden layers"],
  ["hiddenTextPages", "Hidden text"],
  ["annotations", "Comments"],
  ["formFields", "Form fields"],
  ["bookmarks", "Bookmarks"],
  ["scriptsActions", "Links and scripts"],
  ["quoting", "Other mentions"],
];
const count = (r: SanitizeReport | undefined, k: string) => (typeof r?.[k] === "number" ? (r[k] as number) : 0);

function ApplySection({ doc }: { doc: DocState }) {
  const id = doc.id;
  const d = useRedact((s) => s.docs[id]);
  const ref = useRef<HTMLElement>(null);
  useEffect(() => {
    ref.current?.scrollIntoView({ block: "nearest" });
  }, []);
  if (!d?.options) return null;
  const n = d.marks.length;
  const busy = d.phase === "applying";
  const on = Object.values(d.options.sanitize).some(Boolean);
  return (
    <section ref={ref} className="rd-apply" aria-label={`Apply ${plural(n, "mark")}`}>
      <h3>Redact {plural(n, "mark")} for good?</h3>
      <p className="dim">The marked text, images and pages are removed from the file. This can't be undone in the saved file.</p>
      <label className="rd-check">
        <input type="checkbox" checked={on} disabled={busy} onChange={(e) => void st().removeHidden(id, e.target.checked)} />
        Also remove hidden information
      </label>
      {on && (
        <ul className="rd-rows">
          {ROWS.map(([k, label]) => {
            const c = count(d.options?.preview, k);
            return (
              <li key={k} className={c === 0 ? "zero" : ""}>
                <span>{label}</span>
                <span className="rd-num">{c}</span>
              </li>
            );
          })}
        </ul>
      )}
      <p className="rd-saves dim">
        Saves as <strong>{d.options.saveName}</strong>
      </p>
      <div className="rd-actions">
        <button className="rd-btn" disabled={busy} onClick={() => st().cancel(id)}>
          Cancel
        </button>
        <button className="rd-btn primary" disabled={busy} onClick={() => void st().apply(id)}>
          {busy ? "Redacting…" : "Redact"}
        </button>
      </div>
    </section>
  );
}

function Result({ doc }: { doc: DocState }) {
  const o = useRedact((s) => s.docs[doc.id]?.outcome) as Extract<ApplyOutcome, { status: "applied" }> | null | undefined;
  if (!o || o.status !== "applied") return null;
  const removed = ROWS.map(([k, label]) => [label, count(o.sanitize, k)] as const).filter(([, c]) => c > 0);
  const leaks = o.verify.leaks;
  return (
    <div className="rd-body">
      <section className="rd-card result">
        <h3>{plural(o.marks, "mark")} redacted</h3>
        <p className={leaks.length ? "rd-checked warn" : "rd-checked"}>
          <span className="rd-tick">
            <Icon name="check" size={13} />
          </span>
          {leaks.length === 0 ? "Checked: none of the marked text is left in the file." : `Checked: ${plural(leaks.length, "place")} still mention marked text.`}
        </p>
        {leaks.length > 0 && <LeakList leaks={leaks} />}
        <h4>Also removed</h4>
        {removed.length === 0 ? (
          <p className="dim">Nothing else needed removing.</p>
        ) : (
          <ul className="rd-rows">
            {removed.map(([label, c]) => (
              <li key={label}>
                <span>{label}</span>
                <span className="rd-num">{c}</span>
              </li>
            ))}
          </ul>
        )}
        {o.savedTo && (
          <p className="rd-saves">
            <span className="dim">Saved as</span>
            <strong title={o.savedTo}>{baseName(o.savedTo)}</strong>
          </p>
        )}
        <div className="rd-actions">
          {o.savedTo && (
            <button className="rd-btn" onClick={() => void revealInFolder(o.savedTo!)}>
              Show in folder
            </button>
          )}
          <button className="rd-btn primary" onClick={() => st().done(doc.id)}>
            Done
          </button>
        </div>
      </section>
    </div>
  );
}

function LeakList({ leaks }: { leaks: { needle: string; place: string; critical: boolean }[] }) {
  const sorted = [...leaks].sort((a, b) => Number(b.critical) - Number(a.critical));
  return (
    <ul className="rd-leaks">
      {sorted.map((l, i) => (
        <li key={i} className={l.critical ? "critical" : ""}>
          <span className="rd-leak-needle">{l.needle}</span>
          <span className="dim">{l.place}</span>
          {l.critical && <span className="rd-chip crit">Critical</span>}
        </li>
      ))}
    </ul>
  );
}

function Refused({ doc }: { doc: DocState }) {
  const o = useRedact((s) => s.docs[doc.id]?.outcome);
  const leaks = o?.status === "refused" ? o.leaks : [];
  return (
    <div className="rd-body">
      <section className="rd-card refused" role="alert">
        <h3>Nothing was saved</h3>
        <p>
          Some of the marked text would still be in the file after redacting, so Inkwell stopped. Your file is unchanged and your marks are still in place.
        </p>
        <h4>Still found ({leaks.length})</h4>
        <LeakList leaks={leaks} />
        <div className="rd-actions">
          <button className="rd-btn primary" onClick={() => st().cancel(doc.id)}>
            Back to marks
          </button>
        </div>
      </section>
    </div>
  );
}
