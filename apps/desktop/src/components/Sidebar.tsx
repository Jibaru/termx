import { type CoreVersionState, useCoreVersion } from "../lib/useCoreVersion";
import { useShortcut, useWorkbench, useWorkspaceState, withShortcut } from "../workspace/context";
import { AttentionQueue } from "./AttentionQueue";
import { DiagnosticsActions } from "./DiagnosticsActions";
import { PlusIcon, SidebarIcon } from "../icons/glyphs";
import { SessionTree } from "./SessionTree";

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

interface SidebarProps {
  onHide: () => void;
}

/**
 * Left region: the sessions, grouped by project, with the button that
 * starts a new one. The footer names the application and holds the
 * diagnostics actions until there is a settings screen.
 */
export function Sidebar({ onHide }: SidebarProps) {
  const coreVersion = useCoreVersion();
  const workbench = useWorkbench();
  const state = useWorkspaceState();
  const newShortcut = useShortcut("newSession");
  const hideShortcut = useShortcut("toggleSidebar");

  return (
    <aside className="sidebar" aria-label="Sessions" data-testid="sidebar">
      <header className="sidebar-header">
        <h2 className="sidebar-title">Sessions</h2>
        <button
          type="button"
          className="icon-button"
          data-testid="new-session-button"
          aria-label={withShortcut("New session", newShortcut)}
          title={withShortcut("New session", newShortcut)}
          onClick={workbench.openNewSessionDialog}
        >
          <PlusIcon />
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={withShortcut("Hide the sidebar", hideShortcut)}
          title={withShortcut("Hide the sidebar", hideShortcut)}
          onClick={onHide}
        >
          <SidebarIcon />
        </button>
      </header>
      {state.status === "ready" && <AttentionQueue workspace={state.workspace} />}
      <div className="sidebar-sessions">
        {state.status === "ready" && state.workspace.sessions.length > 0 && (
          <SessionTree workspace={state.workspace} />
        )}
        {state.status === "ready" && state.workspace.sessions.length === 0 && (
          <p className="sidebar-empty">No sessions yet.</p>
        )}
        {state.status === "failed" && (
          <p className="sidebar-empty" role="alert">
            The sessions could not be loaded: {state.message}
          </p>
        )}
      </div>
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
