import { Component, type ErrorInfo, type ReactNode } from "react";

import { reportError } from "../lib/errorReporting";
import { describeError } from "../lib/ipc";
import { DiagnosticsActions } from "./DiagnosticsActions";

interface Props {
  children: ReactNode;
}

interface State {
  error: unknown;
  failed: boolean;
}

/**
 * Catches errors thrown while rendering, records them in the log and shows a
 * readable message instead of a blank window.
 */
export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: undefined, failed: false };

  static getDerivedStateFromError(error: unknown): State {
    return { error, failed: true };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo) {
    reportError("react", error, { componentStack: info.componentStack ?? undefined });
  }

  override render() {
    if (!this.state.failed) return this.props.children;

    return (
      <div className="error-fallback" role="alert">
        <h1 className="error-title">Something went wrong</h1>
        <p className="error-text">
          termx hit an unexpected error and could not draw this window. The error was saved to the
          log.
        </p>
        <pre className="error-detail">{describeError(this.state.error)}</pre>
        <button
          type="button"
          className="button"
          onClick={() => {
            this.setState({ error: undefined, failed: false });
          }}
        >
          Try again
        </button>
        <p className="error-text">
          If it keeps happening, export the diagnostics and attach the file to a bug report.
        </p>
        <DiagnosticsActions />
      </div>
    );
  }
}
