import { useEffect, useRef, useState } from "react";
import { openDialog, prettyKeys, commands } from "../lib/commands";
import { redo, requestClose, undo } from "../lib/edit";
import { ZOOM_STEPS, stepZoom } from "../lib/layout";
import { openA11y } from "../lib/a11y";
import { useConvert } from "../lib/convertStore";
import { useProtect } from "../lib/protectStore";
import { useTools } from "../lib/toolsStore";
import { type DocState, type Theme, useStore } from "../store";
import { AssistantToggle } from "./assistant/AssistantPanel";
import { HistoryToggle, openCompareSetup } from "./history/HistoryPanel";
import { Icon, type IconName } from "./Icons";
import { MenuButton } from "./Menu";
import { CommentsToggle } from "./comments/CommentsPanel";
import { FormsToggle } from "./forms/FormsPanel";
import { RedactToggle } from "./redact/RedactPanel";
import { ModeSwitch } from "./Organize";
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
    <button className={on ? "icon-btn on" : "icon-btn"} title={title} aria-label={title} onClick={onClick} disabled={disabled}>
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
        <MenuButton
          className="ax-toggle"
          label="Tools"
          items={[
            { label: "Compress file…", onSelect: () => useTools.getState().open("compress", id) },
            { label: "Recognize text…", onSelect: () => useTools.getState().open("ocr", id) },
            { label: "Protect with a password…", onSelect: () => useProtect.getState().open("protect", id) },
            { label: "Security…", onSelect: () => useProtect.getState().open("security", id) },
            { label: "Compare files…", onSelect: () => void openCompareSetup({ id, name: doc.name }) },
            { label: "Check accessibility…", onSelect: openA11y },
            { label: "Export…", onSelect: () => useConvert.getState().openExport(id) },
            { label: "Create PDF…", onSelect: () => useConvert.getState().openCreate() },
          ]}
        >
          Tools
        </MenuButton>
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
    </>
  );
}

function PageBox({ doc }: { doc: DocState }) {
  const [text, setText] = useState(String(doc.currentPage + 1));
  const editing = useRef(false);
  useEffect(() => {
    if (!editing.current) setText(String(doc.currentPage + 1));
  }, [doc.currentPage, doc.id]);
  const go = () => {
    const n = parseInt(text, 10);
    if (Number.isFinite(n)) st().goTo(doc.id, n - 1);
    editing.current = false;
    setText(String(Math.min(Math.max(n || 1, 1), doc.pageCount)));
  };
  return (
    <div className="pagebox">
      <input
        id="page-input"
        value={text}
        inputMode="numeric"
        onFocus={() => (editing.current = true)}
        onBlur={() => {
          editing.current = false;
          setText(String(doc.currentPage + 1));
        }}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            go();
            (e.target as HTMLInputElement).blur();
          } else if (e.key === "Escape") (e.target as HTMLInputElement).blur();
        }}
      />
      <span className="dim">/ {doc.pageCount}</span>
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
