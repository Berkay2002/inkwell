import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { openA11y } from "../lib/a11y";
import { useConvert } from "../lib/convertStore";
import { useMarks } from "../lib/marksStore";
import { useProtect } from "../lib/protectStore";
import { type ToolItem, filterTools } from "../lib/toolsMenu";
import { useTools } from "../lib/toolsStore";
import { type DocState, useStore } from "../store";
import { openCompareSetup } from "./history/HistoryPanel";
import { Icon } from "./Icons";

/**
 * Every tool the toolbar offers, as a plain list: add an entry with its `group` (a `TOOL_GROUPS` id in
 * lib/toolsMenu.ts) and it shows up under that header and in the filter. Preflight and the other
 * optimize tools go in the "optimize" group, form tools in their own.
 */
export function toolItems(doc: Pick<DocState, "id" | "name">): ToolItem[] {
  const { id, name } = doc;
  return [
    { id: "compress", group: "optimize", label: "Compress file…", keywords: "reduce size smaller optimize", onSelect: () => useTools.getState().open("compress", id) },
    { id: "ocr", group: "optimize", label: "Recognize text…", keywords: "ocr scan searchable", onSelect: () => useTools.getState().open("ocr", id) },
    { id: "header-footer", group: "marks", label: "Header & footer…", keywords: "page numbers date", onSelect: () => useMarks.getState().open("headerFooter", id) },
    { id: "watermark", group: "marks", label: "Watermark…", keywords: "stamp draft confidential", onSelect: () => useMarks.getState().open("watermark", id) },
    { id: "background", group: "marks", label: "Background…", keywords: "colour color image", onSelect: () => useMarks.getState().open("background", id) },
    { id: "bates", group: "marks", label: "Bates numbering…", keywords: "legal numbers", onSelect: () => useMarks.getState().open("bates", id) },
    { id: "protect", group: "protect", label: "Protect with a password…", keywords: "encrypt lock", onSelect: () => useProtect.getState().open("protect", id) },
    { id: "security", group: "protect", label: "Security…", keywords: "permissions restrictions encryption", onSelect: () => useProtect.getState().open("security", id) },
    { id: "export", group: "convert", label: "Export…", keywords: "save as word excel image text", onSelect: () => useConvert.getState().openExport(id) },
    { id: "create", group: "convert", label: "Create PDF…", keywords: "new from images", onSelect: () => useConvert.getState().openCreate() },
    { id: "compare", group: "check", label: "Compare files…", keywords: "diff differences", onSelect: () => void openCompareSetup({ id, name }) },
    { id: "a11y", group: "check", label: "Check accessibility…", keywords: "tagged ada wcag", onSelect: openA11y },
  ];
}

/** The mode group's "Tools": icon, name and a chevron, opening the grouped, filterable list. */
export function ToolsMenu({ doc }: { doc: DocState }) {
  const btn = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const [query, setQuery] = useState("");

  const items = useMemo(() => toolItems(doc), [doc.id, doc.name]);
  const sections = useMemo(() => filterTools(items, query), [items, query]);
  const open = pos != null;

  const close = (refocus = false) => {
    setPos(null);
    setQuery("");
    if (refocus) btn.current?.focus();
  };

  useEffect(() => {
    if (!open) return;
    field.current?.focus();
    const away = (e: Event) => {
      if (!menu.current?.contains(e.target as Node) && !btn.current?.contains(e.target as Node)) close();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close(true);
      }
    };
    const shut = () => close();
    window.addEventListener("mousedown", away, true);
    window.addEventListener("keydown", key, true);
    window.addEventListener("blur", shut);
    window.addEventListener("resize", shut);
    return () => {
      window.removeEventListener("mousedown", away, true);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("blur", shut);
      window.removeEventListener("resize", shut);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const toggle = () => {
    if (open) return close();
    const r = btn.current!.getBoundingClientRect();
    setPos({ left: r.left, top: r.bottom + 4 });
  };

  const run = (it: ToolItem) => {
    close();
    // the tools' panels live in the reader: leave Organize first, as the right-hand toggles do
    if (doc.organize) useStore.getState().patch(doc.id, { organize: false });
    it.onSelect();
  };

  const rows = () => [...(menu.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
  const onKey = (e: React.KeyboardEvent) => {
    const all = rows();
    const i = all.indexOf(document.activeElement as HTMLElement);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (all.length === 0) return;
      if (i < 0) all[e.key === "ArrowDown" ? 0 : all.length - 1].focus();
      else if (e.key === "ArrowUp" && i === 0) field.current?.focus();
      else all[(i + (e.key === "ArrowDown" ? 1 : -1) + all.length) % all.length].focus();
    } else if (e.key === "Enter" && document.activeElement === field.current) {
      e.preventDefault();
      all[0]?.click();
    } else if (i >= 0 && e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
      // typing while a row has focus goes on in the filter box
      field.current?.focus();
    }
  };

  return (
    <>
      <button
        ref={btn}
        className="ax-toggle tools-btn"
        title="Tools"
        aria-label="Tools"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={(e) => {
          e.stopPropagation();
          toggle();
        }}
      >
        <Icon name="tools" size={15} />
        Tools
        <Icon name="down" size={12} />
      </button>
      {pos &&
        createPortal(
          <div ref={menu} className="ink-menu tools-menu" role="menu" style={pos} onClick={(e) => e.stopPropagation()} onKeyDown={onKey}>
            <input
              ref={field}
              className="tools-filter"
              value={query}
              placeholder="Find a tool"
              aria-label="Find a tool"
              spellCheck={false}
              onChange={(e) => setQuery(e.target.value)}
            />
            {sections.length === 0 && <div className="tools-none">No tool matches “{query.trim()}”</div>}
            {sections.map((s) => (
              <div key={s.id} role="group" aria-label={s.title}>
                <div className="tools-head">{s.title}</div>
                {s.items.map((it) => (
                  <button key={it.id} role="menuitem" onClick={() => run(it)}>
                    <span className="grow">{it.label}</span>
                  </button>
                ))}
              </div>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
