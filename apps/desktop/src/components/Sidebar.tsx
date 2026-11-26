import { type CoreVersionState, useCoreVersion } from "../lib/useCoreVersion";
import { DiagnosticsActions } from "./DiagnosticsActions";

function describeCoreVersion(state: CoreVersionState): string {
  switch (state.status) {
    case "loading":
      return "core …";
    case "ready":
      return `core v${state.version}`;
    case "error":
      return "core version unavailable";
  }
}

/**
 * Left region that will list the open terminal sessions. The list is empty
 * for now; the footer names the application and holds the diagnostics
 * actions until there is a settings screen.
 */
export function Sidebar() {
  const coreVersion = useCoreVersion();

  return (
    <aside className="sidebar" aria-label="Sessions">
      <div className="sidebar-sessions" />
      <footer className="sidebar-footer">
        <h1 className="app-name">termx</h1>
        <p className="core-version" data-status={coreVersion.status}>
          {describeCoreVersion(coreVersion)}
        </p>
        <DiagnosticsActions />
      </footer>
    </aside>
  );
}
