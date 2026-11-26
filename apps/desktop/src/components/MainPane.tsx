import type { TerminalController } from "../terminal/TerminalController";
import { TerminalView } from "./TerminalView";

interface MainPaneProps {
  /** The terminal on screen, once there is one. */
  controller: TerminalController | undefined;
}

/** Main region: the active terminal. */
export function MainPane({ controller }: MainPaneProps) {
  return (
    <main className="main-pane" aria-label="Terminal">
      {controller && <TerminalView controller={controller} />}
    </main>
  );
}
