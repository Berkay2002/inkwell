import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect } from "react";
import { AssistantPanel } from "./components/assistant/AssistantPanel";
import { SearchBar, TabBar, Toolbar } from "./components/Chrome";
import { Home } from "./components/Home";
import { CommandPalette, PasswordDialog, Toast } from "./components/Overlays";
import { Sidebar } from "./components/Sidebar";
import { ToolsPanel } from "./components/ToolsPanel";
import { Viewer } from "./components/Viewer";
import { initialFiles } from "./lib/api";
import { commandFor } from "./lib/commands";
import { guardWindowClose } from "./lib/edit";
import { Organize } from "./components/Organize";
import { useActiveDoc, useStore } from "./store";

const isPdf = (p: string) => /\.pdf$/i.test(p);
let launchFilesOpened = false;

export default function App() {
  const doc = useActiveDoc();
  const theme = useStore((s) => s.theme);
  const paletteOpen = useStore((s) => s.paletteOpen);
  const searchOpen = useStore((s) => s.searchOpen);
  const hasPassword = useStore((s) => s.password != null);

  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.dataset.theme = theme;
  }, [theme]);

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
      initialFiles().then((files) => files.forEach((f) => void st.openPath(f)), () => {});
    }

    // The listener registers asynchronously: if this effect is torn down first, unlisten on arrival.
    let off: (() => void) | undefined;
    let cancelled = false;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        if (e.payload.type === "drop") e.payload.paths.filter(isPdf).forEach((p) => void st.openPath(p));
      })
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
          <div className="main">
            <Sidebar doc={doc} />
            <div className="stage">
              <Viewer key={doc.id} doc={doc} />
              {searchOpen && <SearchBar key={doc.id} doc={doc} />}
            </div>
            <ToolsPanel doc={doc} />
            <AssistantPanel doc={doc} />
          </div>
        </>
      ) : (
        <Home />
      )}
      {paletteOpen && <CommandPalette />}
      {hasPassword && <PasswordDialog />}
      <Toast />
    </div>
  );
}
