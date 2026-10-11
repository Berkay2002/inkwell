import { type PointerEvent as RPointerEvent, useEffect, useRef, useState } from "react";
import { Bar } from "./CommandBar";
import { type Geo, MODES, MODE_NAMES } from "../../lib/ai";
import { useAi } from "../../lib/aiStore";
import type { DocState } from "../../store";
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
  type Variant,
  useViewerSelection,
} from "./parts";
import "./assistant.css";

// OpenAI mark from simple-icons (CC0), the assistant runs on ChatGPT through Codex.
const OPENAI =
  "M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z";

/** One pill in the toolbar: the ChatGPT mark toggles the assistant; while it is open the three layouts slide out of
 *  the pill to its right. The layout picked last is kept in the AI settings and used the next time it opens. */
export function AssistantToggle() {
  const open = useAi((s) => s.open);
  const mode = useAi((s) => s.settings.mode);
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
        <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d={OPENAI} />
        </svg>
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
    </>
  );
}

function Header({ doc }: { doc: DocState }) {
  return (
    <div className="ax-head">
      <span className="ax-title">Assistant</span>
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
