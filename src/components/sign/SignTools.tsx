import { useEffect, useRef, useState } from "react";
import { pickSource } from "../../lib/api";
import { useAnnot } from "../../lib/annot";
import { type Saved, type SavedKind, type Tool, banner, signApi, useSign } from "../../lib/sign";
import type { DocState } from "../../store";
import { Icon, type IconName } from "../Icons";
import { savedOf } from "./SignLayer";
import "./sign.css";

const sg = () => useSign.getState();

/** The toolbar's Fill & Sign button. */
export function SignToggle() {
  const open = useSign((s) => s.open);
  return (
    <button className={open ? "icon-btn on" : "icon-btn"} title="Fill & Sign" aria-label="Fill & Sign" aria-pressed={open} onClick={() => sg().setOpen(!open)}>
      <Icon name="signature" />
    </button>
  );
}

const TOOLS: { tool: Tool; icon: IconName; label: string }[] = [
  { tool: "select", icon: "cursor", label: "Select" },
  { tool: "text", icon: "textT", label: "Text" },
  { tool: "cross", icon: "close", label: "Cross" },
  { tool: "check", icon: "check", label: "Check" },
  { tool: "dot", icon: "dot", label: "Dot" },
  { tool: "circle", icon: "roundRect", label: "Rounded box" },
  { tool: "line", icon: "line", label: "Line" },
  { tool: "date", icon: "calendar", label: "Today's date" },
];

/** The floating strip at the left of the page: marks, saved signature and initials, digital signature. */
export function SignStrip({ doc }: { doc: DocState }) {
  const open = useSign((s) => s.open);
  const tool = useSign((s) => s.tool);
  const saved = useSign((s) => s.saved);
  const pick = useSign((s) => s.pick);

  useEffect(() => {
    if (open) {
      void sg().loadSaved();
      void sg().refresh(doc.id);
    }
  }, [open, doc.id, doc.rev]);
  useEffect(() => {
    if (!open) return;
    const k = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || (e.target as HTMLElement)?.closest("input, textarea")) return;
      sg().setTool("select");
      sg().select(doc.id, null);
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [open, doc.id]);

  if (!open) return null;
  const tile = (kind: SavedKind, label: string) => {
    const s = savedOf(saved, pick[kind]);
    return (
      <div className={`sg-tile${tool === kind ? " on" : ""}${s ? "" : " empty"}`}>
        <button
          className="sg-tile-btn"
          title={s ? `Place your ${kind === "signature" ? "signature" : "initials"}` : `Add ${label.toLowerCase()}`}
          aria-label={s ? `Place ${label.toLowerCase()}` : `Add ${label.toLowerCase()}`}
          onClick={() => (s ? sg().setTool(tool === kind ? "select" : kind) : sg().setCreator(kind))}
        >
          {s ? <img src={s.dataUrl} alt="" draggable={false} /> : <span>{kind === "signature" ? "Sign" : "AJ"}</span>}
        </button>
        {s && (
          <button className="sg-tile-x" title={`Change ${label.toLowerCase()}`} aria-label={`Change ${label.toLowerCase()}`} onClick={() => sg().setCreator(kind)}>
            <Icon name="edit" size={11} />
          </button>
        )}
      </div>
    );
  };
  return (
    <div className="an-strip sg-strip" role="toolbar" aria-label="Fill & Sign tools" onMouseDown={(e) => e.preventDefault()}>
      {TOOLS.map((t) => (
        <button
          key={t.tool}
          className={tool === t.tool ? "icon-btn on" : "icon-btn"}
          title={t.label}
          aria-label={t.label}
          aria-pressed={tool === t.tool}
          onClick={() => sg().setTool(t.tool)}
        >
          <Icon name={t.icon} />
        </button>
      ))}
      <span className="sg-div" />
      {tile("signature", "Signature")}
      {tile("initials", "Initials")}
      <span className="sg-div" />
      <button
        className={tool === "digital" ? "icon-btn on" : "icon-btn"}
        title="Digital signature: drag a box on the page"
        aria-label="Digital signature"
        aria-pressed={tool === "digital"}
        onClick={() => {
          sg().setTool(tool === "digital" ? "select" : "digital");
          if (tool !== "digital") sg().setRail("sheet");
        }}
      >
        <Icon name="nib" />
      </button>
    </div>
  );
}

/** Above the page whenever the document is signed: the overall validity, in words. */
export function SignBanner({ doc }: { doc: DocState }) {
  const d = useSign((s) => s.docs[doc.id]);
  const rail = useSign((s) => s.rail);
  useEffect(() => {
    if ((d?.sigsRev ?? -1) !== doc.rev) void sg().refreshSigs(doc.id, doc.rev);
  }, [doc.id, doc.rev, d?.sigsRev]);
  const b = d ? banner(d.sigs) : null;
  if (!b || d?.bannerHidden) return null;
  const icon: IconName = b.tone === "valid" ? "check" : b.tone === "invalid" ? "close" : "question";
  return (
    <div className={`sg-banner ${b.tone}`} role="status">
      <span className="sg-banner-ico">
        <Icon name={icon} size={15} />
      </span>
      <span className="sg-banner-text">{b.text}</span>
      {rail !== "list" && (
        <button className="sg-link" onClick={() => sg().setRail("list")}>
          Details
        </button>
      )}
      <button className="sg-banner-x" title="Hide" aria-label="Hide" onClick={() => sg().hideBanner(doc.id)}>
        <Icon name="close" size={13} />
      </button>
    </div>
  );
}

// ---- the signature creator ----

const FONTS = ["Segoe Script", "Ink Free", "Lucida Handwriting", "Gabriola"];
const INK_CSS = "#1F2A44";

const initialsOf = (name: string) =>
  name
    .split(/\s+/)
    .filter(Boolean)
    .map((w) => w[0]!.toUpperCase())
    .join("")
    .slice(0, 3);

/** A typed signature as a trimmed PNG (base64, no prefix), drawn at 3x for print. */
async function typedPng(text: string, font: string): Promise<string> {
  const px = 120;
  const spec = `${px}px "${font}"`;
  await document.fonts.load(spec).catch(() => {});
  const c = document.createElement("canvas");
  const ctx = c.getContext("2d")!;
  ctx.font = spec;
  const m = ctx.measureText(text);
  const pad = 16;
  c.width = Math.ceil(m.actualBoundingBoxLeft + m.actualBoundingBoxRight) + pad * 2;
  c.height = Math.ceil(m.actualBoundingBoxAscent + m.actualBoundingBoxDescent) + pad * 2;
  ctx.font = spec;
  ctx.fillStyle = INK_CSS;
  ctx.fillText(text, pad + m.actualBoundingBoxLeft, pad + m.actualBoundingBoxAscent);
  return c.toDataURL("image/png").split(",")[1]!;
}

type Stroke = [number, number][];

/** Drawn strokes as a PNG cropped to the ink (base64, no prefix). */
function drawnPng(strokes: Stroke[], scale = 3): string {
  const pts = strokes.flat();
  const xs = pts.map((p) => p[0]);
  const ys = pts.map((p) => p[1]);
  const pad = 6;
  const [x0, y0] = [Math.min(...xs) - pad, Math.min(...ys) - pad];
  const [w, h] = [Math.max(...xs) + pad - x0, Math.max(...ys) + pad - y0];
  const c = document.createElement("canvas");
  c.width = Math.ceil(w * scale);
  c.height = Math.ceil(h * scale);
  const ctx = c.getContext("2d")!;
  ctx.scale(scale, scale);
  ctx.translate(-x0, -y0);
  paint(ctx, strokes);
  return c.toDataURL("image/png").split(",")[1]!;
}

function paint(ctx: CanvasRenderingContext2D, strokes: Stroke[]) {
  ctx.strokeStyle = INK_CSS;
  ctx.lineWidth = 2.6;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const s of strokes) {
    ctx.beginPath();
    s.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    if (s.length === 1) ctx.lineTo(s[0]![0] + 0.1, s[0]![1]);
    ctx.stroke();
  }
}

/** Type, draw or pick a picture of a signature or initials; it is saved (encrypted) for reuse. */
export function SignCreator() {
  const kind = useSign((s) => s.creator);
  const author = useAnnot((s) => s.author.trim() || s.defaultAuthor);
  const [tab, setTab] = useState<"type" | "draw" | "image">("type");
  const [text, setText] = useState("");
  const [font, setFont] = useState(FONTS[0]!);
  const [strokes, setStrokes] = useState<Stroke[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    if (!kind) return;
    setText(kind === "initials" ? initialsOf(author) : author);
    setStrokes([]);
    setErr(null);
    setTab("type");
  }, [kind, author]);
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    c.width = c.clientWidth * dpr;
    c.height = c.clientHeight * dpr;
    const ctx = c.getContext("2d")!;
    ctx.scale(dpr, dpr);
    paint(ctx, strokes);
  }, [strokes, tab]);

  if (!kind) return null;
  const label = kind === "signature" ? "signature" : "initials";
  const close = () => sg().setCreator(null);
  const finish = async (f: () => Promise<Saved | null>) => {
    setBusy(true);
    setErr(null);
    try {
      const s = await f();
      if (s) {
        sg().addSaved(s);
        sg().setTool(kind);
        close();
      }
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };
  const save = () =>
    finish(async () =>
      tab === "type"
        ? signApi.saveDrawn(kind, "typed", font, await typedPng(text.trim(), font))
        : signApi.saveDrawn(kind, "drawn", null, drawnPng(strokes)),
    );
  const fromFile = () =>
    finish(async () => {
      const [f] = await pickSource("images", false);
      return f ? signApi.saveFile(kind, f.token) : null;
    });
  const at = (e: React.PointerEvent<HTMLCanvasElement>): [number, number] => {
    const r = e.currentTarget.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  };
  const ready = tab === "type" ? text.trim().length > 0 : tab === "draw" ? strokes.length > 0 : false;

  return (
    <div className="sg-modal-wrap" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div className="sg-modal" role="dialog" aria-label={`Create ${label}`} onKeyDown={(e) => e.key === "Escape" && close()}>
        <div className="sg-modal-head">
          <span className="sg-modal-title">{kind === "signature" ? "Your signature" : "Your initials"}</span>
          <span className="grow" />
          <button className="ax-icon" aria-label="Close" onClick={close}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="sg-tabs" role="tablist">
          {(["type", "draw", "image"] as const).map((t) => (
            <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? "sg-tab on" : "sg-tab"} onClick={() => setTab(t)}>
              {t === "type" ? "Type" : t === "draw" ? "Draw" : "Image"}
            </button>
          ))}
        </div>
        {tab === "type" && (
          <div className="sg-type">
            <input className="cm-input" value={text} maxLength={60} aria-label={`Your ${label}`} autoFocus onChange={(e) => setText(e.target.value)} />
            <div className="sg-fonts" role="radiogroup" aria-label="Style">
              {FONTS.map((f) => (
                <button key={f} role="radio" aria-checked={font === f} className={font === f ? "sg-font on" : "sg-font"} style={{ fontFamily: `"${f}"` }} onClick={() => setFont(f)}>
                  {text.trim() || (kind === "signature" ? "Your name" : "AJ")}
                </button>
              ))}
            </div>
          </div>
        )}
        {tab === "draw" && (
          <div className="sg-draw">
            <canvas
              ref={canvas}
              className="sg-pad"
              aria-label={`Draw your ${label}`}
              onPointerDown={(e) => {
                e.currentTarget.setPointerCapture(e.pointerId);
                setStrokes([...strokes, [at(e)]]);
              }}
              onPointerMove={(e) => {
                if (e.buttons !== 1 || strokes.length === 0) return;
                const last = strokes[strokes.length - 1]!;
                setStrokes([...strokes.slice(0, -1), [...last, at(e)]]);
              }}
            />
            <div className="sg-pad-line" />
            <button className="sg-link" disabled={strokes.length === 0} onClick={() => setStrokes([])}>
              Clear
            </button>
          </div>
        )}
        {tab === "image" && (
          <div className="sg-image">
            <p className="dim">A PNG or JPEG of your {label}. A photo of paper has its white made transparent.</p>
            <button className="cm-btn" disabled={busy} onClick={() => void fromFile()}>
              Choose an image…
            </button>
          </div>
        )}
        <p className="dim sg-note">Kept on this PC only, encrypted. Placing it adds a picture, not a digital signature.</p>
        {err && (
          <p className="cm-error" role="alert">
            {err}
          </p>
        )}
        <div className="sg-modal-foot">
          <button className="cm-btn" onClick={close}>
            Cancel
          </button>
          {tab !== "image" && (
            <button className="cm-btn primary" disabled={!ready || busy} onClick={() => void save()}>
              Save and place
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
