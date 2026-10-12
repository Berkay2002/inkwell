import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import * as edits from "../lib/edit";
import { useSheets } from "../lib/organizePlus";
import { formatRange } from "../lib/pageRange";
import { useTools } from "../lib/toolsStore";
import type { DocState } from "../store";
import type { MenuEntry } from "./Menu";

/** The page actions offered on a thumbnail (sidebar or Organize grid) for `pages` (0-based, sorted). */
export function pageActions(
  doc: DocState,
  pages: number[],
  own: { rotate?: (quarterTurns: number) => void; remove?: () => void } = {},
): MenuEntry[] {
  const { id } = doc;
  const last = pages[pages.length - 1];
  const rotate = own.rotate ?? ((quarterTurns: number) => void edits.edit(id, { op: "rotate", pages, quarterTurns }));
  return [
    { label: "Rotate right", onSelect: () => rotate(1) },
    { label: "Rotate left", onSelect: () => rotate(-1) },
    "sep",
    { label: "Insert blank page after", onSelect: () => void edits.edit(id, { op: "insertBlank", at: last + 1, like: last }) },
    { label: "Insert pages from file…", onSelect: () => void edits.insertFile(id, last + 1) },
    { label: pages.length > 1 ? `Extract pages ${formatRange(pages)}…` : "Extract page…", onSelect: () => void edits.extract(id, pages) },
    {
      label: pages.length > 1 ? `Recognize text on pages ${formatRange(pages)}…` : "Recognize text on this page…",
      onSelect: () => useTools.getState().open("ocr", id, { scope: "range", range: formatRange(pages) }),
    },
    { label: "Page labels…", onSelect: () => useSheets.getState().openLabels(id, pages[0], last) },
    {
      label: pages.length > 1 ? `Transitions for pages ${formatRange(pages)}…` : "Page transition…",
      onSelect: () => useSheets.getState().openTrans(id, pages),
    },
    "sep",
    {
      label: pages.length > 1 ? `Delete ${pages.length} pages` : "Delete page",
      hint: "Del",
      danger: true,
      onSelect: own.remove ?? (() => pages.length < doc.pageCount && void edits.edit(id, { op: "delete", pages })),
    },
  ];
}

/** A menu at the pointer (right click). Closes on any outside click, Escape, scroll or resize. */
export function ContextMenu({ x, y, items, onClose }: { x: number; y: number; items: MenuEntry[]; onClose: () => void }) {
  const el = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: x, top: y });

  useLayoutEffect(() => {
    const r = el.current!.getBoundingClientRect();
    setPos({ left: Math.min(x, window.innerWidth - r.width - 8), top: Math.min(y, window.innerHeight - r.height - 8) });
    el.current!.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
  }, [x, y]);

  useEffect(() => {
    const away = (e: Event) => !el.current?.contains(e.target as Node) && onClose();
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        const all = [...el.current!.querySelectorAll<HTMLElement>("button:not(:disabled)")];
        const i = all.indexOf(document.activeElement as HTMLElement);
        all[(i + (e.key === "ArrowDown" ? 1 : -1) + all.length) % all.length]?.focus();
        e.preventDefault();
      }
    };
    window.addEventListener("mousedown", away, true);
    window.addEventListener("keydown", key, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    document.addEventListener("scroll", onClose, true);
    return () => {
      window.removeEventListener("mousedown", away, true);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
      document.removeEventListener("scroll", onClose, true);
    };
  }, [onClose]);

  return createPortal(
    <div ref={el} className="ink-menu" role="menu" style={pos} onClick={(e) => e.stopPropagation()}>
      {items.map((it, n) =>
        it === "sep" ? (
          <hr key={n} />
        ) : (
          <button
            key={n}
            role="menuitem"
            className={it.danger ? "danger" : undefined}
            disabled={!!it.disabled}
            title={it.disabled}
            onClick={() => {
              onClose();
              it.onSelect();
            }}
          >
            <span className="check" />
            <span className="grow">{it.label}</span>
            {it.hint && <span className="dim">{it.hint}</span>}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}
