import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { commands, prettyKeys } from "../lib/commands";
import { useToolPrefs } from "../lib/toolPrefs";
import { toolItems, runTool } from "../lib/toolItems";
import { type ToolItem, MAX_PINNED, filterTools, pick } from "../lib/toolsCatalog";
import { type DocState, useStore } from "../store";
import { Icon } from "./Icons";
import { ToolTile } from "./ToolTile";
import "./allTools.css";

/** The panel's id, for `aria-controls` on the Tools button. */
const PANEL_ID = "tools-panel";
/** How long a row has to be hovered or focused before its card shows. */
const CARD_DELAY = 600;

/** The tools for `doc`, rebuilt only when the document changes. */
export function useToolItems(doc: Pick<DocState, "id" | "name">): ToolItem[] {
  return useMemo(() => toolItems(doc), [doc.id, doc.name]);
}

const shortcutOf = (it: ToolItem): string | null => {
  const k = it.command ? commands.find((c) => c.id === it.command)?.keys?.[0] : undefined;
  return k ? prettyKeys(k) : null;
};

/** "Tools" in the mode group, after Read | Edit | Organize: opens and closes the All tools panel. */
export function ToolsButton({ doc }: { doc: DocState }) {
  // the panel lives in the reader, so in Organize it is closed whatever was left in the store
  const open = useToolPrefs((s) => s.panelOpen) && !doc.organize;
  return (
    <button
      className="ax-toggle tools-btn"
      title="All tools"
      aria-label="Tools"
      aria-expanded={open}
      aria-controls={PANEL_ID}
      onClick={() => {
        // from Organize the button opens the panel in the reader, as the right-hand toggles do
        if (doc.organize) useStore.getState().patch(doc.id, { organize: false });
        useToolPrefs.getState().setPanel(!open);
      }}
    >
      <Icon name="tools" size={16} filled={open} />
      Tools
      <Icon name={open ? "up" : "down"} size={16} />
    </button>
  );
}

/** The pinned tools as icon buttons next to Tools (at most three). */
export function PinnedTools({ doc }: { doc: DocState }) {
  const pinned = useToolPrefs((s) => s.pinned);
  const all = useToolItems(doc);
  const items = pick(all, pinned);
  // ids that are not tools any more (renamed, removed) go, so they cannot hold a pin slot
  const recent = useToolPrefs((s) => s.recent);
  useEffect(() => useToolPrefs.getState().prune(all.map((i) => i.id)), [all, pinned, recent]);
  if (items.length === 0) return null;
  return (
    <span className="tb-cluster pinned-tools" role="group" aria-label="Pinned tools">
      {items.map((it) => (
        <button
          key={it.id}
          className="icon-btn"
          title={`${it.label}: ${it.description}`}
          aria-label={it.label}
          data-tool={it.id}
          onClick={() => {
            if (doc.organize) useStore.getState().patch(doc.id, { organize: false });
            runTool(it);
          }}
        >
          <ToolTile icon={it.icon} group={it.group} size={24} />
        </button>
      ))}
    </span>
  );
}

/**
 * The left-slot "All tools" panel (a navigation landmark): a search box that also matches synonyms, the last
 * tools used as chips, then every tool under its group with a one-line description, a pin to put it in the
 * toolbar, and a hover card with the fuller sentence and the shortcut.
 */
export function AllToolsPanel({ doc }: { doc: DocState }) {
  const items = useToolItems(doc);
  const recent = useToolPrefs((s) => s.recent);
  const stored = useToolPrefs((s) => s.pinned);
  // what the toolbar shows: the limit and the stars go by these, not by ids that are no longer tools
  const pinned = useMemo(() => pick(items, stored).map((i) => i.id), [items, stored]);
  const [query, setQuery] = useState("");
  const [card, setCard] = useState<{ item: ToolItem; top: number; left: number } | null>(null);
  const root = useRef<HTMLElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const uid = useId();

  const sections = useMemo(() => filterTools(items, query), [items, query]);
  const searching = query.trim() !== "";
  const recents = useMemo(() => pick(items, recent), [items, recent]);

  // Picking a sidebar panel elsewhere (a toolbar button, Ctrl+B, the palette) hands the slot back: that is done
  // in the store's setSidebar / toggleSidebar, so it holds even when the choice does not change.
  useEffect(() => {
    field.current?.focus();
    return () => clearTimeout(timer.current);
  }, []);

  // Whatever closes the panel (Esc, Ctrl+B, a palette command, a picked tool) while focus is inside it: the
  // focus goes back to the Tools button instead of the page body. A rail or dialog that takes the focus
  // after this commit still wins. The layout cleanup runs before the panel's DOM is removed.
  useLayoutEffect(() => {
    const el = root.current;
    return () => {
      if (el?.contains(document.activeElement)) document.querySelector<HTMLElement>('button[aria-label="Tools"]')?.focus();
    };
  }, []);

  const close = () => {
    clearTimeout(timer.current);
    useToolPrefs.getState().setPanel(false);
  };
  const run = (it: ToolItem) => {
    clearTimeout(timer.current);
    setCard(null);
    useToolPrefs.getState().setPanel(false);
    runTool(it);
  };

  const showCardSoon = (it: ToolItem, el: HTMLElement) => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => {
      const r = el.getBoundingClientRect();
      const side = root.current?.getBoundingClientRect().right ?? r.right;
      setCard({ item: it, top: Math.min(r.top, window.innerHeight - 150), left: side + 8 });
    }, CARD_DELAY);
  };
  const hideCard = () => {
    clearTimeout(timer.current);
    setCard(null);
  };

  const rows = () => [...(root.current?.querySelectorAll<HTMLElement>(".tool-main") ?? [])];
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End") return;
    const all = [field.current!, ...rows()];
    const i = all.indexOf(document.activeElement as HTMLElement);
    // Home and End belong to the text box while it has the caret
    if ((e.key === "Home" || e.key === "End") && i === 0) return;
    const next = e.key === "Home" ? 0 : e.key === "End" ? all.length - 1 : Math.min(all.length - 1, Math.max(0, i + (e.key === "ArrowDown" ? 1 : -1)));
    e.preventDefault();
    all[next]?.focus();
  };

  return (
    <aside className="sidebar tools-sidebar">
      <nav id={PANEL_ID} ref={root} className="tools-panel" aria-label="All tools" onKeyDown={onKeyDown}>
        <header className="tp-head">
          <h2>All tools</h2>
          <button className="icon-btn" aria-label="Close all tools" title="Close (Esc)" onClick={close}>
            <Icon name="close" size={14} />
          </button>
        </header>
        <div className="tp-search">
          <Icon name="search" size={15} />
          <input
            ref={field}
            type="search"
            value={query}
            placeholder="Find a tool"
            aria-label="Find a tool"
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const first = sections[0]?.items[0];
                if (first) {
                  e.preventDefault();
                  run(first);
                }
              }
            }}
          />
        </div>
        <div className="tp-scroll" onScroll={hideCard}>
          {!searching && recents.length > 0 && (
            <section aria-labelledby={`${uid}-recent`}>
              <h3 id={`${uid}-recent`}>Recent</h3>
              <div className="tp-chips">
                {recents.map((it) => (
                  <button key={it.id} className="tool-chip" data-tool={it.id} onClick={() => run(it)} title={it.description}>
                    <ToolTile icon={it.icon} group={it.group} size={24} />
                    {it.label}
                  </button>
                ))}
              </div>
            </section>
          )}
          {sections.map((s) => (
            <section key={s.id} aria-labelledby={`${uid}-h-${s.id}`}>
              <h3 id={`${uid}-h-${s.id}`}>{s.title}</h3>
              <ul>
                {s.items.map((it) => {
                  const on = pinned.includes(it.id);
                  const full = !on && pinned.length >= MAX_PINNED;
                  return (
                    <li key={it.id} className="tool-row">
                      <button
                        className="tool-main"
                        data-tool={it.id}
                        aria-describedby={`${uid}-d-${it.id}`}
                        onClick={() => run(it)}
                        onMouseEnter={(e) => showCardSoon(it, e.currentTarget)}
                        onFocus={(e) => showCardSoon(it, e.currentTarget)}
                        onMouseLeave={hideCard}
                        onBlur={hideCard}
                      >
                        <ToolTile icon={it.icon} group={it.group} size={36} />
                        <span className="tool-text">
                          <span className="tool-name">{it.label}</span>
                          <span className="tool-desc" id={`${uid}-d-${it.id}`}>
                            {it.description}
                          </span>
                        </span>
                      </button>
                      <button
                        className={on ? "tool-pin on" : "tool-pin"}
                        aria-pressed={on}
                        aria-label={`Pin ${it.label} to the toolbar`}
                        title={on ? "Unpin from the toolbar" : full ? `${MAX_PINNED} tools are pinned: unpin one first` : "Pin to the toolbar"}
                        disabled={full}
                        onClick={() =>
                          useToolPrefs.getState().togglePin(
                            it.id,
                            items.map((i) => i.id),
                          )
                        }
                      >
                        <Icon name="pin" size={16} filled={on} />
                      </button>
                    </li>
                  );
                })}
              </ul>
            </section>
          ))}
          {sections.length === 0 && (
            <p className="tools-none" role="status">
              No tool matches “{query.trim()}”
            </p>
          )}
        </div>
      </nav>
      {card && (
        <div className="tool-card" role="presentation" aria-hidden style={{ top: card.top, left: card.left }}>
          <ToolTile icon={card.item.icon} group={card.item.group} size={36} />
          <div>
            <strong>{card.item.label}</strong>
            <p>{card.item.detail ?? card.item.description}</p>
            {shortcutOf(card.item) && <kbd>{shortcutOf(card.item)}</kbd>}
          </div>
        </div>
      )}
    </aside>
  );
}
