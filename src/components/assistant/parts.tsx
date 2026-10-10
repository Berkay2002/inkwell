import { type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { type Citation, MODES, MODE_NAMES } from "../../lib/ai";
import { type Msg, readViewerSelection, useAi } from "../../lib/aiStore";
import type { DocState } from "../../store";
import { Markdown } from "./Markdown";

export type Variant = "docked" | "floating" | "bar";

const PHASES: Record<string, string> = {
  reading: "Reading the document…",
  connecting: "Connecting to Codex…",
  thinking: "Thinking…",
};

const PATHS = {
  doc: "M7 3.5h6.5L18 8v12.5H7zM13.5 3.5V8H18",
  more: "M5.5 12h.01M12 12h.01M18.5 12h.01",
  close: "M6.5 6.5l11 11M17.5 6.5l-11 11",
  send: "M12 18.5v-13M6.5 11l5.5-5.5 5.5 5.5",
  stop: "M8 8h8v8H8z",
  chev: "M9.5 6.5l5.5 5.5-5.5 5.5",
  down: "M7 10l5 5 5-5",
  check: "M6 12.5l4 4 8-9",
  copy: "M9 9h9.5v11H9zM6 15V4h9.5",
  plus: "M12 6v12M6 12h12",
  brain:
    "M12 5.2A3 3 0 0 0 6.6 6.7 3.2 3.2 0 0 0 4.6 10c0 .9.4 1.7 1 2.2A3.3 3.3 0 0 0 7 17a3 3 0 0 0 5 1.6zM12 5.2a3 3 0 0 1 5.4 1.5 3.2 3.2 0 0 1 2 3.3c0 .9-.4 1.7-1 2.2A3.3 3.3 0 0 1 17 17a3 3 0 0 1-5 1.6M12 5.2v13.4",
} as const;

export function Glyph({ name, size = 16 }: { name: keyof typeof PATHS; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={name === "more" ? 3.2 : 1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d={PATHS[name]} />
    </svg>
  );
}

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** A popover that closes on an outside press or Escape. */
export function usePop() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", down, true);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", down, true);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  return { open, setOpen, ref };
}

/** The model and effort in one pill (or, with `icon`, a brain icon whose tooltip names them); the
 *  popover lists what Codex offers. */
export function ModelPicker({ up = false, icon = false }: { up?: boolean; icon?: boolean }) {
  const status = useAi((s) => s.status);
  const settings = useAi((s) => s.settings);
  const { open, setOpen, ref } = usePop();
  const models = typeof status === "object" && status ? status.models : [];
  const model = models.find((m) => m.id === settings.model) ?? models.find((m) => m.isDefault) ?? models[0];
  if (!model) return null;
  const efforts = model.efforts;
  const effort = efforts.includes(settings.effort) ? settings.effort : (model.defaultEffort ?? efforts[0]);
  const set = useAi.getState().setSettings;
  const label = `${model.name}${effort ? ` · ${cap(effort)}` : ""}`;

  return (
    <div className="ax-pop-wrap" ref={ref}>
      {icon ? (
        <button
          className="ax-icon"
          title={`Model and reasoning effort: ${label}`}
          aria-label={`Model and reasoning effort: ${label}`}
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          <Glyph name="brain" size={18} />
        </button>
      ) : (
        <button className="ax-pill" title="Model and reasoning effort" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span className="ax-pill-text">{label}</span>
          <Glyph name="down" size={13} />
        </button>
      )}
      {open && (
        <div className={up ? "ax-pop up" : "ax-pop"} role="menu">
          <div className="ax-pop-label">Model</div>
          {models.map((m) => (
            <button
              key={m.id}
              role="menuitemradio"
              aria-checked={m.id === model.id}
              onClick={() => {
                // A model may not offer the effort chosen for the previous one.
                const next = m.efforts.includes(settings.effort) ? undefined : (m.defaultEffort ?? m.efforts[0]);
                set({ model: m.id, ...(next ? { effort: next } : {}) });
              }}
            >
              <span className="ax-tick">{m.id === model.id && <Glyph name="check" size={13} />}</span>
              {m.name}
            </button>
          ))}
          {efforts.length > 0 && (
            <>
              <div className="ax-pop-label" title="Lower is faster">
                Reasoning effort
              </div>
              {efforts.map((e) => (
                <button key={e} role="menuitemradio" aria-checked={e === effort} onClick={() => set({ effort: e })}>
                  <span className="ax-tick">{e === effort && <Glyph name="check" size={13} />}</span>
                  {cap(e)}
                </button>
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}

/** The "..." menu: chat actions and the layout. */
export function MoreMenu({ doc, up = false }: { doc: DocState; up?: boolean }) {
  const settings = useAi((s) => s.settings);
  const { open, setOpen, ref } = usePop();
  const st = useAi.getState;
  const run = (f: () => void) => () => {
    f();
    setOpen(false);
  };
  return (
    <div className="ax-pop-wrap" ref={ref}>
      <button className="ax-icon" title="More" aria-label="More" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Glyph name="more" />
      </button>
      {open && (
        <div className={up ? "ax-pop right up" : "ax-pop right"} role="menu">
          <button onClick={run(() => st().clear(doc.id))}>Clear chat</button>
          <button onClick={run(() => void navigator.clipboard.writeText(st().transcript(doc.id)))}>Copy whole chat</button>
          <button onClick={run(() => st().setSettings({ hideSources: !settings.hideSources }))}>
            {settings.hideSources ? "Show sources under answers" : "Hide sources under answers"}
          </button>
          <div className="ax-pop-label">Layout</div>
          {MODES.map((m) => (
            <button key={m} role="menuitemradio" aria-checked={m === settings.mode} onClick={run(() => st().setMode(m))}>
              <span className="ax-tick">{m === settings.mode && <Glyph name="check" size={13} />}</span>
              {MODE_NAMES[m]}
            </button>
          ))}
          <div className="ax-pop-sep" />
          <button onClick={run(() => st().setOpen(false))}>Close assistant</button>
        </div>
      )}
    </div>
  );
}

export function CloseButton({ onClick }: { onClick?: () => void }) {
  return (
    <button className="ax-icon" title="Close" aria-label="Close assistant" onClick={onClick ?? (() => useAi.getState().setOpen(false))}>
      <Glyph name="close" />
    </button>
  );
}

/** Not installed / not signed in / still checking. */
export function Gate() {
  const status = useAi((s) => s.status);
  if (status === null || status === "loading")
    return (
      <div className="ax-body">
        <p className="dim">Checking Codex…</p>
      </div>
    );
  return (
    <div className="ax-body">
      <div className="ax-card">
        {!status.installed ? (
          <>
            <h3>Codex isn’t installed</h3>
            <p>
              The assistant uses your ChatGPT subscription through the Codex CLI. Install it, then sign in with{" "}
              <code>codex login</code>.
            </p>
          </>
        ) : (
          <>
            <h3>Sign in to Codex</h3>
            <p>
              Run <code>codex login</code> in a terminal and choose “Sign in with ChatGPT”. Inkwell never sees your
              password or token.
            </p>
          </>
        )}
        {status.error && <p className="dim small">{status.error}</p>}
        <button className="ax-btn primary" onClick={() => useAi.getState().refreshStatus()}>
          Check again
        </button>
      </div>
    </div>
  );
}

export function Consent() {
  return (
    <div className="ax-body">
      <div className="ax-card">
        <h3>Send this document to OpenAI?</h3>
        <p>
          To answer, Inkwell sends the relevant text of this document (and your questions) to ChatGPT through your
          Codex login. Nothing is stored by Inkwell: chats disappear when you close the tab.
        </p>
        <div className="ax-row">
          <button className="ax-btn primary" onClick={() => useAi.getState().grantConsent()}>
            Allow and continue
          </button>
          <button className="ax-btn" onClick={() => useAi.getState().declineConsent()}>
            Not now
          </button>
        </div>
      </div>
    </div>
  );
}

export function NoText() {
  return (
    <div className="ax-card">
      <h3>No readable text</h3>
      <p>This PDF has no selectable text (it is probably a scan), so the assistant has nothing to read.</p>
    </div>
  );
}

export function startersFor(doc: DocState) {
  const ask = useAi.getState().ask;
  return [
    { title: "Summarize this document", hint: "Short overview with page references", run: () => ask(doc.id, { kind: "summarizeDocument" }) },
    {
      title: "Key takeaways",
      hint: "The main points as a list",
      run: () => ask(doc.id, { kind: "ask", question: "List the key takeaways of this document." }),
    },
    {
      title: "What stands out?",
      hint: "Important or surprising details",
      run: () => ask(doc.id, { kind: "ask", question: "What are the most important or surprising points in this document?" }),
    },
    {
      title: `Summarize page ${doc.currentPage + 1}`,
      hint: "Only the page you are on",
      run: () => ask(doc.id, { kind: "summarizePage", page: doc.currentPage }),
    },
  ];
}

export function Empty({ doc }: { doc: DocState }) {
  const prep = useAi((s) => s.chats[doc.id]?.prep ?? null);
  if (typeof prep === "object" && prep !== null && !prep.enough)
    return (
      <div className="ax-body">
        <NoText />
      </div>
    );
  return (
    <div className="ax-body">
      <p className="ax-lead">Ask about “{doc.name}” or start with one of these.</p>
      <div className="ax-starters">
        {startersFor(doc).map((s) => (
          <button key={s.title} className="ax-starter" onClick={s.run}>
            <strong>{s.title}</strong>
            <span>{s.hint}</span>
          </button>
        ))}
      </div>
      {prep === "loading" && <p className="dim small">Reading the document…</p>}
    </div>
  );
}

/** The full conversation, or only its last exchange (the command bar's sheet). */
export function Messages({ doc, variant, lastOnly = false }: { doc: DocState; variant: Variant; lastOnly?: boolean }) {
  const all = useAi((s) => s.chats[doc.id]?.messages);
  const messages = useMemo(() => {
    const m = all ?? [];
    return lastOnly ? m.slice(-2) : m;
  }, [all, lastOnly]);
  const el = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const last = messages[messages.length - 1];

  useLayoutEffect(() => {
    const e = el.current;
    if (e && stick.current) e.scrollTop = e.scrollHeight;
  }, [messages]);

  return (
    <div
      className="ax-body ax-messages"
      ref={el}
      onScroll={(e) => {
        const t = e.currentTarget;
        stick.current = t.scrollHeight - t.scrollTop - t.clientHeight < 48;
      }}
    >
      {messages.map((m) =>
        m.role === "user" ? (
          <div key={m.id} className="ax-q">
            {m.text}
          </div>
        ) : (
          <Answer key={m.id} doc={doc} m={m} variant={variant} isLast={m === last} />
        ),
      )}
    </div>
  );
}

/** The sources an answer cites. `docked` rows, `floating` cards with a page sketch, `bar` compact rows. */
function Sources({ doc, m, variant, active }: { doc: DocState; m: Msg; variant: Variant; active: number | null }) {
  const show = (c: Citation) => useAi.getState().showCitation(doc.id, c);
  return (
    <ul className={`ax-sources ${variant}`}>
      {m.citations.map((c) => {
        const on = active === c.n;
        const page = c.page != null ? `p. ${c.page + 1}` : "";
        const cls = on ? "ax-source on" : "ax-source";
        return (
          <li key={c.n}>
            <button className={cls} data-source={c.n} onClick={() => show(c)} title="Show in document">
              {variant === "floating" && <PageSketch doc={doc} c={c} />}
              {variant === "docked" && (
                <span className="ax-doc-glyph">
                  <Glyph name="doc" size={18} />
                </span>
              )}
              <span className="ax-src-main">
                {variant !== "bar" && (
                  <span className="ax-src-name">
                    <span className="ax-n">{c.n}</span>
                    {variant === "docked" && <span className="ax-src-file">{doc.name}</span>}
                    {variant === "floating" && page && <span className="ax-pg">{page}</span>}
                  </span>
                )}
                <span className="ax-src-q">
                  {variant === "bar" && <span className="ax-n">{c.n}</span>}
                  {c.quote}
                </span>
              </span>
              {variant !== "floating" && page && <span className="ax-pg">{page}</span>}
            </button>
          </li>
        );
      })}
    </ul>
  );
}

/** A tiny page with the cited passage marked where it sits. The tile streams are all in use by the
 *  viewer and the sidebar, so a drawn sketch stands in for a rendered thumbnail. */
function PageSketch({ doc, c }: { doc: DocState; c: Citation }) {
  const size = c.page != null ? doc.sizes[c.page] : undefined;
  const H = size?.[1] || 792;
  const W = size?.[0] || 612;
  const r = c.rects[0];
  const top = r ? Math.min(0.92, Math.max(0.04, r[1] / H)) : 0.4;
  const left = r ? Math.min(0.6, Math.max(0.08, r[0] / W)) : 0.1;
  const width = r ? Math.min(0.84 - left + 0.08, Math.max(0.2, (r[2] - r[0]) / W)) : 0.6;
  return (
    <span className="ax-sketch" aria-hidden>
      <i className="ax-sk-line" style={{ top: "12%", width: "62%" }} />
      <i className="ax-sk-line" style={{ top: "22%" }} />
      <i className="ax-sk-line" style={{ top: "62%" }} />
      <i className="ax-sk-line" style={{ top: "72%", width: "70%" }} />
      <i className="ax-sk-mark" style={{ top: `${top * 100}%`, left: `${left * 100}%`, width: `${width * 100}%` }} />
    </span>
  );
}

function Followups({ doc, m, variant }: { doc: DocState; m: Msg; variant: Variant }) {
  const ask = useAi.getState().ask;
  return (
    <div className={`ax-followups ${variant}`}>
      {m.followups.map((f) => (
        <button key={f} className="ax-chip" onClick={() => ask(doc.id, { kind: "ask", question: f })}>
          <span>{f}</span>
          {variant === "floating" && <Glyph name="chev" size={14} />}
        </button>
      ))}
    </div>
  );
}

export function Answer({ doc, m, variant, isLast, plain = false }: { doc: DocState; m: Msg; variant: Variant; isLast: boolean; plain?: boolean }) {
  const mark = useAi((s) => s.mark);
  const hide = useAi((s) => s.settings.hideSources);
  const cites = useMemo(() => new Map(m.citations.map((c) => [c.n, c])), [m.citations]);
  const known = useMemo(() => new Set(cites.keys()), [cites]);
  const active = mark && mark.doc === doc.id ? mark.n : null;

  if (m.state === "error")
    return (
      <div className="ax-a error">
        <p>{m.error}</p>
        {(m.hint === "login" || m.hint === "install") && (
          <button className="ax-btn" onClick={() => useAi.getState().refreshStatus()}>
            Check Codex again
          </button>
        )}
      </div>
    );

  return (
    <div className="ax-a">
      {m.text === "" && m.state === "streaming" ? (
        <p className="dim">{PHASES[m.phase ?? "reading"] ?? "Thinking…"}</p>
      ) : (
        <Markdown
          text={m.text}
          caret={m.state === "streaming"}
          cites={{ known, active, onCite: (n) => cites.get(n) && useAi.getState().showCitation(doc.id, cites.get(n)!) }}
        />
      )}
      {m.state === "cancelled" && <p className="dim small">Stopped.</p>}
      {m.state === "done" && (
        <>
          {!hide && m.citations.length > 0 && <Sources doc={doc} m={m} variant={variant} active={active} />}
          <div className="ax-actions">
            <button className="ax-link" onClick={() => void navigator.clipboard.writeText(m.text)}>
              Copy
            </button>
            {m.timing && (
              <span className="dim small" title={`prep ${m.timing.prepMs} ms`}>
                {m.timing.firstTokenMs != null ? `${(m.timing.firstTokenMs / 1000).toFixed(1)} s to first word · ` : ""}
                {(m.timing.totalMs / 1000).toFixed(1)} s
              </span>
            )}
          </div>
          {isLast && !plain && m.followups.length > 0 && <Followups doc={doc} m={m} variant={variant} />}
        </>
      )}
    </div>
  );
}

/** What the user selected in the page, with the actions on it. */
function SelStrip({ doc, variant }: { doc: DocState; variant: Variant }) {
  const selection = useAi((s) => s.selection);
  const streaming = useAi((s) => !!s.chats[doc.id]?.messages.some((m) => m.state === "streaming"));
  if (!selection) return null;
  const st = useAi.getState;
  const act = (action: "explain" | "rewrite" | "summarize") => {
    if (streaming) return;
    st().ask(doc.id, { kind: "selection", action, text: selection.text, page: selection.page });
    st().setSelection(null);
    window.getSelection()?.removeAllRanges();
  };
  return (
    <div className={`ax-selstrip ${variant}`}>
      <span className="ax-selq" title={selection.text}>
        p. {selection.page + 1} · “{selection.text.slice(0, 80)}”
      </span>
      <span className="ax-row">
        {(["explain", "rewrite", "summarize"] as const).map((a) => (
          <button key={a} className="ax-chip small" disabled={streaming} onClick={() => act(a)}>
            {cap(a)}
          </button>
        ))}
        <button className="ax-icon small" title="Dismiss" aria-label="Dismiss selection" onClick={() => st().setSelection(null)}>
          <Glyph name="close" size={13} />
        </button>
      </span>
    </div>
  );
}

/** The question box: a pill with a round send button. `lead` sits inside it before the text, `trail`
 *  before the send button. */
export function Composer({
  doc,
  variant,
  lead,
  trail,
  onSend,
}: {
  doc: DocState;
  variant: Variant;
  lead?: ReactNode;
  trail?: ReactNode;
  onSend?: () => void;
}) {
  const [text, setText] = useState("");
  const streaming = useAi((s) => !!s.chats[doc.id]?.messages.some((m) => m.state === "streaming"));
  const prep = useAi((s) => s.chats[doc.id]?.prep);
  const noText = typeof prep === "object" && prep !== null && !prep.enough;
  const st = useAi.getState;

  const send = () => {
    const q = text.trim();
    if (!q || streaming || noText) return;
    setText("");
    onSend?.();
    st().ask(doc.id, { kind: "ask", question: q });
  };

  return (
    <div className={`ax-compose ${variant}`}>
      <SelStrip doc={doc} variant={variant} />
      <div className="ax-input">
        {lead}
        <textarea
          value={text}
          rows={1}
          aria-label="Ask about this document"
          placeholder={noText ? "This PDF has no readable text" : "Ask about this document"}
          disabled={noText}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
            // Ctrl+Arrow moves by word here; it must not turn the page.
            if ((e.ctrlKey || e.metaKey) && e.key.startsWith("Arrow")) e.stopPropagation();
          }}
        />
        {trail}
        {streaming ? (
          <button className="ax-send" title="Stop" aria-label="Stop" onClick={() => st().cancel(doc.id)}>
            <Glyph name="stop" size={14} />
          </button>
        ) : (
          <button className="ax-send" title="Send" aria-label="Send" disabled={!text.trim() || noText} onClick={send}>
            <Glyph name="send" size={16} />
          </button>
        )}
      </div>
    </div>
  );
}

export const FOOTNOTE = "Sent to OpenAI through your Codex login. Chats are not saved.";

/** A selection in the viewer is kept until the user clicks in the viewer again (clicking a panel
 *  button or opening the palette must not lose it). Runs while the panel is closed too, so the
 *  palette's "explain selected text" has something to read. */
export function useViewerSelection(docId: number) {
  useEffect(() => {
    const onSel = () => {
      const s = readViewerSelection();
      if (s) useAi.getState().setSelection(s);
    };
    const onDown = (e: MouseEvent) => {
      if ((e.target as Element | null)?.closest(".viewer")) useAi.getState().setSelection(null);
    };
    document.addEventListener("selectionchange", onSel);
    document.addEventListener("mousedown", onDown, true);
    return () => {
      document.removeEventListener("selectionchange", onSel);
      document.removeEventListener("mousedown", onDown, true);
    };
  }, []);
  // A different tab means a different document: its selection is not this one's.
  useEffect(() => useAi.getState().setSelection(null), [docId]);
}
