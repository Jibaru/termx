import { reportError } from "../lib/errorReporting";
import * as ipc from "../lib/ipc";
import { applyWorkspaceDelta, type WorkspaceDelta, type WorkspaceSnapshot } from "../lib/ipc";

/**
 * The workspace as this window knows it: the snapshot it subscribed to,
 * with every delta applied as it arrives. There is no polling; the Rust
 * side sends what changes.
 */

/** The commands of the workspace the interface uses. Tests replace them. */
export interface WorkspaceBackend {
  subscribeWorkspace: typeof ipc.subscribeWorkspace;
  refreshGit: typeof ipc.refreshGit;
  pickFolder: typeof ipc.pickFolder;
  forgetRecentFolder: typeof ipc.forgetRecentFolder;
  createSession: typeof ipc.createSession;
  createSiblingSession: typeof ipc.createSiblingSession;
  renameSession: typeof ipc.renameSession;
  reorderSession: typeof ipc.reorderSession;
  reorderProject: typeof ipc.reorderProject;
  closeSession: typeof ipc.closeSession;
  stopSession: typeof ipc.stopSession;
  splitPane: typeof ipc.splitPane;
  closePane: typeof ipc.closePane;
  movePane: typeof ipc.movePane;
  resizeSplit: typeof ipc.resizeSplit;
  focusPane: typeof ipc.focusPane;
  showSession: typeof ipc.showSession;
  quitApp: typeof ipc.quitApp;
  cancelQuit: typeof ipc.cancelQuit;
  quitShown: typeof ipc.quitShown;
  onQuitRequested: typeof ipc.onQuitRequested;
  onMenuCommand: typeof ipc.onMenuCommand;
}

export const TAURI_WORKSPACE: WorkspaceBackend = {
  subscribeWorkspace: ipc.subscribeWorkspace,
  refreshGit: ipc.refreshGit,
  pickFolder: ipc.pickFolder,
  forgetRecentFolder: ipc.forgetRecentFolder,
  createSession: ipc.createSession,
  createSiblingSession: ipc.createSiblingSession,
  renameSession: ipc.renameSession,
  reorderSession: ipc.reorderSession,
  reorderProject: ipc.reorderProject,
  closeSession: ipc.closeSession,
  stopSession: ipc.stopSession,
  splitPane: ipc.splitPane,
  closePane: ipc.closePane,
  movePane: ipc.movePane,
  resizeSplit: ipc.resizeSplit,
  focusPane: ipc.focusPane,
  showSession: ipc.showSession,
  quitApp: ipc.quitApp,
  cancelQuit: ipc.cancelQuit,
  quitShown: ipc.quitShown,
  onQuitRequested: ipc.onQuitRequested,
  onMenuCommand: ipc.onMenuCommand,
};

export type WorkspaceState =
  | { status: "loading" }
  | { status: "ready"; workspace: WorkspaceSnapshot }
  | { status: "failed"; message: string };

export class WorkspaceStore {
  private state: WorkspaceState = { status: "loading" };
  private readonly listeners = new Set<() => void>();
  private started: Promise<WorkspaceSnapshot | null> | null = null;

  constructor(readonly backend: WorkspaceBackend = TAURI_WORKSPACE) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getState = (): WorkspaceState => this.state;

  /** The workspace, once it is known. */
  get workspace(): WorkspaceSnapshot | null {
    return this.state.status === "ready" ? this.state.workspace : null;
  }

  /**
   * Subscribes to the workspace, once however often it is called, and
   * resolves with the first snapshot (`null` if subscribing failed). Deltas
   * that arrive before the snapshot are kept and applied on top of it:
   * applying one that the snapshot already holds changes nothing.
   */
  start(): Promise<WorkspaceSnapshot | null> {
    this.started ??= this.subscribeOnce();
    return this.started;
  }

  private async subscribeOnce(): Promise<WorkspaceSnapshot | null> {
    const early: WorkspaceDelta[] = [];
    let ready = false;
    try {
      let snapshot = await this.backend.subscribeWorkspace((delta) => {
        if (ready) this.apply(delta);
        else early.push(delta);
      });
      for (const delta of early) snapshot = applyWorkspaceDelta(snapshot, delta);
      ready = true;
      this.set({ status: "ready", workspace: snapshot });
      return snapshot;
    } catch (error) {
      reportError("workspace-subscribe", error);
      this.set({ status: "failed", message: ipc.describeError(error) });
      // Asking again tries again.
      this.started = null;
      return null;
    }
  }

  /** Applies a delta. Exposed for tests; the subscription calls it. */
  apply(delta: WorkspaceDelta): void {
    if (this.state.status !== "ready") return;
    this.set({ status: "ready", workspace: applyWorkspaceDelta(this.state.workspace, delta) });
  }

  private set(state: WorkspaceState): void {
    this.state = state;
    for (const listener of this.listeners) listener();
  }
}
