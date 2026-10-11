import { useEffect } from "react";
import type { BatchAction, BatchFilePreview, BatchIssue } from "../lib/api";
import { filesWord, hasProtect, saveBlock, splitIssues, stepLabel } from "../lib/batchLogic";
import { type Editor, type Run, type RunRow, useBatch } from "../lib/batchStore";
import { formatBytes } from "../lib/toolsStore";
import { revealInFolder } from "../recents";
import { ActionEditor } from "./ActionEditor";
import { Icon } from "./Icons";
import "./actionWizard.css";

const act = () => useBatch.getState();

/**
 * The Action Wizard: one centred sheet, like Combine. The list of actions, the editor (step library beside the
 * sequence) and the run (files, a dry-run preview, progress, the summary) are three views of it.
 */
export function ActionWizard() {
  const open = useBatch((s) => s.open);
  const view = useBatch((s) => s.view);
  const editor = useBatch((s) => s.editor);
  const run = useBatch((s) => s.run);
  const busy = run?.phase === "running" || run?.phase === "previewing";

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || busy) return;
      // a field being typed in keeps its own Escape (blur)
      if ((e.target as HTMLElement | null)?.closest("input, select, textarea")) return;
      e.stopPropagation();
      if (act().view === "list") act().close();
      else act().backToList();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, busy]);

  if (!open) return null;
  const title = view === "edit" ? (editor?.action.name.trim() || "New action") : view === "run" ? (run?.action.name ?? "Run") : "Action Wizard";
  return (
    <div className="scrim aw-scrim" onMouseDown={() => !busy && act().close()}>
      <section className="aw-sheet" role="dialog" aria-label="Action Wizard" onMouseDown={(e) => e.stopPropagation()}>
        <header className="aw-head">
          {view !== "list" && (
            <button className="btn ghost aw-back" disabled={busy} onClick={() => act().backToList()}>
              <Icon name="right" size={14} />
              Actions
            </button>
          )}
          <Icon name="wand" size={16} />
          <span className="aw-title">{view === "run" ? `Run: ${title}` : title}</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Action Wizard" disabled={busy} onClick={() => act().close()}>
            <Icon name="close" size={14} />
          </button>
        </header>
        {view === "list" && <ListView />}
        {view === "edit" && editor && <EditView editor={editor} />}
        {view === "run" && run && <RunView run={run} />}
      </section>
    </div>
  );
}

// ---------------------------------------------------------------- the list

function outputSentence(a: BatchAction) {
  const suffix = a.output.suffix;
  return a.output.kind === "folder"
    ? `Saved in a folder you choose each time${suffix ? `, ending "${suffix}"` : ""}`
    : `Saved next to each file, ending "${suffix}"`;
}

function ListView() {
  const actions = useBatch((s) => s.actions);
  const loading = useBatch((s) => s.loading);
  const error = useBatch((s) => s.listError);
  const confirm = useBatch((s) => s.confirmDelete);
  const builtIn = actions.filter((a) => a.builtIn);
  const mine = actions.filter((a) => !a.builtIn);
  return (
    <>
      <div className="aw-toolbar">
        <button className="btn primary" onClick={() => act().edit(null)}>
          <Icon name="plus" size={14} />
          New action
        </button>
        <button className="btn ghost" onClick={() => void act().importFile()}>
          <Icon name="download" size={14} />
          Import…
        </button>
        <span className="grow" />
        <span className="dim aw-toolbar-note">Run a set of steps on many files at once</span>
      </div>
      {error && (
        <p className="aw-banner" role="alert">
          {error}
        </p>
      )}
      <div className="aw-body">
        <h3 className="aw-section">Your actions</h3>
        {mine.length === 0 ? (
          <p className="dim aw-none">{loading ? "Loading…" : "None yet. Start from New action, or duplicate one of the built-in actions below."}</p>
        ) : (
          <ul className="aw-cards">
            {mine.map((a) => (
              <Card key={a.id} a={a} confirm={confirm === a.id} />
            ))}
          </ul>
        )}
        <h3 className="aw-section">Built in</h3>
        <ul className="aw-cards">
          {builtIn.map((a) => (
            <Card key={a.id} a={a} confirm={false} />
          ))}
        </ul>
      </div>
    </>
  );
}

function Card({ a, confirm }: { a: BatchAction; confirm: boolean }) {
  return (
    <li className="aw-card">
      <div className="aw-card-head">
        <span className="aw-card-name">{a.name}</span>
        {a.builtIn && <span className="aw-badge">Built in</span>}
      </div>
      {a.description && <p className="aw-card-desc">{a.description}</p>}
      <ol className="aw-card-steps" aria-label="Steps">
        {a.steps.map((s, i) => (
          <li key={i} className={s.kind === "invalid" ? "bad" : ""}>
            {stepLabel(s)}
          </li>
        ))}
      </ol>
      <p className="dim aw-card-out">{outputSentence(a)}</p>
      <div className="aw-card-actions">
        <button className="btn primary" onClick={() => act().startRun(a)}>
          <Icon name="present" size={13} />
          Run
        </button>
        {!a.builtIn && (
          <button className="btn ghost" onClick={() => act().edit(a)}>
            Edit
          </button>
        )}
        <button className="btn ghost" onClick={() => void act().duplicate(a)}>
          Duplicate
        </button>
        <button className="btn ghost" onClick={() => void act().exportAction(a)}>
          Export…
        </button>
        <span className="grow" />
        {!a.builtIn &&
          (confirm ? (
            <>
              <button className="btn danger" onClick={() => void act().remove(a)}>
                Delete
              </button>
              <button className="btn ghost" onClick={() => act().askDelete(null)}>
                Keep
              </button>
            </>
          ) : (
            <button className="ax-icon" title="Delete" aria-label={`Delete ${a.name}`} onClick={() => act().askDelete(a.id)}>
              <Icon name="trash" size={14} />
            </button>
          ))}
      </div>
    </li>
  );
}

// ---------------------------------------------------------------- the editor

function EditView({ editor }: { editor: Editor }) {
  const { action, issues, selected, saving, error } = editor;
  const { blocking, passwordFor } = splitIssues(action, issues, false);
  const block = saveBlock(action);
  const aboutAction = blocking.filter((i) => i.step == null);
  const summary = error ?? aboutAction[0]?.message ?? (blocking.length ? `${blocking.length === 1 ? "A step needs" : "Some steps need"} a look before it can run.` : null);
  const out = action.output;
  return (
    <>
      <ActionEditor action={action} issues={issues} selected={selected} soft={passwordFor} />
      <div className="aw-rules">
        <div className="aw-rule">
          <span className="aw-label">Result</span>
          <div className="ax-seg tl-seg" role="radiogroup" aria-label="Where the results go">
            {(
              [
                ["nextToSource", "Next to each file"],
                ["folder", "In a folder chosen when it runs"],
              ] as const
            ).map(([k, label]) => (
              <button key={k} role="radio" aria-checked={out.kind === k} className={out.kind === k ? "on" : ""} onClick={() => act().setOutput({ kind: k, suffix: out.suffix })}>
                {label}
              </button>
            ))}
          </div>
          <label className="aw-inline">
            <span className="aw-label">Ends with</span>
            <input
              className="tl-field aw-suffix"
              value={out.suffix}
              maxLength={40}
              spellCheck={false}
              aria-label="Ending added to the file name"
              onChange={(e) => act().setOutput({ kind: out.kind, suffix: e.target.value })}
            />
          </label>
        </div>
        <div className="aw-rule">
          <span className="aw-label">If a file fails</span>
          <div className="ax-seg tl-seg" role="radiogroup" aria-label="If a file fails">
            {(
              [
                ["skip", "Skip it and go on"],
                ["stop", "Stop the run"],
              ] as const
            ).map(([k, label]) => (
              <button key={k} role="radio" aria-checked={action.onError === k} className={action.onError === k ? "on" : ""} onClick={() => act().setOnError(k)}>
                {label}
              </button>
            ))}
          </div>
          <span className="dim aw-hint">A result never replaces a file: a taken name gets (2).</span>
        </div>
      </div>
      <footer className="aw-foot">
        <span className={error || aboutAction.length ? "aw-outcome bad" : "aw-outcome dim"} aria-live="polite">
          {summary ?? (block ? block : "Saved actions keep their steps and options, never passwords.")}
        </span>
        <button className="btn ghost" disabled={saving} onClick={() => act().backToList()}>
          Cancel
        </button>
        <button className="btn primary" disabled={saving || block != null} title={block ?? undefined} onClick={() => void act().save()}>
          Save
        </button>
      </footer>
    </>
  );
}

// ---------------------------------------------------------------- the run

const STATE_TEXT = { waiting: "Waiting", running: "Working", ok: "Done", skipped: "Skipped", error: "Failed" } as const;

function RunView({ run }: { run: Run }) {
  if (run.phase === "running" || run.phase === "done") return <Progress run={run} />;
  return <Setup run={run} />;
}

function Setup({ run }: { run: Run }) {
  const { action, files, phase } = run;
  const previewing = phase === "previewing";
  const protect = hasProtect(action);
  const typed = !!(run.openPassword || run.permissionsPassword);
  const { blocking } = splitIssues(action, run.issues, typed);
  const rows = new Map(run.preview?.files.map((f) => [f.index, f]));
  const p = run.preview;
  return (
    <>
      <div className="aw-run">
        <section className="aw-files" aria-label="Files">
          <div className="aw-files-head">
            <h3 className="aw-col-title aw-flat">Files</h3>
            <span className="grow" />
            <button className="btn ghost" disabled={previewing} onClick={() => void act().addFiles()}>
              <Icon name="filePlus" size={14} />
              Add files…
            </button>
            <button className="btn ghost" disabled={previewing} onClick={() => void act().addFolder()}>
              <Icon name="open" size={14} />
              Add folder…
            </button>
          </div>
          <label className="aw-check aw-recursive">
            <input type="checkbox" checked={run.recursive} onChange={(e) => act().setRecursive(e.target.checked)} />
            <span>Include sub-folders when adding a folder</span>
          </label>
          {files.length === 0 ? (
            <div className="aw-empty">
              <p className="aw-empty-title">Choose the files to run it on</p>
              <p className="dim">Pick PDFs, or a whole folder. Nothing is written until you press Run, and the originals are never changed.</p>
            </div>
          ) : (
            <ul className="aw-filelist" aria-label="Files to process">
              {files.map((f, i) => {
                const pv = rows.get(i);
                return (
                  <li key={f.token} className="aw-file">
                    <span className="aw-file-main">
                      <span className="aw-file-name" title={f.path}>
                        {f.path.split(/[\\/]/).pop()}
                      </span>
                      {pv && !run.stale && <PreviewLine pv={pv} />}
                    </span>
                    <button className="ax-icon" aria-label={`Remove ${f.path.split(/[\\/]/).pop()}`} title="Remove" disabled={previewing} onClick={() => act().removeFile(i)}>
                      <Icon name="close" size={14} />
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
          {run.capped && <p className="aw-banner">The folder has more PDFs than a batch takes: the first {files.length} are listed.</p>}
        </section>
        <section className="aw-plan" aria-label="What it does">
          <h3 className="aw-col-title aw-flat">What it does</h3>
          <ol className="aw-plan-steps">
            {action.steps.map((s, i) => (
              <li key={i}>{stepLabel(s)}</li>
            ))}
          </ol>
          <p className="dim aw-plan-out">
            {outputSentence(action)}. {action.onError === "stop" ? "Stops at the first file that fails." : "A file that fails is skipped and the rest go on."}
          </p>
          {protect && (
            <fieldset className="aw-passwords">
              <legend className="aw-label">Passwords for this run</legend>
              <label className="aw-field">
                <span className="aw-label">Password to open the file</span>
                <input
                  className="tl-field"
                  type="password"
                  autoComplete="new-password"
                  value={run.openPassword}
                  disabled={previewing}
                  onChange={(e) => act().setPassword("open", e.target.value)}
                />
              </label>
              <label className="aw-field">
                <span className="aw-label">Password for permissions</span>
                <input
                  className="tl-field"
                  type="password"
                  autoComplete="new-password"
                  value={run.permissionsPassword}
                  disabled={previewing}
                  onChange={(e) => act().setPassword("permissions", e.target.value)}
                />
              </label>
              <p className="dim aw-hint-block">Used for this run only. They are not saved, and the engine never writes them to an action file.</p>
            </fieldset>
          )}
          <div className="aw-preview-bar">
            <button className="btn" disabled={previewing || files.length === 0} onClick={() => void act().preview()}>
              {previewing ? "Checking…" : p && !run.stale ? "Check again" : "Preview"}
            </button>
            <span className="dim aw-hint">Shows, for each file, what would run or be skipped and under which name, without writing anything.</span>
          </div>
          {p && !run.stale && (
            <p className={p.skipped + p.errors > 0 ? "aw-preview-sum warn" : "aw-preview-sum"} aria-live="polite">
              {p.ok === 0
                ? "Nothing would run."
                : `${filesWord(p.ok)} would be processed${p.skipped ? `, ${p.skipped} skipped` : ""}${p.errors ? `, ${p.errors} would fail` : ""}.`}{" "}
              <span className="dim">It cannot tell if a step will find nothing to do or a file will fail partway.</span>
            </p>
          )}
          {p && run.stale && <p className="dim aw-hint-block">The files or password changed: preview again to see what would happen.</p>}
        </section>
      </div>
      <footer className="aw-foot">
        <span className={run.error || blocking.length ? "aw-outcome bad" : "aw-outcome"} aria-live="polite">
          <Outcome run={run} blocking={blocking} />
        </span>
        <button className="btn ghost" disabled={previewing} onClick={() => act().backToList()}>
          Cancel
        </button>
        <button className="btn primary" disabled={previewing || files.length === 0} onClick={() => void act().runNow()}>
          {files.length === 0 ? "Run" : `Run on ${filesWord(files.length)}`}
        </button>
      </footer>
    </>
  );
}

function Outcome({ run, blocking }: { run: Run; blocking: BatchIssue[] }) {
  if (run.error) return <>{run.error}</>;
  if (blocking.length) return <>{blocking[0].message}</>;
  if (run.files.length === 0) return <>Add the files to process</>;
  return <>{run.action.output.kind === "folder" ? "You choose the output folder when you press Run" : "Ready to run"}</>;
}

function PreviewLine({ pv }: { pv: BatchFilePreview }) {
  if (pv.status === "ok") {
    return (
      <span className="aw-pv ok">
        Will run {pv.steps.length} {pv.steps.length === 1 ? "step" : "steps"}
        {pv.outputs.length > 0 && <span className="dim"> → {pv.outputs.join(", ")}</span>}
      </span>
    );
  }
  return (
    <span className={pv.status === "error" ? "aw-pv bad" : "aw-pv warn"}>
      {pv.status === "error" ? "Would fail: " : "Skipped: "}
      {pv.reason}
    </span>
  );
}

function Progress({ run }: { run: Run }) {
  const { rows, report, phase } = run;
  const running = phase === "running";
  const done = rows.filter((r) => r.state === "ok" || r.state === "skipped" || r.state === "error").length;
  const counts = {
    ok: rows.filter((r) => r.state === "ok").length,
    skipped: rows.filter((r) => r.state === "skipped").length,
    error: rows.filter((r) => r.state === "error").length,
  };
  return (
    <>
      <div className="aw-progress">
        <div className="aw-bar" role="progressbar" aria-valuemin={0} aria-valuemax={rows.length || 1} aria-valuenow={done} aria-label="Files done">
          <span style={{ width: `${rows.length ? (done / rows.length) * 100 : 0}%` }} />
        </div>
        <ul className="aw-results" aria-label="Files">
          {run.files.map((f, i) => (
            <ResultRow key={f.token} name={f.path.split(/[\\/]/).pop() ?? f.path} row={rows[i]} total={run.action.steps.length} />
          ))}
        </ul>
      </div>
      <footer className="aw-foot">
        <span className={counts.error || run.error ? "aw-outcome bad" : "aw-outcome"} aria-live="polite">
          {run.error
            ? run.error
            : running
              ? `Working on file ${Math.min(done + 1, rows.length)} of ${rows.length}…`
              : `${report?.cancelled ? "Cancelled. " : report?.stopped ? "Stopped after a failure. " : ""}${counts.ok} done${counts.skipped ? `, ${counts.skipped} skipped` : ""}${counts.error ? `, ${counts.error} failed` : ""}`}
        </span>
        {running ? (
          <button className="btn" onClick={() => act().cancel()}>
            Cancel
          </button>
        ) : (
          <>
            <button className="btn ghost" onClick={() => act().again()}>
              Run again
            </button>
            <button className="btn primary" onClick={() => act().backToList()}>
              Done
            </button>
          </>
        )}
      </footer>
    </>
  );
}

function ResultRow({ name, row, total }: { name: string; row: RunRow | undefined; total: number }) {
  const state = row?.state ?? "waiting";
  const r = row?.report;
  const out = r?.outputs[0]?.path;
  return (
    <li className={`aw-result ${state}`}>
      <span className={`aw-dot ${state}`} aria-hidden />
      <span className="aw-result-main">
        <span className="aw-file-name">{name}</span>
        <span className="aw-result-sub">
          {state === "running" && row ? `Step ${Math.min(row.step + 1, row.steps || total)} of ${row.steps || total}${row.label ? `: ${row.label}` : ""}` : null}
          {state === "ok" && r && (
            <>
              {r.sizeAfter != null ? `${formatBytes(r.sizeBefore)} → ${formatBytes(r.sizeAfter)}` : "Done"}
              {r.outputs.length > 1 ? `, ${r.outputs.length} results` : ""}
            </>
          )}
          {(state === "skipped" || state === "error") && r?.reason}
        </span>
        {state === "ok" && r && r.warnings.length > 0 && <span className="aw-result-sub warn">{r.warnings.join(" ")}</span>}
        {state === "ok" && r && r.notes.length > 0 && <span className="aw-result-sub dim">{r.notes.join(" · ")}</span>}
      </span>
      <span className={`aw-state ${state}`}>{STATE_TEXT[state]}</span>
      {out && (
        <button className="btn ghost aw-reveal" onClick={() => void revealInFolder(out)} title={out}>
          Show in folder
        </button>
      )}
    </li>
  );
}
