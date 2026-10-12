import { useEffect, useState } from "react";
import { linkCheckUrl, linkPickFile, type LinkHighlight, type LinkTarget, type NewLinkTarget, type PageLink } from "../../lib/api";
import { SIGNED_LINKS, confirmOpen, isProblem, linkState, rgbToHex, stateLabel, targetLabel, useNav, viewTarget } from "../../lib/nav";
import { type DocState, useStore } from "../../store";
import { Icon } from "../Icons";

const nav = () => useNav.getState();

const WIDTHS = [0.5, 1, 2, 3];
const COLORS: [number, number, number][] = [
  [0.1, 0.4, 0.8],
  [0.1, 0.11, 0.13],
  [0.66, 0.2, 0.12],
  [0.18, 0.49, 0.31],
];

type Kind = "page" | "uri" | "file";

const HIGHLIGHTS: [LinkHighlight, string][] = [
  ["none", "None"],
  ["invert", "Invert"],
  ["outline", "Outline"],
  ["inset", "Inset"],
];

/** The Links section of the Edit rail: draw, set where a link goes, border, and links from addresses. */
export function LinksPanel({ doc }: { doc: DocState }) {
  const id = doc.id;
  const tool = useNav((s) => s.linkTool);
  const d = useNav((s) => s.docs[id]);
  const pending = d?.pending ?? null;
  const showProblems = useNav((s) => s.showProblems);
  const audit = d?.audit ?? null;
  const all = d?.links[doc.currentPage]?.links ?? [];
  const here = showProblems ? all.filter((k) => isProblem(linkState(k.target))) : all;
  // Certified: the engine leaves the links alone, so the controls that change them are off, with the reason.
  const locked = d?.links[doc.currentPage]?.locked ?? null;
  const signed = d?.links[doc.currentPage]?.signed ?? false;
  const sel: PageLink | null = d?.selectedLink ? (d.links[d.selectedLink.page]?.links.find((k) => k.id === d.selectedLink!.id) ?? null) : null;

  // Leaving the section stops drawing.
  useEffect(() => () => nav().setLinkTool("select"), []);
  // "Show problems" checks every page once per document generation.
  useEffect(() => {
    if (showProblems && !audit) void nav().askAudit(id);
  }, [showProblems, audit, id]);

  return (
    <div className="lk">
      <section className="te-sec">
        <h3 className="te-h3">Add a link</h3>
        <button
          className={tool === "draw" ? "te-btn on" : "te-btn"}
          aria-pressed={tool === "draw"}
          title={locked ?? "Drag a rectangle on the page, then choose where it goes"}
          disabled={locked != null}
          onClick={() => nav().setLinkTool(tool === "draw" ? "select" : "draw")}
        >
          <Icon name="link" size={16} />
          Draw a link
        </button>
        {tool === "draw" && !pending && <p className="dim te-hint">Drag over the words or the picture that should be the link.</p>}
        {!pending && tool !== "draw" && !sel && <p className="dim te-hint">Click a link on the page to change it, or draw a new one.</p>}
      </section>

      {pending && (
        <section className="te-sec">
          <h3 className="te-h3">Where does it go?</h3>
          <TargetForm
            key="new"
            doc={doc}
            submitLabel="Create link"
            showBorder
            onSubmit={async (target, border) => {
              await nav().addLink(id, pending.page, pending.rect, target, border);
            }}
            onCancel={() => nav().setPending(id, null)}
          />
        </section>
      )}

      {sel && !pending && <Selected key={`${d?.selectedLink?.page}:${sel.id}`} doc={doc} page={d!.selectedLink!.page} link={sel} locked={locked} />}

      {(locked || signed) && (
        <p className="dim te-hint" role="note">
          {locked ?? SIGNED_LINKS}
        </p>
      )}

      <section className="te-sec">
        <h3 className="te-h3">Check</h3>
        <button
          className={showProblems ? "te-btn on" : "te-btn"}
          aria-pressed={showProblems}
          title="List the links whose destination is missing, across the whole document"
          onClick={() => nav().setShowProblems(!showProblems)}
        >
          <Icon name="check" size={16} />
          Show problems
        </button>
        {showProblems && !audit && <p className="dim te-hint">Checking the links…</p>}
        {showProblems && audit && (
          <>
            <p className="te-summary">
              <b>{audit.broken === 0 ? "No broken links" : `${audit.broken} broken ${audit.broken === 1 ? "link" : "links"}`}</b>
              <br />
              <span className="dim">
                {audit.total} {audit.total === 1 ? "link" : "links"}, {audit.external} to web addresses
              </span>
            </p>
            {audit.rows.length > 0 && (
              <ul className="te-list">
                {audit.rows.map((r) => (
                  <li key={`${r.page}:${r.id}`}>
                    <button
                      className={sel?.id === r.id ? "te-item on" : "te-item"}
                      onClick={() => {
                        useStore.getState().goTo(id, r.page);
                        nav().selectLink(id, { page: r.page, id: r.id });
                      }}
                    >
                      <span className="st-dot broken" aria-hidden />
                      <span className="te-excerpt">
                        Page {r.page + 1} · {r.what}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </section>

      <section className="te-sec">
        <h3 className="te-h3">From the document</h3>
        <Urls doc={doc} locked={locked} />
      </section>

      <section className="te-sec">
        <h3 className="te-h3">All links</h3>
        <button
          className="te-btn"
          disabled={locked != null}
          title={locked ?? "Counts the links of the whole document, then asks before removing them"}
          onClick={() => void nav().removeAllLinks(id)}
        >
          <Icon name="trash" size={16} />
          Remove all links…
        </button>
      </section>

      {here.length > 0 && (
        <section className="te-sec">
          <h3 className="te-h3">
            On page {doc.currentPage + 1} · {here.length} {here.length === 1 ? "link" : "links"}
          </h3>
          <ul className="te-list">
            {here.map((k) => (
              <li key={k.id}>
                <button
                  className={sel?.id === k.id ? "te-item on" : "te-item"}
                  onClick={() => nav().selectLink(id, { page: doc.currentPage, id: k.id })}
                >
                  <span
                    className={`st-dot ${linkState(k.target)}`}
                    role="img"
                    aria-label={stateLabel(linkState(k.target), k.target.kind === "broken" ? k.target.what : undefined)}
                    title={stateLabel(linkState(k.target), k.target.kind === "broken" ? k.target.what : undefined)}
                  />
                  <span className="te-excerpt">{targetLabel(k.target)}</span>
                  {k.target.kind === "other" && <span className="te-tag">kept</span>}
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      {d?.notice && (
        <p className="te-note" role="status">
          {d.notice}
        </p>
      )}
    </div>
  );
}

function Selected({ doc, page, link, locked }: { doc: DocState; page: number; link: PageLink; locked: string | null }) {
  const id = doc.id;
  const [change, setChange] = useState(false);
  const t = link.target;
  const b = link.border;
  const setBorder = (p: Partial<{ visible: boolean; width: number; color: [number, number, number] }>) =>
    void nav().setLinkBorder(id, page, link.id, { visible: p.visible ?? b.visible, width: p.width ?? b.width, color: p.color ?? b.color });
  const [mark] = useState(() => (b.width > 0 ? b.width : 1));

  return (
    <section className="te-sec">
      <h3 className="te-h3">Selected link</h3>
      <p className="te-summary">
        <b>{targetLabel(t)}</b>
        <br />
        <span className="dim">
          <span className={`st-dot ${linkState(t)}`} aria-hidden /> {t.kind === "broken" ? stateLabel("broken", t.what) : describe(t)}
        </span>
      </p>
      <div className="te-row">
        {t.kind === "page" && (
          <button className="te-btn" onClick={() => useStore.getState().goTo(id, t.page, t.at ? Math.max(0, t.at[1] - 8) : undefined)}>
            Go there
          </button>
        )}
        {t.kind === "uri" && (
          <button className="te-btn" title="Asks first, then opens the address in your browser" onClick={() => void confirmOpen(t.uri)}>
            Open address…
          </button>
        )}
        <button
          className={change ? "te-btn on" : "te-btn"}
          aria-expanded={change}
          disabled={t.kind === "other" || locked != null}
          title={locked ?? undefined}
          onClick={() => setChange(!change)}
        >
          Change target
        </button>
        <span className="grow" />
        <button
          className="te-icon"
          aria-label="Delete link"
          title={locked ?? "Delete link (Delete)"}
          disabled={locked != null}
          onClick={() => void nav().removeLinks(id, page, [link.id])}>
          <Icon name="trash" size={16} />
        </button>
      </div>
      {change && (
        <TargetForm
          key={`chg:${link.id}`}
          doc={doc}
          initial={t}
          submitLabel="Apply"
          onSubmit={async (target) => {
            if (await nav().setLinkTarget(id, page, link.id, target)) setChange(false);
          }}
          onCancel={() => setChange(false)}
        />
      )}
      <div className="te-row">
        <span className="te-label">Highlight</span>
        <select
          className="te-select te-keep"
          aria-label="Highlight style"
          title={locked ?? "How the link looks while it is pressed in Read mode"}
          disabled={locked != null}
          value={link.highlight}
          onChange={(e) => void nav().setLinkHighlight(id, page, link.id, e.target.value as LinkHighlight)}
        >
          {HIGHLIGHTS.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </select>
      </div>
      <div className="te-row">
        <span className="te-label">Border</span>
        <label className="te-check">
          <input type="checkbox" checked={b.visible} disabled={locked != null} onChange={(e) => setBorder({ visible: e.target.checked, width: e.target.checked ? mark : b.width })} />
          Visible
        </label>
      </div>
      {b.visible && (
        <>
          <div className="te-row">
            <span className="te-label">Width</span>
            <select className="te-select te-keep" aria-label="Border width" disabled={locked != null} value={nearestWidth(b.width)} onChange={(e) => setBorder({ width: Number(e.target.value) })}>
              {WIDTHS.map((w) => (
                <option key={w} value={w}>
                  {w} pt
                </option>
              ))}
            </select>
          </div>
          <div className="te-row">
            <span className="te-label">Colour</span>
            <span className="te-swatches">
              {COLORS.map((c) => (
                <button
                  key={c.join()}
                  className={sameColor(c, b.color) ? "te-sw on" : "te-sw"}
                  style={{ background: rgbToHex(c) }}
                  aria-label={`Colour ${rgbToHex(c)}`}
                  disabled={locked != null}
                  onClick={() => setBorder({ color: c })}
                />
              ))}
            </span>
          </div>
        </>
      )}
    </section>
  );
}

/** Where a link goes: a page or view of this document, a web address, or a file. */
function TargetForm(p: {
  doc: DocState;
  initial?: LinkTarget;
  submitLabel: string;
  showBorder?: boolean;
  onSubmit: (t: NewLinkTarget, border?: { visible: boolean }) => Promise<void>;
  onCancel: () => void;
}) {
  const { doc, initial } = p;
  const startKind: Kind = initial?.kind === "uri" ? "uri" : initial?.kind === "file" ? "file" : "page";
  const [kind, setKind] = useState<Kind>(startKind);
  const [page, setPage] = useState(String(initial?.kind === "page" ? initial.page + 1 : doc.currentPage + 1));
  const [at, setAt] = useState<[number, number] | null>(initial?.kind === "page" ? initial.at : null);
  const [uri, setUri] = useState(initial?.kind === "uri" ? initial.uri : "");
  const [uriNote, setUriNote] = useState<{ ok: boolean; text: string } | null>(null);
  const [file, setFile] = useState<{ token: string; name: string } | null>(null);
  const [filePage, setFilePage] = useState("");
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState(false);

  // The address is checked as it is typed, by the same gate that runs when it is saved.
  useEffect(() => {
    if (kind !== "uri") return;
    const v = uri.trim();
    if (!v) {
      setUriNote(null);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      linkCheckUrl(v).then(
        (shown) => live && setUriNote({ ok: true, text: shown }),
        (e) => live && setUriNote({ ok: false, text: String(e) }),
      );
    }, 250);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [kind, uri]);

  const n = parseInt(page, 10);
  const pageOk = Number.isFinite(n) && n >= 1 && n <= doc.pageCount;
  const fp = filePage.trim() === "" ? null : parseInt(filePage, 10) - 1;
  const fileOk = file != null && (fp === null || (Number.isFinite(fp) && fp >= 0));
  const ok = kind === "page" ? pageOk : kind === "uri" ? uriNote?.ok === true : fileOk;

  const useView = () => {
    const v = viewTarget(doc.id);
    setPage(String(v.page + 1));
    setAt(v.at);
  };

  const submit = async () => {
    if (!ok || busy) return;
    let target: NewLinkTarget;
    if (kind === "page") target = { kind: "page", page: n - 1, at };
    else if (kind === "uri") target = { kind: "uri", uri: uri.trim() };
    else target = { kind: "file", file: file!.token, page: fp };
    setBusy(true);
    try {
      await p.onSubmit(target, p.showBorder ? { visible } : undefined);
    } finally {
      setBusy(false);
    }
  };

  const choose = async () => {
    const f = await linkPickFile();
    if (f) setFile({ token: f.token, name: baseName(f.path) });
  };

  return (
    <div className="te-format">
      <span className="te-seg" role="radiogroup" aria-label="Target kind">
        {(
          [
            ["page", "A page"],
            ["uri", "A web address"],
            ["file", "A file"],
          ] as [Kind, string][]
        ).map(([k, label]) => (
          <button key={k} role="radio" aria-checked={kind === k} className={kind === k ? "on lk-seg" : "lk-seg"} onClick={() => setKind(k)}>
            {label}
          </button>
        ))}
      </span>

      {kind === "page" && (
        <>
          <div className="te-row">
            <span className="te-label">Page</span>
            <input
              className="te-num te-keep"
              aria-label="Page number"
              inputMode="numeric"
              value={page}
              onChange={(e) => {
                setPage(e.target.value);
                setAt(null);
              }}
              onKeyDown={(e) => e.key === "Enter" && void submit()}
            />
            <span className="dim">of {doc.pageCount}</span>
          </div>
          <div className="te-row">
            <button className="te-btn" onClick={useView} title="Use the page and place the viewer shows now">
              Use current view
            </button>
          </div>
          <p className="dim te-hint">{at ? `Opens page ${pageOk ? n : "?"} scrolled to this place.` : `Opens the top of page ${pageOk ? n : "?"}.`}</p>
        </>
      )}

      {kind === "uri" && (
        <>
          <input
            className="te-text te-keep"
            aria-label="Web address"
            placeholder="https://example.com or name@example.com"
            value={uri}
            autoFocus
            onChange={(e) => setUri(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void submit()}
          />
          {uriNote && (
            <p className={uriNote.ok ? "te-hint lk-ok" : "te-hint lk-bad"} role="status">
              {uriNote.ok ? `Opens ${uriNote.text}` : uriNote.text}
            </p>
          )}
          <p className="dim te-hint">Addresses open only after you confirm.</p>
        </>
      )}

      {kind === "file" && (
        <>
          <div className="te-row">
            <button className="te-btn" onClick={() => void choose()}>
              <Icon name="open" size={16} />
              Choose file…
            </button>
          </div>
          {file && <p className="te-hint lk-ok">{file.name}</p>}
          {!file && initial?.kind === "file" && <p className="dim te-hint">Now: {initial.name}. Choose a file to change it.</p>}
          <div className="te-row">
            <span className="te-label">Page</span>
            <input
              className="te-num te-keep"
              aria-label="Page in the file (optional)"
              inputMode="numeric"
              placeholder="any"
              value={filePage}
              onChange={(e) => setFilePage(e.target.value)}
            />
            <span className="dim">optional, for PDFs</span>
          </div>
          <p className="dim te-hint">
            The link stores the file's name from this document's folder (with ../ to go up), never a folder of this computer. In Read
            mode it opens PDFs only, after you confirm.
          </p>
        </>
      )}

      {p.showBorder && (
        <label className="te-check">
          <input type="checkbox" checked={visible} onChange={(e) => setVisible(e.target.checked)} />
          Show a border around the link
        </label>
      )}
      <div className="te-row">
        <button className="te-btn primary" disabled={!ok || busy} onClick={() => void submit()}>
          {p.submitLabel}
        </button>
        <button className="te-btn" onClick={p.onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}

/** "Create links from web addresses": the count first, then one undo step for all of them. */
function Urls({ doc, locked }: { doc: DocState; locked: string | null }) {
  const u = useNav((s) => s.docs[doc.id]?.urls ?? null);
  const [asked, setAsked] = useState(false);
  return (
    <>
      <button
        className={asked ? "te-btn on" : "te-btn"}
        aria-expanded={asked}
        disabled={locked != null}
        title={locked ?? undefined}
        onClick={() => {
          const next = !asked;
          setAsked(next);
          if (next) void nav().askUrls(doc.id);
        }}
      >
        <Icon name="search" size={16} />
        Links from web addresses
      </button>
      {asked && !u && <p className="dim te-hint">Looking for web addresses…</p>}
      {asked && u && u.count === 0 && <p className="dim te-hint">No web or mail addresses without a link were found.</p>}
      {asked && u && u.count > 0 && (
        <div className="bm-heads">
          <p className="te-summary">
            <b>
              {u.count} {u.count === 1 ? "link" : "links"}
            </b>{" "}
            on {u.pages} {u.pages === 1 ? "page" : "pages"}, for addresses that are text only.
          </p>
          <ul className="bm-preview">
            {u.rows.slice(0, 5).map((r, i) => (
              <li key={i}>
                <span className="bm-pt">{r.url}</span>
                <span className="dim"> p. {r.page + 1}</span>
              </li>
            ))}
            {u.count > 5 && <li className="dim">and {u.count - 5} more</li>}
          </ul>
          <button
            className="te-btn primary"
            onClick={() => {
              setAsked(false);
              void nav().makeFromUrls(doc.id);
            }}
          >
            Make {u.count} {u.count === 1 ? "link" : "links"}
          </button>
        </div>
      )}
    </>
  );
}

function describe(t: LinkTarget): string {
  switch (t.kind) {
    case "page":
      return t.at ? "Goes to a place in this document" : "Goes to a page of this document";
    case "uri":
      return "Opens a web address, after asking";
    case "file":
      return "Opens another file";
    case "broken":
      return "Its destination is missing";
    default:
      return "An action this editor keeps as it is";
  }
}

const baseName = (p: string) => p.split(/[\\/]/).pop() || p;
const nearestWidth = (w: number) => WIDTHS.reduce((a, b) => (Math.abs(b - w) < Math.abs(a - w) ? b : a));
const sameColor = (a: [number, number, number], b: [number, number, number]) => a.every((v, i) => Math.abs(v - b[i]) < 0.03);
