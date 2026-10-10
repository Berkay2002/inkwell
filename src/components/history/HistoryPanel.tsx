import { useEffect, useMemo, useState } from "react";
import { type Change, type Comparison, type VersionInfo, pickCompareFiles } from "../../lib/history";
import {
  FILTERS,
  WHO_WORD,
  clock,
  dayLabel,
  groupVersions,
  pageChips,
  versionTitle,
  when,
  whoOf,
} from "../../lib/historyLogic";
import { useHistory } from "../../lib/historyStore";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";
import { RevRow } from "./RevRow";
import { MenuButton } from "../Menu";
import "./history.css";

/** The toolbar's clock button. */
export function HistoryToggle() {
  const open = useHistory((s) => s.open);
  const comparing = useHistory((s) => s.compare != null);
  return (
    <button
      className={open || comparing ? "icon-btn on" : "icon-btn"}
      title="Version history (Ctrl+Alt+H)"
      aria-label="Version history"
      aria-pressed={open}
      onClick={() => {
        if (comparing) useHistory.getState().closeCompare();
        useHistory.getState().toggle();
      }}
    >
      <Icon name="clock" />
    </button>
  );
}

export async function pickAndCompareFiles() {
  try {
    const sel = await pickCompareFiles();
    if (!sel) return;
    useHistory.getState().openCompare({
      doc: null,
      a: { kind: "picked", token: sel[0].token },
      b: { kind: "picked", token: sel[1].token },
      names: [sel[0].name, sel[1].name],
    });
  } catch (e) {
    useStore.getState().setUi({ error: String(e) });
  }
}

/** Over the page: "Recover unsaved changes" after a crash, or "same document?" when the file could be an old one. */
export function HistoryBanner({ doc }: { doc: DocState }) {
  const st = useHistory((s) => s.docs[doc.id]?.state);
  const dismissed = useHistory((s) => s.dismissed[doc.id]);
  const h = useHistory.getState();
  if (st?.status === "ambiguous")
    return (
      <div className="hx-banner" role="alert">
        <SamePrompt doc={doc} />
      </div>
    );
  if (st?.recovery && !dismissed)
    return (
      <div className="hx-banner" role="alert">
        <span className="hx-ico">
          <Icon name="clock" size={18} />
        </span>
        <span className="hx-banner-text">
          <strong>Recover unsaved changes?</strong>
          <span className="dim">Inkwell closed unexpectedly. An autosave from {when(st.recovery.created, Date.now())} has changes that were never saved.</span>
        </span>
        <button className="hx-btn primary" onClick={() => void h.recover(doc.id)}>
          Recover
        </button>
        <button className="hx-btn" onClick={() => h.dismissRecovery(doc.id)}>
          Dismiss
        </button>
      </div>
    );
  return null;
}

export function HistoryPanel({ doc }: { doc: DocState }) {
  const isOpen = useHistory((s) => s.open);
  if (!isOpen) return null;
  return (
    <aside className="ax-dock hx-dock" style={{ width: "min(400px, 45vw)" }} aria-label="Version history">
      <section className="ax-panel docked hx-panel">
        <Header doc={doc} />
        <Body doc={doc} />
      </section>
    </aside>
  );
}

function Header({ doc }: { doc: DocState }) {
  const h = useHistory.getState();
  const ready = useHistory((s) => s.docs[doc.id]?.state?.status === "ready");
  const [confirm, setConfirm] = useState<null | "delete" | "never">(null);
  useEffect(() => setConfirm(null), [doc.id]);
  return (
    <>
      <div className="ax-head">
        <span className="ax-title">Version history</span>
        <span className="grow" />
        <MenuButton
          className="ax-icon"
          label="More"
          items={[
            { label: "Compare files…", onSelect: () => void pickAndCompareFiles() },
            ...(ready
              ? [
                  "sep" as const,
                  { label: "Delete history for this file…", danger: true, onSelect: () => setConfirm("delete") },
                  { label: "Never keep history for this file…", danger: true, onSelect: () => setConfirm("never") },
                ]
              : []),
          ]}
        >
          <Icon name="more" size={18} />
        </MenuButton>
        <button className="ax-icon" title="Close" aria-label="Close version history" onClick={() => h.setOpen(false)}>
          <Icon name="close" size={16} />
        </button>
      </div>
      {confirm && (
        <div className="hx-confirm" role="alertdialog">
          <p>
            {confirm === "delete"
              ? "Delete every saved version of this file? The file itself stays as it is. This can't be undone."
              : "Delete every saved version and stop keeping history for this file?"}
          </p>
          <div className="hx-actions">
            <button
              className="hx-btn danger"
              onClick={() => {
                setConfirm(null);
                if (confirm === "delete") void h.deleteHistory(doc.id);
                else void h.neverKeep(doc.id, true);
              }}
            >
              {confirm === "delete" ? "Delete history" : "Delete and turn off"}
            </button>
            <button className="hx-btn" onClick={() => setConfirm(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
    </>
  );
}

function Body({ doc }: { doc: DocState }) {
  const dh = useHistory((s) => s.docs[doc.id]);
  const status = dh?.state?.status ?? "loading";
  switch (status) {
    case "loading":
      return <div className="hx-empty dim">Opening history{"…"}</div>;
    case "unavailable":
      return (
        <Empty title="History is unavailable" icon="lock">
          Another Inkwell window is using the history store, so versions can't be read or recorded here. Close the other window and open this document again.
        </Empty>
      );
    case "off":
      return (
        <Empty title="History is off for this file" icon="clock">
          Inkwell isn't keeping versions of this file.
          <div className="hx-actions center">
            <button className="hx-btn primary" onClick={() => void useHistory.getState().neverKeep(doc.id, false)}>
              Turn history on
            </button>
          </div>
        </Empty>
      );
    case "ambiguous":
      return (
        <Empty title="Same document as before?" icon="file">
          <SamePrompt doc={doc} />
        </Empty>
      );
    case "error":
      return (
        <Empty title="History hit a problem" icon="fileX">
          {dh?.state?.message ?? "Unknown error."}
        </Empty>
      );
    case "none":
      return (
        <Empty title="No history yet" icon="clock">
          Versions appear here as the document changes.
        </Empty>
      );
    default:
      return <List doc={doc} versions={dh?.versions ?? []} />;
  }
}

function Empty({ title, icon, children }: { title: string; icon: "lock" | "clock" | "file" | "fileX"; children: React.ReactNode }) {
  return (
    <div className="hx-empty">
      <span className="hx-empty-ico">
        <Icon name={icon} size={22} />
      </span>
      <h3>{title}</h3>
      <div className="dim">{children}</div>
    </div>
  );
}

/** "Is this the same document as ...?" (the same text is on the banner over the page). */
export function SamePrompt({ doc }: { doc: DocState }) {
  const cands = useHistory((s) => s.docs[doc.id]?.state?.candidates ?? []);
  const h = useHistory.getState();
  const now = Date.now();
  return (
    <div className="hx-same">
      {cands.map((c) => (
        <div key={c.id} className="hx-same-row">
          <span>
            Is this the same document as the one last changed {c.last ? `${dayLabel(c.last.created, now)}, ${clock(c.last.created)}` : "before"}
            {c.last ? ` (“${versionTitle(c.last)}”)` : ""}?
          </span>
          <button className="hx-btn primary" onClick={() => void h.confirmSame(doc.id, c.id)}>
            Yes
          </button>
        </div>
      ))}
      <button className="hx-btn" onClick={() => void h.confirmSame(doc.id, null)}>
        No, it's new
      </button>
    </div>
  );
}

function List({ doc, versions }: { doc: DocState; versions: VersionInfo[] }) {
  const filter = useHistory((s) => s.filter);
  const onlyNamed = useHistory((s) => s.onlyNamed);
  const selected = useHistory((s) => s.selected[doc.id] ?? null);
  const now = Date.now();
  const g = useMemo(() => groupVersions(versions, filter, onlyNamed, now), [versions, filter, onlyNamed, now]);
  const h = useHistory.getState();
  const nothing = g.pinned.length === 0 && g.days.length === 0;
  return (
    <>
      <div className="hx-seg" role="tablist" aria-label="Show versions by">
        {FILTERS.map((f) => (
          <button key={f.id} role="tab" aria-selected={filter === f.id} className={filter === f.id ? "on" : ""} onClick={() => h.setFilter(f.id)}>
            {f.label}
          </button>
        ))}
      </div>
      <label className="hx-switch-row">
        <input type="checkbox" role="switch" checked={onlyNamed} onChange={(e) => h.setOnlyNamed(e.target.checked)} />
        <span className="hx-switch" aria-hidden />
        Only show named versions
      </label>
      <div className="hx-list">
        {g.pinned.length > 0 && (
          <>
            <h4 className="hx-day pinned">
              <Icon name="pin" size={14} /> Pinned
            </h4>
            {g.pinned.map((v) => (
              <Row key={v.id} doc={doc} v={v} selected={selected === v.id} versions={versions} />
            ))}
            {g.days.length > 0 && <hr className="hx-rule" />}
          </>
        )}
        {g.days.map((d) => (
          <section key={d.key}>
            <h4 className="hx-day">{d.label}</h4>
            {d.items.map((v) => (
              <Row key={v.id} doc={doc} v={v} selected={selected === v.id} versions={versions} />
            ))}
          </section>
        ))}
        {nothing && (
          <p className="hx-none dim">
            {onlyNamed ? "No named versions yet. Use “Name this version” on a version to pin it here." : "No versions match this filter."}
          </p>
        )}
      </div>
    </>
  );
}

function Row({ doc, v, selected, versions }: { doc: DocState; v: VersionInfo; selected: boolean; versions: VersionInfo[] }) {
  const h = useHistory.getState();
  const renaming = useHistory((s) => s.renaming === v.id);
  const [draft, setDraft] = useState(v.name ?? "");
  useEffect(() => {
    if (renaming) setDraft(v.name ?? v.label);
  }, [renaming, v.name, v.label]);
  const now = Date.now();
  const toggle = () => h.select(doc.id, selected ? null : v.id);
  const chips = pageChips(v.pagesTouched);
  return (
    <div className={`hx-item${selected ? " sel" : ""}`}>
      <div
        className="hx-row"
        role="button"
        tabIndex={0}
        aria-expanded={selected}
        onClick={toggle}
        onKeyDown={(e) => {
          if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
            e.preventDefault();
            toggle();
          }
        }}
      >
        <span className="hx-ico">
          <Icon name="file" size={20} />
        </span>
        <div className="hx-main">
          {renaming ? (
            <input
              className="hx-name"
              autoFocus
              value={draft}
              placeholder="Name this version"
              aria-label="Version name"
              onClick={(e) => e.stopPropagation()}
              onChange={(e) => setDraft(e.target.value)}
              onFocus={(e) => e.currentTarget.select()}
              onBlur={() => h.setRenaming(null)}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter") void h.rename(doc.id, v.id, draft.trim() || null);
                else if (e.key === "Escape") h.setRenaming(null);
              }}
            />
          ) : (
            <div className="hx-title">{versionTitle(v)}</div>
          )}
          <div className="hx-meta">
            {WHO_WORD[whoOf(v)]} {"·"} {dayLabel(v.created, now) === "Today" ? "" : `${dayLabel(v.created, now)}, `}
            {clock(v.created)}
          </div>
        </div>
        <div className="hx-chips">
          {chips.map((c) => (
            <span key={c} className="hx-chip">
              {c}
            </span>
          ))}
        </div>
        <MenuButton
          className="hx-more"
          label="Version options"
          items={[
            { label: v.name ? "Rename this version" : "Name this version", onSelect: () => h.setRenaming(v.id) },
            ...(v.name ? [{ label: "Remove the name", onSelect: () => void h.rename(doc.id, v.id, null) }] : []),
            { label: "Compare with current", onSelect: () => h.openCompare({ doc: doc.id, a: { kind: "version", id: v.id }, b: { kind: "current" } }) },
            { label: "Save a copy…", onSelect: () => void h.saveCopy(doc.id, v.id) },
            "sep",
            { label: "Restore this version", onSelect: () => void h.restore(doc.id, v.id) },
          ]}
        >
          <Icon name="more" size={16} />
        </MenuButton>
      </div>
      {selected && <Expanded doc={doc} v={v} versions={versions} />}
    </div>
  );
}

function Expanded({ doc, v }: { doc: DocState; v: VersionInfo; versions: VersionInfo[] }) {
  const h = useHistory.getState();
  const ch = useHistory((s) => s.changes[`${doc.id}:${v.id}`]);
  const cur = useHistory((s) => s.current[doc.id] ?? null);
  return (
    <div className="hx-more-wrap">
      <div className="hx-more">
        <div className="hx-changes">
          {ch === "loading" || ch === undefined ? (
            <p className="dim hx-note">Reading changes{"…"}</p>
          ) : "error" in ch ? (
            <p className="hx-note">{ch.error}</p>
          ) : (
            <ChangeLines cmp={ch} v={v} cur={cur} onPick={(i) => h.setCurrent(doc.id, i)} />
          )}
        </div>
        {v.pagesTouched.length > 0 && (
          <div className="hx-pages-row">
            <span className="dim">Pages</span>
            {[...new Set(v.pagesTouched)].sort((a, b) => a - b).map((p) => (
              <button key={p} className="hx-chip btn" onClick={() => useStore.getState().goTo(doc.id, p)}>
                p. {p + 1}
              </button>
            ))}
          </div>
        )}
        <div className="hx-actions">
          <button className="hx-btn primary" onClick={() => void h.restore(doc.id, v.id)}>
            Restore
          </button>
          <button className="hx-btn" onClick={() => void h.saveCopy(doc.id, v.id)}>
            Save a copy
          </button>
          <button className="hx-btn" onClick={() => h.setRenaming(v.id)}>
            Name this version
          </button>
          <button className="hx-btn wide" onClick={() => h.openCompare({ doc: doc.id, a: { kind: "version", id: v.id }, b: { kind: "current" } })}>
            Compare with current
          </button>
        </div>
      </div>
    </div>
  );
}

export function ChangeLines({ cmp, v, cur, onPick }: { cmp: Comparison; v: VersionInfo; cur: number | null; onPick: (i: number) => void }) {
  if (cmp.changes.length === 0)
    return <p className="dim hx-note">{v.kind.type === "import" ? "This is where the history of this file starts." : "No changes you can see in the pages."}</p>;
  return (
    <ul>
      {cmp.changes.map((c, i) => (
        <li key={i}>
          <ChangeButton c={c} on={cur === i} onClick={() => onPick(i)} />
        </li>
      ))}
    </ul>
  );
}

function ChangeButton({ c, on, onClick }: { c: Change; on: boolean; onClick: () => void }) {
  return <RevRow change={c} on={on} onClick={onClick} />;
}
