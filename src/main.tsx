import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { trackReadingPosition } from "./recents";
import { useA11y } from "./lib/a11y";
import { useAi } from "./lib/aiStore";
import { useAnnot } from "./lib/annot";
import { useForms } from "./lib/forms";
import { useHistory } from "./lib/historyStore";
import * as api from "./lib/api";
import { useCombine } from "./lib/combineStore";
import { useConvert } from "./lib/convertStore";
import * as edits from "./lib/edit";
import { useSheets } from "./lib/organizePlus";
import { usePrint } from "./lib/printStore";
import { useProtect } from "./lib/protectStore";
import { useTextEdit } from "./lib/textEdit";
import { useTools } from "./lib/toolsStore";
import { redactApi, useRedact } from "./lib/redact";
import { useStore } from "./store";
import "./styles.css";

// Handle for automated checks (benchmarks, screenshots); harmless in normal use.
(window as unknown as { __inkwell: unknown }).__inkwell = { useStore, useAi, useHistory, useTools, useConvert, useRedact, redactApi, useAnnot, useForms, usePrint, useProtect, useTextEdit, useA11y, useCombine, useSheets, edits, api };
trackReadingPosition(useStore);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
