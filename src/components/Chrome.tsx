import { useEffect, useRef, useState } from "react";
import { openDialog, prettyKeys, commands } from "../lib/commands";
import { redo, requestClose, undo } from "../lib/edit";
import { ZOOM_STEPS, stepZoom } from "../lib/layout";
import { useProtect } from "../lib/protectStore";
import { pageOfLabel, useSheets } from "../lib/organizePlus";
import { pageBoxCh } from "../lib/pageBox";
import { type PageDark, togglePageDark } from "../lib/pageDark";
import { usePagesDark, useSystemDark } from "../lib/usePagesDark";
import { type DocState, pageLabel, type Theme, useStore } from "../store";
import { AssistantToggle } from "./assistant/AssistantPanel";
import { HistoryToggle } from "./history/HistoryPanel";
import { Icon, type IconName } from "./Icons";
import { MenuButton } from "./Menu";
import { CommentsToggle } from "./comments/CommentsPanel";
import { FormsToggle } from "./forms/FormsPanel";
import { RedactToggle } from "./redact/RedactPanel";
import { ModeSwitch } from "./Organize";
import { PinnedTools, ToolsButton } from "./AllTools";
import { WindowControls } from "./WindowControls";

const st = () => useStore.getState();

export function TabBar() {
  const order = useStore((s) => s.order);
  const docs = useStore((s) => s.docs);
  const active = useStore((s) => s.active);
  const security = useProtect((s) => s.security);
  return (
    <div className="tabbar" role="tablist" data-tauri-drag-region>
      <span className="brand" data-tauri-drag-region>
        <svg width="16" height="16" viewBox="0 0 24 24" aria-hidden>
          <path d="M12 2.5c3.6 4.3 6.5 7.5 6.5 11a6.5 6.5 0 0 1-13 0c0-3.5 2.9-6.7 6.5-11Z" fill="currentColor" />
        </svg>
        Inkwell
      </span>
      <div
        role="tab"
        aria-selected={active == null}
        className={active == null ? "tab home-tab on" : "tab home-tab"}
        onMouseDown={(e) => e.button === 0 && useStore.setState({ active: null })}
      >
        <Icon name="home" size={14} />
        <span className="tab-name">Home</span>
      </div>
      {order.map((id) => (
        <div
          key={id}
          role="tab"
          aria-selected={id === active}
          className={id === active ? "tab on" : "tab"}
          title={docs[id].path}
          onMouseDown={(e) => {
            if (e.button === 1) {
              e.preventDefault();
              void requestClose(id);
            } else if (e.button === 0) st().activate(id);
          }}
        >
          <Icon name="file" size={14} />
          {security[id]?.encrypted && (
            <span className="tab-lock" title={security[id].openPassword ? "Opens with a password" : "Has password restrictions"}>
              <Icon name="lock" size={12} />
            </span>
          )}
          <span className="tab-name">{docs[id].name}</span>
          {docs[id].dirty && <span className="tab-dirty" title="Unsaved changes" />}
          <button
            className="tab-close"
            aria-label="Close tab"
            onMouseDown={(e) => e.stopPropagation()}
            onClick={() => void requestClose(id)}
          >
            <Icon name="close" size={12} />
          </button>
        </div>
      ))}
      <button className="icon-btn" title="Open file (Ctrl+O)" onClick={() => void openDialog()}>
        <Icon name="plus" />
      </button>
      <span className="grow" data-tauri-drag-region />
      <WindowControls />
    </div>
  );
}

function Btn({ icon, title, on, onClick, disabled }: { icon: IconName; title: string; on?: boolean; onClick: () => void; disabled?: boolean }) {
  return (
    <button className={on ? "icon-btn on" : "icon-btn"} title={title} aria-label={title} aria-pressed={on} onClick={onClick} disabled={disabled}>
      <Icon name={icon} />
    </button>
  );
}

const themeIcon: Record<Theme, IconName> = { system: "system", light: "sun", dark: "moon" };
const nextTheme: Record<Theme, Theme> = { system: "light", light: "dark", dark: "system" };

export function Toolbar({ doc }: { doc: DocState }) {
  const theme = useStore((s) => s.theme);
  const searchOpen = useStore((s) => s.searchOpen);
  const { id } = doc;
  const keys = (cmd: string) => {
    const k = commands.find((c) => c.id === cmd)?.keys?.[0];
    return k ? ` (${prettyKeys(k)})` : "";
  };

  // Organize keeps the same toolbar: only the viewer's own controls on the left give way to undo and redo.
  // The right-hand cluster stays put; its panels live in the reader, so using one leaves Organize first.
  const leaveOrganize = doc.organize ? () => st().patch(id, { organize: false }) : undefined;

  return (
    <div className="toolbar">
      <ModeSwitch doc={doc} />
      <ToolsButton doc={doc} />
      <PinnedTools doc={doc} />
      <span className="sep" />
      {doc.organize ? (
        <>
          <Btn icon="undo" title={doc.undoLabel ? `Undo ${doc.undoLabel} (Ctrl+Z)` : "Undo (Ctrl+Z)"} disabled={!doc.undoLabel} onClick={() => void undo(id)} />
          <Btn icon="redo" title={doc.redoLabel ? `Redo ${doc.redoLabel} (Ctrl+Y)` : "Redo (Ctrl+Y)"} disabled={!doc.redoLabel} onClick={() => void redo(id)} />
        </>
      ) : (
        <ViewerControls doc={doc} keys={keys} />
      )}
      <span className="grow" />
      <span className="tb-cluster" onClickCapture={leaveOrganize}>
        <Btn icon="search" title={`Find${keys("find")}`} on={searchOpen && !doc.organize} onClick={() => st().setUi({ searchOpen: doc.organize ? true : !searchOpen })} />
        <CommentsToggle />
        <FormsToggle />
        <RedactToggle />
        <HistoryToggle />
        <AssistantToggle />
      </span>
      <Btn icon={themeIcon[theme]} title={`Theme: ${theme}`} onClick={() => st().setTheme(nextTheme[theme])} />
      <Btn icon="command" title={`Command palette${keys("palette")}`} onClick={() => st().setUi({ paletteOpen: true })} />
    </div>
  );
}

function ViewerControls({ doc, keys }: { doc: DocState; keys: (cmd: string) => string }) {
  const sidebar = useStore((s) => s.sidebar);
  const { id } = doc;
  const zoomValue = doc.zoomMode === "custom" ? String(Math.round(doc.zoom * 100) / 100) : doc.zoomMode;
  const presets = ZOOM_STEPS.filter((z) => z >= 0.25 && z <= 8);
  const custom = doc.zoomMode === "custom" && !presets.some((z) => Math.abs(z - doc.zoom) < 0.005);

  return (
    <>
      <Btn icon="sidebar" title={`Toggle sidebar${keys("sidebar")}`} on={sidebar !== "none"} onClick={() => st().toggleSidebar("thumbs")} />
      <Btn icon="outline" title="Outline" on={sidebar === "outline"} onClick={() => st().toggleSidebar("outline")} />
      <Btn icon="paperclip" title="Attachments" on={sidebar === "attachments"} onClick={() => st().toggleSidebar("attachments")} />
      <span className="sep" />
      <PageBox doc={doc} />
      <span className="sep" />
      <Btn icon="minus" title={`Zoom out${keys("zoom-out")}`} onClick={() => st().setZoom(id, stepZoom(doc.zoom, -1))} />
      <select
        className="zoom-select"
        value={zoomValue}
        title="Zoom"
        onChange={(e) => {
          const v = e.target.value;
          if (v === "fitWidth" || v === "fitPage") st().setZoomMode(id, v);
          else st().setZoom(id, Number(v));
        }}
      >
        <option value="fitWidth">Fit width</option>
        <option value="fitPage">Fit page</option>
        {custom && <option value={zoomValue}>{Math.round(doc.zoom * 100)}%</option>}
        {presets.map((z) => (
          <option key={z} value={String(z)}>
            {Math.round(z * 100)}%
          </option>
        ))}
        {doc.zoomMode !== "custom" && <option value="custom" hidden>{Math.round(doc.zoom * 100)}%</option>}
      </select>
      <Btn icon="plus" title={`Zoom in${keys("zoom-in")}`} onClick={() => st().setZoom(id, stepZoom(doc.zoom, 1))} />
      <Btn icon="rotate" title={`Rotate${keys("rotate-cw")}`} onClick={() => st().rotate(id, 1)} />
      <Btn icon="present" title={`Full screen${keys("present")}`} onClick={() => useSheets.getState().openPresent(id, doc.currentPage)} />
      <span className="sep" />
      <DarkPagesButton />
    </>
  );
}

const pageModes: [PageDark, string][] = [
  ["follow", "Follow app theme"],
  ["on", "Always dark"],
  ["off", "Always normal"],
];

/**
 * Shows the pages dark or normal, whatever the app's own theme (the file is never changed). A page-view
 * setting like zoom and rotate: the button flips it, the chevron (or a right-click) chooses Follow / dark / normal.
 */
function DarkPagesButton() {
  const dark = usePagesDark();
  const systemDark = useSystemDark();
  const setting = useStore((s) => s.pageDark);
  const group = useRef<HTMLSpanElement>(null);
  const keys = commands.find((c) => c.id === "toggle-dark-pages")?.keys?.[0];
  const title = `${dark ? "Normal pages" : "Dark pages"}${keys ? ` (${prettyKeys(keys)})` : ""}`;
  return (
    <span
      ref={group}
      className="dp-split"
      onContextMenu={(e) => {
        e.preventDefault();
        group.current?.querySelector<HTMLButtonElement>(".dp-chev")?.click();
      }}
    >
      <Btn
        icon="pageDark"
        title={title}
        on={dark}
        onClick={() => {
          const s = st();
          s.setPageDark(togglePageDark(s.pageDark, s.theme, systemDark));
        }}
      />
      <MenuButton
        className="icon-btn dp-chev"
        label="Page colours"
        items={pageModes.map(([m, label]) => ({ label, checked: setting === m, onSelect: () => st().setPageDark(m) }))}
      >
        <Icon name="down" size={10} />
      </MenuButton>
    </span>
  );
}

function PageBox({ doc }: { doc: DocState }) {
  // With page labels the box shows and takes the label ("iii"); without them, the page number.
  const labelled = doc.labels.length > 0;
  const [text, setText] = useState(pageLabel(doc, doc.currentPage));
  const editing = useRef(false);
  useEffect(() => {
    if (!editing.current) setText(pageLabel(doc, doc.currentPage));
  }, [doc.currentPage, doc.id, doc.labels]);
  const go = () => {
    const byLabel = pageOfLabel(doc.labels, text);
    const n = byLabel != null ? byLabel + 1 : parseInt(text, 10);
    if (Number.isFinite(n)) st().goTo(doc.id, n - 1);
    editing.current = false;
    setText(pageLabel(doc, Math.min(Math.max(n || 1, 1), doc.pageCount) - 1));
  };
  return (
    <div className="pagebox">
      <input
        id="page-input"
        style={{ width: `${pageBoxCh(doc.pageCount, labelled)}ch` }}
        value={text}
        inputMode="numeric"
        onFocus={() => (editing.current = true)}
        onBlur={() => {
          editing.current = false;
          setText(pageLabel(doc, doc.currentPage));
        }}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            go();
            (e.target as HTMLInputElement).blur();
          } else if (e.key === "Escape") (e.target as HTMLInputElement).blur();
        }}
      />
      <span className="dim">{labelled ? `(${doc.currentPage + 1} of ${doc.pageCount})` : `/ ${doc.pageCount}`}</span>
    </div>
  );
}

export function SearchBar({ doc }: { doc: DocState }) {
  const input = useRef<HTMLInputElement>(null);
  const [q, setQ] = useState(doc.search.query);
  const { id, search } = doc;

  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);

  // Debounced live search; an empty box clears the highlights.
  useEffect(() => {
    if (q.trim() === search.query.trim()) return;
    const t = setTimeout(() => st().runSearch(id, q), 220);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, id]);

  const n = search.hits.length;
  const status = !search.query ? "" : n === 0 ? (search.done ? "No results" : "Searching…") : `${search.current + 1} / ${n}${search.done ? "" : "…"}`;
  const close = () => {
    st().setUi({ searchOpen: false });
    document.querySelector<HTMLElement>(".viewer")?.focus();
  };

  return (
    <div className="searchbar">
      <Icon name="search" size={14} />
      <input
        ref={input}
        value={q}
        placeholder="Find in document"
        spellCheck={false}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            if (q.trim() !== search.query.trim()) st().runSearch(id, q);
            else st().stepHit(id, e.shiftKey ? -1 : 1);
          } else if (e.key === "Escape") close();
        }}
      />
      <span className="dim count">{status}</span>
      <Btn icon="up" title="Previous (Shift+Enter)" onClick={() => st().stepHit(id, -1)} disabled={n === 0} />
      <Btn icon="down" title="Next (Enter)" onClick={() => st().stepHit(id, 1)} disabled={n === 0} />
      <Btn icon="close" title="Close (Esc)" onClick={close} />
    </div>
  );
}
