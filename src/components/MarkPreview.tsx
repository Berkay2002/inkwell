import { useMemo } from "react";
import type { MarkKindInfo, PageMark } from "../lib/api";
import { useStore } from "../store";
import { canvasMeasure, CSS_FONT, markOf, type MarksSheet, type PreviewShape, previewShapes, todayDate, toHex, useMarks } from "../lib/marksStore";

type Size = [number, number];

/** The same value whatever the order of its keys, for telling the file's mark from the sheet's. */
function canon(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canon).join(",")}]`;
  if (v && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .filter((k) => o[k] !== undefined)
      .sort()
      .map((k) => `${k}:${canon(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}

/** Pictures are held by a token in the sheet and by a path in the file: compared without them. */
const bare = (m: PageMark) => ({ ...m, picture: undefined });

/** The sheet's mark is what the page already has: the file draws it, so nothing is laid over it. */
function drawnAlready(sheet: MarksSheet, have: MarkKindInfo | undefined, page: number): boolean {
  if (!have || !have.mark || !have.pages.includes(page)) return false;
  return canon(bare(markOf(sheet))) === canon(bare(have.mark));
}

/**
 * The open Page marks sheet drawn over page `page` as the engine will write it: SVG in page points
 * (it lives inside the page space, so zoom and rotation apply), outlined so it reads as a preview.
 */
export function MarkPreview({ docId, page, size }: { docId: number; page: number; size: Size }) {
  const count = useStore((s) => s.docs[docId]?.pageCount ?? 0);
  const sheet = useMarks((s) => (s.sheet && s.sheet.doc === docId ? s.sheet : null));
  const have = useMarks((s) => (sheet ? s.info[docId]?.find((k) => k.kind === sheet.kind) : undefined));
  const date = useMemo(todayDate, []);
  const shapes = useMemo(() => {
    if (!sheet || sheet.busy) return [];
    if (sheet.kind === "bates" && sheet.btFiles.length > 0) return [];
    if (drawnAlready(sheet, have, page)) return [];
    return previewShapes(sheet, page, count, size, canvasMeasure, date);
  }, [sheet, have, page, count, size, date]);
  if (shapes.length === 0) return null;
  return (
    <svg className="mk-preview" width={size[0]} height={size[1]} viewBox={`0 0 ${size[0]} ${size[1]}`} aria-hidden>
      {shapes.map((s, i) => (
        <Shape key={i} s={s} />
      ))}
    </svg>
  );
}

function Shape({ s }: { s: PreviewShape }) {
  switch (s.kind) {
    case "text": {
      return (
        <g opacity={s.opacity}>
          <text
            x={s.x}
            y={s.y}
            fontFamily={CSS_FONT[s.family]}
            fontSize={s.size}
            fontWeight={s.bold ? 700 : 400}
            fontStyle={s.italic ? "italic" : "normal"}
            textDecoration={s.underline ? "underline" : undefined}
            textAnchor={s.anchor}
            fill={toHex(s.color)}
            style={{ whiteSpace: "pre" }}
          >
            {s.text}
          </text>
          {s.anchor === "start" && (
            <rect
              className="mk-slot"
              x={s.x - 2}
              y={s.y - s.size * 0.85}
              width={s.width + 4}
              height={s.size * 1.1}
              rx={2}
            />
          )}
        </g>
      );
    }
    case "group":
      return (
        <g transform={`translate(${s.cx} ${s.cy}) rotate(${-s.rotation})`} opacity={s.opacity}>
          {s.shapes.map((c, i) => (
            <Shape key={i} s={c} />
          ))}
        </g>
      );
    case "rect":
      return <rect x={s.x} y={s.y} width={s.w} height={s.h} fill={toHex(s.color)} opacity={s.opacity} style={s.blend ? { mixBlendMode: "multiply" } : undefined} />;
    case "picture":
      return (
        <g transform={`translate(${s.cx} ${s.cy}) rotate(${-s.rotation})`} opacity={Math.max(s.opacity, 0.35)}>
          <rect className="mk-slot mk-pic" x={-s.w / 2} y={-s.h / 2} width={s.w} height={s.h} rx={4} />
          <text className="mk-pic-name" x={0} y={0} textAnchor="middle" dominantBaseline="middle" fontSize={Math.max(10, Math.min(s.w / 12, 28))}>
            {s.name}
          </text>
        </g>
      );
  }
}
