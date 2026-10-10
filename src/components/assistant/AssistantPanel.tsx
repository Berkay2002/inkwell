import { type PointerEvent as RPointerEvent, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Geo } from "../../lib/ai";
import { useAi } from "../../lib/aiStore";
import type { DocState } from "../../store";
import {
  Answer,
  CloseButton,
  Composer,
  Consent,
  Empty,
  FOOTNOTE,
  Gate,
  Messages,
  ModelPicker,
  MoreMenu,
  NoText,
  type Variant,
  startersFor,
  useViewerSelection,
} from "./parts";
import "./assistant.css";

/** A plain text button in the toolbar (no icon: the panel is called what it is). */
export function AssistantToggle() {
  const open = useAi((s) => s.open);
  return (
    <button
      className={open ? "ax-toggle on" : "ax-toggle"}
      title="Assistant (Ctrl+Shift+A)"
      aria-pressed={open}
      onClick={() => useAi.getState().toggle()}
    >
      Assistant
    </button>
  );
}

export function AssistantPanel({ doc }: { doc: DocState }) {
  const open = useAi((s) => s.open);
  const mode = useAi((s) => s.settings.mode);
  useViewerSelection(doc.id);

  useEffect(() => {
    if (open) useAi.getState().prepare(doc.id);
  }, [open, doc.id]);

  if (!open) return null;
  if (mode === "floating") return <Floating doc={doc} />;
  if (mode === "bar") return <Bar doc={doc} />;
  return <Docked doc={doc} />;
}

/** What the panel shows below its header: consent, sign-in, the chat or the starters, then the composer. */
function Content({ doc, variant, onSend, summary = false }: { doc: DocState; variant: Variant; onSend?: () => void; summary?: boolean }) {
  const status = useAi((s) => s.status);
  const pending = useAi((s) => s.pending?.doc === doc.id);
  const hasMsgs = useAi((s) => (s.chats[doc.id]?.messages.length ?? 0) > 0);
  const ready = typeof status === "object" && status !== null && status.installed && status.loggedIn;
  return (
    <>
      {pending ? (
        <Consent />
      ) : !ready ? (
        <Gate />
      ) : summary ? (
        <Summary doc={doc} />
      ) : hasMsgs ? (
        <Messages doc={doc} variant={variant} />
      ) : (
        <Empty doc={doc} />
      )}
      {ready && !pending && <Composer doc={doc} variant={variant} onSend={onSend} />}
      <div className="ax-foot">{FOOTNOTE}</div>
    </>
  );
}

function Header({ doc }: { doc: DocState }) {
  return (
    <div className="ax-head">
      <span className="ax-title">Assistant</span>
      <span className="grow" />
      <ModelPicker />
      <MoreMenu doc={doc} />
      <CloseButton />
    </div>
  );
}

// ---- docked ------------------------------------------------------------------------------------

function Docked({ doc }: { doc: DocState }) {
  const width = useAi((s) => s.settings.width);
  const drag = useRef<{ x: number; w: number } | null>(null);
  return (
    <aside className="ax-dock" style={{ width }} aria-label="Assistant">
      <div
        className="ax-grip"
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
      <section className="ax-panel docked">
        <Header doc={doc} />
        <Content doc={doc} variant="docked" />
      </section>
    </aside>
  );
}

// ---- floating ----------------------------------------------------------------------------------

const MIN_W = 320;
const MIN_H = 300;

function clampGeo(g: Geo, iw: number, ih: number): Geo {
  const w = Math.round(Math.min(Math.max(g.w, MIN_W), Math.max(MIN_W, iw - 16)));
  const h = Math.round(Math.min(Math.max(g.h, MIN_H), Math.max(MIN_H, ih - 16)));
  return {
    w,
    h,
    x: Math.round(Math.min(Math.max(g.x, 8), Math.max(8, iw - w - 8))),
    y: Math.round(Math.min(Math.max(g.y, 8), Math.max(8, ih - h - 8))),
  };
}

function defaultGeo(iw: number, ih: number): Geo {
  const w = 380;
  const h = Math.min(640, ih - 120);
  return { w, h, x: iw - w - 24, y: 92 };
}

type Grab = "move" | "sw" | "se";

function Floating({ doc }: { doc: DocState }) {
  const saved = useAi((s) => s.settings.float);
  const [, tick] = useState(0);
  const [tab, setTab] = useState<"chat" | "summary">("chat");
  const grab = useRef<{ kind: Grab; px: number; py: number; g: Geo } | null>(null);
  const iw = window.innerWidth;
  const ih = window.innerHeight;
  const g = clampGeo(saved ?? defaultGeo(iw, ih), iw, ih);

  useEffect(() => {
    const f = () => tick((n) => n + 1);
    window.addEventListener("resize", f);
    return () => window.removeEventListener("resize", f);
  }, []);

  const start = (kind: Grab) => (e: RPointerEvent<HTMLElement>) => {
    if (e.button !== 0 || (e.target as Element).closest("button, select, input, textarea")) return;
    grab.current = { kind, px: e.clientX, py: e.clientY, g };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const move = (e: RPointerEvent<HTMLElement>) => {
    const s = grab.current;
    if (!s) return;
    const dx = e.clientX - s.px;
    const dy = e.clientY - s.py;
    let n: Geo = s.g;
    if (s.kind === "move") n = { ...s.g, x: s.g.x + dx, y: s.g.y + dy };
    else if (s.kind === "se") n = { ...s.g, w: s.g.w + dx, h: s.g.h + dy };
    else {
      // south-west: the right edge stays where it is
      const w = Math.max(MIN_W, s.g.w - dx);
      n = { ...s.g, x: s.g.x + s.g.w - w, w, h: s.g.h + dy };
    }
    useAi.setState((st) => ({ settings: { ...st.settings, float: clampGeo(n, window.innerWidth, window.innerHeight) } }));
  };
  const end = () => {
    if (!grab.current) return;
    grab.current = null;
    useAi.getState().setSettings({});
  };
  const events = (kind: Grab) => ({ onPointerDown: start(kind), onPointerMove: move, onPointerUp: end, onPointerCancel: end });

  return (
    <section className="ax-panel floating" style={{ left: g.x, top: g.y, width: g.w, height: g.h }} aria-label="Assistant">
      <div className="ax-head drag" {...events("move")}>
        <span className="ax-title">Assistant</span>
        <span className="grow" />
        <ModelPicker />
        <MoreMenu doc={doc} />
        <CloseButton />
      </div>
      <div className="ax-seg" role="tablist">
        {(["chat", "summary"] as const).map((t) => (
          <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? "on" : ""} onClick={() => setTab(t)}>
            {t === "chat" ? "Chat" : "Summary"}
          </button>
        ))}
      </div>
      <Content doc={doc} variant="floating" summary={tab === "summary"} onSend={() => setTab("chat")} />
      <span className="ax-rz sw" {...events("sw")} />
      <span className="ax-rz se" {...events("se")} />
    </section>
  );
}

/** The floating layout's Summary tab: the document summary, asked for on demand. */
function Summary({ doc }: { doc: DocState }) {
  const all = useAi((s) => s.chats[doc.id]?.messages);
  const prep = useAi((s) => s.chats[doc.id]?.prep);
  let bot;
  for (let i = (all?.length ?? 0) - 2; i >= 0; i--) {
    if (all![i].role === "user" && all![i].kind === "summarizeDocument") {
      bot = all![i + 1];
      break;
    }
  }
  if (bot)
    return (
      <div className="ax-body">
        <Answer doc={doc} m={bot} variant="floating" isLast={false} plain />
      </div>
    );
  if (typeof prep === "object" && prep !== null && !prep.enough)
    return (
      <div className="ax-body">
        <NoText />
      </div>
    );
  return (
    <div className="ax-body">
      <div className="ax-card">
        <h3>Summary of “{doc.name}”</h3>
        <p>A short overview of the whole document, with page references.</p>
        <button className="ax-btn primary" onClick={() => useAi.getState().ask(doc.id, { kind: "summarizeDocument" })}>
          Summarize this document
        </button>
      </div>
    </div>
  );
}

// ---- command bar -------------------------------------------------------------------------------

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
      const b = el.getBoundingClientRect();
      setBox((p) => (p && p.l === b.left && p.r === b.right && p.t === b.top && p.b === b.bottom ? p : { l: b.left, r: b.right, t: b.top, b: b.bottom }));
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

const BAR_H = 52;
const GAP = 14;

function Bar({ doc }: { doc: DocState }) {
  const box = useStageBox();
  const status = useAi((s) => s.status);
  const sheet = useAi((s) => s.sheet);
  const pending = useAi((s) => s.pending?.doc === doc.id);
  const messages = useAi((s) => s.chats[doc.id]?.messages);
  const selection = useAi((s) => s.selection !== null);
  const prep = useAi((s) => s.chats[doc.id]?.prep);
  const ready = typeof status === "object" && status !== null && status.installed && status.loggedIn;
  const last = messages && messages.length >= 2 ? messages[messages.length - 1] : undefined;
  const forced = pending || !ready;
  const showSheet = forced || (sheet && !!messages && messages.length > 0);
  const noText = typeof prep === "object" && prep !== null && !prep.enough;
  if (!box) return null;

  const iw = window.innerWidth;
  const ih = window.innerHeight;
  const barBottom = ih - box.b + 18;
  const stripH = selection ? 50 : 0;
  const sheetBottom = barBottom + (ready && !pending ? BAR_H + GAP + stripH : 0);
  const stageW = box.r - box.l;
  const st = useAi.getState;

  return (
    <>
      {showSheet && (
        <section
          className="ax-panel ax-bar-sheet"
          style={{
            right: iw - box.r + 16,
            bottom: sheetBottom,
            width: Math.min(420, Math.max(300, stageW - 32)),
            maxHeight: Math.max(220, box.b - box.t - (sheetBottom - (ih - box.b)) - 16),
          }}
          aria-label="Assistant"
        >
          <div className="ax-head">
            <span className="ax-title">Assistant</span>
            <span className="grow" />
            {last && last.state === "done" && (
              <button className="ax-btn small" onClick={() => void navigator.clipboard.writeText(last.text)}>
                Copy
              </button>
            )}
            <button className="ax-btn small" disabled={!messages?.length} onClick={() => st().clear(doc.id)}>
              New chat
            </button>
            <CloseButton onClick={forced ? undefined : () => st().setSheet(false)} />
          </div>
          {pending ? <Consent /> : !ready ? <Gate /> : <Messages doc={doc} variant="bar" lastOnly />}
          <div className="ax-foot">{FOOTNOTE}</div>
        </section>
      )}
      {ready && !pending && (
        <div className="ax-bar" style={{ left: (box.l + box.r) / 2, bottom: barBottom, width: Math.min(580, Math.max(280, stageW - 32)) }}>
          {!messages?.length && !noText && (
            <div className="ax-bar-starters">
              {startersFor(doc).map((s) => (
                <button key={s.title} className="ax-chip" onClick={s.run}>
                  {s.title}
                </button>
              ))}
            </div>
          )}
          <Composer
            doc={doc}
            variant="bar"
            trail={
              <>
                <MoreMenu doc={doc} up />
                <ModelPicker up />
              </>
            }
          />
        </div>
      )}
      <Connector />
    </>
  );
}

/** A thin curve from the source row in the sheet to its badge on the page. */
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
      const src = document.querySelector<HTMLElement>(`.ax-bar-sheet [data-source="${mark.n}"]`);
      const badge = document.querySelector<HTMLElement>(".cite-badge");
      const stage = document.querySelector(".stage")?.getBoundingClientRect();
      let next = null;
      if (src && badge && stage) {
        const a = src.getBoundingClientRect();
        const b = badge.getBoundingClientRect();
        const x2 = b.left + b.width / 2;
        const y2 = b.top + b.height / 2;
        if (x2 > stage.left && x2 < stage.right && y2 > stage.top && y2 < stage.bottom)
          next = { x1: Math.round(a.left), y1: Math.round(a.top + a.height / 2), x2: Math.round(x2), y2: Math.round(y2) };
      }
      setLine((p) => (p && next && p.x1 === next.x1 && p.y1 === next.y1 && p.x2 === next.x2 && p.y2 === next.y2 ? p : next));
      raf = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(raf);
  }, [mark]);
  if (!line) return null;
  const dx = Math.max(40, Math.abs(line.x1 - line.x2) / 2);
  return (
    <svg className="ax-connector" aria-hidden>
      <path d={`M${line.x1} ${line.y1} C${line.x1 - dx} ${line.y1} ${line.x2 + dx} ${line.y2} ${line.x2} ${line.y2}`} />
      <circle cx={line.x1} cy={line.y1} r="3" />
    </svg>
  );
}
