import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { trackReadingPosition } from "./recents";
import { useStore } from "./store";
import "./styles.css";

// Handle for automated checks (benchmarks, screenshots); harmless in normal use.
(window as unknown as { __inkwell: unknown }).__inkwell = { useStore };
trackReadingPosition(useStore);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
