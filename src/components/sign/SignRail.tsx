import { useEffect, useRef, useState } from "react";
import { pickSave, pickSource } from "../../lib/api";
import {
  CERTIFY_LABEL, type Identity, type SignatureInfo, SOURCE_LABEL, STATUS, canSign, changeNote, idKey, locked, otherWarnings, outcome, pdfDate, picture, statusWord,
  signApi, signed, uncheckedText, useSign,
} from "../../lib/sign";
import { type DocState, useStore } from "../../store";
import { Icon, type IconName } from "../Icons";
import "./sign.css";

const sg = () => useSign.getState();

/** The right rail: the digital signature sheet, or the document's signatures. */
export function SignRail({ doc }: { doc: DocState }) {
  const rail = useSign((s) => s.rail);
  if (!rail) return null;
  return (
    <aside className="ax-dock sg-dock" style={{ width: "min(360px, 45vw)" }} aria-label={rail === "sheet" ? "Digital signature" : "Signatures"}>
      <section className="ax-panel docked sg-panel">{rail === "sheet" ? <Sheet doc={doc} /> : <List doc={doc} />}</section>
    </aside>
  );
}

function Head({ title, count }: { title: string; count?: number }) {
  return (
    <div className="ax-head">
      <span className="ax-title">{title}</span>
      {count != null && <span className="dim cm-count">{count}</span>}
      <span className="grow" />
      <button className="ax-icon" title="Close" aria-label={`Close ${title}`} onClick={() => sg().setRail(null)}>
        <Icon name="close" size={16} />
      </button>
    </div>
  );
}

const idLabel = (i: Identity) => (i.source === "test" ? `${i.name} (test ID)` : i.name);

function Sheet({ doc }: { doc: DocState }) {
  const s = useSign();
  const d = s.get(doc.id);
  const who = s.ids.find((i) => idKey(i) === s.idChoice) ?? null;
  const [adding, setAdding] = useState<null | "file" | "test">(null);
  useEffect(() => {
    if (!s.idsLoaded) void sg().loadIds();
    void sg().loadSaved();
  }, [s.idsLoaded]);
  // Closing the sheet drops the box on the page.
  useEffect(() => () => sg().setDraft(doc.id, null), [doc.id]);

  const isLocked = locked(d);
  const already = signed(d.sigs).length > 0;
  const pic = picture(s);
  const sign = async () => {
    let to: string | null = null;
    if (doc.scratch) {
      const f = await pickSave(doc.name, { doc: doc.id });
      if (!f) return;
      to = f.token;
    }
    await sg().sign(doc.id, to);
  };

  return (
    <>
      <Head title="Digital signature" />
      <div className="sg-body">
        {isLocked ? (
          <p className="sg-callout">{isLocked}</p>
        ) : !d.draft ? (
          <p className="sg-callout">
            <Icon name="nib" size={15} /> Drag a box on the page where the signature goes, or click an empty signature field.
          </p>
        ) : (
          <p className="dim sg-where">
            {d.draft.field ? `Field "${d.draft.field}"` : "New signature box"} on page {d.draft.page + 1}
          </p>
        )}

        <label className="sg-label" htmlFor="sg-id">
          Signing as
        </label>
        {s.ids.length > 0 ? (
          <>
            <select id="sg-id" className="cm-input sg-select" value={s.idChoice ?? ""} onChange={(e) => sg().setIdChoice(e.target.value)}>
              {s.ids.map((i) => (
                <option key={idKey(i)} value={idKey(i)} disabled={!!i.unusable}>
                  {idLabel(i)}
                </option>
              ))}
            </select>
            {who && (
              <p className="dim sg-idmeta">
                {SOURCE_LABEL[who.source]} · expires {who.notAfter}
                {who.unusable && <span className="sg-bad"> · {who.unusable}</span>}
              </p>
            )}
          </>
        ) : (
          <p className="dim sg-idmeta">{s.idsLoaded ? "No digital ID yet." : "Looking for digital IDs…"}</p>
        )}
        {adding === "file" ? (
          <OpenIdFile onDone={() => setAdding(null)} />
        ) : adding === "test" ? (
          <CreateTestId onDone={() => setAdding(null)} />
        ) : (
          <div className="sg-idlinks">
            <button className="sg-link" onClick={() => setAdding("file")}>
              Open a digital ID file…
            </button>
            <button className="sg-link" onClick={() => setAdding("test")}>
              Create a test ID…
            </button>
          </div>
        )}

        <label className="sg-label" htmlFor="sg-reason">
          Reason
        </label>
        <input id="sg-reason" className="cm-input" placeholder="Optional" maxLength={200} value={s.reason} onChange={(e) => sg().setSheet({ reason: e.target.value })} />

        {pic && (
          <label className="sg-check">
            <input type="checkbox" checked={s.picture} onChange={(e) => sg().setSheet({ picture: e.target.checked })} />
            Show my signature picture
          </label>
        )}
        <label className={already ? "sg-check off" : "sg-check"} title={already ? "Only the first signature can lock the document" : undefined}>
          <input type="checkbox" disabled={already} checked={s.lock && !already} onChange={(e) => sg().setSheet({ lock: e.target.checked })} />
          Lock the document after signing
        </label>

        <p className="dim sg-outcome">{outcome(d.sigs, s.lock && !already, doc.dirty)}</p>
        {d.error && (
          <p className="cm-error" role="alert">
            {d.error}
          </p>
        )}
        <div className="sg-actions">
          {already && (
            <button className="sg-link" onClick={() => sg().setRail("list")}>
              Signatures ({signed(d.sigs).length})
            </button>
          )}
          <span className="grow" />
          <button className="cm-btn primary sg-sign" disabled={!d.draft || !who || !!who.unusable || !!isLocked || s.signing} onClick={() => void sign()}>
            {s.signing ? "Signing…" : "Sign"}
          </button>
        </div>
      </div>
    </>
  );
}

/** A .pfx / .p12 chosen in Windows' dialog, opened with its password for this session. */
function OpenIdFile({ onDone }: { onDone: () => void }) {
  const [file, setFile] = useState<{ token: string; name: string } | null>(null);
  const [pw, setPw] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void pickSource("digitalId", false).then(
      ([f]) => (f ? setFile({ token: f.token, name: f.path.split(/[\\/]/).pop() ?? f.path }) : onDone()),
      (e) => setErr(String(e)),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const open = async () => {
    if (!file) return;
    setBusy(true);
    try {
      sg().addId(await signApi.openIdFile(file.token, pw));
      onDone();
    } catch (e) {
      setErr(String(e));
      setPw("");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="sg-sub">
      {file && <p className="sg-sub-title">{file.name}</p>}
      <input
        className="cm-input"
        type="password"
        placeholder="Password"
        aria-label="Digital ID password"
        autoFocus
        value={pw}
        disabled={!file}
        onChange={(e) => setPw(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && void open()}
      />
      {err && <p className="cm-error">{err}</p>}
      <div className="sg-actions">
        <span className="grow" />
        <button className="cm-btn" onClick={onDone}>
          Cancel
        </button>
        <button className="cm-btn primary" disabled={!file || busy} onClick={() => void open()}>
          Open
        </button>
      </div>
      <p className="dim small">Used for this session only. Nothing is added to Windows' certificate store.</p>
    </div>
  );
}

/** A self-signed ID for trying signing out; its file goes to a temporary folder. */
function CreateTestId({ onDone }: { onDone: () => void }) {
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const create = async () => {
    setBusy(true);
    try {
      sg().addId(await signApi.createTestId(name, email, ""));
      onDone();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="sg-sub">
      <input className="cm-input" placeholder="Name" aria-label="Test ID name" autoFocus maxLength={64} value={name} onChange={(e) => setName(e.target.value)} />
      <input className="cm-input" placeholder="Email" aria-label="Test ID email" maxLength={128} value={email} onChange={(e) => setEmail(e.target.value)} />
      {err && <p className="cm-error">{err}</p>}
      <div className="sg-actions">
        <span className="grow" />
        <button className="cm-btn" onClick={onDone}>
          Cancel
        </button>
        <button className="cm-btn primary" disabled={!name.trim() || !email.trim() || busy} onClick={() => void create()}>
          {busy ? "Creating…" : "Create"}
        </button>
      </div>
      <p className="dim small">Self-signed: others will see "Unknown signer". For trying signing out, not for real documents.</p>
    </div>
  );
}

const TONE_ICON: Record<string, IconName> = { valid: "check", unknown: "question", invalid: "close" };

function List({ doc }: { doc: DocState }) {
  const sigs = useSign((s) => s.docs[doc.id]?.sigs) ?? [];
  const error = useSign((s) => s.docs[doc.id]?.sigsError ?? null);
  const focus = useSign((s) => s.docs[doc.id]?.focus ?? null);
  const done = signed(sigs);
  const empty = sigs.filter((x) => x.status === "unsigned");
  const go = (x: SignatureInfo) => useStore.getState().goTo(doc.id, x.page, Math.max(0, x.rect[1] - 80));
  return (
    <>
      <Head title="Signatures" count={done.length} />
      <div className="sg-body sg-list">
        {error ? <p className="sg-callout">{uncheckedText(error)}</p> : done.length === 0 && <p className="dim">This document has no signatures.</p>}
        {done.map((x) => (
          <Card key={x.field} x={x} focus={x.field === focus?.field ? focus.n : 0} onOpen={() => go(x)} />
        ))}
        {empty.length > 0 && <h3 className="cm-page">Empty signature fields</h3>}
        {empty.map((x) => (
          <div key={x.field} className="sg-empty">
            <span>
              {x.field} <span className="dim">· page {x.page + 1}</span>
            </span>
            <span className="grow" />
            {canSign(x) && (
              <button
                className="sg-link"
                onClick={() => {
                  go(x);
                  sg().signField(doc.id, x);
                }}
              >
                Sign
              </button>
            )}
            {x.refused && <p className="dim small sg-why">{x.refused}</p>}
          </div>
        ))}
        <p className="dim small sg-foot">Trust is checked against Windows' trusted certificates on this PC, without going online.</p>
      </div>
    </>
  );
}

/** `focus` is the request count when the field on the page asked for this card, else 0. */
function Card({ x, focus, onOpen }: { x: SignatureInfo; focus: number; onOpen: () => void }) {
  const focused = focus > 0;
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLElement>(null);
  // Reached from the signature field on the page: show its certificate details and bring the card into view.
  useEffect(() => {
    if (!focus) return;
    setOpen(true);
    ref.current?.scrollIntoView?.({ block: "nearest" });
  }, [focus]);
  const st = STATUS[x.status as keyof typeof STATUS];
  return (
    <article ref={ref} className={`sg-card ${st.tone}${focused ? " focus" : ""}`} aria-current={focused || undefined}>
      <button className="sg-card-main" onClick={onOpen}>
        <span className={`sg-badge ${st.tone}`}>
          <Icon name={TONE_ICON[st.tone]!} size={13} />
        </span>
        <span className="sg-card-text">
          <b>{x.signer || "Unknown"}</b>
          <span className={`sg-state ${st.tone}`}>{statusWord(x)}</span>
          {x.certify != null && <span className="dim">{CERTIFY_LABEL[x.certify]}</span>}
          <span className="dim">{changeNote(x)}</span>
          {otherWarnings(x).map((w) => (
            <span key={w} className="dim">
              {w}
            </span>
          ))}
          {x.intact && !x.trusted && x.trustNote && <span className="dim">Signature intact · {x.trustNote}.</span>}
          <span className="dim">
            Signed {pdfDate(x.time)} per the signer&apos;s clock · page {x.page + 1}
          </span>
          {x.reason && <span className="dim">Reason: {x.reason}</span>}
        </span>
      </button>
      <button className="sg-link sg-more" aria-expanded={open} onClick={() => setOpen(!open)}>
        {open ? "Less" : "Certificate"}
      </button>
      {open && (
        <dl className="sg-details">
          <dt>Issued by</dt>
          <dd>{x.issuer || "—"}</dd>
          <dt>Valid until</dt>
          <dd>{x.certNotAfter || "—"}</dd>
          <dt>Format</dt>
          <dd>
            {x.subFilter} · {x.digest}
          </dd>
          <dt>Fingerprint</dt>
          <dd className="sg-mono">{x.certFingerprint}</dd>
          {x.status === "invalid" && (
            <>
              <dt>Problem</dt>
              <dd>{x.summary}</dd>
            </>
          )}
        </dl>
      )}
    </article>
  );
}
