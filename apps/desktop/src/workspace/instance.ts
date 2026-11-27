import { TAURI_CLAUDE } from "../claude/backend";
import { sessionStore } from "../terminal/sessionStore";
import { Workbench } from "./workbench";
import { WorkspaceStore } from "./workspaceStore";

/**
 * The workbench of this window, on the real workspace and the terminals
 * of `sessionStore` (which the end-to-end suite reaches through its hook).
 */
export const defaultWorkbench = new Workbench(new WorkspaceStore(), sessionStore, {
  claude: TAURI_CLAUDE,
});
