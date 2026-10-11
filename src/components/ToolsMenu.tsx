import { useEffect, useId, useMemo, useRef, useState } from "react";
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
  const [active, setActive] = useState(0);
  const uid = useId();

  const items = useMemo(() => toolItems(doc), [doc.id, doc.name]);
  const sections = useMemo(() => filterTools(items, query), [items, query]);
  const flat = useMemo(() => sections.flatMap((s) => s.items), [sections]);
  const current = flat.length > 0 ? flat[Math.min(active, flat.length - 1)] : null;
  const optionId = (it: ToolItem) => `${uid}-opt-${it.id}`;
  const open = pos != null;

  const close = (refocus = false) => {
    setPos(null);
    setQuery("");
    setActive(0);
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

  // The box keeps focus; the arrows move the highlighted option (aria-activedescendant), Enter runs it.
  const onKey = (e: React.KeyboardEvent) => {
    const n = flat.length;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (n > 0) setActive((i) => (Math.min(i, n - 1) + (e.key === "ArrowDown" ? 1 : -1) + n) % n);
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (current) run(current);
    } else if (e.key === "Tab") {
      // leave the menu with Tab: focus goes back to the button, then moves on from there
      close(true);
    }
  };

  // keep the highlighted option in view
  useEffect(() => {
    if (open && current) document.getElementById(optionId(current))?.scrollIntoView({ block: "nearest" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, current?.id]);

  return (
    <>
      <button
        ref={btn}
        className="ax-toggle tools-btn"
        title="Tools"
        aria-label="Tools"
        aria-haspopup="listbox"
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
          <div
            ref={menu}
            className="ink-menu tools-menu"
            style={pos}
            onClick={(e) => e.stopPropagation()}
            onBlur={(e) => {
              // focus left the menu (not to the Tools button, whose own click toggles): close it
              const to = e.relatedTarget as Node | null;
              if (!menu.current?.contains(to) && to !== btn.current) close();
            }}
          >
            <input
              ref={field}
              className="tools-filter"
              role="combobox"
              aria-expanded
              aria-controls={`${uid}-list`}
              aria-autocomplete="list"
              aria-activedescendant={current ? optionId(current) : undefined}
              value={query}
              placeholder="Find a tool"
              aria-label="Find a tool"
              spellCheck={false}
              onChange={(e) => {
                setQuery(e.target.value);
                setActive(0);
              }}
              onKeyDown={onKey}
            />
            {sections.length === 0 && (
              <div className="tools-none" role="status">
                No tool matches “{query.trim()}”
              </div>
            )}
            <div id={`${uid}-list`} role="listbox" aria-label="Tools">
              {sections.map((s) => (
                <div key={s.id} role="group" aria-labelledby={`${uid}-head-${s.id}`}>
                  <div className="tools-head" id={`${uid}-head-${s.id}`}>
                    {s.title}
                  </div>
                  {s.items.map((it) => (
                    <button
                      key={it.id}
                      id={optionId(it)}
                      role="option"
                      tabIndex={-1}
                      aria-selected={it === current}
                      className={it === current ? "active" : undefined}
                      // keep focus in the box while pointing at a row
                      onMouseDown={(e) => e.preventDefault()}
                      onMouseMove={() => setActive(flat.indexOf(it))}
                      onClick={() => run(it)}
                    >
                      <span className="grow">{it.label}</span>
                    </button>
                  ))}
                </div>
              ))}
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
