import { useEffect, useMemo, useState } from "react";
import { MenuButton } from "../Menu";
import { usePreflight } from "../../lib/preflight";
import { LANGS, langLabel } from "../../lib/a11yLogic";
import {
  type Finding,
  type Group,
  type Plan,
  type PlanGroup,
  type Report,
  type RuleResult,
  PROFILES,
  STATUS_WORD,
  autoCheckDue,
  counts,
  groupRules,
  isTicked,
  outcome,
  profileInfo,
  progressFraction,
  progressText,
  ruleFix,
  verdict,
} from "../../lib/preflightLogic";
import { revealInFolder } from "../../recents";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import "../a11y/a11y.css";
import "./preflight.css";

const st = () => usePreflight.getState();

/** How many findings of a rule show before "Show all". */
const FIRST = 5;

export function PreflightPanel({ doc }: { doc: DocState }) {
  const open = usePreflight((s) => s.open);
  const profile = usePreflight((s) => s.profile);
  const d = usePreflight((s) => s.docs[doc.id]);
  const id = doc.id;
  const phase = d?.phase ?? "idle";

  // Read when the rail opens and again whenever the document or the profile changes.
  useEffect(() => {
    if (!open) return;
    if (autoCheckDue(d, doc.rev, profile)) void st().check(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, id, doc.rev, profile, phase]);

  if (!open) return null;
  const report = d?.report ?? null;
  return (
    <aside className="ax-dock a11y-dock" style={{ width: "min(420px, 45vw)" }} aria-label="Preflight">
      <section className="ax-panel docked a11y-panel pf-panel">
        <div className="ax-head">
          <span className="ax-title">Preflight</span>
          <span className="grow" />
          <button className="a11y-quiet" disabled={phase !== "idle"} onClick={() => void st().check(id)}>
            Check again
          </button>
          <button className="ax-icon" title="Close" aria-label="Close Preflight" onClick={() => st().setOpen(false)}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="a11y-body">
          <ProfilePicker doc={doc} />
          {report ? <Body doc={doc} report={report} /> : <Waiting doc={doc} />}
        </div>
        {report && <Foot doc={doc} />}
      </section>
    </aside>
  );
}

function ProfilePicker({ doc }: { doc: DocState }) {
  const profile = usePreflight((s) => s.profile);
  const phase = usePreflight((s) => s.docs[doc.id]?.phase ?? "idle");
  const info = profileInfo(profile);
  return (
    <div className="pf-profile">
      <label className="pf-label" htmlFor="pf-profile">
        Check against
      </label>
      <select
        id="pf-profile"
        className="a11y-select pf-select"
        value={profile}
        disabled={phase === "converting" || phase === "exporting"}
        onChange={(e) => st().setProfile(e.target.value as typeof profile)}
      >
        {PROFILES.map((p) => (
          <option key={p.id} value={p.id}>
            {p.label}
          </option>
        ))}
      </select>
      <p className="a11y-note">{info.about}</p>
    </div>
  );
}

function Waiting({ doc }: { doc: DocState }) {
  const d = usePreflight((s) => s.docs[doc.id]);
  if (d?.error) return <p className="a11y-error" role="alert">{d.error}</p>;
  // Idle without a report: the check was cancelled, and waits for the person.
  if ((d?.phase ?? "idle") === "idle") {
    return (
      <div className="a11y-card">
        <p>The check was cancelled.</p>
        <div className="a11y-actions">
          <button className="a11y-btn primary" onClick={() => void st().check(doc.id)}>
            Check this document
          </button>
        </div>
      </div>
    );
  }
  return <Running doc={doc} what="check" />;
}

/** A slim progress bar with its label and Cancel. */
function Running({ doc, what }: { doc: DocState; what: "check" | "convert" | "export" }) {
  const d = usePreflight((s) => s.docs[doc.id]);
  const f = progressFraction(d?.progress ?? null);
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
  const d = usePreflight((s) => s.docs[doc.id])!;
  const profile = usePreflight((s) => s.profile);
  const groups = useMemo(() => groupRules(report), [report]);
  // A report that no longer matches the document (its check was cancelled or failed) is not acted on.
  const stale = d.checkedRev !== doc.rev || d.checkedProfile !== profile;
  const busy = d.phase !== "idle" || stale;
  return (
    <>
      <Summary report={report} checking={d.phase === "checking"} />
      {d.phase === "checking" && <Running doc={doc} what="check" />}
      {d.phase === "converting" && <Running doc={doc} what="convert" />}
      {d.phase === "exporting" && <Running doc={doc} what="export" />}
      {stale && d.phase === "idle" && (
        <p className="a11y-note">
          {d.checkedProfile !== profile ? "This report is for another profile." : "The document changed since this check."}{" "}
          <button className="a11y-link" onClick={() => void st().check(doc.id)}>
            Check again
          </button>
        </p>
      )}
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
      {d.converted && d.phase === "idle" && <ConvertedCard doc={doc} />}
      {profileInfo(report.profile).convert && report.plan.total + report.plan.remaining.length > 0 && (
        <OutcomeCard doc={doc} report={report} busy={busy} />
      )}
      {groups.map((g) => (
        <GroupView key={g.name} doc={doc} group={g} plan={report.plan} busy={busy} />
      ))}
    </>
  );
}

function Summary({ report, checking }: { report: Report; checking: boolean }) {
  const c = counts(report);
  const total = Math.max(1, report.results.length);
  const v = verdict(report);
  const seg = (n: number, cls: string) => (n > 0 ? <i className={cls} style={{ flexGrow: n }} /> : null);
  return (
    <div className="a11y-summary" aria-busy={checking}>
      <p className={v.ok ? "a11y-result pf-ok" : "a11y-result pf-bad"}>{v.title}</p>
      <p className="a11y-note">{v.sub}</p>
      {report.claimed && <p className="a11y-note">The file's metadata says it is {report.claimed}.</p>}
      <div className="a11y-seg" role="img" aria-label={`${c.failed} failed, ${c.manual} to check by hand, ${c.passed} passed of ${total}`}>
        {seg(c.failed, "failed")}
        {seg(c.manual, "manual")}
        {seg(c.passed, "passed")}
        {seg(c.info, "skipped")}
      </div>
    </div>
  );
}

// ---- the conversion's outcome card -----------------------------------------------------------------

function OutcomeCard({ doc, report, busy }: { doc: DocState; report: Report; busy: boolean }) {
  const d = usePreflight((s) => s.docs[doc.id])!;
  const lang = usePreflight((s) => s.lang);
  const plan: Plan = report.plan;
  const info = profileInfo(report.profile);
  const out = outcome(plan, d.ticks, d.output);
  const anyTicked = plan.groups.some((g) => isTicked(g, d.ticks));
  const inPlace = d.output === "inPlace";
  return (
    <div className="a11y-card pf-outcome">
      <b>{info.convert}</b>
      <ul className="pf-checks" aria-label="What the conversion changes">
        {plan.groups.map((g) => (
          <GroupCheck key={g.id} g={g} on={isTicked(g, d.ticks)} onChange={(on) => st().setTick(doc.id, g.id, on)} />
        ))}
      </ul>
      {plan.needsLang && (
        <label className="pf-lang">
          <span>Document language</span>
          <select className="a11y-select" value={lang} onChange={(e) => st().setLang(e.target.value)}>
            {(LANGS.includes(lang) ? LANGS : [lang, ...LANGS]).map((l) => (
              <option key={l} value={l}>
                {langLabel(l)}
              </option>
            ))}
          </select>
        </label>
      )}
      <p className="pf-line" role="status" aria-live="polite">
        {out.line}
      </p>
      <div className="a11y-seg-pick" role="radiogroup" aria-label="Where the result goes">
        <button role="radio" aria-checked={!inPlace} className={inPlace ? "" : "on"} onClick={() => st().setOutput(doc.id, "newFile")}>
          New file
        </button>
        <button role="radio" aria-checked={inPlace} className={inPlace ? "on" : ""} onClick={() => st().setOutput(doc.id, "inPlace")}>
          This document
        </button>
      </div>
      <p className="a11y-note">
        {inPlace
          ? "Changes this document as one undo step; Save writes it."
          : `Writes a copy; ${doc.name} stays as it is.`}
      </p>
      <div className="a11y-actions">
        <button className="a11y-btn primary" disabled={busy || !anyTicked} onClick={() => void st().convert(doc.id)}>
          {inPlace ? "Convert in this document" : "Save as new file…"}
        </button>
      </div>
    </div>
  );
}

function GroupCheck({ g, on, onChange }: { g: PlanGroup; on: boolean; onChange: (on: boolean) => void }) {
  const id = `pf-g-${g.id}`;
  const none = g.issues === 0;
  return (
    <li className={g.disabled ? "pf-check off" : "pf-check"}>
      <input id={id} type="checkbox" checked={on} disabled={!!g.disabled} onChange={(e) => onChange(e.target.checked)} />
      <label htmlFor={id}>
        <span className="pf-check-row">
          <span className="pf-check-name">{g.label}</span>
          <span className="pf-check-n">{none ? "nothing to fix" : `${g.issues}`}</span>
        </span>
        <span className="pf-check-effect">{g.disabled ?? g.effect}</span>
      </label>
    </li>
  );
}

function ConvertedCard({ doc }: { doc: DocState }) {
  const c = usePreflight((s) => s.docs[doc.id]?.converted);
  if (!c) return null;
  const v = verdict(c.report);
  const name = c.path.split(/[\\/]/).pop() ?? c.path;
  const failed = counts(c.report).failed;
  return (
    <div className="a11y-card pf-done">
      <b>Saved {name}</b>
      <p className={v.ok ? "pf-ok" : "pf-bad"}>{v.ok ? `The new file passes every rule that Inkwell checks for ${profileInfo(c.report.profile).label}.` : `The new file still fails ${failed} ${failed === 1 ? "rule" : "rules"}.`}</p>
      {c.fixed.length > 0 && (
        <ul className="pf-fixed">
          {c.fixed.slice(0, 8).map((f, i) => (
            <li key={i}>{f}</li>
          ))}
        </ul>
      )}
      <div className="a11y-actions">
        <button className="a11y-btn primary" onClick={() => void useStore.getState().openPath(c.path)}>
          Open
        </button>
        <button className="a11y-btn" onClick={() => revealInFolder(c.path)}>
          Show in folder
        </button>
        <button className="a11y-quiet" onClick={() => st().dismissConverted(doc.id)}>
          Dismiss
        </button>
      </div>
    </div>
  );
}

// ---- groups and rules ----------------------------------------------------------------------------

function GroupView({ doc, group, plan, busy }: { doc: DocState; group: Group; plan: Plan; busy: boolean }) {
  const closed = usePreflight((s) => s.docs[doc.id]?.collapsed[group.name]) ?? false;
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
            <Rule key={x.rule} doc={doc} x={x} plan={plan} busy={busy} />
          ))}
        </ul>
      )}
    </section>
  );
}

const GLYPH: Record<RuleResult["status"], string> = { passed: "✓", failed: "✕", manual: "!", info: "i" };
const MANUAL = "manual";

/** "p. 3 · Font Arial" */
const whereOf = (f: Finding) => [f.page !== null ? `p. ${f.page + 1}` : null, f.object].filter(Boolean).join(" · ");

function Rule({ doc, x, plan, busy }: { doc: DocState; x: RuleResult; plan: Plan; busy: boolean }) {
  const [all, setAll] = useState(false);
  const quiet = x.status === "passed";
  const shown = all ? x.findings : x.findings.slice(0, FIRST);
  const more = Math.max(x.count, x.findings.length) - x.findings.length;
  const first = x.findings.find((f) => f.page !== null);
  const fix = ruleFix(x, plan);
  return (
    <li className={`a11y-rule ${x.status === "info" ? "passed" : x.status}`}>
      <div className="a11y-rule-head">
        <span className={`a11y-glyph ${x.status === "info" ? "skipped" : x.status}`} aria-hidden="true">
          {GLYPH[x.status]}
        </span>
        <span className="a11y-rule-name">{x.name}</span>
        <span className={`a11y-status ${x.status === "info" ? "skipped" : x.status}`}>{STATUS_WORD[x.status]}</span>
      </div>
      {!quiet && (
        <>
          <p className="a11y-hint">
            {x.hint} <span className="pf-clause">{x.clause}</span>
          </p>
          {fix?.group && x.a11yFix === null && <p className="a11y-effect">{fix.group.effect}</p>}
          {shown.length > 0 && (
            <ul className="a11y-findings">
              {shown.map((f, i) => (
                <li key={i} className="a11y-finding">
                  <button
                    className="a11y-finding-main"
                    disabled={f.page === null}
                    onClick={() => st().show(doc.id, f)}
                    title={f.page === null ? undefined : "Show on the page"}
                  >
                    {whereOf(f) && <span className="a11y-where pf-where">{whereOf(f)}</span>}
                    <span className="a11y-msg">
                      {f.message}
                      {f.note && f.note !== MANUAL && <span className="pf-need"> Needs {f.note}.</span>}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {x.findings.length > FIRST && (
            <button className="a11y-link" onClick={() => setAll(!all)}>
              {all ? "Show fewer" : `Show all ${x.findings.length}`}
            </button>
          )}
          {more > 0 && all && <p className="a11y-note">… and {more} more</p>}
          <div className="a11y-actions">
            {fix && !fix.needsNewFile && (
              <button className="a11y-btn" disabled={busy} onClick={() => void st().fixRule(doc.id, x)}>
                Fix
              </button>
            )}
            {fix?.needsNewFile && <span className="a11y-note">Needs a new file: use Convert above.</span>}
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

// ---- footer --------------------------------------------------------------------------------------

function Foot({ doc }: { doc: DocState }) {
  return (
    <div className="a11y-foot">
      <MenuButton
        className="a11y-btn wide"
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
