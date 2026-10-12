import { type PointerEvent as RPointerEvent, useEffect, useRef, useState } from "react";
import { Bar } from "./CommandBar";
import { type Geo, MODES, MODE_NAMES } from "../../lib/ai";
import { providerOf, useAi } from "../../lib/aiStore";
import { type DocState, useStore } from "../../store";
import {
  Answer,
  CloseButton,
  Composer,
  Consent,
  Empty,
  Gate,
  Glyph,
  Messages,
  MoreMenu,
  NoText,
  ProviderPicker,
  type Variant,
  useViewerSelection,
} from "./parts";
import { ProviderMark } from "./providerMarks";
import "./assistant.css";

/** One pill in the toolbar: the provider's mark toggles the assistant; while it is open the three layouts slide out
 *  of the pill to its right. The layout picked last is kept in the AI settings and used the next time it opens. */
export function AssistantToggle() {
  const open = useAi((s) => s.open);
  const mode = useAi((s) => s.settings.mode);
  const active = useStore((s) => s.active);
  const provider = useAi((s) => (active != null ? providerOf(s, active) : s.settings.provider));
  const st = useAi.getState;
  return (
    <div className={open ? "ax-toggle-group on" : "ax-toggle-group"}>
      <button
        className="ax-toggle"
        title="Assistant (Ctrl+Shift+A)"
        aria-label="Assistant"
        aria-pressed={open}
        onClick={() => st().toggle()}
      >
        <ProviderMark provider={provider} />
      </button>
      <div className="ax-layouts" role="radiogroup" aria-label="Assistant layout" aria-hidden={!open} inert={!open}>
        {MODES.map((m) => (
          <button
            key={m}
            role="radio"
            aria-checked={m === mode}
            title={`${MODE_NAMES[m]} (Ctrl+Shift+L cycles)`}
            aria-label={MODE_NAMES[m]}
            onClick={() => (m === mode ? st().setOpen(false) : st().setMode(m))}
          >
            <Glyph name={m} size={16} stroke={1.6} />
          </button>
        ))}
      </div>
    </div>
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
  const status = useAi((s) => s.statuses[providerOf(s, doc.id)]);
  const pending = useAi((s) => s.pending?.doc === doc.id);
  const hasMsgs = useAi((s) => (s.chats[doc.id]?.messages.length ?? 0) > 0);
  const ready = typeof status === "object" && status !== null && status.installed && status.loggedIn;
  return (
    <>
      {pending ? (
        <Consent doc={doc.id} />
      ) : !ready ? (
        <Gate doc={doc.id} />
      ) : summary ? (
        <Summary doc={doc} />
      ) : hasMsgs ? (
        <Messages doc={doc} variant={variant} />
      ) : (
        <Empty doc={doc} />
      )}
      {ready && !pending && <Composer doc={doc} variant={variant} onSend={onSend} />}
    </>
  );
}

function Header({ doc }: { doc: DocState }) {
  return (
    <div className="ax-head">
      <span className="ax-title">Assistant</span>
      <ProviderPicker doc={doc.id} />
      <span className="grow" />
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
    <aside className="ax-dock" style={{ width: `min(${width}px, 45vw)` }} aria-label="Assistant">
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
        <ProviderPicker doc={doc.id} />
        <span className="grow" />
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
