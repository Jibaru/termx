import { useState } from "react";

import { describeError, exportDiagnostics, openLogFolder } from "../lib/ipc";

type Status = { kind: "idle" } | { kind: "busy" } | { kind: "done" | "failed"; text: string };

/**
 * Two small actions for getting the logs out of the app: open the log folder
 * and export a diagnostics bundle. A stand-in until the settings screen and
 * the command palette exist.
 */
export function DiagnosticsActions() {
  const [status, setStatus] = useState<Status>({ kind: "idle" });

  const run = (action: () => Promise<string | null>) => {
    setStatus({ kind: "busy" });
    action()
      .then((text) => {
        setStatus(text === null ? { kind: "idle" } : { kind: "done", text });
      })
      .catch((error: unknown) => {
        setStatus({ kind: "failed", text: describeError(error) });
      });
  };

  return (
    <div className="diagnostics-actions">
      <div className="diagnostics-buttons">
        <button
          type="button"
          className="link-button"
          disabled={status.kind === "busy"}
          onClick={() => {
            run(() => openLogFolder().then(() => null));
          }}
        >
          Open log folder
        </button>
        <button
          type="button"
          className="link-button"
          disabled={status.kind === "busy"}
          onClick={() => {
            run(() => exportDiagnostics().then((bundle) => `Saved ${bundle.path}`));
          }}
        >
          Export diagnostics
        </button>
      </div>
      {(status.kind === "done" || status.kind === "failed") && (
        <p
          className="diagnostics-status"
          data-kind={status.kind}
          role={status.kind === "failed" ? "alert" : "status"}
        >
          {status.text}
        </p>
      )}
    </div>
  );
}
