import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { installGlobalErrorHandlers } from "./lib/errorReporting";
import "./styles.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("Root element #root not found");
}

// Before the first render, so that errors during startup are recorded too.
installGlobalErrorHandlers();

createRoot(container).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
