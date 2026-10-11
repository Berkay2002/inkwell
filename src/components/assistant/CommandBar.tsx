import { type CSSProperties, type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { type Msg, providerOf, useAi } from "../../lib/aiStore";
import { type DocState, useStore } from "../../store";
import { Answer, CloseButton, Composer, Consent, Gate, Glyph, MoreMenu, ProviderPicker, usePop } from "./parts";
import { type BarAt, barStage, surfaceLeft, surfaceMaxH, surfaceWidth, useBar } from "./barView";

// The command bar, after ChatGPT's companion composer: one rounded surface that is the composer at rest,
// grows a header strip once there is a chat, and stretches upward into the whole conversation.

interface Box {
  l: number;
  r: number;
  t: number;
  b: number;
}

/** The viewer area's rectangle in window coordinates, kept current. */
function useStageBox(): Box | null {
  const [box, setBox] = useState<Box | null>(null);
  useLayoutEffect(() => {
    const el = document.querySelector(".stage");
    if (!el) return;
    const f = () => {
      // the part of the area that is on screen: a layout wider than the window must not push the bar out of it
      const b = el.getBoundingClientRect();
      const n = { l: b.left, r: Math.min(b.right, window.innerWidth), t: b.top, b: Math.min(b.bottom, window.innerHeight) };
      setBox((p) => (p && p.l === n.l && p.r === n.r && p.t === n.t && p.b === n.b ? p : n));
    };
    f();
    const ro = new ResizeObserver(f);
    ro.observe(el);
    window.addEventListener("resize", f);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", f);
    };
  }, []);
  return box;
}

const BOTTOM_GAP = 18;

export function Bar({ doc }: { doc: DocState }) {
  const box = useStageBox();
  const status = useAi((s) => s.statuses[providerOf(s, doc.id)]);
  const pending = useAi((s) => s.pending?.doc === doc.id);
  const count = useAi((s) => s.chats[doc.id]?.messages.length ?? 0);
  const hasPast = useAi((s) => (s.past[doc.id]?.length ?? 0) > 0);
  const needsAnswer = useAi((s) => !!s.chats[doc.id]?.messages.some((m) => m.activity.some((a) => a.state === "waiting")));
  const view = useBar((s) => s.view);
  const at = useBar((s) => s.at);
  const dispatch = useBar((s) => s.dispatch);
  const wrap = useRef<HTMLDivElement>(null);
  const fold = useRef<HTMLDivElement>(null);
  const ready = typeof status === "object" && status !== null && status.installed && status.loggedIn;
  const forced = pending || !ready;
  const hasChat = count > 0;
  const stage = barStage(view, { forced, hasChat, hasPast });
  const open = stage === "open";

  // A message that was not there before is a send: the answer is made to be seen. Switching tabs is not one.
  const seen = useRef({ doc: doc.id, n: count });
  useEffect(() => {
    const p = seen.current;
    if (p.doc === doc.id && count > p.n) dispatch("send");
    seen.current = { doc: doc.id, n: count };
  }, [doc.id, count, dispatch]);
  // A turn that waits for Apply / Reject is paused until it is seen.
  useEffect(() => {
    if (needsAnswer) dispatch("attention");
  }, [needsAnswer, dispatch]);

  // Esc steps down one stage; a press on the page closes the chat. Neither fires while a menu, the find bar
  // or a dialog has the key.
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || e.ctrlKey || e.metaKey || e.altKey) return;
      if (useStore.getState().searchOpen || document.querySelector(".ax-pop")) return;
      const a = document.activeElement;
      if (a && a !== document.body && !wrap.current?.contains(a) && !a.closest(".viewer, .stage")) return;
      dispatch("esc");
    };
    const down = (e: PointerEvent) => {
      if ((e.target as Element | null)?.closest(".viewer")) dispatch("outside");
    };
    // The shortcut that toggles the assistant focuses an open bar that does not have the focus yet.
    const focusBar = (e: KeyboardEvent) => {
      if (!(e.ctrlKey && e.shiftKey && !e.altKey && e.key.toLowerCase() === "a")) return;
      if (wrap.current?.contains(document.activeElement)) return;
      const t = wrap.current?.querySelector<HTMLTextAreaElement>("textarea");
      if (!t || t.disabled) return;
      e.preventDefault();
      e.stopPropagation();
      t.focus();
    };
    window.addEventListener("keydown", focusBar, true);
    window.addEventListener("keydown", key, true);
    document.addEventListener("pointerdown", down, true);
    return () => {
      window.removeEventListener("keydown", focusBar, true);
      window.removeEventListener("keydown", key, true);
      document.removeEventListener("pointerdown", down, true);
    };
  }, [dispatch]);

  // The shortcut that opens the assistant lands in the bar.
  useEffect(() => {
    wrap.current?.querySelector<HTMLTextAreaElement>("textarea")?.focus();
  }, [ready, pending]);

  // The chat folds away as inert: a focus inside it would be lost with it, so it moves to the composer first.
  useLayoutEffect(() => {
    if (open || !fold.current?.contains(document.activeElement)) return;
    wrap.current?.querySelector<HTMLTextAreaElement>("textarea")?.focus();
  }, [open]);

  if (!box) return null;
  const stageW = box.r - box.l;
  const w = surfaceWidth(stageW);
  const header = stage !== "bar";

  return (
    <>
      <div
        ref={wrap}
        className="ax-bar"
        style={
          {
            left: surfaceLeft(at, box.l, box.r, w),
            bottom: window.innerHeight - box.b + BOTTOM_GAP,
            width: w,
            "--ax-surface-max": `${surfaceMaxH(box.b - box.t)}px`,
          } as CSSProperties
        }
        onFocusCapture={(e) => {
          if ((e.target as Element).closest(".ax-input textarea")) dispatch("focus");
        }}
        onPointerDownCapture={(e) => {
          if ((e.target as Element).closest(".ax-input")) dispatch("focus");
        }}
      >
        <section className={`ax-bar-surface ${stage}`} aria-label="Assistant">
          {header && (forced ? <PlainHeader doc={doc} /> : <BarHeader doc={doc} open={open} />)}
          <div className="ax-bar-fold" ref={fold} data-open={open} inert={!open}>
            <div className="ax-bar-fold-in">
              {pending ? <Consent doc={doc.id} /> : !ready ? <Gate doc={doc.id} /> : <Thread doc={doc} open={open} />}
            </div>
          </div>
          {ready && !pending && <Composer doc={doc} variant="bar" trail={<MoreMenu doc={doc} up />} />}
        </section>
      </div>
      <Connector />
    </>
  );
}

// ---- header ------------------------------------------------------------------------------------

const Line = ({ d }: { d: string }) => (
  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    <path d={d} />
  </svg>
);
const MINUS = "M6 12h12";
const GRIP = "M9.5 6h.01M14.5 6h.01M9.5 12h.01M14.5 12h.01M9.5 18h.01M14.5 18h.01";
const UP = "M7 14l5-5 5 5";
const DOWN = "M7 10l5 5 5-5";

/** Gate and consent have no chat yet: a title and Close. */
function PlainHeader({ doc }: { doc: DocState }) {
  return (
    <div className="ax-bar-head">
      <span className="ax-bar-title static">Assistant</span>
      <ProviderPicker doc={doc.id} />
      <span className="grow" />
      <CloseButton />
    </div>
  );
}

const firstQuestion = (messages: readonly Msg[] | undefined) => messages?.find((m) => m.role === "user")?.text.trim() || "New chat";

function BarHeader({ doc, open }: { doc: DocState; open: boolean }) {
  const dispatch = useBar((s) => s.dispatch);
  return (
    <div
      className="ax-bar-head"
      onClick={(e) => {
        if (!(e.target as Element).closest("button, .ax-pop")) dispatch("headerClick");
      }}
    >
      <button className="ax-icon small" title="Minimize (Esc)" aria-label="Minimize" onClick={() => dispatch("minimize")}>
        <Line d={MINUS} />
      </button>
      <ChatsMenu doc={doc} />
      <span className="grow" />
      <ProviderPicker doc={doc.id} />
      <button
        className="ax-icon small"
        title={open ? "Collapse the chat" : "Show the chat"}
        aria-label={open ? "Collapse the chat" : "Show the chat"}
        aria-expanded={open}
        onClick={() => dispatch("headerClick")}
      >
        <Line d={open ? DOWN : UP} />
      </button>
      <LayoutMenu />
    </div>
  );
}

/** The chat's title; its menu lists this document's chats and starts a new one. */
function ChatsMenu({ doc }: { doc: DocState }) {
  const messages = useAi((s) => s.chats[doc.id]?.messages);
  const past = useAi((s) => s.past[doc.id]);
  const { open, setOpen, ref } = usePop();
  const st = useAi.getState;
  const streaming = !!messages?.some((m) => m.state === "streaming");
  return (
    <div className="ax-pop-wrap ax-bar-chats" ref={ref}>
      <button className="ax-bar-title" aria-haspopup="menu" aria-expanded={open} title="Chats of this document" onClick={() => setOpen(!open)}>
        <span className="ax-bar-title-text">{firstQuestion(messages)}</span>
        <Glyph name="down" size={13} />
      </button>
      {open && (
        <div className="ax-pop up wide" role="menu">
          <div className="ax-pop-label">Chats of this document</div>
          <button role="menuitemradio" aria-checked onClick={() => setOpen(false)}>
            <span className="ax-tick">
              <Glyph name="check" size={13} />
            </span>
            <span className="ax-pop-text">{firstQuestion(messages)}</span>
          </button>
          {(past ?? []).map((p) => (
            <button
              key={p.id}
              role="menuitemradio"
              aria-checked={false}
              disabled={streaming}
              onClick={() => {
                setOpen(false);
                st().openChat(doc.id, p.id);
              }}
            >
              <span className="ax-tick" />
              <span className="ax-pop-text">{firstQuestion(p.messages)}</span>
            </button>
          ))}
          <div className="ax-pop-sep" />
          <button
            role="menuitem"
            onClick={() => {
              setOpen(false);
              st().newChat(doc.id);
            }}
          >
            <span className="ax-tick">
              <Glyph name="plus" size={13} />
            </span>
            New chat
          </button>
        </div>
      )}
    </div>
  );
}

const AT_WORDS: Record<BarAt, string> = { left: "Move to left", center: "Move to centre", right: "Move to right" };

/** The grip menu: where the chat lives (side panel) and where the bar sits. */
function LayoutMenu() {
  const { open, setOpen, ref } = usePop();
  const at = useBar((s) => s.at);
  const setAt = useBar((s) => s.setAt);
  const dispatch = useBar((s) => s.dispatch);
  const run = (f: () => void) => () => {
    setOpen(false);
    f();
  };
  return (
    <div className="ax-pop-wrap" ref={ref}>
      <button className="ax-icon small" title="Chat options" aria-label="Chat options" aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Line d={GRIP} />
      </button>
      {open && (
        <div className="ax-pop up right" role="menu">
          <button role="menuitem" onClick={run(() => useAi.getState().setMode("docked"))}>
            Open in side panel
          </button>
          <div className="ax-pop-sep" />
          {(["left", "center", "right"] as const)
            .filter((p) => p !== at)
            .map((p) => (
              <button key={p} role="menuitem" onClick={run(() => setAt(p))}>
                {AT_WORDS[p]}
              </button>
            ))}
          <div className="ax-pop-sep" />
          <button role="menuitem" onClick={run(() => dispatch("minimize"))}>
            Minimize
          </button>
        </div>
      )}
    </div>
  );
}

// ---- thread ------------------------------------------------------------------------------------

const GAP_MS = 30 * 60 * 1000;
const clock = (at: number) => new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

/** "Today 14:32", "Yesterday 09:05", "12 Oct 14:32". */
function stamp(at: number): string {
  const days = Math.round((new Date().setHours(0, 0, 0, 0) - new Date(at).setHours(0, 0, 0, 0)) / 864e5);
  if (days === 0) return `Today ${clock(at)}`;
  if (days === 1) return `Yesterday ${clock(at)}`;
  return `${new Date(at).toLocaleDateString([], { day: "numeric", month: "short" })} ${clock(at)}`;
}

/** The whole conversation: a centred time, right-aligned question bubbles and plain answers. */
function Thread({ doc, open }: { doc: DocState; open: boolean }) {
  const messages = useAi((s) => s.chats[doc.id]?.messages);
  const restored = useAi((s) => s.chats[doc.id]?.restored);
  const streaming = !!messages?.some((m) => m.state === "streaming");
  const el = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useLayoutEffect(() => {
    const e = el.current;
    if (e && stick.current) e.scrollTop = e.scrollHeight;
  }, [messages, open]);

  const last = messages?.[messages.length - 1];
  let prevAt = 0;
  const rows: ReactNode[] = [];
  // The assistant does not remember a chat that was brought back: a line marks where it starts afresh.
  const note = (
    <p key="restored" className="ax-restored">
      End of an earlier chat. The assistant starts fresh from your next question.
    </p>
  );
  const list = messages ?? [];
  for (const [i, m] of list.entries()) {
    if (i === restored) rows.push(note);
    if (m.at != null && m.at - prevAt > GAP_MS)
      rows.push(
        <div key={`t${m.id}`} className="ax-datesep">
          {stamp(m.at)}
        </div>,
      );
    if (m.at != null) prevAt = m.at;
    rows.push(m.role === "user" ? <Question key={m.id} doc={doc.id} m={m} /> : <Answer key={m.id} doc={doc} m={m} variant="bar" isLast={m === last} plain />);
  }
  if (restored != null && restored >= list.length) rows.push(note);

  return (
    <div
      className="ax-bar-thread"
      ref={el}
      role="log"
      aria-live="polite"
      aria-busy={streaming}
      aria-label="Conversation"
      onScroll={(e) => {
        const t = e.currentTarget;
        stick.current = t.scrollHeight - t.scrollTop - t.clientHeight < 48;
      }}
    >
      {rows}
    </div>
  );
}

function Question({ doc, m }: { doc: number; m: Msg }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="ax-qrow">
      <div className="ax-q">{m.text}</div>
      <div className="ax-qmeta">
        {m.at != null && <time dateTime={new Date(m.at).toISOString()}>{clock(m.at)}</time>}
        <button className="ax-icon small" title="Edit" aria-label="Edit the question" onClick={() => useAi.getState().setDraft(doc, m.text)}>
          <Glyph name="edit" size={13} />
        </button>
        <button
          className="ax-icon small"
          title={copied ? "Copied" : "Copy"}
          aria-label="Copy the question"
          onClick={() => {
            void navigator.clipboard.writeText(m.text);
            setCopied(true);
            setTimeout(() => setCopied(false), 1200);
          }}
        >
          <Glyph name={copied ? "check" : "copy"} size={13} />
        </button>
      </div>
    </div>
  );
}

// ---- connector ---------------------------------------------------------------------------------

/** A thin curve from a page chip in the chat to its badge on the page. */
function Connector() {
  const mark = useAi((s) => s.mark);
  const [line, setLine] = useState<{ x1: number; y1: number; x2: number; y2: number } | null>(null);
  useEffect(() => {
    if (!mark) {
      setLine(null);
      return;
    }
    let raf = 0;
    const tick = () => {
      // source numbers repeat from answer to answer: the chip of the answer the mark came from
      const src = document.querySelector<HTMLElement>(
        mark.msg != null ? `.ax-bar [data-source="${mark.msg}:${mark.n}"]` : `.ax-bar [data-source$=":${mark.n}"]`,
      );
      const badge = document.querySelector<HTMLElement>(".cite-badge");
      const stage = document.querySelector(".stage")?.getBoundingClientRect();
      let next = null;
      if (src && badge && stage) {
        const a = src.getBoundingClientRect();
        const b = badge.getBoundingClientRect();
        const x2 = b.left + b.width / 2;
        const y2 = b.top + b.height / 2;
        // leave from the edge of the chip that faces the badge
        const up = y2 < a.top;
        if (x2 > stage.left && x2 < stage.right && y2 > stage.top && y2 < stage.bottom && a.width > 0)
          next = { x1: Math.round(a.left + a.width / 2), y1: Math.round(up ? a.top : a.bottom), x2: Math.round(x2), y2: Math.round(y2) };
      }
      setLine((p) => (p && next && p.x1 === next.x1 && p.y1 === next.y1 && p.x2 === next.x2 && p.y2 === next.y2 ? p : next));
      raf = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(raf);
  }, [mark]);
  if (!line) return null;
  const dy = Math.max(40, Math.abs(line.y1 - line.y2) / 2) * (line.y2 < line.y1 ? -1 : 1);
  return (
    <svg className="ax-connector" aria-hidden>
      <path d={`M${line.x1} ${line.y1} C${line.x1} ${line.y1 + dy} ${line.x2} ${line.y2 - dy} ${line.x2} ${line.y2}`} />
      <circle cx={line.x1} cy={line.y1} r="3" />
    </svg>
  );
}
