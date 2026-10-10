import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Citation } from "../../lib/ai";
import { type Msg, readViewerSelection, useAi } from "../../lib/aiStore";
import type { DocState } from "../../store";
import { Markdown } from "./Markdown";
import "./assistant.css";

const PHASES: Record<string, string> = {
  reading: "Reading the document…",
  connecting: "Connecting to Codex…",
  thinking: "Thinking…",
};

const sparkle =
  "M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9ZM19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8Z";

export function SparkIcon({ size = 16 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d={sparkle} />
    </svg>
  );
}

export function AssistantToggle() {
  const open = useAi((s) => s.open);
  return (
    <button
      className={open ? "icon-btn on" : "icon-btn"}
      title="AI Assistant (Ctrl+Shift+A)"
      aria-label="AI Assistant"
      onClick={() => useAi.getState().toggle()}
    >
      <SparkIcon />
    </button>
  );
}

export function AssistantPanel({ doc }: { doc: DocState }) {
  const open = useAi((s) => s.open);
  const width = useAi((s) => s.settings.width);
  const drag = useRef<{ x: number; w: number } | null>(null);

  // Selection tracking: a selection in the viewer is kept until the user clicks in the viewer again
  // (clicking a panel button must not lose it).
  useEffect(() => {
    if (!open) return;
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
  }, [open]);

  // A different tab means a different document: its selection is not this one's.
  useEffect(() => useAi.getState().setSelection(null), [doc.id]);

  useEffect(() => {
    if (open) useAi.getState().prepare(doc.id);
  }, [open, doc.id]);

  if (!open) return null;
  return (
    <aside className="assistant" style={{ width }} aria-label="AI Assistant">
      <div
        className="assistant-grip"
        onPointerDown={(e) => {
          drag.current = { x: e.clientX, w: width };
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          if (!drag.current) return;
          const w = Math.round(Math.min(720, Math.max(300, drag.current.w + drag.current.x - e.clientX)));
          useAi.setState((s) => ({ settings: { ...s.settings, width: w } }));
        }}
        onPointerUp={() => {
          drag.current = null;
          useAi.getState().setSettings({});
        }}
      />
      <Body doc={doc} />
    </aside>
  );
}

function Body({ doc }: { doc: DocState }) {
  const status = useAi((s) => s.status);
  const chat = useAi((s) => s.chats[doc.id]);
  const pending = useAi((s) => s.pending);
  const ready = typeof status === "object" && status !== null && status.installed && status.loggedIn;

  return (
    <>
      <Header doc={doc} />
      {pending && pending.doc === doc.id ? (
        <Consent />
      ) : !ready ? (
        <Gate />
      ) : chat && chat.messages.length > 0 ? (
        <Messages doc={doc} />
      ) : (
        <Empty doc={doc} />
      )}
      {ready && !(pending && pending.doc === doc.id) && <Composer doc={doc} />}
      <div className="assistant-foot">Sent to OpenAI through your Codex login. Chats are not saved.</div>
    </>
  );
}

function Header({ doc }: { doc: DocState }) {
  const status = useAi((s) => s.status);
  const settings = useAi((s) => s.settings);
  const [menu, setMenu] = useState(false);
  const models = typeof status === "object" && status ? status.models : [];
  const model = models.find((m) => m.id === settings.model) ?? models.find((m) => m.isDefault) ?? models[0];
  const efforts = model?.efforts ?? [];
  const st = useAi.getState;

  return (
    <div className="assistant-head">
      <span className="assistant-title">
        <SparkIcon size={15} /> Assistant
      </span>
      <span className="grow" />
      {models.length > 0 && (
        <>
          <select
            className="assistant-select"
            title="Model"
            value={model?.id}
            onChange={(e) => st().setSettings({ model: e.target.value })}
          >
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
          {efforts.length > 0 && (
            <select
              className="assistant-select"
              title="Reasoning effort: lower is faster"
              value={efforts.includes(settings.effort) ? settings.effort : (model?.defaultEffort ?? efforts[0])}
              onChange={(e) => st().setSettings({ effort: e.target.value })}
            >
              {efforts.map((e) => (
                <option key={e} value={e}>
                  {e}
                </option>
              ))}
            </select>
          )}
        </>
      )}
      <div className="assistant-menu">
        <button className="icon-btn" title="More" aria-label="More" onClick={() => setMenu(!menu)}>
          ⋯
        </button>
        {menu && (
          <div className="assistant-pop" onMouseLeave={() => setMenu(false)}>
            <button
              onClick={() => {
                st().clear(doc.id);
                setMenu(false);
              }}
            >
              Clear chat
            </button>
            <button
              onClick={() => {
                void navigator.clipboard.writeText(st().transcript(doc.id));
                setMenu(false);
              }}
            >
              Copy whole chat
            </button>
            <button onClick={() => st().setSettings({ hideSources: !settings.hideSources })}>
              {settings.hideSources ? "Show sources under answers" : "Hide sources under answers"}
            </button>
            <button
              onClick={() => {
                st().setOpen(false);
                setMenu(false);
              }}
            >
              Close panel
            </button>
          </div>
        )}
      </div>
      <button className="icon-btn" title="Close" aria-label="Close assistant" onClick={() => st().setOpen(false)}>
        ✕
      </button>
    </div>
  );
}

/** Not installed / not signed in / still checking. */
function Gate() {
  const status = useAi((s) => s.status);
  const refresh = () => useAi.getState().refreshStatus();
  if (status === null || status === "loading")
    return (
      <div className="assistant-body">
        <p className="dim">Checking Codex…</p>
      </div>
    );
  return (
    <div className="assistant-body">
      <div className="card">
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
        <button className="btn primary" onClick={refresh}>
          Check again
        </button>
      </div>
    </div>
  );
}

function Consent() {
  return (
    <div className="assistant-body">
      <div className="card">
        <h3>Send this document to OpenAI?</h3>
        <p>
          To answer, Inkwell sends the relevant text of this document (and your questions) to ChatGPT through your
          Codex login. Nothing is stored by Inkwell: chats disappear when you close the tab.
        </p>
        <div className="row">
          <button className="btn primary" onClick={() => useAi.getState().grantConsent()}>
            Allow and continue
          </button>
          <button className="btn" onClick={() => useAi.getState().declineConsent()}>
            Not now
          </button>
        </div>
      </div>
    </div>
  );
}

function Empty({ doc }: { doc: DocState }) {
  const prep = useAi((s) => s.chats[doc.id]?.prep ?? null);
  const ask = useAi.getState().ask;
  const noText = typeof prep === "object" && prep !== null && !prep.enough;
  if (noText)
    return (
      <div className="assistant-body">
        <div className="card">
          <h3>No readable text</h3>
          <p>This PDF has no selectable text (it is probably a scan), so the assistant has nothing to read.</p>
        </div>
      </div>
    );
  const starters: { title: string; hint: string; run: () => void }[] = [
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
  return (
    <div className="assistant-body">
      <p className="assistant-lead">Ask about “{doc.name}” or start with one of these.</p>
      <div className="starters">
        {starters.map((s) => (
          <button key={s.title} className="starter" onClick={s.run}>
            <strong>{s.title}</strong>
            <span>{s.hint}</span>
          </button>
        ))}
      </div>
      {prep === "loading" && <p className="dim small">Reading the document…</p>}
    </div>
  );
}

function Messages({ doc }: { doc: DocState }) {
  const messages = useAi((s) => s.chats[doc.id]?.messages ?? []);
  const el = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const last = messages[messages.length - 1];

  useLayoutEffect(() => {
    const e = el.current;
    if (e && stick.current) e.scrollTop = e.scrollHeight;
  }, [messages]);

  return (
    <div
      className="assistant-body messages"
      ref={el}
      onScroll={(e) => {
        const t = e.currentTarget;
        stick.current = t.scrollHeight - t.scrollTop - t.clientHeight < 48;
      }}
    >
      {messages.map((m) => (m.role === "user" ? <div key={m.id} className="msg user">{m.text}</div> : <Answer key={m.id} doc={doc} m={m} isLast={m === last} />))}
    </div>
  );
}

function Answer({ doc, m, isLast }: { doc: DocState; m: Msg; isLast: boolean }) {
  const mark = useAi((s) => s.mark);
  const hide = useAi((s) => s.settings.hideSources);
  const cites = useMemo(() => new Map(m.citations.map((c) => [c.n, c])), [m.citations]);
  const known = useMemo(() => new Set(cites.keys()), [cites]);
  const show = (c: Citation) => useAi.getState().showCitation(doc.id, c);
  const active = mark && mark.doc === doc.id ? mark.n : null;
  const ask = useAi.getState().ask;

  if (m.state === "error")
    return (
      <div className="msg bot error">
        <p>{m.error}</p>
        {(m.hint === "login" || m.hint === "install") && (
          <button className="btn" onClick={() => useAi.getState().refreshStatus()}>
            Check Codex again
          </button>
        )}
      </div>
    );

  return (
    <div className="msg bot">
      {m.text === "" && m.state === "streaming" ? (
        <p className="dim">{PHASES[m.phase ?? "reading"] ?? "Thinking…"}</p>
      ) : (
        <Markdown text={m.text} cites={{ known, active, onCite: (n) => cites.get(n) && show(cites.get(n)!) }} />
      )}
      {m.state === "streaming" && m.text !== "" && <span className="caret" />}
      {m.state === "cancelled" && <p className="dim small">Stopped.</p>}
      {m.state === "done" && (
        <>
          {!hide && m.citations.length > 0 && (
            <ul className="sources">
              {m.citations.map((c) => (
                <li key={c.n}>
                  <button className={active === c.n ? "source on" : "source"} onClick={() => show(c)} title="Show in document">
                    <span className="cite">{c.n}</span>
                    <span className="pg">{c.page != null ? `p. ${c.page + 1}` : "—"}</span>
                    <span className="q">{c.quote}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          <div className="msg-actions">
            <button className="link" onClick={() => void navigator.clipboard.writeText(m.text)}>
              Copy
            </button>
            {m.timing && (
              <span className="dim small" title={`prep ${m.timing.prepMs} ms`}>
                {m.timing.firstTokenMs != null ? `${(m.timing.firstTokenMs / 1000).toFixed(1)} s to first word · ` : ""}
                {(m.timing.totalMs / 1000).toFixed(1)} s
              </span>
            )}
          </div>
          {isLast && m.followups.length > 0 && (
            <div className="followups">
              {m.followups.map((f) => (
                <button key={f} className="chip" onClick={() => ask(doc.id, { kind: "ask", question: f })}>
                  {f}
                </button>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}

function Composer({ doc }: { doc: DocState }) {
  const [text, setText] = useState("");
  const selection = useAi((s) => s.selection);
  const messages = useAi((s) => s.chats[doc.id]?.messages);
  const streaming = !!messages?.some((m) => m.state === "streaming");
  const prep = useAi((s) => s.chats[doc.id]?.prep);
  const noText = typeof prep === "object" && prep !== null && !prep.enough;
  const st = useAi.getState;

  const send = () => {
    const q = text.trim();
    if (!q || streaming || noText) return;
    setText("");
    st().ask(doc.id, { kind: "ask", question: q });
  };
  const sel = (action: "explain" | "rewrite" | "summarize") => {
    if (!selection || streaming) return;
    st().ask(doc.id, { kind: "selection", action, text: selection.text, page: selection.page });
    st().setSelection(null);
    window.getSelection()?.removeAllRanges();
  };

  return (
    <div className="assistant-compose">
      {selection && (
        <div className="selstrip">
          <span className="selq" title={selection.text}>
            p. {selection.page + 1} · “{selection.text.slice(0, 80)}”
          </span>
          <div className="row">
            <button className="chip" disabled={streaming} onClick={() => sel("explain")}>
              Explain
            </button>
            <button className="chip" disabled={streaming} onClick={() => sel("rewrite")}>
              Rewrite
            </button>
            <button className="chip" disabled={streaming} onClick={() => sel("summarize")}>
              Summarize
            </button>
            <button className="icon-btn small" title="Dismiss" aria-label="Dismiss selection" onClick={() => st().setSelection(null)}>
              ✕
            </button>
          </div>
        </div>
      )}
      <div className="input-row">
        <textarea
          value={text}
          rows={1}
          placeholder={noText ? "This PDF has no readable text" : "Ask about this document"}
          disabled={noText}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        {streaming ? (
          <button className="btn" onClick={() => st().cancel(doc.id)}>
            Stop
          </button>
        ) : (
          <button className="btn primary" disabled={!text.trim() || noText} onClick={send}>
            Send
          </button>
        )}
      </div>
    </div>
  );
}
