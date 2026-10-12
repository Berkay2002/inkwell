import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useEffect, type ReactNode } from "react";
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
import { ErrorBoundary, type Variant } from "./components/ErrorBoundary";
import { useActiveDoc, useStore } from "./store";

/** A panel, strip or sheet that crashes shows its own error card; the rest of the window keeps working. */
const Guard = ({ name, variant = "dock", children }: { name: string; variant?: Variant; children: ReactNode }) => {
  const active = useStore((s) => s.active);
  return (
    <ErrorBoundary name={name} variant={variant} resetKey={active}>
      {children}
    </ErrorBoundary>
  );
};

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
      <Guard name="The tab bar" variant="overlay">
        <TabBar />
      </Guard>
      {doc?.organize ? (
        <>
          <Guard name="The toolbar" variant="overlay">
            <Toolbar doc={doc} />
          </Guard>
          <Guard name="Organize pages" variant="fill">
            <Organize key={doc.id} doc={doc} />
          </Guard>
        </>
      ) : doc ? (
        <>
          <Guard name="The toolbar" variant="overlay">
            <Toolbar doc={doc} />
          </Guard>
          <Guard name="Search" variant="overlay">
            <SearchDriver key={doc.id} doc={doc} />
          </Guard>
          {showCompare ? (
            <div className="main">
              <Guard name="Compare" variant="fill">
                <CompareView />
              </Guard>
            </div>
          ) : (
            <div className="main">
              <Guard name="The sidebar">
                <Sidebar doc={doc} />
              </Guard>
              <div className="stage">
                <Guard name="The page view" variant="fill">
                  {viewing ? (
                    <VersionView key={`${doc.id}:${viewing}`} doc={doc} versionId={viewing} />
                  ) : portfolio?.info && !portfolio.cover ? (
                    <PortfolioView key={doc.id} doc={doc} info={portfolio.info} />
                  ) : (
                    <Viewer key={doc.id} doc={doc} />
                  )}
                </Guard>
                {!viewing && portfolio?.info && portfolio.cover && (
                  <Guard name="The portfolio note" variant="overlay">
                    <PortfolioCoverNote doc={doc} />
                  </Guard>
                )}
                {searchOpen && !viewing && (
                  <Guard name="The find bar" variant="overlay">
                    <SearchBar key={doc.id} doc={doc} />
                  </Guard>
                )}
                <Guard name="The history banner" variant="overlay">
                  <HistoryBanner doc={doc} />
                </Guard>
                <Guard name="The security notice" variant="overlay">
                  <SecurityNotice doc={doc} />
                </Guard>
                <Guard name="The comment tools" variant="overlay">
                  <CommentTools doc={doc} />
                </Guard>
                <Guard name="The signature strip" variant="overlay">
                  <SignStrip doc={doc} />
                </Guard>
                {!viewing && (
                  <Guard name="The signing banner" variant="overlay">
                    <SignBanner doc={doc} />
                  </Guard>
                )}
                <Guard name="The measure bar" variant="overlay">
                  <MeasureBar doc={doc} />
                </Guard>
                <Guard name="The form tools" variant="overlay">
                  <FormTools />
                </Guard>
                <Guard name="Articles" variant="overlay">
                  <ThreadHost doc={doc} />
                </Guard>
                <Guard name="The forms banner" variant="overlay">
                  <FormsBanner doc={doc} />
                </Guard>
              </div>
              <Guard name="All tools"><ToolsPanel doc={doc} /></Guard>
              <Guard name="Convert"><ConvertPanel doc={doc} /></Guard>
              <Guard name="Protect"><ProtectPanel doc={doc} /></Guard>
              <Guard name="Page marks"><MarksPanel doc={doc} /></Guard>
              <Guard name="Assistant"><AssistantPanel doc={doc} /></Guard>
              <Guard name="Version history"><HistoryPanel doc={doc} /></Guard>
              <Guard name="Redact"><RedactPanel doc={doc} /></Guard>
              <Guard name="Accessibility"><A11yPanel doc={doc} /></Guard>
              <Guard name="Preflight"><PreflightPanel doc={doc} /></Guard>
              <Guard name="Output preview"><PrepressPanel doc={doc} /></Guard>
              <Guard name="Comments"><CommentsPanel doc={doc} /></Guard>
              <Guard name="Fill & Sign"><SignRail doc={doc} /></Guard>
              <Guard name="Measure"><MeasurePanel doc={doc} /></Guard>
              <Guard name="Edit"><EditPanel doc={doc} /></Guard>
              <Guard name="Forms"><FormsPanel doc={doc} /></Guard>
              <Guard name="Search"><SearchPanel doc={doc} /></Guard>
            </div>
          )}
        </>
      ) : showCompare ? (
        <div className="main" style={{ gridRow: "2 / 4" }}>
          <Guard name="Compare" variant="fill">
            <CompareView />
          </Guard>
        </div>
      ) : (
        <div className="home-wrap">
          <Guard name="Home" variant="fill">
            <Home />
          </Guard>
          <Guard name="Convert">
            <ConvertPanel doc={null} />
          </Guard>
        </div>
      )}
      <Guard name="Print" variant="overlay">
        <PrintSheet />
      </Guard>
      <Guard name="Output preview" variant="overlay">
        <PrepressSheets />
      </Guard>
      <Guard name="Combine files" variant="overlay">
        <CombineSheet />
      </Guard>
      <Guard name="Action Wizard" variant="overlay">
        <ActionWizard />
      </Guard>
      <Guard name="Document JavaScript" variant="overlay">
        <DocJsSheet />
      </Guard>
      <Guard name="Stamps" variant="overlay">
        <StampsManager />
      </Guard>
      <Guard name="Page labels" variant="overlay">
        <LabelsSheet />
      </Guard>
      <Guard name="Page transitions" variant="overlay">
        <TransitionsSheet />
      </Guard>
      <Guard name="Presentation" variant="overlay">
        <Presentation />
      </Guard>
      {paletteOpen && (
        <Guard name="The command palette" variant="overlay">
          <CommandPalette />
        </Guard>
      )}
      {hasPassword && (
        <Guard name="The password dialog" variant="overlay">
          <PasswordDialog />
        </Guard>
      )}
      <Guard name="Signatures" variant="overlay">
        <SignCreator />
      </Guard>
      <Guard name="Form scripts" variant="overlay">
        <ScriptPrompts />
      </Guard>
      <Guard name="Notifications" variant="overlay">
        <Toast />
      </Guard>
    </div>
  );
}
