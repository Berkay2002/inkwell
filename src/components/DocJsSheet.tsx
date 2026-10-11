import { useEffect, useRef, useState } from "react";
import type { DocJsList, DocJsMeta, DocJsSlot } from "../lib/api";
import { type DocJsSheet as SheetState, dirty, isNamed, statusLine, useDocJs } from "../lib/docJsStore";
import { Icon } from "./Icons";
import "./docJs.css";

const act = () => useDocJs.getState();

/** Document JavaScript: every script the file carries, one in a plain editor. Opened from the command palette and All tools. */
export function DocJsSheet() {
  const sheet = useDocJs((s) => s.sheet);
  const open = sheet != null;
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // a field being typed in keeps its own Escape (blur)
        if ((e.target as HTMLElement | null)?.closest("input")) return;
        e.stopPropagation();
        act().close();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open]);
  if (!sheet) return null;
  return (
    <div className="scrim dj-scrim" onMouseDown={() => act().close()}>
      <section className="dj-sheet" role="dialog" aria-label="Document JavaScript" onMouseDown={(e) => e.stopPropagation()}>
        <header className="dj-head">
          <Icon name="code" size={16} />
          <span className="dj-title">Document JavaScript</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label="Close Document JavaScript" onClick={() => act().close()}>
            <Icon name="close" size={14} />
          </button>
        </header>
        <Capabilities list={sheet.list} />
        {sheet.list?.certified ? (
          <p className="dj-note" role="status">
            This document is certified, so its scripts are shown but not changed here: any change would void the certification.
          </p>
        ) : sheet.list?.signed ? (
          <p className="dj-note" role="status">
            This document is signed. A change is saved as an update after the signature, which will show that the file was changed, and
            replaced or deleted script text stays in the signed revision.
          </p>
        ) : null}
        <div className="dj-body">
          <ScriptList sheet={sheet} />
          <Editor sheet={sheet} />
        </div>
      </section>
    </div>
  );
}

const MAX_SHOWN = 3;

/** "What this file can do": the risky calls the scripts mention, found by a word scan, before anything runs. */
function Capabilities({ list }: { list: DocJsList | null }) {
  if (!list) return <div className="dj-caps">Reading the scripts...</div>;
  const none = list.capabilities.length === 0;
  const total = list.named.length + list.docActions.concat(list.pageActions).reduce((n, s) => n + s.steps.filter((x) => x.javascript).length, 0);
  return (
    <div className={`dj-caps${none ? "" : " risky"}`} role="region" aria-label="What this file can do">
      <div className="dj-caps-title">What this file can do</div>
      {none ? (
        <p>
          {total + list.counts.fieldScripts + list.counts.links + list.counts.bookmarks === 0
            ? "This file has no scripts."
            : "No script in this file mentions opening web addresses, sending form data, email, printing, or reading and writing data."}
        </p>
      ) : (
        <ul>
          {list.capabilities.map((c) => (
            <li key={c.key}>
              <b>{c.label}</b>
              <span className="dj-where">
                {c.places.slice(0, MAX_SHOWN).join("; ")}
                {c.count > MAX_SHOWN ? ` and ${c.count - MAX_SHOWN} more` : ""}
              </span>
            </li>
          ))}
        </ul>
      )}
      <p className="dj-caps-note">
        Found by looking for the call names in every script, so a call built from pieces of text can be missed. Nothing here is run
        {list.incomplete ? "; the list is cut at its limits, so there may be more." : "."}
      </p>
    </div>
  );
}

const riskNote = (list: DocJsList, m: DocJsMeta) =>
  m.risks.map((k) => list.capabilities.find((c) => c.key === k)?.label ?? k).join(", ");

function ScriptList({ sheet }: { sheet: SheetState }) {
  const l = sheet.list;
  const [page, setPage] = useState("");
  const [event, setEvent] = useState<"O" | "C">("O");
  if (!l) return <nav className="dj-list" aria-label="Scripts" />;
  const locked = l.certified;
  const pageSlots = l.pageActions;
  const row = (id: string, label: string, m: DocJsMeta | null, extra?: string) => {
    const warn = m && l ? riskNote(l, m) : "";
    return (
      <li key={id}>
        <button className={`dj-row${sheet.selected === id ? " on" : ""}`} onClick={() => act().select(id)} aria-current={sheet.selected === id}>
          <span className="dj-row-name">{label}</span>
          {extra && <span className="dj-row-extra">{extra}</span>}
          {m?.tooLarge && <span className="dj-row-extra">too large</span>}
          {warn && (
            <span className="dj-risk" title={warn} aria-label={`Mentions: ${warn}`}>
              !
            </span>
          )}
        </button>
      </li>
    );
  };
  const slotRows = (s: DocJsSlot) => {
    if (s.steps.length === 0) return row(`${s.id}:0`, s.label, null, "not set");
    return s.steps.map((st, i) =>
      row(st.id, s.steps.length > 1 ? `${s.label} (${i + 1})` : s.label, st, st.javascript ? undefined : st.what),
    );
  };
  return (
    <nav className="dj-list" aria-label="Scripts">
      <div className="dj-group">
        <div className="dj-group-head">
          <span>Document scripts</span>
          <span className="grow" />
          <button className="dj-mini" onClick={() => act().startAdd()} disabled={sheet.busy || locked}>
            Add
          </button>
        </div>
        {sheet.adding != null && (
          <form
            className="dj-add"
            onSubmit={(e) => {
              e.preventDefault();
              void act().commitAdd();
            }}
          >
            <input
              autoFocus
              value={sheet.adding}
              placeholder="Script name (empty: Script 1)"
              aria-label="New script name"
              onChange={(e) => act().setAdding(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), act().setAdding(null))}
            />
            <button className="dj-mini" type="submit" disabled={sheet.busy}>
              Add
            </button>
          </form>
        )}
        <ul>{l.named.length === 0 ? <li className="dj-none">None</li> : l.named.map((n) => row(n.id, n.name, n))}</ul>
      </div>
      <div className="dj-group">
        <div className="dj-group-head">Document actions</div>
        <ul>{l.docActions.flatMap(slotRows)}</ul>
      </div>
      <div className="dj-group">
        <div className="dj-group-head">Page actions</div>
        <ul>{pageSlots.length === 0 ? <li className="dj-none">None</li> : pageSlots.flatMap(slotRows)}</ul>
        <form
          className="dj-add"
          onSubmit={(e) => {
            e.preventDefault();
            act().newPageAction(Number(page), event);
          }}
        >
          <input value={page} inputMode="numeric" placeholder="Page" aria-label="Page number" onChange={(e) => setPage(e.target.value)} />
          <select value={event} aria-label="When" onChange={(e) => setEvent(e.target.value as "O" | "C")}>
            <option value="O">opens</option>
            <option value="C">closes</option>
          </select>
          <button className="dj-mini" type="submit" disabled={sheet.busy || locked}>
            Set
          </button>
        </form>
      </div>
      <div className="dj-group">
        <div className="dj-group-head">Elsewhere in the file</div>
        <ul className="dj-counts">
          <li>
            Fields: {l.counts.fieldScripts} script{l.counts.fieldScripts === 1 ? "" : "s"} in {l.counts.fields} field
            {l.counts.fields === 1 ? "" : "s"}
            {l.counts.fields > 0 && (
              <button className="dj-mini" onClick={() => act().showInForms()}>
                Show in Forms
              </button>
            )}
          </li>
          <li>
            Links: {l.counts.links} with a script · Bookmarks: {l.counts.bookmarks}
          </li>
        </ul>
      </div>
    </nav>
  );
}

function Editor({ sheet }: { sheet: SheetState }) {
  const ta = useRef<HTMLTextAreaElement>(null);
  const gutter = useRef<HTMLPreElement>(null);
  const l = sheet.list;
  const sel = sheet.selected;
  if (!l || !sel) return <div className="dj-editor dj-empty">{l ? "Choose a script, or add one." : ""}</div>;
  const named = isNamed(sel);
  const locked = l.certified;
  const step = l.docActions.concat(l.pageActions).flatMap((s) => s.steps).find((x) => x.id === sel);
  // A step that is not JavaScript: the first one may be replaced, a later one is left alone.
  const other = step && !step.javascript ? step : null;
  const later = other != null && !sel.endsWith(":0");
  const lines = Math.max(1, sheet.text.split("\n").length);
  const status = statusLine(sheet);
  const isDirty = dirty(sheet);
  const title = named ? (l.named.find((n) => n.id === sel)?.name ?? sel) : titleOf(l, sel);

  const onKey = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // the text owns its keys (undo, select all, save): the app's shortcuts must not also act on the document
    e.stopPropagation();
    if (e.key === "Tab" && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      const t = e.currentTarget;
      t.setRangeText("  ", t.selectionStart, t.selectionEnd, "end");
      act().setText(t.value);
    } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
      e.preventDefault();
      void act().save();
    }
  };

  return (
    <section className="dj-editor" aria-label="Script editor">
      <div className="dj-ed-head">
        {sheet.renaming != null ? (
          <form
            className="dj-add"
            onSubmit={(e) => {
              e.preventDefault();
              void act().commitRename();
            }}
          >
            <input
              autoFocus
              value={sheet.renaming}
              aria-label="Script name"
              onChange={(e) => act().setRenaming(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), act().setRenaming(null))}
            />
            <button className="dj-mini" type="submit" disabled={sheet.busy}>
              Rename
            </button>
          </form>
        ) : (
          <span className="dj-ed-title">{title}</span>
        )}
        <span className="grow" />
        {named && sheet.renaming == null && (
          <button className="dj-mini" onClick={() => act().startRename()} disabled={sheet.busy || locked}>
            Rename
          </button>
        )}
        {(named || step) && (
          <button className="dj-mini" onClick={() => void act().remove()} disabled={sheet.busy || locked}>
            {named ? "Delete" : "Clear action"}
          </button>
        )}
      </div>
      {other && (
        <p className="dj-note">
          {later ? `This step does something else (${other.what}) and is not edited here.` : `This action is: ${other.what}. Saving a script here replaces it.`}
        </p>
      )}
      {sheet.confirmSave && (
        <div className="dj-confirm" role="alert">
          <span>This script has a syntax error. Save it anyway?</span>
          <button className="btn" onClick={() => void act().save(true)}>
            Save anyway
          </button>
          <button className="btn ghost" onClick={() => act().cancelConfirmSave()}>
            Keep editing
          </button>
        </div>
      )}
      {sheet.confirmDiscard != null && (
        <div className="dj-confirm" role="alert">
          <span>Discard your changes to this script?</span>
          <button className="btn" onClick={() => act().answerDiscard(true)}>
            Discard
          </button>
          <button className="btn ghost" onClick={() => act().answerDiscard(false)}>
            Keep editing
          </button>
        </div>
      )}
      <div className="dj-code">
        <pre className="dj-gutter" ref={gutter} aria-hidden="true">
          {Array.from({ length: lines }, (_, i) => i + 1).join("\n")}
        </pre>
        <textarea
          ref={ta}
          className="dj-text"
          value={sheet.tooLarge ? "" : sheet.text}
          readOnly={sheet.tooLarge || later || locked}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          wrap="off"
          aria-label="Script text"
          aria-invalid={status.kind === "error"}
          placeholder={sheet.tooLarge ? "Too large to show here (over 1 MiB)." : ""}
          onChange={(e) => act().setText(e.target.value)}
          onKeyDown={onKey}
          onScroll={(e) => {
            if (gutter.current) gutter.current.scrollTop = e.currentTarget.scrollTop;
          }}
        />
      </div>
      <div className="dj-foot">
        <span className={`dj-status ${status.kind}`} role="status">
          {status.text}
        </span>
        <span className="grow" />
        {sheet.error && <span className="dj-error">{sheet.error}</span>}
        <button className="btn primary" disabled={!isDirty || sheet.busy || later || locked} onClick={() => void act().save()}>
          Save script
        </button>
      </div>
    </section>
  );
}

/** The heading of an action's script: "Document will save", "Page 3: page opens". */
function titleOf(l: DocJsList, id: string): string {
  const slot = l.docActions.concat(l.pageActions).find((s) => id.startsWith(`${s.id}:`));
  if (slot) return slot.steps.length > 1 ? `${slot.label} (${Number(id.split(":").pop()) + 1})` : slot.label;
  // A page action not made yet: p:<page>:<event>:0
  const m = /^p:(\d+):(O|C):\d+$/.exec(id);
  return m ? `Page ${Number(m[1]) + 1}: page ${m[2] === "O" ? "opens" : "closes"}` : id;
}
