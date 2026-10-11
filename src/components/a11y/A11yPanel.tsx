import { useEffect, useMemo, useRef, useState } from "react";
import { MenuButton } from "../Menu";
import { useA11y } from "../../lib/a11y";
import {
  type Finding,
  type Group,
  type Report,
  type RuleResult,
  LANGS,
  STATUS_WORD,
  autoCheckDue,
  autotagText,
  counts,
  fixAllText,
  fixEffect,
  fixFor,
  groupRules,
  isOverflowNote,
  langLabel,
  progressFraction,
  progressText,
  summaryLine,
  takesAlt,
  whereText,
} from "../../lib/a11yLogic";
import { renderPage } from "../../lib/history";
import { revealInFolder } from "../../recents";
import type { DocState } from "../../store";
import { Icon } from "../Icons";
import "./a11y.css";

const st = () => useA11y.getState();

/** How many findings of a rule show before "Show all". */
const FIRST = 5;

export function A11yPanel({ doc }: { doc: DocState }) {
  const open = useA11y((s) => s.open);
  const d = useA11y((s) => s.docs[doc.id]);
  const id = doc.id;
  const phase = d?.phase ?? "idle";

  // Read when the rail opens and again whenever the document changes (a fix, undo, an edit).
  useEffect(() => {
    if (!open) return;
    // A cancelled check or one that failed at this revision waits for Check again or the next edit.
    if (autoCheckDue(d, doc.rev)) void st().check(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, id, doc.rev, phase]);

  if (!open) return null;
  const report = d?.report ?? null;
  return (
    <aside className="ax-dock a11y-dock" style={{ width: "min(400px, 45vw)" }} aria-label="Accessibility">
      <section className="ax-panel docked a11y-panel">
        <div className="ax-head">
          <span className="ax-title">Accessibility</span>
          <span className="grow" />
          <button className="a11y-quiet" disabled={phase !== "idle"} onClick={() => void st().check(id)}>
            Check again
          </button>
          <button className="ax-icon" title="Close" aria-label="Close Accessibility" onClick={() => st().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="a11y-body">
          {report ? <Body doc={doc} report={report} /> : <Waiting doc={doc} />}
        </div>
        {report && <Foot doc={doc} report={report} />}
      </section>
    </aside>
  );
}

function Waiting({ doc }: { doc: DocState }) {
  const d = useA11y((s) => s.docs[doc.id]);
  if (d?.error) return <p className="a11y-error" role="alert">{d.error}</p>;
  // Idle without a report: the check was cancelled, and waits for the person.
  if ((d?.phase ?? "idle") === "idle") {
    return (
      <div className="a11y-card">
        <p>The check was cancelled.</p>
        <div className="a11y-actions">
          <button className="a11y-btn primary" onClick={() => void st().check(doc.id)}>
            Check accessibility
          </button>
        </div>
      </div>
    );
  }
  return <Running doc={doc} what="check" />;
}

/** A slim progress bar with its label and Cancel. */
function Running({ doc, what }: { doc: DocState; what: "check" | "autotag" | "export" }) {
  const d = useA11y((s) => s.docs[doc.id]);
  const f = progressFraction(d?.progress ?? null, what);
  return (
    <div className="a11y-run" role="status">
      <div className="a11y-run-row">
        <span>{progressText(d?.progress ?? null, what)}</span>
        <span className="grow" />
        <button className="a11y-quiet" onClick={() => st().cancel(doc.id)}>
          Cancel
        </button>
      </div>
      <div className="a11y-bar" aria-hidden="true">
        <i style={{ width: `${Math.max(3, f * 100)}%` }} />
      </div>
    </div>
  );
}

function Body({ doc, report }: { doc: DocState; report: Report }) {
  const d = useA11y((s) => s.docs[doc.id])!;
  const groups = useMemo(() => groupRules(report), [report]);
  const offer = !report.tagged && d.preview?.untagged && !d.notNow;
  // A report that no longer matches the document (its check was cancelled or failed) is not acted on.
  const stale = d.checkedRev !== doc.rev;
  const busy = d.phase !== "idle" || stale;
  return (
    <>
      {d.phase === "autotagging" ? (
        <div className="a11y-card">
          <b>Tagging this document</b>
          <Running doc={doc} what="autotag" />
        </div>
      ) : offer && d.preview ? (
        <div className="a11y-card">
          <b>This document is not tagged</b>
          <p>{autotagText(d.preview)}</p>
          <div className="a11y-actions">
            <button className="a11y-btn primary" disabled={busy} onClick={() => void st().autotag(doc.id)}>
              Autotag (best effort)
            </button>
            <button className="a11y-quiet" onClick={() => st().dismissAutotag(doc.id)}>
              Not now
            </button>
          </div>
        </div>
      ) : (
        <Summary report={report} checking={d.phase === "checking"} />
      )}
      {d.phase === "checking" && <Running doc={doc} what="check" />}
      {stale && d.phase === "idle" && (
        <p className="a11y-note">
          The document changed since this check.{" "}
          <button className="a11y-link" onClick={() => void st().check(doc.id)}>
            Check again
          </button>
        </p>
      )}
      {d.phase === "exporting" && <Running doc={doc} what="export" />}
      {d.error && (
        <p className="a11y-error" role="alert">
          {d.error}
        </p>
      )}
      {d.exported && d.phase === "idle" && (
        <p className="a11y-note">
          Report saved.{" "}
          <button className="a11y-link" onClick={() => revealInFolder(d.exported!)}>
            Show in folder
          </button>
        </p>
      )}
      {groups.map((g) => (
        <GroupView key={g.name} doc={doc} group={g} />
      ))}
    </>
  );
}

function Summary({ report, checking }: { report: Report; checking: boolean }) {
  const c = counts(report);
  const total = Math.max(1, report.results.length);
  const seg = (n: number, cls: string) => (n > 0 ? <i className={cls} style={{ flexGrow: n }} /> : null);
  return (
    <div className="a11y-summary" aria-busy={checking}>
      <p className="a11y-result">{summaryLine(report)}</p>
      <div className="a11y-seg" role="img" aria-label={`${c.failed} failed, ${c.manual} to check by hand, ${c.passed} passed of ${total}`}>
        {seg(c.failed, "failed")}
        {seg(c.manual, "manual")}
        {seg(c.passed, "passed")}
        {seg(c.skipped, "skipped")}
      </div>
    </div>
  );
}

function GroupView({ doc, group }: { doc: DocState; group: Group }) {
  const closed = useA11y((s) => s.docs[doc.id]?.collapsed[group.name]) ?? false;
  return (
    <section className="a11y-group">
      <button className="a11y-group-head" aria-expanded={!closed} onClick={() => st().toggleGroup(doc.id, group.name)}>
        <span className={closed ? "chev" : "chev open"} aria-hidden="true">
          <Icon name="right" size={12} />
        </span>
        <span className="a11y-group-name">{group.name}</span>
        <span className="a11y-group-n">{group.failed > 0 ? `${group.failed} failed` : group.manual > 0 ? `${group.manual} to check` : "no problems"}</span>
      </button>
      {!closed && (
        <ul className="a11y-rules">
          {group.rules.map((x) => (
            <Rule key={x.rule} doc={doc} x={x} />
          ))}
        </ul>
      )}
    </section>
  );
}

const GLYPH: Record<RuleResult["status"], string> = { passed: "✓", failed: "✕", manual: "!", skipped: "–" };

function Rule({ doc, x }: { doc: DocState; x: RuleResult }) {
  const lang = useA11y((s) => s.lang);
  const d = useA11y((s) => s.docs[doc.id])!;
  const [all, setAll] = useState(false);
  const busy = d.phase !== "idle" || d.checkedRev !== doc.rev;
  // A document-level finding of a rule with a one-click fix only restates the explanation above it.
  const noted = x.findings.filter((f) => !isOverflowNote(f) && !(f.page === null && x.fix && x.fix !== "autotag"));
  const overflow = x.findings.filter(isOverflowNote);
  const shown = all ? noted : noted.slice(0, FIRST);
  const fix = x.status === "failed" ? fixFor(x, lang) : null;
  const first = noted.find((f) => f.page !== null);
  const effect = x.status === "failed" ? fixEffect(x, doc.name, lang) : null;
  const quiet = x.status === "passed" || x.status === "skipped";

  return (
    <li className={`a11y-rule ${x.status}`}>
      <div className="a11y-rule-head">
        <span className={`a11y-glyph ${x.status}`} aria-hidden="true">
          {GLYPH[x.status]}
        </span>
        <span className="a11y-rule-name">{x.name}</span>
        <span className={`a11y-status ${x.status}`}>{STATUS_WORD[x.status]}</span>
      </div>
      {!quiet && (
        <>
          <p className="a11y-hint">{x.hint}</p>
          {x.rule === "primary-language" && x.status === "failed" && (
            <select className="a11y-select" value={lang} aria-label="Document language" onChange={(e) => st().setLang(e.target.value)}>
              {(LANGS.includes(lang) ? LANGS : [lang, ...LANGS]).map((l) => (
                <option key={l} value={l}>
                  {langLabel(l)}
                </option>
              ))}
            </select>
          )}
          {effect && <p className="a11y-effect">{effect}</p>}
          {x.fix === "autotag" && x.status === "failed" && d.preview?.untagged && d.notNow && <p className="a11y-effect">{autotagText(d.preview)}</p>}
          {shown.length > 0 && (
            <ul className="a11y-findings">
              {shown.map((f, i) => (
                <FindingRow key={i} doc={doc} x={x} f={f} busy={busy} />
              ))}
            </ul>
          )}
          {noted.length > FIRST && (
            <button className="a11y-link" onClick={() => setAll(!all)}>
              {all ? "Show fewer" : `Show all ${noted.length}`}
            </button>
          )}
          {overflow.map((f, i) => (
            <p key={i} className="a11y-note">
              {f.message.replace(/^\.{3}\s*/, "…").replace(/^…and/, "… and")}
            </p>
          ))}
          <div className="a11y-actions">
            {fix && (
              <button className="a11y-btn" disabled={busy} onClick={() => void st().fix(doc.id, fix)}>
                {fix.fix === "autotag" ? "Autotag (best effort)" : "Fix"}
              </button>
            )}
            {first && (
              <button className="a11y-btn" onClick={() => st().show(doc.id, first)}>
                Show
              </button>
            )}
          </div>
        </>
      )}
    </li>
  );
}

function FindingRow({ doc, x, f, busy }: { doc: DocState; x: RuleResult; f: Finding; busy: boolean }) {
  const [alt, setAlt] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const alted = takesAlt(x, f);
  const jump = f.page !== null;
  const save = () => {
    const text = alt.trim();
    if (!text || !alted || busy) return;
    void st().fix(doc.id, { fix: "figureAlt", xref: f.xref, alt: text });
  };
  return (
    <li className={alted ? "a11y-finding alt" : "a11y-finding"}>
      <div className="a11y-finding-row">
        <button className="a11y-finding-main" disabled={!jump} onClick={() => st().show(doc.id, f)} title={jump ? "Show on the page" : undefined}>
          {jump && <span className="a11y-where">{whereText(f)}</span>}
          <span className="a11y-msg">{f.message}</span>
        </button>
      </div>
      {alted && (
        <div className="a11y-alt">
          {f.page !== null && f.rect && <Thumb doc={doc} page={f.page} rect={f.rect} />}
          <div className="a11y-alt-field">
            <input
              ref={input}
              type="text"
              value={alt}
              placeholder="Describe this figure"
              aria-label={`Alternate text for the figure on page ${(f.page ?? 0) + 1}`}
              onChange={(e) => setAlt(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && save()}
            />
            <button className="a11y-btn" disabled={busy || !alt.trim()} onClick={save}>
              Save
            </button>
          </div>
        </div>
      )}
    </li>
  );
}

// ---- the figure's thumbnail: its box cut out of a render of its page ---------------------------

const SCALE = 1.25;
const THUMB_W = 96;
const THUMB_H = 72;
const pages = new Map<string, Promise<string>>();

function pageUrl(doc: number, rev: number, page: number): Promise<string> {
  const key = `${doc}:${rev}:${page}`;
  let p = pages.get(key);
  if (!p) {
    p = renderPage(doc, { kind: "current" }, page, SCALE);
    p.catch(() => pages.delete(key));
    pages.set(key, p);
    // Keeps the last few; an evicted one is rendered again if it comes back.
    if (pages.size > 12) {
      const old = pages.keys().next().value!;
      const u = pages.get(old)!;
      pages.delete(old);
      void u.then((url) => URL.revokeObjectURL(url), () => {});
    }
  }
  return p;
}

function Thumb({ doc, page, rect }: { doc: DocState; page: number; rect: [number, number, number, number] }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    setUrl(null);
    pageUrl(doc.id, doc.rev, page).then(
      (u) => live && setUrl(u),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [doc.id, doc.rev, page]);
  const size = doc.sizes[page];
  const w = Math.max(1, rect[2] - rect[0]);
  const h = Math.max(1, rect[3] - rect[1]);
  const k = Math.min(THUMB_W / w, THUMB_H / h);
  return (
    <div className="a11y-thumb" style={{ width: THUMB_W, height: THUMB_H }} aria-hidden="true">
      {url && size && (
        <img
          src={url}
          alt=""
          draggable={false}
          style={{
            width: size[0] * k,
            height: size[1] * k,
            left: -rect[0] * k + (THUMB_W - w * k) / 2,
            top: -rect[1] * k + (THUMB_H - h * k) / 2,
          }}
        />
      )}
    </div>
  );
}

// ---- footer --------------------------------------------------------------------------------------

function Foot({ doc, report }: { doc: DocState; report: Report }) {
  const phase = useA11y((s) => s.docs[doc.id]?.phase ?? "idle");
  const stale = useA11y((s) => s.docs[doc.id]?.checkedRev !== doc.rev);
  const busy = phase !== "idle" || stale;
  const all = fixAllText(report);
  return (
    <div className="a11y-foot">
      <button
        className="a11y-btn primary wide"
        disabled={busy || !all}
        onClick={() => void st().fixAll(doc.id)}
        title={all ? "One undo step" : "Nothing can be fixed automatically"}
      >
        <span>{all ? all.label : "Fix all safe issues"}</span>
        <small>{all ? all.outcome : "No safe fixes left"}</small>
      </button>
      <MenuButton
        className="a11y-btn"
        label="Export report"
        items={[
          { label: "HTML report…", onSelect: () => void st().exportReport(doc.id, "html") },
          { label: "PDF report…", onSelect: () => void st().exportReport(doc.id, "pdf") },
        ]}
      >
        Export report
        <Icon name="down" size={12} />
      </MenuButton>
    </div>
  );
}
