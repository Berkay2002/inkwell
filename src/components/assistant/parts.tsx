import { type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { type Citation, PROVIDERS, PROVIDER_NAMES, type ProviderId } from "../../lib/ai";
import { type Msg, pickOf, providerOf, readViewerSelection, useAi } from "../../lib/aiStore";
import type { DocState } from "../../store";
import { isVerified } from "../../lib/activity";
import { ActivityTrail } from "./ActivityTrail";
import { Markdown } from "./Markdown";
import { PermissionItems } from "./PermissionItems";
import { ProviderMark } from "./providerMarks";

export type Variant = "docked" | "floating" | "bar";

const phaseText = (provider: ProviderId, phase: string): string =>
  phase === "reading"
    ? "Reading the document…"
    : phase === "connecting"
      ? `Connecting to ${PROVIDER_NAMES[provider]}…`
      : "Thinking…";

/** What the user is told about whose subscription pays and where the text goes. */
export const PROVIDER_NOTE: Record<ProviderId, string> = {
  codex: "Uses your ChatGPT login through Codex.",
  claude: "Uses your Claude Code login; usage counts against your Claude plan.",
};

const CLAUDE_INSTALL_URL = "https://docs.claude.com/en/docs/claude-code/setup";

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
  retry: "M5 12a7 7 0 1 0 2.1-5M5 4.5V9h4.5",
  edit: "M5 19l.8-3.8L16.4 4.6a1.9 1.9 0 0 1 2.7 2.7L8.5 17.9zM14.5 6.5l3 3",
  docked: "M4 5.5h16v13H4zM14.5 5.5v13",
  floating: "M4 5.5h16v13H4zM12.5 8.5h5v7h-5z",
  bar: "M4 5.5h16v13H4zM8 14.5h8",
  brain:
    "M12 5.2A3 3 0 0 0 6.6 6.7 3.2 3.2 0 0 0 4.6 10c0 .9.4 1.7 1 2.2A3.3 3.3 0 0 0 7 17a3 3 0 0 0 5 1.6zM12 5.2a3 3 0 0 1 5.4 1.5 3.2 3.2 0 0 1 2 3.3c0 .9-.4 1.7-1 2.2A3.3 3.3 0 0 1 17 17a3 3 0 0 1-5 1.6M12 5.2v13.4",
} as const;

/** `stroke` is the line width in screen px (default: 1.8 of the 24-unit drawing grid). */
export function Glyph({ name, size = 16, stroke }: { name: keyof typeof PATHS; size?: number; stroke?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={stroke ? (stroke * 24) / size : name === "more" ? 3.2 : 1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
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
 *  popover lists what the document's provider offers. */
export function ModelPicker({ doc, up = false, icon = false }: { doc: number; up?: boolean; icon?: boolean }) {
  const provider = useAi((s) => providerOf(s, doc));
  const status = useAi((s) => s.statuses[provider]);
  const settings = useAi((s) => s.settings);
  const { open, setOpen, ref } = usePop();
  const models = typeof status === "object" && status ? status.models : [];
  const pick = pickOf(settings, provider);
  const model = models.find((m) => m.id === pick.model) ?? models.find((m) => m.isDefault) ?? models[0];
  if (!model) return null;
  const efforts = model.efforts;
  const effort = pick.effort && efforts.includes(pick.effort) ? pick.effort : (model.defaultEffort ?? efforts[0]);
  // Codex keeps its choice in `model` / `effort`, Claude in `claudeModel` / `claudeEffort`
  const set = (p: { model?: string; effort?: string }) =>
    useAi.getState().setSettings(
      provider === "claude"
        ? { ...(p.model !== undefined ? { claudeModel: p.model } : {}), ...(p.effort !== undefined ? { claudeEffort: p.effort } : {}) }
        : p,
    );
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
          <Glyph name="brain" size={18} stroke={1.75} />
        </button>
      ) : (
        <button className="ax-pill" title="Model and reasoning effort" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span className="ax-pill-text">{label}</span>
          <Glyph name="down" size={13} />
        </button>
      )}
      {open && (
        <div className={`ax-pop${icon ? " right" : ""}${up ? " up" : ""}`} role="menu">
          <div className="ax-pop-label">Model</div>
          {models.map((m) => (
            <button
              key={m.id}
              role="menuitemradio"
              aria-checked={m.id === model.id}
              onClick={() => {
                // A model may not offer the effort chosen for the previous one.
                const next = pick.effort && m.efforts.includes(pick.effort) ? undefined : (m.defaultEffort ?? m.efforts[0]);
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

/** The provider choices of a document: Codex | Claude with the plan when it is known, then the
 *  note on whose subscription is used. `tick` is the check mark; `after` runs on a pick. */
function ProviderItems({ doc, after }: { doc: number; after?: () => void }) {
  const provider = useAi((s) => providerOf(s, doc));
  const statuses = useAi((s) => s.statuses);
  const busy = useAi((s) => !!s.chats[doc]?.messages.some((m) => m.state === "streaming"));
  const hasMsgs = useAi((s) => (s.chats[doc]?.messages.length ?? 0) > 0);
  return (
    <>
      <div className="ax-pop-label">Assistant</div>
      {PROVIDERS.map((p) => {
        const s = statuses[p];
        const plan = typeof s === "object" && s.loggedIn ? s.plan : null;
        return (
          <button
            key={p}
            role="menuitemradio"
            aria-checked={p === provider}
            disabled={busy}
            onClick={() => {
              useAi.getState().setProvider(doc, p);
              after?.();
            }}
          >
            <span className="ax-tick">{p === provider && <Glyph name="check" size={13} />}</span>
            <ProviderMark provider={p} size={14} />
            {PROVIDER_NAMES[p]}
            {plan && <span className="ax-prov-plan">{cap(plan)}</span>}
          </button>
        );
      })}
      <p className="ax-pop-note">
        {PROVIDER_NOTE[provider]}
        {hasMsgs ? " Switching starts a new chat." : ""}
      </p>
    </>
  );
}

/** The provider in a pill next to the title. */
export function ProviderPicker({ doc, up = false }: { doc: number; up?: boolean }) {
  const provider = useAi((s) => providerOf(s, doc));
  const { open, setOpen, ref } = usePop();
  return (
    <div className="ax-pop-wrap" ref={ref}>
      <button className="ax-pill" title="Which assistant answers" aria-expanded={open} onClick={() => setOpen(!open)}>
        <ProviderMark provider={provider} size={14} />
        <span className="ax-pill-text">{PROVIDER_NAMES[provider]}</span>
        <Glyph name="down" size={13} />
      </button>
      {open && (
        <div className={`ax-pop${up ? " up" : ""}`} role="menu">
          <ProviderItems doc={doc} after={() => setOpen(false)} />
        </div>
      )}
    </div>
  );
}

/** The "..." menu: chat actions. */
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
          <ProviderItems doc={doc.id} after={() => setOpen(false)} />
          <div className="ax-pop-sep" />
          <button onClick={run(() => st().clear(doc.id))}>Clear chat</button>
          <button onClick={run(() => void navigator.clipboard.writeText(st().transcript(doc.id)))}>Copy whole chat</button>
          <button onClick={run(() => st().setSettings({ hideSources: !settings.hideSources }))}>
            {settings.hideSources ? "Show sources under answers" : "Hide sources under answers"}
          </button>
          <div className="ax-pop-sep" />
          <PermissionItems value={settings.permissions} onPick={(p) => run(() => st().setSettings({ permissions: p }))()} tick={<Glyph name="check" size={13} />} />
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
export function Gate({ doc }: { doc: number }) {
  const provider = useAi((s) => providerOf(s, doc));
  const status = useAi((s) => s.statuses[provider] ?? null);
  const claudePath = useAi((s) => s.claudePath);
  const name = PROVIDER_NAMES[provider];
  const other = PROVIDERS.find((p) => p !== provider)!;
  if (status === null || status === "loading")
    return (
      <div className="ax-body">
        <p className="dim">Checking {name}…</p>
      </div>
    );
  const command = provider === "claude" ? "claude login" : "codex login";
  return (
    <div className="ax-body">
      <div className="ax-card">
        <div className="ax-card-mark">
          <ProviderMark provider={provider} size={22} />
        </div>
        {!status.installed ? (
          provider === "claude" ? (
            <>
              <h3>Claude Code isn’t installed</h3>
              <p>
                The assistant uses your Claude subscription through the Claude Code CLI. Install it from{" "}
                <code>{CLAUDE_INSTALL_URL}</code>, then run <code>claude login</code> in a terminal.
              </p>
              <div className="ax-row" style={{ marginBottom: 12 }}>
                <CopyButton text={CLAUDE_INSTALL_URL} label="Copy install link" />
              </div>
              <ClaudePath value={claudePath} />
            </>
          ) : (
            <>
              <h3>Codex isn’t installed</h3>
              <p>
                The assistant uses your ChatGPT subscription through the Codex CLI. Install it, then sign in with{" "}
                <code>codex login</code>.
              </p>
            </>
          )
        ) : provider === "claude" ? (
          <>
            <h3>Sign in to Claude Code</h3>
            <p>
              Run <code>{command}</code> in a terminal and sign in with your Claude account. Inkwell never sees your
              password or token.
            </p>
            <div className="ax-row" style={{ marginBottom: 12 }}>
              <CopyButton text={command} label="Copy command" />
            </div>
          </>
        ) : (
          <>
            <h3>Sign in to Codex</h3>
            <p>
              Run <code>{command}</code> in a terminal and choose “Sign in with ChatGPT”. Inkwell never sees your
              password or token.
            </p>
          </>
        )}
        {status.error && <p className="dim small">{status.error}</p>}
        <div className="ax-row">
          <button className="ax-btn primary" onClick={() => useAi.getState().refreshStatus(provider)}>
            Check again
          </button>
          <button className="ax-btn" onClick={() => useAi.getState().setProvider(doc, other)}>
            Use {PROVIDER_NAMES[other]} instead
          </button>
        </div>
      </div>
    </div>
  );
}

/** A small button that puts `text` on the clipboard and says so. */
function CopyButton({ text, label }: { text: string; label: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      className="ax-btn small"
      onClick={() => {
        void navigator.clipboard.writeText(text);
        setDone(true);
        setTimeout(() => setDone(false), 1500);
      }}
    >
      {done ? "Copied" : label}
    </button>
  );
}

/** Where `claude` lives when it is not on PATH: picked in a file dialog, checked and kept by Rust. */
function ClaudePath({ value }: { value: string | null }) {
  const [why, setWhy] = useState<string | null>(null);
  return (
    <div className="ax-path" style={{ flexWrap: "wrap", alignItems: "center" }}>
      <button
        className="ax-btn small"
        onClick={() => {
          setWhy(null);
          void useAi.getState().pickClaudePath().then(setWhy);
        }}
      >
        Already installed? Choose claude.exe…
      </button>
      {value && (
        <>
          <span
            className="dim small"
            title={value}
            style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
          >
            {value}
          </span>
          <button className="ax-btn small" onClick={() => useAi.getState().clearClaudePath()}>
            Forget
          </button>
        </>
      )}
      {why && (
        <p className="dim small" style={{ flexBasis: "100%", margin: 0 }}>
          {why}
        </p>
      )}
    </div>
  );
}

export function Consent({ doc }: { doc: number }) {
  const provider = useAi((s) => providerOf(s, doc));
  return (
    <div className="ax-body">
      <div className="ax-card">
        <div className="ax-card-mark">
          <ProviderMark provider={provider} size={22} />
        </div>
        {provider === "claude" ? (
          <>
            <h3>Send this document to Anthropic?</h3>
            <p>
              To answer, Inkwell sends the relevant text of this document (and your questions) to Claude through your
              Claude Code login. It counts against your Claude plan. Claude Code keeps its own transcript of the chat on
              this computer (~/.claude/projects).
            </p>
          </>
        ) : (
          <>
            <h3>Send this document to OpenAI?</h3>
            <p>
              To answer, Inkwell sends the relevant text of this document (and your questions) to ChatGPT through your
              Codex login. Nothing is stored by Inkwell: chats disappear when you close the tab.
            </p>
          </>
        )}
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

/** The sources an answer cites. `docked` rows, `floating` cards with a page sketch, `bar` page chips. */
function Sources({ doc, m, variant, active }: { doc: DocState; m: Msg; variant: Variant; active: number | null }) {
  const show = (c: Citation) => useAi.getState().showCitation(doc.id, c, m.id);
  if (variant === "bar")
    return (
      <ul className="ax-sources bar" aria-label="Sources">
        {m.citations.filter(isVerified).map((c) => (
          <li key={c.n}>
            <button
              className={active === c.n ? "ax-pgchip on" : "ax-pgchip"}
              data-source={`${m.id}:${c.n}`}
              title={c.quote}
              // hovering lights the badge on the page without scrolling there; a click is a real jump
              onPointerEnter={() => useAi.getState().liftMark(doc.id, c, m.id)}
              onPointerLeave={() => useAi.getState().dropMark()}
              onFocus={() => useAi.getState().liftMark(doc.id, c, m.id)}
              onBlur={() => useAi.getState().dropMark()}
              onClick={() => show(c)}
            >
              <span className="ax-n">{c.n}</span>
              <span>p. {(c.page ?? 0) + 1}</span>
            </button>
          </li>
        ))}
      </ul>
    );
  return (
    <ul className={`ax-sources ${variant}`}>
      {m.citations.filter(isVerified).map((c) => {
        const on = active === c.n;
        const page = c.page != null ? `p. ${c.page + 1}` : "";
        const cls = on ? "ax-source on" : "ax-source";
        return (
          <li key={c.n}>
            <button className={cls} data-source={`${m.id}:${c.n}`} onClick={() => show(c)} title="Show in document">
              {variant === "floating" && <PageSketch doc={doc} c={c} />}
              {variant === "docked" && (
                <span className="ax-doc-glyph">
                  <Glyph name="doc" size={18} />
                </span>
              )}
              <span className="ax-src-main">
                <span className="ax-src-name">
                  <span className="ax-n">{c.n}</span>
                  {variant === "docked" && <span className="ax-src-file">{doc.name}</span>}
                  {variant === "floating" && page && <span className="ax-pg">{page}</span>}
                </span>
                <span className="ax-src-q">{c.quote}</span>
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

export function Answer({ doc, m, variant, isLast }: { doc: DocState; m: Msg; variant: Variant; isLast: boolean }) {
  const mark = useAi((s) => s.mark);
  const hide = useAi((s) => s.settings.hideSources);
  const cites = useMemo(() => new Map(m.citations.filter(isVerified).map((c) => [c.n, c])), [m.citations]);
  const known = useMemo(() => new Set(cites.keys()), [cites]);
  // source numbers repeat from answer to answer: only the answer the mark came from lights its own
  const active = mark && mark.doc === doc.id && (mark.msg == null || mark.msg === m.id) ? mark.n : null;
  const provider = useAi((s) => providerOf(s, doc.id));

  if (m.state === "error")
    return (
      <div className="ax-a error">
        <p>{m.error}</p>
        {(m.hint === "login" || m.hint === "install") && (
          <button className="ax-btn" onClick={() => useAi.getState().refreshStatus(provider)}>
            Check {PROVIDER_NAMES[provider]} again
          </button>
        )}
      </div>
    );

  return (
    <div className="ax-a">
      <ActivityTrail doc={doc.id} m={m} worked={variant === "bar"} />
      {m.text === "" && m.state === "streaming" ? (
        <p className="dim">{phaseText(provider, m.phase ?? "reading")}</p>
      ) : (
        <Markdown
          text={m.text}
          caret={m.state === "streaming"}
          cites={{ known, active, onCite: (n) => cites.get(n) && useAi.getState().showCitation(doc.id, cites.get(n)!, m.id) }}
        />
      )}
      {m.state === "cancelled" && <p className="dim small">Stopped.</p>}
      {m.state === "done" && (
        <>
          {!hide && m.citations.some(isVerified) && <Sources doc={doc} m={m} variant={variant} active={active} />}
          {variant === "bar" ? (
            <BarActions doc={doc} m={m} isLast={isLast} />
          ) : (
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
          )}
        </>
      )}
    </div>
  );
}

/** The icon row under a reply in the command bar: copy it, ask the same question again. */
function BarActions({ doc, m, isLast }: { doc: DocState; m: Msg; isLast: boolean }) {
  const asked = useAi((s) => {
    const all = s.chats[doc.id]?.messages ?? [];
    const i = all.findIndex((x) => x.id === m.id);
    return i > 0 ? all[i - 1] : undefined;
  });
  const busy = useAi((s) => !!s.chats[doc.id]?.messages.some((x) => x.state === "streaming"));
  return (
    <div className="ax-iconrow">
      <button className="ax-icon small" title="Copy" aria-label="Copy the answer" onClick={() => void navigator.clipboard.writeText(m.text)}>
        <Glyph name="copy" size={14} />
      </button>
      {isLast && asked && (
        <button
          className="ax-icon small"
          title="Ask again"
          aria-label="Ask this again"
          disabled={busy}
          onClick={() =>
            useAi.getState().ask(doc.id, asked.kind === "summarizeDocument" ? { kind: "summarizeDocument" } : { kind: "ask", question: asked.text })
          }
        >
          <Glyph name="retry" size={14} />
        </button>
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

/** The "+" in the composer: what to put in front of the assistant besides typing. */
function AttachMenu({ doc }: { doc: DocState }) {
  const selection = useAi((s) => s.selection);
  const { open, setOpen, ref } = usePop();
  const ask = useAi.getState().ask;
  const items = [
    ...(selection
      ? [{ title: "Explain the selection", run: () => ask(doc.id, { kind: "selection", action: "explain", text: selection.text, page: selection.page }) }]
      : []),
    { title: `Summarize page ${doc.currentPage + 1}`, run: () => ask(doc.id, { kind: "summarizePage", page: doc.currentPage }) },
    { title: "Summarize the document", run: () => ask(doc.id, { kind: "summarizeDocument" }) },
  ];
  return (
    <div className="ax-pop-wrap" ref={ref}>
      <button className="ax-icon" title="Add the selection or page" aria-label="Add the selection or page" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Glyph name="plus" size={18} stroke={2} />
      </button>
      {open && (
        <div className="ax-pop up" role="menu">
          {items.map((i) => (
            <button
              key={i.title}
              onClick={() => {
                setOpen(false);
                i.run();
              }}
            >
              {i.title}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** The question box, after the Codex and ChatGPT composers: one rounded row with "+", the question,
 *  the model as an icon and a round send button. `trail` sits before the model icon. */
export function Composer({
  doc,
  variant,
  trail,
  onSend,
}: {
  doc: DocState;
  variant: Variant;
  trail?: ReactNode;
  onSend?: () => void;
}) {
  const [text, setText] = useState("");
  const box = useRef<HTMLTextAreaElement>(null);
  const streaming = useAi((s) => !!s.chats[doc.id]?.messages.some((m) => m.state === "streaming"));
  const prep = useAi((s) => s.chats[doc.id]?.prep);
  // "Edit" on an earlier question puts its words back here; sending them is a new turn. A draft the user was
  // writing is not thrown away: it stays below the edited question.
  const draft = useAi((s) => (s.draft?.doc === doc.id ? s.draft : null));
  const caret = useRef<number | null>(null);
  useEffect(() => {
    if (!draft) return;
    useAi.getState().clearDraft();
    // (pressing Edit again on the same question finds it already first: nothing to add)
    const next = text.startsWith(draft.text) ? text : draft.text + (text.trim() ? `\n\n${text.trim()}` : "");
    caret.current = draft.text.length;
    setText(next);
    box.current?.focus();
    if (next === text) {
      // nothing will re-render: put the caret now
      box.current?.setSelectionRange(draft.text.length, draft.text.length);
      caret.current = null;
    }
  }, [draft]);
  // the caret goes to the end of the question, once the textarea holds the new text
  useLayoutEffect(() => {
    const t = box.current;
    if (caret.current == null || !t) return;
    t.setSelectionRange(caret.current, caret.current);
    caret.current = null;
  }, [text]);
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
        <AttachMenu doc={doc} />
        <textarea
          ref={box}
          value={text}
          rows={1}
          aria-label="Ask about this document"
          placeholder={noText ? "This PDF has no readable text" : variant === "bar" ? "Ask about this PDF" : "Ask about this document"}
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
        <ModelPicker doc={doc.id} up icon />
        {streaming ? (
          <button className="ax-send" title="Stop" aria-label="Stop" onClick={() => st().cancel(doc.id)}>
            <Glyph name="stop" size={18} stroke={2.5} />
          </button>
        ) : (
          <button className="ax-send" title="Send" aria-label="Send" disabled={!text.trim() || noText} onClick={send}>
            <Glyph name="send" size={18} stroke={2.5} />
          </button>
        )}
      </div>
    </div>
  );
}


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
