import type { LayoutNode, Session, SessionId, WorkspaceSnapshot } from "../lib/ipc";
import { TerminalController, type AutoStart } from "./TerminalController";

/**
 * The terminals of the window, keyed by the id of the session each one
 * shows.
 *
 * A session belongs to the workspace kept by the Rust side, which
 * remembers it between runs. This store only holds what draws it: one
 * controller per session that was shown (or started) in this window, with
 * its xterm.js instance and its scrollback, which stay when the process of
 * the session is restarted and when it leaves the screen.
 *
 * Only the terminals of the panes are on screen. The others keep running:
 * their controllers keep feeding and acknowledging their xterm.js
 * instance, without drawing it and without a WebGL context.
 */

/** What the interface renders from. A new object after every change. */
export interface SessionsSnapshot {
  /** The sessions that have a terminal, in the order they were opened. */
  readonly keys: readonly SessionId[];
  /** The session of the focused pane, if it shows one. */
  readonly activeKey: SessionId | null;
}

/** Makes the terminal of a session. Tests replace it. */
export type ControllerFactory = (
  sessionId: SessionId,
  autoStart: AutoStart,
  size: { cols: number; rows: number } | undefined,
) => TerminalController;

const DEFAULT_FACTORY: ControllerFactory = (sessionId, autoStart, size) =>
  new TerminalController({ sessionId, autoStart, size });

/** The session shown by the pane `pane` of a layout, if it shows one. */
function sessionOfPane(node: LayoutNode, pane: number): SessionId | null {
  if (node.type === "pane") return node.id === pane ? node.session : null;
  return sessionOfPane(node.first, pane) ?? sessionOfPane(node.second, pane);
}

/**
 * The session to put on screen when termx starts with nothing shown: the
 * one of the focused pane, else the first one. `undefined` in a workspace
 * without sessions.
 */
export function sessionToRestore(workspace: WorkspaceSnapshot): Session | undefined {
  const focused = sessionOfPane(workspace.layout.root, workspace.layout.focused);
  return workspace.sessions.find((session) => session.id === focused) ?? workspace.sessions[0];
}

/**
 * How the terminal of `session` starts when it is first shown. A session
 * that was just created starts at once; one restored from the last run
 * (dormant), one that ended, or one that runs without this window having a
 * terminal for it (the page was reloaded) waits for the user: nothing is
 * started, or restarted, behind their back.
 */
export function autoStartFor(session: Session): AutoStart {
  return session.state.status === "starting" ? "fresh" : "dormant";
}

export class SessionStore {
  private readonly controllers = new Map<SessionId, TerminalController>();
  private snapshot: SessionsSnapshot = { keys: [], activeKey: null };
  private readonly listeners = new Set<() => void>();

  constructor(private readonly createController: ControllerFactory = DEFAULT_FACTORY) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): SessionsSnapshot => this.snapshot;

  /** The terminal of this session, if it has one. */
  get(key: SessionId): TerminalController | undefined {
    return this.controllers.get(key);
  }

  /** The terminal of `session`, made the first time it is asked for. */
  controllerFor(session: Session): TerminalController {
    const existing = this.controllers.get(session.id);
    if (existing) return existing;
    const controller = this.createController(session.id, autoStartFor(session), this.sizeHint());
    this.controllers.set(session.id, controller);
    this.update({ ...this.snapshot, keys: [...this.snapshot.keys, session.id] });
    return controller;
  }

  /**
   * Starts a session that is not running, resuming it: Claude Code carries
   * on with its conversation, a shell or a command starts again in its
   * folder. Works whether or not the session is on screen.
   */
  resume(session: Session): void {
    const controller = this.controllerFor(session);
    const status = controller.getStatus().kind;
    if (status === "starting" || status === "running") return;
    controller.startDetached("resume");
  }

  /** Starts a session again from scratch. */
  restart(session: Session): void {
    this.controllerFor(session).startDetached("fresh");
  }

  /** Records which session the focused pane shows. */
  setActive(key: SessionId | null): void {
    if (key === this.snapshot.activeKey) return;
    this.update({ ...this.snapshot, activeKey: key });
  }

  /**
   * Lets go of the terminals of sessions that left the workspace. Closing a
   * session is the workspace's business; this only frees what drew it.
   */
  retain(sessions: ReadonlySet<SessionId>): void {
    const gone = this.snapshot.keys.filter((key) => !sessions.has(key));
    if (gone.length === 0) return;
    for (const key of gone) {
      this.controllers.get(key)?.dispose();
      this.controllers.delete(key);
    }
    const keys = this.snapshot.keys.filter((key) => sessions.has(key));
    const activeKey =
      this.snapshot.activeKey !== null && sessions.has(this.snapshot.activeKey)
        ? this.snapshot.activeKey
        : null;
    this.update({ keys, activeKey });
  }

  /**
   * The size a terminal that starts off the screen is given: that of the
   * terminal of the focused pane, which is the likeliest place it will be
   * shown.
   */
  private sizeHint(): { cols: number; rows: number } | undefined {
    const active = this.snapshot.activeKey;
    const terminal = active === null ? undefined : this.controllers.get(active)?.terminal;
    return terminal ? { cols: terminal.cols, rows: terminal.rows } : undefined;
  }

  private update(snapshot: SessionsSnapshot): void {
    this.snapshot = snapshot;
    for (const listener of this.listeners) listener();
  }
}

/** The terminals of this window. */
export const sessionStore = new SessionStore();
