import { type ReactNode, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Icon } from "./Icons";

export interface MenuItem {
  label: string;
  onSelect: () => void;
  checked?: boolean;
  danger?: boolean;
  hint?: string;
  /** Why the item cannot be used now: it is shown disabled with this as its tooltip. */
  disabled?: string;
}
export type MenuEntry = MenuItem | "sep";

const ITEM_H = 32;

/** A button that opens a small popup menu. The popup is portalled and fixed, so scrolling lists never clip it. */
export function MenuButton({ items, label, className = "icon-btn", children }: { items: MenuEntry[]; label: string; className?: string; children: ReactNode }) {
  const btn = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ right: number; top?: number; bottom?: number } | null>(null);

  useEffect(() => {
    if (!pos) return;
    menu.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
    const close = () => setPos(null);
    const away = (e: Event) => {
      if (!menu.current?.contains(e.target as Node)) close();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close();
        btn.current?.focus();
      }
    };
    window.addEventListener("mousedown", away, true);
    window.addEventListener("keydown", key, true);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    document.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("mousedown", away, true);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
      document.removeEventListener("scroll", close, true);
    };
  }, [pos]);

  const toggle = () => {
    if (pos) return setPos(null);
    const r = btn.current!.getBoundingClientRect();
    const h = items.length * ITEM_H + 16;
    const right = window.innerWidth - r.right;
    setPos(r.bottom + 4 + h > window.innerHeight ? { right, bottom: window.innerHeight - r.top + 4 } : { right, top: r.bottom + 4 });
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const all = [...menu.current!.querySelectorAll<HTMLElement>("button:not(:disabled)")];
    const i = all.indexOf(document.activeElement as HTMLElement);
    all[(i + (e.key === "ArrowDown" ? 1 : -1) + all.length) % all.length]?.focus();
    e.preventDefault();
  };

  return (
    <>
      <button
        ref={btn}
        className={className}
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={!!pos}
        onClick={(e) => {
          e.stopPropagation();
          toggle();
        }}
      >
        {children}
      </button>
      {pos &&
        createPortal(
          // React events bubble through portals to the row that owns the button: keep clicks inside the menu
          <div ref={menu} className="ink-menu" role="menu" style={pos} onClick={(e) => e.stopPropagation()} onKeyDown={onKey}>
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
                    setPos(null);
                    it.onSelect();
                  }}
                >
                  <span className="check">{it.checked && <Icon name="check" size={14} />}</span>
                  <span className="grow">{it.label}</span>
                  {it.hint && <span className="dim">{it.hint}</span>}
                </button>
              ),
            )}
          </div>,
          document.body,
        )}
    </>
  );
}
