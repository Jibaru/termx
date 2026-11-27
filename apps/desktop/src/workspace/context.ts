import { createContext, useContext, useSyncExternalStore } from "react";

import type { NotificationSettings } from "../attention/notificationPrefs";
import type { IntegrationSnapshot } from "../claude/integration";
import type { Binding, CommandId } from "../keybindings/keybindings";
import { shortcutOf } from "../keybindings/keybindings";
import type { WorkspaceSnapshot } from "../lib/ipc";
import type { Platform } from "../lib/platform";
import type { UiState, Workbench } from "./workbench";
import type { WorkspaceState } from "./workspaceStore";

/** What every component of the window reaches through React context. */
export interface WorkbenchContextValue {
  workbench: Workbench;
  platform: Platform;
  bindings: readonly Binding[];
}

export const WorkbenchContext = createContext<WorkbenchContextValue | null>(null);

export function useWorkbenchContext(): WorkbenchContextValue {
  const value = useContext(WorkbenchContext);
  if (!value) throw new Error("useWorkbenchContext needs a WorkbenchContext provider");
  return value;
}

export function useWorkbench(): Workbench {
  return useWorkbenchContext().workbench;
}

export function useWorkspaceState(): WorkspaceState {
  const { workspace } = useWorkbench();
  return useSyncExternalStore(workspace.subscribe, workspace.getState);
}

/** The workspace, once it is known. */
export function useWorkspace(): WorkspaceSnapshot | null {
  const state = useWorkspaceState();
  return state.status === "ready" ? state.workspace : null;
}

export function useUi(): UiState {
  const workbench = useWorkbench();
  return useSyncExternalStore(workbench.subscribe, workbench.getUi);
}

/** Where the Claude Code integration stands, and its install or removal. */
export function useClaudeIntegration(): IntegrationSnapshot {
  const { claude } = useWorkbench();
  return useSyncExternalStore(claude.subscribe, claude.getSnapshot);
}

/** Whether the plugin of Claude Code reports: agent states can be shown. */
export function useAgentsReported(): boolean {
  const workbench = useWorkbench();
  // Renders again when the integration changes.
  useClaudeIntegration();
  return workbench.claude.reporting;
}

export function useNotificationSettings(): NotificationSettings {
  const { notificationPrefs } = useWorkbench();
  return useSyncExternalStore(notificationPrefs.subscribe, notificationPrefs.get);
}

/** The shortcut of a command as the user types it, or `null`. */
export function useShortcut(command: CommandId): string | null {
  const { bindings, platform } = useWorkbenchContext();
  return shortcutOf(bindings, command, platform);
}

/** `label`, followed by its shortcut in parentheses when it has one. */
export function withShortcut(label: string, shortcut: string | null): string {
  return shortcut ? `${label} (${shortcut})` : label;
}
