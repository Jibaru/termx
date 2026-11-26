import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { installGlobalErrorHandlers } from "./lib/errorReporting";
import { sessionStore } from "./terminal/sessionStore";
import { installE2eHook } from "./test/e2eHook";
import "./styles.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("Root element #root not found");
}

// Before the first render, so that errors during startup are recorded too.
installGlobalErrorHandlers();

// Does nothing outside the builds made for the end-to-end suite.
installE2eHook(sessionStore);

createRoot(container).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
