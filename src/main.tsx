import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { trackReadingPosition } from "./recents";
import { useA11y } from "./lib/a11y";
import { usePreflight } from "./lib/preflight";
import { useAi } from "./lib/aiStore";
import { useBar } from "./components/assistant/barView";
import { useAnnot } from "./lib/annot";
import { useForms } from "./lib/forms";
import { useFind } from "./lib/findStore";
import { useSearch } from "./lib/searchStore";
import { useHistory } from "./lib/historyStore";
import * as api from "./lib/api";
import { useCombine } from "./lib/combineStore";
import { useConvert } from "./lib/convertStore";
import * as edits from "./lib/edit";
import { useMarks } from "./lib/marksStore";
import { useSheets } from "./lib/organizePlus";
import { usePrint } from "./lib/printStore";
import { useProtect } from "./lib/protectStore";
import { useImageEdit } from "./lib/imageEdit";
import { useTextEdit } from "./lib/textEdit";
import { useToolPrefs } from "./lib/toolPrefs";
import { useTools } from "./lib/toolsStore";
import { redactApi, useRedact } from "./lib/redact";
import { useStore } from "./store";
import "./styles.css";

// Handle for automated checks (benchmarks, screenshots); harmless in normal use.
(window as unknown as { __inkwell: unknown }).__inkwell = { useStore, useAi, useBar, useHistory, useTools, useConvert, useRedact, redactApi, useAnnot, useForms, useFind, useSearch, usePrint, useProtect, useMarks, useTextEdit, useImageEdit, useA11y, usePreflight, useCombine, useSheets, useToolPrefs, edits, api };
trackReadingPosition(useStore);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
