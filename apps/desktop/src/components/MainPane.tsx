import { type CoreVersionState, useCoreVersion } from "../lib/useCoreVersion";

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

/** Main region. It will host the terminal view; for now it is a placeholder. */
export function MainPane() {
  const coreVersion = useCoreVersion();

  return (
    <main className="main-pane">
      <h1 className="app-name">termx</h1>
      <p className="core-version" data-status={coreVersion.status}>
        {describeCoreVersion(coreVersion)}
      </p>
    </main>
  );
}
