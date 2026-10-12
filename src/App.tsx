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
import { MeasureBar } from "./components/measure/MeasureBar";
import { MeasurePanel } from "./components/measure/MeasurePanel";
import { A11yPanel } from "./components/a11y/A11yPanel";
import { PreflightPanel } from "./components/preflight/PreflightPanel";
import { EditPanel } from "./components/edit/EditPanel";
import { FormTools, FormsBanner, FormsPanel } from "./components/forms/FormsPanel";
import { ScriptPrompts } from "./components/forms/ScriptPrompts";
import { RedactPanel } from "./components/redact/RedactPanel";
import { SignRail } from "./components/sign/SignRail";
import { SignBanner, SignCreator, SignStrip } from "./components/sign/SignTools";
import { SearchDriver, SearchPanel } from "./components/search/SearchPanel";
import { wireCommentRail } from "./lib/commentRail";
import { wirePrepressRail } from "./lib/prepressRail";
import { PrepressPanel } from "./components/prepress/PrepressPanel";
import { PrepressSheets } from "./components/prepress/PrepressSheets";
import { Sidebar } from "./components/Sidebar";
import { ThreadHost } from "./components/threads/Threads";
import { ConvertPanel } from "./components/ConvertPanel";
import { MarksPanel } from "./components/MarksPanel";
import { ProtectPanel, SecurityNotice } from "./components/ProtectPanel";
import { PrintSheet } from "./components/PrintSheet";
import { ActionWizard } from "./components/ActionWizard";
import { CombineSheet } from "./components/CombineSheet";
import { DocJsSheet } from "./components/DocJsSheet";
import { StampsManager } from "./components/comments/StampsManager";
import { LabelsSheet, TransitionsSheet } from "./components/OrganizePlus";
import { Presentation } from "./components/Presentation";
import { ToolsPanel } from "./components/ToolsPanel";
import { PortfolioCoverNote, PortfolioView } from "./components/PortfolioView";
import { Viewer } from "./components/Viewer";
import { type FileRef, initialFiles } from "./lib/api";
import { commandFor } from "./lib/commands";
import { useCombine } from "./lib/combineStore";
import { startHistoryEvents, useHistory } from "./lib/historyStore";
import { usePortfolio } from "./lib/portfolioStore";
import { guardWindowClose } from "./lib/edit";
import { usePagesDark } from "./lib/usePagesDark";
import { useToolPrefs } from "./lib/toolPrefs";
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
  const portfolio = usePortfolio((s) => (doc ? s.docs[doc.id] : undefined));

  useEffect(() => {
    if (theme === "system") document.documentElement.removeAttribute("data-theme");
    else document.documentElement.dataset.theme = theme;
  }, [theme]);

  // The pages' own colours: styles.css inverts the page images under data-pages="dark".
  const pagesDark = usePagesDark();
  useEffect(() => {
    if (pagesDark) document.documentElement.dataset.pages = "dark";
    else document.documentElement.removeAttribute("data-pages");
  }, [pagesDark]);

  // The All tools panel belongs to the reader: Home, a comparison and Organize leave it closed, so coming
  // back neither shows it unasked nor moves focus into it.
  const noDoc = doc == null;
  const organizing = doc?.organize ?? false;
  useEffect(() => {
    if (noDoc || showCompare || organizing) useToolPrefs.getState().setPanel(false);
  }, [noDoc, showCompare, organizing]);

  useEffect(() => startHistoryEvents(), []);
  useEffect(() => wireCommentRail(), []);
  useEffect(() => wirePrepressRail(), []);

  // Every open document's history state (recovery and "same document?" prompts show over the page).
  useEffect(() => {
    if (doc) void useHistory.getState().refresh(doc.id);
  }, [doc?.id]);

  // Page labels show in the page box and under every thumbnail: read again with every generation.
  useEffect(() => {
    if (doc) useStore.getState().loadLabels(doc.id);
  }, [doc?.id, doc?.rev]);

  // A PDF Portfolio (a /Collection over its embedded files) opens as a file list: read again with every generation.
  useEffect(() => {
    if (doc) void usePortfolio.getState().load(doc.id);
  }, [doc?.id, doc?.rev]);

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
      .listen<FileRef[]>("files-dropped", (e) => {
        // The Combine sheet takes the files dropped on it; anywhere else they open in tabs.
        if (useCombine.getState().sheet) useCombine.getState().add(e.payload);
        else e.payload.forEach((f) => void st.openPath(f.path, f.token));
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
        <>
          <Toolbar doc={doc} />
          <Organize key={doc.id} doc={doc} />
        </>
      ) : doc ? (
        <>
          <Toolbar doc={doc} />
          <SearchDriver key={doc.id} doc={doc} />
          {showCompare ? (
            <div className="main">
              <CompareView />
            </div>
          ) : (
            <div className="main">
              <Sidebar doc={doc} />
              <div className="stage">
                {viewing ? (
                  <VersionView key={`${doc.id}:${viewing}`} doc={doc} versionId={viewing} />
                ) : portfolio?.info && !portfolio.cover ? (
                  <PortfolioView key={doc.id} doc={doc} info={portfolio.info} />
                ) : (
                  <Viewer key={doc.id} doc={doc} />
                )}
                {!viewing && portfolio?.info && portfolio.cover && <PortfolioCoverNote doc={doc} />}
                {searchOpen && !viewing && <SearchBar key={doc.id} doc={doc} />}
                <HistoryBanner doc={doc} />
                <SecurityNotice doc={doc} />
                <CommentTools doc={doc} />
                <SignStrip doc={doc} />
                {!viewing && <SignBanner doc={doc} />}
                <MeasureBar doc={doc} />
                <FormTools />
                <ThreadHost doc={doc} />
                <FormsBanner doc={doc} />
              </div>
              <ToolsPanel doc={doc} />
              <ConvertPanel doc={doc} />
              <ProtectPanel doc={doc} />
              <MarksPanel doc={doc} />
              <AssistantPanel doc={doc} />
              <HistoryPanel doc={doc} />
              <RedactPanel doc={doc} />
              <A11yPanel doc={doc} />
              <PreflightPanel doc={doc} />
              <PrepressPanel doc={doc} />
              <CommentsPanel doc={doc} />
              <SignRail doc={doc} />
              <MeasurePanel doc={doc} />
              <EditPanel doc={doc} />
              <FormsPanel doc={doc} />
              <SearchPanel doc={doc} />
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
      <PrintSheet />
      <PrepressSheets />
      <CombineSheet />
      <ActionWizard />
      <DocJsSheet />
      <StampsManager />
      <LabelsSheet />
      <TransitionsSheet />
      <Presentation />
      {paletteOpen && <CommandPalette />}
      {hasPassword && <PasswordDialog />}
      <SignCreator />
      <ScriptPrompts />
      <Toast />
    </div>
  );
}
