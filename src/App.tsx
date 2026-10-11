import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useEffect } from "react";
import { AssistantPanel } from "./components/assistant/AssistantPanel";
import { SearchBar, TabBar, Toolbar } from "./components/Chrome";
import { CompareView } from "./components/history/Compare";
import { HistoryBanner, HistoryPanel } from "./components/history/HistoryPanel";
import { VersionView } from "./components/history/VersionView";
import { Home } from "./components/Home";
import { CommandPalette, PasswordDialog, Toast } from "./components/Overlays";
import { CommentTools, CommentsPanel } from "./components/comments/CommentsPanel";
import { FormTools, FormsPanel } from "./components/forms/FormsPanel";
import { RedactPanel } from "./components/redact/RedactPanel";
import { wireCommentRail } from "./lib/commentRail";
import { Sidebar } from "./components/Sidebar";
import { ConvertPanel } from "./components/ConvertPanel";
import { ToolsPanel } from "./components/ToolsPanel";
import { Viewer } from "./components/Viewer";
import { type FileRef, initialFiles } from "./lib/api";
import { commandFor } from "./lib/commands";
import { startHistoryEvents, useHistory } from "./lib/historyStore";
import { guardWindowClose } from "./lib/edit";
import { Organize } from "./components/Organize";
import { useActiveDoc, useStore } from "./store";

let launchFilesOpened = false;

export default function App() {
  const doc = useActiveDoc();
  const theme = useStore((s) => s.theme);
  const paletteOpen = useStore((s) => s.paletteOpen);
  const searchOpen = useStore((s) => s.searchOpen);
  const hasPassword = useStore((s) => s.password != null);
  const compare = useHistory((s) => s.compare);
  const viewing = useHistory((s) => (doc ? s.selected[doc.id] : undefined));
  const showCompare = compare != null && (compare.doc === null || compare.doc === doc?.id);

  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => startHistoryEvents(), []);
  useEffect(() => wireCommentRail(), []);

  // Every open document's history state (recovery and "same document?" prompts show over the page).
  useEffect(() => {
    if (doc) void useHistory.getState().refresh(doc.id);
  }, [doc?.id]);

  useEffect(() => {
    getCurrentWindow()
      .setTitle(doc ? `${doc.name} — Inkwell` : "Inkwell")
      .catch(() => {});
  }, [doc?.name]);

  useEffect(() => {
    const st = useStore.getState();
    // Command-line files are opened once per app run (StrictMode runs this effect twice in dev).
    if (!launchFilesOpened) {
      launchFilesOpened = true;
      initialFiles().then((files) => files.forEach((f) => void st.openPath(f.path, f.token)), () => {});
    }

    // The listener registers asynchronously: if this effect is torn down first, unlisten on arrival.
    let off: (() => void) | undefined;
    let cancelled = false;
    // Rust turns PDFs dropped from Explorer into file tokens (src-tauri/src/paths.rs).
    getCurrentWebviewWindow()
      .listen<FileRef[]>("files-dropped", (e) => e.payload.forEach((f) => void st.openPath(f.path, f.token)))
      .then((u) => (cancelled ? u() : (off = u)))
      .catch(() => {});

    const onKey = (e: KeyboardEvent) => {
      const c = commandFor(e);
      const t = e.target as HTMLElement | null;
      if (c?.notInText && t?.closest("input, textarea, [contenteditable]")) return;
      if (c) {
        e.preventDefault();
        if (!c.needsDoc || useStore.getState().active != null) c.run();
      } else if (e.key === "Escape" && useStore.getState().searchOpen) {
        useStore.getState().setUi({ searchOpen: false });
      }
    };
    const noMenu = (e: MouseEvent) => e.preventDefault();
    window.addEventListener("keydown", onKey);
    window.addEventListener("contextmenu", noMenu);
    const unguard = guardWindowClose();
    return () => {
      unguard();
      cancelled = true;
      off?.();
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("contextmenu", noMenu);
    };
  }, []);

  return (
    <div className="app">
      <TabBar />
      {doc?.organize ? (
        <Organize key={doc.id} doc={doc} />
      ) : doc ? (
        <>
          <Toolbar doc={doc} />
          {showCompare ? (
            <div className="main">
              <CompareView />
            </div>
          ) : (
            <div className="main">
              <Sidebar doc={doc} />
              <div className="stage">
                {viewing ? <VersionView key={`${doc.id}:${viewing}`} doc={doc} versionId={viewing} /> : <Viewer key={doc.id} doc={doc} />}
                {searchOpen && !viewing && <SearchBar key={doc.id} doc={doc} />}
                <HistoryBanner doc={doc} />
                <CommentTools doc={doc} />
                <FormTools />
              </div>
              <ToolsPanel doc={doc} />
              <ConvertPanel doc={doc} />
              <AssistantPanel doc={doc} />
              <HistoryPanel doc={doc} />
              <RedactPanel doc={doc} />
              <CommentsPanel doc={doc} />
              <FormsPanel doc={doc} />
            </div>
          )}
        </>
      ) : showCompare ? (
        <div className="main" style={{ gridRow: "2 / 4" }}>
          <CompareView />
        </div>
      ) : (
        <div className="home-wrap">
          <Home />
          <ConvertPanel doc={null} />
        </div>
      )}
      {paletteOpen && <CommandPalette />}
      {hasPassword && <PasswordDialog />}
      <Toast />
    </div>
  );
}
