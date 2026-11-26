import {
  closeSession,
  createSession,
  getWorkspaceSnapshot,
  type LaunchMode,
  type LayoutNode,
  type NewSession,
  type Session,
  type SessionId,
  type WorkspaceSnapshot,
} from "../lib/ipc";
import { TerminalController } from "./TerminalController";

/**
 * The terminals of the window, keyed by the id of the session each one
 * shows.
 *
 * A session belongs to the workspace kept by the Rust side, which
 * remembers it between runs. This store only holds what draws it: one
 * controller per session that was opened here, with its xterm.js instance
 * and its scrollback, which stay when the process of the session is
 * restarted.
 *
 * Only the active terminal is on screen. The others keep running: their
 * controllers keep feeding their xterm.js instance.
 *
 * This is the store of the single-terminal window. It shows one session:
 * the one termx had on screen when it last ran, or a new shell.
 */

/** What the interface renders from. A new object after every change. */
export interface SessionsSnapshot {
  /** The sessions that have a terminal, in the order they were opened. */
  readonly keys: readonly SessionId[];
  /** The session on screen, if there is one. */
  readonly activeKey: SessionId | null;
}

/** What the store needs from the workspace. Tests replace it. */
export interface WorkspaceBackend {
  getWorkspaceSnapshot: () => Promise<WorkspaceSnapshot>;
  createSession: (session: NewSession) => Promise<Session>;
  closeSession: (id: SessionId) => Promise<void>;
}

const TAURI_WORKSPACE: WorkspaceBackend = { getWorkspaceSnapshot, createSession, closeSession };

/** The session shown by the pane `pane` of a layout, if it shows one. */
function sessionOfPane(node: LayoutNode, pane: number): SessionId | null {
  if (node.type === "pane") return node.id === pane ? node.session : null;
  return sessionOfPane(node.first, pane) ?? sessionOfPane(node.second, pane);
}

/**
 * The session to put on screen when termx starts: the one in the focused
 * pane, else the first one. `undefined` in a workspace without sessions.
 */
export function sessionToRestore(workspace: WorkspaceSnapshot): Session | undefined {
  const focused = sessionOfPane(workspace.layout.root, workspace.layout.focused);
  return workspace.sessions.find((session) => session.id === focused) ?? workspace.sessions[0];
}

export class SessionStore {
  private readonly controllers = new Map<SessionId, TerminalController>();
  private snapshot: SessionsSnapshot = { keys: [], activeKey: null };
  private readonly listeners = new Set<() => void>();
  /** The first terminal being opened, so that it is opened once. */
  private first: Promise<void> | null = null;

  /**
   * `createController` builds the terminal of a session, and `backend` is
   * the workspace. Tests replace both.
   */
  constructor(
    private readonly createController: (
      sessionId: SessionId,
      autoStart: LaunchMode,
    ) => TerminalController = (sessionId, autoStart) =>
      new TerminalController({ sessionId, autoStart }),
    private readonly backend: WorkspaceBackend = TAURI_WORKSPACE,
  ) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): SessionsSnapshot => this.snapshot;

  /** The terminal of this session. */
  get(key: SessionId): TerminalController | undefined {
    return this.controllers.get(key);
  }

  /**
   * Opens a terminal for a session and puts it on screen. Its process is
   * started when the terminal is first shown: from scratch, or resuming
   * for a session restored from the previous run. A session that has a
   * terminal already is only put on screen.
   */
  open(sessionId: SessionId, autoStart: LaunchMode = "fresh"): SessionId {
    if (this.controllers.has(sessionId)) {
      this.activate(sessionId);
      return sessionId;
    }
    this.controllers.set(sessionId, this.createController(sessionId, autoStart));
    this.update({ keys: [...this.snapshot.keys, sessionId], activeKey: sessionId });
    return sessionId;
  }

  /**
   * Opens the first terminal, unless there is one already: for the session
   * termx last had on screen, which is resumed, or else for a new shell in
   * the home directory. Asking again while that is in progress changes
   * nothing. A failure was logged where it happened; asking again tries
   * again.
   */
  ensureOne(): Promise<void> {
    this.first ??= this.restoreOrCreate().catch(() => {
      this.first = null;
    });
    return this.first;
  }

  private async restoreOrCreate(): Promise<void> {
    if (this.controllers.size > 0) return;
    const workspace = await this.backend.getWorkspaceSnapshot();
    // Something else opened a terminal while the workspace was being read.
    if (this.controllers.size > 0) return;
    const session =
      sessionToRestore(workspace) ?? (await this.backend.createSession({ kind: "shell" }));
    if (this.controllers.size > 0) return;
    // Nothing runs after a restart of termx: a session that was restored
    // carries on where it was, in its folder.
    this.open(session.id, session.state.status === "dormant" ? "resume" : "fresh");
  }

  /** Puts another terminal on screen. */
  activate(key: SessionId): void {
    if (!this.controllers.has(key) || key === this.snapshot.activeKey) return;
    this.update({ ...this.snapshot, activeKey: key });
  }

  /**
   * Closes a session for good, and its terminal with it. A neighbour
   * becomes active.
   */
  close(key: SessionId): void {
    const controller = this.controllers.get(key);
    if (!controller) return;
    this.controllers.delete(key);
    controller.dispose();
    // A failure is logged where it happens; the terminal is gone either way.
    void this.backend.closeSession(key).catch(() => undefined);

    const index = this.snapshot.keys.indexOf(key);
    const keys = this.snapshot.keys.filter((other) => other !== key);
    const activeKey =
      this.snapshot.activeKey === key
        ? (keys[Math.min(index, keys.length - 1)] ?? null)
        : this.snapshot.activeKey;
    this.update({ keys, activeKey });
  }

  private update(snapshot: SessionsSnapshot): void {
    this.snapshot = snapshot;
    for (const listener of this.listeners) listener();
  }
}

/** The terminals of this window. */
export const sessionStore = new SessionStore();
