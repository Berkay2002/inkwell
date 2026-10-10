import type { ReactNode } from "react";

/** The small markdown subset the model answers in: paragraphs, bullets, headings, **bold**, `code`
 *  and [n] citation markers. Everything is built as React nodes, never HTML, so a hostile
 *  document cannot inject markup through the answer. */
type Block = { t: "p"; text: string } | { t: "h"; text: string } | { t: "ul"; items: string[] } | { t: "ol"; items: string[] };

function blocks(src: string): Block[] {
  const out: Block[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length) out.push({ t: "p", text: para.join(" ") });
    para = [];
  };
  for (const raw of src.split("\n")) {
    const line = raw.trim();
    const li = /^(?:([-*•])|(\d+)[.)])\s+(.*)$/.exec(line);
    const h = /^#{1,6}\s+(.*)$/.exec(line);
    if (!line) flush();
    else if (h) {
      flush();
      out.push({ t: "h", text: h[1] });
    } else if (li) {
      flush();
      const kind = li[1] ? "ul" : "ol";
      const last = out[out.length - 1];
      if (last && last.t === kind) last.items.push(li[3]);
      else out.push({ t: kind, items: [li[3]] });
    } else para.push(line);
  }
  flush();
  return out;
}

const TOKEN = /(\*\*[^*]+\*\*|`[^`]+`|\[\d+(?:\s*[,;]\s*\d+)*\])/;

export interface CiteProps {
  /** Citation numbers that can be clicked (the answer is complete). */
  known: Set<number>;
  active: number | null;
  onCite: (n: number) => void;
}

function inline(text: string, c: CiteProps): ReactNode[] {
  return text.split(TOKEN).map((part, i) => {
    if (part.startsWith("**") && part.length > 4) return <strong key={i}>{part.slice(2, -2)}</strong>;
    if (part.startsWith("`") && part.length > 2) return <code key={i}>{part.slice(1, -1)}</code>;
    if (part.startsWith("[") && /^\[\d/.test(part)) {
      const nums = part.slice(1, -1).split(/[,;]/).map((n) => Number(n.trim()));
      return (
        <span key={i} className="ax-cite-group">
          {nums.map((n) =>
            c.known.has(n) ? (
              <button
                key={n}
                className={c.active === n ? "ax-cite on" : "ax-cite"}
                title="Show in document"
                onClick={() => c.onCite(n)}
              >
                {n}
              </button>
            ) : (
              <span key={n} className="ax-cite off">
                {n}
              </span>
            ),
          )}
        </span>
      );
    }
    return part;
  });
}

export function Markdown({ text, cites }: { text: string; cites: CiteProps }) {
  return (
    <>
      {blocks(text).map((b, i) => {
        if (b.t === "p") return <p key={i}>{inline(b.text, cites)}</p>;
        if (b.t === "h") return <h4 key={i}>{inline(b.text, cites)}</h4>;
        const L = b.t;
        return (
          <L key={i}>
            {b.items.map((it, j) => (
              <li key={j}>{inline(it, cites)}</li>
            ))}
          </L>
        );
      })}
    </>
  );
}
