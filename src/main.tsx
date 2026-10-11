import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { trackReadingPosition } from "./recents";
import { useAi } from "./lib/aiStore";
import { useAnnot } from "./lib/annot";
import { useHistory } from "./lib/historyStore";
import { useConvert } from "./lib/convertStore";
import { usePrint } from "./lib/printStore";
import { useMarks } from "./lib/marksStore";
import { useProtect } from "./lib/protectStore";
import { useTools } from "./lib/toolsStore";
import { redactApi, useRedact } from "./lib/redact";
import { useStore } from "./store";
import "./styles.css";

// Handle for automated checks (benchmarks, screenshots); harmless in normal use.
(window as unknown as { __inkwell: unknown }).__inkwell = { useStore, useAi, useHistory, useTools, useConvert, useRedact, redactApi, useAnnot, usePrint, useProtect, useMarks };
trackReadingPosition(useStore);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
