import { useAi } from "../lib/aiStore";
import type { Changes, Cipher, Printing, SecurityInfo } from "../lib/api";
import * as edits from "../lib/edit";
import {
  CHANGES_LABEL,
  outcome,
  PRINTING_LABEL,
  type ProtectSheet,
  readiness,
  restrictionNotice,
  securityRows,
  strength,
  useProtect,
} from "../lib/protectStore";
import type { DocState } from "../store";
import { Icon } from "./Icons";
import "./tools.css";
import "./protect.css";

const st = () => useProtect.getState();

/**
 * Protect and Security in the same right-hand rail as Compress and Export. Protect sets what the
 * file opens with and what it allows; Security reads what a file has and removes it.
 */
export function ProtectPanel({ doc }: { doc: DocState }) {
  const sheet = useProtect((s) => s.sheet);
  const info = useProtect((s) => s.security[doc.id]);
  const width = useAi((s) => s.settings.width);
  if (!sheet || sheet.doc !== doc.id) return null;
  const title = sheet.view === "security" ? "Security" : "Protect";
  return (
    <aside className="ax-dock" style={{ width: `min(${width}px, 45vw)` }} aria-label={title}>
      <section className="ax-panel docked tl pr">
        <div className="ax-head">
          <span className="ax-title">{title}</span>
          <span className="grow" />
          <button className="ax-icon" title="Close" aria-label={`Close ${title}`} onClick={() => st().close()}>
            <Icon name="close" size={14} />
          </button>
        </div>
        {sheet.phase === "working" && <Working sheet={sheet} />}
        {sheet.phase === "done" && <Done sheet={sheet} doc={doc} />}
        {(sheet.phase === "options" || sheet.phase === "error") &&
          (sheet.view === "security" ? <SecurityView sheet={sheet} info={info} doc={doc} /> : <ProtectView sheet={sheet} info={info} doc={doc} />)}
      </section>
    </aside>
  );
}

function Seg<T extends string>({ label, value, items, onChange }: { label: string; value: T; items: { id: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <div className="ax-seg tl-seg" role="radiogroup" aria-label={label}>
      {items.map((i) => (
        <button key={i.id} role="radio" aria-checked={value === i.id} className={value === i.id ? "on" : ""} onClick={() => onChange(i.id)}>
          {i.label}
        </button>
      ))}
    </div>
  );
}

/** Five thin segments and a word: the word carries the meaning, the fill only repeats it. */
export function Meter({ password }: { password: string }) {
  const s = strength(password);
  return (
    <div className="pr-meter" aria-live="polite">
      <span className="pr-segs" aria-hidden>
        {[1, 2, 3, 4, 5].map((i) => (
          <span key={i} className={i <= s.segments ? "on" : ""} />
        ))}
      </span>
      <span className="pr-strength">{s.label ? `Password strength: ${s.label}` : " "}</span>
    </div>
  );
}

function Password({ label, value, onChange, bad }: { label: string; value: string; onChange: (v: string) => void; bad?: boolean }) {
  return (
    <label className="pr-field">
      <span className="tl-label">{label}</span>
      <input
        className={bad ? "tl-field bad" : "tl-field"}
        type="password"
        autoComplete="off"
        spellCheck={false}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}

const PRINTING: Printing[] = ["none", "low", "high"];
const CHANGES: Changes[] = ["none", "pages", "forms", "comments", "allButExtract"];
const CIPHERS: { id: Cipher; label: string }[] = [
  { id: "aes256", label: "AES-256" },
  { id: "aes128", label: "AES-128 (older readers)" },
];

function ProtectView({ sheet, info, doc }: { sheet: ProtectSheet; info: SecurityInfo | undefined; doc: DocState }) {
  const set = (p: Partial<ProtectSheet>) => st().set(p);
  const why = readiness(sheet);
  const needsOwner = !!info?.encrypted && !info.unlocked;
  const sentence = outcome(sheet);
  const mismatch = sheet.openConfirm !== "" && sheet.openConfirm !== sheet.openPassword;
  const pmismatch = sheet.permissionsConfirm !== "" && sheet.permissionsConfirm !== sheet.permissionsPassword;
  const ready = why == null && (!needsOwner || sheet.currentPassword !== "");
  return (
    <>
      <div className="ax-body tl-body">
        {info?.encrypted && (
          <p className="dim small pr-note">
            This file already has password security ({info.method ?? "unknown method"}). What you set here replaces it.
          </p>
        )}
        {needsOwner && (
          <>
            <Password label="Current permissions password" value={sheet.currentPassword} onChange={(currentPassword) => set({ currentPassword })} />
          </>
        )}

        <div className="pr-group">
          <label className="tl-switch">
            <input type="checkbox" checked={sheet.requireOpen} onChange={(e) => set({ requireOpen: e.target.checked })} />
            <span>Require a password to open</span>
          </label>
          {sheet.requireOpen && (
            <>
              <Password label="Password" value={sheet.openPassword} onChange={(openPassword) => set({ openPassword })} />
              <Password label="Confirm password" value={sheet.openConfirm} onChange={(openConfirm) => set({ openConfirm })} bad={mismatch} />
              <Meter password={sheet.openPassword} />
              {mismatch && <p className="dim small">The two passwords don't match yet.</p>}
            </>
          )}
        </div>

        <div className="pr-group">
          <label className="tl-switch">
            <input type="checkbox" checked={sheet.restrict} onChange={(e) => set({ restrict: e.target.checked })} />
            <span>Restrict editing and printing</span>
          </label>
          {sheet.restrict && (
            <>
              <Password label="Permissions password" value={sheet.permissionsPassword} onChange={(permissionsPassword) => set({ permissionsPassword })} />
              <Password
                label="Confirm permissions password"
                value={sheet.permissionsConfirm}
                onChange={(permissionsConfirm) => set({ permissionsConfirm })}
                bad={pmismatch}
              />
              <Meter password={sheet.permissionsPassword} />
              <label className="tl-label" htmlFor="pr-printing">
                Printing allowed
              </label>
              <select id="pr-printing" className="tl-field" value={sheet.printing} onChange={(e) => set({ printing: e.target.value as Printing })}>
                {PRINTING.map((p) => (
                  <option key={p} value={p}>
                    {PRINTING_LABEL[p]}
                  </option>
                ))}
              </select>
              <label className="tl-label" htmlFor="pr-changes">
                Changes allowed
              </label>
              <select id="pr-changes" className="tl-field" value={sheet.changes} onChange={(e) => set({ changes: e.target.value as Changes })}>
                {CHANGES.map((c) => (
                  <option key={c} value={c}>
                    {CHANGES_LABEL[c]}
                  </option>
                ))}
              </select>
              <label className="tl-switch">
                <input type="checkbox" checked={sheet.copy} onChange={(e) => set({ copy: e.target.checked })} />
                <span>Allow copying of text and images</span>
              </label>
              <label className="tl-switch">
                <input
                  type="checkbox"
                  checked={sheet.accessibility || sheet.copy}
                  disabled={sheet.copy}
                  onChange={(e) => set({ accessibility: e.target.checked })}
                />
                <span>Allow screen readers</span>
              </label>
            </>
          )}
        </div>

        <label className="tl-label">Encryption</label>
        <Seg<Cipher> label="Encryption" value={sheet.cipher} items={CIPHERS} onChange={(cipher) => set({ cipher })} />

        {sheet.error && (
          <p className="tl-warn small" role="alert">
            {sheet.error}
          </p>
        )}
      </div>
      <div className="tl-foot">
        {sheet.requireOpen && <p className="dim small pr-warn">If you lose the open password, the file can't be recovered. Inkwell does not keep a copy of it.</p>}
        <p className="pr-outcome" aria-live="polite">
          {sentence.map((s, i) => (
            <span key={s + i}>{s} </span>
          ))}
        </p>
        {why && <p className="dim small">{why}</p>}
        {sheet.confirmInPlace ? (
          <>
            <p className="small">
              Replace <span className="tl-mono">{doc.name}</span> with the protected file? The earlier version stays in the version history.
            </p>
            <button className="ax-btn primary tl-wide" disabled={!ready} onClick={() => void st().run("inPlace")}>
              Protect and replace this file
            </button>
            <button className="ax-btn tl-wide" onClick={() => set({ confirmInPlace: false })}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <button className="ax-btn primary tl-wide" disabled={!ready} onClick={() => void st().run("copy")}>
              Protect and save a copy…
            </button>
            <button className="ax-btn tl-wide pr-quiet" disabled={!ready} onClick={() => set({ confirmInPlace: true })}>
              Protect this file instead
            </button>
          </>
        )}
      </div>
    </>
  );
}

function SecurityView({ sheet, info, doc }: { sheet: ProtectSheet; info: SecurityInfo | undefined; doc: DocState }) {
  const set = (p: Partial<ProtectSheet>) => st().set(p);
  if (!info) {
    return (
      <div className="ax-body tl-body">
        <p className="dim">Reading the security of this file…</p>
      </div>
    );
  }
  const rows = securityRows(info);
  const needsOwner = info.encrypted && !info.unlocked;
  return (
    <>
      <div className="ax-body tl-body">
        <dl className="pr-rows pr-file">
          <div>
            <dt>File</dt>
            <dd className="tl-mono">{doc.name}</dd>
          </div>
          <div>
            <dt>Pages</dt>
            <dd>{doc.pageCount}</dd>
          </div>
        </dl>
        <label className="tl-label">Security</label>
        {!info.encrypted ? (
          <>
            <p className="tl-big">No password security</p>
            <p className="dim">{info.origin === "removed" ? "It was removed here. Save the file to keep that." : "Anyone can open this file and do anything with it."}</p>
          </>
        ) : (
          <>
            <p className="dim small">
              {restrictionNotice(info) ?? (info.openPassword ? "Opening this file needs a password." : "This file has no restrictions.")}
              {info.unsaved ? " Not saved yet." : ""}
            </p>
            <dl className="pr-rows">
              {rows.map((r) => (
                <div key={r.label}>
                  <dt>{r.label}</dt>
                  <dd>{r.value}</dd>
                </div>
              ))}
            </dl>
          </>
        )}
        {info.encrypted && needsOwner && (
          <>
            <Password
              label="Permissions password"
              value={sheet.currentPassword}
              bad={!!sheet.error}
              onChange={(currentPassword) => set({ currentPassword, error: null })}
            />
            <p className="dim small">Removing security needs the permissions password. You can undo it before saving.</p>
          </>
        )}
        {info.encrypted && !needsOwner && <p className="dim small">Removing security is one step you can undo before saving.</p>}
        {sheet.error && (
          <p className="tl-warn small" role="alert">
            {sheet.error}
          </p>
        )}
      </div>
      <div className="tl-foot">
        {info.encrypted && (
          <button className="ax-btn primary tl-wide" disabled={needsOwner && sheet.currentPassword === ""} onClick={() => void st().remove()}>
            Remove security
          </button>
        )}
        <button
          className={info.encrypted ? "ax-btn tl-wide pr-quiet" : "ax-btn primary tl-wide"}
          onClick={() => set({ view: "protect", error: null })}
        >
          {info.encrypted ? "Change security…" : "Protect with a password…"}
        </button>
      </div>
    </>
  );
}

function Working({ sheet }: { sheet: ProtectSheet }) {
  return (
    <div className="ax-body tl-body">
      <p className="tl-big">{sheet.view === "security" ? "Removing security" : "Protecting the file"}</p>
      <p className="dim">Writing the file and checking it opens again.</p>
    </div>
  );
}

function Done({ sheet, doc }: { sheet: ProtectSheet; doc: DocState }) {
  const r = sheet.result;
  const removed = r?.output === "removed";
  return (
    <>
      <div className="ax-body tl-body">
        <p className="tl-big">{removed ? "Security removed" : r?.output === "copy" ? "Protected copy saved" : "File protected"}</p>
        <p className="dim small tl-file tl-mono">{r?.name}</p>
        <p className="dim">
          {removed
            ? doc.dirty
              ? "Save the file to keep this. You can undo it before saving."
              : "The file is saved without password security."
            : r?.output === "copy"
              ? "This tab is now the protected copy. The original file is unchanged."
              : "The earlier version is in the version history."}
        </p>
      </div>
      <div className="tl-foot">
        {removed && doc.dirty && (
          <button className="ax-btn primary tl-wide" onClick={() => void edits.save(doc.id)}>
            Save
          </button>
        )}
        <button className={removed && doc.dirty ? "ax-btn tl-wide" : "ax-btn primary tl-wide"} onClick={() => st().close()}>
          Done
        </button>
      </div>
    </>
  );
}

/** A slim strip above the page of a file that restricts something: what it restricts, never silently dropped. */
export function SecurityNotice({ doc }: { doc: DocState }) {
  const info = useProtect((s) => s.security[doc.id]);
  const dismissed = useProtect((s) => s.dismissed[doc.id]);
  const text = info && info.origin === "file" ? restrictionNotice(info) : null;
  if (!info || !text || dismissed) return null;
  return (
    <div className="pr-notice" role="status">
      <Icon name="lock" size={14} />
      <span>
        {text} The owner's restrictions are kept when you save.
      </span>
      <button className="tl-link" onClick={() => st().open("security", doc.id)}>
        Details
      </button>
      <button className="ax-icon" title="Dismiss" aria-label="Dismiss" onClick={() => st().dismiss(doc.id)}>
        <Icon name="close" size={12} />
      </button>
    </div>
  );
}
